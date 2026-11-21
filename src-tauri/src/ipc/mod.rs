//! Tauri commands and events: the only door between the webview and the Rust core.
//!
//! # Command pattern
//!
//! Every command is a thin wrapper around a plain async function:
//!
//! ```ignore
//! #[tauri::command]
//! pub async fn thing(state: State<'_, AppState>, arg: String) -> Result<T, IpcError> {
//!     thing_impl(&state, &arg).await
//! }
//!
//! pub async fn thing_impl(state: &AppState, arg: &str) -> Result<T, IpcError> { ... }
//! ```
//!
//! The wrapper only extracts Tauri's managed state; all logic lives in
//! `thing_impl`, which tests call directly with an `AppState` they built (no
//! window, no Tauri runtime). Errors from git or the store are converted by `?`
//! through the `From` impls in [`error`].
//!
//! Argument names in the signature are what the UI sends, converted to camelCase
//! (`repo_id` is `repoId` in `invoke`). Each command's signature must match the
//! table in `contracts/mod.rs`.
//!
//! # Layout
//!
//! * [`repos`]: registering repositories (`list_repositories`, `add_repository`,
//!   `remove_repository`, `set_base`).
//! * [`changes`]: reading a repository's worktrees and diffs.
//! * [`security`]: the git filter-driver guard and its trust commands
//!   (`get_repository_security`, `trust_repository_filters`,
//!   `revoke_repository_filters`).
//! * [`storage`]: `get_storage_status` (are repositories saved?).
//! * [`dialog`]: `pick_repository_folder` (native folder picker, opened by the core).
//! * [`sessions`]: agent sessions (`list_sessions`, `get_session_timeline`).
//! * [`attribution`]: which agent turns explain a worktree's changed files
//!   (`get_worktree_attribution`, `get_file_activity`).
//! * [`error`]: the one place errors become `IpcError`.

pub mod changes;
pub mod dialog;
mod error;
pub mod repos;
pub mod security;
pub mod sessions;
pub mod storage;
// WHY-16
pub mod attribution;

// Commands are not re-exported here. `#[tauri::command]` also generates helper
// macros (`__cmd__<name>`) that `generate_handler!` looks up next to the function,
// and a named `pub use repos::add_repository` does not carry them (the build
// fails). `lib.rs` therefore registers each command by its full path
// (`ipc::repos::add_repository`).

/// Proves the UI <-> Rust bridge works by returning the app version.
#[tauri::command]
pub fn ping() -> String {
    version()
}

/// Kept separate from the command so it can be unit tested without Tauri.
fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_returns_crate_version() {
        assert_eq!(ping(), env!("CARGO_PKG_VERSION"));
        assert!(!version().is_empty());
    }
}
