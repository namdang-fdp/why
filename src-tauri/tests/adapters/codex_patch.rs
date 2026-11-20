//! The `apply_patch` parser, on its own and through the adapter.

use serde_json::json;
use why_lib::adapters::codex::patch::{HunkLine, MAX_EDITS, PatchOp, parse_patch};
use why_lib::adapters::normalize;
use why_lib::events::{EXCERPT_MAX_BYTES, EventKind};

use crate::codex_common::envelope;

#[test]
fn add_update_delete_move_in_one_patch() {
    // Built from a line list: a `\` string continuation would eat the leading
    // space of the context line.
    let text = [
        "*** Begin Patch",
        "*** Add File: /r/new.py",
        "+one",
        "+two",
        "*** Update File: /r/old.py",
        "@@ def f():",
        " keep",
        "-gone",
        "+new",
        "@@",
        "+tail",
        "*** Update File: /r/a.py",
        "*** Move to: /r/b.py",
        "@@",
        "-x",
        "+y",
        "*** Delete File: /r/dead.py",
        "*** End Patch",
        "",
    ]
    .join("\n");
    let p = parse_patch(&text);
    assert!(p.is_clean(), "{:?}", p.problems);
    assert_eq!(p.edits.len(), 4);

    assert_eq!(p.edits[0].op, PatchOp::Add);
    assert_eq!(p.edits[0].added(), 2);

    let upd = &p.edits[1];
    assert_eq!(upd.op, PatchOp::Update);
    assert_eq!(upd.hunks.len(), 2);
    assert_eq!(upd.hunks[0].header.as_deref(), Some("def f():"));
    assert_eq!(
        upd.hunks[0].lines,
        vec![
            HunkLine::Context("keep".into()),
            HunkLine::Removed("gone".into()),
            HunkLine::Added("new".into())
        ]
    );
    assert_eq!((upd.added(), upd.removed()), (2, 1));

    assert_eq!(p.edits[2].move_to.as_deref(), Some("/r/b.py"));
    assert_eq!(p.edits[3].op, PatchOp::Delete);
    assert!(p.edits[3].hunks.is_empty());
}

#[test]
fn update_without_at_at_gets_an_implicit_hunk() {
    let p = parse_patch("*** Begin Patch\n*** Update File: f\n-a\n+b\n*** End Patch");
    assert!(p.is_clean());
    assert_eq!(p.edits[0].hunks.len(), 1);
    assert_eq!(p.edits[0].hunks[0].header, None);
}

#[test]
fn crlf_patches_parse_like_lf_ones() {
    let lf = "*** Begin Patch\n*** Add File: /r/x\n+a\n+b\n*** End Patch\n";
    let crlf = lf.replace('\n', "\r\n");
    let (a, b) = (parse_patch(lf), parse_patch(&crlf));
    assert!(b.is_clean(), "{:?}", b.problems);
    assert_eq!(a, b, "same edits, no stray \\r in paths or content");
}

#[test]
fn a_lone_cr_inside_content_is_kept() {
    // Only one line has \r: it is file content (a CRLF file), not the
    // patch's own line ending.
    let p = parse_patch("*** Begin Patch\n*** Add File: /r/x\n+a\r\n+b\n*** End Patch");
    assert_eq!(p.edits[0].hunks[0].lines[0], HunkLine::Added("a\r".into()));
}

#[test]
fn missing_end_patch_keeps_what_was_read() {
    let p = parse_patch("*** Begin Patch\n*** Add File: /r/x\n+a");
    assert!(!p.complete);
    assert_eq!(p.edits.len(), 1);
    assert_eq!(p.edits[0].added(), 1);
    assert!(p.problems.iter().any(|m| m.contains("End Patch")));
}

#[test]
fn empty_hunks_and_empty_patches_are_fine() {
    let p = parse_patch("*** Begin Patch\n*** Update File: f\n@@\n@@\n*** End Patch");
    assert!(p.is_clean());
    assert!(p.edits[0].hunks.is_empty());
    let p = parse_patch("*** Begin Patch\n*** End Patch");
    assert!(p.is_clean() && p.edits.is_empty());
    let p = parse_patch("");
    assert!(p.edits.is_empty() && !p.complete);
}

