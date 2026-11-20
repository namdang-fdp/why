# settings-local-main-only

- Harness: Claude Code 2.1.322
- Model: haiku alias (claude-haiku-5-5)
- Command: `session in <ws>/fixture-wt, <ws>/fixture-repo/.claude/settings.local.json holds the hooks`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `w3-local-main-only`: `Say ok. No tools.`
- What happened: Hooks only in the main checkout's .claude/settings.local.json: they DO fire for the worktree session.
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
