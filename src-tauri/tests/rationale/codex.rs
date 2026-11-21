//! Codex: rationale from the WHY-45 `basic-main` rollout, and the shell-outcome
//! upgrade (`unknown` -> `ok`/`failed`) from the rollout's structured records.

use serde_json::json;
use why_lib::contracts::{
    BlockPosition, EvidenceLabel, RationaleResult, ShellOutcome, ShellOutcomes,
    ShellOutcomesResult, ToolOutcome, TranscriptSource, TranscriptUnavailable,
};
use why_lib::events::{EXCERPT_MAX_BYTES, EventKind, NormalizedEvent, ToolClass};
use why_lib::ipc::rationale::{
    get_shell_outcomes_impl, get_tool_rationale_impl, get_turn_rationale_impl,
};
use why_wire::Harness;

use crate::common::*;

const SESSION: &str = "01a1f812-8d57-7393-87df-7cfc4fefbfb3";
const UNITTEST_NO_TESTS: &str = "exec-66426901-f66d-46f0-9d72-b07658d67571"; // exit 5
const SH_EXIT_3: &str = "exec-e6fcfc9d-ad86-446c-b9c7-225e0dda6701";
const UNITTEST_FAIL: &str = "exec-188bdd44-6624-44f7-8c8a-d5f52c7117c8"; // turn 2, exit 1

/// The fixture's events with every `transcript_path` pointing at a temp copy of
/// its rollout (optionally with the CLI version rewritten).
pub fn fixture_events(rig: &Rig, version: Option<&str>) -> Vec<NormalizedEvent> {
    let dir = codex_fixture("basic-main");
    let mut bytes = std::fs::read_to_string(dir.join("transcript.jsonl")).unwrap();
    if let Some(version) = version {
        bytes = bytes.replace(
            "\"cli_version\":\"0.170.0\"",
            &format!("\"cli_version\":\"{version}\""),
        );
    }
    let file = rig.codex_file("rollout-basic.jsonl", bytes.as_bytes());
    let mut events = replay(&dir.join("hooks.jsonl"));
    rewrite_paths(&mut events, |_| Some(file.clone()));
    events
}

fn turn_ids(events: &[NormalizedEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.kind == EventKind::Prompt)
        .filter_map(|e| e.turn_id.clone())
        .collect()
}

fn ok_outcomes(result: ShellOutcomesResult) -> ShellOutcomes {
    match result {
        ShellOutcomesResult::Ok { result } => result,
        other => panic!("expected outcomes, got {other:?}"),
    }
}

#[tokio::test]
async fn turn_rationale_is_the_agents_messages_for_that_turn() {
    let rig = Rig::new();
    let events = fixture_events(&rig, None);
    let turns = turn_ids(&events);
    let sid = session_id(Harness::Codex, SESSION);
    let result = on_both(&events, |state| {
        let (roots, sid, turn) = (rig.roots.clone(), sid.clone(), turns[0].clone());
        async move {
            get_turn_rationale_impl(&state, &roots, &sid, &turn)
                .await
                .unwrap()
        }
    })
    .await;
    let RationaleResult::Ok { rationale } = &result else {
        panic!("{result:?}");
    };
    assert_eq!(rationale.evidence, EvidenceLabel::Declared);
    assert_eq!(rationale.source, TranscriptSource::CodexRollout);
    assert_eq!(rationale.harness_version.as_deref(), Some("0.170.0"));
    assert!(rationale.version_supported);
    assert!(!rationale.truncated);
    assert!(rationale.blocks.len() >= 3);
    assert!(
        rationale
            .blocks
            .iter()
            .all(|b| b.position == BlockPosition::Turn)
    );
    println!(
        "SAMPLE codex get_turn_rationale:\n{}",
        serde_json::to_string_pretty(&result).unwrap()
    );

    // Turn 2 has its own single message and none of turn 1's.
    let second = on_both(&events, |state| {
        let (roots, sid, turn) = (rig.roots.clone(), sid.clone(), turns[1].clone());
        async move {
            get_turn_rationale_impl(&state, &roots, &sid, &turn)
                .await
                .unwrap()
        }
    })
    .await;
    assert_eq!(texts(&second).len(), 1);
}

