//! The filter-driver guard through the IPC commands (WHY-39): no command that
//! reads a working tree may run a repository-controlled filter program until the
//! user trusts it. Every "never ran" assertion has a positive control (the same
//! fixture, trusted, DOES run the program).

use std::path::PathBuf;
use std::sync::Arc;

use why_lib::contracts::{
    DiffMode, FilterGuardStatus, FilterScope, IpcError, RepoId, RepositorySecurity, Worktree,
};
use why_lib::git::GitRunner;
use why_lib::ipc::changes::{get_changes_impl, get_file_diff_impl, list_worktrees_impl};
use why_lib::ipc::repos::{add_repository_impl, remove_repository_impl, set_base_impl};
use why_lib::ipc::security::{
    get_repository_security_impl, revoke_repository_filters_impl, trust_repository_filters_impl,
};
use why_lib::state::AppState;
use why_lib::store::MemoryRepoStore;

use crate::common::{TestRepo, git, test_runner, write_executable_script};

/// Records every invocation of the driver, then passes the file through.
struct Driver {
    marker: PathBuf,
    script: PathBuf,
}

impl Driver {
    fn new(repo: &TestRepo, name: &str) -> Self {
        let marker = repo.root.path().join(format!("{name}.marker"));
        let script = repo.root.path().join(format!("{name}.sh"));
        write_executable_script(
            &script,
            &format!("echo \"$@\" >> '{}'\ncat\n", marker.display()),
        );
        Driver { marker, script }
    }
    fn ran(&self) -> bool {
        self.marker.exists()
    }
    fn reset(&self) {
        let _ = std::fs::remove_file(&self.marker);
    }
    fn command(&self, arg: &str) -> String {
        format!("'{}' {arg}", self.script.display())
    }
}

struct Fx {
    repo: TestRepo,
    /// The linked worktree (branch `feature`, base `main`).
    wt: PathBuf,
    state: AppState,
    repo_id: RepoId,
    driver: Driver,
}

impl Fx {
    /// A repository with a committed baseline and a linked worktree; NO filter
    /// driver configured yet.
    async fn plain(runner: GitRunner) -> Fx {
        let repo = TestRepo::with_initial_commit();
        repo.write("a.txt", "a\n");
        repo.commit_all("a");
        let wt = repo.add_worktree("wt", "feature");
        let state = AppState::new(runner, Arc::new(MemoryRepoStore::new()));
        let added = add_repository_impl(&state, repo.path.to_str().unwrap())
            .await
            .unwrap();
        let driver = Driver::new(&repo, "evil");
        Fx {
            repo,
            wt,
            state,
            repo_id: added.id,
            driver,
        }
    }

    /// `plain` plus a repository-local clean driver for every file.
    async fn with_local_clean() -> Fx {
        let fx = Self::plain(test_runner()).await;
        fx.repo.write(".gitattributes", "* filter=evil\n");
        fx.repo
            .git(["config", "filter.evil.clean", &fx.driver.command("clean")]);
        fx.dirty();
        fx
    }

    /// Same-length rewrites in both worktrees: git must hash them (see
    /// `tests/git/filters.rs::dirty`).
    fn dirty(&self) {
        for dir in [&self.repo.path, &self.wt] {
            crate::common::write_in(dir, "a.txt", b"b\n");
            crate::common::write_in(dir, "README.md", b"world\n");
        }
        // The linked worktree needs the attributes too (untracked file).
        crate::common::write_in(&self.wt, ".gitattributes", b"* filter=evil\n");
    }

    fn security(&self) -> impl Future<Output = Result<RepositorySecurity, IpcError>> + '_ {
        get_repository_security_impl(&self.state, &self.repo_id)
    }

    async fn worktrees(&self) -> Vec<Worktree> {
        list_worktrees_impl(&self.state, &self.repo_id)
            .await
            .unwrap()
    }

    /// Id of the linked worktree.
    async fn wt_id(&self) -> why_lib::contracts::WorktreeId {
        self.worktrees()
            .await
            .into_iter()
            .find(|w| !w.is_main)
            .unwrap()
            .id
    }
}

fn is_blocked(err: &IpcError) -> bool {
    matches!(err, IpcError::UntrustedFilters { .. })
}

#[tokio::test]
async fn control_the_fixture_really_runs_the_driver_under_plain_git() {
    let fx = Fx::with_local_clean().await;
    git(&fx.wt, ["-c", "core.fsmonitor=false", "status"]);
    assert!(fx.driver.ran(), "fixture is not live");
}

