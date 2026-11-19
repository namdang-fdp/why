//! Integration tests for the watcher: a real `WatchHub` (real inotify, real git)
//! on throw-away repositories. No Tauri app; the hub's channel is read directly.
//!
//! Timing: the debouncer flushes 200 ms after the last change (1 s at most), so
//! "an event arrives" waits up to a generous few seconds, and "no event" waits
//! out a quiet window well past the debounce delay. Never a tight timeout.

#[path = "../git/common.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{TestRepo, test_runner};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};
use why_lib::contracts::{RepoId, RepositoryChanged, WorktreeChanged, WorktreeId};
use why_lib::git;
use why_lib::ipc::repos::{add_repository_impl, remove_repository_impl};
use why_lib::state::AppState;
use why_lib::store::{MemoryRepoStore, RepoRecord, RepoStore};
use why_lib::watch::{NotifyingRepoStore, WatchConfig, WatchHub};

/// Longest we wait for an expected event.
const EXPECT_WITHIN: Duration = Duration::from_secs(5);
/// How long "nothing happens" must hold.
const QUIET: Duration = Duration::from_millis(1200);

struct Watched {
    repo: TestRepo,
    hub: WatchHub,
    rx: Receiver<WorktreeChanged>,
    main_id: WorktreeId,
}

fn common_dir(repo: &TestRepo) -> PathBuf {
    repo.path.join(".git").canonicalize().unwrap()
}

fn wt_id(repo: &TestRepo, root: &Path) -> WorktreeId {
    WorktreeId(git::worktree_id(&common_dir(repo), root))
}

/// Register `repo` in a memory store and start a hub on it, waiting until the
/// first sync has installed the watches.
async fn watch(repo: TestRepo, cfg: WatchConfig) -> Watched {
    let common = common_dir(&repo);
    let store = Arc::new(MemoryRepoStore::new());
    store
        .insert(RepoRecord {
            id: RepoId(git::repo_id(&common)),
            name: "repo".into(),
            root_path: repo.path.clone(),
            common_dir: common,
        })
        .unwrap();
    let (hub, rx) = WatchHub::start(test_runner(), store as Arc<dyn RepoStore>, cfg);
    hub.refresh().await;
    let main_id = wt_id(&repo, &repo.path);
    assert_eq!(hub.worktree_ids(), vec![main_id.clone()]);
    Watched {
        repo,
        hub,
        rx,
        main_id,
    }
}

/// No periodic reconcile during the test (only the explicit `refresh`), so
/// every event comes from the OS watcher.
fn cfg() -> WatchConfig {
    WatchConfig {
        reconcile_every: Duration::from_secs(3600),
        ..WatchConfig::default()
    }
}

impl Watched {
    /// Next event, or panic after `EXPECT_WITHIN`.
    async fn next(&mut self) -> WorktreeChanged {
        tokio::time::timeout(EXPECT_WITHIN, self.rx.recv())
            .await
            .expect("timed out waiting for a worktree-changed event")
            .expect("hub channel closed")
    }

    /// Every event until the channel has been quiet for `QUIET`.
    async fn drain(&mut self) -> Vec<WorktreeChanged> {
        let mut out = Vec::new();
        while let Ok(Some(e)) = tokio::time::timeout(QUIET, self.rx.recv()).await {
            out.push(e);
        }
        out
    }

    /// Wait until an event satisfies `pred` (other events are skipped).
    async fn wait_for(&mut self, pred: impl Fn(&WorktreeChanged) -> bool) -> WorktreeChanged {
        let deadline = Instant::now() + EXPECT_WITHIN;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(e)) if pred(&e) => return e,
                Ok(Some(_)) => continue,
                _ => panic!("timed out waiting for a matching event"),
            }
        }
    }
}

fn refetch_all_for(id: &WorktreeId) -> impl Fn(&WorktreeChanged) -> bool + '_ {
    move |e| &e.worktree_id == id && e.paths.is_empty()
}

