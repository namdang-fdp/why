//! End to end: real adapters -> real git resolver -> ingest -> event store ->
//! `get_worktree_attribution` / `get_file_activity`, on throw-away repositories,
//! once per store backend.

use std::os::unix::fs::symlink;

use serde_json::json;
use why_lib::contracts::{
    AttributionStatus, DiffMode, EvidenceLabel, IpcError, SessionId, ToolOutcome,
};
use why_lib::ipc::attribution::{get_file_activity_impl, get_worktree_attribution_impl};
use why_lib::ipc::sessions::get_session_timeline_impl;
use why_wire::Harness;

use crate::common::{git, write_in};
use crate::rig::{BACKENDS, Backend, End, Rig, change, file, label, labels, s, table};

fn seed(files: &'static [&'static str]) -> impl FnOnce(&crate::common::TestRepo) {
    move |repo| {
        for f in files {
            repo.write(f, "base\n");
        }
        repo.commit_all("base files");
    }
}

async fn attribution(rig: &Rig, wt: &crate::rig::Wt) -> why_lib::contracts::WorktreeAttribution {
    get_worktree_attribution_impl(&rig.state, &wt.id, DiffMode::VsBase)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// The acceptance scenario
// ---------------------------------------------------------------------------

/// Two agents (a Claude Code session and a Codex session) in two worktrees of
/// the same repository, both touching `shared.rs`, plus one file changed with
/// no event at all. Nothing from one worktree may show up in the other.
#[tokio::test]
async fn acceptance_two_agents_two_worktrees_and_one_unexplained_file() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(
            backend,
            seed(&["shared.rs", "only_a.rs", "only_b.rs", "mystery.rs"]),
        )
        .await;
        let a = rig.linked("wt-a", "feature/a").await;
        let b = rig.linked("wt-b", "feature/b").await;

        // What git sees.
        for f in ["shared.rs", "only_a.rs", "mystery.rs"] {
            change(&a, f);
        }
        for f in ["shared.rs", "only_b.rs"] {
            change(&b, f);
        }

        // Claude Code in worktree A: two turns. `mystery.rs` is never edited.
        rig.c_prompt("claude-a", "p1", &a.path, "update shared")
            .await;
        rig.c_edit(
            "claude-a",
            "p1",
            "tu1",
            &a.path,
            &s(&a.path.join("shared.rs")),
            End::Ok,
        )
        .await;
        rig.c_prompt("claude-a", "p2", &a.path, "update only_a")
            .await;
        rig.c_edit(
            "claude-a",
            "p2",
            "tu2",
            &a.path,
            &s(&a.path.join("only_a.rs")),
            End::Ok,
        )
        .await;
        // The same agent also reaches into worktree B's file by absolute path.
        // The event belongs to worktree A (its cwd), and the file lies outside
        // A's root: it must explain nothing, in A or in B.
        rig.c_edit(
            "claude-a",
            "p2",
            "tu3",
            &a.path,
            &s(&b.path.join("only_b.rs")),
            End::Ok,
        )
        .await;

        // Codex in worktree B: relative patch paths, two turns.
        rig.x_update("codex-b", "t1", "call1", &b.path, "shared.rs", End::Ok)
            .await;
        rig.x_update("codex-b", "t2", "call2", &b.path, "only_b.rs", End::Ok)
            .await;

        let in_a = attribution(&rig, &a).await;
        let in_b = attribution(&rig, &b).await;
        println!("{}", table("worktree A (Claude Code)", &in_a));
        println!("{}", table("worktree B (Codex)", &in_b));

        // --- worktree A
        assert_eq!(
            in_a.files
                .iter()
                .map(|f| f.change.path.as_str())
                .collect::<Vec<_>>(),
            ["mystery.rs", "only_a.rs", "shared.rs"]
        );
        let shared_a = file(&in_a, "shared.rs");
        assert_eq!(shared_a.status, AttributionStatus::Explained);
        assert_eq!(shared_a.evidence, EvidenceLabel::Observed);
        assert_eq!(
            labels(&shared_a.explained_by),
            ["claude-code:claude-a#0(p1)"]
        );
        assert_eq!(
            labels(&file(&in_a, "only_a.rs").explained_by),
            ["claude-code:claude-a#1(p2)"]
        );
        let mystery = file(&in_a, "mystery.rs");
        assert_eq!(mystery.status, AttributionStatus::Unexplained);
        assert_eq!(mystery.evidence, EvidenceLabel::Unknown);
        assert!(mystery.explained_by.is_empty() && mystery.attempts.is_empty());

        // --- worktree B: only Codex, never Claude's reach-across
        assert_eq!(
            in_b.files
                .iter()
                .map(|f| f.change.path.as_str())
                .collect::<Vec<_>>(),
            ["only_b.rs", "shared.rs"]
        );
        assert_eq!(
            labels(&file(&in_b, "shared.rs").explained_by),
            ["codex:codex-b#0(t1)"]
        );
        assert_eq!(
            labels(&file(&in_b, "only_b.rs").explained_by),
            ["codex:codex-b#1(t2)"]
        );
        assert!(in_a.shell_leads.is_empty() && in_b.shell_leads.is_empty());

        // The turn index in a reference is the turn's index in the timeline.
        for (wt_attr, sid) in [(&in_a, "claude-code:claude-a"), (&in_b, "codex:codex-b")] {
            let timeline = get_session_timeline_impl(&rig.state, &SessionId(sid.into()))
                .await
                .unwrap();
            for r in wt_attr.files.iter().flat_map(|f| &f.explained_by) {
                assert_eq!(
                    timeline.turns[r.turn_index as usize].id,
                    r.turn_id,
                    "{}",
                    label(r)
                );
            }
        }

        // `get_file_activity` answers the same question for one path, per worktree.
        let act_a = get_file_activity_impl(&rig.state, &a.id, "shared.rs")
            .await
            .unwrap();
        let act_b = get_file_activity_impl(&rig.state, &b.id, "shared.rs")
            .await
            .unwrap();
        assert_eq!(labels(&act_a), ["claude-code:claude-a#0(p1)"]);
        assert_eq!(labels(&act_b), ["codex:codex-b#0(t1)"]);
    }
}

