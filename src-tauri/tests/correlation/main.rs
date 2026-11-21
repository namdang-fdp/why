//! Integration tests for the file <-> turn correlation (WHY-16).
//!
//! - `store.rs`: the `EventStore::{edit_calls, shell_turns}` contract, run
//!   against BOTH the memory and the SQLite store with hand-built events.
//! - `scenarios.rs`: end to end through the real adapters, the git resolver, the
//!   ingest step and `get_worktree_attribution` on throw-away git repositories,
//!   again on both stores. Includes the acceptance scenario, whose output table is
//!   printed (run with `-- --nocapture` to see it).
//!
//! - `cross_worktree.rs`: decision 32 (WHY-65), an edit belongs to the edited
//!   file's own worktree, on both stores.
//!
//! Everything lives in temp dirs; nothing touches `$HOME` or the network.

// The git fixtures (temp repos, env scrubbing), included by path because each
// `tests/<dir>/main.rs` is its own crate.
#[path = "../git/common.rs"]
mod common;

mod cross_worktree;
mod rig;
mod scenarios;
mod store;
