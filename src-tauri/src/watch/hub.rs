//! [`WatchHub`]: owns the OS watcher, keeps the set of watched worktrees in line
//! with the registered repositories, and turns raw notifications into
//! debounced [`WorktreeChanged`] values on a channel. No Tauri in here; the
//! adapter in `watch/mod.rs` forwards the channel to the webview.
//!
//! ```text
//!  notify thread (inotify)  ─┐     tokio tasks (Tauri's runtime)
//!  why-poller thread (fallback)─┤   ─────────────────────────────
//!  Router::handle_event      │
//!    route path, drop ignored│
//!    ─ RawSender::try_send ──┼─► bounded raw ──► run_debouncer ──► out ──► adapter: app.emit
//!      (full: mark worktree  │    channel        (due-first flush,   (bounded)
//!       "overflowed")        │                    overflow => refetch-all)
//!    ─ Maint ────────────────┴─► maintenance_loop: every 30 s, or 300 ms after a signal
//!       Resync | RebuildIgnore | RefreshTracked     ├ sync(): store -> git worktree list ->
//!       | WatchLimit(path) | RootGone(root)         │   registry swap -> watch set diff -> mtimes
//!  NotifyingRepoStore ── ResyncHandle ─────────────►├ rebuild ignore rules / tracked-ignored set
//!   (insert/remove)                                 └ switch a root to polling, forget dead roots
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, Weak};
use std::time::{Duration, SystemTime};

use notify::event::{CreateKind, EventKind, MetadataKind, ModifyKind, RemoveKind, RenameMode};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::async_runtime::{self, JoinHandle};
use tokio::sync::mpsc;

use super::debounce::{Change, DebounceConfig, RawChange, RawSender, raw_channel, run_debouncer};
use super::ignore_rules::IgnoreRules;
use super::poller::Poller;
use super::route::{PathInfo, Registry, RepoSpec, Routed, WorktreeSpec};
use crate::contracts::{RepoId, RepositoryChanged, WorktreeChanged, WorktreeId};
use crate::git::{self, GitRunner, RepoInfo};
use crate::store::{RepoRecord, RepoStore};

/// Printed whenever the kernel refuses another inotify watch.
const WATCH_LIMIT_HINT: &str = "inotify watch limit reached (fs.inotify.max_user_watches); \
     falling back to polling. Raise it with e.g. \
     `sudo sysctl fs.inotify.max_user_watches=524288`";

/// After a maintenance signal, wait this long before acting: `git worktree add`
/// creates the directory first and writes HEAD and the checkout afterwards, and
/// a burst of signals (a `git checkout` that creates 50 directories) folds into
/// one round of work.
const SETTLE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy)]
pub struct WatchConfig {
    pub debounce: DebounceConfig,
    /// How often to re-list worktrees and compare HEAD/index mtimes, as a
    /// safety net for anything the OS watcher missed.
    pub reconcile_every: Duration,
    /// Scan interval for the polling fallback.
    pub poll_interval: Duration,
    /// Skip the OS watcher and poll everything (tests of the fallback path).
    pub force_polling: bool,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            debounce: DebounceConfig::default(),
            reconcile_every: Duration::from_secs(30),
            poll_interval: Duration::from_secs(2),
            force_polling: false,
        }
    }
}

/// Work for the maintenance task (things that need git or blocking I/O, which
/// the notify thread must not do itself).
#[derive(Debug)]
enum Maint {
    /// Re-read the store and re-list worktrees now.
    Resync,
    /// Rebuild this worktree's ignore rules (a `.gitignore`, `info/exclude` or
    /// a directory changed).
    RebuildIgnore(WorktreeId),
    /// Re-list this worktree's tracked-but-ignored files (its index changed).
    RefreshTracked(WorktreeId),
    /// The kernel refused a watch under this path: poll its root instead.
    WatchLimit(PathBuf),
    /// This watched root was deleted or moved: its watch is dead.
    RootGone(PathBuf),
}

/// Asks the hub to re-read the registered repositories now. Cheap to clone;
/// callable from any thread. See `NotifyingRepoStore`.
#[derive(Clone)]
pub struct ResyncHandle(Arc<dyn Fn() + Send + Sync>);

impl ResyncHandle {
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub fn request(&self) {
        (self.0)();
    }
}

/// Watches every worktree of every registered repository.
///
/// Lifecycle: [`WatchHub::start`] spawns the debounce and maintenance tasks;
/// the maintenance task syncs immediately and then every
/// [`WatchConfig::reconcile_every`]. Registration changes made through a
/// `NotifyingRepoStore` wake it at once ([`WatchHub::resync_handle`]).
/// Dropping the hub stops everything.
pub struct WatchHub {
    inner: Arc<Inner>,
    tasks: Vec<JoinHandle<()>>,
}

