//! Repository validation: "is this directory (or a folder inside it) a git
//! repository, and where do its pieces live?"
//!
//! Git has three directories that matter, and they differ in a linked worktree:
//!
//! ```text
//! main checkout   /code/app            toplevel   (working files)
//!                 /code/app/.git       common_dir == git_dir
//!
//! linked worktree /code/app-feature    toplevel
//!                 (has a *file* `.git` containing "gitdir: /code/app/.git/worktrees/app-feature")
//!                 /code/app/.git       common_dir   (shared: objects, refs, config)
//!                 /code/app/.git/worktrees/app-feature   git_dir   (private: HEAD, index)
//! ```

use std::path::{Path, PathBuf};

use super::error::{GitError, ParseError};
use super::oid::path_from_bytes;
use super::runner::GitRunner;

/// Where a repository's pieces live. All paths are absolute and canonical
/// (symlinks resolved), so two ways of spelling the same directory compare equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// Root of the working tree that `path` belongs to. `None` for a bare
    /// repository, which has no working files.
    pub toplevel: Option<PathBuf>,
    /// The directory shared by *all* worktrees of this repository (objects,
    /// branches, tags, config). Identifies "the repository".
    pub common_dir: PathBuf,
    /// This worktree's private git directory (its own `HEAD` and index). Equal
    /// to `common_dir` for the main worktree.
    pub git_dir: PathBuf,
    pub is_bare: bool,
}

/// Validate that `path` is inside a git repository and describe it.
///
/// `path` may be any directory inside the working tree (git walks upwards),
/// not only its root. Returns [`GitError::NotARepository`] otherwise.
///
/// Each fact is a separate `rev-parse` call (run concurrently). `rev-parse` has
/// no `-z` mode, so with several answers in one output a path containing a
/// newline would be indistinguishable from two answers. One answer per process
/// means "stdout minus the single trailing newline" is exactly the path, even
/// for a directory called `re\npo`.
pub async fn open_repo(runner: &GitRunner, path: &Path) -> Result<RepoInfo, GitError> {
    // `--path-format=absolute` makes git print absolute paths instead of
    // sometimes-relative ones like `.git`.
    let query =
        |flag: &'static str| runner.run(path, ["rev-parse", "--path-format=absolute", flag]);
    let (common, git_dir, bare, top) = tokio::join!(
        query("--git-common-dir"),
        query("--git-dir"),
        query("--is-bare-repository"),
        query("--show-toplevel"),
    );
    // If `path` is not a repository all four fail alike; report the first.
    let common_dir = parse_path(&common?)?;
    let git_dir = parse_path(&git_dir?)?;
    let is_bare = parse_bool(&bare?)?;

    let toplevel = match top {
        Ok(out) => Some(parse_path(&out)?),
        // A bare repository has no working tree, so `--show-toplevel` is fatal
        // ("this operation must be run in a work tree", exit 128). That is NOT
        // "not a repository": the toplevel is simply `None`. The same message
        // appears when `path` is *inside* the `.git` directory of a normal repo;
        // `is_bare` is then false and we report the original error.
        Err(GitError::NonZeroExit { args, code, stderr })
            if stderr.contains("must be run in a work tree") =>
        {
            if !is_bare {
                return Err(GitError::NonZeroExit { args, code, stderr });
            }
            None
        }
        Err(e) => return Err(e),
    };

    canonicalize_info(RepoInfo {
        toplevel,
        common_dir,
        git_dir,
        is_bare,
    })
    .await
}

/// Parse the stdout of a single-path `rev-parse` query: the path followed by
/// exactly one `\n`. Only that one newline is removed; any others belong to the
/// path. Pure function; the path is NOT canonicalized here.
pub(crate) fn parse_path(out: &[u8]) -> Result<PathBuf, ParseError> {
    let path = out.strip_suffix(b"\n").unwrap_or(out);
    if path.is_empty() {
        return Err(ParseError::new("rev-parse printed an empty path"));
    }
    Ok(path_from_bytes(path))
}

/// Parse `--is-bare-repository` output (`true` / `false`).
pub(crate) fn parse_bool(out: &[u8]) -> Result<bool, ParseError> {
    match out.trim_ascii() {
        b"true" => Ok(true),
        b"false" => Ok(false),
        other => Err(ParseError::new(format!(
            "--is-bare-repository printed {:?}",
            String::from_utf8_lossy(other)
        ))),
    }
}

/// Resolve symlinks (`/tmp` -> `/private/tmp` on macOS, user symlinks to
/// `~/code`, ...) so equal directories get equal paths and equal ids.
async fn canonicalize_info(info: RepoInfo) -> Result<RepoInfo, GitError> {
    let toplevel = match info.toplevel {
        Some(p) => Some(tokio::fs::canonicalize(p).await?),
        None => None,
    };
    Ok(RepoInfo {
        toplevel,
        common_dir: tokio::fs::canonicalize(info.common_dir).await?,
        git_dir: tokio::fs::canonicalize(info.git_dir).await?,
        is_bare: info.is_bare,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_loses_only_the_final_newline() {
        assert_eq!(
            parse_path(b"/code/app\n").unwrap(),
            PathBuf::from("/code/app")
        );
        assert_eq!(
            parse_path(b"/code/my wt\n").unwrap(),
            PathBuf::from("/code/my wt")
        );
        // A newline INSIDE the path is kept.
        assert_eq!(
            parse_path(b"/code/re\npo\n").unwrap(),
            PathBuf::from("/code/re\npo")
        );
        // Tolerate a missing final newline.
        assert_eq!(parse_path(b"/x").unwrap(), PathBuf::from("/x"));
    }

    #[test]
    fn empty_path_is_an_error() {
        assert!(parse_path(b"").is_err());
        assert!(parse_path(b"\n").is_err());
    }

    #[test]
    fn bool_parsing() {
        assert!(parse_bool(b"true\n").unwrap());
        assert!(!parse_bool(b"false\n").unwrap());
        assert!(parse_bool(b"maybe\n").is_err());
        assert!(parse_bool(b"").is_err());
    }
}
