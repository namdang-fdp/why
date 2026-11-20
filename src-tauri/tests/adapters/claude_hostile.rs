//! Hostile and odd payloads: the adapter must never panic and must say
//! "nothing" rather than guess. The input comes from another program.

use serde_json::{Value, json};
use why_lib::adapters::claude::{self, extract, shell};
use why_lib::adapters::normalize;
use why_lib::contracts::{EvidenceLabel, ToolOutcome};
use why_lib::events::{EXCERPT_MAX_BYTES, EventKind, ToolClass};

use crate::common::*;

fn run(payload: Value) -> Vec<why_lib::events::NormalizedEvent> {
    normalize(&envelope(1, payload))
}

#[test]
fn non_object_and_missing_session_yield_nothing() {
    for payload in [
        Value::Null,
        json!(42),
        json!("Stop"),
        json!([1, 2]),
        json!({}),
        json!({"hook_event_name": "Stop"}),
        json!({"hook_event_name": "Stop", "session_id": ""}),
        json!({"hook_event_name": "Stop", "session_id": 7}),
    ] {
        assert!(run(payload.clone()).is_empty(), "{payload}");
    }
}

#[test]
fn unknown_events_are_ignored_and_counted() {
    let before = claude::ignored_events();
    for name in ["Notification", "PreCompact", "FutureEvent2027", "", "stop"] {
        assert!(run(base(name)).is_empty(), "{name:?}");
    }
    assert!(claude::ignored_events() >= before + 5);
}

#[test]
fn wrong_types_degrade_instead_of_panicking() {
    // tool_name / ids / strings of the wrong type.
    let evs = run(with(
        base("PreToolUse"),
        json!({"tool_name": 5, "tool_use_id": [1], "tool_input": "oops"}),
    ));
    assert!(evs.is_empty(), "no usable tool name: nothing to show");

    // tool_input not an object: still a row, with no file and no text.
    for input in [json!("rm -rf /"), json!(null), json!(12), json!([1])] {
        let evs = run(with(
            base("PreToolUse"),
            json!({"tool_name": "Write", "tool_use_id": "t", "tool_input": input}),
        ));
        assert_eq!(evs.len(), 1);
        assert!(evs[0].files.is_empty());
        assert_eq!(evs[0].tool.as_ref().unwrap().class, ToolClass::Edit);
    }

    // prompt of the wrong type: the event exists, the text does not.
    let evs = run(with(base("UserPromptSubmit"), json!({"prompt": {"a": 1}})));
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].text, None);

    // Post with a string tool_response and a failure with a non-string error.
    let evs = run(with(
        base("PostToolUse"),
        json!({"tool_name": "Edit", "tool_use_id": "t",
               "tool_input": {"file_path": "/x"}, "tool_response": "weird"}),
    ));
    assert_eq!(evs[0].files, ["/x"]);
    assert_eq!(evs[0].text, None);
    let evs = run(with(
        base("PostToolUseFailure"),
        json!({"tool_name": "Bash", "tool_use_id": "t", "error": 9,
               "tool_input": {"command": "false"}}),
    ));
    assert_eq!(
        evs[0].kind,
        EventKind::ToolFinished {
            outcome: ToolOutcome::Failed
        }
    );

    // PostToolBatch whose tool_calls are the wrong shape.
    for calls in [
        json!("x"),
        json!([1, null, "s"]),
        json!([{"tool_name": 3}]),
        json!({}),
    ] {
        assert!(run(with(base("PostToolBatch"), json!({"tool_calls": calls}))).is_empty());
    }
}

#[test]
fn huge_strings_are_capped_on_a_char_boundary() {
    // 3-byte characters, so a naive byte cut lands inside one.
    let big = "世".repeat(EXCERPT_MAX_BYTES);
    for (hook, extra) in [
        ("UserPromptSubmit", json!({"prompt": big})),
        ("Stop", json!({"last_assistant_message": big})),
        (
            "PreToolUse",
            json!({"tool_name": "Bash", "tool_use_id": "t",
                   "tool_input": {"command": big, "description": big}}),
        ),
    ] {
        let evs = run(with(base(hook), extra));
        assert!(!evs.is_empty(), "{hook}");
        // `Stop`-like marker events carry no text; every text that exists is capped.
        let texts: Vec<_> = evs.into_iter().filter_map(|e| e.text).collect();
        assert!(!texts.is_empty(), "{hook}");
        for t in texts {
            assert!(t.len() <= EXCERPT_MAX_BYTES, "{hook}: {} bytes", t.len());
            assert!(t.ends_with('…'));
        }
    }

    // A giant diff in a successful Edit is capped too.
    let lines: Vec<String> = (0..50_000).map(|i| format!("+line {i}")).collect();
    let evs = run(with(
        base("PostToolUse"),
        json!({"tool_name": "Edit", "tool_use_id": "t",
               "tool_input": {"file_path": "/x"},
               "tool_response": {"filePath": "/x", "structuredPatch":
                   [{"oldStart":1,"oldLines":0,"newStart":1,"newLines":50000,"lines": lines}]}}),
    ));
    assert!(evs[0].text.as_ref().unwrap().len() <= EXCERPT_MAX_BYTES);
}

#[test]
fn paths_with_spaces_and_unicode_survive_untouched() {
    for path in [
        "/TMP/my repo/src/a b.py",
        "/TMP/репо/日本語/файл.rs",
        "/TMP/a\tb\n/c",
    ] {
        let evs = run(with(
            base("PostToolUse"),
            json!({"tool_name": "Write", "tool_use_id": "t",
                   "tool_input": {"file_path": path, "content": "x"},
                   "tool_response": {"type": "create", "filePath": path, "content": "x",
                                      "structuredPatch": [], "originalFile": null}}),
        ));
        assert_eq!(evs[0].files, [path]);
    }
}

