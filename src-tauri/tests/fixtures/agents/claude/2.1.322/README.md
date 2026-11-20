# Claude Code 2.1.322 fixtures

Recorded with `claude -p --model haiku` (claude-haiku-5-5), hooks injected via `--settings`, user settings skipped (`--setting-sources project,local`), clean environment. Findings: `docs/agents/payloads.md`.

Per scenario folder: `README.md` (command, prompt, what happened), `hooks.jsonl` (every hook payload in arrival order: `event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin`), `transcript.jsonl` (the session JSONL from `~/.claude/projects/…`, slimmed and scrubbed: `api-request*`, `session-owner`, `atis-latch` and most attachment bodies removed), `subagent-transcript.jsonl` (+`.meta.json`), `result.json` (the CLI's final result record), `hook-results.jsonl` (the CLI's `hook_response` stream events; only SessionStart unless run with `--include-hook-events`). `behavior/` holds the fail-open experiments (stdout, exit codes, timeout, missing binary, settings lookup in worktrees).
