//! Commands that manage the set of registered repositories.
//!
//! Owner after WHY-40: WHY-6 (SQLite store + watcher). `list_repositories`,
//! `add_repository` and `remove_repository` are real and only use the
//! [`RepoStore`](crate::store::RepoStore) trait, so they keep working when the
//! store is swapped. `set_base` is a stub for WHY-6 to fill in.

use std::path::PathBuf;

use tauri::State;

use crate::contracts::{IpcError, RepoId, Repository, Worktree, WorktreeId};
use crate::git;
use crate::ipc::changes;
use crate::state::AppState;
use crate::store::RepoRecord;

/// All registered repositories, ordered by name.
#[tauri::command]
pub async fn list_repositories(state: State<'_, AppState>) -> Result<Vec<Repository>, IpcError> {
    list_repositories_impl(&state).await
}

pub async fn list_repositories_impl(state: &AppState) -> Result<Vec<Repository>, IpcError> {
    let records = state.repos.list()?;
    Ok(records.iter().map(RepoRecord::to_contract).collect())
}

/// Register the repository that contains `path`. Adding a repository that is
/// already registered (even through another of its worktrees) returns the
/// existing registration.
#[tauri::command]
pub async fn add_repository(
    state: State<'_, AppState>,
    path: String,
) -> Result<Repository, IpcError> {
    add_repository_impl(&state, &path).await
}

pub async fn add_repository_impl(state: &AppState, path: &str) -> Result<Repository, IpcError> {
    if path.trim().is_empty() {
        return Err(invalid("path is empty"));
    }
    // Canonicalize first: resolves symlinks and relative paths, and gives a clear
    // error for a path that does not exist (git would only say "not a repository").
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|e| invalid(format!("cannot open {path:?}: {e}")))?;
    if !tokio::fs::metadata(&canonical)
        .await
        .map_err(|e| invalid(format!("cannot open {path:?}: {e}")))?
        .is_dir()
    {
        return Err(invalid(format!("{path:?} is not a directory")));
    }

    // GitError -> IpcError happens in the `?` (see `ipc/error.rs`).
    let info = git::open_repo(&state.git, &canonical).await?;
    if info.is_bare {
        // A bare repository has no working tree, so there is nothing to review.
        return Err(invalid("bare repositories are not supported"));
    }

    let root_path = resolve_root(&state.git, &info).await?;
    let name = root_path
        .file_name()
        // A root path like `/` has no final component; fall back to the whole path.
        .unwrap_or(root_path.as_os_str())
        .to_string_lossy()
        .into_owned();
    let record = RepoRecord {
        id: RepoId(git::repo_id(&info.common_dir)),
        name,
        root_path,
        common_dir: info.common_dir,
    };

    // Idempotent: if the id is already stored we get the existing record back.
    Ok(state.repos.insert(record)?.into_record().to_contract())
}

/// The checkout a repository is registered under: its main worktree, so that
/// adding it through any of its worktrees gives the same name and root.
///
/// `info.toplevel` is the root of the checkout the user pointed at, which is
/// already the main worktree when `git_dir == common_dir`. Only for a linked
/// worktree do we have to ask git where the main one is, and that answer is
/// verified rather than trusted: for a submodule or a `--separate-git-dir`
/// checkout git derives the main entry of `worktree list` from the git directory,
/// so it can name the metadata directory instead of a checkout. If the answer
/// does not check out, fall back to the checkout the user gave us.
async fn resolve_root(runner: &git::GitRunner, info: &git::RepoInfo) -> Result<PathBuf, IpcError> {
    // `Some` because bare repositories were rejected by the caller.
    let given = info
        .toplevel
        .clone()
        .ok_or_else(|| invalid("repository has no working tree"))?;
    if info.git_dir == info.common_dir {
        return Ok(given);
    }

    match main_checkout(runner, info).await {
        Some(main) => Ok(main),
        None => {
            tracing::warn!(
                path = %given.display(),
                "main worktree could not be verified; registering the given checkout instead"
            );
            Ok(given)
        }
    }
}