#[tokio::test]
async fn tool_rationale_has_what_was_said_before_and_after_the_call() {
    let rig = Rig::new();
    let events = fixture_events(&rig, None);
    let sid = session_id(Harness::Codex, SESSION);
    let event_id = tool_event_id(&events, UNITTEST_NO_TESTS);
    let result = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move {
            get_tool_rationale_impl(&state, &roots, &sid, event_id)
                .await
                .unwrap()
        }
    })
    .await;
    let RationaleResult::Ok { rationale } = result else {
        panic!("expected text");
    };
    assert_eq!(rationale.blocks.len(), 2);
    assert_eq!(rationale.blocks[0].position, BlockPosition::Before);
    assert!(
        rationale.blocks[0]
            .text
            .contains("requested file changes are applied")
    );
    assert_eq!(rationale.blocks[1].position, BlockPosition::After);
    assert!(rationale.blocks[1].text.contains("exit 5"));
}

#[tokio::test]
async fn shell_outcomes_upgrade_unknown_calls_from_the_structured_records() {
    let rig = Rig::new();
    let events = fixture_events(&rig, None);
    let sid = session_id(Harness::Codex, SESSION);
    let result = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move { get_shell_outcomes_impl(&state, &roots, &sid).await.unwrap() }
    })
    .await;
    let outcomes = ok_outcomes(result);
    assert!(outcomes.version_supported);
    assert_eq!(outcomes.harness_version.as_deref(), Some("0.170.0"));
    assert!(!outcomes.truncated);
    assert_eq!(outcomes.unresolved, 0);

    let by_id = |id: &str| -> &ShellOutcome {
        outcomes
            .outcomes
            .iter()
            .find(|o| o.tool_use_id == id)
            .unwrap_or_else(|| panic!("{id}"))
    };
    assert_eq!(
        (
            by_id(UNITTEST_NO_TESTS).exit_code,
            by_id(UNITTEST_NO_TESTS).outcome
        ),
        (5, ToolOutcome::Failed)
    );
    assert_eq!(
        (by_id(SH_EXIT_3).exit_code, by_id(SH_EXIT_3).outcome),
        (3, ToolOutcome::Failed)
    );
    assert_eq!(
        (by_id(UNITTEST_FAIL).exit_code, by_id(UNITTEST_FAIL).outcome),
        (1, ToolOutcome::Failed)
    );
    for outcome in &outcomes.outcomes {
        assert_eq!(outcome.evidence, EvidenceLabel::Observed);
        assert_eq!(outcome.source, TranscriptSource::CodexRollout);
        assert_eq!(
            outcome.outcome,
            if outcome.exit_code == 0 {
                ToolOutcome::Ok
            } else {
                ToolOutcome::Failed
            }
        );
    }

    println!("SHELL OUTCOMES (WHY-45 codex/0.170.0/basic-main):");
    println!("  tool_use_id -> exit code -> outcome (evidence)");
    for o in &outcomes.outcomes {
        println!(
            "  {} -> {} -> {:?} ({:?})",
            o.tool_use_id, o.exit_code, o.outcome, o.evidence
        );
    }
}

// ---- hand-built rollouts for the rules ------------------------------------

fn record(kind: &str, payload: serde_json::Value) -> String {
    format!("{}\n", json!({"type": kind, "payload": payload}))
}

pub fn meta(version: &str) -> String {
    record("session_meta", json!({"cli_version": version}))
}

pub fn agent_message(turn: &str, text: &str) -> String {
    record(
        "event_msg",
        json!({"type":"item_completed","turn_id":turn,
               "item":{"type":"AgentMessage","content":[{"type":"Text","text":text}]}}),
    )
}

pub fn command(turn: &str, id: &str, code: Option<i64>) -> String {
    record(
        "event_msg",
        json!({"type":"item_completed","turn_id":turn,
               "item":{"type":"CommandExecution","id":id,"exit_code":code}}),
    )
}

