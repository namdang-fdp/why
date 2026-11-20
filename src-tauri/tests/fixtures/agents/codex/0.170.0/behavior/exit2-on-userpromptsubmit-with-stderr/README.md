# exit2-on-userpromptsubmit-with-stderr

- Harness: Codex 0.170.0
- Model: gpt-6-luna, reasoning effort low
- Command: `EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=UserPromptSubmit WHY_CAPTURE_STDERR=PROMPT_BLOCKED_MARKER' run-codex.sh ...`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `cx-e2-exit2-prompt-stderr`: `Use apply_patch to create src/cxe2.py containing the line E = 1, then reply with the single word done.`
- What happened: UserPromptSubmit exits 2 with stderr: the turn completes with zero model usage, no file created.
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
