//! In-memory [`EventStore`]: the backend until WHY-12's SQLite store lands, and
//! the fake that tests plug in. Nothing survives a restart.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use super::clock::Clock;
use super::correlation::{
    Call, CallEvent, EditTouch, ShellTurn, build_edit_touches, build_shell_turns,
    distinct_sessions, fold_calls, tool_event_class,
};
use super::edits::stored_form;
use super::model::{EventKey, NormalizedEvent, SessionKey, ToolClass};
use super::rationale::SourceEvent;
use super::timeline::{TimelinePosition, TurnIndices, build_summary, build_timeline, turn_indices};
use super::{AppendOutcome, EventStore};
use crate::contracts::{SessionSummary, SessionTimeline, WorktreeId};
use crate::provenance::{EditRecord, EditSource};
use crate::store::StoreError;

#[derive(Debug, Default)]
struct Inner {
    /// Every stored event's key, for O(1) deduplication.
    keys: HashSet<EventKey>,
    /// Each session's events in ARRIVAL order (the timeline builder relies on
    /// it to break timestamp ties).
    sessions: HashMap<SessionKey, Vec<NormalizedEvent>>,
    /// WHY-64: every stored edit op, in arrival order (the order
    /// `edit_records` returns). Kept apart from the events, like SQLite's
    /// `edit_ops` table, so the two stores behave alike.
    edits: Vec<EditRecord>,
    /// The next event's arrival number: SQLite's `events.seq`, which an
    /// [`EditRecord::seq`] reports.
    next_seq: u64,
}

/// An [`EventStore`] in two hash maps behind one `std::sync::Mutex` (held for
/// microseconds, never across an `.await`; see `store/memory.rs` for why a std
/// mutex is the right choice).
#[derive(Debug, Default)]
pub struct MemoryEventStore {
    inner: Mutex<Inner>,
    /// For the stale-session rule. `None` (what [`MemoryEventStore::new`]
    /// builds) never applies it, so tests with made-up timestamps stay `live`.
    clock: Option<Clock>,
}

impl MemoryEventStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that demotes stale `live` sessions, judged by `clock`.
    pub fn with_clock(clock: Clock) -> Self {
        Self {
            inner: Mutex::default(),
            clock: Some(clock),
        }
    }

    fn now_ms(&self) -> Option<u64> {
        self.clock.as_ref().map(Clock::now_ms)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Inner>, StoreError> {
        self.inner
            .lock()
            .map_err(|_| StoreError::Backend("event store lock poisoned".to_string()))
    }
}

/// The calls of one tool class that happened in worktree `wt` in one session.
/// Events resolved to another worktree are ignored here, so a call split across
/// two worktrees (a Pre in one, a Post in the other) is never stitched together.
fn calls_in(events: &[NormalizedEvent], wt: &WorktreeId, classes: &[ToolClass]) -> Vec<Call> {
    fold_calls(
        events
            .iter()
            .filter(|e| {
                e.worktree_id.as_ref() == Some(wt)
                    && tool_event_class(e).is_some_and(|class| classes.contains(&class))
            })
            .filter_map(CallEvent::from_event),
    )
}

fn session_turn_indices(events: &[NormalizedEvent]) -> TurnIndices {
    let positions: Vec<_> = events
        .iter()
        .filter(|e| !e.kind.is_lifecycle())
        .map(TimelinePosition::from_event)
        .collect();
    turn_indices(&positions)
}

impl EventStore for MemoryEventStore {
    fn append(&self, events: &[NormalizedEvent]) -> Result<AppendOutcome, StoreError> {
        let mut inner = self.lock()?;
        let mut outcome = AppendOutcome::default();
        let mut changed_sessions = HashSet::new();
        for ev in events {
            // `insert` returns false when the key was already there: that check
            // and the insert are one operation, under the lock.
            if inner.keys.insert(ev.key.clone()) {
                let seq = inner.next_seq;
                inner.next_seq += 1;
                // The event and its ops go in under the same lock: no reader
                // ever sees one without the other.
                for (index, op) in ev.edits.iter().enumerate() {
                    let Some((_, op)) = stored_form(op) else {
                        continue;
                    };
                    inner.edits.push(EditRecord {
                        source: EditSource {
                            session: ev.session.clone(),
                            turn_id: ev.turn_id.clone(),
                            agent_id: ev.agent_id.clone(),
                            tool_use_id: ev.tool.as_ref().and_then(|t| t.use_id.clone()),
                            event_key: ev.key.clone(),
                        },
                        seq,
                        op_index: u32::try_from(index).unwrap_or(u32::MAX),
                        at_ms: ev.at_ms,
                        op,
                    });
                }
                let mut stored = ev.clone();
                // Like SQLite: the ops live in `edits`, not on the event.
                stored.edits = Vec::new();
                inner
                    .sessions
                    .entry(ev.session.clone())
                    .or_default()
                    .push(stored);
                changed_sessions.insert(&ev.session);
                outcome.inserted += 1;
            } else {
                outcome.duplicates += 1;
            }
        }
        // A session can appear in several worktree lists even after its cwd
        // changes. Use its stored history, including events outside this batch.
        let changed_worktrees: HashSet<_> = changed_sessions
            .into_iter()
            .flat_map(|session| &inner.sessions[session])
            .filter_map(|ev| ev.worktree_id.as_ref())
            .collect();
        outcome.changed_worktrees = changed_worktrees.into_iter().cloned().collect();
        outcome.changed_worktrees.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(outcome)
    }