#[tokio::test]
async fn editing_a_tracked_file_emits_one_event_with_its_path() {
    let mut w = watch(TestRepo::with_initial_commit(), cfg()).await;

    let started = Instant::now();
    w.repo.write("README.md", "changed\n");
    let e = w.next().await;
    let latency = started.elapsed();

    assert_eq!(e.worktree_id, w.main_id);
    assert_eq!(e.paths, vec!["README.md".to_string()]);
    // 200 ms debounce + scheduling; generous bound against slow CI.
    assert!(latency < Duration::from_secs(2), "took {latency:?}");
    eprintln!("edit -> event latency: {latency:?}");
    assert!(w.drain().await.is_empty(), "exactly one event for one edit");
}

#[tokio::test]
async fn a_thousand_files_give_a_bounded_number_of_events() {
    let mut w = watch(TestRepo::with_initial_commit(), cfg()).await;
    // Create the directory first and let that event pass: a NEW directory
    // alone already means "refetch all", and this test is about the path cap.
    std::fs::create_dir(w.repo.path.join("gen")).unwrap();
    w.drain().await;
    for i in 0..1000 {
        w.repo.write(&format!("gen/f{i}.txt"), "x");
    }
    let events = w.drain().await;
    eprintln!(
        "1000 files -> {} events, path counts {:?}",
        events.len(),
        events.iter().map(|e| e.paths.len()).collect::<Vec<_>>()
    );
    assert!(!events.is_empty());
    assert!(events.len() <= 5, "got {} events", events.len());
    // More than `max_paths` changed: the batch says "refetch everything".
    assert!(events.iter().any(|e| e.paths.is_empty()));
}

#[tokio::test]
async fn gitignored_changes_emit_nothing() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "node_modules/\n*.log\n");
    repo.write("README.md", "hi\n");
    repo.commit_all("init");
    let mut w = watch(repo, cfg()).await;

    w.repo.write("node_modules/pkg/index.js", "x");
    w.repo.write("node_modules/pkg/deep/a/b/c.js", "x");
    w.repo.write("debug.log", "x");
    assert_eq!(w.drain().await, vec![], "ignored paths must not emit");

    // ... while a normal file still does.
    w.repo.write("README.md", "changed\n");
    assert_eq!(w.next().await.paths, vec!["README.md".to_string()]);
}

#[tokio::test]
async fn editing_gitignore_updates_the_rules() {
    let mut w = watch(TestRepo::with_initial_commit(), cfg()).await;
    w.repo.write(".gitignore", "build/\n");
    let e = w.next().await;
    assert_eq!(e.paths, vec![".gitignore".to_string()]);
    w.drain().await;

    w.repo.write("build/out.bin", "x");
    assert_eq!(
        w.drain().await,
        vec![],
        "new rule applies without a restart"
    );
}

#[tokio::test]
async fn files_in_a_new_directory_are_not_lost() {
    let mut w = watch(TestRepo::with_initial_commit(), cfg()).await;
    // mkdir + write in one go: inotify watches `src/` only after the file
    // inside it already exists, so the file itself is never reported. The
    // directory creation must therefore mean "refetch all".
    w.repo.write("src/deep/main.rs", "fn main() {}\n");
    let id = w.main_id.clone();
    w.wait_for(refetch_all_for(&id)).await;
}

#[tokio::test]
async fn commit_emits_refetch_all() {
    let mut w = watch(TestRepo::with_initial_commit(), cfg()).await;
    w.repo.write("a.txt", "a\n");
    w.repo.commit_all("add a");
    let id = w.main_id.clone();
    w.wait_for(refetch_all_for(&id)).await;
}

#[tokio::test]
async fn branch_switch_emits_refetch_all() {
    let mut w = watch(TestRepo::with_initial_commit(), cfg()).await;
    w.repo.git(["switch", "-q", "-c", "other"]);
    let id = w.main_id.clone();
    w.wait_for(refetch_all_for(&id)).await;
}

#[tokio::test]
async fn our_own_git_status_does_not_trigger_events() {
    let mut w = watch(TestRepo::with_initial_commit(), cfg()).await;
    w.repo.write("dirty.txt", "x");
    w.drain().await; // the write itself

    // The app reads status constantly; with GIT_OPTIONAL_LOCKS=0 git must not
    // refresh the index, so reading never looks like a change (no feedback loop).
    for _ in 0..3 {
        git::read_status(&test_runner(), &w.repo.path)
            .await
            .unwrap();
    }
    assert_eq!(w.drain().await, vec![]);
}

