use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

// Path representation: git paths are arbitrary bytes, but JSON strings must be valid
// Unicode. Contracts therefore carry paths as `String`, converted lossily (invalid
// bytes become U+FFFD) by the core, and set `path_lossy: true` when that happened.
// The UI can render the path but must not send a lossy path back as an identifier
// (the core would not find it). Trade-off: a rare, ugly edge case stays simple for
// the 99.9% of paths that are UTF-8, instead of every path becoming a byte array.
// Known v0 limitation: a lossy path cannot be sent back to `get_file_diff`, so such
// files are listed but cannot be opened until the core adds a lookup that does not
// rely on the string (for example an index into the last change list).
// `Worktree::path` and `Repository::root_path` are plain strings without such a flag
// for v0, but they have the same weakness: the core reads worktree paths from
// `git worktree list`, so a non-UTF-8 directory name would be converted lossily and
// could no longer be opened. Known limitation; add a flag if it is ever seen.

/// What to compare. `VsBase` and `Uncommitted` compare against the working tree;
/// `Staged` compares against the index instead.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum DiffMode {
    /// merge-base with the base ref -> working tree, including untracked files.
    VsBase,
    /// HEAD -> working tree (staged and unstaged together).
    Uncommitted,
    /// HEAD -> index (what `git commit` would record).
    Staged,
}

/// How a file changed.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    /// File type changed, for example a regular file became a symlink.
    TypeChanged,
    /// Not tracked by git and not ignored.
    Untracked,
    /// Merge conflict, unresolved.
    Conflicted,
}

/// One entry of the changed-files list.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// Path relative to the worktree root, with `/` separators.
    pub path: String,
    /// Previous path for renames and copies; `None` otherwise.
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    /// Part of the change is in the index.
    pub staged: bool,
    /// Part of the change is only in the working tree.
    pub unstaged: bool,
    /// Added lines; `None` for binary files or when not computed.
    pub additions: Option<u32>,
    /// Removed lines; `None` for binary files or when not computed.
    pub deletions: Option<u32>,
    pub binary: bool,
    /// `path` (or `old_path`) was not valid UTF-8 and was converted lossily.
    pub path_lossy: bool,
}

/// Both sides of one file's diff, ready for a side-by-side editor.
///
/// A content field is `None` when that side does not exist (`old_content` for an
/// added file, `new_content` for a deleted one) or is deliberately withheld
/// (`binary` or `too_large`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: ChangeStatus,
    pub old_content: Option<String>,
    pub new_content: Option<String>,
    pub binary: bool,
    /// Content exceeds the size limit and was not loaded.
    pub too_large: bool,
    pub path_lossy: bool,
}
