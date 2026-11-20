#!/usr/bin/env bash
# Run one headless Claude Code turn with the capture hooks, from a clean environment.
# Usage: run-claude.sh <ws> <scenario> <cwd> <prompt> [extra claude args...]
#   <ws>        scratch dir that holds claude-settings.json and logs/ (outside any repo)
#   <scenario>  name; hook payloads go to <ws>/logs/<scenario>.jsonl, CLI output to .out.jsonl
# Containment (the model's Bash tool is NOT confined by `env -i` or `cd`):
#   * bubblewrap: read-only root, writable only <ws>, a private /tmp, ~/.claude and
#     ~/.claude.json (auth + transcripts). See lib.sh; proof: sandbox-check.sh.
#     WHY_CAPTURE_NO_SANDBOX=1 disables it (e.g. nested-namespace problems); say so in the README.
#   * --allowedTools lists only the commands the scenarios need (override with ALLOWED_TOOLS,
#     one pattern per line); anything else is denied in headless mode.
# `env -i` keeps the developer's CLAUDE_*/CODEX_* variables out of the run (and of the
# fixtures). --setting-sources project,local skips ~/.claude/settings.json hooks and plugins;
# --settings injects ours. Nothing under ~/.claude or ~/.codex is edited by hand.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"
ws=$1 scen=$2 cwd=$3 prompt=$4; shift 4
warn_user_hooks claude
record_version "$ws" claude

default_tools='Bash(python3 -m unittest:*)
Bash(ls:*)
Bash(cat:*)
Bash(exit 3)
Bash(rm src/old.py:*)
Bash(printf:*)'
mapfile -t allowed <<<"${ALLOWED_TOOLS:-$default_tools}"

sandbox=()
if [ -z "${WHY_CAPTURE_NO_SANDBOX:-}" ]; then
  sandbox_build "$ws" "$HOME/.claude" "$HOME/.claude.json"
  sandbox=("${SANDBOX[@]}")
fi

cd "$cwd"
env -i HOME="$HOME" PATH="$PATH" WHY_CAPTURE_LOG="$ws/logs/$scen.jsonl" \
  ${EXTRA_ENV:-} \
  timeout "${RUN_TIMEOUT:-300}" "${sandbox[@]}" claude -p "$prompt" --model haiku \
    --setting-sources project,local --settings "${SETTINGS:-$ws/claude-settings.json}" \
    --permission-mode acceptEdits --allowedTools "${allowed[@]}" \
    --output-format stream-json --verbose "$@" </dev/null >> "$ws/logs/$scen.out.jsonl" 2>> "$ws/logs/$scen.err"