#[tokio::test]
async fn new_linked_worktree_is_picked_up_and_watched() {
    let mut w = watch(TestRepo::with_initial_commit(), cfg()).await;

    let wt_path = w.repo.add_worktree("feature", "feature");
    let new_id = wt_id(&w.repo, &wt_path);

    // The `.git/worktrees/` change triggers a resync (no periodic reconcile in
    // this test); the new worktree is announced with a refetch-all event.
    w.wait_for(refetch_all_for(&new_id)).await;
    assert!(w.hub.worktree_ids().contains(&new_id));
    w.drain().await;

    // Its files are watched...
    common::write_in(&wt_path, "README.md", b"in feature\n");
    let e = w.wait_for(|e| e.worktree_id == new_id).await;
    assert_eq!(e.paths, vec!["README.md".to_string()]);
    w.drain().await;

    // ... and so is its private HEAD (in `.git/worktrees/feature/`).
    common::git(&wt_path, ["switch", "-q", "-c", "feature-2"]);
    w.wait_for(refetch_all_for(&new_id)).await;
}

#[tokio::test]
async fn removed_worktree_is_unwatched_on_reconcile() {
    let repo = TestRepo::with_initial_commit();
    let wt_path = repo.add_worktree("gone", "gone");
    let w = watch_any(repo).await;
    let gone_id = wt_id(&w.repo, &wt_path);
    assert!(w.hub.worktree_ids().contains(&gone_id));

    w.repo.git([
        std::ffi::OsStr::new("worktree"),
        std::ffi::OsStr::new("remove"),
        wt_path.as_os_str(),
    ]);
    w.hub.refresh().await;
    assert_eq!(w.hub.worktree_ids(), vec![w.main_id.clone()]);
}

/// Like `watch` but without asserting there is only the main worktree.
async fn watch_any(repo: TestRepo) -> Watched {
    let common = common_dir(&repo);
    let store = Arc::new(MemoryRepoStore::new());
    store
        .insert(RepoRecord {
            id: RepoId(git::repo_id(&common)),
            name: "repo".into(),
            root_path: repo.path.clone(),
            common_dir: common,
        })
        .unwrap();
    let (hub, rx) = WatchHub::start(test_runner(), store as Arc<dyn RepoStore>, cfg());
    hub.refresh().await;
    let main_id = wt_id(&repo, &repo.path);
    Watched {
        repo,
        hub,
        rx,
        main_id,
    }
}

#[tokio::test]
async fn polling_fallback_still_reports_changes() {
    let mut w = watch(
        TestRepo::with_initial_commit(),
        WatchConfig {
            force_polling: true,
            poll_interval: Duration::from_millis(200),
            ..cfg()
        },
    )
    .await;
    // A NEW file: notify's poller compares mtimes in whole seconds, so an
    // edit within the same second as the previous write to that file would go
    // unnoticed until the next one (a known limit of the fallback). Creation
    // is always detected.
    w.repo.write("polled.txt", "x\n");
    let e = w
        .wait_for(|e| e.paths.contains(&"polled.txt".to_string()))
        .await;
    assert_eq!(e.worktree_id, w.main_id);
}

#[tokio::test]
async fn sync_with_no_repositories_unwatches_everything() {
    let w = watch(TestRepo::with_initial_commit(), cfg()).await;
    w.hub.sync(&[]).await;
    assert!(w.hub.worktree_ids().is_empty());
}

#[tokio::test]
async fn reconcile_catches_a_commit_the_watcher_missed() {
    // Poll only once an hour: the OS watcher is effectively blind, so only the
    // reconcile's HEAD/index/ref mtime comparison can notice the commit.
    let mut w = watch(
        TestRepo::with_initial_commit(),
        WatchConfig {
            force_polling: true,
            poll_interval: Duration::from_secs(3600),
            ..cfg()
        },
    )
    .await;
    // mtimes have a coarse granularity on some filesystems; make sure the
    // commit's writes land in a later tick than the first sync's stamps.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    w.repo.write("b.txt", "b\n");
    w.repo.commit_all("add b");
    assert_eq!(w.drain().await, vec![], "the blind watcher saw nothing");

    w.hub.refresh().await;
    let id = w.main_id.clone();
    w.wait_for(refetch_all_for(&id)).await;
}

