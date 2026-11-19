//! `get_storage_status`: tells the UI whether repositories are saved to disk.
//!
//! `StorageMode` is decided once at startup (`lib.rs`) and never changes, so
//! this is a plain read of Tauri-managed state: infallible, no `AppState`.

use tauri::State;

use crate::contracts::StorageStatus;
use crate::store::StorageMode;

/// Whether repositories persist across restarts (see [`StorageStatus`]).
#[tauri::command]
pub fn get_storage_status(mode: State<'_, StorageMode>) -> StorageStatus {
    get_storage_status_impl(&mode)
}

pub fn get_storage_status_impl(mode: &StorageMode) -> StorageStatus {
    match mode {
        StorageMode::Persistent(path) => StorageStatus {
            persistent: true,
            path: Some(path.to_string_lossy().into_owned()),
            reason: None,
        },
        StorageMode::Temporary { reason } => StorageStatus {
            persistent: false,
            path: None,
            reason: Some(reason.clone()),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn persistent_storage_reports_its_path_and_no_reason() {
        let mode = StorageMode::Persistent(PathBuf::from("/data/why.db"));
        assert_eq!(
            get_storage_status_impl(&mode),
            StorageStatus {
                persistent: true,
                path: Some("/data/why.db".into()),
                reason: None,
            }
        );
    }

    #[test]
    fn temporary_storage_reports_the_reason_and_no_path() {
        let mode = StorageMode::Temporary {
            reason: "cannot create /ro: read-only file system".into(),
        };
        assert_eq!(
            get_storage_status_impl(&mode),
            StorageStatus {
                persistent: false,
                path: None,
                reason: Some("cannot create /ro: read-only file system".into()),
            }
        );
    }
}
