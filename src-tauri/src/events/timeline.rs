//! Turning a session's stored events into the UI's [`SessionTimeline`].
//!
//! Pure functions, shared by every [`EventStore`](super::EventStore) backend:
//! the memory store calls them directly, and the SQLite store (WHY-12) can load
//! a session's events and call them too, so the pairing and turn rules live in
//! one place.
//!
//! Rules:
//! - Events are ordered by `at_ms`; ties keep arrival order (callers pass
//!   events in arrival order, and the sort is stable).
//! - A tool call's start and finish are merged into ONE entry by
//!   `(agent_id, tool_use_id)`, whichever arrived first. No finish means outcome
//!   `Unknown` (never "rejected": the result may just not have been delivered).
//! - Lifecycle events (`SessionStart`/`SessionEnd`) set the status and are not
//!   shown in turns. The last one in time wins: start/resume → `live`, end →
//!   `ended`; none at all → `unknown`.
//! - Stale sessions: a crashed agent never sends `SessionEnd`, so `live` would
//!   last forever. A `live` session whose last event is more than
//!   [`STALE_AFTER_MS`] before "now" is shown as `unknown` ([`effective_status`]).
//!   "Now" is an argument: this module never reads the clock.
//! - Turns are keyed by `turn_id` and ordered by their first event. Events with
//!   no turn id (none seen in the WHY-45 fixtures except lifecycle ones) go to a
//!   turn with an empty id.

use std::collections::HashMap;

use chrono::{DateTime, SecondsFormat};

use super::model::{EventKind, NormalizedEvent, SessionKey, ToolClass};
use crate::contracts::{
    Harness as ContractHarness, SessionStatus, SessionSummary, SessionTimeline, TimelineEvent,
    TimelineEventKind, ToolOutcome, Turn,
};

