# Security Policy

## Supported versions

WHY is early software (pre-1.0) and has no stable release yet. Only the latest commit on `main`
and, once releases exist, the latest release receive security fixes.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Use GitHub's private vulnerability reporting:
[report a vulnerability](https://github.com/namdang-fdp/why/security/advisories/new).

Include what you found, how to reproduce it (a minimal repository or a sequence of git commands is
ideal), and what you think the impact is. WHY is maintained by one person in their spare time, so
responses are best effort: expect an acknowledgement within about a week, and a fix or a decision
as soon as is practical. Please give the maintainer a reasonable chance to fix the issue before you
disclose it publicly. Reports are credited in the advisory unless you ask otherwise.

## Threat model

WHY is a **passive observer**. It runs `git` against repositories on your machine that coding
agents are actively working in, and it must neither harm those repositories nor run code they
contain.

**Trust boundaries.** WHY trusts the local user. It does **not** trust the contents of the
repositories it observes: their config, `.gitattributes`, file names, file contents and hooks are
all untrusted input, because an agent (or a malicious dependency or prompt injection steering an
agent) may have written them. Opening a repository in WHY should never execute that repository's
code.

**What WHY does to defend itself:**

- All git runs through one runner: arguments as a vector, never a shell, with bounded concurrency
  and a timeout.
- `GIT_OPTIONAL_LOCKS=0`, so reading status never takes `index.lock` and cannot interfere with an
  agent's own git operations.
- `GIT_NO_LAZY_FETCH=1` prevents fetching missing objects from a partial clone. The runner
  requires Git 2.45.1 or newer before it runs repository commands.
- `core.fsmonitor=false` is forced on every call, because a repo-local `core.fsmonitor` can name a
  program that git runs during `git status`.
- Diff commands must pass `--no-ext-diff --no-textconv` so repo-configured diff drivers cannot run
  programs, and user-supplied revisions go after `--end-of-options` so they cannot be read as flags.
- **Git filter drivers are guarded, not just forced off.** See "Git filter drivers" below.
- WHY does not run network git commands (`fetch`, `pull`, `push`, `clone`, `ls-remote`), does not
  write to the repositories it observes, and its own code makes no network requests.
- The webview runs under a Content Security Policy and Tauri's capability system (see
  `src-tauri/tauri.conf.json` and `src-tauri/capabilities/`).

**In scope** (please report):

- Any way for a crafted repository, worktree, branch name, config, `.gitattributes`, file name or
  file content to make WHY execute code, write to a repository, or read files outside the
  repository it was pointed at.
- Any way for WHY to break or block a running agent's git operations (for example by holding
  `index.lock`).
- Flags or option injection through revisions, refs or paths passed to git.
- Path traversal, symlink or canonicalisation problems in IPC commands that accept paths.
- Webview or IPC issues: a content-security-policy bypass, or a way for page content to call
  privileged commands it should not reach.
- Leaks of local data (for example agent transcripts, once WHY stores them) to anywhere outside
  the machine.

**Out of scope:**

- Attacks that already require the attacker to run code as your user, or to modify WHY's binary or
  its local data directory.
- Vulnerabilities in git itself, Tauri, the OS webview or other dependencies. Please report those
  upstream; if WHY can mitigate one, tell us.
- Findings about the roadmap items that do not exist yet (hooks, collector, provenance engine).

## Git filter drivers

