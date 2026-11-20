# basic-main

- Harness: Codex 0.170.0
- Model: gpt-6-luna, reasoning effort low
- Command: `run-codex.sh <ws> cx1-main-basic <ws>/fixture-repo '<prompt 1>'; then RESUME_ID=<session-id> run-codex.sh <ws> cx1-main-basic <ws>/fixture-repo '<prompt 2>'`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt (turn 1) for `cx1-main-basic`: `Do these steps in order, using the apply_patch tool for every file change: 1) create src/notes.py containing the line NOTE = 1. 2) edit src/calc.py to add a function subtract(a, b). 3) delete the file src/sub.py. 4) run the tests with the shell command: python3 -m unittest discover -s tests . 5) run the shell command: sh -c "exit 3" . Then reply with one sentence.`
- Prompt (turn 2) for `cx1-main-basic`: `Run the shell command python3 -m unittest discover -s tests and report the result in one sentence. Do not change any files.  [RESUME_ID=<session-id>]`
- What happened: Turn 1: apply_patch add / update / delete, shell command (the unittest call ran 0 tests / exit 5 because the prompt had a stray trailing ' .'), failing shell command (sh -c 'exit 3' .). Turn 2 (`codex exec resume`, same session id, new turn id): shell command.
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
