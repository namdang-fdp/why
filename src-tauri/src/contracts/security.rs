use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

use super::error::IpcError;
use super::ids::RepoId;

/// Which git config file layer a setting came from. Tells the user (and the
/// guard) whether the *user* or the *repository* wrote it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum FilterScope {
    System,
    Global,
    /// The repository's own `.git/config` (and files it includes).
    Local,
    /// A linked worktree's `config.worktree`.
    Worktree,
    /// The environment WHY was started in.
    Command,
    /// A scope this version does not recognise; treated like `local`.
    Unknown,
}

/// A git *filter driver* configured for a repository: a program git runs while
/// reading files (see `SECURITY.md`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct FilterDriverInfo {
    /// The driver's name, as `.gitattributes` refers to it (`filter=<name>`).
    pub name: String,
    /// The commands, exactly as configured. `None` = not set.
    pub clean: Option<String>,
    pub smudge: Option<String>,
    pub process: Option<String>,
    /// Scope of the setting that decides trust: the repository-controlled
    /// `clean`/`process` for a driver that needs trust, else the scope of its
    /// program.
    pub scope: FilterScope,
    /// Config file that setting came from (for display, e.g. `.git/config`).
    pub origin: String,
    /// A program the repository controls would run when WHY reads files: WHY
    /// refuses to read until the user trusts it. `false` = the user's own
    /// driver (typically git-lfs), allowed.
    pub needs_trust: bool,
    /// The user trusted exactly this configuration (`needs_trust` drivers only).
    pub trusted: bool,
    /// Found in the root `.gitattributes` or `.git/info/attributes`. Best effort:
    /// attribute files in subdirectories are not checked, so `false` does not
    /// mean unused.
    pub referenced: bool,
    /// Stable identity of this exact configuration.
    pub fingerprint: String,
}

/// The overall state of a repository's filter drivers.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum FilterGuardStatus {
    /// No filter drivers at all.
    Clear,
    /// Only the user's own drivers (global/system config, e.g. git-lfs).
    Info,
    /// The repository configures a driver the user has not trusted: WHY does
    /// not read its files.
    Blocked,
    /// The repository configures drivers and the user trusted them.
    Trusted,
}

/// What `get_repository_security` reports: everything the UI needs to explain
/// the state and to ask for trust.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct RepositorySecurity {
    pub repo_id: RepoId,
    pub status: FilterGuardStatus,
    /// Identity of the drivers that need trust, across all worktrees; empty
    /// when none do. `trust_repository_filters` must be given the value the user
    /// was shown, so a configuration that changed in between is not trusted blind.
    pub fingerprint: String,
    /// Every driver found in any worktree of the repository, by name.
    pub drivers: Vec<FilterDriverInfo>,
}

/// What `revoke_repository_filters` reports. Revoking is a store write that has
/// either happened or not; the rescan that follows only refreshes the report,
/// and can fail on its own (a worktree git cannot read). So the two are kept
/// apart: `Ok(RevokeResult)` always means "trust was forgotten", and the UI
/// must treat whatever it read under the old decision as stale either way.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct RevokeResult {
    /// The fresh report; `None` when the rescan failed (see `rescan_error`).
    pub security: Option<RepositorySecurity>,
    /// Why the rescan failed; `None` when it worked.
    pub rescan_error: Option<IpcError>,
}
