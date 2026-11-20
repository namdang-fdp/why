# AGENTS.md: worker rules for WHY

WHY is a local-first desktop app (Tauri 2 + Rust core, React 19 + TypeScript UI) for humans reviewing code that coding agents wrote across many repos and git worktrees. It is a **passive observer**: it reads repositories where agents are working and must never disturb them.

- Design notes and decisions: `why-brain/` (Obsidian vault; start at `00 - WHY.md`, then `02 - Architecture and Stack.md`, `06 - Decisions.md`). **Read-only for workers.**
- Task state lives in Kaneo (project `WHY`), not in the repo. Your brief names the task and the paths you own.
- The owner reviews every diff in depth to learn Rust and systems programming: write clear, idiomatic code with short comments on non-obvious choices (the *why*, not the *what*).

## Layout

- `src-tauri/src/`: Rust core. `git/` (all git access), `store/` (persistence trait + backends), `ipc/` (Tauri commands), `state.rs` (shared `AppState`), `contracts/` (IPC types), `watch/`.
- `ui/`: React frontend (Vite root). Rust workers do not touch it; UI workers do not touch `src-tauri/`.
- `ui/src/contracts/` is **generated** from `src-tauri/src/contracts/` by ts-rs. Never hand-edit it; run `pnpm gen:contracts` and commit the result. A test fails if they drift.

## Workflow

- One task = one worktree `why-worktrees/WHY-N-slug` + branch `feature/WHY-N-slug` (prefix `feature/`, never `why/`) + its own `WHY_DEV_PORT` in `.env`. Work only in that folder.
- Stay inside the paths your brief says you own. If you need a change elsewhere (especially in `contracts/`), stop and report instead of editing.
- **Do not commit, push, stash or switch branches until told to.** When told: small atomic commits with conventional messages (`feat(git): ...`), each ending with a `Co-Authored-By:` trailer naming the model that actually wrote the code; then push and `gh pr create` against `main`.
- Never merge a PR, force-push `main`, or delete branches/worktrees.
- Other agents work in parallel. Do not reformat or rename code outside your task.

## Verification

- Before reporting: `pnpm lint` and `pnpm test` must pass (they run clippy `-D warnings`, `cargo fmt --check`, eslint, `tsc`, cargo tests, vitest). Run `pnpm fmt` to fix formatting.
- Report what you did **not** verify (for example "did not launch the app on macOS"). Never claim a pass you did not see.

## Rust rules

- **Async:** Tauri owns the tokio runtime. Never use `#[tokio::main]` or build a second `Runtime`; spawn with `tauri::async_runtime::spawn`. Use `tokio` for types only (process, sync, fs, time).
- **Errors:** library errors are `thiserror` enums. `anyhow` is for app-level glue only. No `unwrap`/`expect` on fallible runtime paths (app startup and tests are fine).
- **IPC errors:** `GitError`/`StoreError` become `IpcError` only in `ipc/error.rs` (`From` impls, exhaustive `match`, no `_ =>`). Commands just use `?`.
- **Commands:** a thin `#[tauri::command]` wrapper over a plain `async fn x_impl(state: &AppState, ...)` that tests call directly. Register in `lib.rs` by full path (`ipc::repos::add_repository`).
- **Shared state:** one `GitRunner` for the whole app (inside `AppState`) so its semaphore is a real global cap. Storage goes through the `RepoStore` trait, never a concrete backend, in commands.
- **Contracts:** no `u64`/`i64` (ts-rs makes them `bigint`, which JSON cannot carry); use `u32`. `Option<T>` is `T | null`. Field names are camelCase on the wire.

## Git safety (the app runs git against repos agents are using)

- All git runs through `GitRunner`: argument vectors, **never a shell**, bounded concurrency and timeout.
- Read-only and non-interfering. The runner sets `GIT_OPTIONAL_LOCKS=0` (so `git status` never takes `index.lock` and breaks an agent's own `git add`/`commit`) and `-c core.fsmonitor=false`. Repo config is untrusted: a repo-local `core.fsmonitor` can name a hook program that git **executes** during `git status`, so merely observing a repo could run arbitrary code. `GitRunner::with_env` can set where git reads config from but cannot override these.
- `filter.<name>.clean|process` drivers (picked by `.gitattributes`) run during `status` and `diff` against the working tree, and no `-c` switch disables them. They are guarded one layer up (WHY-39, `git/filters.rs`): `read_status` / `read_changes` require a `ReadPermit` that only `guard_working_tree` / `authorize` can mint, and it is refused for drivers that come from repository-controlled config until the user trusts them. Any new git call that reads the working tree (status, diff against the tree, `diff-files`, `ls-files -m`, `hash-object` without `--no-filters`) MUST take a `ReadPermit`; `status` keeps `--ignore-submodules=dirty`. Never weaken the runner's protections or the guard.
- Put `--end-of-options` before any user-supplied revision or ref so it cannot be read as a flag.
- Diff commands must pass `--no-ext-diff --no-textconv` (repo config could otherwise run arbitrary programs).
- Never run network commands (`fetch`, `pull`, `push`, `clone`, `ls-remote`) and never write to a repo (`checkout`, `reset`, `add`, `commit`, ...).

## Test isolation

- Integration tests build real repos in temp dirs only (`tempfile`), via the helpers in `src-tauri/tests/git/common.rs`.
- Scrub inherited `GIT_*` variables (git hooks and `git rebase -x` set `GIT_DIR`, which **beats `-C`** and makes git act on the real repository), and set `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`. The fixture helpers do this for setup; for the code under test use `common::test_runner()` (a `GitRunner::with_isolated_config()`) so the developer's git config cannot change results. Never set process-wide env vars in tests (racy across threads); use `GitRunner::with_env`.
- **Never run a test suite under `git rebase -x`** (or any hook). An earlier task corrupted the real repo that way.
- Tests must not touch the network, `$HOME`, or any path outside their temp dir.

## UI rules

- No Node APIs in `ui/src` (it runs in a webview). Node is for tooling only (`scripts/`, config).
- Zustand: subscribe with selectors (`useStore(s => s.x)`), never the whole store.
- Model async data as a discriminated union (`{ status: "idle" } | { status: "loading" } | { status: "ok"; data } | { status: "error"; error }`), not separate booleans.
- All data goes through the `DataSource` seam (real Tauri backend or fixtures), never `invoke` directly from components.
- Keep strict TypeScript: no `any`, no non-null assertions to silence errors.
