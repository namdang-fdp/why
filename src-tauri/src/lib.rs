//! WHY core. Everything lives in this library so `main.rs` stays a thin
//! launcher and the code is testable without starting a window.

pub mod contracts;
pub mod git;
pub mod ipc;
pub mod state;
pub mod store;
pub mod watch;

use std::sync::Arc;

use tauri::Manager;
use tracing_subscriber::EnvFilter;

use crate::git::GitRunner;
use crate::state::AppState;
use crate::store::{MemoryRepoStore, RepoStore, SqliteRepoStore, StorageMode};
use crate::watch::NotifyingRepoStore;

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

    tauri::Builder::default()
        // Registered for the core's own use (`ipc::dialog`); no capability
        // grants the webview any `dialog:*` permission, see that module.
        .plugin(tauri_plugin_dialog::init())
        // `setup` runs once the app (and its path API) exists, before any
        // window loads, so commands never see the state missing.
        .setup(|app| {
            let (store, storage) = open_repo_store(app.handle());
            // Managed on its own (not inside `AppState`) so `get_storage_status`
            // can read it; `Temporary` means changes are lost at exit.
            app.manage(storage);
            // The ONE runner; the watcher gets a clone, and clones share its
            // semaphore, so its git calls count against the same global cap.
            let git = GitRunner::new();
            let resync = watch::start_for_app(app.handle(), git.clone(), Arc::clone(&store));
            // Commands see the store through the notifying decorator, so adding
            // or removing a repository resyncs the watcher immediately.
            let repos: Arc<dyn RepoStore> = Arc::new(NotifyingRepoStore::new(store, resync));
            app.manage(AppState::new(git, repos));
            #[cfg(debug_assertions)]
            dev_register_from_env(app.handle());
            Ok(())
        })
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
            ipc::storage::get_storage_status,
            ipc::dialog::pick_repository_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running WHY");
}

/// Open the SQLite store at `<app data dir>/why.db`, creating the directory.
///
/// On failure (unwritable data dir, corrupt file, a database from a newer WHY)
/// the app still starts, with an in-memory store and a loud error in the log:
/// the user can keep reviewing, and nothing on disk is touched or overwritten.
/// The cost is that repositories added in this session are forgotten on exit.
/// Crashing at startup would be worse: the user could not even see the error.
fn open_repo_store(app: &tauri::AppHandle) -> (Arc<dyn RepoStore>, StorageMode) {
    let opened = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data dir: {e}"))
        .and_then(|dir| {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            let path = dir.join("why.db");
            let store = SqliteRepoStore::open(&path).map_err(|e| e.to_string())?;
            tracing::info!(path = %path.display(), "opened repository store");
            Ok((store, path))
        });
    match opened {
        Ok((store, path)) => (Arc::new(store), StorageMode::Persistent(path)),
        Err(e) => {
            tracing::error!(
                error = %e,
                "CANNOT OPEN THE REPOSITORY DATABASE; using a temporary in-memory store. \
                 Repositories added now will be forgotten when WHY exits."
            );
            (
                Arc::new(MemoryRepoStore::new()),
                StorageMode::Temporary { reason: e },
            )
        }
    }
}

/// Dev builds only: `WHY_DEV_REGISTER=/path/to/repo pnpm dev` registers that
/// repository through the real `add_repository` command logic 3 s after start,
/// so the watcher's "newly registered repo" path can be checked by hand before
/// the UI can add repositories. Compiled out of release builds.
#[cfg(debug_assertions)]
fn dev_register_from_env(app: &tauri::AppHandle) {
    let Some(path) = std::env::var_os("WHY_DEV_REGISTER") else {
        return;
    };
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let path = path.to_string_lossy().into_owned();
        let state = handle.state::<AppState>();
        match ipc::repos::add_repository_impl(&state, &path).await {
            Ok(repo) => tracing::info!(id = %repo.id.0, %path, "dev: registered repository"),
            Err(e) => tracing::warn!(?e, %path, "dev: cannot register repository"),
        }
    });
}
