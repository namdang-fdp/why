//! Filesystem watcher: tells the UI when files in a registered worktree change.
//!
//! * [`debounce`]: pure batching logic (quiet window + max delay + path cap).
//! * [`ignore_rules`]: gitignore matching, so build output does not flood events.
//! * [`route`]: maps an absolute changed path to a worktree (or drops it).
//! * [`poller`]: the polling fallback (high-resolution fingerprints).
//! * [`hub`]: owns the OS watcher (notify: inotify on Linux, FSEvents on macOS),
//!   keeps it in sync with the registered repositories, and produces debounced
//!   [`WorktreeChanged`] values on a channel. Testable without Tauri.
//! * [`notifying_store`]: a `RepoStore` decorator that wakes the hub when a
//!   repository is added or removed.
//! * [`start_for_app`]: the thin Tauri adapter that emits those values as the
//!   `worktree-changed` event.
//!
//! The watcher only ever reads: it never writes inside a watched repository and
//! only runs read-only git commands (`worktree list`, `rev-parse`) through the
//! shared [`GitRunner`].

pub mod debounce;
pub mod hub;
pub mod ignore_rules;
pub mod notifying_store;
pub mod poller;
pub mod route;

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager};

pub use debounce::{DebounceConfig, RawChange};
pub use hub::{ResyncHandle, WatchConfig, WatchHub};
pub use notifying_store::NotifyingRepoStore;

use crate::contracts::{WORKTREE_CHANGED_EVENT, WorktreeChanged};
use crate::git::GitRunner;
use crate::store::RepoStore;

/// Start watching every worktree of every repository in `store` and forward
/// changes to the webview as [`WORKTREE_CHANGED_EVENT`]. The hub is put in
/// Tauri's managed state (`State<'_, WatchHub>`, for diagnostics).
///
/// Returns the handle to wrap the store in a [`NotifyingRepoStore`], so adding
/// or removing a repository through the IPC commands resyncs the watcher at
/// once. Pass the *unwrapped* store here: the hub only reads it.
///
/// Pass a clone of the app's one `GitRunner` (clones share its semaphore, so
/// the watcher's git calls count against the same global cap).
pub fn start_for_app(app: &AppHandle, git: GitRunner, store: Arc<dyn RepoStore>) -> ResyncHandle {
    let (hub, mut events) = WatchHub::start(git, store, WatchConfig::default());
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = events.recv().await {
            log_event(&ev);
            if let Err(e) = handle.emit(WORKTREE_CHANGED_EVENT, &ev) {
                tracing::warn!(%e, "cannot emit {WORKTREE_CHANGED_EVENT}");
            }
        }
    });
    let resync = hub.resync_handle();
    app.manage(hub);
    resync
}

fn log_event(ev: &WorktreeChanged) {
    tracing::debug!(
        worktree = %ev.worktree_id.0,
        paths = ev.paths.len(),
        sample = ?ev.paths.iter().take(3).collect::<Vec<_>>(),
        "emit {WORKTREE_CHANGED_EVENT}"
    );
}
