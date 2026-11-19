//! Test fixtures: build real repositories in temp directories.
//!
//! Fixture setup uses plain blocking `std::process::Command` (not the runner
//! under test) so a runner bug cannot hide itself by also breaking the setup.

#![allow(dead_code)] // each test module uses a different subset

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// Remove every `GIT_*` variable the test process inherited (except
/// `GIT_EXEC_PATH`, which only locates git's helpers).
///
/// This is not cosmetic. `git rebase -x <cmd>` (and hooks) run `<cmd>` with
/// `GIT_DIR` pointing at the *real* repository, and `GIT_DIR` beats `-C`. A
/// fixture that inherited it would run `git init`, `git commit`, `git branch -m`
/// and `git config` against the developer's actual repository instead of the
/// temp dir (this happened once: it renamed `main`, flipped `core.bare` and wrote
/// a test identity into the shared config).
fn scrub_inherited_git_env(cmd: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") && key != "GIT_EXEC_PATH" {
            cmd.env_remove(key);
        }
    }
}

/// Run git for fixture setup with a hermetic environment: ignore the
/// developer's global/system config (their signing, hooks, aliases, default
/// branch...) and any inherited `GIT_*` variables, so tests behave the same on
/// every machine and can never touch a repository outside their temp dir.
pub fn git_raw<I, S>(dir: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    scrub_inherited_git_env(&mut cmd);
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .output()
        .expect("failed to spawn git (is it installed?)")
}

/// Canary: abort loudly if git, run in `dir`, resolves a git directory outside
/// `root`. Cheap insurance that a fixture can never modify a real repository.
pub fn assert_isolated(dir: &Path, root: &Path) {
    let git_dir = git(dir, ["rev-parse", "--absolute-git-dir"]);
    assert!(
        Path::new(&git_dir).starts_with(root),
        "fixture escaped its temp dir: git dir {git_dir} is not under {}",
        root.display()
    );
}

