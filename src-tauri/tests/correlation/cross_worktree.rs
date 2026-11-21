//! Decision 32 (WHY-65): an edit belongs to the worktree of the EDITED FILE, not
//! to the session's cwd. Real adapters, the real git resolver and ingest, on
//! throw-away repositories, once per store backend.
//!
//! The WHY-19 dogfood finding these cases come from: a session whose cwd was the
//! main checkout wrote a file in another registered worktree by absolute path,
//! and that worktree showed "No sessions recorded".

use std::os::unix::fs::symlink;
use std::path::Path;

use serde_json::json;
use why_lib::events::{AppendOutcome, SessionKey};
use why_lib::provenance::{EditKind, Observed};
use why_wire::Harness;

use crate::common::{TestRepo, git, write_in};
use crate::rig::{BACKENDS, Rig, Wt, s};

fn seed(repo: &TestRepo) {
    repo.write("shared.rs", "base\n");
    repo.commit_all("base");
}

fn claude(session: &str) -> SessionKey {
    SessionKey::new(Harness::ClaudeCode, session)
}

/// A successful Claude `Edit` of `file` (Pre, then Post with a real
/// `structuredPatch`); returns the Post and what the store reported for it.
async fn edit_env(
    rig: &Rig,
    session: &str,
    use_id: &str,
    cwd: &Path,
    file: &str,
) -> (why_wire::Envelope, AppendOutcome) {
    let base = |hook: &str| {
        json!({"session_id": session, "hook_event_name": hook, "prompt_id": "p1",
               "tool_name": "Edit", "tool_use_id": use_id,
               "tool_input": {"file_path": file, "old_string": "b", "new_string": "B"}})
    };
    rig.send(Harness::ClaudeCode, "PreToolUse", cwd, base("PreToolUse"))
        .await;
    let mut post = base("PostToolUse");
    post["tool_response"] = json!({
        "filePath": file, "originalFile": "a\nb\n", "userModified": false,
        "structuredPatch": [{"oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
                              "lines": [" a", "-b", "+B"]}],
    });
    rig.send_outcome(Harness::ClaudeCode, "PostToolUse", cwd, post)
        .await
}

async fn edit(rig: &Rig, session: &str, use_id: &str, cwd: &Path, file: &str) -> AppendOutcome {
    edit_env(rig, session, use_id, cwd, file).await.1
}

fn sessions(rig: &Rig, wt: &Wt) -> Vec<String> {
    rig.state
        .events
        .sessions_for_worktree(&wt.id)
        .unwrap()
        .into_iter()
        .map(|s| s.id.0)
        .collect()
}

fn ids(outcome: &AppendOutcome) -> Vec<&str> {
    outcome
        .changed_worktrees
        .iter()
        .map(|w| w.0.as_str())
        .collect()
}

fn records(rig: &Rig, wt: &Wt, path: &str) -> usize {
    rig.state.events.edit_records(&wt.id, path).unwrap().len()
}

fn touches(rig: &Rig, wt: &Wt, path: &str) -> Vec<String> {
    rig.state
        .events
        .edit_calls(&wt.id, &[path.to_owned()])
        .unwrap()
        .iter()
        .map(|t| format!("{}#{}", t.session.session_id, t.turn_id))
        .collect()
}