#[tokio::test]
async fn a_subagent_stop_alone_locates_the_childs_turn() {
    let rig = Rig::new();
    let rollout = [meta("0.170.0"), agent_message("child-turn", "child words")].concat();
    let child = rig.codex_file("child.jsonl", rollout.as_bytes());
    let mut stop = Ev::new(
        Harness::Codex,
        "cs",
        "SubagentStop",
        EventKind::SubagentStopped,
    )
    .turn("child-turn")
    .path(child.to_str().unwrap());
    stop.agent = Some("child");
    let result = on_both(&[stop.build()], |state| {
        let roots = rig.roots.clone();
        async move {
            get_turn_rationale_impl(
                &state,
                &roots,
                &session_id(Harness::Codex, "cs"),
                "child-turn",
            )
            .await
            .unwrap()
        }
    })
    .await;
    assert_eq!(texts(&result), ["child words"]);
}

#[tokio::test]
async fn a_clipped_codex_message_marks_turn_and_tool_rationale_truncated() {
    let rig = Rig::new();
    let rollout = [
        meta("0.170.0"),
        agent_message("t1", &"ế".repeat(EXCERPT_MAX_BYTES)),
        command("t1", "exec-clip", Some(0)),
    ]
    .concat();
    let path = rig.codex_file("r.jsonl", rollout.as_bytes());
    let events = shell_call(
        Harness::Codex,
        "cs",
        "t1",
        "exec-clip",
        None,
        500,
        path.to_str(),
    );
    let event_id = tool_event_id(&events, "exec-clip");
    let results = on_both(&events, |state| {
        let roots = rig.roots.clone();
        async move {
            let sid = session_id(Harness::Codex, "cs");
            [
                get_turn_rationale_impl(&state, &roots, &sid, "t1")
                    .await
                    .unwrap(),
                get_tool_rationale_impl(&state, &roots, &sid, event_id)
                    .await
                    .unwrap(),
            ]
        }
    })
    .await;
    for result in results {
        let RationaleResult::Ok { rationale } = result else {
            panic!("expected text")
        };
        assert!(rationale.truncated);
        assert!(rationale.blocks[0].text.ends_with('…'));
        assert!(rationale.blocks[0].text.len() <= EXCERPT_MAX_BYTES);
    }
}

#[tokio::test]
async fn shell_exits_match_the_turn_and_join_the_earliest_finish_only_row() {
    let rig = Rig::new();
    let rollout = [
        meta("0.170.0"),
        command("t2", "exec-wrong-turn", Some(0)),
        command("t1", "exec-finish-only", Some(3)),
    ]
    .concat();
    let path = rig.codex_file("r.jsonl", rollout.as_bytes());
    let mut events = shell_call(
        Harness::Codex,
        "cs",
        "t1",
        "exec-wrong-turn",
        None,
        500,
        path.to_str(),
    );
    for (turn, at) in [("t2", 800), ("t1", 700)] {
        events.push(
            Ev::new(
                Harness::Codex,
                "cs",
                "PostToolUse",
                EventKind::ToolFinished {
                    outcome: ToolOutcome::Unknown,
                },
            )
            .at(at)
            .turn(turn)
            .tool("exec-finish-only", ToolClass::Shell)
            .path(path.to_str().unwrap())
            .build(),
        );
    }
    let (result, row_id) = on_both(&events, |state| {
        let roots = rig.roots.clone();
        async move {
            let sid = session_id(Harness::Codex, "cs");
            let timeline = why_lib::ipc::sessions::get_session_timeline_impl(&state, &sid)
                .await
                .unwrap();
            let row_id = timeline.turns[0]
                .events
                .iter()
                .find(|e| e.tool_use_id.as_deref() == Some("exec-finish-only"))
                .unwrap()
                .id
                .clone();
            (
                get_shell_outcomes_impl(&state, &roots, &sid).await.unwrap(),
                row_id,
            )
        }
    })
    .await;
    let result = ok_outcomes(result);
    assert_eq!(result.unresolved, 1);
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(result.outcomes[0].event_id, row_id);
    assert_eq!(result.outcomes[0].turn_id, "t1");
    assert_eq!(result.outcomes[0].exit_code, 3);
}

