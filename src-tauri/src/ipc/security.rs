//! The filter-driver guard at the IPC layer (WHY-39; threat model in
//! `git/filters.rs` and `SECURITY.md`).
//!
//! * [`permit_for`]: what every working-tree read calls first. It is the ONE
//!   bridge between "what the user trusted" (the store) and the git layer's
//!   `ReadPermit`, so there is exactly one place that decides.
//! * [`get_repository_security`], [`trust_repository_filters`],
//!   [`revoke_repository_filters`]: let the UI show the drivers a repository
//!   configures and record the user's decision.
//!
//! Trust is a *per-repository* decision, but config can differ per worktree
//! (`config.worktree`), so the repository-level views scan every worktree and
//! take the union. The guard itself checks the single worktree about to be read.

use std::collections::HashSet;
use std::path::Path;

use futures_util::future::join_all;
use tauri::State;

use crate::contracts::{
    FilterDriverInfo, FilterGuardStatus, FilterScope, IpcError, RepoId, RepositorySecurity,
    RevokeResult,
};
use crate::git::{self, ConfigScope, FilterDriver, ReadPermit};
use crate::ipc::changes::repo_info;
use crate::state::AppState;
use crate::store::RepoRecord;

/// Never read more than this much of an attributes file: it is untrusted input
/// and only used for a best-effort display hint.
const ATTRIBUTES_READ_CAP: u64 = 256 * 1024;

/// Permission to read `worktree`'s files with git, or the reason it is refused.
///
/// Scans the worktree's filter drivers *now* and checks them against the
/// repository's trusted fingerprints. Call it immediately before the git
/// command it guards: the longer the gap, the longer a driver written in
/// between goes unseen.
pub(crate) async fn permit_for(
    state: &AppState,
    repo: &RepoId,
    worktree: &Path,
) -> Result<ReadPermit, IpcError> {
    let trusted = state.repos.trusted_filters(repo)?;
    Ok(git::guard_working_tree(&state.git, worktree, &trusted).await?)
}

/// A scanned driver plus the best-effort "is it referenced" hint.
struct Found {
    driver: FilterDriver,
    referenced: bool,
}

/// Every driver configured in any worktree of `repo`, deduplicated by exact
/// configuration. A failure to scan ANY worktree is an error (fail closed): an
/// unreadable worktree must not make the repository look clean.
async fn scan_repository(state: &AppState, repo: &RepoRecord) -> Result<Vec<Found>, IpcError> {
    let entries = git::list_worktrees(&state.git, &repo_info(repo)).await?;
    let mut roots: Vec<_> = entries
        .into_iter()
        .filter(|e| !e.is_bare && e.prunable.is_none())
        .map(|e| e.path)
        .collect();
    // Dropping deleted directories keeps a stale worktree from turning into an
    // error; fall back to the registered root so we always scan something.
    let mut existing = Vec::new();
    for root in roots.drain(..) {
        if tokio::fs::try_exists(&root).await.unwrap_or(false) {
            existing.push(root);
        }
    }
    if existing.is_empty() {
        existing.push(repo.root_path.clone());
    }

    let common_dir = repo.common_dir.clone();
    let scans = existing.iter().map(|root| async {
        let drivers = git::scan_filter_drivers(&state.git, root).await?;
        // Both files are repository-controlled; see `read_attributes`.
        let mut attrs = read_attributes(root, Path::new(".gitattributes")).await;
        attrs.push('\n');
        attrs.push_str(&read_attributes(&common_dir, Path::new("info/attributes")).await);
        Ok::<_, IpcError>(
            drivers
                .into_iter()
                .map(|driver| {
                    let referenced = git::attributes_reference(&attrs, &driver.name);
                    Found { driver, referenced }
                })
                .collect::<Vec<_>>(),
        )
    });

    let mut out: Vec<Found> = Vec::new();
    for scanned in join_all(scans).await {
        for found in scanned? {
            match out
                .iter_mut()
                .find(|f| f.driver.fingerprint() == found.driver.fingerprint())
            {
                Some(existing) => existing.referenced |= found.referenced,
                None => out.push(found),
            }
        }
    }
    out.sort_by(|a, b| a.driver.name.cmp(&b.driver.name));
    Ok(out)
}

