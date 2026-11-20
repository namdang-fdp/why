//! The allowlist writer: roots from the registry and from git, atomic and
//! private, and tolerant of a failing store.

use std::os::unix::fs::MetadataExt;

use why_lib::collector::allowlist_sync::{
    AllowlistState, SyncTiming, compute, run, sync_file, wrap_store, write_atomic,
};
use why_lib::store::{InsertOutcome, StoreError};
use why_wire::Allowlist;

use super::*;

#[tokio::test]
async fn compute_lists_the_main_root_and_linked_worktrees_outside_it() {
    let rig = Rig::new();
    let linked = rig.repo.add_worktree("wt-out", "feature/out");
    assert!(
        !linked.starts_with(&rig.repo.path),
        "linked worktree lives outside the root"
    );
    let list = compute(&git_common::test_runner(), &*rig.repos)
        .await
        .unwrap();
    let root = rig.repo.path.to_str().unwrap().to_owned();
    assert_eq!(list.repo_roots, std::slice::from_ref(&root));
    assert!(list.worktree_roots.contains(&root));
    assert!(
        list.worktree_roots
            .contains(&linked.to_string_lossy().into_owned())
    );
    assert!(list.allows(&linked.join("src")));
    assert!(!list.allows(rig.tmp.path()));
}

#[test]
fn the_file_is_written_atomically_privately_and_only_when_it_changes() {
    let tmp = short_tmp();
    let path = tmp.path().join("data/hook-allowlist.json");
    let a = Allowlist::new(1, vec!["/a".into()], vec!["/a".into()]);
    assert!(sync_file(&path, &a).unwrap());
    assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    let ino = std::fs::metadata(&path).unwrap().ino();

    // Same roots, newer timestamp: no rewrite (the inode would change).
    let same = Allowlist::new(999, vec!["/a".into()], vec!["/a".into()]);
    assert!(!sync_file(&path, &same).unwrap());
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), ino);

    let b = Allowlist::new(2, vec!["/a".into(), "/b".into()], vec!["/a".into()]);
    assert!(sync_file(&path, &b).unwrap());
    assert_eq!(
        Allowlist::read_from(&path).unwrap().repo_roots,
        ["/a", "/b"]
    );
    // No temp files left behind.
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(names.len(), 1);
    assert!(write_atomic(Path::new("/"), b"x").is_err());
}

/// A reader that never sees a half-written file: while a writer replaces the
/// file over and over, every successful read must parse.
#[test]
fn readers_never_see_a_torn_allowlist() {
    let tmp = short_tmp();
    let path = tmp.path().join("hook-allowlist.json");
    sync_file(&path, &Allowlist::new(0, vec!["/seed".into()], vec![])).unwrap();
    let writer = {
        let path = path.clone();
        std::thread::spawn(move || {
            for i in 0..300 {
                let roots = (0..(i % 50 + 1)).map(|n| format!("/r/{n}")).collect();
                write_atomic(&path, &Allowlist::new(i, roots, vec![]).to_bytes()).unwrap();
            }
        })
    };
    while !writer.is_finished() {
        Allowlist::read_from(&path).expect("a torn or missing file was observed");
    }
    writer.join().unwrap();
}

#[tokio::test]
async fn the_running_task_adds_worktrees_after_a_registration_and_on_wake() {
    let rig = Rig::new();
    let core = rig.start().await; // runs the refresh task
    let path = rig.paths.allowlist_path.clone();
    eventually(Duration::from_secs(5), "first write", || path.exists()).await;
    assert!(Allowlist::read_from(&path).unwrap().allows(&rig.repo.path));

    // Registering writes the new root BEFORE `insert` returns (no waiting)...
    let second = TestRepo::with_initial_commit();
    let second_linked = second.add_worktree("s2", "feature/s2");
    let common = second.path.join(".git").canonicalize().unwrap();
    let inserted = rig
        .repos
        .insert(RepoRecord {
            id: RepoId(git::repo_id(&common)),
            name: "second".into(),
            root_path: second.path.clone(),
            common_dir: common,
        })
        .unwrap();
    assert!(matches!(inserted, InsertOutcome::Inserted(_)));
    assert!(Allowlist::read_from(&path).unwrap().allows(&second.path));
    // ...and the background refresh then adds that repository's linked worktree.
    eventually(
        Duration::from_secs(5),
        "linked worktree of the new repo",
        || Allowlist::read_from(&path).is_ok_and(|l| l.allows(&second_linked)),
    )
    .await;

    // A worktree created later is picked up when woken (repository-changed).
    let late = rig.repo.add_worktree("late", "feature/late");
    core.wake_allowlist().notify_one();
    eventually(Duration::from_secs(5), "late worktree allowed", || {
        Allowlist::read_from(&path).is_ok_and(|l| l.allows(&late))
    })
    .await;
}

