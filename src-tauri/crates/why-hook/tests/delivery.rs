//! The real `why-hook` binary against hostile surroundings: no app, a dead
//! socket, a stuck reader, a full disk quota, a forged cwd. Every test checks
//! the two promises (exit 0, no output) and then where the line did or did not
//! end up: the socket, the spool, or nowhere.
//!
//! All state lives in temp dirs; the hook is told its paths with
//! `--data-dir` / `--socket-path`, and its environment is cleared, so nothing
//! of the developer's machine can leak in or be touched.

#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;
use why_wire::{Allowlist, Envelope, MAX_LINE_BYTES, Reduction, decode_line, reduction_of};

/// One sandbox: a data dir (allowlist + spool), a SHORT socket path (`sun_path`
/// is ~108 bytes, and a long `$TMPDIR` would break `bind`), and a repo dir.
struct Sandbox {
    _tmp: TempDir,
    data: PathBuf,
    socket: PathBuf,
    repo: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::Builder::new()
            .prefix("wh")
            .tempdir_in("/tmp")
            .unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let sb = Sandbox {
            data: root.join("data"),
            socket: root.join("s.sock"),
            repo: root.join("repo"),
            _tmp: tmp,
        };
        assert!(
            sb.socket.as_os_str().len() < 100,
            "sandbox path too long for a socket"
        );
        fs::create_dir_all(&sb.data).unwrap();
        fs::create_dir_all(sb.repo.join("sub/deep")).unwrap();
        sb.allow(&[&sb.repo.clone()]);
        sb
    }

    fn allow(&self, roots: &[&Path]) {
        let roots = roots
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let list = Allowlist::new(1, roots, vec![]);
        fs::write(self.data.join("hook-allowlist.json"), list.to_bytes()).unwrap();
    }

    fn spool_dir(&self) -> PathBuf {
        self.data.join("spool")
    }

    /// Every line in every spool file, newest file last.
    fn spooled_lines(&self) -> Vec<Vec<u8>> {
        let mut files: Vec<_> = fs::read_dir(self.spool_dir())
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        files
            .iter()
            .flat_map(|f| {
                let bytes = fs::read(f).unwrap();
                bytes
                    .split_inclusive(|&b| b == b'\n')
                    // The spool frames each record with a leading blank line.
                    .filter(|l| *l != b"\n")
                    .map(<[u8]>::to_vec)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn payload(&self, cwd: &Path, extra: &str) -> String {
        format!(
            r#"{{"session_id":"s1","cwd":{},"hook_event_name":"PostToolUse","tool_name":"Write","tool_use_id":"t1"{extra}}}"#,
            serde_json::to_string(&cwd.to_string_lossy()).unwrap()
        )
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_why-hook"));
        cmd.args([
            "--harness",
            "claude-code",
            "--instance",
            "installed",
            "--data-dir",
        ])
        .arg(&self.data)
        .arg("--socket-path")
        .arg(&self.socket)
        .env_clear()
        .current_dir(&self.repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        cmd
    }

    /// Run the hook with `stdin`; asserts the contract and returns the wall time.
    fn run(&self, stdin: &str) -> Duration {
        self.run_with(stdin, &[])
    }

    /// Like [`Sandbox::run`] with extra environment variables. A watchdog kills
    /// a hook that has not exited after 5 s: a hook that blocks (a FIFO opened
    /// without `O_NONBLOCK`, say) must FAIL the test, not hang it.
    fn run_with(&self, stdin: &str, env: &[(&str, &Path)]) -> Duration {
        let started = Instant::now();
        let mut cmd = self.command();
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.stdin(Stdio::piped()).spawn().unwrap();
        let mut pipe = child.stdin.take().unwrap();
        let bytes = stdin.as_bytes().to_vec();
        let writer = std::thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        });
        let pid = child.id();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watchdog = {
            let done = std::sync::Arc::clone(&done);
            std::thread::spawn(move || {
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(5) {
                    if done.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            })
        };
        let out = child.wait_with_output().unwrap();
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        watchdog.join().unwrap();
        writer.join().unwrap();
        let elapsed = started.elapsed();
        assert_eq!(
            out.status.code(),
            Some(0),
            "exit status (killed by the watchdog = hung)"
        );
        assert!(
            out.stdout.is_empty(),
            "stdout: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(
            out.stderr.is_empty(),
            "stderr: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        elapsed
    }
}

fn read_line(listener: &UnixListener) -> Vec<u8> {
    let (mut conn, _) = listener.accept().unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = Vec::new();
    conn.read_to_end(&mut buf).unwrap();
    buf
}

fn envelope(line: &[u8]) -> Envelope {
    assert_eq!(line.last(), Some(&b'\n'), "line must end with a newline");
    decode_line(line).unwrap()
}

#[test]
fn app_up_delivers_over_the_socket_and_spools_nothing() {
    let sb = Sandbox::new();
    let listener = UnixListener::bind(&sb.socket).unwrap();
    sb.run(&sb.payload(&sb.repo.join("sub"), ""));
    let env = envelope(&read_line(&listener));
    assert_eq!(env.event, "PostToolUse");
    assert_eq!(env.cwd, sb.repo.join("sub").to_string_lossy());
    assert!(
        !sb.spool_dir().exists(),
        "nothing is spooled when the app took it"
    );
}

#[test]
fn app_down_spools_the_line_with_private_modes() {
    let sb = Sandbox::new();
    sb.run(&sb.payload(&sb.repo, ""));
    let lines = sb.spooled_lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(envelope(&lines[0]).payload["session_id"], "s1");
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&sb.spool_dir()), 0o700);
    let file = fs::read_dir(sb.spool_dir())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(mode(&file), 0o600);
}

#[test]
fn a_socket_file_with_nobody_listening_spools() {
    let sb = Sandbox::new();
    drop(UnixListener::bind(&sb.socket).unwrap()); // leaves the file behind
    assert!(sb.socket.exists());
    // Sibling tests spawn processes. A fork duplicates every open descriptor,
    // including the listener we just dropped, until the child execs, so the
    // "dead" socket can answer for a few milliseconds. Wait until it is dead.
    let start = Instant::now();
    while std::os::unix::net::UnixStream::connect(&sb.socket).is_ok() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "listener never died"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    sb.run(&sb.payload(&sb.repo, ""));
    assert_eq!(sb.spooled_lines().len(), 1);
}

/// A listener that never reads: a big line cannot fit the socket buffer, so the
/// write would block forever without the budget.
#[test]
fn a_reader_that_never_reads_cannot_hold_the_hook() {
    let sb = Sandbox::new();
    let _listener = UnixListener::bind(&sb.socket).unwrap(); // never accepts
    let big = format!(r#","blob":"{}""#, "x".repeat(900_000));
    let elapsed = sb.run(&sb.payload(&sb.repo, &big));
    // 100 ms budget + process start + the spool write; generous CI slack.
    assert!(elapsed < Duration::from_millis(1000), "took {elapsed:?}");
    let lines = sb.spooled_lines();
    assert_eq!(lines.len(), 1, "the event must be spooled instead");
    assert!(lines[0].len() > 900_000);
}

/// A unix listener with a tiny backlog that never accepts, plus clients that
/// fill it. std's `UnixListener::bind` uses a large backlog (SOMAXCONN), so the
/// raw calls are needed to make `connect` itself unable to complete.
fn full_backlog_listener(path: &Path) -> (std::os::fd::OwnedFd, Vec<std::os::fd::OwnedFd>) {
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    unsafe {
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (d, s) in addr.sun_path.iter_mut().zip(path.as_os_str().as_bytes()) {
            *d = *s as libc::c_char;
        }
        let len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
        let ptr = (&addr as *const libc::sockaddr_un).cast::<libc::sockaddr>();
        let listener = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
        assert!(listener >= 0);
        assert_eq!(libc::bind(listener, ptr, len), 0);
        assert_eq!(libc::listen(listener, 0), 0);
        let mut clients = Vec::new();
        loop {
            let c = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
            libc::fcntl(c, libc::F_SETFL, libc::O_NONBLOCK);
            if libc::connect(c, ptr, len) != 0 {
                libc::close(c); // EAGAIN: the backlog is full now
                break;
            }
            clients.push(OwnedFd::from_raw_fd(c));
            assert!(clients.len() < 1000, "backlog never filled");
        }
        (OwnedFd::from_raw_fd(listener), clients)
    }
}

/// The other way to block: a listener whose backlog is full, so `connect`
/// itself cannot complete.
#[test]
fn a_full_listen_backlog_cannot_hold_the_hook() {
    let sb = Sandbox::new();
    let (_listener, _clients) = full_backlog_listener(&sb.socket);
    let elapsed = sb.run(&sb.payload(&sb.repo, ""));
    assert!(elapsed < Duration::from_millis(1000), "took {elapsed:?}");
    // Spooled, so `connect` really was refused service (not vacuously fast).
    assert_eq!(sb.spooled_lines().len(), 1);
}

#[test]
fn an_unwritable_spool_dir_still_exits_silently() {
    let sb = Sandbox::new();
    fs::set_permissions(&sb.data, fs::Permissions::from_mode(0o500)).unwrap();
    sb.run(&sb.payload(&sb.repo, ""));
    fs::set_permissions(&sb.data, fs::Permissions::from_mode(0o700)).unwrap();
    // (As root the directory mode is ignored; the exit-0/silence asserts above
    // are the point either way.)
}

#[test]
fn a_full_spool_drops_new_events() {
    let sb = Sandbox::new();
    fs::create_dir_all(sb.spool_dir()).unwrap();
    // A sparse 51 MB file: counts as 51 MB for the cap, costs no disk.
    let filler = sb.spool_dir().join("old.jsonl");
    fs::File::create(&filler)
        .unwrap()
        .set_len(51 * 1024 * 1024)
        .unwrap();
    sb.run(&sb.payload(&sb.repo, ""));
    let files: Vec<_> = fs::read_dir(sb.spool_dir()).unwrap().flatten().collect();
    assert_eq!(files.len(), 1, "no new spool file past the cap");
    assert_eq!(fs::metadata(&filler).unwrap().len(), 51 * 1024 * 1024);
}

#[test]
fn a_cwd_outside_the_allowlist_leaves_no_trace() {
    let sb = Sandbox::new();
    let listener = UnixListener::bind(&sb.socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let outside = sb._tmp.path().join("elsewhere");
    fs::create_dir(&outside).unwrap();
    // `repo-evil` shares a string prefix with the allowed `repo`.
    let evil = sb._tmp.path().join("repo-evil");
    fs::create_dir(&evil).unwrap();
    for cwd in [
        outside,
        evil,
        PathBuf::from("/"),
        PathBuf::from("/nonexistent/dir"),
    ] {
        sb.run(&sb.payload(&cwd, ""));
    }
    assert!(listener.accept().is_err(), "nothing may be sent");
    assert!(!sb.spool_dir().exists(), "nothing may be spooled");
}

#[test]
fn a_missing_or_corrupt_allowlist_captures_nothing() {
    let sb = Sandbox::new();
    let list = sb.data.join("hook-allowlist.json");
    for content in [
        None,
        Some(b"".to_vec()),
        Some(b"{not json".to_vec()),
        Some(br#"{"v":99,"repo_roots":["/"]}"#.to_vec()),
        Some(br#"{"v":1,"repo_roots":"nope"}"#.to_vec()),
    ] {
        match &content {
            None => {
                let _ = fs::remove_file(&list);
            }
            Some(bytes) => fs::write(&list, bytes).unwrap(),
        }
        sb.run(&sb.payload(&sb.repo, ""));
        assert!(
            !sb.spool_dir().exists(),
            "spooled with allowlist {content:?}"
        );
    }
}

#[test]
fn symlinks_are_judged_by_where_they_lead() {
    let sb = Sandbox::new();
    // A link elsewhere that leads INTO the allowed repo: allowed.
    let into = sb._tmp.path().join("shortcut");
    symlink(sb.repo.join("sub"), &into).unwrap();
    sb.run(&sb.payload(&into, ""));
    assert_eq!(
        sb.spooled_lines().len(),
        1,
        "symlink into a root is allowed"
    );

    // A link inside the repo that leads OUT of it: refused.
    let outside = sb._tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let out = sb.repo.join("escape");
    symlink(&outside, &out).unwrap();
    sb.run(&sb.payload(&out, ""));
    // And a `..` walk out of the repo.
    sb.run(&sb.payload(&sb.repo.join("sub/../../outside"), ""));
    assert_eq!(sb.spooled_lines().len(), 1, "escapes must not add lines");
}

#[test]
fn an_oversized_payload_becomes_a_truncated_envelope() {
    let sb = Sandbox::new();
    let listener = UnixListener::bind(&sb.socket).unwrap();
    let two_mb = format!(
        r#","tool_input":{{"file_path":"/x","content":"{}"}}"#,
        "y".repeat(2 * 1024 * 1024)
    );
    sb.run(&sb.payload(&sb.repo, &two_mb));
    let line = read_line(&listener);
    assert!(line.len() < MAX_LINE_BYTES);
    assert!(
        line.len() < 4096,
        "the marker must be small, got {}",
        line.len()
    );
    let env = envelope(&line);
    assert_eq!(reduction_of(&env.payload), Some(Reduction::Truncated));
    assert_eq!(env.payload["session_id"], "s1");
    assert_eq!(env.payload["tool_name"], "Write");
    assert_eq!(env.payload["tool_use_id"], "t1");
    assert!(env.payload["original_bytes"].as_u64().unwrap() > 2 * 1024 * 1024);
    assert!(env.payload.get("tool_input").is_none());
}

/// Bigger than the hook will even buffer (8 MiB): still a timeline event, from
/// the ids at the start of the payload.
#[test]
fn a_twenty_megabyte_payload_still_yields_a_truncated_envelope() {
    let sb = Sandbox::new();
    let listener = UnixListener::bind(&sb.socket).unwrap();
    let huge = format!(
        r#","tool_input":{{"file_path":"/x","content":"{}"}},"late_id":"y""#,
        "y".repeat(20 * 1024 * 1024)
    );
    sb.run(&sb.payload(&sb.repo, &huge));
    let env = envelope(&read_line(&listener));
    assert_eq!(reduction_of(&env.payload), Some(Reduction::Truncated));
    assert_eq!(env.payload["session_id"], "s1");
    assert_eq!(env.event, "PostToolUse");
    assert_eq!(env.cwd, sb.repo.to_string_lossy());
    assert!(env.payload["original_bytes"].as_u64().unwrap() >= 20 * 1024 * 1024);
}

fn mkfifo(path: &Path) {
    let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

/// A FIFO where the allowlist should be: opening it for reading would block
/// until someone writes, stalling the agent. The hook must treat it as "no
/// allowlist" and return at once.
#[test]
fn a_fifo_at_the_allowlist_path_cannot_stall_the_hook() {
    let sb = Sandbox::new();
    let list = sb.data.join("hook-allowlist.json");
    fs::remove_file(&list).unwrap();
    mkfifo(&list);
    let listener = UnixListener::bind(&sb.socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let elapsed = sb.run(&sb.payload(&sb.repo, ""));
    assert!(elapsed < Duration::from_millis(1000), "took {elapsed:?}");
    assert!(
        listener.accept().is_err(),
        "no allowlist means nothing is sent"
    );
    assert!(!sb.spool_dir().exists());
}

/// A FIFO where today's spool file should be (app down): the append must be
/// refused, not block in `open`.
#[test]
fn a_fifo_at_the_spool_path_cannot_stall_the_hook() {
    let sb = Sandbox::new();
    fs::create_dir_all(sb.spool_dir()).unwrap();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    // Today (and tomorrow, in case the run straddles midnight UTC).
    for day in [now_ms, now_ms + 86_400_000] {
        mkfifo(
            &sb.spool_dir()
                .join(format!("{}.jsonl", why_wire::spool::date_stamp(day))),
        );
    }
    let elapsed = sb.run(&sb.payload(&sb.repo, ""));
    assert!(elapsed < Duration::from_millis(1000), "took {elapsed:?}");
}

/// A submodule or nested clone inside a registered repository is a different
/// repository: not captured, even though its path starts with the root.
#[test]
fn nested_unregistered_repositories_are_not_captured() {
    let sb = Sandbox::new();
    let listener = UnixListener::bind(&sb.socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let submodule = sb.repo.join("vendor/sub");
    fs::create_dir_all(submodule.join("src")).unwrap();
    fs::write(submodule.join(".git"), "gitdir: ../../.git/modules/sub\n").unwrap();
    let clone = sb.repo.join("vendor/clone");
    fs::create_dir_all(clone.join(".git")).unwrap();
    for cwd in [&submodule, &submodule.join("src"), &clone] {
        sb.run(&sb.payload(cwd, ""));
    }
    assert!(listener.accept().is_err(), "nothing may be sent");
    assert!(!sb.spool_dir().exists(), "nothing may be spooled");

    // The same nested worktree once the app LISTS it: captured. App down, so
    // the line is spooled.
    sb.allow(&[&sb.repo.clone(), &submodule]);
    drop(listener);
    fs::remove_file(&sb.socket).unwrap();
    sb.run(&sb.payload(&submodule.join("src"), ""));
    assert_eq!(sb.spooled_lines().len(), 1);
    // ...and its unlisted sibling clone still is not.
    sb.run(&sb.payload(&clone, ""));
    assert_eq!(sb.spooled_lines().len(), 1);
}

/// `CODEX_HOME` / `CLAUDE_CONFIG_DIR` move the credential files to places no
/// built-in literal matches; the hook reads them from ITS environment.
#[test]
fn credentials_under_configured_homes_are_redacted() {
    let sb = Sandbox::new();
    let codex = sb._tmp.path().join("my codex \"home\"");
    let claude = sb._tmp.path().join("cfg-claude");
    fs::create_dir_all(&codex).unwrap();
    fs::create_dir_all(&claude).unwrap();
    let env = [
        ("CODEX_HOME", codex.as_path()),
        ("CLAUDE_CONFIG_DIR", claude.as_path()),
    ];
    for target in [codex.join("auth.json"), claude.join(".credentials.json")] {
        let extra = format!(
            r#","tool_input":{{"file_path":{}}},"tool_response":{{"file":{{"content":"SECRET-TOKEN"}}}}"#,
            serde_json::to_string(&target).unwrap()
        );
        sb.run_with(&sb.payload(&sb.repo, &extra), &env);
    }
    let lines = sb.spooled_lines();
    assert_eq!(lines.len(), 2);
    for line in &lines {
        assert!(!String::from_utf8_lossy(line).contains("SECRET-TOKEN"));
        assert_eq!(
            reduction_of(&envelope(line).payload),
            Some(Reduction::Redacted)
        );
    }
    // Control: without the variables the same odd path is NOT recognised (that is
    // the gap this closes), so the content would pass through.
    let extra = format!(
        r#","tool_input":{{"file_path":{}}},"tool_response":{{"file":{{"content":"SECRET-TOKEN"}}}}"#,
        serde_json::to_string(&codex.join("auth.json")).unwrap()
    );
    sb.run(&sb.payload(&sb.repo, &extra));
    let last = sb.spooled_lines().pop().unwrap();
    assert!(String::from_utf8_lossy(&last).contains("SECRET-TOKEN"));
}

#[test]
fn a_payload_touching_agent_credentials_is_redacted_everywhere() {
    let sb = Sandbox::new();
    let listener = UnixListener::bind(&sb.socket).unwrap();
    let read = r#","tool_input":{"file_path":"/home/u/.claude/.credentials.json"},"tool_response":{"file":{"content":"SECRET-TOKEN"}}"#;
    sb.run(&sb.payload(&sb.repo, read));
    let line = read_line(&listener);
    assert!(!String::from_utf8_lossy(&line).contains("SECRET-TOKEN"));
    assert_eq!(
        reduction_of(&envelope(&line).payload),
        Some(Reduction::Redacted)
    );
    // Same for the spool (app down).
    drop(listener);
    fs::remove_file(&sb.socket).unwrap();
    sb.run(&sb.payload(&sb.repo, read));
    let spooled = sb.spooled_lines();
    assert!(!String::from_utf8_lossy(&spooled[0]).contains("SECRET-TOKEN"));
}

#[test]
fn fifty_concurrent_hooks_spool_fifty_intact_lines() {
    let sb = Sandbox::new();
    let children: Vec<_> = (0..50)
        .map(|i| {
            let mut child = sb.command().stdin(Stdio::piped()).spawn().unwrap();
            let body = sb.payload(
                &sb.repo,
                &format!(r#","n":{i},"pad":"{}""#, "z".repeat(20_000)),
            );
            child
                .stdin
                .take()
                .unwrap()
                .write_all(body.as_bytes())
                .unwrap();
            child
        })
        .collect();
    for c in children {
        let out = c.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stdout.is_empty() && out.stderr.is_empty());
    }
    let lines = sb.spooled_lines();
    assert_eq!(lines.len(), 50, "a line was lost");
    let mut seen: Vec<u64> = lines
        .iter()
        .map(|l| envelope(l).payload["n"].as_u64().unwrap())
        .collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        (0..50).collect::<Vec<u64>>(),
        "interleaved or duplicated"
    );
}
