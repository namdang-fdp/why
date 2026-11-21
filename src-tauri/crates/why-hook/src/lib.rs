//! `why-hook`: the program Claude Code and Codex run for every hook event.
//!
//! # The hard rules (see `docs/agents/payloads.md` §4)
//!
//! * **Never write to stdout.** On `SessionStart` and `UserPromptSubmit` both
//!   harnesses inject a hook's stdout into the model's context.
//! * **Never write to stderr, always exit 0.** Exit 2 (with stderr text for
//!   Codex) blocks tools, rejects prompts or traps the agent in a `Stop` loop.
//!   A Rust panic would print to stderr and exit 101; `main` silences and
//!   catches it.
//! * **Be fast.** The harness waits for the hook, so its run time is added to
//!   every tool call. No Tauri, no async runtime; one bounded stdin read.
//!
//! The flow, in this order (each step can end the run silently):
//!
//! 1. read stdin (bounded) and build an [`Envelope`];
//! 2. **allowlist**: `cwd` (canonicalized) must be inside a root the app listed
//!    in `hook-allowlist.json`; otherwise nothing is sent and nothing is spooled
//!    (a missing or corrupt file counts as "not allowed");
//! 3. reduce the payload if it touches an agent credential file or does not fit
//!    one wire line (`why_wire::reduced`);
//! 4. **send** the line to the collector's socket within a hard budget
//!    ([`transport`]);
//! 5. on any failure, **spool** it to disk for the app to ingest later
//!    (`why_wire::spool`).
//!
//! Everything is in this library (not `main.rs`) so it can be unit tested.

#[cfg(unix)]
pub mod transport;

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use why_wire::{
    Allowlist, Envelope, Harness, INSTANCE_ENV, Instance, InstancePaths, PROTOCOL_VERSION, PathEnv,
    Reduction, WireError, credential_paths, encode_line, mentions_credentials, reduce_payload,
    reduce_prefix, reduction_of,
};

/// Total time for connect + write to the collector's socket. The agent waits
/// for the hook, so this is added to its tool call in the worst case;
/// payloads.md §7 asks for <= 200 ms.
pub const SEND_BUDGET: Duration = Duration::from_millis(100);

/// Most stdin we keep: 8 MiB. Bigger than one wire line (`MAX_LINE_BYTES`, 1 MiB)
/// on purpose: a payload over the line limit is still parsed so that the hook
/// can send a small "truncated" envelope with its ids (the timeline then shows
/// the event), instead of silently losing it. Past 8 MiB only the first 8 MiB
/// are kept and scanned for ids (`reduce_prefix`), never parsed whole.
pub const MAX_STDIN_BYTES: usize = 8 * 1024 * 1024;

/// How much stdin we read and throw away after [`MAX_STDIN_BYTES`]. The
/// harness writes the whole payload into our stdin pipe; if we exited without
/// reading it, its write would fail with `EPIPE`, which it may log as a hook
/// error. Draining avoids that. Bounded, so a peer that never closes stdin
/// cannot keep us busy forever (the harness's own timeout also applies).
pub const DRAIN_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookArgs {
    /// `--harness <claude-code|codex>`; anything else (or nothing) is `Unknown`.
    pub harness: Harness,
    /// `--instance <installed|dev:PORT>`; an empty string is an invalid value.
    pub instance: Option<String>,
    data_dir: Option<PathBuf>,
    socket_path: Option<PathBuf>,
}

/// Parse `why-hook --harness <name> [--instance <installed|dev:PORT>]`.
/// App-generated configs also supply `--data-dir` and `--socket-path`.
/// `args` excludes the program name. Unknown arguments are ignored.
pub fn parse_args<I>(args: I) -> HookArgs
where
    I: IntoIterator<Item = OsString>,
{
    let mut out = HookArgs {
        harness: Harness::Unknown,
        instance: None,
        data_dir: None,
        socket_path: None,
    };
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        // Missing values stay invalid and leave the next flag available to parse.
        let mut value = || args.next_if(|v| !v.as_encoded_bytes().starts_with(b"--"));
        match arg.to_str() {
            Some("--harness") => {
                if let Some(v) = value() {
                    // `FromStr for Harness` is infallible (unknown -> Unknown).
                    out.harness = v.to_string_lossy().parse().unwrap_or(Harness::Unknown);
                }
            }
            Some("--instance") => {
                out.instance = Some(value().unwrap_or_default().to_string_lossy().into_owned());
            }
            Some("--data-dir") => out.data_dir = Some(value().unwrap_or_default().into()),
            Some("--socket-path") => out.socket_path = Some(value().unwrap_or_default().into()),
            _ => {}
        }
    }
    out
}

