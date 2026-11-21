//! Files and ids that must never hurt: gone, escaping, special or huge
//! transcripts, ids that are not ours, and which `transcript_path` is used.

use std::io::Write;
use std::os::unix::ffi::OsStrExt;

use serde_json::json;
use why_lib::contracts::{
    IpcError, RationaleResult, ShellOutcomesResult, ToolOutcome, TranscriptUnavailable,
};
use why_lib::events::{EventKind, NormalizedEvent};
use why_lib::ipc::rationale::{
    RationaleRoots, get_shell_outcomes_impl, get_tool_rationale_impl, get_turn_rationale_impl,
};
use why_wire::Harness;

use crate::codex::{agent_message, command, meta};
use crate::common::*;

fn unavailable(reason: TranscriptUnavailable) -> RationaleResult {
    RationaleResult::Unavailable { reason }
}

async fn claude_turn(events: &[NormalizedEvent], rig: &Rig) -> RationaleResult {
    on_both(events, |state| {
        let (roots, sid) = (rig.roots.clone(), session_id(Harness::ClaudeCode, "cs"));
        async move {
            get_turn_rationale_impl(&state, &roots, &sid, "p1")
                .await
                .unwrap()
        }
    })
    .await
}

async fn codex_turn(events: &[NormalizedEvent], rig: &Rig, turn: &'static str) -> RationaleResult {
    on_both(events, |state| {
        let (roots, sid) = (rig.roots.clone(), session_id(Harness::Codex, "cs"));
        async move {
            get_turn_rationale_impl(&state, &roots, &sid, turn)
                .await
                .unwrap()
        }
    })
    .await
}

fn claude_events(path: Option<&str>) -> Vec<NormalizedEvent> {
    vec![prompt(Harness::ClaudeCode, "cs", "p1", 1_000, path)]
}

fn codex_events(path: Option<&str>) -> Vec<NormalizedEvent> {
    vec![prompt(Harness::Codex, "cs", "t1", 1_000, path)]
}

#[tokio::test]
async fn no_recorded_path_is_no_transcript() {
    let rig = Rig::new();
    assert_eq!(
        claude_turn(&claude_events(None), &rig).await,
        unavailable(TranscriptUnavailable::NoTranscript)
    );
    assert_eq!(
        codex_turn(&codex_events(None), &rig, "t1").await,
        unavailable(TranscriptUnavailable::NoTranscript)
    );
}

#[tokio::test]
async fn a_transcript_deleted_after_the_events_were_recorded_is_missing() {
    let rig = Rig::new();
    let claude = rig.claude_file("gone.jsonl", b"{}\n");
    let codex = rig.codex_file("gone.jsonl", b"{}\n");
    let (c, x) = (
        claude.to_str().unwrap().to_owned(),
        codex.to_str().unwrap().to_owned(),
    );
    std::fs::remove_file(&claude).unwrap();
    std::fs::remove_file(&codex).unwrap();
    assert_eq!(
        claude_turn(&claude_events(Some(&c)), &rig).await,
        unavailable(TranscriptUnavailable::Missing)
    );
    assert_eq!(
        codex_turn(&codex_events(Some(&x)), &rig, "t1").await,
        unavailable(TranscriptUnavailable::Missing)
    );
}

#[tokio::test]
async fn a_stored_path_outside_the_root_or_through_a_symlink_is_refused() {
    let rig = Rig::new();
    // A file that is NOT under either harness directory.
    let outside = rig.dir.path().join("secret.jsonl");
    std::fs::write(
        &outside,
        [meta("0.170.0"), agent_message("t1", "SECRET")].concat(),
    )
    .unwrap();
    let o = outside.to_str().unwrap();
    assert_eq!(
        claude_turn(&claude_events(Some(o)), &rig).await,
        unavailable(TranscriptUnavailable::OutsideRoot)
    );
    assert_eq!(
        codex_turn(&codex_events(Some(o)), &rig, "t1").await,
        unavailable(TranscriptUnavailable::OutsideRoot)
    );

    // A symlink INSIDE the root that points out of it.
    let c_link = rig.claude_root().join("-p/escape.jsonl");
    std::fs::create_dir_all(c_link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &c_link).unwrap();
    let x_link = rig.codex_root().join("escape.jsonl");
    std::os::unix::fs::symlink(&outside, &x_link).unwrap();
    assert_eq!(
        claude_turn(&claude_events(c_link.to_str()), &rig).await,
        unavailable(TranscriptUnavailable::OutsideRoot)
    );
    assert_eq!(
        codex_turn(&codex_events(x_link.to_str()), &rig, "t1").await,
        unavailable(TranscriptUnavailable::OutsideRoot)
    );

    // `..` cannot climb out either.
    let sneaky = format!("{}/-p/../../secret.jsonl", rig.claude_root().display());
    assert_eq!(
        claude_turn(&claude_events(Some(&sneaky)), &rig).await,
        unavailable(TranscriptUnavailable::OutsideRoot)
    );
}

