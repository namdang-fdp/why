//! Debounce and coalesce raw change notifications into `WorktreeChanged` events.
//!
//! A single `git checkout` or `cargo build` touches hundreds of files within
//! milliseconds, and an editor's "save" is often three syscalls (write a temp
//! file, rename it over the original, update metadata). Forwarding each one
//! would make the UI refetch the same diff dozens of times. Instead, per
//! worktree, we collect changes until the worktree has been quiet for
//! [`DebounceConfig::quiet`], then emit ONE event with the deduplicated paths.
//!
//! The quiet window alone is not enough: an agent writing a file every 100 ms
//! would keep the window open forever and the UI would never update. So a
//! batch is also flushed once its *first* change is [`DebounceConfig::max_delay`]
//! old, even if changes keep coming. Debounce bounds the event rate; max-delay
//! bounds the latency.
//!
//! The input channel is bounded (see [`raw_channel`]): memory stays bounded
//! even if the OS reports changes faster than we can process them, and a
//! dropped change is never silently lost, it turns into "refetch everything".
//!
//! This module is pure logic on tokio channels and timers, with no notify and
//! no Tauri, so tests drive it with tokio's paused clock (`tokio::time::pause`)
//! and run in microseconds.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{Notify, mpsc};
use tokio::time::{Instant, sleep_until};

use crate::contracts::{WorktreeChanged, WorktreeId};

/// One filtered notification about one worktree: what the router sends in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawChange {
    pub worktree: WorktreeId,
    pub change: Change,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// This file (relative to the worktree root) changed.
    Path(String),
    /// Something changed that is not a single file (HEAD, index, refs, a
    /// directory...): refetch everything.
    All,
    /// This path was renamed away and no longer exists, so we cannot tell
    /// whether it was a file or a whole directory. Unless a [`Change::Settled`]
    /// for it arrives before the batch flushes, the batch becomes "refetch
    /// everything" (a directory moved out of the checkout took an unknown set
    /// of files with it).
    MovedAway(String),
    /// The rename that moved this path away has been matched with its
    /// destination, and the destination is a file: it was a plain file rename
    /// (an editor's atomic save), so the path alone is accurate.
    Settled(String),
}

impl RawChange {
    pub fn path(worktree: WorktreeId, path: String) -> Self {
        Self {
            worktree,
            change: Change::Path(path),
        }
    }