// ----------------------------------------------------------------------------
// Registration through the real command logic (ipc `*_impl`), as the app does.

/// An app-like setup: memory store, hub on the bare store, commands on the
/// notifying decorator. Nothing registered yet.
struct App {
    state: AppState,
    hub: WatchHub,
    rx: Receiver<WorktreeChanged>,
}

async fn app(cfg: WatchConfig) -> App {
    let store: Arc<dyn RepoStore> = Arc::new(MemoryRepoStore::new());
    let (hub, rx) = WatchHub::start(test_runner(), Arc::clone(&store), cfg);
    hub.refresh().await; // the startup baseline
    let repos: Arc<dyn RepoStore> = Arc::new(NotifyingRepoStore::new(store, hub.resync_handle()));
    App {
        state: AppState::new(test_runner(), repos),
        hub,
        rx,
    }
}

impl App {
    async fn register(&self, path: &Path) -> RepoRecord {
        let repo = add_repository_impl(&self.state, path.to_str().unwrap())
            .await
            .unwrap();
        self.state.repos.get(&repo.id).unwrap().unwrap()
    }

    async fn wait_for(&mut self, pred: impl Fn(&WorktreeChanged) -> bool) -> WorktreeChanged {
        let deadline = Instant::now() + EXPECT_WITHIN;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(e)) if pred(&e) => return e,
                Ok(Some(_)) => continue,
                _ => panic!("timed out waiting for a matching event"),
            }
        }
    }

    async fn drain(&mut self) -> Vec<WorktreeChanged> {
        let mut out = Vec::new();
        while let Ok(Some(e)) = tokio::time::timeout(QUIET, self.rx.recv()).await {
            out.push(e);
        }
        out
    }
}

fn main_id(rec: &RepoRecord) -> WorktreeId {
    WorktreeId(git::worktree_id(&rec.common_dir, &rec.root_path))
}