// ---------------------------------------------------------------------------
// Attempts, shell commands, git as the source of truth
// ---------------------------------------------------------------------------

/// A rejected edit and a Pre without Post on a file git shows as changed: the
/// file stays `unexplained`, with both listed as attempts.
#[tokio::test]
async fn a_failed_and_an_unconfirmed_edit_are_attempts_not_explanations() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(backend, seed(&["x.rs"])).await;
        let wt = rig.linked("wt", "feature/x").await;
        change(&wt, "x.rs");

        rig.c_prompt("c1", "p1", &wt.path, "edit x").await;
        let x = s(&wt.path.join("x.rs"));
        rig.c_edit("c1", "p1", "fails", &wt.path, &x, End::Failed)
            .await;
        rig.c_edit("c1", "p1", "lost", &wt.path, &x, End::Lost)
            .await;

        let attr = attribution(&rig, &wt).await;
        println!("{}", table("failed + unconfirmed", &attr));
        let f = file(&attr, "x.rs");
        assert_eq!(f.status, AttributionStatus::Unexplained);
        assert!(f.explained_by.is_empty());
        let mut outcomes: Vec<_> = f.attempts.iter().map(|r| r.outcome).collect();
        outcomes.sort_by_key(|o| format!("{o:?}"));
        assert_eq!(outcomes, [ToolOutcome::Failed, ToolOutcome::Unknown]);

        // The file-activity view shows every outcome, newest first (the
        // unconfirmed one was sent after the failed one).
        let act = get_file_activity_impl(&rig.state, &wt.id, "x.rs")
            .await
            .unwrap();
        assert_eq!(
            act.iter().map(|r| r.outcome).collect::<Vec<_>>(),
            [ToolOutcome::Unknown, ToolOutcome::Failed]
        );
    }
}

/// A later successful edit of the same file explains it even after an earlier
/// failed attempt; the failed one stays visible as an attempt.
#[tokio::test]
async fn a_retry_that_succeeded_explains_the_file_and_keeps_the_failed_attempt() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(backend, seed(&["x.rs"])).await;
        let wt = rig.linked("wt", "feature/x").await;
        change(&wt, "x.rs");
        let x = s(&wt.path.join("x.rs"));
        rig.c_edit("c1", "p1", "first", &wt.path, &x, End::Failed)
            .await;
        rig.c_edit("c1", "p1", "second", &wt.path, &x, End::Ok)
            .await;

        let attr = attribution(&rig, &wt).await;
        let f = file(&attr, "x.rs");
        assert_eq!(f.status, AttributionStatus::Explained);
        assert_eq!(f.explained_by.len(), 1);
        assert_eq!(f.explained_by[0].tool_use_id.as_deref(), Some("second"));
        assert_eq!(f.attempts.len(), 1);
        assert_eq!(f.attempts[0].outcome, ToolOutcome::Failed);
    }
}

