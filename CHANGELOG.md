# Changelog

All notable changes to WHY are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The release workflow copies the section for the tag being released (for example `## [0.1.0]`) into
the GitHub release notes, so keep each version section self-contained. Details are in
[`docs/release-checklist.md`](docs/release-checklist.md).

## [Unreleased]

## [0.3.0] - 2026-11-22

Line provenance: click a line in the diff and WHY tells you which agent edit wrote it, or that it
does not know. Every answer is labelled by how well it is backed (**Observed** or **Unknown**), and
**Unknown** is shown, never hidden. Linux only, as before.

**How far to trust it:** the replay engine is validated against 40 hand-written scenarios
(Claude Code and Codex edits, uncaptured changes, ambiguous patches). Its accuracy on real sessions
has **not yet been measured**: a benchmark on real sessions follows this release. Treat an
**Unknown** as a reason to look, and report any line whose origin is wrong.

### Added

- **A line-origin gutter in the diff.** On the modified side of a file's diff, one mark per run of
  lines: a stable colour per agent turn, nothing for lines unchanged since the base, and a hatched
  mark in the risk colour for lines WHY cannot attribute. Hover a mark for the harness, session,
  turn and prompt, or for the reason a line is unknown in plain words ("No captured agent edit
  wrote this line": it may be a person, a shell command or a lost event; WHY does not guess). The
  gutter is drawn only when the right side of the diff is the working-tree file and its line count
  matches, so a mark never lands on text it does not describe.
- **Line to inspector.** Click a mark, or press `o` on a line (`.` and `,` jump to the next and
  previous unknown run), and the _File_ tab shows the origin chain: which session and turn, the
  prompt, the tool call, with a link to the turn in the timeline, and the agent's own reason on
  demand (**Declared**, never shown as if it were the evidence). A per-file coverage line counts
  lines explained by Claude Code and by Codex **separately**, plus unchanged and unknown lines.
- **A replay engine.** WHY rebuilds each file's per-line origins from the edits the hooks recorded
  (Claude Code `Edit` and `Write`, Codex `apply_patch`). It checks every Claude edit against the
  file as the agent saw it, resyncs when something else changed the file, places a Codex patch by
  its context and keeps an ambiguous placement as Unknown instead of guessing, and aligns the
  result with the real file. A line is never attributed because its text equals another line's, and
  never by time.
- **Exact edit capture.** For every captured edit, WHY now stores per-line **hashes**, counts and
  positions in its event database (**no line text**). See "Changed" and `SECURITY.md`.
- **Edits belong to the edited file's worktree.** A session whose working directory is one
  worktree but which edits a file in another (by absolute path) now shows up in both, and the edit
  is recorded for the worktree that owns the file. A file inside a nested repository is no longer
  attributed to the outer worktree.
- **The Codex hook card shows "Receiving events"** with the time of the last event once WHY has
  seen one, instead of asking you to review the hook forever. "Spool files waiting" reads 0 when
  the spool directory does not exist yet.

### Changed

- **The event database moves to schema v3 the first time v0.3 starts.** Older versions cannot read
  it: they refuse the file and fall back to an in-memory store, so their sessions are not saved.
  Back up `events.db` in WHY's data directory before upgrading if you may go back.
- **An oversized hook payload keeps the edited file's path** (and only that), so the edit can still
  be filed; a payload that names an agent credential file is redacted instead, even when the file
  is in a custom config directory.
- **A Codex patch containing a context line equal to `*** End Patch` is no longer cut short.** This
  also affected the v0.2 timeline.

### Security

- **WHY now stores line hashes of the edits it records**, in the same local database and under the
  same 30-day retention as the timeline. They are not line text, but they are **not secret**:
  someone with access to the file can test a guess, so a short or simple line (say a one-line
  `.env`) can be confirmed. Nothing leaves your machine. Details in `SECURITY.md`.
- `get_line_origins` reads the file's base version by blob id and the working-tree file through the
  same filter-driver guard and no-symlink reads as the diff; it never runs a repository's filter
  drivers.

### Known limitations

- **Accuracy on real sessions is unmeasured** (see above). Fixtures only so far.
- **Edits made by shell commands are not attributed** (`sed -i`, formatters, code generators): the
  lines show as Unknown. A design that fits the hook's latency budget is planned for v0.3.1.
- **Codex coverage is lower than Claude Code's.** A Codex patch carries context but no line numbers
  and no original file, so an ambiguous patch gives Unknown lines. WHY reports the two harnesses
  separately.
- **Whether a file ends with a newline is unknown** after an edit that reaches its end; this does
  not change which line is whose.
- **Only edits recorded while the hooks were installed and the repository registered** can be
  attributed, and only for 30 days. Older changes show as unknown.
- **Each request reads git status for the whole worktree** to find the file, so opening a file in a
  very large repository can be slow.
- **The session timeline does not highlight a file edited in another worktree** than the session's
  own (the line origin and the rail do).
- Linux only, unsigned builds, no auto-update, needs Git 2.45.1 or newer (unchanged).

## [0.2.1] - 2026-11-21

Polish from the first dogfood of v0.2.0: a settings page, a calmer top bar, and a Why inspector that
answers "why does this file look like this, and how far can I trust that?" first. This is a UI
release: it changes nothing about what WHY captures, how hooks are installed, or what it reads from
your repositories. Linux only, as before.

### Added

- **A settings page.** A gear at the bottom of the left rail (or `Ctrl/Cmd+,`) opens a full settings
  page with a sidebar: **Editor** (font, size, line height, ligatures, whitespace and theme, with a
  live preview) and **Agent hooks** (the same install, review and uninstall flow as before, moved
  here from a dialog). It replaces the top-bar _Agent hooks_ button and the editor-settings popover,
  and the command palette entries open the right section. Settings are still stored locally in the
  app, as before.
- **A worktree overview in the inspector** (the new _Worktree_ tab). It shows the task as the
  verbatim first prompt of the latest session that has one, or "No task context captured" (WHY never
  guesses a task from a branch name); the changed-file and session counts; the added and removed
  lines; and an **evidence coverage** bar: how many changed files have a captured agent edit, only
  an unconfirmed attempt, or none. A control lists the files without an applied edit, the same set
  as the rail's _Unexplained_ filter.
- **A turn strip** above a session's turns, marking turns with edits, test runs and failures. A very
  long session is grouped into at most 80 segments.

### Changed

- **Selecting a worktree opens its first changed file**, or the file you last had open there,
  instead of an empty "Select a file". Keyboard navigation in the rail keeps working and focus does
  not move.
- **The inspector is file-first.** The _File_ tab shows one evidence chip (explained by N captured
  edits, an unconfirmed attempt, or no captured agent edit) and the captured edits behind it, and
  no session log. In a session, the start and end times and the Codex exit-code note are collapsed
  under _Session details_; runs of three or more shell or tool calls fold into one row (by the kind
  WHY recorded, never by guessing from the command text); and outcome and evidence chips appear only
  when they add information. A session in which no outcome was recorded says so once, instead of
  repeating "unknown" on every row. _Why?_ on a tool call shows on hover or focus.
- **A calmer top bar.** The logo mark replaces the "WHY" wordmark and Search moves to the left. The
  repository and worktree names live in the rail, and the base is a compact button next to the diff
  modes. The `?` shortcuts dialog has two columns.

### Known limitations

- **Linux only**, unsigned builds, no auto-update (unchanged). Needs Git 2.45.1 or newer.
- **Edits made by shell commands are not attributed.** Codex in particular often edits files
  through shell commands, so its files can show "No captured agent edit". Capturing shell writes
  is planned for v0.3.
- **A session is attributed to the worktree of its working directory.** If an agent started in one
  worktree edits a file in another by absolute path, the other worktree shows no evidence for it.
- **The Codex card keeps asking you to review the hook in `/hooks`**, because WHY cannot read
  Codex's trust store, even after you approved it. The _Agent hooks_ status can also read "Spool
  files waiting: unknown" until the spool directory exists.
- **File-level attribution only.** A file explained by an agent turn may still hold changes WHY did
  not see; line-level provenance is planned for v0.3.

## [0.2.0] - 2026-11-21

Session context: WHY now sees what Claude Code and Codex did in each worktree, turn by turn, and
shows which agent turns explain each changed file. Every piece of it is labelled by how well it is
backed: **Observed** (the harness recorded it), **Declared** (the agent's own words) or **Unknown**
(not enough evidence). Linux only, as before.

### Added

- **Agent hooks, installed with your consent.** A new _Agent hooks_ dialog (top bar, command
  palette, or the empty session list) adds WHY's hook to your **user-level** Claude Code
  (`~/.claude/settings.json`) and Codex (`~/.codex/hooks.json`) config. You see the exact diff
  first, and nothing is written until you confirm. WHY edits only its own lines (the rest of the
  file stays byte for byte), refuses symlinked, invalid or foreign-owned files, and never touches
  project-level config. Uninstall restores your original file exactly when you have not edited it
  since, and otherwise removes only WHY's entries. Codex asks you to approve a new hook in its own
  `/hooks` screen; WHY never bypasses that, and it respects managed (enterprise) policy.
- **A fail-open hook and a local collector.** The `why-hook` binary ships with the app. Agents run
  it on every event; it forwards the event over a local Unix socket and **always exits 0 within a
  short timeout**, so a slow, closed or broken WHY can never stall or fail your agent. Events made
  while WHY is closed are spooled on disk and arrive when it starts. Only repositories you registered in
  WHY are captured; everything else is dropped by the hook itself.
- **Claude Code and Codex adapters.** Hook payloads become one event model: prompts, tool calls
  paired start-to-finish by id, edits with the files they touched (Claude `Write`/`Edit`, Codex
  `apply_patch`), shell commands, test runs, subagents. A tool call that started but never reported
  a result is shown as **unknown**, never as success. A Claude edit the harness rejected is kept as a
  failed attempt.
- **Session timeline in the inspector.** Each worktree lists its agent sessions (harness, model,
  task, status, turn count); a session opens as a virtualized timeline of turns and events that
  updates live. Selecting a file highlights the turns that touched it and the other way round.
- **Which turns explain each changed file.** The changed-files list marks each file as explained by
  one or more agent turns, as _Agent tried, not applied_, or as _No captured agent edit_ (WHY never
  guesses who made such a change: a human, a shell command or a lost event all look the same). A
  worktree also lists the turns that ran shell commands there, as leads, because what a shell
  command changed is only known to git. Attribution is never made by time.
- **"Why?" on demand.** Each turn and tool call can show what the agent said it was doing,
  labelled **Declared**. WHY reads it from the harness's own transcript only when you open it, never
  copies it into its database, and tells you plainly when the transcript is gone or unreadable.
- **Recovered Codex outcomes.** Codex hooks carry no exit code, so shell commands start as
  _unknown_. When you open a Codex session WHY reads the exit code from the structured record in
  Codex's own session log and shows, for example, _failed · exit 3 · from rollout_ (Observed). It
  never infers an outcome from command output.
- **Dev builds stay apart from the installed app.** A `pnpm dev` run uses its own data directory,
  socket, spool and hook binary, so developing WHY cannot disturb the copy you use.

### Changed

- `pnpm lint` also checks formatting with Prettier. The build (`pnpm build`) bundles `why-hook`.

### Known limitations

- **Linux only**, unsigned builds, no auto-update (unchanged). Needs Git 2.45.1 or newer.
- **File-level attribution only.** A file explained by an agent turn may still hold changes WHY did
  not see; line-level provenance is planned for v0.3.
- **Codex hook approval cannot be confirmed by WHY**: it cannot read Codex's trust store, so the
  dialog keeps reminding you to approve the hook in `/hooks`.
- **Long Claude transcripts** are read from the start up to a budget (about 16 MiB), so "Why?" for
  a tool call late in a very long session may come back empty and marked as truncated.
- **A config directory inside a git worktree is refused** (for example a `$HOME` that is itself a
  git repository), so hooks cannot be installed there.
- Session context was tested against Claude Code 2.1.322 and Codex 0.170.0. Newer versions are
  read, but WHY says when a transcript's version is one it has not checked, and recovers no Codex
  exit codes from an unchecked version.

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

[Unreleased]: https://github.com/namdang-fdp/why/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/namdang-fdp/why/releases/tag/v0.2.1
[0.2.0]: https://github.com/namdang-fdp/why/releases/tag/v0.2.0
[0.1.0]: https://github.com/namdang-fdp/why/releases/tag/v0.1.0