#[test]
fn paths_with_spaces_and_unicode_are_preserved() {
    let p = parse_patch(
        "*** Begin Patch\n*** Add File: /r/my dir/tệp mới 日本.py\n+x\n\
         *** Update File: /r/a b.py\n*** Move to: /r/c d.py\n@@\n-1\n+2\n*** End Patch",
    );
    assert_eq!(p.edits[0].path, "/r/my dir/tệp mới 日本.py");
    assert_eq!(p.edits[1].path, "/r/a b.py");
    assert_eq!(p.edits[1].move_to.as_deref(), Some("/r/c d.py"));
}

#[test]
fn malformed_input_is_reported_not_fatal() {
    for text in [
        "not a patch at all",
        "*** Begin Patch\n*** Frobnicate: x\n*** End Patch",
        "*** Begin Patch\n*** Add File:\n+x\n*** End Patch",
        "*** Begin Patch\n+orphan line\n*** End Patch",
        "*** Begin Patch\n*** Update File: f\n?? weird\n*** End Patch",
        "*** End Patch",
        "\u{0}\u{1}\u{2}",
        "*** Begin Patch\n*** Move to: z\n*** End Patch",
    ] {
        let p = parse_patch(text);
        assert!(!p.is_clean(), "{text:?} should not be clean");
    }
}

#[test]
fn every_prefix_of_a_valid_patch_parses_without_panic() {
    // A patch cut off anywhere (including inside a multi-byte character) must
    // not panic: this is the "lost or truncated delivery" case.
    let text = "*** Begin Patch\n*** Add File: /r/日本語.py\n+héllo wörld\n+日本語\n\
*** Update File: /r/b.py\n@@ fn é()\n ctx\n-old\n+new\n*** Delete File: /r/c.py\n*** End Patch\n";
    for end in 0..=text.len() {
        if text.is_char_boundary(end) {
            let _ = parse_patch(&text[..end]);
        }
    }
}

#[test]
fn edit_count_is_capped() {
    let mut text = String::from("*** Begin Patch\n");
    for i in 0..(MAX_EDITS + 50) {
        text.push_str(&format!("*** Delete File: /r/f{i}\n"));
    }
    text.push_str("*** End Patch\n");
    let p = parse_patch(&text);
    assert_eq!(p.edits.len(), MAX_EDITS);
    assert!(p.problems.iter().any(|m| m.contains("more than")));
}

/// A huge patch through the adapter: the event text stays within the excerpt
/// cap, and the files list within the edit cap.
#[test]
fn huge_patch_through_the_adapter_is_capped() {
    let mut patch = String::from("*** Begin Patch\n");
    for i in 0..5000 {
        patch.push_str(&format!(
            "*** Add File: /r/a_very_long_directory_name_to_inflate_the_summary/file_{i}.py\n+x\n"
        ));
    }
    patch.push_str("*** End Patch\n");
    let payload = json!({
        "hook_event_name": "PreToolUse", "session_id": "s", "turn_id": "t",
        "cwd": "/r", "tool_name": "apply_patch", "tool_use_id": "u",
        "tool_input": {"command": patch},
    });
    let events = normalize(&envelope(payload, 1));
    assert_eq!(events.len(), 1);
    let text = events[0].text.as_deref().unwrap();
    assert!(text.len() <= EXCERPT_MAX_BYTES, "{} bytes", text.len());
    assert!(events[0].files.len() <= MAX_EDITS);
}

#[test]
fn adapter_reports_a_malformed_patch_as_an_attempt() {
    let payload = json!({
        "hook_event_name": "PreToolUse", "session_id": "s", "turn_id": "t",
        "cwd": "/r", "tool_name": "apply_patch", "tool_use_id": "u",
        "tool_input": {"command": "garbage"},
    });
    let events = normalize(&envelope(payload, 1));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::ToolStarted);
    assert!(events[0].files.is_empty());
    assert!(
        events[0]
            .text
            .as_deref()
            .unwrap()
            .contains("not understood")
    );
}
