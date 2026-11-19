//! Git CLI integration: repository validation, worktree discovery, base
//! detection and status (shells out to the `git` executable).
//!
//! WHY is a *passive observer* of repositories where coding agents are working
//! right now, so every function here only reads, never takes a lock an agent
//! could trip over, and bounds its own cost (see [`runner`]).
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
pub mod error;
pub mod oid;
pub mod repo;
pub mod runner;
pub mod status;
pub mod worktree;

pub use base::{BaseInfo, detect_base};
pub use error::{GitError, ParseError};
pub use oid::Oid;
pub use repo::{RepoInfo, open_repo};
pub use runner::GitRunner;
pub use status::{
    AheadBehind, BranchHead, BranchStatus, ConflictKind, FileState, RenameOp, StatusEntry,
    StatusKind, StatusSnapshot, parse_status, read_status,
};
pub use worktree::{WorktreeEntry, list_worktrees, parse_worktree_list, worktree_id};