impl HookArgs {
    fn resolve_paths(self, instance: Instance, env: &PathEnv) -> Option<InstancePaths> {
        match (self.data_dir, self.socket_path) {
            (None, None) => InstancePaths::resolve(instance, env).ok(),
            (Some(data_dir), Some(socket_path)) => {
                InstancePaths::from_hook_paths(instance, data_dir, socket_path)
            }
            _ => None,
        }
    }
}

/// Which instance to talk to: `--instance`, else `$WHY_INSTANCE`, else the
/// installed app. An empty `$WHY_INSTANCE` counts as unset. `None` when the
/// value given is invalid: then the event is dropped rather than sent to an
/// instance nobody asked for (a dev event in the installed app's history would
/// be worse than a lost one).
pub fn choose_instance(arg: Option<&str>, env_value: Option<&str>) -> Option<Instance> {
    match arg.or(env_value.filter(|v| !v.is_empty())) {
        None => Some(Instance::Installed),
        Some(text) => text.parse().ok(),
    }
}

/// What came in on stdin.
#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    /// The whole payload (at most [`MAX_STDIN_BYTES`]).
    Whole(Vec<u8>),
    /// More than [`MAX_STDIN_BYTES`]: the first [`MAX_STDIN_BYTES`] bytes, and
    /// the size of the whole (a lower bound once [`DRAIN_LIMIT_BYTES`] is hit).
    Oversized { head: Vec<u8>, total: u64 },
}

/// Read stdin, keeping at most [`MAX_STDIN_BYTES`] of it and draining the rest
/// (see [`DRAIN_LIMIT_BYTES`]). `None` when reading fails. A payload cut short
/// by the harness closing stdin early is returned as-is; it simply fails to
/// parse later.
pub fn read_input(mut input: impl Read) -> Option<Input> {
    let mut buf = Vec::new();
    // `take(N + 1)`: reading one byte past the limit is how we learn the
    // payload is too big without buffering all of it.
    let read = input
        .by_ref()
        .take(MAX_STDIN_BYTES as u64 + 1)
        .read_to_end(&mut buf);
    if buf.len() > MAX_STDIN_BYTES {
        let drained = io::copy(&mut input.take(DRAIN_LIMIT_BYTES), &mut io::sink()).unwrap_or(0);
        let total = buf.len() as u64 + drained;
        buf.truncate(MAX_STDIN_BYTES);
        return Some(Input::Oversized { head: buf, total });
    }
    read.ok().map(|_| Input::Whole(buf))
}

/// The envelope for an [`Input::Oversized`] payload: ids scanned from its head,
/// no content. Always succeeds (a head with no readable ids still yields the
/// marker, and the event still shows up).
pub fn build_oversized_envelope(
    head: &[u8],
    total: u64,
    harness: Harness,
    received_at_ms: u64,
    pid: u32,
    fallback_cwd: &str,
) -> Envelope {
    let payload = reduce_prefix(head, Reduction::Truncated, total);
    let text = |key: &str| payload.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    Envelope {
        v: PROTOCOL_VERSION,
        harness,
        harness_version: None,
        event: text("hook_event_name").unwrap_or_default(),
        received_at_ms,
        cwd: text("cwd").unwrap_or_else(|| fallback_cwd.to_owned()),
        pid,
        payload,
    }
}

/// Wrap a raw hook payload in an [`Envelope`]. `None` when it is not a JSON
/// object (garbage, empty, truncated): such input is dropped silently.
///
/// `fallback_cwd` is used when the payload has no `cwd` (both harnesses send
/// one today; the hook process's own cwd is the session cwd anyway).
pub fn build_envelope(
    input: &[u8],
    harness: Harness,
    received_at_ms: u64,
    pid: u32,
    fallback_cwd: &str,
) -> Option<Envelope> {
    let payload: serde_json::Value = serde_json::from_slice(input).ok()?;
    let object = payload.as_object()?;
    let text = |key: &str| object.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    Some(Envelope {
        v: PROTOCOL_VERSION,
        harness,
        // No payload carries the harness version today (payloads.md §7).
        harness_version: None,
        event: text("hook_event_name").unwrap_or_default(),
        received_at_ms,
        cwd: text("cwd").unwrap_or_else(|| fallback_cwd.to_owned()),
        pid,
        payload,
    })
}

