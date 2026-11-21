//! The git-backed worktree resolver on real repositories.

use std::os::unix::fs::symlink;

use why_lib::collector::GitWorktreeResolver;
use why_lib::events::ingest::WorktreeResolver;
use why_lib::git::{list_worktrees, open_repo};

use super::*;

fn resolver(rig: &Rig) -> GitWorktreeResolver {
    GitWorktreeResolver::new(git_common::test_runner(), Arc::clone(&rig.repos))
}

#[tokio::test]
async fn subdirectories_and_linked_worktrees_resolve_to_their_own_worktree() {
    let rig = Rig::new();
    let sub = rig.repo.path.join("src/deep");
    std::fs::create_dir_all(&sub).unwrap();
    let linked = rig.repo.add_worktree("wt-two", "feature/two");
    let r = resolver(&rig);

    let main = r.resolve(rig.repo.path.to_str().unwrap()).await.unwrap();
    assert_eq!(main.id, rig.main_wt);
    assert_eq!(main.root, rig.repo.path);
    let from_sub = r.resolve(sub.to_str().unwrap()).await.unwrap();
    assert_eq!(
        from_sub.id, rig.main_wt,
        "a subdirectory resolves to its worktree root"
    );
    assert_eq!(from_sub.root, rig.repo.path);

    let other = r.resolve(linked.to_str().unwrap()).await.unwrap();
    assert_ne!(other.id, rig.main_wt);
    assert_eq!(other.root, linked);

    // The ids are exactly those the IPC layer reports for the same worktrees.
    let info = open_repo(&git_common::test_runner(), &rig.repo.path)
        .await
        .unwrap();
    let listed: Vec<String> = list_worktrees(&git_common::test_runner(), &info)
        .await
        .unwrap()
        .iter()
        .map(|e| e.id(&info.common_dir))
        .collect();
    assert!(
        listed.contains(&main.id.0) && listed.contains(&other.id.0),
        "{listed:?}"
    );
}

#[tokio::test]
async fn symlinks_resolve_by_their_target() {
    let rig = Rig::new();
    let link = rig.tmp.path().join("shortcut");
    symlink(&rig.repo.path, &link).unwrap();
    let got = resolver(&rig)
        .resolve(link.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(got.id, rig.main_wt);
}

#[tokio::test]
async fn unregistered_missing_and_empty_cwds_resolve_to_nothing() {
    let rig = Rig::new();
    let stranger = TestRepo::with_initial_commit(); // a real repo, not registered
    let plain = rig.tmp.path().join("not-a-repo");
    std::fs::create_dir(&plain).unwrap();
    let r = resolver(&rig);
    for cwd in [
        stranger.path.to_str().unwrap(),
        plain.to_str().unwrap(),
        "/nonexistent/dir",
        "",
    ] {
        assert_eq!(r.resolve(cwd).await, None, "{cwd:?}");
    }
    assert_eq!(r.cached(), 0, "negative answers are not cached");
}

#[tokio::test]
async fn hits_are_cached_and_the_cache_follows_the_registry() {
    let rig = Rig::new();
    let r = resolver(&rig);
    let cwd = rig.repo.path.to_str().unwrap();
    assert!(r.resolve(cwd).await.is_some());
    assert_eq!(r.cached(), 1);
    assert!(r.resolve(cwd).await.is_some());
    assert_eq!(r.cached(), 1);

    // Unregister the repository: the cached hit must NOT keep attributing.
    let id = rig.repos.list().unwrap().remove(0).id;
    assert!(rig.repos.remove(&id).unwrap());
    assert_eq!(r.resolve(cwd).await, None);
    assert_eq!(r.cached(), 0);
}

#[tokio::test]
async fn a_deleted_worktree_directory_stops_resolving() {
    let rig = Rig::new();
    let linked = rig.repo.add_worktree("gone", "feature/gone");
    let r = resolver(&rig);
    assert!(r.resolve(linked.to_str().unwrap()).await.is_some());
    std::fs::remove_dir_all(&linked).unwrap();
    // The cwd is canonicalized BEFORE the cache is consulted, so a directory
    // that no longer exists is not served from the cache: such events are
    // stored without a worktree (the owner's decision).
    assert_eq!(r.resolve(linked.to_str().unwrap()).await, None);
}

/// The cache is keyed by where the cwd points NOW: a symlink retargeted from
/// one registered worktree to another must not keep answering with the first.
#[tokio::test]
async fn a_retargeted_symlink_resolves_to_the_new_worktree() {
    let rig = Rig::new();
    let linked = rig.repo.add_worktree("second", "feature/second");
    let link = rig.tmp.path().join("current");
    let r = resolver(&rig);

    symlink(&rig.repo.path, &link).unwrap();
    let first = r.resolve(link.to_str().unwrap()).await.unwrap();
    assert_eq!(first.id, rig.main_wt);
    assert_eq!(r.cached(), 1);

    std::fs::remove_file(&link).unwrap();
    symlink(&linked, &link).unwrap();
    let second = r.resolve(link.to_str().unwrap()).await.unwrap();
    assert_ne!(
        second.id, rig.main_wt,
        "the old answer was served from the cache"
    );
    assert_eq!(second.root, linked);
    assert_eq!(r.cached(), 2, "keyed by canonical path");
}

/// A worktree whose `.git` entry was replaced is looked up again.
#[tokio::test]
async fn a_replaced_dot_git_invalidates_the_cached_answer() {
    let rig = Rig::new();
    let linked = rig.repo.add_worktree("third", "feature/third");
    let r = resolver(&rig);
    assert!(r.resolve(linked.to_str().unwrap()).await.is_some());
    // Remove the worktree's `.git` pointer: git no longer sees a checkout here.
    std::fs::remove_file(linked.join(".git")).unwrap();
    assert_eq!(
        r.resolve(linked.to_str().unwrap()).await,
        None,
        "a stale hit was served although the checkout metadata changed"
    );
    assert_eq!(r.cached(), 0);
}

/// A repository nested inside a cached worktree must not keep being answered
/// as that worktree: the outer root's `.git` is unchanged, so only a look at the
/// directories between the cached directory and the root can notice it.
#[tokio::test]
async fn a_nested_repository_that_appears_later_is_not_answered_from_the_cache() {
    let rig = Rig::new();
    let r = resolver(&rig);
    let dir = rig.repo.path.join("vendor/lib");
    std::fs::create_dir_all(&dir).unwrap();

    // Warm: a plain directory of the main checkout.
    let first = r.resolve(dir.to_str().unwrap()).await.unwrap();
    assert_eq!(first.id, rig.main_wt);
    assert_eq!(r.cached(), 1);

    // It becomes a repository of its own (a vendored clone, a submodule).
    git_common::git(&dir, ["init", "-q"]);
    assert!(
        r.resolve(dir.to_str().unwrap()).await.is_none(),
        "an unregistered nested repository belongs to no registered worktree"
    );
    // A directory inside it too, and the outer worktree itself is unaffected.
    let deeper = dir.join("src");
    std::fs::create_dir_all(&deeper).unwrap();
    assert!(r.resolve(deeper.to_str().unwrap()).await.is_none());
    let outer = r.resolve(rig.repo.path.to_str().unwrap()).await.unwrap();
    assert_eq!(outer.id, rig.main_wt);
}
