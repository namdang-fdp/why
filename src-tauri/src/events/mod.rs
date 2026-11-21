//! Agent activity: sessions, turns and events captured from hooks.
//!
//! The same seam pattern as [`crate::store`]: the IPC commands and the
//! collector depend only on the [`EventStore`] trait; backends plug in behind
//! it. [`MemoryEventStore`] is the fake tests plug in and the fallback when
//! `events.db` cannot be opened; [`SqliteEventStore`] is the real, persistent
//! store (decision 24), chosen in `lib.rs` only.
//!
//! Data flow (v0.2):
//!
//! ```text
//! why-hook ──socket──▶ collector (WHY-13) ──▶ ingest::ingest
//!                                              ├─ adapters::normalize (WHY-14/15)
//!                                              ├─ WorktreeResolver   (WHY-13)
//!                                              └─ EventStore::append (WHY-12)
//! UI ──IPC──▶ ipc::sessions ──▶ EventStore::{sessions_for_worktree, timeline}
//! ```
//!
//! # Errors
//!
//! The trait reuses [`StoreError`] instead of defining its own enum: a broken
//! events database fails in exactly the ways the repository database does (a
//! backend error the UI cannot act on), so a second enum would add a second
//! `From` impl in `ipc/error.rs` that maps to the same `IpcError::Internal`.
//! If WHY-12 needs a variant (say, "database from a newer WHY"), adding it to
//! `StoreError` forces the mapping to be written (the match is exhaustive).

pub mod clock;
pub mod correlation;
pub mod ingest;
pub mod memory;
pub mod model;
pub mod sqlite;
pub mod timeline;

#[cfg(test)]
pub(crate) mod test_support;

use std::time::Duration;

use crate::contracts::{SessionSummary, SessionTimeline, WorktreeId};
use crate::store::StoreError;

pub use clock::Clock;
pub use correlation::{EditTouch, ShellTurn, canonical_path};
pub use memory::MemoryEventStore;
pub use model::{
    EXCERPT_MAX_BYTES, EventKey, EventKind, NormalizedEvent, SessionKey, ToolCall, ToolClass,
    excerpt,
};
pub use sqlite::{SqliteEventStore, open_event_store};

/// How long events are kept (decision 22).
pub const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// What [`EventStore::append`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppendOutcome {
    /// Events stored for the first time.
    pub inserted: usize,
    /// Events whose [`EventKey`] was already stored (a redelivery); ignored.
    pub duplicates: usize,
    /// Every worktree that lists a session with a newly inserted event, once
    /// per worktree. Includes worktrees associated through earlier events.
    pub changed_worktrees: Vec<WorktreeId>,
}

/// Everything the core keeps about agent sessions.
///
/// Same conventions as [`crate::store::RepoStore`]: synchronous, `&self`,
/// `Send + Sync` (it lives in `AppState` as `Arc<dyn EventStore>`), quick.
pub trait EventStore: Send + Sync {
    /// Store events, skipping any whose [`EventKey`] is already stored (also
    /// within `events` itself). Idempotent: appending the same batch twice
    /// stores it once. Sessions are implicit: the first event of a
    /// [`SessionKey`] creates it, later ones (including a resume's
    /// `SessionStart`) update it. Compute changed worktrees with the append
    /// so memberships and insertions are read from the same store state.
    fn append(&self, events: &[NormalizedEvent]) -> Result<AppendOutcome, StoreError>;

    /// Sessions with at least one event in this worktree, most recently active
    /// first.
    fn sessions_for_worktree(&self, wt: &WorktreeId) -> Result<Vec<SessionSummary>, StoreError>;

    /// One session's timeline, or `None` if no such session is stored.
    fn timeline(&self, session: &SessionKey) -> Result<Option<SessionTimeline>, StoreError>;

    /// Forget every session whose LAST event is older than `cutoff_ms` (Unix
    /// milliseconds), with all its events; returns how many sessions went.
    /// Whole sessions, not single events: pruning a long session's early events
    /// would leave a timeline without its first prompt or its `SessionStart`.
    fn prune_older_than(&self, cutoff_ms: u64) -> Result<usize, StoreError>;

    /// The edit-class tool calls of worktree `wt` that named at least one of
    /// `paths`, newest first (WHY-16). `paths` must already be in
    /// [`canonical_path`] form: worktree-relative, `/`-separated; a path that
    /// is not canonical simply matches nothing. Each result lists only the
    /// queried paths its call named. A successful finish's targets take
    /// precedence when it reports them. Start and finish are merged into one
    /// call, with outcome `Unknown` when no finish was seen. Only
    /// events resolved to `wt` count: the same relative path in another
    /// worktree of the repository is a different file.
    ///
    /// Cost: SQLite reads the index rows of the queried paths plus the events
    /// of the calls they point at, plus the timeline ordering fields of the
    /// matching sessions. The memory store scans all stored events.
    ///
    /// The default is an error, not an empty list: a store that forgot to
    /// implement this must fail loudly, because "no edits found" would label
    /// every changed file `unexplained`. Both real stores override it.
    fn edit_calls(
        &self,
        _wt: &WorktreeId,
        _paths: &[String],
    ) -> Result<Vec<EditTouch>, StoreError> {
        Err(unsupported("edit_calls"))
    }

    /// The turns that ran shell commands in worktree `wt`, newest first. A
    /// lead for the reviewer ("this turn ran commands here"), never an
    /// attribution: what a command changed is only known to git. Start-only
    /// attempts remain unconfirmed in the timeline.
    ///
    /// Same default as [`EventStore::edit_calls`]: an error, never "none".
    fn shell_turns(&self, _wt: &WorktreeId) -> Result<Vec<ShellTurn>, StoreError> {
        Err(unsupported("shell_turns"))
    }
}

fn unsupported(method: &str) -> StoreError {
    StoreError::Backend(format!("this event store does not implement {method}"))
}