/// Revocation is part of unregistering: the moment `remove` returns, the file on
/// disk no longer allows the repository. Nothing is awaited, nothing polled.
#[test]
fn unregistering_revokes_on_disk_before_remove_returns() {
    let tmp = short_tmp();
    let path = tmp.path().join("data/hook-allowlist.json");
    let memory: Arc<dyn RepoStore> = Arc::new(MemoryRepoStore::new());
    let (repos, _state) = wrap_store(memory, Some(path.clone()));
    let repo_dir = tmp.path().join("work/repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    let rec = RepoRecord {
        id: RepoId("r1".into()),
        name: "repo".into(),
        root_path: repo_dir.clone(),
        common_dir: repo_dir.join(".git"),
    };

    repos.insert(rec.clone()).unwrap();
    assert!(
        Allowlist::read_from(&path).unwrap().allows(&repo_dir),
        "allowed at once"
    );

    assert!(repos.remove(&rec.id).unwrap());
    let after = Allowlist::read_from(&path).unwrap();
    assert!(!after.allows(&repo_dir), "revoked before remove returned");
    assert!(after.repo_roots.is_empty());
    // Removing something that is not registered changes nothing and is fine.
    assert!(!repos.remove(&rec.id).unwrap());
}

/// If the revocation cannot be made durable, the removal FAILS and the
/// repository stays registered (consistent with the file that still lists it).
#[test]
fn a_failed_revocation_fails_the_removal() {
    let tmp = short_tmp();
    let data = tmp.path().join("data");
    let path = data.join("hook-allowlist.json");
    let memory: Arc<dyn RepoStore> = Arc::new(MemoryRepoStore::new());
    let (repos, _state) = wrap_store(memory, Some(path.clone()));
    let repo_dir = tmp.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    let rec = RepoRecord {
        id: RepoId("r1".into()),
        name: "repo".into(),
        root_path: repo_dir,
        common_dir: tmp.path().join("repo/.git"),
    };
    repos.insert(rec.clone()).unwrap();

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = repos.remove(&rec.id);
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Root ignores directory modes; assert only where they apply.
    if unsafe { libc::geteuid() } != 0 {
        assert!(
            result.is_err(),
            "the removal must fail when revocation fails"
        );
        assert!(repos.get(&rec.id).unwrap().is_some(), "still registered");
    }
}

/// A refresh that was computed before a removal must not bring the repository
/// back (git may take a while; the user's click happens meanwhile).
#[test]
fn a_stale_background_refresh_cannot_resurrect_a_removed_repository() {
    let tmp = short_tmp();
    let path = tmp.path().join("hook-allowlist.json");
    let memory: Arc<dyn RepoStore> = Arc::new(MemoryRepoStore::new());
    let (repos, state) = wrap_store(memory, Some(path.clone()));
    let dir = tmp.path().join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    let rec = RepoRecord {
        id: RepoId("r1".into()),
        name: "repo".into(),
        root_path: dir.clone(),
        common_dir: dir.join(".git"),
    };
    repos.insert(rec.clone()).unwrap();

    let started_at = state.generation();
    let mut stale = std::collections::HashMap::new();
    stale.insert(
        "r1".to_owned(),
        why_lib::collector::allowlist_sync::RepoRoots {
            root: dir.to_string_lossy().into_owned(),
            worktrees: vec![],
        },
    );
    repos.remove(&rec.id).unwrap(); // happens while "git was running"
    assert!(
        !state.replace(stale, started_at),
        "stale refresh must be discarded"
    );
    assert!(!Allowlist::read_from(&path).unwrap().allows(&dir));
}

