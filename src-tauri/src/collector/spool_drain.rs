//! Ingesting the spool: the events hooks parked while the app was not
//! listening. The file-level protocol (rename, lock, read, delete) lives in
//! `why_wire::spool`; this is the app side that calls it.
//!
//! A file is deleted only after EVERY line in it was ingested (or was
//! recognised as garbage). If the store fails midway the file stays and the
//! next drain replays it: events already stored are ignored by their key, so
//! replaying is safe and losing is not.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;
use why_wire::spool::{self, ClaimedFile};
use why_wire::{WireError, decode_line};

use super::pipeline::Pipeline;
use super::stats::{Stats, bump};
use crate::events::RETENTION;

/// Hooks cap the spool at `MAX_SPOOL_BYTES`; a file this big cannot be ours.
const MAX_FILE_BYTES: u64 = 2 * spool::MAX_SPOOL_BYTES;

/// What one [`SpoolDrainer::drain_once`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainReport {
    /// Envelopes handed to the pipeline.
    pub ingested: u64,
    /// Lines skipped as garbage (torn write, not an envelope, unknown version).
    pub malformed: u64,
    /// Records (or whole files) older than the retention window, dropped
    /// unread by the store so replay never resurrects an expired session.
    pub expired: u64,
    /// Files fully ingested and deleted.
    pub files_done: u64,
    /// Files left for the next drain (busy lock, or an ingest error).
    pub files_deferred: u64,
}

pub struct SpoolDrainer {
    dir: PathBuf,
    pipeline: Arc<Pipeline>,
    stats: Arc<Stats>,
    lock_wait: Duration,
    ticket: AtomicU64,
}

impl SpoolDrainer {
    pub fn new(dir: PathBuf, pipeline: Arc<Pipeline>, stats: Arc<Stats>) -> Self {
        Self {
            dir,
            pipeline,
            stats,
            lock_wait: Duration::from_secs(2),
            // Unique per run, so a renamed file never overwrites a leftover.
            ticket: AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            ),
        }
    }

    /// Drain every spool file once. Never panics, never fails: problems are
    /// counted, logged and retried next time.
    pub async fn drain_once(&self) -> DrainReport {
        let mut report = DrainReport::default();
        let dir = self.dir.clone();
        let Ok(files) =
            tauri::async_runtime::spawn_blocking(move || spool::pending_files(&dir)).await
        else {
            return report;
        };
        for path in files {
            let ticket = self.ticket.fetch_add(1, Ordering::Relaxed);
            let lock_wait = self.lock_wait;
            // File work is blocking (`flock` waits, reads): off the async workers.
            let claimed = tauri::async_runtime::spawn_blocking(move || {
                let mut claimed = spool::claim(&path, ticket, lock_wait).ok().flatten()?;
                let bytes = claimed.read_all(MAX_FILE_BYTES).ok()?;
                Some((claimed, bytes))
            })
            .await
            .ok()
            .flatten();
            let Some((claimed, bytes)) = claimed else {
                report.files_deferred += 1;
                continue;
            };
            self.ingest_file(claimed, bytes, &mut report).await;
        }
        report
    }

    async fn ingest_file(&self, claimed: ClaimedFile, bytes: Vec<u8>, report: &mut DrainReport) {
        let now = SystemTime::now();
        let cutoff = now.checked_sub(RETENTION).unwrap_or(UNIX_EPOCH);
        // A file nobody has appended to within the retention window holds only
        // expired events: delete it without reading a line.
        let whole_file_expired = claimed.modified().is_ok_and(|m| m < cutoff);
        let cutoff_ms = cutoff
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(0))
            .unwrap_or(0);
        for line in bytes.split_inclusive(|&b| b == b'\n') {
            // Blank lines are the spool's record framing (`why_wire::spool`).
            if line == b"\n" || line == b"\r\n" {
                continue;
            }
            if whole_file_expired {
                report.expired += 1;
                bump(&self.stats.spool_expired);
                continue;
            }
            // A line without its `\n` is a torn append (a short write): the
            // hook never retries, so there is nothing to complete it with.
            let decoded = if line.ends_with(b"\n") {
                decode_line(line)
            } else {
                Err(WireError::NotUtf8)
            };
            match decoded {
                // Older than the retention window: the periodic prune would
                // delete the session again, so never insert it.
                Ok(env) if env.received_at_ms < cutoff_ms => {
                    report.expired += 1;
                    bump(&self.stats.spool_expired);
                }
                Ok(env) => match self.pipeline.ingest(&env).await {
                    Ok(_) => {
                        report.ingested += 1;
                        bump(&self.stats.spool_ingested);
                    }
                    Err(e) => {
                        // Keep the file: dropping `claimed` releases its lock and
                        // leaves it for the next drain.
                        tracing::warn!(error = %e, file = %claimed.path().display(), "spool ingest failed; will retry");
                        bump(&self.stats.ingest_errors);
                        report.files_deferred += 1;
                        return;
                    }
                },
                Err(e) => {
                    tracing::debug!(error = %e, "skipping a bad spool line");
                    report.malformed += 1;
                    bump(&self.stats.spool_malformed);
                }
            }
        }
        match tauri::async_runtime::spawn_blocking(move || claimed.delete()).await {
            Ok(Ok(())) => {
                report.files_done += 1;
                bump(&self.stats.spool_files_done);
            }
            _ => report.files_deferred += 1,
        }
    }
}

/// Drain now, then every `every`, or when `wake` is notified, until shutdown.
pub async fn run(
    drainer: Arc<SpoolDrainer>,
    every: Duration,
    wake: Arc<tokio::sync::Notify>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        let report = drainer.drain_once().await;
        if report.ingested + report.malformed > 0 {
            tracing::info!(?report, "drained the hook spool");
        }
        tokio::select! {
            _ = tokio::time::sleep(every) => {}
            _ = wake.notified() => {}
            _ = shutdown.changed() => return,
        }
    }
}