/// Milliseconds since the Unix epoch; 0 if the clock is before 1970.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The process that spawned us (see [`Envelope::pid`]).
fn parent_pid() -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(debug_assertions)]
fn panic_if_requested(requested: bool) {
    if requested {
        panic!("test panic requested by WHY_HOOK_TEST_PANIC");
    }
}

/// The whole hook, minus the panic guard (`main` adds that). Returns nothing:
/// there is no outcome a caller could act on without breaking the rules above.
pub fn run() {
    // Dev builds only: lets the integration test prove that a panic anywhere in
    // here still ends in a silent exit 0. Compiled out of release builds.
    #[cfg(debug_assertions)]
    panic_if_requested(std::env::var_os("WHY_HOOK_TEST_PANIC").is_some());

    let args = parse_args(std::env::args_os().skip(1));
    let env_instance = match std::env::var(INSTANCE_ENV) {
        Ok(v) => Some(v),
        Err(std::env::VarError::NotPresent) => None,
        // Not UTF-8: certainly not a valid instance; make it fail to parse.
        Err(std::env::VarError::NotUnicode(_)) => Some("\u{fffd}".to_owned()),
    };
    let Some(instance) = choose_instance(args.instance.as_deref(), env_instance.as_deref()) else {
        // Still drain stdin so the harness's write does not fail.
        let _ = io::copy(
            &mut io::stdin().lock().take(DRAIN_LIMIT_BYTES),
            &mut io::sink(),
        );
        return;
    };
    let Some(input) = read_input(io::stdin().lock()) else {
        return;
    };
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (envelope, raw) = match input {
        Input::Whole(bytes) => {
            let Some(envelope) = build_envelope(&bytes, args.harness, now_ms(), parent_pid(), &cwd)
            else {
                return;
            };
            (envelope, bytes)
        }
        Input::Oversized { head, total } => {
            let envelope =
                build_oversized_envelope(&head, total, args.harness, now_ms(), parent_pid(), &cwd);
            (envelope, head)
        }
    };

    let Some(paths) = args.resolve_paths(instance, &PathEnv::from_process()) else {
        return;
    };
    // Step 2: the allowlist decides before anything touches the socket or disk.
    if !is_allowed(&paths.allowlist_path, &envelope.cwd) {
        return;
    }
    // Where THIS user's agents keep their credentials (the variables move them).
    let credentials = credential_paths(
        std::env::var_os("CODEX_HOME").as_deref(),
        std::env::var_os("CLAUDE_CONFIG_DIR").as_deref(),
        std::env::var_os("HOME").as_deref(),
    );
    let Some(line) = prepare_line(envelope, &raw, &credentials) else {
        return;
    };
    deliver(&paths, &line, now_ms(), SEND_BUDGET);
}

/// Is `cwd` inside a root of the allowlist at `allowlist_path`?
///
/// Every failure answers "no" (safe default: capture nothing): no file, an
/// unreadable or corrupt one, a version we do not know, a `cwd` that no longer
/// exists or cannot be canonicalized. `canonicalize` resolves symlinks and
/// `..`, so a symlink inside a repo that points elsewhere does not smuggle
/// events in, and one pointing into a registered repo is recognised. A cwd
/// inside a repository NESTED in a registered one (submodule, vendored clone)
/// is refused unless that inner checkout is itself listed
/// (`Allowlist::allows_checkout`).
pub fn is_allowed(allowlist_path: &Path, cwd: &str) -> bool {
    if cwd.is_empty() {
        return false;
    }
    let Ok(allowlist) = Allowlist::read_from(allowlist_path) else {
        return false;
    };
    match std::fs::canonicalize(cwd) {
        Ok(canonical) => allowlist.allows_checkout(&canonical),
        Err(_) => false,
    }
}