#[tokio::test]
async fn a_session_in_a_edits_a_file_of_b_and_belongs_to_both() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed).await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await; // the main checkout
        let b = rig.linked("wt-b", "feature/b").await;
        let file = s(&b.path.join("src/lib.rs"));
        write_in(&b.path, "src/lib.rs", b"a\nb\n");

        // The session's cwd is the MAIN checkout; the file is in B.
        let outcome = edit(&rig, "s1", "tu1", &a.path, &file).await;
        assert_eq!(
            {
                let mut v = ids(&outcome);
                v.sort();
                v
            },
            {
                let mut v = vec![a.id.0.as_str(), b.id.0.as_str()];
                v.sort();
                v
            },
            "{backend:?}: both worktrees are told to refresh"
        );

        // The op, the v0.2 file -> turn attribution and the session list are B's.
        assert_eq!(records(&rig, &b, "src/lib.rs"), 1, "{backend:?}");
        assert_eq!(records(&rig, &a, "src/lib.rs"), 0, "{backend:?}");
        assert_eq!(touches(&rig, &b, "src/lib.rs"), ["s1#p1"], "{backend:?}");
        assert!(touches(&rig, &a, "src/lib.rs").is_empty(), "{backend:?}");
        assert_eq!(sessions(&rig, &a), ["claude-code:s1"], "{backend:?}");
        assert_eq!(sessions(&rig, &b), ["claude-code:s1"], "{backend:?}");

        // The op itself: exact, relative to B's root, with its hunk.
        let rec = &rig.state.events.edit_records(&b.id, "src/lib.rs").unwrap()[0];
        assert_eq!(rec.op.worktree_id, Some(b.id.clone()));
        assert_eq!(rec.op.path, "src/lib.rs");
        assert_eq!(rec.op.kind, EditKind::Update);
        assert_eq!(rec.op.observed, Observed::Exact);
        assert_eq!(rec.op.hunks.len(), 1);
        assert_eq!(rec.source.session, claude("s1"));
        assert_eq!(rec.source.tool_use_id.as_deref(), Some("tu1"));

        // (Duplicate delivery is covered by `a_redelivery_stores_the_op_once`.)
    }
}

#[tokio::test]
async fn the_same_relative_path_in_a_and_b_stays_separate() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed).await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await;
        let b = rig.linked("wt-b", "feature/b").await;
        // Two sessions, each editing `shared.rs` in its OWN worktree; a third
        // reaches across from A into B's copy.
        edit(&rig, "in-a", "t1", &a.path, &s(&a.path.join("shared.rs"))).await;
        edit(&rig, "in-b", "t2", &b.path, &s(&b.path.join("shared.rs"))).await;
        edit(&rig, "cross", "t3", &a.path, &s(&b.path.join("shared.rs"))).await;

        assert_eq!(records(&rig, &a, "shared.rs"), 1, "{backend:?}");
        assert_eq!(records(&rig, &b, "shared.rs"), 2, "{backend:?}");
        assert_eq!(touches(&rig, &a, "shared.rs"), ["in-a#p1"], "{backend:?}");
        let mut in_b = touches(&rig, &b, "shared.rs");
        in_b.sort();
        assert_eq!(in_b, ["cross#p1", "in-b#p1"], "{backend:?}");
        // Sessions: A lists in-a and cross (its cwd), B lists in-b and cross
        // (its file).
        let mut in_a = sessions(&rig, &a);
        in_a.sort();
        assert_eq!(
            in_a,
            ["claude-code:cross", "claude-code:in-a"],
            "{backend:?}"
        );
        let mut in_b = sessions(&rig, &b);
        in_b.sort();
        assert_eq!(
            in_b,
            ["claude-code:cross", "claude-code:in-b"],
            "{backend:?}"
        );
    }
}

#[tokio::test]
async fn an_edit_through_a_symlinked_directory_belongs_to_the_real_worktree() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed).await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await;
        let b = rig.linked("wt-b", "feature/b").await;
        write_in(&b.path, "sub/keep.txt", b"x\n");
        // A link OUTSIDE both worktrees that points into B's `sub/`.
        let links = tempfile::tempdir().unwrap();
        let link = links.path().join("ln");
        symlink(b.path.join("sub"), &link).unwrap();

        edit(&rig, "s1", "t1", &a.path, &s(&link.join("f.rs"))).await;
        assert_eq!(records(&rig, &b, "sub/f.rs"), 1, "{backend:?}");
        assert_eq!(touches(&rig, &b, "sub/f.rs"), ["s1#p1"], "{backend:?}");
        assert_eq!(sessions(&rig, &b), ["claude-code:s1"], "{backend:?}");
    }
}

