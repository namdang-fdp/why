use why_lib::git::{GitRunner, detect_base};

use crate::common::{TestRepo, git};

/// Commit a new file with unique content on whatever is checked out in `dir`.
fn commit_file(dir: &std::path::Path, name: &str) -> String {
    crate::common::write_in(dir, name, format!("{name}\n").as_bytes());
    git(dir, ["add", "-A"]);
    git(dir, ["commit", "-q", "-m", name]);
    git(dir, ["rev-parse", "HEAD"])
}

#[tokio::test]
async fn clone_uses_origin_head_and_measures_ahead_behind() {
    let (_origin_root, origin) = TestRepo::bare_origin_with_commit();
    let clone = TestRepo::clone_of(&origin);
    let runner = GitRunner::new();

    // Branch point.
    let fork = clone.git(["rev-parse", "HEAD"]);
    clone.git(["switch", "-q", "-c", "feature"]);
    commit_file(&clone.path, "f1.txt"); // feature: +1

    // origin/main moves on by 2 commits that feature does not have.
    clone.git(["switch", "-q", "main"]);
    commit_file(&clone.path, "m1.txt");
    commit_file(&clone.path, "m2.txt");
    clone.git(["push", "-q", "origin", "main"]);
    clone.git(["switch", "-q", "feature"]);
    clone.git(["fetch", "-q", "origin"]);

    // origin/HEAD exists because we cloned.
    assert_eq!(
        clone.git(["symbolic-ref", "refs/remotes/origin/HEAD"]),
        "refs/remotes/origin/main"
    );

    let base = detect_base(&runner, &clone.path, None)
        .await
        .unwrap()
        .expect("a base");
    assert_eq!(base.base_ref, "origin/main");
    assert_eq!(base.merge_base.as_str(), fork);
    // Asymmetric on purpose (1 vs 2): a swapped pair would fail.
    assert_eq!((base.ahead, base.behind), (1, 2));
}

#[tokio::test]
async fn origin_head_may_point_at_a_branch_that_is_not_main() {
    let (_origin_root, origin) = TestRepo::bare_origin_with_commit();
    // Origin's default branch is `develop`; `main` also exists there.
    git(&origin, ["branch", "develop", "main"]);
    git(&origin, ["symbolic-ref", "HEAD", "refs/heads/develop"]);
    let clone = TestRepo::clone_of(&origin);
    clone.git(["switch", "-q", "-c", "feature", "origin/develop"]);
    commit_file(&clone.path, "x.txt");

    let base = detect_base(&GitRunner::new(), &clone.path, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        base.base_ref, "origin/develop",
        "origin/HEAD wins over origin/main"
    );
    assert_eq!((base.ahead, base.behind), (1, 0));
}

