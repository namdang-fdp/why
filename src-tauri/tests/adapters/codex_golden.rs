//! Golden tests: every recorded Codex scenario, replayed through the adapter
//! and the timeline builder.
//!
//! Two layers, deliberately:
//! 1. a **snapshot** per scenario (`codex_golden/<scenario>.txt`): the whole
//!    timeline rendered as text, so any behaviour change shows up as a diff
//!    the owner can read;
//! 2. **explicit assertions** below for the numbers that matter (turns, files,
//!    outcomes), so a snapshot that was re-blessed with a bug still fails.
//!
//! Regenerate snapshots with
//! `cargo run --manifest-path src-tauri/Cargo.toml --example update_codex_golden`
//! and review the resulting diff.

use why_lib::contracts::{
    SessionStatus, SessionTimeline, TimelineEventKind as K, ToolOutcome as O,
};

use crate::codex_common::{self, scenarios};
use crate::codex_snapshot::{golden_path, snapshot};

#[test]
fn every_codex_scenario_matches_its_snapshot() {
    let all = scenarios();
    assert!(all.len() >= 14, "only {} Codex scenarios found", all.len());
    for (name, hooks) in all {
        let actual = snapshot(&hooks);
        let path = golden_path(&name);
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("cannot read snapshot for {name}: {error}; see update_codex_golden\n{actual}")
        });
        assert_eq!(
            actual, expected,
            "snapshot for {name} differs; see update_codex_golden"
        );
    }
}

fn only(name: &str) -> SessionTimeline {
    let (_, hooks) = scenarios()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no scenario {name}"));
    let mut t = codex_common::timelines(&codex_common::load(&hooks));
    assert_eq!(t.len(), 1, "{name}: one session expected");
    t.remove(0)
}

fn kinds(tl: &SessionTimeline, turn: usize) -> Vec<(K, Option<O>)> {
    tl.turns[turn]
        .events
        .iter()
        .map(|e| (e.kind, e.outcome))
        .collect()
}

#[test]
fn basic_main_two_turns_resume_is_one_session() {
    let tl = only("basic-main");
    assert_eq!(tl.session.model.as_deref(), Some("gpt-6-luna"));
    assert_eq!(tl.session.status, SessionStatus::Ended);
    assert_eq!(tl.turns.len(), 2, "resume keeps the session, adds a turn");

    let t0 = &tl.turns[0];
    let edits: Vec<_> = t0.events.iter().filter(|e| e.kind == K::Edit).collect();
    assert_eq!(edits.len(), 3, "add, update, delete");
    assert!(edits.iter().all(|e| e.outcome == Some(O::Ok)));
    let files: Vec<&str> = edits
        .iter()
        .flat_map(|e| e.files.iter().map(String::as_str))
        .collect();
    assert_eq!(
        files,
        [
            "/TMP/fixture-repo/src/notes.py",
            "/TMP/fixture-repo/src/calc.py",
            "/TMP/fixture-repo/src/sub.py"
        ]
    );
    assert!(
        edits[0]
            .summary
            .starts_with("A /TMP/fixture-repo/src/notes.py (+1)")
    );
    assert!(edits[2].summary.starts_with("D "));

    // Shell: no exit code in the hook, so never Ok/Failed.
    let shells: Vec<_> = t0
        .events
        .iter()
        .filter(|e| matches!(e.kind, K::Shell | K::TestRun))
        .collect();
    assert_eq!(shells.len(), 4);
    assert!(shells.iter().all(|e| e.outcome == Some(O::Unknown)));
    assert_eq!(shells.iter().filter(|e| e.kind == K::TestRun).count(), 1);

    // The closing message is Declared, the turn has ended.
    assert!(t0.ended_at.is_some());
    assert!(t0.events.iter().any(|e| e.kind == K::AssistantMessage));
    assert_eq!(
        tl.turns[1]
            .events
            .iter()
            .filter(|e| e.kind == K::TestRun)
            .count(),
        1
    );
}

#[test]
fn rejected_patch_is_unknown_and_shell_edit_is_just_a_shell_call() {
    let tl = only("failing-patch-and-shell-edit");
    assert_eq!(tl.turns.len(), 1);
    let edit = tl.turns[0]
        .events
        .iter()
        .find(|e| e.kind == K::Edit)
        .unwrap();
    assert_eq!(edit.outcome, Some(O::Unknown), "Pre without Post");
    assert_eq!(edit.files, ["/TMP/fixture-repo/src/calc.py"]);
    // The file created by `printf > file` is invisible to hooks: no files.
    let shell = tl.turns[0]
        .events
        .iter()
        .find(|e| e.kind == K::Shell)
        .unwrap();
    assert!(shell.files.is_empty());
}

