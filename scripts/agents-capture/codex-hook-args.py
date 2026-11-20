#!/usr/bin/env python3
"""Print `-c` override arguments (one per line) registering log-hook.sh on every Codex event.

Used instead of a project .codex/hooks.json because project-level config is ignored until
the project is trusted, and trusting persists to ~/.codex/config.toml (off limits here).
Usage: codex-hook-args.py <path-to-log-hook.sh> [timeout-secs]
Env: HOOK_CMD replaces the command (e.g. a missing binary; give it already shell-quoted),
     HOOK_EVENTS=A,B limits the events.
"""
import json
import os
import shlex
import sys

EVENTS = [
    "SessionStart", "SessionEnd", "SubagentStart", "SubagentStop", "PreToolUse",
    "PermissionRequest", "PostToolUse", "PreModelRequest", "PostModelRequest",
    "PreCompact", "PostCompact", "UserPromptSubmit", "Stop", "Interrupt",
]
hook = sys.argv[1]
timeout = f", timeout = {int(sys.argv[2])}" if len(sys.argv) > 2 else ""
# Two layers of quoting: shlex for the shell that runs the hook, then a TOML basic string
# (JSON string syntax is a subset of TOML's, so json.dumps escapes quotes and backslashes).
shell_cmd = os.environ.get("HOOK_CMD", f"{shlex.quote(hook)} codex")
toml_cmd = json.dumps(shell_cmd, ensure_ascii=False)
for e in os.environ.get("HOOK_EVENTS", ",".join(EVENTS)).split(","):
    print("-c")
    print(f"hooks.{e}=[{{hooks=[{{type=\"command\", command={toml_cmd}{timeout}}}]}}]")
