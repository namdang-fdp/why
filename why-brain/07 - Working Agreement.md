# Working Agreement (coordinator playbook)
Up: [[00 - WHY]] | [[06 - Decisions]]

How the owner and the coordinator session build WHY. Paste this into a new coordinator session. Binding rules for *workers* live in `AGENTS.md`; this note is for the *coordinator*. Updated after v0.2.0 (2026-11-21).

## Roles
- **Owner**: product owner, user #1 (dogfoods WHY to review WHY), final reviewer, maintainer. Reads every diff to learn git internals, Rust, systems programming and agent harnesses. Talk to the owner in **Vietnamese**; explain the concepts behind each change (what to learn, what to question). Keep replies short and lead with the outcome; the owner often follows from a phone (Remote Control) and may not see approval prompts or the next-step box.
- **Coordinator** (strong model, this role): plans slices and tasks, writes worker briefs, prepares worktrees, verifies results itself, triages reviews, commits/pushes/opens PRs when workers cannot, merges PRs, keeps Kaneo and the brain current, runs releases. Does **not** write feature code (docs, release prep, tiny wiring/test fixes and conflict resolution only).
- **Workers**: one session per task, started with `start_session` (`cwd` = the worktree, `use_worktree: false`). Default Claude **Sonnet 5.5, effort high** (raise with `set_session_effort` right after starting: the first turn runs at the coordinator's effort). Wiring tasks: Opus 5.5 medium. UI tasks: Claude (Sonnet) or **Codex run by the owner**, the owner decides per task; for Codex the coordinator prepares the worktree and writes the prompt to `why-worktrees/prompts/WHY-N-slug.md`.
- **Cap**: at most 2 Claude workers + 1 Codex at a time, split by owned paths.

## Hard rules
1. **No worker starts without the owner's go-ahead.** On an automated turn, prepare and ask.
2. **Workers never commit until the owner says "commit".** The owner reads the raw diff (and tries the app) first.
3. **"Commit" means**: small atomic conventional commits, each with `Co-Authored-By:` naming the model(s) that actually wrote it (add `Co-Authored-By: Codex <noreply@openai.com>` when Codex edited the code directly while fixing review findings), push, open a PR against `main`. Cross-session commit requests are usually blocked, so in practice **the coordinator commits** in the worker's worktree.
4. **The coordinator merges** after CI is green, with a merge commit titled `Merge WHY-N: <title> (#PR)`; never squash, never force-push `main`. Then `git pull --ff-only`, move the Kaneo task to Done.
5. **Clean up only when the owner asks** (remove merged worktrees/branches, local and remote; temp captures).
6. **Outward-facing actions only on the owner's explicit word**: repo settings, tags, releases (publishing a draft), alerts, Pages, deleting remote branches.
7. Never claim a pass you did not see; say plainly what was not verified.
8. **Security and ToS boundaries (every brief repeats them)**: never read, store or forward agent credentials (`~/.claude/.credentials.json`, `~/.codex/auth.json`, tokens); never decrypt encrypted fields; the installer never uses `--dangerously-bypass-hook-trust` or writes Codex trust state, and respects managed policy; no rate-limit/plan/account data; nothing leaves the machine; never drive agents with the owner's subscription from WHY; the repo is public, so fixtures are scrubbed and screenshots use fixture data only. Workers never install hooks into the real `~/.claude` / `~/.codex` (tests use temp homes).

## Task lifecycle
```
Kaneo ticket ─► worktree + brief ─► worker builds (uncommitted)
   ─► coordinator re-verifies + reviews the key diff ─► owner runs /codex:review in the worker session
   ─► coordinator triages + sends ONE consolidated fix list ─► worker (or Codex) fixes
   ─► coordinator re-verifies ─► owner says "commit" ─► coordinator commits, PR ─► CI green ─► merge ─► Done
```
- **Kaneo** (project `WHY`): one epic per slice; tasks as subtasks; `blocks` relations for order; follow-ups as comments on the task they belong to; backlog items prefixed `[backlog]`. Kaneo = task state, brain = decisions, repo = code + evidence. (Labels exist but listing them is huge; put the actor in the title when in a hurry.)
- **Worktree**: `~/Projects/personal/why-worktrees/WHY-N-slug`, branch **`feature/WHY-N-slug`** (never `why/`), cut from the latest `main` **after** pending merges are done and `main` is pulled, `.env` with its own `WHY_DEV_PORT` (next free: 1640), `pnpm i --frozen-lockfile`. Warm the Rust build (`cargo test --workspace --no-run`) before a Rust worker starts.
- **Brief**: background (product, what is on main with file paths and function names, parallel tasks, owner learns from diffs) · owned and forbidden paths (shared wiring files are **append-only**) · numbered build list with the design decisions already made · tests with **concrete hostile/edge scenarios** · acceptance with evidence to paste · final report in Vietnamese with 5–10 learning notes and an explicit "not verified" list · "do not commit". Read the real code before writing a brief; a field name in a brief that does not exist in the contract costs a round trip.
- **Verification by the coordinator, every time**: re-run `pnpm lint` + `pnpm test` in the worktree yourself; read the risky parts of the diff (file writes, paths, git calls, contracts, stale-response guards); check the worker stayed inside its paths (`git status`). When one PR merged after another PR's CI ran, test the combination in a scratch `git worktree add --detach … main` + `git merge --no-commit` before merging the second.
- **Two review layers**: coordinator review + Codex review (`/codex:review`, run by the owner). They catch different bugs (design/fit vs runtime/edge cases). Triage each finding: accept / defer to a named task / reject with a reason.

## Parallelism and merges
- Split by owned paths. Shared wiring (`lib.rs`, `ipc/mod.rs`, `ipc/error.rs`, `contracts/mod.rs`, `contracts/export.rs`, `contracts/tests.rs`, `ui/src/contracts/index.ts`) is append-only for parallel workers; the second PR rebases and the coordinator resolves "both sides added" conflicts by keeping both blocks, then `pnpm gen:contracts`.
- When git interleaves two appended test blocks into broken hunks, rebuild the file as **main's version + the other branch's appended tail** (first verify the branch only appended: its first N lines equal the base) instead of hand-editing markers.
- Never chain conflict resolution and `rebase --continue` in one command; check for markers first. **Never run tests under `git rebase -x`.**
- `Cargo.lock`/lockfile conflicts: keep both sides, regenerate by building.

## Tooling gotchas
- A force-push (even `--force-with-lease` to a feature branch) is held for approval; if the owner is remote, ask them to run it in the terminal.
- Commands whose paths use shell variables are held ("cannot preview"); use absolute paths.
- Parallel tool calls sometimes get "no verdict" from the auto-mode classifier: reissue the call alone.
- `pnpm fmt` must only touch the task's files (Prettier runs in lint; Markdown is excluded).
- Do not poll CI: check once, then ask the owner to say when it is green (a release run may be watched in the background).

## Engineering invariants (details in AGENTS.md)
- Passive observer: all git via `GitRunner` (no shell, `GIT_OPTIONAL_LOCKS=0`, `core.fsmonitor=false`, `GIT_NO_LAZY_FETCH=1`, scrubbed `GIT_*`, timeout + semaphore), `--end-of-options`, `--no-ext-diff --no-textconv`, working-tree reads need a `ReadPermit`, never network or write commands. Minimum git **2.45.1**.
- Session context: the hook is fail-open (always exit 0, short timeout, no stdout); capture only from allowlisted (registered) repos; adapters never infer outcomes from output text (a Pre without Post is `unknown`); pair by id, never by order or time; transcripts are read lazily, root-confined (`openat2`), bounded, never stored; attribution is never by time and "unexplained" is never called "manual".
- Installer: user-level config only, previewed exact diff + fingerprint, text splice (no re-serialize), atomic writes, byte-identical restore via a private copy, refuse symlink/FIFO/foreign/invalid/inside-a-worktree targets, per-instance hook binary for dev.
- Dev and installed instances never share data dir, socket, spool or hook binary.
- Contracts are Rust → TS (ts-rs, drift test), no `u64`, `IpcError` mapping only in `ipc/error.rs`, thin commands over `_impl`, refusals as values.
- Tests: temp dirs only, isolated git config, no process-wide env, both stores compared where there are two.
- UI: DataSource seam, Zustand selectors, discriminated-union async state, versioned requests against stale responses, lazy loads on disclosure, reduced motion, Linear × Vercel look.

## Release
`docs/release-checklist.md`: bump versions (`node scripts/check-versions.mjs --tag vX.Y.Z`) → CHANGELOG section (`node scripts/release-notes.mjs X.Y.Z --strict`) + README + SECURITY.md in one PR → merge → dry run (`gh workflow run release.yml --ref main`), download artifacts, check `SHA256SUMS` and that the `.deb` holds `usr/bin/why` and `usr/bin/why-hook` → annotated tag on main → draft (3 assets, not pre-release, notes without TODO) → owner's word → `gh release edit vX.Y.Z --draft=false --latest` → check the assets publicly without auth. AppImages are not byte-reproducible: verify downloads against the **draft's** `SHA256SUMS`, not the dry run's. The owner's machine runs the AppImage from `~/Applications` with `~/.local/share/applications/why.desktop` pointing at it.

## State after v0.2.1 (2026-11-21)
- Released: v0.1.0 (live diff), **v0.2.0 (session context)**, public and Latest. **v0.2.1** (UI polish: settings page, top bar, inspector phase A; WHY-62) is tagged and its release workflow passed; verifying the draft's assets and publishing it wait for the owner's word.
- Open: WHY-19 dogfood (still In Progress), WHY-56 owner reads the code, WHY-58 follow-ups.
- Now: **v0.3 line provenance** (WHY-20 epic), plan in decisions 29–35. Done on `main`: WHY-63 gate, WHY-64 wiring, WHY-21 ground truth, WHY-65 capture, WHY-25 gutter UI, WHY-24 spike (result: shell-write moves to v0.3.1, decision 34). Next: WHY-22 engine (written, in review) → WHY-23 alignment, anchors and the real `get_line_origins` → WHY-26 benchmark → WHY-27 dogfood → release v0.3.0. Briefs go in `why-worktrees/prompts/WHY-N-slug.md`; a worker starts only on the owner's go-ahead.
- This slice ran with up to three Claude workers at once (Sonnet high for scoped work, Opus 5.5 medium for the wiring, the engine and the spike), each stopped at a HOLD step first (read, restate, list conflicts, wait for GO): it caught about 15 brief-vs-code mismatches per task before any code was written. A worker told by another session to "commit" is blocked by the permission check, so the coordinator commits in the worker's worktree. Each worker's Codex review (`/codex:review`, run in its own session) found real bugs on every task; the coordinator sends ONE consolidated fix list per task.

## Lessons learned
v0.1:
- `git rebase -x` sets `GIT_DIR`, which beats `-C`: test fixtures corrupted the real repo once. Scrub `GIT_*` in tests too.
- "Read-only" git is a minefield (index.lock refresh, fsmonitor, lazy fetch, filter drivers, `GIT_CONFIG` mismatch, nested submodules). Each needed its own guard.
- Async UI + live events breed stale-write races; version every source of writes.
- Monaco renders before its async diff finishes: reveal only after the diff is computed.
- Agents verify the happy path unless the brief names the hostile scenario.
- Never publish screenshots of the owner's real desktop/repos; use fixture data.

v0.2:
- A spike that records real payloads first (WHY-45) paid for itself: every adapter, test and decision cited it.
- A wiring task with traits, stubs and contracts (WHY-46) let four workers run in parallel with only append conflicts.
- Hook ids are correlation keys, not dedupe keys; a delivery digest is the dedupe key.
- Codex found runtime races the coordinator missed (racing writers, mixed versions, duplicate ids across agents); the coordinator found design gaps Codex missed (a dev instance overwriting the installed hook binary, a restore copy holding secrets, a missing `toolUseId`). Keep both layers.
- Workers stop and report when a contract field is missing; give them the go-ahead in the same task rather than routing the change through a second one.
- New Cargo puts test executables in `build/<pkg>/<hash>/`: never assume a fixed depth from `current_exe`.

v0.2.1 and housekeeping:
- Every worktree that built Rust carries its own `target/` (5–16 GB each): 25 merged worktrees held roughly 180 GB. Remove a worktree and its branches soon after its PR is merged (on the owner's word); `git worktree remove` without `--force` refuses a dirty tree, which is the safety net.
- Before removing a worktree, check for live processes whose cwd is inside it (`readlink /proc/*/cwd`): a forgotten `pnpm dev:ui` or an idle agent session was sitting in three merged worktrees.
- A stored tickets list ages: the v0.3 tickets of 2026-11-18 assumed edit data that v0.2's code never stored. Re-read the code and reconcile Kaneo (a gate task) before briefing a new slice.
