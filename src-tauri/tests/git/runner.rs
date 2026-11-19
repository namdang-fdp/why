use std::time::{Duration, Instant};

use why_lib::git::{GitError, GitRunner, read_status};

use crate::common::{TestRepo, git, git_raw};

#[tokio::test]
async fn returns_raw_stdout() {
    let repo = TestRepo::with_initial_commit();
    let out = GitRunner::new()
        .run(&repo.path, ["rev-parse", "--is-inside-work-tree"])
        .await
        .unwrap();
    assert_eq!(out, b"true\n");
}

#[tokio::test]
async fn not_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    let err = GitRunner::new()
        .run(dir.path(), ["rev-parse", "--git-dir"])
        .await
        .unwrap_err();
    assert!(
        matches!(&err, GitError::NotARepository { path } if path == dir.path()),
        "got {err:?}"
    );
}

#[tokio::test]
async fn non_zero_exit_carries_code_and_stderr() {
    let repo = TestRepo::with_initial_commit();
    let err = GitRunner::new()
        .run(&repo.path, ["log", "no-such-ref"])
        .await
        .unwrap_err();
    match &err {
        GitError::NonZeroExit { code, stderr, args } => {
            assert_eq!(*code, Some(128));
            assert!(stderr.contains("no-such-ref"), "stderr: {stderr}");
            assert!(args.contains("log"));
        }
        other => panic!("expected NonZeroExit, got {other:?}"),
    }
    assert!(err.is_exit_code(128));
    assert!(!err.is_exit_code(1));
}

#[tokio::test]
async fn git_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let err = GitRunner::new()
        .with_program("definitely-not-a-real-git-binary")
        .run(dir.path(), ["status"])
        .await
        .unwrap_err();
    assert!(matches!(err, GitError::GitNotFound { .. }), "got {err:?}");
}

#[tokio::test]
async fn missing_directory_is_not_reported_as_git_not_found() {
    // `-C <missing>` fails inside git (exit 128), proving we get a different
    // error than a missing executable.
    let err = GitRunner::new()
        .run("/definitely/not/here".as_ref(), ["status"])
        .await
        .unwrap_err();
    assert!(matches!(err, GitError::NonZeroExit { .. }), "got {err:?}");
}

/// `git -c alias.slow='!sleep N' slow` is a portable way to make git itself run slowly.
fn slow_args(seconds: &str) -> Vec<String> {
    vec![
        "-c".into(),
        format!("alias.slow=!sleep {seconds}"),
        "slow".into(),
    ]
}

#[tokio::test]
async fn timeout_returns_at_the_deadline_instead_of_waiting() {
    let repo = TestRepo::with_initial_commit();
    let runner = GitRunner::with_limits(8, Duration::from_millis(200));
    let started = Instant::now();
    let err = runner.run(&repo.path, slow_args("5")).await.unwrap_err();
    assert!(matches!(err, GitError::Timeout { .. }), "got {err:?}");
    // Returned at the timeout, not after the 5 s sleep. (This shows the call
    // returned; the `sleep` the alias spawned is a grandchild that SIGKILL on
    // git does not reach, so it is not proof the whole process tree died.)
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn concurrency_is_bounded_by_the_semaphore() {
    let repo = TestRepo::with_initial_commit();
    let args = slow_args("0.4");

    // One slot: two 0.4 s commands must run one after the other (>= 0.8 s).
    let serial = GitRunner::with_limits(1, Duration::from_secs(10));
    let started = Instant::now();
    let (a, b) = tokio::join!(serial.run(&repo.path, &args), serial.run(&repo.path, &args));
    a.unwrap();
    b.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(780));

    // Two slots: they overlap (well under the serial 0.8 s plus spawn noise).
    let parallel = GitRunner::with_limits(2, Duration::from_secs(10));
    let started = Instant::now();
    let (a, b) = tokio::join!(
        parallel.run(&repo.path, &args),
        parallel.run(&repo.path, &args)
    );
    a.unwrap();
    b.unwrap();
    assert!(started.elapsed() < Duration::from_millis(780));
}

#[tokio::test]
async fn queueing_time_does_not_count_towards_the_timeout() {
    // One slot, timeout 1 s, two 0.6 s commands: the second waits ~0.6 s for
    // the slot, so total wall time (1.2 s) exceeds the timeout, yet neither
    // command times out because the clock starts after the slot is acquired.
    let repo = TestRepo::with_initial_commit();
    let runner = GitRunner::with_limits(1, Duration::from_secs(1));
    let args = slow_args("0.6");
    let (a, b) = tokio::join!(runner.run(&repo.path, &args), runner.run(&repo.path, &args));
    a.unwrap();
    b.unwrap();
}

/// The reason for `GIT_OPTIONAL_LOCKS=0`: a plain `git status` rewrites the
/// index when file stat data is stale; ours must not touch it.
#[tokio::test]
async fn status_does_not_rewrite_the_index() {
    let repo = TestRepo::with_initial_commit();
    repo.write("a.txt", "same\n");
    repo.commit_all("add a");
    let index = repo.path.join(".git/index");

    // Make the cached stat info stale without changing content: rewrite the
    // identical bytes (new mtime). A refreshing `git status` would rewrite the index.
    std::thread::sleep(Duration::from_millis(1100)); // so mtime visibly differs
    repo.write("a.txt", "same\n");

    let before = std::fs::read(&index).unwrap();
    let snapshot = read_status(&GitRunner::new(), &repo.path).await.unwrap();
    assert!(snapshot.entries.is_empty(), "content is identical: clean");
    assert_eq!(
        std::fs::read(&index).unwrap(),
        before,
        "index must be untouched"
    );
    assert!(!repo.path.join(".git/index.lock").exists());

    // Control: the same status WITHOUT the protection does refresh the index,
    // which proves the setup above would have caught a regression.
    let out = git_raw(&repo.path, ["status", "--porcelain"]);
    assert!(out.status.success());
    assert_ne!(
        std::fs::read(&index).unwrap(),
        before,
        "control failed: plain git status did not refresh the index; test is not meaningful"
    );
    let _ = git(&repo.path, ["status", "--short"]);
}

/// Passive-observer guarantee: a repo-local `core.fsmonitor` hook (which git
/// would EXECUTE during `git status`) must never run through our runner.
#[cfg(unix)]
#[tokio::test]
async fn status_never_runs_a_configured_fsmonitor_hook() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::with_initial_commit();
    let marker = repo.base().join("fsmonitor-ran");
    let hook = repo.base().join("fsmonitor-hook.sh");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    repo.git(["config", "core.fsmonitor", hook.to_str().unwrap()]);

    // Control: plain git DOES run the hook, so a green result below means something.
    let out = git_raw(&repo.path, ["status", "--porcelain"]);
    assert!(out.status.success());
    assert!(
        marker.exists(),
        "control failed: plain git status did not run the hook"
    );
    std::fs::remove_file(&marker).unwrap();

    // Through the runner (`-c core.fsmonitor=false`): the hook must not run.
    read_status(&GitRunner::new(), &repo.path).await.unwrap();
    GitRunner::new()
        .run(&repo.path, ["rev-parse", "HEAD"])
        .await
        .unwrap();
    assert!(
        !marker.exists(),
        "the fsmonitor hook was executed by the runner"
    );
}

#[tokio::test]
async fn reports_the_installed_git_version() {
    let v = why_lib::git::git_version(&GitRunner::new()).await.unwrap();
    assert!(
        v.meets_minimum(),
        "installed git {v} is older than the supported minimum {}",
        why_lib::git::MIN_GIT_VERSION
    );
}
