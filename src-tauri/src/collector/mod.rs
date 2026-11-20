//! The collector: the app side of `why-hook`. **Stub**: WHY-13 implements it.
//!
//! What WHY-13 builds behind [`start`] (decisions 21-26, payloads.md §4/§7):
//! - bind `paths.socket_path` (parent dir created `0700`; remove a stale socket
//!   only after checking no other instance answers on it; mind the ~108-byte
//!   `sun_path` limit), read newline-delimited envelopes bounded by
//!   `why_wire::MAX_LINE_BYTES`, `why_wire::decode_line` each;
//! - for each envelope, `events::ingest::ingest(&env, &resolver, &*events)`,
//!   with a git-backed `WorktreeResolver` (using `git` + `repos`), then emit
//!   `SESSIONS_CHANGED_EVENT` for every `changed_worktrees` entry;
//! - drain `paths.spool_dir` at start-up and after reconnects;
//! - write `paths.allowlist_path` from `repos` whenever repositories change, so
//!   the hook can drop events from unregistered repositories while the app is
//!   down;
//! - use `InstancePaths::hook_args` when writing the user-level hook config, so
//!   the hook receives the collector's resolved paths;
//! - prune with `EventStore::prune_older_than(now - events::RETENTION)`.
//!
//! The collector runs on Tauri's runtime (`tauri::async_runtime::spawn`), never
//! a runtime of its own.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::AppHandle;
use why_wire::InstancePaths;

use crate::events::EventStore;
use crate::git::GitRunner;
use crate::store::RepoStore;

/// Keeps the collector alive; dropping or [`stop`](CollectorHandle::stop)ping
/// it shuts the collector down (WHY-13).
#[derive(Debug)]
pub struct CollectorHandle {
    socket_path: PathBuf,
}

impl CollectorHandle {
    /// Where the collector listens (or would, for the stub).
    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    /// Stop listening and remove the socket. No-op in the stub.
    pub fn stop(self) {}
}

/// Start the collector for this instance. Never fails the app: a collector
/// that cannot start (socket in use, path too long) only means sessions are
/// not captured, which WHY-13 reports in the log and, later, in the UI.
///
/// `git` is a clone of the app's ONE runner (clones share its semaphore), for
/// resolving event cwds to worktrees.
pub fn start(
    app: &AppHandle,
    paths: &InstancePaths,
    git: GitRunner,
    repos: Arc<dyn RepoStore>,
    events: Arc<dyn EventStore>,
) -> CollectorHandle {
    // The stub uses none of these; WHY-13 does.
    let _ = (app, git, repos, events);
    tracing::info!(
        instance = %paths.instance,
        socket = %paths.socket_path.display(),
        spool = %paths.spool_dir.display(),
        allowlist = %paths.allowlist_path.display(),
        "collector not implemented yet (WHY-13); hook events are not received"
    );
    CollectorHandle {
        socket_path: paths.socket_path.clone(),
    }
}