/// The wire line for `envelope`, reducing its payload when it must not or
/// cannot go as-is. `raw` is the payload as read from stdin (for its size and
/// the credential check); `credentials` are this user's actual credential file
/// paths (`why_wire::credential_paths`). `None` only if even the reduced
/// envelope would not encode, which cannot happen for a well-formed one.
pub fn prepare_line(mut envelope: Envelope, raw: &[u8], credentials: &[String]) -> Option<Vec<u8>> {
    // Credentials first: even if it would fit, never forward it. (An oversized
    // payload is already reduced to ids; `raw` is then only its head.)
    if reduction_of(&envelope.payload).is_none() && mentions_credentials(raw, credentials) {
        envelope.payload = reduce_payload(&envelope.payload, Reduction::Redacted, raw.len());
    }
    // An oversized payload is reduced before this point, so the check above
    // skipped it, but a truncated payload now keeps the edited file's path
    // (`tool_input.file_path`): that path gets the same credential check, with
    // this user's real credential locations. A match is redacted, which keeps
    // no path at all.
    if reduction_of(&envelope.payload) == Some(Reduction::Truncated)
        && let Some(path) = envelope.payload.pointer("/tool_input/file_path")
        && mentions_credentials(&serde_json::to_vec(path).unwrap_or_default(), credentials)
    {
        let original = envelope.payload["original_bytes"].as_u64().unwrap_or(0);
        let original = usize::try_from(original).unwrap_or(usize::MAX);
        envelope.payload = reduce_payload(&envelope.payload, Reduction::Redacted, original);
    }
    match encode_line(&envelope) {
        Ok(line) => Some(line),
        Err(WireError::TooLarge { .. }) => {
            envelope.payload = reduce_payload(&envelope.payload, Reduction::Truncated, raw.len());
            encode_line(&envelope).ok()
        }
        Err(_) => None,
    }
}