/// A `sed -i` in a shell command changed the file: no edit event names it, so it
/// is `unexplained`, and the turn is a shell LEAD, never a toucher of the file.
#[tokio::test]
async fn a_shell_only_change_is_unexplained_and_the_turn_is_a_lead() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(backend, seed(&["x.rs", "y.rs"])).await;
        let wt = rig.linked("wt", "feature/x").await;
        change(&wt, "x.rs");
        change(&wt, "y.rs");

        rig.c_prompt("c1", "p1", &wt.path, "rename things").await;
        rig.c_bash("c1", "p1", "sh1", &wt.path, "sed -i s/a/b/ x.rs")
            .await;
        rig.c_bash("c1", "p1", "sh2", &wt.path, "cargo fmt").await;
        // A Codex turn with one command, and an edit to y.rs in the same session.
        rig.x_bash("x1", "t1", "c1", &wt.path, "ls").await;
        rig.x_update("x1", "t2", "c2", &wt.path, "y.rs", End::Ok)
            .await;

        let attr = attribution(&rig, &wt).await;
        println!("{}", table("shell only", &attr));
        let x = file(&attr, "x.rs");
        assert_eq!(x.status, AttributionStatus::Unexplained);
        assert!(x.explained_by.is_empty() && x.attempts.is_empty(), "{x:?}");
        assert_eq!(file(&attr, "y.rs").status, AttributionStatus::Explained);

        // Newest first: the Codex turn, then the Claude turn with two commands.
        let leads: Vec<_> = attr
            .shell_leads
            .iter()
            .map(|l| (l.session_id.0.as_str(), l.turn_id.as_str(), l.command_count))
            .collect();
        assert_eq!(leads, [("codex:x1", "t1", 1), ("claude-code:c1", "p1", 2)]);
    }
}

/// The agent edited a file and then reverted it: git shows no change, so the
/// file is not listed at all (git is the source of truth for "changed").
#[tokio::test]
async fn an_edit_that_was_reverted_is_not_listed() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(backend, seed(&["kept.rs", "reverted.rs"])).await;
        let wt = rig.linked("wt", "feature/x").await;
        change(&wt, "kept.rs");
        write_in(&wt.path, "reverted.rs", b"base\n"); // back to the committed content
        rig.c_edit(
            "c1",
            "p1",
            "e1",
            &wt.path,
            &s(&wt.path.join("kept.rs")),
            End::Ok,
        )
        .await;
        rig.c_edit(
            "c1",
            "p1",
            "e2",
            &wt.path,
            &s(&wt.path.join("reverted.rs")),
            End::Ok,
        )
        .await;

        let attr = attribution(&rig, &wt).await;
        assert_eq!(
            attr.files
                .iter()
                .map(|f| f.change.path.as_str())
                .collect::<Vec<_>>(),
            ["kept.rs"]
        );
        // The activity view is about edits, not about git: it still knows.
        let act = get_file_activity_impl(&rig.state, &wt.id, "reverted.rs")
            .await
            .unwrap();
        assert_eq!(act.len(), 1);
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Subdirectory cwd, a Codex patch path relative to that subdirectory, names
/// with spaces and non-ASCII characters, and `./a/../b.rs`-style spellings all
/// land on the one canonical git path.
#[tokio::test]
async fn path_spellings_all_meet_git_s_spelling() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(
            backend,
            seed(&[
                "sub/x.rs",
                "sub/dir with space/né 世界.rs",
                "b.rs",
                "other/c.rs",
            ]),
        )
        .await;
        let wt = rig.linked("wt", "feature/x").await;
        for f in [
            "sub/x.rs",
            "sub/dir with space/né 世界.rs",
            "b.rs",
            "other/c.rs",
        ] {
            change(&wt, f);
        }
        let sub = wt.path.join("sub");

        // Codex, cwd = the subdirectory, patch path relative to it.
        rig.x_update("x1", "t1", "c1", &sub, "x.rs", End::Ok).await;
        // Claude, cwd = the subdirectory, absolute path with spaces and unicode.
        rig.c_edit(
            "c1",
            "p1",
            "e1",
            &sub,
            &s(&wt.path.join("sub/dir with space/né 世界.rs")),
            End::Ok,
        )
        .await;
        // `.` and `..` in the middle of an absolute path.
        rig.c_edit(
            "c1",
            "p2",
            "e2",
            &wt.path,
            &format!("{}/./a/../b.rs", s(&wt.path)),
            End::Ok,
        )
        .await;
        // Codex relative path that climbs out of the subdirectory and back in.
        rig.x_update("x1", "t2", "c2", &sub, "../other/c.rs", End::Ok)
            .await;

        let attr = attribution(&rig, &wt).await;
        println!("{}", table("path spellings", &attr));
        for path in [
            "sub/x.rs",
            "sub/dir with space/né 世界.rs",
            "b.rs",
            "other/c.rs",
        ] {
            assert_eq!(
                file(&attr, path).status,
                AttributionStatus::Explained,
                "{path}"
            );
        }
    }
}