fn mkfifo(path: &std::path::Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

#[tokio::test]
async fn a_fifo_is_refused_promptly_not_waited_on() {
    let rig = Rig::new();
    let c = rig.claude_root().join("-p/pipe.jsonl");
    std::fs::create_dir_all(c.parent().unwrap()).unwrap();
    mkfifo(&c);
    let x = rig.codex_root().join("pipe.jsonl");
    mkfifo(&x);
    let started = std::time::Instant::now();
    assert_eq!(
        claude_turn(&claude_events(c.to_str()), &rig).await,
        unavailable(TranscriptUnavailable::Unreadable)
    );
    assert_eq!(
        codex_turn(&codex_events(x.to_str()), &rig, "t1").await,
        unavailable(TranscriptUnavailable::Unreadable)
    );
    assert!(
        started.elapsed().as_secs() < 5,
        "a FIFO must not block the read"
    );
}

#[tokio::test]
async fn a_missing_harness_directory_is_unreadable_not_a_crash() {
    let rig = Rig::new();
    let none = RationaleRoots::default();
    let events = claude_events(Some("/x/y.jsonl"));
    let sid = session_id(Harness::ClaudeCode, "cs");
    let result = on_both(&events, |state| {
        let (roots, sid) = (none.clone(), sid.clone());
        async move {
            get_turn_rationale_impl(&state, &roots, &sid, "p1")
                .await
                .unwrap()
        }
    })
    .await;
    assert_eq!(result, unavailable(TranscriptUnavailable::Unreadable));
    let _ = rig;
}

/// A line of valid JSON of an unknown record type, `size` bytes long.
fn junk(size: usize) -> String {
    format!(
        "{}\n",
        json!({"type": "progress", "pad": "x".repeat(size - 40)})
    )
}

fn write_junk(file: &mut std::fs::File, total: usize) {
    let line = junk(60_000);
    for _ in 0..total / line.len() {
        file.write_all(line.as_bytes()).unwrap();
    }
}

#[tokio::test]
async fn a_50_mb_claude_transcript_is_read_within_its_budget() {
    let rig = Rig::new();
    let path = rig.claude_root().join("-p/big.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = std::fs::File::create(&path).unwrap();
    // The turn's prompt and one reply come first...
    writeln!(
        file,
        "{}",
        json!({"type":"user","promptId":"p1","version":"2.1.322","message":{"content":[]}})
    )
    .unwrap();
    writeln!(file, "{}", json!({"type":"assistant","uuid":"a1","message":{"content":[{"type":"text","text":"early words"}]}})).unwrap();
    // ...then 50 MB of other records, then a tool call far beyond the budget.
    write_junk(&mut file, 50 * 1024 * 1024);
    writeln!(file, "{}", json!({"type":"assistant","uuid":"a9","message":{"content":[{"type":"text","text":"late"},{"type":"tool_use","id":"toolu_LATE","name":"Bash","input":{}}]}})).unwrap();
    drop(file);
    let p = path.to_str().unwrap();
    let mut events = claude_events(Some(p));
    events.extend(shell_call(
        Harness::ClaudeCode,
        "cs",
        "p1",
        "toolu_LATE",
        None,
        2_000,
        Some(p),
    ));

    // The turn is found early; the read stops at its budget and says so.
    let started = std::time::Instant::now();
    let turn = claude_turn(&events, &rig).await;
    let RationaleResult::Ok { rationale } = &turn else {
        panic!("{turn:?}");
    };
    assert_eq!(texts(&turn), ["early words"]);
    assert!(rationale.truncated);

    // The call is beyond the budget: "not found YET" (ok, empty, truncated),
    // never a definite `notFound`.
    let event_id = tool_event_id(&events, "toolu_LATE");
    let tool = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), session_id(Harness::ClaudeCode, "cs"));
        async move {
            get_tool_rationale_impl(&state, &roots, &sid, event_id)
                .await
                .unwrap()
        }
    })
    .await;
    let RationaleResult::Ok { rationale } = &tool else {
        panic!("{tool:?}");
    };
    assert!(
        rationale.blocks.is_empty() && rationale.truncated,
        "{rationale:?}"
    );
    assert!(started.elapsed().as_secs() < 30);
}

