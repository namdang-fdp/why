# missing-binary

- Harness: Codex 0.170.0
- Model: gpt-6-luna, reasoning effort low
- Command: `HOOK_CMD=/nonexistent/why-hook run-codex.sh ...  (hooks.jsonl is absent: nothing ran)`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `cx-e4-missing`: `Use apply_patch to create src/cxe.py containing the line E = 1, then reply with the single word done.`
- What happened: Hook command does not exist.
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