#[tokio::test]
async fn a_mixed_version_session_leaves_all_shell_calls_unknown() {
    let rig = Rig::new();
    let mut events = Vec::new();
    for (version, id, at) in [
        ("9.9.9", "exec-unsupported", 500),
        ("0.170.0", "exec-supported", 600),
    ] {
        let rollout = [meta(version), command("t1", id, Some(0))].concat();
        let path = rig.codex_file(&format!("{id}.jsonl"), rollout.as_bytes());
        events.extend(shell_call(
            Harness::Codex,
            "cs",
            "t1",
            id,
            None,
            at,
            path.to_str(),
        ));
    }
    let result = on_both(&events, |state| {
        let roots = rig.roots.clone();
        async move {
            get_shell_outcomes_impl(&state, &roots, &session_id(Harness::Codex, "cs"))
                .await
                .unwrap()
        }
    })
    .await;
    let result = ok_outcomes(result);
    assert!(!result.version_supported);
    assert!(result.outcomes.is_empty());
    assert_eq!(result.unresolved, 2);
    assert_eq!(result.harness_version.as_deref(), Some("9.9.9"));
}

#[tokio::test]
async fn an_id_seen_twice_is_left_out_even_when_the_codes_agree() {
    let rig = Rig::new();
    let rollout = [
        meta("0.170.0"),
        command("t1", "exec-dup-same", Some(1)),
        command("t1", "exec-dup-same", Some(1)),
        command("t1", "exec-dup-diff", Some(0)),
        command("t2", "exec-dup-diff", Some(2)),
        command("t1", "exec-once", Some(0)),
    ]
    .concat();
    let path = rig.codex_file("r.jsonl", rollout.as_bytes());
    let p = path.to_str().unwrap();
    let mut events = vec![prompt(Harness::Codex, "cs", "t1", 500, Some(p))];
    for (i, id) in ["exec-dup-same", "exec-dup-diff", "exec-once"]
        .iter()
        .enumerate()
    {
        events.extend(shell_call(
            Harness::Codex,
            "cs",
            "t1",
            id,
            Some(ToolOutcome::Unknown),
            1_000 + 10 * i as u64,
            Some(p),
        ));
    }
    let sid = session_id(Harness::Codex, "cs");
    let outcomes = ok_outcomes(
        on_both(&events, |state| {
            let (roots, sid) = (rig.roots.clone(), sid.clone());
            async move { get_shell_outcomes_impl(&state, &roots, &sid).await.unwrap() }
        })
        .await,
    );
    let ids: Vec<_> = outcomes
        .outcomes
        .iter()
        .map(|o| o.tool_use_id.as_str())
        .collect();
    assert_eq!(ids, ["exec-once"]);
    assert_eq!(outcomes.outcomes[0].outcome, ToolOutcome::Ok);
    assert_eq!(outcomes.unresolved, 2);
}

#[tokio::test]
async fn an_untrusted_cli_version_gives_no_exit_codes() {
    let rig = Rig::new();
    let events = fixture_events(&rig, Some("9.9.9"));
    let sid = session_id(Harness::Codex, SESSION);
    let outcomes = ok_outcomes(
        on_both(&events, |state| {
            let (roots, sid) = (rig.roots.clone(), sid.clone());
            async move { get_shell_outcomes_impl(&state, &roots, &sid).await.unwrap() }
        })
        .await,
    );
    assert!(outcomes.outcomes.is_empty());
    assert!(!outcomes.version_supported);
    assert_eq!(outcomes.harness_version.as_deref(), Some("9.9.9"));
    assert!(outcomes.unresolved > 0);
}