/// The agent's cwd is a symlink to the worktree. The paths it reports keep the
/// symlink's spelling; they must still be matched to the worktree's git paths.
#[tokio::test]
async fn a_symlinked_cwd_still_matches() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(backend, seed(&["x.rs", "sub/y.rs"])).await;
        let wt = rig.linked("wt", "feature/x").await;
        change(&wt, "x.rs");
        change(&wt, "sub/y.rs");
        let link = rig.repo.base().join("shortcut");
        symlink(&wt.path, &link).unwrap();

        rig.c_edit("c1", "p1", "e1", &link, &s(&link.join("x.rs")), End::Ok)
            .await;
        // Codex: relative patch path, cwd = a subdirectory reached through the link.
        rig.x_update("x1", "t1", "c1", &link.join("sub"), "y.rs", End::Ok)
            .await;

        let attr = attribution(&rig, &wt).await;
        println!("{}", table("symlinked cwd", &attr));
        assert_eq!(file(&attr, "x.rs").status, AttributionStatus::Explained);
        assert_eq!(file(&attr, "sub/y.rs").status, AttributionStatus::Explained);
    }
}

#[tokio::test]
async fn a_symlinked_parent_matches_the_target_directory_without_following_the_leaf() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed(&["src/a.rs", "leaf", "other.rs"])).await;
        let wt = rig.linked("wt", "feature/x").await;
        symlink("src", wt.path.join("link")).unwrap();
        std::fs::remove_file(wt.path.join("leaf")).unwrap();
        symlink("other.rs", wt.path.join("leaf")).unwrap();
        change(&wt, "src/a.rs");
        change(&wt, "other.rs");

        rig.x_update("x1", "t1", "e1", &wt.path, "link/a.rs", End::Ok)
            .await;
        rig.x_update("x1", "t1", "e2", &wt.path, "leaf", End::Ok)
            .await;

        let attr = attribution(&rig, &wt).await;
        assert_eq!(file(&attr, "src/a.rs").status, AttributionStatus::Explained);
        assert_eq!(file(&attr, "leaf").status, AttributionStatus::Explained);
        assert_eq!(
            file(&attr, "other.rs").status,
            AttributionStatus::Unexplained
        );
    }
}

#[tokio::test]
async fn a_rewritten_claude_edit_explains_the_file_named_in_its_response() {
    for backend in BACKENDS {
        let rig = Rig::new(backend, seed(&["requested.rs", "executed.rs"])).await;
        let wt = rig.linked("wt", "feature/x").await;
        change(&wt, "requested.rs");
        change(&wt, "executed.rs");

        let mut payload = json!({
            "session_id": "c1", "hook_event_name": "PreToolUse", "prompt_id": "p1",
            "tool_name": "Edit", "tool_use_id": "e1",
            "tool_input": {"file_path": wt.path.join("requested.rs")}
        });
        rig.send(Harness::ClaudeCode, "PreToolUse", &wt.path, payload.clone())
            .await;
        payload["hook_event_name"] = json!("PostToolUse");
        payload["tool_response"] = json!({"filePath": wt.path.join("executed.rs")});
        rig.send(Harness::ClaudeCode, "PostToolUse", &wt.path, payload)
            .await;

        let attr = attribution(&rig, &wt).await;
        assert_eq!(
            file(&attr, "requested.rs").status,
            AttributionStatus::Unexplained
        );
        assert_eq!(
            file(&attr, "executed.rs").status,
            AttributionStatus::Explained
        );
    }
}

