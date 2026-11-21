//! SQLite-backed [`EventStore`]: `events.db`, the persistent home of agent
//! sessions (decision 24: its own file, WAL, single writer).
//!
//! It behaves exactly like [`MemoryEventStore`](super::MemoryEventStore): the
//! same keys, the same ordering, and the SAME pure functions in
//! [`super::timeline`] build every timeline and summary. The database only
//! stores events and answers "which events belong to this session / worktree";
//! all the pairing and turn rules stay in Rust. A shared conformance suite
//! (`tests/events/`) runs the same cases against both stores so they cannot drift.
//!
//! # Portability (decision 4)
//!
//! Rows are identified by the stable [`EventKey`] and [`SessionKey`] (harness +
//! harness session id), never by a SQLite rowid. The one integer `events.seq`
//! only remembers arrival order inside this file; it leaves the store only as
//! the ordering key [`EditRecord::seq`], never as an identity.
//!
//! # Threads
//!
//! The trait is synchronous. A call blocks its thread for the duration of a
//! SQLite statement: microseconds for a single event, a few milliseconds for a
//! timeline. **WHY-13's collector should wrap `append` / `prune_older_than` in
//! `tauri::async_runtime::spawn_blocking`** (they take the writer lock and may
//! wait on `fsync`/`busy_timeout`); the IPC read commands are short enough to
//! call directly, but `spawn_blocking` is the safe choice for a big timeline too.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::types::Type;
use rusqlite::{Connection, OpenFlags, Row, TransactionBehavior, params};
use why_wire::Harness;

use super::clock::Clock;
use super::correlation::{
    CallEvent, EditTouch, ShellTurn, build_edit_touches, build_shell_turns, canonical_paths,
    distinct_sessions, fold_calls, indexed_files,
};
use super::edits::{
    FORMAT_V1, decode_after, decode_before, decode_hunks, decode_kind as decode_edit_kind,
    decode_observed, encode_after, encode_before, encode_hunks, encode_kind as encode_edit_kind,
    encode_observed, stored_form,
};
use super::model::{EventKey, EventKind, NormalizedEvent, SessionKey, ToolCall, ToolClass};
use super::rationale::{SourceEvent, SourceTool};
use super::timeline::{TimelinePosition, TurnIndices, build_summary, build_timeline, turn_indices};
use super::{AppendOutcome, EventStore, MemoryEventStore};
use crate::contracts::{EvidenceLabel, SessionSummary, SessionTimeline, ToolOutcome, WorktreeId};
use crate::provenance::{EditOp, EditRecord, EditSource, Observed};
use crate::store::StoreError;

/// Schema migrations, in order; migration `i` upgrades version `i` to `i + 1`
/// (`PRAGMA user_version`). Same rules as `store/sqlite.rs`: never edit a shipped
/// entry, append a new one; a newer database than we know is refused untouched.
const MIGRATIONS: &[&str] = &[
    // v1: sessions, their worktrees, their events.
    //
    // * `sessions` is the parent row, created by a session's first event.
    //   `last_event_ms` is a running MAX kept by `append`, so retention is
    //   `DELETE ... WHERE last_event_ms < cutoff` (whole sessions) without
    //   reading any event. `WITHOUT ROWID`: the composite text key IS the table.
    // * `session_worktrees` answers "sessions of this worktree" and "which
    //   worktrees does this session appear in" without scanning events. A session
    //   can span worktrees (its cwd changes), so it is a link table, not a column.
    // * `events` holds one row per `NormalizedEvent`. Closed sets (kind, outcome,
    //   tool class, evidence) are stored as stable lowercase strings, readable in
    //   the `sqlite3` CLI; an unknown string is refused on read, not guessed.
    //   `files` is a JSON array (order and duplicates kept): it is only ever read
    //   together with its event, so a child table would cost joins for nothing.
    //   There is no raw-payload column: `NormalizedEvent` does not carry one, and
    //   the full text stays in the harness's own transcript (`transcript_path`).
    // * Timestamps are Unix milliseconds (INTEGER, i64 range).
    // * ON DELETE CASCADE: deleting a session removes its events and links in
    //   the same statement (needs `foreign_keys=ON`, set on every connection).
    "CREATE TABLE sessions (
         harness         TEXT    NOT NULL,
         session_id      TEXT    NOT NULL,
         first_event_ms  INTEGER NOT NULL,
         last_event_ms   INTEGER NOT NULL,
         PRIMARY KEY (harness, session_id)
     ) WITHOUT ROWID;
     CREATE INDEX sessions_by_last_event ON sessions (last_event_ms);

     CREATE TABLE session_worktrees (
         worktree_id TEXT NOT NULL,
         harness     TEXT NOT NULL,
         session_id  TEXT NOT NULL,
         PRIMARY KEY (worktree_id, harness, session_id),
         FOREIGN KEY (harness, session_id)
             REFERENCES sessions (harness, session_id) ON DELETE CASCADE
     ) WITHOUT ROWID;
     CREATE INDEX session_worktrees_by_session
         ON session_worktrees (harness, session_id);

     CREATE TABLE events (
         seq             INTEGER PRIMARY KEY,
         event_key       TEXT    NOT NULL UNIQUE,
         harness         TEXT    NOT NULL,
         session_id      TEXT    NOT NULL,
         hook_event      TEXT    NOT NULL,
         kind            TEXT    NOT NULL,
         resumed         INTEGER,
         outcome         TEXT,
         at_ms           INTEGER NOT NULL,
         turn_id         TEXT,
         agent_id        TEXT,
         tool_use_id     TEXT,
         tool_name       TEXT,
         tool_class      TEXT,
         cwd             TEXT    NOT NULL,
         worktree_id     TEXT,
         model           TEXT,
         text            TEXT,
         files           TEXT    NOT NULL,
         evidence        TEXT    NOT NULL,
         transcript_path TEXT,
         FOREIGN KEY (harness, session_id)
             REFERENCES sessions (harness, session_id) ON DELETE CASCADE
     );
     CREATE INDEX events_by_session ON events (harness, session_id, seq);",
    // v2 (WHY-16): the file -> edit index, and two indexes for correlation.
    //
    // * `event_files` is a derived index: one row per (worktree, canonical
    //   path, event) for events of EDIT-class tool calls. It answers "which
    //   calls named this file in this worktree" with a primary-key range scan
    //   instead of decoding every event's `files` JSON. It holds nothing the
    //   events table does not: it can always be rebuilt from it (the backfill
    //   below does exactly that). The path is stored canonical (see
    //   `correlation::canonical_path`), which is why the backfill is Rust code,
    //   not SQL: `./a/../b.rs` cannot be resolved with `json_each` alone.
    //   `ON DELETE CASCADE` on the event, so pruning a session (which cascades
    //   to its events) cleans the index in the same statement;
    //   `event_files_by_event` is what lets that cascade find its rows without a
    //   table scan.
    // * `events_by_tool_use` finds both halves of a call (Pre and Post) by the
    //   harness's `tool_use_id`; partial, because id-less events are never
    //   paired.
    // * `events_shell_by_worktree` finds a worktree's shell and test-run events
    //   (the "this turn ran commands here" leads) without scanning the rest.
    "CREATE TABLE event_files (
         worktree_id TEXT NOT NULL,
         path        TEXT NOT NULL,
         event_key   TEXT NOT NULL
             REFERENCES events (event_key) ON DELETE CASCADE,
         PRIMARY KEY (worktree_id, path, event_key)
     ) WITHOUT ROWID;
     CREATE INDEX event_files_by_event ON event_files (event_key);
     CREATE INDEX events_by_tool_use
         ON events (harness, session_id, tool_use_id) WHERE tool_use_id IS NOT NULL;
     CREATE INDEX events_shell_by_worktree
         ON events (worktree_id) WHERE tool_class IN ('shell', 'test_run');",
    // v3 (WHY-64): the replay inputs of line provenance (decision 29).
    //
    // * One row per stored `EditOp`, keyed by its event and its position in
    //   that event (`op_index`). `ON DELETE CASCADE` on the event, so the
    //   30-day prune (which deletes sessions, cascading to events) removes the
    //   ops in the same statement; the primary key's `event_key` prefix is what
    //   lets that cascade find the rows without a table scan.
    // * `event_seq` copies `events.seq` so `edit_ops_by_file` alone answers
    //   "this worktree + file, in arrival order" (seq, then op_index) without
    //   a sort step. `worktree_id` is the OP's worktree (decision 32), which
    //   may differ from its event's.
    // * NO LINE TEXT, anywhere: `before`, `after` and `hunks` hold 16-byte line
    //   hashes in the binary layout of `events/edits.rs`, whose version is
    //   `format` (a later layout writes a new number next to old rows).
    //   BLOB, not JSON: `before` holds one hash per line of the old file.
    // * A rowid table, not `WITHOUT ROWID`: rows can be tens of KB, and SQLite
    //   advises rowid tables for rows that large.
    // * Closed sets (`kind`, `observed`) are stable lowercase strings, like
    //   the events table.
    "CREATE TABLE edit_ops (
         event_key    TEXT    NOT NULL
             REFERENCES events (event_key) ON DELETE CASCADE,
         op_index     INTEGER NOT NULL,
         event_seq    INTEGER NOT NULL,
         worktree_id  TEXT    NOT NULL,
         path         TEXT    NOT NULL,
         kind         TEXT    NOT NULL,
         renamed_from TEXT,
         observed     TEXT    NOT NULL,
         format       INTEGER NOT NULL,
         before       BLOB,
         after        BLOB,
         hunks        BLOB    NOT NULL,
         PRIMARY KEY (event_key, op_index)
     );
     CREATE INDEX edit_ops_by_file ON edit_ops (worktree_id, path, event_seq, op_index);",
];

/// The schema version this build writes (`PRAGMA user_version` after `open`).
/// Tests compare with it instead of a bare number, so a new migration does not
/// have to edit them.
pub const SCHEMA_VERSION: usize = MIGRATIONS.len();

/// How long a statement waits for another connection's lock before failing with
/// `SQLITE_BUSY`. Inside the app the writer mutex already serializes writes and
/// WAL readers never wait; this only matters for a second process on the file.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// All events of one session, in arrival order, with the columns in the order
/// [`event_from_row`] reads them. Served by `events_by_session`
/// (the index is already in `seq` order: no sort step). Public so a test can
/// `EXPLAIN QUERY PLAN` the exact text the store runs.
pub const SESSION_EVENTS_SQL: &str = "SELECT event_key, harness, session_id, hook_event, kind, \
     resumed, outcome, at_ms, turn_id, agent_id, tool_use_id, tool_name, tool_class, cwd, \
     worktree_id, model, text, files, evidence, transcript_path \
     FROM events WHERE harness = ?1 AND session_id = ?2 ORDER BY seq";

