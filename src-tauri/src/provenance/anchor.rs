//! A stable pointer to a range of lines (WHY-23 gives it behaviour).
//!
//! Path-relative and content-keyed, not keyed by an absolute path or a
//! worktree id (decision 4, portability): the same commit checked out in
//! another worktree, on another machine or after a move must resolve the same
//! anchor. The content hash lets a consumer check that the lines are still the
//! ones the anchor was made for, and find them again after they moved.

use super::line::LinesDigest;

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