/// A file outside the worktree root is kept in the event but never matched, not
/// even to a changed file with the same name inside the worktree.
#[tokio::test]
async fn a_file_outside_the_root_is_never_matched() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(backend, seed(&["x.rs"])).await;
        let wt = rig.linked("wt", "feature/x").await;
        change(&wt, "x.rs");
        let elsewhere = rig.repo.base().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        // Absolute paths outside the root: a sibling directory, a look-alike
        // prefix of the worktree path, and `..` out of the root.
        for (i, file) in [
            s(&elsewhere.join("x.rs")),
            format!("{}-other/x.rs", s(&wt.path)),
            format!("{}/../x.rs", s(&wt.path)),
        ]
        .iter()
        .enumerate()
        {
            rig.c_edit("c1", "p1", &format!("e{i}"), &wt.path, file, End::Ok)
                .await;
        }

        let attr = attribution(&rig, &wt).await;
        assert_eq!(file(&attr, "x.rs").status, AttributionStatus::Unexplained);
        assert!(
            get_file_activity_impl(&rig.state, &wt.id, "x.rs")
                .await
                .unwrap()
                .is_empty()
        );
    }
}

/// A rename in git, with the agent having edited the old name, the new name, or
/// both: an edit of either name explains the renamed file.
#[tokio::test]
async fn a_rename_matches_edits_of_the_old_and_the_new_name() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(backend, |repo| {
            repo.write(
                "old.rs",
                "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\n",
            );
            repo.commit_all("base");
        })
        .await;
        let wt = rig.linked("wt", "feature/x").await;
        git(&wt.path, ["mv", "old.rs", "new.rs"]);
        write_in(
            &wt.path,
            "new.rs",
            b"alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\nTHETA\n",
        );

        rig.c_edit(
            "c1",
            "p1",
            "before",
            &wt.path,
            &s(&wt.path.join("old.rs")),
            End::Ok,
        )
        .await;
        rig.c_edit(
            "c1",
            "p2",
            "after",
            &wt.path,
            &s(&wt.path.join("new.rs")),
            End::Ok,
        )
        .await;

        let attr = attribution(&rig, &wt).await;
        println!("{}", table("rename", &attr));
        let f = file(&attr, "new.rs");
        assert_eq!(f.change.old_path.as_deref(), Some("old.rs"));
        assert_eq!(f.status, AttributionStatus::Explained);
        let mut seen: Vec<_> = f
            .explained_by
            .iter()
            .map(|r| (r.tool_use_id.as_deref().unwrap(), r.path.as_str()))
            .collect();
        seen.sort();
        assert_eq!(seen, [("after", "new.rs"), ("before", "old.rs")]);

        // Only the old name edited: still explains the renamed file.
        let rig2 = Rig::new(backend, |repo| {
            repo.write(
                "old.rs",
                "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\n",
            );
            repo.commit_all("base");
        })
        .await;
        let wt2 = rig2.linked("wt", "feature/y").await;
        git(&wt2.path, ["mv", "old.rs", "new.rs"]);
        rig2.c_edit(
            "c1",
            "p1",
            "only-old",
            &wt2.path,
            &s(&wt2.path.join("old.rs")),
            End::Ok,
        )
        .await;
        let attr2 = attribution(&rig2, &wt2).await;
        assert_eq!(file(&attr2, "new.rs").status, AttributionStatus::Explained);
    }
}

// ---------------------------------------------------------------------------
// Deliveries and worktree membership
// ---------------------------------------------------------------------------

