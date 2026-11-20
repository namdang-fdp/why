# failing-patch-and-shell-edit

- Harness: Codex 0.170.0
- Model: gpt-6-luna, reasoning effort low
- Command: `run-codex.sh <ws> cx5-failing-patch <ws>/fixture-repo '<prompt>'`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `cx5-failing-patch`: `First use apply_patch to update src/calc.py replacing the line "    return a * 99" with "    return a" (that line does not exist, just try it). Then use a shell command: printf "C = 3\n" > src/cshell.py . Then reply done.`
- What happened: apply_patch whose context does not match (PreToolUse fires, PostToolUse does NOT) and a shell command that creates a file (hook shows only the command string).
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
