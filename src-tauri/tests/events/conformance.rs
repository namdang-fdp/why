//! The conformance suite: every case below runs against BOTH stores (the
//! `conformance!` macro at the bottom instantiates it once per backend), and
//! `differential` feeds both the same random history and demands identical
//! answers. A behaviour that is not in this file is not guaranteed to match.

use std::collections::HashSet;

use why_lib::contracts::{SessionStatus, TimelineEventKind, ToolOutcome};
use why_lib::events::{
    AppendOutcome, Clock, EventKind, EventStore, MemoryEventStore, NormalizedEvent, SessionKey,
    SqliteEventStore, excerpt,
};
use why_wire::Harness;

use crate::common::*;

/// A store plus whatever keeps it alive (the temp dir of a file database).
pub struct Fixture {
    pub store: Box<dyn EventStore>,
    _dir: Option<tempfile::TempDir>,
}

pub fn memory(clock: Clock) -> Fixture {
    Fixture {
        store: Box::new(MemoryEventStore::with_clock(clock)),
        _dir: None,
    }
}

pub fn sqlite(clock: Clock) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteEventStore::open(&dir.path().join("events.db"))
        .unwrap()
        .with_clock(clock);
    Fixture {
        store: Box::new(store),
        _dir: Some(dir),
    }
}

/// Test events use tiny timestamps (1..~500 ms); a clock at 1,000 ms keeps
/// every such session far from "stale".
const NOW: u64 = 1_000;
const MIN: u64 = 60 * 1000;

fn key(session: &str) -> SessionKey {
    SessionKey::new(Harness::ClaudeCode, session)
}

fn outcome(inserted: usize, duplicates: usize, worktrees: &[&str]) -> AppendOutcome {
    AppendOutcome {
        inserted,
        duplicates,
        changed_worktrees: worktrees.iter().map(|w| wt(w)).collect(),
    }
}

type Make = fn(Clock) -> Fixture;

fn fresh(make: Make) -> Fixture {
    make(Clock::fixed(NOW))
}

fn idempotent_redelivery(make: Make) {
    let f = fresh(make);
    let s = &*f.store;
    let batch = [start("s1", 1, false), prompt("s1", 2, "p1", "hi")];
    assert_eq!(s.append(&batch).unwrap(), outcome(2, 0, &["wt-1"]));
    // The same envelopes delivered again (re-sent from the spool).
    assert_eq!(s.append(&batch).unwrap(), outcome(0, 2, &[]));
    // Duplicates inside one batch, too.
    let p = prompt("s1", 3, "p2", "again");
    assert_eq!(s.append(&[p.clone(), p]).unwrap(), outcome(1, 1, &["wt-1"]));
    let t = s.timeline(&key("s1")).unwrap().unwrap();
    assert_eq!(t.turns.len(), 2);
    assert_eq!(t.turns[0].events.len(), 1);
    // An empty batch is a no-op.
    assert_eq!(s.append(&[]).unwrap(), outcome(0, 0, &[]));
}

fn pre_and_post_are_both_kept_and_paired(make: Make) {
    let f = fresh(make);
    let s = &*f.store;
    let pre = tool("s1", "toolu_1", None, 10, "p1");
    let post = tool("s1", "toolu_1", Some(ToolOutcome::Ok), 20, "p1");
    assert_eq!(
        s.append(&[pre, post]).unwrap(),
        outcome(2, 0, &["wt-1"]),
        "a shared tool_use_id must not dedupe"
    );
    let t = s.timeline(&key("s1")).unwrap().unwrap();
    let events = &t.turns[0].events;
    assert_eq!(events.len(), 1, "one entry per tool call");
    assert_eq!(events[0].kind, TimelineEventKind::Shell);
    assert_eq!(events[0].outcome, Some(ToolOutcome::Ok));
    assert_eq!(
        events[0].at, "1970-01-01T00:00:00.010Z",
        "shown at the start"
    );
}

