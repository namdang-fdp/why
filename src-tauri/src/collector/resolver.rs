//! [`GitWorktreeResolver`]: which registered worktree does an event's `cwd`
//! belong to?
//!
//! Never a string-prefix guess: a cwd can be a subdirectory of the worktree, a
//! linked worktree outside the main checkout, or a symlink. Git knows, so we
//! ask it (`rev-parse --show-toplevel` / `--git-common-dir`, via the app's one
//! [`GitRunner`], read-only), then match the repository by its canonical common
//! dir against the registry. There is no time-based attribution ("the agent was
//! probably in repo X a minute ago"): an event either resolves through git or
//! belongs to no worktree.
//!
//! `open_repo` only reads git's own metadata; it never reads the working tree,
//! so it needs no `ReadPermit` (filter drivers run only for status/diff).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::contracts::WorktreeId;
use crate::events::ingest::{ResolvedWorktree, WorktreeResolver};
use crate::git::{GitRunner, open_repo, worktree_id};
use crate::store::RepoStore;

/// Bound on remembered cwds. A session uses a handful of directories; the cap
/// only stops a pathological stream of distinct cwds from growing the map.
const MAX_CACHE_ENTRIES: usize = 4096;

/// What identifies the checkout metadata a cached answer was computed from: the
/// `.git` entry (directory or gitdir-pointer file) of the worktree root. If it
/// is replaced, retargeted or deleted, the answer is stale.
type GitMarker = Option<(u64, std::time::SystemTime)>;

fn git_marker(toplevel: &std::path::Path) -> GitMarker {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(toplevel.join(".git")).ok()?;
    Some((meta.ino(), meta.modified().ok()?))
}

struct Entry {
    resolved: ResolvedWorktree,
    marker: GitMarker,
}

#[derive(Default)]
struct Cache {
    /// The registered repository ids the entries were computed for. A change
    /// (a repository added or removed) invalidates everything: an entry for a
    /// repository that was just unregistered must not keep attributing events.
    registry: Vec<String>,
    /// Keyed by the CANONICAL cwd, never by the spelling the event used: a
    /// symlink retargeted to another worktree must not keep the old answer.
    by_cwd: HashMap<PathBuf, Entry>,
}

pub struct GitWorktreeResolver {
    git: GitRunner,
    repos: Arc<dyn RepoStore>,
    cache: Mutex<Cache>,
}

impl GitWorktreeResolver {
    pub fn new(git: GitRunner, repos: Arc<dyn RepoStore>) -> Self {
        Self {
            git,
            repos,
            cache: Mutex::new(Cache::default()),
        }
    }

    /// Entries currently cached (for tests).
    pub fn cached(&self) -> usize {
        self.cache
            .lock()
            .map(|c| c.by_cwd.len())
            .unwrap_or_default()
    }

    /// `Some(hit)` if `canonical` is cached for this registry and its checkout
    /// metadata is unchanged; clears the cache when the registry changed. A
    /// poisoned lock just means "no cache".
    fn lookup(&self, registry: &[String], canonical: &std::path::Path) -> Option<ResolvedWorktree> {
        let mut cache = self.cache.lock().ok()?;
        if cache.registry != registry {
            cache.registry = registry.to_vec();
            cache.by_cwd.clear();
        }
        let entry = cache.by_cwd.get(canonical)?;
        if git_marker(&entry.resolved.root) == entry.marker {
            return Some(entry.resolved.clone());
        }
        cache.by_cwd.remove(canonical);
        None
    }

    fn remember(&self, registry: &[String], canonical: PathBuf, wt: &ResolvedWorktree) {
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        // The registry may have changed while git ran: then the answer may be
        // stale, and the next lookup recomputes it.
        if cache.registry != registry {
            return;
        }
        if cache.by_cwd.len() >= MAX_CACHE_ENTRIES {
            cache.by_cwd.clear();
        }
        let marker = git_marker(&wt.root);
        cache.by_cwd.insert(
            canonical,
            Entry {
                resolved: wt.clone(),
                marker,
            },
        );
    }
}

impl WorktreeResolver for GitWorktreeResolver {
    async fn resolve(&self, cwd: &str) -> Option<ResolvedWorktree> {
        if cwd.is_empty() {
            return None;
        }
        // The registry is read on EVERY call, so a hit is always checked against
        // the repositories registered right now (cheap: a list from the store).
        let records = self.repos.list().ok()?;
        let mut registry: Vec<String> = records.iter().map(|r| r.id.0.clone()).collect();
        registry.sort();
        // Canonical FIRST (one `realpath`), then the cache: the key is where the
        // cwd points now. An unresolvable cwd (deleted) is not worth a git call.
        let canonical = tokio::fs::canonicalize(cwd).await.ok();
        // Looked up before the early return below so that an emptied registry
        // also clears the cache.
        let hit = canonical.as_deref().and_then(|c| self.lookup(&registry, c));
        if records.is_empty() {
            return None;
        }
        if hit.is_some() {
            return hit;
        }
        let canonical = canonical?;

        // Negative answers are NOT cached: a directory that is not a worktree
        // now may become one (`git worktree add`), and a miss costs one failing
        // git call, bounded by the runner's global concurrency cap.
        let info = open_repo(&self.git, &canonical).await.ok()?;
        let toplevel: PathBuf = info.toplevel?; // `None`: bare repo or inside `.git`
        // Registered? Match the repository by its canonical common dir (shared
        // by the main checkout and every linked worktree).
        if !records.iter().any(|r| r.common_dir == info.common_dir) {
            return None;
        }

        let resolved = ResolvedWorktree {
            id: WorktreeId(worktree_id(&info.common_dir, &toplevel)),
            root: toplevel,
        };
        self.remember(&registry, canonical, &resolved);
        Some(resolved)
    }
}
