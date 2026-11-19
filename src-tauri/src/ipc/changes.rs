//! Commands that read a repository's worktrees and changes (the diff service).
//!
//! Each command is a thin `#[tauri::command]` wrapper over a plain `*_impl`
//! function that tests call directly. The git work lives in [`crate::git`]
//! (`diff.rs`, `status.rs`, `base.rs`); this file does what only the IPC layer
//! can: find the worktree an id refers to, decide *which* comparison a
//! [`DiffMode`] means, enforce the rules for input coming from the webview, and
//! map git-level types to the contracts.

use std::io;
use std::path::Path;

use futures_util::future::join_all;
use tauri::State;

use crate::contracts::{
    BranchInfo, ChangeStatus, DiffMode, FileChange, FileDiff, IpcError, RepoId, Worktree,
    WorktreeId,
};
use crate::git::{
    self, BaseInfo, Change, ChangeKind, ChangeSet, GitError, OldSide, Side, Target, WorktreeEntry,
};
use crate::state::AppState;
use crate::store::RepoRecord;

/// A worktree id resolved to the things git needs.
#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    pub id: WorktreeId,
    pub repo: RepoRecord,
    pub entry: WorktreeEntry,
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
    state: &AppState,
    repo_id: &RepoId,
) -> Result<Vec<Worktree>, IpcError> {
    let repo = state.repos.get(repo_id)?.ok_or(IpcError::NotFound)?;
    let entries = git::list_worktrees(&state.git, &repo_info(&repo)).await?;

    // One future per worktree, all polled together. This is concurrency, not
    // parallelism: the futures mostly wait for child `git` processes. The cap
    // on how many run at once is the runner's semaphore, which every command
    // shares, so ten worktrees do not mean ten times the load.
    // (A bare repository's own entry has no files to review: skip it.)
    let views = join_all(
        entries
            .into_iter()
            .filter(|e| !e.is_bare)
            .map(|entry| async {
                let resolved = resolved_from(&repo, entry);
                build_worktree_lenient(state, &resolved).await
            }),
    )
    .await;
    views.into_iter().collect()
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
    state: &AppState,
    worktree_id: &WorktreeId,
    mode: DiffMode,
) -> Result<Vec<FileChange>, IpcError> {
    let wt = resolve_worktree(state, worktree_id).await?;
    require_checkout(&wt).await?;
    let mut set = changes_for_mode(state, &wt, mode).await?;
    // The enriched step: line counts for untracked files. `list_worktrees`
    // skips it (it only needs the number of entries), so a directory full of
    // untracked files cannot slow the rail down.
    git::count_untracked_lines(&wt.entry.path, &mut set.changes).await;
    Ok(set.changes.iter().map(to_file_change).collect())
}

/// Both sides of one file's diff.
///
/// # The `path` argument is untrusted
///
/// It comes from the webview. It is used **only as a lookup key** into the
/// change list the core computes itself; every filesystem path and git object
/// name below is built from the list's own bytes. Three gates, in order:
///
/// 1. [`git::diff::check_request_path`] refuses absolute paths, `..`, NUL bytes
///    (`InvalidInput`).
/// 2. The path must be in the current change list for this worktree and mode,
///    either as a file's `path` or as the `old_path` of a rename (`NotFound`).
///    So `../../etc/passwd` or a file that merely exists on disk is never read.
/// 3. Reading from disk canonicalizes the parent directory and checks it is
///    inside the worktree, and never follows a final symlink
///    ([`git::diff::read_worktree_side`]).
///
/// A request that matches only an `old_path` returns the rename's diff (with
/// `path` set to the new name).
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
    state: &AppState,
    worktree_id: &WorktreeId,
    path: &str,
    mode: DiffMode,
) -> Result<FileDiff, IpcError> {
    git::diff::check_request_path(path).map_err(invalid)?;
    let wt = resolve_worktree(state, worktree_id).await?;
    require_checkout(&wt).await?;
    let set = changes_for_mode(state, &wt, mode).await?;

    // Gate 2. Compare bytes: a lossy (non-UTF-8) entry can never equal a valid
    // `&str`, so such files are listed but not openable (see contracts docs).
    let wanted = path.as_bytes();
    let change = set
        .changes
        .iter()
        .find(|c| git::oid::path_bytes(&c.path) == wanted)
        .or_else(|| {
            set.changes.iter().find(|c| {
                c.old_path
                    .as_deref()
                    .is_some_and(|old| git::oid::path_bytes(old) == wanted)
            })
        })
        .ok_or(IpcError::NotFound)?;

    let root = &wt.entry.path;
    let canonical_root = tokio::fs::canonicalize(root).await.map_err(io_error)?;
    let (old, new) = if change.is_gitlink() {
        // A submodule's "content" is a commit id: nothing to show as text.
        (Ok(Side::NotAFile), Ok(Side::NotAFile))
    } else {
        tokio::join!(
            old_side(state, root, &set, change),
            new_side(state, root, &canonical_root, change, mode)
        )
    };
    let (old, new) = (old?, new?);

    // Any side that is not text hides both: half a diff would mislead.
    let is_binary = |s: &Side| matches!(s, Side::Binary | Side::NotAFile);
    let binary = change.binary || is_binary(&old) || is_binary(&new);
    let too_large = !binary && (old == Side::TooLarge || new == Side::TooLarge);
    let content = |side: Side| match side {
        Side::Text(text) if !binary && !too_large => Some(text),
        _ => None,
    };

    Ok(FileDiff {
        path: change.path.to_string_lossy().into_owned(),
        old_path: change
            .old_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        status: to_status(change.kind),
        old_content: content(old),
        new_content: content(new),
        binary,
        too_large,
        path_lossy: path_is_lossy(change),
    })
}