fn post_before_pre_still_pairs(make: Make) {
    let f = fresh(make);
    let s = &*f.store;
    // Arrival order: Post first, with an even earlier timestamp (clock skew).
    s.append(&[tool("s1", "toolu_1", Some(ToolOutcome::Failed), 10, "p1")])
        .unwrap();
    s.append(&[tool("s1", "toolu_1", None, 15, "p1")]).unwrap();
    s.append(&[tool("s1", "toolu_2", Some(ToolOutcome::Ok), 12, "p1")])
        .unwrap();
    s.append(&[tool("s1", "toolu_2", None, 11, "p1")]).unwrap();
    let t = s.timeline(&key("s1")).unwrap().unwrap();
    let events = &t.turns[0].events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].outcome, Some(ToolOutcome::Ok)); // toolu_2 started at 11
    assert_eq!(events[1].outcome, Some(ToolOutcome::Failed)); // toolu_1 started at 15
    assert_eq!(events[1].at, "1970-01-01T00:00:00.015Z");
}

fn pre_without_post_is_unknown(make: Make) {
    let f = fresh(make);
    f.store
        .append(&[tool("s1", "toolu_1", None, 10, "p1")])
        .unwrap();
    let t = f.store.timeline(&key("s1")).unwrap().unwrap();
    assert_eq!(t.turns[0].events[0].outcome, Some(ToolOutcome::Unknown));
}

fn merged_tool_uses_the_starts_arrival_order(make: Make) {
    let f = fresh(make);
    f.store
        .append(&[
            tool("s1", "toolu_1", Some(ToolOutcome::Ok), 10, "p1"),
            prompt("s1", 15, "p1", "before the call"),
            tool("s1", "toolu_1", None, 15, "p1"),
            stop("s1", 15, "p1"),
        ])
        .unwrap();
    let t = f.store.timeline(&key("s1")).unwrap().unwrap();
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

fn resume_is_the_same_session(make: Make) {
    let f = fresh(make);
    let s = &*f.store;
    s.append(&[
        start("s1", 1, false),
        prompt("s1", 2, "p1", "first"),
        end("s1", 3, "p1"),
    ])
    .unwrap();
    let ended = s.sessions_for_worktree(&wt("wt-1")).unwrap();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].status, SessionStatus::Ended);

    s.append(&[start("s1", 10, true), prompt("s1", 11, "p2", "second")])
        .unwrap();
    let sessions = s.sessions_for_worktree(&wt("wt-1")).unwrap();
    assert_eq!(sessions.len(), 1, "a resume is the same session");
    let one = &sessions[0];
    assert_eq!(one.status, SessionStatus::Live);
    assert_eq!(one.turn_count, 2);
    assert_eq!(one.task.as_deref(), Some("first"));
    assert_eq!(one.started_at, "1970-01-01T00:00:00.001Z");
    assert_eq!(one.last_event_at, "1970-01-01T00:00:00.011Z");
}

fn equal_timestamps_keep_arrival_order(make: Make) {
    let f = fresh(make);
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
    f.store.append(&[a, b]).unwrap();
    let t = f.store.timeline(&key("s1")).unwrap().unwrap();
    let texts: Vec<&str> = t.turns[0]
        .events
        .iter()
        .map(|e| e.summary.as_str())
        .collect();
    assert_eq!(texts, ["first", "second"]);
}

/// Arrival order is a property of the whole history, not of one batch: events
/// with the same timestamp arriving in different appends keep their order.
fn equal_timestamps_keep_arrival_order_across_appends(make: Make) {
    let f = fresh(make);
    for (i, name) in ["one", "two", "three"].into_iter().enumerate() {
        let mut e = event(
            "s1",
            &format!("Notification{i}"),
            EventKind::AssistantMessage,
            5,
            Some("p1"),
        );
        e.text = Some(name.into());
        f.store.append(&[e]).unwrap();
    }
    let t = f.store.timeline(&key("s1")).unwrap().unwrap();
    let texts: Vec<&str> = t.turns[0]
        .events
        .iter()
        .map(|e| e.summary.as_str())
        .collect();
    assert_eq!(texts, ["one", "two", "three"]);
}

