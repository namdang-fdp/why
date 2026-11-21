//! What only the SQLite store has: persistence, concurrency, space reuse and the
//! query plans of its two read paths.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use rusqlite::{Connection, OpenFlags};
use why_lib::contracts::ToolOutcome;
use why_lib::events::sqlite::{SCHEMA_VERSION, SESSION_EVENTS_SQL, WORKTREE_SESSIONS_SQL};
use why_lib::events::{Clock, EventStore, NormalizedEvent, SessionKey, SqliteEventStore};
use why_wire::Harness;

use crate::common::*;

fn db(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("events.db")
}

fn key(session: &str) -> SessionKey {
    SessionKey::new(Harness::ClaudeCode, session)
}

fn pragma(path: &std::path::Path, name: &str) -> i64 {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    conn.pragma_query_value(None, name, |r| r.get(0)).unwrap()
}

/// A realistic mixed history for persistence checks.
fn history() -> Vec<NormalizedEvent> {
    vec![
        start("s1", 1, false),
        prompt("s1", 2, "p1", "fix the bug"),
        tool("s1", "t1", None, 3, "p1"),
        tool("s1", "t1", Some(ToolOutcome::Ok), 4, "p1"),
        stop("s1", 5, "p1"),
        prompt("s2", 6, "p1", "other session"),
    ]
}

#[test]
fn file_database_uses_wal_incremental_vacuum_and_current_version() {
    let dir = tempfile::tempdir().unwrap();
    drop(SqliteEventStore::open(&db(&dir)).unwrap());
    let conn = Connection::open(db(&dir)).unwrap();
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
    assert_eq!(pragma(&db(&dir), "user_version"), SCHEMA_VERSION as i64);
    // 2 = INCREMENTAL. It only sticks if set before the first write.
    assert_eq!(pragma(&db(&dir), "auto_vacuum"), 2);
}

#[test]
fn restart_gives_the_same_timeline_and_keeps_deduplicating() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Clock::fixed(1_000);
    let (before_timeline, before_list) = {
        let store = SqliteEventStore::open(&db(&dir))
            .unwrap()
            .with_clock(clock.clone());
        store.append(&history()).unwrap();
        (
            store.timeline(&key("s1")).unwrap().unwrap(),
            store.sessions_for_worktree(&wt("wt-1")).unwrap(),
        )
    }; // dropped: both connections closed

    for _ in 0..2 {
        // Reopening twice also shows the migration is idempotent.
        let store = SqliteEventStore::open(&db(&dir))
            .unwrap()
            .with_clock(clock.clone());
        assert_eq!(
            store.timeline(&key("s1")).unwrap().unwrap(),
            before_timeline
        );
        assert_eq!(
            store.sessions_for_worktree(&wt("wt-1")).unwrap(),
            before_list
        );
        let again = store.append(&history()).unwrap();
        assert_eq!(
            (again.inserted, again.duplicates),
            (0, 6),
            "keys survive restarts"
        );
    }
    assert_eq!(pragma(&db(&dir), "user_version"), SCHEMA_VERSION as i64);
}

#[test]
fn arrival_order_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let texts = ["one", "two", "three"];
    {
        let store = SqliteEventStore::open(&db(&dir)).unwrap();
        for (i, t) in texts.iter().enumerate() {
            let mut e = prompt("s1", 5, "p1", t); // equal timestamps
            e.key = why_lib::events::EventKey(format!("k{i}"));
            store.append(&[e]).unwrap();
        }
    }
    let store = SqliteEventStore::open(&db(&dir)).unwrap();
    let t = store.timeline(&key("s1")).unwrap().unwrap();
    let got: Vec<&str> = t.turns[0]
        .events
        .iter()
        .map(|e| e.summary.as_str())
        .collect();
    assert_eq!(got, texts);
}

