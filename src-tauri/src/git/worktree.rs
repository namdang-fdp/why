//! Worktree discovery: every checkout of a repository, including ones that were
//! created outside WHY (by an agent, by `git worktree add` in a terminal, ...).
//!
//! A repository has one *main* worktree plus any number of *linked* worktrees.
//! Git records the linked ones under `<common_dir>/worktrees/<name>/`, so
//! `git worktree list` can enumerate them from any of them.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::{GitError, ParseError};
use super::oid::{Oid, path_bytes, path_from_bytes};
use super::repo::RepoInfo;
use super::runner::GitRunner;

/// One entry of `git worktree list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    /// Where the worktree lives. Canonical when the directory exists; for a
    /// prunable (deleted) worktree it is best-effort (see [`canonicalize_best_effort`]).
    pub path: PathBuf,
    /// The commit checked out. `None` for an unborn branch (no commits yet) and
    /// for a bare repository.
    pub head: Option<Oid>,
    /// Full ref name of the checked-out branch, e.g. `refs/heads/feature/x`.
    /// `None` when detached or bare.
    pub branch: Option<String>,
    /// `true` for the first entry: git always lists the main worktree first.
    pub is_main: bool,
    pub is_bare: bool,
    /// HEAD points directly at a commit instead of a branch.
    pub is_detached: bool,
    /// `Some(reason)` if the worktree is locked (`git worktree lock`), which
    /// protects it from pruning/moving. An empty string means locked without a reason.
    pub locked: Option<String>,
    /// `Some(reason)` if git considers the entry stale, typically because its
    /// directory was deleted. `git worktree prune` would remove it.
    ///
    /// `None` does NOT guarantee that `path` exists: a *locked* worktree whose
    /// directory was deleted is reported as locked but not prunable (the lock
    /// exists to prevent pruning). Check the directory before running git in it.
    pub prunable: Option<String>,
}

impl WorktreeEntry {
    /// Branch name without the `refs/heads/` prefix.
    pub fn branch_short(&self) -> Option<&str> {
        self.branch
            .as_deref()
            .map(|b| b.strip_prefix("refs/heads/").unwrap_or(b))
    }

    /// Stable id of this worktree inside the repository whose shared directory
    /// is `common_dir`. See [`worktree_id`].
    pub fn id(&self, common_dir: &Path) -> String {
        worktree_id(common_dir, &self.path)
    }
}

/// List all worktrees of `repo`, main worktree first.
///
/// Runs git inside `repo.common_dir` (the shared `.git` directory) rather than
/// in a working tree: it works for bare repositories and still works if someone
/// deleted the main checkout's folder.
pub async fn list_worktrees(
    runner: &GitRunner,
    repo: &RepoInfo,
) -> Result<Vec<WorktreeEntry>, GitError> {
    // `-z` terminates every line with NUL instead of `\n`. Required for
    // correctness: a lock reason or a path may itself contain a newline.
    let out = runner
        .run(&repo.common_dir, ["worktree", "list", "--porcelain", "-z"])
        .await?;
    let mut entries = parse_worktree_list(&out)?;
    for entry in &mut entries {
        entry.path = canonicalize_best_effort(&entry.path).await;
    }
    Ok(entries)
}

/// Parse `git worktree list --porcelain -z`. Pure function.
///
/// Format: one record per worktree, each line `key` or `key value`, lines
/// separated by NUL, records separated by an extra NUL (an empty line):
///
/// ```text
/// worktree /code/app\0HEAD 49b4b0…\0branch refs/heads/main\0\0
/// worktree /code/wt\0HEAD 49b4b0…\0detached\0\0
/// worktree /code/old\0HEAD …\0branch refs/heads/x\0locked why\0\0
/// ```
pub fn parse_worktree_list(out: &[u8]) -> Result<Vec<WorktreeEntry>, ParseError> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;

    for line in out.split(|&b| b == 0) {
        if line.is_empty() {
            // Blank line: end of record.
            entries.extend(current.take());
            continue;
        }
        // Split at the FIRST space only; the value (a path, a lock reason) may
        // itself contain spaces.
        let (key, value) = match line.iter().position(|&b| b == b' ') {
            Some(i) => (&line[..i], Some(&line[i + 1..])),
            None => (line, None),
        };

        if key == b"worktree" {
            // A new record starts. (Tolerate a missing blank line before it.)
            entries.extend(current.take());
            let path = value.ok_or_else(|| ParseError::new("`worktree` line without a path"))?;
            current = Some(WorktreeEntry {
                path: path_from_bytes(path),
                head: None,
                branch: None,
                is_main: entries.is_empty(),
                is_bare: false,
                is_detached: false,
                locked: None,
                prunable: None,
            });
            continue;
        }

        let Some(entry) = current.as_mut() else {
            return Err(ParseError::new(format!(
                "attribute {:?} before any `worktree` line",
                String::from_utf8_lossy(key)
            )));
        };
        match key {
            b"HEAD" => {
                let oid = Oid::from_bytes(value.unwrap_or_default())?;
                // An unborn branch is reported as the all-zero id.
                entry.head = (!oid.is_null()).then_some(oid);
            }
            b"branch" => entry.branch = value.map(lossy),
            b"bare" => entry.is_bare = true,
            b"detached" => entry.is_detached = true,
            b"locked" => entry.locked = Some(value.map(lossy).unwrap_or_default()),
            b"prunable" => entry.prunable = Some(value.map(lossy).unwrap_or_default()),
            // Unknown keys: newer git versions may add some. Ignore them.
            _ => {}
        }
    }
    entries.extend(current);
    Ok(entries)
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Canonicalize `path`, tolerating a path that no longer exists.
///
/// A prunable worktree's directory is gone, so `canonicalize` fails with
/// `NotFound`. Rather than return an unstable raw path, canonicalize the nearest
/// existing ancestor and re-append the missing tail. This keeps the id of a
/// deleted worktree equal to the id it had while it still existed.
pub(crate) async fn canonicalize_best_effort(path: &Path) -> PathBuf {
    if let Ok(p) = tokio::fs::canonicalize(path).await {
        return p;
    }
    let mut missing_tail = Vec::new();
    let mut ancestor = path;
    while let Some(parent) = ancestor.parent() {
        if let Some(name) = ancestor.file_name() {
            missing_tail.push(name.to_owned());
        }
        if let Ok(mut canon) = tokio::fs::canonicalize(parent).await {
            canon.extend(missing_tail.iter().rev());
            return canon;
        }
        ancestor = parent;
    }
    path.to_path_buf()
}