fn turns_are_ordered_and_closed_by_stop(make: Make) {
    let f = fresh(make);
    f.store
        .append(&[
            prompt("s1", 30, "p2", "two"),
            prompt("s1", 10, "p1", "one"),
            stop("s1", 20, "p1"),
        ])
        .unwrap();
    let t = f.store.timeline(&key("s1")).unwrap().unwrap();
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
}

fn events_without_a_turn_id_go_to_the_empty_id_turn(make: Make) {
    let f = fresh(make);
    let mut loose = event("s1", "Notification", EventKind::AssistantMessage, 5, None);
    loose.text = Some("no turn".into());
    f.store
        .append(&[start("s1", 1, false), loose, prompt("s1", 10, "p1", "hi")])
        .unwrap();
    let t = f.store.timeline(&key("s1")).unwrap().unwrap();
    let ids: Vec<&str> = t.turns.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["", "p1"]);
    assert_eq!(t.turns[0].events.len(), 1, "SessionStart is not an entry");
    assert_eq!(t.session.turn_count, 2);
}

fn worktree_filtering_and_sorting(make: Make) {
    let f = fresh(make);
    let s = &*f.store;
    let mut other = prompt("s3", 50, "p1", "elsewhere");
    other.worktree_id = Some(wt("wt-2"));
    // `a` and `b` end at the same millisecond: the id breaks the tie.
    s.append(&[
        prompt("s1", 10, "p1", "old"),
        prompt("b", 40, "p1", "tie b"),
        prompt("a", 40, "p1", "tie a"),
        other,
    ])
    .unwrap();
    let ids: Vec<String> = s
        .sessions_for_worktree(&wt("wt-1"))
        .unwrap()
        .into_iter()
        .map(|x| x.id.0)
        .collect();
    assert_eq!(ids, ["claude-code:a", "claude-code:b", "claude-code:s1"]);
    assert!(s.sessions_for_worktree(&wt("nope")).unwrap().is_empty());
    assert!(s.timeline(&key("missing")).unwrap().is_none());
}

fn append_reports_every_worktree_listing_a_changed_session(make: Make) {
    let f = fresh(make);
    let s = &*f.store;
    let mut unrelated = start("s2", 2, false);
    unrelated.worktree_id = Some(wt("wt-3"));
    s.append(&[start("s1", 1, false), unrelated.clone()])
        .unwrap();

    let mut resumed = start("s1", 3, true);
    resumed.worktree_id = Some(wt("wt-2"));
    let out = s.append(&[resumed, unrelated]).unwrap();
    assert_eq!(out, outcome(1, 1, &["wt-1", "wt-2"]));
    assert_eq!(
        s.sessions_for_worktree(&wt("wt-1")).unwrap(),
        s.sessions_for_worktree(&wt("wt-2")).unwrap()
    );

    // An event with no worktree still reports the worktrees from history.
    let mut unresolved = prompt("s1", 4, "p1", "cwd is unavailable");
    unresolved.worktree_id = None;
    assert_eq!(
        s.append(&[unresolved]).unwrap(),
        outcome(1, 0, &["wt-1", "wt-2"])
    );
}

fn status_is_unknown_without_lifecycle_events(make: Make) {
    let f = fresh(make);
    f.store.append(&[prompt("s1", 2, "p1", "hi")]).unwrap();
    assert_eq!(
        f.store.sessions_for_worktree(&wt("wt-1")).unwrap()[0].status,
        SessionStatus::Unknown
    );
}

