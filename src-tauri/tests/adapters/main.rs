//! Integration tests for the adapters, through the PUBLIC API only.
//!
//! WHY-14 (Claude Code): `claude_golden` replays every WHY-45 fixture through
//! `normalize` + ingest + timeline and compares with `golden/claude/*.json`;
//! `claude_hostile` feeds malformed payloads; `claude_transcript` tests the lazy
//! transcript reader on temp files. Nothing touches `$HOME` or the network.
//!
//! (WHY-15 adds its own `mod` lines below.)

mod common;

mod claude_golden;
mod claude_hostile;
mod claude_transcript;