#[tokio::test]
async fn no_remote_falls_back_to_local_main() {
    let repo = TestRepo::with_initial_commit();
    let fork = repo.git(["rev-parse", "HEAD"]);
    repo.git(["switch", "-q", "-c", "feature"]);
    commit_file(&repo.path, "a.txt");
    commit_file(&repo.path, "b.txt");

    let base = detect_base(&GitRunner::new(), &repo.path, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(base.base_ref, "main");
    assert_eq!(base.merge_base.as_str(), fork);
    assert_eq!((base.ahead, base.behind), (2, 0));
}

#[tokio::test]
async fn master_is_a_fallback_too() {
    let repo = TestRepo::with_initial_commit();
    repo.git(["branch", "-m", "main", "master"]);
    repo.git(["switch", "-q", "-c", "feature"]);
    commit_file(&repo.path, "a.txt");
    let base = detect_base(&GitRunner::new(), &repo.path, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(base.base_ref, "master");
}

#[tokio::test]
async fn a_branch_is_never_its_own_base() {
    // Sitting on `main` with no remote: the only candidate is main itself.
    let repo = TestRepo::with_initial_commit();
    let r = detect_base(&GitRunner::new(), &repo.path, None)
        .await
        .unwrap();
    assert_eq!(r, None);
}

#[tokio::test]
async fn on_main_with_a_remote_compares_against_origin_main() {
    let (_origin_root, origin) = TestRepo::bare_origin_with_commit();
    let clone = TestRepo::clone_of(&origin);
    commit_file(&clone.path, "local.txt"); // local main is 1 ahead of origin/main
    let base = detect_base(&GitRunner::new(), &clone.path, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(base.base_ref, "origin/main");
    assert_eq!((base.ahead, base.behind), (1, 0));
}

#[tokio::test]
async fn nothing_to_compare_against_is_none_not_an_error() {
    // (c) no origin, no main/master: only a branch called `trunk`.
    let repo = TestRepo::new();
    repo.git(["switch", "-q", "-c", "trunk"]); // unborn: renames the branch
    repo.write("a.txt", "a\n");
    repo.commit_all("first");
    let r = detect_base(&GitRunner::new(), &repo.path, None)
        .await
        .unwrap();
    assert_eq!(r, None);
}

#[tokio::test]
async fn unborn_head_is_none() {
    let repo = TestRepo::new();
    let r = detect_base(&GitRunner::new(), &repo.path, None)
        .await
        .unwrap();
    assert_eq!(r, None);
}

#[tokio::test]
async fn unrelated_histories_have_no_merge_base() {
    let repo = TestRepo::with_initial_commit();
    repo.git(["switch", "-q", "--orphan", "island"]);
    repo.write("island.txt", "x\n");
    repo.commit_all("island root");
    // `main` exists but shares no history with `island`.
    let r = detect_base(&GitRunner::new(), &repo.path, None)
        .await
        .unwrap();
    assert_eq!(r, None);
}

#[tokio::test]
async fn override_wins_and_bad_override_falls_back() {
    let repo = TestRepo::with_initial_commit();
    repo.git(["switch", "-q", "-c", "release"]);
    commit_file(&repo.path, "r.txt");
    repo.git(["switch", "-q", "-c", "feature"]);
    commit_file(&repo.path, "f.txt");
    let runner = GitRunner::new();

    let b = detect_base(&runner, &repo.path, Some("release"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.base_ref, "release");
    assert_eq!((b.ahead, b.behind), (1, 0));

    // A ref that does not exist is skipped, then auto-detection finds `main`.
    let b = detect_base(&runner, &repo.path, Some("no-such-branch"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.base_ref, "main");
    assert_eq!((b.ahead, b.behind), (2, 0));

    // An override that RESOLVES to the worktree's own branch is skipped, however
    // it is spelled (`HEAD` included); otherwise HEAD would be compared with
    // itself and report a bogus 0/0.
    for own in [
        "feature",
        "HEAD",
        "@",
        "refs/heads/feature",
        "heads/feature",
    ] {
        let b = detect_base(&runner, &repo.path, Some(own))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(b.base_ref, "main", "override {own:?}");
        assert_eq!((b.ahead, b.behind), (2, 0), "override {own:?}");
    }

    // A bare commit id names no ref at all and is a fine override.
    let release_tip = repo.git(["rev-parse", "release"]);
    let b = detect_base(&runner, &repo.path, Some(&release_tip))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.base_ref, release_tip);
    assert_eq!((b.ahead, b.behind), (1, 0));

    // Values that look like options are data, not flags.
    let b = detect_base(&runner, &repo.path, Some("--output=/tmp/pwned"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.base_ref, "main");
}

#[tokio::test]
async fn detached_head_still_finds_a_base() {
    let repo = TestRepo::with_initial_commit();
    let wt_path = repo.base().join("detached wt");
    repo.git([
        "worktree",
        "add",
        "-q",
        "--detach",
        wt_path.to_str().unwrap(),
    ]);
    commit_file(&wt_path, "d.txt");

    let b = detect_base(&GitRunner::new(), &wt_path, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.base_ref, "main"); // detached: no "own branch" to exclude
    assert_eq!((b.ahead, b.behind), (1, 0));
}

#[tokio::test]
async fn works_inside_a_linked_worktree() {
    let repo = TestRepo::with_initial_commit();
    let wt = repo.add_worktree("agent-wt", "agent/task");
    commit_file(&wt, "agent.txt");
    commit_file(&repo.path, "main-moves.txt");

    let b = detect_base(&GitRunner::new(), &wt, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.base_ref, "main");
    assert_eq!((b.ahead, b.behind), (1, 1));
}

#[tokio::test]
async fn local_main_ahead_of_origin_is_the_base_for_a_branch_cut_from_it() {
    // Work merged into local main but not pushed. A worktree cut from local
    // main must not show those commits as the agent's own.
    let (_origin_root, origin) = TestRepo::bare_origin_with_commit();
    let clone = TestRepo::clone_of(&origin);
    commit_file(&clone.path, "merged1.txt");
    let cut = commit_file(&clone.path, "merged2.txt"); // local main: 2 ahead of origin/main
    clone.git(["switch", "-q", "-c", "feature"]);
    commit_file(&clone.path, "agent.txt");

    let b = detect_base(&GitRunner::new(), &clone.path, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        b.base_ref, "main",
        "not origin/main (which would give ahead = 3)"
    );
    assert_eq!(b.merge_base.as_str(), cut);
    assert_eq!((b.ahead, b.behind), (1, 0));
}

#[tokio::test]
async fn branch_cut_from_origin_main_while_local_main_has_unpushed_commits() {
    let (_origin_root, origin) = TestRepo::bare_origin_with_commit();
    let clone = TestRepo::clone_of(&origin);
    let fork = clone.git(["rev-parse", "origin/main"]);
    commit_file(&clone.path, "unpushed1.txt");
    commit_file(&clone.path, "unpushed2.txt");
    clone.git(["switch", "-q", "-c", "feature", "origin/main"]);
    commit_file(&clone.path, "agent.txt");

    // Both candidates give ahead = 1 (the agent's commit); origin/main wins the
    // tie by priority and is also the one the branch was cut from (behind 0).
    let b = detect_base(&GitRunner::new(), &clone.path, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.base_ref, "origin/main");
    assert_eq!(b.merge_base.as_str(), fork);
    assert_eq!((b.ahead, b.behind), (1, 0));
}

#[tokio::test]
async fn equal_candidates_prefer_the_remote_default() {
    // main == origin/main: identical numbers, priority order decides.
    let (_origin_root, origin) = TestRepo::bare_origin_with_commit();
    let clone = TestRepo::clone_of(&origin);
    clone.git(["switch", "-q", "-c", "feature"]);
    commit_file(&clone.path, "x.txt");
    let b = detect_base(&GitRunner::new(), &clone.path, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.base_ref, "origin/main");
    assert_eq!((b.ahead, b.behind), (1, 0));
}
