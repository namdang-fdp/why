//! Commands that read agent sessions (v0.2 session context).
//!
//! Owner after WHY-46: WHY-12 (store) / WHY-17 (UI). Both commands only use the
//! [`EventStore`](crate::events::EventStore) trait, so they keep working when
//! WHY-12 swaps the memory store for SQLite.

use tauri::State;

use crate::contracts::{IpcError, SessionId, SessionSummary, SessionTimeline, WorktreeId};
use crate::events::SessionKey;
use crate::state::AppState;

/// The agent sessions seen in a worktree, most recently active first. An
/// unknown worktree simply has no sessions (not an error: sessions can exist
/// before the UI has listed the worktree, and vice versa).
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, AppState>,
    worktree_id: WorktreeId,
) -> Result<Vec<SessionSummary>, IpcError> {
    list_sessions_impl(&state, &worktree_id).await
}

pub async fn list_sessions_impl(
    state: &AppState,
    worktree_id: &WorktreeId,
) -> Result<Vec<SessionSummary>, IpcError> {
    Ok(state.events.sessions_for_worktree(worktree_id)?)
}

/// One session's turns and events.
#[tauri::command]
pub async fn get_session_timeline(
    state: State<'_, AppState>,
    session_id: SessionId,
) -> Result<SessionTimeline, IpcError> {
    get_session_timeline_impl(&state, &session_id).await
}

pub async fn get_session_timeline_impl(
    state: &AppState,
    session_id: &SessionId,
) -> Result<SessionTimeline, IpcError> {
    // Ids are opaque to the UI, which only passes back what `list_sessions`
    // gave it; one that does not parse was not made by us.
    let key = SessionKey::from_contract(session_id).ok_or_else(|| IpcError::InvalidInput {
        message: format!("{:?} is not a session id", session_id.0),
    })?;
    state.events.timeline(&key)?.ok_or(IpcError::NotFound)
}
