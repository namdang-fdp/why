# basic-main

- Harness: Claude Code 2.1.322
- Model: haiku alias (claude-haiku-5-5)
- Command: `run-claude.sh <ws> s1-main-basic <ws>/fixture-repo '<prompt 1>'; then the same with '<prompt 2>' --resume <session-id>`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt (turn 1) for `s1-main-basic`: `Do these steps in order using tools: 1) Read src/calc.py. 2) Create a new file src/greet.py containing a function greet(name) that returns "Hello, " + name. 3) Edit src/calc.py to add a function subtract(a, b). 4) Run the tests with: python3 -m unittest discover -s tests . Then say done in one sentence.`
- Prompt (turn 2) for `s1-main-basic`: `Now: 1) Use the Edit tool to replace the exact text "DOES_NOT_EXIST_XYZ" with "y" in src/calc.py (it is not there; just try). 2) Use the MultiEdit tool if you have it (otherwise say you lack it) to make two edits in src/greet.py. 3) Edit tests/test_calc.py so test_add asserts add(1, 2) == 4, then run: python3 -m unittest discover -s tests . Report the result in one sentence.  [run with --resume <session-id>]`
- What happened: Turn 1 (fresh session): Read, Write (new file), Edit, Bash (tests pass). Turn 2 (`--resume`, same session id): ToolSearch for MultiEdit (not available), Edit, Bash (tests fail but the command is wrapped in `; echo exit=$?`, so no PostToolUseFailure).
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
