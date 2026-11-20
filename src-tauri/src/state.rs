//! Application state shared by every IPC command.
//!
//! Tauri keeps one value per type in its "managed state" (`Builder::manage`).
//! A command asks for it with a `State<'_, AppState>` parameter, and Tauri hands
//! out a shared reference; the state must therefore be `Send + Sync + 'static`.
//! Commands run concurrently, so everything here is either immutable or does its
//! own locking (the store, the runner's semaphore).

use std::sync::Arc;

use crate::events::{EventStore, MemoryEventStore};
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
    /// Agent sessions and their events (v0.2). Same seam as `repos`: memory
    /// now, SQLite `events.db` after WHY-12, chosen in `lib.rs` only.
    pub events: Arc<dyn EventStore>,
}

impl AppState {
    /// State with an empty in-memory event store. `lib.rs` replaces it with
    /// [`AppState::with_events`]; tests that do not care about sessions keep it.
    ///
    /// A builder method instead of a third `new` argument on purpose: every
    /// existing test builds `AppState::new(git, repos)`, and those files belong
    /// to other tasks; a new parameter would touch all of them for no gain.
    pub fn new(git: GitRunner, repos: Arc<dyn RepoStore>) -> Self {
        Self {
            git,
            repos,
            events: Arc::new(MemoryEventStore::new()),
        }
    }

    /// Use `events` as the event store.
    pub fn with_events(mut self, events: Arc<dyn EventStore>) -> Self {
        self.events = events;
        self
    }
}
