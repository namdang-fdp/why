//! The replay seam: what the engine (WHY-22) receives and returns. The body of
//! [`replay`] is a stub today; WHY-22 replaces the body, not the signature.

use super::edit::{EditRecord, FileSnapshot};
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
    /// `EventStore::edit_records` returns them.
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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReplayOutput {
    pub lines: Vec<ReplayedLine>,
    /// `None`: the replayed file does not exist (never created, or deleted).
    pub final_newline: Option<bool>,
}

/// Rebuild each line's origin from the recorded edits.
///
/// STUB (WHY-64): applies nothing and answers the baseline's lines with origin
/// `Unknown(NotReplayed)`. Honest rather than a fake success: no line is ever
/// attributed by code that did not do the work.
pub fn replay(input: &ReplayInput) -> ReplayOutput {
    let Some(base) = &input.baseline else {
        return ReplayOutput::default();
    };
    ReplayOutput {
        lines: base
            .lines
            .iter()
            .map(|&hash| ReplayedLine {
                hash,
                origin: LineOrigin::Unknown(UnknownReason::NotReplayed),
            })
            .collect(),
        final_newline: Some(base.fingerprint.final_newline),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::line::LineHasher;

    #[test]
    fn the_stub_attributes_nothing() {
        let h = LineHasher::default();
        let base = FileSnapshot::of_text(&h, b"a\nb\nc");
        let out = replay(&ReplayInput {
            baseline: Some(base.clone()),
            records: Vec::new(),
        });
        assert_eq!(out.final_newline, Some(false));
        assert_eq!(out.lines.len(), 3);
        for (line, hash) in out.lines.iter().zip(&base.lines) {
            assert_eq!(line.hash, *hash);
            assert_eq!(line.origin, LineOrigin::Unknown(UnknownReason::NotReplayed));
        }
        let absent = replay(&ReplayInput::default());
        assert!(absent.lines.is_empty());
        assert_eq!(absent.final_newline, None, "absent, not empty");
    }
}
