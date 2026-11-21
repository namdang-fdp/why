//! Claude Code: turn and tool rationale from the WHY-45 `basic-main` fixture
//! and from hand-built transcripts.

use serde_json::{Value, json};
use why_lib::contracts::ShellOutcomesResult;
use why_lib::contracts::{
    BlockPosition, EvidenceLabel, RationaleResult, TranscriptSource, TranscriptUnavailable,
};
use why_lib::events::{EXCERPT_MAX_BYTES, EventKind, ToolClass};
use why_lib::ipc::rationale::{
    get_shell_outcomes_impl, get_tool_rationale_impl, get_turn_rationale_impl,
};
use why_wire::Harness;

use crate::common::*;

const SESSION: &str = "69c7ffb9-e8d9-43db-8d22-958e94e1e889";

/// The real prompt ids of the fixture (the hooks carry them whole).
fn fixture_events(rig: &Rig) -> (Vec<why_lib::events::NormalizedEvent>, String, String) {
    let dir = claude_fixture("basic-main");
    let file = rig.claude_file(
        &format!("{SESSION}.jsonl"),
        &std::fs::read(dir.join("transcript.jsonl")).unwrap(),
    );
    let mut events = replay(&dir.join("hooks.jsonl"));
    let dest = file.clone();
    rewrite_paths(&mut events, |_| Some(dest.clone()));
    let turns: Vec<String> = events
        .iter()
        .filter(|e| e.kind == EventKind::Prompt)
        .filter_map(|e| e.turn_id.clone())
        .collect();
    (events, turns[0].clone(), turns[1].clone())
}

#[tokio::test]
async fn turn_rationale_returns_the_agents_final_words_as_declared() {
    let rig = Rig::new();
    let (events, turn1, turn2) = fixture_events(&rig);
    let sid = session_id(Harness::ClaudeCode, SESSION);

    let result = on_both(&events, |state| {
        let (roots, sid, turn) = (rig.roots.clone(), sid.clone(), turn1.clone());
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
    assert_eq!(rationale.source, TranscriptSource::ClaudeTranscript);
    assert_eq!(rationale.harness_version.as_deref(), Some("2.1.322"));
    assert!(rationale.version_supported);
    assert!(!rationale.truncated);
    assert!(
        rationale.blocks[0]
            .text
            .starts_with("Done — all four steps ran"),
        "{:?}",
        rationale.blocks
    );
    assert!(
        rationale
            .blocks
            .iter()
            .all(|b| b.position == BlockPosition::Turn && !b.encrypted)
    );
    assert!(rationale.blocks[0].timestamp.is_some());
    println!(
        "SAMPLE claude get_turn_rationale:\n{}",
        serde_json::to_string_pretty(&result).unwrap()
    );

    // The second turn is read from the same file and does not include turn 1.
    let second = on_both(&events, |state| {
        let (roots, sid, turn) = (rig.roots.clone(), sid.clone(), turn2.clone());
        async move {
            get_turn_rationale_impl(&state, &roots, &sid, &turn)
                .await
                .unwrap()
        }
    })
    .await;
    assert!(
        texts(&second)[0].starts_with("The test run failed"),
        "{:?}",
        texts(&second)
    );
}

fn line(v: Value) -> String {
    format!("{v}\n")
}

/// A transcript where the agent narrates before the call `toolu_B`.
fn narrated() -> String {
    [
        line(
            json!({"type":"user","promptId":"p1","uuid":"u1","version":"2.1.322",
                    "message":{"role":"user","content":[{"type":"text","text":"go"}]}}),
        ),
        line(
            json!({"type":"assistant","uuid":"a1","timestamp":"2026-11-20T08:00:00Z",
                    "message":{"content":[
                        {"type":"text","text":"First I will read the file."},
                        {"type":"tool_use","id":"toolu_A","name":"Read","input":{}}]}}),
        ),
        line(
            json!({"type":"user","promptId":"p1","uuid":"u2","version":"2.1.322",
                    "message":{"content":[{"type":"tool_result"}]}}),
        ),
        line(
            json!({"type":"assistant","uuid":"a2","timestamp":"2026-11-20T08:00:05Z",
                    "message":{"content":[
                        {"type":"text","text":"Now the off-by-one fix."},
                        {"type":"tool_use","id":"toolu_B","name":"Bash","input":{}}]}}),
        ),
    ]
    .concat()
}

fn narrated_events(path: &str) -> Vec<why_lib::events::NormalizedEvent> {
    let mut events = vec![prompt(Harness::ClaudeCode, "cs", "p1", 1_000, Some(path))];
    events.extend(shell_call(
        Harness::ClaudeCode,
        "cs",
        "p1",
        "toolu_B",
        Some(why_lib::contracts::ToolOutcome::Ok),
        2_000,
        Some(path),
    ));
    events
}

#[tokio::test]
async fn tool_rationale_is_the_narration_just_before_the_call() {
    let rig = Rig::new();
    let path = rig.claude_file("cs.jsonl", narrated().as_bytes());
    let events = narrated_events(path.to_str().unwrap());
    let sid = session_id(Harness::ClaudeCode, "cs");
    let event_id = tool_event_id(&events, "toolu_B");
    let result = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move {
            get_tool_rationale_impl(&state, &roots, &sid, event_id)
                .await
                .unwrap()
        }
    })
    .await;
    let RationaleResult::Ok { rationale } = &result else {
        panic!("{result:?}");
    };
    // Only the narration of the same step: not "First I will read the file."
    // (that belongs to toolu_A, the previous step).
    assert_eq!(texts(&result), ["Now the off-by-one fix."]);
    assert_eq!(rationale.blocks[0].position, BlockPosition::Before);
    assert_eq!(rationale.evidence, EvidenceLabel::Declared);
    println!(
        "SAMPLE claude get_tool_rationale:\n{}",
        serde_json::to_string_pretty(&result).unwrap()
    );
}

