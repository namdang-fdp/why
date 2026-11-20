//! SQLite-backed [`RepoStore`]: what the app remembers survives a restart.
//!
//! One file (`why.db` in the app data dir), one connection behind a mutex,
//! schema versioned with `PRAGMA user_version`. See the comments on
//! [`SqliteRepoStore::open`] for the pragmas and on [`MIGRATIONS`] for how the
//! schema evolves.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{InsertOutcome, RepoRecord, RepoStore, StoreError};
use crate::contracts::{RepoId, WorktreeId};

/// Schema migrations, in order. Migration `i` (0-based) upgrades the database
/// from version `i` to version `i + 1`; the version a database is at is stored
/// in its header as `PRAGMA user_version` (an integer SQLite reserves for the
/// application, 0 in a new file).
///
/// Rules: never edit or reorder an entry once it has shipped; append a new one.
/// A database written by a newer app (version > `MIGRATIONS.len()`) is refused
/// rather than guessed at.
const MIGRATIONS: &[&str] = &[
    // v1: registered repositories and per-worktree base overrides.
    //
    // Paths are BLOBs, not TEXT: a Unix path is arbitrary bytes (anything but
    // NUL) and need not be valid UTF-8. Storing the raw bytes means every path
    // round-trips exactly; TEXT would force a lossy conversion.
    //
    // Timestamps are Unix seconds (INTEGER). They are not part of `RepoRecord`
    // today; they are cheap to keep and useful for debugging and later sorting.
    //
    // `base_overrides` has no foreign key to `repositories`: a `WorktreeId` does
    // not say which repository it belongs to (see `RepoStore::remove`).
    "CREATE TABLE repositories (
         id          TEXT PRIMARY KEY NOT NULL,
         name        TEXT NOT NULL,
         root_path   BLOB NOT NULL,
         common_dir  BLOB NOT NULL,
         added_at    INTEGER NOT NULL
     );
     CREATE TABLE base_overrides (
         worktree_id TEXT PRIMARY KEY NOT NULL,
         base_ref    TEXT NOT NULL,
         updated_at  INTEGER NOT NULL
     );",
    // v2 (WHY-39): which git filter drivers the user trusted, per repository.
    //
    // One row per trusted driver fingerprint (see `git::filters`): a new or
    // changed driver has a different fingerprint, so it has no row and the guard
    // asks again. Unlike `base_overrides` this table is keyed by the repository
    // id, so `remove` deletes the rows itself (no foreign key: ids are strings
    // derived from the path, and a row must never outlive its repository).
    "CREATE TABLE filter_trust (
         repo_id     TEXT NOT NULL,
         fingerprint TEXT NOT NULL,
         trusted_at  INTEGER NOT NULL,
         PRIMARY KEY (repo_id, fingerprint)
     );",
];

/// How long a statement waits for another connection's lock before failing
/// with `SQLITE_BUSY`. Only matters if a second process (a second app instance,
/// the `sqlite3` CLI) opens the same file; inside the app the mutex already
/// serializes access.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the app's repository data lives this session (set once in `lib.rs`
/// `setup`, kept in Tauri's managed state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageMode {
    /// The SQLite database at this path.
    Persistent(PathBuf),
    /// The database could not be opened; everything is kept in memory and
    /// forgotten at exit. `reason` is the error, for display.
    Temporary { reason: String },
}

impl StorageMode {
    pub fn is_persistent(&self) -> bool {
        matches!(self, StorageMode::Persistent(_))
    }
}

/// A [`RepoStore`] in a SQLite database.
///
/// Concurrency: one `Connection` behind a `std::sync::Mutex`, so all access is
/// serialized. `rusqlite::Connection` is `Send` but not `Sync` (SQLite's
/// connection object must not be used from two threads at once), and wrapping
/// it in a `Mutex` is exactly what makes the store `Sync`, which the trait
/// requires.
///
/// Why one connection is enough: the store holds a handful of rows, every query
/// is a primary-key lookup or a tiny scan that takes microseconds, and writes
/// happen when the user clicks something. Queueing on a mutex costs nothing at
/// that scale. If that changes, the next steps are:
/// * a small pool of read-only connections (WAL lets them read concurrently with
///   the writer) plus this single writer, when reads become slow or frequent;
/// * a dedicated writer thread fed by a channel, when writes become frequent or
///   batched (e.g. persisting review state on every keystroke), so callers never
///   wait for `fsync`.
#[derive(Debug)]
pub struct SqliteRepoStore {
    conn: Mutex<Connection>,
}

