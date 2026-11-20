# Changelog

All notable changes to WHY are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The release workflow copies the section for the tag being released (for example `## [0.1.0]`) into
the GitHub release notes, so keep each version section self-contained. Details are in
[`docs/release-checklist.md`](docs/release-checklist.md).

## [Unreleased]

## [0.1.0] - 2026-11-20

First release: a read-only desktop viewer that shows the live diff of every repository and git
worktree your coding agents are working in. Linux only for now (AppImage and `.deb`).

### Added

- **Repositories and worktrees.** Add a repository with the native folder picker (or the command
  palette, or `a`) and WHY lists all of its linked git worktrees. Worktrees are labelled by branch,
  show a status dot that pulses when something changes, group clean ones under "N clean", and can
  be filtered with `/`. A worktree git cannot read is listed with its error instead of breaking
  the list, and worktrees an agent adds or removes show up without a restart. Removing a repository
  only forgets it in WHY; nothing on disk is touched.
- **Three diff modes per worktree.** _Vs base_ (everything since the branch left its base branch),
  _Uncommitted_ (working tree against `HEAD`) and _Staged_ (the index against `HEAD`). New
  untracked files show up as additions. _Vs base_ falls back to _Uncommitted_ for a worktree that
  has no base.
- **Automatic base detection, with an override.** The base is the merge-base with the best
  candidate among `origin/HEAD`, `origin/main`, `origin/master`, `main` and `master`: the one the
  branch is the fewest commits ahead of, so work already merged into a local `main` is not shown as
  the agent's. You can pick another base per worktree from a popover that suggests branches and
  validates your input; the choice is remembered.
- **Monaco diff view.** A read-only, self-hosted Monaco editor (no CDN, no network) with split and
  unified layouts, keyboard navigation across files and hunks, and previews for non-text changes.
- **Live updates.** WHY watches your worktrees (native file events, with a polling fallback). When
  an agent edits, stages or commits, the file list, change counts and the open diff refresh in
  place without losing your scroll position, and changed rows flash briefly.
- **Keyboard and command palette.** `j`/`k` for files, `[`/`]` for worktrees, `1`/`2`/`3` for the
  diff modes, `i` for the inspector, `Ctrl+K` (`⌘K`) for the palette that jumps to repositories,
  worktrees and files, and `?` for the shortcut list.
- **Editor settings.** Font, font size, line height, ligatures, whitespace rendering and theme
  (WHY Dark and a Tokyo-Night-inspired dark theme).
- **Persistence.** Registered repositories and base overrides are stored in a local SQLite
  database in the app data directory, and panel layout is restored on launch. If the database
  cannot be opened, WHY runs on a temporary in-memory store and says so in a banner.
- **Polish.** First-run onboarding, resizable panels, and short transitions that respect
  `prefers-reduced-motion`.

### Security

WHY is a passive observer: it runs `git` against repositories agents are using and must never
disturb them.

- Read-only and offline: WHY never runs a command that writes to a repository or touches the
  network, and it keeps partial clones from downloading objects on demand
  (`GIT_NO_LAZY_FETCH=1`).
- All git runs through one runner: argument vectors (never a shell), a global cap on concurrent
  processes, and a per-call timeout.
- It does not take locks an agent needs: `GIT_OPTIONAL_LOCKS=0` for `git status`, and diff commands
  never refresh (rewrite) the index.
- A repository's own configuration is treated as untrusted: `core.fsmonitor` is forced off, and
  diffs run with `--no-ext-diff --no-textconv`, so merely looking at a repo does not run programs
  it names.
- Working-tree files are opened without following symlinks.
- The webview has no filesystem or dialog permissions; the folder picker is a command in the Rust
  core that returns only the chosen path.
- Git filter drivers need your approval. A repository's own config can define filter drivers
  (`filter.<name>.clean` or `process`, picked by `.gitattributes`) that git runs while it reads
  files, so merely reading a repository could run a program an agent wrote there. Before every read
  WHY checks the worktree's filter configuration and refuses to read it until you choose "Trust and
  continue" in a panel that lists each program, its full command and where it is defined. Trust is
  stored per driver as a fingerprint, so a new or edited driver is asked about again; you can stop
  trusting a repository later. Drivers from your own git config (for example git-lfs) are allowed
  without asking.
- The legacy `GIT_CONFIG` variable is removed from every git call, `git status` no longer descends
  into dirty submodules (their own filters would run), and WHY re-reads a repository when its git
  config or `info/attributes` changes.

### Known limitations

- **Linux only.** The author develops and tests on Linux, and the release ships only an AppImage
  and a `.deb`. macOS and Windows are untested and have no builds.
- **Git 2.45.1 or newer is required.** Older versions cannot reliably honour
  `GIT_NO_LAZY_FETCH`, so WHY reports an error instead of reading the repository. Some current
  distributions ship an older git (for example Ubuntu 22.04 and 24.04, and Debian 12).
- **The "Why" inspector is a placeholder.** Session context, provenance and the reasons behind each
  change arrive in v0.2 and later; v0.1 is the live diff only.
- **The filter-driver guard has limits.** A process that can write a repository's git config while
  WHY is open can still race the check, and trust covers the configured command string (a command
  that points at a file inside the repository trusts whatever that file later contains). See
  [`SECURITY.md`](SECURITY.md) for these and the other limits.
- **Unsigned builds.** The release assets are not code-signed and WHY has no auto-update. Check the
  download against `SHA256SUMS`.

[Unreleased]: https://github.com/namdang-fdp/why/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/namdang-fdp/why/releases/tag/v0.1.0