fn prune_drops_whole_sessions_older_than_the_cutoff(make: Make) {
    let f = fresh(make);
    let s = &*f.store;
    s.append(&[
        prompt("old", 10, "p1", "old"),
        prompt("long", 10, "p1", "first"),
        prompt("long", 200, "p2", "recent"),
        prompt("new", 150, "p1", "new"),
        // Exactly at the cutoff: kept (the rule is "older than").
        prompt("edge", 100, "p1", "edge"),
    ])
    .unwrap();
    assert_eq!(s.prune_older_than(100).unwrap(), 1);
    assert!(s.timeline(&key("old")).unwrap().is_none());
    assert_eq!(s.timeline(&key("long")).unwrap().unwrap().turns.len(), 2);
    assert!(s.timeline(&key("new")).unwrap().is_some());
    assert!(s.timeline(&key("edge")).unwrap().is_some());
    assert_eq!(s.sessions_for_worktree(&wt("wt-1")).unwrap().len(), 3);
    // Pruned keys are forgotten: a late redelivery is stored again.
    assert_eq!(
        s.append(&[prompt("old", 10, "p1", "old")])
            .unwrap()
            .inserted,
        1
    );
    assert_eq!(s.prune_older_than(100).unwrap(), 1);
    // Nothing to prune; and a cutoff beyond everything prunes everything.
    assert_eq!(s.prune_older_than(100).unwrap(), 0);
    assert_eq!(s.prune_older_than(u64::MAX).unwrap(), 3);
    assert!(s.sessions_for_worktree(&wt("wt-1")).unwrap().is_empty());
}

/// A redelivery is a no-op EVEN when it carries a later timestamp: it must not
/// move the session in the list nor save it from retention.
fn a_duplicate_with_a_later_time_changes_nothing(make: Make) {
    let f = fresh(make);
    let s = &*f.store;
    let old = prompt("old", 10, "p1", "old");
    s.append(&[old.clone(), prompt("newer", 50, "p1", "newer")])
        .unwrap();
    let before = s.sessions_for_worktree(&wt("wt-1")).unwrap();

    // Same EventKey, but stamped much later (and in another worktree).
    let mut again = old.clone();
    again.at_ms = 900;
    again.worktree_id = Some(wt("wt-9"));
    assert_eq!(s.append(&[again]).unwrap(), outcome(0, 1, &[]));

    assert_eq!(s.sessions_for_worktree(&wt("wt-1")).unwrap(), before);
    assert!(s.sessions_for_worktree(&wt("wt-9")).unwrap().is_empty());
    // Still prunable at its ORIGINAL time.
    assert_eq!(s.prune_older_than(20).unwrap(), 1);
    assert!(s.timeline(&key("old")).unwrap().is_none());
}

/// A cutoff above every storable timestamp deletes everything, including a
/// session whose last event is the largest SQLite can store.
fn prune_with_a_huge_cutoff_removes_everything(make: Make) {
    let f = fresh(make);
    let s = &*f.store;
    let at_max = i64::MAX as u64;
    s.append(&[
        prompt("max", at_max, "p1", "last moment"),
        prompt("s1", 1, "p1", "x"),
    ])
    .unwrap();
    assert_eq!(s.prune_older_than(u64::MAX).unwrap(), 2);
    assert!(s.timeline(&key("max")).unwrap().is_none());
    assert!(s.sessions_for_worktree(&wt("wt-1")).unwrap().is_empty());
}

