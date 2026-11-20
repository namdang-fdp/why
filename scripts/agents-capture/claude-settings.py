#!/usr/bin/env python3
"""Print a Claude Code settings JSON that registers log-hook.sh on every hook event.

Usage: claude-settings.py <path-to-log-hook.sh> [timeout-secs]
"""
import json
import shlex
import sys

EVENTS = [
    "SessionStart", "Setup", "UserPromptSubmit", "UserPromptExpansion", "PreToolUse",
    "PermissionRequest", "PermissionDenied", "PostToolUse", "PostToolUseFailure",
    "PostToolBatch", "Notification", "SubagentStart", "SubagentStop", "TaskCreated",
    "TaskCompleted", "Stop", "StopFailure", "TeammateIdle", "InstructionsLoaded",
    "ConfigChange", "CwdChanged", "DirectoryAdded", "FileChanged", "WorktreeCreate",
    "WorktreeRemove", "PreCompact", "PostCompact", "SessionEnd",
]
hook = sys.argv[1]
timeout = int(sys.argv[2]) if len(sys.argv) > 2 else None
# The command runs through `sh -c`, so the path must be shell-quoted (spaces in a checkout path).
h = {"type": "command", "command": f"{shlex.quote(hook)} claude"}
if timeout:
    h["timeout"] = timeout
# WorktreeCreate/Remove replace git's own worktree handling when configured, and
# FileChanged needs a file-name matcher, so they are excluded from the default run.
skip = {"WorktreeCreate", "WorktreeRemove", "FileChanged"}
print(json.dumps({"hooks": {e: [{"hooks": [h]}] for e in EVENTS if e not in skip}}, indent=2))
