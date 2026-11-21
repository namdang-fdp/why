//! WHY-55: `EventStore::last_event_ms` (the newest event of a harness), run
//! against BOTH stores.

use why_lib::events::{Clock, EventKind};
use why_wire::Harness;

use crate::common::*;
use crate::conformance::{Fixture, memory, sqlite};

type Make = fn(Clock) -> Fixture;

fn fresh(make: Make) -> Fixture {
    make(Clock::fixed(1_000))
}

fn codex_event(session: &str, at: u64) -> why_lib::events::NormalizedEvent {
    let mut ev = event(session, "Stop", EventKind::Stop, at, Some("t1"));
    ev.session = why_lib::events::SessionKey::new(Harness::Codex, session);
    ev.key = why_lib::events::EventKey(format!("codex-{session}-{at}"));
    ev
}

fn last_event_is_per_harness(make: Make) {
    let f = fresh(make);
    // Nothing yet: "none", for both.
    assert_eq!(f.store.last_event_ms(Harness::ClaudeCode).unwrap(), None);
    assert_eq!(f.store.last_event_ms(Harness::Codex).unwrap(), None);

    // Claude only. The newest event wins whatever its kind, and arrival order
    // does not matter (a late, older delivery cannot move it back).
    f.store
        .append(&[
            start("c1", 50, false),
            prompt("c1", 90, "p1", "hi"),
            start("c2", 70, false),
        ])
        .unwrap();
    assert_eq!(
        f.store.last_event_ms(Harness::ClaudeCode).unwrap(),
        Some(90)
    );
    assert_eq!(f.store.last_event_ms(Harness::Codex).unwrap(), None);

    // Both, each with its own time.
    f.store
        .append(&[codex_event("x1", 40), codex_event("x1", 60)])
        .unwrap();
    assert_eq!(f.store.last_event_ms(Harness::Codex).unwrap(), Some(60));
    assert_eq!(
        f.store.last_event_ms(Harness::ClaudeCode).unwrap(),
        Some(90)
    );
    assert_eq!(f.store.last_event_ms(Harness::Unknown).unwrap(), None);

    // Pruned sessions no longer count.
    assert_eq!(f.store.prune_older_than(65).unwrap(), 1);
    assert_eq!(f.store.last_event_ms(Harness::Codex).unwrap(), None);
    assert_eq!(
        f.store.last_event_ms(Harness::ClaudeCode).unwrap(),
        Some(90)
    );
}

macro_rules! last_event_conformance {
    ($modname:ident, $make:path) => {
        mod $modname {
            use super::*;
            #[test]
            fn last_event_is_per_harness() {
                super::last_event_is_per_harness($make);
            }
        }
    };
}

last_event_conformance!(memory_store, memory);
last_event_conformance!(sqlite_store, sqlite);
