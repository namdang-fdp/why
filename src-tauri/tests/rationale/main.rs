//! Integration tests for the lazy rationale commands (WHY-53): the agent's
//! own words and Codex shell outcomes, read from the harness transcript only
//! when a command is called.
//!
//! - `common.rs`: temp harness roots, fixture replay, both event stores.
//! - `claude.rs` / `codex.rs`: the happy paths on the committed WHY-45
//!   fixtures, plus the Codex exit-code rules.
//! - `hostile.rs`: files that are gone, escaping, special or huge, ids that
//!   are not ours, and the rule for which `transcript_path` is used.
//!
//! Every test runs against BOTH the memory and the SQLite store and asserts
//! the answers are identical. Everything lives in temp dirs; nothing touches
//! `$HOME` or the network. Run with `-- --nocapture` to see the sample
//! responses and the shell-outcome table.

#![cfg(target_os = "linux")]

mod claude;
mod codex;
mod common;
mod hostile;
