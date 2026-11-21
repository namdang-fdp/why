//! Which transcript file, and which ids, a lazy rationale read should use
//! (WHY-53). Pure: no I/O and no clock.
//!
//! The IPC commands in `ipc/rationale.rs` read a harness transcript only when
//! the user asks. To do that they need, from the stored events, three things:
//! the turn or tool call the user means, the `transcript_path` recorded for
//! THAT turn or call, and (for Codex shell outcomes) which shell calls still
//! have no outcome. Both [`EventStore`](super::EventStore) backends load a
//! session's events as [`SourceEvent`]s (no message text: transcript text is
//! never copied into WHY's database) and call the SAME functions here, so the
//! rule cannot differ between the memory and SQLite stores.
//!
//! # Which path
//!
//! A session can carry several `transcript_path` values (a resume starts a new
//! file, a file may have moved, a subagent has its own). The rule, in order:
//!
//! 1. A **turn** uses the events of that turn only, skipping lifecycle events.
//!    A `SubagentStopped` path belongs to the child, whose turn ID Codex
//!    reports. Claude reports the parent turn, so its stop path is skipped.
//!    Of those, the event with the **latest** `at_ms` that has a path wins; on
//!    equal times the later arrival wins.
//! 2. A **tool call** made by a subagent uses the path of that agent's
//!    `SubagentStopped` event when there is one (Claude's tool hooks report the
//!    PARENT transcript, but the subagent's own words live in its file), else
//!    the path on the call's own events. A main-agent call uses its own events.
//!
//! Paths only ever come from stored events, never from a command argument.

use std::collections::HashMap;

use why_wire::Harness;

use super::correlation::tool_event_class;
use super::model::{EventKind, NormalizedEvent, ToolClass};
use crate::contracts::ToolOutcome;

/// One stored event reduced to what a rationale read needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEvent {
    /// The event's dedupe key, also used to identify its timeline entry.
    pub key: String,
    pub turn_id: Option<String>,
    pub agent_id: Option<String>,
    /// Session lifecycle (`SessionStart` / `SessionEnd`): in no turn.
    pub lifecycle: bool,
    /// A subagent finished: its `transcript_path` is the subagent's file.
    pub subagent_stopped: bool,
    /// Set for OBSERVED tool starts and finishes only (the same rule as
    /// [`correlation::tool_event_class`](super::correlation)).
    pub tool: Option<SourceTool>,
    pub at_ms: u64,
    pub transcript_path: Option<String>,
}

impl SourceEvent {
    /// The projection of a full event (the memory store's path; SQLite fills
    /// the same fields from columns).
    pub fn from_event(ev: &NormalizedEvent) -> Self {
        Self {
            key: ev.key.0.clone(),
            turn_id: ev.turn_id.clone(),
            agent_id: ev.agent_id.clone(),
            lifecycle: ev.kind.is_lifecycle(),
            subagent_stopped: ev.kind == EventKind::SubagentStopped,
            tool: tool_event_class(ev).map(|class| SourceTool {
                use_id: ev.tool.as_ref().and_then(|t| t.use_id.clone()),
                class,
                finished: match ev.kind {
                    EventKind::ToolFinished { outcome } => Some(outcome),
                    _ => None,
                },
            }),
            at_ms: ev.at_ms,
            transcript_path: ev.transcript_path.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTool {
    pub use_id: Option<String>,
    pub class: ToolClass,
    /// `None` for a start, the outcome for a finish.
    pub finished: Option<ToolOutcome>,
}

/// Where a turn's words live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSource {
    pub transcript_path: Option<String>,
}

/// Where one tool call's words live, and the turn it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSource {
    pub tool_use_id: String,
    /// The turn that holds the call's timeline entry.
    pub turn_id: String,
    pub agent_id: Option<String>,
    pub transcript_path: Option<String>,
}

/// A shell or test-run call with no known outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownShellCall {
    pub use_id: String,
    /// The timeline entry's `id`, so the UI can join the result to its row.
    pub event_id: String,
    pub turn_id: String,
    pub agent_id: Option<String>,
    pub transcript_path: Option<String>,
}

/// The latest path (see the module docs) among `events`.
fn latest_path<'a>(events: impl Iterator<Item = &'a SourceEvent>) -> Option<String> {
    let mut best: Option<&SourceEvent> = None;
    for ev in events.filter(|e| e.transcript_path.is_some()) {
        // `>=`: on equal times the later arrival wins (events are in arrival order).
        if best.is_none_or(|b| ev.at_ms >= b.at_ms) {
            best = Some(ev);
        }
    }
    best.and_then(|e| e.transcript_path.clone())
}