fn stale_live_sessions_become_unknown(make: Make) {
    let last = 5 * MIN;
    let status_at = |now: u64, events: &[NormalizedEvent]| {
        let f = make(Clock::fixed(now));
        f.store.append(events).unwrap();
        let listed = f.store.sessions_for_worktree(&wt("wt-1")).unwrap()[0].status;
        let timeline = f
            .store
            .timeline(&key("s1"))
            .unwrap()
            .unwrap()
            .session
            .status;
        assert_eq!(listed, timeline, "list and timeline agree");
        listed
    };
    let live = [start("s1", 1, false), prompt("s1", last, "p1", "work")];

    assert_eq!(status_at(last, &live), SessionStatus::Live);
    assert_eq!(
        status_at(last + 30 * MIN, &live),
        SessionStatus::Live,
        "exactly 30 min"
    );
    assert_eq!(
        status_at(last + 30 * MIN + 1, &live),
        SessionStatus::Unknown
    );
    // A clean end stays `ended`, however long ago.
    let ended = [start("s1", 1, false), end("s1", last, "p1")];
    assert_eq!(
        status_at(last + 24 * 60 * MIN, &ended),
        SessionStatus::Ended
    );
    // No lifecycle events: `unknown` already.
    let bare = [prompt("s1", last, "p1", "work")];
    assert_eq!(
        status_at(last + 24 * 60 * MIN, &bare),
        SessionStatus::Unknown
    );
    // Any new event revives it (activity resets the clock).
    let mut revived = live.to_vec();
    revived.push(prompt("s1", last + 40 * MIN, "p2", "back"));
    assert_eq!(status_at(last + 41 * MIN, &revived), SessionStatus::Live);
}

fn awkward_text_round_trips_exactly(make: Make) {
    let f = fresh(make);
    // Capped excerpts of multi-byte text right at the 8 KiB boundary.
    let cut = excerpt(&"世".repeat(10_000));
    let exact = "a".repeat(8 * 1024);
    let long_path = format!("/{}", "d/".repeat(60_000)); // ~120 KB
    let nul = "before\0after \u{1F600}";

    let mut a = prompt("s1", 1, "p1", &cut);
    a.files = vec![long_path.clone(), "dup".into(), "dup".into(), nul.into()];
    a.cwd = long_path.clone();
    let b = prompt("s1", 2, "p2", &exact);
    let mut c = prompt("s1", 3, "p3", nul);
    c.cwd = long_path.clone();
    // NUL inside identifiers, too.
    let d = prompt("s\0nul", 4, "turn\0x", "ids with NUL");
    f.store.append(&[a, b, c, d]).unwrap();

    let t = f.store.timeline(&key("s1")).unwrap().unwrap();
    assert_eq!(t.turns[0].events[0].summary, cut);
    assert!(cut.len() <= 8 * 1024 && cut.ends_with('…'));
    assert_eq!(
        t.turns[0].events[0].files,
        [long_path.as_str(), "dup", "dup", nul]
    );
    assert_eq!(t.session.cwd, long_path, "the last event's cwd");
    assert_eq!(t.turns[1].events[0].summary, exact);
    assert_eq!(t.turns[2].events[0].summary, nul);
    let t = f.store.timeline(&key("s\0nul")).unwrap().unwrap();
    assert_eq!(t.turns[0].id, "turn\0x");
}

/// Both stores in sync: `$modname::$case` for every case in this file.
macro_rules! conformance {
    ($modname:ident, $make:path) => {
        mod $modname {
            use super::*;
            macro_rules! case {
                ($case:ident) => {
                    #[test]
                    fn $case() {
                        super::$case($make);
                    }
                };
            }
            case!(idempotent_redelivery);
            case!(pre_and_post_are_both_kept_and_paired);
            case!(post_before_pre_still_pairs);
            case!(pre_without_post_is_unknown);
            case!(merged_tool_uses_the_starts_arrival_order);
            case!(resume_is_the_same_session);
            case!(equal_timestamps_keep_arrival_order);
            case!(equal_timestamps_keep_arrival_order_across_appends);
            case!(turns_are_ordered_and_closed_by_stop);
            case!(events_without_a_turn_id_go_to_the_empty_id_turn);
            case!(worktree_filtering_and_sorting);
            case!(append_reports_every_worktree_listing_a_changed_session);
            case!(status_is_unknown_without_lifecycle_events);
            case!(prune_drops_whole_sessions_older_than_the_cutoff);
            case!(a_duplicate_with_a_later_time_changes_nothing);
            case!(prune_with_a_huge_cutoff_removes_everything);
            case!(stale_live_sessions_become_unknown);
            case!(awkward_text_round_trips_exactly);
        }
    };
}

