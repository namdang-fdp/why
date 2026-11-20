//! Keeping `hook-allowlist.json` equal to "the registered repositories and
//! their worktrees" (decision 22), so the hook can drop everything else even
//! while the app is not running.
//!
//! # Two speeds
//!
//! * **Registering and unregistering are synchronous.** [`AllowlistingStore`]
//!   wraps the `RepoStore`: `remove` rewrites the file (without the repository)
//!   BEFORE it touches the registry, and fails the removal if the file cannot be
//!   written, so "unregistered" never coexists with "still captured", even if
//!   WHY is closed one millisecond later. `insert` adds the repository's root
//!   to the file at once (no git involved) and wakes the refresh task.
//! * **Worktrees come from git, in the background.** The refresh task lists each
//!   repository's worktrees from its registered `common_dir` (so linked
//!   worktrees keep capturing even if the main checkout is gone) when woken
//!   (a repository was added, or the watcher emitted `repository-changed`
//!   because a linked worktree appeared or vanished) and, as a safety net,
//!   every `refresh` (15 s). Events from a brand-new worktree in that gap are
//!   dropped by the hook; that is the price of an allowlist the hook can read in
//!   microseconds without running git.
//!
//! The background refresh must never resurrect a repository that was removed
//! while git was running: every synchronous change bumps a generation counter,
//! and a refresh computed against an older generation is discarded.
//!
//! If the store cannot be listed (error) the old file is kept: an empty
//! allowlist would silently switch capture off.

use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use tokio::sync::{Notify, watch};
use why_wire::Allowlist;

use crate::contracts::{RepoId, WorktreeId};
use crate::git::{GitRunner, RepoInfo, list_worktrees};
use crate::store::{InsertOutcome, RepoRecord, RepoStore, StoreError};

/// Cadence of the background refresh.
#[derive(Debug, Clone, Copy)]
pub struct SyncTiming {
    /// Safety-net interval: worktrees are re-read from git at least this often.
    pub refresh: Duration,
}