struct Inner {
    git: GitRunner,
    store: Arc<dyn RepoStore>,
    /// Shared with the notify callback, which only reads it. Replaced as a
    /// whole by `sync`, so the callback never sees a half-updated table.
    registry: Arc<RwLock<Registry>>,
    /// The OS watcher and the poller. A std mutex: only ever locked inside
    /// `spawn_blocking` (adding a recursive watch walks the tree), never
    /// across an `.await`.
    watchers: Mutex<Watchers>,
    raw: RawSender,
    maint: mpsc::UnboundedSender<Maint>,
    /// Repositories whose set of worktrees changed. Unbounded: one tiny message
    /// per `git worktree add/remove`, at the pace of a person or an agent.
    repo_events: mpsc::UnboundedSender<RepositoryChanged>,
    /// Serializes `sync` (a tokio mutex because it is held across git calls).
    sync_state: tokio::sync::Mutex<SyncState>,
}

#[derive(Default)]
struct SyncState {
    /// Worktrees we watch, with what we learned about them.
    known: HashMap<WorktreeId, Known>,
    /// The first sync after start is the baseline: nothing is "new" yet. After
    /// it, every newly watched worktree gets a refetch-all event, because edits
    /// made before its watch existed were not seen.
    initial_done: bool,
}

struct Known {
    spec: Arc<WorktreeSpec>,
    stamp: Stamp,
}

/// The ids of the worktrees each repository has in `known`.
fn members_by_repo(known: &HashMap<WorktreeId, Known>) -> HashMap<RepoId, HashSet<WorktreeId>> {
    let mut by_repo: HashMap<RepoId, HashSet<WorktreeId>> = HashMap::new();
    for (id, k) in known {
        by_repo
            .entry(k.spec.repo.clone())
            .or_default()
            .insert(id.clone());
    }
    by_repo
}

/// Modification times of a worktree's HEAD, index and checked-out branch ref,
/// compared at each reconcile to catch changes the watcher missed.
type Stamp = Vec<Option<SystemTime>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Native,
    Poll,
}

struct Watchers {
    router: Router,
    poll_interval: Duration,
    /// `None` if the OS watcher could not be created (e.g. too many inotify
    /// instances) or polling is forced; everything is polled then.
    native: Option<RecommendedWatcher>,
    /// Created on first use: most setups never need it.
    poll: Option<Poller>,
    watched: HashMap<PathBuf, Backend>,
}

impl WatchHub {
    /// Start watching the repositories in `store`. Returns the hub and the
    /// stream of debounced events. Must be called inside Tauri's runtime or a
    /// tokio runtime (it spawns tasks via `tauri::async_runtime`).
    pub fn start(
        git: GitRunner,
        store: Arc<dyn RepoStore>,
        cfg: WatchConfig,
    ) -> (Self, mpsc::Receiver<WorktreeChanged>) {
        let (hub, events, _repo_events) = Self::start_with_repo_events(git, store, cfg);
        (hub, events)
    }

    /// Like [`WatchHub::start`], plus the stream of [`RepositoryChanged`] events:
    /// one whenever a registered repository's set of worktrees changed (a linked
    /// worktree added, removed or pruned) after the startup baseline.
    pub fn start_with_repo_events(
        git: GitRunner,
        store: Arc<dyn RepoStore>,
        cfg: WatchConfig,
    ) -> (
        Self,
        mpsc::Receiver<WorktreeChanged>,
        mpsc::UnboundedReceiver<RepositoryChanged>,
    ) {
        let (raw, raw_rx) = raw_channel(cfg.debounce.queue_capacity);
        // Debounced events: bounded, async on both sides, so backpressure is
        // fine; at most a few per second leave the debouncer.
        let (out_tx, out_rx) = mpsc::channel(256);
        // Unbounded: messages are tiny, produced only for non-ignored
        // directory events, ignore-file/index changes and watch errors (at the
        // pace of the user's or agent's actions), and drained every 300 ms.
        let (maint, maint_rx) = mpsc::unbounded_channel();
        let (repo_events, repo_events_rx) = mpsc::unbounded_channel();

        let registry = Arc::new(RwLock::new(Registry::default()));
        let router = Router {
            registry: Arc::clone(&registry),
            raw: raw.clone(),
            maint: maint.clone(),
        };
        let native = if cfg.force_polling {
            None
        } else {
            // ONE watcher for all repositories. Each inotify instance is a file
            // descriptor plus a thread, and `fs.inotify.max_user_instances` is
            // often only 128 per user (shared with every other app), so one per
            // worktree would not scale to an agent fleet.
            //
            // `follow_symlinks(false)`: a symlink inside a checkout may point
            // anywhere (`~`, `/`). Following it would watch, and report on,
            // directories outside the registered checkout, and watching one
            // directory under two names confuses which path inotify reports.
            let config = notify::Config::default().with_follow_symlinks(false);
            match RecommendedWatcher::new(router.clone(), config) {
                Ok(w) => Some(w),
                Err(e) => {
                    tracing::warn!(%e, "cannot create the OS file watcher; polling instead");
                    None
                }
            }
        };

        let inner = Arc::new(Inner {
            git,
            store,
            registry,
            watchers: Mutex::new(Watchers {
                router,
                poll_interval: cfg.poll_interval,
                native,
                poll: None,
                watched: HashMap::new(),
            }),
            raw,
            maint,
            repo_events,
            sync_state: tokio::sync::Mutex::new(SyncState::default()),
        });

        let tasks = vec![
            async_runtime::spawn(run_debouncer(raw_rx, out_tx, cfg.debounce)),
            // A `Weak` so the task does not keep the hub alive by itself.
            async_runtime::spawn(maintenance_loop(
                Arc::downgrade(&inner),
                maint_rx,
                cfg.reconcile_every,
            )),
        ];
        (Self { inner, tasks }, out_rx, repo_events_rx)
    }

