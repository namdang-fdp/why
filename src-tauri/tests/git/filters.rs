//! The filter-driver guard at the git layer (WHY-39), against real repositories.
//!
//! Every "WHY never ran the program" assertion has a POSITIVE CONTROL: the same
//! fixture, read by plain `git status`, DOES create the marker. Without it a
//! passing test could just mean the files were not stat-dirty.

use std::collections::HashSet;

use why_lib::git::{
    ConfigScope, GitError, GitRunner, guard_working_tree, read_changes, scan_filter_drivers,
};

use crate::common::{Marked, TestRepo, dirty, git, status_of, test_runner, write_in};

/// Repo with a committed baseline, THEN a repository-local clean driver for
/// every file. Returns the driver recorder.
fn repo_with_local_clean_filter() -> (TestRepo, Marked) {
    let repo = TestRepo::with_initial_commit();
    repo.write("a.txt", "a\n");
    repo.commit_all("a");
    let driver = Marked::new(&repo);
    repo.write(".gitattributes", "* filter=evil\n");
    repo.git(["config", "filter.evil.clean", &driver.command("clean")]);
    (repo, driver)
}

#[tokio::test]
async fn control_plain_git_status_does_run_the_driver() {
    let (repo, driver) = repo_with_local_clean_filter();
    dirty(&repo.path);
    // Fixture setup git (isolated, hermetic), exactly the commands WHY runs.
    git(&repo.path, ["-c", "core.fsmonitor=false", "status"]);
    assert!(driver.ran(), "fixture is not live: the driver never ran");
}

#[tokio::test]
async fn scan_reports_the_local_driver_with_its_scope() {
    let (repo, driver) = repo_with_local_clean_filter();
    let drivers = scan_filter_drivers(&test_runner(), &repo.path)
        .await
        .unwrap();
    assert_eq!(drivers.len(), 1);
    let d = &drivers[0];
    assert_eq!(d.name, "evil");
    let clean = d.clean.as_ref().unwrap();
    assert_eq!(clean.value, driver.command("clean"));
    assert_eq!(clean.scope, ConfigScope::Local);
    assert!(clean.origin.ends_with(".git/config"), "{}", clean.origin);
    assert!(d.needs_trust());
    // Scanning only reads config: it never ran the program.
    assert!(!driver.ran());
}

#[tokio::test]
async fn guard_refuses_and_nothing_runs_until_the_driver_is_trusted() {
    let (repo, driver) = repo_with_local_clean_filter();
    dirty(&repo.path);
    let runner = test_runner();

    let err = guard_working_tree(&runner, &repo.path, &HashSet::new())
        .await
        .unwrap_err();
    let GitError::UntrustedFilters { drivers } = &err else {
        panic!("expected UntrustedFilters, got {err:?}");
    };
    assert_eq!(drivers[0].name, "evil");
    assert!(!driver.ran(), "the guard itself ran the program");

    // Trusting exactly this configuration lets the read through, and the read
    // really does run the driver (positive control through WHY's own code).
    let trusted: HashSet<String> = [drivers[0].fingerprint()].into();
    let permit = guard_working_tree(&runner, &repo.path, &trusted)
        .await
        .unwrap();
    why_lib::git::read_status(&runner, &permit).await.unwrap();
    assert!(driver.ran(), "trusted: WHY should have run the driver");
}

#[tokio::test]
async fn a_changed_command_is_not_the_trusted_one() {
    let (repo, driver) = repo_with_local_clean_filter();
    let runner = test_runner();
    let first = scan_filter_drivers(&runner, &repo.path).await.unwrap();
    let trusted: HashSet<String> = [first[0].fingerprint()].into();
    guard_working_tree(&runner, &repo.path, &trusted)
        .await
        .expect("trusted as configured");

    repo.git(["config", "filter.evil.clean", &driver.command("other-arg")]);
    assert!(
        guard_working_tree(&runner, &repo.path, &trusted)
            .await
            .is_err(),
        "an edited command must be asked about again"
    );
}

