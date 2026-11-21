//! v0.3 edit capture (WHY-65): the exact `EditOp`s the adapters produce, on the
//! real golden fixtures and on synthetic payloads.
//!
//! Rules under test (see `adapters/claude/edit_op.rs`, `adapters/codex/edit_ops.rs`):
//! an op comes ONLY from a successful `PostToolUse`; a failed or rejected edit
//! and a Pre without a Post produce none; unreadable data is `Unparsed`, never
//! a partial guess; hashes follow the one `LineHasher` policy.

use serde_json::{Value, json};
use why_lib::adapters::normalize;
use why_lib::provenance::{
    EditKind, EditOp, HunkLineKind, LineHasher, Observed, UnobservedReason, hash_lines,
};
use why_wire::Envelope;

use crate::{codex_common, common};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn ops_of(envs: &[Envelope]) -> Vec<EditOp> {
    envs.iter()
        .flat_map(normalize)
        .flat_map(|ev| ev.edits)
        .collect()
}

fn claude_env(payload: Value) -> Envelope {
    common::envelope(1, payload)
}

fn codex_env(payload: Value) -> Envelope {
    codex_common::envelope(payload, 1)
}

fn claude_post(tool: &str, input: Value, response: Value) -> Envelope {
    claude_env(common::with(
        common::base("PostToolUse"),
        json!({ "tool_name": tool, "tool_use_id": "t1", "tool_input": input, "tool_response": response }),
    ))
}

fn codex_tool(hook: &str, patch: &str, response: Option<&str>) -> Envelope {
    let mut p = json!({
        "hook_event_name": hook, "session_id": "cx", "turn_id": "t1", "cwd": "/wt",
        "tool_name": "apply_patch", "tool_use_id": "u1", "tool_input": { "command": patch },
    });
    if let Some(r) = response {
        p["tool_response"] = json!(r);
    }
    codex_env(p)
}

const OK: &str =
    "Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess. Updated the following files:\nM x\n";

fn unparsed(op: &EditOp) -> bool {
    op.observed == Observed::Unobserved(UnobservedReason::Unparsed)
        && op.hunks.is_empty()
        && op.before.is_none()
        && op.after.is_none()
}

fn kind_name(k: EditKind) -> &'static str {
    match k {
        EditKind::Create => "create",
        EditKind::Update => "update",
        EditKind::Delete => "delete",
    }
}

// ---------------------------------------------------------------------------
// golden fixtures: Claude
// ---------------------------------------------------------------------------

/// The number of successful Write/Edit PostToolUse payloads in a scenario.
fn successful_claude_edits(envs: &[Envelope]) -> usize {
    envs.iter()
        .filter(|e| {
            e.payload["hook_event_name"] == "PostToolUse"
                && matches!(e.payload["tool_name"].as_str(), Some("Write" | "Edit"))
        })
        .count()
}

#[test]
fn every_claude_fixture_yields_one_exact_op_per_successful_write_or_edit() {
    let mut total = 0;
    for (name, envs) in common::scenarios() {
        let ops = ops_of(&envs);
        assert_eq!(ops.len(), successful_claude_edits(&envs), "{name}");
        assert!(
            ops.iter().all(|o| o.observed == Observed::Exact),
            "{name}: {ops:?}"
        );
        // Adapters leave the worktree to ingest.
        assert!(ops.iter().all(|o| o.worktree_id.is_none()), "{name}");
        total += ops.len();
    }
    assert!(
        total >= 4,
        "the fixtures hold at least four edits, got {total}"
    );
}

