//! The lazy transcript reader on temp files only (nothing under `$HOME`).

#![cfg(target_os = "linux")]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use why_lib::adapters::claude::transcript::{
    Limits, Query, TranscriptError, TranscriptScan, projects_root, read_context,
};

fn line(v: Value) -> String {
    format!("{v}\n")
}

fn user(prompt_id: &str, uuid: &str) -> String {
    line(
        json!({"type":"user","promptId":prompt_id,"uuid":uuid,"version":"2.1.322",
                "message":{"role":"user","content":[{"type":"text","text":"hi"}]}}),
    )
}

fn assistant(uuid: &str, blocks: Value) -> String {
    line(
        json!({"type":"assistant","uuid":uuid,"timestamp":"2026-11-20T08:00:00Z",
                "message":{"role":"assistant","content":blocks}}),
    )
}

fn text(t: &str) -> Value {
    json!({"type":"text","text":t})
}

fn tool_use(id: &str) -> Value {
    json!({"type":"tool_use","id":id,"name":"Bash","input":{"command":"ls"}})
}

/// A temp `projects` root holding `s.jsonl` with `content`.
fn rig(content: &[u8]) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("projects");
    fs::create_dir_all(root.join("-p")).unwrap();
    let file = root.join("-p").join("s.jsonl");
    fs::write(&file, content).unwrap();
    (dir, root, file)
}

fn sample() -> String {
    [
        line(json!({"type":"queue-operation","operation":"enqueue"})),
        user("p1", "u1"),
        assistant("a1", json!([{"type":"thinking","thinking":"","signature":"x"}, text("First I read it."), tool_use("t1")])),
        line(json!({"type":"future-record-type","anything":[1,2,3]})),
        user("p1", "u2"), // tool_result record: same promptId
        assistant("a2", json!([text("Now the edit."), tool_use("t2")])),
        user("p1", "u3"),
        assistant("a3", json!([text("All done.")])),
        user("p2", "u4"),
        assistant("a4", json!([text("Second turn.")])),
    ]
    .concat()
}

#[test]
fn finds_the_narration_before_a_tool_call() {
    let (_d, root, file) = rig(sample().as_bytes());
    let c = read_context(
        &root,
        &file,
        Query::BeforeToolUse("t2"),
        0,
        Limits::default(),
    )
    .unwrap();
    assert!(c.found);
    let texts: Vec<_> = c.texts.iter().map(|t| t.text.as_str()).collect();
    // Only the run since the last user/tool-result record; not "First I read it.".
    assert_eq!(texts, ["Now the edit."]);
    assert_eq!(c.texts[0].uuid.as_deref(), Some("a2"));
    assert_eq!(c.version.as_deref(), Some("2.1.322"));
    assert!(c.version_supported);
    assert_eq!(c.skipped_lines, 0);
}

#[test]
fn finds_all_assistant_text_of_a_turn() {
    let (_d, root, file) = rig(sample().as_bytes());
    let c = read_context(&root, &file, Query::Turn("p1"), 0, Limits::default()).unwrap();
    assert!(c.found);
    let texts: Vec<_> = c.texts.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, ["First I read it.", "Now the edit.", "All done."]);
    // Unknown ids: not found, no texts, no error.
    let none = read_context(&root, &file, Query::Turn("nope"), 0, Limits::default()).unwrap();
    assert!(!none.found && none.texts.is_empty());
    let none = read_context(
        &root,
        &file,
        Query::BeforeToolUse("nope"),
        0,
        Limits::default(),
    )
    .unwrap();
    assert!(!none.found && none.texts.is_empty());
}

