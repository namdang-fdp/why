//! Wire-format tests. Each one pins the exact JSON for a type, so renaming a field,
//! changing an enum tag or turning `null` into "missing" fails here, in the same
//! change that causes it, instead of breaking the UI at runtime.

use std::fmt::Debug;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::*;

/// The value serializes to exactly `expected`, and `expected` deserializes back to the value.
fn assert_wire<T>(value: &T, expected: Value)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    assert_eq!(
        &serde_json::to_value(value).unwrap(),
        &expected,
        "serialized JSON changed"
    );
    let back: T = serde_json::from_value(expected).unwrap();
    assert_eq!(
        &back, value,
        "JSON no longer deserializes to the same value"
    );
}

/// `key` is present and explicitly `null`. Plain indexing (`value["key"]`) would not do:
/// it also returns `Null` for a key that is missing, so an omitted field would pass.
fn assert_null(value: &Value, key: &str) {
    assert_eq!(
        value.get(key),
        Some(&Value::Null),
        "`{key}` must be present and null"
    );
}

#[test]
fn ids_are_plain_strings() {
    assert_wire(&RepoId("r1".into()), json!("r1"));
    assert_wire(&WorktreeId("w1".into()), json!("w1"));
}

#[test]
fn repository_wire_format() {
    let repo = Repository {
        id: RepoId("r1".into()),
        name: "why".into(),
        root_path: "/home/me/why".into(),
    };
    assert_wire(
        &repo,
        json!({ "id": "r1", "name": "why", "rootPath": "/home/me/why" }),
    );
}

#[test]
fn worktree_wire_format() {
    let wt = Worktree {
        id: WorktreeId("w1".into()),
        repo_id: RepoId("r1".into()),
        path: "/home/me/why-wt".into(),
        branch: Some("agent/x".into()),
        head_sha: Some("abc123".into()),
        is_main: false,
        is_detached: false,
        is_locked: false,
        is_prunable: false,
        base: Some(BranchInfo {
            base_ref: "origin/main".into(),
            merge_base: Some("def456".into()),
            ahead: 3,
            behind: 1,
        }),
        change_count: 7,
        load_error: None,
        last_activity: Some(1_700_000_000),
    };
    assert_wire(
        &wt,
        json!({
            "id": "w1", "repoId": "r1", "path": "/home/me/why-wt",
            "branch": "agent/x", "headSha": "abc123",
            "isMain": false, "isDetached": false, "isLocked": false, "isPrunable": false,
            "base": { "baseRef": "origin/main", "mergeBase": "def456", "ahead": 3, "behind": 1 },
            "changeCount": 7, "loadError": null, "lastActivity": 1700000000
        }),
    );
}

#[test]
fn worktree_options_serialize_as_null() {
    let wt = Worktree {
        id: WorktreeId("w2".into()),
        repo_id: RepoId("r1".into()),
        path: "/p".into(),
        branch: None,
        head_sha: None,
        is_main: true,
        is_detached: true,
        is_locked: false,
        is_prunable: false,
        base: None,
        change_count: 0,
        load_error: None,
        last_activity: None,
    };
    let value = serde_json::to_value(&wt).unwrap();
    // Present-and-null, not omitted: the TypeScript type is `string | null`, never `undefined`.
    for key in ["branch", "headSha", "base", "loadError", "lastActivity"] {
        assert_null(&value, key);
    }
}

#[test]
fn worktree_load_error_is_a_tagged_ipc_error() {
    let wt = Worktree {
        id: WorktreeId("w3".into()),
        repo_id: RepoId("r1".into()),
        path: "/p".into(),
        branch: Some("broken".into()),
        head_sha: None,
        is_main: false,
        is_detached: false,
        is_locked: false,
        is_prunable: false,
        base: None,
        change_count: 0,
        load_error: Some(IpcError::GitFailed {
            message: "fatal: not a git repository".into(),
        }),
        last_activity: None,
    };
    let value = serde_json::to_value(&wt).unwrap();
    assert_eq!(
        value["loadError"],
        json!({ "kind": "gitFailed", "message": "fatal: not a git repository" })
    );
    let back: Worktree = serde_json::from_value(value).unwrap();
    assert_eq!(back, wt);
}