/// The brief's cross-check: the Context/Removed hashes of every hunk equal
/// `LineHasher` over the same lines of `originalFile` at `old_start`.
#[test]
fn claude_hunk_hashes_agree_with_the_original_file_at_old_start() {
    let mut checked = 0;
    for (name, envs) in common::scenarios() {
        for env in &envs {
            let p = &env.payload;
            if p["hook_event_name"] != "PostToolUse" {
                continue;
            }
            let Some(original) = p["tool_response"]["originalFile"].as_str() else {
                continue;
            };
            let ops = ops_of(std::slice::from_ref(env));
            let op = &ops[0];
            let file_lines = hash_lines(original.as_bytes());
            assert_eq!(op.before.as_ref().unwrap().lines, file_lines, "{name}");
            for hunk in &op.hunks {
                let start = hunk.old_start.unwrap() as usize;
                let old_side: Vec<_> = hunk
                    .lines
                    .iter()
                    .filter(|l| l.kind != HunkLineKind::Added)
                    .map(|l| l.hash)
                    .collect();
                // `oldStart` is 1-based; a hunk of only added lines has no old
                // side to compare (and then `old_start` is "after which line").
                if !old_side.is_empty() {
                    assert_eq!(
                        &file_lines[start - 1..start - 1 + old_side.len()],
                        &old_side[..],
                        "{name}: hunk at {start}"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 2, "only {checked} hunks cross-checked");
}

#[test]
fn claude_golden_ops_by_scenario() {
    let by = |name: &str| {
        let (_, envs) = common::scenarios()
            .into_iter()
            .find(|(n, _)| n == name)
            .unwrap();
        ops_of(&envs)
    };

    // A Write over an existing file: positions + original + whole new file.
    let ops = by("write-update-and-bash-edits");
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, EditKind::Update);
    assert!(ops[0].before.is_some() && ops[0].after.is_some());
    assert!(ops[0].path.ends_with("src/greet.py"));

    // A Write create, then two Edits (the rejected third one has no op).
    let ops = by("basic-main");
    let kinds: Vec<_> = ops.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [EditKind::Create, EditKind::Update, EditKind::Update],
        "{ops:?}"
    );
    assert!(ops[0].before.is_none() && ops[0].after.is_some());
    assert_eq!(ops[0].hunks.len(), 1);
    assert_eq!(ops[0].hunks[0].old_start, None);
    assert!(ops[1].hunks.iter().all(|h| h.old_start.is_some()));
    assert!(ops[1].after.is_none(), "an Edit carries no whole new file");

    // The linked-worktree scenario: one create, by absolute path in that worktree.
    let ops = by("worktree");
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, EditKind::Create);
    assert!(ops[0].path.starts_with("/TMP/fixture-wt/"));
}

// ---------------------------------------------------------------------------
// golden fixtures: Codex
// ---------------------------------------------------------------------------

fn codex_scenario(name: &str) -> Vec<Envelope> {
    let (_, path) = codex_common::scenarios()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no codex scenario {name}"));
    codex_common::load(&path)
}

#[test]
fn codex_golden_ops_by_scenario() {
    // A rejected patch: a Pre and no Post, so no op at all.
    assert!(ops_of(&codex_scenario("failing-patch-and-shell-edit")).is_empty());

    // Add, Update, Delete, each a patch of its own (Posts that said Success).
    let ops = ops_of(&codex_scenario("basic-main"));
    let kinds: Vec<_> = ops.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [EditKind::Create, EditKind::Update, EditKind::Delete],
        "{ops:?}"
    );
    assert!(ops.iter().all(|o| o.observed == Observed::Exact));
    assert!(
        ops.iter()
            .all(|o| o.before.is_none() && o.after.is_none() && o.worktree_id.is_none())
    );
    assert!(
        ops[1]
            .hunks
            .iter()
            .all(|h| h.old_start.is_none() && !h.lines.is_empty())
    );
    assert!(ops[2].hunks.is_empty());

    // The linked worktree: a RELATIVE patch path becomes absolute under the cwd.
    let ops = ops_of(&codex_scenario("worktree"));
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, EditKind::Create);
    assert!(
        ops[0].path.starts_with("/TMP/fixture-wt/") && ops[0].path.ends_with("src/cx_wt.py"),
        "{}",
        ops[0].path
    );
}

/// `cargo test -- --nocapture edit_op_table` prints the table of the report.
#[test]
fn edit_op_table() {
    let mut rows: Vec<(String, Vec<EditOp>)> = Vec::new();
    for (name, envs) in common::scenarios() {
        rows.push((format!("claude/2.1.322/{name}"), ops_of(&envs)));
    }
    for (name, path) in codex_common::scenarios() {
        rows.push((
            format!("codex/0.170.0/{}", name.replace("__", "/")),
            ops_of(&codex_common::load(&path)),
        ));
    }
    println!("\n{:<62} {:>3}  kinds / unobserved", "fixture", "ops");
    for (name, ops) in rows.iter().filter(|(_, o)| !o.is_empty()) {
        let kinds: Vec<_> = ops.iter().map(|o| kind_name(o.kind)).collect();
        let unobserved = ops.iter().filter(|o| o.observed != Observed::Exact).count();
        println!(
            "{name:<62} {:>3}  {} / {unobserved}",
            ops.len(),
            kinds.join(",")
        );
    }
    let empty = rows.iter().filter(|(_, o)| o.is_empty()).count();
    println!("(and {empty} fixtures with no edit op)");
    // Nothing in the real fixtures is unobserved: they are whole, well-formed payloads.
    assert!(
        rows.iter()
            .flat_map(|(_, o)| o)
            .all(|o| o.observed == Observed::Exact)
    );
}

