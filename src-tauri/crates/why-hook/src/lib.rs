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
//! WHY-46 is the stub: it reads stdin, builds an [`Envelope`] and stops there.
//! WHY-13 adds the allowlist check, the socket send and the spool. Everything
//! is in this library (not `main.rs`) so it can be unit tested.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use why_wire::{
    Envelope, Harness, INSTANCE_ENV, Instance, InstancePaths, MAX_LINE_BYTES, PROTOCOL_VERSION,
    PathEnv,
};

/// Most stdin we keep. A payload bigger than one wire line can never be sent
/// as-is, so there is no point holding more of it in memory.
pub const MAX_STDIN_BYTES: usize = MAX_LINE_BYTES;

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

/// Read stdin, keeping at most [`MAX_STDIN_BYTES`]. Returns `None` when the
/// payload is larger than that (it is then drained, see [`DRAIN_LIMIT_BYTES`])
/// or when reading fails. A payload cut short by the harness closing stdin
/// early is returned as-is; it simply fails to parse later.
pub fn read_bounded(mut input: impl Read) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    // `take(N + 1)`: reading one byte past the limit is how we learn the
    // payload is too big without buffering all of it.
    let read = input
        .by_ref()
        .take(MAX_STDIN_BYTES as u64 + 1)
        .read_to_end(&mut buf);
    if buf.len() > MAX_STDIN_BYTES {
        let _ = io::copy(&mut input.take(DRAIN_LIMIT_BYTES), &mut io::sink());
        return None;
    }
    read.ok().map(|_| buf)
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
    let Some(input) = read_bounded(io::stdin().lock()) else {
        return;
    };
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(envelope) = build_envelope(&input, args.harness, now_ms(), parent_pid(), &cwd) else {
        return;
    };

    // WHY-13 continues from here: check `envelope.cwd` against the allowlist at
    // `paths.allowlist_path`, send `encode_line(&envelope)` to
    // `paths.socket_path` with a short timeout, and append to `paths.spool_dir`
    // when the app is not there. The stub computes both and drops them.
    let Some(paths) = args.resolve_paths(instance, &PathEnv::from_process()) else {
        return;
    };
    let line = why_wire::encode_line(&envelope);
    drop((paths, line));
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
    fn read_bounded_keeps_small_input_and_refuses_big_input() {
        assert_eq!(read_bounded(&b"{}"[..]), Some(b"{}".to_vec()));
        assert_eq!(read_bounded(&b""[..]), Some(Vec::new()));
        let exact = vec![b'x'; MAX_STDIN_BYTES];
        assert_eq!(
            read_bounded(&exact[..]).map(|v| v.len()),
            Some(MAX_STDIN_BYTES)
        );
        let big = vec![b'x'; MAX_STDIN_BYTES + 1];
        assert_eq!(read_bounded(&big[..]), None);
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