    fn sessions_for_worktree(&self, wt: &WorktreeId) -> Result<Vec<SessionSummary>, StoreError> {
        let inner = self.lock()?;
        let now = self.now_ms();
        let mut found: Vec<(u64, SessionSummary)> = inner
            .sessions
            .iter()
            .filter(|(_, events)| events.iter().any(|e| e.worktree_id.as_ref() == Some(wt)))
            .filter_map(|(key, events)| {
                let last = events.iter().map(|e| e.at_ms).max()?;
                Some((last, build_summary(key, events, now)?))
            })
            .collect();
        // Most recent first; the id breaks ties so the order is deterministic
        // (HashMap iteration order is not).
        found.sort_by(|(a_at, a), (b_at, b)| b_at.cmp(a_at).then_with(|| a.id.0.cmp(&b.id.0)));
        Ok(found.into_iter().map(|(_, s)| s).collect())
    }

    fn timeline(&self, session: &SessionKey) -> Result<Option<SessionTimeline>, StoreError> {
        let inner = self.lock()?;
        Ok(inner
            .sessions
            .get(session)
            .and_then(|events| build_timeline(session, events, self.now_ms())))
    }

    fn prune_older_than(&self, cutoff_ms: u64) -> Result<usize, StoreError> {
        let mut inner = self.lock()?;
        let Inner {
            keys,
            sessions,
            edits,
            ..
        } = &mut *inner;
        let before = sessions.len();
        sessions.retain(|_, events| {
            let keep = events.iter().any(|e| e.at_ms >= cutoff_ms);
            if !keep {
                for e in events.iter() {
                    keys.remove(&e.key);
                }
            }
            keep
        });
        // A pruned session's ops go with it (SQLite: ON DELETE CASCADE).
        edits.retain(|r| sessions.contains_key(&r.source.session));
        Ok(before - sessions.len())
    }

    // Complexity: O(all stored events) per call, because there is no index. This
    // backend is the test fake and the fallback when `events.db` cannot be
    // opened; the SQLite store has the real index (`event_files`).
    fn edit_calls(&self, wt: &WorktreeId, paths: &[String]) -> Result<Vec<EditTouch>, StoreError> {
        let inner = self.lock()?;
        let wanted: HashSet<&str> = paths.iter().map(String::as_str).collect();
        let mut calls: Vec<Call> = Vec::new();
        let mut indices = HashMap::new();
        for (session, events) in &inner.sessions {
            let session_calls: Vec<Call> = calls_in(events, wt, &[ToolClass::Edit])
                .into_iter()
                .filter(|c| c.paths.iter().any(|p| wanted.contains(p.as_str())))
                .collect();
            if session_calls.is_empty() {
                continue;
            }
            indices.insert(session.clone(), session_turn_indices(events));
            calls.extend(session_calls);
        }
        Ok(build_edit_touches(calls, &wanted, &indices))
    }

    // O(all stored ops) per call; see `edit_calls` for why that is acceptable.
    fn edit_records(&self, wt: &WorktreeId, path: &str) -> Result<Vec<EditRecord>, StoreError> {
        let inner = self.lock()?;
        // `edits` is already in (seq, op_index) order: appended in arrival order.
        Ok(inner
            .edits
            .iter()
            .filter(|r| r.op.worktree_id.as_ref() == Some(wt) && r.op.path == path)
            .cloned()
            .collect())
    }