#[tokio::test]
async fn a_newly_registered_repo_is_watched_without_waiting_for_reconcile() {
    let mut a = app(cfg()).await; // periodic reconcile: once an hour
    let repo = TestRepo::with_initial_commit();
    // Edited BEFORE registration: the watch does not exist yet, so the event
    // announcing the new worktree must be a refetch-all.
    repo.write("README.md", "edited before registration\n");
    let rec = a.register(&repo.path).await;
    let id = main_id(&rec);

    a.wait_for(refetch_all_for(&id)).await;
    assert!(a.hub.worktree_ids().contains(&id));
    a.drain().await;

    repo.write("README.md", "after\n");
    let e = a.wait_for(|e| e.worktree_id == id).await;
    assert_eq!(e.paths, vec!["README.md".to_string()]);

    // Unregistering stops watching, also without a reconcile.
    remove_repository_impl(&a.state, &rec.id).await.unwrap();
    let deadline = Instant::now() + EXPECT_WITHIN;
    while !a.hub.worktree_ids().is_empty() {
        assert!(Instant::now() < deadline, "still watched after removal");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn separate_git_dir_checkout_is_watched_at_its_real_root() {
    let base = tempfile::tempdir().unwrap();
    let base_path = base.path().canonicalize().unwrap();
    let work = base_path.join("work");
    let meta = base_path.join("meta.git");
    common::git(
        &base_path,
        [
            std::ffi::OsStr::new("init"),
            std::ffi::OsStr::new("-q"),
            std::ffi::OsStr::new("-b"),
            std::ffi::OsStr::new("main"),
            std::ffi::OsStr::new("--separate-git-dir"),
            meta.as_os_str(),
            work.as_os_str(),
        ],
    );
    common::assert_isolated(&work, &base_path);
    common::configure(&work);
    common::write_in(&work, "README.md", b"hi\n");
    common::git(&work, ["add", "-A"]);
    common::git(&work, ["commit", "-q", "-m", "init"]);

    let mut a = app(cfg()).await;
    let rec = a.register(&work).await;
    assert_eq!(rec.root_path, work);
    let id = main_id(&rec);
    a.wait_for(refetch_all_for(&id)).await;
    a.drain().await;

    common::write_in(&work, "README.md", b"changed\n");
    let e = a.wait_for(|e| e.worktree_id == id).await;
    assert_eq!(e.paths, vec!["README.md".to_string()]);
}

#[tokio::test]
async fn submodule_edits_and_staging_route_to_the_submodule() {
    let sub_origin = TestRepo::with_initial_commit();
    let parent = TestRepo::with_initial_commit();
    // Local-path submodules need `protocol.file.allow` since git 2.38.
    parent.git([
        std::ffi::OsStr::new("-c"),
        std::ffi::OsStr::new("protocol.file.allow=always"),
        std::ffi::OsStr::new("submodule"),
        std::ffi::OsStr::new("add"),
        std::ffi::OsStr::new("-q"),
        sub_origin.path.as_os_str(),
        std::ffi::OsStr::new("sub"),
    ]);
    parent.git(["commit", "-q", "-m", "add submodule"]);
    let sub_path = parent.path.join("sub");
    common::configure(&sub_path);

    let mut a = app(cfg()).await;
    let parent_rec = a.register(&parent.path).await;
    let sub_rec = a.register(&sub_path).await;
    assert_eq!(sub_rec.root_path, sub_path);
    let sub_id = main_id(&sub_rec);
    a.wait_for(refetch_all_for(&sub_id)).await;
    a.drain().await;

    // A working-file edit: the longest root (the submodule's) wins.
    common::write_in(&sub_path, "README.md", b"changed in sub\n");
    let e = a.wait_for(|e| e.paths == ["README.md"]).await;
    assert_eq!(e.worktree_id, sub_id);
    a.drain().await;

    // Staging writes `.git/modules/sub/index`, which is inside the PARENT's
    // `.git` too: the longest common dir (the submodule's) must win.
    common::git(&sub_path, ["add", "README.md"]);
    a.wait_for(refetch_all_for(&sub_id)).await;
    let _ = parent_rec;
}

#[tokio::test]
async fn tracked_files_matching_ignore_rules_still_notify() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "dist/\n");
    repo.write("dist/snap.txt", "v1\n");
    repo.write("README.md", "hi\n");
    repo.git(["add", ".gitignore", "README.md"]);
    repo.git(["add", "-f", "dist/snap.txt"]); // force-added despite the rule
    repo.git(["commit", "-q", "-m", "init"]);
    let mut w = watch(repo, cfg()).await;

    w.repo.write("dist/other.txt", "x"); // untracked and ignored
    assert_eq!(w.drain().await, vec![]);

    w.repo.write("dist/snap.txt", "v2\n"); // tracked: git status shows it
    let e = w.next().await;
    assert_eq!(e.paths, vec!["dist/snap.txt".to_string()]);
}

#[tokio::test]
async fn hitting_the_watch_limit_later_switches_that_root_to_polling() {
    let mut w = watch(
        TestRepo::with_initial_commit(),
        WatchConfig {
            poll_interval: Duration::from_millis(100),
            ..cfg()
        },
    )
    .await;
    assert!(w.hub.polled_roots().is_empty());

    // What notify reports when a new subdirectory cannot get an inotify watch.
    w.hub.simulate_watch_limit(&w.repo.path.join("src/new"));
    let id = w.main_id.clone();
    w.wait_for(refetch_all_for(&id)).await; // that subtree may have been blind
    assert_eq!(w.hub.polled_roots(), vec![w.repo.path.clone()]);
    w.drain().await;

    w.repo.write("README.md", "seen by the poller\n");
    let e = w.wait_for(|e| e.paths == ["README.md"]).await;
    assert_eq!(e.worktree_id, id);
}

