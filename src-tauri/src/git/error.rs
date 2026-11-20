//! Error types for the git layer.
//!
//! Two layers on purpose:
//! * [`ParseError`]: the pure parsers (`&[u8]` in, value out) can only fail
//!   because git printed something we did not expect. No I/O involved.
//! * [`GitError`]: everything that can go wrong while *running* git, plus a
//!   wrapped `ParseError` for when the output of a successful run is malformed.

use std::path::PathBuf;
use std::time::Duration;

use super::filters::FilterDriver;
use super::version::GitVersion;

/// Git printed output that does not match the documented format.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(pub String);

impl ParseError {
    pub fn new(msg: impl Into<String>) -> Self {
        ParseError(msg.into())
    }
}

/// Anything that can go wrong while running a git command.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The `git` executable could not be started because it does not exist.
    ///
    /// The runner never sets a working directory on the child (it passes `-C`
    /// instead), so a `NotFound` from `spawn` can only mean the program itself
    /// is missing, never "the directory is missing".
    #[error("git executable `{program}` not found (is git installed and on PATH?)")]
    GitNotFound { program: String },

    /// Git is too old to reliably disable lazy fetching.
    #[error("Git {installed} is installed. WHY requires Git {minimum} or newer.")]
    UnsupportedVersion {
        installed: GitVersion,
        minimum: GitVersion,
    },

    /// `git` ran but the directory is not inside a repository.
    #[error("not a git repository: {}", path.display())]
    NotARepository { path: PathBuf },

    /// `git` ran and exited with a non-zero status. Callers that treat a
    /// specific code as a normal answer (e.g. `merge-base` exits 1 for "no
    /// common ancestor") look at `code`.
    #[error("`git {args}` failed ({}): {stderr}", code.map_or("killed by signal".to_string(), |c| format!("exit {c}")))]
    NonZeroExit {
        /// Space-joined (lossy) arguments, for the message only.
        args: String,
        /// `None` when the process was terminated by a signal.
        code: Option<i32>,
        /// First ~1 KiB of stderr, trimmed.
        stderr: String,
    },

    /// The command ran longer than the configured timeout and was killed.
    #[error("`git {args}` timed out after {timeout:?}")]
    Timeout { args: String, timeout: Duration },

    /// Any other I/O failure around the child process.
    #[error("I/O error while running git: {0}")]
    Io(#[from] std::io::Error),

    /// WHY refused to read the working tree because the repository configures
    /// filter drivers (programs git would run while reading files) that the
    /// user has not trusted. Nothing was run. See `git::filters`.
    #[error("untrusted git filter drivers: {}", drivers.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", "))]
    UntrustedFilters { drivers: Vec<FilterDriver> },

    /// Git succeeded but its output was not in the expected format.
    #[error("unexpected git output: {0}")]
    Parse(#[from] ParseError),
}

impl GitError {
    /// `true` for a non-zero exit with exactly this code.
    pub fn is_exit_code(&self, expected: i32) -> bool {
        matches!(self, GitError::NonZeroExit { code: Some(c), .. } if *c == expected)
    }
}