impl Default for SyncTiming {
    fn default() -> Self {
        Self {
            refresh: Duration::from_secs(15),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The roots one registered repository contributes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoRoots {
    /// The registered main checkout.
    pub root: String,
    /// Every worktree git lists for the repository (the main one included).
    pub worktrees: Vec<String>,
}

/// Worktree roots of one repository, listed from its registered `common_dir`.
///
/// Not from the main checkout (`open_repo(root)`): a repository may keep its
/// git directory elsewhere (`--separate-git-dir`) and the main checkout may be
/// deleted while linked worktrees and their shared metadata live on; the
/// registry still knows the common dir. A repository whose git call fails
/// (directory gone, git missing) contributes only its registered root:
/// forgetting it would stop capture for a repository the user did register.
async fn roots_of(git: &GitRunner, rec: &RepoRecord) -> RepoRoots {
    let as_text = |p: &Path| p.to_str().map(str::to_owned);
    let mut out = RepoRoots {
        root: as_text(&rec.root_path).unwrap_or_default(),
        worktrees: Vec::new(),
    };
    // `list_worktrees` only reads `common_dir`; the other fields are not used.
    let info = RepoInfo {
        toplevel: Some(rec.root_path.clone()),
        common_dir: rec.common_dir.clone(),
        git_dir: rec.common_dir.clone(),
        is_bare: false,
    };
    if let Ok(entries) = list_worktrees(git, &info).await {
        // Bare entries have no checkout; prunable ones point at a directory
        // that is gone.
        out.worktrees = entries
            .iter()
            .filter(|e| !e.is_bare && e.prunable.is_none())
            .filter_map(|e| as_text(&e.path))
            .collect();
    }
    out
}

/// Roots of every registered repository, keyed by repository id.
pub async fn compute_roots(
    git: &GitRunner,
    repos: &dyn RepoStore,
) -> Result<HashMap<String, RepoRoots>, StoreError> {
    let mut out = HashMap::new();
    for rec in repos.list()? {
        out.insert(rec.id.0.clone(), roots_of(git, &rec).await);
    }
    Ok(out)
}

fn build(repos: &HashMap<String, RepoRoots>) -> Allowlist {
    let mut repo_roots = Vec::new();
    let mut worktree_roots = Vec::new();
    for r in repos.values() {
        if r.root.is_empty() {
            continue;
        }
        repo_roots.push(r.root.clone());
        worktree_roots.push(r.root.clone());
        worktree_roots.extend(r.worktrees.iter().cloned());
    }
    Allowlist::new(now_ms(), repo_roots, worktree_roots)
}

/// The allowlist the registry implies right now.
pub async fn compute(git: &GitRunner, repos: &dyn RepoStore) -> Result<Allowlist, StoreError> {
    Ok(build(&compute_roots(git, repos).await?))
}

#[derive(Default)]
struct Inner {
    /// Bumped by every synchronous change; see the module docs.
    generation: u64,
    repos: HashMap<String, RepoRoots>,
}

/// What the allowlist file should say, and the one place that writes it.
pub struct AllowlistState {
    /// `None`: no data dir (the collector is disabled); nothing is written.
    path: Option<PathBuf>,
    wake: Arc<Notify>,
    inner: Mutex<Inner>,
}

impl AllowlistState {
    /// State seeded with the registered roots (no git, nothing written yet:
    /// the first background refresh fills in worktrees and writes the file).
    pub fn new(path: Option<PathBuf>, initial: &[RepoRecord]) -> Arc<Self> {
        let repos = initial
            .iter()
            .map(|r| {
                let root = r.root_path.to_str().unwrap_or_default().to_owned();
                (
                    r.id.0.clone(),
                    RepoRoots {
                        root,
                        worktrees: Vec::new(),
                    },
                )
            })
            .collect();
        Arc::new(Self {
            path,
            wake: Arc::new(Notify::new()),
            inner: Mutex::new(Inner {
                generation: 0,
                repos,
            }),
        })
    }

    /// Wake the background refresh (a worktree appeared, a repository was added).
    pub fn wake(&self) -> Arc<Notify> {
        Arc::clone(&self.wake)
    }

    fn lock(&self) -> io::Result<std::sync::MutexGuard<'_, Inner>> {
        self.inner
            .lock()
            .map_err(|_| io::Error::other("allowlist state lock poisoned"))
    }

    pub fn generation(&self) -> u64 {
        self.lock().map(|i| i.generation).unwrap_or(0)
    }

    /// Write the file for `inner`'s content (if it changed).
    fn write(&self, inner: &Inner) -> io::Result<bool> {
        match &self.path {
            Some(path) => sync_file(path, &build(&inner.repos)),
            None => Ok(false),
        }
    }

    /// A repository was registered: allow its root now, then ask for worktrees.
    pub fn repo_added(&self, rec: &RepoRecord) -> io::Result<()> {
        {
            let mut inner = self.lock()?;
            inner.generation += 1;
            let root = rec.root_path.to_str().unwrap_or_default().to_owned();
            inner
                .repos
                .entry(rec.id.0.clone())
                .or_insert_with(|| RepoRoots {
                    root,
                    worktrees: Vec::new(),
                });
            self.write(&inner)?;
        }
        self.wake.notify_one();
        Ok(())
    }

    /// A repository is about to be unregistered: revoke it on disk. An error
    /// means the revocation is NOT durable and the caller must not proceed.
    pub fn repo_removed(&self, id: &RepoId) -> io::Result<()> {
        let mut inner = self.lock()?;
        inner.generation += 1;
        inner.repos.remove(&id.0);
        self.write(&inner)?;
        Ok(())
    }

    /// Apply a background refresh computed while the state was at generation
    /// `computed_at`. Discarded (returns `false`) when a registration change
    /// happened meanwhile: the next refresh recomputes.
    pub fn replace(&self, computed: HashMap<String, RepoRoots>, computed_at: u64) -> bool {
        let Ok(mut inner) = self.lock() else {
            return false;
        };
        if inner.generation != computed_at {
            return false;
        }
        inner.repos = computed;
        match self.write(&inner) {
            Ok(true) => tracing::info!(repos = inner.repos.len(), "wrote the hook allowlist"),
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %e, "cannot write the hook allowlist"),
        }
        true
    }
}

/// A `RepoStore` decorator that keeps the allowlist file in step with the
/// registry SYNCHRONOUSLY (see the module docs). Wrap the store with
/// [`wrap_store`] in `lib.rs`.
pub struct AllowlistingStore {
    inner: Arc<dyn RepoStore>,
    state: Arc<AllowlistState>,
}

/// Wrap `repos` so registration changes update the allowlist at `path` before
/// they return. Returns the wrapped store and the state the collector's
/// background refresh shares.
pub fn wrap_store(
    repos: Arc<dyn RepoStore>,
    path: Option<PathBuf>,
) -> (Arc<dyn RepoStore>, Arc<AllowlistState>) {
    let initial = repos.list().unwrap_or_default();
    let state = AllowlistState::new(path, &initial);
    let store = AllowlistingStore {
        inner: repos,
        state: Arc::clone(&state),
    };
    (Arc::new(store), state)
}

impl RepoStore for AllowlistingStore {
    fn list(&self) -> Result<Vec<RepoRecord>, StoreError> {
        self.inner.list()
    }