#[tokio::test]
async fn a_codex_rollout_bigger_than_the_window_reports_truncation_and_never_guesses() {
    let rig = Rig::new();
    let path = rig.codex_root().join("2026/11/20/big.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(meta("0.170.0").as_bytes()).unwrap();
    file.write_all(command("t1", "exec-head", Some(1)).as_bytes())
        .unwrap();
    file.write_all(agent_message("t1", "head words").as_bytes())
        .unwrap();
    write_junk(&mut file, 20 * 1024 * 1024);
    file.write_all(command("t2", "exec-tail", Some(0)).as_bytes())
        .unwrap();
    drop(file);
    let p = path.to_str().unwrap();
    let mut events = vec![
        prompt(Harness::Codex, "cs", "t1", 500, Some(p)),
        prompt(Harness::Codex, "cs", "t2", 900, Some(p)),
    ];
    events.extend(shell_call(
        Harness::Codex,
        "cs",
        "t1",
        "exec-head",
        Some(ToolOutcome::Unknown),
        1_000,
        Some(p),
    ));
    events.extend(shell_call(
        Harness::Codex,
        "cs",
        "t2",
        "exec-tail",
        Some(ToolOutcome::Unknown),
        1_100,
        Some(p),
    ));

    let sid = session_id(Harness::Codex, "cs");
    let outcomes = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move { get_shell_outcomes_impl(&state, &roots, &sid).await.unwrap() }
    })
    .await;
    let ShellOutcomesResult::Ok { result } = outcomes else {
        panic!("{outcomes:?}");
    };
    // Found in the tail window: upgraded. In the skipped middle/head: left out.
    assert_eq!(
        result
            .outcomes
            .iter()
            .map(|o| o.tool_use_id.as_str())
            .collect::<Vec<_>>(),
        ["exec-tail"]
    );
    assert_eq!(result.unresolved, 1);
    assert!(result.truncated);
    assert!(result.version_supported, "the version comes from the head");

    // t1's words are before the window: "not found yet", not "silent".
    let turn = codex_turn(&events, &rig, "t1").await;
    let RationaleResult::Ok { rationale } = &turn else {
        panic!("{turn:?}");
    };
    assert!(rationale.blocks.is_empty() && rationale.truncated);
}

#[tokio::test]
async fn ids_that_are_not_ours_are_errors_and_unknown_ones_are_not_found() {
    let rig = Rig::new();
    let events = claude_events(None);
    let all = stores(&events);
    for (label, state) in &all.states {
        let garbage = [
            why_lib::contracts::SessionId("nonsense".into()),
            why_lib::contracts::SessionId("gemini:x".into()),
            why_lib::contracts::SessionId("codex:".into()),
        ];
        for id in &garbage {
            let e = get_turn_rationale_impl(state, &rig.roots, id, "p1")
                .await
                .unwrap_err();
            assert!(invalid_input(&e), "{label}: {e:?}");
            let e = get_tool_rationale_impl(state, &rig.roots, id, "u")
                .await
                .unwrap_err();
            assert!(invalid_input(&e), "{label}: {e:?}");
            let e = get_shell_outcomes_impl(state, &rig.roots, id)
                .await
                .unwrap_err();
            assert!(invalid_input(&e), "{label}: {e:?}");
        }
        // Valid shape, no such session.
        let absent = session_id(Harness::Codex, "never-seen");
        assert_eq!(
            get_turn_rationale_impl(state, &rig.roots, &absent, "p1")
                .await
                .unwrap_err(),
            IpcError::NotFound
        );
        assert_eq!(
            get_shell_outcomes_impl(state, &rig.roots, &absent)
                .await
                .unwrap_err(),
            IpcError::NotFound
        );
        // Real session, no such turn or tool call, or an empty id.
        let sid = session_id(Harness::ClaudeCode, "cs");
        assert_eq!(
            get_turn_rationale_impl(state, &rig.roots, &sid, "p-none")
                .await
                .unwrap_err(),
            IpcError::NotFound
        );
        assert_eq!(
            get_tool_rationale_impl(state, &rig.roots, &sid, "t-none")
                .await
                .unwrap_err(),
            IpcError::NotFound
        );
        let e = get_turn_rationale_impl(state, &rig.roots, &sid, "")
            .await
            .unwrap_err();
        assert!(invalid_input(&e), "{label}: {e:?}");
        let e = get_tool_rationale_impl(state, &rig.roots, &sid, "")
            .await
            .unwrap_err();
        assert!(invalid_input(&e), "{label}: {e:?}");
    }
}

