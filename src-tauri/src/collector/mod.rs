//! The collector: the app side of `why-hook`.
//!
//! ```text
//! why-hook ──unix socket──▶ server (bounded lines) ──mpsc(1024)──▶ ingest task ─┐
//!    │                                                                          ├─▶ Pipeline ─▶ EventStore
//!    └─(app down)─▶ spool/<date>.jsonl ──── spool drain (start, every 30 s) ────┘      │
//!                                                                                      └─▶ `sessions-changed` (coalesced)
//! app ──▶ hook-allowlist.json (allowlist_sync) ◀── read by the hook on every event
//! ```
//!
//! Layout: everything except the thin Tauri glue at the bottom of this file is
//! Tauri-free ([`spawn_core`]), so the integration tests in
//! `tests/collector/` drive the real thing with temp directories.
//!
//! The collector runs on Tauri's runtime (`tauri::async_runtime::spawn`), never
//! a runtime of its own. It never fails the app: if it cannot start (another
//! WHY owns the socket, the path is too long) sessions are simply not captured
//! and the reason is logged; the status says so ([`CollectorStatus`]).
//!
//! Backpressure, end to end: a hook never waits for the app (it has a 100 ms
//! budget and then spools), the server never waits for ingest (`try_send` on a
//! bounded queue: when full the envelope is dropped and counted), and live
//! events go through ONE ingest task. (The spool drain also writes, from its own
//! task; that is safe because the store serializes `append` and ignores an event
//! it already has, by key.) Memory is bounded at every hop.

pub mod allowlist_sync;
#[cfg(unix)]
pub mod install;
pub mod pipeline;
pub mod resolver;
#[cfg(unix)]
pub mod server;
#[cfg(unix)]
pub mod spool_drain;
pub mod stats;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Listener};
use tokio::sync::{Notify, mpsc, watch};
use why_wire::InstancePaths;

pub use pipeline::{AdapterNormalizer, ChangeNotifier, Normalizer, Pipeline};
pub use resolver::GitWorktreeResolver;
pub use stats::{Stats, StatsSnapshot};

use crate::contracts::{
    REPOSITORY_CHANGED_EVENT, SESSIONS_CHANGED_EVENT, SessionsChanged, WorktreeId,
};
use crate::events::{EventStore, RETENTION};
use crate::git::GitRunner;
use crate::store::RepoStore;

/// Capacity of the queue between the server and the ingest task. Events are
/// small (a few KB) and ingest drains it at store speed; 1024 absorbs a burst
/// of every agent on the machine at once without holding megabytes.
const INGEST_QUEUE: usize = 1024;

/// Timings, overridable by tests.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub allowlist: allowlist_sync::SyncTiming,
    /// How often the spool is drained after startup.
    pub spool_every: Duration,
    /// `sessions-changed` coalescing window.
    pub notify_window: Duration,
    /// How often old sessions are pruned.
    pub retention_every: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            allowlist: allowlist_sync::SyncTiming::default(),
            spool_every: Duration::from_secs(30),
            notify_window: Duration::from_millis(100),
            retention_every: Duration::from_secs(24 * 60 * 60),
        }
    }
}

/// Everything [`spawn_core`] needs.
pub struct CoreConfig {
    pub paths: InstancePaths,
    pub git: GitRunner,
    /// The registry. In the app this is the store wrapped by
    /// [`allowlist_sync::wrap_store`], whose `state` is `allowlist` below.
    pub repos: Arc<dyn RepoStore>,
    pub events: Arc<dyn EventStore>,
    /// What the hook allowlist file should say; shared with the wrapped store.
    pub allowlist: Arc<allowlist_sync::AllowlistState>,
    /// The adapters in production ([`AdapterNormalizer`]).
    pub normalizer: Arc<dyn Normalizer>,
    /// Called once per changed worktree per coalescing window.
    pub emit: Arc<dyn Fn(WorktreeId) + Send + Sync>,
    pub timing: Timing,
    #[cfg(unix)]
    pub server: server::ServerConfig,
}

/// Whether events are being received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectorStatus {
    Starting,
    Listening,
    /// Not receiving; the text says why (logged at error level as well).
    Disabled(String),
}

/// A running collector core. Dropping it (or [`CoreHandle::stop`]) shuts it down.
pub struct CoreHandle {
    stats: Arc<Stats>,
    status: watch::Receiver<CollectorStatus>,
    shutdown: watch::Sender<bool>,
    /// Set once the socket is bound; removed on stop.
    #[cfg(unix)]
    guard: Arc<OnceLock<Arc<server::SocketGuard>>>,
    allowlist_wake: Arc<Notify>,
    spool_wake: Arc<Notify>,
}

impl CoreHandle {
    pub fn stats(&self) -> StatsSnapshot {
        self.stats.snapshot()
    }

    pub fn status(&self) -> CollectorStatus {
        self.status.borrow().clone()
    }