// ---------------------------------------------------------------------------
// synthetic: Claude
// ---------------------------------------------------------------------------

#[test]
fn claude_edit_with_two_hunks_through_normalize() {
    let original = "a\nb\nc\nd\ne\nf\ng\nh\n";
    let response = json!({
        "filePath": "/TMP/fixture-repo/x.rs", "originalFile": original, "userModified": false,
        "structuredPatch": [
            { "oldStart": 1, "oldLines": 3, "newStart": 1, "newLines": 3,
              "lines": [" a", "-b", "+B", " c"] },
            { "oldStart": 6, "oldLines": 3, "newStart": 6, "newLines": 3,
              "lines": [" f", "-g", "+G", " h"] },
        ],
    });
    let ops = ops_of(&[claude_post(
        "Edit",
        json!({ "file_path": "/TMP/fixture-repo/x.rs" }),
        response,
    )]);
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].path, "/TMP/fixture-repo/x.rs");
    assert_eq!(ops[0].hunks.len(), 2);
    assert_eq!(ops[0].hunks[1].old_start, Some(6));
}

#[test]
fn claude_write_create_and_overwrite_through_normalize() {
    let create = ops_of(&[claude_post(
        "Write",
        json!({ "file_path": "/r/n.py" }),
        json!({ "type": "create", "filePath": "/r/n.py", "content": "x\ny\n",
                "structuredPatch": [], "originalFile": null, "userModified": false }),
    )]);
    assert_eq!(create[0].kind, EditKind::Create);
    assert_eq!(create[0].hunks[0].lines.len(), 2);

    let overwrite = ops_of(&[claude_post(
        "Write",
        json!({ "file_path": "/r/n.py" }),
        json!({ "type": "update", "filePath": "/r/n.py", "content": "x\nZ\n", "originalFile": "x\ny\n",
                "structuredPatch": [{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
                    "lines": [" x", "-y", "+Z"] }], "userModified": false }),
    )]);
    assert_eq!(overwrite[0].kind, EditKind::Update);
    assert_eq!(
        overwrite[0].after,
        Some(LineHasher::default().fingerprint(b"x\nZ\n"))
    );
}

#[test]
fn a_crlf_original_hashes_like_its_lf_twin() {
    let patch = |lf: bool| {
        let (nl, cr) = if lf { ("\n", "") } else { ("\r\n", "\r") };
        claude_post(
            "Edit",
            json!({ "file_path": "/r/f" }),
            json!({ "filePath": "/r/f", "originalFile": format!("a{nl}b{nl}"), "userModified": false,
                "structuredPatch": [{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
                    "lines": [format!(" a{cr}"), format!("-b{cr}"), format!("+c{cr}")] }] }),
        )
    };
    let lf = ops_of(&[patch(true)]);
    let crlf = ops_of(&[patch(false)]);
    assert_eq!(lf[0].hunks, crlf[0].hunks);
    assert_eq!(lf[0].before, crlf[0].before);
}

#[test]
fn a_no_newline_marker_and_user_modified_are_handled() {
    let marker = ops_of(&[claude_post(
        "Edit",
        json!({ "file_path": "/r/f" }),
        json!({ "filePath": "/r/f", "originalFile": "a\nb", "userModified": false,
            "structuredPatch": [{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
                "lines": [" a", "-b", "\\ No newline at end of file", "+c"] }] }),
    )]);
    assert_eq!(marker[0].observed, Observed::Exact);
    assert_eq!(
        marker[0].hunks[0].lines.len(),
        3,
        "the marker is not a line"
    );
    assert!(!marker[0].before.as_ref().unwrap().fingerprint.final_newline);

    let modified = ops_of(&[claude_post(
        "Write",
        json!({ "file_path": "/r/f" }),
        json!({ "type": "update", "filePath": "/r/f", "content": "a\nB\n", "originalFile": "a\nb\n",
            "userModified": true,
            "structuredPatch": [{ "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
                "lines": [" a", "-b", "+B"] }] }),
    )]);
    assert_eq!(modified[0].observed, Observed::Exact);
    assert!(modified[0].after.is_none(), "content cannot be trusted");
}