    /// Re-read the registered repositories from the store and sync to them.
    pub async fn refresh(&self) {
        self.inner.refresh().await;
    }

    /// Watch exactly the worktrees of `repos` (adding and removing watches as
    /// needed). The periodic reconcile re-reads the store, so pass the store's
    /// current content.
    pub async fn sync(&self, repos: &[RepoRecord]) {
        self.inner.sync(repos).await;
    }

    /// A handle that makes the hub re-read the store soon (within ~300 ms).
    pub fn resync_handle(&self) -> ResyncHandle {
        let tx = self.inner.maint.clone();
        ResyncHandle::new(move || {
            // Err only once the hub is gone; nothing to resync then.
            let _ = tx.send(Maint::Resync);
        })
    }

    /// Ids of the worktrees currently watched (for diagnostics and tests).
    pub fn worktree_ids(&self) -> Vec<WorktreeId> {
        read(&self.inner.registry)
            .worktrees
            .iter()
            .map(|w| w.id.clone())
            .collect()
    }

    /// Roots currently polled instead of watched natively (diagnostics, tests).
    pub fn polled_roots(&self) -> Vec<PathBuf> {
        lock(&self.inner.watchers)
            .watched
            .iter()
            .filter(|(_, b)| **b == Backend::Poll)
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// Feed the hub a "watch limit reached" error for `path`, exactly as the
    /// OS watcher would report it. The real limit cannot be exhausted safely in
    /// a test, so tests use this to exercise the fallback.
    #[doc(hidden)]
    pub fn simulate_watch_limit(&self, path: &Path) {
        let mut router = lock(&self.inner.watchers).router.clone();
        let err = notify::Error::new(notify::ErrorKind::MaxFilesWatch).add_path(path.to_owned());
        notify::EventHandler::handle_event(&mut router, Err(err));
    }
}

impl Drop for WatchHub {
    fn drop(&mut self) {
        // The tasks would end on their own once every channel sender is gone,
        // but the notify thread holds senders until `Inner` drops; stop them now.
        for t in &self.tasks {
            t.abort();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // The watcher bookkeeping stays usable after a panic elsewhere; at worst a
    // watch is missing and the next reconcile adds it.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read<T>(l: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(PoisonError::into_inner)
}

async fn maintenance_loop(
    inner: Weak<Inner>,
    mut rx: mpsc::UnboundedReceiver<Maint>,
    every: Duration,
) {
    let mut tick = tokio::time::interval(every);
    // If a sync took longer than the period, don't fire a burst to catch up.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let mut batch = Vec::new();
        let mut periodic = false;
        // The first tick completes immediately: that is the initial sync.
        tokio::select! {
            _ = tick.tick() => periodic = true,
            msg = rx.recv() => {
                let Some(msg) = msg else { return };
                batch.push(msg);
                tokio::time::sleep(SETTLE).await;
                while let Ok(msg) = rx.try_recv() {
                    batch.push(msg);
                }
            }
        }
        let Some(inner) = inner.upgrade() else {
            return; // hub dropped
        };
        inner.maintain(periodic, batch).await;
    }
}

impl Inner {
    async fn maintain(self: &Arc<Self>, periodic: bool, batch: Vec<Maint>) {
        let mut resync = periodic;
        let mut rebuild = HashSet::new();
        let mut tracked = HashSet::new();
        for msg in batch {
            match msg {
                Maint::Resync => resync = true,
                Maint::RebuildIgnore(id) => {
                    rebuild.insert(id);
                }
                Maint::RefreshTracked(id) => {
                    tracked.insert(id);
                }
                Maint::WatchLimit(path) => self.switch_to_polling(path).await,
                Maint::RootGone(root) => {
                    self.forget_watch(root).await;
                    resync = true;
                }
            }
        }
        if resync {
            self.refresh().await;
        }
        for id in &rebuild {
            self.rebuild_ignore(id).await;
        }
        for id in tracked.difference(&rebuild) {
            if let Some(spec) = self.spec(id) {
                let set = tracked_ignored(&self.git, &spec.root).await;
                *spec
                    .tracked_ignored
                    .write()
                    .unwrap_or_else(PoisonError::into_inner) = set;
            }
        }
    }

