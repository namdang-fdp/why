//! `get_line_origins` (WHY-64): the engine is a stub, the request check is not.

use std::sync::Arc;

use why_lib::contracts::{IpcError, LineOriginsResult, LineOriginsUnavailable, WorktreeId};
use why_lib::ipc::provenance::get_line_origins_impl;
use why_lib::state::AppState;
use why_lib::store::MemoryRepoStore;

use crate::common::test_runner;

fn state() -> AppState {
    AppState::new(test_runner(), Arc::new(MemoryRepoStore::new()))
}

#[tokio::test]
async fn answers_engine_not_ready_for_a_valid_path() {
    let wt = WorktreeId("wt-1".into());
    for path in ["src/lib.rs", "a.rs", "dir/with space/ünïcode.ts"] {
        assert_eq!(
            get_line_origins_impl(&state(), &wt, path).await.unwrap(),
            LineOriginsResult::Unavailable {
                reason: LineOriginsUnavailable::EngineNotReady
            },
            "{path:?}"
        );
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