impl SqliteRepoStore {
    /// Open (or create) the database file at `path` and bring its schema up to
    /// date. The parent directory must exist.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)
            .map_err(backend(format!("cannot open database {}", path.display())))?;
        Self::init(conn, true)
    }

    /// A private, empty database that lives in memory (for tests).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn =
            Connection::open_in_memory().map_err(backend("cannot open in-memory database"))?;
        Self::init(conn, false)
    }

    fn init(mut conn: Connection, on_disk: bool) -> Result<Self, StoreError> {
        // Per-connection settings first: they are not stored in the file, so they
        // must be set on every open.
        //
        // `busy_timeout`: see `BUSY_TIMEOUT`.
        conn.busy_timeout(BUSY_TIMEOUT)
            .map_err(backend("cannot set busy_timeout"))?;
        // `foreign_keys`: SQLite only enforces FOREIGN KEY constraints when this
        // is on, and it is off by default for backward compatibility. No table
        // has one yet; turning it on now means a future migration that adds one
        // is enforced without anyone remembering this line.
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(backend("cannot enable foreign_keys"))?;

        // Check the version BEFORE anything writes to the file. Switching the
        // journal mode to WAL (below) already modifies the database, and a file
        // written by a newer app must be left exactly as it was.
        let version = user_version(&conn)?;
        if version > MIGRATIONS.len() {
            return Err(newer_schema(version));
        }

        if on_disk {
            // `journal_mode=WAL` (write-ahead log). In the default rollback
            // mode a writer copies the old pages aside, rewrites the database
            // in place, and must lock out readers while it does. In WAL mode a
            // writer appends new pages to `why.db-wal` instead and the main
            // file is updated later (a "checkpoint"), so readers keep reading
            // the last committed state while a write is in progress: readers
            // never block the writer and the writer never blocks readers (there
            // is still only one writer at a time). Unlike the other pragmas this
            // one is persistent: it is stored in the file. In-memory databases
            // cannot use WAL, so it is skipped there.
            let mode: String = conn
                .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
                .map_err(backend("cannot enable WAL"))?;
            if !mode.eq_ignore_ascii_case("wal") {
                tracing::warn!(%mode, "SQLite refused WAL journal mode; continuing");
            }
            // `synchronous=NORMAL`: with WAL, NORMAL fsyncs the log only at
            // checkpoints, not on every commit. The database can never be
            // corrupted by a crash or power loss; the worst case is that the
            // last few commits before a *power* loss (not an app crash: the OS
            // still has the data) are rolled back. Losing "repository X was
            // registered a second ago" is acceptable for this data, and it
            // spares an fsync per click. (FULL would fsync every commit.)
            conn.pragma_update(None, "synchronous", "NORMAL")
                .map_err(backend("cannot set synchronous=NORMAL"))?;
        }

        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// A mutex is "poisoned" if a thread panicked while holding it. The data
    /// behind it may be half-updated, so std makes `lock()` return an error. We
    /// report that as a store error instead of panicking again, so one bug does
    /// not take down every later command. (SQLite's own state is still
    /// consistent: an interrupted statement is rolled back by SQLite.)
    fn lock(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.conn
            .lock()
            .map_err(|_| StoreError::Backend("SQLite store lock poisoned".to_string()))
    }
}

/// Apply every migration the database has not seen yet. Each one runs in its own
/// transaction together with the `user_version` bump, so a crash halfway leaves
/// the database at the previous version, never in between. Re-opening an
/// up-to-date database runs nothing (idempotent).
fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    loop {
        // `IMMEDIATE` takes the write lock at `BEGIN`, not at the first write. If
        // two app instances start at once, the second waits here (busy_timeout)
        // and then re-reads the version below, instead of both reading "0" and
        // both trying to create the tables.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(backend("cannot start migration transaction"))?;
        let version = user_version(&tx)?;
        if version > MIGRATIONS.len() {
            return Err(newer_schema(version));
        }
        let Some(sql) = MIGRATIONS.get(version) else {
            return Ok(()); // up to date; dropping `tx` rolls back the empty transaction
        };
        tx.execute_batch(sql)
            .map_err(backend(format!("migration to v{} failed", version + 1)))?;
        // PRAGMA values cannot be bound as parameters; the number is ours, not
        // user input. `user_version` lives in the file header and is part of the
        // transaction like any other write.
        // (`usize` is not `ToSql`: SQLite integers are i64.)
        let next = i64::try_from(version + 1).map_err(|_| newer_schema(version))?;
        tx.pragma_update(None, "user_version", next)
            .map_err(backend("cannot record schema version"))?;
        tx.commit().map_err(backend(format!(
            "cannot commit migration to v{}",
            version + 1
        )))?;
        tracing::info!(version = version + 1, "migrated repository store schema");
    }
}

