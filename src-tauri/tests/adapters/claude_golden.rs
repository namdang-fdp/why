//! Golden replay of every Claude fixture scenario, plus the named behaviours.
//!
//! Each scenario's `hooks.jsonl` goes through `normalize` + ingest + timeline.
//! The result is rendered as a small text structure (turns → one line per
//! event) and compared with `golden/claude/<scenario>.json`.
//!
//! Updating goldens after an intended change:
//!   `cargo run --manifest-path src-tauri/Cargo.toml --example regenerate_claude_goldens`
//! then READ the diff of `golden/` before committing: it is the behaviour.
//!
//! Run with `-- --nocapture` to print the scenario table.

use serde_json::Value;
use why_lib::adapters::claude::extract::is_synthetic_prompt;
use why_lib::contracts::{TimelineEvent, ToolOutcome};
use why_lib::events::{EventKind, ToolClass};

use crate::common::*;

mod render;

use render::render;

#[tokio::test]
async fn every_scenario_matches_its_golden_file() {
    let mut table = vec![format!(
        "{:<46} {:>5} {:>5}  {:<30} {}",
        "scenario", "turns", "events", "files touched", "tool outcomes (ok/failed/unknown)"
    )];
    let mut checked = 0;
    for (name, envs) in scenarios() {
        let timelines = replay(&envs).await;
        let actual = render(&timelines);

        let path = golden_path(&name);
        let expected: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!(
                    "{}: {e} (see the regenerate_claude_goldens example)",
                    path.display()
                )
            }))
            .unwrap();
        assert_eq!(
            expected,
            actual,
            "{name}: replay differs from {}\nactual:\n{}",
            path.display(),
            serde_json::to_string_pretty(&actual).unwrap()
        );
        checked += 1;

        // Table row.
        let events: Vec<&TimelineEvent> = timelines
            .iter()
            .flat_map(|t| t.turns.iter().flat_map(|u| &u.events))
            .collect();
        let mut files: Vec<&str> = events
            .iter()
            .flat_map(|e| e.files.iter().map(String::as_str))
            .collect();
        files.sort();
        files.dedup();
        let count = |o| events.iter().filter(|e| e.outcome == Some(o)).count();
        table.push(format!(
            "{:<46} {:>5} {:>5}  {:<30} {}/{}/{}",
            name,
            timelines.iter().map(|t| t.turns.len()).sum::<usize>(),
            events.len(),
            files.join(","),
            count(ToolOutcome::Ok),
            count(ToolOutcome::Failed),
            count(ToolOutcome::Unknown),
        ));
    }
    println!("\n{}\n", table.join("\n"));
    assert_eq!(
        checked, 16,
        "fixture scenario count changed: add/update goldens"
    );
}

fn scenario(name: &str) -> Vec<why_wire::Envelope> {
    scenarios()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no scenario {name}"))
        .1
}

#[tokio::test]
async fn resume_keeps_one_session_with_resumed_start() {
    let envs = scenario("basic-main");
    let timelines = replay(&envs).await;
    assert_eq!(
        timelines.len(),
        1,
        "resume must not create a second session"
    );
    assert_eq!(timelines[0].turns.len(), 2);

    let starts: Vec<bool> = normalize_all(&envs)
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::SessionStart { resumed } => Some(resumed),
            _ => None,
        })
        .collect();
    assert_eq!(starts, [false, true]);
}

#[tokio::test]
async fn a_compound_command_reports_shell_success_without_a_test_result() {
    // failures: `python3 -m unittest ...` and `exit 3` fire PostToolUseFailure.
    let events = normalize_all(&scenario("failures"));
    let bash_finished: Vec<_> = events
        .iter()
        .filter(|e| e.tool.as_ref().is_some_and(|t| t.name == "Bash"))
        .filter_map(|e| match &e.kind {
            EventKind::ToolFinished { outcome } => Some((e.hook_event.as_str(), *outcome, &e.tool)),
            _ => None,
        })
        .collect();
    assert_eq!(bash_finished.len(), 2);
    for (hook, outcome, _) in &bash_finished {
        assert_eq!(*hook, "PostToolUseFailure");
        assert_eq!(*outcome, ToolOutcome::Failed);
    }
    // The unittest line is flagged as a test run, `exit 3` is plain shell.
    let classes: Vec<ToolClass> = bash_finished
        .iter()
        .map(|(_, _, t)| t.as_ref().unwrap().class)
        .collect();
    assert_eq!(classes, [ToolClass::TestRun, ToolClass::Shell]);

    // basic-main: `python3 -m unittest ... 2>&1; echo "exit=$?"` is a SUCCESSFUL
    // PostToolUse even though the test failed inside. Not inferred from text.
    let events = normalize_all(&scenario("basic-main"));
    let echo = events
        .iter()
        .find(|e| {
            e.hook_event == "PostToolUse" && e.text.as_deref().is_some_and(|t| t.contains("echo"))
        })
        .expect("the `; echo exit=$?` Bash post");
    assert_eq!(
        echo.kind,
        EventKind::ToolFinished {
            outcome: ToolOutcome::Ok
        }
    );
    assert_eq!(echo.tool.as_ref().unwrap().class, ToolClass::Shell);
}

