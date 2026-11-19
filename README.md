# WHY

> Because git diff tells you what, not why.

WHY is a local-first desktop app (Tauri 2, Rust, React) for humans to review code written by coding agents across many repositories and worktrees. A left rail of projects/worktrees/files, a live diff, and an inspector explain why each hunk exists and where to look first, backed by a deterministic provenance engine that replays agent edit events and labels evidence Observed / Declared / Inferred / Unknown.

**Status:** early development (v0.1 scaffold).

Design notes live in [`why-brain/`](why-brain/) (Obsidian vault; start at `00 - WHY.md`).

## Development

**Prerequisites:** Node 24+, pnpm 10, Rust 1.90+, and the [Tauri system dependencies](https://v2.tauri.app/start/prerequisites/) (on Linux: `webkit2gtk-4.1`, `gtk3`, etc.).

```bash
pnpm i
cp .env.example .env   # optional; defaults work
pnpm dev               # Vite + Tauri window with hot reload
```

| Command | What it does |
|---|---|
| `pnpm dev` | Run the app in dev mode (Vite dev server + Tauri window) |
| `pnpm build` | Production bundle (`tauri build`) |
| `pnpm test` | `cargo test` + `vitest run` |
| `pnpm lint` | `cargo clippy -- -D warnings`, `cargo fmt --check`, `eslint`, `tsc --noEmit` |
| `pnpm fmt` | `cargo fmt` + `prettier --write` |

Layout: `src-tauri/` is the Rust core, `ui/` is the React frontend (Vite root).

**Async rule (Rust):** Tauri already owns a tokio runtime. Don't add `#[tokio::main]` or create a second runtime; spawn with `tauri::async_runtime::spawn` and use the `tokio` crate only for its types (process, sync, fs, time).

### Several worktrees at once

Each git worktree needs its own dev port. The port comes from `WHY_DEV_PORT` (default `1420`), read from the shell or from a gitignored `.env`. Vite serves hot reload on the same port.

```bash
# worktree A
echo WHY_DEV_PORT=1420 > .env && pnpm dev
# worktree B
echo WHY_DEV_PORT=1430 > .env && pnpm dev
```

Or one-off: `WHY_DEV_PORT=1430 pnpm dev`. Note each worktree also compiles its own `src-tauri/target/`; set `CARGO_TARGET_DIR` to share one if disk space matters.
