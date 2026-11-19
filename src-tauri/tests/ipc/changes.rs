//! Tests for `ipc::changes` (the diff service) and `set_base`, run against real
//! repositories in temp directories with the developer's git config ignored.
//!
//! `set_base` lives here (not in `repos.rs`) so the repo/store task and this one
//! never edit the same test file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use why_lib::contracts::{
    ChangeStatus, DiffMode, FileChange, FileDiff, IpcError, RepoId, Worktree, WorktreeId,
};
use why_lib::ipc::changes::{get_changes_impl, get_file_diff_impl, list_worktrees_impl};
use why_lib::ipc::repos::{add_repository_impl, set_base_impl};
use why_lib::state::AppState;
use why_lib::store::MemoryRepoStore;

use crate::common::{TestRepo, git, git_raw, test_runner, write_in};

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

fn state() -> AppState {
    AppState::new(test_runner(), Arc::new(MemoryRepoStore::new()))
}

fn path_str(p: &Path) -> &str {
    p.to_str().expect("temp paths are UTF-8")
}

/// A repository with a base branch (`main`, several committed files) and a linked
/// worktree `wt` on branch `feature`, registered in a fresh [`AppState`].
struct Fixture {
    repo: TestRepo,
    wt: PathBuf,
    state: AppState,
    repo_id: RepoId,
    wt_id: WorktreeId,
}

impl Fixture {
    async fn new() -> Self {
        let repo = TestRepo::with_initial_commit();
        repo.write("mod.txt", "one\ntwo\nthree\n");
        repo.write("del.txt", "gone\nsoon\n");
        repo.write(
            "ren.txt",
            "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\n",
        );
        write_in(&repo.path, "bin.dat", b"\x00\x01\x02binary\x00");
        repo.commit_all("base files");
        let wt = repo.add_worktree("wt", "feature");

        let state = state();
        let added = add_repository_impl(&state, path_str(&repo.path))
            .await
            .unwrap();
        let wt_id = worktree_id_at(&state, &added.id, &wt).await;
        Fixture {
            repo,
            wt,
            state,
            repo_id: added.id,
            wt_id,
        }
    }

    fn wt_git<I, S>(&self, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        git(&self.wt, args)
    }

    fn write(&self, rel: &str, contents: &str) {
        write_in(&self.wt, rel, contents.as_bytes());
    }

    async fn changes(&self, mode: DiffMode) -> Vec<FileChange> {
        get_changes_impl(&self.state, &self.wt_id, mode)
            .await
            .unwrap()
    }

    async fn diff(&self, path: &str, mode: DiffMode) -> Result<FileDiff, IpcError> {
        get_file_diff_impl(&self.state, &self.wt_id, path, mode).await
    }

    async fn worktrees(&self) -> Vec<Worktree> {
        list_worktrees_impl(&self.state, &self.repo_id)
            .await
            .unwrap()
    }

    /// Make a variety of changes in the linked worktree (see the assertions in
    /// `every_mode_lists_the_expected_files`).
    fn make_changes(&self) {
        // A commit on the branch: part of "vs base", not of "uncommitted".
        self.write("committed.txt", "from a commit\n");
        self.wt_git(["add", "committed.txt"]);
        self.wt_git(["commit", "-q", "-m", "branch work"]);

        self.write("mod.txt", "one\nTWO\nthree\nfour\n"); // unstaged: +2 -1
        self.write("added.txt", "new file\n"); // staged add
        self.wt_git(["add", "added.txt"]);
        std::fs::remove_file(self.wt.join("del.txt")).unwrap(); // unstaged delete
        self.wt_git(["mv", "ren.txt", "renamed.txt"]); // staged rename
        self.write("new/dir/file.txt", "a\nb\nc"); // untracked, nested, no final newline
        write_in(&self.wt, "logo.bin", b"\x00\x00\x00"); // untracked binary
        self.write("dir with space/ünï cödé.txt", "unicode\n"); // untracked odd name
    }
}

/// Rewrite `rel` with `contents` and set its modification time an hour into the
/// past: the file is then "stat-dirty" for git (its timestamp no longer matches
/// the index entry) even though it may hold the same bytes. A plain rewrite right
/// after the checkout does not do it: a timestamp in the same second as the
/// index file is "racily clean" and git compares bytes instead of stat data.
fn rewrite_stat_dirty(root: &Path, rel: &str, contents: &[u8]) {
    write_in(root, rel, contents);
    let file = std::fs::File::options()
        .write(true)
        .open(root.join(rel))
        .unwrap();
    file.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
}

/// The id the core derives for the worktree at `path`.
async fn worktree_id_at(state: &AppState, repo_id: &RepoId, path: &Path) -> WorktreeId {
    list_worktrees_impl(state, repo_id)
        .await
        .unwrap()
        .into_iter()
        .find(|w| Path::new(&w.path) == path)
        .unwrap_or_else(|| panic!("no worktree at {}", path.display()))
        .id
}

/// `(path, status, staged, unstaged)` for easy comparison.
fn summary(changes: &[FileChange]) -> Vec<(&str, ChangeStatus, bool, bool)> {
    changes
        .iter()
        .map(|c| (c.path.as_str(), c.status, c.staged, c.unstaged))
        .collect()
}

fn find<'a>(changes: &'a [FileChange], path: &str) -> &'a FileChange {
    changes
        .iter()
        .find(|c| c.path == path)
        .unwrap_or_else(|| panic!("{path} not in {:?}", summary(changes)))
}

// ---------------------------------------------------------------------------
// get_changes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_mode_lists_the_expected_files() {
    let f = Fixture::new().await;
    f.make_changes();
    use ChangeStatus::*;

    // vs base: everything since the merge-base, committed or not, plus untracked.
    let vs_base = f.changes(DiffMode::VsBase).await;
    assert_eq!(
        summary(&vs_base),
        [
            ("added.txt", Added, true, false),
            ("committed.txt", Added, false, false), // in a commit: neither flag
            ("del.txt", Deleted, false, true),
            ("dir with space/ünï cödé.txt", Untracked, false, true),
            ("logo.bin", Untracked, false, true),
            ("mod.txt", Modified, false, true),
            ("new/dir/file.txt", Untracked, false, true),
            ("renamed.txt", Renamed, true, false),
        ]
    );
    // Counts (numstat) and the rename's old path.
    let m = find(&vs_base, "mod.txt");
    assert_eq!((m.additions, m.deletions), (Some(2), Some(1)));
    let d = find(&vs_base, "del.txt");
    assert_eq!((d.additions, d.deletions), (Some(0), Some(2)));
    let r = find(&vs_base, "renamed.txt");
    assert_eq!(r.old_path.as_deref(), Some("ren.txt"));
    // Untracked files: additions = line count; no deletions; binary detected.
    let n = find(&vs_base, "new/dir/file.txt");
    assert_eq!((n.additions, n.deletions, n.binary), (Some(3), None, false));
    let b = find(&vs_base, "logo.bin");
    assert_eq!((b.additions, b.binary), (None, true));
    assert!(vs_base.iter().all(|c| !c.path_lossy));

    // uncommitted: HEAD -> working tree; the commit on the branch is not part of it.
    let uncommitted = f.changes(DiffMode::Uncommitted).await;
    let paths: Vec<_> = uncommitted.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "added.txt",
            "del.txt",
            "dir with space/ünï cödé.txt",
            "logo.bin",
            "mod.txt",
            "new/dir/file.txt",
            "renamed.txt"
        ]
    );

    // staged: only what is in the index. No untracked files, no unstaged-only edits.
    let staged = f.changes(DiffMode::Staged).await;
    assert_eq!(
        summary(&staged),
        [
            ("added.txt", Added, true, false),
            ("renamed.txt", Renamed, true, false)
        ]
    );
}

