//! A stable pointer to a range of lines (WHY-22 defined it, WHY-23 gives it
//! behaviour).
//!
//! Path-relative and content-keyed, not keyed by an absolute path or a
//! worktree id (decision 4, portability): the same commit checked out in
//! another worktree, on another machine or after a move must resolve the same
//! anchor. The content hash lets a consumer check that the lines are still the
//! ones the anchor was made for, and find them again after they moved.
//!
//! # Re-anchoring, and why it never guesses
//!
//! [`Anchor::resolve`] answers against the CURRENT lines of the file:
//!
//! The content is searched for everywhere. Exactly one place holds it: that
//! place is [`Resolution::Exact`] when it is the recorded range, else
//! [`Resolution::Moved`]. Several places hold it, or none: [`Resolution::Lost`].
//! Two identical blocks mean WHY cannot tell which one the anchor was made for,
//! and a comment attached to the wrong block is worse than a comment that says
//! "I lost track".
//!
//! Uniqueness is checked even when the recorded range still holds the content:
//! if an identical block exists elsewhere too (the original moved and a copy
//! took its place, or the block was duplicated), a comment made for one of them
//! cannot be told from the other, so the answer is `Lost`.
//!
//! Pure: no IO. Anchors are not shown in the UI yet; v0.5 comments use them.

use std::ops::RangeInclusive;

use super::line::{LineHash, LineHasher, LinesDigest};

/// Searching costs `windows * range length` hash steps (the digest is not a
/// rolling hash). Above this many steps the search is skipped and the anchor is
/// `Lost`: an honest "cannot tell" beats a stalled app on a hostile file.
const MAX_SEARCH_STEPS: usize = 50_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Anchor {
    /// The commit (full object id) the line numbers refer to.
    pub revision: String,
    /// Repository-relative path, `/`-separated.
    pub file: String,
    /// 1-based, inclusive.
    pub start_line: u32,
    /// 1-based, inclusive (`end_line >= start_line`).
    pub end_line: u32,
    /// [`LineHasher::digest`](super::line::LineHasher::digest) of the line
    /// hashes of `start_line..=end_line`.
    pub content_hash: LinesDigest,
}

/// Where an [`Anchor`]'s lines are now. Ranges are 1-based and inclusive, like
/// the anchor's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The recorded line numbers still hold the recorded content.
    Exact(RangeInclusive<u32>),
    /// The content is somewhere else, and nowhere but there.
    Moved(RangeInclusive<u32>),
    /// The content is gone, changed, or present more than once elsewhere.
    Lost,
}

impl Anchor {
    /// The anchor of `range` (1-based, inclusive) of a file whose lines are
    /// `lines`. `None` when the range is empty, starts before line 1 or runs
    /// past the end: an anchor must point at real lines.
    pub fn for_range(
        revision: impl Into<String>,
        file: impl Into<String>,
        lines: &[LineHash],
        range: RangeInclusive<u32>,
    ) -> Option<Self> {
        let (start, end) = (*range.start(), *range.end());
        let window = window(lines, start, end)?;
        Some(Self {
            revision: revision.into(),
            file: file.into(),
            start_line: start,
            end_line: end,
            content_hash: LineHasher::default().digest(window),
        })
    }

    /// How many lines the anchor covers.
    fn len(&self) -> usize {
        // `end_line >= start_line` is a documented invariant, but the struct
        // is plain: a bad value must not underflow.
        (self.end_line.saturating_sub(self.start_line) as usize).saturating_add(1)
    }

    /// Find the anchor's lines in `current` (see the module docs).
    pub fn resolve(&self, current: &[LineHash]) -> Resolution {
        let hasher = LineHasher::default();
        if self.start_line == 0 || self.end_line < self.start_line {
            return Resolution::Lost; // a malformed anchor points nowhere
        }
        let len = self.len();
        if len > current.len() {
            return Resolution::Lost;
        }
        let windows = current.len() - len + 1;
        if windows.saturating_mul(len) > MAX_SEARCH_STEPS {
            return Resolution::Lost;
        }
        let mut found: Option<usize> = None;
        for at in 0..windows {
            if hasher.digest(&current[at..at + len]) == self.content_hash {
                if found.is_some() {
                    // A second place: the anchor could be either. Never guess.
                    return Resolution::Lost;
                }
                found = Some(at);
            }
        }
        match found {
            Some(at) => {
                let (start, end) = (at as u32 + 1, (at + len) as u32);
                if (start, end) == (self.start_line, self.end_line) {
                    Resolution::Exact(start..=end)
                } else {
                    Resolution::Moved(start..=end)
                }
            }
            None => Resolution::Lost,
        }
    }
}

