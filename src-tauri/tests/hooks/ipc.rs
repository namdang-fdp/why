//! The IPC layer: `*_impl` functions over a fixture, and the collector health.

use why_lib::collector::{CollectorStatus, StatsSnapshot};
use why_lib::contracts::{
    ApplyResult, ChangeOutcome, CollectorState, HookState, PreviewResult, RefusalReason,
};
use why_lib::hooks::{CollectorSnapshot, LastEvents};
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

    let report = hook_status_impl(&fx.ctx, None, None).await.unwrap();
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

    let report = hook_status_impl(&fx.ctx, Some(snap), None).await.unwrap();
    let c = report.collector;
    assert_eq!(c.status, CollectorState::Listening);
    assert_eq!(c.events_received, Some(7));
    assert_eq!(c.spool_ingested, Some(2));
    assert_eq!(c.spool_files, Some(2));
    assert_eq!(c.socket_path, fx.ctx.paths.socket_path.to_string_lossy());
    // The event store was not asked here: "last event" is unknown, said so, not faked.
    assert_eq!(c.unavailable, ["lastEventAt"]);

    let disabled = CollectorSnapshot {
        status: CollectorStatus::Disabled("socket in use".into()),
        stats: StatsSnapshot::default(),
    };
    let c = hook_status_impl(&fx.ctx, Some(disabled), None)
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

// ---- WHY-55: the last event per harness ------------------------------------

const MS: u64 = 1_795_163_026_192; // a plausible Unix ms

#[tokio::test]
async fn the_last_event_is_reported_per_harness() {
    let fx = Fx::new();
    let status = |last: Option<LastEvents>| {
        let ctx = fx.ctx.clone();
        async move { hook_status_impl(&ctx, None, last).await.unwrap() }
    };

    // The store could not be asked: unknown, for both.
    let r = status(None).await;
    assert_eq!(r.claude_code.last_event_at, None);
    assert_eq!(r.collector.unavailable, ["lastEventAt"]);

    // Asked, and nothing was ever received: no time, still listed as unknown.
    let r = status(Some(LastEvents::default())).await;
    assert_eq!(
        (&r.claude_code.last_event_at, &r.codex.last_event_at),
        (&None, &None)
    );
    assert_eq!(r.collector.unavailable, ["lastEventAt"]);

    // Claude only: its time is there, Codex has none, and the fact is no longer unavailable.
    let r = status(Some(LastEvents {
        claude_code: Some(MS),
        codex: None,
    }))
    .await;
    assert_eq!(
        r.claude_code.last_event_at.as_deref(),
        Some(why_lib::events::timeline::rfc3339_from_ms(MS).as_str())
    );
    assert_eq!(r.codex.last_event_at, None);
    assert!(r.collector.unavailable.is_empty());

    // Both, with their own times.
    let r = status(Some(LastEvents {
        claude_code: Some(MS),
        codex: Some(MS + 60_000),
    }))
    .await;
    let (c, x) = (
        r.claude_code.last_event_at.unwrap(),
        r.codex.last_event_at.unwrap(),
    );
    assert!(x > c, "{x} should be after {c}");
    assert!(r.collector.unavailable.is_empty());
}

#[tokio::test]
async fn last_events_come_from_the_event_store_and_fail_to_unknown() {
    use why_lib::events::{EventStore, MemoryEventStore};
    use why_lib::ipc::hooks::last_events;
    let store = MemoryEventStore::new();
    assert_eq!(last_events(&store), Some(LastEvents::default()));

    // A store that cannot answer (the trait default) is "unknown", never "none".
    struct Silent;
    impl EventStore for Silent {
        fn append(
            &self,
            _: &[why_lib::events::NormalizedEvent],
        ) -> Result<why_lib::events::AppendOutcome, why_lib::store::StoreError> {
            unreachable!()
        }
        fn sessions_for_worktree(
            &self,
            _: &why_lib::contracts::WorktreeId,
        ) -> Result<Vec<why_lib::contracts::SessionSummary>, why_lib::store::StoreError> {
            unreachable!()
        }
        fn timeline(
            &self,
            _: &why_lib::events::SessionKey,
        ) -> Result<Option<why_lib::contracts::SessionTimeline>, why_lib::store::StoreError>
        {
            unreachable!()
        }
        fn prune_older_than(&self, _: u64) -> Result<usize, why_lib::store::StoreError> {
            unreachable!()
        }
    }
    assert_eq!(last_events(&Silent), None);
}

// ---- WHY-55: the spool directory ------------------------------------------

#[tokio::test]
async fn a_missing_spool_directory_is_an_empty_spool() {
    let fx = Fx::new();
    assert!(
        !fx.ctx.paths.spool_dir.exists(),
        "the fixture has no spool yet"
    );
    let c = hook_status_impl(&fx.ctx, None, None)
        .await
        .unwrap()
        .collector;
    assert_eq!(c.spool_files, Some(0));
    // An existing, empty one is 0 as well.
    std::fs::create_dir_all(&fx.ctx.paths.spool_dir).unwrap();
    let c = hook_status_impl(&fx.ctx, None, None)
        .await
        .unwrap()
        .collector;
    assert_eq!(c.spool_files, Some(0));
}

#[tokio::test]
async fn a_spool_path_that_cannot_be_read_stays_unknown() {
    // Not a directory: reading it fails with something other than NotFound.
    let fx = Fx::new();
    let spool = &fx.ctx.paths.spool_dir;
    std::fs::create_dir_all(spool.parent().unwrap()).unwrap();
    std::fs::write(spool, "not a directory").unwrap();
    let c = hook_status_impl(&fx.ctx, None, None)
        .await
        .unwrap()
        .collector;
    assert_eq!(c.spool_files, None);
}

#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_spool_directory_stays_unknown() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new();
    let spool = &fx.ctx.paths.spool_dir;
    std::fs::create_dir_all(spool).unwrap();
    std::fs::set_permissions(spool, std::fs::Permissions::from_mode(0o000)).unwrap();
    // As root the permission bits do not apply: nothing to test then.
    if std::fs::read_dir(spool).is_ok() {
        eprintln!("skipped: running as root, a mode-000 directory is still readable");
        std::fs::set_permissions(spool, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let c = hook_status_impl(&fx.ctx, None, None)
        .await
        .unwrap()
        .collector;
    // Restore before asserting so the temp dir can be removed.
    std::fs::set_permissions(spool, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(c.spool_files, None);
}
