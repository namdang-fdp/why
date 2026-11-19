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
- `core.fsmonitor=false` is forced on every call, because a repo-local `core.fsmonitor` can name a
  program that git runs during `git status`.
- Diff commands must pass `--no-ext-diff --no-textconv` so repo-configured diff drivers cannot run
  programs, and user-supplied revisions go after `--end-of-options` so they cannot be read as flags.
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

## Known residual risks

- **Git filter drivers.** `filter.<name>.clean` and `filter.<name>.process` commands, selected by
  `.gitattributes`, can still run during `git status`. The runner's other protections do not stop
  this. A repository that defines such a driver could therefore execute a program on the machine
  when WHY reads its status. Until this is mitigated, treat adding a repository to WHY like
  running `git status` in it, and do not add repositories you do not trust.
- **Early code.** Several parts of the core are still being written, and the app has had no
  external security review.
