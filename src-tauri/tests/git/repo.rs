use why_lib::git::{GitError, GitRunner, open_repo};

use crate::common::{TestRepo, git};

#[tokio::test]
async fn normal_repository() {
    let repo = TestRepo::with_initial_commit();
    let info = open_repo(&GitRunner::new(), &repo.path).await.unwrap();
    assert_eq!(info.toplevel.as_deref(), Some(repo.path.as_path()));
    assert_eq!(info.common_dir, repo.path.join(".git"));
    assert_eq!(info.git_dir, info.common_dir); // main worktree: same directory
    assert!(!info.is_bare);
}

#[tokio::test]
async fn works_from_a_subdirectory() {
    let repo = TestRepo::with_initial_commit();
    repo.write("deep/er/file.txt", "x");
    let info = open_repo(&GitRunner::new(), &repo.path.join("deep/er"))
        .await
        .unwrap();
    assert_eq!(info.toplevel.as_deref(), Some(repo.path.as_path()));
}

#[tokio::test]
async fn repository_without_commits_is_still_a_repository() {
    let repo = TestRepo::new();
    let info = open_repo(&GitRunner::new(), &repo.path).await.unwrap();
    assert_eq!(info.toplevel.as_deref(), Some(repo.path.as_path()));
}

#[tokio::test]
async fn linked_worktree_has_private_git_dir_and_shared_common_dir() {
    let repo = TestRepo::with_initial_commit();
    let wt = repo.add_worktree("wt one", "feature");

    let info = open_repo(&GitRunner::new(), &wt).await.unwrap();
    assert_eq!(info.toplevel.as_deref(), Some(wt.as_path()));
    assert_eq!(info.common_dir, repo.path.join(".git"));
    // Git sanitises the directory name for its admin dir: "wt one" -> "wt-one".
    assert_eq!(info.git_dir, repo.path.join(".git/worktrees/wt-one"));
    assert_ne!(info.git_dir, info.common_dir);

    // The linked worktree's `.git` is a FILE pointing at its private git dir.
    let dot_git = std::fs::read_to_string(wt.join(".git")).unwrap();
    assert!(dot_git.starts_with("gitdir: "), "{dot_git}");
}

#[tokio::test]
async fn bare_repository_has_no_toplevel() {
    let (_root, bare) = TestRepo::bare_origin_with_commit();
    let info = open_repo(&GitRunner::new(), &bare).await.unwrap();
    assert!(info.is_bare);
    assert_eq!(info.toplevel, None);
    assert_eq!(info.common_dir, bare);
}

#[tokio::test]
async fn not_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    let err = open_repo(&GitRunner::new(), dir.path()).await.unwrap_err();
    assert!(
        matches!(err, GitError::NotARepository { .. }),
        "got {err:?}"
    );
}

#[tokio::test]
async fn inside_dot_git_directory_is_an_error_not_a_bare_repo() {
    let repo = TestRepo::with_initial_commit();
    let err = open_repo(&GitRunner::new(), &repo.path.join(".git"))
        .await
        .unwrap_err();
    assert!(matches!(err, GitError::NonZeroExit { .. }), "got {err:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn paths_are_canonicalized_through_symlinks() {
    let repo = TestRepo::with_initial_commit();
    let link = repo.base().join("link-to-repo");
    std::os::unix::fs::symlink(&repo.path, &link).unwrap();

    let info = open_repo(&GitRunner::new(), &link).await.unwrap();
    assert_eq!(info.toplevel.as_deref(), Some(repo.path.as_path()));
    assert_eq!(info.common_dir, repo.path.join(".git"));
    let _ = git(&repo.path, ["status", "--short"]);
}

/// `rev-parse` has no `-z`, so this used to break: a repository directory whose
/// name contains a newline is valid on Unix.
#[cfg(unix)]
#[tokio::test]
async fn repository_directory_with_a_newline_in_its_name() {
    let repo = TestRepo::new_named("re\npo");
    repo.write("a.txt", "x\n");
    repo.commit_all("first");
    let wt = repo.add_worktree("wt\nlinked", "feature");
    let runner = GitRunner::new();

    let info = open_repo(&runner, &repo.path).await.unwrap();
    assert_eq!(info.toplevel.as_deref(), Some(repo.path.as_path()));
    assert_eq!(info.common_dir, repo.path.join(".git"));

    let linked = open_repo(&runner, &wt).await.unwrap();
    assert_eq!(linked.toplevel.as_deref(), Some(wt.as_path()));
    assert_eq!(linked.common_dir, info.common_dir);
    assert!(linked.git_dir.starts_with(repo.path.join(".git/worktrees")));

    // And the rest of the pipeline copes with such paths.
    let wts = why_lib::git::list_worktrees(&runner, &info).await.unwrap();
    assert_eq!(wts.len(), 2);
    assert_eq!(wts[1].path, wt);
    assert!(
        why_lib::git::read_status(&runner, &wt)
            .await
            .unwrap()
            .entries
            .is_empty()
    );
}