#[test]
fn two_threads_appending_the_same_batch_store_each_event_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteEventStore::open(&db(&dir)).unwrap());
    let batch: Vec<_> = (0..500)
        .map(|n| with_key(prompt("s1", 1 + n, "p1", "x"), n))
        .collect();
    let batch = Arc::new(batch);
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let (store, batch) = (Arc::clone(&store), Arc::clone(&batch));
            thread::spawn(move || store.append(&batch).unwrap())
        })
        .collect();
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let inserted: usize = outcomes.iter().map(|o| o.inserted).sum();
    let duplicates: usize = outcomes.iter().map(|o| o.duplicates).sum();
    assert_eq!((inserted, duplicates), (500, 500), "{outcomes:?}");
    let t = store.timeline(&key("s1")).unwrap().unwrap();
    assert_eq!(t.turns[0].events.len(), 500);
}

/// Two store instances = two writer connections on one file (a second app
/// instance, or the `sqlite3` CLI). `busy_timeout` makes them take turns and
/// the UNIQUE key still stores each event once.
#[test]
fn two_instances_on_one_file_store_each_event_once() {
    let dir = tempfile::tempdir().unwrap();
    let a = Arc::new(SqliteEventStore::open(&db(&dir)).unwrap());
    let b = Arc::new(SqliteEventStore::open(&db(&dir)).unwrap());
    let batch: Arc<Vec<_>> = Arc::new(
        (0..300)
            .map(|n| with_key(prompt("s1", 1 + n, "p1", "x"), n))
            .collect(),
    );
    let handles: Vec<_> = [a, b]
        .into_iter()
        .map(|s| {
            let batch = Arc::clone(&batch);
            thread::spawn(move || s.append(&batch).unwrap())
        })
        .collect();
    let total: usize = handles
        .into_iter()
        .map(|h| h.join().unwrap().inserted)
        .sum();
    assert_eq!(total, 300);
}

/// A reader polling while one huge batch commits sees either nothing or ALL of
/// it: never a half-written batch.
#[test]
fn a_reader_during_a_long_write_sees_a_consistent_snapshot() {
    const N: u64 = 30_000;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteEventStore::open(&db(&dir)).unwrap());
    let batch: Vec<_> = (0..N)
        .map(|n| with_key(prompt("s1", 1 + n, "p1", &"word ".repeat(40)), n))
        .collect();

    let started = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let (store, started, done) = (Arc::clone(&store), started.clone(), done.clone());
        thread::spawn(move || {
            // Sets `done` when the thread ends for ANY reason (success, `Err`,
            // panic), so the reader below can never wait for it forever.
            struct SignalOnDrop(Arc<AtomicBool>);
            impl Drop for SignalOnDrop {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _signal = SignalOnDrop(done);
            started.store(true, Ordering::SeqCst);
            store.append(&batch)
        })
    };

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let (mut reads, mut overlapped) = (0, 0);
    let mut seen = std::collections::BTreeSet::new();
    while !done.load(Ordering::SeqCst) || reads < 3 {
        assert!(
            std::time::Instant::now() < deadline,
            "writer did not finish in 120 s"
        );
        let was_writing = started.load(Ordering::SeqCst) && !done.load(Ordering::SeqCst);
        let count = store
            .timeline(&key("s1"))
            .unwrap()
            .map_or(0, |t| t.turns[0].events.len() as u64);
        if was_writing && !done.load(Ordering::SeqCst) {
            overlapped += 1;
        }
        seen.insert(count);
        reads += 1;
    }
    // `join().unwrap()` re-raises a writer panic; the inner `unwrap` surfaces an
    // `Err` from `append`: either way the test fails instead of hanging.
    let appended = writer.join().expect("writer thread panicked").unwrap();
    assert_eq!(appended.inserted as u64, N);
    assert!(
        seen.iter().all(|&c| c == 0 || c == N),
        "a half-written batch was visible: {seen:?}"
    );
    println!("snapshot test: {reads} reads, {overlapped} overlapped the write, saw {seen:?}");
}

