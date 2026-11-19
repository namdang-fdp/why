//! WHY core. Everything lives in this library so `main.rs` stays a thin
//! launcher and the code is testable without starting a window.

pub mod contracts;
pub mod git;
pub mod ipc;
pub mod store;
pub mod watch;

use tracing_subscriber::EnvFilter;

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
        // Every command the webview may `invoke` must be registered here.
        .invoke_handler(tauri::generate_handler![ipc::ping])
        .run(tauri::generate_context!())
        .expect("error while running WHY");
}
