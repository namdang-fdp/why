//! IPC contracts: the data types that cross the boundary between the Rust core and
//! the webview. Rust is the single source of truth; the TypeScript types in
//! `ui/src/contracts/` are generated from these definitions by ts-rs
//! (`pnpm gen:contracts`) and committed. A test fails if they drift apart.
//!
//! # Commands
//!
//! Every command returns `Result<T, IpcError>`. On the TypeScript side a failed
//! `invoke` rejects with the serialized `IpcError`.
//!
//! | Command | Returns | Purpose |
//! |---|---|---|
//! | `list_repositories()` | `Vec<Repository>` | Registered repositories |
//! | `add_repository(path: String)` | `Repository` | Register the repository containing `path` |
//! | `remove_repository(id: RepoId)` | `()` | Unregister (never touches the repo on disk) |
//! | `list_worktrees(repo_id: RepoId)` | `Vec<Worktree>` | Worktrees of one repository |
//! | `get_changes(worktree_id: WorktreeId, mode: DiffMode)` | `Vec<FileChange>` | Changed-files list |
//! | `get_file_diff(worktree_id: WorktreeId, path: String, mode: DiffMode)` | `FileDiff` | Both sides of one file |
//! | `set_base(worktree_id: WorktreeId, base_ref: String)` | `Worktree` | Choose the base ref for `DiffMode::VsBase`; an empty string clears the override |
//! | `get_repository_security(repo_id: RepoId)` | `RepositorySecurity` | The repository's git filter drivers and whether WHY may read its files |
//! | `trust_repository_filters(repo_id: RepoId, fingerprint: String)` | `RepositorySecurity` | Trust the drivers the user was shown (`fingerprint` must still match) |
//! | `revoke_repository_filters(repo_id: RepoId)` | `RevokeResult` | Forget the trust decision (always done when `Ok`; the fresh report may be missing if the rescan failed) |
//! | `get_storage_status()` | `StorageStatus` | Whether repositories are saved or only kept for this session |
//! | `pick_repository_folder()` | `Option<String>` | Native folder picker (`None` = cancelled) |
//! | `list_sessions(worktree_id: WorktreeId)` | `Vec<SessionSummary>` | Agent sessions seen in a worktree, most recent first |
//! | `get_session_timeline(session_id: SessionId)` | `SessionTimeline` | One session's turns and events |
//!
//! # Event
//!
//! `worktree-changed` ([`WORKTREE_CHANGED_EVENT`]) carries a [`WorktreeChanged`]
//! payload; the core emits it when the file watcher sees a change.
//!
//! `repository-changed` ([`REPOSITORY_CHANGED_EVENT`]) carries a
//! [`RepositoryChanged`] payload; the core emits it when a repository's set of
//! worktrees changes (added, removed, pruned).
//!
//! `sessions-changed` ([`SESSIONS_CHANGED_EVENT`]) carries a [`SessionsChanged`]
//! payload; the core emits it when hook events for a worktree's sessions arrive.
//!
//! # Wire format rules
//!
//! - Field names are camelCase (`#[serde(rename_all = "camelCase")]`).
//! - `Option<T>` is always present and serializes as `null` when `None`, so the
//!   TypeScript type is `T | null` and never `undefined`.
//! - Unit-only enums serialize as strings; [`IpcError`] is internally tagged with `kind`.
//! - Counts are `u32` (TypeScript `number`); see `repo.rs` for why 64-bit ints are banned.
//! - Paths are `String` plus a `path_lossy` flag; see `changes.rs`.

mod changes;
mod error;
mod events;
mod ids;
mod repo;
mod security;
mod sessions;
mod storage;

pub use changes::{ChangeStatus, DiffMode, FileChange, FileDiff};
pub use error::IpcError;
pub use events::{
    REPOSITORY_CHANGED_EVENT, RepositoryChanged, SESSIONS_CHANGED_EVENT, SessionsChanged,
    WORKTREE_CHANGED_EVENT, WorktreeChanged,
};
pub use ids::{RepoId, WorktreeId};
pub use repo::{BranchInfo, Repository, Worktree};
pub use security::{
    FilterDriverInfo, FilterGuardStatus, FilterScope, RepositorySecurity, RevokeResult,
};
pub use sessions::{
    EvidenceLabel, Harness, SessionId, SessionStatus, SessionSummary, SessionTimeline,
    TimelineEvent, TimelineEventKind, ToolOutcome, Turn,
};
pub use storage::StorageStatus;

// `ts-rs` is a dev-dependency: the types derive `TS` only under `cfg(test)`
// (`#[cfg_attr(test, derive(TS))]`), so the app binary does not contain it.
// Binding generation and the serde wire-format tests only matter for development,
// so they are compiled for `cargo test` only.
#[cfg(test)]
mod export;
#[cfg(test)]
mod tests;