/// `None` when no event of `turn_id` is stored (an unknown turn: the caller
/// reports `NotFound`). Otherwise the turn's transcript path, if any recorded.
pub fn locate_turn(events: &[SourceEvent], harness: Harness, turn_id: &str) -> Option<TurnSource> {
    let in_turn = || {
        events
            .iter()
            .filter(|e| !e.lifecycle && e.turn_id.as_deref() == Some(turn_id))
    };
    in_turn().next()?;
    Some(TurnSource {
        transcript_path: latest_path(
            in_turn().filter(|e| !e.subagent_stopped || harness == Harness::Codex),
        ),
    })
}

/// The timeline keeps the last start, or the first finish if the start was
/// lost. These tie rules match its stable sort of events in arrival order.
fn call_anchor<'a>(call: impl Iterator<Item = &'a SourceEvent> + Clone) -> Option<&'a SourceEvent> {
    call.clone()
        .filter(|e| e.tool.as_ref().is_some_and(|t| t.finished.is_none()))
        .max_by_key(|e| e.at_ms)
        .or_else(|| call.min_by_key(|e| e.at_ms))
}

/// Locate the observed call for a timeline entry with a harness tool ID.
pub fn locate_tool(events: &[SourceEvent], harness: Harness, event_id: &str) -> Option<ToolSource> {
    let picked = events.iter().find(|e| e.key == event_id)?;
    let use_id = picked.tool.as_ref()?.use_id.as_deref()?;
    let matching_id = || {
        events
            .iter()
            .filter(|e| e.tool.as_ref().and_then(|t| t.use_id.as_deref()) == Some(use_id))
    };
    let call = matching_id().filter(|e| e.agent_id == picked.agent_id);
    let anchor = call_anchor(call.clone())?;
    let own = latest_path(call);
    let transcript_path = match &picked.agent_id {
        Some(agent) => latest_path(
            events
                .iter()
                .filter(|e| e.subagent_stopped && e.agent_id.as_ref() == Some(agent)),
        )
        .or_else(|| {
            own.filter(|path| {
                // Claude's tool hooks can still point at the parent file. If
                // another agent used the same ID there, the anchor is ambiguous.
                harness != Harness::ClaudeCode
                    || !matching_id().any(|e| {
                        e.agent_id != picked.agent_id && e.transcript_path.as_ref() == Some(path)
                    })
            })
        }),
        None => own,
    };
    Some(ToolSource {
        tool_use_id: use_id.to_owned(),
        turn_id: anchor.turn_id.clone().unwrap_or_default(),
        agent_id: picked.agent_id.clone(),
        transcript_path,
    })
}

