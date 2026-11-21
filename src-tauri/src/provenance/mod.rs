//! Line provenance (v0.3): which captured agent edit wrote each line of a
//! file, or `Unknown` when that cannot be proven.
//!
//! Pure code: no IO, no tokio, no git, no SQLite. Every input arrives as a
//! value (the base revision's lines from git, the edits from the
//! [`EventStore`](crate::events::EventStore)), which keeps the engine
//! deterministic and testable against hand-written ground truth.
//!
//! ```text
//! adapters ──EditOp──▶ ingest ──▶ EventStore::append  (events.db v3: edit_ops)
//! EventStore::edit_records(wt, path) ──▶ ReplayInput ──replay()──▶ ReplayOutput
//!                                        (WHY-22)        (WHY-23 aligns it with
//!                                                         the real file)
//! ```
//!
//! # Decisions (`why-brain/06 - Decisions.md`)
//!
//! - **29**: replay inputs are per-line HASHES and counts, never line text
//!   ([`line`] is the one hashing policy, [`edit`] the stored shape).
//! - **30**: Codex hunks have no positions; ambiguous context is `Unknown`,
//!   and Codex coverage is reported on its own.
//! - **31**: shell writes are a spike (WHY-24), not part of this seam.
//! - **32**: an edit belongs to the edited file's own worktree
//!   ([`edit::EditOp::worktree_id`]).
//! - **33**: the engine and its ground truth have different authors. The
//!   scenario format and its loader live in the integration tests
//!   (`src-tauri/tests/provenance/`), out of reach of production code and of
//!   the engine.
//!
//! # Privacy: what the hashes do and do not promise
//!
//! No line text is stored. The hashes are NOT secret: they are unsalted, so a
//! short, low-entropy line (a one-line `.env`) can be guessed by hashing
//! candidates. All hashing goes through [`line::LineHasher`] so a per-install
//! key can be added later without touching call sites; no key is used yet.

pub mod anchor;
pub mod edit;
pub mod line;
pub mod origin;
pub mod replay;

pub use anchor::Anchor;
pub use edit::{
    EditHunk, EditKind, EditOp, EditRecord, EditSource, FileSnapshot, HunkLineHash, HunkLineKind,
    Observed, UnobservedReason,
};
pub use line::{
    FileFingerprint, LineHash, LineHasher, LinesDigest, fingerprint, has_final_newline, hash_lines,
    split_lines,
};
pub use origin::{LineOrigin, UnknownReason};
pub use replay::{ReplayInput, ReplayOutput, ReplayedLine, replay};
