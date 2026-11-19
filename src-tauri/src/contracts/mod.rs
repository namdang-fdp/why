//! IPC contracts: the data types that cross the boundary between the Rust core and
//! the webview. Rust is the single source of truth; the TypeScript types in
//! `ui/src/contracts/` are generated from these definitions by ts-rs
//! (`pnpm gen:contracts`) and committed. A test fails if they drift apart.
//!
//! # Planned commands (not implemented yet)
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
//! | `set_base(worktree_id: WorktreeId, base_ref: String)` | `Worktree` | Choose the base ref for `DiffMode::VsBase` |
//!
//! # Event
//!
//! `worktree-changed` ([`WORKTREE_CHANGED_EVENT`]) carries a [`WorktreeChanged`]
//! payload; the core emits it when the file watcher sees a change.
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

pub use changes::{ChangeStatus, DiffMode, FileChange, FileDiff};
pub use error::IpcError;
pub use events::{WORKTREE_CHANGED_EVENT, WorktreeChanged};
pub use ids::{RepoId, WorktreeId};
pub use repo::{BranchInfo, Repository, Worktree};

// `ts-rs` is a dev-dependency: the types derive `TS` only under `cfg(test)`
// (`#[cfg_attr(test, derive(TS))]`), so the app binary does not contain it.
// The serde wire-format tests only matter for development, so they are compiled
// for `cargo test` only.
#[cfg(test)]
mod tests;
