# subagent

- Harness: Codex 0.170.0
- Model: gpt-6-luna, reasoning effort low
- Command: `run-codex.sh <ws> cx2-subagent <ws>/fixture-repo '<prompt>'`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `cx2-subagent`: `Spawn a subagent (use the spawn_agent tool) and ask it to create the file src/cxsub.py containing the line S = 1 via apply_patch, wait for it to finish, then reply done.`
- What happened: spawn_agent: a subagent applies a patch. Subagent hooks use the parent's session_id plus agent_id; the subagent has its own transcript_path.
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
