# write-update-and-bash-edits

- Harness: Claude Code 2.1.322
- Model: haiku alias (claude-haiku-5-5)
- Command: `run-claude.sh <ws> s6-write-update-bash <ws>/fixture-repo '<prompt>' --include-hook-events`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `s6-write-update-bash`: `Do exactly these tool calls in order: (a) Read src/greet.py, then Write src/greet.py with new content "def greet(name):\n    return \"Hi, \" + name\n" (overwrite). (b) Bash with the exact command: rm src/old.py && printf "B = 2\n" > src/bshell.py . Then reply done.  [src/old.py was pre-created by hand]`
- What happened: Write over an existing file (tool_response type `update` with originalFile + structuredPatch) and a Bash command that deletes/creates files (the hook payload says nothing about those file changes).
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