#[tokio::test]
async fn untrusted_local_driver_blocks_every_working_tree_read() {
    let fx = Fx::with_local_clean().await;
    let wt_id = fx.wt_id().await;
    fx.driver.reset();

    // list_worktrees: still lists everything, each worktree carrying the reason.
    let list = fx.worktrees().await;
    assert_eq!(list.len(), 2);
    for w in &list {
        assert!(
            matches!(&w.load_error, Some(e) if is_blocked(e)),
            "{} should be blocked: {:?}",
            w.path,
            w.load_error
        );
        assert_eq!(w.change_count, 0);
    }

    // get_changes in every mode.
    for mode in [DiffMode::Staged, DiffMode::Uncommitted, DiffMode::VsBase] {
        let err = get_changes_impl(&fx.state, &wt_id, mode).await.unwrap_err();
        assert!(is_blocked(&err), "{mode:?}: {err:?}");
        let err = get_file_diff_impl(&fx.state, &wt_id, "a.txt", mode)
            .await
            .unwrap_err();
        assert!(is_blocked(&err), "{mode:?}: {err:?}");
    }

    // set_base builds a refreshed worktree view, which reads the tree.
    let err = set_base_impl(&fx.state, &wt_id, "main").await.unwrap_err();
    assert!(is_blocked(&err), "{err:?}");

    assert!(!fx.driver.ran(), "a command ran the untrusted driver");

    // The report explains it.
    let sec = fx.security().await.unwrap();
    assert_eq!(sec.status, FilterGuardStatus::Blocked);
    assert!(!sec.fingerprint.is_empty());
    assert_eq!(sec.drivers.len(), 1);
    let d = &sec.drivers[0];
    assert_eq!(d.name, "evil");
    assert_eq!(
        d.clean.as_deref(),
        Some(fx.driver.command("clean").as_str())
    );
    assert_eq!(d.scope, FilterScope::Local);
    assert!(d.needs_trust && !d.trusted && d.referenced);
    assert!(!fx.driver.ran(), "reporting ran the driver");
}

#[tokio::test]
async fn trusting_lets_reads_through_and_they_do_run_the_driver() {
    let fx = Fx::with_local_clean().await;
    let wt_id = fx.wt_id().await;
    fx.driver.reset();

    let sec = fx.security().await.unwrap();
    let after = trust_repository_filters_impl(&fx.state, &fx.repo_id, &sec.fingerprint)
        .await
        .unwrap();
    assert_eq!(after.status, FilterGuardStatus::Trusted);
    assert!(after.drivers[0].trusted);

    // Positive control: now WHY's own commands run the program.
    let changes = get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted)
        .await
        .unwrap();
    assert!(!changes.is_empty());
    assert!(fx.driver.ran(), "trusted, yet WHY never ran the driver");
    fx.driver.reset();
    let list = fx.worktrees().await;
    assert!(list.iter().all(|w| w.load_error.is_none()));
    get_changes_impl(&fx.state, &wt_id, DiffMode::VsBase)
        .await
        .unwrap();
    get_file_diff_impl(&fx.state, &wt_id, "a.txt", DiffMode::Uncommitted)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_stale_fingerprint_is_refused_and_trusts_nothing() {
    let fx = Fx::with_local_clean().await;
    let seen = fx.security().await.unwrap().fingerprint;

    // The configuration changes after the user saw it, before they clicked.
    fx.repo
        .git(["config", "filter.evil.clean", &fx.driver.command("swapped")]);
    let err = trust_repository_filters_impl(&fx.state, &fx.repo_id, &seen)
        .await
        .unwrap_err();
    assert!(matches!(err, IpcError::InvalidInput { .. }), "{err:?}");
    assert!(
        fx.state
            .repos
            .trusted_filters(&fx.repo_id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fx.security().await.unwrap().status,
        FilterGuardStatus::Blocked
    );

    // Nonsense is refused too.
    let err = trust_repository_filters_impl(&fx.state, &fx.repo_id, "bogus")
        .await
        .unwrap_err();
    assert!(matches!(err, IpcError::InvalidInput { .. }));
    assert!(!fx.driver.ran());
}

#[tokio::test]
async fn a_changed_or_new_driver_invalidates_the_trust() {
    let fx = Fx::with_local_clean().await;
    let wt_id = fx.wt_id().await;
    let sec = fx.security().await.unwrap();
    trust_repository_filters_impl(&fx.state, &fx.repo_id, &sec.fingerprint)
        .await
        .unwrap();
    get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted)
        .await
        .unwrap();
    fx.driver.reset();

    // 1. The trusted driver's command is edited.
    fx.repo
        .git(["config", "filter.evil.clean", &fx.driver.command("edited")]);
    let err = get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted)
        .await
        .unwrap_err();
    assert!(is_blocked(&err), "{err:?}");
    assert!(!fx.driver.ran(), "the edited command ran without asking");
    assert_eq!(
        fx.security().await.unwrap().status,
        FilterGuardStatus::Blocked
    );

    // 2. Back to exactly what was trusted: allowed again (it IS what the user
    //    approved), then a SECOND driver appears: blocked again.
    fx.repo
        .git(["config", "filter.evil.clean", &fx.driver.command("clean")]);
    get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted)
        .await
        .unwrap();
    fx.driver.reset();
    let second = Driver::new(&fx.repo, "second");
    fx.repo
        .git(["config", "filter.second.clean", &second.command("clean")]);
    let err = get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted)
        .await
        .unwrap_err();
    assert!(is_blocked(&err));
    assert!(!fx.driver.ran() && !second.ran());
}

