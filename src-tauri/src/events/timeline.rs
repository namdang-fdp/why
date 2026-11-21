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

/// The fields that place an event in the timeline. SQLite can read these
/// without loading the event's message or files.
#[derive(Clone, Copy)]
pub(crate) struct TimelinePosition<S> {
    pub turn_id: S,
    pub at_ms: u64,
    pub agent_id: Option<S>,
    pub use_id: Option<S>,
    pub is_start: bool,
}

impl<S: AsRef<str>> TimelinePosition<S> {
    fn borrowed(&self) -> TimelinePosition<&str> {
        TimelinePosition {
            turn_id: self.turn_id.as_ref(),
            at_ms: self.at_ms,
            agent_id: self.agent_id.as_ref().map(AsRef::as_ref),
            use_id: self.use_id.as_ref().map(AsRef::as_ref),
            is_start: self.is_start,
        }
    }
}

impl<'a> TimelinePosition<&'a str> {
    pub(crate) fn from_event(ev: &'a NormalizedEvent) -> Self {
        let use_id = match ev.kind {
            EventKind::ToolStarted | EventKind::ToolFinished { .. } => {
                ev.tool.as_ref().and_then(|tool| tool.use_id.as_deref())
            }
            _ => None,
        };
        Self {
            turn_id: ev.turn_id.as_deref().unwrap_or_default(),
            at_ms: ev.at_ms,
            agent_id: ev.agent_id.as_deref(),
            use_id,
            is_start: ev.kind == EventKind::ToolStarted,
        }
    }
}

struct Entry<'a, T> {
    position: TimelinePosition<&'a str>,
    arrival_index: usize,
    value: T,
}

/// Both the rendered timeline and the attribution index merge and order rows
/// here. The index only needs the position, so its entry payload is `()`.
struct Entries<'a, T> {
    entries: Vec<Entry<'a, T>>,
    tools: HashMap<(Option<&'a str>, &'a str), usize>,
}

impl<'a, T> Entries<'a, T> {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            tools: HashMap::new(),
        }
    }

    fn push(
        &mut self,
        position: TimelinePosition<&'a str>,
        arrival_index: usize,
        create: impl FnOnce() -> T,
        merge: impl FnOnce(&mut T),
    ) {
        if let Some(id) = position.use_id {
            let slot = self
                .tools
                .entry((position.agent_id, id))
                .or_insert(self.entries.len());
            if let Some(entry) = self.entries.get_mut(*slot) {
                if position.is_start {
                    // A finish may have a different turn ID or be stamped
                    // earlier after a clock adjustment. The start anchors it.
                    entry.position = position;
                    entry.arrival_index = arrival_index;
                }
                merge(&mut entry.value);
                return;
            }
        }
        self.entries.push(Entry {
            position,
            arrival_index,
            value: create(),
        });
    }

    fn ordered(mut self) -> Vec<Entry<'a, T>> {
        self.entries
            .sort_by_key(|e| (e.position.at_ms, e.arrival_index));
        self.entries
    }
}

#[derive(Default)]
pub(crate) struct TurnIndices {
    ids: Vec<String>,
    by_id: HashMap<String, usize>,
    by_tool: HashMap<Option<String>, HashMap<String, usize>>,
}

impl TurnIndices {
    pub(crate) fn locate(
        &self,
        turn: &str,
        agent_id: &Option<String>,
        use_id: Option<&str>,
    ) -> Option<(&str, u32)> {
        let index = use_id
            .and_then(|id| self.by_tool.get(agent_id)?.get(id))
            .or_else(|| self.by_id.get(turn))?;
        Some((&self.ids[*index], turn_index(*index)))
    }
}

