//! Files the store must NOT touch: if `events.db` cannot be opened the app falls
//! back to memory, and the file on disk stays byte-for-byte as it was.

use std::fs;
use std::path::Path;

use rusqlite::Connection;
use why_lib::events::{Clock, EventStore, SqliteEventStore, open_event_store};
use why_wire::Harness;

use crate::common::*;

/// The fallback store must work: append, list and read back.
fn assert_usable(store: &dyn EventStore) {
    store
        .append(&[prompt("s1", 1, "p1", "still works")])
        .unwrap();
    let key = why_lib::events::SessionKey::new(Harness::ClaudeCode, "s1");
    assert!(store.timeline(&key).unwrap().is_some());
    assert_eq!(store.sessions_for_worktree(&wt("wt-1")).unwrap().len(), 1);
}

/// Open must fail, the fallback must work, and the file must be unchanged.
/// (Main file only: opening a WAL database may legitimately create `-wal`/`-shm`
/// side files.)
fn assert_refused_untouched(dir: &Path, expect_in_error: &str) {
    let path = dir.join("events.db");
    let before = fs::read(&path).unwrap();
    let err = SqliteEventStore::open(&path).unwrap_err().to_string();
    assert!(err.contains(expect_in_error), "error was: {err}");
    assert_eq!(fs::read(&path).unwrap(), before, "open() changed the file");

    let fallback = open_event_store(Ok(dir), Clock::fixed(1_000));
    assert_usable(&*fallback);
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "fallback changed the file"
    );
}

#[test]
fn a_file_of_garbage_falls_back_and_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    // Not even the SQLite header: 8 KiB of a repeating pattern.
    let garbage: Vec<u8> = (0..8192u32).map(|i| (i * 31 % 251) as u8).collect();
    fs::write(dir.path().join("events.db"), &garbage).unwrap();
    assert_refused_untouched(dir.path(), "not a database");
}

#[test]
fn a_database_from_a_newer_why_falls_back_and_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    {
        let conn = Connection::open(dir.path().join("events.db")).unwrap();
        conn.execute_batch("CREATE TABLE future (x); PRAGMA user_version = 99;")
            .unwrap();
    }
    assert_refused_untouched(dir.path(), "newer");
}

/// A real, current-version database, then damaged with `sql` (via a plain
/// connection, so the file stays a valid SQLite file at `user_version = 1`).
fn damaged_v1(sql: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.db");
    drop(SqliteEventStore::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(sql).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1, "premise: the version still says current");
    drop(conn);
    dir
}

#[test]
fn a_current_version_database_missing_a_table_falls_back_and_is_left_alone() {
    let dir = damaged_v1("DROP TABLE session_worktrees;");
    assert_refused_untouched(dir.path(), "session_worktrees");
}

#[test]
fn a_current_version_database_missing_a_column_falls_back_and_is_left_alone() {
    let dir = damaged_v1("ALTER TABLE events DROP COLUMN transcript_path;");
    assert_refused_untouched(dir.path(), "transcript_path");
}

#[test]
fn a_current_version_database_missing_an_index_falls_back_and_is_left_alone() {
    let dir = damaged_v1("DROP INDEX events_by_session;");
    assert_refused_untouched(dir.path(), "events_by_session");
}

