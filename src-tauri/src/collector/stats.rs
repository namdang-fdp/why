//! Counters the collector keeps, readable by tests and (later) a diagnostics
//! page. Plain atomics: bumped from many tasks, read rarely, never needed to be
//! consistent with each other.

use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! counters {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {
        /// Live counters (shared as `Arc<Stats>`).
        #[derive(Debug, Default)]
        pub struct Stats { $($(#[$doc])* pub $name: AtomicU64,)* }

        /// A copy of the counters at one moment.
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
        pub struct StatsSnapshot { $(pub $name: u64,)* }

        impl Stats {
            pub fn snapshot(&self) -> StatsSnapshot {
                StatsSnapshot { $($name: self.$name.load(Ordering::Relaxed),)* }
            }
        }
    };
}

counters! {
    /// Connections accepted from our own uid.
    connections,
    /// Connections refused because the peer is another user (`SO_PEERCRED`).
    rejected_peer,
    /// Connections refused because too many were open (a flood).
    rejected_busy,
    /// Connections closed for being silent or slow (idle/overall timeout).
    timeouts,
    /// Envelopes decoded and queued for ingest.
    received,
    /// Lines that were not an envelope (not JSON, missing fields, not UTF-8).
    malformed,
    /// Lines longer than `MAX_LINE_BYTES`.
    oversized,
    /// Envelopes with a protocol version this build does not speak.
    unsupported_version,
    /// Connections that ended in the middle of a line (a hook that gave up).
    unterminated,
    /// Envelopes dropped because the ingest queue was full (backpressure).
    dropped_backpressure,
    /// Envelopes dropped because the queued bytes would exceed the memory budget.
    dropped_memory,
    /// Envelopes the ingest task finished (stored or ignored).
    ingested,
    /// Envelopes whose ingest failed (store error).
    ingest_errors,
    /// Envelopes ingested from the spool.
    spool_ingested,
    /// Spool lines that were not a valid envelope (a torn write, an old format).
    spool_malformed,
    /// Spool records skipped because they were older than the retention window.
    spool_expired,
    /// Spool files fully ingested and deleted.
    spool_files_done,
}

/// `counter += 1`.
pub fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}