fn user_version(conn: &Connection) -> Result<usize, StoreError> {
    let v: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(backend("cannot read schema version"))?;
    // `user_version` is a signed 32-bit integer; a negative value is not one we wrote.
    usize::try_from(v).map_err(|_| newer_schema_raw(v))
}

fn newer_schema(version: usize) -> StoreError {
    StoreError::Backend(format!(
        "database schema version {version} is newer than this version of WHY supports ({}); \
         refusing to open it so no data is lost. Update WHY.",
        MIGRATIONS.len()
    ))
}

fn newer_schema_raw(version: i64) -> StoreError {
    StoreError::Backend(format!(
        "database schema version {version} is not one this version of WHY understands; \
         refusing to open it"
    ))
}

/// `map_err` helper: wrap a rusqlite error as `StoreError::Backend` with context.
fn backend(context: impl Into<String>) -> impl FnOnce(rusqlite::Error) -> StoreError {
    let context = context.into();
    move |e| StoreError::Backend(format!("{context}: {e}"))
}

fn now_unix() -> i64 {
    // A clock before 1970 is a broken machine, not a reason to fail a write.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

// Path <-> bytes. On Unix a path IS a byte string, so this is lossless both ways
// (`OsStrExt` exposes the bytes). Elsewhere (Windows paths are UTF-16) we fall
// back to UTF-8, which is lossy only for unpaired surrogates; WHY targets Unix
// first.
#[cfg(unix)]
fn path_to_blob(p: &Path) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes()
}

#[cfg(unix)]
fn blob_to_path(b: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_vec(b))
}

#[cfg(not(unix))]
fn path_to_blob(p: &Path) -> &[u8] {
    // `to_str` fails only for paths that are not valid Unicode.
    p.to_str().unwrap_or_default().as_bytes()
}

#[cfg(not(unix))]
fn blob_to_path(b: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&b).into_owned())
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepoRecord> {
    Ok(RepoRecord {
        id: RepoId(row.get(0)?),
        name: row.get(1)?,
        root_path: blob_to_path(row.get(2)?),
        common_dir: blob_to_path(row.get(3)?),
    })
}

const SELECT_REPO: &str = "SELECT id, name, root_path, common_dir FROM repositories";

fn get_locked(conn: &Connection, id: &RepoId) -> Result<Option<RepoRecord>, StoreError> {
    conn.query_row(
        &format!("{SELECT_REPO} WHERE id = ?1"),
        [&id.0],
        row_to_record,
    )
    .optional()
    .map_err(backend("cannot read repository"))
}

impl RepoStore for SqliteRepoStore {
    fn list(&self) -> Result<Vec<RepoRecord>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare_cached(SELECT_REPO)
            .map_err(backend("cannot list repositories"))?;
        let mut repos = stmt
            .query_map([], row_to_record)
            .map_err(backend("cannot list repositories"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend("cannot read repository row"))?;
        // Sorted in Rust, not with `ORDER BY`: SQLite would compare the path
        // BLOBs byte by byte, while `PathBuf` compares component by component
        // (`/a/b` < `/a-b` for paths, but `-` (0x2D) < `/` (0x2F) as bytes). Using
        // the same key as `MemoryRepoStore` keeps the two backends interchangeable.
        repos.sort_by(|a, b| (&a.name, &a.root_path).cmp(&(&b.name, &b.root_path)));
        Ok(repos)
    }

    fn get(&self, id: &RepoId) -> Result<Option<RepoRecord>, StoreError> {
        let conn = self.lock()?;
        get_locked(&conn, id)
    }

    fn insert(&self, rec: RepoRecord) -> Result<InsertOutcome, StoreError> {
        let conn = self.lock()?;
        // `ON CONFLICT DO NOTHING` makes a duplicate id a no-op instead of an
        // error; `changed == 0` then tells us the row was already there. Both
        // statements run under the same mutex guard, so nothing can slip in
        // between the insert and the read-back.
        let changed = conn
            .execute(
                "INSERT INTO repositories (id, name, root_path, common_dir, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO NOTHING",
                params![
                    rec.id.0,
                    rec.name,
                    path_to_blob(&rec.root_path),
                    path_to_blob(&rec.common_dir),
                    now_unix()
                ],
            )
            .map_err(backend("cannot insert repository"))?;
        if changed == 1 {
            return Ok(InsertOutcome::Inserted(rec));
        }
        match get_locked(&conn, &rec.id)? {
            Some(existing) => Ok(InsertOutcome::AlreadyPresent(existing)),
            // Impossible while we hold the lock, unless another process deleted
            // the row in between. Report it rather than panic.
            None => Err(StoreError::Backend(format!(
                "repository {} vanished during insert",
                rec.id.0
            ))),
        }
    }