/// Shell and test-run calls whose outcome is `unknown` right now: no finish was
/// seen, or the latest finish said `unknown` (Codex's hooks never carry an exit
/// code). Pairing and "latest finish wins" follow the timeline's rule
/// (`correlation::fold_calls` pairs the same way), so a call the timeline shows as `ok` or `failed` is
/// never listed. Calls without a `tool_use_id` cannot be matched to a rollout
/// record and are skipped. Oldest first.
pub fn unknown_shell_calls(events: &[SourceEvent]) -> Vec<UnknownShellCall> {
    #[derive(Default)]
    struct Acc<'a> {
        finish: Option<(u64, ToolOutcome)>,
        events: Vec<&'a SourceEvent>,
    }
    let mut order = Vec::new();
    let mut calls: HashMap<_, Acc<'_>> = HashMap::new();
    for ev in events {
        let Some(tool) = &ev.tool else { continue };
        if !matches!(tool.class, ToolClass::Shell | ToolClass::TestRun) {
            continue;
        }
        let Some(use_id) = tool.use_id.as_deref() else {
            continue;
        };
        let key = (ev.agent_id.as_deref(), use_id);
        let acc = calls.entry(key).or_insert_with(|| {
            order.push(key);
            Acc::default()
        });
        acc.events.push(ev);
        if let Some(outcome) = tool.finished
            && acc.finish.is_none_or(|(at, _)| ev.at_ms >= at)
        {
            acc.finish = Some((ev.at_ms, outcome));
        }
    }

    order
        .into_iter()
        .filter_map(|key| {
            let acc = calls.remove(&key)?;
            if matches!(acc.finish, Some((_, ToolOutcome::Ok | ToolOutcome::Failed))) {
                return None;
            }
            let call = acc.events.iter().copied();
            let anchor = call_anchor(call.clone())?;
            Some(UnknownShellCall {
                use_id: key.1.to_owned(),
                event_id: anchor.key.clone(),
                turn_id: anchor.turn_id.clone().unwrap_or_default(),
                agent_id: anchor.agent_id.clone(),
                transcript_path: latest_path(call),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(key: &str, turn: Option<&str>, at: u64, path: Option<&str>) -> SourceEvent {
        SourceEvent {
            key: key.into(),
            turn_id: turn.map(str::to_owned),
            agent_id: None,
            lifecycle: false,
            subagent_stopped: false,
            tool: None,
            at_ms: at,
            transcript_path: path.map(str::to_owned),
        }
    }

    fn tool_ev(
        key: &str,
        turn: &str,
        at: u64,
        id: &str,
        class: ToolClass,
        finished: Option<ToolOutcome>,
        path: Option<&str>,
    ) -> SourceEvent {
        SourceEvent {
            tool: Some(SourceTool {
                use_id: Some(id.into()),
                class,
                finished,
            }),
            ..ev(key, Some(turn), at, path)
        }
    }

    #[test]
    fn a_turn_uses_its_latest_path_and_ignores_lifecycle_and_subagent_stops() {
        let mut stop = ev("sub", Some("t1"), 50, Some("/sub.jsonl"));
        stop.subagent_stopped = true;
        stop.agent_id = Some("a".into());
        let mut start = ev("start", None, 1, Some("/lifecycle.jsonl"));
        start.lifecycle = true;
        let events = [
            start,
            ev("p", Some("t1"), 10, Some("/old.jsonl")),
            ev("q", Some("t1"), 20, Some("/new.jsonl")),
            ev("r", Some("t1"), 30, None),
            stop,
            ev("other", Some("t2"), 99, Some("/other.jsonl")),
        ];
        let found = locate_turn(&events, Harness::ClaudeCode, "t1").unwrap();
        assert_eq!(found.transcript_path.as_deref(), Some("/new.jsonl"));
        assert_eq!(locate_turn(&events, Harness::ClaudeCode, "nope"), None);
        // Only a lifecycle event: the turn does not exist.
        assert_eq!(locate_turn(&events, Harness::ClaudeCode, ""), None);
    }

    #[test]
    fn a_subagent_tool_call_uses_the_subagents_own_file() {
        let mut call = tool_ev(
            "c",
            "t1",
            5,
            "u1",
            ToolClass::Edit,
            None,
            Some("/parent.jsonl"),
        );
        call.agent_id = Some("a".into());
        let mut stop = ev("s", Some("t1"), 9, Some("/agent.jsonl"));
        stop.subagent_stopped = true;
        stop.agent_id = Some("a".into());
        let found = locate_tool(&[call, stop], Harness::ClaudeCode, "c").unwrap();
        assert_eq!(found.transcript_path.as_deref(), Some("/agent.jsonl"));
        assert_eq!(found.agent_id.as_deref(), Some("a"));
    }

    #[test]
    fn only_calls_without_a_known_outcome_are_listed() {
        let sh = ToolClass::Shell;
        let events = [
            tool_ev("a1", "t1", 1, "unk", sh, None, Some("/r.jsonl")),
            tool_ev(
                "a2",
                "t1",
                2,
                "unk",
                sh,
                Some(ToolOutcome::Unknown),
                Some("/r.jsonl"),
            ),
            tool_ev("b1", "t1", 3, "ok", sh, None, Some("/r.jsonl")),
            tool_ev(
                "b2",
                "t1",
                4,
                "ok",
                sh,
                Some(ToolOutcome::Ok),
                Some("/r.jsonl"),
            ),
            tool_ev("c1", "t2", 5, "lost", ToolClass::TestRun, None, None),
            tool_ev(
                "d1",
                "t2",
                6,
                "edit",
                ToolClass::Edit,
                None,
                Some("/r.jsonl"),
            ),
        ];
        let calls = unknown_shell_calls(&events);
        let ids: Vec<_> = calls.iter().map(|c| c.use_id.as_str()).collect();
        assert_eq!(ids, ["unk", "lost"]);
        assert_eq!(calls[0].event_id, "a1");
        assert_eq!(calls[0].transcript_path.as_deref(), Some("/r.jsonl"));
        assert_eq!(calls[1].transcript_path, None);
    }
}
