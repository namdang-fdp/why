//! Draining the spool: idempotent, crash-safe, and race-free against hooks that
//! keep appending.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use why_lib::collector::spool_drain::SpoolDrainer;
use why_lib::collector::{ChangeNotifier, GitWorktreeResolver, Pipeline, Stats};
use why_lib::contracts::{SessionSummary, SessionTimeline, WorktreeId};
use why_lib::events::{AppendOutcome, EventStore, NormalizedEvent, RETENTION, SessionKey};
use why_lib::store::StoreError;
use why_wire::spool::{self, MAX_SPOOL_BYTES};
use why_wire::{Harness, encode_line};

use super::*;

fn spool_line(rig: &Rig, session: &str) -> Vec<u8> {
    encode_line(&envelope(session, &rig.repo.path, now_ms())).unwrap()
}

fn append(rig: &Rig, session: &str) {
    spool::append_line(
        &rig.paths.spool_dir,
        &spool_line(rig, session),
        now_ms(),
        MAX_SPOOL_BYTES,
    )
    .unwrap();
}

fn drainer(rig: &Rig, events: Arc<dyn EventStore>) -> (SpoolDrainer, Arc<Stats>) {
    let stats = Arc::new(Stats::default());
    let pipeline = Arc::new(Pipeline::new(
        Arc::new(TestNormalizer),
        GitWorktreeResolver::new(git_common::test_runner(), Arc::clone(&rig.repos)),
        events,
        ChangeNotifier::new(Arc::new(|_| {}), Duration::from_millis(10)),
    ));
    (
        SpoolDrainer::new(rig.paths.spool_dir.clone(), pipeline, Arc::clone(&stats)),
        stats,
    )
}

fn spool_files(dir: &Path) -> usize {
    spool::pending_files(dir).len()
}

#[tokio::test]
async fn draining_twice_stores_each_event_once_and_deletes_the_file() {
    let rig = Rig::new();
    for i in 0..5 {
        append(&rig, &format!("s{i}"));
    }
    let (d, stats) = drainer(&rig, Arc::clone(&rig.events) as _);
    let first = d.drain_once().await;
    assert_eq!((first.ingested, first.files_done), (5, 1));
    assert_eq!(rig.sessions(), 5);
    assert_eq!(
        spool_files(&rig.paths.spool_dir),
        0,
        "file deleted after full ingest"
    );

    let second = d.drain_once().await;
    assert_eq!(second.ingested, 0);
    assert_eq!(rig.sessions(), 5, "no duplicates");

    // The same bytes arriving AGAIN (a hook that sent over the socket and then
    // also spooled, or a copied file) are recognised by their event key.
    for i in 0..5 {
        append(&rig, &format!("s{i}"));
    }
    let third = d.drain_once().await;
    assert_eq!(third.ingested, 5, "redelivered");
    assert_eq!(rig.sessions(), 5, "...but stored once");
    assert_eq!(stats.snapshot().spool_ingested, 10);
}

#[tokio::test]
async fn startup_drain_picks_up_events_spooled_while_the_app_was_down() {
    let rig = Rig::new();
    append(&rig, "from-spool");
    let _core = rig.start().await;
    eventually(Duration::from_secs(5), "spool ingested at start", || {
        rig.has_session("from-spool")
    })
    .await;
    eventually(Duration::from_secs(5), "file removed", || {
        spool_files(&rig.paths.spool_dir) == 0
    })
    .await;
}

#[tokio::test]
async fn bad_lines_are_skipped_and_the_file_is_still_deleted() {
    let rig = Rig::new();
    let dir = &rig.paths.spool_dir;
    std::fs::create_dir_all(dir).unwrap();
    let mut content = Vec::new();
    content.extend(b"garbage\n\n");
    content.extend(spool_line(&rig, "ok1"));
    content.extend(b"{\"v\":42}\n");
    content.extend(spool_line(&rig, "ok2"));
    content.extend(b"{\"v\":1,\"torn"); // a torn append: no newline
    std::fs::write(dir.join("2026-11-20.jsonl"), content).unwrap();
    let (d, _) = drainer(&rig, Arc::clone(&rig.events) as _);
    let r = d.drain_once().await;
    assert_eq!((r.ingested, r.malformed, r.files_done), (2, 3, 1));
    assert!(rig.has_session("ok1") && rig.has_session("ok2"));
    assert_eq!(spool_files(dir), 0);
}

#[tokio::test]
async fn a_leftover_from_a_crashed_drain_is_ingested() {
    let rig = Rig::new();
    let dir = &rig.paths.spool_dir;
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("2026-11-19.jsonl.draining-5"),
        spool_line(&rig, "left"),
    )
    .unwrap();
    let (d, _) = drainer(&rig, Arc::clone(&rig.events) as _);
    assert_eq!(d.drain_once().await.files_done, 1);
    assert!(rig.has_session("left"));
}

/// Fails the first `n` appends, then behaves.
struct Flaky {
    inner: MemoryEventStore,
    failures_left: AtomicUsize,
}

impl EventStore for Flaky {
    fn append(&self, events: &[NormalizedEvent]) -> Result<AppendOutcome, StoreError> {
        if self.failures_left.load(Ordering::SeqCst) > 0 {
            self.failures_left.fetch_sub(1, Ordering::SeqCst);
            return Err(StoreError::Backend("disk full (simulated)".into()));
        }
        self.inner.append(events)
    }
    fn sessions_for_worktree(&self, wt: &WorktreeId) -> Result<Vec<SessionSummary>, StoreError> {
        self.inner.sessions_for_worktree(wt)
    }
    fn timeline(&self, s: &SessionKey) -> Result<Option<SessionTimeline>, StoreError> {
        self.inner.timeline(s)
    }
    fn prune_older_than(&self, cutoff_ms: u64) -> Result<usize, StoreError> {
        self.inner.prune_older_than(cutoff_ms)
    }
}