#[test]
fn storage_status_wire_format() {
    assert_wire(
        &StorageStatus {
            persistent: true,
            path: Some("/data/why.db".into()),
            reason: None,
        },
        json!({ "persistent": true, "path": "/data/why.db", "reason": null }),
    );
    assert_wire(
        &StorageStatus {
            persistent: false,
            path: None,
            reason: Some("cannot create /ro".into()),
        },
        json!({ "persistent": false, "path": null, "reason": "cannot create /ro" }),
    );
}

#[test]
fn enums_serialize_as_camel_case_strings() {
    assert_wire(&DiffMode::VsBase, json!("vsBase"));
    assert_wire(&DiffMode::Uncommitted, json!("uncommitted"));
    assert_wire(&DiffMode::Staged, json!("staged"));

    let cases = [
        (ChangeStatus::Added, "added"),
        (ChangeStatus::Modified, "modified"),
        (ChangeStatus::Deleted, "deleted"),
        (ChangeStatus::Renamed, "renamed"),
        (ChangeStatus::Copied, "copied"),
        (ChangeStatus::TypeChanged, "typeChanged"),
        (ChangeStatus::Untracked, "untracked"),
        (ChangeStatus::Conflicted, "conflicted"),
    ];
    for (status, text) in cases {
        assert_wire(&status, json!(text));
    }
}

#[test]
fn file_change_wire_format() {
    let change = FileChange {
        path: "src/new_name.rs".into(),
        old_path: Some("src/old_name.rs".into()),
        status: ChangeStatus::Renamed,
        staged: true,
        unstaged: false,
        additions: Some(4),
        deletions: Some(2),
        binary: false,
        path_lossy: false,
    };
    assert_wire(
        &change,
        json!({
            "path": "src/new_name.rs", "oldPath": "src/old_name.rs", "status": "renamed",
            "staged": true, "unstaged": false, "additions": 4, "deletions": 2,
            "binary": false, "pathLossy": false
        }),
    );
}

#[test]
fn file_change_missing_options_deserialize_as_none() {
    // Being lenient on input (serde treats a missing `Option` as `None`) lets old
    // payloads keep working; we are strict only on output.
    let change: FileChange = serde_json::from_value(json!({
        "path": "logo.png", "status": "added", "staged": false, "unstaged": true,
        "binary": true, "pathLossy": false
    }))
    .unwrap();
    assert_eq!(change.old_path, None);
    assert_eq!(change.additions, None);
    assert_eq!(change.deletions, None);

    let out = serde_json::to_value(&change).unwrap();
    for key in ["oldPath", "additions", "deletions"] {
        assert_null(&out, key);
    }
}

#[test]
fn file_diff_wire_format() {
    let diff = FileDiff {
        path: "big.json".into(),
        old_path: None,
        status: ChangeStatus::Modified,
        old_content: None,
        new_content: None,
        binary: false,
        too_large: true,
        path_lossy: false,
    };
    assert_wire(
        &diff,
        json!({
            "path": "big.json", "oldPath": null, "status": "modified",
            "oldContent": null, "newContent": null,
            "binary": false, "tooLarge": true, "pathLossy": false
        }),
    );
}

#[test]
fn worktree_changed_wire_format() {
    let event = WorktreeChanged {
        worktree_id: WorktreeId("w1".into()),
        paths: vec!["src/a.rs".into()],
    };
    assert_wire(&event, json!({ "worktreeId": "w1", "paths": ["src/a.rs"] }));

    let refetch_all = WorktreeChanged {
        worktree_id: WorktreeId("w1".into()),
        paths: vec![],
    };
    assert_wire(&refetch_all, json!({ "worktreeId": "w1", "paths": [] }));
    assert_eq!(WORKTREE_CHANGED_EVENT, "worktree-changed");
}