/// The old side: the blob the diff compared against.
///
/// Read by object id (taken from `git diff`'s own output), so the size check and
/// the read cannot disagree and the path is never parsed as revision syntax. A
/// conflicted file has no blob id; its old content is looked up in the old
/// commit by path and may legitimately be absent.
async fn old_side(
    state: &AppState,
    root: &Path,
    set: &ChangeSet,
    change: &Change,
) -> Result<Side, IpcError> {
    match change.kind {
        // Did not exist before.
        ChangeKind::Added | ChangeKind::Untracked => Ok(Side::Absent),
        _ => {
            let (spec, absent_ok) = match change.old_oid.as_deref().and_then(git::diff::oid_spec) {
                Some(spec) => (spec, false),
                None => {
                    let name = change.old_path.as_deref().unwrap_or(&change.path);
                    (git::diff::commit_spec(&set.old_rev, name), true)
                }
            };
            git::diff::read_blob_side(&state.git, root, spec, absent_ok)
                .await
                .map_err(blob_error)
        }
    }
}

/// The new side: the staged blob for `Staged`, the file on disk otherwise.
async fn new_side(
    state: &AppState,
    root: &Path,
    canonical_root: &Path,
    change: &Change,
    mode: DiffMode,
) -> Result<Side, IpcError> {
    if change.kind == ChangeKind::Deleted {
        return Ok(Side::Absent);
    }
    match mode {
        // The index holds the version `git commit` would record, which can
        // differ from the file on disk (edited after `git add`). Its blob id
        // comes from `git diff --cached`; a conflicted file has no stage-0 blob.
        DiffMode::Staged => match change.new_oid.as_deref().and_then(git::diff::oid_spec) {
            Some(spec) => git::diff::read_blob_side(&state.git, root, spec, false)
                .await
                .map_err(blob_error),
            None => Ok(Side::Absent),
        },
        DiffMode::VsBase | DiffMode::Uncommitted => {
            git::diff::read_worktree_side(canonical_root, &change.path)
                .await
                .map_err(io_error)
        }
    }
}

/// Reading a blob by id failed with git's "no such object" (exit 128). Besides
/// corruption, the usual cause is a partial clone: the object was never
/// downloaded, and the runner forbids lazy fetching (`GIT_NO_LAZY_FETCH`), so
/// WHY reports it instead of contacting a remote.
fn blob_error(err: GitError) -> IpcError {
    if err.is_exit_code(128) {
        tracing::warn!(error = %err, "blob is not in the local object database");
        return IpcError::GitFailed {
            message: "object not available locally (partial clone?)".to_string(),
        };
    }
    err.into()
}

// ---------------------------------------------------------------------------
// Resolving worktrees
// ---------------------------------------------------------------------------

/// Find the worktree an id refers to by listing the worktrees of every
/// registered repository and comparing ids.
///
/// An id is a one-way hash (see `git::worktree_id`), so it cannot be turned
/// back into a path; searching is the only way. No cache yet: the cost is one
/// `git worktree list` per registered repository, run concurrently. A
/// repository that cannot be listed (deleted from disk, say) is logged and
/// skipped, so it cannot make worktrees of *other* repositories unreachable.
pub(crate) async fn resolve_worktree(
    state: &AppState,
    id: &WorktreeId,
) -> Result<Resolved, IpcError> {
    let repos = state.repos.list()?;
    let searches = repos.iter().map(|repo| async move {
        match git::list_worktrees(&state.git, &repo_info(repo)).await {
            Ok(entries) => entries
                .into_iter()
                .find(|e| !e.is_bare && git::worktree_id(&repo.common_dir, &e.path) == id.0)
                .map(|entry| resolved_from(repo, entry)),
            Err(e) => {
                tracing::warn!(repo = %repo.root_path.display(), error = %e, "cannot list worktrees while resolving an id");
                None
            }
        }
    });
    join_all(searches)
        .await
        .into_iter()
        .flatten()
        .next()
        .ok_or(IpcError::NotFound)
}

