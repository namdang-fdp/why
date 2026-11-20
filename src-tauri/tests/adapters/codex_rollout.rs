//! The lazy rollout reader: real fixture rollouts plus hostile files, all in
//! temp dirs (nothing reads `~/.codex`).

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::json;
use tempfile::TempDir;
use why_lib::adapters::codex::rollout::{
    MAX_LINE_BYTES, MAX_SCAN_BYTES, RolloutContext, RolloutError, read_context,
};

use crate::codex_common::codex_root;

/// A fake `~/.codex/sessions`.
struct Sessions {
    _dir: TempDir,
    root: PathBuf,
}

impl Sessions {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        std::fs::create_dir_all(&root).unwrap();
        Self { _dir: dir, root }
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.root.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn read(
        &self,
        path: &Path,
        turn: &str,
        tool: Option<&str>,
    ) -> Result<RolloutContext, RolloutError> {
        read_context(path, &self.root, turn, tool)
    }
}

// Ids from basic-main/transcript.jsonl (turn 1 = the long one).
const TURN1: &str = "01a1f812-8eb3-7732-9caa-21c0b36398fc";
const TURN2: &str = "01a1f813-3059-70a2-8b37-f7ad14b20d38";
const UNITTEST_NO_TESTS: &str = "exec-66426901-f66d-46f0-9d72-b07658d67571"; // exit 5
const SH_EXIT_3: &str = "exec-e6fcfc9d-ad86-446c-b9c7-225e0dda6701";
const UNITTEST_FAIL: &str = "exec-188bdd44-6624-44f7-8c8a-d5f52c7117c8"; // turn 2, exit 1

fn fixture_rollout(s: &Sessions) -> PathBuf {
    let bytes = std::fs::read(codex_root().join("basic-main/transcript.jsonl")).unwrap();
    s.write("rollout-basic.jsonl", &bytes)
}

fn record(kind: &str, payload: serde_json::Value) -> String {
    format!("{}\n", json!({"type": kind, "payload": payload}))
}

fn meta(version: &str) -> String {
    record(
        "session_meta",
        json!({"cli_version": version, "base_instructions": {"text": "PRIVATE-OWNER-TEXT"}}),
    )
}

fn agent_message(turn: &str, text: &str) -> String {
    record(
        "event_msg",
        json!({"type":"item_completed","turn_id":turn,
               "item":{"type":"AgentMessage","content":[{"type":"Text","text":text}]}}),
    )
}

fn command(turn: &str, id: &str, code: i64) -> String {
    record(
        "event_msg",
        json!({"type":"item_completed","turn_id":turn,
               "item":{"type":"CommandExecution","id":id,"exit_code":code}}),
    )
}

#[test]
fn real_rollout_gives_exit_codes_that_the_hook_cannot() {
    let s = Sessions::new();
    let path = fixture_rollout(&s);

    let c = s.read(&path, TURN1, Some(UNITTEST_NO_TESTS)).unwrap();
    assert_eq!(c.cli_version.as_deref(), Some("0.170.0"));
    assert!(c.version_trusted);
    assert_eq!(c.exit_code, Some(5));
    assert!(!c.truncated);

    assert_eq!(
        s.read(&path, TURN1, Some(SH_EXIT_3)).unwrap().exit_code,
        Some(3)
    );
    assert_eq!(
        s.read(&path, TURN2, Some(UNITTEST_FAIL)).unwrap().exit_code,
        Some(1)
    );
}

#[test]
fn real_rollout_gives_narration_around_a_call() {
    let s = Sessions::new();
    let path = fixture_rollout(&s);
    let c = s.read(&path, TURN1, Some(UNITTEST_NO_TESTS)).unwrap();
    // The agent said this right before running the test command...
    assert!(
        c.narration_before
            .as_deref()
            .unwrap()
            .contains("requested file changes are applied"),
        "{:?}",
        c.narration_before
    );
    // ...and its final answer comes after.
    assert!(c.narration_after.as_deref().unwrap().contains("exit 5"));
    assert!(c.turn_messages.len() >= 3);
}