#[test]
fn ipc_error_is_internally_tagged_with_kind() {
    assert_wire(&IpcError::NotARepo, json!({ "kind": "notARepo" }));
    assert_wire(&IpcError::GitMissing, json!({ "kind": "gitMissing" }));
    assert_wire(&IpcError::Timeout, json!({ "kind": "timeout" }));
    assert_wire(&IpcError::NotFound, json!({ "kind": "notFound" }));
    assert_wire(
        &IpcError::GitFailed {
            message: "fatal: bad object".into(),
        },
        json!({ "kind": "gitFailed", "message": "fatal: bad object" }),
    );
    assert_wire(
        &IpcError::InvalidInput {
            message: "empty ref".into(),
        },
        json!({ "kind": "invalidInput", "message": "empty ref" }),
    );
    assert_wire(
        &IpcError::Internal {
            message: "internal storage error".into(),
        },
        json!({ "kind": "internal", "message": "internal storage error" }),
    );
}

#[test]
fn ipc_error_has_readable_display() {
    // `Display` comes from thiserror and is meant for logs.
    assert_eq!(
        IpcError::GitFailed {
            message: "boom".into()
        }
        .to_string(),
        "git failed: boom"
    );
    assert_eq!(
        IpcError::Internal {
            message: "boom".into()
        }
        .to_string(),
        "internal error: boom"
    );
}

#[test]
fn repository_changed_wire_format() {
    assert_eq!(REPOSITORY_CHANGED_EVENT, "repository-changed");
    assert_wire(
        &RepositoryChanged {
            repo_id: RepoId("r1".into()),
        },
        json!({ "repoId": "r1" }),
    );
}

fn sample_driver() -> FilterDriverInfo {
    FilterDriverInfo {
        name: "evil".into(),
        clean: Some("./x clean".into()),
        smudge: None,
        process: None,
        scope: FilterScope::Local,
        origin: ".git/config".into(),
        needs_trust: true,
        trusted: false,
        referenced: true,
        fingerprint: "abc".into(),
    }
}

#[test]
fn untrusted_filters_error_wire_format() {
    assert_wire(
        &IpcError::UntrustedFilters {
            drivers: vec![sample_driver()],
        },
        json!({
            "kind": "untrustedFilters",
            "drivers": [{
                "name": "evil",
                "clean": "./x clean",
                "smudge": null,
                "process": null,
                "scope": "local",
                "origin": ".git/config",
                "needsTrust": true,
                "trusted": false,
                "referenced": true,
                "fingerprint": "abc"
            }]
        }),
    );
}

#[test]
fn repository_security_wire_format() {
    assert_wire(
        &RepositorySecurity {
            repo_id: RepoId("r1".into()),
            status: FilterGuardStatus::Blocked,
            fingerprint: "f".into(),
            drivers: vec![],
        },
        json!({ "repoId": "r1", "status": "blocked", "fingerprint": "f", "drivers": [] }),
    );
    for (status, wire) in [
        (FilterGuardStatus::Clear, "clear"),
        (FilterGuardStatus::Info, "info"),
        (FilterGuardStatus::Trusted, "trusted"),
    ] {
        assert_eq!(serde_json::to_value(status).unwrap(), json!(wire));
    }
}

