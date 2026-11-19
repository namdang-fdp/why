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