fn resolved_from(repo: &RepoRecord, entry: WorktreeEntry) -> Resolved {
    Resolved {
        id: WorktreeId(git::worktree_id(&repo.common_dir, &entry.path)),
        repo: repo.clone(),
        entry,
    }
}

/// `list_worktrees` needs a `RepoInfo` but only reads `common_dir`; the store
/// kept exactly that, so rebuild the rest of the struct from it.
fn repo_info(repo: &RepoRecord) -> git::RepoInfo {
    git::RepoInfo {
        toplevel: Some(repo.root_path.clone()),
        common_dir: repo.common_dir.clone(),
        git_dir: repo.common_dir.clone(),
        is_bare: false,
    }
}

/// The worktree's directory must exist before we run git in it. A prunable
/// worktree (directory deleted) is listed by `list_worktrees` but has no files.
async fn require_checkout(wt: &Resolved) -> Result<(), IpcError> {
    match tokio::fs::try_exists(&wt.entry.path).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(IpcError::NotFound),
        Err(e) => Err(io_error(e)),
    }
}

// ---------------------------------------------------------------------------
// Building the Worktree contract
// ---------------------------------------------------------------------------

/// Which base override a [`Worktree`] is built with.
#[derive(Debug, Clone)]
pub(crate) enum BaseChoice {
    /// The one in the store (what every normal read uses).
    Stored,
    /// A candidate that is *not stored yet* (`None` = "no override"). `set_base`
    /// builds the refreshed worktree with it first and persists it only if that
    /// worked, so a failure cannot leave a stored choice nobody saw succeed.
    Candidate(Option<String>),
}

/// Everything the UI shows for one worktree: flags, base and change count.
/// Strict: any failure is an error. Used by `set_base`.
pub(crate) async fn build_worktree(
    state: &AppState,
    wt: &Resolved,
    choice: BaseChoice,
) -> Result<Worktree, IpcError> {
    let entry = &wt.entry;
    if entry.prunable.is_some() || !dir_exists(&entry.path).await {
        // Directory gone: nothing to measure. Listed, with zero changes.
        return Ok(worktree_view(wt, None, 0));
    }
    let override_ref = match choice {
        BaseChoice::Stored => state.repos.base_override(&wt.id)?,
        BaseChoice::Candidate(candidate) => candidate,
    };
    let base = base_for(state, wt, override_ref).await?;
    // Same rule as `get_changes`: vs the merge-base, or uncommitted when there is
    // no base. Counted from the cheap list, so the number always equals
    // `get_changes(..).len()`.
    let old = match &base {
        Some(b) => OldSide::Commit(b.merge_base.clone()),
        None => OldSide::Head,
    };
    let target = Target::WorkingTree {
        include_untracked: true,
    };
    let set = git::read_changes(&state.git, &entry.path, &old, target).await?;
    let change_count = u32::try_from(set.changes.len()).unwrap_or(u32::MAX);
    Ok(worktree_view(wt, base, change_count))
}

/// [`build_worktree`] for the list: one broken worktree must not hide the
/// healthy ones. A worktree whose git commands fail (a corrupt `.git` file, a
/// timeout, a pathological file) is listed with `base: None` and
/// `change_count: 0`, and the error goes to the log.
// WHY-9: surface per-worktree error (needs a contract field; none yet, so the
// UI cannot tell "broken" from "clean" for such an entry).
async fn build_worktree_lenient(state: &AppState, wt: &Resolved) -> Result<Worktree, IpcError> {
    match build_worktree(state, wt, BaseChoice::Stored).await {
        // Our own storage failing is not a property of one worktree.
        Err(err @ IpcError::Internal { .. }) => Err(err),
        Err(err) => {
            tracing::warn!(
                worktree = %wt.entry.path.display(),
                error = %err,
                "cannot measure worktree; listing it without base or changes"
            );
            Ok(worktree_view(wt, None, 0))
        }
        Ok(view) => Ok(view),
    }
}

fn worktree_view(wt: &Resolved, base: Option<BaseInfo>, change_count: u32) -> Worktree {
    let entry = &wt.entry;
    Worktree {
        id: wt.id.clone(),
        repo_id: wt.repo.id.clone(),
        path: entry.path.to_string_lossy().into_owned(),
        branch: entry.branch_short().map(str::to_owned),
        head_sha: entry.head.as_ref().map(ToString::to_string),
        is_main: entry.is_main,
        is_detached: entry.is_detached,
        is_locked: entry.locked.is_some(),
        is_prunable: entry.prunable.is_some(),
        base: base.map(|b| BranchInfo {
            base_ref: b.base_ref,
            merge_base: Some(b.merge_base.to_string()),
            ahead: b.ahead,
            behind: b.behind,
        }),
        change_count,
    }
}