#[test]
fn turn_only_query_returns_messages_without_an_exit_code() {
    let s = Sessions::new();
    let c = s.read(&fixture_rollout(&s), TURN2, None).unwrap();
    assert_eq!(c.exit_code, None);
    assert_eq!(c.narration_before, None);
    assert_eq!(c.turn_messages.len(), 1);
}

#[test]
fn unknown_tool_use_id_or_turn_returns_nothing() {
    let s = Sessions::new();
    let path = fixture_rollout(&s);
    let c = s.read(&path, TURN1, Some("exec-does-not-exist")).unwrap();
    assert_eq!((c.exit_code, c.narration_before.is_none()), (None, true));
    let c = s.read(&path, "no-such-turn", Some(SH_EXIT_3)).unwrap();
    assert_eq!(c.exit_code, None, "the id belongs to another turn");
}

#[test]
fn private_rollout_content_never_comes_back() {
    let s = Sessions::new();
    // Real-shaped file with owner-specific text in places that must be ignored.
    let mut text = meta("0.170.0");
    text.push_str(&record(
        "response_item",
        json!({"type":"message","role":"developer","content":[{"type":"input_text","text":"PRIVATE-DEV-MSG"}]}),
    ));
    text.push_str(&record(
        "event_msg",
        json!({"type":"token_count","rate_limits":{"plan_type":"PRIVATE-PLAN"}}),
    ));
    text.push_str(&record("world_state", json!({"x": "PRIVATE-WORLD"})));
    text.push_str(&agent_message("t", "hello"));
    let path = s.write("r.jsonl", text.as_bytes());
    let c = s.read(&path, "t", None).unwrap();
    let dump = format!("{c:?}");
    for secret in [
        "PRIVATE-OWNER-TEXT",
        "PRIVATE-DEV-MSG",
        "PRIVATE-PLAN",
        "PRIVATE-WORLD",
    ] {
        assert!(!dump.contains(secret), "{secret} leaked");
    }
    assert_eq!(c.turn_messages, ["hello"]);
}

#[test]
fn encrypted_message_stays_opaque() {
    let s = Sessions::new();
    let blob = format!("gAAAAAB{}", "Zm9v".repeat(50));
    let text = format!("{}{}", meta("0.170.0"), agent_message("t", &blob));
    let c = s
        .read(&s.write("r.jsonl", text.as_bytes()), "t", None)
        .unwrap();
    assert_eq!(c.turn_messages, ["(encrypted)"]);
}

#[test]
fn truncated_last_line_and_garbage_are_skipped() {
    let s = Sessions::new();
    let mut text = meta("0.170.0");
    text.push_str("this is not json\n");
    text.push_str("{\"type\":\"event_msg\",\"payload\":{\"type\":\n"); // cut mid-record
    text.push_str(&agent_message("t", "kept"));
    text.push_str(&command("t", "exec-1", 0));
    text.push_str("{\"type\":\"event_msg\",\"payload\":{\"type\":\"item_comp"); // no newline: a write in progress
    let mut bytes = text.into_bytes();
    bytes.extend_from_slice(&[0xff, 0xfe, b'\n']); // invalid UTF-8 line
    let c = s
        .read(&s.write("r.jsonl", &bytes), "t", Some("exec-1"))
        .unwrap();
    assert_eq!(c.exit_code, Some(0));
    assert_eq!(c.turn_messages, ["kept"]);
}

#[test]
fn unknown_record_types_are_tolerated() {
    let s = Sessions::new();
    let mut text = meta("0.170.0");
    text.push_str(&record("brand_new_record", json!({"anything": [1, 2, 3]})));
    text.push_str(&record("event_msg", json!({"type": "future_event"})));
    text.push_str(&record("event_msg", json!(42)));
    text.push_str("{\"no_type_at_all\": true}\n");
    text.push_str("[1,2,3]\n");
    text.push_str(&command("t", "exec-1", 7));
    let c = s
        .read(&s.write("r.jsonl", text.as_bytes()), "t", Some("exec-1"))
        .unwrap();
    assert_eq!(c.exit_code, Some(7));
}