#[tokio::test]
async fn change_count_equals_the_length_of_get_changes() {
    let f = Fixture::new().await;
    f.make_changes();
    let wt = f
        .worktrees()
        .await
        .into_iter()
        .find(|w| w.id == f.wt_id)
        .unwrap();
    assert_eq!(
        wt.change_count as usize,
        f.changes(DiffMode::VsBase).await.len()
    );

    // The main worktree has no base (it is on `main`): counted as `uncommitted`.
    f.repo.write("README.md", "changed\n");
    f.repo.write("untracked.txt", "x");
    let main = f.worktrees().await.into_iter().find(|w| w.is_main).unwrap();
    assert!(main.base.is_none());
    let main_changes = get_changes_impl(&f.state, &main.id, DiffMode::Uncommitted)
        .await
        .unwrap();
    assert_eq!(main.change_count as usize, main_changes.len());
    assert_eq!(main.change_count, 2);
}

#[tokio::test]
async fn staged_and_unstaged_edits_to_one_file_set_both_flags() {
    let f = Fixture::new().await;
    f.write("mod.txt", "staged edit\n");
    f.wt_git(["add", "mod.txt"]);
    f.write("mod.txt", "staged edit\nand a later edit\n");

    for mode in [DiffMode::VsBase, DiffMode::Uncommitted, DiffMode::Staged] {
        let c = f.changes(mode).await;
        let m = find(&c, "mod.txt");
        assert!(m.staged && m.unstaged, "{mode:?}: {m:?}");
    }
}

#[tokio::test]
async fn a_clean_worktree_has_no_changes() {
    let f = Fixture::new().await;
    for mode in [DiffMode::VsBase, DiffMode::Uncommitted, DiffMode::Staged] {
        assert!(f.changes(mode).await.is_empty(), "{mode:?}");
    }
}

#[tokio::test]
async fn vs_base_without_a_base_is_invalid_input() {
    // The main worktree is on `main` itself: nothing to compare it with.
    let f = Fixture::new().await;
    let main = f.worktrees().await.into_iter().find(|w| w.is_main).unwrap();
    let err = get_changes_impl(&f.state, &main.id, DiffMode::VsBase)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        IpcError::InvalidInput {
            message: "worktree has no base; use uncommitted".to_string()
        }
    );
    // ... while uncommitted works.
    f.repo.write("README.md", "edited\n");
    let c = get_changes_impl(&f.state, &main.id, DiffMode::Uncommitted)
        .await
        .unwrap();
    assert_eq!(c.len(), 1);
}

#[tokio::test]
async fn an_unborn_repository_diffs_against_the_empty_tree() {
    let repo = TestRepo::new(); // no commits
    repo.write("staged.txt", "s\n");
    repo.git(["add", "staged.txt"]);
    repo.write("loose.txt", "l1\nl2\n");
    let state = state();
    let added = add_repository_impl(&state, path_str(&repo.path))
        .await
        .unwrap();
    let wt = list_worktrees_impl(&state, &added.id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!((wt.head_sha.clone(), wt.base.clone()), (None, None));
    assert_eq!(wt.change_count, 2);

    let uncommitted = get_changes_impl(&state, &wt.id, DiffMode::Uncommitted)
        .await
        .unwrap();
    assert_eq!(
        summary(&uncommitted),
        [
            ("loose.txt", ChangeStatus::Untracked, false, true),
            ("staged.txt", ChangeStatus::Added, true, false)
        ]
    );
    let staged = get_changes_impl(&state, &wt.id, DiffMode::Staged)
        .await
        .unwrap();
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].additions, Some(1));

    // The old side of an added file is empty, the new side is the file.
    let d = get_file_diff_impl(&state, &wt.id, "staged.txt", DiffMode::Staged)
        .await
        .unwrap();
    assert_eq!(
        (d.old_content, d.new_content.as_deref()),
        (None, Some("s\n"))
    );

    assert!(matches!(
        get_changes_impl(&state, &wt.id, DiffMode::VsBase).await,
        Err(IpcError::InvalidInput { .. })
    ));
}

#[tokio::test]
async fn conflicted_files_are_reported_once_in_every_mode() {
    let repo = TestRepo::with_initial_commit();
    repo.write("c.txt", "1\n2\n3\n");
    repo.commit_all("base");
    repo.git(["checkout", "-q", "-b", "other"]);
    repo.write("c.txt", "1\nOTHER\n3\n");
    repo.commit_all("other");
    repo.git(["checkout", "-q", "main"]);
    repo.write("c.txt", "1\nMAIN\n3\n");
    repo.commit_all("main");
    let merge = git_raw(&repo.path, ["merge", "other"]);
    assert!(!merge.status.success(), "the merge must conflict");

    let state = state();
    let added = add_repository_impl(&state, path_str(&repo.path))
        .await
        .unwrap();
    let wt = list_worktrees_impl(&state, &added.id)
        .await
        .unwrap()
        .remove(0);
    for mode in [DiffMode::Uncommitted, DiffMode::Staged] {
        let c = get_changes_impl(&state, &wt.id, mode).await.unwrap();
        assert_eq!(c.len(), 1, "{mode:?}: {c:?}");
        assert_eq!(c[0].path, "c.txt");
        assert_eq!(c[0].status, ChangeStatus::Conflicted);
    }
    // The file on disk holds the conflict markers; that is what the reviewer sees.
    let d = get_file_diff_impl(&state, &wt.id, "c.txt", DiffMode::Uncommitted)
        .await
        .unwrap();
    assert!(d.new_content.unwrap().contains("<<<<<<<"));
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_are_listed_and_shown_as_their_target_text() {
    let f = Fixture::new().await;
    std::os::unix::fs::symlink("mod.txt", f.wt.join("link")).unwrap();
    let c = f.changes(DiffMode::Uncommitted).await;
    let l = find(&c, "link");
    assert_eq!(l.status, ChangeStatus::Untracked);
    assert_eq!(l.additions, Some(1));
    let d = f.diff("link", DiffMode::Uncommitted).await.unwrap();
    assert_eq!(d.new_content.as_deref(), Some("mod.txt"));
    assert_eq!(d.old_content, None);

    // A tracked file that becomes a symlink is a type change; both sides are text.
    std::fs::remove_file(f.wt.join("mod.txt")).unwrap();
    std::os::unix::fs::symlink("del.txt", f.wt.join("mod.txt")).unwrap();
    let c = f.changes(DiffMode::Uncommitted).await;
    assert_eq!(find(&c, "mod.txt").status, ChangeStatus::TypeChanged);
    let d = f.diff("mod.txt", DiffMode::Uncommitted).await.unwrap();
    assert_eq!(d.old_content.as_deref(), Some("one\ntwo\nthree\n"));
    assert_eq!(d.new_content.as_deref(), Some("del.txt"));
}

#[cfg(unix)]
#[tokio::test]
async fn non_utf8_paths_are_flagged_lossy_and_cannot_be_opened() {
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new().await;
    let name = std::ffi::OsStr::from_bytes(b"caf\xe9.txt");
    std::fs::write(f.wt.join(name), "latin\n").unwrap();

    let c = f.changes(DiffMode::Uncommitted).await;
    assert_eq!(c.len(), 1);
    assert!(c[0].path_lossy);
    assert_eq!(c[0].path, "caf\u{fffd}.txt");
    // Known v0 limitation (see contracts): the lossy string finds nothing.
    assert_eq!(
        f.diff(&c[0].path, DiffMode::Uncommitted).await.unwrap_err(),
        IpcError::NotFound
    );
}

#[tokio::test]
async fn touched_but_unchanged_files_are_not_changes() {
    // Rewriting a file with identical bytes changes its timestamp only. The
    // diff does not refresh the index (it must not write), so git reports such
    // files as modified; the core has to filter them out again.
    let f = Fixture::new().await;
    rewrite_stat_dirty(&f.wt, "mod.txt", b"one\ntwo\nthree\n"); // text, same content
    rewrite_stat_dirty(&f.wt, "bin.dat", b"\x00\x01\x02binary\x00"); // binary, same content
    for mode in [DiffMode::VsBase, DiffMode::Uncommitted, DiffMode::Staged] {
        assert!(f.changes(mode).await.is_empty(), "{mode:?}");
    }

    // A real binary edit is still reported.
    write_in(&f.wt, "bin.dat", b"\x00\x01\x02different\x00");
    let c = f.changes(DiffMode::Uncommitted).await;
    assert_eq!(
        summary(&c),
        [("bin.dat", ChangeStatus::Modified, false, true)]
    );
    assert!(c[0].binary);
}

#[tokio::test]
async fn untracked_directory_with_its_own_repository_is_listed_but_not_readable() {
    let f = Fixture::new().await;
    let inner = f.wt.join("inner");
    std::fs::create_dir(&inner).unwrap();
    git(&inner, ["init", "-q", "-b", "main"]);
    write_in(&inner, "x.txt", b"x");
    let c = f.changes(DiffMode::Uncommitted).await;
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0].path, "inner/");
    assert_eq!(c[0].additions, None);
    let d = f.diff("inner/", DiffMode::Uncommitted).await.unwrap();
    assert!(d.binary);
    assert_eq!((d.old_content, d.new_content), (None, None));
}

