//! The Kaneo acceptance benchmark: 50,000 events. `#[ignore]`d so it stays out of
//! the normal suite; run it with
//!
//! ```text
//! cargo test --release --manifest-path src-tauri/Cargo.toml --test events \
//!     -- --ignored --nocapture bench
//! ```

use std::time::{Duration, Instant};

use why_lib::contracts::ToolOutcome;
use why_lib::events::{EventStore, NormalizedEvent, SessionKey, SqliteEventStore};
use why_wire::Harness;

use crate::common::*;

const TOTAL: usize = 50_000;
const SESSIONS: usize = 100;
const BIG_SESSION: usize = 2_250;

/// One realistic session worth of events: a start, then turns of
/// prompt -> (tool Pre + Post) x k -> assistant message -> stop. `n` events
/// exactly; `base_ms` keeps timestamps increasing.
fn session_events(id: &str, n: usize, base_ms: u64, counter: &mut u64) -> Vec<NormalizedEvent> {
    let mut out = Vec::with_capacity(n);
    let mut at = base_ms;
    let mut push = |mut ev: NormalizedEvent, out: &mut Vec<NormalizedEvent>| {
        *counter += 1;
        ev.key = why_lib::events::EventKey(format!("b{counter}"));
        out.push(ev);
    };
    push(start(id, at, false), &mut out);
    let mut turn = 0;
    while out.len() < n {
        turn += 1;
        let t = format!("p{turn}");
        at += 1_000;
        let mut p = prompt(
            id,
            at,
            &t,
            &"please refactor the module and keep the tests green. ".repeat(4),
        );
        p.files = vec![];
        push(p, &mut out);
        for call in 0..8 {
            if out.len() + 2 > n {
                break;
            }
            let use_id = format!("{t}-c{call}");
            at += 200;
            let mut pre = tool(id, &use_id, None, at, &t);
            pre.text = Some("cargo test --workspace -- --nocapture".into());
            pre.files = vec![format!("/repo/src/module_{call}.rs")];
            push(pre, &mut out);
            at += 800;
            push(tool(id, &use_id, Some(ToolOutcome::Ok), at, &t), &mut out);
        }
        if out.len() < n {
            at += 500;
            push(stop(id, at, &t), &mut out);
        }
    }
    out.truncate(n);
    out
}

fn history() -> Vec<NormalizedEvent> {
    let mut counter = 0;
    let rest = (TOTAL - BIG_SESSION) / (SESSIONS - 1);
    let mut all = session_events("big", BIG_SESSION, 1_000, &mut counter);
    for i in 1..SESSIONS {
        let n = if i == SESSIONS - 1 {
            TOTAL - BIG_SESSION - rest * (SESSIONS - 2)
        } else {
            rest
        };
        all.extend(session_events(
            &format!("s{i}"),
            n,
            1_000 + i as u64,
            &mut counter,
        ));
    }
    assert_eq!(all.len(), TOTAL);
    all
}

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let v = f();
    (v, start.elapsed())
}

fn run(batch_size: usize) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteEventStore::open(&dir.path().join("events.db")).unwrap();
    // Each session's events arrive in order, session after session, in batches
    // of `batch_size` (a batch may span two sessions at a boundary).
    let events = history();
    let (_, append) = time(|| {
        for chunk in events.chunks(batch_size) {
            store.append(chunk).unwrap();
        }
    });
    let per_event = append.as_secs_f64() * 1e6 / TOTAL as f64;
    println!(
        "batch={batch_size:>3}: append {TOTAL} events in {append:?} ({per_event:.1} us/event, {:.0} events/s)",
        TOTAL as f64 / append.as_secs_f64()
    );

    // Best of 5 so a cold cache does not dominate.
    let best = |f: &dyn Fn()| (0..5).map(|_| time(f).1).min().unwrap();
    let list = best(&|| {
        let s = store.sessions_for_worktree(&wt("wt-1")).unwrap();
        assert_eq!(s.len(), SESSIONS);
    });
    let timeline = best(&|| {
        let t = store
            .timeline(&SessionKey::new(Harness::ClaudeCode, "big"))
            .unwrap()
            .unwrap();
        assert!(t.turns.len() > 10);
    });
    println!(
        "batch={batch_size:>3}: sessions_for_worktree (100 sessions, 50k events) best of 5: {list:?}"
    );
    println!("batch={batch_size:>3}: timeline (2,250-event session) best of 5: {timeline:?}");
    let size = std::fs::metadata(dir.path().join("events.db"))
        .unwrap()
        .len();
    println!(
        "batch={batch_size:>3}: events.db main file {:.1} MiB",
        size as f64 / 1_048_576.0
    );
}

#[test]
#[ignore = "benchmark: run with --release --ignored --nocapture"]
fn bench_50k_events_batch_1() {
    run(1);
}

#[test]
#[ignore = "benchmark: run with --release --ignored --nocapture"]
fn bench_50k_events_batch_50() {
    run(50);
}