#[tokio::test]
async fn revoking_blocks_again() {
    let fx = Fx::with_local_clean().await;
    let wt_id = fx.wt_id().await;
    let fp = fx.security().await.unwrap().fingerprint;
    trust_repository_filters_impl(&fx.state, &fx.repo_id, &fp)
        .await
        .unwrap();
    get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted)
        .await
        .unwrap();
    fx.driver.reset();

    let after = revoke_repository_filters_impl(&fx.state, &fx.repo_id)
        .await
        .unwrap();
    assert_eq!(
        after.security.expect("rescan worked").status,
        FilterGuardStatus::Blocked
    );
    assert_eq!(after.rescan_error, None);
    let err = get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted)
        .await
        .unwrap_err();
    assert!(is_blocked(&err));
    assert!(!fx.driver.ran());
}

#[tokio::test]
async fn removing_and_re_adding_a_repository_does_not_inherit_trust() {
    let fx = Fx::with_local_clean().await;
    let fp = fx.security().await.unwrap().fingerprint;
    trust_repository_filters_impl(&fx.state, &fx.repo_id, &fp)
        .await
        .unwrap();

    remove_repository_impl(&fx.state, &fx.repo_id)
        .await
        .unwrap();
    let again = add_repository_impl(&fx.state, fx.repo.path.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(again.id, fx.repo_id, "same path, same id");
    let sec = get_repository_security_impl(&fx.state, &again.id)
        .await
        .unwrap();
    assert_eq!(sec.status, FilterGuardStatus::Blocked);
}

#[tokio::test]
async fn process_driver_variant_is_blocked_until_trusted() {
    let fx = Fx::plain(test_runner()).await;
    fx.repo.write(".gitattributes", "* filter=evil\n");
    fx.repo.git([
        "config",
        "filter.evil.process",
        &fx.driver.command("process"),
    ]);
    fx.dirty();
    let wt_id = fx.wt_id().await;
    fx.driver.reset();

    let err = get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted)
        .await
        .unwrap_err();
    assert!(is_blocked(&err));
    assert!(!fx.driver.ran());

    let sec = fx.security().await.unwrap();
    assert_eq!(
        sec.drivers[0].process.as_deref(),
        Some(fx.driver.command("process").as_str())
    );
    trust_repository_filters_impl(&fx.state, &fx.repo_id, &sec.fingerprint)
        .await
        .unwrap();
    let _ = get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted).await;
    assert!(
        fx.driver.ran(),
        "control: trusted process driver is started"
    );
}

#[tokio::test]
async fn a_users_global_driver_is_allowed_and_reported_as_info() {
    let probe = TestRepo::new();
    let user = Driver::new(&probe, "lfs");
    let global = probe.root.path().join("global.gitconfig");
    std::fs::write(
        &global,
        format!("[filter \"lfs\"]\n\tclean = {}\n", user.command("clean")),
    )
    .unwrap();
    let runner = GitRunner::new()
        .with_env("GIT_CONFIG_GLOBAL", &global)
        .with_env("GIT_CONFIG_NOSYSTEM", "1");
    let fx = Fx::plain(runner).await;
    fx.repo.write(".gitattributes", "* filter=lfs\n");
    fx.dirty();
    crate::common::write_in(&fx.wt, ".gitattributes", b"* filter=lfs\n");
    let wt_id = fx.wt_id().await;

    let sec = fx.security().await.unwrap();
    assert_eq!(sec.status, FilterGuardStatus::Info);
    assert!(sec.fingerprint.is_empty(), "nothing needs trust");
    let d = &sec.drivers[0];
    assert_eq!(
        (d.name.as_str(), d.scope, d.needs_trust),
        ("lfs", FilterScope::Global, false)
    );

    // Allowed without asking, and (it is the user's own program) it runs.
    get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted)
        .await
        .unwrap();
    assert!(
        user.ran(),
        "the user's own driver should run as in `git status`"
    );

    // Nothing to trust here.
    let err = trust_repository_filters_impl(&fx.state, &fx.repo_id, "")
        .await
        .unwrap_err();
    assert!(matches!(err, IpcError::InvalidInput { .. }));
}