    fn spec(&self, id: &WorktreeId) -> Option<Arc<WorktreeSpec>> {
        read(&self.registry)
            .worktrees
            .iter()
            .find(|w| &w.id == id)
            .cloned()
    }

    /// Run `f` on the watchers in a blocking thread: adding a recursive native
    /// watch or a polled root walks the whole tree.
    async fn with_watchers<R: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&mut Watchers) -> R + Send + 'static,
    ) -> Option<R> {
        let inner = Arc::clone(self);
        match async_runtime::spawn_blocking(move || f(&mut lock(&inner.watchers))).await {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(%e, "watcher: updating watches failed");
                None
            }
        }
    }

    async fn refresh(self: &Arc<Self>) {
        match self.store.list() {
            Ok(repos) => self.sync(&repos).await,
            Err(e) => tracing::warn!(%e, "watcher: cannot list repositories"),
        }
    }

    async fn sync(self: &Arc<Self>, repos: &[RepoRecord]) {
        let mut st = self.sync_state.lock().await;
        let initial = !st.initial_done;
        let mut old = std::mem::take(&mut st.known);
        // Which worktrees each repository had before this sync, to notice a
        // linked worktree being added or removed.
        let before = members_by_repo(&old);
        let mut known: HashMap<WorktreeId, Known> = HashMap::new();
        let mut repo_specs = Vec::new();
        let mut changed: HashSet<WorktreeId> = HashSet::new();

        for rec in repos {
            repo_specs.push(RepoSpec {
                id: rec.id.clone(),
                common_dir: rec.common_dir.clone(),
            });
            // `open_repo(common_dir)` would fail (rev-parse inside `.git` has no
            // toplevel), and `list_worktrees` only needs the common dir anyway.
            let info = RepoInfo {
                toplevel: Some(rec.root_path.clone()),
                common_dir: rec.common_dir.clone(),
                git_dir: rec.common_dir.clone(),
                is_bare: false,
            };
            let entries = match git::list_worktrees(&self.git, &info).await {
                Ok(entries) => entries,
                Err(e) => {
                    // Transient (timeout, repo mid-operation) or the repo is gone.
                    // Keep watching what we had; the next reconcile retries.
                    tracing::warn!(repo = %rec.id.0, %e, "watcher: cannot list worktrees");
                    let keep: Vec<_> = old
                        .iter()
                        .filter(|(_, k)| k.spec.repo == rec.id)
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in keep {
                        if let Some(k) = old.remove(&id) {
                            known.insert(id, k);
                        }
                    }
                    continue;
                }
            };

            for entry in entries {
                let root = if entry.is_main {
                    // Trust the registered root for the main worktree: it was
                    // resolved and verified when the repository was added. For a
                    // submodule or a `--separate-git-dir` checkout, git derives
                    // the main entry from the git directory and can name the
                    // metadata directory instead of the checkout.
                    rec.root_path.clone()
                } else if entry.is_bare || entry.prunable.is_some() {
                    continue;
                } else {
                    entry.path.clone()
                };
                // A locked worktree can be missing without being prunable.
                if !root.is_dir() {
                    continue;
                }
                let id = WorktreeId(git::worktree_id(&rec.common_dir, &root));
                if known.contains_key(&id) {
                    continue; // same checkout listed twice (registered via a linked root)
                }
                if let Some(mut k) = old.remove(&id) {
                    let stamp = stamp(&k.spec.git_dir, &rec.common_dir).await;
                    if stamp != k.stamp {
                        changed.insert(id.clone());
                    }
                    k.stamp = stamp;
                    known.insert(id, k);
                    continue;
                }
                let Some(spec) = self.new_spec(id.clone(), rec, root, entry.is_main).await else {
                    continue;
                };
                let stamp = stamp(&spec.git_dir, &rec.common_dir).await;
                if !initial {
                    tracing::info!(worktree = %id.0, path = %spec.root.display(), "new worktree");
                    changed.insert(id.clone());
                }
                known.insert(id, Known { spec, stamp });
            }
        }
        for k in old.values() {
            tracing::info!(worktree = %k.spec.id.0, "worktree no longer watched");
        }

        // Publish the routing table BEFORE adding watches, so the first events
        // from a new watch already find their worktree.
        let worktrees: Vec<_> = known.values().map(|k| Arc::clone(&k.spec)).collect();
        let mut desired: Vec<PathBuf> = worktrees.iter().map(|w| w.root.clone()).collect();
        desired.extend(repo_specs.iter().map(|r| r.common_dir.clone()));
        *self
            .registry
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Registry {
            repos: repo_specs,
            worktrees,
        };
        st.known = known;
        // Membership: only for repositories we already knew (a repository that
        // was just registered is not "changed", the UI is adding it itself), and
        // only after the baseline. A repository we could not list this time kept
        // its old entries, so it compares equal and stays quiet.
        let after = members_by_repo(&st.known);
        let membership_changed: Vec<RepoId> = repos
            .iter()
            .filter(|rec| {
                !initial
                    && before
                        .get(&rec.id)
                        .is_some_and(|was| Some(was) != after.get(&rec.id))
            })
            .map(|rec| rec.id.clone())
            .collect();

        // Still holding the sync lock: two overlapping syncs (the periodic one
        // and a resync) must apply their watch sets in the same order as their
        // registries, or an older set could win.
        let installed = self
            .with_watchers(move |w| w.set_watched(covering_set(desired)))
            .await
            .unwrap_or_default();
        if !initial {
            // A watch (re)installed after the baseline: whatever happened in
            // that tree before it existed went unseen.
            changed.extend(self.worktrees_under(&installed));
        }
        st.initial_done = true;
        drop(st);

        for id in changed {
            self.raw.send(RawChange::refetch_all(id));
        }
        for repo_id in membership_changed {
            tracing::info!(repo = %repo_id.0, "worktrees of the repository changed");
            // Err only when nobody listens (`start` drops the receiver).
            let _ = self.repo_events.send(RepositoryChanged { repo_id });
        }
    }

    /// Ids of the worktrees whose root is inside one of `roots`.
    fn worktrees_under(&self, roots: &[PathBuf]) -> Vec<WorktreeId> {
        read(&self.registry)
            .worktrees
            .iter()
            .filter(|w| roots.iter().any(|r| w.root.starts_with(r)))
            .map(|w| w.id.clone())
            .collect()
    }

    async fn new_spec(
        &self,
        id: WorktreeId,
        rec: &RepoRecord,
        root: PathBuf,
        is_main: bool,
    ) -> Option<Arc<WorktreeSpec>> {
        // The main worktree's git dir is the common dir. A linked worktree has
        // a `.git` FILE pointing at `<common>/worktrees/<name>`; ask git rather
        // than parse it. Done once per worktree, then cached in `known`.
        let git_dir = if is_main {
            rec.common_dir.clone()
        } else {
            match git::open_repo(&self.git, &root).await {
                Ok(info) => info.git_dir,
                Err(e) => {
                    tracing::warn!(path = %root.display(), %e, "watcher: cannot open worktree");
                    return None;
                }
            }
        };
        let ignore = build_rules(&root, &rec.common_dir).await;
        let tracked = tracked_ignored(&self.git, &root).await;
        Some(Arc::new(WorktreeSpec {
            id,
            repo: rec.id.clone(),
            root,
            git_dir,
            common_dir: rec.common_dir.clone(),
            ignore: RwLock::new(ignore),
            tracked_ignored: RwLock::new(tracked),
        }))
    }

    async fn rebuild_ignore(&self, id: &WorktreeId) {
        let Some(spec) = self.spec(id) else { return };
        let rules = build_rules(&spec.root, &spec.common_dir).await;
        let tracked = tracked_ignored(&self.git, &spec.root).await;
        let changed = {
            let mut cur = spec.ignore.write().unwrap_or_else(PoisonError::into_inner);
            let changed = *cur != rules;
            *cur = rules;
            changed
        };
        *spec
            .tracked_ignored
            .write()
            .unwrap_or_else(PoisonError::into_inner) = tracked;
        // The rules decide what the UI should show too (git status hides
        // ignored files): if they changed, whatever was hidden or shown may
        // differ now. Most rebuilds (a directory created, a file renamed) find
        // the same rules and stay silent.
        if changed {
            self.raw.send(RawChange::refetch_all(id.clone()));
        }
    }

    /// A watch under `path` hit the kernel limit after it was installed (a new
    /// subdirectory could not be watched): that subtree is blind. Poll the
    /// whole root it belongs to instead.
    async fn switch_to_polling(self: &Arc<Self>, path: PathBuf) {
        let switched = self
            .with_watchers(move |w| w.switch_root_to_polling(&path))
            .await
            .flatten();
        if let Some(root) = switched {
            for id in self.worktrees_under(std::slice::from_ref(&root)) {
                self.raw.send(RawChange::refetch_all(id));
            }
        }
    }

    /// `root` was deleted or moved: inotify dropped its watch. Forget it, so
    /// the next sync installs a fresh one if the directory exists again (for
    /// example `git worktree remove` + `git worktree add` at the same path).
    async fn forget_watch(self: &Arc<Self>, root: PathBuf) {
        self.with_watchers(move |w| w.forget(&root)).await;
    }
}