#[test]
fn a_payload_from_a_newer_version_with_extra_fields_still_works() {
    let evs = run(with(
        base("PreToolUse"),
        json!({
            "tool_name": "Bash", "tool_use_id": "t",
            "tool_input": {"command": "cargo test", "description": "run", "new_flag": true,
                           "nested": {"a": [1, 2, {"b": null}]}},
            "future_field": {"x": 1}, "model": "claude-next-9",
            "permission_mode": "somethingNew", "effort": {"level": "ultra", "extra": 1},
        }),
    ));
    // Command row (a test run) and the Declared description.
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0].tool.as_ref().unwrap().class, ToolClass::TestRun);
    assert_eq!(evs[1].kind, EventKind::AssistantMessage);
    assert_eq!(evs[1].evidence, EvidenceLabel::Declared);
    // Claude has no `model` in payloads (payloads.md §2): we do not invent one.
    assert!(evs.iter().all(|e| e.model.is_none()));
}

#[test]
fn envelope_event_name_is_the_fallback_not_the_authority() {
    // Payload has no hook_event_name: the envelope's name is used.
    let mut payload = base("Stop");
    payload.as_object_mut().unwrap().remove("hook_event_name");
    let mut env = envelope(1, payload);
    env.event = "Stop".into();
    assert_eq!(normalize(&env).len(), 1);
    // The payload's own name wins over a contradicting envelope.
    let mut env = envelope(1, base("Stop"));
    env.event = "SessionEnd".into();
    assert_eq!(normalize(&env)[0].kind, EventKind::Stop);
}

#[test]
fn only_edit_rejections_in_a_batch_become_events() {
    let rejected = "<tool_use_error>String to replace not found in file.</tool_use_error>";
    let payload = with(
        base("PostToolBatch"),
        json!({"agent_id": "a1", "tool_calls": [
            {"tool_name": "Edit", "tool_use_id": "e1", "tool_input": {"file_path": "/r/a.py"},
             "tool_response": rejected},
            // Same marker but not an edit: not ours.
            {"tool_name": "Read", "tool_use_id": "r1", "tool_input": {"file_path": "/r/b.py"},
             "tool_response": rejected},
            // A successful edit: its own Post event is the record.
            {"tool_name": "Write", "tool_use_id": "w1", "tool_input": {"file_path": "/r/c.py"},
             "tool_response": "File created"},
            // A failed Bash is a PostToolUseFailure, never a batch event.
            {"tool_name": "Bash", "tool_use_id": "b1", "tool_input": {"command": "false"},
             "tool_response": "Exit code 1"},
            {"tool_name": "Edit", "tool_use_id": "e2", "tool_input": {"file_path": "/r/d.py"},
             "tool_response": rejected},
        ]}),
    );
    let evs = normalize_all(&[envelope(1, payload)]);
    assert_eq!(evs.len(), 2);
    let e = &evs[0];
    assert_eq!(
        e.kind,
        EventKind::ToolFinished {
            outcome: ToolOutcome::Failed
        }
    );
    assert_eq!(e.files, ["/r/a.py"]);
    assert_eq!(e.agent_id.as_deref(), Some("a1"));
    assert_eq!(e.tool.as_ref().unwrap().use_id.as_deref(), Some("e1"));
    assert_eq!(e.tool.as_ref().unwrap().class, ToolClass::Edit);
    assert_eq!(evs[1].tool.as_ref().unwrap().use_id.as_deref(), Some("e2"));
}

#[test]
fn stop_emits_declared_message_then_stop_with_positional_keys() {
    let env = envelope(
        5,
        with(base("Stop"), json!({"last_assistant_message": "done."})),
    );
    let evs = normalize(&env);
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0].kind, EventKind::AssistantMessage);
    assert_eq!(evs[0].evidence, EvidenceLabel::Declared);
    assert_eq!(evs[1].kind, EventKind::Stop);
    assert_eq!(evs[1].evidence, EvidenceLabel::Observed);
    assert_ne!(evs[0].key, evs[1].key);
    // Empty message: only the Stop.
    let env = envelope(5, with(base("Stop"), json!({"last_assistant_message": ""})));
    assert_eq!(normalize(&env).len(), 1);
}

#[test]
fn normalization_is_deterministic() {
    let env = envelope(
        9,
        with(
            base("PreToolUse"),
            json!({"tool_name": "Bash", "tool_use_id": "t",
                   "tool_input": {"command": "pnpm test", "description": "d"}}),
        ),
    );
    assert_eq!(normalize(&env), normalize(&env.clone()));
}

#[test]
fn test_run_heuristic_is_documented_behaviour() {
    assert!(shell::is_test_run("cargo nextest run"));
    assert!(!shell::is_test_run("cd src-tauri && cargo nextest run"));
    assert!(!shell::is_test_run("echo pytest"));
    assert_eq!(
        claude::classify("Bash", &json!({"command": "go test ./..."})),
        ToolClass::TestRun
    );
    assert_eq!(
        claude::classify("Bash", &json!({"command": 5})),
        ToolClass::Shell
    );
    assert_eq!(claude::classify("Agent", &json!({})), ToolClass::Subagent);
    assert_eq!(claude::classify("mcp__x__y", &json!({})), ToolClass::Other);
    assert_eq!(extract::exit_code("Exit code 2"), Some(2));
}