#[tokio::test]
async fn a_worktree_recreated_at_the_same_path_is_watched_again() {
    let repo = TestRepo::with_initial_commit();
    let wt_path = repo.add_worktree("wt", "wt");
    let mut w = watch_any(repo).await;
    let id = wt_id(&w.repo, &wt_path);
    assert!(w.hub.worktree_ids().contains(&id));

    // Remove and re-create it at the same path in quick succession, before
    // any reconcile: the old inotify watch on that root died with it.
    w.repo.git([
        std::ffi::OsStr::new("worktree"),
        std::ffi::OsStr::new("remove"),
        wt_path.as_os_str(),
    ]);
    w.repo.git([
        std::ffi::OsStr::new("worktree"),
        std::ffi::OsStr::new("add"),
        std::ffi::OsStr::new("-q"),
        std::ffi::OsStr::new("-b"),
        std::ffi::OsStr::new("wt2"),
        wt_path.as_os_str(),
    ]);
    w.wait_for(refetch_all_for(&id)).await;
    w.drain().await;

    common::write_in(&wt_path, "README.md", b"in the new checkout\n");
    let e = w.wait_for(|e| e.worktree_id == id).await;
    assert_eq!(e.paths, vec!["README.md".to_string()]);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_directories_are_not_followed() {
    let repo = TestRepo::with_initial_commit();
    let outside = repo.base().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, repo.path.join("link")).unwrap();
    let mut w = watch(repo, cfg()).await;

    std::fs::write(outside.join("file.txt"), "x").unwrap();
    std::fs::create_dir(outside.join("dir")).unwrap();
    assert_eq!(
        w.drain().await,
        vec![],
        "nothing outside the checkout is watched"
    );
}

#[tokio::test]
async fn a_directory_moved_out_of_the_checkout_means_refetch_all() {
    let repo = TestRepo::new();
    repo.write("d/a.txt", "a\n");
    repo.write("d/b.txt", "b\n");
    repo.commit_all("init");
    let mut w = watch(repo, cfg()).await;

    // Same filesystem, outside the worktree: inotify reports only the source
    // of the rename, and by the time we look it is gone.
    std::fs::rename(w.repo.path.join("d"), w.repo.base().join("d-moved")).unwrap();
    let events = w.drain().await;
    assert!(!events.is_empty());
    assert!(
        events.iter().all(|e| e.paths.is_empty()),
        "the files under d/ are gone too: {events:?}"
    );
}

#[tokio::test]
async fn an_editors_atomic_save_reports_just_the_file() {
    let mut w = watch(TestRepo::with_initial_commit(), cfg()).await;
    // Write a temp file, then rename it over the real one.
    w.repo.write(".README.md.swp", "saved\n");
    std::fs::rename(
        w.repo.path.join(".README.md.swp"),
        w.repo.path.join("README.md"),
    )
    .unwrap();
    let events = w.drain().await;
    assert!(
        events.iter().all(|e| !e.paths.is_empty()),
        "a file rename is not a refetch-all: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e.paths.contains(&"README.md".to_string()))
    );
}

#[tokio::test]
async fn editing_info_exclude_updates_the_rules() {
    let repo = TestRepo::with_initial_commit();
    let exclude = repo.path.join(".git/info/exclude");
    std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    std::fs::write(&exclude, "gen/\n").unwrap();
    std::fs::create_dir(repo.path.join("gen")).unwrap();
    let mut w = watch(repo, cfg()).await;

    w.repo.write("gen/a.txt", "x");
    assert_eq!(w.drain().await, vec![], "excluded");

    std::fs::write(&exclude, "# nothing excluded any more\n").unwrap();
    let id = w.main_id.clone();
    w.wait_for(refetch_all_for(&id)).await; // rules rebuilt
    w.drain().await;

    w.repo.write("gen/b.txt", "x");
    let e = w.wait_for(|e| e.paths == ["gen/b.txt"]).await;
    assert_eq!(e.worktree_id, id);
}

#[tokio::test]
async fn moving_a_directory_with_a_gitignore_updates_the_rules() {
    let repo = TestRepo::new();
    repo.write("a/.gitignore", "*.tmp\n");
    repo.write("README.md", "hi\n");
    repo.commit_all("init");
    let mut w = watch(repo, cfg()).await;

    w.repo.write("a/x.tmp", "x");
    assert_eq!(w.drain().await, vec![], "ignored by a/.gitignore");

    // Move `a/` (and its .gitignore) to `b/`, then recreate an empty `a/`:
    // the old rule must not keep hiding `a/*.tmp`.
    std::fs::rename(w.repo.path.join("a"), w.repo.path.join("b")).unwrap();
    std::fs::create_dir(w.repo.path.join("a")).unwrap();
    w.drain().await; // directory events + rebuild

    w.repo.write("a/y.tmp", "y");
    let e = w.wait_for(|e| e.paths == ["a/y.tmp"]).await;
    assert_eq!(e.worktree_id, w.main_id);
    w.drain().await;

    w.repo.write("b/z.tmp", "z");
    assert_eq!(w.drain().await, vec![], "the rule moved with its directory");
}

