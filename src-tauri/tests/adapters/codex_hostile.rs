//! Hook payloads that are missing fields, have the wrong types, or carry
//! things this adapter has never seen. The contract: no panic, and either a
//! sensible event or nothing.

use serde_json::{Value, json};
use why_lib::adapters::normalize;
use why_lib::contracts::{EvidenceLabel, ToolOutcome};
use why_lib::events::{EXCERPT_MAX_BYTES, EventKind, ToolClass};

use crate::codex_common::{envelope, load, scenarios};

fn run(payload: Value) -> Vec<why_lib::events::NormalizedEvent> {
    normalize(&envelope(payload, 1))
}

#[test]
fn missing_session_id_yields_nothing() {
    for event in ["SessionStart", "UserPromptSubmit", "PreToolUse", "Stop"] {
        assert!(run(json!({"hook_event_name": event})).is_empty(), "{event}");
        assert!(
            run(json!({"hook_event_name": event, "session_id": ""})).is_empty(),
            "{event}"
        );
        assert!(
            run(json!({"hook_event_name": event, "session_id": 42})).is_empty(),
            "{event}"
        );
    }
}

#[test]
fn payload_that_is_not_an_object_yields_nothing() {
    for p in [
        json!(null),
        json!("x"),
        json!(1),
        json!([1, 2]),
        json!(true),
    ] {
        assert!(run(p).is_empty());
    }
}

#[test]
fn unknown_and_ignored_events_yield_nothing() {
    for event in [
        "PreModelRequest",
        "PostModelRequest",
        "PermissionRequest",
        "PreCompact",
        "Interrupt",
        "BrandNewEventFromCodex0171",
        "",
    ] {
        let events = run(json!({"hook_event_name": event, "session_id": "s", "turn_id": "t"}));
        assert!(events.is_empty(), "{event:?}");
    }
}

#[test]
fn wrong_types_degrade_field_by_field() {
    let events = run(json!({
        "hook_event_name": "UserPromptSubmit", "session_id": "s",
        "turn_id": 7, "prompt": ["not", "text"], "model": {"x": 1},
        "transcript_path": null, "cwd": 5,
    }));
    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(e.kind, EventKind::Prompt);
    assert_eq!(
        (&e.turn_id, &e.text, &e.model, &e.transcript_path),
        (&None, &None, &None, &None)
    );
    assert_eq!(e.cwd, "/repo", "falls back to the envelope's cwd");
}

#[test]
fn tool_events_tolerate_odd_inputs() {
    // No tool_name: skipped. tool_input a string / array / absent: still an event.
    assert!(run(json!({"hook_event_name":"PreToolUse","session_id":"s"})).is_empty());
    assert!(run(json!({"hook_event_name":"PreToolUse","session_id":"s","tool_name":9})).is_empty());
    for input in [
        json!("ls"),
        json!([1]),
        json!(null),
        json!({"command": 5}),
        json!({}),
    ] {
        for tool in [
            "Bash",
            "apply_patch",
            "collaborationspawn_agent",
            "mcp__x__y",
        ] {
            let events = run(json!({
                "hook_event_name":"PreToolUse","session_id":"s","tool_name":tool,
                "tool_use_id": 3, "tool_input": input,
            }));
            assert_eq!(events.len(), 1, "{tool} {input}");
            assert_eq!(events[0].tool.as_ref().unwrap().use_id, None);
        }
    }
}

#[test]
fn post_tool_use_response_of_the_wrong_type_is_unknown() {
    for response in [
        json!(null),
        json!(5),
        json!({"exit": 0}),
        json!(["Exit code: 0"]),
    ] {
        let events = run(json!({
            "hook_event_name":"PostToolUse","session_id":"s","tool_name":"apply_patch",
            "tool_use_id":"u","tool_input":{"command":"*** Begin Patch\n*** End Patch"},
            "tool_response": response,
        }));
        assert_eq!(
            events[0].kind,
            EventKind::ToolFinished {
                outcome: ToolOutcome::Unknown
            },
            "{response}"
        );
    }
}