#[tokio::test]
async fn a_submodule_entry_is_shown_as_not_text() {
    let f = Fixture::new().await;
    // A gitlink (mode 160000) in the index without any submodule checkout.
    let sha = f.wt_git(["rev-parse", "HEAD"]);
    f.wt_git([
        "update-index".to_string(),
        "--add".to_string(),
        "--cacheinfo".to_string(),
        format!("160000,{sha},vendor/sub"),
    ]);
    let c = f.changes(DiffMode::Staged).await;
    // Staged add; also "unstaged" because no checkout of it exists on disk.
    assert_eq!(
        summary(&c),
        [("vendor/sub", ChangeStatus::Added, true, true)]
    );
    let d = f.diff("vendor/sub", DiffMode::Staged).await.unwrap();
    assert!(d.binary);
    assert_eq!((d.old_content, d.new_content), (None, None));
}

// ---------------------------------------------------------------------------
// get_file_diff: contents
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_diff_contents_for_each_status() {
    let f = Fixture::new().await;
    f.make_changes();
    let mode = DiffMode::VsBase;

    let modified = f.diff("mod.txt", mode).await.unwrap();
    assert_eq!(modified.status, ChangeStatus::Modified);
    assert_eq!(modified.old_content.as_deref(), Some("one\ntwo\nthree\n"));
    assert_eq!(
        modified.new_content.as_deref(),
        Some("one\nTWO\nthree\nfour\n")
    );

    let added = f.diff("added.txt", mode).await.unwrap();
    assert_eq!(added.old_content, None);
    assert_eq!(added.new_content.as_deref(), Some("new file\n"));

    // Added in a commit on the branch: old side is the merge-base, where it is absent.
    let committed = f.diff("committed.txt", mode).await.unwrap();
    assert_eq!(committed.old_content, None);
    assert_eq!(committed.new_content.as_deref(), Some("from a commit\n"));

    let deleted = f.diff("del.txt", mode).await.unwrap();
    assert_eq!(deleted.status, ChangeStatus::Deleted);
    assert_eq!(deleted.old_content.as_deref(), Some("gone\nsoon\n"));
    assert_eq!(deleted.new_content, None);

    // Rename: old content from the old name, new content from the new name.
    let renamed = f.diff("renamed.txt", mode).await.unwrap();
    assert_eq!(renamed.status, ChangeStatus::Renamed);
    assert_eq!(renamed.old_path.as_deref(), Some("ren.txt"));
    assert_eq!(renamed.old_content, renamed.new_content);
    assert!(renamed.old_content.unwrap().starts_with("alpha\n"));
    // Asking by the old name finds the same rename.
    let by_old = f.diff("ren.txt", mode).await.unwrap();
    assert_eq!(by_old.path, "renamed.txt");

    let untracked = f.diff("new/dir/file.txt", mode).await.unwrap();
    assert_eq!(untracked.status, ChangeStatus::Untracked);
    assert_eq!(untracked.old_content, None);
    assert_eq!(untracked.new_content.as_deref(), Some("a\nb\nc"));

    let odd = f.diff("dir with space/ünï cödé.txt", mode).await.unwrap();
    assert_eq!(odd.new_content.as_deref(), Some("unicode\n"));

    // Uncommitted compares with HEAD, so the committed file is not a change there.
    assert_eq!(
        f.diff("committed.txt", DiffMode::Uncommitted)
            .await
            .unwrap_err(),
        IpcError::NotFound
    );
}

#[tokio::test]
async fn the_staged_diff_reads_the_index_not_the_file_on_disk() {
    let f = Fixture::new().await;
    f.write("mod.txt", "staged version\n");
    f.wt_git(["add", "mod.txt"]);
    f.write("mod.txt", "working tree version\n"); // edited after `git add`

    let staged = f.diff("mod.txt", DiffMode::Staged).await.unwrap();
    assert_eq!(staged.old_content.as_deref(), Some("one\ntwo\nthree\n"));
    assert_eq!(staged.new_content.as_deref(), Some("staged version\n"));

    let uncommitted = f.diff("mod.txt", DiffMode::Uncommitted).await.unwrap();
    assert_eq!(
        uncommitted.new_content.as_deref(),
        Some("working tree version\n")
    );

    // A staged rename reads old content from HEAD under the old name and new
    // content from the index under the new name.
    f.wt_git(["mv", "ren.txt", "moved.txt"]);
    let renamed = f.diff("moved.txt", DiffMode::Staged).await.unwrap();
    assert_eq!(renamed.old_path.as_deref(), Some("ren.txt"));
    assert!(renamed.new_content.unwrap().starts_with("alpha\n"));
}

#[tokio::test]
async fn binary_invalid_utf8_and_oversized_sides_are_withheld() {
    let f = Fixture::new().await;
    // Binary: modified tracked binary file.
    write_in(&f.wt, "bin.dat", b"\x00\x01\x02other\x00");
    let d = f.diff("bin.dat", DiffMode::Uncommitted).await.unwrap();
    assert!(d.binary && !d.too_large);
    assert_eq!((d.old_content, d.new_content), (None, None));

    // Not valid UTF-8 (Latin-1 text) counts as binary: the editor needs a real string.
    write_in(&f.wt, "latin1.txt", b"caf\xe9\n");
    let d = f.diff("latin1.txt", DiffMode::Uncommitted).await.unwrap();
    assert!(d.binary);
    assert_eq!(d.new_content, None);

    // Too large: untracked file one byte over the 1 MiB cap.
    let cap = why_lib::git::diff::MAX_CONTENT_BYTES as usize;
    write_in(&f.wt, "big.txt", "x".repeat(cap + 1).as_bytes());
    let d = f.diff("big.txt", DiffMode::Uncommitted).await.unwrap();
    assert!(d.too_large && !d.binary);
    assert_eq!((d.old_content.clone(), d.new_content.clone()), (None, None));
    // ... and exactly at the cap it is loaded.
    write_in(&f.wt, "big.txt", "x".repeat(cap).as_bytes());
    let d = f.diff("big.txt", DiffMode::Uncommitted).await.unwrap();
    assert!(!d.too_large);
    assert_eq!(d.new_content.map(|s| s.len()), Some(cap));

    // Too large on the OLD side (a committed blob), checked through `cat-file -s`.
    f.write("huge.txt", &"y".repeat(cap + 10));
    f.wt_git(["add", "huge.txt"]);
    f.wt_git(["commit", "-q", "-m", "huge"]);
    f.write("huge.txt", "small now\n");
    let d = f.diff("huge.txt", DiffMode::Uncommitted).await.unwrap();
    assert!(d.too_large);
    // Both sides are withheld, not just the big one.
    assert_eq!((d.old_content, d.new_content), (None, None));
}

