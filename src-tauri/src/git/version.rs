//! Which git versions we support, and how to ask the installed git for its version.
//!
//! ## Minimum version: 2.36.0
//!
//! The newest feature we depend on is `git worktree list --porcelain -z`. Each
//! feature and the release notes (`Documentation/RelNotes/<version>.adoc` in
//! git's repository) that introduced it:
//!
//! | We use | Since | Release notes |
//! |---|---|---|
//! | `status --porcelain=v2` (+ `--branch` headers) | 2.11.0 | 2.11.0: "Enhance `git status --porcelain` output …" |
//! | `--no-optional-locks` / `GIT_OPTIONAL_LOCKS` | 2.15.0 | 2.15.0 |
//! | `rev-parse --end-of-options` | 2.30.0 | 2.30.0 |
//! | `worktree list`: locked annotation | 2.30.0 | 2.30.0 |
//! | `rev-parse --path-format=absolute` | 2.31.0 | 2.31.0 |
//! | `worktree list --porcelain`: `locked` / `prunable` lines | 2.31.0 | 2.31.0 |
//! | **`worktree list --porcelain -z`** | **2.36.0** | 2.36.0 |
//!
//! Not pinned down from release notes: `for-each-ref --format=%(symref)`. The
//! notes of 2.1 to 2.20 do not mention it, so the version that introduced it is
//! unknown; it is documented in git's reference and works on 2.56 (what the tests
//! ran on). Everything else we call (`-C`, `merge-base`, `rev-list --left-right
//! --count`, `symbolic-ref`, `rev-parse --verify`) is much older than 2.11.
//!
//! The tests only ran on git 2.56.0; the table says what the *documentation*
//! requires, not what was exercised on old binaries.
//!
//! WHY does not enforce this at startup yet. [`git_version`] and
//! [`GitVersion::meets_minimum`] are there for a later "git too old" message.

use std::cmp::Ordering;
use std::fmt;
use std::path::Path;

use super::error::{GitError, ParseError};
use super::runner::GitRunner;

/// Oldest git whose documented behaviour covers everything WHY relies on.
pub const MIN_GIT_VERSION: GitVersion = GitVersion {
    major: 2,
    minor: 36,
    patch: 0,
};

/// `major.minor.patch` of a git executable. Ordered (derive compares fields in
/// declaration order, which is exactly version order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    /// `true` if this version is at least [`MIN_GIT_VERSION`].
    pub fn meets_minimum(&self) -> bool {
        self.cmp(&MIN_GIT_VERSION) != Ordering::Less
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Ask the git that `runner` would spawn for its version.
pub async fn git_version(runner: &GitRunner) -> Result<GitVersion, GitError> {
    // `--version` needs no repository; `.` only satisfies the runner's `-C`.
    let out = runner.run(Path::new("."), ["--version"]).await?;
    Ok(parse_git_version(&out)?)
}

/// Parse `git --version` output. Pure function.
///
/// Real-world shapes: `git version 2.56.0`, `git version 2.39.5 (Apple
/// Git-154)`, `git version 2.43.0.windows.1`, `git version 2.45.0-rc0`. We read
/// the token after `git version `, take up to three dot-separated components
/// and use the leading digits of each (a missing patch is 0).
pub(crate) fn parse_git_version(out: &[u8]) -> Result<GitVersion, ParseError> {
    let text = String::from_utf8_lossy(out);
    let bad = || ParseError::new(format!("unrecognised git version output {:?}", text.trim()));
    let token = text
        .trim()
        .strip_prefix("git version ")
        .and_then(|rest| rest.split_whitespace().next())
        .ok_or_else(bad)?;

    let mut parts = token.split('.').map(|p| {
        let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok()
    });
    let major = parts.next().flatten().ok_or_else(bad)?;
    let minor = parts.next().flatten().ok_or_else(bad)?;
    let patch = parts.next().flatten().unwrap_or(0);
    Ok(GitVersion {
        major,
        minor,
        patch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u32, minor: u32, patch: u32) -> GitVersion {
        GitVersion {
            major,
            minor,
            patch,
        }
    }

    #[test]
    fn parses_real_world_outputs() {
        assert_eq!(
            parse_git_version(b"git version 2.56.0\n").unwrap(),
            v(2, 56, 0)
        );
        assert_eq!(
            parse_git_version(b"git version 2.39.5 (Apple Git-154)\n").unwrap(),
            v(2, 39, 5)
        );
        assert_eq!(
            parse_git_version(b"git version 2.43.0.windows.1\n").unwrap(),
            v(2, 43, 0)
        );
        assert_eq!(
            parse_git_version(b"git version 2.45.0-rc0\n").unwrap(),
            v(2, 45, 0)
        );
        assert_eq!(
            parse_git_version(b"git version 2.40\n").unwrap(),
            v(2, 40, 0)
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_git_version(b"").is_err());
        assert!(parse_git_version(b"hello").is_err());
        assert!(parse_git_version(b"git version x.y.z").is_err());
        assert!(parse_git_version(b"git version 2").is_err());
    }

    #[test]
    fn ordering_and_minimum() {
        assert!(v(2, 36, 0).meets_minimum());
        assert!(v(2, 56, 0).meets_minimum());
        assert!(v(3, 0, 0).meets_minimum());
        assert!(!v(2, 35, 9).meets_minimum());
        assert!(!v(1, 99, 0).meets_minimum());
        assert!(v(2, 9, 0) < v(2, 10, 0)); // numeric, not textual, comparison
    }
}