#[test]
fn an_unfinished_record_is_parsed_only_after_the_writer_appends_its_newline() {
    let words = assistant("a", json!([text("late words")]));
    let (prefix, suffix) = words.split_at(words.len() - 6);
    let content = format!("{}{prefix}", user("p1", "u"));
    let (_d, root, file) = rig(content.as_bytes());
    let mut scan =
        TranscriptScan::open(&root, &file, Query::Turn("p1"), 0, Limits::default()).unwrap();
    let c = scan.read_page().unwrap();
    assert_eq!(c.next_offset, content.len() as u64);
    assert!(c.found && c.texts.is_empty() && !c.truncated);
    assert_eq!(c.skipped_lines, 0);

    let mut f = fs::OpenOptions::new().append(true).open(&file).unwrap();
    f.write_all(suffix.as_bytes()).unwrap();
    let c2 = scan.read_page().unwrap();
    assert_eq!(c2.next_offset, (content.len() + suffix.len()) as u64);
    assert_eq!(c2.skipped_lines, 0);
    assert!(c2.found);
    assert_eq!(c2.texts[0].text, "late words");
}

#[test]
fn both_queries_keep_their_context_across_pages() {
    let prefix = format!(
        "{}{}",
        user("p1", "u"),
        assistant("a1", json!([text("first")]))
    );
    let second = "second ".repeat(100);
    let content = format!(
        "{prefix}{}{}{}",
        assistant("a2", json!([text(&second), tool_use("wanted")])),
        user("p2", "u2"),
        assistant("a3", json!([text("other turn")]))
    );
    let (_d, root, file) = rig(content.as_bytes());
    let limits = Limits {
        max_bytes: prefix.len() as u64,
        ..Limits::default()
    };
    for query in [Query::Turn("p1"), Query::BeforeToolUse("wanted")] {
        let mut scan = TranscriptScan::open(&root, &file, query, 0, limits).unwrap();
        let mut texts = Vec::new();
        let mut offset = 0;
        loop {
            let c = scan.read_page().unwrap();
            assert!(c.next_offset > offset);
            offset = c.next_offset;
            assert_eq!(c.version.as_deref(), Some("2.1.322"));
            texts.extend(c.texts.into_iter().map(|t| t.text));
            if !c.truncated {
                assert!(c.found);
                break;
            }
        }
        assert_eq!(texts, ["first", second.as_str()]);
    }
}

#[test]
fn a_tool_scan_retains_the_most_recent_narration_when_it_hits_the_limit() {
    let early = assistant("old", json!(vec![text("old"); 1024]));
    let content = format!(
        "{early}{}",
        assistant("tool", json!([text("recent"), tool_use("wanted")]))
    );
    let (_d, root, file) = rig(content.as_bytes());
    let limits = Limits {
        max_bytes: early.len() as u64,
        ..Limits::default()
    };
    let mut scan =
        TranscriptScan::open(&root, &file, Query::BeforeToolUse("wanted"), 0, limits).unwrap();
    let first = scan.read_page().unwrap();
    assert!(!first.found && first.texts.is_empty() && first.truncated);
    let result = scan.read_page().unwrap();
    assert!(result.found && result.narration_truncated);
    assert_eq!(result.texts.len(), 1024);
    assert_eq!(result.texts.last().unwrap().text, "recent");
}

#[test]
fn garbage_and_unknown_records_are_skipped_not_fatal() {
    let content = format!(
        "not json at all\n{{\"half\":\n\u{0}\u{1}\n{}{}{}",
        line(json!({"type":"mystery","x":null})),
        user("p1", "u1"),
        assistant("a1", json!([text("ok"), {"type":"image"}, 5, null])),
    );
    let (_d, root, file) = rig(content.as_bytes());
    let c = read_context(&root, &file, Query::Turn("p1"), 0, Limits::default()).unwrap();
    assert_eq!(c.skipped_lines, 3);
    assert_eq!(c.texts.len(), 1);
}