#[test]
fn a_different_cli_version_degrades_without_guessing() {
    let s = Sessions::new();
    for version in ["0.171.2", "1.0.0", "garbage"] {
        let text = format!(
            "{}{}{}{}",
            meta(version),
            agent_message("t", "before"),
            command("t", "exec-1", 3),
            agent_message("t", "after")
        );
        let c = s
            .read(&s.write("r.jsonl", text.as_bytes()), "t", Some("exec-1"))
            .unwrap();
        assert!(!c.version_trusted, "{version}");
        assert_eq!(
            c.exit_code, None,
            "{version}: shape unverified, no exit code"
        );
        assert_eq!(c.narration_before.as_deref(), Some("before"));
        assert_eq!(c.narration_after.as_deref(), Some("after"));
    }
    // No session_meta at all: same.
    let text = command("t", "exec-1", 3);
    let c = s
        .read(&s.write("r.jsonl", text.as_bytes()), "t", Some("exec-1"))
        .unwrap();
    assert_eq!((c.cli_version, c.exit_code), (None, None));
}

#[test]
fn conflicting_exit_codes_for_one_id_are_not_trusted() {
    let s = Sessions::new();
    let text = format!(
        "{}{}{}",
        meta("0.170.0"),
        command("t", "exec-1", 0),
        command("t", "exec-1", 1)
    );
    let c = s
        .read(&s.write("r.jsonl", text.as_bytes()), "t", Some("exec-1"))
        .unwrap();
    assert_eq!(c.exit_code, None);
}

#[test]
fn oversized_line_is_skipped_and_reading_continues() {
    let s = Sessions::new();
    let mut bytes = meta("0.170.0").into_bytes();
    bytes.extend(std::iter::repeat_n(b'x', MAX_LINE_BYTES * 2));
    bytes.push(b'\n');
    bytes.extend(command("t", "exec-1", 4).into_bytes());
    let c = s
        .read(&s.write("r.jsonl", &bytes), "t", Some("exec-1"))
        .unwrap();
    assert_eq!(c.exit_code, Some(4));
}

#[test]
fn a_50_mb_rollout_is_read_within_the_bound() {
    let s = Sessions::new();
    let path = s.root.join("big.jsonl");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    f.write_all(meta("0.170.0").as_bytes()).unwrap();
    // ~50 MB of unrelated records, then the evidence at the very end.
    let filler = record(
        "response_item",
        json!({"type": "message", "text": "y".repeat(900)}),
    );
    let mut written = 0usize;
    while written < 50 * 1024 * 1024 {
        f.write_all(filler.as_bytes()).unwrap();
        written += filler.len();
    }
    f.write_all(agent_message("t", "late narration").as_bytes())
        .unwrap();
    f.write_all(command("t", "exec-9", 2).as_bytes()).unwrap();
    f.flush().unwrap();
    drop(f);
    assert!(std::fs::metadata(&path).unwrap().len() > 50 * 1024 * 1024);

    let started = std::time::Instant::now();
    let c = s.read(&path, "t", Some("exec-9")).unwrap();
    assert!(c.truncated);
    assert!(
        c.bytes_scanned <= MAX_SCAN_BYTES + 2 * 1024 * 1024 + 1,
        "scanned {} bytes",
        c.bytes_scanned
    );
    assert!(started.elapsed().as_secs() < 20);
    // Evidence in the tail window is still found; the version came from the head.
    assert_eq!(c.exit_code, Some(2));
    assert_eq!(c.narration_before.as_deref(), Some("late narration"));
    assert!(c.version_trusted);
}

#[test]
fn missing_file_is_an_io_error() {
    let s = Sessions::new();
    let err = s.read(&s.root.join("nope.jsonl"), "t", None).unwrap_err();
    assert!(
        matches!(err, RolloutError::Io(ref e) if e.kind() == std::io::ErrorKind::NotFound),
        "{err:?}"
    );
}

