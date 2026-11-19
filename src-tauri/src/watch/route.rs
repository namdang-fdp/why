//! Routing: which worktree does a changed absolute path belong to, and does it
//! matter?
//!
//! The OS reports absolute paths. One watcher covers every repository, so for
//! each path we decide, in this order:
//!
//! 1. **Inside a repository's git directory** (`<common_dir>/...`, the longest
//!    matching one, so a submodule's `.git/modules/<name>` wins over its parent's
//!    `.git`): almost everything there is git's internal bookkeeping (`objects/`,
//!    `logs/`, lock files) and is ignored. Only these matter:
//!    * `HEAD`, `index` of the main worktree (directly in the common dir) and
//!      `worktrees/<name>/HEAD|index` of a linked one: what is checked out or
//!      staged changed, so the UI refetches everything;
//!    * `refs/**`, `packed-refs`: branches moved (shared by all worktrees, so
//!      every worktree of the repository is told);
//!    * `worktrees/` or `worktrees/<name>` appearing or disappearing: a
//!      worktree was added or removed, so the worktree list is re-read;
//!    * `info/exclude`: the repository-wide ignore rules changed.
//!
//!    Checked first because the main worktree's git dir (`.git/`) usually sits
//!    *inside* its root.
//! 2. **Inside a worktree root** (the longest matching root wins: a linked
//!    worktree may live inside the main checkout, e.g. `.worktrees/x`): the
//!    path relative to that root, unless it is gitignored (and not tracked) or
//!    inside some `.git` (a nested repository's internals).

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use super::debounce::{Change, RawChange};
use super::ignore_rules::IgnoreRules;
use crate::contracts::{RepoId, WorktreeId};

/// What the router needs to know about one watched worktree.
#[derive(Debug)]
pub struct WorktreeSpec {
    pub id: WorktreeId,
    pub repo: RepoId,
    /// Canonical root of the working files.
    pub root: PathBuf,
    /// The worktree's private git dir: `common_dir` for the main worktree,
    /// `<common_dir>/worktrees/<name>` for a linked one.
    pub git_dir: PathBuf,
    /// The repository's shared git dir (for `info/exclude`).
    pub common_dir: PathBuf,
    /// Rebuilt when ignore files change, so behind its own lock: the router
    /// only needs `&self` access to the registry.
    pub ignore: RwLock<IgnoreRules>,
    /// Tracked files that the ignore rules match (force-added, or committed
    /// before the rule existed), plus all their parent directories. Git still
    /// reports changes to them, so they are never dropped as "ignored".
    pub tracked_ignored: RwLock<HashSet<PathBuf>>,
}

impl WorktreeSpec {
    /// Is `rel` ignored, taking tracked-but-ignored files into account?
    fn is_ignored(&self, rel: &Path, is_dir: bool) -> bool {
        // A panic while rebuilding these left the old values in place; still
        // usable, so recover the guard instead of failing.
        if self
            .tracked_ignored
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(rel)
        {
            return false;
        }
        self.ignore
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_ignored(rel, is_dir)
    }
}

#[derive(Debug)]
pub struct RepoSpec {
    pub id: RepoId,
    pub common_dir: PathBuf,
}

/// The routing table, replaced as a whole on every sync.
#[derive(Debug, Default)]
pub struct Registry {
    pub repos: Vec<RepoSpec>,
    pub worktrees: Vec<Arc<WorktreeSpec>>,
}

/// What the event said about a path, beyond the path itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PathInfo {
    /// The path is (or, for a removal, was) a directory.
    pub is_dir: bool,
    /// The path was deleted or renamed away.
    pub removed: bool,
    /// The path was renamed away and is gone, so whether it was a file or a
    /// directory is unknown (inotify does not say for the source of a rename).
    pub moved_away: bool,
}

/// The outcome of routing one path.
#[derive(Debug, PartialEq, Eq)]
pub enum Routed {
    Change(RawChange),
    /// The set of worktrees of some repository may have changed: re-list.
    Resync,
    /// This worktree's ignore rules may have changed: rebuild them.
    RebuildIgnore(WorktreeId),
    /// This worktree's index changed: re-list its tracked-but-ignored files.
    RefreshTracked(WorktreeId),
    /// A watched worktree root itself was deleted or moved: its watch is dead
    /// and must be re-installed if the directory comes back.
    RootGone(PathBuf),
}

impl Registry {
    /// Route one absolute path. Usually returns zero or one item; a ref change
    /// fans out to every worktree of the repository.
    pub fn route(&self, path: &Path, info: PathInfo) -> Vec<Routed> {
        if let Some(out) = self.route_git_dir(path) {
            return out;
        }
        self.route_worktree(path, info).unwrap_or_default()
    }