/// Walking the tree for `.gitignore` files is blocking I/O: keep it off the
/// async worker threads.
async fn build_rules(root: &Path, common_dir: &Path) -> IgnoreRules {
    let (root, common) = (root.to_owned(), common_dir.to_owned());
    async_runtime::spawn_blocking(move || IgnoreRules::build(&root, &common))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(%e, "watcher: building ignore rules failed");
            IgnoreRules::default()
        })
}

/// Tracked files that the ignore rules match, plus their parent directories,
/// relative to `root`. `ls-files` only reads the index; it never writes it.
async fn tracked_ignored(git: &GitRunner, root: &Path) -> HashSet<PathBuf> {
    // -c: tracked ("cached") files; -i --exclude-standard: only those matched
    // by .gitignore / info/exclude / core.excludesFile; -z: NUL-separated raw
    // bytes, so any file name survives.
    let out = match git
        .run(root, ["ls-files", "-z", "-c", "-i", "--exclude-standard"])
        .await
    {
        Ok(out) => out,
        Err(e) => {
            tracing::debug!(path = %root.display(), %e, "watcher: cannot list tracked ignored files");
            return HashSet::new();
        }
    };
    let mut set = HashSet::new();
    for raw in out.split(|&b| b == 0).filter(|s| !s.is_empty()) {
        let path = git::oid::path_from_bytes(raw);
        // Parents too: an event for `dist/` must not be dropped when
        // `dist/snapshot.txt` is tracked.
        let mut p: &Path = &path;
        while let Some(parent) = p.parent() {
            if parent.as_os_str().is_empty() {
                break;
            }
            set.insert(parent.to_owned());
            p = parent;
        }
        set.insert(path);
    }
    set
}

