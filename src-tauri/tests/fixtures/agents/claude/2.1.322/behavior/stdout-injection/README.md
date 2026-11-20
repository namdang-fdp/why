# stdout-injection

- Harness: Claude Code 2.1.322
- Model: haiku alias (claude-haiku-5-5)
- Command: `EXTRA_ENV='WHY_CAPTURE_STDOUT=MARKER_{event}' run-claude.sh ... --include-hook-events`
  (run scripts live in `scripts/agents-capture/`; `<ws>` was a throw-away temp dir holding a toy git repo `fixture-repo` and a linked worktree `fixture-wt`, see `make-fixture-repo.sh`)
- Prompt for `e1-stdout`: `First use the Read tool on README.md. Then, without more tools, list every distinct token in your context that starts with MARKER_ (tokens only, comma separated).`
- What happened: Every hook prints MARKER_<event> on stdout. The model was asked to list the markers it sees.
- Files: `hooks.jsonl` = one record per hook invocation, in arrival order (`event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin` = the payload). Other files are described in the harness README.
- Scrubbed: home -> /HOME, temp dir -> /TMP, WHY checkout -> /REPO; see `scripts/agents-capture/save-fixtures.py`.
