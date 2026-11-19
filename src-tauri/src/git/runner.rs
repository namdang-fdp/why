//! The one place that spawns `git`.
//!
//! Every git call in WHY goes through [`GitRunner::run`], so the safety rules
//! for a *passive observer* live here and nowhere else:
//!
//! * **No shell.** Arguments are passed as `OsStr`s straight to `execve`, so a
//!   file name like `a; rm -rf ~` is just bytes, never code.
//! * **`GIT_OPTIONAL_LOCKS=0`.** See the comment on [`GitRunner::command`].
//! * **Bounded.** A timeout kills runaway children; a semaphore caps how many
//!   git processes we run at once so a watcher burst cannot fork-bomb the box.

use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::sync::Semaphore;

use super::error::GitError;

/// Default per-command timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// Default number of git processes allowed to run at the same time.
pub const DEFAULT_MAX_CONCURRENCY: usize = 8;

/// How much of git's stderr we keep in an error message.
const STDERR_EXCERPT_BYTES: usize = 1024;

/// Spawns git with our safety settings. Cheap to clone (the semaphore is shared
/// between clones, so the concurrency limit is global to one runner).
#[derive(Debug, Clone)]
pub struct GitRunner {
    program: OsString,
    semaphore: Arc<Semaphore>,
    timeout: Duration,
    /// Extra environment for every spawned git (see [`GitRunner::with_env`]).
    extra_env: Vec<(OsString, OsString)>,
}