/// Recursive watches must not overlap: the main worktree's root already covers
/// its `.git/` (the common dir) and any worktree nested inside it. Watching a
/// directory twice on one inotify instance would double the events and make
/// "unwatch one of them" remove the other's watch too. Keep only the
/// outermost directories.
fn covering_set(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    // Shorter paths first, so an ancestor is kept before its descendants.
    dirs.sort_by_key(|p| p.components().count());
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if !out.iter().any(|kept| d.starts_with(kept)) {
            out.push(d);
        }
    }
    out
}

/// See [`Stamp`]. Reads only metadata (and the 40-byte HEAD file); missing
/// files give `None`, which compares fine.
async fn stamp(git_dir: &Path, common_dir: &Path) -> Stamp {
    async fn mtime(p: &Path) -> Option<SystemTime> {
        tokio::fs::metadata(p).await.ok()?.modified().ok()
    }
    let mut s = vec![
        mtime(&git_dir.join("HEAD")).await,
        mtime(&git_dir.join("index")).await,
    ];
    // A commit moves the branch ref, not HEAD (HEAD still says "ref: refs/heads/x").
    if let Ok(head) = tokio::fs::read_to_string(git_dir.join("HEAD")).await
        && let Some(r) = head.trim().strip_prefix("ref: ")
        // Never follow something odd like `ref: ../../etc`.
        && r.starts_with("refs/")
        && !r.contains("..")
    {
        s.push(mtime(&common_dir.join(r)).await);
        s.push(mtime(&common_dir.join("packed-refs")).await);
    }
    s
}

/// Did adding a watch fail because the kernel's watch limit was reached?
/// notify reports inotify's ENOSPC as `MaxFilesWatch`; older paths may still
/// surface it as a raw I/O error 28 (ENOSPC on Linux and macOS).
pub fn is_watch_limit(e: &notify::Error) -> bool {
    match &e.kind {
        notify::ErrorKind::MaxFilesWatch => true,
        notify::ErrorKind::Io(io) => io.raw_os_error() == Some(28),
        _ => false,
    }
}

