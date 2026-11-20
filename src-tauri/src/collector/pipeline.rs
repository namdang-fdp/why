//! From a received [`Envelope`] to stored events and a UI notification.
//!
//! This is `events::ingest::ingest` taken apart into its three steps, because
//! the collector needs to treat them differently:
//!
//! 1. normalize (pure; swappable through [`Normalizer`] so tests can run the
//!    pipeline while the WHY-14/15 adapters are still stubs);
//! 2. resolve each distinct cwd to a worktree (async: runs git);
//! 3. store (sync: the `EventStore` trait is blocking, and SQLite may take
//!    milliseconds, so it runs on a blocking thread, not on an async worker).
//!
//! Step 3 reuses `ingest::store_events` unchanged, through [`PreResolved`]: a
//! resolver that answers from the map step 2 already filled, so the future
//! never actually waits and can be driven to completion with `block_on` inside
//! the blocking thread.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use why_wire::Envelope;

use super::resolver::GitWorktreeResolver;
use crate::contracts::WorktreeId;
use crate::events::ingest::{ResolvedWorktree, WorktreeResolver, store_events};
use crate::events::{AppendOutcome, EventStore, NormalizedEvent};
use crate::store::StoreError;

/// Turns an envelope into events. Production: the harness adapters.
pub trait Normalizer: Send + Sync {
    fn normalize(&self, env: &Envelope) -> Vec<NormalizedEvent>;
}

/// The real thing: [`crate::adapters::normalize`].
#[derive(Debug, Clone, Copy, Default)]
pub struct AdapterNormalizer;

impl Normalizer for AdapterNormalizer {
    fn normalize(&self, env: &Envelope) -> Vec<NormalizedEvent> {
        crate::adapters::normalize(env)
    }
}

/// Answers `resolve` from answers computed beforehand.
struct PreResolved(HashMap<String, Option<ResolvedWorktree>>);

impl WorktreeResolver for PreResolved {
    async fn resolve(&self, cwd: &str) -> Option<ResolvedWorktree> {
        self.0.get(cwd).cloned().flatten()
    }
}

/// Batches "this worktree's sessions changed" notices.
///
/// An agent doing tool calls produces several events per second; the UI only
/// needs to re-read once per burst. The first notice starts a window; every
/// worktree noticed inside it is emitted once when the window ends.
pub struct ChangeNotifier {
    emit: Arc<dyn Fn(WorktreeId) + Send + Sync>,
    window: Duration,
    pending: Mutex<HashSet<WorktreeId>>,
    scheduled: AtomicBool,
}

impl ChangeNotifier {
    pub fn new(emit: Arc<dyn Fn(WorktreeId) + Send + Sync>, window: Duration) -> Arc<Self> {
        Arc::new(Self {
            emit,
            window,
            pending: Mutex::new(HashSet::new()),
            scheduled: AtomicBool::new(false),
        })
    }

    pub fn notify(self: &Arc<Self>, worktrees: Vec<WorktreeId>) {
        if worktrees.is_empty() {
            return;
        }
        if let Ok(mut pending) = self.pending.lock() {
            pending.extend(worktrees);
        }
        // Only the caller that flips the flag starts the window.
        if self.scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(this.window).await;
            // Reset BEFORE draining: a notice arriving in between starts a new
            // window instead of being left behind.
            this.scheduled.store(false, Ordering::SeqCst);
            let batch: Vec<WorktreeId> = this
                .pending
                .lock()
                .map(|mut p| p.drain().collect())
                .unwrap_or_default();
            for wt in batch {
                (this.emit)(wt);
            }
        });
    }
}

/// Everything needed to turn one envelope into stored events. Shared (behind
/// an `Arc`) by the live ingest task and the spool drain.
pub struct Pipeline {
    normalizer: Arc<dyn Normalizer>,
    resolver: GitWorktreeResolver,
    events: Arc<dyn EventStore>,
    notifier: Arc<ChangeNotifier>,
}

impl Pipeline {
    pub fn new(
        normalizer: Arc<dyn Normalizer>,
        resolver: GitWorktreeResolver,
        events: Arc<dyn EventStore>,
        notifier: Arc<ChangeNotifier>,
    ) -> Self {
        Self {
            normalizer,
            resolver,
            events,
            notifier,
        }
    }

    /// Normalize, resolve and store `env`, then notify the UI.
    ///
    /// Idempotent: a redelivery (the spool replaying an event the socket also
    /// delivered) carries the same event keys, which the store ignores.
    pub async fn ingest(&self, env: &Envelope) -> Result<AppendOutcome, StoreError> {
        let events = self.normalizer.normalize(env);
        if events.is_empty() {
            return Ok(AppendOutcome::default());
        }
        // One git resolution per distinct cwd (usually one per envelope).
        let mut resolved = HashMap::new();
        for ev in &events {
            if !resolved.contains_key(&ev.cwd) {
                let wt = self.resolver.resolve(&ev.cwd).await;
                resolved.insert(ev.cwd.clone(), wt);
            }
        }

        let store = Arc::clone(&self.events);
        let handle = tokio::runtime::Handle::current();
        let outcome = tauri::async_runtime::spawn_blocking(move || {
            handle.block_on(store_events(events, &PreResolved(resolved), &*store))
        })
        .await
        .map_err(|e| StoreError::Backend(format!("ingest task failed: {e}")))??;

        self.notifier.notify(outcome.changed_worktrees.clone());
        Ok(outcome)
    }
}