impl Default for GitRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl GitRunner {
    /// `git` from `PATH`, 10 s timeout, at most 8 concurrent processes.
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_CONCURRENCY, DEFAULT_TIMEOUT)
    }

    /// Choose the concurrency cap (clamped to at least 1) and the timeout.
    pub fn with_limits(max_concurrency: usize, timeout: Duration) -> Self {
        Self {
            program: OsString::from("git"),
            semaphore: Arc::new(Semaphore::new(max_concurrency.max(1))),
            timeout,
            extra_env: Vec::new(),
        }
    }

    /// Use a different executable (an absolute path to a specific git, or a
    /// non-existent name in tests to exercise [`GitError::GitNotFound`]).
    pub fn with_program(mut self, program: impl Into<OsString>) -> Self {
        self.program = program.into();
        self
    }

    /// Set an extra environment variable for every git this runner spawns.
    ///
    /// For configuring *where git reads its settings from* (`GIT_CONFIG_GLOBAL`,
    /// `HOME`, ...), for example to run against a known configuration instead of
    /// the user's. It is NOT a way to loosen the runner's safety rules: the
    /// variables the runner sets itself (`GIT_OPTIONAL_LOCKS`,
    /// `GIT_TERMINAL_PROMPT`, `LC_ALL`) are applied after these and always win,
    /// and the variables that redirect git to another repository (`GIT_DIR`, ...)
    /// are always removed, so neither can be overridden here.
    ///
    /// Set per runner, never process-wide: `std::env::set_var` is visible to every
    /// thread, so concurrent tests (or commands) would race on it.
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    /// Ignore the user's and the system's git configuration: only the
    /// repository's own config (and the runner's `-c` flags) apply. Makes git's
    /// behaviour independent of the machine, which tests need (a developer's
    /// `safe.bareRepository=explicit`, aliases, signing settings, ...).
    /// Shorthand for two [`GitRunner::with_env`] calls.
    pub fn with_isolated_config(self) -> Self {
        self.with_env("GIT_CONFIG_GLOBAL", "/dev/null")
            .with_env("GIT_CONFIG_NOSYSTEM", "1")
    }

    /// Run `git -C <dir> <args...>` and return its raw stdout bytes.
    ///
    /// Output is bytes, not `String`: paths in git output may not be UTF-8.
    ///
    /// Errors: [`GitError::GitNotFound`], [`GitError::NotARepository`],
    /// [`GitError::NonZeroExit`] (with the exit code and a stderr excerpt),
    /// [`GitError::Timeout`], [`GitError::Io`].
    pub async fn run<I, S>(&self, dir: &Path, args: I) -> Result<Vec<u8>, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        let summary = summarize_args(&args);

        // Wait for a free slot *before* the clock starts: the timeout measures
        // how long git runs, not how long we queued behind other commands. The
        // permit is released when `_permit` drops at the end of this function.
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| std::io::Error::other("git runner semaphore closed"))?;

        let mut command = self.command(dir, &args);
        let started = Instant::now();
        let child = command.spawn().map_err(|e| match e.kind() {
            ErrorKind::NotFound => GitError::GitNotFound {
                program: self.program.to_string_lossy().into_owned(),
            },
            _ => GitError::Io(e),
        })?;

        // `wait_with_output` reads stdout and stderr concurrently while waiting
        // (so a chatty child cannot block on a full pipe) and takes ownership
        // of the child. When the timeout fires, this future is dropped, which
        // drops the `Child`, and `kill_on_drop(true)` sends it SIGKILL.
        let output = match tokio::time::timeout(self.timeout, child.wait_with_output()).await {
            Ok(result) => result?,
            Err(_elapsed) => {
                tracing::warn!(args = %summary, timeout = ?self.timeout, "git timed out; killed");
                return Err(GitError::Timeout {
                    args: summary,
                    timeout: self.timeout,
                });
            }
        };
        tracing::debug!(args = %summary, elapsed = ?started.elapsed(), status = %output.status, "git finished");

        if output.status.success() {
            return Ok(output.stdout);
        }

        // With LC_ALL=C git's messages are always English, so matching on the
        // text is stable. Exit code 128 is git's generic "fatal:" status.
        if output.status.code() == Some(128) && contains(&output.stderr, b"not a git repository") {
            return Err(GitError::NotARepository {
                path: dir.to_path_buf(),
            });
        }
        Err(GitError::NonZeroExit {
            args: summary,
            code: output.status.code(),
            stderr: stderr_excerpt(&output.stderr),
        })
    }

    /// Build the command line and environment.
    fn command(&self, dir: &Path, args: &[OsString]) -> Command {
        let mut cmd = Command::new(&self.program);

        // `--no-pager`: we read a pipe, so git would not page anyway; belt and braces.
        cmd.arg("--no-pager");

        // THREAT: repository config is untrusted input for a passive observer.
        // Anything that can write `.git/config` (an agent, a build script, a
        // tool run in the repo) can name a program that git then *executes*
        // while we merely look at the checkout. `core.fsmonitor` is the one
        // that fires on a plain `git status`: if it names a hook script, git
        // runs it with the index timestamp to ask "what changed?" (and `true`
        // would auto-start a background fsmonitor daemon). So WHY would run
        // arbitrary code just by opening a repository. `-c` has the highest
        // precedence, so we force it off for EVERY command, not only status.
        // Child gits (e.g. status inside a submodule) inherit `-c` too.
        //
        // Audit of other settings that can make a read-only command execute a
        // program (the commands we run: rev-parse, symbolic-ref, for-each-ref,
        // merge-base, rev-list, worktree list, status):
        //  * hooks (`core.hooksPath`, `.git/hooks`): only run by write commands
        //    (commit, checkout, merge...), plus `post-index-change` when the
        //    index is written, which GIT_OPTIONAL_LOCKS=0 prevents. Not run by
        //    anything we call.
        //  * pager (`core.pager`, `pager.*`): off via `--no-pager`.
        //  * network helpers (`core.sshCommand`, `credential.helper`,
        //    `http.*`, `core.askPass`): only used by fetch/push/ls-remote.
        //    WHY must not call those (it never needs the network).
        //  * `diff.external`, `diff.<driver>.command`, `diff.<driver>.textconv`
        //    (and `GIT_EXTERNAL_DIFF`): run by diff/log -p/show, none of which
        //    we use yet. WHY-5 (diffs) MUST pass `--no-ext-diff --no-textconv`.
        //  * `gpg.program`: only for signature display/verification, unused.
        //  * `filter.<name>.clean|process` (selected by `.gitattributes`): NOT
        //    covered. `git status` runs the `clean` filter on tracked files
        //    whose stat data changed, to compare their content (verified by
        //    hand on git 2.56: the filter ran under GIT_OPTIONAL_LOCKS=0).
        //    There is no `-c` switch that disables every filter by name, so
        //    this stays a RESIDUAL RISK: it needs a `filter.*` entry in the
        //    repo's own/global config, i.e. the same trust boundary as the
        //    user running `git status` there themselves.
        cmd.arg("-c").arg("core.fsmonitor=false");

        // `-C <dir>`: git changes directory itself. We deliberately do NOT call
        // `Command::current_dir`, because then a missing directory and a missing
        // `git` binary would both surface as `NotFound` from `spawn`.
        cmd.arg("-C").arg(dir).args(args);

        // Caller-chosen variables first, so the protected ones below override them.
        cmd.envs(self.extra_env.iter().map(|(k, v)| (k, v)));

        // WHY is a passive observer. `git status` normally *refreshes the index*:
        // when it notices a file's stat data changed it rewrites `.git/index`,
        // taking `.git/index.lock` while it does so. That races with the coding
        // agent running its own `git add` / `git commit` in the same worktree
        // (the agent's command fails with "Unable to create index.lock: File
        // exists"). `GIT_OPTIONAL_LOCKS=0` tells git to skip every *optional*
        // lock-and-write like that. Status still reports correct results, it
        // just does not cache the refresh. Same effect as `git --no-optional-locks`.
        cmd.env("GIT_OPTIONAL_LOCKS", "0");
        // Never block waiting for a username/password on a terminal we do not have.
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        // C locale: English, stable messages for the few places we match on stderr.
        cmd.env("LC_ALL", "C");

        // These variables, if inherited (e.g. WHY launched from a git hook),
        // override `-C` and make git operate on a different repository.
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_NAMESPACE",
            "GIT_PREFIX",
        ] {
            cmd.env_remove(var);
        }

        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Kill the child if the future owning it is dropped (timeout, or the
            // caller cancelling the task). Without this a dropped `Child` would
            // keep running as an orphan.
            .kill_on_drop(true);
        cmd
    }
}

