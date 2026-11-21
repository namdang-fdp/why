//! Codex: where its user-level hooks live and which events WHY wants.
//!
//! Sources (learn.chatgpt.com/docs/hooks, formerly developers.openai.com/
//! codex/hooks, read 2026-11-21; parts of that page were only available as a
//! summary, see the WHY-51 report): the same JSON shape as Claude under a
//! top-level `hooks` key, in `$CODEX_HOME/hooks.json` (default
//! `~/.codex/hooks.json`). Non-managed hooks only run after the user approves
//! them in `/hooks`; approval is tied to the hook's hash. The 8 event names WHY
//! uses are in the page's list of 14 (the same list the fixtures' hooks.jsonl
//! show firing: all 8 plus Pre/PostModelRequest).
//!
//! WHY never writes `config.toml`, a `[hooks.state]` trust table or passes
//! `--dangerously-bypass-hook-trust`: trust is the user's decision.

use std::path::PathBuf;

use super::{HookError, HooksEnv};

/// Not `PreModelRequest` / `PostModelRequest`: they fire on every model call
/// and WHY does not need them. SessionEnd is capped at 3 s by Codex.
pub const EVENTS: &[(&str, u32)] = &[
    ("SessionStart", 5),
    ("SessionEnd", 2),
    ("UserPromptSubmit", 5),
    ("PreToolUse", 5),
    ("PostToolUse", 5),
    ("Stop", 5),
    ("SubagentStart", 5),
    ("SubagentStop", 5),
];

/// What to tell the user: WHY cannot see Codex's trust decision (it lives in
/// `config.toml`, whose contents WHY does not read).
pub const REVIEW_HINT: &str = "Codex runs a new or changed hook only after you approve it. \
Open Codex and approve WHY's hooks in `/hooks`. WHY cannot tell whether you already did, \
and it never bypasses the approval.";

/// `$CODEX_HOME` or `~/.codex`.
pub fn config_dir(env: &HooksEnv) -> Result<PathBuf, HookError> {
    env.dir_from("CODEX_HOME", env.codex_home.as_deref(), ".codex")
}

pub fn config_file(env: &HooksEnv) -> Result<PathBuf, HookError> {
    Ok(config_dir(env)?.join("hooks.json"))
}

/// Administrator requirements. `/etc/codex/requirements.toml` on Linux and
/// macOS (learn.chatgpt.com/codex/enterprise/managed-configuration);
/// `allow_managed_hooks_only = true` is a TOP-LEVEL key there and
/// `[features] hooks = false` turns hooks off (learn.chatgpt.com/docs/hooks).
pub fn default_requirements() -> Vec<PathBuf> {
    vec!["/etc/codex/requirements.toml".into()]
}
