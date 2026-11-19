//! Application state shared by every IPC command.
//!
//! Tauri keeps one value per type in its "managed state" (`Builder::manage`).
//! A command asks for it with a `State<'_, AppState>` parameter, and Tauri hands
//! out a shared reference; the state must therefore be `Send + Sync + 'static`.
//! Commands run concurrently, so everything here is either immutable or does its
//! own locking (the store, the runner's semaphore).

use std::sync::Arc;

use crate::git::GitRunner;
use crate::store::RepoStore;

pub struct AppState {
    /// The ONE `GitRunner` for the whole app. Its semaphore caps how many git
    /// processes run at once, and that cap is only global if every command shares
    /// this single runner. Do not build a runner per command or per repository:
    /// ten repositories would each get their own cap of 8 and could spawn 80
    /// concurrent git processes against the user's disk.
    pub git: GitRunner,
    /// Where registered repositories (and base overrides) are remembered. A trait
    /// object so the backend (memory now, SQLite later) is chosen in `lib.rs`
    /// only; see `store/mod.rs`.
    pub repos: Arc<dyn RepoStore>,
}

impl AppState {
    pub fn new(git: GitRunner, repos: Arc<dyn RepoStore>) -> Self {
        Self { git, repos }
    }
}
