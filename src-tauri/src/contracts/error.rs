use serde::{Deserialize, Serialize};
use thiserror::Error;
#[cfg(test)]
use ts_rs::TS;

use super::security::FilterDriverInfo;

/// Error returned by every IPC command, shaped so the UI can `switch (err.kind)`.
///
/// Internally tagged: each value is an object with a `kind` discriminator, plus
/// extra fields for the variants that carry data. `thiserror` supplies `Display`
/// (and `std::error::Error`) for logs; the UI shows its own copy per `kind`.
#[derive(Serialize, Deserialize, Error, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum IpcError {
    /// The path is not inside a git repository.
    #[error("not a git repository")]
    NotARepo,
    /// The `git` executable could not be found or started.
    #[error("git executable not found")]
    GitMissing,
    /// git ran but exited with an error; `message` is its (trimmed) stderr.
    #[error("git failed: {message}")]
    GitFailed { message: String },
    /// A git invocation took too long and was killed.
    #[error("git command timed out")]
    Timeout,
    /// The repository, worktree or file id does not exist (any more).
    #[error("not found")]
    NotFound,
    /// The caller passed a value the core rejects, for example an empty ref name.
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    /// WHY did not read the working tree because the repository configures git
    /// filter drivers (programs git would run while reading files) that the user
    /// has not trusted. Nothing was run. The UI explains this and offers
    /// `trust_repository_filters`.
    #[error("the repository configures untrusted git filter programs")]
    UntrustedFilters { drivers: Vec<FilterDriverInfo> },
    /// A failure inside WHY itself (for example the storage backend), not caused
    /// by git or by the caller. `message` is short and generic; the details are in
    /// the core's log.
    #[error("internal error: {message}")]
    Internal { message: String },
}