/// Like [`git_raw`] but panics unless git exits 0. Returns trimmed stdout.
pub fn git<I, S>(dir: &Path, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<_> = args.into_iter().collect();
    let out = git_raw(dir, &args);
    assert!(
        out.status.success(),
        "git {:?} failed in {}: {}",
        args.iter()
            .map(|a| a.as_ref().to_string_lossy())
            .collect::<Vec<_>>(),
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository inside a temp directory. The directory (and every worktree or
/// clone created next to the repo) is deleted when this value drops.
pub struct TestRepo {
    /// Owns the temp directory.
    pub root: TempDir,
    /// Canonical path of the working tree (`<root>/repo`).
    pub path: PathBuf,
}

impl TestRepo {
    /// `git init -b main` plus local config, no commits.
    pub fn new() -> Self {
        Self::new_named("repo")
    }

    /// Like [`TestRepo::new`] with a chosen directory name (e.g. one with a newline).
    pub fn new_named(dir_name: &str) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        // Canonical, so expectations match what the library returns even where
        // the temp dir is behind a symlink (macOS: /var -> /private/var).
        let base = root.path().canonicalize().unwrap();
        let path = base.join(dir_name);
        std::fs::create_dir(&path).unwrap();
        let repo = TestRepo { root, path };
        git(&repo.path, ["init", "-q", "-b", "main"]);
        assert_isolated(&repo.path, &base);
        configure(&repo.path);
        repo
    }

    /// `new()` plus one commit (`README.md`) on `main`.
    pub fn with_initial_commit() -> Self {
        let repo = Self::new();
        repo.write("README.md", "hello\n");
        repo.commit_all("initial");
        repo
    }

    /// Clone `origin` (a bare repo) like a developer would; sets `origin/HEAD`.
    pub fn clone_of(origin: &Path) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let base = root.path().canonicalize().unwrap();
        let path = base.join("clone");
        git(
            &base,
            [
                OsStr::new("clone"),
                OsStr::new("-q"),
                origin.as_os_str(),
                path.as_os_str(),
            ],
        );
        assert_isolated(&path, &base);
        configure(&path);
        TestRepo { root, path }
    }

    /// Directory that holds the repo, handy for placing siblings next to it.
    pub fn base(&self) -> PathBuf {
        self.path.parent().unwrap().to_path_buf()
    }

    pub fn git<I, S>(&self, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        git(&self.path, args)
    }

    /// Write a file (creating parent directories) relative to the working tree.
    pub fn write(&self, rel: &str, contents: &str) {
        write_in(&self.path, rel, contents.as_bytes());
    }

    pub fn commit_all(&self, msg: &str) -> String {
        self.git(["add", "-A"]);
        self.git(["commit", "-q", "-m", msg]);
        self.git(["rev-parse", "HEAD"])
    }

    /// Create a linked worktree on a new branch next to the repo. Returns its
    /// canonical path.
    pub fn add_worktree(&self, name: &str, branch: &str) -> PathBuf {
        let path = self.base().join(name);
        self.git([
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("-q"),
            OsStr::new("-b"),
            OsStr::new(branch),
            path.as_os_str(),
        ]);
        path.canonicalize().unwrap()
    }

    /// Create a bare repository with `main` containing one commit; returns its path.
    pub fn bare_origin_with_commit() -> (TempDir, PathBuf) {
        let root = tempfile::tempdir().expect("tempdir");
        let base = root.path().canonicalize().unwrap();
        let seed = base.join("seed");
        std::fs::create_dir(&seed).unwrap();
        git(&seed, ["init", "-q", "-b", "main"]);
        assert_isolated(&seed, &base);
        configure(&seed);
        write_in(&seed, "README.md", b"origin\n");
        git(&seed, ["add", "-A"]);
        git(&seed, ["commit", "-q", "-m", "origin initial"]);
        let bare = base.join("origin.git");
        git(
            &base,
            [
                OsStr::new("clone"),
                OsStr::new("-q"),
                OsStr::new("--bare"),
                seed.as_os_str(),
                bare.as_os_str(),
            ],
        );
        (root, bare)
    }
}

/// Local identity and settings; applies to all worktrees (config is shared).
pub fn configure(dir: &Path) {
    git(dir, ["config", "user.name", "Test User"]);
    git(dir, ["config", "user.email", "test@example.com"]);
    git(dir, ["config", "commit.gpgsign", "false"]);
    git(dir, ["config", "init.defaultBranch", "main"]);
}

/// Write bytes to `dir/rel`, creating parent directories.
pub fn write_in(dir: &Path, rel: &str, contents: &[u8]) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

/// A `GitRunner` that ignores the developer's global/system git config, like
/// [`git_raw`] does for fixture setup. Use it in every test that runs library
/// code, so a setting such as `safe.bareRepository=explicit` on the developer's
/// machine cannot change the result.
pub fn test_runner() -> why_lib::git::GitRunner {
    why_lib::git::GitRunner::new().with_isolated_config()
}

/// Write an executable `/bin/sh` script that is safe to run right away.
///
/// # The race this avoids (ETXTBSY, "Text file busy")
///
/// Cargo runs tests on parallel threads, and many of them spawn child processes.
/// `fork` copies every open file descriptor into the child, and the child keeps
/// them until it calls `exec`. If another test's `fork` lands while this
/// function's write descriptor is still open, that child holds a *writable*
/// reference to the script's inode for a few microseconds, and the kernel refuses
/// to `exec` a file somebody still has open for writing: `ETXTBSY`. The test then
/// fails with `Io(Os { code: 26, kind: ExecutableFileBusy })` although nothing is
/// wrong (see rust-lang/rust#114554).
///
/// Writing to a temporary name and renaming does NOT help: the inherited
/// descriptor refers to the inode, which is the same after the rename. So this
/// helper makes the file *provably* runnable before returning: it executes the
/// script once in "probe" mode and retries, bounded (50 x 10 ms), for as long as
/// the error is `ExecutableFileBusy`. A successful `exec` means no writer is left
/// (and none can appear again, nobody writes to the file any more), so the real
/// exec later, by the test or by a git hook, cannot hit the race.
///
/// The probe must not have side effects, so a guard line is inserted right after
/// the shebang: the script exits at once when `WHY_TEST_PROBE` is set. `body` is
/// the script WITHOUT the `#!/bin/sh` line.
#[cfg(unix)]
pub fn write_executable_script(path: &Path, body: &str) {
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(
        path,
        format!("#!/bin/sh\n[ -z \"$WHY_TEST_PROBE\" ] || exit 0\n{body}"),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();

    for attempt in 0..50 {
        match Command::new(path).env("WHY_TEST_PROBE", "1").status() {
            Ok(status) => {
                assert!(
                    status.success(),
                    "probe of {} failed: {status}",
                    path.display()
                );
                return;
            }
            Err(e) if e.kind() == ErrorKind::ExecutableFileBusy && attempt < 49 => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => panic!("cannot run {}: {e}", path.display()),
        }
    }
}