    fn remove(&self, id: &RepoId) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        // One transaction: either the repository and its trust both go, or
        // neither does.
        let tx = conn
            .transaction()
            .map_err(backend("cannot start transaction"))?;
        tx.execute("DELETE FROM filter_trust WHERE repo_id = ?1", [&id.0])
            .map_err(backend("cannot remove filter trust"))?;
        let changed = tx
            .execute("DELETE FROM repositories WHERE id = ?1", [&id.0])
            .map_err(backend("cannot remove repository"))?;
        tx.commit().map_err(backend("cannot commit removal"))?;
        Ok(changed > 0)
    }

    fn base_override(&self, wt: &WorktreeId) -> Result<Option<String>, StoreError> {
        self.lock()?
            .query_row(
                "SELECT base_ref FROM base_overrides WHERE worktree_id = ?1",
                [&wt.0],
                |row| row.get(0),
            )
            .optional()
            .map_err(backend("cannot read base override"))
    }

    fn set_base_override(&self, wt: &WorktreeId, base: Option<String>) -> Result<(), StoreError> {
        let conn = self.lock()?;
        match base {
            // Upsert: insert, or replace the base of an existing row.
            Some(base) => conn.execute(
                "INSERT INTO base_overrides (worktree_id, base_ref, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(worktree_id) DO UPDATE
                 SET base_ref = excluded.base_ref, updated_at = excluded.updated_at",
                params![wt.0, base, now_unix()],
            ),
            None => conn.execute("DELETE FROM base_overrides WHERE worktree_id = ?1", [&wt.0]),
        }
        .map_err(backend("cannot write base override"))?;
        Ok(())
    }

    fn trusted_filters(&self, repo: &RepoId) -> Result<HashSet<String>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare_cached("SELECT fingerprint FROM filter_trust WHERE repo_id = ?1")
            .map_err(backend("cannot read filter trust"))?;
        stmt.query_map([&repo.0], |row| row.get::<_, String>(0))
            .map_err(backend("cannot read filter trust"))?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(backend("cannot read filter trust row"))
    }

    fn set_trusted_filters(
        &self,
        repo: &RepoId,
        fingerprints: &[String],
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        // Replace the whole set atomically: a crash must not leave half of it.
        // IMMEDIATE: take the write lock before the existence check, so no other
        // connection can remove the repository between the check and the write.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(backend("cannot start transaction"))?;
        let registered: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM repositories WHERE id = ?1)",
                [&repo.0],
                |row| row.get(0),
            )
            .map_err(backend("cannot check the repository"))?;
        if !registered {
            return Ok(false); // dropping `tx` rolls back; nothing was written
        }
        tx.execute("DELETE FROM filter_trust WHERE repo_id = ?1", [&repo.0])
            .map_err(backend("cannot clear filter trust"))?;
        let now = now_unix();
        for fingerprint in fingerprints {
            tx.execute(
                "INSERT OR IGNORE INTO filter_trust (repo_id, fingerprint, trusted_at)
                 VALUES (?1, ?2, ?3)",
                params![repo.0, fingerprint, now],
            )
            .map_err(backend("cannot write filter trust"))?;
        }
        tx.commit().map_err(backend("cannot commit filter trust"))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, name: &str, root: &str) -> RepoRecord {
        RepoRecord {
            id: RepoId(id.to_string()),
            name: name.to_string(),
            root_path: PathBuf::from(root),
            common_dir: PathBuf::from(root).join(".git"),
        }
    }

    #[test]
    fn insert_get_list_remove_in_memory() {
        let store = SqliteRepoStore::open_in_memory().unwrap();
        let a = rec("a", "app", "/code/app");
        assert_eq!(
            store.insert(a.clone()).unwrap(),
            InsertOutcome::Inserted(a.clone())
        );
        // Same id again: the stored record wins.
        assert_eq!(
            store.insert(rec("a", "renamed", "/elsewhere")).unwrap(),
            InsertOutcome::AlreadyPresent(a.clone())
        );
        assert_eq!(store.get(&a.id).unwrap(), Some(a.clone()));
        assert_eq!(store.get(&RepoId("zzz".into())).unwrap(), None);
        assert_eq!(store.list().unwrap(), vec![a.clone()]);
        assert!(store.remove(&a.id).unwrap());
        assert!(!store.remove(&a.id).unwrap());
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn list_order_matches_the_memory_store() {
        // `/a/b` vs `/a-b` sorts differently as bytes and as paths; both
        // backends must agree.
        let sqlite = SqliteRepoStore::open_in_memory().unwrap();
        let memory = super::super::MemoryRepoStore::new();
        for r in [
            rec("3", "zeta", "/a/zeta"),
            rec("2", "app", "/a-b"),
            rec("1", "app", "/a/b"),
        ] {
            sqlite.insert(r.clone()).unwrap();
            memory.insert(r).unwrap();
        }
        assert_eq!(sqlite.list().unwrap(), memory.list().unwrap());
        let ids: Vec<String> = sqlite.list().unwrap().into_iter().map(|r| r.id.0).collect();
        assert_eq!(ids, ["1", "2", "3"]);
    }

    #[test]
    fn base_override_set_replace_clear() {
        let store = SqliteRepoStore::open_in_memory().unwrap();
        let wt = WorktreeId("w1".into());
        assert_eq!(store.base_override(&wt).unwrap(), None);
        store
            .set_base_override(&wt, Some("origin/main".into()))
            .unwrap();
        store
            .set_base_override(&wt, Some("develop".into()))
            .unwrap();
        assert_eq!(
            store.base_override(&wt).unwrap().as_deref(),
            Some("develop")
        );
        store.set_base_override(&wt, None).unwrap();
        assert_eq!(store.base_override(&wt).unwrap(), None);
        // Clearing an absent override is fine.
        store.set_base_override(&wt, None).unwrap();
    }

    #[test]
    fn filter_trust_is_replaced_per_repo_and_dies_with_the_registration() {
        let store = SqliteRepoStore::open_in_memory().unwrap();
        let (a, b) = (RepoId("a".into()), RepoId("b".into()));
        store.insert(rec("a", "a", "/a")).unwrap();
        store.insert(rec("b", "b", "/b")).unwrap();
        assert!(store.trusted_filters(&a).unwrap().is_empty());

        store
            .set_trusted_filters(&a, &["f1".into(), "f2".into(), "f2".into()])
            .unwrap();
        store.set_trusted_filters(&b, &["x".into()]).unwrap();
        assert_eq!(store.trusted_filters(&a).unwrap().len(), 2);

        // Replace, not add.
        store.set_trusted_filters(&a, &["f3".into()]).unwrap();
        let set = store.trusted_filters(&a).unwrap();
        assert!(set.contains("f3") && !set.contains("f1"));
        assert!(store.trusted_filters(&b).unwrap().contains("x"));

        // Removing the repository drops its trust (the id is derived from the
        // path, so a re-add must not inherit it).
        assert!(store.remove(&a).unwrap());
        assert!(store.trusted_filters(&a).unwrap().is_empty());

        store.set_trusted_filters(&b, &[]).unwrap();
        assert!(store.trusted_filters(&b).unwrap().is_empty());
    }

    #[test]
    fn a_late_trust_write_for_a_removed_repository_is_refused() {
        // The trust command scans first and writes later; the user may have
        // removed the repository in between.
        let store = SqliteRepoStore::open_in_memory().unwrap();
        let a = RepoId("a".into());
        // Never registered: nothing is written.
        assert!(!store.set_trusted_filters(&a, &["late".into()]).unwrap());
        assert!(store.trusted_filters(&a).unwrap().is_empty());

        store.insert(rec("a", "a", "/a")).unwrap();
        assert!(store.set_trusted_filters(&a, &["f".into()]).unwrap());
        assert!(store.remove(&a).unwrap());
        // The "in-flight" write lands after the removal: refused, and a
        // re-registration of the same path (same id) starts untrusted.
        assert!(!store.set_trusted_filters(&a, &["late".into()]).unwrap());
        store.insert(rec("a", "a", "/a")).unwrap();
        assert!(store.trusted_filters(&a).unwrap().is_empty());
    }

    #[test]
    fn poisoned_lock_is_a_store_error_not_a_panic() {
        let store = std::sync::Arc::new(SqliteRepoStore::open_in_memory().unwrap());
        let s = store.clone();
        let _ = std::thread::spawn(move || {
            let _guard = s.conn.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(matches!(store.list(), Err(StoreError::Backend(_))));
    }

    #[test]
    fn in_memory_database_is_at_the_latest_version() {
        let store = SqliteRepoStore::open_in_memory().unwrap();
        let conn = store.lock().unwrap();
        assert_eq!(user_version(&conn).unwrap(), MIGRATIONS.len());
    }
}