#[tokio::test]
async fn a_tool_the_transcript_does_not_hold_is_not_found_not_an_error() {
    let rig = Rig::new();
    let path = rig.claude_file("cs.jsonl", narrated().as_bytes());
    let mut events = narrated_events(path.to_str().unwrap());
    // A call the store knows, the file does not.
    events.extend(shell_call(
        Harness::ClaudeCode,
        "cs",
        "p1",
        "toolu_GHOST",
        None,
        3_000,
        path.to_str(),
    ));
    let sid = session_id(Harness::ClaudeCode, "cs");
    let event_id = tool_event_id(&events, "toolu_GHOST");
    let result = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move {
            get_tool_rationale_impl(&state, &roots, &sid, event_id)
                .await
                .unwrap()
        }
    })
    .await;
    assert_eq!(
        result,
        RationaleResult::Unavailable {
            reason: TranscriptUnavailable::NotFound
        }
    );
}

#[tokio::test]
async fn a_newer_harness_version_still_returns_text_but_is_marked_unsupported() {
    let rig = Rig::new();
    let newer = narrated().replace("2.1.322", "3.0.1");
    let path = rig.claude_file("cs.jsonl", newer.as_bytes());
    let events = narrated_events(path.to_str().unwrap());
    let sid = session_id(Harness::ClaudeCode, "cs");
    let event_id = tool_event_id(&events, "toolu_B");
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
    assert_eq!(rationale.blocks.len(), 1);
    assert_eq!(rationale.harness_version.as_deref(), Some("3.0.1"));
    assert!(!rationale.version_supported);
}

#[tokio::test]
async fn claude_shell_outcomes_are_not_applicable_because_hooks_report_failures() {
    let rig = Rig::new();
    let (events, _, _) = fixture_events(&rig);
    let sid = session_id(Harness::ClaudeCode, SESSION);
    let result = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move { get_shell_outcomes_impl(&state, &roots, &sid).await.unwrap() }
    })
    .await;
    assert_eq!(
        result,
        ShellOutcomesResult::Unavailable {
            reason: TranscriptUnavailable::NotApplicable
        }
    );
}

