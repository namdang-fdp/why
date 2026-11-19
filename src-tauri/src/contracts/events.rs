use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

use super::ids::WorktreeId;

/// Name of the Tauri event the core emits when a worktree changes on disk.
/// Exported to TypeScript as `WORKTREE_CHANGED_EVENT` so both sides share one spelling.
pub const WORKTREE_CHANGED_EVENT: &str = "worktree-changed";

/// Payload of the `worktree-changed` event.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct WorktreeChanged {
    pub worktree_id: WorktreeId,
    /// Paths (relative to the worktree root) known to have changed. Empty means
    /// "unknown, refetch everything" (for example after a branch switch).
    pub paths: Vec<String>,
}
