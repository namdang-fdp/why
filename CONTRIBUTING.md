# Contributing to WHY

Thanks for your interest. WHY is an early, single-maintainer project, so a few ground rules keep
contributions useful for everyone.

By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Issues first, pull requests by agreement

During v0.1 the maintainer reviews every change in depth and is still shaping the core design. So:

- **Bug reports, questions and ideas are welcome** as [issues](https://github.com/namdang-fdp/why/issues/new/choose).
- **For anything bigger than a small fix** (new feature, refactor, new dependency, change to an IPC
  contract), open an issue first and wait for a "yes, send a PR". A PR that arrives without that
  agreement may be closed even if it is well made; that is about capacity, not quality.
- **Small, obvious fixes** (typos, docs, a clear bug with a test) can go straight to a PR.
- **Security problems** do not belong in public issues; follow [`SECURITY.md`](SECURITY.md).

To propose a feature, describe the reviewer's problem first, then the change. WHY is deliberately
narrow: a passive, local-first observer for reviewing agent work. It is not an IDE, an agent
harness, an automated reviewer or an orchestrator. The product direction and the reasons behind past
choices are in [`why-brain/`](why-brain/), especially
[`00 - WHY.md`](<why-brain/00 - WHY.md>) and [`06 - Decisions.md`](<why-brain/06 - Decisions.md>).
Please read them before proposing something that touches scope.

## Set up

See [Development in the README](README.md#development) for prerequisites and commands. In short:

```bash
pnpm i
pnpm dev     # app with hot reload
pnpm lint    # clippy -D warnings, cargo fmt --check, eslint, tsc
pnpm test    # cargo test + vitest
pnpm fmt     # fix formatting
```

**Both `pnpm lint` and `pnpm test` must pass before you open a PR.** CI runs the same two commands.
Say in the PR what you did not verify (for example "not tried on macOS").

## Branches, commits and pull requests

1. Fork the repository and branch from `main`. Any descriptive branch name is fine.
2. Keep commits small and atomic, with [Conventional Commits](https://www.conventionalcommits.org/)
   messages and a scope when it helps: `feat(git): ...`, `fix(ui): ...`, `docs: ...`, `chore(deps): ...`.
3. If an AI assistant wrote part of a commit, add a `Co-Authored-By:` trailer naming the model that
   actually wrote it.
4. Open the PR against `main` and fill in the template. Link the issue it resolves.
5. The maintainer reviews every PR. Expect questions: the maintainer is using these reviews to
   learn the codebase, so short comments explaining non-obvious choices (the _why_, not the _what_)
   speed things up a lot.
6. PRs are merged by the maintainer only.

## Rules that are easy to get wrong

These come from [`AGENTS.md`](AGENTS.md), which is the binding rulebook (for humans and for AI
agents). Read it before touching the Rust core. The highlights:

- **WHY must never disturb the repositories it observes.** All git access goes through the shared
  `GitRunner` (argument vectors, never a shell, bounded concurrency, timeouts, `GIT_OPTIONAL_LOCKS=0`,
  `core.fsmonitor=false`). Never run network git commands or anything that writes to a repository.
  Diff commands must pass `--no-ext-diff --no-textconv`, and user-supplied revisions go after
  `--end-of-options`. Never weaken the runner's protections.
- **Generated contracts.** `ui/src/contracts/` is generated from `src-tauri/src/contracts/`. Never
  edit it by hand: run `pnpm gen:contracts` and commit the result. A test fails when they drift.
  Contract types avoid `u64`/`i64` (use `u32`) and use camelCase on the wire.
- **Rust:** no `#[tokio::main]` or second runtime; `thiserror` enums for library errors; no
  `unwrap`/`expect` on fallible runtime paths; IPC errors are mapped in `ipc/error.rs` only.
- **Tests are isolated.** Integration tests build real repositories in temp directories using the
  helpers in `src-tauri/tests/git/common.rs`, scrub inherited `GIT_*` variables, and never touch
  the network, `$HOME` or any path outside their temp directory. Never run the test suite from a
  `git rebase -x` or any git hook.
- **UI:** no Node APIs in `ui/src`; subscribe to Zustand with selectors; model async data as a
  discriminated union; all data goes through the `DataSource` seam; strict TypeScript (no `any`).

## Using coding agents on this repository

Agents are welcome to help, and the maintainer uses them heavily. They must follow
[`AGENTS.md`](AGENTS.md); `CLAUDE.md` simply points at it. You, as the human opening the PR, are
responsible for understanding and being able to explain every line you submit.

## Where things live

| What                           | Where                                                 |
| ------------------------------ | ----------------------------------------------------- |
| Rust core                      | `src-tauri/src/` (`git/`, `store/`, `ipc/`, `watch/`) |
| IPC contract types (Rust)      | `src-tauri/src/contracts/`                            |
| React UI                       | `ui/src/`                                             |
| Generated TypeScript contracts | `ui/src/contracts/` (do not edit)                     |
| Design notes and decisions     | [`why-brain/`](why-brain/)                            |
| Worker/agent rules             | [`AGENTS.md`](AGENTS.md)                              |

## License of contributions

Unless you explicitly state otherwise, any contribution you intentionally submit is licensed under
the same terms as the project: [Apache-2.0](LICENSE-APACHE) OR [MIT](LICENSE-MIT), at your option,
without additional terms or conditions.