#[tokio::test]
async fn repository_without_filters_is_unaffected() {
    let repo = TestRepo::with_initial_commit();
    dirty(&repo.path);
    // Exit code 1 with empty output from `git config --get-regexp`: no drivers.
    assert!(
        scan_filter_drivers(&test_runner(), &repo.path)
            .await
            .unwrap()
            .is_empty()
    );
    status_of(&test_runner(), &repo.path).await.unwrap();
}

#[tokio::test]
async fn global_driver_is_allowed_and_a_local_override_of_it_is_not() {
    let (repo, _local) = repo_with_local_clean_filter();
    repo.git(["config", "--unset", "filter.evil.clean"]);
    let user = Marked::new(&repo);
    // The USER's config: a global file, as git-lfs would write.
    let global = repo.root.path().join("global.gitconfig");
    std::fs::write(
        &global,
        format!("[filter \"evil\"]\n\tclean = {}\n", user.command("clean")),
    )
    .unwrap();
    let runner = GitRunner::new()
        .with_env("GIT_CONFIG_GLOBAL", &global)
        .with_env("GIT_CONFIG_NOSYSTEM", "1");
    dirty(&repo.path);

    let drivers = scan_filter_drivers(&runner, &repo.path).await.unwrap();
    assert_eq!(
        drivers[0].clean.as_ref().unwrap().scope,
        ConfigScope::Global
    );
    assert!(!drivers[0].needs_trust());
    let permit = guard_working_tree(&runner, &repo.path, &HashSet::new())
        .await
        .expect("the user's own driver is allowed without asking");
    why_lib::git::read_status(&runner, &permit).await.unwrap();
    assert!(user.ran(), "the user's own driver runs, as in `git status`");
    user.reset();

    // Now the repository overrides the command of that same driver name:
    // the program that would run is the repository's, so it is untrusted.
    let evil = Marked::new(&repo);
    repo.git(["config", "filter.evil.clean", &evil.command("local")]);
    let err = guard_working_tree(&runner, &repo.path, &HashSet::new())
        .await
        .unwrap_err();
    assert!(matches!(err, GitError::UntrustedFilters { .. }), "{err:?}");
    assert!(!evil.ran() && !user.ran());
}

#[tokio::test]
async fn long_running_process_drivers_are_guarded_too() {
    let repo = TestRepo::with_initial_commit();
    repo.write("a.txt", "a\n");
    repo.commit_all("a");
    let driver = Marked::new(&repo);
    repo.write(".gitattributes", "* filter=evil\n");
    // `process`, not `clean`: the long-running protocol variant.
    repo.git(["config", "filter.evil.process", &driver.command("process")]);
    dirty(&repo.path);

    // Control: git starts the program (it fails the handshake; irrelevant).
    git(&repo.path, ["status"]);
    assert!(driver.ran(), "control: plain git runs a process driver");
    driver.reset();

    let runner = test_runner();
    let err = guard_working_tree(&runner, &repo.path, &HashSet::new())
        .await
        .unwrap_err();
    assert!(matches!(err, GitError::UntrustedFilters { .. }));
    assert!(!driver.ran());
}

#[tokio::test]
async fn a_filter_configured_only_inside_a_submodule_never_runs() {
    // The hole this pins: `git status` starts a nested status in each submodule,
    // which reads the SUBMODULE's own config (`.git/modules/<n>/config`), a file
    // the outer repository's scan never sees.
    let inner = TestRepo::with_initial_commit();
    inner.write("f.txt", "inner\n");
    inner.commit_all("inner");

    let outer = TestRepo::with_initial_commit();
    outer.git([
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        inner.path.to_str().unwrap(),
        "sub",
    ]);
    outer.commit_all("add submodule");

    let sub = outer.path.join("sub");
    let driver = Marked::new(&outer);
    write_in(&sub, ".gitattributes", b"* filter=evil\n");
    git(
        &sub,
        ["config", "filter.evil.clean", &driver.command("clean")],
    );
    write_in(&sub, "f.txt", b"INNER\n"); // same length as "inner\n"

    // Control: a plain outer `git status` does run the submodule's driver.
    git(&outer.path, ["-c", "core.fsmonitor=false", "status"]);
    assert!(
        driver.ran(),
        "control: outer status recursed into the submodule"
    );
    driver.reset();

    // WHY's status (and the changes list built on it) must not.
    let runner = test_runner();
    let permit = crate::common::permit(&runner, &outer.path).await.unwrap();
    why_lib::git::read_status(&runner, &permit).await.unwrap();
    read_changes(
        &runner,
        &permit,
        &why_lib::git::OldSide::Head,
        why_lib::git::Target::WorkingTree {
            include_untracked: true,
        },
    )
    .await
    .unwrap();
    assert!(
        !driver.ran(),
        "WHY ran a filter driver that lives in a submodule's config"
    );
}