/// Lossy, space-joined arguments for log and error messages only.
fn summarize_args(args: &[OsString]) -> String {
    args.iter()
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Trimmed, lossy, length-capped stderr for error messages.
fn stderr_excerpt(stderr: &[u8]) -> String {
    let cut = &stderr[..stderr.len().min(STDERR_EXCERPT_BYTES)];
    let text = String::from_utf8_lossy(cut);
    let mut s = text.trim().to_string();
    if stderr.len() > STDERR_EXCERPT_BYTES {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_trims_and_truncates() {
        assert_eq!(stderr_excerpt(b"fatal: nope\n"), "fatal: nope");
        let long = vec![b'x'; 5000];
        let e = stderr_excerpt(&long);
        assert!(e.ends_with('…'));
        assert!(e.chars().count() <= STDERR_EXCERPT_BYTES + 1);
    }

    #[test]
    fn contains_finds_needle() {
        assert!(contains(
            b"fatal: not a git repository (or any)",
            b"not a git repository"
        ));
        assert!(!contains(b"short", b"not a git repository"));
        assert!(!contains(b"", b"x"));
    }

    /// The environment a runner would give git: `Some(v)` = set, `None` = removed.
    fn env_of(runner: &GitRunner, key: &str) -> Option<Option<OsString>> {
        let cmd = runner.command(Path::new("/x"), &[]);
        cmd.as_std()
            .get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .map(|(_, v)| v.map(OsStr::to_os_string))
    }

    #[test]
    fn with_env_reaches_the_child() {
        let r = GitRunner::new().with_env("WHY_TEST_VAR", "1");
        assert_eq!(env_of(&r, "WHY_TEST_VAR"), Some(Some("1".into())));
        assert_eq!(env_of(&GitRunner::new(), "WHY_TEST_VAR"), None);

        let iso = GitRunner::new().with_isolated_config();
        assert_eq!(
            env_of(&iso, "GIT_CONFIG_GLOBAL"),
            Some(Some("/dev/null".into()))
        );
        assert_eq!(env_of(&iso, "GIT_CONFIG_NOSYSTEM"), Some(Some("1".into())));
    }

    #[test]
    fn with_env_cannot_override_the_runners_protections() {
        let r = GitRunner::new()
            .with_env("GIT_OPTIONAL_LOCKS", "1")
            .with_env("GIT_TERMINAL_PROMPT", "1")
            .with_env("GIT_DIR", "/somewhere/else");
        assert_eq!(env_of(&r, "GIT_OPTIONAL_LOCKS"), Some(Some("0".into())));
        assert_eq!(env_of(&r, "GIT_TERMINAL_PROMPT"), Some(Some("0".into())));
        // Removed (`None`), not set.
        assert_eq!(env_of(&r, "GIT_DIR"), Some(None));
    }

    #[test]
    fn concurrency_is_clamped_to_one() {
        let r = GitRunner::with_limits(0, Duration::from_secs(1));
        assert_eq!(r.semaphore.available_permits(), 1);
    }
}