    /// Wait until the collector is listening or has given up.
    pub async fn ready(&self) -> CollectorStatus {
        let mut rx = self.status.clone();
        loop {
            let current = rx.borrow_and_update().clone();
            if current != CollectorStatus::Starting {
                return current;
            }
            if rx.changed().await.is_err() {
                return rx.borrow().clone();
            }
        }
    }

    /// Re-read worktrees from git and rewrite the allowlist now.
    pub fn wake_allowlist(&self) -> Arc<Notify> {
        Arc::clone(&self.allowlist_wake)
    }

    /// Drain the spool now.
    pub fn wake_spool(&self) -> Arc<Notify> {
        Arc::clone(&self.spool_wake)
    }

    /// Stop everything and remove the socket file (only if it is ours).
    pub fn stop(&self) {
        let _ = self.shutdown.send(true);
        #[cfg(unix)]
        if let Some(guard) = self.guard.get() {
            guard.release();
        }
    }
}

impl Drop for CoreHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the collector's tasks. Returns at once; use [`CoreHandle::ready`] to
/// wait for the socket.
pub fn spawn_core(cfg: CoreConfig) -> CoreHandle {
    let stats = Arc::new(Stats::default());
    let (status_tx, status_rx) = watch::channel(CollectorStatus::Starting);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let allowlist_wake = cfg.allowlist.wake();
    let spool_wake = Arc::new(Notify::new());
    #[cfg(unix)]
    let guard: Arc<OnceLock<Arc<server::SocketGuard>>> = Arc::new(OnceLock::new());

    #[cfg(unix)]
    tauri::async_runtime::spawn(run_core(
        cfg,
        Arc::clone(&stats),
        status_tx,
        shutdown_rx,
        Arc::clone(&guard),
        Arc::clone(&allowlist_wake),
        Arc::clone(&spool_wake),
    ));
    #[cfg(not(unix))]
    {
        let _ = (cfg, shutdown_rx, &allowlist_wake, &spool_wake);
        let _ = status_tx.send(CollectorStatus::Disabled(
            "the collector needs unix sockets".into(),
        ));
    }

    CoreHandle {
        stats,
        status: status_rx,
        shutdown: shutdown_tx,
        #[cfg(unix)]
        guard,
        allowlist_wake,
        spool_wake,
    }
}

#[cfg(unix)]
async fn run_core(
    cfg: CoreConfig,
    stats: Arc<Stats>,
    status: watch::Sender<CollectorStatus>,
    shutdown: watch::Receiver<bool>,
    guard_slot: Arc<OnceLock<Arc<server::SocketGuard>>>,
    allowlist_wake: Arc<Notify>,
    spool_wake: Arc<Notify>,
) {
    let CoreConfig {
        paths,
        git,
        repos,
        events,
        allowlist,
        normalizer,
        emit,
        timing,
        server: server_cfg,
    } = cfg;

    // Bind FIRST: if another instance owns the socket we must not touch its
    // allowlist, spool or database either (same paths).
    let bound = match server::bind(&paths.socket_path).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "collector disabled: agent sessions will not be captured");
            let _ = status.send(CollectorStatus::Disabled(e.to_string()));
            return;
        }
    };
    let _ = guard_slot.set(Arc::clone(&bound.guard));
    // `stop()` may have run before the guard existed.
    if *shutdown.borrow() {
        bound.guard.release();
        return;
    }
    tracing::info!(socket = %paths.socket_path.display(), "collector listening");

    let notifier = ChangeNotifier::new(emit, timing.notify_window);
    let resolver = GitWorktreeResolver::new(git.clone(), Arc::clone(&repos));
    let pipeline = Arc::new(Pipeline::new(
        normalizer,
        resolver,
        Arc::clone(&events),
        notifier,
    ));

    // Allowlist before the first hook can matter, but not awaited: the socket
    // answers meanwhile.
    tauri::async_runtime::spawn(allowlist_sync::run(
        git,
        repos,
        allowlist,
        timing.allowlist,
        allowlist_wake,
        shutdown.clone(),
    ));

    // The only writer to the store for live events.
    let (tx, rx) = mpsc::channel::<server::Queued>(INGEST_QUEUE);
    let budget = server::ByteBudget::new(server_cfg.max_queued_bytes);
    tauri::async_runtime::spawn(ingest_loop(
        rx,
        Arc::clone(&budget),
        Arc::clone(&pipeline),
        Arc::clone(&stats),
    ));

    let drainer = Arc::new(spool_drain::SpoolDrainer::new(
        paths.spool_dir.clone(),
        Arc::clone(&pipeline),
        Arc::clone(&stats),
    ));
    tauri::async_runtime::spawn(spool_drain::run(
        drainer,
        timing.spool_every,
        spool_wake,
        shutdown.clone(),
    ));
    tauri::async_runtime::spawn(retention_loop(
        events,
        timing.retention_every,
        shutdown.clone(),
    ));

    let _ = status.send(CollectorStatus::Listening);
    server::serve(bound.listener, tx, budget, stats, server_cfg, shutdown).await;
    // `serve` returned: shutdown. Remove the socket (idempotent with `stop`).
    bound.guard.release();
}