#[tokio::test]
async fn a_new_file_in_a_directory_that_does_not_exist_yet() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed).await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await;
        let b = rig.linked("wt-b", "feature/b").await;
        // `newdir/deeper` is nowhere on disk: git is asked about the nearest
        // directory that exists (B's root) and the missing part is kept.
        let file = s(&b.path.join("newdir/deeper/n.rs"));
        assert!(!b.path.join("newdir").exists());
        edit(&rig, "s1", "t1", &a.path, &file).await;
        assert_eq!(records(&rig, &b, "newdir/deeper/n.rs"), 1, "{backend:?}");
        assert_eq!(
            touches(&rig, &b, "newdir/deeper/n.rs"),
            ["s1#p1"],
            "{backend:?}"
        );
    }
}

#[tokio::test]
async fn a_file_outside_every_registered_worktree_keeps_the_event_but_has_no_op() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed).await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await;
        let outside = tempfile::tempdir().unwrap();
        let file = s(&outside.path().join("x.rs"));

        let outcome = edit(&rig, "s1", "t1", &a.path, &file).await;
        assert_eq!(
            ids(&outcome),
            [a.id.0.as_str()],
            "{backend:?}: only the cwd's worktree"
        );
        // The event is stored (the session is listed in its cwd's worktree) ...
        assert_eq!(sessions(&rig, &a), ["claude-code:s1"], "{backend:?}");
        let timeline = rig.state.events.timeline(&claude("s1")).unwrap().unwrap();
        let files: Vec<_> = timeline
            .turns
            .iter()
            .flat_map(|t| &t.events)
            .flat_map(|e| e.files.iter().map(String::as_str))
            .collect();
        assert!(files.contains(&file.as_str()), "{backend:?}: {files:?}");
        // ... but there is no op, and nothing is attributed to A by prefix.
        assert_eq!(records(&rig, &a, "x.rs"), 0, "{backend:?}");
        assert!(touches(&rig, &a, "x.rs").is_empty(), "{backend:?}");
    }
}

#[tokio::test]
async fn a_nested_repository_is_not_attributed_to_the_outer_worktree_by_prefix() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed).await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await;
        // A repository inside A's tree (a vendored checkout, a submodule's
        // directory) that WHY does not know: its files are not A's.
        let nested = a.path.join("vendor/lib");
        std::fs::create_dir_all(&nested).unwrap();
        git(&nested, ["init", "-q"]);
        write_in(&a.path, "vendor/lib/x.rs", b"a\nb\n");

        edit(&rig, "s1", "t1", &a.path, &s(&nested.join("x.rs"))).await;
        assert_eq!(records(&rig, &a, "vendor/lib/x.rs"), 0, "{backend:?}");
        assert_eq!(records(&rig, &a, "x.rs"), 0, "{backend:?}");
        assert!(
            touches(&rig, &a, "vendor/lib/x.rs").is_empty(),
            "{backend:?}"
        );
        // A file of A itself still is.
        edit(&rig, "s1", "t2", &a.path, &s(&a.path.join("shared.rs"))).await;
        assert_eq!(records(&rig, &a, "shared.rs"), 1, "{backend:?}");
    }
}

#[tokio::test]
async fn a_redelivery_stores_the_op_once_and_announces_nothing() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed).await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await;
        let b = rig.linked("wt-b", "feature/b").await;
        let file = s(&b.path.join("f.rs"));
        let (post, first) = edit_env(&rig, "s1", "t1", &a.path, &file).await;
        assert_eq!(first.inserted, 1);

        let again = rig.deliver(&post).await;
        assert_eq!((again.inserted, again.duplicates), (0, 1), "{backend:?}");
        assert!(again.changed_worktrees.is_empty(), "{backend:?}");
        assert_eq!(records(&rig, &b, "f.rs"), 1, "{backend:?}");
    }
}