#[tokio::test]
async fn scan_failure_is_an_error_not_an_empty_list() {
    // A directory git cannot enter: the scan fails, and that must NOT be read as
    // "no filters" (the guard fails closed). Only exit 1 with empty stderr, git's
    // "nothing matched", means "no drivers".
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    let err = guard_working_tree(&test_runner(), &missing, &HashSet::new()).await;
    assert!(err.is_err());
}

#[tokio::test]
async fn a_legacy_git_config_variable_cannot_hide_a_local_driver() {
    // `GIT_CONFIG` is honoured by `git config` (the scan) but ignored by
    // `status`/`diff` (the read). If it survived, the scan would read an empty
    // file, find no drivers, and bless a read that still sees the local driver.
    let (repo, driver) = repo_with_local_clean_filter();
    dirty(&repo.path);
    let empty = repo.root.path().join("empty.gitconfig");
    std::fs::write(&empty, "").unwrap();
    let runner = test_runner().with_env("GIT_CONFIG", &empty);

    let err = guard_working_tree(&runner, &repo.path, &HashSet::new())
        .await
        .unwrap_err();
    let GitError::UntrustedFilters { drivers } = &err else {
        panic!("expected the local driver to block, got {err:?}");
    };
    assert!(!driver.ran());

    // Control: trusted, the very same runner does run the driver.
    let trusted: HashSet<String> = [drivers[0].fingerprint()].into();
    let permit = guard_working_tree(&runner, &repo.path, &trusted)
        .await
        .unwrap();
    why_lib::git::read_status(&runner, &permit).await.unwrap();
    assert!(driver.ran(), "control: the fixture driver is live");
}

#[tokio::test]
async fn a_driver_injected_through_the_process_environment_needs_trust() {
    // `GIT_CONFIG_COUNT/KEY_n/VALUE_n` apply to every git command (scan and read
    // agree), scope "command". A repository can influence the environment WHY
    // is started from (direnv...), so this is not treated as the user's own.
    let repo = TestRepo::with_initial_commit();
    repo.write("a.txt", "a\n");
    repo.commit_all("a");
    let driver = Marked::new(&repo);
    repo.write(".gitattributes", "* filter=evil\n");
    dirty(&repo.path);
    let runner = test_runner()
        .with_env("GIT_CONFIG_COUNT", "1")
        .with_env("GIT_CONFIG_KEY_0", "filter.evil.clean")
        .with_env("GIT_CONFIG_VALUE_0", driver.command("env"));

    let err = guard_working_tree(&runner, &repo.path, &HashSet::new())
        .await
        .unwrap_err();
    let GitError::UntrustedFilters { drivers } = &err else {
        panic!("expected a block, got {err:?}");
    };
    assert_eq!(
        drivers[0].clean.as_ref().unwrap().scope,
        ConfigScope::Command
    );
    assert!(!driver.ran());

    let trusted: HashSet<String> = [drivers[0].fingerprint()].into();
    let permit = guard_working_tree(&runner, &repo.path, &trusted)
        .await
        .unwrap();
    why_lib::git::read_status(&runner, &permit).await.unwrap();
    assert!(driver.ran(), "control: the injected driver is live");
}