/// A subagent's own tool call: its words are in the subagent's file, which the
/// store knows from the `SubagentStop` event, not in the parent transcript the
/// tool hooks name.
#[tokio::test]
async fn a_subagent_tool_call_is_read_from_the_subagents_file() {
    let rig = Rig::new();
    let dir = claude_fixture("subagent");
    let parent = rig.claude_file(
        "parent.jsonl",
        &std::fs::read(dir.join("transcript.jsonl")).unwrap(),
    );
    let child = rig.claude_file(
        "parent/subagents/agent-1.jsonl",
        &std::fs::read(dir.join("subagent-transcript.jsonl")).unwrap(),
    );
    let mut events = replay(&dir.join("hooks.jsonl"));
    let (p, c) = (parent.clone(), child.clone());
    rewrite_paths(&mut events, |orig| {
        Some(
            if orig.contains("agents/agent-") || orig.contains("subagents") {
                c.clone()
            } else {
                p.clone()
            },
        )
    });
    let call = events
        .iter()
        .find(|e| {
            e.agent_id.is_some() && e.tool.as_ref().is_some_and(|t| t.class == ToolClass::Edit)
        })
        .expect("the subagent's Write call");
    let event_id = call.key.0.as_str();
    let session = call.session.clone();
    let result = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), session.to_contract());
        async move {
            get_tool_rationale_impl(&state, &roots, &sid, event_id)
                .await
                .unwrap()
        }
    })
    .await;
    // Found in the subagent's file...
    assert!(matches!(result, RationaleResult::Ok { .. }), "{result:?}");

    // ...whereas the parent transcript the tool hooks name does not hold it,
    // so without the subagent's own path the answer is `notFound`.
    let mut parent_only = events.clone();
    rewrite_paths(&mut parent_only, |_| Some(parent.clone()));
    let result = on_both(&parent_only, |state| {
        let (roots, sid) = (rig.roots.clone(), session.to_contract());
        async move {
            get_tool_rationale_impl(&state, &roots, &sid, event_id)
                .await
                .unwrap()
        }
    })
    .await;
    assert_eq!(
        result,
        RationaleResult::Unavailable {
            reason: TranscriptUnavailable::NotFound
        }
    );
}

#[tokio::test]
async fn the_timeline_distinguishes_two_agents_calls_with_the_same_harness_id() {
    let rig = Rig::new();
    let parent = rig.claude_file("parent.jsonl", narrated().as_bytes());
    let child_text = narrated().replace("Now the off-by-one fix.", "Child narration.");
    let child = rig.claude_file("child.jsonl", child_text.as_bytes());
    let mut events = narrated_events(parent.to_str().unwrap());
    let mut call = Ev::new(
        Harness::ClaudeCode,
        "cs",
        "PreToolUse",
        EventKind::ToolStarted,
    )
    .turn("p1")
    .at(3_000)
    .tool("toolu_B", ToolClass::Shell)
    .path(parent.to_str().unwrap());
    call.agent = Some("child");
    let call = call.build();
    let child_event_id = call.key.0.clone();
    events.push(call);
    let sid = session_id(Harness::ClaudeCode, "cs");
    let before_stop = on_both(&events, |state| {
        let (roots, sid, id) = (rig.roots.clone(), sid.clone(), child_event_id.clone());
        async move {
            get_tool_rationale_impl(&state, &roots, &sid, &id)
                .await
                .unwrap()
        }
    })
    .await;
    assert_eq!(
        before_stop,
        RationaleResult::Unavailable {
            reason: TranscriptUnavailable::NoTranscript
        }
    );

    let mut stop = Ev::new(
        Harness::ClaudeCode,
        "cs",
        "SubagentStop",
        EventKind::SubagentStopped,
    )
    .turn("p1")
    .at(4_000)
    .path(child.to_str().unwrap());
    stop.agent = Some("child");
    events.push(stop.build());
    let results = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move {
            let timeline = why_lib::ipc::sessions::get_session_timeline_impl(&state, &sid)
                .await
                .unwrap();
            let mut results = Vec::new();
            for row in &timeline.turns[0].events {
                if row.tool_use_id.as_deref() == Some("toolu_B") {
                    let rationale = get_tool_rationale_impl(&state, &roots, &sid, &row.id).await;
                    results.push((row.agent_id.clone(), rationale.map(|r| texts(&r))));
                }
            }
            results
        }
    })
    .await;
    assert_eq!(
        results,
        [
            (None, Ok(vec!["Now the off-by-one fix.".to_owned()])),
            (
                Some("child".to_owned()),
                Ok(vec!["Child narration.".to_owned()])
            ),
        ]
    );
}

#[tokio::test]
async fn a_clipped_claude_block_marks_the_turn_truncated() {
    let rig = Rig::new();
    let transcript = narrated().replace("Now the off-by-one fix.", &"ế".repeat(EXCERPT_MAX_BYTES));
    let path = rig.claude_file("cs.jsonl", transcript.as_bytes());
    let events = narrated_events(path.to_str().unwrap());
    let result = on_both(&events, |state| {
        let roots = rig.roots.clone();
        async move {
            get_turn_rationale_impl(&state, &roots, &session_id(Harness::ClaudeCode, "cs"), "p1")
                .await
                .unwrap()
        }
    })
    .await;
    let RationaleResult::Ok { rationale } = result else {
        panic!("expected text")
    };
    let block = rationale.blocks.last().unwrap();
    assert!(rationale.truncated);
    assert!(block.text.ends_with('…'));
    assert!(block.text.len() <= EXCERPT_MAX_BYTES);
}
