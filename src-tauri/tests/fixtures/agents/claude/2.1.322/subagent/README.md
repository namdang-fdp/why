# subagent

- Harness: Claude Code 2.1.322
- Model: haiku alias (claude-haiku-5-5)
- Command: `run-claude.sh <ws> s2-subagent <ws>/fixture-repo '<prompt>'`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `s2-subagent`: `Use the Agent tool (subagent_type general-purpose) with a prompt telling it to create the file src/sub.py containing one function sub_helper() returning 1 using the Write tool, then to reply "created". After it returns, say done in one sentence.`
- What happened: Agent tool (general-purpose subagent writes a file). In `-p` mode the subagent was launched asynchronously: the parent's `Stop` fires before the subagent's tool calls.
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
