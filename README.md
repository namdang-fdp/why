<p align="center"><img src="assets/brand/logo.svg" alt="WHY logo" width="96" height="96"></p>

# WHY

> Because `git diff` tells you what, not why.

[![CI](https://github.com/namdang-fdp/why/actions/workflows/ci.yml/badge.svg)](https://github.com/namdang-fdp/why/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
![Status: early, v0.1 in progress](https://img.shields.io/badge/status-early%20%C2%B7%20v0.1%20in%20progress-orange)
![Platform: Linux first](https://img.shields.io/badge/platform-Linux%20first-lightgrey)

WHY is a local-first desktop app (Tauri 2 + Rust, React + TypeScript) for humans who review code
written by coding agents (Claude Code, Codex, and others) across many repositories and git
worktrees. It watches the repos your agents are working in and shows you the live diff, with the
goal of attaching the reason behind each change and telling you where to look first.

> [!WARNING]
> **WHY is early and not usable end to end yet.** v0.1 is in progress, there is no release to
> download, and Linux is the only platform the author develops and tests on. The sections below
> separate what exists today from what is planned.

<!-- TODO(screenshots): add a screenshot of the shell (repo rail, diff, inspector) once the UI is
     wired to the real core (WHY-9), then a short GIF of a live diff update. -->

## The problem

Coding agents write code fast. A human still has to understand and approve it, so review is now
the bottleneck. `git diff` shows _what_ changed. It does not show the task the agent was given,
why it made a change, whether a block was necessary, or whether the agent drifted from scope. Today
people paste diffs back into the agent's chat and ask "why did you do that?".

WHY aims to answer three questions in one place: what changed, how it changed, and why.

An agent's own explanation can be an after-the-fact rationalisation, so WHY is not meant to explain
every line. The aim is to route a reviewer's attention (unknown origin, heavy churn, untested code,
large deletions, scope drift) and to label every explanation by how well it is backed by evidence.
It never replaces reading the code, and the human always owns the approval.

## Status: what exists today and what is planned

Only the first list below exists. Everything in the table is a plan, not a promise. The detailed
roadmap is in [`why-brain/04 - MVP and Validation.md`](<why-brain/04 - MVP and Validation.md>).

**Built and merged on `main` today:**

- Project scaffold: Tauri 2 shell, Rust core, React 19 UI, lint and test scripts.
- IPC contracts between the Rust core and the UI, with TypeScript types generated from Rust.
- Git core: one shared git runner, repository registration, worktree discovery, and status
  snapshots (all read-only).
- UI shell: the three-pane layout (repo rail, review area, inspector), keyboard shortcuts and a
  command palette, running on fixture data.

**Not yet usable:** the UI is not connected to the real core yet, so you cannot review a real
repository in the app today.

| Version | Theme                 | What it is meant to deliver                                                                                                      | State       |
| ------- | --------------------- | -------------------------------------------------------------------------------------------------------------------------------- | ----------- |
| v0.1    | Workspace + live diff | Register repos, discover worktrees, diff against the base branch including untracked files, live updates, a Monaco diff view     | In progress |
| v0.2    | Session context       | An event store, a fail-open hook and collector, Claude Code and Codex adapters, a per-session timeline in the inspector          | Planned     |
| v0.3    | Line provenance       | A deterministic replay engine that attributes lines to the edits that produced them, with an accuracy benchmark                  | Planned     |
| v0.4    | Attention             | Signals (unknown origin, churn, untested, large deletion, scope drift), an attention-ordered review queue, per-hunk review state | Planned     |
| v0.5    | Close the loop        | Anchored comments, feedback back to the originating agent, a portable provenance format, first external users                    | Planned     |

The provenance model labels every piece of evidence **Observed**, **Declared**, **Inferred** or
**Unknown**, and never presents a guess as fact. That model is design work for v0.2 and v0.3; none
of it is implemented yet.

## Install

There is no release yet, so build from source (see [Development](#development)). The release
workflow in [`.github/workflows/release.yml`](.github/workflows/release.yml) is a draft that will
publish Linux AppImage and `.deb` bundles once the author starts tagging releases.

## Safety model: a passive observer

WHY runs `git` against repositories that agents are actively using, so it is built never to disturb
them.

- **Read-only.** WHY never runs `checkout`, `reset`, `add`, `commit` or any other command that
  writes to a repository.
- **No network.** WHY only issues local Git commands and sets `GIT_NO_LAZY_FETCH=1` to keep
  partial clones from downloading missing objects on demand.
- **One controlled entry point.** Every git call goes through a single runner: arguments are passed
  as an argument vector (never through a shell), concurrency is capped, and each call has a
  timeout.
- **No lock contention.** The runner sets `GIT_OPTIONAL_LOCKS=0`, so `git status` does not take
  `index.lock` and cannot break an agent's own `git add` or `git commit`.
- **Repository config is untrusted.** The runner forces `core.fsmonitor=false`, because a
  repo-local setting can name a program that git would execute during `git status`. Diff commands
  must also pass `--no-ext-diff --no-textconv`.

One residual risk is known: git filter drivers selected by `.gitattributes` can still run during
`status`. It is described in [`SECURITY.md`](SECURITY.md), which also explains how to report a
vulnerability.

## Architecture at a glance

- `src-tauri/`: the Rust core. `git/` (all git access), `store/` (persistence behind a trait),
  `ipc/` (Tauri commands), `watch/`, and `contracts/` (the types shared with the UI).
- `ui/`: the React frontend. Its `src/contracts/` folder is generated from the Rust contracts
  with [ts-rs](https://github.com/Aleph-Alpha/ts-rs); do not edit it by hand.
- Design notes and the decision log live in [`why-brain/`](why-brain/), an
  [Obsidian](https://obsidian.md) vault. Start with [`00 - WHY.md`](<why-brain/00 - WHY.md>), then
  [`02 - Architecture and Stack.md`](<why-brain/02 - Architecture and Stack.md>) and
  [`06 - Decisions.md`](<why-brain/06 - Decisions.md>).

## Prior art

WHY is not the first tool in this space: agent workspaces (Conductor, Crystal, Vibe Kanban), vendor
apps that show session context next to diffs, and live-diff viewers each cover a piece of it. The
bet is the combination: harness-agnostic, many repos and worktrees in one place, and evidence-backed
"why". The comparison and sources are in [`why-brain/05 - Research.md`](<why-brain/05 - Research.md>).

## Development

**Prerequisites:** Node 24+, pnpm 10, Rust 1.90+, Git 2.45.1+ (to disable lazy fetching), and the
[Tauri system dependencies](https://v2.tauri.app/start/prerequisites/). On Debian and Ubuntu:

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev
```

```bash
pnpm i
cp .env.example .env   # optional; defaults work
pnpm dev               # Vite + Tauri window with hot reload
```

| Command      | What it does                                                                 |
| ------------ | ---------------------------------------------------------------------------- |
| `pnpm dev`   | Run the app in dev mode (Vite dev server + Tauri window)                     |
| `pnpm build` | Production bundle (`tauri build`)                                            |
| `pnpm test`  | `cargo test` + `vitest run`                                                  |
| `pnpm lint`  | `cargo clippy -- -D warnings`, `cargo fmt --check`, `eslint`, `tsc --noEmit` |
| `pnpm fmt`   | `cargo fmt` + `prettier --write`                                             |

Layout: `src-tauri/` is the Rust core, `ui/` is the React frontend (Vite root).

**Async rule (Rust):** Tauri already owns a tokio runtime. Don't add `#[tokio::main]` or create a
second runtime; spawn with `tauri::async_runtime::spawn` and use the `tokio` crate only for its
types (process, sync, fs, time).

### Several worktrees at once

Each git worktree needs its own dev port. The port comes from `WHY_DEV_PORT` (default `1420`), read
from the shell or from a gitignored `.env`. Vite serves hot reload on the same port.

```bash
# worktree A
echo WHY_DEV_PORT=1420 > .env && pnpm dev
# worktree B
echo WHY_DEV_PORT=1430 > .env && pnpm dev
```

Or one-off: `WHY_DEV_PORT=1430 pnpm dev`. Note each worktree also compiles its own
`src-tauri/target/`; set `CARGO_TARGET_DIR` to share one if disk space matters.

## Contributing

Bug reports and ideas are welcome as [issues](https://github.com/namdang-fdp/why/issues). Please
read [`CONTRIBUTING.md`](CONTRIBUTING.md) before opening a pull request, and note the
[Code of Conduct](CODE_OF_CONDUCT.md). If you point a coding agent at this repo, it should read
[`AGENTS.md`](AGENTS.md) first.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this project, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.

## Acknowledgements

Built on [Tauri](https://tauri.app), [React](https://react.dev), [Tailwind CSS](https://tailwindcss.com),
[shadcn/ui](https://ui.shadcn.com) and [Radix UI](https://www.radix-ui.com). The multi-worktree
navigation and live-diff ideas are inspired by Orca and LGTM.