#[test]
fn session_enums_wire_format() {
    assert_wire(&Harness::ClaudeCode, json!("claudeCode"));
    assert_wire(&Harness::Codex, json!("codex"));
    assert_wire(&Harness::Unknown, json!("unknown"));
    assert_wire(&SessionStatus::Live, json!("live"));
    assert_wire(&SessionStatus::Ended, json!("ended"));
    assert_wire(&SessionStatus::Unknown, json!("unknown"));
    assert_wire(&EvidenceLabel::Observed, json!("observed"));
    assert_wire(&EvidenceLabel::Declared, json!("declared"));
    assert_wire(&EvidenceLabel::Inferred, json!("inferred"));
    assert_wire(&EvidenceLabel::Unknown, json!("unknown"));
    assert_wire(&ToolOutcome::Ok, json!("ok"));
    assert_wire(&ToolOutcome::Failed, json!("failed"));
    assert_wire(&ToolOutcome::Unknown, json!("unknown"));
    for (kind, text) in [
        (TimelineEventKind::Prompt, "prompt"),
        (TimelineEventKind::AssistantMessage, "assistantMessage"),
        (TimelineEventKind::ToolCall, "toolCall"),
        (TimelineEventKind::Edit, "edit"),
        (TimelineEventKind::Shell, "shell"),
        (TimelineEventKind::TestRun, "testRun"),
        (TimelineEventKind::Subagent, "subagent"),
        (TimelineEventKind::Stop, "stop"),
    ] {
        assert_wire(&kind, json!(text));
    }
}

fn sample_summary() -> SessionSummary {
    SessionSummary {
        id: SessionId("codex:s1".into()),
        harness: Harness::Codex,
        model: None,
        worktree_id: Some(WorktreeId("w1".into())),
        cwd: "/home/me/why".into(),
        task: None,
        started_at: "2026-11-20T15:28:47.123Z".into(),
        last_event_at: "2026-11-20T15:30:00.000Z".into(),
        status: SessionStatus::Live,
        turn_count: 2,
    }
}

#[test]
fn session_summary_wire_format() {
    let summary = sample_summary();
    assert_wire(
        &summary,
        json!({
            "id": "codex:s1",
            "harness": "codex",
            "model": null,
            "worktreeId": "w1",
            "cwd": "/home/me/why",
            "task": null,
            "startedAt": "2026-11-20T15:28:47.123Z",
            "lastEventAt": "2026-11-20T15:30:00.000Z",
            "status": "live",
            "turnCount": 2
        }),
    );
    let v = serde_json::to_value(&summary).unwrap();
    assert_null(&v, "model");
    assert_null(&v, "task");
}

#[test]
fn session_timeline_wire_format() {
    let timeline = SessionTimeline {
        session: sample_summary(),
        turns: vec![Turn {
            id: "t1".into(),
            index: 0,
            prompt: Some("add subtract".into()),
            started_at: "2026-11-20T15:28:47.123Z".into(),
            ended_at: None,
            events: vec![TimelineEvent {
                id: "e1".into(),
                kind: TimelineEventKind::Edit,
                at: "2026-11-20T15:28:48.000Z".into(),
                tool_name: Some("apply_patch".into()),
                summary: "Update src/calc.py".into(),
                files: vec!["src/calc.py".into()],
                evidence: EvidenceLabel::Observed,
                outcome: Some(ToolOutcome::Unknown),
                agent_id: None,
                tool_use_id: Some("call_1".into()),
            }],
        }],
    };
    let v = serde_json::to_value(&timeline).unwrap();
    assert_eq!(
        v["turns"][0],
        json!({
            "id": "t1",
            "index": 0,
            "prompt": "add subtract",
            "startedAt": "2026-11-20T15:28:47.123Z",
            "endedAt": null,
            "events": [{
                "id": "e1",
                "kind": "edit",
                "at": "2026-11-20T15:28:48.000Z",
                "toolName": "apply_patch",
                "summary": "Update src/calc.py",
                "files": ["src/calc.py"],
                "evidence": "observed",
                "outcome": "unknown",
                "agentId": null,
                "toolUseId": "call_1"
            }]
        })
    );
    assert_null(&v["turns"][0], "endedAt");
    assert_null(&v["turns"][0]["events"][0], "agentId");
    let back: SessionTimeline = serde_json::from_value(v).unwrap();
    assert_eq!(back, timeline);
}