#[cfg(unix)]
#[tokio::test]
async fn a_fifo_never_hangs_the_service() {
    let mkfifo = |path: &Path| {
        std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    let f = Fixture::new().await;
    let five = Duration::from_secs(5);

    // An untracked FIFO is not something git lists, so it never reaches a read.
    if !mkfifo(&f.wt.join("pipe")) {
        eprintln!("mkfifo not available; skipping");
        return;
    }
    let list = tokio::time::timeout(
        five,
        get_changes_impl(&f.state, &f.wt_id, DiffMode::Uncommitted),
    )
    .await
    .expect("listing must not block")
    .unwrap();
    assert!(list.is_empty(), "{list:?}");

    // The file reader itself refuses to open anything but regular files.
    let root = std::fs::canonicalize(&f.wt).unwrap();
    let side = tokio::time::timeout(
        five,
        why_lib::git::diff::read_worktree_side(&root, Path::new("pipe")),
    )
    .await
    .expect("reading a FIFO must not block")
    .unwrap();
    assert_eq!(side, why_lib::git::Side::NotAFile);

    // A tracked file replaced by a FIFO: `git diff --numstat` itself gives up
    // ("cannot hash"), so the service reports a git error, quickly. Known
    // limitation: a pathological worktree makes its list fail instead of
    // listing the other files.
    std::fs::remove_file(f.wt.join("mod.txt")).unwrap();
    assert!(mkfifo(&f.wt.join("mod.txt")));
    let result = tokio::time::timeout(
        five,
        get_changes_impl(&f.state, &f.wt_id, DiffMode::Uncommitted),
    )
    .await
    .expect("listing must not block");
    assert!(
        matches!(result, Err(IpcError::GitFailed { .. })),
        "{result:?}"
    );

    // ... but `list_worktrees` still lists every worktree: the failing one
    // without base or changes (WHY-9 will add a field that says why), the
    // healthy main worktree normally.
    let listed = tokio::time::timeout(five, list_worktrees_impl(&f.state, &f.repo_id))
        .await
        .expect("listing must not block")
        .unwrap();
    assert_eq!(listed.len(), 2);
    let broken = listed.iter().find(|w| w.id == f.wt_id).unwrap();
    assert_eq!((broken.base.clone(), broken.change_count), (None, 0));
    assert!(listed.iter().any(|w| w.is_main));
}

// ---------------------------------------------------------------------------
// get_file_diff: the path is untrusted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hostile_paths_are_rejected() {
    let f = Fixture::new().await;
    f.make_changes();
    // Something real outside the worktree, so a successful read would be visible.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "TOP SECRET").unwrap();

    let attacks = [
        "../../etc/passwd",
        "../secret.txt",
        "a/../../etc/passwd",
        "/etc/passwd",
        &format!("{}/secret.txt", outside.path().display()),
        "a\0b",
        "",
        "..",
    ];
    for attack in attacks {
        let err = f.diff(attack, DiffMode::VsBase).await.unwrap_err();
        assert!(
            matches!(err, IpcError::InvalidInput { .. }),
            "{attack:?} gave {err:?}"
        );
    }
}