    /// The worktree and relative path of `path`, if it is a working file (used
    /// to settle a rename once its destination is known).
    pub fn worktree_rel(&self, path: &Path) -> Option<(WorktreeId, String)> {
        if self.git_dir_of(path).is_some() {
            return None;
        }
        let wt = self.worktree_of(path)?;
        let rel = path.strip_prefix(&wt.root).ok()?;
        Some((wt.id.clone(), rel_to_string(rel)))
    }

    /// The registered repository whose common dir contains `path`; the most
    /// specific (longest) one if several do.
    fn git_dir_of(&self, path: &Path) -> Option<&RepoSpec> {
        self.repos
            .iter()
            .filter(|r| path.starts_with(&r.common_dir))
            .max_by_key(|r| r.common_dir.components().count())
    }

    /// The worktree whose root contains `path`; the most specific one.
    fn worktree_of(&self, path: &Path) -> Option<&Arc<WorktreeSpec>> {
        self.worktrees
            .iter()
            .filter(|w| path.starts_with(&w.root))
            .max_by_key(|w| w.root.components().count())
    }

    fn route_git_dir(&self, path: &Path) -> Option<Vec<Routed>> {
        let repo = self.git_dir_of(path)?;
        let rel = path.strip_prefix(&repo.common_dir).ok()?;
        // Lossy so a non-UTF-8 name still takes a slot (as U+FFFD) instead of
        // shifting the pattern below; the names we match on are all ASCII.
        let lossy: Vec<_> = rel
            .components()
            .filter(|c| matches!(c, Component::Normal(_)))
            .map(|c| c.as_os_str().to_string_lossy())
            .collect();
        let parts: Vec<&str> = lossy.iter().map(|s| s.as_ref()).collect();

        // Git never writes a ref or the index in place: it writes `x.lock` and
        // renames it over `x`. The rename reports `x`, which is what we want;
        // the `.lock` file alone is noise.
        if parts.last().is_some_and(|p| p.ends_with(".lock")) {
            return Some(Vec::new());
        }

        let of_repo = || self.worktrees.iter().filter(|w| w.repo == repo.id);
        let refetch = |w: &Arc<WorktreeSpec>| Routed::Change(RawChange::refetch_all(w.id.clone()));
        // HEAD/index of one worktree: refetch, and re-read which ignored files
        // are tracked (a `git add -f` changes that).
        let head_or_index = |git_dir: &Path, is_index: bool| -> Vec<Routed> {
            let mut out = Vec::new();
            for w in of_repo().filter(|w| w.git_dir == git_dir) {
                out.push(refetch(w));
                if is_index {
                    out.push(Routed::RefreshTracked(w.id.clone()));
                }
            }
            out
        };
        let out = match parts.as_slice() {
            ["HEAD"] => head_or_index(&repo.common_dir, false),
            ["index"] => head_or_index(&repo.common_dir, true),
            // A branch or tag moved: any worktree of this repository may be on it
            // or use it as its base.
            ["packed-refs"] | ["refs", ..] => of_repo().map(refetch).collect(),
            // A linked worktree's private HEAD or index. `rel.parent()` is
            // `worktrees/<name>` with the name's raw bytes.
            ["worktrees", _, last @ ("HEAD" | "index")] => {
                head_or_index(&repo.common_dir.join(rel.parent()?), *last == "index")
            }
            // `worktrees/` or one entry in it was created or deleted.
            ["worktrees"] | ["worktrees", _] => vec![Routed::Resync],
            // Repository-wide ignore rules.
            ["info", "exclude"] | ["info"] => of_repo()
                .map(|w| Routed::RebuildIgnore(w.id.clone()))
                .collect(),
            // objects/, logs/, config, FETCH_HEAD, ...: not interesting.
            _ => Vec::new(),
        };
        Some(out)
    }

