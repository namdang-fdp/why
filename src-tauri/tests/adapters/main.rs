//! Integration tests for the harness adapters, through the PUBLIC API only.
//! Everything works on the committed WHY-45 fixtures or temp dirs; nothing
//! touches `$HOME`, `~/.claude`, `~/.codex` or the network.
//!
//! WHY-14 (Claude Code):
//! * `claude_golden`: replays every fixture through `normalize` + ingest +
//!   timeline and compares with `golden/claude/*.json`.
//! * `claude_hostile`: malformed payloads.
//! * `claude_transcript`: the lazy transcript reader on temp files.
//!
//! WHY-15 (Codex):
//! * `codex_golden`: replays every fixture through `normalize` +
//!   `build_timeline` and compares with a checked-in snapshot.
//! * `codex_patch`: the `apply_patch` parser.
//! * `codex_hostile`: malformed / unexpected hook payloads.
//! * `codex_rollout`: the lazy rollout reader.

mod common;

mod claude_golden;
mod claude_hostile;
mod claude_transcript;

mod codex_common;

mod codex_golden;
mod codex_hostile;
mod codex_patch;
#[cfg(target_os = "linux")]
mod codex_rollout;
mod codex_snapshot;