#[test]
fn shell_outcome_is_never_inferred_from_output() {
    // A passing-looking and a failing-looking output: both unknown.
    for output in [
        "OK\n",
        "FAILED (failures=1)\nExit code: 1",
        "Exit code: 0\nSuccess",
        "",
    ] {
        let events = run(json!({
            "hook_event_name":"PostToolUse","session_id":"s","tool_name":"Bash",
            "tool_use_id":"u","tool_input":{"command":"python3 -m unittest"},
            "tool_response": output,
        }));
        assert_eq!(
            events[0].kind,
            EventKind::ToolFinished {
                outcome: ToolOutcome::Unknown
            },
            "{output:?}"
        );
        assert_eq!(events[0].tool.as_ref().unwrap().class, ToolClass::TestRun);
    }
}

#[test]
fn extra_unknown_fields_are_ignored() {
    let events = run(json!({
        "hook_event_name":"UserPromptSubmit","session_id":"s","turn_id":"t","prompt":"hi",
        "added_in_0_171": {"deep": [1,2,3]}, "another": null,
    }));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text.as_deref(), Some("hi"));
}

#[test]
fn huge_prompt_and_message_are_capped_on_a_char_boundary() {
    let big = "世".repeat(EXCERPT_MAX_BYTES);
    let prompt = run(json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":big}));
    assert!(prompt[0].text.as_ref().unwrap().len() <= EXCERPT_MAX_BYTES);
    let stop = run(json!({"hook_event_name":"Stop","session_id":"s","last_assistant_message":big}));
    assert_eq!(stop.len(), 2);
    assert!(stop[0].text.as_ref().unwrap().len() <= EXCERPT_MAX_BYTES);
    assert_eq!(
        stop[0].evidence,
        EvidenceLabel::Declared,
        "the agent's own words"
    );
    assert_eq!(stop[1].evidence, EvidenceLabel::Observed);
    let subagent =
        run(json!({"hook_event_name":"SubagentStart","session_id":"s","agent_type":big}));
    let summary = subagent[0].text.as_deref().unwrap();
    assert!(summary.len() <= EXCERPT_MAX_BYTES);
    assert!(summary.ends_with('…'));
}

#[test]
fn encrypted_spawn_message_is_never_copied() {
    let blob = format!("gAAAAAB{}", "Zm9v".repeat(40));
    let events = run(json!({
        "hook_event_name":"PreToolUse","session_id":"s","tool_name":"collaborationspawn_agent",
        "tool_use_id":"u","tool_input":{"task_name":"t1","fork_turns":"all","message":blob},
    }));
    assert_eq!(events[0].text.as_deref(), Some("t1: (encrypted)"));
    assert_eq!(events[0].tool.as_ref().unwrap().class, ToolClass::Subagent);
    assert!(!format!("{events:?}").contains("gAAAA"));
}

/// Mutate every recorded payload one field at a time (remove it, null it, make
/// it a number, a list, an object): nothing may panic. A poor man's fuzzer
/// seeded with the real shapes.
#[test]
fn every_fixture_payload_survives_single_field_mutation() {
    let mut mutations = 0;
    for (_, hooks) in scenarios() {
        for env in load(&hooks) {
            let Some(obj) = env.payload.as_object() else {
                continue;
            };
            for key in obj.keys() {
                for variant in [
                    None,
                    Some(json!(null)),
                    Some(json!(1)),
                    Some(json!([1])),
                    Some(json!({"a": 1})),
                    Some(json!("")),
                ] {
                    let mut payload = env.payload.clone();
                    match &variant {
                        None => {
                            payload.as_object_mut().unwrap().remove(key);
                        }
                        Some(v) => payload[key] = v.clone(),
                    }
                    let mut e = env.clone();
                    e.payload = payload;
                    let events = normalize(&e);
                    // Keys stay unique within one envelope.
                    let mut keys: Vec<_> = events.iter().map(|x| x.key.clone()).collect();
                    keys.sort();
                    keys.dedup();
                    assert_eq!(keys.len(), events.len());
                    mutations += 1;
                }
            }
        }
    }
    assert!(mutations > 5000, "{mutations}");
}
