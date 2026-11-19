use std::collections::HashSet;

use why_lib::git::{GitRunner, list_worktrees, open_repo, worktree_id};

use crate::common::TestRepo;

#[tokio::test]
async fn lists_main_detached_locked_and_prunable() {
    let repo = TestRepo::with_initial_commit();
    let runner = GitRunner::new();

    // Created "outside WHY": plain `git worktree add`.
    let feature = repo.add_worktree("wt feature", "feature/one");
    let locked = repo.add_worktree("wt locked", "locked-branch");
    repo.git([
        "worktree",
        "lock",
        "--reason",
        "agent is running",
        locked.to_str().unwrap(),
    ]);
    let detached = repo.base().join("wt detached");
    repo.git([
        "worktree",
        "add",
        "-q",
        "--detach",
        detached.to_str().unwrap(),
    ]);
    let detached = detached.canonicalize().unwrap();
    // Deleting the directory leaves git's admin entry behind: "prunable".
    let gone = repo.add_worktree("wt gone", "gone-branch");
    std::fs::remove_dir_all(&gone).unwrap();

    let info = open_repo(&runner, &repo.path).await.unwrap();
    let wts = list_worktrees(&runner, &info).await.unwrap();
    assert_eq!(wts.len(), 5, "{wts:#?}");

    let by_path = |p: &std::path::Path| {
        wts.iter()
            .find(|w| w.path == p)
            .unwrap_or_else(|| panic!("no entry for {p:?} in {wts:#?}"))
    };

    // Main worktree first.
    assert!(wts[0].is_main);
    assert_eq!(wts[0].path, repo.path);
    assert_eq!(wts[0].branch_short(), Some("main"));
    assert_eq!(wts.iter().filter(|w| w.is_main).count(), 1);
    assert!(wts[0].head.is_some());

    let f = by_path(&feature);
    assert_eq!(f.branch_short(), Some("feature/one")); // slash in branch name
    assert!(!f.is_detached && f.locked.is_none() && f.prunable.is_none());

    let l = by_path(&locked);
    assert_eq!(l.locked.as_deref(), Some("agent is running"));

    let d = by_path(&detached);
    assert!(d.is_detached);
    assert_eq!(d.branch, None);
    assert!(d.head.is_some());

    // The deleted worktree cannot be canonicalized, but we still report it
    // under its canonical-ish path and flag it prunable.
    let g = by_path(&gone);
    assert!(g.prunable.is_some(), "{g:?}");
    assert_eq!(g.branch_short(), Some("gone-branch"));
}

#[tokio::test]
async fn listing_from_a_linked_worktree_gives_the_same_answer() {
    let repo = TestRepo::with_initial_commit();
    let runner = GitRunner::new();
    let wt = repo.add_worktree("wt", "feature");

    let from_main = list_worktrees(&runner, &open_repo(&runner, &repo.path).await.unwrap())
        .await
        .unwrap();
    let from_linked = list_worktrees(&runner, &open_repo(&runner, &wt).await.unwrap())
        .await
        .unwrap();
    assert_eq!(from_main, from_linked);
}

#[tokio::test]
async fn unborn_repository_has_one_worktree_without_head() {
    let repo = TestRepo::new();
    let runner = GitRunner::new();
    let wts = list_worktrees(&runner, &open_repo(&runner, &repo.path).await.unwrap())
        .await
        .unwrap();
    assert_eq!(wts.len(), 1);
    assert_eq!(wts[0].head, None);
    assert_eq!(wts[0].branch_short(), Some("main"));
}

#[tokio::test]
async fn bare_repository_lists_one_bare_entry() {
    let (_root, bare) = TestRepo::bare_origin_with_commit();
    let runner = GitRunner::new();
    let wts = list_worktrees(&runner, &open_repo(&runner, &bare).await.unwrap())
        .await
        .unwrap();
    assert_eq!(wts.len(), 1);
    assert!(wts[0].is_bare && wts[0].is_main);
}

#[tokio::test]
async fn ids_are_unique_stable_and_independent_of_the_branch() {
    let repo = TestRepo::with_initial_commit();
    let runner = GitRunner::new();
    let wt = repo.add_worktree("wt", "feature");
    let info = open_repo(&runner, &repo.path).await.unwrap();

    let before = list_worktrees(&runner, &info).await.unwrap();
    let ids: Vec<String> = before.iter().map(|w| w.id(&info.common_dir)).collect();
    assert_eq!(
        ids.iter().collect::<HashSet<_>>().len(),
        ids.len(),
        "unique"
    );
    assert_eq!(ids[1], worktree_id(&info.common_dir, &wt));

    // An agent switches branch inside the worktree: same worktree, same id.
    repo.git(["branch", "other"]);
    crate::common::git(&wt, ["switch", "-q", "other"]);
    let after = list_worktrees(&runner, &info).await.unwrap();
    assert_eq!(after[1].branch_short(), Some("other"));
    assert_eq!(after[1].id(&info.common_dir), ids[1]);

    // Listing again gives the same ids (deterministic).
    let again: Vec<String> = list_worktrees(&runner, &info)
        .await
        .unwrap()
        .iter()
        .map(|w| w.id(&info.common_dir))
        .collect();
    assert_eq!(again, ids);
}

#[tokio::test]
async fn a_deleted_worktree_keeps_the_id_it_had_while_it_existed() {
    let repo = TestRepo::with_initial_commit();
    let runner = GitRunner::new();
    let wt = repo.add_worktree("soon gone", "tmp");
    let info = open_repo(&runner, &repo.path).await.unwrap();

    let id_alive = worktree_id(&info.common_dir, &wt);
    std::fs::remove_dir_all(&wt).unwrap();

    let wts = list_worktrees(&runner, &info).await.unwrap();
    let gone = wts
        .iter()
        .find(|w| w.prunable.is_some())
        .expect("prunable entry");
    assert_eq!(gone.id(&info.common_dir), id_alive);
}

#[tokio::test]
async fn a_locked_worktree_with_a_deleted_directory_is_not_prunable() {
    // Pitfall for callers: `prunable: None` does not mean the directory exists.
    let repo = TestRepo::with_initial_commit();
    let runner = GitRunner::new();
    let wt = repo.add_worktree("locked gone", "lk");
    repo.git(["worktree", "lock", wt.to_str().unwrap()]);
    std::fs::remove_dir_all(&wt).unwrap();

    let info = open_repo(&runner, &repo.path).await.unwrap();
    let wts = list_worktrees(&runner, &info).await.unwrap();
    let entry = wts.iter().find(|w| w.branch_short() == Some("lk")).unwrap();
    assert_eq!(entry.locked.as_deref(), Some(""));
    assert_eq!(entry.prunable, None);
    assert!(!entry.path.exists());

    // Running git in it fails cleanly (exit 128), it does not hang or panic.
    let err = why_lib::git::read_status(&runner, &entry.path)
        .await
        .unwrap_err();
    assert!(
        matches!(err, why_lib::git::GitError::NonZeroExit { .. }),
        "{err:?}"
    );
}
