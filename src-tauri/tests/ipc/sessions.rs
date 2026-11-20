//! `list_sessions` / `get_session_timeline` against the in-memory event store.
//! No git needed: the commands only read the `EventStore`.

use std::sync::Arc;

use serde_json::json;
use why_lib::contracts::{IpcError, SessionId, SessionStatus, ToolOutcome, WorktreeId};
use why_lib::events::{
    EventKind, EventStore, MemoryEventStore, NormalizedEvent, ToolCall, ToolClass,
};
use why_lib::ipc::sessions::{get_session_timeline_impl, list_sessions_impl};
use why_lib::state::AppState;
use why_lib::store::MemoryRepoStore;
use why_wire::{Envelope, Harness, PROTOCOL_VERSION};

use crate::common::test_runner;

fn state_with(events: &[NormalizedEvent]) -> AppState {
    let store = Arc::new(MemoryEventStore::new());
    store.append(events).unwrap();
    AppState::new(test_runner(), Arc::new(MemoryRepoStore::new())).with_events(store)
}

/// A Codex event as an adapter would build it, already resolved to `wt`.
fn codex(hook: &str, kind: EventKind, at_ms: u64, turn: Option<&str>, wt: &str) -> NormalizedEvent {
    let env = Envelope {
        v: PROTOCOL_VERSION,
        harness: Harness::Codex,
        harness_version: None,
        event: hook.into(),
        received_at_ms: at_ms,
        cwd: "/repo".into(),
        pid: 1,
        payload: json!({ "session_id": "s1", "turn_id": turn, "hook_event_name": hook }),
    };
    let mut ev = NormalizedEvent::from_envelope(&env, 0, "s1", kind);
    ev.turn_id = turn.map(str::to_owned);
    ev.worktree_id = Some(WorktreeId(wt.into()));
    ev.model = Some("gpt-6-luna".into());
    ev
}

fn session_events() -> Vec<NormalizedEvent> {
    let mut prompt = codex(
        "UserPromptSubmit",
        EventKind::Prompt,
        2_000,
        Some("t1"),
        "wt-a",
    );
    prompt.text = Some("create notes.py".into());
    let mut patch = codex(
        "PreToolUse",
        EventKind::ToolStarted,
        3_000,
        Some("t1"),
        "wt-a",
    );
    patch.tool = Some(ToolCall {
        use_id: Some("call_1".into()),
        name: "apply_patch".into(),
        class: ToolClass::Edit,
    });
    patch.files = vec!["src/notes.py".into()];
    vec![
        codex(
            "SessionStart",
            EventKind::SessionStart { resumed: false },
            1_000,
            None,
            "wt-a",
        ),
        prompt,
        patch,
        codex("Stop", EventKind::Stop, 4_000, Some("t1"), "wt-a"),
    ]
}

#[tokio::test]
async fn list_sessions_returns_the_worktrees_sessions() {
    let state = state_with(&session_events());
    let sessions = list_sessions_impl(&state, &WorktreeId("wt-a".into()))
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.id, SessionId("codex:s1".into()));
    assert_eq!(s.model.as_deref(), Some("gpt-6-luna"));
    assert_eq!(s.task.as_deref(), Some("create notes.py"));
    assert_eq!(s.status, SessionStatus::Live);
    assert_eq!(s.turn_count, 1);
    assert_eq!(s.started_at, "1970-01-01T00:00:01.000Z");

    let none = list_sessions_impl(&state, &WorktreeId("wt-b".into()))
        .await
        .unwrap();
    assert!(
        none.is_empty(),
        "an unknown worktree has no sessions, not an error"
    );
}

#[tokio::test]
async fn timeline_round_trips_the_listed_id() {
    let state = state_with(&session_events());
    let listed = list_sessions_impl(&state, &WorktreeId("wt-a".into()))
        .await
        .unwrap();
    let timeline = get_session_timeline_impl(&state, &listed[0].id)
        .await
        .unwrap();
    assert_eq!(timeline.session, listed[0]);
    let turn = &timeline.turns[0];
    assert_eq!(turn.prompt.as_deref(), Some("create notes.py"));
    assert_eq!(turn.ended_at.as_deref(), Some("1970-01-01T00:00:04.000Z"));
    let edit = turn
        .events
        .iter()
        .find(|e| e.tool_name.as_deref() == Some("apply_patch"))
        .unwrap();
    assert_eq!(edit.files, ["src/notes.py"]);
    // The Pre arrived, the Post never did: unknown, not "rejected".
    assert_eq!(edit.outcome, Some(ToolOutcome::Unknown));
}

#[tokio::test]
async fn unknown_and_malformed_session_ids() {
    let state = state_with(&session_events());
    assert_eq!(
        get_session_timeline_impl(&state, &SessionId("codex:nope".into())).await,
        Err(IpcError::NotFound)
    );
    for bad in ["", "s1", "gemini:s1", "codex:"] {
        let err = get_session_timeline_impl(&state, &SessionId(bad.into()))
            .await
            .unwrap_err();
        assert!(
            matches!(err, IpcError::InvalidInput { .. }),
            "{bad:?}: {err:?}"
        );
    }
}