#[tokio::test]
async fn a_codex_patch_over_two_worktrees_splits_per_file() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed).await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await;
        let b = rig.linked("wt-b", "feature/b").await;
        let (fa, fb) = (s(&a.path.join("one.rs")), s(&b.path.join("two.rs")));
        let patch = [
            "*** Begin Patch".to_owned(),
            format!("*** Add File: {fa}"),
            "+a".to_owned(),
            format!("*** Add File: {fb}"),
            "+b".to_owned(),
            "*** End Patch".to_owned(),
        ];
        let refs: Vec<&str> = patch.iter().map(String::as_str).collect();
        rig.x_patch("cx", "t1", "u1", &a.path, &refs, crate::rig::End::Ok)
            .await;

        assert_eq!(records(&rig, &a, "one.rs"), 1, "{backend:?}");
        assert_eq!(records(&rig, &b, "two.rs"), 1, "{backend:?}");
        assert_eq!(records(&rig, &a, "two.rs"), 0, "{backend:?}");
        // Each worktree's call names only ITS file.
        assert_eq!(touches(&rig, &a, "one.rs"), ["cx#t1"], "{backend:?}");
        assert_eq!(touches(&rig, &b, "two.rs"), ["cx#t1"], "{backend:?}");
        assert!(touches(&rig, &a, "two.rs").is_empty(), "{backend:?}");
        let b_touch = &rig
            .state
            .events
            .edit_calls(&b.id, &["one.rs".into(), "two.rs".into()])
            .unwrap();
        assert_eq!(b_touch.len(), 1);
        assert_eq!(
            b_touch[0].paths,
            ["two.rs"],
            "{backend:?}: no leak of A's file"
        );
    }
}

#[tokio::test]
async fn a_rename_keeps_its_source_inside_one_worktree_and_drops_it_across_worktrees() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed).await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await;
        let b = rig.linked("wt-b", "feature/b").await;
        let patch = |from: &str, to: &str| {
            vec![
                "*** Begin Patch".to_owned(),
                format!("*** Update File: {from}"),
                format!("*** Move to: {to}"),
                "@@".to_owned(),
                "-base".to_owned(),
                "+moved".to_owned(),
                "*** End Patch".to_owned(),
            ]
        };
        let send = |use_id: &'static str, lines: Vec<String>| {
            let rig = &rig;
            let cwd = a.path.clone();
            async move {
                let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
                rig.x_patch("cx", "t1", use_id, &cwd, &refs, crate::rig::End::Ok)
                    .await;
            }
        };
        // Within A: the old path is kept, relative to A's root.
        send(
            "u1",
            patch(&s(&a.path.join("old.rs")), &s(&a.path.join("sub/new.rs"))),
        )
        .await;
        let rec = &rig.state.events.edit_records(&a.id, "sub/new.rs").unwrap()[0];
        assert_eq!(
            rec.op.renamed_from.as_deref(),
            Some("old.rs"),
            "{backend:?}"
        );

        // From A's file to B's: no single file history, so no source. The
        // destination's hunks describe A's OLD file, so they are never kept as
        // exact for B; the removal is recorded in A as a Delete.
        send(
            "u2",
            patch(&s(&a.path.join("old2.rs")), &s(&b.path.join("new2.rs"))),
        )
        .await;
        let rec = &rig.state.events.edit_records(&b.id, "new2.rs").unwrap()[0];
        assert_eq!(rec.op.renamed_from, None, "{backend:?}");
        assert_eq!(
            rec.op.observed,
            Observed::Unobserved(why_lib::provenance::UnobservedReason::Unparsed),
            "{backend:?}"
        );
        assert!(rec.op.hunks.is_empty(), "{backend:?}");
        let gone = &rig.state.events.edit_records(&a.id, "old2.rs").unwrap();
        assert_eq!(gone.len(), 1, "{backend:?}");
        assert_eq!(gone[0].op.kind, EditKind::Delete);
        assert_eq!(gone[0].op.observed, Observed::Exact);
        assert_eq!(gone[0].op_index, 1, "appended after the event's own op");

        // From outside every worktree: nothing to delete, still never replayed.
        let outside = tempfile::tempdir().unwrap();
        send(
            "u3",
            patch(
                &s(&outside.path().join("o.rs")),
                &s(&b.path.join("new3.rs")),
            ),
        )
        .await;
        let rec = &rig.state.events.edit_records(&b.id, "new3.rs").unwrap()[0];
        assert!(rec.op.hunks.is_empty(), "{backend:?}");
    }
}

