#!/usr/bin/env bash
# Run one headless `codex exec` turn with the capture hooks, from a clean environment.
# Usage: run-codex.sh <ws> <scenario> <cwd> <prompt> [extra codex exec args...]
# Hooks are injected per run as `-c hooks.<Event>=...` overrides (see codex-hook-args.py)
# and run without persisted trust via --dangerously-bypass-hook-trust. ~/.codex/config.toml
# and hooks.json are never written; `notify=[]` stops the owner's own notify program from
# firing during spikes. `env -i` keeps the caller's CLAUDE_*/CODEX_* variables out.
# CODEX_HOME cannot be isolated without copying credentials (forbidden), so user-level hooks
# in ~/.codex/ DO fire too: their names are printed before the run (warn_user_hooks).
# Containment: bubblewrap with read-only root, writable only <ws>, a private /tmp and ~/.codex
# (see lib.sh, sandbox-check.sh). Codex also sandboxes its own shell tool; whether nested
# namespaces work was not verified (WHY_CAPTURE_NO_SANDBOX=1 disables the outer layer).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"
ws=$1 scen=$2 cwd=$3 prompt=$4; shift 4
warn_user_hooks codex
record_version "$ws" codex
mapfile -t hook_args < <(python3 "$here/codex-hook-args.py" "$here/log-hook.sh" ${HOOK_TIMEOUT:-})

sandbox=()
if [ -z "${WHY_CAPTURE_NO_SANDBOX:-}" ]; then
  sandbox_build "$ws" "$HOME/.codex"
  sandbox=("${SANDBOX[@]}")
fi

cd "$cwd"
env -i HOME="$HOME" PATH="$PATH" WHY_CAPTURE_LOG="$ws/logs/$scen.jsonl" ${EXTRA_ENV:-} \
  timeout "${RUN_TIMEOUT:-300}" "${sandbox[@]}" codex exec --json -m "${CODEX_MODEL:-gpt-6-luna}" \
    -c 'model_reasoning_effort="low"' -c 'notify=[]' \
    "${hook_args[@]}" --dangerously-bypass-hook-trust --skip-git-repo-check \
    -s workspace-write "$@" ${RESUME_ID:+resume "$RESUME_ID"} "$prompt" </dev/null >> "$ws/logs/$scen.out.jsonl" 2>> "$ws/logs/$scen.err"
