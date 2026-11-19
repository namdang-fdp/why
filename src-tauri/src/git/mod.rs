//! Git CLI integration: repository validation, worktree discovery, base
//! detection and status (shells out to the `git` executable).
//!
//! WHY is a *passive observer* of repositories where coding agents are working
//! right now, so every function here only reads, never takes a lock an agent
//! could trip over, and bounds its own cost (see [`runner`]).
//!
//! Requires **git >= 2.45.1** to disable lazy fetching. See [`version`] for
//! the versions required by individual Git features.
//!
//! Layering: this module returns types that model *what git reports*
//! ([`StatusSnapshot`], [`WorktreeEntry`], ...). Mapping them to the UI-facing
//! IPC contracts happens in a later layer, so a change in the UI contract does
//! not ripple into git parsing and vice versa.
//!
//! Design rule: parsers are pure functions over `&[u8]` (testable with
//! hand-written fixtures); the `async fn`s next to them only run git and call
//! the parser.

pub mod base;
pub mod diff;
pub mod error;
pub mod oid;
pub mod repo;
pub mod runner;
pub mod status;
pub mod version;
pub mod worktree;

pub use base::{BaseInfo, detect_base};
pub use diff::{
    Change, ChangeKind, ChangeSet, DiffEntry, OldSide, Side, Target, count_untracked_lines,
    parse_diff, read_changes,
};
pub use error::{GitError, ParseError};
pub use oid::Oid;
pub use repo::{RepoInfo, open_repo};
pub use runner::GitRunner;
pub use status::{
    AheadBehind, BranchHead, BranchStatus, ConflictKind, FileState, RenameOp, StatusEntry,
    StatusKind, StatusSnapshot, parse_status, read_status,
};
pub use version::{GitVersion, MIN_GIT_VERSION, git_version};
pub use worktree::{WorktreeEntry, list_worktrees, parse_worktree_list, repo_id, worktree_id};