#[tokio::test]
async fn harness_rejected_edits_are_recorded_as_failed_edits_with_the_file() {
    let timelines = replay(&scenario("failures")).await;
    let rejected: Vec<&TimelineEvent> = timelines[0]
        .turns
        .iter()
        .flat_map(|t| &t.events)
        .filter(|e| e.tool_name.as_deref() == Some("Edit"))
        .collect();
    // Two Edits, both rejected before running (no Pre/Post at all).
    assert_eq!(rejected.len(), 2);
    for e in rejected {
        assert_eq!(e.outcome, Some(ToolOutcome::Failed));
        assert_eq!(e.files, ["src/calc.py"]);
        assert!(
            e.summary.starts_with("rejected by Claude Code:"),
            "{}",
            e.summary
        );
    }
}

#[tokio::test]
async fn synthetic_task_notification_turn_is_flagged_and_subagent_events_are_grouped() {
    let envs = scenario("subagent");
    let events = normalize_all(&envs);
    let synthetic: Vec<_> = events
        .iter()
        .filter(|e| e.kind == EventKind::Prompt)
        .filter(|e| e.text.as_deref().is_some_and(is_synthetic_prompt))
        .collect();
    assert_eq!(synthetic.len(), 1, "exactly the task-notification prompt");

    let sub: Vec<_> = events.iter().filter(|e| e.agent_id.is_some()).collect();
    assert!(
        sub.len() >= 5,
        "start, pre/post Write, message, stop: {}",
        sub.len()
    );
    let ids: std::collections::HashSet<_> = sub.iter().map(|e| e.agent_id.clone()).collect();
    assert_eq!(ids.len(), 1, "one subagent");
    let stop = sub
        .iter()
        .find(|e| e.kind == EventKind::SubagentStopped)
        .unwrap();
    let stop_payload = &envs
        .iter()
        .find(|e| e.event == "SubagentStop")
        .unwrap()
        .payload;
    assert_eq!(
        stop.transcript_path.as_deref().unwrap(),
        stop_payload["agent_transcript_path"].as_str().unwrap()
    );
    let tool = sub
        .iter()
        .find(|e| e.kind == EventKind::ToolStarted)
        .unwrap();
    assert_eq!(
        tool.transcript_path.as_deref().unwrap(),
        stop_payload["transcript_path"].as_str().unwrap()
    );
    // Same session as the parent.
    let sessions: std::collections::HashSet<_> = events.iter().map(|e| e.session.clone()).collect();
    assert_eq!(sessions.len(), 1);

    // The subagent's Write shows up in the timeline carrying its agent id, and
    // the parent's Agent call does not.
    let timelines = replay(&envs).await;
    let all: Vec<&TimelineEvent> = timelines[0].turns.iter().flat_map(|t| &t.events).collect();
    let write = all
        .iter()
        .find(|e| e.tool_name.as_deref() == Some("Write"))
        .unwrap();
    assert!(write.agent_id.is_some());
    assert_eq!(write.files, ["src/sub.py"]);
    let agent_call = all
        .iter()
        .find(|e| e.tool_name.as_deref() == Some("Agent"))
        .unwrap();
    assert!(agent_call.agent_id.is_none());
}