#[test]
fn sessions_changed_wire_format() {
    assert_wire(
        &SessionsChanged {
            worktree_id: WorktreeId("w1".into()),
        },
        json!({ "worktreeId": "w1" }),
    );
    assert_eq!(SESSIONS_CHANGED_EVENT, "sessions-changed");
}

// WHY-16: attribution contracts.
fn sample_turn_ref(outcome: ToolOutcome) -> TurnRef {
    TurnRef {
        harness: Harness::Codex,
        session_id: SessionId("codex:s1".into()),
        turn_id: "t1".into(),
        turn_index: 2,
        agent_id: None,
        tool_use_id: Some("call_1".into()),
        outcome,
        at: "2026-11-20T15:28:47.123Z".into(),
        path: "src/a.rs".into(),
    }
}

#[test]
fn turn_ref_wire_format() {
    assert_wire(
        &sample_turn_ref(ToolOutcome::Unknown),
        json!({
            "harness": "codex",
            "sessionId": "codex:s1",
            "turnId": "t1",
            "turnIndex": 2,
            "agentId": null,
            "toolUseId": "call_1",
            "outcome": "unknown",
            "at": "2026-11-20T15:28:47.123Z",
            "path": "src/a.rs"
        }),
    );
}

#[test]
fn attribution_statuses_are_plain_strings_and_there_is_no_manual() {
    assert_wire(&AttributionStatus::Explained, json!("explained"));
    assert_wire(&AttributionStatus::Unexplained, json!("unexplained"));
}

#[test]
fn worktree_attribution_wire_format() {
    let value = WorktreeAttribution {
        worktree_id: WorktreeId("w1".into()),
        mode: DiffMode::VsBase,
        files: vec![FileAttribution {
            change: FileChange {
                path: "src/a.rs".into(),
                old_path: None,
                status: ChangeStatus::Modified,
                staged: false,
                unstaged: true,
                additions: Some(3),
                deletions: Some(1),
                binary: false,
                path_lossy: false,
            },
            status: AttributionStatus::Unexplained,
            evidence: EvidenceLabel::Unknown,
            explained_by: vec![],
            attempts: vec![sample_turn_ref(ToolOutcome::Failed)],
        }],
        shell_leads: vec![ShellLead {
            harness: Harness::ClaudeCode,
            session_id: SessionId("claude-code:s2".into()),
            turn_id: "p9".into(),
            turn_index: 0,
            command_count: 2,
            first_at: "2026-11-20T15:30:00.000Z".into(),
        }],
    };
    let wire = serde_json::to_value(&value).unwrap();
    assert_eq!(wire["worktreeId"], "w1");
    assert_eq!(wire["mode"], "vsBase");
    let file = &wire["files"][0];
    assert_eq!(file["status"], "unexplained");
    assert_eq!(file["evidence"], "unknown");
    assert_eq!(file["explainedBy"], json!([]));
    assert_eq!(file["attempts"][0]["outcome"], "failed");
    assert_eq!(file["change"]["path"], "src/a.rs");
    assert_eq!(
        wire["shellLeads"][0],
        json!({
            "harness": "claudeCode",
            "sessionId": "claude-code:s2",
            "turnId": "p9",
            "turnIndex": 0,
            "commandCount": 2,
            "firstAt": "2026-11-20T15:30:00.000Z"
        })
    );
    let back: WorktreeAttribution = serde_json::from_value(wire).unwrap();
    assert_eq!(back, value);
}

// ---- hook installer (WHY-51) ----

fn sample_binary() -> BinaryInfo {
    BinaryInfo {
        status: BinaryStatus::Unverified,
        stable_path: "/h/.local/share/why/bin/why-hook".into(),
        bundled_path: None,
    }
}

