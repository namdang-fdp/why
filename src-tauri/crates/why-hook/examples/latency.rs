//! Measures the hook's wall time per run, the way an agent experiences it:
//! spawn the binary, feed one payload, wait for exit. Timed from this process
//! (not a shell loop, whose own fork/exec noise would be included).
//!
//! ```text
//! cargo build --release -p why-hook
//! cargo run --release -p why-hook --example latency [runs]
//! ```
//!
//! Three scenarios, `runs` times each (default 1000):
//!   * `app up`: a listener thread accepts and drains connections (a stand-in
//!     for the collector: the hook never waits for a reply, so only that the
//!     socket accepts matters);
//!   * `app down`: no socket, every event is appended to the spool;
//!   * `outside allowlist`: the cheapest exit, for reference.
//!
//! Everything lives in a temp directory; the hook runs with a cleared
//! environment. Not part of `cargo test`.

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use why_wire::Allowlist;

fn hook_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap(); // target/<profile>/examples/latency
    exe.parent().unwrap().parent().unwrap().join("why-hook")
}

fn run_once(data: &Path, socket: &Path, cwd: &Path) -> Duration {
    let payload = format!(
        r#"{{"session_id":"s","cwd":{},"hook_event_name":"PostToolUse","tool_name":"Edit","tool_use_id":"t","tool_input":{{"file_path":"/x","old_string":"a","new_string":"b"}}}}"#,
        serde_json::to_string(cwd).unwrap()
    );
    let started = Instant::now();
    let mut child = Command::new(hook_binary())
        .args([
            "--harness",
            "claude-code",
            "--instance",
            "installed",
            "--data-dir",
        ])
        .arg(data)
        .arg("--socket-path")
        .arg(socket)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait().unwrap();
    started.elapsed()
}

fn report(name: &str, mut samples: Vec<Duration>) {
    samples.sort();
    let pct = |p: f64| samples[((samples.len() as f64 * p) as usize).min(samples.len() - 1)];
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "| {name:<18} | {:>6.2} | {:>6.2} | {:>6.2} | {:>6.2} |",
        ms(pct(0.50)),
        ms(pct(0.90)),
        ms(pct(0.99)),
        ms(*samples.last().unwrap())
    );
}

fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1000);
    let tmp = tempfile::Builder::new()
        .prefix("wl")
        .tempdir_in("/tmp")
        .unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (data, socket, repo, other) = (
        root.join("data"),
        root.join("s.sock"),
        root.join("repo"),
        root.join("other"),
    );
    for d in [&data, &repo, &other] {
        std::fs::create_dir_all(d).unwrap();
    }
    let allow = Allowlist::new(1, vec![repo.to_string_lossy().into_owned()], vec![]);
    std::fs::write(data.join("hook-allowlist.json"), allow.to_bytes()).unwrap();

    println!("{runs} runs each; wall time per hook run, milliseconds\n");
    println!("| scenario           |    p50 |    p90 |    p99 |    max |");
    println!("|--------------------|--------|--------|--------|--------|");

    // App down first (no socket): every run spools.
    report(
        "app down (spool)",
        (0..runs).map(|_| run_once(&data, &socket, &repo)).collect(),
    );
    std::fs::remove_dir_all(data.join("spool")).unwrap();

    // App up: a thread that accepts and reads everything.
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut conn = conn;
                let mut sink = Vec::new();
                let _ = conn.read_to_end(&mut sink);
            });
        }
    });
    report(
        "app up (socket)",
        (0..runs).map(|_| run_once(&data, &socket, &repo)).collect(),
    );
    assert!(!data.join("spool").exists(), "app-up runs must not spool");

    report(
        "outside allowlist",
        (0..runs)
            .map(|_| run_once(&data, &socket, &other))
            .collect(),
    );
}
