use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

// Newtype pattern: a tuple struct with one field. In Rust, `RepoId` and `WorktreeId`
// are distinct types, so passing a worktree id where a repo id is expected is a
// compile error. On the wire a newtype serializes as its inner value, so JSON and
// TypeScript see a plain string (`type RepoId = string`).
//
// Contracts never *compute* ids. The core (git module) derives them; the UI treats
// them as opaque strings and only passes them back.

/// Opaque identifier of a registered repository.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(test, derive(TS))]
pub struct RepoId(pub String);

/// Opaque identifier of a worktree. Derived by the git module from the canonical
/// worktree path and the git common dir, so it is stable across restarts.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(test, derive(TS))]
pub struct WorktreeId(pub String);
