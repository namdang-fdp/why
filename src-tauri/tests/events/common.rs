//! Event builders through the PUBLIC API only (the in-crate `test_support` is
//! `cfg(test)` of the library and invisible from here).

use serde_json::json;
use why_lib::contracts::{ToolOutcome, WorktreeId};
use why_lib::events::{EventKey, EventKind, NormalizedEvent, ToolCall, ToolClass};
use why_wire::{Envelope, Harness, PROTOCOL_VERSION};

pub fn envelope(hook: &str, at_ms: u64, payload: serde_json::Value) -> Envelope {
    Envelope {
        v: PROTOCOL_VERSION,
        harness: Harness::ClaudeCode,
        harness_version: None,
        event: hook.into(),
        received_at_ms: at_ms,
        cwd: "/repo".into(),
        pid: 1,
        payload,
    }
}

/// A Claude event in `session`, turn `turn`, worktree `wt-1`; the key is derived
/// from a realistic envelope, like an adapter would.
pub fn event(
    session: &str,
    hook: &str,
    kind: EventKind,
    at_ms: u64,
    turn: Option<&str>,
) -> NormalizedEvent {
    let env = envelope(
        hook,
        at_ms,
        json!({ "session_id": session, "hook_event_name": hook, "prompt_id": turn }),
    );
    let mut ev = NormalizedEvent::from_envelope(&env, 0, session, kind);
    ev.turn_id = turn.map(str::to_owned);
    ev.worktree_id = Some(WorktreeId("wt-1".into()));
    ev
}

pub fn start(session: &str, at: u64, resumed: bool) -> NormalizedEvent {
    event(
        session,
        "SessionStart",
        EventKind::SessionStart { resumed },
        at,
        None,
    )
}

pub fn end(session: &str, at: u64, turn: &str) -> NormalizedEvent {
    event(session, "SessionEnd", EventKind::SessionEnd, at, Some(turn))
}

pub fn prompt(session: &str, at: u64, turn: &str, text: &str) -> NormalizedEvent {
    let mut e = event(
        session,
        "UserPromptSubmit",
        EventKind::Prompt,
        at,
        Some(turn),
    );
    e.text = Some(text.into());
    e
}

pub fn stop(session: &str, at: u64, turn: &str) -> NormalizedEvent {
    event(session, "Stop", EventKind::Stop, at, Some(turn))
}

/// `PreToolUse` (`finished: None`) or `PostToolUse` for `use_id`.
pub fn tool(
    session: &str,
    use_id: &str,
    finished: Option<ToolOutcome>,
    at_ms: u64,
    turn: &str,
) -> NormalizedEvent {
    let (hook, kind) = match finished {
        None => ("PreToolUse", EventKind::ToolStarted),
        Some(outcome) => ("PostToolUse", EventKind::ToolFinished { outcome }),
    };
    let env = envelope(
        hook,
        at_ms,
        json!({ "session_id": session, "hook_event_name": hook, "prompt_id": turn, "tool_use_id": use_id }),
    );
    let mut ev = NormalizedEvent::from_envelope(&env, 0, session, kind);
    ev.turn_id = Some(turn.into());
    ev.worktree_id = Some(WorktreeId("wt-1".into()));
    ev.tool = Some(ToolCall {
        use_id: Some(use_id.into()),
        name: "Bash".into(),
        class: ToolClass::Shell,
    });
    ev
}

/// Give `ev` a unique dedupe key (bulk tests make thousands of otherwise
/// identical events).
pub fn with_key(mut ev: NormalizedEvent, n: u64) -> NormalizedEvent {
    ev.key = EventKey(format!("bulk-{n}"));
    ev
}

pub fn wt(id: &str) -> WorktreeId {
    WorktreeId(id.into())
}