/// Send `line` to the collector; spool it when that does not work. Never
/// reports a failure: if the spool is also unusable (unwritable, full) the
/// event is lost, which is the correct outcome for a passive observer.
pub fn deliver(paths: &InstancePaths, line: &[u8], now_ms: u64, budget: Duration) {
    #[cfg(unix)]
    {
        use why_wire::spool;
        if transport::send_line(&paths.socket_path, line, budget).is_ok() {
            return;
        }
        let _: Result<spool::Appended, _> =
            spool::append_line(&paths.spool_dir, line, now_ms, spool::MAX_SPOOL_BYTES);
    }
    #[cfg(not(unix))]
    let _ = (paths, line, now_ms, budget);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn args_pick_the_harness_and_instance() {
        let a = parse_args(os(&["--harness", "codex", "--instance", "dev:1460"]));
        assert_eq!(a.harness, Harness::Codex);
        assert_eq!(a.instance.as_deref(), Some("dev:1460"));
        assert_eq!(
            parse_args(os(&["--harness", "claude-code"])).harness,
            Harness::ClaudeCode
        );
    }

    #[test]
    fn bad_or_missing_args_never_fail() {
        for args in [
            os(&[]),
            os(&["--harness"]),
            os(&["--harness", "gemini"]),
            os(&["--what", "--harness=codex", "x"]),
        ] {
            let a = parse_args(args);
            assert_eq!(a.harness, Harness::Unknown);
        }
    }

    #[test]
    fn instance_precedence_and_invalid_values() {
        assert_eq!(choose_instance(None, None), Some(Instance::Installed));
        assert_eq!(
            choose_instance(None, Some("dev:1420")),
            Some(Instance::Dev { port: 1420 })
        );
        assert_eq!(
            choose_instance(Some("installed"), Some("dev:1420")),
            Some(Instance::Installed),
            "the argument beats the environment"
        );
        assert_eq!(choose_instance(Some("dev:nope"), None), None);
        assert_eq!(choose_instance(None, Some("dev")), None);
        assert_eq!(choose_instance(None, Some("")), Some(Instance::Installed));
    }

    #[test]
    fn a_missing_explicit_instance_never_falls_back() {
        for flags in [
            &["--harness", "codex", "--instance"][..],
            &["--instance", "--harness", "codex"],
        ] {
            let args = parse_args(os(flags));
            assert_eq!(args.harness, Harness::Codex);
            assert_eq!(choose_instance(args.instance.as_deref(), None), None);
            assert_eq!(
                choose_instance(args.instance.as_deref(), Some("dev:1460")),
                None
            );
        }
    }

    #[test]
    fn configured_paths_ignore_the_hook_environment() {
        let shell_env = PathEnv {
            runtime_dir: Some("/shell/run".into()),
            data_home: Some("/shell/data".into()),
            home: Some("/shell/home".into()),
        };
        for instance in [Instance::Installed, Instance::Dev { port: 1460 }] {
            for runtime in [None, Some(std::ffi::OsStr::new("/desktop/run"))] {
                let expected = InstancePaths::from_installed_data_dir(
                    instance,
                    std::path::Path::new("/desktop/My Data/why"),
                    runtime,
                );
                let args = parse_args(expected.hook_args());
                let instance = choose_instance(args.instance.as_deref(), Some("dev:1420")).unwrap();
                assert_eq!(args.resolve_paths(instance, &shell_env), Some(expected));
            }
        }
    }

    #[test]
    fn incomplete_or_relative_configured_paths_never_fall_back() {
        let env = PathEnv {
            home: Some("/home/test".into()),
            ..PathEnv::default()
        };
        for flags in [
            &["--data-dir", "/app/why"][..],
            &["--socket-path", "/app/run/why.sock"],
            &["--data-dir", "/app/why", "--socket-path"],
            &[
                "--data-dir",
                "relative",
                "--socket-path",
                "/app/run/why.sock",
            ],
        ] {
            assert_eq!(
                parse_args(os(flags)).resolve_paths(Instance::Installed, &env),
                None
            );
        }
    }

    #[test]
    fn read_input_keeps_small_input_and_cuts_big_input() {
        assert_eq!(read_input(&b"{}"[..]), Some(Input::Whole(b"{}".to_vec())));
        assert_eq!(read_input(&b""[..]), Some(Input::Whole(Vec::new())));
        let exact = vec![b'x'; MAX_STDIN_BYTES];
        assert!(
            matches!(read_input(&exact[..]), Some(Input::Whole(v)) if v.len() == MAX_STDIN_BYTES)
        );
        let big = vec![b'x'; MAX_STDIN_BYTES + 10];
        match read_input(&big[..]) {
            Some(Input::Oversized { head, total }) => {
                assert_eq!(head.len(), MAX_STDIN_BYTES);
                assert_eq!(total, MAX_STDIN_BYTES as u64 + 10);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_oversized_envelope_keeps_the_ids_from_the_head() {
        let head = br#"{"session_id":"s1","cwd":"/r","hook_event_name":"PostToolUse","tool_input":{"content":"xxxx"#;
        let env =
            build_oversized_envelope(head, 20_000_000, Harness::ClaudeCode, 5, 6, "/fallback");
        assert_eq!(
            (env.event.as_str(), env.cwd.as_str()),
            ("PostToolUse", "/r")
        );
        assert_eq!(env.payload["session_id"], "s1");
        assert_eq!(env.payload["original_bytes"], 20_000_000);
        let no_ids = build_oversized_envelope(b"\xff", 9, Harness::Codex, 5, 6, "/fallback");
        assert_eq!(no_ids.cwd, "/fallback");
        assert!(why_wire::encode_line(&no_ids).is_ok());
    }

    #[test]
    fn envelope_from_a_real_claude_payload() {
        // Shape taken from the WHY-45 fixture `claude/2.1.322/basic-main`.
        let payload = json!({
            "session_id": "69c7ffb9-e8d9-43db-8d22-958e94e1e889",
            "transcript_path": "/HOME/.claude/projects/-TMP-fixture-repo/69c7ffb9.jsonl",
            "cwd": "/TMP/fixture-repo",
            "hook_event_name": "SessionStart",
            "source": "startup"
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let env = build_envelope(&bytes, Harness::ClaudeCode, 7, 99, "/elsewhere").unwrap();
        assert_eq!(env.v, PROTOCOL_VERSION);
        assert_eq!(env.event, "SessionStart");
        assert_eq!(env.cwd, "/TMP/fixture-repo");
        assert_eq!((env.received_at_ms, env.pid), (7, 99));
        assert_eq!(env.payload, payload);
        // And it fits on the wire.
        assert!(why_wire::encode_line(&env).is_ok());
    }

    #[test]
    fn non_objects_are_dropped() {
        for input in [
            &b""[..],
            b"garbage",
            b"[1,2]",
            b"\"str\"",
            b"{\"cut",
            b"\xff\xfe",
        ] {
            assert!(
                build_envelope(input, Harness::Codex, 0, 0, "/").is_none(),
                "{input:?}"
            );
        }
    }

    /// The binary test `a_panic_inside_the_hook_is_silent_and_exits_zero` only
    /// proves something if the switch really panics; this pins that.
    #[cfg(debug_assertions)]
    #[test]
    fn the_test_panic_switch_really_panics() {
        assert!(std::panic::catch_unwind(|| panic_if_requested(true)).is_err());
        assert!(std::panic::catch_unwind(|| panic_if_requested(false)).is_ok());
    }

    #[test]
    fn missing_cwd_and_event_fall_back() {
        let env = build_envelope(b"{}", Harness::Codex, 0, 0, "/proc/cwd").unwrap();
        assert_eq!(env.cwd, "/proc/cwd");
        assert_eq!(env.event, "");
    }
}