/// A redelivered envelope (re-sent from the spool) must not double-count.
#[tokio::test]
async fn redelivered_events_do_not_double_count() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(backend, seed(&["x.rs"])).await;
        let wt = rig.linked("wt", "feature/x").await;
        change(&wt, "x.rs");
        let x = s(&wt.path.join("x.rs"));
        let pre = serde_json::json!({"session_id": "c1", "hook_event_name": "PreToolUse",
            "prompt_id": "p1", "tool_name": "Edit", "tool_use_id": "e1",
            "tool_input": {"file_path": x}});
        let env = rig
            .send(why_wire::Harness::ClaudeCode, "PreToolUse", &wt.path, pre)
            .await;
        let mut post = serde_json::json!({"session_id": "c1", "hook_event_name": "PostToolUse",
            "prompt_id": "p1", "tool_name": "Edit", "tool_use_id": "e1",
            "tool_input": {"file_path": x}, "tool_response": {"filePath": x}});
        post["tool_response"] = serde_json::json!({"filePath": x});
        let post_env = rig
            .send(why_wire::Harness::ClaudeCode, "PostToolUse", &wt.path, post)
            .await;
        for _ in 0..3 {
            rig.deliver(&env).await;
            rig.deliver(&post_env).await;
        }

        let attr = attribution(&rig, &wt).await;
        assert_eq!(file(&attr, "x.rs").explained_by.len(), 1);
        assert!(file(&attr, "x.rs").attempts.is_empty());
    }
}

/// An event whose cwd is outside every registered repository is kept (owner
/// decision) but has no worktree, so it appears in no worktree's attribution.
#[tokio::test]
async fn an_event_without_a_worktree_appears_nowhere() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(backend, seed(&["x.rs"])).await;
        let wt = rig.linked("wt", "feature/x").await;
        change(&wt, "x.rs");
        let stray = rig.repo.base().join("not-a-repo");
        std::fs::create_dir_all(&stray).unwrap();
        // Same relative file name, edited in a directory no worktree owns.
        rig.c_edit("c1", "p1", "e1", &stray, &s(&stray.join("x.rs")), End::Ok)
            .await;

        let attr = attribution(&rig, &wt).await;
        assert_eq!(file(&attr, "x.rs").status, AttributionStatus::Unexplained);
        assert!(
            get_file_activity_impl(&rig.state, &wt.id, "x.rs")
                .await
                .unwrap()
                .is_empty()
        );
        // ... yet the event itself was stored.
        let stored = get_session_timeline_impl(&rig.state, &SessionId("claude-code:c1".into()))
            .await
            .unwrap();
        assert_eq!(stored.turns.len(), 1);
    }
}

/// Several sessions edit one worktree: all of them are listed, newest first.
#[tokio::test]
async fn several_sessions_in_one_worktree_are_all_listed() {
    for backend in BACKENDS {
        println!("--- backend {backend:?}");
        let rig = Rig::new(backend, seed(&["x.rs"])).await;
        let wt = rig.linked("wt", "feature/x").await;
        change(&wt, "x.rs");
        let x = s(&wt.path.join("x.rs"));
        rig.c_edit("c1", "p1", "e1", &wt.path, &x, End::Ok).await;
        rig.x_update("x1", "t1", "c1", &wt.path, "x.rs", End::Ok)
            .await;
        rig.c_edit("c2", "p1", "e1", &wt.path, &x, End::Ok).await; // same ids, other session

        let attr = attribution(&rig, &wt).await;
        assert_eq!(
            labels(&file(&attr, "x.rs").explained_by),
            [
                "claude-code:c2#0(p1)",
                "codex:x1#0(t1)",
                "claude-code:c1#0(p1)"
            ]
        );
    }
}

// ---------------------------------------------------------------------------
// The IPC edges
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_activity_rejects_paths_that_are_not_inside_the_worktree() {
    let rig = Rig::new(Backend::Memory, seed(&["x.rs"])).await;
    let wt = rig.linked("wt", "feature/x").await;
    for bad in ["", "..", "../x.rs", "/etc/passwd", "a/../../x"] {
        let err = get_file_activity_impl(&rig.state, &wt.id, bad)
            .await
            .unwrap_err();
        assert!(
            matches!(err, IpcError::InvalidInput { .. }),
            "{bad:?}: {err:?}"
        );
    }
    // A spelling that merely needs cleaning is fine.
    assert!(
        get_file_activity_impl(&rig.state, &wt.id, "./a/../x.rs")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn an_unknown_worktree_is_not_found_for_attribution_and_empty_for_activity() {
    let rig = Rig::new(Backend::Memory, seed(&["x.rs"])).await;
    let ghost = why_lib::contracts::WorktreeId("no-such".into());
    let err = get_worktree_attribution_impl(&rig.state, &ghost, DiffMode::VsBase)
        .await
        .unwrap_err();
    assert!(matches!(err, IpcError::NotFound), "{err:?}");
    assert!(
        get_file_activity_impl(&rig.state, &ghost, "x.rs")
            .await
            .unwrap()
            .is_empty()
    );
}