    fn shell_turns(&self, wt: &WorktreeId) -> Result<Vec<ShellTurn>, StoreError> {
        let inner = self.lock()?;
        let mut calls: Vec<Call> = Vec::new();
        for events in inner.sessions.values() {
            // Shell commands and test runs: both are processes that may write.
            calls.extend(calls_in(
                events,
                wt,
                &[ToolClass::Shell, ToolClass::TestRun],
            ));
        }
        let indices: HashMap<SessionKey, TurnIndices> = distinct_sessions(&calls)
            .into_iter()
            .filter_map(|k| {
                let events = inner.sessions.get(&k)?;
                Some((k, session_turn_indices(events)))
            })
            .collect();
        Ok(build_shell_turns(calls, &indices))
    }

    fn source_events(&self, session: &SessionKey) -> Result<Option<Vec<SourceEvent>>, StoreError> {
        let inner = self.lock()?;
        Ok(inner
            .sessions
            .get(session)
            .map(|events| events.iter().map(SourceEvent::from_event).collect()))
    }
}

#[cfg(test)]
mod tests {
    use why_wire::Harness;

    use super::*;
    use crate::contracts::{SessionStatus, TimelineEventKind, ToolOutcome};
    use crate::events::model::EventKind;
    use crate::events::test_support::{event, tool};

    fn wt() -> WorktreeId {
        WorktreeId("wt-1".into())
    }

    fn key(session: &str) -> SessionKey {
        SessionKey::new(Harness::ClaudeCode, session)
    }

    fn start(session: &str, at: u64, resumed: bool) -> NormalizedEvent {
        event(
            session,
            "SessionStart",
            EventKind::SessionStart { resumed },
            at,
            None,
        )
    }

