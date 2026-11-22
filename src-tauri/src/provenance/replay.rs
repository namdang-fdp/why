//! The replay engine (WHY-22): rebuild, from a file's base revision and its
//! captured edits, which edit wrote each line, or `Unknown` with a reason.
//!
//! The semantics are the ground truth's (`tests/provenance/scenarios/
//! README.md`, "Semantics (v0.3)"); the engine never reads that oracle's code
//! (decision 33). In short:
//!
//! - Base lines start as `Base`. Lines a hunk adds become `Edit(that op)`;
//!   context lines and lines outside every hunk keep what they had; removed
//!   lines are gone. Content equality never transfers an origin.
//! - A Claude op's `before` (the full old file) is checked against the
//!   replayed state. If they differ, something uncaptured changed the file,
//!   and the state is RESYNCED to `before` with the rule-5 matcher
//!   ([`matcher`]): provably unchanged lines keep their origin, the rest
//!   become `NoCapture`.
//! - Claude hunks are placed at their own `old_start` and must match there.
//!   Codex hunks (no positions) are located by context: one placement is
//!   applied, several make the placement-dependent lines `AmbiguousMatch`.
//! - After an unobserved op, or an op the engine cannot apply faithfully, the
//!   state is UNKNOWN until a later op's `before` reveals it; the lines that
//!   changed in between get that op's reason.
//!
//! Pure: no IO, no clock, no randomness. The same input gives the same output.

mod engine;
mod matcher;
#[cfg(test)]
mod tests;

use super::edit::{EditRecord, EditSource, FileSnapshot};
use super::line::LineHash;
use super::origin::{LineOrigin, UnknownReason};

/// Everything the engine knows about one file in one worktree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReplayInput {
    /// The file at the base revision, as a full snapshot (line hashes plus the
    /// fingerprint, which keeps `final_newline`, so the first edit's `before`
    /// can be verified exactly). `None`: the file did not exist there, which
    /// is not the same as an existing empty file.
    pub baseline: Option<FileSnapshot>,
    /// The file's captured edits, in `(seq, op_index)` order, as
    /// `EventStore::edit_records` returns them. (The engine sorts them by
    /// that key again, stably, so a caller's slip cannot reorder history.)
    /// Renames are the caller's job: the records of the file's earlier paths
    /// must already be included; the engine treats a rename as an update.
    pub records: Vec<EditRecord>,
}

/// One line of the replayed file: its content hash and where it came from.
/// The hash is what alignment with the real file (WHY-23) matches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayedLine {
    pub hash: LineHash,
    pub origin: LineOrigin,
}

/// The REPLAYED file (the state after the last record), line by line, in
/// order. Alignment with the real file on disk is WHY-23's job.
///
/// `final_newline` has three meanings, told apart by `lines`:
///
/// - `None` with NO lines: the file is absent (never created, or deleted);
/// - `None` WITH lines: the file exists but whether it ends with a newline is
///   unknown: the last op with no `after` fingerprint created the file
///   (Codex), or had a hunk that includes the last line or appends after it
///   (hunks carry no newline marker). An op strictly inside the file keeps
///   the flag; WHY never guesses a harness's newline habits;
/// - `Some(_)`: known. An existing empty file is `Some(false)`.
///
/// When [`ReplayReport::unresolved`] is set, this is the LAST KNOWN state
/// instead (see there); [`replay`] alone cannot say which, so WHY-23 should
/// call [`replay_report`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReplayOutput {
    pub lines: Vec<ReplayedLine>,
    pub final_newline: Option<bool>,
}

/// The full answer: [`ReplayOutput`] plus what it cannot express on its own.
///
/// Added by WHY-22 next to the WHY-64 seam (whose types other code already
/// builds literally, so they keep their fields) so that WHY-23 can align
/// honestly: a hash-only output would make WHY-23 call `noCapture` what is
/// really `payloadReduced` or `ambiguousMatch`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReplayReport {
    /// The replayed file, or the last known state when `unresolved` is set.
    pub output: ReplayOutput,
    /// Lines of `output` whose TEXT is not known exactly, because an
    /// ambiguous Codex placement may or may not have rewritten them. Sorted by
    /// `index`. Such a line's origin is usually `AmbiguousMatch`; its `hash`
    /// is one possible text (the one it had before that op when that is still
    /// possible), `others` the rest. The lines of one ambiguous hunk are
    /// LINKED (one placement is true for all of them): each line holding one
    /// of its texts is necessary, not sufficient, for the real file to be
    /// consistent with the replay. (`[x, x]` after `-x +y` lists `y` for both
    /// lines, yet `[y, y]` is not possible.)
    pub alternatives: Vec<LineAlternatives>,
    /// `Some`: the replay could not follow the file to the end. From the op
    /// named in `since` on, WHY does not know the file's lines: an unobserved
    /// op (`PayloadReduced` / `Unparsed`), an op whose hunks or `after`
    /// contradict the state (`ReplayMismatch`), or an ambiguous placement
    /// the engine does not merge line by line (`AmbiguousMatch`: it changes
    /// the line count, has more than 64 places, lies over lines an earlier
    /// ambiguous hunk left uncertain, or its places interleave with another
    /// hunk's). No later op's `before`
    /// revealed the file, so `output` is the state just BEFORE that op
    /// (absent if `output` has no lines and no `final_newline`), and every
    /// line of the real file that does not provably match it (rule 5) should
    /// get `reason`, not `noCapture`. Later unobserved ops never replace an
    /// earlier `since`: the earliest reason wins.
    pub unresolved: Option<Unresolved>,
}

/// See [`ReplayReport::alternatives`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineAlternatives {
    /// 0-based index into [`ReplayOutput::lines`].
    pub index: usize,
    /// The other possible hashes of that line (never its own `hash`).
    pub others: Vec<LineHash>,
}

/// See [`ReplayReport::unresolved`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    /// The op from which on the file's state is unknown.
    pub since: EditSource,
    pub reason: UnknownReason,
}

/// Rebuild each line's origin from the recorded edits. The output part of
/// [`replay_report`]; see there for the cases it cannot express.
pub fn replay(input: &ReplayInput) -> ReplayOutput {
    replay_report(input).output
}

/// Rebuild each line's origin from the recorded edits, with everything WHY-23
/// needs to align the result with the real file.
pub fn replay_report(input: &ReplayInput) -> ReplayReport {
    engine::run(input)
}
