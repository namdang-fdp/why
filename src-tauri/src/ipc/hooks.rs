//! Commands that install and remove WHY's hooks in the agents' user-level
//! config (WHY-51). All logic lives in [`crate::hooks`]; these are thin.
//!
//! The file work is blocking (fsync, flock), so each `_impl` runs it on
//! Tauri's blocking pool instead of stalling an async worker.
//!
//! Expected refusals (invalid config, managed policy, a file that changed since
//! the preview, ...) are VALUES (`PreviewResult::Refused`, `ApplyResult::Stale`),
//! not `Err`, so the UI can render them; `Err(IpcError)` is only for failures
//! nobody can act on (I/O).

use tauri::{AppHandle, Manager, State};
use why_wire::Harness;

use crate::collector::CollectorHandle;
use crate::contracts::{ApplyResult, HookHarness, HookStatusReport, IpcError, PreviewResult};
use crate::events::EventStore;
use crate::hooks::{CollectorSnapshot, HookContext, HookError, LastEvents, Target};
use crate::state::AppState;

fn context(app: &AppHandle) -> Result<HookContext, IpcError> {
    // `try_state`: the context only exists when the instance's paths could be
    // resolved at startup; `State<_>` would panic otherwise.
    app.try_state::<HookContext>()
        .map(|s| s.inner().clone())
        .ok_or_else(|| IpcError::Internal {
            message: "the hook installer is unavailable (no data directory)".into(),
        })
}

async fn run<T: Send + 'static>(
    ctx: &HookContext,
    work: impl FnOnce(&HookContext) -> Result<T, HookError> + Send + 'static,
) -> Result<T, IpcError> {
    let ctx = ctx.clone();
    tauri::async_runtime::spawn_blocking(move || work(&ctx))
        .await
        .map_err(|e| IpcError::Internal {
            message: format!("hook installer task failed: {e}"),
        })?
        .map_err(IpcError::from)
}

/// The exact change installing the hooks would make. Writes nothing.
#[tauri::command]
pub async fn preview_install(
    app: AppHandle,
    harness: HookHarness,
) -> Result<PreviewResult, IpcError> {
    preview_install_impl(&context(&app)?, harness).await
}

pub async fn preview_install_impl(
    ctx: &HookContext,
    harness: HookHarness,
) -> Result<PreviewResult, IpcError> {
    run(ctx, move |c| {
        c.preview_install_result(Target::from(harness))
    })
    .await
}

/// Install the hooks the user previewed. `fingerprint` is the one the preview
/// returned; if the file changed since, nothing is written and a fresh
/// preview comes back as `stale`.
#[tauri::command]
pub async fn install(
    app: AppHandle,
    harness: HookHarness,
    fingerprint: String,
) -> Result<ApplyResult, IpcError> {
    install_impl(&context(&app)?, harness, &fingerprint).await
}

pub async fn install_impl(
    ctx: &HookContext,
    harness: HookHarness,
    fingerprint: &str,
) -> Result<ApplyResult, IpcError> {
    let fingerprint = fingerprint.to_owned();
    run(ctx, move |c| {
        c.install_result(Target::from(harness), &fingerprint)
    })
    .await
}

/// The exact change removing the hooks would make. Writes nothing.
#[tauri::command]
pub async fn preview_uninstall(
    app: AppHandle,
    harness: HookHarness,
) -> Result<PreviewResult, IpcError> {
    preview_uninstall_impl(&context(&app)?, harness).await
}

pub async fn preview_uninstall_impl(
    ctx: &HookContext,
    harness: HookHarness,
) -> Result<PreviewResult, IpcError> {
    run(ctx, move |c| {
        c.preview_uninstall_result(Target::from(harness))
    })
    .await
}

/// Remove WHY's hooks (restoring the original bytes when the file is untouched
/// since WHY wrote it).
#[tauri::command]
pub async fn uninstall(
    app: AppHandle,
    harness: HookHarness,
    fingerprint: String,
) -> Result<ApplyResult, IpcError> {
    uninstall_impl(&context(&app)?, harness, &fingerprint).await
}

pub async fn uninstall_impl(
    ctx: &HookContext,
    harness: HookHarness,
    fingerprint: &str,
) -> Result<ApplyResult, IpcError> {
    let fingerprint = fingerprint.to_owned();
    run(ctx, move |c| {
        c.uninstall_result(Target::from(harness), &fingerprint)
    })
    .await
}

/// Per-harness hook health plus the running collector's.
#[tauri::command]
pub async fn hook_status(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<HookStatusReport, IpcError> {
    let collector = app
        .try_state::<CollectorHandle>()
        .map(|h| CollectorSnapshot {
            status: h.status(),
            stats: h.stats(),
        });
    let last_events = last_events(state.events.as_ref());
    hook_status_impl(&context(&app)?, collector, last_events).await
}

/// When each harness last sent an event, from the event store. `None` when the
/// store cannot answer: the status then says "unknown" instead of "never".
pub fn last_events(events: &dyn EventStore) -> Option<LastEvents> {
    Some(LastEvents {
        claude_code: events.last_event_ms(Harness::ClaudeCode).ok()?,
        codex: events.last_event_ms(Harness::Codex).ok()?,
    })
}

pub async fn hook_status_impl(
    ctx: &HookContext,
    collector: Option<CollectorSnapshot>,
    last_events: Option<LastEvents>,
) -> Result<HookStatusReport, IpcError> {
    run(ctx, move |c| Ok(c.hook_status(collector, last_events))).await
}
