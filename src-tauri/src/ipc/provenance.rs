//! `get_line_origins`: which captured edit wrote each line of a file (WHY-64).
//!
//! Today the replay engine is a stub, so the command validates its request and
//! answers `unavailable { engineNotReady }` (a value, not an error: the UI
//! shows "not available yet"). It reads no file and runs no git, so it needs
//! no `ReadPermit`. WHY-22/23 fill the body: `EventStore::edit_records`, the
//! base revision's lines, `provenance::replay`, alignment, runs.

use tauri::State;

use crate::contracts::{IpcError, LineOriginsResult, LineOriginsUnavailable, WorktreeId};
use crate::events::canonical_path;
use crate::git;
use crate::state::AppState;

#[tauri::command]
pub async fn get_line_origins(
    state: State<'_, AppState>,
    worktree_id: WorktreeId,
    path: String,
) -> Result<LineOriginsResult, IpcError> {
    get_line_origins_impl(&state, &worktree_id, &path).await
}

pub async fn get_line_origins_impl(
    _state: &AppState,
    _worktree_id: &WorktreeId,
    path: &str,
) -> Result<LineOriginsResult, IpcError> {
    // The path comes from the webview. Two gates: the shared request check
    // refuses hostile input (absolute, `..`, NUL, empty; also `./x`, exactly
    // like `get_file_diff`, since the UI sends the paths `get_changes`
    // listed); `canonical_path` then gives the one spelling the edit index is
    // keyed by (`a//b.rs` and `a/b.rs` are the same file).
    git::diff::check_request_path(path).map_err(invalid)?;
    let _canonical = canonical_path(path)
        .ok_or_else(|| invalid(format!("{path:?} is not a path inside the worktree")))?;
    Ok(LineOriginsResult::Unavailable {
        reason: LineOriginsUnavailable::EngineNotReady,
    })
}

fn invalid(message: impl Into<String>) -> IpcError {
    IpcError::InvalidInput {
        message: message.into(),
    }
}
