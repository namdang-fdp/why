//! Claude Code: where its user-level hooks live and which events WHY wants.
//!
//! Sources (code.claude.com/docs/en/hooks and /settings, read 2026-11-21):
//! hooks live under a top-level `hooks` key of `settings.json`:
//! `{"hooks": {"<Event>": [{"matcher"?: "...", "hooks": [{"type": "command",
//! "command": "...", "timeout": <seconds>}]}]}}`. The user-level file is
//! `~/.claude/settings.json`, moved by `CLAUDE_CONFIG_DIR`. Managed settings
//! (`allowManagedHooksOnly`, `disableAllHooks`) override user settings.

use std::path::PathBuf;

use super::{HookError, HooksEnv};

/// (event, timeout in seconds). All ten names are in the docs' event list and
/// were observed in the WHY-45 fixtures. No `matcher`: every occurrence.
///
/// Timeouts: the harness default is 600 s, which would stall a session if the
/// hook ever hung; `why-hook` itself gives up in ~200 ms, so 5 s is generous.
/// SessionEnd runs on a tiny budget in both harnesses (Claude 1.5 s default),
/// and Codex caps it at 3 s, so it gets 2.
pub const EVENTS: &[(&str, u32)] = &[
    ("SessionStart", 5),
    ("SessionEnd", 2),
    ("UserPromptSubmit", 5),
    ("PreToolUse", 5),
    ("PostToolUse", 5),
    ("PostToolUseFailure", 5),
    ("PostToolBatch", 5),
    ("Stop", 5),
    ("SubagentStart", 5),
    ("SubagentStop", 5),
];

/// `$CLAUDE_CONFIG_DIR` or `~/.claude`.
pub fn config_dir(env: &HooksEnv) -> Result<PathBuf, HookError> {
    env.dir_from(
        "CLAUDE_CONFIG_DIR",
        env.claude_config_dir.as_deref(),
        ".claude",
    )
}

pub fn config_file(env: &HooksEnv) -> Result<PathBuf, HookError> {
    Ok(config_dir(env)?.join("settings.json"))
}

/// Managed settings, per code.claude.com/docs/en/managed-settings (read
/// 2026-11-21): macOS `/Library/Application Support/ClaudeCode/`, Linux and WSL
/// `/etc/claude-code/`, each with `managed-settings.json` and an optional
/// `managed-settings.d/*.json` drop-in directory. `allowManagedHooksOnly` is
/// honoured in managed settings only; `disableAllHooks` is honoured from any
/// file (so a managed one counts, and WHY ignores it in the user's own file).
/// WHY only ever reads these.
pub fn default_managed() -> (Vec<PathBuf>, Vec<PathBuf>) {
    (
        vec![
            "/etc/claude-code/managed-settings.json".into(),
            "/Library/Application Support/ClaudeCode/managed-settings.json".into(),
        ],
        vec![
            "/etc/claude-code/managed-settings.d".into(),
            "/Library/Application Support/ClaudeCode/managed-settings.d".into(),
        ],
    )
}