#[tokio::test]
async fn a_path_that_is_not_in_the_change_list_is_not_found() {
    let f = Fixture::new().await;
    f.make_changes();
    // Real files inside the worktree, but not changes: tracked-and-clean, and
    // tracked metadata under `.git`.
    for path in ["README.md", ".git/config", ".git", "nonexistent.txt", "new"] {
        assert_eq!(
            f.diff(path, DiffMode::VsBase).await.unwrap_err(),
            IpcError::NotFound,
            "{path}"
        );
    }
    // In the list for one mode but not for another.
    assert!(f.diff("mod.txt", DiffMode::VsBase).await.is_ok());
    assert_eq!(
        f.diff("mod.txt", DiffMode::Staged).await.unwrap_err(),
        IpcError::NotFound
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_pointing_outside_is_shown_as_text_and_never_followed() {
    let f = Fixture::new().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "TOP SECRET").unwrap();

    // Untracked symlink to a file outside: the "content" is the link target text.
    let target = outside.path().join("secret.txt");
    std::os::unix::fs::symlink(&target, f.wt.join("leak")).unwrap();
    // Untracked symlink to a directory outside, and a path through it.
    std::os::unix::fs::symlink(outside.path(), f.wt.join("dir_link")).unwrap();

    let d = f.diff("leak", DiffMode::Uncommitted).await.unwrap();
    assert_eq!(d.new_content.as_deref(), target.to_str());
    assert!(!d.new_content.unwrap().contains("TOP SECRET"));

    // `dir_link/secret.txt` is not in the change list (git sees only `dir_link`).
    assert_eq!(
        f.diff("dir_link/secret.txt", DiffMode::Uncommitted)
            .await
            .unwrap_err(),
        IpcError::NotFound
    );
    let d = f.diff("dir_link", DiffMode::Uncommitted).await.unwrap();
    assert!(!d.new_content.unwrap().contains("TOP SECRET"));
}

#[cfg(unix)]
#[tokio::test]
async fn replacing_a_tracked_directory_with_a_symlink_leaks_nothing() {
    let f = Fixture::new().await;
    // Track `d/f`, then replace the directory `d` by a symlink to an outside
    // directory that has a file of the same name.
    f.write("d/f", "tracked\n");
    f.wt_git(["add", "d/f"]);
    f.wt_git(["commit", "-q", "-m", "add d/f"]);
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("f"), "TOP SECRET").unwrap();
    std::fs::remove_dir_all(f.wt.join("d")).unwrap();
    std::os::unix::fs::symlink(outside.path(), f.wt.join("d")).unwrap();

    for mode in [DiffMode::VsBase, DiffMode::Uncommitted] {
        for path in ["d/f", "d"] {
            if let Ok(d) = f.diff(path, mode).await {
                let all = format!("{d:?}");
                assert!(!all.contains("TOP SECRET"), "{mode:?} {path}: {all}");
            }
        }
    }

    // And the reader itself (the third gate) refuses to look through the link,
    // even if a caller handed it that path.
    let root = std::fs::canonicalize(&f.wt).unwrap();
    let err = why_lib::git::diff::read_worktree_side(&root, Path::new("d/f"))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    let err = why_lib::git::diff::read_worktree_side(&root, Path::new("../x"))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[tokio::test]
async fn unknown_ids_are_not_found() {
    let f = Fixture::new().await;
    let nope = WorktreeId("0000000000000000".to_string());
    assert_eq!(
        get_changes_impl(&f.state, &nope, DiffMode::Uncommitted)
            .await
            .unwrap_err(),
        IpcError::NotFound
    );
    assert_eq!(
        get_file_diff_impl(&f.state, &nope, "mod.txt", DiffMode::Uncommitted)
            .await
            .unwrap_err(),
        IpcError::NotFound
    );
    assert_eq!(
        list_worktrees_impl(&f.state, &RepoId("nope".into()))
            .await
            .unwrap_err(),
        IpcError::NotFound
    );
    assert_eq!(
        set_base_impl(&f.state, &nope, "main").await.unwrap_err(),
        IpcError::NotFound
    );
}

// ---------------------------------------------------------------------------
// Passive observer: no writes to the repository
// ---------------------------------------------------------------------------

/// What identifies the index file's on-disk state: contents, mtime and inode (an
/// atomic rewrite replaces the file, so the inode changes even if the bytes do not).
fn index_fingerprint(worktree: &Path) -> (Vec<u8>, std::time::SystemTime, u64) {
    let git_dir = git(worktree, ["rev-parse", "--absolute-git-dir"]);
    let index = Path::new(&git_dir).join("index");
    let meta = std::fs::metadata(&index).unwrap();
    (
        std::fs::read(&index).unwrap(),
        meta.modified().unwrap(),
        inode(&meta),
    )
}

#[cfg(unix)]
fn inode(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino()
}

/// No inode numbers off Unix: the bytes and mtime still catch a rewrite.
#[cfg(not(unix))]
fn inode(_: &std::fs::Metadata) -> u64 {
    0
}

#[tokio::test]
async fn reading_changes_never_rewrites_the_index() {
    let f = Fixture::new().await;
    f.make_changes();
    // Stat-dirty files (timestamp differs from the index, content identical) are
    // exactly what makes a plain `git diff` rewrite `.git/index`.
    rewrite_stat_dirty(&f.wt, "mod.txt", b"one\nTWO\nthree\nfour\n");
    rewrite_stat_dirty(&f.wt, "bin.dat", b"\x00\x01\x02binary\x00");
    rewrite_stat_dirty(&f.wt, "added.txt", b"new file\n");

    let before = index_fingerprint(&f.wt);
    for mode in [DiffMode::VsBase, DiffMode::Uncommitted, DiffMode::Staged] {
        f.changes(mode).await;
        f.diff("added.txt", mode).await.ok();
    }
    f.worktrees().await;
    assert_eq!(index_fingerprint(&f.wt), before, "the index was written");
    // (No `index.lock` is left behind either.)
    let git_dir = git(&f.wt, ["rev-parse", "--absolute-git-dir"]);
    assert!(!Path::new(&git_dir).join("index.lock").exists());
}

#[tokio::test]
async fn negative_control_plain_git_diff_rewrites_the_index() {
    // Proves the test above has teeth: the *unguarded* command, run with the
    // runner's `GIT_OPTIONAL_LOCKS=0` but without `diff.autoRefreshIndex=false`,
    // does rewrite the index of a repository with a stat-dirty file. If a future
    // git stops doing that, this test fails and the guard may be reconsidered.
    let f = Fixture::new().await;
    rewrite_stat_dirty(&f.wt, "mod.txt", b"one\ntwo\nthree\n"); // stat-dirty, identical content
    let before = index_fingerprint(&f.wt);
    // `git_raw` scrubs inherited `GIT_*` variables and ignores the developer's
    // config; `--no-optional-locks` is the command-line spelling of the runner's
    // `GIT_OPTIONAL_LOCKS=0`.
    let out = git_raw(
        &f.wt,
        ["--no-optional-locks", "diff", "--raw", "HEAD", "--"],
    );
    assert!(out.status.success());
    assert_ne!(index_fingerprint(&f.wt), before);
}

// ---------------------------------------------------------------------------
// list_worktrees
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_worktrees_describes_every_kind_of_worktree() {
    let f = Fixture::new().await;
    f.make_changes();
    // A locked one, a detached one, and a prunable one (directory deleted).
    let locked = f.repo.add_worktree("locked", "locked-branch");
    f.repo.git([
        std::ffi::OsString::from("worktree"),
        "lock".into(),
        "--reason".into(),
        "busy".into(),
        locked.clone().into_os_string(),
    ]);
    let detached = f.repo.base().join("detached");
    f.repo.git([
        std::ffi::OsString::from("worktree"),
        "add".into(),
        "-q".into(),
        "--detach".into(),
        detached.clone().into_os_string(),
    ]);
    let gone = f.repo.add_worktree("gone", "gone-branch");
    std::fs::remove_dir_all(&gone).unwrap();

    let list = f.worktrees().await;
    assert_eq!(list.len(), 5);
    assert!(list[0].is_main, "the main worktree comes first");

    let by_branch = |b: &str| {
        list.iter()
            .find(|w| w.branch.as_deref() == Some(b))
            .unwrap()
    };

    let feature = by_branch("feature");
    assert_eq!(feature.id, f.wt_id);
    assert_eq!(Path::new(&feature.path), f.wt);
    assert_eq!(feature.base.as_ref().unwrap().base_ref, "main");
    assert_eq!(feature.base.as_ref().unwrap().ahead, 1); // `committed.txt`
    assert_eq!(feature.change_count, 8);
    assert!(feature.head_sha.is_some() && !feature.is_main);
    // Something happened here (the checkout and the edits just made): a recent time.
    assert!(feature.last_activity.is_some_and(|t| t > 1_600_000_000));
    assert!(!feature.is_locked && !feature.is_prunable && !feature.is_detached);

    assert!(by_branch("locked-branch").is_locked);
    assert_eq!(by_branch("locked-branch").change_count, 0);

    let det = list.iter().find(|w| w.is_detached).unwrap();
    assert!(det.branch.is_none() && det.head_sha.is_some());

    // The prunable worktree is listed with zero changes, not an error ...
    let p = by_branch("gone-branch");
    assert!(p.is_prunable);
    assert_eq!((p.change_count, p.base.clone()), (0, None));
    assert_eq!(
        p.last_activity, None,
        "a vanished directory has no activity"
    );
    // ... and asking for its changes says it is gone.
    assert_eq!(
        get_changes_impl(&f.state, &p.id, DiffMode::Uncommitted)
            .await
            .unwrap_err(),
        IpcError::NotFound
    );

    // Every non-prunable worktree: change_count == the list's length.
    for w in list.iter().filter(|w| !w.is_prunable) {
        let mode = if w.base.is_some() {
            DiffMode::VsBase
        } else {
            DiffMode::Uncommitted
        };
        let n = get_changes_impl(&f.state, &w.id, mode).await.unwrap().len();
        assert_eq!(w.change_count as usize, n, "{}", w.path);
    }
}

// ---------------------------------------------------------------------------
// set_base
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_base_validates_stores_and_clears() {
    let f = Fixture::new().await;
    // A second candidate base: `release`, one commit behind `main`.
    f.repo.git(["branch", "release", "HEAD~1"]);
    f.make_changes(); // one commit on `feature`

    let auto = f
        .worktrees()
        .await
        .into_iter()
        .find(|w| w.id == f.wt_id)
        .unwrap();
    assert_eq!(auto.base.as_ref().unwrap().base_ref, "main");

    // Valid: returns the refreshed worktree and the choice is remembered.
    let set = set_base_impl(&f.state, &f.wt_id, "release").await.unwrap();
    let base = set.base.as_ref().unwrap();
    assert_eq!(base.base_ref, "release");
    assert_eq!(base.ahead, 2); // `base files` + `branch work`
    let listed = f
        .worktrees()
        .await
        .into_iter()
        .find(|w| w.id == f.wt_id)
        .unwrap();
    assert_eq!(listed, set);
    // Measured against `release` (one commit further back) the commit that
    // added `mod.txt` and friends is part of the branch's work too.
    let c = f.changes(DiffMode::VsBase).await;
    assert!(c.iter().any(|c| c.path == "mod.txt"), "{:?}", summary(&c));

    // Surrounding whitespace is ignored.
    let set = set_base_impl(&f.state, &f.wt_id, "  main \n")
        .await
        .unwrap();
    assert_eq!(set.base.unwrap().base_ref, "main");

    // Clearing (empty or blank) goes back to auto-detection.
    set_base_impl(&f.state, &f.wt_id, "release").await.unwrap();
    let cleared = set_base_impl(&f.state, &f.wt_id, "").await.unwrap();
    assert_eq!(cleared.base.unwrap().base_ref, "main");
}

#[tokio::test]
async fn set_base_rejects_what_is_not_a_usable_base() {
    let f = Fixture::new().await;
    let invalid = |e: IpcError| matches!(e, IpcError::InvalidInput { .. });
    for bad in [
        "no-such-branch",
        "--help",                     // would be a flag without `--end-of-options`
        "--upload-pack=touch /tmp/x", // ditto
        "main:README.md",             // a blob, not a commit
        "main^{tree}",
        "feature", // the worktree's own branch
        "HEAD",    // ... or HEAD itself
        "a\0b",
    ] {
        let err = set_base_impl(&f.state, &f.wt_id, bad).await.unwrap_err();
        assert!(invalid(err.clone()), "{bad:?} gave {err:?}");
    }
    // A rejected choice is not stored: the base is still the detected one.
    let w = f
        .worktrees()
        .await
        .into_iter()
        .find(|w| w.id == f.wt_id)
        .unwrap();
    assert_eq!(w.base.unwrap().base_ref, "main");

    // A commit id is fine, and so is a tag (peeled to its commit).
    let sha = f.repo.git(["rev-parse", "main~1"]);
    f.repo.git(["tag", "v1", "main"]);
    assert!(set_base_impl(&f.state, &f.wt_id, &sha).await.is_ok());
    assert!(set_base_impl(&f.state, &f.wt_id, "v1").await.is_ok());
}

#[tokio::test]
async fn a_stored_base_that_no_longer_exists_falls_back_to_the_detected_base() {
    let f = Fixture::new().await;
    f.repo.git(["branch", "release", "HEAD~1"]);
    let set = set_base_impl(&f.state, &f.wt_id, "release").await.unwrap();
    assert_eq!(set.base.unwrap().base_ref, "release");

    // The branch disappears (merged and deleted). The stored choice is stale, but
    // nothing may fail: the worktree is measured against the detected base again.
    f.repo.git(["branch", "-D", "release"]);
    f.make_changes();
    let w = f
        .worktrees()
        .await
        .into_iter()
        .find(|w| w.id == f.wt_id)
        .unwrap();
    assert_eq!(w.base.as_ref().unwrap().base_ref, "main");
    let c = f.changes(DiffMode::VsBase).await;
    assert_eq!(w.change_count as usize, c.len());
    assert!(!c.is_empty());
}

// ---------------------------------------------------------------------------
// One failing worktree, set_base atomicity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_corrupt_worktree_does_not_hide_the_healthy_ones() {
    let f = Fixture::new().await;
    f.make_changes();
    let healthy = f.repo.add_worktree("healthy", "healthy-branch");
    let broken = f.repo.add_worktree("broken", "broken-branch");
    // Point the checkout's `.git` file at nothing: every git command in it fails
    // with "not a git repository", while `git worktree list` (which reads the
    // main repository) still lists it, and its directory exists.
    std::fs::write(broken.join(".git"), "gitdir: /nonexistent/place\n").unwrap();

    let list = f.worktrees().await;
    assert_eq!(list.len(), 4);
    let by_branch = |b: &str| {
        list.iter()
            .find(|w| w.branch.as_deref() == Some(b))
            .unwrap()
    };

    let bad = by_branch("broken-branch");
    assert_eq!((bad.base.clone(), bad.change_count), (None, 0));
    assert_eq!(Path::new(&bad.path), broken);
    // ... and says why, so the UI can tell "broken" from "clean".
    assert!(
        matches!(
            bad.load_error,
            Some(IpcError::GitFailed { .. } | IpcError::NotARepo)
        ),
        "{:?}",
        bad.load_error
    );
    // The others are unaffected and fully measured.
    let ok = by_branch("healthy-branch");
    assert_eq!(Path::new(&ok.path), healthy);
    assert_eq!(ok.base.as_ref().unwrap().base_ref, "main");
    assert_eq!(by_branch("feature").change_count, 8);
    assert!(list.iter().any(|w| w.is_main));
    // Only the broken one carries an error.
    assert!(list.iter().filter(|w| w.load_error.is_some()).count() == 1);
}

