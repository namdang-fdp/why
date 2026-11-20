//! Integration tests for `SqliteRepoStore` against real database files in temp
//! directories (WAL needs a real file: it creates `why.db-wal` / `why.db-shm`
//! next to it, so each test gets its own directory).

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::Connection;
use why_lib::contracts::{RepoId, WorktreeId};
use why_lib::store::{InsertOutcome, RepoRecord, RepoStore, SqliteRepoStore, StoreError};

fn rec(id: &str, name: &str, root: impl Into<PathBuf>) -> RepoRecord {
    let root = root.into();
    RepoRecord {
        id: RepoId(id.to_string()),
        name: name.to_string(),
        common_dir: root.join(".git"),
        root_path: root,
    }
}

fn db_in(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("why.db")
}

#[test]
fn data_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_in(&dir);
    let a = rec("a", "app", "/code/app");
    let wt = WorktreeId("w1".into());
    {
        let store = SqliteRepoStore::open(&path).unwrap();
        store.insert(a.clone()).unwrap();
        store
            .set_base_override(&wt, Some("origin/main".into()))
            .unwrap();
    } // dropped: connection closed
    let store = SqliteRepoStore::open(&path).unwrap();
    assert_eq!(store.list().unwrap(), vec![a.clone()]);
    assert_eq!(
        store.base_override(&wt).unwrap().as_deref(),
        Some("origin/main")
    );
    assert_eq!(
        store.insert(a.clone()).unwrap(),
        InsertOutcome::AlreadyPresent(a)
    );
}

#[test]
fn file_database_uses_wal_and_latest_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_in(&dir);
    drop(SqliteRepoStore::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);
}

#[test]
fn reopening_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_in(&dir);
    // Opening three times must not re-run the migration (it would fail with
    // "table repositories already exists") nor change the version.
    for _ in 0..3 {
        let store = SqliteRepoStore::open(&path).unwrap();
        store.insert(rec("a", "app", "/code/app")).unwrap();
    }
    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM repositories", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn v1_database_is_upgraded_to_v2_keeping_its_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_in(&dir);
    {
        // A database exactly as the released v1 schema left it.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE repositories (
                 id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL,
                 root_path BLOB NOT NULL, common_dir BLOB NOT NULL, added_at INTEGER NOT NULL);
             CREATE TABLE base_overrides (
                 worktree_id TEXT PRIMARY KEY NOT NULL, base_ref TEXT NOT NULL,
                 updated_at INTEGER NOT NULL);
             INSERT INTO repositories VALUES ('a', 'app', CAST('/code/app' AS BLOB),
                 CAST('/code/app/.git' AS BLOB), 1);
             INSERT INTO base_overrides VALUES ('w1', 'origin/main', 1);
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let store = SqliteRepoStore::open(&path).unwrap();
    assert_eq!(store.list().unwrap(), vec![rec("a", "app", "/code/app")]);
    assert_eq!(
        store
            .base_override(&WorktreeId("w1".into()))
            .unwrap()
            .as_deref(),
        Some("origin/main")
    );
    // The new table works and starts empty: an upgrade trusts nothing.
    let a = RepoId("a".into());
    assert!(store.trusted_filters(&a).unwrap().is_empty());
    store.set_trusted_filters(&a, &["fp".into()]).unwrap();
    assert!(store.trusted_filters(&a).unwrap().contains("fp"));
}

#[test]
fn filter_trust_survives_reopen_and_is_dropped_with_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_in(&dir);
    let a = rec("a", "app", "/code/app");
    {
        let store = SqliteRepoStore::open(&path).unwrap();
        store.insert(a.clone()).unwrap();
        store
            .set_trusted_filters(&a.id, &["f1".into(), "f2".into()])
            .unwrap();
    }
    let store = SqliteRepoStore::open(&path).unwrap();
    assert_eq!(store.trusted_filters(&a.id).unwrap().len(), 2);
    assert!(store.remove(&a.id).unwrap());
    // Re-adding the same path (same id) must not inherit the old decision.
    store.insert(a.clone()).unwrap();
    assert!(store.trusted_filters(&a.id).unwrap().is_empty());
}

#[test]
fn newer_schema_is_refused_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_in(&dir);
    {
        // Simulate a database written by a future WHY: version 99, some data,
        // still in the default rollback journal mode.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE future (x INTEGER); INSERT INTO future VALUES (42);
             PRAGMA user_version = 99;",
        )
        .unwrap();
    }
    let err = SqliteRepoStore::open(&path).unwrap_err();
    let StoreError::Backend(msg) = err;
    assert!(msg.contains("newer"), "unexpected message: {msg}");
    assert!(msg.contains("99"), "unexpected message: {msg}");

    // Nothing was changed: same version, same data, journal mode not switched.
    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 99);
    let x: i64 = conn
        .query_row("SELECT x FROM future", [], |r| r.get(0))
        .unwrap();
    assert_eq!(x, 42);
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "delete");
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_round_trip() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let dir = tempfile::tempdir().unwrap();
    let path = db_in(&dir);
    // 0xFF can never appear in UTF-8; a TEXT column would have mangled it.
    let root = PathBuf::from(OsStr::from_bytes(b"/code/caf\xff/r\xe9po"));
    let r = rec("a", "app", root.clone());
    SqliteRepoStore::open(&path)
        .unwrap()
        .insert(r.clone())
        .unwrap();

    let store = SqliteRepoStore::open(&path).unwrap();
    let back = store.get(&r.id).unwrap().unwrap();
    assert_eq!(back.root_path, root);
    assert_eq!(back.common_dir, root.join(".git"));
    assert_eq!(back, r);
}

#[test]
fn concurrent_inserts_from_many_threads() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteRepoStore::open(&db_in(&dir)).unwrap());

    // 8 threads each insert 25 distinct repositories plus one shared id that
    // every thread races to insert.
    let handles: Vec<_> = (0..8)
        .map(|t| {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                let mut inserted_shared = 0;
                for i in 0..25 {
                    let r = rec(&format!("t{t}-{i}"), "n", format!("/r/{t}/{i}"));
                    assert!(matches!(
                        store.insert(r).unwrap(),
                        InsertOutcome::Inserted(_)
                    ));
                    if let InsertOutcome::Inserted(_) =
                        store.insert(rec("shared", "s", "/shared")).unwrap()
                    {
                        inserted_shared += 1;
                    }
                }
                inserted_shared
            })
        })
        .collect();
    let winners: i32 = handles.into_iter().map(|h| h.join().unwrap()).sum();

    assert_eq!(winners, 1, "exactly one thread inserts the shared id");
    assert_eq!(store.list().unwrap().len(), 8 * 25 + 1);
}