impl Watchers {
    /// Make the watched set equal `desired`; returns the roots newly installed.
    fn set_watched(&mut self, desired: Vec<PathBuf>) -> Vec<PathBuf> {
        let stale: Vec<PathBuf> = self
            .watched
            .keys()
            .filter(|p| !desired.contains(p))
            .cloned()
            .collect();
        for p in stale {
            self.unwatch(&p);
        }
        let mut installed = Vec::new();
        for p in desired {
            if !self.watched.contains_key(&p) && self.watch(p.clone()) {
                installed.push(p);
            }
        }
        installed
    }

    fn unwatch(&mut self, p: &Path) {
        match self.watched.remove(p) {
            Some(Backend::Native) => {
                // Fails if the directory was deleted (the kernel dropped the
                // watch already): nothing left to undo.
                if let Some(Err(e)) = self.native.as_mut().map(|w| w.unwatch(p)) {
                    tracing::debug!(path = %p.display(), %e, "unwatch failed");
                }
            }
            Some(Backend::Poll) => {
                if let Some(poll) = &self.poll {
                    poll.unwatch(p);
                }
            }
            None => {}
        }
    }

    /// Drop our record of a dead watch (see `Inner::forget_watch`).
    fn forget(&mut self, root: &Path) {
        if self.watched.contains_key(root) {
            tracing::info!(path = %root.display(), "watched root disappeared");
            self.unwatch(root);
        }
    }

    /// Move the native watch covering `path` to the poller. Returns the root.
    fn switch_root_to_polling(&mut self, path: &Path) -> Option<PathBuf> {
        let root = self
            .watched
            .iter()
            .filter(|(r, b)| **b == Backend::Native && path.starts_with(r))
            .map(|(r, _)| r.clone())
            .max_by_key(|r| r.components().count())?;
        tracing::warn!(path = %root.display(), "{WATCH_LIMIT_HINT}");
        self.unwatch(&root);
        self.poll_root(root.clone()).then_some(root)
    }

    /// Install a watch on `p`; `true` on success.
    fn watch(&mut self, p: PathBuf) -> bool {
        if let Some(native) = self.native.as_mut() {
            // Recursive: notify adds one inotify watch per directory under `p`
            // (inotify itself is not recursive) and adds new ones as
            // directories are created.
            match native.watch(&p, RecursiveMode::Recursive) {
                Ok(()) => {
                    tracing::info!(path = %p.display(), "watching");
                    self.watched.insert(p, Backend::Native);
                    return true;
                }
                Err(e) if is_watch_limit(&e) => {
                    tracing::warn!(path = %p.display(), "{WATCH_LIMIT_HINT}");
                    // Drop the directories that did get a watch before the
                    // limit hit, so they are not reported twice.
                    let _ = native.unwatch(&p);
                }
                Err(e) => {
                    // Typically the directory vanished between listing and
                    // watching. The next reconcile tries again.
                    tracing::warn!(path = %p.display(), %e, "cannot watch");
                    return false;
                }
            }
        }
        self.poll_root(p)
    }

    fn poll_root(&mut self, p: PathBuf) -> bool {
        if self.poll.is_none() {
            match Poller::new(self.router.clone(), self.poll_interval) {
                Ok(poller) => self.poll = Some(poller),
                Err(e) => {
                    tracing::warn!(%e, "cannot start the polling thread");
                    return false;
                }
            }
        }
        let Some(poll) = &self.poll else { return false };
        tracing::info!(path = %p.display(), interval = ?self.poll_interval, "polling");
        poll.watch(p.clone());
        self.watched.insert(p, Backend::Poll);
        true
    }
}

/// The notify callback. Runs on notify's own thread (and the poller's), NOT on
/// a tokio worker: it is plain synchronous code, so it may block briefly (stat
/// calls) without stalling the async runtime. It talks to the async side only
/// through channels whose `send` never blocks and never `.await`s, which is
/// what makes them safe to call from any thread. Anything that needs git or a
/// tree walk is handed to the maintenance task as a [`Maint`] message.
#[derive(Clone)]
struct Router {
    registry: Arc<RwLock<Registry>>,
    raw: RawSender,
    maint: mpsc::UnboundedSender<Maint>,
}