/// The sessions listed for a worktree. Public for the same reason.
pub const WORKTREE_SESSIONS_SQL: &str = "SELECT s.harness, s.session_id, s.last_event_ms \
     FROM session_worktrees w \
     JOIN sessions s ON s.harness = w.harness AND s.session_id = w.session_id \
     WHERE w.worktree_id = ?1";

/// A [`EventStore`] in a SQLite file: ONE writer connection and ONE reader
/// connection, each behind its own `std::sync::Mutex`.
///
/// Why two connections: in WAL mode readers see the last committed state while a
/// write is in progress, but only through a *different* connection. With a single
/// shared connection a `timeline()` call would simply queue behind a long
/// `append` on the mutex (correct, but the UI would stall behind the collector).
/// The reader is opened read-only, so it cannot write by mistake, and every read
/// runs inside one read transaction (one snapshot for several statements).
///
/// Why ONE reader and not a pool: a read is milliseconds and the UI asks for one
/// session list/timeline at a time. A pool is a drop-in change here (a `Vec` of
/// reader mutexes plus a round-robin index) if benchmarks ever say otherwise.
#[derive(Debug)]
pub struct SqliteEventStore {
    writer: Mutex<Connection>,
    reader: Mutex<Connection>,
    /// `None`: the stale-session rule is not applied (see
    /// [`SqliteEventStore::with_clock`]).
    clock: Option<Clock>,
}

impl SqliteEventStore {
    /// Open (or create) the database at `path` and bring its schema up to date.
    /// The parent directory must exist. Fails, leaving the file as it was, for a
    /// file that is not a database, was written by a newer WHY, or has the right
    /// version but a damaged schema.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        // Judge an EXISTING file through a read-only connection first. A writable
        // connection can attach a leftover `-wal` and checkpoint it into the main
        // file when it closes, which would modify a database we are about to
        // refuse. A read-only connection never writes either file.
        if path.exists() {
            precheck_read_only(path)?;
        }
        // Only now that the file is acceptable: owner-only permissions (Unix).
        ensure_owner_only(path)?;
        let mut writer = Connection::open(path)
            .map_err(backend(format!("cannot open database {}", path.display())))?;
        init_writer(&mut writer)?;
        // After the writer succeeded the file is a valid, migrated database, so
        // a read-only connection is safe to open (and `-shm` exists for WAL).
        let reader = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(backend("cannot open read connection"))?;
        reader
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(backend("cannot set busy_timeout"))?;
        Ok(Self {
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
            clock: None,
        })
    }

    /// Demote stale `live` sessions to `unknown`, judged by `clock` (see
    /// [`super::timeline::effective_status`]). Without it the rule is off.
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    fn now_ms(&self) -> Option<u64> {
        self.clock.as_ref().map(Clock::now_ms)
    }
}

/// The tables and columns this version of WHY needs, and its indexes. A database
/// whose `user_version` says "current" but that lacks one of them (someone
/// dropped a table, a half-restored backup) would open fine and then fail on the
/// first query, so it is refused up front and the app falls back to memory.
/// Keep in step with [`MIGRATIONS`]: a new migration extends these lists.
const EXPECTED_TABLES: &[(&str, &[&str])] = &[
    (
        "sessions",
        &["harness", "session_id", "first_event_ms", "last_event_ms"],
    ),
    (
        "session_worktrees",
        &["worktree_id", "harness", "session_id"],
    ),
    ("event_files", &["worktree_id", "path", "event_key"]),
    (
        "edit_ops",
        &[
            "event_key",
            "op_index",
            "event_seq",
            "worktree_id",
            "path",
            "kind",
            "renamed_from",
            "observed",
            "format",
            "before",
            "after",
            "hunks",
        ],
    ),
    (
        "events",
        &[
            "seq",
            "event_key",
            "harness",
            "session_id",
            "hook_event",
            "kind",
            "resumed",
            "outcome",
            "at_ms",
            "turn_id",
            "agent_id",
            "tool_use_id",
            "tool_name",
            "tool_class",
            "cwd",
            "worktree_id",
            "model",
            "text",
            "files",
            "evidence",
            "transcript_path",
        ],
    ),
];
const EXPECTED_INDEXES: &[&str] = &[
    "sessions_by_last_event",
    "session_worktrees_by_session",
    "events_by_session",
    "event_files_by_event",
    "events_by_tool_use",
    "events_shell_by_worktree",
    "edit_ops_by_file",
];

/// `Err` unless every expected table, column and index exists.
fn validate_schema(conn: &Connection) -> Result<(), StoreError> {
    for (table, columns) in EXPECTED_TABLES {
        // PRAGMA arguments cannot be bound; `table` is one of our constants.
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(backend("cannot inspect schema"))?;
        let have: HashSet<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(backend("cannot inspect schema"))?
            .collect::<Result<_, _>>()
            .map_err(backend("cannot inspect schema"))?;
        if let Some(missing) = columns.iter().find(|c| !have.contains(**c)) {
            return Err(StoreError::Backend(format!(
                "event database is damaged: table {table} is missing column {missing} \
                 (or the table is gone); refusing to use it"
            )));
        }
    }
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'index'")
        .map_err(backend("cannot inspect schema"))?;
    let have: HashSet<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(backend("cannot inspect schema"))?
        .collect::<Result<_, _>>()
        .map_err(backend("cannot inspect schema"))?;
    if let Some(missing) = EXPECTED_INDEXES.iter().find(|i| !have.contains(**i)) {
        return Err(StoreError::Backend(format!(
            "event database is damaged: index {missing} is missing; refusing to use it"
        )));
    }
    Ok(())
}

/// Version and (for a current database) schema checks through a read-only
/// connection, before any writable connection touches the file.
fn precheck_read_only(path: &Path) -> Result<(), StoreError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(backend(format!("cannot open database {}", path.display())))?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(backend("cannot set busy_timeout"))?;
    // The first read of the file: one that is not a database fails here.
    let version = user_version(&conn)?;
    if version > MIGRATIONS.len() {
        return Err(newer_schema(version));
    }
    if version == MIGRATIONS.len() {
        validate_schema(&conn)?;
    }
    Ok(())
}

/// The captured prompts, commands and messages are as private as the spool
/// (decision 22: `0600`). SQLite gives `-wal` and `-shm` the main file's mode, so
/// fixing the main file fixes all three. Create it ourselves with `0600` (the
/// umask can only remove bits), and tighten an existing, looser file. Not
/// applicable on Windows, where access follows the profile directory's ACL.
#[cfg(unix)]
fn ensure_owner_only(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let io = |what: &str| {
        let what = what.to_owned();
        move |e: std::io::Error| StoreError::Backend(format!("{what} {}: {e}", path.display()))
    };
    if !path.exists() {
        // `create_new`: never follow or reuse something that appeared meanwhile.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io("cannot create")(e)),
        }
    }
    let mode = std::fs::metadata(path)
        .map_err(io("cannot stat"))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(io("cannot restrict permissions of"))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_owner_only(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

/// Settings and schema for the writer connection, in this order:
///
/// 1. per-connection pragmas (not stored in the file);
/// 2. the version check, BEFORE anything writes: switching to WAL already
///    modifies the file, and a database from a newer WHY must stay byte-identical;
/// 3. `auto_vacuum`, only on a brand-new file;
/// 4. WAL, then the migrations.
fn init_writer(conn: &mut Connection) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(backend("cannot set busy_timeout"))?;
    // SQLite enforces FOREIGN KEY (and so ON DELETE CASCADE) only when asked.
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(backend("cannot enable foreign_keys"))?;

    // Also the first read of the file: a file that is not a database fails here.
    let version = user_version(conn)?;
    if version > MIGRATIONS.len() {
        return Err(newer_schema(version));
    }

    if version == 0 {
        // `auto_vacuum=INCREMENTAL` makes SQLite track free pages so that
        // `PRAGMA incremental_vacuum` can hand them back to the OS after a prune
        // WITHOUT a full `VACUUM` (which rewrites the whole file and needs
        // exclusive access). It only takes effect before the first table exists,
        // so it is set here, on a new file, before WAL and the migrations write
        // anything. Without it freed pages are still reused by later inserts; the
        // file just never shrinks.
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")
            .map_err(backend("cannot set auto_vacuum"))?;
    }

    // WAL: appends go to `events.db-wal`, readers keep their snapshot meanwhile.
    // Persistent (stored in the file). See `store/sqlite.rs` for the long form.
    let mode: String = conn
        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
        .map_err(backend("cannot enable WAL"))?;
    if !mode.eq_ignore_ascii_case("wal") {
        tracing::warn!(%mode, "SQLite refused WAL journal mode; continuing");
    }
    // NORMAL with WAL: a crash cannot corrupt the file; a power cut may lose the
    // last few commits. Hook events are an activity log, not accounting.
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(backend("cannot set synchronous=NORMAL"))?;

    migrate(conn)?;
    validate_schema(conn)
}

/// One transaction per migration, together with the `user_version` bump, so a
/// crash leaves the previous version, never something in between. `IMMEDIATE`
/// takes the write lock up front so two instances starting at once cannot both
/// migrate.
fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    loop {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(backend("cannot start migration transaction"))?;
        let version = user_version(&tx)?;
        if version > MIGRATIONS.len() {
            return Err(newer_schema(version));
        }
        let Some(sql) = MIGRATIONS.get(version) else {
            return Ok(());
        };
        tx.execute_batch(sql)
            .map_err(backend(format!("migration to v{} failed", version + 1)))?;
        // Data fixes that SQL cannot express run in the SAME transaction as the
        // schema change and the version bump: either the whole step happened or
        // none of it did.
        if version + 1 == 2 {
            backfill_event_files(&tx)?;
        }
        let next = i64::try_from(version + 1).map_err(|_| newer_schema(version))?;
        tx.pragma_update(None, "user_version", next)
            .map_err(backend("cannot record schema version"))?;
        tx.commit().map_err(backend(format!(
            "cannot commit migration to v{}",
            version + 1
        )))?;
        tracing::info!(version = version + 1, "migrated event store schema");
    }
}

