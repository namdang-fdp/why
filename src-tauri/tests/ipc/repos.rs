use std::sync::Arc;

use why_lib::contracts::{IpcError, RepoId};
use why_lib::ipc::repos::{add_repository_impl, list_repositories_impl, remove_repository_impl};
use why_lib::state::AppState;
use why_lib::store::MemoryRepoStore;

use crate::common::{TestRepo, configure, git, test_runner, write_in};

fn state() -> AppState {
    // Isolated runner: the developer's git config must not influence the result.
    AppState::new(test_runner(), Arc::new(MemoryRepoStore::new()))
}

fn path_str(p: &std::path::Path) -> &str {
    p.to_str().expect("temp paths are UTF-8")
}

#[tokio::test]
async fn adding_via_main_and_linked_worktree_gives_one_repository() {
    let repo = TestRepo::with_initial_commit();
    let linked = repo.add_worktree("linked", "feature");
    let state = state();

    let via_main = add_repository_impl(&state, path_str(&repo.path))
        .await
        .unwrap();
    let via_linked = add_repository_impl(&state, path_str(&linked))
        .await
        .unwrap();

    // Same repository: same id, and the record describes the MAIN worktree even
    // when it was added through the linked one.
    assert_eq!(via_main, via_linked);
    assert_eq!(via_linked.name, "repo");
    assert_eq!(via_linked.root_path, path_str(&repo.path));

    let listed = list_repositories_impl(&state).await.unwrap();
    assert_eq!(listed, vec![via_main]);
}

#[tokio::test]
async fn adding_from_a_subdirectory_works() {
    let repo = TestRepo::with_initial_commit();
    repo.write("deep/er/file.txt", "x");
    let sub = repo.path.join("deep/er");

    let added = add_repository_impl(&state(), path_str(&sub)).await.unwrap();
    assert_eq!(added.root_path, path_str(&repo.path));
}

#[tokio::test]
async fn two_repositories_are_listed_by_name() {
    let a = TestRepo::new_named("zeta");
    let b = TestRepo::new_named("alpha");
    let state = state();
    add_repository_impl(&state, path_str(&a.path))
        .await
        .unwrap();
    add_repository_impl(&state, path_str(&b.path))
        .await
        .unwrap();

    let names: Vec<_> = list_repositories_impl(&state)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(names, ["alpha", "zeta"]);
}

#[tokio::test]
async fn a_directory_that_is_not_a_repository_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let err = add_repository_impl(&state(), path_str(dir.path()))
        .await
        .unwrap_err();
    assert_eq!(err, IpcError::NotARepo);
}

#[tokio::test]
async fn bare_repositories_are_invalid_input() {
    let (_root, bare) = TestRepo::bare_origin_with_commit();
    let err = add_repository_impl(&state(), path_str(&bare))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, IpcError::InvalidInput { message } if message.contains("bare")),
        "{err:?}"
    );
}

#[tokio::test]
async fn missing_empty_and_file_paths_are_invalid_input() {
    let state = state();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "x").unwrap();

    for bad in [
        "".to_string(),
        "   ".to_string(),
        path_str(&dir.path().join("does-not-exist")).to_string(),
        path_str(&file).to_string(),
    ] {
        let err = add_repository_impl(&state, &bad).await.unwrap_err();
        assert!(
            matches!(err, IpcError::InvalidInput { .. }),
            "{bad:?}: {err:?}"
        );
    }
    assert!(list_repositories_impl(&state).await.unwrap().is_empty());
}

#[tokio::test]
async fn remove_unregisters_and_unknown_id_is_not_found() {
    let repo = TestRepo::with_initial_commit();
    let state = state();
    let added = add_repository_impl(&state, path_str(&repo.path))
        .await
        .unwrap();

    remove_repository_impl(&state, &added.id).await.unwrap();
    assert!(list_repositories_impl(&state).await.unwrap().is_empty());
    // The repository on disk is untouched.
    assert!(repo.path.join(".git").exists());

    // Removing again, or an id that never existed.
    assert_eq!(
        remove_repository_impl(&state, &added.id).await,
        Err(IpcError::NotFound)
    );
    assert_eq!(
        remove_repository_impl(&state, &RepoId("nope".into())).await,
        Err(IpcError::NotFound)
    );
}

#[tokio::test]
async fn submodule_is_registered_under_its_checkout_not_the_git_metadata() {
    let sub_origin = TestRepo::with_initial_commit();
    let parent = TestRepo::with_initial_commit();
    // `protocol.file.allow=always`: git refuses local-path submodules by default.
    parent.git([
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        path_str(&sub_origin.path),
        "mods/inner",
    ]);
    let checkout = parent.path.join("mods/inner").canonicalize().unwrap();

    let added = add_repository_impl(&state(), path_str(&checkout))
        .await
        .unwrap();
    // Git's own `worktree list` would name `<parent>/.git/modules/mods/inner` here.
    assert_eq!(added.root_path, path_str(&checkout));
    assert_eq!(added.name, "inner");
}

/// `git init --separate-git-dir`: the checkout has a `.git` FILE and the real git
/// directory lives elsewhere, so `worktree list` reports that directory.
struct SeparateGitDir {
    _root: tempfile::TempDir,
    checkout: std::path::PathBuf,
}

fn separate_git_dir_repo() -> SeparateGitDir {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let checkout = base.join("work");
    std::fs::create_dir(&checkout).unwrap();
    let git_dir = base.join("meta.git");
    git(
        &base,
        [
            "init".as_ref(),
            "-q".as_ref(),
            "-b".as_ref(),
            "main".as_ref(),
            "--separate-git-dir".as_ref(),
            git_dir.as_os_str(),
            checkout.as_os_str(),
        ],
    );
    configure(&checkout);
    write_in(&checkout, "README.md", b"hello\n");
    git(&checkout, ["add", "-A"]);
    git(&checkout, ["commit", "-q", "-m", "initial"]);
    SeparateGitDir {
        _root: root,
        checkout: checkout.canonicalize().unwrap(),
    }
}

#[tokio::test]
async fn separate_git_dir_checkout_is_registered_under_the_checkout() {
    let repo = separate_git_dir_repo();
    let added = add_repository_impl(&state(), path_str(&repo.checkout))
        .await
        .unwrap();
    assert_eq!(added.root_path, path_str(&repo.checkout));
    assert_eq!(added.name, "work");
}

#[tokio::test]
async fn linked_worktree_of_a_separate_git_dir_repo_falls_back_to_the_given_checkout() {
    let repo = separate_git_dir_repo();
    let linked = repo.checkout.parent().unwrap().join("linked");
    git(
        &repo.checkout,
        [
            "worktree".as_ref(),
            "add".as_ref(),
            "-q".as_ref(),
            "-b".as_ref(),
            "feature".as_ref(),
            linked.as_os_str(),
        ],
    );
    let linked = linked.canonicalize().unwrap();
    let state = state();

    // Git names the metadata directory as the main worktree; that fails
    // verification, so the checkout the user gave is used instead.
    let via_linked = add_repository_impl(&state, path_str(&linked))
        .await
        .unwrap();
    assert_eq!(via_linked.root_path, path_str(&linked));

    // Same repository either way: adding the main checkout later finds the
    // existing registration (the first one wins; insert never overwrites).
    let via_main = add_repository_impl(&state, path_str(&repo.checkout))
        .await
        .unwrap();
    assert_eq!(via_main, via_linked);
    assert_eq!(list_repositories_impl(&state).await.unwrap().len(), 1);
}
