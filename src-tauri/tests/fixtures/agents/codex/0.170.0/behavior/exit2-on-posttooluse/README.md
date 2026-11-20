# exit2-on-posttooluse

- Harness: Codex 0.170.0
- Model: gpt-6-luna, reasoning effort low
- Command: `EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=PostToolUse' run-codex.sh ...`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `cx-e2-exit2-post`: `Use apply_patch to create src/cxe.py containing the line E = 1, then reply with the single word done.`
- What happened: PostToolUse exits 2.
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
