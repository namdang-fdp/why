//! WHY core. Everything lives in this library so `main.rs` stays a thin
//! launcher and the code is testable without starting a window.

pub mod contracts;
pub mod git;
pub mod ipc;
pub mod state;
pub mod store;
pub mod watch;

use std::sync::Arc;

use tracing_subscriber::EnvFilter;

use crate::git::GitRunner;
use crate::state::AppState;
use crate::store::{MemoryRepoStore, RepoStore};

// Async rule: Tauri already owns a tokio runtime. Do NOT add `#[tokio::main]` or
// build a second `Runtime`; spawn with `tauri::async_runtime::spawn`. Use the
// `tokio` crate only for its types (process, sync, fs, time).

/// Build and run the Tauri application. Blocks until the last window closes.
pub fn run() {
    // Log level comes from `RUST_LOG` (e.g. `RUST_LOG=why_lib=debug`); default is `info`.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting WHY");

    // WHY-6: replace this line with the SQLite-backed store (nothing else changes).
    let repos: Arc<dyn RepoStore> = Arc::new(MemoryRepoStore::new());
    let state = AppState::new(GitRunner::new(), repos);

    tauri::Builder::default()
        // Managed state: commands receive it as `State<'_, AppState>`.
        .manage(state)
        // Every command the webview may `invoke` must be registered here.
        .invoke_handler(tauri::generate_handler![
            ipc::ping,
            ipc::repos::list_repositories,
            ipc::repos::add_repository,
            ipc::repos::remove_repository,
            ipc::repos::set_base,
            ipc::changes::list_worktrees,
            ipc::changes::get_changes,
            ipc::changes::get_file_diff,
        ])
        .run(tauri::generate_context!())
        .expect("error while running WHY");
}