/// v2 backfill: index the edit events a v1 database already holds. Applies the
/// same rule as [`indexed_files`], split in two because SQL cannot canonicalize
/// a path: the `WHERE` clause below is that function's guard (a worktree, an
/// OBSERVED edit-class tool start or finish), and [`canonical_paths`], the very
/// function `indexed_files` ends with, makes the paths. Idempotent (`INSERT OR
/// IGNORE`). One unreadable row is skipped with a warning rather than failing
/// the migration: a bad row must not lock the user out of their whole history
/// (the app would fall back to memory).
fn backfill_event_files(conn: &Connection) -> Result<(), StoreError> {
    let mut select = conn
        .prepare(
            "SELECT event_key, worktree_id, files FROM events \
             WHERE worktree_id IS NOT NULL AND tool_class = 'edit' \
               AND evidence = 'observed' \
               AND kind IN ('tool_started', 'tool_finished')",
        )
        .map_err(backend("cannot prepare event_files backfill"))?;
    let mut insert = conn
        .prepare(INSERT_EVENT_FILE)
        .map_err(backend("cannot prepare event_files insert"))?;
    let rows = select
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(backend("cannot read events for the backfill"))?;
    for row in rows {
        let (key, worktree, files) = row.map_err(backend("cannot read an event row"))?;
        let Ok(files) = serde_json::from_str::<Vec<String>>(&files) else {
            tracing::warn!(event_key = %key, "skipping an event with unreadable files in the backfill");
            continue;
        };
        for path in canonical_paths(&files) {
            insert
                .execute(params![worktree, path, key])
                .map_err(backend("cannot backfill event_files"))?;
        }
    }
    Ok(())
}

fn user_version(conn: &Connection) -> Result<usize, StoreError> {
    let v: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(backend("cannot read schema version"))?;
    usize::try_from(v).map_err(|_| {
        StoreError::Backend(format!(
            "event database schema version {v} is not one this version of WHY understands; \
             refusing to open it"
        ))
    })
}

fn newer_schema(version: usize) -> StoreError {
    StoreError::Backend(format!(
        "event database schema version {version} is newer than this version of WHY supports \
         ({}); refusing to open it so no data is lost. Update WHY.",
        MIGRATIONS.len()
    ))
}

/// `map_err` helper: wrap a rusqlite error as `StoreError::Backend` with context.
fn backend(context: impl Into<String>) -> impl FnOnce(rusqlite::Error) -> StoreError {
    let context = context.into();
    move |e| StoreError::Backend(format!("{context}: {e}"))
}

/// The trait takes `u64` milliseconds; SQLite integers are `i64`. A value above
/// `i64::MAX` (year 292 million) cannot come from a clock; it is refused rather
/// than silently clamped, which would store a wrong time.
fn ms_to_i64(ms: u64) -> Result<i64, StoreError> {
    i64::try_from(ms).map_err(|_| StoreError::Backend(format!("timestamp {ms} ms is out of range")))
}

// ---- Encoding: closed sets as stable strings -------------------------------

/// `(kind, resumed, outcome)` columns of an [`EventKind`].
fn encode_kind(kind: &EventKind) -> (&'static str, Option<bool>, Option<&'static str>) {
    match kind {
        EventKind::SessionStart { resumed } => ("session_start", Some(*resumed), None),
        EventKind::SessionEnd => ("session_end", None, None),
        EventKind::Prompt => ("prompt", None, None),
        EventKind::AssistantMessage => ("assistant_message", None, None),
        EventKind::ToolStarted => ("tool_started", None, None),
        EventKind::ToolFinished { outcome } => (
            "tool_finished",
            None,
            Some(match outcome {
                ToolOutcome::Ok => "ok",
                ToolOutcome::Failed => "failed",
                ToolOutcome::Unknown => "unknown",
            }),
        ),
        EventKind::SubagentStarted => ("subagent_started", None, None),
        EventKind::SubagentStopped => ("subagent_stopped", None, None),
        EventKind::Stop => ("stop", None, None),
    }
}

fn decode_kind(kind: &str, resumed: Option<bool>, outcome: Option<&str>) -> Option<EventKind> {
    Some(match kind {
        "session_start" => EventKind::SessionStart {
            resumed: resumed.unwrap_or(false),
        },
        "session_end" => EventKind::SessionEnd,
        "prompt" => EventKind::Prompt,
        "assistant_message" => EventKind::AssistantMessage,
        "tool_started" => EventKind::ToolStarted,
        "tool_finished" => EventKind::ToolFinished {
            outcome: match outcome? {
                "ok" => ToolOutcome::Ok,
                "failed" => ToolOutcome::Failed,
                "unknown" => ToolOutcome::Unknown,
                _ => return None,
            },
        },
        "subagent_started" => EventKind::SubagentStarted,
        "subagent_stopped" => EventKind::SubagentStopped,
        "stop" => EventKind::Stop,
        _ => return None,
    })
}

fn encode_class(class: ToolClass) -> &'static str {
    match class {
        ToolClass::Edit => "edit",
        ToolClass::Shell => "shell",
        ToolClass::TestRun => "test_run",
        ToolClass::Subagent => "subagent",
        ToolClass::Other => "other",
    }
}

fn decode_class(s: &str) -> Option<ToolClass> {
    Some(match s {
        "edit" => ToolClass::Edit,
        "shell" => ToolClass::Shell,
        "test_run" => ToolClass::TestRun,
        "subagent" => ToolClass::Subagent,
        "other" => ToolClass::Other,
        _ => return None,
    })
}

fn encode_evidence(e: EvidenceLabel) -> &'static str {
    match e {
        EvidenceLabel::Observed => "observed",
        EvidenceLabel::Declared => "declared",
        EvidenceLabel::Inferred => "inferred",
        EvidenceLabel::Unknown => "unknown",
    }
}

fn decode_evidence(s: &str) -> Option<EvidenceLabel> {
    Some(match s {
        "observed" => EvidenceLabel::Observed,
        "declared" => EvidenceLabel::Declared,
        "inferred" => EvidenceLabel::Inferred,
        "unknown" => EvidenceLabel::Unknown,
        _ => return None,
    })
}

/// Exactly the spellings `Harness::as_str` writes (not `Harness::from_str`,
/// which maps any unknown name to `Unknown` and would hide a bad row).
fn decode_harness(s: &str) -> Option<Harness> {
    [Harness::ClaudeCode, Harness::Codex, Harness::Unknown]
        .into_iter()
        .find(|h| h.as_str() == s)
}

/// A decoding failure as a rusqlite error, so it travels through `query_map`.
fn bad_column(index: usize, what: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        Type::Text,
        format!("unknown {what} {value:?}").into(),
    )
}

/// Read one row of [`EVENT_COLUMNS`] back into a [`NormalizedEvent`].
fn event_from_row(row: &Row<'_>) -> rusqlite::Result<NormalizedEvent> {
    let harness: String = row.get(1)?;
    let kind: String = row.get(4)?;
    let outcome: Option<String> = row.get(6)?;
    let at_ms: i64 = row.get(7)?;
    let tool_name: Option<String> = row.get(11)?;
    let tool_class: Option<String> = row.get(12)?;
    let files: String = row.get(17)?;
    let evidence: String = row.get(18)?;

    let tool = match (tool_name, tool_class) {
        (Some(name), Some(class)) => Some(ToolCall {
            use_id: row.get(10)?,
            name,
            class: decode_class(&class).ok_or_else(|| bad_column(12, "tool class", &class))?,
        }),
        _ => None,
    };
    Ok(NormalizedEvent {
        key: EventKey(row.get(0)?),
        session: SessionKey {
            harness: decode_harness(&harness).ok_or_else(|| bad_column(1, "harness", &harness))?,
            session_id: row.get(2)?,
        },
        hook_event: row.get(3)?,
        kind: decode_kind(&kind, row.get(5)?, outcome.as_deref())
            .ok_or_else(|| bad_column(4, "event kind", &kind))?,
        at_ms: u64::try_from(at_ms).map_err(|_| bad_column(7, "timestamp", &at_ms.to_string()))?,
        turn_id: row.get(8)?,
        agent_id: row.get(9)?,
        tool,
        cwd: row.get(13)?,
        worktree_id: row.get::<_, Option<String>>(14)?.map(WorktreeId),
        model: row.get(15)?,
        text: row.get(16)?,
        files: serde_json::from_str(&files).map_err(|_| bad_column(17, "files", &files))?,
        evidence: decode_evidence(&evidence)
            .ok_or_else(|| bad_column(18, "evidence", &evidence))?,
        transcript_path: row.get(19)?,
        // Stored ops are read back through `edit_records`, never attached to
        // the events a timeline is built from.
        edits: Vec::new(),
        edited_files: None,
    })
}

/// A session's events in arrival order.
fn load_events(conn: &Connection, key: &SessionKey) -> Result<Vec<NormalizedEvent>, StoreError> {
    let mut stmt = conn
        .prepare_cached(SESSION_EVENTS_SQL)
        .map_err(backend("cannot prepare session events"))?;
    stmt.query_map(
        params![key.harness.as_str(), key.session_id],
        event_from_row,
    )
    .map_err(backend("cannot read session events"))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(backend("cannot read an event row"))
}

fn lock<'a>(
    conn: &'a Mutex<Connection>,
    what: &str,
) -> Result<MutexGuard<'a, Connection>, StoreError> {
    // Poisoned = a thread panicked while holding it; report instead of
    // panicking again (SQLite itself rolled the interrupted statement back).
    conn.lock()
        .map_err(|_| StoreError::Backend(format!("event store {what} lock poisoned")))
}

const UPSERT_SESSION: &str = "INSERT INTO sessions (harness, session_id, first_event_ms, last_event_ms) \
     VALUES (?1, ?2, ?3, ?3) \
     ON CONFLICT (harness, session_id) DO UPDATE SET \
         first_event_ms = min(first_event_ms, excluded.first_event_ms), \
         last_event_ms = max(last_event_ms, excluded.last_event_ms)";

// `append` looks the key up first (so a duplicate touches nothing), and the
// unique `event_key` with `DO NOTHING` stays as the backstop that makes the
// insert itself atomic: 0 changed rows means "already there".
const INSERT_EVENT: &str = "INSERT INTO events (event_key, harness, session_id, hook_event, kind, \
     resumed, outcome, at_ms, turn_id, agent_id, tool_use_id, tool_name, tool_class, cwd, \
     worktree_id, model, text, files, evidence, transcript_path) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
     ?19, ?20) ON CONFLICT (event_key) DO NOTHING";

