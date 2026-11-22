//! `get_line_origins` (WHY-64): the request check. The real answers are tested
//! in `provenance_real.rs` (WHY-23).

use std::sync::Arc;

use why_lib::contracts::{IpcError, WorktreeId};
use why_lib::ipc::provenance::get_line_origins_impl;
use why_lib::state::AppState;
use why_lib::store::MemoryRepoStore;

use crate::common::test_runner;

fn state() -> AppState {
    AppState::new(test_runner(), Arc::new(MemoryRepoStore::new()))
}

#[tokio::test]
async fn a_valid_path_in_an_unknown_worktree_is_not_found() {
    // WHY-23: the stub's `engineNotReady` is gone; the request is now resolved.
    let wt = WorktreeId("wt-1".into());
    for path in ["src/lib.rs", "a.rs", "dir/with space/ünïcode.ts"] {
        let err = get_line_origins_impl(&state(), &wt, path)
            .await
            .unwrap_err();
        assert!(matches!(err, IpcError::NotFound), "{path:?}: {err:?}");
    }
}

#[tokio::test]
async fn refuses_hostile_paths() {
    let wt = WorktreeId("wt-1".into());
    for path in [
        "",
        "/etc/passwd",
        "../outside.rs",
        "src/../../x",
        "a\0b",
        ".",
        "./a.rs",
    ] {
        let err = get_line_origins_impl(&state(), &wt, path)
            .await
            .unwrap_err();
        assert!(
            matches!(err, IpcError::InvalidInput { .. }),
            "{path:?}: {err:?}"
        );
    }
}
