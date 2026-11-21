//! The IPC layer: `*_impl` functions over a fixture, and the collector health.

use why_lib::collector::{CollectorStatus, StatsSnapshot};
use why_lib::contracts::{
    ApplyResult, ChangeOutcome, CollectorState, HookState, PreviewResult, RefusalReason,
};
use why_lib::hooks::CollectorSnapshot;
use why_lib::ipc::hooks::{
    hook_status_impl, install_impl, preview_install_impl, preview_uninstall_impl, uninstall_impl,
};

use crate::common::*;

#[tokio::test]
async fn the_ipc_flow_previews_installs_reports_and_uninstalls() {
    let fx = Fx::new();
    let PreviewResult::Ready { preview } = preview_install_impl(&fx.ctx, CLAUDE).await.unwrap()
    else {
        panic!("no preview");
    };
    assert_eq!(preview.outcome, ChangeOutcome::CreateFile);

    let applied = install_impl(&fx.ctx, CLAUDE, &preview.fingerprint)
        .await
        .unwrap();
    assert!(matches!(
        applied,
        ApplyResult::Applied {
            outcome: ChangeOutcome::CreateFile,
            ..
        }
    ));

    let report = hook_status_impl(&fx.ctx, None).await.unwrap();
    assert_eq!(report.claude_code.state, HookState::Installed);
    assert_eq!(report.codex.state, HookState::NotInstalled);
    assert_eq!(report.instance, "installed");
    assert_eq!(report.collector.status, CollectorState::NotRunning);
    assert_eq!(report.collector.unavailable, ["lastEventAt"]);

    let PreviewResult::Ready { preview } = preview_uninstall_impl(&fx.ctx, CLAUDE).await.unwrap()
    else {
        panic!("no preview");
    };
    assert_eq!(preview.outcome, ChangeOutcome::DeleteFile);
    let applied = uninstall_impl(&fx.ctx, CLAUDE, &preview.fingerprint)
        .await
        .unwrap();
    assert!(matches!(applied, ApplyResult::Applied { .. }));
}

#[tokio::test]
async fn refusals_come_back_as_values_not_errors() {
    let fx = Fx::new();
    fx.write(&fx.claude_file(), "[]");
    match preview_install_impl(&fx.ctx, CLAUDE).await.unwrap() {
        PreviewResult::Refused { reason, .. } => assert_eq!(reason, RefusalReason::ConfigInvalid),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn collector_health_reports_counters_and_what_is_missing() {
    let fx = Fx::new();
    let snap = CollectorSnapshot {
        status: CollectorStatus::Listening,
        stats: StatsSnapshot {
            received: 7,
            spool_ingested: 2,
            ..Default::default()
        },
    };
    // Two spooled files waiting.
    let spool = &fx.ctx.paths.spool_dir;
    std::fs::create_dir_all(spool).unwrap();
    std::fs::write(spool.join("a.jsonl"), "x").unwrap();
    std::fs::write(spool.join("b.jsonl"), "x").unwrap();

    let report = hook_status_impl(&fx.ctx, Some(snap)).await.unwrap();
    let c = report.collector;
    assert_eq!(c.status, CollectorState::Listening);
    assert_eq!(c.events_received, Some(7));
    assert_eq!(c.spool_ingested, Some(2));
    assert_eq!(c.spool_files, Some(2));
    assert_eq!(c.socket_path, fx.ctx.paths.socket_path.to_string_lossy());
    // The collector has no "last event" timestamp: said so, not faked.
    assert_eq!(c.unavailable, ["lastEventAt"]);

    let disabled = CollectorSnapshot {
        status: CollectorStatus::Disabled("socket in use".into()),
        stats: StatsSnapshot::default(),
    };
    let c = hook_status_impl(&fx.ctx, Some(disabled))
        .await
        .unwrap()
        .collector;
    assert_eq!(
        c.status,
        CollectorState::Disabled {
            reason: "socket in use".into()
        }
    );
}
