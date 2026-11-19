use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

use super::ids::{RepoId, WorktreeId};

// Number types: every count below is `u32`. ts-rs maps `u32` to TypeScript `number`
// (exact up to 2^53), while `u64`/`i64` become `bigint`, which `JSON.parse` never
// produces, so the generated type would lie about the wire value. Do not use 64-bit
// integers in contracts; a test fails if `bigint` ever shows up in the bindings.

/// A repository the user registered. One repository has one or more worktrees.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub id: RepoId,
    /// Display name, normally the directory name of the main worktree.
    pub name: String,
    /// Absolute path of the main worktree.
    pub root_path: String,
}

/// One git worktree (the main checkout or a linked one).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub id: WorktreeId,
    pub repo_id: RepoId,
    /// Absolute path of the worktree directory.
    pub path: String,
    /// Short branch name (`main`), or `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Full commit SHA of HEAD; `None` for an unborn branch (no commits yet).
    pub head_sha: Option<String>,
    pub is_main: bool,
    pub is_detached: bool,
    pub is_locked: bool,
    /// `git worktree prune` would remove it (its directory is gone).
    pub is_prunable: bool,
    /// How this worktree relates to its base branch; `None` when no base is known
    /// (for example the main worktree on the base branch itself).
    pub base: Option<BranchInfo>,
    /// Number of changed files, so the rail can show a badge without fetching the
    /// list. Counted like `get_changes` with `DiffMode::VsBase`, or with
    /// `DiffMode::Uncommitted` when `base` is `None`.
    pub change_count: u32,
}

/// A worktree's position relative to its base ref.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    /// The ref changes are measured against, for example `origin/main`.
    pub base_ref: String,
    /// Common ancestor of HEAD and `base_ref`; `None` if the histories are unrelated.
    pub merge_base: Option<String>,
    /// Commits on HEAD that are not on the base.
    pub ahead: u32,
    /// Commits on the base that are not on HEAD.
    pub behind: u32,
}