#[test]
fn hook_state_is_a_tagged_union() {
    assert_wire(&HookState::NotInstalled, json!({ "kind": "notInstalled" }));
    assert_wire(&HookState::Installed, json!({ "kind": "installed" }));
    assert_wire(
        &HookState::Outdated {
            reasons: vec!["a timeout differs".into()],
        },
        json!({ "kind": "outdated", "reasons": ["a timeout differs"] }),
    );
    assert_wire(
        &HookState::NeedsReview { hint: "h".into() },
        json!({ "kind": "needsReview", "hint": "h" }),
    );
    assert_wire(
        &HookState::DisabledByUserSetting {
            message: "m".into(),
        },
        json!({ "kind": "disabledByUserSetting", "message": "m" }),
    );
    assert_wire(
        &HookState::DisabledByPolicy {
            source: "/etc/x".into(),
        },
        json!({ "kind": "disabledByPolicy", "source": "/etc/x" }),
    );
    assert_wire(
        &HookState::ConfigInvalid {
            message: "m".into(),
        },
        json!({ "kind": "configInvalid", "message": "m" }),
    );
}

#[test]
fn hook_preview_and_results_wire_format() {
    let preview = HookPreview {
        harness: HookHarness::ClaudeCode,
        kind: PreviewKind::Install,
        config_path: "/h/.claude/settings.json".into(),
        fingerprint: "absent".into(),
        outcome: ChangeOutcome::CreateFile,
        before: None,
        after: Some("{}\n".into()),
        diff: "d".into(),
        binary: sample_binary(),
        notes: vec![],
    };
    let wire = serde_json::to_value(&preview).unwrap();
    assert_eq!(wire["harness"], "claudeCode");
    assert_eq!(wire["kind"], "install");
    assert_eq!(wire["outcome"], "createFile");
    assert_null(&wire, "before");
    assert_eq!(wire["configPath"], "/h/.claude/settings.json");
    assert_null(&wire["binary"], "bundledPath");
    assert_eq!(wire["binary"]["status"], "unverified");

    let ready = PreviewResult::Ready {
        preview: preview.clone(),
    };
    let wire = serde_json::to_value(&ready).unwrap();
    assert_eq!(wire["result"], "ready");
    assert_eq!(wire["preview"]["fingerprint"], "absent");
    assert_wire(
        &PreviewResult::Refused {
            reason: RefusalReason::DisabledByPolicy,
            message: "no".into(),
        },
        json!({ "result": "refused", "reason": "disabledByPolicy", "message": "no" }),
    );
    let stale = ApplyResult::Stale { preview };
    assert_eq!(serde_json::to_value(&stale).unwrap()["result"], "stale");
    let wire = serde_json::to_value(&ApplyResult::Applied {
        outcome: ChangeOutcome::RestoreOriginal,
        status: HarnessHookStatus {
            harness: HookHarness::Codex,
            config_path: "/h/.codex/hooks.json".into(),
            state: HookState::NotInstalled,
            events: vec!["Stop".into()],
        },
    })
    .unwrap();
    assert_eq!(wire["result"], "applied");
    assert_eq!(wire["outcome"], "restoreOriginal");
    assert_eq!(wire["status"]["state"], json!({ "kind": "notInstalled" }));
}

#[test]
fn collector_health_wire_format() {
    let health = CollectorHealth {
        socket_path: "/run/why/collector.sock".into(),
        status: CollectorState::Disabled {
            reason: "in use".into(),
        },
        events_received: Some(3),
        spool_ingested: None,
        spool_files: Some(0),
        unavailable: vec!["lastEventAt".into()],
    };
    let wire = serde_json::to_value(&health).unwrap();
    assert_eq!(
        wire["status"],
        json!({ "kind": "disabled", "reason": "in use" })
    );
    assert_eq!(wire["eventsReceived"], 3);
    assert_null(&wire, "spoolIngested");
    assert_eq!(wire["unavailable"], json!(["lastEventAt"]));
    assert_wire(&CollectorState::Listening, json!({ "kind": "listening" }));
    assert_wire(&HookHarness::Codex, json!("codex"));
}

