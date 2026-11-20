//! The hook allowlist: which directories `why-hook` may capture events from.
//!
//! Decision 22: WHY captures only registered repositories. The app writes this
//! file (`<data dir>/hook-allowlist.json`, atomically) and the hook reads it on
//! every run, so events from unregistered repositories are dropped even while
//! the app is not running (nothing is sent, nothing is spooled).
//!
//! The file lists canonical roots: each registered repository's main worktree
//! and every linked worktree git knows about. Linked worktrees usually live
//! *outside* the main root (`why-worktrees/WHY-13-...`), so a repo root alone
//! would not cover them.
//!
//! Safe defaults, in this order: a missing, unreadable, oversized, corrupt or
//! newer-versioned file means "capture nothing". A hook that guessed would
//! record a stranger's code; one that drops loses some timeline detail.

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Version of the file format. Bump when a change would make an older hook
/// misread a newer file (a hook that does not know the version captures
/// nothing, see above). Adding an optional field does not need a bump.
pub const ALLOWLIST_VERSION: u32 = 1;

/// The hook refuses a bigger file than this: it reads it on every event, and a
/// runaway file must not slow the agent down. 1 MiB holds ~10,000 roots.
pub const MAX_ALLOWLIST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Error)]
pub enum AllowlistError {
    #[error("cannot read the allowlist: {0}")]
    Io(#[from] std::io::Error),
    #[error("allowlist is larger than {MAX_ALLOWLIST_BYTES} bytes")]
    TooLarge,
    #[error("allowlist is not valid JSON: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("allowlist version {v} is not supported (this build reads {ALLOWLIST_VERSION})")]
    UnsupportedVersion { v: u32 },
}

/// Canonical directories events may come from. Wire format (snake_case):
/// `{"v":1,"written_at_ms":1,"repo_roots":["/a"],"worktree_roots":["/a","/b"]}`.
///
/// Two lists because they answer different questions (what the user
/// registered vs. what git says belongs to it); the hook treats them alike.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Allowlist {
    pub v: u32,
    /// For humans debugging ("is this file stale?"); never read for decisions.
    pub written_at_ms: u64,
    pub repo_roots: Vec<String>,
    pub worktree_roots: Vec<String>,
}

#[derive(Deserialize)]
struct VersionProbe {
    v: u32,
}

impl Allowlist {
    /// An allowlist at this build's version. Roots are sorted and deduplicated so
    /// the same set always serializes to the same bytes (the app compares bytes
    /// to skip needless rewrites).
    pub fn new(written_at_ms: u64, repo_roots: Vec<String>, worktree_roots: Vec<String>) -> Self {
        let tidy = |mut v: Vec<String>| {
            v.sort_unstable();
            v.dedup();
            v
        };
        Self {
            v: ALLOWLIST_VERSION,
            written_at_ms,
            repo_roots: tidy(repo_roots),
            worktree_roots: tidy(worktree_roots),
        }
    }

