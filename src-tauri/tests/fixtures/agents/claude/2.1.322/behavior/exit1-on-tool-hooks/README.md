# exit1-on-tool-hooks

- Harness: Claude Code 2.1.322
- Model: haiku alias (claude-haiku-5-5)
- Command: `EXTRA_ENV='WHY_CAPTURE_EXIT=1 WHY_CAPTURE_ONLY=PreToolUse,PostToolUse' run-claude.sh ... --include-hook-events`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `e2-exit1-tool`: `Use the Write tool to create the file src/e.py containing the line E = 1. Then reply with the single word done.`
- What happened: Pre/PostToolUse hooks exit 1.
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