#[test]
fn prune_returns_space_to_the_file_and_leaves_no_orphans() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteEventStore::open(&db(&dir)).unwrap();
    // 20 sessions x 200 events with real text, so there are pages to free.
    let events: Vec<_> = (0..4_000u64)
        .map(|n| {
            let session = format!("s{}", n % 20);
            with_key(prompt(&session, 10 + n, "p1", &"payload ".repeat(100)), n)
        })
        .collect();
    store.append(&events).unwrap();
    assert_eq!(store.prune_older_than(u64::MAX).unwrap(), 20);

    let conn = Connection::open_with_flags(db(&dir), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    // ON DELETE CASCADE: events and worktree links went with their sessions.
    assert_eq!(
        (
            count("sessions"),
            count("events"),
            count("session_worktrees")
        ),
        (0, 0, 0)
    );
    // incremental_vacuum handed the pages back; none are left on the freelist.
    let free: i64 = conn
        .pragma_query_value(None, "freelist_count", |r| r.get(0))
        .unwrap();
    assert_eq!(
        free, 0,
        "freed pages were returned, not left on the freelist"
    );
}

#[test]
fn query_plans_use_the_indexes() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteEventStore::open(&db(&dir)).unwrap();
    store.append(&history()).unwrap();
    let conn = Connection::open_with_flags(db(&dir), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();

    let plan = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> Vec<String> {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        stmt.query_map(args, |r| r.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let timeline = plan(SESSION_EVENTS_SQL, &[&"claude-code", &"s1"]);
    let worktree = plan(WORKTREE_SESSIONS_SQL, &[&"wt-1"]);
    let prune = plan("DELETE FROM sessions WHERE last_event_ms < ?1", &[&1i64]);
    println!("PLAN timeline  : {timeline:?}");
    println!("PLAN worktree  : {worktree:?}");
    println!("PLAN prune     : {prune:?}");

    let joined = |p: &[String]| p.join(" | ");
    assert!(
        joined(&timeline).contains("events_by_session"),
        "{timeline:?}"
    );
    assert!(
        !joined(&timeline).contains("TEMP B-TREE"),
        "the index is already in arrival order: no sort step: {timeline:?}"
    );
    assert!(joined(&worktree).contains("PRIMARY KEY"), "{worktree:?}");
    assert!(
        !joined(&worktree).contains("SCAN"),
        "no full scan: {worktree:?}"
    );
    assert!(
        joined(&prune).contains("sessions_by_last_event"),
        "{prune:?}"
    );
}

/// Owner-only (decision 22: the spool is 0600, so is the durable copy). SQLite
/// gives `-wal`/`-shm` the main file's mode. The umask is process-wide, so the
/// test does not change it: it checks the creation mode (0600 even where the
/// umask would allow 0644) and that a looser existing file is tightened.
#[cfg(unix)]
#[test]
fn the_database_and_its_wal_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let sidecar = |dir: &tempfile::TempDir, ext: &str| dir.path().join(format!("events.db-{ext}"));

    let dir = tempfile::tempdir().unwrap();
    {
        let store = SqliteEventStore::open(&db(&dir)).unwrap();
        store.append(&history()).unwrap();
        // Alive: -wal and -shm exist now.
        assert_eq!(mode(&db(&dir)), 0o600, "main file");
        assert_eq!(mode(&sidecar(&dir, "wal")), 0o600, "-wal");
        assert_eq!(mode(&sidecar(&dir, "shm")), 0o600, "-shm");
    }

    // An existing file that is too open (written by a build before this rule).
    std::fs::set_permissions(db(&dir), std::fs::Permissions::from_mode(0o644)).unwrap();
    let store = SqliteEventStore::open(&db(&dir)).unwrap();
    store.append(&history()).unwrap();
    assert_eq!(mode(&db(&dir)), 0o600, "tightened on open");
    assert_eq!(
        mode(&sidecar(&dir, "wal")),
        0o600,
        "-wal of the reopened file"
    );
}
