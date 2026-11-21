//! What one captured edit looks like to the replay engine: hashes and
//! positions, never line text (decision 29).
//!
//! Internal types, not IPC contracts (`u64` is fine here). Adapters (WHY-65)
//! build [`EditOp`]s from the harness payloads (Claude `Write`/`Edit`
//! `structuredPatch` + `originalFile`, Codex `apply_patch`); the ingest step
//! fills [`EditOp::worktree_id`] and the repo-relative [`EditOp::path`]; the
//! [`EventStore`](crate::events::EventStore) keeps them and hands them back as
//! [`EditRecord`]s.
//!
//! # Ordering and verification
//!
//! The replay applies a file's records in [`EditRecord::seq`] order (arrival
//! order of the events, then [`EditRecord::op_index`] inside one event) and
//! VERIFIES each edit's [`EditOp::before`] against the state it replayed so
//! far. Arrival order is not a promise that the edits happened in that order
//! (two sessions can write the same file concurrently, each hook is its own
//! process), so a mis-ordered or concurrent edit must surface as a mismatch,
//! which becomes `Unknown`, never as a confident wrong attribution.
//!
//! Ops come only from SUCCESSFUL edits (`PostToolUse`, outcome ok): a failed or
//! rejected edit changed nothing and carries no op. So the `TurnRef` the UI
//! eventually gets for an edited line has outcome `ok`; its `turn_index` comes
//! from the session's timeline (`events::timeline::turn_indices`).

use crate::contracts::WorktreeId;
use crate::events::{EventKey, SessionKey};

use super::line::{FileFingerprint, LineHash, LineHasher, has_final_newline};

/// What the edit did to the file as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// The file did not exist before (Claude `Write` with `type: "create"`,
    /// Codex `*** Add File`). Its content arrives as ONE hunk of only
    /// [`HunkLineKind::Added`] lines: Claude's create has an empty
    /// `structuredPatch`, so the adapter makes that hunk from `content`.
    Create,
    /// The file existed and was changed (or overwritten whole).
    Update,
    /// The file was removed. No hunks.
    Delete,
}

/// The file as the harness saw it BEFORE the edit (Claude `originalFile`).
///
/// The whole line-hash sequence, not only a digest, on purpose. A digest can
/// tell the replay "your state is wrong" but not how to fix it. After a
/// `cargo fmt`, a `sed` or a human touch between two Claude edits, the next
/// edit's `before.lines` is the only thing the replay can RESYNC from: it says
/// exactly which lines the file had, so lines that still match keep their
/// origin and the ones that changed behind WHY's back become `Unknown`.
/// It is still hashes only, so it stays inside decision 29.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSnapshot {
    pub fingerprint: FileFingerprint,
    pub lines: Vec<LineHash>,
}

impl FileSnapshot {
    /// The snapshot of `text` (the full old file, as bytes).
    pub fn of_text(hasher: &LineHasher, text: &[u8]) -> Self {
        Self::of_lines(hasher, hasher.hash_lines(text), has_final_newline(text))
    }

    /// The snapshot of a file given as its line hashes.
    pub fn of_lines(hasher: &LineHasher, lines: Vec<LineHash>, final_newline: bool) -> Self {
        Self {
            fingerprint: hasher.fingerprint_of(&lines, final_newline),
            lines,
        }
    }

    /// The fingerprint really describes `lines` (a decoded row could be
    /// damaged; the engine should not trust one that fails this).
    pub fn is_consistent(&self, hasher: &LineHasher) -> bool {
        hasher.fingerprint_of(&self.lines, self.fingerprint.final_newline) == self.fingerprint
    }
}

/// One line of a hunk. Named `HunkLineHash` so it never clashes with the
/// Codex parser's text-carrying `adapters::codex::patch::HunkLine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HunkLineHash {
    pub kind: HunkLineKind,
    pub hash: LineHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkLineKind {
    /// Unchanged line shown for context (` ` in a unified diff).
    Context,
    /// Line taken out (`-`).
    Removed,
    /// Line put in (`+`).
    Added,
}