/// The main worktree of the repository described by `info`, if git reports one
/// that is a real checkout of this same repository; `None` otherwise.
async fn main_checkout(runner: &git::GitRunner, info: &git::RepoInfo) -> Option<PathBuf> {
    let main = git::list_worktrees(runner, info)
        .await
        .ok()?
        .into_iter()
        .find(|w| w.is_main && !w.is_bare)?
        .path;
    // Same repository, and `main` really is the top of a working tree.
    let found = git::open_repo(runner, &main).await.ok()?;
    (found.common_dir == info.common_dir && found.toplevel.as_deref() == Some(main.as_path()))
        .then_some(main)
}

/// Unregister a repository. Never touches the repository on disk.
#[tauri::command]
pub async fn remove_repository(state: State<'_, AppState>, id: RepoId) -> Result<(), IpcError> {
    remove_repository_impl(&state, &id).await
}

pub async fn remove_repository_impl(state: &AppState, id: &RepoId) -> Result<(), IpcError> {
    if state.repos.remove(id)? {
        Ok(())
    } else {
        Err(IpcError::NotFound)
    }
}

/// Choose the base ref that `DiffMode::VsBase` compares against.
///
/// An empty string clears the override (back to auto-detection). Anything else
/// must be a ref (or commit id) that
///
/// 1. resolves to a commit, and
/// 2. is *usable* as a base: not the worktree's own branch or `HEAD`, and
///    sharing history with `HEAD`. `detect_base` silently ignores an override
///    that fails this, which would make `set_base` look successful while the UI
///    kept showing the old base; so it is refused here with `InvalidInput`.
///
/// Returns the refreshed [`Worktree`] (new base, ahead/behind, change count).
#[tauri::command]
pub async fn set_base(
    state: State<'_, AppState>,
    worktree_id: WorktreeId,
    base_ref: String,
) -> Result<Worktree, IpcError> {
    set_base_impl(&state, &worktree_id, &base_ref).await
}

pub async fn set_base_impl(
    state: &AppState,
    worktree_id: &WorktreeId,
    base_ref: &str,
) -> Result<Worktree, IpcError> {
    let wt = changes::resolve_worktree(state, worktree_id).await?;
    let wanted = base_ref.trim();

    // Validate, then build the refreshed `Worktree` *with the candidate* (not
    // stored yet), and persist only after that worked: a failure anywhere leaves
    // the stored override exactly as it was.
    let candidate = if wanted.is_empty() {
        None
    } else {
        if wanted.contains('\0') {
            return Err(invalid("base ref contains a NUL byte"));
        }
        if !tokio::fs::try_exists(&wt.entry.path).await.unwrap_or(false) {
            // The directory is gone (prunable worktree): nothing to compare.
            return Err(IpcError::NotFound);
        }

        // 1. Does it name a commit? `^{commit}` peels tags to the commit they
        //    point at and rejects trees and blobs. `--quiet --verify` makes
        //    "no" a silent exit code 1 instead of a fatal error. The ref is
        //    user input, so `--end-of-options` stops it being read as a flag.
        let spec = format!("{wanted}^{{commit}}");
        match state
            .git
            .run(
                &wt.entry.path,
                [
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "--end-of-options",
                    spec.as_str(),
                ],
            )
            .await
        {
            Ok(_) => {}
            Err(e) if e.is_exit_code(1) => {
                return Err(invalid(format!("`{wanted}` is not a commit")));
            }
            Err(e) => return Err(e.into()),
        }

        // 2. Would `detect_base` really use it? When it does, it reports the
        //    override under exactly the name we passed.
        let usable = git::detect_base(&state.git, &wt.entry.path, Some(wanted)).await?;
        if usable.is_none_or(|b| b.base_ref != wanted) {
            return Err(invalid(format!(
                "`{wanted}` cannot be the base: it is this worktree's own branch, \
                 shares no history with HEAD, or HEAD has no commits yet"
            )));
        }
        Some(wanted.to_string())
    };

    let view = changes::build_worktree(
        state,
        &wt,
        changes::BaseChoice::Candidate(candidate.clone()),
    )
    .await?;
    state.repos.set_base_override(&wt.id, candidate)?;
    Ok(view)
}

fn invalid(message: impl Into<String>) -> IpcError {
    IpcError::InvalidInput {
        message: message.into(),
    }
}