// WHY-53: lazy rationale contracts.
#[test]
fn rationale_wire_format() {
    let ok = RationaleResult::Ok {
        rationale: Rationale {
            evidence: EvidenceLabel::Declared,
            source: TranscriptSource::ClaudeTranscript,
            blocks: vec![RationaleBlock {
                text: "I'll fix the off-by-one first.".into(),
                position: BlockPosition::Before,
                timestamp: Some("2026-11-20T08:00:00Z".into()),
                encrypted: false,
            }],
            harness_version: Some("2.1.322".into()),
            version_supported: true,
            truncated: false,
        },
    };
    assert_wire(
        &ok,
        json!({
            "status": "ok",
            "rationale": {
                "evidence": "declared",
                "source": "claudeTranscript",
                "blocks": [{
                    "text": "I'll fix the off-by-one first.",
                    "position": "before",
                    "timestamp": "2026-11-20T08:00:00Z",
                    "encrypted": false
                }],
                "harnessVersion": "2.1.322",
                "versionSupported": true,
                "truncated": false
            }
        }),
    );
    // A reason is a value, not an error.
    assert_wire(
        &RationaleResult::Unavailable {
            reason: TranscriptUnavailable::OutsideRoot,
        },
        json!({ "status": "unavailable", "reason": "outsideRoot" }),
    );
}

#[test]
fn transcript_unavailable_reasons_are_stable_strings() {
    for (reason, wire) in [
        (TranscriptUnavailable::NoTranscript, "noTranscript"),
        (TranscriptUnavailable::Missing, "missing"),
        (TranscriptUnavailable::OutsideRoot, "outsideRoot"),
        (
            TranscriptUnavailable::UnsupportedPlatform,
            "unsupportedPlatform",
        ),
        (TranscriptUnavailable::NotFound, "notFound"),
        (TranscriptUnavailable::Unreadable, "unreadable"),
        (
            TranscriptUnavailable::UnsupportedHarness,
            "unsupportedHarness",
        ),
        (TranscriptUnavailable::NotApplicable, "notApplicable"),
    ] {
        assert_wire(&reason, json!(wire));
    }
}

#[test]
fn shell_outcomes_wire_format() {
    let value = ShellOutcomesResult::Ok {
        result: ShellOutcomes {
            outcomes: vec![ShellOutcome {
                tool_use_id: "exec-1".into(),
                event_id: "abc:0".into(),
                turn_id: "t1".into(),
                agent_id: None,
                exit_code: 3,
                outcome: ToolOutcome::Failed,
                evidence: EvidenceLabel::Observed,
                source: TranscriptSource::CodexRollout,
            }],
            unresolved: 1,
            harness_version: Some("0.170.0".into()),
            version_supported: true,
            truncated: false,
        },
    };
    assert_wire(
        &value,
        json!({
            "status": "ok",
            "result": {
                "outcomes": [{
                    "toolUseId": "exec-1",
                    "eventId": "abc:0",
                    "turnId": "t1",
                    "agentId": null,
                    "exitCode": 3,
                    "outcome": "failed",
                    "evidence": "observed",
                    "source": "codexRollout"
                }],
                "unresolved": 1,
                "harnessVersion": "0.170.0",
                "versionSupported": true,
                "truncated": false
            }
        }),
    );
    assert_wire(
        &ShellOutcomesResult::Unavailable {
            reason: TranscriptUnavailable::NotApplicable,
        },
        json!({ "status": "unavailable", "reason": "notApplicable" }),
    );
}

// ---- WHY-64: line provenance ------------------------------------------------