/// `mkfifo` for tests; `false` if the tool is missing.
#[cfg(unix)]
fn mkfifo(path: &Path) -> bool {
    std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(unix)]
#[tokio::test]
async fn set_base_that_fails_leaves_the_stored_override_unchanged() {
    let f = Fixture::new().await;
    f.repo.git(["branch", "release", "HEAD~1"]);
    set_base_impl(&f.state, &f.wt_id, "release").await.unwrap();
    let stored = || f.state.repos.base_override(&f.wt_id).unwrap();
    assert_eq!(stored().as_deref(), Some("release"));

    // Now make *measuring* the worktree fail (a tracked file replaced by a FIFO
    // breaks `git diff --numstat`), while the ref itself is perfectly valid.
    std::fs::remove_file(f.wt.join("mod.txt")).unwrap();
    if !mkfifo(&f.wt.join("mod.txt")) {
        eprintln!("mkfifo not available; skipping");
        return;
    }
    let err = set_base_impl(&f.state, &f.wt_id, "main").await.unwrap_err();
    assert!(matches!(err, IpcError::GitFailed { .. }), "{err:?}");
    assert_eq!(
        stored().as_deref(),
        Some("release"),
        "choice must not be stored"
    );

    // Clearing is the same operation and the same guarantee.
    let err = set_base_impl(&f.state, &f.wt_id, "").await.unwrap_err();
    assert!(matches!(err, IpcError::GitFailed { .. }), "{err:?}");
    assert_eq!(stored().as_deref(), Some("release"));

    // Once the worktree can be measured again, it works and is stored.
    std::fs::remove_file(f.wt.join("mod.txt")).unwrap();
    f.write("mod.txt", "one\ntwo\nthree\n");
    set_base_impl(&f.state, &f.wt_id, "main").await.unwrap();
    assert_eq!(stored().as_deref(), Some("main"));
}

// ---------------------------------------------------------------------------
// One row per path; names that look like revision syntax
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_file_removed_from_the_index_but_kept_on_disk_is_one_row_per_mode() {
    let f = Fixture::new().await;
    f.wt_git(["rm", "--cached", "-q", "mod.txt"]); // the file stays on disk
    f.write("mod.txt", "one\nTWO\nthree\n"); // ... and is edited afterwards

    // Working-tree views: ONE row (git itself reports "deleted" AND "untracked"),
    // modified, both flags set, readable with baseline and current content.
    for mode in [DiffMode::VsBase, DiffMode::Uncommitted] {
        let c = f.changes(mode).await;
        let rows: Vec<_> = c.iter().filter(|c| c.path == "mod.txt").collect();
        assert_eq!(rows.len(), 1, "{mode:?}: {c:?}");
        assert_eq!(rows[0].status, ChangeStatus::Modified, "{mode:?}");
        assert!(rows[0].staged && rows[0].unstaged, "{mode:?}");
        let d = f.diff("mod.txt", mode).await.unwrap();
        assert_eq!(
            d.old_content.as_deref(),
            Some("one\ntwo\nthree\n"),
            "{mode:?}"
        );
        assert_eq!(
            d.new_content.as_deref(),
            Some("one\nTWO\nthree\n"),
            "{mode:?}"
        );
    }

    // Index view: deleted, as git says; one row, no untracked duplicate.
    let c = f.changes(DiffMode::Staged).await;
    assert_eq!(
        summary(&c),
        [("mod.txt", ChangeStatus::Deleted, true, false)]
    );
    let d = f.diff("mod.txt", DiffMode::Staged).await.unwrap();
    assert_eq!(d.old_content.as_deref(), Some("one\ntwo\nthree\n"));
    assert_eq!(d.new_content, None);

    // The count in the rail agrees with the list.
    let wt = f
        .worktrees()
        .await
        .into_iter()
        .find(|w| w.id == f.wt_id)
        .unwrap();
    assert_eq!(
        wt.change_count as usize,
        f.changes(DiffMode::VsBase).await.len()
    );
}

#[cfg(unix)] // `:` is not a legal file-name character on Windows
#[tokio::test]
async fn names_that_look_like_revision_syntax_can_be_opened() {
    let f = Fixture::new().await;
    // Committed and later edited (old side by blob id), staged then edited on
    // disk (index side by blob id), and untracked (disk).
    f.write("0:foo", "committed\n");
    f.write(":x", "committed too\n");
    // `:x` would be read as pathspec magic; `./` makes it a plain path.
    f.wt_git(["add", "--", "0:foo", "./:x"]);
    f.wt_git(["commit", "-q", "-m", "odd names"]);
    f.write("0:foo", "staged version\n");
    f.wt_git(["add", "0:foo"]);
    f.write("0:foo", "disk version\n");
    f.write(":x", "edited on disk\n");
    f.write("a:b", "brand new\n");

    let d = f.diff("0:foo", DiffMode::Staged).await.unwrap();
    assert_eq!(d.old_content.as_deref(), Some("committed\n"));
    assert_eq!(d.new_content.as_deref(), Some("staged version\n"));
    let d = f.diff("0:foo", DiffMode::Uncommitted).await.unwrap();
    assert_eq!(d.new_content.as_deref(), Some("disk version\n"));

    let d = f.diff(":x", DiffMode::Uncommitted).await.unwrap();
    assert_eq!(d.old_content.as_deref(), Some("committed too\n"));
    assert_eq!(d.new_content.as_deref(), Some("edited on disk\n"));

    let d = f.diff("a:b", DiffMode::VsBase).await.unwrap();
    assert_eq!(d.new_content.as_deref(), Some("brand new\n"));

    // They are real list entries too, and still not a way out of the worktree.
    let c = f.changes(DiffMode::Uncommitted).await;
    assert!(c.iter().any(|c| c.path == "0:foo") && c.iter().any(|c| c.path == ":x"));
    assert!(matches!(
        f.diff("0:../../etc/passwd", DiffMode::Uncommitted).await,
        Err(IpcError::InvalidInput { .. }) // a real `..` component
    ));
}

