use std::path::{Path, PathBuf};

use why_lib::git::{
    AheadBehind, BranchHead, ConflictKind, FileState, GitRunner, RenameOp, StatusEntry, StatusKind,
    read_status,
};

use crate::common::{TestRepo, git_raw, write_in};

fn find<'a>(entries: &'a [StatusEntry], path: &str) -> &'a StatusEntry {
    entries
        .iter()
        .find(|e| e.path == Path::new(path))
        .unwrap_or_else(|| panic!("no entry for {path:?} in {entries:#?}"))
}

#[tokio::test]
async fn clean_worktree() {
    let repo = TestRepo::with_initial_commit();
    let s = read_status(&GitRunner::new(), &repo.path).await.unwrap();
    assert!(s.entries.is_empty());
    assert_eq!(s.branch.head, Some(BranchHead::Branch("main".into())));
    assert_eq!(
        s.branch.oid.unwrap().as_str(),
        repo.git(["rev-parse", "HEAD"])
    );
    assert_eq!(s.branch.upstream, None);
    assert_eq!(s.branch.ahead_behind, None);
}

#[tokio::test]
async fn unborn_branch_has_no_oid() {
    let repo = TestRepo::new();
    repo.write("new.txt", "x\n");
    let s = read_status(&GitRunner::new(), &repo.path).await.unwrap();
    assert_eq!(s.branch.oid, None);
    assert_eq!(s.branch.head, Some(BranchHead::Branch("main".into())));
    assert_eq!(find(&s.entries, "new.txt").kind, StatusKind::Untracked);
}

#[tokio::test]
async fn staged_rename_unstaged_modify_and_nested_untracked() {
    let repo = TestRepo::with_initial_commit();
    // Enough content for git's similarity-based rename detection.
    let body = "line one\nline two\nline three\nline four\nline five\n";
    repo.write("old name.rs", body);
    repo.write("mod.rs", "fn a() {}\n");
    repo.commit_all("base files");

    repo.git(["mv", "old name.rs", "new name.rs"]); // staged rename
    repo.write("mod.rs", "fn a() {}\nfn b() {}\n"); // unstaged modify
    repo.write("dir/nested/deep file.txt", "u\n"); // untracked, nested

    let s = read_status(&GitRunner::new(), &repo.path).await.unwrap();
    assert_eq!(s.entries.len(), 3, "{:#?}", s.entries);

    let renamed = find(&s.entries, "new name.rs");
    assert_eq!(
        renamed.kind,
        StatusKind::Renamed {
            staged: FileState::Renamed,
            unstaged: FileState::Unmodified,
            orig_path: PathBuf::from("old name.rs"),
            op: RenameOp::Rename,
            score: 100,
        }
    );
    assert_eq!(
        find(&s.entries, "mod.rs").kind,
        StatusKind::Changed {
            staged: FileState::Unmodified,
            unstaged: FileState::Modified
        }
    );
    // `--untracked-files=all` lists the file, not just `dir/`.
    assert_eq!(
        find(&s.entries, "dir/nested/deep file.txt").kind,
        StatusKind::Untracked
    );
}

#[tokio::test]
async fn staged_and_unstaged_changes_to_the_same_file() {
    let repo = TestRepo::with_initial_commit();
    repo.write("a.txt", "1\n");
    repo.commit_all("a");
    repo.write("a.txt", "2\n");
    repo.git(["add", "a.txt"]);
    repo.write("a.txt", "3\n");
    repo.write("added.txt", "new\n");
    repo.git(["add", "added.txt"]);
    repo.write("README.md", "hello\n");
    std::fs::remove_file(repo.path.join("README.md")).unwrap();

    let s = read_status(&GitRunner::new(), &repo.path).await.unwrap();
    assert_eq!(
        find(&s.entries, "a.txt").kind,
        StatusKind::Changed {
            staged: FileState::Modified,
            unstaged: FileState::Modified
        }
    );
    assert_eq!(
        find(&s.entries, "added.txt").kind,
        StatusKind::Changed {
            staged: FileState::Added,
            unstaged: FileState::Unmodified
        }
    );
    assert_eq!(
        find(&s.entries, "README.md").kind,
        StatusKind::Changed {
            staged: FileState::Unmodified,
            unstaged: FileState::Deleted
        }
    );
}

/// Create each file, run status, and check every name round-trips and the
/// `PathBuf` really points at the file.
async fn assert_names_round_trip(names: &[&str]) {
    let repo = TestRepo::with_initial_commit();
    for (i, name) in names.iter().enumerate() {
        repo.write(name, &format!("{i}\n"));
    }

    let s = read_status(&GitRunner::new(), &repo.path).await.unwrap();
    let shown: Vec<_> = s
        .entries
        .iter()
        .map(|e| e.display_path().into_owned())
        .collect();
    assert_eq!(shown.len(), names.len(), "{shown:?}");
    for expected in names {
        assert!(
            shown.iter().any(|n| n == expected),
            "missing {expected:?} in {shown:?}"
        );
        assert!(repo.path.join(&find(&s.entries, expected).path).is_file());
    }
}

