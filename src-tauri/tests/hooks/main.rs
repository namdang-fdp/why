//! Integration tests for the hook installer (WHY-51).
//!
//! Isolation: every test builds its own temp "home" and data dir and passes
//! them through `HooksEnv::for_home`; nothing reads `$HOME`, `$CLAUDE_CONFIG_DIR`
//! or `$CODEX_HOME`, no process-wide env var is set, and no managed-policy path
//! points at the real `/etc` (the default is NO policy, unlike
//! `HooksEnv::from_process`). The "hook binary" is a tiny shell script.

mod common;
mod ipc;
mod scenarios;