pub(crate) fn turn_indices<S: AsRef<str>>(positions: &[TimelinePosition<S>]) -> TurnIndices {
    let mut sorted: Vec<_> = positions.iter().enumerate().collect();
    sorted.sort_by_key(|&(_, position)| position.at_ms);
    let mut entries = Entries::new();
    for (arrival, position) in sorted {
        entries.push(position.borrowed(), arrival, || (), |_| ());
    }

    let mut turns = TurnIndices::default();
    for entry in entries.ordered() {
        let position = entry.position;
        let index = *turns
            .by_id
            .entry(position.turn_id.to_owned())
            .or_insert_with(|| {
                turns.ids.push(position.turn_id.to_owned());
                turns.ids.len() - 1
            });
        if let Some(id) = position.use_id {
            turns
                .by_tool
                .entry(position.agent_id.map(str::to_owned))
                .or_default()
                .insert(id.to_owned(), index);
        }
    }
    turns
}

fn turn_index(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
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

fn timeline_event(
    ev: &NormalizedEvent,
    kind: TimelineEventKind,
    outcome: Option<ToolOutcome>,
) -> TimelineEvent {
    TimelineEvent {
        id: ev.key.0.clone(),
        kind,
        at: rfc3339_from_ms(ev.at_ms),
        tool_name: ev.tool.as_ref().map(|t| t.name.clone()),
        summary: ev.text.clone().unwrap_or_default(),
        files: ev.files.clone(),
        evidence: ev.evidence,
        outcome,
        agent_id: ev.agent_id.clone(),
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
    let mut entries = Entries::new();
    let mut turn_ends: HashMap<&str, u64> = HashMap::new();
    let mut prompts: HashMap<&str, String> = HashMap::new();

    for &(arrival_index, ev) in &sorted {
        let position = TimelinePosition::from_event(ev);
        let (kind, outcome) = match &ev.kind {
            EventKind::SessionStart { .. } => {
                status = SessionStatus::Live;
                continue;
            }
            EventKind::SessionEnd => {
                status = SessionStatus::Ended;
                continue;
            }
            EventKind::Prompt => {
                if let Some(text) = &ev.text {
                    prompts
                        .entry(position.turn_id)
                        .or_insert_with(|| text.clone());
                }
                (TimelineEventKind::Prompt, None)
            }
            EventKind::AssistantMessage => (TimelineEventKind::AssistantMessage, None),
            EventKind::SubagentStarted | EventKind::SubagentStopped => {
                (TimelineEventKind::Subagent, None)
            }
            EventKind::Stop => {
                let end = turn_ends.entry(position.turn_id).or_insert(ev.at_ms);
                *end = (*end).max(ev.at_ms);
                (TimelineEventKind::Stop, None)
            }
            EventKind::ToolStarted | EventKind::ToolFinished { .. } => {
                let class = ev.tool.as_ref().map_or(ToolClass::Other, |tool| tool.class);
                let outcome = match &ev.kind {
                    EventKind::ToolFinished { outcome } => *outcome,
                    _ => ToolOutcome::Unknown,
                };
                (kind_for_tool(class), Some(outcome))
            }
        };
        entries.push(
            position,
            arrival_index,
            || timeline_event(ev, kind, outcome),
            |event| {
                if position.is_start {
                    event.at = rfc3339_from_ms(ev.at_ms);
                    event.id = ev.key.0.clone();
                    if let Some(text) = &ev.text {
                        event.summary = text.clone();
                    }
                } else {
                    event.outcome = outcome;
                    if event.summary.is_empty() {
                        event.summary = ev.text.clone().unwrap_or_default();
                    }
                }
                merge_files(&mut event.files, &ev.files);
            },
        );
    }

    // Group into turns, ordered by each turn's first merged entry.
    let mut turns: Vec<Turn> = Vec::new();
    let mut by_turn: HashMap<&str, usize> = HashMap::new();
    for entry in entries.ordered() {
        let turn = entry.position.turn_id;
        let i = *by_turn.entry(turn).or_insert_with(|| {
            turns.push(Turn {
                id: turn.to_owned(),
                index: turn_index(turns.len()),
                prompt: prompts.get(turn).cloned(),
                started_at: entry.value.at.clone(),
                ended_at: turn_ends.get(turn).map(|&ms| rfc3339_from_ms(ms)),
                events: Vec::new(),
            });
            turns.len() - 1
        });
        turns[i].events.push(entry.value);
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