/// The main checkout is gone but the repository's git directory (kept elsewhere
/// with `--separate-git-dir`) and a linked worktree survive: capture continues.
#[tokio::test]
async fn linked_worktrees_survive_the_removal_of_the_main_checkout() {
    let tmp = short_tmp();
    let base = tmp.path().canonicalize().unwrap();
    let (main, gitdir, linked) = (base.join("main"), base.join("gitdir"), base.join("linked"));
    let g = |dir: &Path, args: &[&str]| git_common::git(dir, args.iter().copied());
    std::fs::create_dir_all(&base).unwrap();
    g(
        &base,
        &[
            "init",
            "-b",
            "main",
            "--separate-git-dir",
            gitdir.to_str().unwrap(),
            main.to_str().unwrap(),
        ],
    );
    git_common::configure(&main);
    std::fs::write(main.join("a.txt"), "a").unwrap();
    g(&main, &["add", "-A"]);
    g(&main, &["commit", "-m", "init"]);
    g(
        &main,
        &[
            "worktree",
            "add",
            "-b",
            "feature/x",
            linked.to_str().unwrap(),
        ],
    );
    std::fs::remove_dir_all(&main).unwrap();

    let memory: Arc<dyn RepoStore> = Arc::new(MemoryRepoStore::new());
    let common = gitdir.canonicalize().unwrap();
    memory
        .insert(RepoRecord {
            id: RepoId(git::repo_id(&common)),
            name: "main".into(),
            root_path: main.clone(),
            common_dir: common,
        })
        .unwrap();
    let list = compute(&git_common::test_runner(), &*memory).await.unwrap();
    assert!(list.allows(&linked.canonicalize().unwrap()), "{list:?}");
    assert!(list.allows(&main), "the registered root is still listed");
}

/// A store whose `list` fails.
struct Broken;

impl why_lib::store::RepoStore for Broken {
    fn list(&self) -> Result<Vec<RepoRecord>, StoreError> {
        Err(StoreError::Backend("db locked".into()))
    }
    fn get(&self, _: &RepoId) -> Result<Option<RepoRecord>, StoreError> {
        Err(StoreError::Backend("db locked".into()))
    }
    fn insert(&self, _: RepoRecord) -> Result<InsertOutcome, StoreError> {
        Err(StoreError::Backend("db locked".into()))
    }
    fn remove(&self, _: &RepoId) -> Result<bool, StoreError> {
        Err(StoreError::Backend("db locked".into()))
    }
    fn base_override(&self, _: &WorktreeId) -> Result<Option<String>, StoreError> {
        Ok(None)
    }
    fn set_base_override(&self, _: &WorktreeId, _: Option<String>) -> Result<(), StoreError> {
        Ok(())
    }
    fn trusted_filters(&self, _: &RepoId) -> Result<std::collections::HashSet<String>, StoreError> {
        Ok(Default::default())
    }
    fn set_trusted_filters(&self, _: &RepoId, _: &[String]) -> Result<bool, StoreError> {
        Ok(false)
    }
}

#[tokio::test]
async fn a_failing_store_keeps_the_old_allowlist() {
    let tmp = short_tmp();
    let path = tmp.path().join("hook-allowlist.json");
    let old = Allowlist::new(1, vec!["/keep".into()], vec!["/keep".into()]);
    sync_file(&path, &old).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let state = AllowlistState::new(Some(path.clone()), &[]);
    let task = tokio::spawn(run(
        git_common::test_runner(),
        Arc::new(Broken),
        state,
        SyncTiming {
            refresh: Duration::from_millis(40),
        },
        Arc::new(tokio::sync::Notify::new()),
        stop_rx,
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    stop_tx.send(true).unwrap();
    task.await.unwrap();
    assert_eq!(Allowlist::read_from(&path).unwrap().repo_roots, ["/keep"]);
}