    /// The roots, both lists together.
    pub fn roots(&self) -> impl Iterator<Item = &str> {
        self.repo_roots
            .iter()
            .chain(&self.worktree_roots)
            .map(String::as_str)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        // Cannot fail for strings and integers; an empty file would parse as
        // "corrupt", i.e. capture nothing, which is the safe outcome anyway.
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Parse the file's bytes. Reads `v` first, so a newer format is reported as
    /// [`AllowlistError::UnsupportedVersion`] even if its fields changed.
    pub fn parse(bytes: &[u8]) -> Result<Self, AllowlistError> {
        if bytes.len() as u64 > MAX_ALLOWLIST_BYTES {
            return Err(AllowlistError::TooLarge);
        }
        let probe: VersionProbe = serde_json::from_slice(bytes)?;
        if probe.v != ALLOWLIST_VERSION {
            return Err(AllowlistError::UnsupportedVersion { v: probe.v });
        }
        Ok(serde_json::from_slice(bytes)?)
    }

    /// Read and parse the file at `path`, reading at most
    /// [`MAX_ALLOWLIST_BYTES`] + 1 bytes (enough to notice it is too big).
    ///
    /// The file must be a REGULAR file. Opening a FIFO for reading blocks until
    /// a writer appears, which would stall every agent tool call behind this
    /// hook, so on unix it is opened `O_NONBLOCK | O_NOFOLLOW` (a FIFO then
    /// opens at once, a symlink is refused) and checked with `fstat` before any
    /// byte is read. Anything else counts as "no allowlist".
    pub fn read_from(path: &Path) -> Result<Self, AllowlistError> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(AllowlistError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the allowlist is not a regular file",
            )));
        }
        let mut buf = Vec::new();
        file.take(MAX_ALLOWLIST_BYTES + 1).read_to_end(&mut buf)?;
        Self::parse(&buf)
    }

    /// Is `cwd` one of the roots or inside one?
    ///
    /// `cwd` must already be canonical (symlinks resolved, `..` gone): the
    /// hook canonicalizes it, which is what makes a symlink *into* a root
    /// count and a symlink *out of* one not. The comparison is component-wise
    /// (`Path::starts_with`), never a string prefix: `/home/u/repo-evil` is not
    /// inside `/home/u/repo`.
    ///
    /// Unusable roots never match: relative or empty ones (`starts_with("")`
    /// is true for every path) and `/` itself (it would allow everything).
    pub fn allows(&self, cwd: &Path) -> bool {
        cwd.is_absolute()
            && self.roots().any(|root| {
                let root = Path::new(root);
                root.is_absolute() && root.parent().is_some() && cwd.starts_with(root)
            })
    }
}

impl Allowlist {
    /// The deepest root that contains `cwd` (see [`Allowlist::allows`] for the
    /// rules), if any.
    fn deepest_root(&self, cwd: &Path) -> Option<&Path> {
        if !cwd.is_absolute() {
            return None;
        }
        self.roots()
            .map(Path::new)
            .filter(|root| root.is_absolute() && root.parent().is_some() && cwd.starts_with(root))
            .max_by_key(|root| root.components().count())
    }

