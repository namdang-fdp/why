//! The ONLY place where internal errors become [`IpcError`]s.
//!
//! Commands return `Result<T, IpcError>`. Inside a command, `?` on a
//! `Result<_, GitError>` or `Result<_, StoreError>` converts the error through
//! the `From` impls below (that is what `?` does: it calls `From::from` on the
//! error). Keeping every mapping here means one file decides what the UI sees,
//! and the `match`es are exhaustive (no `_ =>` arm): adding a variant to
//! `GitError` or `StoreError` is a compile error until it is mapped here.
//!
//! Each conversion logs the full internal error first. The UI only receives the
//! short, stable form; the detail (arguments, paths, I/O causes) stays in the log.

use crate::contracts::IpcError;
use crate::git::GitError;
use crate::hooks::HookError;
use crate::ipc::security::driver_info;
use crate::store::StoreError;

impl From<GitError> for IpcError {
    fn from(err: GitError) -> Self {
        tracing::warn!(error = ?err, "git error: {err}");
        match err {
            GitError::GitNotFound { .. } => IpcError::GitMissing,
            GitError::UnsupportedVersion { .. } => IpcError::GitFailed {
                message: err.to_string(),
            },
            GitError::NotARepository { .. } => IpcError::NotARepo,
            GitError::NonZeroExit { args, code, stderr } => IpcError::GitFailed {
                // `stderr` is already trimmed and capped by the runner. An empty
                // one (git failing silently) would give the UI a blank message.
                message: if stderr.is_empty() {
                    match code {
                        Some(c) => format!("`git {args}` exited with status {c}"),
                        None => format!("`git {args}` was killed by a signal"),
                    }
                } else {
                    stderr
                },
            },
            // The guard refused to read: tell the UI which drivers blocked it.
            // (Nothing is trusted here by definition, and `referenced` is only
            // known to the full security report.)
            GitError::UntrustedFilters { drivers } => IpcError::UntrustedFilters {
                drivers: drivers
                    .iter()
                    .map(|d| driver_info(d, &Default::default(), false))
                    .collect(),
            },
            GitError::Timeout { .. } => IpcError::Timeout,
            GitError::Io(_) => IpcError::GitFailed {
                message: "I/O error while running git".to_string(),
            },
            GitError::Parse(_) => IpcError::GitFailed {
                message: "unexpected output from git".to_string(),
            },
        }
    }
}

impl From<StoreError> for IpcError {
    fn from(err: StoreError) -> Self {
        tracing::error!(error = ?err, "store error: {err}");
        match err {
            StoreError::Backend(_) => IpcError::Internal {
                message: "internal storage error".to_string(),
            },
        }
    }
}

impl From<HookError> for IpcError {
    fn from(err: HookError) -> Self {
        tracing::warn!(error = ?err, "hook installer error: {err}");
        match err {
            // User-fixable refusals. The installer normally turns these into
            // `Refused` values before they get here; this keeps `?` honest.
            HookError::NoHome
            | HookError::InvalidEnv(_)
            | HookError::ConfigInvalid(_)
            | HookError::DisabledByPolicy(_)
            | HookError::UnsafeTarget(_)
            | HookError::ReadOnly
            | HookError::Locked
            | HookError::TooLarge
            | HookError::HookBinaryMissing(_) => IpcError::InvalidInput {
                message: err.to_string(),
            },
            // The cause may name paths; it stays in the log.
            HookError::Io(_) => IpcError::Internal {
                message: "I/O error while updating the agent config".to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::git::ParseError;

    fn map(e: GitError) -> IpcError {
        e.into()
    }

    #[test]
    fn git_not_found_is_git_missing() {
        let e = GitError::GitNotFound {
            program: "git".into(),
        };
        assert_eq!(map(e), IpcError::GitMissing);
    }

    #[test]
    fn not_a_repository() {
        let e = GitError::NotARepository {
            path: PathBuf::from("/tmp/x"),
        };
        assert_eq!(map(e), IpcError::NotARepo);
    }

    #[test]
    fn non_zero_exit_forwards_stderr() {
        let e = GitError::NonZeroExit {
            args: "rev-parse HEAD".into(),
            code: Some(128),
            stderr: "fatal: bad revision".into(),
        };
        assert_eq!(
            map(e),
            IpcError::GitFailed {
                message: "fatal: bad revision".into()
            }
        );
    }

    #[test]
    fn non_zero_exit_with_empty_stderr_still_has_a_message() {
        let exit = GitError::NonZeroExit {
            args: "diff".into(),
            code: Some(1),
            stderr: String::new(),
        };
        assert_eq!(
            map(exit),
            IpcError::GitFailed {
                message: "`git diff` exited with status 1".into()
            }
        );
        let signal = GitError::NonZeroExit {
            args: "diff".into(),
            code: None,
            stderr: String::new(),
        };
        assert_eq!(
            map(signal),
            IpcError::GitFailed {
                message: "`git diff` was killed by a signal".into()
            }
        );
    }

    #[test]
    fn timeout() {
        let e = GitError::Timeout {
            args: "status".into(),
            timeout: Duration::from_secs(10),
        };
        assert_eq!(map(e), IpcError::Timeout);
    }

    #[test]
    fn io_and_parse_get_short_messages_without_internals() {
        let io = GitError::Io(std::io::Error::other("secret path /home/u/x"));
        let mapped = map(io);
        assert_eq!(
            mapped,
            IpcError::GitFailed {
                message: "I/O error while running git".into()
            }
        );

        let parse = GitError::Parse(ParseError::new("garbage"));
        assert_eq!(
            map(parse),
            IpcError::GitFailed {
                message: "unexpected output from git".into()
            }
        );
    }

    #[test]
    fn store_backend_error_hides_the_detail() {
        let e = StoreError::Backend("disk I/O error at /home/u/why.db".into());
        assert_eq!(
            IpcError::from(e),
            IpcError::Internal {
                message: "internal storage error".into()
            }
        );
    }

    #[test]
    fn untrusted_filters_carry_their_drivers_to_the_ui() {
        use crate::git::parse_filter_config;
        let out = b"local\0file:.git/config\0filter.evil.clean\n./x clean\0";
        let drivers = parse_filter_config(out).unwrap();
        let IpcError::UntrustedFilters { drivers } = map(GitError::UntrustedFilters { drivers })
        else {
            panic!("wrong variant");
        };
        assert_eq!(drivers.len(), 1);
        assert_eq!(drivers[0].name, "evil");
        assert_eq!(drivers[0].clean.as_deref(), Some("./x clean"));
        assert!(drivers[0].needs_trust && !drivers[0].trusted);
    }

    #[test]
    fn question_mark_converts_via_from() {
        fn fails() -> Result<(), IpcError> {
            Err(GitError::NotARepository {
                path: PathBuf::from("/x"),
            })?;
            Ok(())
        }
        assert_eq!(fails(), Err(IpcError::NotARepo));
    }
}