#[tokio::test]
async fn write_create_vs_update_and_bash_edits_stay_invisible() {
    let events = normalize_all(&scenario("write-update-and-bash-edits"));
    let write_post = events
        .iter()
        .find(|e| {
            e.hook_event == "PostToolUse" && e.tool.as_ref().is_some_and(|t| t.name == "Write")
        })
        .unwrap();
    assert!(
        write_post.text.as_deref().unwrap().starts_with("update:"),
        "{:?}",
        write_post.text
    );
    assert!(
        write_post
            .text
            .as_deref()
            .unwrap()
            .contains("old content recorded")
    );

    let created = normalize_all(&scenario("basic-main"))
        .into_iter()
        .find(|e| {
            e.hook_event == "PostToolUse" && e.tool.as_ref().is_some_and(|t| t.name == "Write")
        })
        .unwrap();
    assert!(
        created.text.as_deref().unwrap().starts_with("create:"),
        "{:?}",
        created.text
    );
    assert!(
        !created
            .text
            .as_deref()
            .unwrap()
            .contains("old content recorded")
    );
    assert_eq!(created.files, ["/TMP/fixture-repo/src/greet.py"]);

    // `rm ... && printf > file` is a plain shell row that touches no files:
    // only git knows.
    let bash = events
        .iter()
        .find(|e| e.tool.as_ref().is_some_and(|t| t.name == "Bash"))
        .unwrap();
    assert!(bash.files.is_empty());
    assert_eq!(bash.tool.as_ref().unwrap().class, ToolClass::Shell);
}

#[tokio::test]
async fn parallel_tool_calls_pair_by_id_not_by_position() {
    // basic-main's second turn: Pre ToolSearch, Pre Read, Post Read, Post ToolSearch.
    let timelines = replay(&scenario("basic-main")).await;
    let turn = &timelines[0].turns[1];
    let search = turn
        .events
        .iter()
        .find(|e| e.tool_name.as_deref() == Some("ToolSearch"))
        .unwrap();
    let read = turn
        .events
        .iter()
        .find(|e| e.tool_name.as_deref() == Some("Read"))
        .unwrap();
    assert_eq!(search.outcome, Some(ToolOutcome::Ok));
    assert_eq!(read.outcome, Some(ToolOutcome::Ok));
    // One row per call: no duplicate rows from the interleaving.
    assert_eq!(
        turn.events.iter().filter(|e| e.tool_name.is_some()).count(),
        // ToolSearch, Read, the Edit the harness rejected (batch trace only),
        // the Edit that ran, Bash
        5
    );
}

#[tokio::test]
async fn post_before_pre_still_pairs() {
    // Deliveries from separate hook processes can arrive out of order.
    let call = |hook: &str, at: u64| {
        envelope(
            at,
            with(
                base(hook),
                serde_json::json!({
                    "tool_name": "Edit", "tool_use_id": "toolu_1",
                    "tool_input": {"file_path": "/TMP/fixture-repo/a.py"},
                    "tool_response": {"filePath": "/TMP/fixture-repo/a.py", "structuredPatch": [], "originalFile": "x"},
                }),
            ),
        )
    };
    let prompt = envelope(
        10,
        with(
            base("UserPromptSubmit"),
            serde_json::json!({"prompt": "go"}),
        ),
    );
    // Post arrives FIRST in the stream but carries the earlier time of the two.
    let envs = [prompt, call("PostToolUse", 20), call("PreToolUse", 21)];
    let timelines = replay(&envs).await;
    let tools: Vec<_> = timelines[0].turns[0]
        .events
        .iter()
        .filter(|e| e.tool_name.is_some())
        .collect();
    assert_eq!(tools.len(), 1, "paired into one row");
    assert_eq!(tools[0].outcome, Some(ToolOutcome::Ok));
    assert_eq!(tools[0].files, ["a.py"]);
}

#[tokio::test]
async fn files_are_relative_to_the_worktree_root_even_from_a_subdirectory() {
    // The fixture `subdir` scenario only reads; build an edit from `src/`.
    let mut env = envelope(
        5,
        with(
            base("PostToolUse"),
            serde_json::json!({
                "tool_name": "Write", "tool_use_id": "t1",
                "tool_input": {"file_path": "/TMP/fixture-repo/src/new.py", "content": "x"},
                "tool_response": {"type": "create", "filePath": "/TMP/fixture-repo/src/new.py",
                                   "content": "x", "structuredPatch": [], "originalFile": null},
            }),
        ),
    );
    env.cwd = "/TMP/fixture-repo/src".into();
    let tl = replay(&[env]).await;
    assert_eq!(tl[0].turns[0].events[0].files, ["src/new.py"]);

    // The linked-worktree scenario resolves to the worktree and its own root.
    let tl = replay(&scenario("worktree")).await;
    assert_eq!(
        tl[0].session.worktree_id.as_ref().map(|w| w.0.as_str()),
        Some("/TMP/fixture-wt")
    );
    let files: Vec<&String> = tl[0]
        .turns
        .iter()
        .flat_map(|t| &t.events)
        .flat_map(|e| &e.files)
        .collect();
    assert_eq!(files, ["src/wt_note.py"]);
}
