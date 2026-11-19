use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

/// Where this session's repositories are kept, so the UI can warn before the
/// user adds something that will be forgotten.
///
/// `persistent: true` means the SQLite database opened and `path` is its file
/// (`reason` is `None`). `persistent: false` means it could not be opened, the
/// app fell back to an in-memory store and `reason` says why (`path` is `None`):
/// everything added now is lost when WHY exits.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct StorageStatus {
    pub persistent: bool,
    /// The database file; only when `persistent`. A lossy string for a
    /// non-UTF-8 path (display only; the UI never sends it back).
    pub path: Option<String>,
    /// Why the database could not be opened; only when not `persistent`.
    pub reason: Option<String>,
}
