# worktree

- Harness: Codex 0.170.0
- Model: gpt-6-luna, reasoning effort low
- Command: `run-codex.sh <ws> cx3-worktree <ws>/fixture-wt '<prompt>'`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `cx3-worktree`: `Use apply_patch to create src/cx_wt.py containing the line W = 1, then reply done.`
- What happened: Session started in the linked worktree: apply_patch.
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