// ---------------------------------------------------------------------------
// Opening worktree files safely
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn the_open_guard_refuses_symlinks_and_never_blocks_on_fifos() {
    use why_lib::git::diff::{Opened, open_in_worktree, open_nofollow};
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join("real.txt"), "hello\n").unwrap();
    std::os::unix::fs::symlink("real.txt", root.join("link")).unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    if !mkfifo(&root.join("pipe")) {
        eprintln!("mkfifo not available; skipping");
        return;
    }
    let five = Duration::from_secs(5);

    // Low level: a symlink swapped in at the last moment is refused by the
    // kernel (`O_NOFOLLOW`): an error without an OS code, i.e. our own refusal.
    let err = open_nofollow(&root.join("link")).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(err.raw_os_error().is_none());
    // A FIFO opens at once (`O_NONBLOCK`; a plain open would wait for a writer),
    // and `fstat` on the handle shows it is not a regular file.
    let fifo = tokio::time::timeout(five, open_nofollow(&root.join("pipe")))
        .await
        .expect("opening a FIFO must not block")
        .unwrap();
    assert!(!fifo.metadata().await.unwrap().is_file());

    // The guarded open classifies everything without reading special files.
    let look = |name: &'static str| {
        let root = root.clone();
        async move {
            tokio::time::timeout(five, open_in_worktree(&root, Path::new(name)))
                .await
                .expect("must not block")
                .unwrap()
        }
    };
    assert!(matches!(
        look("real.txt").await,
        Opened::File { len: 6, .. }
    ));
    assert!(matches!(look("link").await, Opened::Symlink(t) if t == Path::new("real.txt")));
    assert!(matches!(look("pipe").await, Opened::NotAFile));
    assert!(matches!(look("sub").await, Opened::NotAFile));
    assert!(matches!(look("missing").await, Opened::Absent));
    assert!(matches!(look("nodir/file").await, Opened::Absent));
}

#[cfg(unix)]
#[tokio::test]
async fn line_counting_uses_the_same_guarded_open() {
    use why_lib::git::{Change, ChangeKind, count_untracked_lines};
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("f"), "1\n2\n3\n4\n5\n").unwrap();

    std::fs::create_dir(root.join("ok")).unwrap();
    std::fs::write(root.join("ok/f"), "a\nb\n").unwrap();
    // `swapped` used to be a directory; now it is a symlink to the outside one.
    std::os::unix::fs::symlink(outside.path(), root.join("swapped")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("f"), root.join("leaf_link")).unwrap();
    let has_fifo = mkfifo(&root.join("pipe"));

    let untracked = |path: &str| Change {
        path: path.into(),
        old_path: None,
        kind: ChangeKind::Untracked,
        staged: false,
        unstaged: true,
        additions: None,
        deletions: None,
        binary: false,
        old_mode: 0,
        new_mode: 0,
        old_oid: None,
        new_oid: None,
    };
    let mut changes = vec![
        untracked("ok/f"),
        untracked("swapped/f"), // parent directory is a symlink out of the root
        untracked("leaf_link"), // a symlink: one line, like git, never followed
        untracked("pipe"),      // a FIFO: must not block
    ];
    tokio::time::timeout(
        Duration::from_secs(5),
        count_untracked_lines(&root, &mut changes),
    )
    .await
    .expect("counting must not block");

    assert_eq!(changes[0].additions, Some(2));
    assert_eq!(
        changes[1].additions, None,
        "counted through a symlinked directory"
    );
    assert_eq!(changes[2].additions, Some(1));
    if has_fifo {
        assert_eq!(changes[3].additions, None);
    }
}

// ---------------------------------------------------------------------------
// Other object formats and partial clones
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_sha256_repository_works_before_and_after_its_first_commit() {
    let root = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(root.path()).unwrap().join("repo");
    std::fs::create_dir(&path).unwrap();
    if !git_raw(
        &path,
        ["init", "-q", "--object-format=sha256", "-b", "main"],
    )
    .status
    .success()
    {
        eprintln!("this git cannot create SHA-256 repositories; skipping");
        return;
    }
    crate::common::configure(&path);
    write_in(&path, "staged.txt", b"s\n");
    git(&path, ["add", "staged.txt"]);
    write_in(&path, "loose.txt", b"l\n");

    let state = state();
    let repo = add_repository_impl(&state, path_str(&path)).await.unwrap();
    let wt = list_worktrees_impl(&state, &repo.id)
        .await
        .unwrap()
        .remove(0);

    // Unborn: the empty tree of THIS repository is the SHA-256 one.
    let empty = why_lib::git::diff::empty_tree_id(&state.git, &path)
        .await
        .unwrap();
    assert_eq!(
        empty,
        "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321"
    );
    assert_eq!(wt.change_count, 2);
    let staged = get_changes_impl(&state, &wt.id, DiffMode::Staged)
        .await
        .unwrap();
    assert_eq!(
        summary(&staged),
        [("staged.txt", ChangeStatus::Added, true, false)]
    );
    let d = get_file_diff_impl(&state, &wt.id, "staged.txt", DiffMode::Staged)
        .await
        .unwrap();
    assert_eq!(d.new_content.as_deref(), Some("s\n"));

    // After a commit (64-hex object ids everywhere), edits are diffed normally.
    git(&path, ["commit", "-q", "-m", "first"]);
    write_in(&path, "staged.txt", b"s\nchanged\n");
    let d = get_file_diff_impl(&state, &wt.id, "staged.txt", DiffMode::Uncommitted)
        .await
        .unwrap();
    assert_eq!(d.old_content.as_deref(), Some("s\n"));
    assert_eq!(d.new_content.as_deref(), Some("s\nchanged\n"));
}

/// A blobless clone (`--filter=blob:none`) of a local bare repository, served
/// over `file://` (no network). One file with two versions; after `reset --soft`
/// HEAD is the *first* commit, whose blob was never downloaded.
struct PartialClone {
    _root: tempfile::TempDir,
    path: PathBuf,
}

fn partial_clone() -> PartialClone {
    let root = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(root.path()).unwrap();
    let seed = base.join("seed");
    std::fs::create_dir(&seed).unwrap();
    git(&seed, ["init", "-q", "-b", "main"]);
    crate::common::configure(&seed);
    write_in(&seed, "a.txt", b"version one\n");
    git(&seed, ["add", "-A"]);
    git(&seed, ["commit", "-q", "-m", "one"]);
    write_in(&seed, "a.txt", b"version two, a bit longer\n");
    git(&seed, ["commit", "-q", "-a", "-m", "two"]);

    let origin = base.join("origin.git");
    git(
        &base,
        [
            std::ffi::OsString::from("clone"),
            "-q".into(),
            "--bare".into(),
            seed.clone().into_os_string(),
            origin.clone().into_os_string(),
        ],
    );
    // A server must opt in to serving filtered clones.
    git(&origin, ["config", "uploadpack.allowFilter", "true"]);
    git(&origin, ["config", "uploadpack.allowAnySHA1InWant", "true"]);

    let clone = base.join("clone");
    git(
        &base,
        [
            std::ffi::OsString::from("clone"),
            "-q".into(),
            "--filter=blob:none".into(),
            format!("file://{}", origin.display()).into(),
            clone.clone().into_os_string(),
        ],
    );
    crate::common::configure(&clone);
    // Move HEAD back without touching the index or any blob: the old commit's
    // blob ("version one") is missing locally, the index/file hold "version two".
    git(&clone, ["reset", "--soft", "-q", "HEAD~1"]);
    PartialClone {
        _root: root,
        path: clone,
    }
}