const INSERT_EVENT_FILE: &str =
    "INSERT OR IGNORE INTO event_files (worktree_id, path, event_key) VALUES (?1, ?2, ?3)";

const INSERT_EDIT_OP: &str = "INSERT INTO edit_ops (event_key, op_index, event_seq, worktree_id, \
     path, kind, renamed_from, observed, format, before, after, hunks) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)";

/// One file's ops in replay order, with their event's identity, in the column
/// order [`edit_record_from_row`] reads. Served by `edit_ops_by_file` (already
/// in `(event_seq, op_index)` order: no sort step). Public so a test can
/// `EXPLAIN QUERY PLAN` the exact text the store runs.
pub const EDIT_RECORDS_SQL: &str = "SELECT o.event_seq, o.op_index, o.worktree_id, o.path, \
     o.kind, o.renamed_from, o.observed, o.format, o.before, o.after, o.hunks, \
     e.event_key, e.harness, e.session_id, e.turn_id, e.agent_id, e.tool_use_id, e.at_ms \
     FROM edit_ops o JOIN events e ON e.event_key = o.event_key \
     WHERE o.worktree_id = ?1 AND o.path = ?2 \
     ORDER BY o.event_seq, o.op_index";

fn encode_error(e: String) -> StoreError {
    StoreError::Backend(format!("cannot encode an edit op: {e}"))
}

/// Store `ev`'s ops (already known to be a NEW event, stored as `seq`).
fn insert_edit_ops(
    stmt: &mut rusqlite::CachedStatement<'_>,
    ev: &NormalizedEvent,
    seq: i64,
) -> Result<(), StoreError> {
    for (index, op) in ev.edits.iter().enumerate() {
        let Some((wt, op)) = stored_form(op) else {
            continue;
        };
        let index = i64::try_from(index).map_err(|_| encode_error("too many ops".into()))?;
        let before = op
            .before
            .as_ref()
            .map(encode_before)
            .transpose()
            .map_err(encode_error)?;
        let after = op.after.as_ref().map(encode_after);
        let hunks = encode_hunks(&op.hunks).map_err(encode_error)?;
        stmt.execute(params![
            ev.key.0,
            index,
            seq,
            wt.0,
            op.path,
            encode_edit_kind(op.kind),
            op.renamed_from,
            encode_observed(op.observed),
            FORMAT_V1,
            before,
            after,
            hunks,
        ])
        .map_err(backend("cannot store an edit op"))?;
    }
    Ok(())
}

/// A BLOB column that does not decode, as a rusqlite error.
fn bad_blob(index: usize, what: &str, why: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        Type::Blob,
        format!("damaged {what}: {why}").into(),
    )
}

/// Read one row of [`EDIT_RECORDS_SQL`].
fn edit_record_from_row(row: &Row<'_>) -> rusqlite::Result<EditRecord> {
    let seq: i64 = row.get(0)?;
    let op_index: i64 = row.get(1)?;
    let kind: String = row.get(4)?;
    let observed: String = row.get(6)?;
    let format: i64 = row.get(7)?;
    if format != FORMAT_V1 {
        // A row from a newer layout cannot be read by this build: refuse it
        // rather than misreading its bytes.
        return Err(bad_column(7, "edit op format", &format.to_string()));
    }
    let before: Option<Vec<u8>> = row.get(8)?;
    let after: Option<Vec<u8>> = row.get(9)?;
    let hunks: Vec<u8> = row.get(10)?;
    let harness: String = row.get(12)?;
    let at_ms: i64 = row.get(17)?;
    let observed =
        decode_observed(&observed).ok_or_else(|| bad_column(6, "observed", &observed))?;
    let op = EditOp {
        worktree_id: Some(WorktreeId(row.get(2)?)),
        path: row.get(3)?,
        kind: decode_edit_kind(&kind).ok_or_else(|| bad_column(4, "edit kind", &kind))?,
        renamed_from: row.get(5)?,
        before: before
            .map(|b| decode_before(&b))
            .transpose()
            .map_err(|e| bad_blob(8, "before", e))?,
        after: after
            .map(|b| decode_after(&b))
            .transpose()
            .map_err(|e| bad_blob(9, "after", e))?,
        hunks: decode_hunks(&hunks).map_err(|e| bad_blob(10, "hunks", e))?,
        observed,
    };
    if matches!(observed, Observed::Unobserved(_)) && !op.hunks.is_empty() {
        return Err(bad_blob(10, "hunks", "an unobserved op has hunks".into()));
    }
    Ok(EditRecord {
        source: EditSource {
            session: SessionKey {
                harness: decode_harness(&harness)
                    .ok_or_else(|| bad_column(12, "harness", &harness))?,
                session_id: row.get(13)?,
            },
            turn_id: row.get(14)?,
            agent_id: row.get(15)?,
            tool_use_id: row.get(16)?,
            event_key: EventKey(row.get(11)?),
        },
        seq: u64::try_from(seq).map_err(|_| bad_column(0, "seq", &seq.to_string()))?,
        op_index: u32::try_from(op_index)
            .map_err(|_| bad_column(1, "op index", &op_index.to_string()))?,
        at_ms: u64::try_from(at_ms).map_err(|_| bad_column(17, "timestamp", &at_ms.to_string()))?,
        op,
    })
}

impl EventStore for SqliteEventStore {
    fn append(&self, events: &[NormalizedEvent]) -> Result<AppendOutcome, StoreError> {
        let mut outcome = AppendOutcome::default();
        if events.is_empty() {
            return Ok(outcome);
        }
        let mut conn = lock(&self.writer, "writer")?;
        // IMMEDIATE: take the write lock now, so the batch is one all-or-nothing
        // unit and never fails halfway on "database is locked". If anything below
        // returns early, dropping `tx` rolls the whole batch back.
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(backend("cannot start append transaction"))?;

        let mut changed_sessions: HashSet<&SessionKey> = HashSet::new();
        {
            let mut upsert = tx
                .prepare_cached(UPSERT_SESSION)
                .map_err(backend("cannot prepare session upsert"))?;
            let mut seen = tx
                .prepare_cached("SELECT 1 FROM events WHERE event_key = ?1")
                .map_err(backend("cannot prepare key lookup"))?;
            let mut insert = tx
                .prepare_cached(INSERT_EVENT)
                .map_err(backend("cannot prepare event insert"))?;
            let mut link = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO session_worktrees (worktree_id, harness, session_id) \
                     VALUES (?1, ?2, ?3)",
                )
                .map_err(backend("cannot prepare worktree link"))?;

            let mut index = tx
                .prepare_cached(INSERT_EVENT_FILE)
                .map_err(backend("cannot prepare file index insert"))?;
            let mut insert_op = tx
                .prepare_cached(INSERT_EDIT_OP)
                .map_err(backend("cannot prepare edit op insert"))?;

            for ev in events {
                let at = ms_to_i64(ev.at_ms)?;
                let harness = ev.session.harness.as_str();
                // A redelivery must be a TRUE no-op: even one with a different
                // timestamp may not move the session's times or links. So look
                // the key up before touching anything else.
                if seen
                    .exists(params![ev.key.0])
                    .map_err(backend("cannot check event key"))?
                {
                    outcome.duplicates += 1;
                    continue;
                }
                // The parent row (the foreign key needs it), created or widened
                // only for an event that is really new.
                upsert
                    .execute(params![harness, ev.session.session_id, at])
                    .map_err(backend("cannot upsert session"))?;

                let (kind, resumed, kind_outcome) = encode_kind(&ev.kind);
                let files = serde_json::to_string(&ev.files)
                    .map_err(|e| StoreError::Backend(format!("cannot encode files: {e}")))?;
                let inserted = insert
                    .execute(params![
                        ev.key.0,
                        harness,
                        ev.session.session_id,
                        ev.hook_event,
                        kind,
                        resumed,
                        kind_outcome,
                        at,
                        ev.turn_id,
                        ev.agent_id,
                        ev.tool.as_ref().and_then(|t| t.use_id.as_deref()),
                        ev.tool.as_ref().map(|t| t.name.as_str()),
                        ev.tool.as_ref().map(|t| encode_class(t.class)),
                        ev.cwd,
                        ev.worktree_id.as_ref().map(|w| w.0.as_str()),
                        ev.model,
                        ev.text,
                        files,
                        encode_evidence(ev.evidence),
                        ev.transcript_path,
                    ])
                    .map_err(backend("cannot insert event"))?;
                if inserted == 0 {
                    outcome.duplicates += 1;
                    continue;
                }
                outcome.inserted += 1;
                changed_sessions.insert(&ev.session);
                // The ops, in the SAME transaction as their event (both or
                // neither). `events.seq` is the rowid the insert just made.
                if !ev.edits.is_empty() {
                    insert_edit_ops(&mut insert_op, ev, tx.last_insert_rowid())?;
                }
                if let Some(wt) = &ev.worktree_id {
                    link.execute(params![wt.0, harness, ev.session.session_id])
                        .map_err(backend("cannot link session to worktree"))?;
                }
                // The file index, in the SAME transaction as the event: a
                // crash can never leave an event without its index rows.
                // (A redelivery `continue`d above, so it adds none.) Each
                // file is indexed under the worktree that OWNS it, and the
                // session is listed there too (decision 32): that is what
                // makes a session that edited a file of worktree B show up in
                // B and what `edit_calls(B, ..)` finds.
                for (wt, path) in indexed_files(ev) {
                    link.execute(params![wt.0, harness, ev.session.session_id])
                        .map_err(backend("cannot link session to worktree"))?;
                    index
                        .execute(params![wt.0, path, ev.key.0])
                        .map_err(backend("cannot index event files"))?;
                }
            }
        }

        // Every worktree that lists a changed session, from the stored links:
        // that includes worktrees reached through EARLIER events, not just this
        // batch. A `BTreeSet<String>` sorts bytewise, like the memory store.
        let mut worktrees: BTreeSet<String> = BTreeSet::new();
        {
            let mut stmt = tx
                .prepare_cached(
                    "SELECT worktree_id FROM session_worktrees \
                     WHERE harness = ?1 AND session_id = ?2",
                )
                .map_err(backend("cannot prepare changed worktrees"))?;
            for session in &changed_sessions {
                let rows = stmt
                    .query_map(
                        params![session.harness.as_str(), session.session_id],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(backend("cannot read changed worktrees"))?;
                for wt in rows {
                    worktrees.insert(wt.map_err(backend("cannot read a worktree id"))?);
                }
            }
        }
        outcome.changed_worktrees = worktrees.into_iter().map(WorktreeId).collect();

        tx.commit().map_err(backend("cannot commit append"))?;
        Ok(outcome)
    }

