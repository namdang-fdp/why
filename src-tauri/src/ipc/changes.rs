//! Commands that read a repository's worktrees and changes.
//!
//! Owner after WHY-40: WHY-5 (diff service). Every command below is a stub that
//! keeps its final signature; replace the body of each `*_impl` (and drop the
//! leading underscores from the parameters you use). The command wrappers
//! should not need to change.

use tauri::State;

use crate::contracts::{DiffMode, FileChange, FileDiff, IpcError, RepoId, Worktree, WorktreeId};
use crate::state::AppState;

fn not_implemented() -> IpcError {
    IpcError::InvalidInput {
        message: "not implemented yet (WHY-5)".to_string(),
    }
}

/// Worktrees of one registered repository.
#[tauri::command]
pub async fn list_worktrees(
    state: State<'_, AppState>,
    repo_id: RepoId,
) -> Result<Vec<Worktree>, IpcError> {
    list_worktrees_impl(&state, &repo_id).await
}

pub async fn list_worktrees_impl(
    _state: &AppState,
    _repo_id: &RepoId,
) -> Result<Vec<Worktree>, IpcError> {
    Err(not_implemented())
}

/// The changed-files list of one worktree.
#[tauri::command]
pub async fn get_changes(
    state: State<'_, AppState>,
    worktree_id: WorktreeId,
    mode: DiffMode,
) -> Result<Vec<FileChange>, IpcError> {
    get_changes_impl(&state, &worktree_id, mode).await
}

pub async fn get_changes_impl(
    _state: &AppState,
    _worktree_id: &WorktreeId,
    _mode: DiffMode,
) -> Result<Vec<FileChange>, IpcError> {
    Err(not_implemented())
}

/// Both sides of one file's diff.
#[tauri::command]
pub async fn get_file_diff(
    state: State<'_, AppState>,
    worktree_id: WorktreeId,
    path: String,
    mode: DiffMode,
) -> Result<FileDiff, IpcError> {
    get_file_diff_impl(&state, &worktree_id, &path, mode).await
}

pub async fn get_file_diff_impl(
    _state: &AppState,
    _worktree_id: &WorktreeId,
    _path: &str,
    _mode: DiffMode,
) -> Result<FileDiff, IpcError> {
    Err(not_implemented())
}