    fn route_worktree(&self, path: &Path, info: PathInfo) -> Option<Vec<Routed>> {
        let wt = self.worktree_of(path)?;
        let rel = path.strip_prefix(&wt.root).ok()?;
        if rel.as_os_str().is_empty() {
            // The root directory itself. Deleted or moved: its inotify watch
            // died with it.
            return Some(if info.removed || info.moved_away {
                vec![Routed::RootGone(wt.root.clone())]
            } else {
                Vec::new()
            });
        }
        // Inside any `.git`: a linked worktree's `.git` file, or the internals
        // of a nested repository (submodule, vendored clone). Not our files.
        if rel.components().any(|c| c.as_os_str() == ".git") {
            return Some(Vec::new());
        }

        let id = wt.id.clone();
        let mut out = Vec::new();
        if rel.file_name().is_some_and(|n| n == ".gitignore") {
            out.push(Routed::RebuildIgnore(id.clone()));
        }

        if info.moved_away {
            // Unknown kind: drop it only if it would be ignored either way.
            if wt.is_ignored(rel, false) && wt.is_ignored(rel, true) {
                return Some(out);
            }
            // It may have been a directory holding a `.gitignore`.
            out.push(Routed::RebuildIgnore(id.clone()));
            out.push(Routed::Change(RawChange {
                worktree: id,
                change: Change::MovedAway(rel_to_string(rel)),
            }));
            return Some(out);
        }

        if wt.is_ignored(rel, info.is_dir) {
            return Some(out);
        }
        if info.is_dir {
            // A directory appeared, vanished or was renamed. inotify is not
            // recursive: notify adds a watch on a new directory only after
            // seeing it created, so files written into it in the meantime
            // (`mkdir -p src && echo > src/a.rs`) produce no event at all. And a
            // removed directory takes an unknown set of files with it. Either
            // way we cannot list what changed, so: refetch everything. The
            // directory may also carry a `.gitignore`: rebuild the rules.
            out.push(Routed::RebuildIgnore(id.clone()));
            out.push(Routed::Change(RawChange::refetch_all(id)));
        } else {
            out.push(Routed::Change(RawChange::path(id, rel_to_string(rel))));
        }
        Some(out)
    }
}

