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
                "agentId": null
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