#[tokio::test]
async fn a_repository_without_filters_is_unaffected() {
    let fx = Fx::plain(test_runner()).await;
    fx.dirty();
    let wt_id = fx.wt_id().await;
    let sec = fx.security().await.unwrap();
    assert_eq!(sec.status, FilterGuardStatus::Clear);
    assert!(sec.drivers.is_empty() && sec.fingerprint.is_empty());
    let changes = get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted)
        .await
        .unwrap();
    assert!(!changes.is_empty());
    assert!(fx.worktrees().await.iter().all(|w| w.load_error.is_none()));
}

#[tokio::test]
async fn a_worktree_scoped_driver_blocks_only_that_worktree_but_taints_the_repository() {
    let fx = Fx::plain(test_runner()).await;
    // `config.worktree` is private to one linked worktree.
    fx.repo.git(["config", "extensions.worktreeConfig", "true"]);
    fx.repo.write(".gitattributes", "* filter=evil\n");
    git(
        &fx.wt,
        [
            "config",
            "--worktree",
            "filter.evil.clean",
            &fx.driver.command("clean"),
        ],
    );
    fx.dirty();
    fx.driver.reset();

    let list = fx.worktrees().await;
    let main = list.iter().find(|w| w.is_main).unwrap();
    let linked = list.iter().find(|w| !w.is_main).unwrap();
    assert!(main.load_error.is_none(), "main worktree has no driver");
    assert!(matches!(&linked.load_error, Some(e) if is_blocked(e)));
    assert!(!fx.driver.ran());

    let sec = fx.security().await.unwrap();
    assert_eq!(sec.status, FilterGuardStatus::Blocked);
    assert_eq!(sec.drivers[0].scope, FilterScope::Worktree);

    // Trusting from the repository view covers that worktree.
    trust_repository_filters_impl(&fx.state, &fx.repo_id, &sec.fingerprint)
        .await
        .unwrap();
    let wt_id = fx.wt_id().await;
    get_changes_impl(&fx.state, &wt_id, DiffMode::Uncommitted)
        .await
        .unwrap();
    assert!(fx.driver.ran());
}

#[tokio::test]
async fn an_unreferenced_driver_is_still_blocked_but_reported_as_unreferenced() {
    let fx = Fx::plain(test_runner()).await;
    // Configured, but no `.gitattributes` selects it.
    fx.repo
        .git(["config", "filter.idle.clean", &fx.driver.command("clean")]);
    let sec = fx.security().await.unwrap();
    assert_eq!(
        sec.status,
        FilterGuardStatus::Blocked,
        "attributes can appear any time"
    );
    assert!(!sec.drivers[0].referenced);
}

#[tokio::test]
async fn attribute_hints_never_follow_symlinks_or_open_fifos() {
    use std::time::Duration;

    // The hint ("is this driver selected by .gitattributes?") reads files the
    // repository controls. Neither a symlink out of the repository nor a FIFO
    // may be read or waited on.
    let fx = Fx::plain(test_runner()).await;
    fx.repo
        .git(["config", "filter.idle.clean", &fx.driver.command("clean")]);
    let outside = fx.repo.root.path().join("outside-attributes");
    std::fs::write(&outside, "* filter=idle\n").unwrap();

    // Control: a regular file in the same place IS read.
    fx.repo.write(".gitattributes", "* filter=idle\n");
    assert!(fx.security().await.unwrap().drivers[0].referenced);
    std::fs::remove_file(fx.repo.path.join(".gitattributes")).unwrap();

    // A symlink to a file outside the repository: not followed.
    std::os::unix::fs::symlink(&outside, fx.repo.path.join(".gitattributes")).unwrap();
    assert!(!fx.security().await.unwrap().drivers[0].referenced);
    std::fs::remove_file(fx.repo.path.join(".gitattributes")).unwrap();

    // `.git/info/attributes` as a symlink: same.
    std::fs::create_dir_all(fx.repo.path.join(".git/info")).unwrap();
    std::os::unix::fs::symlink(&outside, fx.repo.path.join(".git/info/attributes")).unwrap();
    assert!(!fx.security().await.unwrap().drivers[0].referenced);
    std::fs::remove_file(fx.repo.path.join(".git/info/attributes")).unwrap();

    // A FIFO with no writer would block an `open` forever.
    let fifo = fx.repo.path.join(".gitattributes");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    if made.is_ok_and(|s| s.success()) {
        let report = tokio::time::timeout(Duration::from_secs(10), fx.security())
            .await
            .expect("the security report hung on a FIFO")
            .unwrap();
        assert!(!report.drivers[0].referenced);
    }
}