/// Valid on every platform.
#[tokio::test]
async fn awkward_file_names_space_and_unicode() {
    assert_names_round_trip(&["with space.txt", "tệp mới.rs", "sub dir/ünï.txt"]).await;
}

/// A newline and a double quote are not allowed in Windows file names (and
/// would panic the fixture there), so these run on Unix only.
#[cfg(unix)]
#[tokio::test]
async fn awkward_file_names_newline_and_quote() {
    assert_names_round_trip(&["line1\nline2.txt", "quote\"d.txt", "dir\nname/f.txt"]).await;
}

#[cfg(unix)]
#[tokio::test]
async fn non_utf8_file_name_is_preserved() {
    use std::os::unix::ffi::OsStrExt;
    let repo = TestRepo::with_initial_commit();
    let name = std::ffi::OsStr::from_bytes(b"caf\xe9-\xff.txt");
    std::fs::write(repo.path.join(name), "x").unwrap();

    let s = read_status(&GitRunner::new(), &repo.path).await.unwrap();
    assert_eq!(s.entries.len(), 1);
    let e = &s.entries[0];
    assert_eq!(e.path.as_os_str().as_bytes(), b"caf\xe9-\xff.txt");
    assert_eq!(e.display_path(), "caf\u{fffd}-\u{fffd}.txt");
    assert!(repo.path.join(&e.path).is_file(), "usable to open the file");
}

#[tokio::test]
async fn merge_conflict_yields_unmerged_entries() {
    let repo = TestRepo::with_initial_commit();
    repo.write("c.txt", "base\n");
    repo.write("gone.txt", "base\n");
    repo.commit_all("base");

    repo.git(["switch", "-q", "-c", "other"]);
    repo.write("c.txt", "other side\n");
    repo.git(["rm", "-q", "gone.txt"]);
    repo.commit_all("other");

    repo.git(["switch", "-q", "main"]);
    repo.write("c.txt", "main side\n");
    repo.write("gone.txt", "base changed on main\n");
    repo.commit_all("main");

    // A conflicting merge exits 1; do not use the asserting helper.
    let out = git_raw(&repo.path, ["merge", "other"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let s = read_status(&GitRunner::new(), &repo.path).await.unwrap();
    assert_eq!(
        find(&s.entries, "c.txt").kind,
        StatusKind::Unmerged {
            conflict: ConflictKind::BothModified
        }
    );
    // `other` deleted it while `main` modified it.
    assert_eq!(
        find(&s.entries, "gone.txt").kind,
        StatusKind::Unmerged {
            conflict: ConflictKind::DeletedByThem
        }
    );
}

#[tokio::test]
async fn upstream_and_ahead_behind_headers() {
    let (_root, origin) = TestRepo::bare_origin_with_commit();
    let clone = TestRepo::clone_of(&origin);
    write_in(&clone.path, "local.txt", b"x\n");
    clone.commit_all("local 1");
    write_in(&clone.path, "local2.txt", b"y\n");
    clone.commit_all("local 2");

    let s = read_status(&GitRunner::new(), &clone.path).await.unwrap();
    assert_eq!(s.branch.upstream.as_deref(), Some("origin/main"));
    assert_eq!(
        s.branch.ahead_behind,
        Some(AheadBehind {
            ahead: 2,
            behind: 0
        })
    );
}

#[tokio::test]
async fn detached_head_header() {
    let repo = TestRepo::with_initial_commit();
    repo.git(["checkout", "-q", "--detach"]);
    let s = read_status(&GitRunner::new(), &repo.path).await.unwrap();
    assert_eq!(s.branch.head, Some(BranchHead::Detached));
    assert!(s.branch.oid.is_some());
}

#[tokio::test]
async fn status_runs_in_a_linked_worktree_independently() {
    let repo = TestRepo::with_initial_commit();
    let wt = repo.add_worktree("wt", "feature");
    write_in(&wt, "only-in-wt.txt", b"x\n");

    let main = read_status(&GitRunner::new(), &repo.path).await.unwrap();
    let linked = read_status(&GitRunner::new(), &wt).await.unwrap();
    assert!(
        main.entries.is_empty(),
        "the file lives only in the other worktree"
    );
    assert_eq!(linked.entries.len(), 1);
    assert_eq!(
        linked.branch.head,
        Some(BranchHead::Branch("feature".into()))
    );
}