/// The text of `base/rel` for the "is this driver referenced" hint, or empty.
///
/// `base/rel` is controlled by the repository, and the hint is optional, so the
/// read is as defensive as `get_file_diff`'s: [`open_in_worktree`] canonicalises
/// the parent and requires it inside `base`, refuses to follow a symlink (it
/// could point at `/etc/shadow`, whose text we would then scan) and treats a
/// FIFO or device as "not a file" instead of opening it (opening a FIFO with no
/// writer blocks forever, which would hang the security report and the trust
/// request). Anything but a regular file is simply "no hint". The read itself
/// is capped at [`ATTRIBUTES_READ_CAP`] bytes.
async fn read_attributes(base: &Path, rel: &Path) -> String {
    use tokio::io::AsyncReadExt;
    let Ok(canonical) = tokio::fs::canonicalize(base).await else {
        return String::new();
    };
    let Ok(git::diff::Opened::File { file, .. }) =
        git::diff::open_in_worktree(&canonical, rel).await
    else {
        return String::new();
    };
    let mut buf = Vec::new();
    if file
        .take(ATTRIBUTES_READ_CAP)
        .read_to_end(&mut buf)
        .await
        .is_err()
    {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Build the contract view of a scan under a given set of trusted fingerprints.
fn assemble(repo: &RepoId, found: &[Found], trusted: &HashSet<String>) -> RepositorySecurity {
    let needing: Vec<&FilterDriver> = found
        .iter()
        .map(|f| &f.driver)
        .filter(|d| d.needs_trust())
        .collect();
    let all_trusted = needing.iter().all(|d| trusted.contains(&d.fingerprint()));
    let status = if found.is_empty() {
        FilterGuardStatus::Clear
    } else if needing.is_empty() {
        FilterGuardStatus::Info
    } else if all_trusted {
        FilterGuardStatus::Trusted
    } else {
        FilterGuardStatus::Blocked
    };
    RepositorySecurity {
        repo_id: repo.clone(),
        status,
        fingerprint: git::set_fingerprint(needing.iter().copied()),
        drivers: found
            .iter()
            .map(|f| driver_info(&f.driver, trusted, f.referenced))
            .collect(),
    }
}

/// The contract view of one driver. Also used by `ipc/error.rs` to describe the
/// drivers that blocked a read.
pub(crate) fn driver_info(
    driver: &FilterDriver,
    trusted: &HashSet<String>,
    referenced: bool,
) -> FilterDriverInfo {
    let needs_trust = driver.needs_trust();
    // The setting that explains the state: the repository-controlled program for
    // a driver that needs trust, else whichever program the driver runs.
    let deciding = driver
        .untrusted_setting()
        .or(driver.process.as_ref())
        .or(driver.clean.as_ref())
        .or(driver.smudge.as_ref());
    FilterDriverInfo {
        name: driver.name.clone(),
        clean: driver.clean.as_ref().map(|s| s.value.clone()),
        smudge: driver.smudge.as_ref().map(|s| s.value.clone()),
        process: driver.process.as_ref().map(|s| s.value.clone()),
        scope: deciding.map_or(FilterScope::Unknown, |s| scope_info(&s.scope)),
        origin: deciding.map(|s| s.origin.clone()).unwrap_or_default(),
        needs_trust,
        trusted: needs_trust && trusted.contains(&driver.fingerprint()),
        referenced,
        fingerprint: driver.fingerprint(),
    }
}

fn scope_info(scope: &ConfigScope) -> FilterScope {
    match scope {
        ConfigScope::System => FilterScope::System,
        ConfigScope::Global => FilterScope::Global,
        ConfigScope::Local => FilterScope::Local,
        ConfigScope::Worktree => FilterScope::Worktree,
        ConfigScope::Command => FilterScope::Command,
        ConfigScope::Other(_) => FilterScope::Unknown,
    }
}

fn registered(state: &AppState, id: &RepoId) -> Result<RepoRecord, IpcError> {
    state.repos.get(id)?.ok_or(IpcError::NotFound)
}

/// The filter drivers a repository configures and whether WHY may read it.
#[tauri::command]
pub async fn get_repository_security(
    state: State<'_, AppState>,
    repo_id: RepoId,
) -> Result<RepositorySecurity, IpcError> {
    get_repository_security_impl(&state, &repo_id).await
}

pub async fn get_repository_security_impl(
    state: &AppState,
    repo_id: &RepoId,
) -> Result<RepositorySecurity, IpcError> {
    let repo = registered(state, repo_id)?;
    let found = scan_repository(state, &repo).await?;
    let trusted = state.repos.trusted_filters(repo_id)?;
    Ok(assemble(repo_id, &found, &trusted))
}

/// Trust the drivers the user was shown.
///
/// `fingerprint` is the value from [`RepositorySecurity`] the user saw. The core
/// scans again and trusts what it finds *now*, only if that is still exactly
/// what was shown (`InvalidInput` otherwise): the UI never gets to name what is
/// trusted, so neither a stale screen nor a compromised page can trust a driver
/// nobody reviewed.
#[tauri::command]
pub async fn trust_repository_filters(
    state: State<'_, AppState>,
    repo_id: RepoId,
    fingerprint: String,
) -> Result<RepositorySecurity, IpcError> {
    trust_repository_filters_impl(&state, &repo_id, &fingerprint).await
}

pub async fn trust_repository_filters_impl(
    state: &AppState,
    repo_id: &RepoId,
    fingerprint: &str,
) -> Result<RepositorySecurity, IpcError> {
    let repo = registered(state, repo_id)?;
    let found = scan_repository(state, &repo).await?;
    let needing: Vec<&FilterDriver> = found
        .iter()
        .map(|f| &f.driver)
        .filter(|d| d.needs_trust())
        .collect();
    if needing.is_empty() {
        return Err(IpcError::InvalidInput {
            message: "this repository has no filter programs to trust".to_string(),
        });
    }
    if git::set_fingerprint(needing.iter().copied()) != fingerprint {
        return Err(IpcError::InvalidInput {
            message: "the repository's filter configuration changed; review it again".to_string(),
        });
    }
    let fingerprints: Vec<String> = needing.iter().map(|d| d.fingerprint()).collect();
    // Conditional on the registration still existing (see the trait docs): a
    // repository removed while we were scanning must not get trust written back.
    if !state.repos.set_trusted_filters(repo_id, &fingerprints)? {
        return Err(IpcError::NotFound);
    }
    tracing::info!(repo = %repo.root_path.display(), drivers = fingerprints.len(), "user trusted filter drivers");
    Ok(assemble(
        repo_id,
        &found,
        &fingerprints.into_iter().collect(),
    ))
}

/// Forget the trust decision: the repository is blocked again if it still
/// configures filter drivers.
///
/// `Ok` means trust IS forgotten. The rescan that builds the fresh report is a
/// separate step whose failure is reported inside the result, not as an error:
/// returning `Err` after the write succeeded would tell the UI the revocation
/// failed while the store already says otherwise.
#[tauri::command]
pub async fn revoke_repository_filters(
    state: State<'_, AppState>,
    repo_id: RepoId,
) -> Result<RevokeResult, IpcError> {
    revoke_repository_filters_impl(&state, &repo_id).await
}

pub async fn revoke_repository_filters_impl(
    state: &AppState,
    repo_id: &RepoId,
) -> Result<RevokeResult, IpcError> {
    let repo = registered(state, repo_id)?;
    if !state.repos.set_trusted_filters(repo_id, &[])? {
        return Err(IpcError::NotFound);
    }
    Ok(match scan_repository(state, &repo).await {
        Ok(found) => RevokeResult {
            security: Some(assemble(repo_id, &found, &HashSet::new())),
            rescan_error: None,
        },
        Err(error) => {
            tracing::warn!(%error, "trust was revoked, but the repository could not be rescanned");
            RevokeResult {
                security: None,
                rescan_error: Some(error),
            }
        }
    })
}