/// A store that removes the repository right before the trust write lands: the
/// "user removed it while the trust command was scanning" race, made
/// deterministic.
struct RemovesBeforeTrustWrite(MemoryRepoStore);

impl why_lib::store::RepoStore for RemovesBeforeTrustWrite {
    fn list(&self) -> Result<Vec<why_lib::store::RepoRecord>, why_lib::store::StoreError> {
        self.0.list()
    }
    fn get(
        &self,
        id: &RepoId,
    ) -> Result<Option<why_lib::store::RepoRecord>, why_lib::store::StoreError> {
        self.0.get(id)
    }
    fn insert(
        &self,
        rec: why_lib::store::RepoRecord,
    ) -> Result<why_lib::store::InsertOutcome, why_lib::store::StoreError> {
        self.0.insert(rec)
    }
    fn remove(&self, id: &RepoId) -> Result<bool, why_lib::store::StoreError> {
        self.0.remove(id)
    }
    fn base_override(
        &self,
        wt: &why_lib::contracts::WorktreeId,
    ) -> Result<Option<String>, why_lib::store::StoreError> {
        self.0.base_override(wt)
    }
    fn set_base_override(
        &self,
        wt: &why_lib::contracts::WorktreeId,
        base: Option<String>,
    ) -> Result<(), why_lib::store::StoreError> {
        self.0.set_base_override(wt, base)
    }
    fn trusted_filters(
        &self,
        repo: &RepoId,
    ) -> Result<std::collections::HashSet<String>, why_lib::store::StoreError> {
        self.0.trusted_filters(repo)
    }
    fn set_trusted_filters(
        &self,
        repo: &RepoId,
        fingerprints: &[String],
    ) -> Result<bool, why_lib::store::StoreError> {
        // The removal lands in the gap between the command's scan and its write.
        self.0.remove(repo)?;
        self.0.set_trusted_filters(repo, fingerprints)
    }
}

#[tokio::test]
async fn trust_confirmed_while_the_repository_is_removed_is_not_remembered() {
    use why_lib::store::RepoStore;

    let repo = TestRepo::with_initial_commit();
    let driver = Driver::new(&repo, "evil");
    repo.write(".gitattributes", "* filter=evil\n");
    repo.git(["config", "filter.evil.clean", &driver.command("clean")]);
    let store = Arc::new(RemovesBeforeTrustWrite(MemoryRepoStore::new()));
    let state = AppState::new(test_runner(), store.clone());
    let added = add_repository_impl(&state, repo.path.to_str().unwrap())
        .await
        .unwrap();
    let fp = get_repository_security_impl(&state, &added.id)
        .await
        .unwrap()
        .fingerprint;

    let err = trust_repository_filters_impl(&state, &added.id, &fp)
        .await
        .unwrap_err();
    assert_eq!(err, IpcError::NotFound);

    // Re-adding the same path (same id) must NOT inherit the late approval.
    let again = add_repository_impl(&state, repo.path.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(again.id, added.id);
    assert!(store.trusted_filters(&again.id).unwrap().is_empty());
    assert_eq!(
        get_repository_security_impl(&state, &again.id)
            .await
            .unwrap()
            .status,
        FilterGuardStatus::Blocked
    );
}

#[tokio::test]
async fn revoking_succeeds_even_when_the_rescan_fails() {
    let fx = Fx::with_local_clean().await;
    let fp = fx.security().await.unwrap().fingerprint;
    trust_repository_filters_impl(&fx.state, &fx.repo_id, &fp)
        .await
        .unwrap();

    // Break the linked worktree so that scanning it fails: its `.git` file
    // points nowhere. (`git worktree list` still lists it.)
    std::fs::write(fx.wt.join(".git"), "gitdir: /nonexistent/nowhere\n").unwrap();
    assert!(
        fx.security().await.is_err(),
        "control: the rescan does fail"
    );

    let result = revoke_repository_filters_impl(&fx.state, &fx.repo_id)
        .await
        .expect("the revocation itself succeeded");
    assert_eq!(result.security, None);
    assert!(result.rescan_error.is_some());
    // And trust really is gone.
    assert!(
        fx.state
            .repos
            .trusted_filters(&fx.repo_id)
            .unwrap()
            .is_empty()
    );
}