    fn get(&self, id: &RepoId) -> Result<Option<RepoRecord>, StoreError> {
        self.inner.get(id)
    }

    fn insert(&self, rec: RepoRecord) -> Result<InsertOutcome, StoreError> {
        let outcome = self.inner.insert(rec)?;
        if let InsertOutcome::Inserted(rec) = &outcome {
            // Failing to ALLOW is safe (capture stays off until the next
            // refresh), so the registration still succeeds.
            if let Err(e) = self.state.repo_added(rec) {
                tracing::warn!(error = %e, "cannot add the repository to the hook allowlist");
            }
        }
        Ok(outcome)
    }

    fn remove(&self, id: &RepoId) -> Result<bool, StoreError> {
        // Revoke FIRST, then forget. If the revocation cannot be written the
        // removal fails (the repository stays registered and consistent with
        // the file) instead of leaving a repository that hooks still capture.
        if self.inner.get(id)?.is_some() {
            self.state
                .repo_removed(id)
                .map_err(|e| StoreError::Backend(format!("cannot revoke capture on disk: {e}")))?;
        }
        self.inner.remove(id)
    }

    fn base_override(&self, wt: &WorktreeId) -> Result<Option<String>, StoreError> {
        self.inner.base_override(wt)
    }

    fn set_base_override(&self, wt: &WorktreeId, base: Option<String>) -> Result<(), StoreError> {
        self.inner.set_base_override(wt, base)
    }

    fn trusted_filters(&self, repo: &RepoId) -> Result<HashSet<String>, StoreError> {
        self.inner.trusted_filters(repo)
    }

    fn set_trusted_filters(
        &self,
        repo: &RepoId,
        fingerprints: &[String],
    ) -> Result<bool, StoreError> {
        self.inner.set_trusted_filters(repo, fingerprints)
    }
}

/// Replace `path` with `bytes` atomically and privately (`0600`).
///
/// Write a temp file in the SAME directory, flush it to disk, then `rename`
/// over the target. `rename` within one filesystem is atomic: a hook reading at
/// any moment sees the old complete file or the new complete file, never half
/// of one (a half-written file would parse as corrupt and switch capture off).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("allowlist path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".hook-allowlist.tmp-{}", std::process::id()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Write `list` to `path` unless the file already holds the same roots.
/// Returns whether it wrote.
pub fn sync_file(path: &Path, list: &Allowlist) -> io::Result<bool> {
    if let Ok(current) = Allowlist::read_from(path)
        && current.repo_roots == list.repo_roots
        && current.worktree_roots == list.worktree_roots
    {
        return Ok(false);
    }
    write_atomic(path, &list.to_bytes())?;
    Ok(true)
}

/// Refresh worktrees from git when woken and every `timing.refresh`, until
/// shutdown. See the module docs.
pub async fn run(
    git: GitRunner,
    repos: Arc<dyn RepoStore>,
    state: Arc<AllowlistState>,
    timing: SyncTiming,
    wake: Arc<Notify>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        // Captured BEFORE reading the registry: a registration change after
        // this point makes `replace` discard what we compute.
        let generation = state.generation();
        match compute_roots(&git, &*repos).await {
            Ok(roots) => {
                if !state.replace(roots, generation) {
                    tracing::debug!("allowlist refresh discarded: the registry changed meanwhile");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot list repositories; keeping the old allowlist")
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(timing.refresh) => {}
            _ = wake.notified() => {}
            _ = shutdown.changed() => return,
        }
    }
}
