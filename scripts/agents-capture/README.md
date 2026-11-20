# agents-capture

Scripts that record what Claude Code and Codex really emit (WHY-45), so a newer harness version can be re-recorded. They never touch `~/.claude` or `~/.codex` configuration: hooks are injected per run (`claude --settings`, `codex -c hooks.*`), in a throw-away repo under `/tmp`.

| Script | Purpose |
|---|---|
| `make-fixture-repo.sh <dir>` | toy git repo `fixture-repo` + linked worktree `fixture-wt` (inherited `GIT_*` variables are unset, global/system git config disabled) |
| `log-hook.sh` / `log-hook.py` | the capture hook: appends one JSON line per event to `$WHY_CAPTURE_LOG`, prints nothing, exits 0. Behaviour knobs for experiments: `WHY_CAPTURE_STDOUT`, `WHY_CAPTURE_STDERR` (`{event}` is replaced), `WHY_CAPTURE_EXIT`, `WHY_CAPTURE_SLEEP`, `WHY_CAPTURE_ONLY=Event,Event` |
| `claude-settings.py <log-hook.sh> [timeout]` | prints a Claude settings JSON hooking every event (executable path shell-quoted) |
| `codex-hook-args.py <log-hook.sh> [timeout]` | prints `-c hooks.<Event>=…` args for Codex (shell-quoted, then TOML-escaped; `HOOK_CMD`, `HOOK_EVENTS` override) |
| `lib.sh` | shared helpers: bubblewrap prefix, user-hook warning, version recording |
| `run-claude.sh <ws> <scenario> <cwd> <prompt> [args]` | one headless Claude turn: clean env (`env -i`), user settings skipped, sandboxed, narrowed tools |
| `run-codex.sh <ws> <scenario> <cwd> <prompt> [args]` | one `codex exec` turn: clean env, sandboxed; `RESUME_ID=<id>` resumes |
| `sandbox-check.sh [dir]` | command-level proof (no agent) that the sandbox denies writes outside the scratch dir |
| `save-fixtures.py [--strict] [--also-scan TEXT] <ws> <out> <repo>` | scrubs the raw logs/transcripts into `src-tauri/tests/fixtures/agents/` |

Setup used: `ws=$(mktemp -d); make-fixture-repo.sh $ws; mkdir $ws/logs; claude-settings.py $PWD/log-hook.sh > $ws/claude-settings.json`, then `run-*.sh $ws <name> $ws/fixture-repo "<prompt>"`.

## Containment (the model's shell tool is not confined by `env -i` or `cd`)

- `run-*.sh` run the agent under **bubblewrap**: read-only root, writable only the scratch workspace, a private `/tmp`, and the harness's own state (`~/.claude` + `~/.claude.json`, or `~/.codex`; needed for auth and transcripts). A write anywhere else (other checkouts, the rest of `$HOME`) fails. `sandbox-check.sh` proves this without starting an agent. `WHY_CAPTURE_NO_SANDBOX=1` turns it off; if you do, say so next to the fixtures.
- `run-claude.sh` passes a **narrow `--allowedTools`** (only the commands the scenarios use; override with `ALLOWED_TOOLS`, one pattern per line). Anything else is denied in headless mode.
- **Residual risk:** the agent can still *read* anything the user can read (for example `~/.claude/settings.json`; one recorded run did exactly that), and it can write inside the writable state directories. Do not record sessions whose prompts could make the agent print secrets.
- **Not verified:** that Claude Code and Codex start and authenticate under this bwrap line (a Claude run may also want `~/.claude.json.lock` or XDG dirs; Codex runs its own sandbox inside, and nested user namespaces may not work). Re-check with one throw-away prompt after any change.
- User-level hooks: `--setting-sources project,local` skips Claude's, but `CODEX_HOME` cannot be isolated without copying credentials, so Codex user-level hooks would fire too. The run scripts print the **names** of user hooks that may also fire before each run.
- Do not run these under `git rebase -x` or any git hook.

## Exporting fixtures (`save-fixtures.py`)

- Builds everything in a staging dir; on success replaces **only the scenario folders it generated**. A scenario with no recorded input is skipped (or aborts with `--strict`), never written empty. Any error or leak-scan hit exits non-zero with nothing written.
- Harness versions and models are read from the recorded data (Claude transcript `version`, Codex `session_meta.cli_version`); missing or mixed versions abort, and `<ws>/versions/<harness>` (written by the run scripts) is cross-checked. Output goes to `<out>/<harness>/<version>/`, so a new CLI version never overwrites the old reference.
- Values of secret-looking keys and common token shapes are redacted, and a final leak scan checks for home/temp paths, e-mail addresses, ids and token shapes (`--also-scan` adds literals such as your name).
- **The scan is a backstop, not a review.** Re-recorded fixtures need a manual read of the free-form prompts, assistant text and tool output before they are committed.