/// A newer WHY that exited without checkpointing leaves its data (and the new
/// `user_version`) only in `events.db-wal`. A writable connection would attach
/// that WAL and checkpoint it into the main file when it closes: modifying a
/// database we refuse. Both the main file and the WAL must stay byte-identical.
#[test]
fn a_newer_database_with_a_leftover_wal_is_not_checkpointed() {
    let source = tempfile::tempdir().unwrap();
    let src_db = source.path().join("events.db");
    {
        let conn = Connection::open(&src_db).unwrap();
        let _: String = conn
            .pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))
            .unwrap();
        conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        conn.execute_batch(
            "CREATE TABLE future (x); INSERT INTO future VALUES (1); PRAGMA user_version = 99;",
        )
        .unwrap();
        // Keep the connection open for good: closing the last connection would
        // checkpoint. We copy the files while it is still alive, which gives
        // exactly "a process died and left a WAL behind".
        std::mem::forget(conn);
    }
    let copy = |to: &Path| {
        fs::copy(&src_db, to.join("events.db")).unwrap();
        fs::copy(
            source.path().join("events.db-wal"),
            to.join("events.db-wal"),
        )
        .unwrap();
    };

    // Premise: the version lives in the WAL, not in the main file.
    let probe = tempfile::tempdir().unwrap();
    copy(probe.path());
    {
        let conn = Connection::open(probe.path().join("events.db")).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, 99, "the WAL is attached when read");
        std::mem::forget(conn);
    }
    let main_only = tempfile::tempdir().unwrap();
    fs::copy(&src_db, main_only.path().join("events.db")).unwrap();
    {
        let conn = Connection::open(main_only.path().join("events.db")).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_ne!(v, 99, "premise: without the WAL the file looks older");
    }

    let dir = tempfile::tempdir().unwrap();
    copy(dir.path());
    let main_before = fs::read(dir.path().join("events.db")).unwrap();
    let wal = dir.path().join("events.db-wal");
    let wal_before = fs::read(&wal).unwrap();
    assert!(!wal_before.is_empty());

    let err = SqliteEventStore::open(&dir.path().join("events.db"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("newer"), "error was: {err}");
    let fallback = open_event_store(Ok(dir.path()), Clock::fixed(1_000));
    assert_usable(&*fallback);

    assert_eq!(
        fs::read(dir.path().join("events.db")).unwrap(),
        main_before,
        "main file"
    );
    assert_eq!(fs::read(&wal).unwrap(), wal_before, "the WAL");
}

#[test]
fn a_truncated_real_database_falls_back_and_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.db");
    {
        let store = SqliteEventStore::open(&path).unwrap();
        store.append(&[prompt("s1", 1, "p1", "x")]).unwrap();
    }
    // Make it a plain, self-contained file, then cut it in half mid-page.
    {
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "journal_mode", "DELETE").unwrap();
    }
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, &bytes[..bytes.len() / 2 + 7]).unwrap();
    let before = fs::read(&path).unwrap();
    // Either open fails (and we fall back) or it opens a prefix; in both cases
    // the app must start and the file must not be rewritten by open().
    let store = open_event_store(Ok(dir.path()), Clock::fixed(1_000));
    let _ = store.sessions_for_worktree(&wt("wt-1"));
    assert_eq!(fs::read(&path).unwrap(), before);
}

/// Directory permissions only bind non-root users; the helper tells us whether
/// they bind here, and restores them so the temp dir can be deleted.
#[cfg(unix)]
mod read_only_dir {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    struct ReadOnly<'a>(&'a Path);
    impl<'a> ReadOnly<'a> {
        /// `None` when the OS does not enforce it (running as root).
        fn make(dir: &'a Path) -> Option<Self> {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
            let guard = Self(dir);
            if fs::write(dir.join("probe"), b"x").is_ok() {
                return None; // not enforced; guard drop restores
            }
            Some(guard)
        }
    }
    impl Drop for ReadOnly<'_> {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn an_empty_read_only_directory_falls_back_without_creating_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let Some(_ro) = ReadOnly::make(dir.path()) else {
            eprintln!("skipped: directory permissions are not enforced (root?)");
            return;
        };
        let path = dir.path().join("events.db");
        assert!(
            SqliteEventStore::open(&path).is_err(),
            "open itself must fail"
        );
        assert!(!path.exists(), "no file may appear");
        let fallback = open_event_store(Ok(dir.path()), Clock::fixed(1_000));
        assert_usable(&*fallback);
        assert!(!path.exists());
    }

    #[test]
    fn a_read_only_directory_with_a_valid_database_falls_back_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        {
            let store = SqliteEventStore::open(&path).unwrap();
            store.append(&[prompt("s1", 1, "p1", "x")]).unwrap();
        } // closing the last connection checkpoints and removes -wal/-shm
        let Some(_ro) = ReadOnly::make(dir.path()) else {
            eprintln!("skipped: directory permissions are not enforced (root?)");
            return;
        };
        let before = fs::read(&path).unwrap();
        // WAL needs to create `-shm`/`-wal` next to the file: impossible here.
        assert!(
            SqliteEventStore::open(&path).is_err(),
            "open itself must fail"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        let fallback = open_event_store(Ok(dir.path()), Clock::fixed(1_000));
        assert_usable(&*fallback);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn no_data_dir_at_all_falls_back_to_memory() {
    let reason = "no app data dir".to_string();
    let store = open_event_store(Err(&reason), Clock::fixed(1_000));
    assert_usable(&*store);
}
