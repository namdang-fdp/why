# exit2-on-stop-with-stderr

- Harness: Codex 0.170.0
- Model: gpt-6-luna, reasoning effort low
- Command: `EXTRA_ENV='WHY_CAPTURE_EXIT=2 WHY_CAPTURE_ONLY=Stop WHY_CAPTURE_STDERR=KEEP_GOING_MARKER' RUN_TIMEOUT=100 run-codex.sh ...`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `cx-e2-exit2-stop-stderr`: `Reply with the single word done. Use no tools.`
- What happened: Stop exits 2 with stderr: the turn is continued again and again until the 100s wrapper timeout killed it (rc=124).
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