    /// [`Allowlist::allows`], plus: `cwd` must not be inside a DIFFERENT git
    /// checkout nested in an allowed root.
    ///
    /// A registered repository can contain other repositories (a submodule, a
    /// vendored clone, a nested worktree) that the user never registered. Their
    /// paths still start with the registered root, so the prefix test alone
    /// would capture them. Walking from `cwd` up to (excluding) the deepest
    /// matching root, any `.git` entry (a directory for a clone, a file for a
    /// submodule or linked worktree) means "another repository here". If that
    /// inner checkout IS allowlisted it is itself the deepest matching root and
    /// the walk stops there, so it is accepted. Cost: a few `lstat` calls, no
    /// git, so it works while the app is down.
    pub fn allows_checkout(&self, cwd: &Path) -> bool {
        let Some(root) = self.deepest_root(cwd) else {
            return false;
        };
        let mut dir = cwd;
        while dir != root {
            if std::fs::symlink_metadata(dir.join(".git")).is_ok() {
                return false;
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => return false,
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(roots: &[&str]) -> Allowlist {
        Allowlist::new(1, roots.iter().map(|s| s.to_string()).collect(), vec![])
    }

    #[test]
    fn matching_is_component_wise() {
        let a = list(&["/home/u/repo", "/work/wt"]);
        for yes in ["/home/u/repo", "/home/u/repo/src/deep", "/work/wt/x"] {
            assert!(a.allows(Path::new(yes)), "{yes}");
        }
        for no in [
            "/home/u/repo-evil",
            "/home/u",
            "/home/u/repository",
            "/",
            "relative/repo",
            "",
        ] {
            assert!(!a.allows(Path::new(no)), "{no:?}");
        }
    }

    fn touch_git(dir: &Path, as_file: bool) {
        std::fs::create_dir_all(dir).unwrap();
        if as_file {
            std::fs::write(dir.join(".git"), "gitdir: /elsewhere\n").unwrap();
        } else {
            std::fs::create_dir_all(dir.join(".git")).unwrap();
        }
    }

    #[test]
    fn nested_unregistered_checkouts_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap().join("repo");
        touch_git(&root, false);
        let a = list(&[root.to_str().unwrap()]);

        // The repository itself, and plain subdirectories: accepted.
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        assert!(a.allows_checkout(&root));
        assert!(a.allows_checkout(&root.join("src/deep")));

        // A submodule (`.git` FILE), a nested clone (`.git` DIR): rejected, at
        // their root and anywhere below it.
        touch_git(&root.join("vendor/sub"), true);
        touch_git(&root.join("vendor/clone"), false);
        std::fs::create_dir_all(root.join("vendor/sub/src")).unwrap();
        for bad in ["vendor/sub", "vendor/sub/src", "vendor/clone"] {
            assert!(!a.allows_checkout(&root.join(bad)), "{bad}");
            assert!(
                a.allows(&root.join(bad)),
                "(the plain prefix test would accept {bad})"
            );
        }

        // The same nested worktree, once allowlisted, is accepted.
        let inner = root.join("vendor/sub");
        let both = list(&[root.to_str().unwrap(), inner.to_str().unwrap()]);
        assert!(both.allows_checkout(&inner));
        assert!(both.allows_checkout(&inner.join("src")));
        // ...but its sibling clone still is not.
        assert!(!both.allows_checkout(&root.join("vendor/clone")));
        assert!(!both.allows_checkout(tmp.path()));
    }

    #[test]
    fn unusable_roots_never_match() {
        for bad in ["", "/", "relative", "./x"] {
            let a = list(&[bad]);
            assert!(!a.allows(Path::new("/anything")), "root {bad:?}");
            assert!(!a.allows(Path::new("/")), "root {bad:?}");
        }
    }

    #[test]
    fn round_trip_is_deterministic() {
        let a = Allowlist::new(
            5,
            vec!["/b".into(), "/a".into(), "/a".into()],
            vec!["/c".into()],
        );
        assert_eq!(a.repo_roots, ["/a", "/b"]);
        assert_eq!(Allowlist::parse(&a.to_bytes()).unwrap(), a);
        let same = Allowlist::new(5, vec!["/a".into(), "/b".into()], vec!["/c".into()]);
        assert_eq!(a.to_bytes(), same.to_bytes());
    }

    #[test]
    fn wire_field_names_are_pinned() {
        let v: serde_json::Value = serde_json::from_slice(&list(&["/a"]).to_bytes()).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["repo_roots", "v", "worktree_roots", "written_at_ms"]);
        assert_eq!(v["v"], 1);
    }

    #[test]
    fn corrupt_unknown_or_huge_files_are_errors() {
        assert!(matches!(
            Allowlist::parse(b""),
            Err(AllowlistError::Malformed(_))
        ));
        assert!(matches!(
            Allowlist::parse(b"{\"v\":1"),
            Err(AllowlistError::Malformed(_))
        ));
        assert!(matches!(
            Allowlist::parse(br#"{"v":1,"repo_roots":"/a"}"#),
            Err(AllowlistError::Malformed(_))
        ));
        assert!(matches!(
            Allowlist::parse(br#"{"v":2,"anything":1}"#),
            Err(AllowlistError::UnsupportedVersion { v: 2 })
        ));
        let huge = vec![b' '; MAX_ALLOWLIST_BYTES as usize + 1];
        assert!(matches!(
            Allowlist::parse(&huge),
            Err(AllowlistError::TooLarge)
        ));
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        let a = Allowlist::parse(
            br#"{"v":1,"written_at_ms":1,"repo_roots":["/a"],"worktree_roots":[],"later":true}"#,
        )
        .unwrap();
        assert!(a.allows(Path::new("/a/x")));
    }
}