    fn prompt(session: &str, at: u64, turn: &str, text: &str) -> NormalizedEvent {
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

    #[test]
    fn duplicate_delivery_is_idempotent() {
        let store = MemoryEventStore::new();
        let batch = [start("s1", 1, false), prompt("s1", 2, "p1", "hi")];
        assert_eq!(
            store.append(&batch).unwrap(),
            AppendOutcome {
                inserted: 2,
                duplicates: 0,
                changed_worktrees: vec![wt()],
            }
        );
        // The same envelopes delivered again (e.g. re-sent from the spool).
        assert_eq!(
            store.append(&batch).unwrap(),
            AppendOutcome {
                inserted: 0,
                duplicates: 2,
                changed_worktrees: vec![],
            }
        );
        // Duplicates inside one batch, too.
        let p = prompt("s1", 3, "p2", "again");
        assert_eq!(
            store.append(&[p.clone(), p]).unwrap(),
            AppendOutcome {
                inserted: 1,
                duplicates: 1,
                changed_worktrees: vec![wt()],
            }
        );

        let t = store.timeline(&key("s1")).unwrap().unwrap();
        assert_eq!(t.turns.len(), 2);
        assert_eq!(t.turns[0].events.len(), 1);
    }

    #[test]
    fn pre_and_post_with_the_same_tool_use_id_are_both_kept_and_paired() {
        let store = MemoryEventStore::new();
        let pre = tool("s1", "toolu_1", None, 10, "p1");
        let post = tool("s1", "toolu_1", Some(ToolOutcome::Ok), 20, "p1");
        assert_eq!(
            store.append(&[pre, post]).unwrap(),
            AppendOutcome {
                inserted: 2,
                duplicates: 0,
                changed_worktrees: vec![wt()],
            },
            "a shared tool_use_id must not dedupe"
        );
        let t = store.timeline(&key("s1")).unwrap().unwrap();
        let events = &t.turns[0].events;
        assert_eq!(events.len(), 1, "one entry per tool call");
        assert_eq!(events[0].kind, TimelineEventKind::Shell);
        assert_eq!(events[0].outcome, Some(ToolOutcome::Ok));
        assert_eq!(
            events[0].at, "1970-01-01T00:00:00.010Z",
            "shown at the start"
        );
    }

    #[test]
    fn post_arriving_before_pre_still_pairs() {
        let store = MemoryEventStore::new();
        // Arrival order: Post first (separate hook processes race), and its
        // timestamp is even earlier than the Pre's (clock skew).
        store
            .append(&[tool("s1", "toolu_1", Some(ToolOutcome::Failed), 10, "p1")])
            .unwrap();
        store
            .append(&[tool("s1", "toolu_1", None, 15, "p1")])
            .unwrap();
        // A later call, so the order of entries is observable.
        store
            .append(&[tool("s1", "toolu_2", Some(ToolOutcome::Ok), 12, "p1")])
            .unwrap();
        store
            .append(&[tool("s1", "toolu_2", None, 11, "p1")])
            .unwrap();

        let t = store.timeline(&key("s1")).unwrap().unwrap();
        let events = &t.turns[0].events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].outcome, Some(ToolOutcome::Ok)); // toolu_2 started at 11
        assert_eq!(events[1].outcome, Some(ToolOutcome::Failed)); // toolu_1 started at 15
        assert_eq!(events[1].at, "1970-01-01T00:00:00.015Z");
    }

    #[test]
    fn merged_tool_uses_the_starts_arrival_order_for_timestamp_ties() {
        let store = MemoryEventStore::new();
        store
            .append(&[
                tool("s1", "toolu_1", Some(ToolOutcome::Ok), 10, "p1"),
                prompt("s1", 15, "p1", "before the call"),
                tool("s1", "toolu_1", None, 15, "p1"),
                event("s1", "Stop", EventKind::Stop, 15, Some("p1")),
            ])
            .unwrap();
        let t = store.timeline(&key("s1")).unwrap().unwrap();
        let kinds: Vec<_> = t.turns[0].events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                TimelineEventKind::Prompt,
                TimelineEventKind::Shell,
                TimelineEventKind::Stop
            ]
        );
    }

    #[test]
    fn pre_without_post_is_unknown_never_rejected() {
        let store = MemoryEventStore::new();
        store
            .append(&[tool("s1", "toolu_1", None, 10, "p1")])
            .unwrap();
        let t = store.timeline(&key("s1")).unwrap().unwrap();
        assert_eq!(t.turns[0].events[0].outcome, Some(ToolOutcome::Unknown));
    }

    #[test]
    fn resume_does_not_create_a_second_session() {
        let store = MemoryEventStore::new();
        store
            .append(&[
                start("s1", 1, false),
                prompt("s1", 2, "p1", "first"),
                event("s1", "SessionEnd", EventKind::SessionEnd, 3, Some("p1")),
            ])
            .unwrap();
        let ended = store.sessions_for_worktree(&wt()).unwrap();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].status, SessionStatus::Ended);

        // `claude --resume s1`: same session id, SessionStart source "resume".
        store
            .append(&[start("s1", 10, true), prompt("s1", 11, "p2", "second")])
            .unwrap();
        let sessions = store.sessions_for_worktree(&wt()).unwrap();
        assert_eq!(sessions.len(), 1, "a resume is the same session");
        let s = &sessions[0];
        assert_eq!(s.status, SessionStatus::Live);
        assert_eq!(s.turn_count, 2);
        assert_eq!(s.task.as_deref(), Some("first"));
        assert_eq!(s.started_at, "1970-01-01T00:00:00.001Z");
        assert_eq!(s.last_event_at, "1970-01-01T00:00:00.011Z");
    }

    #[test]
    fn append_reports_every_worktree_listing_a_changed_session() {
        let store = MemoryEventStore::new();
        let previous = start("s1", 1, false);
        let mut unrelated = start("s2", 2, false);
        unrelated.worktree_id = Some(WorktreeId("wt-3".into()));
        store.append(&[previous, unrelated.clone()]).unwrap();

        let mut resumed = start("s1", 3, true);
        let new_worktree = WorktreeId("wt-2".into());
        resumed.worktree_id = Some(new_worktree.clone());
        let outcome = store.append(&[resumed, unrelated]).unwrap();
        assert_eq!(outcome.inserted, 1);
        assert_eq!(outcome.duplicates, 1);
        assert_eq!(outcome.changed_worktrees, [wt(), new_worktree.clone()]);
        assert_eq!(
            store.sessions_for_worktree(&wt()).unwrap(),
            store.sessions_for_worktree(&new_worktree).unwrap()
        );

        let mut unresolved = prompt("s1", 4, "p1", "cwd is unavailable");
        unresolved.worktree_id = None;
        let outcome = store.append(&[unresolved]).unwrap();
        assert_eq!(outcome.changed_worktrees, [wt(), new_worktree]);
    }

    #[test]
    fn status_is_unknown_without_lifecycle_events() {
        let store = MemoryEventStore::new();
        store.append(&[prompt("s1", 2, "p1", "hi")]).unwrap();
        assert_eq!(
            store.sessions_for_worktree(&wt()).unwrap()[0].status,
            SessionStatus::Unknown
        );
    }

    #[test]
    fn turns_are_ordered_and_closed_by_stop() {
        let store = MemoryEventStore::new();
        store
            .append(&[
                prompt("s1", 30, "p2", "two"),
                prompt("s1", 10, "p1", "one"),
                event("s1", "Stop", EventKind::Stop, 20, Some("p1")),
            ])
            .unwrap();
        let t = store.timeline(&key("s1")).unwrap().unwrap();
        assert_eq!(t.turns.len(), 2);
        assert_eq!(
            (t.turns[0].index, t.turns[0].prompt.as_deref()),
            (0, Some("one"))
        );
        assert_eq!(
            t.turns[0].ended_at.as_deref(),
            Some("1970-01-01T00:00:00.020Z")
        );
        assert_eq!(
            (t.turns[1].index, t.turns[1].prompt.as_deref()),
            (1, Some("two"))
        );
        assert_eq!(t.turns[1].ended_at, None, "still running");
        let kinds: Vec<TimelineEventKind> = t.turns[0].events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [TimelineEventKind::Prompt, TimelineEventKind::Stop]);
    }

    #[test]
    fn equal_timestamps_keep_arrival_order() {
        let store = MemoryEventStore::new();
        let mut a = event("s1", "Stop", EventKind::AssistantMessage, 5, Some("p1"));
        a.text = Some("first".into());
        let mut b = event(
            "s1",
            "Notification",
            EventKind::AssistantMessage,
            5,
            Some("p1"),
        );
        b.text = Some("second".into());
        store.append(&[a, b]).unwrap();
        let t = store.timeline(&key("s1")).unwrap().unwrap();
        let texts: Vec<&str> = t.turns[0]
            .events
            .iter()
            .map(|e| e.summary.as_str())
            .collect();
        assert_eq!(texts, ["first", "second"]);
    }

    #[test]
    fn sessions_are_filtered_by_worktree_and_sorted_by_recency() {
        let store = MemoryEventStore::new();
        let mut other = prompt("s3", 50, "p1", "elsewhere");
        other.worktree_id = Some(WorktreeId("wt-2".into()));
        store
            .append(&[
                prompt("s1", 10, "p1", "old"),
                prompt("s2", 40, "p1", "new"),
                other,
            ])
            .unwrap();
        let ids: Vec<String> = store
            .sessions_for_worktree(&wt())
            .unwrap()
            .into_iter()
            .map(|s| s.id.0)
            .collect();
        assert_eq!(ids, ["claude-code:s2", "claude-code:s1"]);
        assert!(
            store
                .sessions_for_worktree(&WorktreeId("nope".into()))
                .unwrap()
                .is_empty()
        );
        assert!(store.timeline(&key("missing")).unwrap().is_none());
    }

    #[test]
    fn events_without_a_turn_id_go_to_the_empty_id_turn() {
        // Not seen in the WHY-45 fixtures (only lifecycle events lack a turn
        // id), but a backend must place them the same way: in a turn whose id
        // is "", ordered like any other turn. Lifecycle events stay out.
        let store = MemoryEventStore::new();
        let mut loose = event("s1", "Notification", EventKind::AssistantMessage, 5, None);
        loose.text = Some("no turn".into());
        store
            .append(&[start("s1", 1, false), loose, prompt("s1", 10, "p1", "hi")])
            .unwrap();
        let t = store.timeline(&key("s1")).unwrap().unwrap();
        let ids: Vec<&str> = t.turns.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["", "p1"]);
        assert_eq!(t.turns[0].events.len(), 1, "SessionStart is not an entry");
        assert_eq!(t.turns[0].prompt, None);
        assert_eq!(t.session.turn_count, 2);
    }

    #[test]
    fn prune_drops_whole_sessions_older_than_the_cutoff() {
        let store = MemoryEventStore::new();
        store
            .append(&[
                prompt("old", 10, "p1", "old"),
                // Started long ago, still active: kept whole.
                prompt("long", 10, "p1", "first"),
                prompt("long", 200, "p2", "recent"),
                prompt("new", 150, "p1", "new"),
            ])
            .unwrap();
        assert_eq!(store.prune_older_than(100).unwrap(), 1);
        assert!(store.timeline(&key("old")).unwrap().is_none());
        assert_eq!(
            store.timeline(&key("long")).unwrap().unwrap().turns.len(),
            2
        );
        assert!(store.timeline(&key("new")).unwrap().is_some());
        // Pruned keys are forgotten: a late redelivery is stored again (and
        // pruned again next time), rather than silently ignored forever.
        assert_eq!(
            store
                .append(&[prompt("old", 10, "p1", "old")])
                .unwrap()
                .inserted,
            1
        );
        assert_eq!(store.prune_older_than(100).unwrap(), 1);
    }
}