async fn dir_exists(path: &Path) -> bool {
    tokio::fs::try_exists(path).await.unwrap_or(false)
}

/// Detect the worktree's base, honouring the user's override from the store.
///
/// `detect_base` quietly ignores an override it cannot use (unknown ref, the
/// worktree's own branch, no shared history) and falls back to auto-detection;
/// that is the right behaviour for a *stored* choice that went stale (the user
/// deleted the branch), but it deserves a log line. We notice the fallback by
/// comparing the answer with what was asked for.
async fn base_for(
    state: &AppState,
    wt: &Resolved,
    override_ref: Option<String>,
) -> Result<Option<BaseInfo>, IpcError> {
    let found = git::detect_base(&state.git, &wt.entry.path, override_ref.as_deref()).await?;
    if let Some(wanted) = &override_ref
        && found.as_ref().map(|b| b.base_ref.as_str()) != Some(wanted.as_str())
    {
        tracing::warn!(
            worktree = %wt.entry.path.display(),
            base_override = %wanted,
            "base override is not usable; falling back to the detected base"
        );
    }
    Ok(found)
}

/// Run the comparison a [`DiffMode`] stands for.
async fn changes_for_mode(
    state: &AppState,
    wt: &Resolved,
    mode: DiffMode,
) -> Result<ChangeSet, IpcError> {
    let path = &wt.entry.path;
    let set = match mode {
        DiffMode::Staged => {
            git::read_changes(&state.git, path, &OldSide::Head, Target::Index).await?
        }
        DiffMode::Uncommitted => {
            let target = Target::WorkingTree {
                include_untracked: true,
            };
            git::read_changes(&state.git, path, &OldSide::Head, target).await?
        }
        DiffMode::VsBase => {
            let stored = state.repos.base_override(&wt.id)?;
            let base = base_for(state, wt, stored)
                .await?
                .ok_or_else(|| invalid("worktree has no base; use uncommitted"))?;
            let target = Target::WorkingTree {
                include_untracked: true,
            };
            let old = OldSide::Commit(base.merge_base);
            git::read_changes(&state.git, path, &old, target).await?
        }
    };
    Ok(set)
}

// ---------------------------------------------------------------------------
// Mapping to contracts and errors
// ---------------------------------------------------------------------------

fn to_file_change(c: &Change) -> FileChange {
    FileChange {
        path: c.path.to_string_lossy().into_owned(),
        old_path: c
            .old_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        status: to_status(c.kind),
        staged: c.staged,
        unstaged: c.unstaged,
        additions: c.additions,
        deletions: c.deletions,
        binary: c.binary,
        path_lossy: path_is_lossy(c),
    }
}

/// Did converting `path` or `old_path` to a `String` lose bytes?
fn path_is_lossy(c: &Change) -> bool {
    c.path.to_str().is_none() || c.old_path.as_deref().is_some_and(|p| p.to_str().is_none())
}

fn to_status(kind: ChangeKind) -> ChangeStatus {
    match kind {
        ChangeKind::Added => ChangeStatus::Added,
        ChangeKind::Modified => ChangeStatus::Modified,
        ChangeKind::Deleted => ChangeStatus::Deleted,
        ChangeKind::Renamed => ChangeStatus::Renamed,
        ChangeKind::Copied => ChangeStatus::Copied,
        ChangeKind::TypeChanged => ChangeStatus::TypeChanged,
        ChangeKind::Untracked => ChangeStatus::Untracked,
        ChangeKind::Conflicted => ChangeStatus::Conflicted,
    }
}

fn invalid(message: impl Into<String>) -> IpcError {
    IpcError::InvalidInput {
        message: message.into(),
    }
}

/// A file-system failure while reading a worktree file (not a git failure, so
/// it does not go through `ipc/error.rs`). The details are logged; the UI gets
/// a short, stable answer.
fn io_error(err: io::Error) -> IpcError {
    match err.kind() {
        io::ErrorKind::NotFound => IpcError::NotFound,
        // Our own refusals in `read_worktree_side` (not a plain relative path,
        // or it leaves the worktree).
        io::ErrorKind::InvalidInput => invalid("path is not a plain relative path"),
        io::ErrorKind::PermissionDenied if err.raw_os_error().is_none() => {
            invalid("path leaves the worktree")
        }
        _ => {
            tracing::warn!(error = %err, "cannot read a worktree file");
            IpcError::Internal {
                message: "could not read the file".to_string(),
            }
        }
    }
}