    fn sessions_for_worktree(&self, wt: &WorktreeId) -> Result<Vec<SessionSummary>, StoreError> {
        let now = self.now_ms();
        let mut conn = lock(&self.reader, "reader")?;
        // One read transaction: the session list and every session's events come
        // from the same snapshot, even if the writer commits in between.
        let tx = conn
            .transaction()
            .map_err(backend("cannot start read transaction"))?;

        let sessions: Vec<(SessionKey, i64)> = {
            let mut stmt = tx
                .prepare_cached(WORKTREE_SESSIONS_SQL)
                .map_err(backend("cannot prepare worktree sessions"))?;
            let rows = stmt
                .query_map([&wt.0], |row| {
                    let harness: String = row.get(0)?;
                    Ok((
                        SessionKey {
                            harness: decode_harness(&harness)
                                .ok_or_else(|| bad_column(0, "harness", &harness))?,
                            session_id: row.get(1)?,
                        },
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(backend("cannot list worktree sessions"))?;
            rows.collect::<Result<_, _>>()
                .map_err(backend("cannot read a session row"))?
        };

        let mut found = Vec::with_capacity(sessions.len());
        for (key, last_ms) in sessions {
            let events = load_events(&tx, &key)?;
            if let Some(summary) = build_summary(&key, &events, now) {
                found.push((last_ms, summary));
            }
        }
        // The memory store's comparator: most recent first, the id breaks ties.
        found.sort_by(|(a_at, a), (b_at, b)| b_at.cmp(a_at).then_with(|| a.id.0.cmp(&b.id.0)));
        Ok(found.into_iter().map(|(_, s)| s).collect())
    }

    fn timeline(&self, session: &SessionKey) -> Result<Option<SessionTimeline>, StoreError> {
        let now = self.now_ms();
        let mut conn = lock(&self.reader, "reader")?;
        let tx = conn
            .transaction()
            .map_err(backend("cannot start read transaction"))?;
        let events = load_events(&tx, session)?;
        Ok(build_timeline(session, &events, now))
    }

    fn prune_older_than(&self, cutoff_ms: u64) -> Result<usize, StoreError> {
        let mut conn = lock(&self.writer, "writer")?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(backend("cannot start prune transaction"))?;
        // Whole sessions: a session's LAST event decides. ON DELETE CASCADE
        // removes its events and worktree links in this same statement, so a
        // crash can never leave events without their session.
        let removed = match i64::try_from(cutoff_ms) {
            Ok(cutoff) => tx.execute("DELETE FROM sessions WHERE last_event_ms < ?1", [cutoff]),
            // A cutoff above every storable timestamp: everything is older.
            // (Clamping to `i64::MAX` and using `<` would keep a session whose
            // last event is exactly `i64::MAX`.)
            Err(_) => tx.execute("DELETE FROM sessions", []),
        }
        .map_err(backend("cannot prune sessions"))?;
        tx.commit().map_err(backend("cannot commit prune"))?;

        if removed > 0 {
            // Hand the freed pages back to the OS (see `auto_vacuum` above). Best
            // effort: the prune itself is already committed.
            if let Err(e) = release_free_pages(&conn) {
                tracing::warn!(error = %e, "incremental_vacuum after prune failed");
            }
        }
        Ok(removed)
    }

    fn last_event_ms(&self, harness: Harness) -> Result<Option<u64>, StoreError> {
        let conn = lock(&self.reader, "reader")?;
        // `sessions.last_event_ms` is the running MAX of its events' times
        // (kept by `append`), so this reads one small row per session of the
        // harness instead of scanning every event.
        let max: Option<i64> = conn
            .prepare_cached("SELECT max(last_event_ms) FROM sessions WHERE harness = ?1")
            .map_err(backend("cannot prepare last event"))?
            .query_row([harness.as_str()], |row| row.get(0))
            .map_err(backend("cannot read the last event time"))?;
        max.map(|ms| {
            u64::try_from(ms).map_err(|_| StoreError::Backend(format!("bad last event time {ms}")))
        })
        .transpose()
    }

    fn edit_records(&self, wt: &WorktreeId, path: &str) -> Result<Vec<EditRecord>, StoreError> {
        let conn = lock(&self.reader, "reader")?;
        // One statement: its own snapshot, no transaction needed.
        let mut stmt = conn
            .prepare_cached(EDIT_RECORDS_SQL)
            .map_err(backend("cannot prepare edit records"))?;
        stmt.query_map(params![wt.0, path], edit_record_from_row)
            .map_err(backend("cannot read edit records"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend("cannot read an edit op row"))
    }

    fn edit_calls(&self, wt: &WorktreeId, paths: &[String]) -> Result<Vec<EditTouch>, StoreError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = lock(&self.reader, "reader")?;
        // One snapshot for every statement below.
        let tx = conn
            .transaction()
            .map_err(backend("cannot start read transaction"))?;

        // 1. Index probe: the events that name a queried path in this worktree.
        //    Chunked because SQLite caps the number of bound parameters.
        let mut calls_wanted: BTreeSet<(String, String, String)> = BTreeSet::new();
        let mut loose_keys: BTreeSet<String> = BTreeSet::new();
        for chunk in paths.chunks(PATH_CHUNK) {
            let sql = format!(
                "SELECT e.harness, e.session_id, e.tool_use_id, e.event_key \
                 FROM event_files f JOIN events e ON e.event_key = f.event_key \
                 WHERE f.worktree_id = ?1 AND f.path IN ({})",
                placeholders(chunk.len(), 2)
            );
            let mut stmt = tx
                .prepare(&sql)
                .map_err(backend("cannot prepare file index probe"))?;
            let bound = std::iter::once(wt.0.as_str()).chain(chunk.iter().map(String::as_str));
            let rows = stmt
                .query_map(rusqlite::params_from_iter(bound), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(backend("cannot probe the file index"))?;
            for row in rows {
                let (harness, session, use_id, key) =
                    row.map_err(backend("cannot read a file index row"))?;
                match use_id {
                    // A call has two halves; fetch both below, by id.
                    Some(id) => calls_wanted.insert((harness, session, id)),
                    // No id: the event is a call of its own (it cannot pair).
                    None => loose_keys.insert(key),
                };
            }
        }

        // 2. Both halves of each matched call (a finish may carry no `files`,
        //    so the index alone would miss it and report the call as Unknown).
        //    NOT filtered by the events' own worktree: a session that started
        //    in worktree A and edited a file of B has its events resolved to A
        //    (decision 32). Each half's paths are its `event_files` rows in
        //    `wt`, so the call lists only the queried worktree's files.
        let mut events: Vec<CallEvent> = Vec::new();
        {
            let mut by_call = tx
                .prepare_cached(&format!(
                    "{EDIT_CALL_EVENTS_SQL} AND harness = ?1 AND session_id = ?2 \
                     AND tool_use_id = ?3 ORDER BY seq"
                ))
                .map_err(backend("cannot prepare call events"))?;
            let mut file_rows = tx
                .prepare_cached(
                    "SELECT path FROM event_files WHERE worktree_id = ?1 AND event_key = ?2",
                )
                .map_err(backend("cannot prepare call paths"))?;
            for (harness, session, id) in &calls_wanted {
                collect_edit_call_events(
                    &mut by_call,
                    &mut file_rows,
                    wt,
                    params![harness, session, id],
                    &mut events,
                )?;
            }
            let mut by_key = tx
                .prepare_cached(&format!("{EDIT_CALL_EVENTS_SQL} AND event_key = ?1"))
                .map_err(backend("cannot prepare call event"))?;
            for key in &loose_keys {
                collect_edit_call_events(
                    &mut by_key,
                    &mut file_rows,
                    wt,
                    params![key],
                    &mut events,
                )?;
            }
        }
        let calls = fold_calls(events);

        // 3. Merged timeline turn numbers, from a narrow query per session.
        let indices = session_indices(&tx, &distinct_sessions(&calls))?;
        let wanted: HashSet<&str> = paths.iter().map(String::as_str).collect();
        Ok(build_edit_touches(calls, &wanted, &indices))
    }

    fn shell_turns(&self, wt: &WorktreeId) -> Result<Vec<ShellTurn>, StoreError> {
        let mut conn = lock(&self.reader, "reader")?;
        let tx = conn
            .transaction()
            .map_err(backend("cannot start read transaction"))?;
        let mut events: Vec<CallEvent> = Vec::new();
        {
            // The literal `IN ('shell', 'test_run')` is what lets SQLite use the
            // partial index `events_shell_by_worktree`.
            let mut stmt = tx
                .prepare_cached(SHELL_EVENTS_SQL)
                .map_err(backend("cannot prepare shell events"))?;
            collect_call_events(&mut stmt, params![wt.0], &mut events)?;
        }
        let calls = fold_calls(events);
        let indices = session_indices(&tx, &distinct_sessions(&calls))?;
        Ok(build_shell_turns(calls, &indices))
    }

    fn source_events(&self, session: &SessionKey) -> Result<Option<Vec<SourceEvent>>, StoreError> {
        let mut conn = lock(&self.reader, "reader")?;
        let tx = conn
            .transaction()
            .map_err(backend("cannot start read transaction"))?;
        let mut stmt = tx
            .prepare_cached(SOURCE_EVENTS_SQL)
            .map_err(backend("cannot prepare source events"))?;
        let events = stmt
            .query_map(
                params![session.harness.as_str(), session.session_id],
                source_event_from_row,
            )
            .map_err(backend("cannot read source events"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend("cannot read a source event row"))?;
        // A stored session always has at least one event (sessions are implicit).
        Ok((!events.is_empty()).then_some(events))
    }
}

/// What a lazy transcript read needs from a session's events, in arrival
/// order: the ids and the `transcript_path`, never the message text.
const SOURCE_EVENTS_SQL: &str = "SELECT event_key, kind, outcome, evidence, turn_id, agent_id, \
     tool_use_id, tool_name, tool_class, at_ms, transcript_path \
     FROM events WHERE harness = ?1 AND session_id = ?2 ORDER BY seq";

/// One row of [`SOURCE_EVENTS_SQL`]; the same projection as
/// [`SourceEvent::from_event`].
fn source_event_from_row(row: &Row<'_>) -> rusqlite::Result<SourceEvent> {
    let kind: String = row.get(1)?;
    let outcome: Option<String> = row.get(2)?;
    let evidence: String = row.get(3)?;
    let tool_name: Option<String> = row.get(7)?;
    let tool_class: Option<String> = row.get(8)?;
    let at_ms: i64 = row.get(9)?;
    let kind = decode_kind(&kind, None, outcome.as_deref())
        .ok_or_else(|| bad_column(1, "event kind", &kind))?;
    let evidence =
        decode_evidence(&evidence).ok_or_else(|| bad_column(3, "evidence", &evidence))?;
    // Same rule as `correlation::tool_event_class`: an OBSERVED tool start or finish.
    let tool = match (&kind, tool_name, tool_class) {
        (EventKind::ToolStarted | EventKind::ToolFinished { .. }, Some(_), Some(class))
            if evidence == EvidenceLabel::Observed =>
        {
            Some(SourceTool {
                use_id: row.get(6)?,
                class: decode_class(&class).ok_or_else(|| bad_column(8, "tool class", &class))?,
                finished: match kind {
                    EventKind::ToolFinished { outcome } => Some(outcome),
                    _ => None,
                },
            })
        }
        _ => None,
    };
    Ok(SourceEvent {
        key: row.get(0)?,
        turn_id: row.get(4)?,
        agent_id: row.get(5)?,
        lifecycle: kind.is_lifecycle(),
        subagent_stopped: kind == EventKind::SubagentStopped,
        tool,
        at_ms: u64::try_from(at_ms).map_err(|_| bad_column(9, "timestamp", &at_ms.to_string()))?,
        transcript_path: row.get(10)?,
    })
}

/// How many `?` one `IN (...)` list may hold. SQLite's default limit is 32766
/// (older builds 999); 400 keeps well under both.
const PATH_CHUNK: usize = 400;

/// The columns [`collect_edit_call_events`] reads: edit-class tool events,
/// whatever worktree they were resolved to; callers append the rest of the
/// WHERE clause. The last column is the event key (its files are looked up in
/// `event_files`).
const EDIT_CALL_EVENTS_SQL: &str = "SELECT harness, session_id, turn_id, agent_id, tool_use_id, kind, \
     outcome, at_ms, files, event_key FROM events WHERE tool_class = 'edit' \
     AND evidence = 'observed' AND kind IN ('tool_started', 'tool_finished')";

/// Shell and test-run tool events of a worktree, in arrival order (`files` is
/// selected for the shared row decoder; shell events have none).
const SHELL_EVENTS_SQL: &str = "SELECT harness, session_id, turn_id, agent_id, tool_use_id, kind, \
     outcome, at_ms, files FROM events WHERE worktree_id = ?1 \
     AND tool_class IN ('shell', 'test_run') AND evidence = 'observed' AND kind IN ('tool_started', 'tool_finished') \
     ORDER BY seq";

/// Only the fields that place entries in the timeline, in arrival order.
const TURN_ORDER_SQL: &str = "SELECT COALESCE(turn_id, ''), at_ms, agent_id, \
         CASE WHEN kind IN ('tool_started', 'tool_finished') \
              AND tool_name IS NOT NULL AND tool_class IS NOT NULL THEN tool_use_id END, \
         kind = 'tool_started' \
     FROM events WHERE harness = ?1 AND session_id = ?2 \
         AND kind NOT IN ('session_start', 'session_end') ORDER BY seq";

/// `?<from>, ?<from+1>, ...` for `n` placeholders.
fn placeholders(n: usize, from: usize) -> String {
    (from..from + n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Run a statement that selects the [`SHELL_EVENTS_SQL`] columns and push each
/// row as a [`CallEvent`].
fn collect_call_events(
    stmt: &mut rusqlite::CachedStatement<'_>,
    params: impl rusqlite::Params,
    out: &mut Vec<CallEvent>,
) -> Result<(), StoreError> {
    let rows = stmt
        .query_map(params, call_event_from_row)
        .map_err(backend("cannot read call events"))?;
    for row in rows {
        out.push(row.map_err(backend("cannot read a call event row"))?);
    }
    Ok(())
}

/// Like [`collect_call_events`] for edit calls: each event's `paths` are its
/// `event_files` rows in `wt` (the files of that worktree it named), instead of
/// every canonical path of its `files` list.
fn collect_edit_call_events(
    stmt: &mut rusqlite::CachedStatement<'_>,
    file_rows: &mut rusqlite::CachedStatement<'_>,
    wt: &WorktreeId,
    params: impl rusqlite::Params,
    out: &mut Vec<CallEvent>,
) -> Result<(), StoreError> {
    let rows = stmt
        .query_map(params, |row| {
            Ok((call_event_from_row(row)?, row.get::<_, String>(9)?))
        })
        .map_err(backend("cannot read call events"))?;
    for row in rows {
        let (mut event, key) = row.map_err(backend("cannot read a call event row"))?;
        let mut paths = file_rows
            .query_map(params![wt.0, key], |r| r.get::<_, String>(0))
            .map_err(backend("cannot read call paths"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend("cannot read a call path"))?;
        paths.sort();
        event.paths = paths;
        out.push(event);
    }
    Ok(())
}

/// One row of [`SHELL_EVENTS_SQL`] or [`EDIT_CALL_EVENTS_SQL`] as a [`CallEvent`].
fn call_event_from_row(row: &Row<'_>) -> rusqlite::Result<CallEvent> {
    let harness: String = row.get(0)?;
    let kind: String = row.get(5)?;
    let outcome: Option<String> = row.get(6)?;
    let at_ms: i64 = row.get(7)?;
    let files: String = row.get(8)?;
    let finished = match decode_kind(&kind, None, outcome.as_deref())
        .ok_or_else(|| bad_column(5, "event kind", &kind))?
    {
        EventKind::ToolFinished { outcome } => Some(outcome),
        _ => None,
    };
    let files: Vec<String> =
        serde_json::from_str(&files).map_err(|_| bad_column(8, "files", &files))?;
    Ok(CallEvent {
        session: SessionKey {
            harness: decode_harness(&harness).ok_or_else(|| bad_column(0, "harness", &harness))?,
            session_id: row.get(1)?,
        },
        turn_id: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        agent_id: row.get(3)?,
        use_id: row.get(4)?,
        finished,
        at_ms: u64::try_from(at_ms).map_err(|_| bad_column(7, "timestamp", &at_ms.to_string()))?,
        reported_paths: !files.is_empty(),
        paths: canonical_paths(&files),
    })
}

/// Number each session's merged timeline entries without decoding payloads.
fn session_indices(
    conn: &Connection,
    sessions: &[SessionKey],
) -> Result<HashMap<SessionKey, TurnIndices>, StoreError> {
    let mut stmt = conn
        .prepare_cached(TURN_ORDER_SQL)
        .map_err(backend("cannot prepare turn order"))?;
    let mut out = HashMap::new();
    for key in sessions {
        let rows = stmt
            .query_map(params![key.harness.as_str(), key.session_id], |row| {
                let at: i64 = row.get(1)?;
                Ok(TimelinePosition::<String> {
                    turn_id: row.get(0)?,
                    at_ms: u64::try_from(at)
                        .map_err(|_| bad_column(1, "timestamp", &at.to_string()))?,
                    agent_id: row.get(2)?,
                    use_id: row.get(3)?,
                    is_start: row.get(4)?,
                })
            })
            .map_err(backend("cannot read turn order"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend("cannot read a turn order row"))?;
        out.insert(key.clone(), turn_indices(&rows));
    }
    Ok(out)
}

/// `PRAGMA incremental_vacuum` frees one page per step of the statement, so it
/// must be stepped to the end (a plain `execute` would stop after the first).
fn release_free_pages(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA incremental_vacuum")?;
    let mut rows = stmt.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

/// The event store for this run: `events.db` in `data_dir`, or, when that cannot
/// be opened, an in-memory store with a loud error in the log.
///
/// Like `open_repo_store` in `lib.rs`: a corrupt file, a database from a newer
/// WHY, an unwritable directory or an unknown data dir must not stop the app.
/// The file is never overwritten or deleted (opening fails before any write).
/// The cost is that events captured this session are forgotten at exit.
/// `Err` in `data_dir` is "the instance has no data dir at all".
pub fn open_event_store(data_dir: Result<&Path, &String>, clock: Clock) -> Arc<dyn EventStore> {
    let opened = data_dir.map_err(|e| e.clone()).and_then(|dir| {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let path = dir.join("events.db");
        let store = SqliteEventStore::open(&path).map_err(|e| e.to_string())?;
        tracing::info!(path = %path.display(), "opened event store");
        Ok(store)
    });
    match opened {
        Ok(store) => Arc::new(store.with_clock(clock)),
        Err(e) => {
            tracing::error!(
                error = %e,
                "CANNOT OPEN THE EVENT DATABASE (events.db); using a temporary in-memory store. \
                 Agent sessions captured now will be forgotten when WHY exits."
            );
            Arc::new(MemoryEventStore::with_clock(clock))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::test_support::{event, tool};

    fn open_tmp() -> (tempfile::TempDir, SqliteEventStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteEventStore::open(&dir.path().join("events.db")).unwrap();
        (dir, store)
    }

    /// Every field of every kind survives write + read unchanged. The timeline
    /// does not surface `hook_event`, `resumed`, `transcript_path` or the dedupe
    /// key, so only a direct row comparison can catch a lost column.
    #[test]
    fn every_field_round_trips_exactly() {
        let (_dir, store) = open_tmp();
        let mut all = vec![
            event(
                "s1",
                "SessionStart",
                EventKind::SessionStart { resumed: true },
                1,
                None,
            ),
            event("s1", "SessionEnd", EventKind::SessionEnd, 2, Some("p1")),
            event("s1", "UserPromptSubmit", EventKind::Prompt, 3, Some("p1")),
            event("s1", "Stop", EventKind::AssistantMessage, 4, Some("p1")),
            event(
                "s1",
                "SubagentStart",
                EventKind::SubagentStarted,
                5,
                Some("p1"),
            ),
            event(
                "s1",
                "SubagentStop",
                EventKind::SubagentStopped,
                6,
                Some("p1"),
            ),
            event("s1", "Stop", EventKind::Stop, 7, Some("p1")),
            tool("s1", "t1", None, 8, "p1"),
            tool("s1", "t1", Some(ToolOutcome::Ok), 9, "p1"),
            tool("s1", "t2", Some(ToolOutcome::Failed), 10, "p1"),
            tool("s1", "t3", Some(ToolOutcome::Unknown), 11, "p1"),
        ];
        // Optional fields, awkward text and a tool with no use id.
        let full = &mut all[2];
        full.agent_id = Some("agent-7".into());
        full.model = Some("m".into());
        full.text = Some("nul \0 byte, 世界, \u{1F600} and \"quotes\"".into());
        full.files = vec!["b".into(), "a".into(), "b".into()]; // order and duplicate kept
        full.cwd = "/a/\0/b".into();
        full.worktree_id = None;
        full.evidence = EvidenceLabel::Declared;
        full.transcript_path = Some("/t/rollout.jsonl".into());
        for (i, class) in [
            ToolClass::Edit,
            ToolClass::Shell,
            ToolClass::TestRun,
            ToolClass::Subagent,
            ToolClass::Other,
        ]
        .into_iter()
        .enumerate()
        {
            let mut t = tool("s1", "x", None, 20 + i as u64, "p1");
            t.key = EventKey(format!("class-{i}"));
            t.tool = Some(ToolCall {
                use_id: None,
                name: "Custom".into(),
                class,
            });
            all.push(t);
        }
        store.append(&all).unwrap();

        let conn = store.reader.lock().unwrap();
        let loaded = load_events(&conn, &all[0].session).unwrap();
        assert_eq!(loaded, all, "stored in arrival order, nothing lost");
    }

    #[test]
    fn an_unknown_stored_value_is_an_error_not_a_panic() {
        let (_dir, store) = open_tmp();
        let ev = event("s1", "Stop", EventKind::Stop, 1, Some("p1"));
        store.append(std::slice::from_ref(&ev)).unwrap();
        store
            .writer
            .lock()
            .unwrap()
            .execute("UPDATE events SET kind = 'from_the_future'", [])
            .unwrap();
        let err = store.timeline(&ev.session).unwrap_err();
        assert!(err.to_string().contains("from_the_future"), "{err}");
    }

    #[test]
    fn a_timestamp_beyond_i64_is_refused_and_nothing_is_stored() {
        let (_dir, store) = open_tmp();
        let ok = event("s1", "Stop", EventKind::Stop, 1, Some("p1"));
        let bad = event("s1", "Stop", EventKind::Stop, u64::MAX, Some("p2"));
        assert!(store.append(&[ok.clone(), bad]).is_err());
        // The whole batch rolled back, including the valid first event.
        assert!(store.timeline(&ok.session).unwrap().is_none());
    }

    // ---- WHY-16: the file index and its v2 migration ---------------------

    /// `EXPLAIN QUERY PLAN` detail lines of `sql` with `n` text parameters.
    fn plan(store: &SqliteEventStore, sql: &str, n: usize) -> String {
        let conn = store.reader.lock().unwrap();
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(std::iter::repeat_n("x", n)),
                |r| r.get::<_, String>(3),
            )
            .unwrap();
        rows.map(Result::unwrap).collect::<Vec<_>>().join(" | ")
    }

    /// The correlation queries run on indexes, never a table scan: this is the
    /// "an index query must not load every event" requirement, checked against
    /// the exact SQL text the store runs.
    #[test]
    fn correlation_queries_use_their_indexes() {
        let (_dir, store) = open_tmp();

        let probe = "SELECT e.harness, e.session_id, e.tool_use_id, e.event_key \
             FROM event_files f JOIN events e ON e.event_key = f.event_key \
             WHERE f.worktree_id = ?1 AND f.path IN (?2, ?3)";
        let p = plan(&store, probe, 3);
        assert!(p.contains("SEARCH f USING PRIMARY KEY"), "{p}");
        assert!(!p.contains("SCAN"), "{p}");

        let by_call = format!(
            "{EDIT_CALL_EVENTS_SQL} AND harness = ?1 AND session_id = ?2 AND tool_use_id = ?3 ORDER BY seq"
        );
        let p = plan(&store, &by_call, 3);
        assert!(p.contains("events_by_tool_use"), "{p}");
        assert!(!p.contains("SCAN"), "{p}");

        // The per-event file lookup of a call half (`edit_calls`).
        let paths = "SELECT path FROM event_files WHERE worktree_id = ?1 AND event_key = ?2";
        let p = plan(&store, paths, 2);
        assert!(p.contains("SEARCH"), "{p}");
        assert!(!p.contains("SCAN"), "{p}");

        let p = plan(&store, SHELL_EVENTS_SQL, 1);
        assert!(p.contains("events_shell_by_worktree"), "{p}");
        assert!(!p.contains("SCAN"), "{p}");

        let p = plan(&store, TURN_ORDER_SQL, 2);
        assert!(p.contains("events_by_session"), "{p}");
    }

    /// Insert `events` the way WHY v1 did: the v1 schema exactly, no `event_files`.
    fn write_v1_database(path: &Path, events: &[NormalizedEvent], corrupt_files_of: Option<&str>) {
        let conn = Connection::open(path).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")
            .unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        for ev in events {
            let at = ev.at_ms as i64;
            conn.execute(
                UPSERT_SESSION,
                params![ev.session.harness.as_str(), ev.session.session_id, at],
            )
            .unwrap();
            let (kind, resumed, outcome) = encode_kind(&ev.kind);
            conn.execute(
                INSERT_EVENT,
                params![
                    ev.key.0,
                    ev.session.harness.as_str(),
                    ev.session.session_id,
                    ev.hook_event,
                    kind,
                    resumed,
                    outcome,
                    at,
                    ev.turn_id,
                    ev.agent_id,
                    ev.tool.as_ref().and_then(|t| t.use_id.as_deref()),
                    ev.tool.as_ref().map(|t| t.name.as_str()),
                    ev.tool.as_ref().map(|t| encode_class(t.class)),
                    ev.cwd,
                    ev.worktree_id.as_ref().map(|w| w.0.as_str()),
                    ev.model,
                    ev.text,
                    serde_json::to_string(&ev.files).unwrap(),
                    encode_evidence(ev.evidence),
                    ev.transcript_path,
                ],
            )
            .unwrap();
            if let Some(wt) = &ev.worktree_id {
                conn.execute(
                    "INSERT OR IGNORE INTO session_worktrees (worktree_id, harness, session_id) \
                     VALUES (?1, ?2, ?3)",
                    params![wt.0, ev.session.harness.as_str(), ev.session.session_id],
                )
                .unwrap();
            }
        }
        if let Some(key) = corrupt_files_of {
            conn.execute(
                "UPDATE events SET files = 'not json' WHERE event_key = ?1",
                [key],
            )
            .unwrap();
        }
    }

    fn count(path: &Path, sql: &str) -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row(sql, [], |r| r.get(0))
            .unwrap()
    }

    /// A v1 `events.db` opens, is migrated in place, backfilled with the same
    /// rule `append` uses, answers correlation queries, and a second open changes
    /// nothing.
    #[test]
    fn a_v1_database_migrates_in_place_and_backfills_the_file_index() {
        use crate::events::test_support::tool;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");

        let with_files = |mut ev: NormalizedEvent, files: &[&str], class: ToolClass| {
            ev.files = files.iter().map(|f| (*f).to_owned()).collect();
            ev.tool.as_mut().unwrap().class = class;
            ev
        };
        // `tool` builds Bash events in wt-1; reshape them into the cases we need.
        let pre = with_files(
            tool("s1", "t1", None, 10, "p1"),
            &["./a/../b.rs", "/abs/x.rs"],
            ToolClass::Edit,
        );
        let post = with_files(
            tool("s1", "t1", Some(ToolOutcome::Ok), 11, "p1"),
            &[],
            ToolClass::Edit,
        );
        let shell = with_files(
            tool("s1", "t2", None, 12, "p1"),
            &["b.rs"],
            ToolClass::Shell,
        );
        let mut no_wt = with_files(tool("s1", "t3", None, 13, "p1"), &["b.rs"], ToolClass::Edit);
        no_wt.worktree_id = None;
        let broken = with_files(tool("s1", "t4", None, 14, "p1"), &["c.rs"], ToolClass::Edit);
        let events = [pre, post, shell, no_wt, broken.clone()];
        write_v1_database(&path, &events, Some(&broken.key.0));

        // Premise: a genuine v1 file.
        assert_eq!(count(&path, "PRAGMA user_version"), 1);
        assert_eq!(
            count(
                &path,
                "SELECT count(*) FROM sqlite_schema WHERE name = 'event_files'"
            ),
            0
        );

        for round in 0..2 {
            let store = SqliteEventStore::open(&path).unwrap();
            assert_eq!(
                count(&path, "PRAGMA user_version"),
                SCHEMA_VERSION as i64,
                "round {round}"
            );
            // Only the `b.rs` row of the edit start: the finish names no file, the shell
            // event is not an edit, `no_wt` has no worktree, `broken` is unreadable
            // (skipped, not fatal).
            assert_eq!(
                count(&path, "SELECT count(*) FROM event_files"),
                1,
                "round {round}"
            );
            // No event was touched by the migration.
            assert_eq!(
                count(&path, "SELECT count(*) FROM events"),
                5,
                "round {round}"
            );

            let touches = store
                .edit_calls(&WorktreeId("wt-1".into()), &["b.rs".to_owned()])
                .unwrap();
            assert_eq!(touches.len(), 1, "round {round}");
            assert_eq!(
                touches[0].outcome,
                ToolOutcome::Ok,
                "paired with its finish"
            );
            assert_eq!(touches[0].turn_id, "p1");
        }

        // New events after the migration are indexed by `append`.
        let store = SqliteEventStore::open(&path).unwrap();
        let mut fresh = tool("s2", "n1", Some(ToolOutcome::Ok), 20, "p9");
        fresh.files = vec!["new.rs".into()];
        fresh.tool.as_mut().unwrap().class = ToolClass::Edit;
        store.append(&[fresh]).unwrap();
        assert_eq!(
            store
                .edit_calls(&WorktreeId("wt-1".into()), &["new.rs".to_owned()])
                .unwrap()
                .len(),
            1
        );
    }

    /// A failed migration leaves a v1 file exactly as it was (the whole step is
    /// one transaction): simulate a failure after the schema changes by making
    /// the backfill's target already exist as something else.
    #[test]
    fn a_failed_v2_migration_leaves_the_v1_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        write_v1_database(&path, &[], None);
        // `event_files` exists with the wrong shape, so `CREATE TABLE event_files`
        // inside the v2 step fails.
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE event_files (x)")
            .unwrap();

        assert!(SqliteEventStore::open(&path).is_err());
        assert_eq!(count(&path, "PRAGMA user_version"), 1, "version not bumped");
        assert_eq!(
            count(
                &path,
                "SELECT count(*) FROM sqlite_schema WHERE name = 'events_by_tool_use'"
            ),
            0,
            "the step's other statements rolled back too"
        );
    }

    // ---- v3 (WHY-64): edit ops ---------------------------------------------

    use crate::provenance::{
        EditHunk, EditKind, FileSnapshot, HunkLineHash, HunkLineKind, LineHasher, UnobservedReason,
    };

    /// A successful edit of `path` in `wt` carrying a `before` of `lines`
    /// lines and one 3-line hunk.
    fn edit_event(
        session: &str,
        use_id: &str,
        at: u64,
        path: &str,
        lines: usize,
    ) -> NormalizedEvent {
        let h = LineHasher::default();
        let text: String = (0..lines).map(|i| format!("line {i}\n")).collect();
        let mut ev = tool(session, use_id, Some(ToolOutcome::Ok), at, "p1");
        ev.tool.as_mut().unwrap().class = ToolClass::Edit;
        ev.files = vec![path.to_owned()];
        ev.edits = vec![EditOp {
            worktree_id: Some(WorktreeId("wt-1".into())),
            path: path.to_owned(),
            kind: EditKind::Update,
            renamed_from: None,
            before: Some(FileSnapshot::of_text(&h, text.as_bytes())),
            after: None,
            hunks: vec![EditHunk {
                old_start: Some(1),
                header: Some(h.hash_line(b"fn f() {")),
                lines: [
                    HunkLineKind::Context,
                    HunkLineKind::Removed,
                    HunkLineKind::Added,
                ]
                .into_iter()
                .enumerate()
                .map(|(i, kind)| HunkLineHash {
                    kind,
                    hash: h.hash_line(format!("hunk {i}").as_bytes()),
                })
                .collect(),
            }],
            observed: Observed::Exact,
        }];
        ev
    }

    #[test]
    fn edit_records_use_their_index() {
        let (_dir, store) = open_tmp();
        let p = plan(&store, EDIT_RECORDS_SQL, 2);
        assert!(p.contains("edit_ops_by_file"), "{p}");
        assert!(!p.contains("SCAN"), "{p}");
        assert!(!p.contains("TEMP B-TREE"), "no sort step: {p}");
    }

    /// The ops and their event are one unit: if storing an op fails, the
    /// event is not stored either (and neither is the rest of the batch).
    #[test]
    fn an_op_that_cannot_be_stored_rolls_back_its_event() {
        let (_dir, store) = open_tmp();
        store
            .writer
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER refuse_ops BEFORE INSERT ON edit_ops \
                 BEGIN SELECT RAISE(ABORT, 'refused by test'); END;",
            )
            .unwrap();
        let plain = event("s1", "UserPromptSubmit", EventKind::Prompt, 1, Some("p1"));
        let edit = edit_event("s1", "t1", 2, "a.rs", 3);
        assert!(store.append(&[plain, edit.clone()]).is_err());
        let key = SessionKey::new(Harness::ClaudeCode, "s1");
        assert_eq!(store.timeline(&key).unwrap(), None, "nothing was stored");

        store
            .writer
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER refuse_ops")
            .unwrap();
        assert_eq!(store.append(&[edit]).unwrap().inserted, 1);
        let wt = WorktreeId("wt-1".into());
        assert_eq!(store.edit_records(&wt, "a.rs").unwrap().len(), 1);
    }

    #[test]
    fn a_row_in_an_unknown_format_or_with_damaged_bytes_is_refused() {
        let (_dir, store) = open_tmp();
        store
            .append(&[edit_event("s1", "t1", 1, "a.rs", 3)])
            .unwrap();
        let wt = WorktreeId("wt-1".into());
        let writer = store.writer.lock().unwrap();
        writer
            .execute("UPDATE edit_ops SET format = 2", [])
            .unwrap();
        drop(writer);
        assert!(store.edit_records(&wt, "a.rs").is_err(), "newer layout");

        let writer = store.writer.lock().unwrap();
        writer
            .execute("UPDATE edit_ops SET format = 1, hunks = x'0100'", [])
            .unwrap();
        drop(writer);
        assert!(store.edit_records(&wt, "a.rs").is_err(), "truncated hunks");
    }

    /// The numbers for the owner's "should `before` be capped?" decision
    /// (printed with `--nocapture`), plus a bound so a layout change that
    /// bloats rows is noticed.
    #[test]
    fn storage_cost_of_edit_ops() {
        let (dir, store) = open_tmp();
        let row_bytes = |store: &SqliteEventStore| -> i64 {
            store
                .reader
                .lock()
                .unwrap()
                .query_row(
                    "SELECT sum(length(event_key) + 8 + length(worktree_id) + length(path) \
                     + length(kind) + length(observed) + coalesce(length(before), 0) \
                     + coalesce(length(after), 0) + length(hunks)) FROM edit_ops",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        store
            .append(&[edit_event("s1", "big", 1, "big.rs", 1000)])
            .unwrap();
        let one = row_bytes(&store);
        eprintln!("edit_ops: one edit with a 1,000-line `before`: {one} bytes of row data");
        // 16,025 B of `before` + ~60 B of hunks + keys.
        assert!((16_000..17_000).contains(&one), "{one}");

        // A "typical session": 40 Claude edits of 300-line files.
        let (dir2, store2) = open_tmp();
        let page = |d: &tempfile::TempDir| {
            let c = Connection::open(d.path().join("events.db")).unwrap();
            c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
            let pages: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
            let size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
            pages * size
        };
        let empty = page(&dir2);
        let session: Vec<_> = (0..40)
            .map(|i| {
                edit_event(
                    "s2",
                    &format!("t{i}"),
                    10 + i,
                    &format!("f{}.rs", i % 8),
                    300,
                )
            })
            .collect();
        store2.append(&session).unwrap();
        let session_rows = row_bytes(&store2);
        let grown = page(&dir2) - empty;
        eprintln!(
            "edit_ops: 40 edits of 300-line files: {session_rows} bytes of row data \
             ({} per edit); events.db grew {grown} bytes including the events themselves",
            session_rows / 40
        );
        drop((dir, dir2));
    }

    /// Write a genuine v2 file: the v1 helper, then the v2 step exactly as a
    /// v2 WHY ran it (schema, backfill, version).
    fn write_v2_database(path: &Path, events: &[NormalizedEvent]) {
        write_v1_database(path, events, None);
        let mut conn = Connection::open(path).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute_batch(MIGRATIONS[1]).unwrap();
        backfill_event_files(&tx).unwrap();
        tx.pragma_update(None, "user_version", 2).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn a_v2_database_migrates_to_v3_and_keeps_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        let mut old_edit = tool("s1", "t1", Some(ToolOutcome::Ok), 2, "p1");
        old_edit.tool.as_mut().unwrap().class = ToolClass::Edit;
        old_edit.files = vec!["a.rs".into()];
        let events = [
            event("s1", "UserPromptSubmit", EventKind::Prompt, 1, Some("p1")),
            old_edit,
            event("s1", "Stop", EventKind::Stop, 3, Some("p1")),
        ];
        write_v2_database(&path, &events);
        assert_eq!(count(&path, "PRAGMA user_version"), 2, "premise");
        assert_eq!(
            count(&path, "SELECT count(*) FROM event_files"),
            1,
            "premise"
        );

        for round in 0..2 {
            let store = SqliteEventStore::open(&path).unwrap();
            assert_eq!(
                count(&path, "PRAGMA user_version"),
                SCHEMA_VERSION as i64,
                "round {round}"
            );
            assert_eq!(count(&path, "SELECT count(*) FROM events"), 3);
            assert_eq!(count(&path, "SELECT count(*) FROM sessions"), 1);
            assert_eq!(count(&path, "SELECT count(*) FROM session_worktrees"), 1);
            assert_eq!(count(&path, "SELECT count(*) FROM event_files"), 1);
            assert_eq!(count(&path, "SELECT count(*) FROM edit_ops"), 0);
            let wt = WorktreeId("wt-1".into());
            // Old data still answers; old events simply have no ops.
            assert_eq!(store.edit_calls(&wt, &["a.rs".into()]).unwrap().len(), 1);
            assert!(store.edit_records(&wt, "a.rs").unwrap().is_empty());
            let key = SessionKey::new(Harness::ClaudeCode, "s1");
            assert_eq!(store.timeline(&key).unwrap().unwrap().turns.len(), 1);
        }

        // New events after the migration store their ops.
        let store = SqliteEventStore::open(&path).unwrap();
        store
            .append(&[edit_event("s1", "t9", 9, "a.rs", 2)])
            .unwrap();
        let wt = WorktreeId("wt-1".into());
        assert_eq!(store.edit_records(&wt, "a.rs").unwrap().len(), 1);
    }

    #[test]
    fn a_failed_v3_migration_leaves_the_v2_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        write_v2_database(&path, &[]);
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE edit_ops (x)")
            .unwrap();
        assert!(SqliteEventStore::open(&path).is_err());
        assert_eq!(count(&path, "PRAGMA user_version"), 2, "version not bumped");
        assert_eq!(
            count(
                &path,
                "SELECT count(*) FROM sqlite_schema WHERE name = 'edit_ops_by_file'"
            ),
            0
        );
    }

    #[test]
    fn unobserved_ops_round_trip_without_content() {
        let (_dir, store) = open_tmp();
        let mut ev = edit_event("s1", "t1", 1, "a.rs", 3);
        ev.edits[0].observed = Observed::Unobserved(UnobservedReason::PayloadReduced);
        store.append(&[ev]).unwrap();
        let got = store
            .edit_records(&WorktreeId("wt-1".into()), "a.rs")
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].op.observed,
            Observed::Unobserved(UnobservedReason::PayloadReduced)
        );
        assert!(got[0].op.hunks.is_empty() && got[0].op.before.is_none());
    }
}