#[test]
fn line_origins_result_wire_format() {
    assert_wire(
        &LineOriginsResult::Unavailable {
            reason: LineOriginsUnavailable::EngineNotReady,
        },
        json!({ "status": "unavailable", "reason": "engineNotReady" }),
    );
    for (reason, wire) in [
        (LineOriginsUnavailable::NotText, "notText"),
        (LineOriginsUnavailable::TooLarge, "tooLarge"),
    ] {
        assert_eq!(serde_json::to_value(reason).unwrap(), json!(wire));
    }

    let mut turn = sample_turn_ref(ToolOutcome::Ok);
    turn.agent_id = None;
    let origins = FileLineOrigins {
        worktree_id: WorktreeId("wt-1".into()),
        path: "src/a.rs".into(),
        line_count: 6,
        runs: vec![
            LineRun {
                start_line: 1,
                end_line: 2,
                origin: LineOriginKind::Base,
                evidence: EvidenceLabel::Observed,
            },
            LineRun {
                start_line: 3,
                end_line: 5,
                origin: LineOriginKind::Edit { turn },
                evidence: EvidenceLabel::Observed,
            },
            LineRun {
                start_line: 6,
                end_line: 6,
                origin: LineOriginKind::Unknown {
                    reason: LineUnknownReason::AmbiguousMatch,
                },
                evidence: EvidenceLabel::Unknown,
            },
        ],
        coverage: LineCoverage {
            lines: 6,
            base: 2,
            unknown: 1,
            explained: vec![HarnessLines {
                harness: Harness::Codex,
                lines: 3,
            }],
        },
    };
    let value = serde_json::to_value(LineOriginsResult::Ok {
        origins: origins.clone(),
    })
    .unwrap();
    assert_null(&value["origins"]["runs"][1]["origin"]["turn"], "agentId");
    assert_wire(
        &LineOriginsResult::Ok { origins },
        json!({
            "status": "ok",
            "origins": {
                "worktreeId": "wt-1",
                "path": "src/a.rs",
                "lineCount": 6,
                "runs": [
                    { "startLine": 1, "endLine": 2,
                      "origin": { "kind": "base" }, "evidence": "observed" },
                    { "startLine": 3, "endLine": 5,
                      "origin": { "kind": "edit", "turn": {
                          "harness": "codex",
                          "sessionId": "codex:s1",
                          "turnId": "t1",
                          "turnIndex": 2,
                          "agentId": null,
                          "toolUseId": "call_1",
                          "outcome": "ok",
                          "at": "2026-11-20T15:28:47.123Z",
                          "path": "src/a.rs"
                      } },
                      "evidence": "observed" },
                    { "startLine": 6, "endLine": 6,
                      "origin": { "kind": "unknown", "reason": "ambiguousMatch" },
                      "evidence": "unknown" }
                ],
                "coverage": {
                    "lines": 6, "base": 2, "unknown": 1,
                    "explained": [ { "harness": "codex", "lines": 3 } ]
                }
            }
        }),
    );
}

#[test]
fn line_unknown_reasons_are_stable_strings_and_there_is_no_manual() {
    let all = [
        (LineUnknownReason::NoCapture, "noCapture"),
        (LineUnknownReason::PayloadReduced, "payloadReduced"),
        (LineUnknownReason::Unparsed, "unparsed"),
        (LineUnknownReason::ReplayMismatch, "replayMismatch"),
        (LineUnknownReason::AmbiguousMatch, "ambiguousMatch"),
        (LineUnknownReason::ConcurrentWriter, "concurrentWriter"),
        (LineUnknownReason::NotReplayed, "notReplayed"),
    ];
    for (reason, wire) in all {
        assert_eq!(serde_json::to_value(reason).unwrap(), json!(wire));
        assert_eq!(
            serde_json::from_value::<LineUnknownReason>(json!(wire)).unwrap(),
            reason
        );
    }
    for bad in ["manual", "human"] {
        assert!(serde_json::from_value::<LineUnknownReason>(json!(bad)).is_err());
        assert!(
            serde_json::from_value::<LineOriginKind>(json!({ "kind": bad })).is_err(),
            "{bad}"
        );
    }
}
