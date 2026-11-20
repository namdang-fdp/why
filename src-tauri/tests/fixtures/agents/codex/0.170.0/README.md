# Codex 0.170.0 fixtures

Recorded with `codex exec --json -m gpt-6-luna` (reasoning effort low), hooks injected per run via `-c hooks.<Event>=…` and `--dangerously-bypass-hook-trust`, clean environment. Findings: `docs/agents/payloads.md`.

Per scenario folder: `README.md` (command, prompt, what happened), `hooks.jsonl` (every hook payload in arrival order: `event`, `argv`, `env`, `cwd`, `mono_ns`, `wall`, `stdin`), `transcript.jsonl` (the rollout from `~/.codex/sessions/…`, scrubbed: base instructions, developer messages, AGENTS.md text, tool catalogues and `world_state` are replaced by `<redacted…>`; account ids removed), `transcript-1.jsonl` (subagent rollout). `behavior/` holds the fail-open experiments; `exec-events.jsonl` is the `codex exec --json` stream (a missing `hooks.jsonl` means no hook ran).
