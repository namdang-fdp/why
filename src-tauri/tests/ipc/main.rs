//! Integration tests for the IPC command logic (`*_impl` functions), run against
//! real git repositories in temp directories. No Tauri window is involved.
//!
//! One module per command file, so parallel tasks never edit the same lines:
//! `repos.rs` belongs to the repo/store task, `changes.rs` to the diff service.

// Reuse the git test fixtures (temp repos, env scrubbing). Included by path
// because each `tests/<dir>/main.rs` is its own crate and cannot `use` another's.
#[path = "../git/common.rs"]
mod common;

mod changes;
mod repos;
