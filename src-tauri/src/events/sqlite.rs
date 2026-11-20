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
//! only remembers arrival order inside this file and is never read out of it.
//!
//! # Threads
//!
//! The trait is synchronous. A call blocks its thread for the duration of a
//! SQLite statement: microseconds for a single event, a few milliseconds for a
//! timeline. **WHY-13's collector should wrap `append` / `prune_older_than` in
//! `tauri::async_runtime::spawn_blocking`** (they take the writer lock and may
//! wait on `fsync`/`busy_timeout`); the IPC read commands are short enough to
//! call directly, but `spawn_blocking` is the safe choice for a big timeline too.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::types::Type;
use rusqlite::{Connection, OpenFlags, Row, TransactionBehavior, params};
use why_wire::Harness;

use super::clock::Clock;
use super::model::{EventKey, EventKind, NormalizedEvent, SessionKey, ToolCall, ToolClass};
use super::timeline::{build_summary, build_timeline};
use super::{AppendOutcome, EventStore, MemoryEventStore};
use crate::contracts::{EvidenceLabel, SessionSummary, SessionTimeline, ToolOutcome, WorktreeId};
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
];

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
                if let Some(wt) = &ev.worktree_id {
                    link.execute(params![wt.0, harness, ev.session.session_id])
                        .map_err(backend("cannot link session to worktree"))?;
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
}