/// `lines[start-1 ..= end-1]` for a 1-based inclusive range, if it exists.
fn window(lines: &[LineHash], start: u32, end: u32) -> Option<&[LineHash]> {
    if start == 0 || end < start {
        return None;
    }
    lines.get(start as usize - 1..end as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hs(text: &str) -> Vec<LineHash> {
        let h = LineHasher::default();
        text.split_whitespace()
            .map(|s| h.hash_line(s.as_bytes()))
            .collect()
    }

    fn anchor(text: &str, range: RangeInclusive<u32>) -> Anchor {
        Anchor::for_range("abc123", "src/a.rs", &hs(text), range).unwrap()
    }

    #[test]
    fn for_range_records_the_range_and_refuses_nonsense() {
        let a = anchor("a b c d", 2..=3);
        assert_eq!((a.start_line, a.end_line), (2, 3));
        assert_eq!(
            (a.revision.as_str(), a.file.as_str()),
            ("abc123", "src/a.rs")
        );
        let lines = hs("a b c d");
        for bad in [0..=1, 4..=5, 5..=5] {
            assert!(
                Anchor::for_range("r", "f", &lines, bad.clone()).is_none(),
                "{bad:?}"
            );
        }
        assert!(Anchor::for_range("r", "f", &[], 1..=1).is_none());
        // A reversed range is not a range (built from parts: clippy rightly
        // dislikes the literal).
        let (a, b) = (3, 2);
        assert!(Anchor::for_range("r", "f", &lines, a..=b).is_none());
    }

    #[test]
    fn same_content_gives_exact() {
        let a = anchor("a b c d", 2..=3);
        assert_eq!(a.resolve(&hs("a b c d")), Resolution::Exact(2..=3));
    }

    #[test]
    fn unrelated_edits_keep_exact_or_moved() {
        let a = anchor("a b c d", 2..=3);
        // A change AFTER the range: still exact.
        assert_eq!(a.resolve(&hs("a b c D e")), Resolution::Exact(2..=3));
        // A line inserted BEFORE: the block moves down by one.
        assert_eq!(a.resolve(&hs("a new b c d")), Resolution::Moved(3..=4));
        // A line removed before it: moves up.
        assert_eq!(a.resolve(&hs("b c d")), Resolution::Moved(1..=2));
    }

    #[test]
    fn changed_content_is_lost() {
        let a = anchor("a b c d", 2..=3);
        assert_eq!(a.resolve(&hs("a b X d")), Resolution::Lost);
        assert_eq!(a.resolve(&hs("a")), Resolution::Lost);
        assert_eq!(a.resolve(&[]), Resolution::Lost);
    }

    #[test]
    fn two_identical_blocks_elsewhere_are_lost_never_guessed() {
        let a = anchor("a b c d", 2..=3);
        // The block moved AND was copied: two candidates, no way to tell.
        assert_eq!(a.resolve(&hs("x y b c z b c")), Resolution::Lost);
        // Overlapping candidates count too.
        let rep = anchor("x x y", 1..=2);
        assert_eq!(rep.resolve(&hs("z x x x")), Resolution::Lost);
    }

    #[test]
    fn a_copy_of_the_block_makes_even_a_confirmed_position_lost() {
        let a = anchor("a b c d", 2..=3);
        // The recorded range still holds the block, but so does another one.
        assert_eq!(a.resolve(&hs("a b c d b c")), Resolution::Lost);
        // The original moved and a copy took its place.
        assert_eq!(a.resolve(&hs("x b c b c")), Resolution::Lost);
    }

    #[test]
    fn a_malformed_anchor_is_lost_not_a_panic() {
        let mut a = anchor("a b c", 1..=2);
        a.start_line = 0;
        assert_eq!(a.resolve(&hs("a b c")), Resolution::Lost);
        a.start_line = 3;
        a.end_line = 1;
        assert_eq!(a.resolve(&hs("a b c")), Resolution::Lost);
    }

    #[test]
    fn rerunning_after_unrelated_edits_keeps_the_answer() {
        // A property in miniature: the answer depends only on whether the
        // content survives and how many times it occurs.
        let a = anchor("p q r s t u", 3..=4);
        let mut file = hs("p q r s t u");
        for edit in 0..20 {
            // Edits strictly after the block, never touching it.
            file.push(LineHasher::default().hash_line(format!("tail{edit}").as_bytes()));
            assert_eq!(a.resolve(&file), Resolution::Exact(3..=4), "edit {edit}");
        }
        // The same edits made ABOVE the block shift it, and `Moved` follows.
        let mut above = Vec::new();
        for edit in 0..20u32 {
            above.insert(
                0,
                LineHasher::default().hash_line(format!("head{edit}").as_bytes()),
            );
            let mut f = above.clone();
            f.extend(hs("p q r s t u"));
            assert_eq!(
                a.resolve(&f),
                Resolution::Moved(edit + 4..=edit + 5),
                "edit {edit}"
            );
        }
    }
}
