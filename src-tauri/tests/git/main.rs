//! Integration tests for the git layer: real `git` against throw-away repos.
//!
//! One test binary (`tests/git/main.rs` + modules) instead of one file per
//! topic: each integration-test crate links the whole `why_lib` (which pulls in
//! Tauri), so fewer crates means much faster builds.

mod base;
mod common;
mod repo;
mod runner;
mod status;
mod worktree;