/// Every file under `.git/objects` with its size: new objects or packs show up.
fn object_fingerprint(repo: &Path) -> Vec<(PathBuf, u64)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                walk(&entry.path(), out);
            } else {
                out.push((entry.path(), meta.len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo.join(".git/objects"), &mut out);
    out.sort();
    out
}

#[tokio::test]
async fn a_partial_clone_is_never_fetched_into() {
    let pc = partial_clone();
    let before = object_fingerprint(&pc.path);

    let state = state();
    let repo = add_repository_impl(&state, path_str(&pc.path))
        .await
        .unwrap();
    // The listing degrades (the worktree cannot be measured offline) and does
    // not fail; the file requests report an error instead of downloading.
    let wt = list_worktrees_impl(&state, &repo.id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(wt.change_count, 0);
    for mode in [DiffMode::Uncommitted, DiffMode::Staged] {
        let err = get_changes_impl(&state, &wt.id, mode).await.unwrap_err();
        assert!(
            matches!(err, IpcError::GitFailed { .. }),
            "{mode:?}: {err:?}"
        );
        let err = get_file_diff_impl(&state, &wt.id, "a.txt", mode)
            .await
            .unwrap_err();
        assert!(
            matches!(err, IpcError::GitFailed { .. }),
            "{mode:?}: {err:?}"
        );
    }
    // A direct blob read by id says "not available locally" instead of fetching.
    let missing = git(&pc.path, ["rev-parse", "HEAD:a.txt"]);
    let err = why_lib::git::diff::read_blob_side(
        &state.git,
        &pc.path,
        why_lib::git::diff::oid_spec(&missing).unwrap(),
        false,
    )
    .await
    .unwrap_err();
    assert!(err.is_exit_code(128), "{err:?}");

    assert_eq!(
        object_fingerprint(&pc.path),
        before,
        "an object or pack was written: WHY fetched from the remote"
    );
}

#[tokio::test]
async fn negative_control_plain_git_fetches_into_a_partial_clone() {
    // Proves the test above has teeth: the same diff WITHOUT `GIT_NO_LAZY_FETCH`
    // downloads the missing blob and writes a new pack. (`git_raw` scrubs the
    // inherited `GIT_*` variables.)
    let pc = partial_clone();
    let before = object_fingerprint(&pc.path);
    let out = git_raw(&pc.path, ["diff", "--cached", "--numstat"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_ne!(object_fingerprint(&pc.path), before);
}

// ---------------------------------------------------------------------------
// Performance (ignored by default; run explicitly, see the report)
// ---------------------------------------------------------------------------

/// `cargo test --test ipc -- --ignored --nocapture bench_100_changed_files`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark: run explicitly with --ignored --nocapture"]
async fn bench_100_changed_files() {
    let repo = TestRepo::with_initial_commit();
    for i in 0..120 {
        repo.write(
            &format!("src/mod{i}/file{i}.rs"),
            &format!("fn f{i}() {{}}\n").repeat(40),
        );
    }
    repo.commit_all("many files");
    let wt = repo.add_worktree("wt", "feature");
    // 40 unstaged edits, 20 staged edits, 15 deletions, 10 staged renames, 15 untracked.
    for i in 0..40 {
        write_in(
            &wt,
            &format!("src/mod{i}/file{i}.rs"),
            b"fn changed() {}\nfn more() {}\n",
        );
    }
    for i in 40..60 {
        write_in(&wt, &format!("src/mod{i}/file{i}.rs"), b"fn staged() {}\n");
        git(&wt, ["add", &format!("src/mod{i}/file{i}.rs")]);
    }
    for i in 60..75 {
        std::fs::remove_file(wt.join(format!("src/mod{i}/file{i}.rs"))).unwrap();
    }
    for i in 75..85 {
        git(
            &wt,
            [
                "mv",
                &format!("src/mod{i}/file{i}.rs"),
                &format!("src/mod{i}/renamed{i}.rs"),
            ],
        );
    }
    for i in 0..15 {
        write_in(&wt, &format!("untracked/u{i}.txt"), b"u\nu\nu\n");
    }

    let state = state();
    let added = add_repository_impl(&state, path_str(&repo.path))
        .await
        .unwrap();
    let wt_id = worktree_id_at(&state, &added.id, &wt).await;

    async fn time<T, F: std::future::Future<Output = T>>(
        label: &str,
        mut run: impl FnMut() -> F,
    ) -> T {
        let mut samples = Vec::new();
        let mut last = None;
        for _ in 0..7 {
            let t = Instant::now();
            last = Some(run().await);
            samples.push(t.elapsed());
        }
        samples.sort();
        eprintln!(
            "{label:<28} min {:>6.1} ms   median {:>6.1} ms   max {:>6.1} ms",
            samples[0].as_secs_f64() * 1e3,
            samples[3].as_secs_f64() * 1e3,
            samples[6].as_secs_f64() * 1e3
        );
        assert!(
            samples[3] < Duration::from_millis(150),
            "{label}: median over 150 ms"
        );
        last.unwrap()
    }

    for mode in [DiffMode::VsBase, DiffMode::Uncommitted, DiffMode::Staged] {
        let list = time(&format!("get_changes {mode:?}"), || {
            get_changes_impl(&state, &wt_id, mode)
        })
        .await;
        eprintln!("  -> {} files", list.unwrap().len());
    }
    let one = time("get_file_diff (1 file)", || {
        get_file_diff_impl(&state, &wt_id, "src/mod1/file1.rs", DiffMode::VsBase)
    })
    .await;
    assert!(one.is_ok());
    // The cost of resolving a worktree id: one `git worktree list` per registered
    // repository (here one), paid by every get_changes / get_file_diff call.
    let info = why_lib::git::open_repo(&state.git, &repo.path)
        .await
        .unwrap();
    time("resolve id (worktree list)", || {
        why_lib::git::list_worktrees(&state.git, &info)
    })
    .await
    .unwrap();
    // Where the time goes, one piece at a time (all are part of get_changes).
    let base = time("  detect_base", || {
        why_lib::git::detect_base(&state.git, &wt, None)
    })
    .await
    .unwrap()
    .unwrap();
    time("  git status", || {
        why_lib::git::read_status(&state.git, &wt)
    })
    .await
    .unwrap();
    let target = why_lib::git::Target::WorkingTree {
        include_untracked: true,
    };
    let old = why_lib::git::OldSide::Commit(base.merge_base.clone());
    time("  read_changes (status+diff)", || {
        why_lib::git::read_changes(&state.git, &wt, &old, target)
    })
    .await
    .unwrap();
    let list = time("list_worktrees (2 worktrees)", || {
        list_worktrees_impl(&state, &added.id)
    })
    .await;
    eprintln!(
        "  -> change_count {:?}",
        list.unwrap()
            .iter()
            .map(|w| w.change_count)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn last_activity_is_the_newest_edit_or_else_the_head_log() {
    let f = Fixture::new().await;

    // Clean worktree: the HEAD log (the checkout that created it) is the signal.
    let clean = f.worktrees().await;
    let clean = clean.iter().find(|w| w.id == f.wt_id).unwrap();
    assert_eq!(clean.change_count, 0);
    let log_time = clean
        .last_activity
        .expect("a clean worktree has a HEAD log");
    assert!(log_time > 1_600_000_000);

    // An edit stamped far in the future wins over the log.
    f.write("mod.txt", "edited\n");
    let far = std::time::UNIX_EPOCH + Duration::from_secs(4_000_000_000);
    std::fs::File::options()
        .write(true)
        .open(f.wt.join("mod.txt"))
        .unwrap()
        .set_modified(far)
        .unwrap();
    let list = f.worktrees().await;
    let edited = list.iter().find(|w| w.id == f.wt_id).unwrap();
    assert_eq!(edited.last_activity, Some(4_000_000_000));
}