#[test]
fn a_complete_record_at_the_start_of_the_tail_window_is_kept() {
    let s = Sessions::new();
    let mut bytes = meta("0.170.0").into_bytes();
    bytes.extend_from_slice(b"filler\n");
    let boundary = bytes.len();
    let evidence = command("t", "exec-1", 4) + &agent_message("t", "after");
    bytes.extend(evidence.as_bytes());
    // Position the command record exactly at `file_len - MAX_SCAN_BYTES`.
    bytes.resize(boundary + MAX_SCAN_BYTES as usize, b'x');
    let c = s
        .read(&s.write("boundary.jsonl", &bytes), "t", Some("exec-1"))
        .unwrap();
    assert_eq!(c.exit_code, Some(4));
    assert_eq!(c.narration_after.as_deref(), Some("after"));

    // Without the preceding newline, the same bytes are only a line fragment.
    bytes[boundary - 1] = b' ';
    let c = s
        .read(&s.write("boundary.jsonl", &bytes), "t", Some("exec-1"))
        .unwrap();
    assert!(c.version_trusted);
    assert_eq!(c.exit_code, None);
    assert_eq!(c.turn_messages, ["after"]);
}

#[test]
fn paths_outside_the_sessions_root_are_refused() {
    let s = Sessions::new();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("rollout.jsonl");
    std::fs::write(&file, meta("0.170.0")).unwrap();
    assert!(matches!(
        s.read(&file, "t", None),
        Err(RolloutError::OutsideRoot)
    ));

    // `..` that climbs out of the root (to a real file outside) is refused;
    // `..` that returns inside is fine.
    let climb_out = s.root.join("..").join("outside-copy.jsonl");
    std::fs::write(&climb_out, meta("0.170.0")).unwrap();
    assert!(matches!(
        s.read(&climb_out, "t", None),
        Err(RolloutError::OutsideRoot)
    ));
    s.write("in.jsonl", meta("0.170.0").as_bytes());
    let climb_in = s.root.join("..").join("sessions").join("in.jsonl");
    assert!(s.read(&climb_in, "t", None).is_ok());

    // A credentials file next to the sessions dir is out of reach too.
    let auth = s.root.parent().unwrap().join("auth.json");
    std::fs::write(&auth, "{\"token\":\"SECRET\"}").unwrap();
    assert!(matches!(
        s.read(&auth, "t", None),
        Err(RolloutError::OutsideRoot)
    ));
}

#[cfg(unix)]
#[test]
fn symlink_pointing_outside_is_refused_but_inside_is_allowed() {
    use std::os::unix::fs::symlink;
    let s = Sessions::new();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("secret.jsonl");
    std::fs::write(&target, meta("0.170.0")).unwrap();
    let link = s.root.join("link.jsonl");
    symlink(&target, &link).unwrap();
    assert!(matches!(
        s.read(&link, "t", None),
        Err(RolloutError::OutsideRoot)
    ));

    let real = s.write("real.jsonl", meta("0.170.0").as_bytes());
    let inner = s.root.join("inner-link.jsonl");
    symlink(&real, &inner).unwrap();
    assert!(s.read(&inner, "t", None).is_ok());

    // A dangling link: an error, not a panic.
    let dangling = s.root.join("dangling.jsonl");
    symlink(s.root.join("gone"), &dangling).unwrap();
    assert!(s.read(&dangling, "t", None).is_err());
}

#[cfg(unix)]
#[test]
fn fifo_and_directory_are_refused_without_blocking() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let s = Sessions::new();
    let fifo = s.root.join("pipe.jsonl");
    let c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    // No writer exists: a blocking open would hang this test forever.
    assert!(matches!(
        s.read(&fifo, "t", None),
        Err(RolloutError::NotRegularFile)
    ));

    let dir = s.root.join("2026");
    std::fs::create_dir(&dir).unwrap();
    assert!(matches!(
        s.read(&dir, "t", None),
        Err(RolloutError::NotRegularFile)
    ));
}