impl notify::EventHandler for Router {
    fn handle_event(&mut self, res: notify::Result<Event>) {
        // Never panic here: a panic would kill notify's thread and silently
        // stop all watching. Every fallible step logs and moves on.
        let event = match res {
            Ok(event) => event,
            Err(e) if is_watch_limit(&e) => {
                // A new directory inside a watched tree could not be watched.
                if e.paths.is_empty() {
                    tracing::warn!("{WATCH_LIMIT_HINT} (path unknown; not switched)");
                }
                for p in e.paths {
                    let _ = self.maint.send(Maint::WatchLimit(p));
                }
                return;
            }
            Err(e) => {
                tracing::debug!(%e, "watch error");
                return;
            }
        };
        if event.need_rescan() {
            // The kernel's event queue overflowed and events were dropped: we
            // cannot know what changed, so everything is "refetch all".
            for w in &read(&self.registry).worktrees {
                self.raw.send(RawChange::refetch_all(w.id.clone()));
            }
            return;
        }
        if !is_content_event(&event.kind) {
            return;
        }

        // A matched rename. Its two halves were already routed as separate
        // `From` and `To` events; this pairing tells us what the source was.
        // If the destination is a file, the source was a file too, so the
        // "unknown kind" of the `From` half is settled.
        if let EventKind::Modify(ModifyKind::Name(RenameMode::Both)) = event.kind {
            if let [from, to] = event.paths.as_slice()
                && !to.is_dir()
                && let Some((worktree, rel)) = read(&self.registry).worktree_rel(from)
            {
                self.raw.send(RawChange {
                    worktree,
                    change: Change::Settled(rel),
                });
            }
            return;
        }

        // inotify flags directory events (IN_ISDIR), which notify reports as
        // `Folder`; for other kinds a stat tells us (false if already deleted).
        let folder_kind = matches!(
            event.kind,
            EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder)
        );
        let removed = matches!(event.kind, EventKind::Remove(_));
        let rename_from = matches!(
            event.kind,
            EventKind::Modify(ModifyKind::Name(RenameMode::From))
        );
        // Route under the read lock, act after releasing it.
        let routed: Vec<Routed> = {
            let reg = read(&self.registry);
            event
                .paths
                .iter()
                .flat_map(|p| {
                    let exists = p.symlink_metadata().is_ok();
                    let info = PathInfo {
                        is_dir: folder_kind || p.is_dir(),
                        removed: removed || (rename_from && !exists),
                        moved_away: rename_from && !exists,
                    };
                    reg.route(p, info)
                })
                .collect()
        };
        for r in routed {
            // `send` fails only once the hub shut down; nothing to do then.
            let _ = match r {
                Routed::Change(c) => {
                    self.raw.send(c);
                    Ok(())
                }
                Routed::Resync => self.maint.send(Maint::Resync),
                Routed::RebuildIgnore(id) => self.maint.send(Maint::RebuildIgnore(id)),
                Routed::RefreshTracked(id) => self.maint.send(Maint::RefreshTracked(id)),
                Routed::RootGone(root) => self.maint.send(Maint::RootGone(root)),
            };
        }
    }
}

/// Reading a file is not a change: `Access` events (open, close-without-write)
/// fire every time git (including our own `git status`) or an editor reads a
/// file, and an atime update is the same thing. Everything else (create,
/// modify, rename, remove, and unknown kinds) counts.
fn is_content_event(kind: &EventKind) -> bool {
    !matches!(
        kind,
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covering_set_drops_nested_dirs() {
        let mut got = covering_set(vec![
            "/c/app/.git".into(),
            "/c/app-feat".into(),
            "/c/app".into(),
            "/c/app/.wt/x".into(),
            "/c/app".into(), // duplicate
        ]);
        got.sort();
        assert_eq!(got, [PathBuf::from("/c/app"), PathBuf::from("/c/app-feat")]);
    }

    #[test]
    fn watch_limit_detection() {
        assert!(is_watch_limit(&notify::Error::new(
            notify::ErrorKind::MaxFilesWatch
        )));
        assert!(is_watch_limit(&notify::Error::io(
            std::io::Error::from_raw_os_error(28)
        )));
        assert!(!is_watch_limit(&notify::Error::path_not_found()));
        assert!(!is_watch_limit(&notify::Error::io(std::io::Error::other(
            "x"
        ))));
    }

    #[test]
    fn access_events_are_not_changes() {
        use notify::event::AccessKind;
        assert!(!is_content_event(&EventKind::Access(AccessKind::Any)));
        assert!(!is_content_event(&EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::AccessTime
        ))));
        assert!(is_content_event(&EventKind::Create(CreateKind::File)));
        assert!(is_content_event(&EventKind::Modify(ModifyKind::Any)));
        assert!(is_content_event(&EventKind::Any));
    }
}