/// One hunk of an edit, lines in the ORDER THE PATCH HAS THEM: a Claude
/// `structuredPatch` hunk interleaves context, `-` and `+` lines, and
/// flattening it (all removed, then all added) would lose where each added
/// line went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditHunk {
    /// The harness's own `oldStart` (Claude gives it, Codex does not), with
    /// unified-diff meaning: the 1-based line number, in the file BEFORE this
    /// edit, of the hunk's first context or removed line; for a hunk with
    /// neither (a pure insertion) the line AFTER which the added lines go
    /// (`0` = at the top). This pure-insertion meaning is WHY's definition:
    /// every Claude hunk observed so far has context lines, so what Claude
    /// sends when `oldLines` is 0 is not observed; the adapter converts to this
    /// meaning if it turns out to differ. `None` for a created file's hunk.
    pub old_start: Option<u32>,
    /// Codex's `@@ <text>` context header (usually the enclosing function),
    /// hashed like a line. Not a line of the hunk: a locating hint (the hunk
    /// starts after a line with this content) that cuts down ambiguous
    /// matches.
    pub header: Option<LineHash>,
    pub lines: Vec<HunkLineHash>,
}

/// Whether WHY saw the edit's content exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// Hunks (and `before`/`after` when present) are the harness's own data.
    Exact,
    /// WHY knows an edit happened but not what it did. Such an op carries NO
    /// hunks and no `before`/`after`, and is never guessed: lines it may have
    /// touched become `Unknown`.
    Unobserved(UnobservedReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnobservedReason {
    /// The hook dropped the payload: it was over the 1 MiB wire cap, or it was
    /// redacted (see `why_wire::reduced`).
    PayloadReduced,
    /// The payload arrived but the adapter could not read the edit out of it
    /// (an unknown shape, a malformed patch).
    Unparsed,
}

/// One edit of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditOp {
    /// The worktree that owns the edited file (decision 32: not necessarily
    /// the session's cwd). Filled by ingest (WHY-65); adapters leave it
    /// `None`. An op still `None` at `append` cannot be looked up and is not
    /// stored (see [`EventStore::append`](crate::events::EventStore::append)).
    pub worktree_id: Option<WorktreeId>,
    /// Worktree-relative, in `canonical_path` form, once ingest has run.
    /// Before that, the harness's own (usually absolute) path.
    pub path: String,
    /// For an `Unobserved` op the payload may not say whether the file was
    /// created or updated; the adapter then sets `Update` (or `Delete` when
    /// the tool can only delete), and the engine must not rely on `kind` for
    /// an unobserved op.
    pub kind: EditKind,
    /// The path the file had before a rename (Codex `*** Move to:`). The store
    /// keeps it but does not follow rename chains: the engine does.
    pub renamed_from: Option<String>,
    /// The file before the edit, when the payload carries it (Claude
    /// `originalFile`). `None` for Codex and for a created file.
    pub before: Option<FileSnapshot>,
    /// The file after the edit, when the payload makes it known (Claude
    /// `Write` carries the whole `content`). A fingerprint is enough: the
    /// lines themselves follow from `before` + hunks.
    pub after: Option<FileFingerprint>,
    pub hunks: Vec<EditHunk>,
    pub observed: Observed,
}

/// Which captured tool call produced an op.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EditSource {
    pub session: SessionKey,
    /// Claude `prompt_id`, Codex `turn_id`.
    pub turn_id: Option<String>,
    /// Set when a subagent made the edit.
    pub agent_id: Option<String>,
    pub tool_use_id: Option<String>,
    /// The stored event the op came with (its dedupe key).
    pub event_key: EventKey,
}

/// An [`EditOp`] as the store hands it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRecord {
    pub source: EditSource,
    /// Arrival order of the event across the whole store (SQLite `events.seq`).
    /// Increasing, not contiguous.
    pub seq: u64,
    /// Position of the op among its event's ops (one `apply_patch` can edit
    /// several files, or one file twice).
    pub op_index: u32,
    /// When the hook saw the event (Unix milliseconds). For display only:
    /// order is `seq`, never time.
    pub at_ms: u64,
    pub op: EditOp,
}
