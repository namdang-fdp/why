//! Spawns the real `why-hook` binary with hostile input and checks the two
//! promises a hook must keep whatever happens: exit status 0, and not one byte
//! on stdout or stderr (payloads.md §4: stdout reaches the model, exit 2 blocks
//! the agent).
//!
//! Cargo builds the binary before running this test and passes its path in
//! `CARGO_BIN_EXE_why-hook`. We never write an executable ourselves, so the
//! ETXTBSY race that `write_executable_script` works around in the app's git
//! tests (exec of a file another thread still has open for writing) cannot
//! happen here.

use std::io::{ErrorKind, Write};
use std::process::{Command, Output, Stdio};
use std::thread;

use tempfile::TempDir;

/// How stdin is fed to the hook.
enum Stdin {
    /// Write these bytes from a thread, then close the pipe.
    Bytes(Vec<u8>),
    /// `/dev/null`: closed before the hook reads anything.
    Closed,
}

/// Run the hook in an empty environment whose `HOME` and `XDG_*` point into a
/// temp dir, so nothing it might compute or touch is the developer's own.
fn run_hook(args: &[&str], stdin: Stdin, extra_env: &[(&str, &str)]) -> Output {
    let home = TempDir::new().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_why-hook"));
    cmd.args(args)
        .env_clear()
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_RUNTIME_DIR", home.path().join("run"))
        .current_dir(home.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    match stdin {
        Stdin::Closed => {
            cmd.stdin(Stdio::null());
            cmd.output().unwrap()
        }
        Stdin::Bytes(bytes) => {
            cmd.stdin(Stdio::piped());
            let mut child = cmd.spawn().unwrap();
            let mut pipe = child.stdin.take().unwrap();
            // Write from a thread: a 10 MB payload is bigger than the pipe
            // buffer, so writing it before collecting the output could block
            // forever. If the hook stops reading and exits, the write fails
            // with BrokenPipe; that is allowed, not a test failure.
            let writer = thread::spawn(move || match pipe.write_all(&bytes) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::BrokenPipe => {}
                Err(e) => panic!("writing the hook's stdin: {e}"),
            });
            let out = child.wait_with_output().unwrap();
            writer.join().unwrap();
            out
        }
    }
}

#[track_caller]
fn assert_silent_success(out: &Output) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "exit status must be 0: {:?}",
        out.status
    );
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty, got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        out.stderr.is_empty(),
        "stderr must be empty, got {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

const CLAUDE_PAYLOAD: &str = r#"{"session_id":"s1","transcript_path":"/HOME/t.jsonl","cwd":"/TMP/fixture-repo","hook_event_name":"UserPromptSubmit","prompt_id":"p1","prompt":"hello"}"#;

#[test]
fn valid_payload() {
    let out = run_hook(
        &["--harness", "claude-code"],
        Stdin::Bytes(CLAUDE_PAYLOAD.into()),
        &[],
    );
    assert_silent_success(&out);
}

#[test]
fn garbage_stdin() {
    let garbage: Vec<u8> = (0..4096u32).map(|i| (i * 31 % 256) as u8).collect();
    assert_silent_success(&run_hook(
        &["--harness", "codex"],
        Stdin::Bytes(garbage),
        &[],
    ));
    assert_silent_success(&run_hook(
        &["--harness", "codex"],
        Stdin::Bytes(b"not json\n".to_vec()),
        &[],
    ));
}

#[test]
fn empty_stdin() {
    assert_silent_success(&run_hook(
        &["--harness", "codex"],
        Stdin::Bytes(Vec::new()),
        &[],
    ));
}

#[test]
fn stdin_closed_before_anything_is_written() {
    assert_silent_success(&run_hook(&["--harness", "claude-code"], Stdin::Closed, &[]));
}

#[test]
fn stdin_closed_mid_payload() {
    let half = CLAUDE_PAYLOAD.as_bytes()[..CLAUDE_PAYLOAD.len() / 2].to_vec();
    assert_silent_success(&run_hook(
        &["--harness", "claude-code"],
        Stdin::Bytes(half),
        &[],
    ));
}

#[test]
fn ten_megabytes_of_stdin() {
    // A valid-looking JSON object far past the 1 MiB limit.
    let mut big = br#"{"hook_event_name":"PostToolUse","tool_response":""#.to_vec();
    big.extend(std::iter::repeat_n(b'x', 10 * 1024 * 1024));
    big.extend_from_slice(br#""}"#);
    assert_silent_success(&run_hook(
        &["--harness", "claude-code"],
        Stdin::Bytes(big),
        &[],
    ));
}

#[test]
fn unknown_or_missing_harness_argument() {
    let payload = || Stdin::Bytes(CLAUDE_PAYLOAD.into());
    assert_silent_success(&run_hook(&["--harness", "gemini"], payload(), &[]));
    assert_silent_success(&run_hook(&["--harness"], payload(), &[]));
    assert_silent_success(&run_hook(&[], payload(), &[]));
    assert_silent_success(&run_hook(&["--bogus", "-x", "--help"], payload(), &[]));
}

#[test]
fn invalid_instance_from_argument_or_environment() {
    let payload = || Stdin::Bytes(CLAUDE_PAYLOAD.into());
    assert_silent_success(&run_hook(
        &["--harness", "codex", "--instance", "dev:nope"],
        payload(),
        &[],
    ));
    assert_silent_success(&run_hook(
        &["--harness", "codex"],
        payload(),
        &[("WHY_INSTANCE", "staging")],
    ));
    assert_silent_success(&run_hook(
        &["--harness", "codex"],
        payload(),
        &[("WHY_INSTANCE", "dev:1460")],
    ));
}

#[test]
fn no_home_at_all() {
    // `env_clear` + no HOME/XDG: path resolution fails; still silent.
    let out = Command::new(env!("CARGO_BIN_EXE_why-hook"))
        .args(["--harness", "codex"])
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_silent_success(&out);
}

/// Only meaningful for debug builds of the hook, where `WHY_HOOK_TEST_PANIC`
/// is compiled in (tests build the binary in debug unless run with `--release`).
#[cfg(debug_assertions)]
#[test]
fn a_panic_inside_the_hook_is_silent_and_exits_zero() {
    let out = run_hook(
        &["--harness", "claude-code"],
        Stdin::Bytes(CLAUDE_PAYLOAD.into()),
        &[("WHY_HOOK_TEST_PANIC", "1")],
    );
    assert_silent_success(&out);
}
