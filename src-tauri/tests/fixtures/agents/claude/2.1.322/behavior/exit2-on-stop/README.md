# exit2-on-stop

- Harness: Claude Code 2.1.322
- Model: haiku alias (claude-haiku-5-5)
- Command: `EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=Stop,SessionStart,SessionEnd' run-claude.sh ... --include-hook-events`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `e2-exit2-stop`: `Use the Write tool to create the file src/e.py containing the line E = 1. Then reply with the single word done.`
- What happened: Stop/SessionStart/SessionEnd hooks exit 2: the agent is forced to continue. NOTE: in this run the model also ran read-only Bash greps/listings looking for the hook, including `grep -n log-hook` over the owner's ~/.claude/settings.json (output: only `exit=2`); allowedTools Bash is not confined to the temp dir.
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
