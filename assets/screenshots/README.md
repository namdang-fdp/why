# Screenshots for the README

| File           | What                                                                  | Size                    |
| -------------- | --------------------------------------------------------------------- | ----------------------- |
| `why-v0.1.png` | The WHY window on the built-in demo (fixture) data, a split Rust diff | 2880 × 1800 px, ~260 KB |

The screenshot shows **fixture data only** (the sample `why` and `billing-api` repositories with
`/home/dev/...` paths), never a real repository, so it is safe to publish.

A GIF of a live diff update is planned after v0.1. It is not referenced from the README yet.

## How `why-v0.1.png` was made

It is a headless-Chrome capture of the UI running on fixtures, so it can be regenerated whenever the
UI changes:

1. Start the fixture UI: `pnpm dev:ui` (it serves on `WHY_DEV_PORT`, default 1420). In a plain
   browser there is no Rust core, so the UI uses `ui/src/fixtures/`.
2. Open it in a 1440 × 900 viewport at device scale factor 2 with a dark colour scheme (the 2880 ×
   1800 px output).
3. Pin the browser clock to just after the fixtures' timestamps (`1_700_004_000` seconds), so the
   rail reads "7m" and "1h" instead of "3y".
4. Click `mod.rs` in the changed-files list (the `agent/WHY-4-git-cli` worktree of `why` is selected
   by default), wait for the Monaco diff to render, move the pointer away from any hover state, and
   take the screenshot. Split view and the inspector are the defaults.

The capture used Playwright (`playwright-core`, driving the system Chrome) from outside the
project, so nothing was added to `package.json`. Check the result for any real names or paths
before committing a new capture.