#[tokio::test]
async fn a_store_failure_keeps_the_file_and_the_retry_loses_and_duplicates_nothing() {
    let rig = Rig::new();
    for i in 0..3 {
        append(&rig, &format!("f{i}"));
    }
    let flaky = Arc::new(Flaky {
        inner: MemoryEventStore::new(),
        failures_left: AtomicUsize::new(1),
    });
    let (d, stats) = drainer(&rig, Arc::clone(&flaky) as _);

    let first = d.drain_once().await;
    assert_eq!(first.files_done, 0);
    assert_eq!(first.files_deferred, 1);
    assert_eq!(
        spool_files(&rig.paths.spool_dir),
        1,
        "file must survive a store error"
    );
    assert_eq!(stats.snapshot().ingest_errors, 1);

    let second = d.drain_once().await;
    assert_eq!(second.files_done, 1);
    assert_eq!(spool_files(&rig.paths.spool_dir), 0);
    assert_eq!(
        flaky
            .inner
            .sessions_for_worktree(&rig.main_wt)
            .unwrap()
            .len(),
        3
    );
}

/// The race this design exists for: hooks keep appending while the app drains.
/// Every appended event must end up stored, none lost to a rename or delete.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_lost_when_hooks_append_during_a_drain() {
    const TOTAL: usize = 600;
    let rig = Rig::new();
    let (d, _) = drainer(&rig, Arc::clone(&rig.events) as _);
    let dir = rig.paths.spool_dir.clone();
    let lines: Vec<Vec<u8>> = (0..TOTAL)
        .map(|i| spool_line(&rig, &format!("r{i}")))
        .collect();

    // Writers behave like hooks: open, append one line, close, repeat.
    let writers: Vec<_> = lines
        .chunks(TOTAL / 3)
        .map(|chunk| {
            let (dir, chunk) = (dir.clone(), chunk.to_vec());
            std::thread::spawn(move || {
                for line in chunk {
                    // A hook that finds the spool busy retries a few times
                    // inside `append_line`; at worst the test retries.
                    let mut tries = 0;
                    while spool::append_line(&dir, &line, now_ms(), MAX_SPOOL_BYTES).is_err() {
                        tries += 1;
                        assert!(tries < 100);
                    }
                }
            })
        })
        .collect();

    while writers.iter().any(|w| !w.is_finished()) {
        d.drain_once().await;
    }
    for w in writers {
        w.join().unwrap();
    }
    d.drain_once().await;

    assert_eq!(rig.sessions(), TOTAL, "an event was lost");
    assert_eq!(spool_files(&dir), 0);
    let _ = Harness::Codex; // (imported for the key type used above)
}

fn day_ms() -> u64 {
    24 * 60 * 60 * 1000
}

/// A torn append (disk full mid-write) leaves a fragment without its newline.
/// The next record must still be ingested.
#[tokio::test]
async fn a_good_record_after_a_torn_fragment_is_ingested() {
    let rig = Rig::new();
    let dir = &rig.paths.spool_dir;
    std::fs::create_dir_all(dir).unwrap();
    let file = dir.join(format!("{}.jsonl", spool::date_stamp(now_ms())));
    std::fs::write(&file, b"\n{\"v\":1,\"harness\":\"claude-co").unwrap();
    append(&rig, "after-the-tear");
    let (d, _) = drainer(&rig, Arc::clone(&rig.events) as _);
    let r = d.drain_once().await;
    assert_eq!((r.ingested, r.malformed, r.files_done), (1, 1, 1));
    assert!(rig.has_session("after-the-tear"));
}

/// Records older than the retention window are not ingested (the prune would
/// only delete them again), and an old file is deleted without being read.
#[tokio::test]
async fn expired_spool_records_are_never_replayed() {
    let rig = Rig::new();
    let old_ms = now_ms() - RETENTION.as_millis() as u64 - day_ms();
    let dir = &rig.paths.spool_dir;
    std::fs::create_dir_all(dir).unwrap();

    // 1. One file mixing an old and a fresh record (file mtime is recent).
    let mut mixed = Vec::new();
    mixed.extend(b"\n");
    mixed.extend(encode_line(&envelope("old-record", &rig.repo.path, old_ms)).unwrap());
    mixed.extend(b"\n");
    mixed.extend(spool_line(&rig, "fresh-record"));
    std::fs::write(dir.join("2026-11-20.jsonl"), mixed).unwrap();

    // 2. A whole file nobody appended to for longer than the window, even
    // though its lines CLAIM to be recent: deleted unread.
    let stale_file = dir.join("2026-01-01.jsonl");
    std::fs::write(&stale_file, spool_line(&rig, "in-stale-file")).unwrap();
    let long_ago = std::time::SystemTime::now()
        - std::time::Duration::from_millis(RETENTION.as_millis() as u64 + day_ms());
    std::fs::File::options()
        .write(true)
        .open(&stale_file)
        .unwrap()
        .set_modified(long_ago)
        .unwrap();

    let (d, stats) = drainer(&rig, Arc::clone(&rig.events) as _);
    let r = d.drain_once().await;
    assert_eq!((r.ingested, r.expired, r.files_done), (1, 2, 2));
    assert!(rig.has_session("fresh-record"));
    assert!(!rig.has_session("old-record") && !rig.has_session("in-stale-file"));
    assert_eq!(spool_files(dir), 0, "expired files are deleted too");
    assert_eq!(stats.snapshot().spool_expired, 2);
}