#[tokio::test]
async fn a_call_the_hooks_already_settled_is_not_touched() {
    let rig = Rig::new();
    let rollout = [
        meta("0.170.0"),
        command("t1", "exec-ok", Some(7)),
        command("t1", "exec-bad", Some(0)),
        command("t1", "exec-unk", Some(2)),
        command("t1", "exec-nocode", None),
    ]
    .concat();
    let path = rig.codex_file("r.jsonl", rollout.as_bytes());
    let p = path.to_str().unwrap();
    let mut events = vec![prompt(Harness::Codex, "cs", "t1", 500, Some(p))];
    events.extend(shell_call(
        Harness::Codex,
        "cs",
        "t1",
        "exec-ok",
        Some(ToolOutcome::Ok),
        1_000,
        Some(p),
    ));
    events.extend(shell_call(
        Harness::Codex,
        "cs",
        "t1",
        "exec-bad",
        Some(ToolOutcome::Failed),
        1_100,
        Some(p),
    ));
    events.extend(shell_call(
        Harness::Codex,
        "cs",
        "t1",
        "exec-unk",
        None,
        1_200,
        Some(p),
    ));
    events.extend(shell_call(
        Harness::Codex,
        "cs",
        "t1",
        "exec-nocode",
        Some(ToolOutcome::Unknown),
        1_300,
        Some(p),
    ));
    events.extend(shell_call(
        Harness::Codex,
        "cs",
        "t1",
        "exec-absent",
        Some(ToolOutcome::Unknown),
        1_400,
        Some(p),
    ));
    let sid = session_id(Harness::Codex, "cs");
    let outcomes = ok_outcomes(
        on_both(&events, |state| {
            let (roots, sid) = (rig.roots.clone(), sid.clone());
            async move { get_shell_outcomes_impl(&state, &roots, &sid).await.unwrap() }
        })
        .await,
    );
    let ids: Vec<_> = outcomes
        .outcomes
        .iter()
        .map(|o| (o.tool_use_id.as_str(), o.exit_code))
        .collect();
    // exec-ok / exec-bad came from the hooks and stay as they are; a record with
    // no integer code and a call with no record stay unknown.
    assert_eq!(ids, [("exec-unk", 2)]);
    assert_eq!(outcomes.unresolved, 2);
}

#[tokio::test]
async fn an_encrypted_message_stays_encrypted() {
    let rig = Rig::new();
    let blob = format!("gAAAA{}", "x".repeat(60));
    let rollout = [
        meta("0.170.0"),
        agent_message("t1", &blob),
        agent_message("t1", "plain words"),
    ]
    .concat();
    let path = rig.codex_file("r.jsonl", rollout.as_bytes());
    let events = vec![prompt(Harness::Codex, "cs", "t1", 500, path.to_str())];
    let sid = session_id(Harness::Codex, "cs");
    let result = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move {
            get_turn_rationale_impl(&state, &roots, &sid, "t1")
                .await
                .unwrap()
        }
    })
    .await;
    let RationaleResult::Ok { rationale } = result else {
        panic!("expected text");
    };
    assert_eq!(rationale.blocks[0].text, "(encrypted)");
    assert!(rationale.blocks[0].encrypted);
    assert_eq!(rationale.blocks[1].text, "plain words");
    assert!(!rationale.blocks[1].encrypted);
    assert!(!rationale.blocks.iter().any(|b| b.text.contains("gAAAA")));
}

/// A turn or call the store knows but the rollout does not hold is `notFound`.
#[tokio::test]
async fn an_anchor_missing_from_the_rollout_is_not_found() {
    let rig = Rig::new();
    let path = rig.codex_file(
        "r.jsonl",
        [meta("0.170.0"), agent_message("t1", "hi")]
            .concat()
            .as_bytes(),
    );
    let p = path.to_str().unwrap();
    let mut events = vec![
        prompt(Harness::Codex, "cs", "t1", 500, Some(p)),
        prompt(Harness::Codex, "cs", "t-missing", 600, Some(p)),
    ];
    events.extend(shell_call(
        Harness::Codex,
        "cs",
        "t1",
        "exec-ghost",
        None,
        700,
        Some(p),
    ));
    let sid = session_id(Harness::Codex, "cs");
    let not_found = RationaleResult::Unavailable {
        reason: TranscriptUnavailable::NotFound,
    };
    let turn = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move {
            get_turn_rationale_impl(&state, &roots, &sid, "t-missing")
                .await
                .unwrap()
        }
    })
    .await;
    assert_eq!(turn, not_found);
    let event_id = tool_event_id(&events, "exec-ghost");
    let tool = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move {
            get_tool_rationale_impl(&state, &roots, &sid, event_id)
                .await
                .unwrap()
        }
    })
    .await;
    assert_eq!(tool, not_found);
}
