//! Where a line came from, as the engine answers it.

use super::edit::{EditSource, UnobservedReason};
use crate::contracts::LineUnknownReason;

/// The origin of one line of a file.
///
/// There is deliberately no "manual" or "human" variant: a line no captured
/// edit explains is `Unknown`, because WHY cannot tell a person from a shell
/// command, a formatter or a lost hook delivery (see `contracts/attribution.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineOrigin {
    /// The line is unchanged since the base revision.
    Base,
    /// This captured edit wrote the line.
    Edit(EditSource),
    /// WHY cannot prove where the line came from.
    Unknown(UnknownReason),
}

/// Why a line's origin is unknown. Each one is an honest, explainable answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnknownReason {
    /// No captured edit explains the line (a shell write, a human, a hook that
    /// was not installed yet, a lost delivery).
    NoCapture,
    /// An edit touched the file but its payload was reduced by the hook (over
    /// the wire cap, or redacted).
    PayloadReduced,
    /// An edit was captured but its payload could not be read.
    Unparsed,
    /// An edit's `before` did not match the replayed state, so what came
    /// before it cannot be trusted line by line.
    ReplayMismatch,
    /// A position-less hunk (Codex) matched its context in several places.
    AmbiguousMatch,
    /// Reserved for the WHY-24 spike: another writer changed the file while
    /// an agent was editing it.
    ConcurrentWriter,
    /// The replay engine has not run (the WHY-64 stub answers this).
    NotReplayed,
}

/// The wire form. Two enums on purpose: the engine's vocabulary can grow
/// internal detail without changing the contract, and this exhaustive match
/// (no `_ =>` arm) makes every new internal reason pick its wire answer.
impl From<UnknownReason> for LineUnknownReason {
    fn from(reason: UnknownReason) -> Self {
        match reason {
            UnknownReason::NoCapture => LineUnknownReason::NoCapture,
            UnknownReason::PayloadReduced => LineUnknownReason::PayloadReduced,
            UnknownReason::Unparsed => LineUnknownReason::Unparsed,
            UnknownReason::ReplayMismatch => LineUnknownReason::ReplayMismatch,
            UnknownReason::AmbiguousMatch => LineUnknownReason::AmbiguousMatch,
            UnknownReason::ConcurrentWriter => LineUnknownReason::ConcurrentWriter,
            UnknownReason::NotReplayed => LineUnknownReason::NotReplayed,
        }
    }
}

impl From<UnobservedReason> for UnknownReason {
    fn from(reason: UnobservedReason) -> Self {
        // Exhaustive on purpose: a new unobserved reason must pick its answer.
        match reason {
            UnobservedReason::PayloadReduced => UnknownReason::PayloadReduced,
            UnobservedReason::Unparsed => UnknownReason::Unparsed,
        }
    }
}