// ---------------------------------------------------------------------------
// Repository-scoped events: linked worktrees coming and going
// ---------------------------------------------------------------------------

/// Like `watch_any`, with the `repository-changed` stream too.
async fn watch_repo_events(
    repo: TestRepo,
) -> (Watched, UnboundedReceiver<RepositoryChanged>, RepoId) {
    let common = common_dir(&repo);
    let repo_id = RepoId(git::repo_id(&common));
    let store = Arc::new(MemoryRepoStore::new());
    store
        .insert(RepoRecord {
            id: repo_id.clone(),
            name: "repo".into(),
            root_path: repo.path.clone(),
            common_dir: common,
        })
        .unwrap();
    let (hub, rx, repo_rx) =
        WatchHub::start_with_repo_events(test_runner(), store as Arc<dyn RepoStore>, cfg());
    hub.refresh().await; // the baseline: nothing is "changed" yet
    let main_id = wt_id(&repo, &repo.path);
    (
        Watched {
            repo,
            hub,
            rx,
            main_id,
        },
        repo_rx,
        repo_id,
    )
}

async fn next_repo_event(rx: &mut UnboundedReceiver<RepositoryChanged>) -> RepositoryChanged {
    tokio::time::timeout(EXPECT_WITHIN, rx.recv())
        .await
        .expect("timed out waiting for a repository-changed event")
        .expect("hub channel closed")
}

async fn quiet(rx: &mut UnboundedReceiver<RepositoryChanged>) -> Vec<RepositoryChanged> {
    let mut out = Vec::new();
    while let Ok(Some(e)) = tokio::time::timeout(QUIET, rx.recv()).await {
        out.push(e);
    }
    out
}

#[tokio::test]
async fn adding_a_linked_worktree_announces_its_repository() {
    let (w, mut repo_rx, repo_id) = watch_repo_events(TestRepo::with_initial_commit()).await;
    assert_eq!(
        quiet(&mut repo_rx).await,
        vec![],
        "the baseline is not a change"
    );

    w.repo.add_worktree("feature", "feature");

    assert_eq!(
        next_repo_event(&mut repo_rx).await,
        RepositoryChanged { repo_id }
    );
}

#[tokio::test]
async fn removing_a_linked_worktree_announces_its_repository() {
    let repo = TestRepo::with_initial_commit();
    let wt_path = repo.add_worktree("gone", "gone");
    let (w, mut repo_rx, repo_id) = watch_repo_events(repo).await;
    assert_eq!(quiet(&mut repo_rx).await, vec![]);

    w.repo.git([
        std::ffi::OsStr::new("worktree"),
        std::ffi::OsStr::new("remove"),
        wt_path.as_os_str(),
    ]);

    assert_eq!(
        next_repo_event(&mut repo_rx).await,
        RepositoryChanged { repo_id }
    );
}

#[tokio::test]
async fn deleting_a_worktree_directory_announces_its_repository() {
    // Not `git worktree remove`: the directory just disappears, git still lists
    // it (prunable) and the watcher stops watching it.
    let repo = TestRepo::with_initial_commit();
    let wt_path = repo.add_worktree("vanished", "vanished");
    let (_w, mut repo_rx, repo_id) = watch_repo_events(repo).await;
    assert_eq!(quiet(&mut repo_rx).await, vec![]);

    std::fs::remove_dir_all(&wt_path).unwrap();

    assert_eq!(
        next_repo_event(&mut repo_rx).await,
        RepositoryChanged { repo_id }
    );
}

#[tokio::test]
async fn ordinary_edits_and_commits_do_not_announce_the_repository() {
    let (mut w, mut repo_rx, _) = watch_repo_events(TestRepo::with_initial_commit()).await;
    w.repo.write("README.md", "edited\n");
    w.repo.git(["commit", "-q", "-am", "edit"]);
    w.wait_for(refetch_all_for(&w.main_id.clone())).await;
    assert_eq!(quiet(&mut repo_rx).await, vec![]);
}
