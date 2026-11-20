//! Retention: old sessions are pruned at startup with a 30-day cutoff.

use std::sync::atomic::{AtomicU64, Ordering};

use why_lib::contracts::{SessionSummary, SessionTimeline};
use why_lib::events::{AppendOutcome, RETENTION};
use why_lib::store::StoreError;

use super::*;

/// Records the cutoff of the first `prune_older_than` call.
struct Counting {
    inner: MemoryEventStore,
    cutoff: AtomicU64,
    calls: AtomicU64,
}

impl EventStore for Counting {
    fn append(&self, e: &[NormalizedEvent]) -> Result<AppendOutcome, StoreError> {
        self.inner.append(e)
    }
    fn sessions_for_worktree(&self, wt: &WorktreeId) -> Result<Vec<SessionSummary>, StoreError> {
        self.inner.sessions_for_worktree(wt)
    }
    fn timeline(&self, s: &SessionKey) -> Result<Option<SessionTimeline>, StoreError> {
        self.inner.timeline(s)
    }
    fn prune_older_than(&self, cutoff_ms: u64) -> Result<usize, StoreError> {
        self.cutoff.store(cutoff_ms, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.prune_older_than(cutoff_ms)
    }
}

#[tokio::test]
async fn startup_prunes_with_a_thirty_day_cutoff_and_then_repeats() {
    let rig = Rig::new();
    let store = Arc::new(Counting {
        inner: MemoryEventStore::new(),
        cutoff: AtomicU64::new(0),
        calls: AtomicU64::new(0),
    });
    let mut cfg = rig.config(Arc::new(TestNormalizer));
    cfg.events = Arc::clone(&store) as Arc<dyn EventStore>;
    cfg.timing.retention_every = Duration::from_millis(100);
    let core = why_lib::collector::spawn_core(cfg);
    core.ready().await;

    eventually(Duration::from_secs(5), "pruned at least twice", || {
        store.calls.load(Ordering::SeqCst) >= 2
    })
    .await;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let expected = now_ms - RETENTION.as_millis() as u64;
    let got = store.cutoff.load(Ordering::SeqCst);
    assert!(
        got.abs_diff(expected) < 10_000,
        "cutoff {got} is not ~30 days before now ({expected})"
    );
}
