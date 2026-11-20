//! Builders for events in unit tests.

use serde_json::{Value, json};
use why_wire::{Envelope, Harness};

use super::model::{EventKind, NormalizedEvent, ToolCall, ToolClass};
use crate::contracts::{ToolOutcome, WorktreeId};

pub fn envelope(harness: Harness, event: &str, at_ms: u64, payload: Value) -> Envelope {
    Envelope {
        v: why_wire::PROTOCOL_VERSION,
        harness,
        harness_version: None,
        event: event.into(),
        received_at_ms: at_ms,
        cwd: "/repo".into(),
        pid: 1,
        payload,
    }
}

/// A Claude event in session `session`, turn `turn`, worktree `wt-1`. The key
/// is derived from a realistic envelope, like an adapter would.
pub fn event(
    session: &str,
    hook: &str,
    kind: EventKind,
    at_ms: u64,
    turn: Option<&str>,
) -> NormalizedEvent {
    let env = envelope(
        Harness::ClaudeCode,
        hook,
        at_ms,
        json!({ "session_id": session, "hook_event_name": hook, "prompt_id": turn }),
    );
    let mut ev = NormalizedEvent::from_envelope(&env, 0, session, kind);
    ev.turn_id = turn.map(str::to_owned);
    ev.worktree_id = Some(WorktreeId("wt-1".into()));
    ev
}

/// A tool start (`PreToolUse`) or finish (`PostToolUse`) for `use_id`.
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
        Harness::ClaudeCode,
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