#[test]
fn garbage_and_unreadable_edits_are_unparsed_with_their_path() {
    for response in [
        json!({ "structuredPatch": [{ "oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1,
                "lines": ["no prefix"] }], "originalFile": "x\n" }),
        json!({ "structuredPatch": "nope", "originalFile": "x" }),
        json!("a plain string"),
        json!(null),
    ] {
        let ops = ops_of(&[claude_post(
            "Edit",
            json!({ "file_path": "/r/f" }),
            response.clone(),
        )]);
        assert_eq!(ops.len(), 1, "{response}");
        assert!(unparsed(&ops[0]), "{response}");
        assert_eq!(ops[0].path, "/r/f");
    }
    // NotebookEdit (no reader) and an edit with no path at all.
    let nb = ops_of(&[claude_post(
        "NotebookEdit",
        json!({ "notebook_path": "/r/n.ipynb" }),
        json!({ "ok": true }),
    )]);
    assert!(unparsed(&nb[0]));
    assert_eq!(nb[0].path, "/r/n.ipynb");
    assert!(ops_of(&[claude_post("Edit", json!({}), json!({ "x": 1 }))]).is_empty());
}

#[test]
fn failed_rejected_and_started_edits_have_no_op() {
    let input = json!({ "file_path": "/r/f", "old_string": "a", "new_string": "b" });
    // Pre.
    let pre = claude_env(common::with(
        common::base("PreToolUse"),
        json!({ "tool_name": "Edit", "tool_use_id": "t1", "tool_input": input }),
    ));
    // PostToolUseFailure.
    let failed = claude_env(common::with(
        common::base("PostToolUseFailure"),
        json!({ "tool_name": "Edit", "tool_use_id": "t1", "tool_input": input, "error": "boom" }),
    ));
    // A harness-rejected edit: only a PostToolBatch entry exists.
    let rejected = claude_env(common::with(
        common::base("PostToolBatch"),
        json!({ "tool_calls": [{ "tool_name": "Edit", "tool_use_id": "t2", "tool_input": input,
            "tool_response": "<tool_use_error>String to replace not found</tool_use_error>" }] }),
    ));
    for env in [pre, failed, rejected] {
        let events = normalize(&env);
        assert!(!events.is_empty());
        assert!(events.iter().all(|e| e.edits.is_empty()), "{env:?}");
    }
}

#[test]
fn a_reduced_claude_payload_is_unobserved_with_the_kept_path() {
    // What the hook sends for a 2 MB Write: ids, the marker, and the path.
    let reduced = claude_env(json!({
        "session_id": "s1", "hook_event_name": "PostToolUse", "tool_name": "Write",
        "tool_use_id": "t1", "cwd": "/TMP/fixture-repo",
        "tool_input": { "file_path": "/TMP/fixture-repo/big.py" },
        "truncated": true, "original_bytes": 2_000_000u64,
    }));
    let ops = ops_of(std::slice::from_ref(&reduced));
    assert_eq!(ops.len(), 1);
    assert_eq!(
        ops[0].observed,
        Observed::Unobserved(UnobservedReason::PayloadReduced)
    );
    assert_eq!(ops[0].path, "/TMP/fixture-repo/big.py");
    assert!(ops[0].hunks.is_empty() && ops[0].before.is_none());
    // The timeline still shows the file for the reduced edit.
    assert_eq!(normalize(&reduced)[0].files, ["/TMP/fixture-repo/big.py"]);

    // No path kept (older hook, or a path the hook refused): nothing to attach to.
    let pathless = claude_env(json!({
        "session_id": "s1", "hook_event_name": "PostToolUse", "tool_name": "Write",
        "tool_use_id": "t1", "truncated": true, "original_bytes": 9,
    }));
    assert!(ops_of(&[pathless]).is_empty());
    // Redacted: never an op, even if a path were present.
    let redacted = claude_env(json!({
        "session_id": "s1", "hook_event_name": "PostToolUse", "tool_name": "Write",
        "tool_use_id": "t1", "tool_input": { "file_path": "/r/x" },
        "redacted": true, "original_bytes": 9,
    }));
    assert!(ops_of(&[redacted]).is_empty());
}

// ---------------------------------------------------------------------------
// synthetic: Codex
// ---------------------------------------------------------------------------