    pub fn refetch_all(worktree: WorktreeId) -> Self {
        Self {
            worktree,
            change: Change::All,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DebounceConfig {
    /// Flush a worktree's batch once no change arrived for this long.
    pub quiet: Duration,
    /// Flush a batch at the latest this long after its first change.
    pub max_delay: Duration,
    /// A batch with more distinct paths than this is sent as an empty list
    /// ("refetch everything"): past a few hundred files, a full refetch is
    /// cheaper for the UI than a long list, and the payload stays small.
    pub max_paths: usize,
    /// Capacity of the raw channel from the notify thread (see [`raw_channel`]).
    pub queue_capacity: usize,
}

impl Default for DebounceConfig {
    fn default() -> Self {
        Self {
            quiet: Duration::from_millis(200),
            max_delay: Duration::from_secs(1),
            max_paths: 256,
            queue_capacity: 4096,
        }
    }
}

/// State shared by both ends of the raw channel: which worktrees lost a change
/// because the channel was full.
#[derive(Debug, Default)]
struct Overflow {
    worktrees: Mutex<HashSet<WorktreeId>>,
    /// Wakes the debouncer when a worktree overflows, so it does not wait for
    /// the next message (there may be none) to notice.
    wake: Notify,
}

/// Sending half of the raw channel. Cheap to clone; usable from any thread
/// (it never blocks and never `.await`s).
#[derive(Debug, Clone)]
pub struct RawSender {
    tx: mpsc::Sender<RawChange>,
    overflow: Arc<Overflow>,
}

/// Receiving half, consumed by [`run_debouncer`].
#[derive(Debug)]
pub struct RawReceiver {
    rx: mpsc::Receiver<RawChange>,
    overflow: Arc<Overflow>,
}

/// A bounded channel for raw changes.
///
/// Why bounded with `try_send`: the sender is notify's plain OS thread. It
/// cannot `.await` a full channel, and blocking it (`blocking_send`) would stop
/// it from reading the kernel's event queue, which would then overflow and
/// lose events anyway. Unbounded would let memory grow without limit during a
/// flood. So when the channel is full we drop the change and remember its
/// worktree; the next batch for that worktree is then "refetch everything",
/// which is always correct, just less precise.
pub fn raw_channel(capacity: usize) -> (RawSender, RawReceiver) {
    let (tx, rx) = mpsc::channel(capacity.max(1));
    let overflow = Arc::new(Overflow::default());
    (
        RawSender {
            tx,
            overflow: Arc::clone(&overflow),
        },
        RawReceiver { rx, overflow },
    )
}

impl RawSender {
    /// Queue a change, or record an overflow if the queue is full. Returns
    /// `false` only if the debouncer has stopped (the hub is shutting down).
    pub fn send(&self, change: RawChange) -> bool {
        match self.tx.try_send(change) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(change)) => {
                self.overflow
                    .worktrees
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(change.worktree);
                // `notify_one` stores a permit if the debouncer is not waiting
                // right now, so the wake-up is never lost.
                self.overflow.wake.notify_one();
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

/// Changes collected for one worktree since its last flush.
#[derive(Debug)]
struct Pending {
    first: Instant,
    last: Instant,
    /// `BTreeSet`: deduplicates and gives a sorted, deterministic list.
    paths: BTreeSet<String>,
    /// Paths renamed away whose kind (file or directory) is still unknown.
    unresolved: HashSet<String>,
    /// Set once anything asked for a full refetch, or `paths` overflowed;
    /// `paths` is then irrelevant and kept empty.
    all: bool,
}

impl Pending {
    fn deadline(&self, cfg: &DebounceConfig) -> Instant {
        (self.last + cfg.quiet).min(self.first + cfg.max_delay)
    }

    fn into_event(self, worktree_id: WorktreeId) -> WorktreeChanged {
        let all = self.all || !self.unresolved.is_empty();
        WorktreeChanged {
            worktree_id,
            paths: if all {
                Vec::new()
            } else {
                self.paths.into_iter().collect()
            },
        }
    }
}

/// Run the debouncer until the input is closed (all senders dropped), then
/// flush what is pending and return. Also returns early if `output` is closed.
///
/// One task for all worktrees: it sleeps until the earliest deadline among the
/// pending batches, so idle worktrees cost nothing and there is no per-worktree
/// timer task to clean up.
pub async fn run_debouncer(
    mut input: RawReceiver,
    output: mpsc::Sender<WorktreeChanged>,
    cfg: DebounceConfig,
) {
    let mut pending: HashMap<WorktreeId, Pending> = HashMap::new();
    loop {
        // Flush what is due BEFORE taking more input. If we only flushed when
        // the input queue happened to be empty, a queue that never drains (a
        // flood of events) would postpone every flush forever and max-delay
        // would mean nothing.
        let now = Instant::now();
        let due: Vec<WorktreeId> = pending
            .iter()
            .filter(|(_, p)| p.deadline(&cfg) <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            if let Some(p) = pending.remove(&id)
                && output.send(p.into_event(id)).await.is_err()
            {
                return; // nobody listens any more
            }
        }

        let next_deadline = pending.values().map(|p| p.deadline(&cfg)).min();
        tokio::select! {
            // `biased`: poll the branches in this order (no random choice), so
            // tests are deterministic. Fairness to the timer comes from the
            // due-check at the top of the loop, not from `select!`.
            biased;
            msg = input.rx.recv() => match msg {
                Some(change) => add(&mut pending, change, &cfg),
                None => {
                    // Input closed: the hub is shutting down. Flush and stop.
                    take_overflow(&input, &mut pending, &cfg);
                    for (id, p) in pending.drain() {
                        if output.send(p.into_event(id)).await.is_err() {
                            return;
                        }
                    }
                    return;
                }
            },
            () = input.overflow.wake.notified() => {}
            // Disabled (never polled) while nothing is pending.
            () = sleep_until(next_deadline.unwrap_or_else(Instant::now)), if next_deadline.is_some() => {}
        }
        take_overflow(&input, &mut pending, &cfg);
    }
}

/// Turn "a change for this worktree was dropped" into a refetch-all change.
fn take_overflow(
    input: &RawReceiver,
    pending: &mut HashMap<WorktreeId, Pending>,
    cfg: &DebounceConfig,
) {
    let lost: Vec<WorktreeId> = input
        .overflow
        .worktrees
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain()
        .collect();
    for wt in lost {
        add(pending, RawChange::refetch_all(wt), cfg);
    }
}

fn add(pending: &mut HashMap<WorktreeId, Pending>, change: RawChange, cfg: &DebounceConfig) {
    let now = Instant::now();
    let p = pending.entry(change.worktree).or_insert_with(|| Pending {
        first: now,
        last: now,
        paths: BTreeSet::new(),
        unresolved: HashSet::new(),
        all: false,
    });
    p.last = now;
    if p.all {
        return;
    }
    match change.change {
        Change::All => {
            p.all = true;
            p.paths.clear();
            p.unresolved.clear();
        }
        Change::Path(path) => {
            p.paths.insert(path);
        }
        Change::MovedAway(path) => {
            p.unresolved.insert(path.clone());
            p.paths.insert(path);
        }
        Change::Settled(path) => {
            p.unresolved.remove(&path);
        }
    }
    if p.paths.len() > cfg.max_paths {
        p.all = true;
        p.paths.clear();
        p.unresolved.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::advance;

    fn wt(s: &str) -> WorktreeId {
        WorktreeId(s.to_string())
    }

    struct Harness {
        tx: RawSender,
        rx: mpsc::Receiver<WorktreeChanged>,
    }

    /// Start the debouncer on the current (paused-clock) test runtime.
    fn start(cfg: DebounceConfig) -> Harness {
        let (tx, input) = raw_channel(cfg.queue_capacity);
        let (output, rx) = mpsc::channel(64);
        tokio::spawn(run_debouncer(input, output, cfg));
        Harness { tx, rx }
    }

    impl Harness {
        fn send(&self, wt_id: &str, change: Change) {
            assert!(self.tx.send(RawChange {
                worktree: wt(wt_id),
                change,
            }));
        }

        fn path(&self, wt_id: &str, p: &str) {
            self.send(wt_id, Change::Path(p.to_string()));
        }

        /// Let the debouncer task run (it only progresses when we yield).
        async fn settle(&self) {
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
        }

        fn try_next(&mut self) -> Option<WorktreeChanged> {
            self.rx.try_recv().ok()
        }
    }

    // `start_paused = true`: tokio's clock does not move unless we `advance` it
    // (or every task is idle, in which case it auto-advances to the next timer).
    // That makes timing tests exact and instant instead of slow and flaky.

    #[tokio::test(start_paused = true)]
    async fn burst_becomes_one_event_after_quiet_period() {
        let mut h = start(DebounceConfig::default());
        h.path("w", "b.rs");
        h.path("w", "a.rs");
        h.path("w", "b.rs"); // duplicate
        h.settle().await;

        advance(Duration::from_millis(199)).await;
        h.settle().await;
        assert_eq!(h.try_next(), None, "still inside the quiet window");

        advance(Duration::from_millis(2)).await;
        h.settle().await;
        assert_eq!(
            h.try_next(),
            Some(WorktreeChanged {
                worktree_id: wt("w"),
                paths: vec!["a.rs".into(), "b.rs".into()], // deduped, sorted
            })
        );
        assert_eq!(h.try_next(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn each_change_restarts_the_quiet_window() {
        let mut h = start(DebounceConfig::default());
        for _ in 0..3 {
            h.path("w", "a.rs");
            h.settle().await;
            advance(Duration::from_millis(150)).await;
            h.settle().await;
            assert_eq!(h.try_next(), None);
        }
        advance(Duration::from_millis(60)).await;
        h.settle().await;
        assert!(h.try_next().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn continuous_writes_flush_at_max_delay() {
        let mut h = start(DebounceConfig::default());
        let mut events = Vec::new();
        // A write every 100 ms for 2.5 s: the quiet window never elapses.
        for i in 0..25 {
            h.path("w", &format!("f{i}"));
            h.settle().await;
            advance(Duration::from_millis(100)).await;
            h.settle().await;
            while let Some(e) = h.try_next() {
                events.push((i, e));
            }
        }
        // Flushed at ~1 s and ~2 s despite the constant stream.
        assert_eq!(events.len(), 2, "events: {events:?}");
        assert_eq!(events[0].0, 9, "first flush 1 s after the first write");
        assert_eq!(events[0].1.paths.len(), 10);
    }

    #[tokio::test(start_paused = true)]
    async fn a_queue_that_never_drains_cannot_starve_the_deadline() {
        let mut h = start(DebounceConfig::default());
        h.path("w", "a.rs");
        h.settle().await; // "w" is pending from t=0

        // Fill the queue with far more changes for another worktree than the
        // debouncer can take in one poll (tokio's cooperative budget makes a
        // task yield after ~128 channel operations), then jump past the
        // deadline in one step. The queue is still full when the debouncer
        // next runs.
        for i in 0..3000 {
            h.path("x", &format!("x{i}"));
        }
        advance(Duration::from_millis(1500)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        // "w" was due: it must have been flushed already, long before the
        // 3000 queued changes were drained (that takes ~25 more polls).
        let mut flushed = Vec::new();
        while let Some(e) = h.try_next() {
            flushed.push(e);
        }
        assert!(
            flushed
                .iter()
                .any(|e| e.worktree_id == wt("w") && e.paths == ["a.rs"]),
            "w must be flushed despite the full queue; got {flushed:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn too_many_paths_become_refetch_all() {
        let mut h = start(DebounceConfig {
            max_paths: 5,
            ..DebounceConfig::default()
        });
        for i in 0..6 {
            h.path("w", &format!("f{i}"));
        }
        h.settle().await;
        advance(Duration::from_millis(250)).await;
        h.settle().await;
        assert_eq!(
            h.try_next(),
            Some(WorktreeChanged {
                worktree_id: wt("w"),
                paths: vec![]
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_queue_turns_into_refetch_all_for_that_worktree() {
        // Capacity 2, and the debouncer is not started yet, so nothing drains.
        let (tx, input) = raw_channel(2);
        for i in 0..5 {
            assert!(tx.send(RawChange::path(wt("w"), format!("f{i}"))));
        }
        tx.send(RawChange::path(wt("other"), "o".into())); // also dropped
        let (output, mut rx) = mpsc::channel(8);
        tokio::spawn(run_debouncer(input, output, DebounceConfig::default()));
        for _ in 0..10 {
            tokio::task::yield_now().await; // let it take the queue in
        }

        advance(Duration::from_millis(300)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        let mut got = HashMap::new();
        while let Ok(e) = rx.try_recv() {
            got.insert(e.worktree_id.clone(), e.paths);
        }
        // Both worktrees lost changes: both get "refetch everything", even
        // "other", none of whose changes made it into the queue.
        assert_eq!(got.get(&wt("w")), Some(&vec![]));
        assert_eq!(got.get(&wt("other")), Some(&vec![]));
    }

    #[tokio::test(start_paused = true)]
    async fn refetch_all_wins_over_paths() {
        let mut h = start(DebounceConfig::default());
        h.path("w", "a.rs");
        h.send("w", Change::All);
        h.path("w", "b.rs");
        h.settle().await;
        advance(Duration::from_millis(250)).await;
        h.settle().await;
        assert_eq!(h.try_next().unwrap().paths, Vec::<String>::new());
    }

    #[tokio::test(start_paused = true)]
    async fn moved_away_without_settle_means_refetch_all() {
        let mut h = start(DebounceConfig::default());
        // An editor's atomic save: temp file renamed over the real one.
        h.send("w", Change::MovedAway(".a.rs.tmp".into()));
        h.path("w", "a.rs");
        h.send("w", Change::Settled(".a.rs.tmp".into()));
        h.settle().await;
        advance(Duration::from_millis(250)).await;
        h.settle().await;
        assert_eq!(
            h.try_next().unwrap().paths,
            vec![".a.rs.tmp".to_string(), "a.rs".to_string()]
        );

        // A directory moved out of the checkout: never settled.
        h.send("w", Change::MovedAway("src".into()));
        h.settle().await;
        advance(Duration::from_millis(250)).await;
        h.settle().await;
        assert_eq!(h.try_next().unwrap().paths, Vec::<String>::new());
    }

    #[tokio::test(start_paused = true)]
    async fn worktrees_are_batched_independently() {
        let mut h = start(DebounceConfig::default());
        h.path("a", "x");
        h.settle().await;
        advance(Duration::from_millis(100)).await;
        h.path("b", "y");
        h.settle().await;

        advance(Duration::from_millis(101)).await;
        h.settle().await;
        assert_eq!(h.try_next().unwrap().worktree_id, wt("a"));
        assert_eq!(h.try_next(), None, "b is still in its window");

        advance(Duration::from_millis(100)).await;
        h.settle().await;
        assert_eq!(h.try_next().unwrap().worktree_id, wt("b"));
    }

    #[tokio::test(start_paused = true)]
    async fn closing_input_flushes_pending_and_stops() {
        let Harness { tx, mut rx } = start(DebounceConfig::default());
        tx.send(RawChange::path(wt("w"), "a".into()));
        drop(tx);
        let e = rx.recv().await.unwrap();
        assert_eq!(e.paths, vec!["a".to_string()]);
        assert!(rx.recv().await.is_none(), "task ended, output closed");
    }
}