/// A relative path as the UI sees it: `/`-separated (the contract uses
/// strings; a non-UTF-8 name is converted lossily, see `contracts/changes.rs`).
fn rel_to_string(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, repo: &str, root: &str, git_dir: &str, common: &str) -> Arc<WorktreeSpec> {
        Arc::new(WorktreeSpec {
            id: WorktreeId(id.into()),
            repo: RepoId(repo.into()),
            root: root.into(),
            git_dir: git_dir.into(),
            common_dir: common.into(),
            ignore: RwLock::new(IgnoreRules::default()),
            tracked_ignored: RwLock::new(HashSet::new()),
        })
    }

    /// Repo "r": main at /c/app (git dir /c/app/.git), linked at /c/app-feat and
    /// nested /c/app/.wt/x. Repo "s": a submodule at /c/app/sub whose git dir
    /// lives inside the parent's: /c/app/.git/modules/sub.
    fn registry() -> Registry {
        let g = "/c/app/.git";
        let sg = "/c/app/.git/modules/sub";
        Registry {
            repos: vec![
                RepoSpec {
                    id: RepoId("r".into()),
                    common_dir: g.into(),
                },
                RepoSpec {
                    id: RepoId("s".into()),
                    common_dir: sg.into(),
                },
            ],
            worktrees: vec![
                spec("main", "r", "/c/app", g, g),
                spec(
                    "feat",
                    "r",
                    "/c/app-feat",
                    "/c/app/.git/worktrees/app-feat",
                    g,
                ),
                spec("nested", "r", "/c/app/.wt/x", "/c/app/.git/worktrees/x", g),
                spec("sub", "s", "/c/app/sub", sg, sg),
            ],
        }
    }

    /// Route and summarize: `"id:path"`, `"id:*"` (refetch all), `"id:?path"`
    /// (moved away), `"resync"`, `"rebuild:id"`, `"tracked:id"`, `"gone:root"`.
    fn r_with(path: &str, info: PathInfo) -> Vec<String> {
        registry()
            .route(Path::new(path), info)
            .into_iter()
            .map(|x| match x {
                Routed::Change(c) => match c.change {
                    Change::Path(p) => format!("{}:{p}", c.worktree.0),
                    Change::All => format!("{}:*", c.worktree.0),
                    Change::MovedAway(p) => format!("{}:?{p}", c.worktree.0),
                    Change::Settled(p) => format!("{}:={p}", c.worktree.0),
                },
                Routed::Resync => "resync".into(),
                Routed::RebuildIgnore(w) => format!("rebuild:{}", w.0),
                Routed::RefreshTracked(w) => format!("tracked:{}", w.0),
                Routed::RootGone(p) => format!("gone:{}", p.display()),
            })
            .collect()
    }

    fn r(path: &str) -> Vec<String> {
        r_with(path, PathInfo::default())
    }

    #[test]
    fn working_files_route_to_the_most_specific_worktree() {
        assert_eq!(r("/c/app/src/main.rs"), ["main:src/main.rs"]);
        assert_eq!(r("/c/app-feat/src/lib.rs"), ["feat:src/lib.rs"]);
        assert_eq!(r("/c/app/.wt/x/a.txt"), ["nested:a.txt"]);
        assert_eq!(r("/c/app/sub/lib.rs"), ["sub:lib.rs"]);
        assert!(r("/elsewhere/file").is_empty());
        assert!(r("/c/app").is_empty());
    }

    #[test]
    fn git_dir_head_index_refs() {
        assert_eq!(r("/c/app/.git/HEAD"), ["main:*"]);
        assert_eq!(r("/c/app/.git/index"), ["main:*", "tracked:main"]);
        assert_eq!(r("/c/app/.git/worktrees/app-feat/HEAD"), ["feat:*"]);
        assert_eq!(
            r("/c/app/.git/worktrees/x/index"),
            ["nested:*", "tracked:nested"]
        );
        assert_eq!(
            r("/c/app/.git/refs/heads/main"),
            ["main:*", "feat:*", "nested:*"]
        );
        assert_eq!(r("/c/app/.git/packed-refs").len(), 3);
    }

    #[test]
    fn submodule_git_dir_wins_over_the_parents() {
        // `.git/modules/sub/index` is inside the parent's `.git` too; the
        // longest common dir (the submodule's) must get it.
        assert_eq!(r("/c/app/.git/modules/sub/index"), ["sub:*", "tracked:sub"]);
        assert_eq!(r("/c/app/.git/modules/sub/refs/heads/main"), ["sub:*"]);
    }

    #[test]
    fn git_internals_and_lock_files_are_ignored() {
        assert!(r("/c/app/.git/objects/ab/cdef").is_empty());
        assert!(r("/c/app/.git/logs/HEAD").is_empty());
        assert!(r("/c/app/.git/index.lock").is_empty());
        assert!(r("/c/app/.git/refs/heads/main.lock").is_empty());
        assert!(r("/c/app/.git/worktrees/x/ORIG_HEAD").is_empty());
        assert!(
            r("/c/app-feat/.git").is_empty(),
            "linked worktree's .git file"
        );
        assert!(r("/c/app/vendor/sub/.git/objects/x").is_empty());
    }

    #[test]
    fn worktrees_dir_changes_trigger_resync() {
        assert_eq!(r("/c/app/.git/worktrees"), ["resync"]);
        assert_eq!(r("/c/app/.git/worktrees/new-one"), ["resync"]);
    }

    #[test]
    fn exclude_change_rebuilds_rules_of_every_worktree_of_the_repo() {
        assert_eq!(
            r("/c/app/.git/info/exclude"),
            ["rebuild:main", "rebuild:feat", "rebuild:nested"]
        );
    }

    #[test]
    fn gitignore_change_rebuilds_rules_and_is_reported() {
        assert_eq!(r("/c/app/.gitignore"), ["rebuild:main", "main:.gitignore"]);
    }

    #[test]
    fn a_directory_change_means_refetch_all_and_rebuild() {
        let dir = PathInfo {
            is_dir: true,
            ..PathInfo::default()
        };
        assert_eq!(r_with("/c/app/src", dir), ["rebuild:main", "main:*"]);
    }

    #[test]
    fn a_path_renamed_away_is_unresolved() {
        let gone = PathInfo {
            moved_away: true,
            removed: true,
            ..PathInfo::default()
        };
        assert_eq!(r_with("/c/app/src", gone), ["rebuild:main", "main:?src"]);
    }

    #[test]
    fn removing_a_root_reports_it_gone() {
        let removed = PathInfo {
            removed: true,
            is_dir: true,
            ..PathInfo::default()
        };
        assert_eq!(r_with("/c/app-feat", removed), ["gone:/c/app-feat"]);
    }

    #[test]
    fn tracked_ignored_files_are_not_dropped() {
        let reg = registry();
        let main = &reg.worktrees[0];
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "dist/\n").unwrap();
        *main.ignore.write().unwrap() = IgnoreRules::build(tmp.path(), &tmp.path().join(".git"));
        main.tracked_ignored
            .write()
            .unwrap()
            .extend([PathBuf::from("dist"), PathBuf::from("dist/snap.txt")]);
        let route = |p: &str| reg.route(Path::new(p), PathInfo::default()).len();
        assert_eq!(route("/c/app/dist/snap.txt"), 1, "tracked: reported");
        assert_eq!(route("/c/app/dist/other.txt"), 0, "untracked: ignored");
    }
}
