//! Tauri commands and events: the only door between the webview and the Rust core.

mod error;

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