/// `u64` milliseconds since the epoch as RFC 3339 UTC with milliseconds
/// (`2026-11-20T15:28:47.123Z`). Out-of-range values (beyond year 262143)
/// cannot come from a real clock; they are clamped to the epoch.
pub fn rfc3339_from_ms(ms: u64) -> String {
    let dt = i64::try_from(ms)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
        .unwrap_or_default();
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn harness_to_contract(h: why_wire::Harness) -> ContractHarness {
    match h {
        why_wire::Harness::ClaudeCode => ContractHarness::ClaudeCode,
        why_wire::Harness::Codex => ContractHarness::Codex,
        why_wire::Harness::Unknown => ContractHarness::Unknown,
    }
}

/// A session with no `SessionEnd` and no event for this long is no longer
/// called `live`: its agent most likely crashed or was killed.
pub const STALE_AFTER_MS: u64 = 30 * 60 * 1000;

/// The status to show: `status` (from the lifecycle events), except that `live`
/// becomes `unknown` once `last_event_ms` is more than [`STALE_AFTER_MS`] before
/// `now_ms`. Only `live` is demoted: `ended` is a fact the harness told us, and
/// `unknown` cannot get less certain. `saturating_sub`: an event stamped in the
/// future (clock skew) is "just now", never stale.
pub fn effective_status(status: SessionStatus, last_event_ms: u64, now_ms: u64) -> SessionStatus {
    if status == SessionStatus::Live && now_ms.saturating_sub(last_event_ms) > STALE_AFTER_MS {
        SessionStatus::Unknown
    } else {
        status
    }
}

/// One timeline entry while it is being assembled.
struct Entry {
    at_ms: u64,
    arrival_index: usize,
    turn: String,
    event: TimelineEvent,
}

fn kind_for_tool(class: ToolClass) -> TimelineEventKind {
    match class {
        ToolClass::Edit => TimelineEventKind::Edit,
        ToolClass::Shell => TimelineEventKind::Shell,
        ToolClass::TestRun => TimelineEventKind::TestRun,
        ToolClass::Subagent => TimelineEventKind::Subagent,
        ToolClass::Other => TimelineEventKind::ToolCall,
    }
}

fn entry(
    ev: &NormalizedEvent,
    arrival_index: usize,
    kind: TimelineEventKind,
    outcome: Option<ToolOutcome>,
) -> Entry {
    Entry {
        at_ms: ev.at_ms,
        arrival_index,
        turn: ev.turn_id.clone().unwrap_or_default(),
        event: TimelineEvent {
            id: ev.key.0.clone(),
            kind,
            at: rfc3339_from_ms(ev.at_ms),
            tool_name: ev.tool.as_ref().map(|t| t.name.clone()),
            summary: ev.text.clone().unwrap_or_default(),
            files: ev.files.clone(),
            evidence: ev.evidence,
            outcome,
            agent_id: ev.agent_id.clone(),
        },
    }
}

/// Add `extra` files to `files`, keeping order and skipping duplicates.
fn merge_files(files: &mut Vec<String>, extra: &[String]) {
    for f in extra {
        if !files.contains(f) {
            files.push(f.clone());
        }
    }
}

/// Build a session's timeline from its events (in arrival order). `None` when
/// there are no events. `now_ms` is the current time for the stale-session rule;
/// `None` skips the rule (a store built without a clock).
pub fn build_timeline(
    key: &SessionKey,
    events: &[NormalizedEvent],
    now_ms: Option<u64>,
) -> Option<SessionTimeline> {
    if events.is_empty() {
        return None;
    }
    let mut sorted: Vec<_> = events.iter().enumerate().collect();
    // Stable: equal timestamps keep arrival order.
    sorted.sort_by_key(|&(_, ev)| ev.at_ms);

    let mut status = SessionStatus::Unknown;
    let mut entries: Vec<Entry> = Vec::new();
    // (agent_id, tool_use_id) -> index in `entries`.
    let mut tools: HashMap<(Option<&str>, &str), usize> = HashMap::new();
    let mut turn_ends: HashMap<String, u64> = HashMap::new();
    let mut prompts: HashMap<String, String> = HashMap::new();

    for &(arrival_index, ev) in &sorted {
        match &ev.kind {
            EventKind::SessionStart { .. } => status = SessionStatus::Live,
            EventKind::SessionEnd => status = SessionStatus::Ended,
            EventKind::Prompt => {
                let turn = ev.turn_id.clone().unwrap_or_default();
                if let Some(text) = &ev.text {
                    prompts.entry(turn).or_insert_with(|| text.clone());
                }
                entries.push(entry(ev, arrival_index, TimelineEventKind::Prompt, None));
            }
            EventKind::AssistantMessage => {
                entries.push(entry(
                    ev,
                    arrival_index,
                    TimelineEventKind::AssistantMessage,
                    None,
                ));
            }
            EventKind::SubagentStarted | EventKind::SubagentStopped => {
                entries.push(entry(ev, arrival_index, TimelineEventKind::Subagent, None));
            }
            EventKind::Stop => {
                let turn = ev.turn_id.clone().unwrap_or_default();
                let end = turn_ends.entry(turn).or_insert(ev.at_ms);
                *end = (*end).max(ev.at_ms);
                entries.push(entry(ev, arrival_index, TimelineEventKind::Stop, None));
            }
            EventKind::ToolStarted | EventKind::ToolFinished { .. } => {
                let (class, use_id) = match &ev.tool {
                    Some(t) => (t.class, t.use_id.as_deref()),
                    None => (ToolClass::Other, None),
                };
                let outcome = match &ev.kind {
                    EventKind::ToolFinished { outcome } => *outcome,
                    _ => ToolOutcome::Unknown,
                };
                let is_start = matches!(ev.kind, EventKind::ToolStarted);
                let existing =
                    use_id.and_then(|id| tools.get(&(ev.agent_id.as_deref(), id)).copied());
                match existing {
                    // The other half is already there: merge into it.
                    Some(i) => {
                        let e = &mut entries[i];
                        if is_start {
                            // The entry shows the call where it started.
                            e.at_ms = ev.at_ms;
                            e.arrival_index = arrival_index;
                            e.turn = ev.turn_id.clone().unwrap_or_default();
                            e.event.at = rfc3339_from_ms(ev.at_ms);
                            e.event.id = ev.key.0.clone();
                            if let Some(text) = &ev.text {
                                e.event.summary = text.clone();
                            }
                        } else {
                            e.event.outcome = Some(outcome);
                            if e.event.summary.is_empty() {
                                e.event.summary = ev.text.clone().unwrap_or_default();
                            }
                        }
                        merge_files(&mut e.event.files, &ev.files);
                    }
                    None => {
                        if let Some(id) = use_id {
                            tools.insert((ev.agent_id.as_deref(), id), entries.len());
                        }
                        entries.push(entry(
                            ev,
                            arrival_index,
                            kind_for_tool(class),
                            Some(outcome),
                        ));
                    }
                }
            }
        }
    }

    // A merged row takes both its timestamp and arrival order from the start.
    entries.sort_by_key(|e| (e.at_ms, e.arrival_index));

    // Group into turns, ordered by each turn's first entry.
    let mut turns: Vec<Turn> = Vec::new();
    let mut turn_index: HashMap<String, usize> = HashMap::new();
    for e in entries {
        let i = *turn_index.entry(e.turn.clone()).or_insert_with(|| {
            turns.push(Turn {
                id: e.turn.clone(),
                index: u32::try_from(turns.len()).unwrap_or(u32::MAX),
                prompt: prompts.get(&e.turn).cloned(),
                started_at: e.event.at.clone(),
                ended_at: turn_ends.get(&e.turn).map(|&ms| rfc3339_from_ms(ms)),
                events: Vec::new(),
            });
            turns.len() - 1
        });
        turns[i].events.push(e.event);
    }

    let &(_, first) = sorted.first()?;
    let &(_, last) = sorted.last()?;
    let session = SessionSummary {
        id: key.to_contract(),
        harness: harness_to_contract(key.harness),
        model: sorted.iter().rev().find_map(|(_, e)| e.model.clone()),
        worktree_id: sorted.iter().rev().find_map(|(_, e)| e.worktree_id.clone()),
        cwd: last.cwd.clone(),
        task: sorted
            .iter()
            .find(|(_, e)| e.kind == EventKind::Prompt && e.text.is_some())
            .and_then(|(_, e)| e.text.clone()),
        started_at: rfc3339_from_ms(first.at_ms),
        last_event_at: rfc3339_from_ms(last.at_ms),
        status: match now_ms {
            Some(now) => effective_status(status, last.at_ms, now),
            None => status,
        },
        turn_count: u32::try_from(turns.len()).unwrap_or(u32::MAX),
    };
    Some(SessionTimeline { session, turns })
}

/// The summary alone. Builds the whole timeline (fine for the memory store; a
/// database backend may compute summaries with a query instead, as long as it
/// gives the same result: the tests below are the reference).
pub fn build_summary(
    key: &SessionKey,
    events: &[NormalizedEvent],
    now_ms: Option<u64>,
) -> Option<SessionSummary> {
    build_timeline(key, events, now_ms).map(|t| t.session)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60 * 1000;

    #[test]
    fn only_live_sessions_go_stale_and_only_past_the_limit() {
        let last = 1_000 * MIN;
        let at = |age: u64| effective_status(SessionStatus::Live, last, last + age);
        assert_eq!(at(0), SessionStatus::Live);
        assert_eq!(
            at(STALE_AFTER_MS),
            SessionStatus::Live,
            "exactly N is not older"
        );
        assert_eq!(at(STALE_AFTER_MS + 1), SessionStatus::Unknown);
        // Ended is a fact, unknown stays unknown, however old.
        let old = last + 100 * STALE_AFTER_MS;
        assert_eq!(
            effective_status(SessionStatus::Ended, last, old),
            SessionStatus::Ended
        );
        assert_eq!(
            effective_status(SessionStatus::Unknown, last, old),
            SessionStatus::Unknown
        );
        // An event "from the future" is never stale (and never underflows).
        assert_eq!(
            effective_status(SessionStatus::Live, last, 0),
            SessionStatus::Live
        );
    }
}