/// A stable identifier for a worktree: 16 lowercase hex characters.
///
/// It is derived from the *canonical* common dir (which repository) and the
/// *canonical* worktree path (which checkout of it), and from nothing else.
/// In particular the **branch is not part of the id**: an agent running
/// `git switch other-branch` inside a worktree must not make the app think it
/// is a different worktree and lose its review state.
///
/// Why SHA-256 from a crate and not `std::hash`: ids are persisted (SQLite) and
/// later exported, so they must never change. `std`'s `DefaultHasher` is
/// documented as unspecified and may change between Rust releases. SHA-256 is a
/// fixed, well-known algorithm; we keep the first 8 bytes (64 bits), plenty to
/// avoid collisions among a person's worktrees. `sha2` was already in the
/// dependency tree through Tauri.
///
/// Trade-off: the id follows the path, so `git worktree move` yields a new id.
///
/// Callers should pass canonical paths (as [`RepoInfo`] and
/// [`WorktreeEntry::path`] already are).
pub fn worktree_id(common_dir: &Path, worktree_path: &Path) -> String {
    let mut hasher = Sha256::new();
    // Domain separator + version, so a future change of scheme cannot collide.
    hasher.update(b"why:worktree-id:v1\0");
    hasher.update(path_bytes(common_dir));
    // NUL cannot appear in a path, so `("/a", "b/c")` and `("/a/b", "c")` hash
    // differently: the boundary between the two paths is unambiguous.
    hasher.update([0u8]);
    hasher.update(path_bytes(worktree_path));
    let digest = hasher.finalize();
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// A stable identifier for a repository: 16 lowercase hex characters.
///
/// Derived from the *canonical git common dir* only. The common dir is shared by
/// the main checkout and every linked worktree, so adding the same repository
/// through any of its worktrees yields the same id. Same scheme as
/// [`worktree_id`] (SHA-256, first 8 bytes) with its own domain separator, so a
/// repo id can never equal a worktree id by accident.
///
/// Trade-off: the id follows the common dir's path, so moving or re-cloning the
/// repository gives it a new id (and a new registration).
pub fn repo_id(common_dir: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"why:repo-id:v1\0");
    hasher.update(path_bytes(common_dir));
    let digest = hasher.finalize();
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "49b4b03d7dd07d73f8b334a20f142f4f8c913376";
    const B: &str = "1111111111111111111111111111111111111111";

    fn nul(parts: &[&str]) -> Vec<u8> {
        let mut v = Vec::new();
        for p in parts {
            v.extend_from_slice(p.as_bytes());
            v.push(0);
        }
        v
    }

    #[test]
    fn parses_main_detached_locked_prunable() {
        let out = nul(&[
            "worktree /code/app",
            &format!("HEAD {A}"),
            "branch refs/heads/main",
            "",
            "worktree /code/wt det",
            &format!("HEAD {B}"),
            "detached",
            "",
            "worktree /code/locked",
            &format!("HEAD {A}"),
            "branch refs/heads/feat/x",
            "locked agent is running",
            "",
            "worktree /code/gone",
            &format!("HEAD {A}"),
            "branch refs/heads/gone",
            "prunable gitdir file points to non-existent location",
            "",
        ]);
        let wts = parse_worktree_list(&out).unwrap();
        assert_eq!(wts.len(), 4);

        assert!(wts[0].is_main);
        assert_eq!(wts[0].path, PathBuf::from("/code/app"));
        assert_eq!(wts[0].branch_short(), Some("main"));
        assert_eq!(wts[0].head.as_ref().unwrap().as_str(), A);

        assert!(!wts[1].is_main);
        assert!(wts[1].is_detached);
        assert_eq!(wts[1].branch, None);
        assert_eq!(wts[1].path, PathBuf::from("/code/wt det")); // space kept

        assert_eq!(wts[2].locked.as_deref(), Some("agent is running"));
        assert_eq!(wts[2].branch_short(), Some("feat/x"));
        assert_eq!(wts[2].prunable, None);

        assert_eq!(
            wts[3].prunable.as_deref(),
            Some("gitdir file points to non-existent location")
        );
    }

    #[test]
    fn locked_without_reason_and_multiline_reason() {
        let out = nul(&[
            "worktree /a",
            &format!("HEAD {A}"),
            "branch refs/heads/main",
            "",
            "worktree /b",
            &format!("HEAD {A}"),
            "branch refs/heads/b",
            "locked",
            "",
            "worktree /c",
            &format!("HEAD {A}"),
            "branch refs/heads/c",
            "locked line one\nline two",
            "",
        ]);
        let wts = parse_worktree_list(&out).unwrap();
        assert_eq!(wts[1].locked.as_deref(), Some(""));
        assert_eq!(wts[2].locked.as_deref(), Some("line one\nline two"));
    }

    #[test]
    fn bare_and_unborn() {
        let out = nul(&["worktree /srv/r.git", "bare", ""]);
        let wts = parse_worktree_list(&out).unwrap();
        assert!(wts[0].is_bare && wts[0].is_main);
        assert_eq!(wts[0].head, None);

        let zero = "0".repeat(40);
        let out = nul(&[
            "worktree /new",
            &format!("HEAD {zero}"),
            "branch refs/heads/main",
            "",
        ]);
        let wts = parse_worktree_list(&out).unwrap();
        assert_eq!(wts[0].head, None);
        assert_eq!(wts[0].branch_short(), Some("main"));
    }

    #[test]
    fn unicode_and_newline_in_path() {
        let out = nul(&[
            "worktree /code/tệp mới\nsecond line",
            &format!("HEAD {A}"),
            "detached",
            "",
        ]);
        let wts = parse_worktree_list(&out).unwrap();
        assert_eq!(wts[0].path, PathBuf::from("/code/tệp mới\nsecond line"));
    }

    #[test]
    fn tolerates_missing_final_blank_and_unknown_keys() {
        let out = nul(&[
            "worktree /a",
            &format!("HEAD {A}"),
            "future-key some value",
            "detached",
        ]);
        let wts = parse_worktree_list(&out).unwrap();
        assert_eq!(wts.len(), 1);
        assert!(wts[0].is_detached);
    }

    #[test]
    fn empty_output_is_empty_list() {
        assert!(parse_worktree_list(b"").unwrap().is_empty());
    }

    #[test]
    fn rejects_orphan_attribute_and_bad_oid() {
        assert!(parse_worktree_list(&nul(&["detached"])).is_err());
        assert!(parse_worktree_list(&nul(&["worktree /a", "HEAD nothex"])).is_err());
    }

    #[test]
    fn id_is_stable_and_depends_only_on_the_two_paths() {
        let a = worktree_id(Path::new("/r/.git"), Path::new("/r"));
        assert_eq!(a.len(), 16);
        assert_eq!(a, worktree_id(Path::new("/r/.git"), Path::new("/r")));
        // Known answer: pins the algorithm so an accidental change fails loudly.
        assert_eq!(a, "8f8f708e0e1bd4f6");
        assert_ne!(a, worktree_id(Path::new("/r/.git"), Path::new("/r2")));
        assert_ne!(a, worktree_id(Path::new("/other/.git"), Path::new("/r")));
    }

    #[test]
    fn repo_id_is_stable_and_depends_only_on_the_common_dir() {
        let a = repo_id(Path::new("/r/.git"));
        assert_eq!(a.len(), 16);
        assert_eq!(a, repo_id(Path::new("/r/.git")));
        // Known answer: pins the algorithm (and domain separator).
        assert_eq!(a, "c86f971790e37dab");
        assert_ne!(a, repo_id(Path::new("/other/.git")));
    }

    #[test]
    fn repo_id_and_worktree_id_never_collide_for_the_same_input() {
        // Different domain separators: even a repo whose common dir equals a
        // worktree path gets a different id.
        let dir = Path::new("/r/.git");
        assert_ne!(repo_id(dir), worktree_id(dir, dir));
    }

    #[test]
    fn id_boundary_is_unambiguous() {
        let x = worktree_id(Path::new("/a"), Path::new("b/c"));
        let y = worktree_id(Path::new("/a/b"), Path::new("c"));
        assert_ne!(x, y);
    }
}
