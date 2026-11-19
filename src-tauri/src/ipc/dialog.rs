//! `pick_repository_folder`: the native "choose a folder" dialog, opened by the
//! Rust core.
//!
//! # Why a command instead of the dialog plugin's JavaScript API
//!
//! `tauri-plugin-dialog` could be used straight from the webview
//! (`open({ directory: true })`), but that needs the `dialog:allow-open`
//! permission in a capability, and then *any script running in the webview*
//! (a compromised dependency, an XSS in a rendered diff) could pop dialogs with
//! arbitrary options: file filters, save dialogs, message boxes. We only ever
//! want one thing: "pick a folder". So the plugin is registered (the core needs
//! it) but **no `dialog:*` permission is granted** to any window; the webview's
//! only way to open a picker is this command, which hard-codes the options and
//! returns just the chosen path. Least privilege: the UI gets the capability
//! it needs, not the library's whole surface.
//!
//! The path is only a *suggestion* from the user; it still goes through
//! `add_repository`, which validates it like any other input.

use tauri::{AppHandle, Window};
use tauri_plugin_dialog::{DialogExt, FilePath};
use tokio::sync::oneshot;

use crate::contracts::IpcError;

/// Ask the user for a folder. `Ok(None)` means they cancelled.
#[tauri::command]
pub async fn pick_repository_folder(
    app: AppHandle,
    window: Window,
) -> Result<Option<String>, IpcError> {
    // The callback form is non-blocking: a blocking pick would park one of the
    // async runtime's worker threads for as long as the dialog is open. The
    // oneshot channel turns the callback back into something we can `.await`.
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .set_title("Add repository")
        // Parent = our window, so the picker is modal over WHY and does not
        // get lost behind it.
        .set_parent(&window)
        .pick_folder(move |picked| {
            // The receiver is gone only if the command was dropped; nothing to do.
            let _ = tx.send(picked);
        });
    let picked = rx.await.map_err(|_| IpcError::Internal {
        message: "the folder dialog closed unexpectedly".to_string(),
    })?;
    folder_to_string(picked)
}

/// Turn the plugin's answer into the contract's `Option<String>`.
///
/// Split out so tests can cover it without a window. A `file://` URL that is
/// not a local path cannot happen with a desktop folder picker; it is reported
/// as an internal error rather than guessed at.
pub(crate) fn folder_to_string(picked: Option<FilePath>) -> Result<Option<String>, IpcError> {
    let Some(picked) = picked else {
        return Ok(None);
    };
    match picked.into_path() {
        // Lossy for non-UTF-8 names, like every path in the contracts.
        Ok(path) => Ok(Some(path.to_string_lossy().into_owned())),
        Err(e) => {
            tracing::warn!(error = %e, "the folder dialog returned a non-path");
            Err(IpcError::Internal {
                message: "the folder dialog returned an unusable location".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn a_cancelled_dialog_is_none() {
        assert_eq!(folder_to_string(None), Ok(None));
    }

    #[test]
    fn a_chosen_folder_becomes_its_path_string() {
        let picked = FilePath::Path(PathBuf::from("/home/me/code"));
        assert_eq!(
            folder_to_string(Some(picked)),
            Ok(Some("/home/me/code".to_string()))
        );
    }

    #[test]
    fn a_non_file_url_is_an_internal_error() {
        let url = FilePath::Url("https://example.com/x".parse().unwrap());
        assert!(matches!(
            folder_to_string(Some(url)),
            Err(IpcError::Internal { .. })
        ));
    }
}
