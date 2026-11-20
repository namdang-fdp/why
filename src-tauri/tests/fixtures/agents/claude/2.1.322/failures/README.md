# failures

- Harness: Claude Code 2.1.322
- Model: haiku alias (claude-haiku-5-5)
- Command: `run-claude.sh <ws> s5-failures <ws>/fixture-repo '<prompt>'`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `s5-failures`: `Call these tools exactly, one after the other, and do not skip any even if you expect an error: (a) Bash with the exact command: python3 -m unittest discover -s tests   (b) Edit on src/calc.py with old_string "NOPE_NOT_PRESENT" and new_string "x" without reading the file first (c) Edit on src/calc.py replacing "return a + b" with "return a + b" is not allowed, so instead Read src/calc.py then Edit with old_string "NOPE_NOT_PRESENT" new_string "x" (d) Bash with the exact command: exit 3 . Then reply with the single word done.`
- What happened: Bash that exits non-zero (PostToolUseFailure) and Edits the harness rejects before running (no Pre/PostToolUse at all).
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