/// One task owns LIVE ingest: envelopes are stored in arrival order, one at a time.
async fn ingest_loop(
    mut rx: mpsc::Receiver<server::Queued>,
    budget: Arc<server::ByteBudget>,
    pipeline: Arc<Pipeline>,
    stats: Arc<Stats>,
) {
    while let Some(queued) = rx.recv().await {
        let result = pipeline.ingest(&queued.env).await;
        budget.release(queued.bytes);
        match result {
            Ok(_) => stats::bump(&stats.ingested),
            Err(e) => {
                tracing::warn!(error = %e, "cannot store an event");
                stats::bump(&stats.ingest_errors);
            }
        }
    }
}

/// Delete sessions older than [`RETENTION`] now and then every `every`.
async fn retention_loop(
    events: Arc<dyn EventStore>,
    every: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(0))
            .unwrap_or(0);
        let cutoff = now_ms.saturating_sub(u64::try_from(RETENTION.as_millis()).unwrap_or(0));
        let store = Arc::clone(&events);
        match tauri::async_runtime::spawn_blocking(move || store.prune_older_than(cutoff)).await {
            Ok(Ok(0)) => {}
            Ok(Ok(n)) => tracing::info!(sessions = n, "pruned sessions older than 30 days"),
            Ok(Err(e)) => tracing::warn!(error = %e, "cannot prune old sessions"),
            Err(e) => tracing::warn!(error = %e, "prune task failed"),
        }
        tokio::select! {
            _ = tokio::time::sleep(every) => {}
            _ = shutdown.changed() => return,
        }
    }
}

// ───────────────────────── Tauri glue ─────────────────────────

/// Keeps the collector alive; dropping or [`stop`](CollectorHandle::stop)ping
/// it shuts the collector down and removes its socket.
pub struct CollectorHandle {
    socket_path: PathBuf,
    core: CoreHandle,
    /// Unsubscribes from `repository-changed` on stop.
    listener: Mutex<Option<(AppHandle, tauri::EventId)>>,
}

impl std::fmt::Debug for CollectorHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollectorHandle")
            .field("socket_path", &self.socket_path)
            .field("status", &self.core.status())
            .finish()
    }
}

impl CollectorHandle {
    /// Where the collector listens.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn status(&self) -> CollectorStatus {
        self.core.status()
    }

    pub fn stats(&self) -> StatsSnapshot {
        self.core.stats()
    }

    /// Stop listening and remove the socket.
    pub fn stop(self) {
        drop(self);
    }

    /// Same as [`stop`](CollectorHandle::stop), through a shared reference: the
    /// handle lives in Tauri's managed state, which is NOT dropped when the
    /// process exits, so `lib.rs` calls this on `RunEvent::Exit`. Idempotent.
    pub fn shutdown(&self) {
        if let Ok(mut slot) = self.listener.lock()
            && let Some((app, id)) = slot.take()
        {
            app.unlisten(id);
        }
        self.core.stop();
    }
}

impl Drop for CollectorHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Start the collector for this instance. Never fails the app: a collector
/// that cannot start (socket in use, path too long) only means sessions are
/// not captured; the reason is logged and in [`CollectorHandle::status`].
///
/// `git` is a clone of the app's ONE runner (clones share its semaphore), for
/// resolving event cwds to worktrees. `repos` must be the store wrapped by
/// [`allowlist_sync::wrap_store`] and `allowlist` its state.
pub fn start(
    app: &AppHandle,
    paths: &InstancePaths,
    git: GitRunner,
    repos: Arc<dyn RepoStore>,
    allowlist: Arc<allowlist_sync::AllowlistState>,
    events: Arc<dyn EventStore>,
) -> CollectorHandle {
    let emitter = app.clone();
    let emit: Arc<dyn Fn(WorktreeId) + Send + Sync> = Arc::new(move |worktree_id| {
        if let Err(e) = emitter.emit(SESSIONS_CHANGED_EVENT, &SessionsChanged { worktree_id }) {
            tracing::warn!(%e, "cannot emit {SESSIONS_CHANGED_EVENT}");
        }
    });
    let core = spawn_core(CoreConfig {
        paths: paths.clone(),
        git,
        repos,
        events,
        allowlist,
        normalizer: Arc::new(AdapterNormalizer),
        emit,
        timing: Timing::default(),
        #[cfg(unix)]
        server: server::ServerConfig::default(),
    });

    // The watcher emits `repository-changed` when a linked worktree appears or
    // vanishes: re-read the worktrees now so the hook allowlist follows.
    let wake = core.wake_allowlist();
    let id = app.listen(REPOSITORY_CHANGED_EVENT, move |_| wake.notify_one());

    CollectorHandle {
        socket_path: paths.socket_path.clone(),
        core,
        listener: Mutex::new(Some((app.clone(), id))),
    }
}