#[test]
fn unsupported_or_missing_version_is_a_flag_not_an_error() {
    let content = format!(
        "{}{}",
        line(json!({"type":"user","promptId":"p1","version":"9.0.1"})),
        assistant("a", json!([text("hello")])),
    );
    let (_d, root, file) = rig(content.as_bytes());
    let c = read_context(&root, &file, Query::Turn("p1"), 0, Limits::default()).unwrap();
    assert_eq!(c.version.as_deref(), Some("9.0.1"));
    assert!(!c.version_supported);
    assert_eq!(c.texts.len(), 1);

    let (_d, root, file) = rig(assistant("a", json!([text("hello")])).as_bytes());
    let c = read_context(
        &root,
        &file,
        Query::BeforeToolUse("x"),
        0,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(c.version, None);
    assert!(!c.version_supported);
}

#[test]
fn a_50_mb_file_is_read_in_bounded_slices() {
    // Sparse 50 MB of zero bytes: one giant "line" with no newline. The call
    // must return quickly having read at most `max_bytes`.
    let (_d, root, file) = rig(b"");
    let f = fs::OpenOptions::new().write(true).open(&file).unwrap();
    f.set_len(50 * 1024 * 1024).unwrap();
    let limits = Limits {
        max_bytes: 1024 * 1024,
        max_line_bytes: 64 * 1024,
    };
    let c = read_context(&root, &file, Query::Turn("p"), 0, limits).unwrap();
    assert!(c.truncated, "stopped by the byte budget");
    assert!(!c.found);
    assert_eq!(
        c.next_offset, limits.max_bytes,
        "skip through the giant line"
    );
    assert_eq!(c.skipped_lines, 0, "the line has no newline yet");

    // Many real lines: it can also stop within an ordinary record.
    let big: String = (0..20_000)
        .map(|i| assistant(&format!("a{i}"), json!([text(&"x".repeat(200))])))
        .collect();
    let (_d, root, file) = rig(big.as_bytes());
    let limits = Limits {
        max_bytes: 256 * 1024,
        max_line_bytes: 64 * 1024,
    };
    let c = read_context(&root, &file, Query::Turn("p"), 0, limits).unwrap();
    assert!(c.truncated);
    assert_eq!(c.next_offset, limits.max_bytes);
}

#[test]
fn an_oversize_line_is_skipped_once_across_several_pages() {
    let huge = line(json!({"type":"attachment","blob":"x".repeat(2000)}));
    let content = format!(
        "{huge}{}{}",
        user("p1", "u"),
        assistant("a", json!([text("after")]))
    );
    let (_d, root, file) = rig(content.as_bytes());
    let limits = Limits {
        max_bytes: 200,
        max_line_bytes: 500,
    };
    let mut scan = TranscriptScan::open(&root, &file, Query::Turn("p1"), 0, limits).unwrap();
    let mut texts = Vec::new();
    let mut skipped = 0;
    let mut offset = 0;
    for _ in 0..content.len().div_ceil(limits.max_bytes as usize) {
        let c = scan.read_page().unwrap();
        assert!(c.next_offset > offset && c.next_offset - offset <= limits.max_bytes);
        offset = c.next_offset;
        skipped += c.skipped_lines;
        texts.extend(c.texts.into_iter().map(|t| t.text));
    }
    assert_eq!(offset, content.len() as u64);
    assert_eq!(skipped, 1);
    assert_eq!(texts, ["after"]);
}

#[test]
fn long_text_is_capped_with_the_shared_excerpt_rule() {
    let content = format!(
        "{}{}",
        user("p1", "u"),
        assistant("a", json!([text(&"世".repeat(20_000))]))
    );
    let (_d, root, file) = rig(content.as_bytes());
    let c = read_context(&root, &file, Query::Turn("p1"), 0, Limits::default()).unwrap();
    assert!(c.texts[0].text.len() <= why_lib::events::EXCERPT_MAX_BYTES);
}

#[test]
fn oversized_metadata_is_dropped_before_it_is_copied_into_each_block() {
    let large = "é".repeat(40_000);
    let content = format!(
        "{}{}",
        user("p1", "u"),
        line(json!({"type":"assistant", "uuid":large, "timestamp":large,
            "message":{"content":vec![text("ok"); 256]}}))
    );
    let (_d, root, file) = rig(content.as_bytes());
    let c = read_context(&root, &file, Query::Turn("p1"), 0, Limits::default()).unwrap();
    assert_eq!(c.texts.len(), 256);
    assert!(
        c.texts
            .iter()
            .all(|t| t.uuid.is_none() && t.timestamp.is_none())
    );
}

#[test]
fn missing_file_and_outside_paths_are_refused() {
    let (d, root, file) = rig(sample().as_bytes());
    let q = Query::Turn("p1");

    let missing = root.join("-p").join("gone.jsonl");
    assert!(matches!(
        read_context(&root, &missing, q, 0, Limits::default()),
        Err(TranscriptError::NotFound)
    ));

    // A real file outside the root.
    let outside = d.path().join("elsewhere.jsonl");
    fs::write(&outside, sample()).unwrap();
    assert!(matches!(
        read_context(&root, &outside, q, 0, Limits::default()),
        Err(TranscriptError::OutsideAllowedRoot)
    ));

    // `..` out of the root.
    let sneaky = root
        .join("-p")
        .join("..")
        .join("..")
        .join("elsewhere.jsonl");
    assert!(matches!(
        read_context(&root, &sneaky, q, 0, Limits::default()),
        Err(TranscriptError::OutsideAllowedRoot)
    ));

    // A sibling directory whose name merely starts with the root's name.
    let evil = d.path().join("projects-evil");
    fs::create_dir_all(&evil).unwrap();
    let evil_file = evil.join("s.jsonl");
    fs::write(&evil_file, sample()).unwrap();
    assert!(matches!(
        read_context(&root, &evil_file, q, 0, Limits::default()),
        Err(TranscriptError::OutsideAllowedRoot)
    ));

    // A directory is not a transcript.
    let dir = root.join("-p");
    assert!(matches!(
        read_context(&root, &dir, q, 0, Limits::default()),
        Err(TranscriptError::NotRegularFile)
    ));

    // The honest case still works.
    assert!(read_context(&root, &file, q, 0, Limits::default()).is_ok());
}

#[test]
fn symlinks_in_any_component_and_fifos_are_refused() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    let (d, root, _file) = rig(sample().as_bytes());
    let q = Query::Turn("p1");

    // A link inside the root pointing to a file outside it.
    let secret = d.path().join("secret.txt");
    fs::write(&secret, "{\"type\":\"user\",\"promptId\":\"p1\"}\n").unwrap();
    let link = root.join("-p").join("link.jsonl");
    symlink(&secret, &link).unwrap();
    assert!(matches!(
        read_context(&root, &link, q, 0, Limits::default()),
        Err(TranscriptError::Symlink)
    ));

    let dir_link = root.join("linked-project");
    symlink(d.path(), &dir_link).unwrap();
    assert!(matches!(
        read_context(&root, &dir_link.join("secret.txt"), q, 0, Limits::default()),
        Err(TranscriptError::Symlink)
    ));

    // A FIFO inside the root: must not hang, must be refused.
    let fifo = root.join("-p").join("pipe.jsonl");
    let c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path; mkfifo only creates a node.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(matches!(
        read_context(&root, &fifo, q, 0, Limits::default()),
        Err(TranscriptError::NotRegularFile)
    ));
}

#[test]
fn projects_root_follows_claude_config_dir_then_home() {
    let home = Path::new("/home/u");
    assert_eq!(
        projects_root(Some("/cfg"), Some(home)),
        Some(PathBuf::from("/cfg/projects"))
    );
    for bad in [Some(""), Some("relative/dir"), None] {
        assert_eq!(
            projects_root(bad, Some(home)),
            Some(PathBuf::from("/home/u/.claude/projects")),
            "{bad:?}"
        );
    }
    assert_eq!(projects_root(None, None), None);
    assert_eq!(projects_root(None, Some(Path::new("rel"))), None);
}