conformance!(memory_store, memory);
conformance!(sqlite_store, sqlite);

// ---- Differential: same history in, identical answers out -------------------

/// A tiny deterministic generator (no `rand` dependency): the same seed always
/// makes the same history, so a failure is reproducible.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }
}

fn random_history(seed: u64, n: usize) -> Vec<NormalizedEvent> {
    let mut rng = Lcg(seed);
    let mut out = Vec::new();
    for i in 0..n {
        let session = format!("s{}", rng.next(4));
        let turn = format!("p{}", rng.next(3));
        // A small timestamp range on purpose: many ties.
        let at = 1 + rng.next(40);
        let use_id = format!("toolu_{}", rng.next(5));
        let mut ev = match rng.next(8) {
            0 => start(&session, at, rng.next(2) == 0),
            1 => end(&session, at, &turn),
            2 => prompt(&session, at, &turn, &format!("prompt {i}")),
            3 => stop(&session, at, &turn),
            4 | 5 => tool(&session, &use_id, None, at, &turn),
            6 => tool(&session, &use_id, Some(ToolOutcome::Ok), at, &turn),
            _ => tool(&session, &use_id, Some(ToolOutcome::Failed), at, &turn),
        };
        ev.worktree_id = match rng.next(4) {
            0 => None,
            1 => Some(wt("wt-2")),
            _ => Some(wt("wt-1")),
        };
        ev.files = vec![format!("f{}", rng.next(3))];
        // Redeliveries: some events repeat an earlier key.
        if !out.is_empty() && rng.next(6) == 0 {
            let earlier: &NormalizedEvent = &out[rng.next(out.len() as u64) as usize];
            ev = earlier.clone();
        }
        out.push(ev);
    }
    out
}

#[test]
fn differential_random_histories_agree() {
    for seed in 0..25 {
        for now in [NOW, 100 * MIN] {
            let mem = memory(Clock::fixed(now));
            let sql = sqlite(Clock::fixed(now));
            let history = random_history(seed, 120);
            let mut rng = Lcg(seed ^ 0xABCDEF);
            let mut rest = &history[..];
            while !rest.is_empty() {
                let take = (1 + rng.next(9) as usize).min(rest.len());
                let (batch, tail) = rest.split_at(take);
                rest = tail;
                assert_eq!(
                    mem.store.append(batch).unwrap(),
                    sql.store.append(batch).unwrap(),
                    "seed {seed}: append outcome"
                );
            }
            for w in ["wt-1", "wt-2", "wt-none"] {
                assert_eq!(
                    mem.store.sessions_for_worktree(&wt(w)).unwrap(),
                    sql.store.sessions_for_worktree(&wt(w)).unwrap(),
                    "seed {seed} now {now}: sessions of {w}"
                );
            }
            let sessions: HashSet<_> = history.iter().map(|e| e.session.clone()).collect();
            for session in sessions {
                assert_eq!(
                    mem.store.timeline(&session).unwrap(),
                    sql.store.timeline(&session).unwrap(),
                    "seed {seed} now {now}: timeline of {}",
                    session.session_id
                );
            }
            assert_eq!(
                mem.store.prune_older_than(20).unwrap(),
                sql.store.prune_older_than(20).unwrap(),
                "seed {seed}: prune count"
            );
            for w in ["wt-1", "wt-2"] {
                assert_eq!(
                    mem.store.sessions_for_worktree(&wt(w)).unwrap(),
                    sql.store.sessions_for_worktree(&wt(w)).unwrap(),
                    "seed {seed}: sessions of {w} after prune"
                );
            }
        }
    }
}
