//! Persistence seam: *what* the app needs to remember about repositories,
//! independent of *where* it is kept.
//!
//! [`RepoStore`] is a trait (an interface) on purpose. It is the seam between two
//! pieces of work that proceed in parallel:
//!
//! * the IPC commands (`ipc/repos.rs`, `ipc/changes.rs`) depend only on the trait;
//! * the storage backends implement it: [`MemoryRepoStore`] now, SQLite later.
//!
//! This is dependency inversion: the high-level code (commands) owns the
//! interface it needs, and the low-level code (a database) is plugged in behind
//! it. Swapping the backend is a one-line change in `lib.rs`, and commands can be
//! tested against the in-memory store without touching disk.
//!
//! The methods are synchronous. An implementation that talks to a database hides
//! its own locking or threading behind the trait (see [`memory`] for why a plain
//! `std::sync::Mutex` is fine), so callers stay simple. Implementations must be
//! quick; if a backend ever becomes slow, callers can wrap it in
//! `spawn_blocking` without changing this trait.

pub mod memory;

use std::path::PathBuf;

use crate::contracts::{RepoId, Repository, WorktreeId};

pub use memory::MemoryRepoStore;

/// A registered repository as the core stores it.
///
/// An internal type, not an IPC contract: it also carries `common_dir`, which the
/// UI never needs, and it uses `PathBuf` where the contract uses `String`.
/// [`RepoRecord::to_contract`] converts it at the IPC boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRecord {
    /// Derived from `common_dir` by `git::repo_id`.
    pub id: RepoId,
    /// Display name, normally the directory name of the main worktree.
    pub name: String,
    /// Canonical path of the main worktree.
    pub root_path: PathBuf,
    /// Canonical shared git directory (identifies the repository).
    pub common_dir: PathBuf,
}

impl RepoRecord {
    /// The UI-facing shape. The path conversion is lossy for non-UTF-8 paths;
    /// see the note on paths in `contracts/changes.rs`.
    pub fn to_contract(&self) -> Repository {
        Repository {
            id: self.id.clone(),
            name: self.name.clone(),
            root_path: self.root_path.to_string_lossy().into_owned(),
        }
    }
}

/// Result of [`RepoStore::insert`]. Registering a repository twice is not an
/// error (the user may add it again from another worktree), so the caller can
/// tell the two cases apart but usually just wants the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertOutcome {
    /// The record was new and is now stored.
    Inserted(RepoRecord),
    /// A record with this id already existed; this is the stored one, unchanged
    /// (the record passed to `insert` is discarded).
    AlreadyPresent(RepoRecord),
}

impl InsertOutcome {
    /// The record that is stored after the call, whichever case it was.
    pub fn into_record(self) -> RepoRecord {
        match self {
            InsertOutcome::Inserted(r) | InsertOutcome::AlreadyPresent(r) => r,
        }
    }
}

/// Something went wrong in the storage backend.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The backend failed: a database error, a poisoned lock, a full disk...
    /// The text is for logs; the UI only gets a short generic message.
    #[error("storage backend failed: {0}")]
    Backend(String),
}

/// Everything the core persists about registered repositories.
///
/// `Send + Sync` is required because the store lives in `AppState` as an
/// `Arc<dyn RepoStore>` shared by every command, and commands run on whatever
/// thread of Tauri's runtime picks them up. `&self` methods (not `&mut self`)
/// mean the implementation owns its interior mutability.
pub trait RepoStore: Send + Sync {
    /// All repositories, in a deterministic order (by name, then root path).
    fn list(&self) -> Result<Vec<RepoRecord>, StoreError>;

    /// One repository by id, or `None`.
    fn get(&self, id: &RepoId) -> Result<Option<RepoRecord>, StoreError>;

    /// Store `rec` unless a record with the same id exists. Idempotent: a second
    /// insert of the same repository returns [`InsertOutcome::AlreadyPresent`]
    /// with the original record, never an error.
    fn insert(&self, rec: RepoRecord) -> Result<InsertOutcome, StoreError>;

    /// Unregister a repository. Returns `false` if it was not registered.
    /// Never touches the repository on disk.
    ///
    /// Base overrides are keyed by worktree, and a `WorktreeId` is an opaque hash
    /// that does not say which repository it belongs to, so a backend cannot
    /// cascade-delete them here unless it records that link itself. Orphaned
    /// overrides are harmless: nothing looks them up once the repository is gone.
    fn remove(&self, id: &RepoId) -> Result<bool, StoreError>;

    /// The base ref the user picked for a worktree (e.g. `origin/main`), if any.
    /// `None` means "use the auto-detected base".
    fn base_override(&self, wt: &WorktreeId) -> Result<Option<String>, StoreError>;

    /// Set (`Some`) or clear (`None`) the base override of a worktree.
    fn set_base_override(&self, wt: &WorktreeId, base: Option<String>) -> Result<(), StoreError>;
}