#[test]
fn a_codex_pre_without_post_failed_and_unknown_outcomes_have_no_op() {
    let patch = "*** Begin Patch\n*** Add File: a.py\n+x\n*** End Patch";
    let pre = codex_tool("PreToolUse", patch, None);
    let failed = codex_tool(
        "PostToolUse",
        patch,
        Some("Exit code: 1\nWall time: 0 seconds\nOutput:\nerror\n"),
    );
    let unknown = codex_tool("PostToolUse", patch, Some("who knows"));
    let no_success_line = codex_tool("PostToolUse", patch, Some("Exit code: 0\nOutput:\nhmm"));
    for env in [pre, failed, unknown, no_success_line] {
        assert!(ops_of(&[env]).is_empty());
    }
    // The Post that said Success does.
    assert_eq!(
        ops_of(&[codex_tool("PostToolUse", patch, Some(OK))]).len(),
        1
    );
}

#[test]
fn codex_add_update_move_and_delete_in_one_patch() {
    let patch = "*** Begin Patch\n\
        *** Add File: new.py\n+a\n+b\n\
        *** Update File: calc.py\n@@ def add():\n     return 1\n-old\n+new\n\
        *** Update File: old.py\n*** Move to: moved.py\n@@\n x\n-y\n+z\n\
        *** Delete File: dead.py\n\
        *** End Patch";
    let ops = ops_of(&[codex_tool("PostToolUse", patch, Some(OK))]);
    let shape: Vec<_> = ops
        .iter()
        .map(|o| {
            (
                kind_name(o.kind),
                o.path.as_str(),
                o.renamed_from.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            ("create", "/wt/new.py", None),
            ("update", "/wt/calc.py", None),
            ("update", "/wt/moved.py", Some("/wt/old.py")),
            ("delete", "/wt/dead.py", None),
        ]
    );
    assert!(ops[1].hunks[0].header.is_some());
    assert!(ops.iter().all(|o| o.observed == Observed::Exact));
}

#[test]
fn a_codex_section_with_a_parse_problem_is_unparsed_for_that_file_only() {
    let patch = "*** Begin Patch\n\
        *** Update File: good.py\n@@\n-a\n+b\n\
        *** Update File: bad.py\n@@\n-a\n??? what\n+b\n\
        *** End Patch";
    let ops = ops_of(&[codex_tool("PostToolUse", patch, Some(OK))]);
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0].observed, Observed::Exact);
    assert!(unparsed(&ops[1]));
    assert_eq!(ops[1].path, "/wt/bad.py");
}

#[test]
fn a_reduced_codex_payload_has_no_op_because_the_paths_are_in_the_patch() {
    let reduced = codex_env(json!({
        "hook_event_name": "PostToolUse", "session_id": "cx", "turn_id": "t1", "cwd": "/wt",
        "tool_name": "apply_patch", "tool_use_id": "u1",
        "truncated": true, "original_bytes": 3_000_000u64,
    }));
    assert!(ops_of(&[reduced]).is_empty());
}

#[test]
fn hostile_codex_patches_never_panic() {
    for patch in [
        "",
        "*** Begin Patch",
        "*** Begin Patch\n*** End Patch",
        "*** Update File:\n",
        "\u{0}\u{1}*** Add File: \u{0}\n+x",
        &"*** Add File: a\n+x\n".repeat(2000),
    ] {
        let _ = ops_of(&[codex_tool("PostToolUse", patch, Some(OK))]);
    }
    // Not even a string.
    let weird = codex_env(json!({
        "hook_event_name": "PostToolUse", "session_id": "cx", "cwd": "/wt",
        "tool_name": "apply_patch", "tool_input": { "command": 7 }, "tool_response": OK,
    }));
    assert!(ops_of(&[weird]).is_empty());
}

#[test]
fn a_codex_context_line_that_looks_like_a_marker_does_not_end_the_patch() {
    // ` *** End Patch` is an unchanged line of a markdown file; the second
    // section must still be read, and both are exact.
    let patch = "*** Begin Patch\n*** Update File: a.md\n@@\n before\n *** End Patch\n-old\n+new\n\
                 *** Update File: b.md\n@@\n-x\n+y\n*** End Patch";
    let ops = ops_of(&[codex_tool("PostToolUse", patch, Some(OK))]);
    assert_eq!(ops.len(), 2);
    assert!(ops.iter().all(|o| o.observed == Observed::Exact));
    assert_eq!(ops[0].hunks[0].lines.len(), 4);
    assert_eq!(ops[1].path, "/wt/b.md");
    // The timeline lists both files too (this was a v0.2 bug: the second
    // section was lost).
    let events = normalize(&codex_tool("PostToolUse", patch, Some(OK)));
    assert_eq!(events[0].files, ["/wt/a.md", "/wt/b.md"]);
}
