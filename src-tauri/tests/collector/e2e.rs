//! End to end: the real `why-hook` binary -> the real collector -> the real git
//! resolver -> the memory store, in temp directories.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use why_wire::Allowlist;

use super::*;

/// Path of the `why-hook` binary built by THIS `cargo test --workspace`.
///
/// `CARGO_BIN_EXE_why-hook` only exists for tests of the package that owns the
/// binary, and this test belongs to the app package. Cargo builds every
/// workspace member's binaries before running any test, into the same target
/// directory and profile as this test executable (whatever `CARGO_TARGET_DIR`,
/// `--target` or `--release` the outer command used), so the binary sits next to
/// the profile directory (`.../debug` or `.../<triple>/debug`). No nested
/// `cargo build` (that would run with the inherited environment: network, cargo
/// home, another target dir).
///
/// Where THIS test executable lives depends on the Cargo version: older Cargo
/// puts it in `<profile>/deps/`, newer Cargo (the `build-dir` layout) in
/// `<profile>/build/<pkg>/<hash>/`. So walk up from it to the first ancestor
/// that holds `why-hook` instead of assuming a fixed depth.
///
/// Missing = a loud failure with the fix, never a skip: an end-to-end test that
/// silently does not run proves nothing.
fn hook_binary() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let exe = std::env::current_exe().unwrap();
        let found = exe
            .ancestors()
            .skip(1)
            .map(|dir| dir.join("why-hook"))
            .find(|bin| bin.is_file());
        found.unwrap_or_else(|| {
            panic!(
                "no why-hook binary in any parent directory of {}. Run the tests with \
                 `cargo test --workspace` (or `pnpm test:rust`), which builds it, or \
                 `cargo build -p why-hook` first.",
                exe.display()
            )
        })
    })
}

/// Run the hook as an agent would: payload on stdin, hook config as arguments.
fn run_hook(rig: &Rig, cwd: &Path, session: &str, n: u32) {
    let payload = format!(
        r#"{{"session_id":"{session}","cwd":{},"hook_event_name":"UserPromptSubmit","prompt_id":"p{n}","prompt":"hello {n}"}}"#,
        serde_json::to_string(cwd).unwrap()
    );
    let mut child = Command::new(hook_binary())
        .args([
            "--harness",
            "claude-code",
            "--instance",
            "installed",
            "--data-dir",
        ])
        .arg(&rig.paths.data_dir)
        .arg("--socket-path")
        .arg(rig.socket())
        .env_clear()
        .current_dir(&rig.repo.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

async fn wait_for_allowlist(rig: &Rig) {
    eventually(
        Duration::from_secs(5),
        "allowlist written by the app",
        || Allowlist::read_from(&rig.paths.allowlist_path).is_ok_and(|l| l.allows(&rig.repo.path)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hook_to_store_through_the_socket() {
    let rig = Rig::new();
    let core = rig.start().await;
    wait_for_allowlist(&rig).await;
    let sub = rig.repo.path.join("src");
    std::fs::create_dir_all(&sub).unwrap();

    run_hook(&rig, &sub, "e2e-1", 1);
    eventually(Duration::from_secs(5), "event in the store", || {
        rig.has_session("e2e-1")
    })
    .await;
    assert_eq!(
        rig.sessions(),
        1,
        "attributed to the registered worktree via git"
    );
    let timeline = rig
        .events
        .timeline(&SessionKey::new(Harness::ClaudeCode, "e2e-1"))
        .unwrap()
        .unwrap();
    assert_eq!(timeline.session.worktree_id, Some(rig.main_wt.clone()));
    assert_eq!(timeline.session.cwd, sub.to_string_lossy());
    assert!(
        !rig.paths.spool_dir.exists(),
        "the socket took it: nothing spooled"
    );
    assert_eq!(core.stats().received, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_of_events_sends_few_coalesced_notifications() {
    let rig = Rig::new();
    let _core = rig.start().await;
    wait_for_allowlist(&rig).await;
    for n in 0..15 {
        run_hook(&rig, &rig.repo.path, &format!("burst-{n}"), n);
    }
    eventually(Duration::from_secs(5), "all stored", || {
        rig.sessions() == 15
    })
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await; // let the last window close
    let emitted = rig.emitted.lock().unwrap().clone();
    assert!(!emitted.is_empty());
    assert!(emitted.iter().all(|w| *w == rig.main_wt));
    assert!(
        emitted.len() < 15,
        "15 events must not mean 15 UI refreshes: {}",
        emitted.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_spooled_while_the_app_was_down_arrive_when_it_starts() {
    let rig = Rig::new();
    // The app has written the allowlist in an earlier run; it is not running now.
    let list = Allowlist::new(
        1,
        vec![rig.repo.path.to_string_lossy().into_owned()],
        vec![],
    );
    std::fs::create_dir_all(&rig.paths.data_dir).unwrap();
    std::fs::write(&rig.paths.allowlist_path, list.to_bytes()).unwrap();

    run_hook(&rig, &rig.repo.path, "down-1", 1);
    run_hook(&rig, &rig.repo.path, "down-2", 2);
    assert!(rig.paths.spool_dir.exists(), "hooks must have spooled");
    assert!(!rig.has_session("down-1"));

    let _core = rig.start().await;
    eventually(Duration::from_secs(5), "spooled events ingested", || {
        rig.has_session("down-1") && rig.has_session("down-2")
    })
    .await;
    assert_eq!(rig.sessions(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repository_the_app_does_not_know_is_not_captured() {
    let rig = Rig::new();
    let core = rig.start().await;
    wait_for_allowlist(&rig).await;
    let stranger = TestRepo::with_initial_commit();
    run_hook(&rig, &stranger.path, "stranger", 1);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!rig.has_session("stranger"));
    assert_eq!(core.stats().received, 0, "the hook must not even connect");
    assert!(!rig.paths.spool_dir.exists());
}