**The risk.** A *filter driver* is a pair of config entries plus an attribute:
`[filter "x"] clean = <program>` in `.git/config` (or `process`, the long-running variant) and
`* filter=x` in `.gitattributes`. While git decides whether a tracked file changed it runs the
program on the file. Measured on git 2.56, these commands do it: `status`, `diff` against the
working tree, `diff-files`, `diff-index` without `--cached`, `ls-files -m` and `hash-object`
(without `--no-filters`). These do not: `diff --cached`, `cat-file`, `rev-parse`, `show`,
`worktree list`, `ls-files -c -i` (the watcher's call). Nothing like `core.fsmonitor=false` turns
filters off for a whole invocation, so the runner cannot fix this alone. Anything that can write a
repository's config (an agent, a build script, a prompt-injected tool) could therefore make WHY run
a program just by *looking* at the repository.

**What WHY does.** Every call that reads the working tree is guarded (`git/filters.rs`):

1. WHY asks git for the `filter.*` configuration of the worktree about to be read, together with the
   config scope each setting comes from (`git config --show-scope --show-origin`). The *effective*
   value counts, so a repository that overrides the `clean` command of a driver you defined globally
   is caught too.
2. Settings from your own config files (system and global) are trusted like running
   `git status` yourself. Typical case: git-lfs. WHY shows a quiet note and reads normally.
3. Settings from the repository's scopes (`.git/config` and files it includes, a linked worktree's
   `config.worktree`), from WHY's own process environment (`GIT_CONFIG_COUNT/KEY_n/VALUE_n`,
   `GIT_CONFIG_PARAMETERS`: a repository can influence the shell WHY is started from, for example
   through `direnv`) or from a scope WHY does not recognise are **untrusted**: WHY does not read
   that repository, shows it as *blocked*, lists each program and where it was defined, and asks.
   "Trust and continue" stores a fingerprint of the exact configuration; a new driver, or a driver
   whose command changed, is asked about again. Trust is revoked from the repository's menu, and
   removing the repository forgets it, even if a trust confirmation was in flight (the write is
   conditional on the repository still being registered). The confirmation lists every program in
   full and trusts exactly the report it showed; if the configuration changes while it is open, it
   closes and asks again. A scan that fails (git error, unparseable output) blocks the
   read; only git's plain "no filters configured" answer means "none".
   The legacy `GIT_CONFIG` variable is removed from every git call: `git config` (the scan)
   honours it but `status` and `diff` (the read) do not, so it could make the scan see a different
   configuration than the read.
4. The functions that read the working tree require a `ReadPermit` that only the guard can create,
   so the compiler stops new code from skipping it.
5. `git status` runs with `--ignore-submodules=dirty`. Without it git starts a nested `status` in
   each submodule and runs the filters configured in the *submodule's* own config, which the scan
   does not cover. (Verified: such a driver ran on an outer `git status`.)
6. The file watcher re-reads a repository when its `config`, `config.worktree` or
   `info/attributes` change, so a driver added behind WHY's back is noticed on the next read.

**What this does not cover.**

- **A race.** The scan and the git command that follows are two steps. A process that can write the
  repository's config can add a driver in that gap, and one that retries in a loop will eventually
  win. The guard narrows the gap (it scans immediately before the read) but cannot close it: git has
  no way to read "the configuration I just checked". Treat a repository an attacker can write to
  while WHY is open as not fully protected.
- **What you trusted can change.** Trust covers the configured *command string*. If it is a path
  inside the repository (`./scripts/scan.sh`), whatever that file later contains will run, and
  WHY does not notice. The trust dialog says so; trust only commands you recognise.
- **Your own drivers.** A driver in your global or system config is trusted by construction, even if
  a repository's `.gitattributes` selects it for files you did not expect. A global command written
  as a relative path would run whatever is in the repository.
- **Remove and re-add inside one confirmation.** The trust write only checks that the repository is
  still registered. Removing and re-adding the same path while a confirmation is in flight (ids
  come from the path) cannot be told apart from the original registration.
- **Not a sandbox.** A trusted program runs with your permissions.
- Overriding the drivers with `-c filter.<name>.clean=` instead of blocking was evaluated and
  rejected: it needs every name (names containing `=` cannot be passed with `-c`), also needs
  `required=false` per driver (git otherwise aborts), is still racy, and shows wrong results for
  converting drivers such as git-lfs (every file would look modified).

## Known residual risks

- **Git filter drivers: mitigated, with limits.** See "Git filter drivers" above for what is
  guarded and for the race and command-path limits that remain.
- **Early code.** Several parts of the core are still being written, and the app has had no
  external security review.