#[tokio::test]
async fn a_reduced_claude_edit_is_stored_as_an_unobserved_op_under_the_files_worktree() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed).await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await;
        let b = rig.linked("wt-b", "feature/b").await;
        let file = s(&b.path.join("big.rs"));
        // What the hook sends for a Write over 1 MiB: ids, the marker, the path.
        let (_, outcome) = rig
            .send_outcome(
                Harness::ClaudeCode,
                "PostToolUse",
                &a.path,
                json!({"session_id": "s1", "hook_event_name": "PostToolUse", "prompt_id": "p1",
                       "tool_name": "Write", "tool_use_id": "t1", "cwd": s(&a.path),
                       "tool_input": {"file_path": file},
                       "truncated": true, "original_bytes": 2_000_000u64}),
            )
            .await;
        assert_eq!(outcome.inserted, 1);
        let recs = rig.state.events.edit_records(&b.id, "big.rs").unwrap();
        assert_eq!(recs.len(), 1, "{backend:?}");
        assert_eq!(
            recs[0].op.observed,
            Observed::Unobserved(why_lib::provenance::UnobservedReason::PayloadReduced)
        );
        assert!(recs[0].op.hunks.is_empty() && recs[0].op.before.is_none());
        assert_eq!(sessions(&rig, &b), ["claude-code:s1"], "{backend:?}");
    }
}

#[tokio::test]
async fn the_shared_files_list_never_names_another_worktrees_file_by_a_bare_path() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, |repo: &TestRepo| {
            repo.write("src/lib.rs", "base\n");
            repo.commit_all("base");
        })
        .await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await;
        let b = rig.linked("wt-b", "feature/b").await;
        // A session of A edits B's `src/lib.rs` (A ALSO has a `src/lib.rs`),
        // then its own.
        let in_b = s(&b.path.join("src/lib.rs"));
        edit(&rig, "s1", "t1", &a.path, &in_b).await;
        edit(&rig, "s1", "t2", &a.path, &s(&a.path.join("src/lib.rs"))).await;

        let timeline = rig.state.events.timeline(&claude("s1")).unwrap().unwrap();
        let files: Vec<&str> = timeline
            .turns
            .iter()
            .flat_map(|t| &t.events)
            .flat_map(|e| e.files.iter().map(String::as_str))
            .collect();
        // B's file stays absolute (it cannot be mistaken for A's) ...
        assert!(files.contains(&in_b.as_str()), "{backend:?}: {files:?}");
        // ... and the only bare `src/lib.rs` is the one that really is A's.
        let bare = files.iter().filter(|f| **f == "src/lib.rs").count();
        assert_eq!(bare, 1, "{backend:?}: one row for A's own edit: {files:?}");
        // Attribution still goes through the owner.
        assert_eq!(touches(&rig, &b, "src/lib.rs"), ["s1#p1"], "{backend:?}");
        assert_eq!(touches(&rig, &a, "src/lib.rs"), ["s1#p1"], "{backend:?}");
        let b_calls = rig
            .state
            .events
            .edit_calls(&b.id, &["src/lib.rs".to_owned()])
            .unwrap();
        assert_eq!(
            b_calls.len(),
            1,
            "{backend:?}: only the call that edited B's file"
        );
        assert_eq!(b_calls[0].tool_use_id.as_deref(), Some("t1"));
    }
}

#[tokio::test]
async fn a_nested_repository_created_after_a_warm_lookup_is_not_the_outer_worktrees() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed).await;
        let a = rig.worktree_at(&rig.repo.path.clone()).await;
        let nested = a.path.join("vendor/lib");
        write_in(&a.path, "vendor/lib/x.rs", b"a\nb\n");

        // Warm: while `vendor/lib` is a plain directory, it is A's.
        edit(&rig, "s1", "t1", &a.path, &s(&nested.join("x.rs"))).await;
        assert_eq!(records(&rig, &a, "vendor/lib/x.rs"), 1, "{backend:?}");

        // It becomes a repository of its own; the next edit there is not A's.
        git(&nested, ["init", "-q"]);
        write_in(&a.path, "vendor/lib/y.rs", b"a\nb\n");
        edit(&rig, "s1", "t2", &a.path, &s(&nested.join("y.rs"))).await;
        assert_eq!(records(&rig, &a, "vendor/lib/y.rs"), 0, "{backend:?}");
        assert!(
            touches(&rig, &a, "vendor/lib/y.rs").is_empty(),
            "{backend:?}"
        );
    }
}