#[test]
fn subagent_events_carry_agent_id_and_keep_their_own_turn() {
    let tl = only("subagent");
    // FINDING: Codex gives the subagent its own turn_id, so the timeline
    // (which groups by turn_id) shows it as a second turn with no prompt.
    // Folding it under the parent turn needs the parent link, which no hook
    // payload carries; see the WHY-15 report.
    assert_eq!(tl.turns.len(), 2);
    let sub_turn = &tl.turns[1];
    assert!(sub_turn.prompt.is_none());
    assert!(sub_turn.events.iter().all(|e| e.agent_id.is_some()));
    let edit = sub_turn.events.iter().find(|e| e.kind == K::Edit).unwrap();
    assert_eq!(edit.outcome, Some(O::Ok));
    assert_eq!(edit.files, ["/TMP/fixture-repo/src/cxsub.py"]);
    assert!(
        sub_turn
            .events
            .iter()
            .any(|e| e.kind == K::Subagent && e.summary.contains("Created `src/cxsub.py`"))
    );

    let parent = &tl.turns[0];
    assert!(parent.events.iter().all(|e| e.agent_id.is_none()));
    let spawn = parent
        .events
        .iter()
        .find(|e| e.kind == K::Subagent)
        .unwrap();
    assert_eq!(spawn.summary, "create_file: (encrypted)");
    assert!(
        !format!("{tl:?}").contains("gAAAA"),
        "the blob never reaches the timeline"
    );
}

#[test]
fn subagent_stop_records_the_child_rollout() {
    let hooks = codex_common::codex_root().join("subagent/hooks.jsonl");
    let mut env = codex_common::load(&hooks)
        .into_iter()
        .find(|env| env.event == "SubagentStop")
        .unwrap();
    let parent = env.payload["transcript_path"].as_str().unwrap();
    let child = env.payload["agent_transcript_path"].as_str().unwrap();
    assert_ne!(child, parent);
    let events = why_lib::adapters::normalize(&env);
    assert_eq!(events[0].transcript_path.as_deref(), Some(child));

    env.payload
        .as_object_mut()
        .unwrap()
        .remove("agent_transcript_path");
    let events = why_lib::adapters::normalize(&env);
    assert_eq!(events[0].transcript_path, None);
}

#[test]
fn worktree_relative_patch_path_is_joined_to_cwd() {
    let tl = only("worktree");
    let edit = tl.turns[0]
        .events
        .iter()
        .find(|e| e.kind == K::Edit)
        .unwrap();
    assert_eq!(edit.outcome, Some(O::Ok));
    assert_eq!(edit.files.len(), 1);
    assert!(edit.files[0].ends_with("/src/cx_wt.py"), "{:?}", edit.files);
    assert!(edit.files[0].starts_with('/'), "{:?}", edit.files);
}

#[test]
fn subdir_session_is_one_turn_of_shell() {
    let tl = only("subdir");
    assert_eq!(tl.turns.len(), 1);
    assert_eq!(
        kinds(&tl, 0).iter().filter(|(k, _)| *k == K::Shell).count(),
        1
    );
    assert!(tl.session.cwd.ends_with("fixture-repo/src") || tl.session.cwd.ends_with("src"));
}

#[test]
fn model_request_events_never_appear() {
    for (name, hooks) in scenarios() {
        for env in codex_common::load(&hooks) {
            if env.event.ends_with("ModelRequest") {
                assert!(why_lib::adapters::normalize(&env).is_empty(), "{name}");
            }
        }
    }
}

#[test]
fn replay_is_deterministic_and_keys_are_unique() {
    for (name, hooks) in scenarios() {
        let envs = codex_common::load(&hooks);
        let a = codex_common::replay(&envs);
        let b = codex_common::replay(&envs);
        assert_eq!(a, b, "{name}");
        let mut keys: Vec<_> = a.values().flatten().map(|e| e.key.clone()).collect();
        let n = keys.len();
        keys.sort();
        keys.dedup();
        // Identical payloads in the same millisecond may merge (documented in
        // EventKey); the fixtures must not hit that.
        assert_eq!(keys.len(), n, "{name}: duplicate event keys");
    }
}