#[tokio::test]
async fn a_harness_without_a_reader_is_unsupported_not_an_error() {
    let rig = Rig::new();
    let events = vec![prompt(Harness::Unknown, "cs", "t1", 1_000, Some("/x"))];
    let sid = session_id(Harness::Unknown, "cs");
    let turn = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move {
            get_turn_rationale_impl(&state, &roots, &sid, "t1")
                .await
                .unwrap()
        }
    })
    .await;
    assert_eq!(turn, unavailable(TranscriptUnavailable::UnsupportedHarness));
    let shell = on_both(&events, |state| {
        let (roots, sid) = (rig.roots.clone(), sid.clone());
        async move { get_shell_outcomes_impl(&state, &roots, &sid).await.unwrap() }
    })
    .await;
    assert_eq!(
        shell,
        ShellOutcomesResult::Unavailable {
            reason: TranscriptUnavailable::UnsupportedHarness
        }
    );
}

/// A resumed session has several rollout files. Each turn is read from the file
/// recorded on ITS events, and when one turn's events name two files the latest
/// wins. The Codex subagent's stop names its own turn and rollout.
#[tokio::test]
async fn each_turn_uses_the_path_recorded_on_its_own_events() {
    let rig = Rig::new();
    let old = rig.codex_file(
        "old.jsonl",
        [
            meta("0.170.0"),
            agent_message("t1", "from the old file"),
            agent_message("t2", "stale t2"),
        ]
        .concat()
        .as_bytes(),
    );
    let new = rig.codex_file(
        "new.jsonl",
        [meta("0.170.0"), agent_message("t2", "from the new file")]
            .concat()
            .as_bytes(),
    );
    let child = rig.codex_file(
        "child.jsonl",
        [
            meta("0.170.0"),
            agent_message("child-turn", "subagent file"),
        ]
        .concat()
        .as_bytes(),
    );
    let mut stop = Ev::new(
        Harness::Codex,
        "cs",
        "SubagentStop",
        EventKind::SubagentStopped,
    )
    .at(9_000)
    .turn("child-turn")
    .path(child.to_str().unwrap());
    stop.agent = Some("agent-1");
    let events = vec![
        prompt(Harness::Codex, "cs", "t1", 1_000, old.to_str()),
        // t2 started in the old file, then the session moved to the new one.
        prompt(Harness::Codex, "cs", "t2", 2_000, old.to_str()),
        Ev::new(Harness::Codex, "cs", "Stop", EventKind::Stop)
            .at(3_000)
            .turn("t2")
            .path(new.to_str().unwrap())
            .build(),
        stop.build(),
    ];
    let t1 = codex_turn(&events, &rig, "t1").await;
    assert_eq!(texts(&t1), ["from the old file"]);
    let t2 = codex_turn(&events, &rig, "t2").await;
    assert_eq!(texts(&t2), ["from the new file"]);
}

/// The SQLite projection and the memory projection must be the same rows: the
/// selection rules are shared, so a difference here would be a store bug.
#[test]
fn both_stores_project_the_same_source_events() {
    let mut events = replay(&codex_fixture("subagent").join("hooks.jsonl"));
    events.extend(replay(&claude_fixture("subagent").join("hooks.jsonl")));
    let all = stores(&events);
    let mut keys = std::collections::BTreeSet::new();
    for ev in &events {
        keys.insert(ev.session.clone());
    }
    for key in &keys {
        let rows: Vec<_> = all
            .states
            .iter()
            .map(|(_, s)| s.events.source_events(key).unwrap().unwrap())
            .collect();
        assert!(!rows[0].is_empty());
        assert_eq!(rows[0], rows[1], "{key:?}");
    }
    let absent = why_lib::events::SessionKey::new(Harness::Codex, "absent");
    for (label, state) in &all.states {
        assert_eq!(
            state.events.source_events(&absent).unwrap(),
            None,
            "{label}"
        );
    }
}
