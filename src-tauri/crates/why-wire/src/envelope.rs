//! [`Envelope`]: one hook event as `why-hook` forwards it.
//!
//! The harnesses' payloads carry no harness name and no version (see
//! `docs/agents/payloads.md` §7), so the hook wraps each payload in an envelope
//! that adds what only the hook knows: which harness ran it, when, where, and
//! from which process. The payload itself is kept verbatim as a JSON value; the
//! app's adapters (`src-tauri/src/adapters/`) interpret it.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Version of the envelope format. Bump it when a change would make an older app
/// misread a newer hook's envelope (a field changes meaning or type); adding an
/// optional field does not need a bump, because unknown fields are ignored.
///
/// The app rejects envelopes with a version it does not know instead of
/// guessing (see [`crate::frame::decode_line`]).
pub const PROTOCOL_VERSION: u32 = 1;

/// The coding agent that ran the hook.
///
/// On the wire: `"claude-code"`, `"codex"`. The same spelling is what
/// `why-hook --harness <name>` accepts, and it ends up in users' hook configs,
/// so it must never change (pinned by tests).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    ClaudeCode,
    Codex,
    /// A harness this build does not know (a newer hook, or a typo in a hook
    /// config). `#[serde(other)]` makes any unknown string land here instead of
    /// failing the whole envelope, so an old app keeps accepting the rest.
    #[serde(other)]
    Unknown,
}

impl Harness {
    /// The stable spelling (`claude-code`, `codex`, `unknown`).
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::ClaudeCode => "claude-code",
            Harness::Codex => "codex",
            Harness::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Harness {
    type Err = std::convert::Infallible;

    /// Never fails: anything that is not a known name is [`Harness::Unknown`]
    /// (the hook must not refuse to run because of a bad argument).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "claude-code" => Harness::ClaudeCode,
            "codex" => Harness::Codex,
            _ => Harness::Unknown,
        })
    }
}

/// One hook event, as the hook forwards it and the collector receives it.
///
/// Field names are snake_case on the wire (this is not a ts-rs contract; the UI
/// never sees it), and `u64` is fine here for the same reason.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Envelope {
    /// Envelope format version, [`PROTOCOL_VERSION`] when written by this build.
    pub v: u32,
    pub harness: Harness,
    /// The harness's version, when the hook can tell (no payload carries it
    /// today, so this is usually `None`).
    pub harness_version: Option<String>,
    /// `hook_event_name` from the payload, as received (`PreToolUse`, ...).
    /// Kept as a string: harnesses add events faster than we add enum variants.
    pub event: String,
    /// When the hook read the payload, in milliseconds since the Unix epoch
    /// (the hook's wall clock). Set ONCE by the hook, so an envelope re-sent
    /// from the spool keeps it: that is what lets the app recognise a duplicate
    /// delivery (see `events::EventKey` in the app).
    pub received_at_ms: u64,
    /// The `cwd` field of the payload when present, else the hook process's own
    /// working directory (which the harnesses set to the session cwd). It can be
    /// a subdirectory of a worktree; the app resolves it with git.
    pub cwd: String,
    /// Best effort: the id of the process that spawned the hook (the harness,
    /// or a short-lived `sh -c` in between). Useful for logs and as a liveness
    /// hint; never an identity (pids are reused). `0` when unknown.
    pub pid: u32,
    /// The harness's stdin payload, verbatim.
    pub payload: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_spellings_are_pinned() {
        // These strings end up in users' hook configs (`--harness codex`) and in
        // stored envelopes; changing one silently breaks installed hooks.
        for (h, s) in [
            (Harness::ClaudeCode, "claude-code"),
            (Harness::Codex, "codex"),
        ] {
            assert_eq!(serde_json::to_value(h).unwrap(), serde_json::json!(s));
            assert_eq!(s.parse::<Harness>().unwrap(), h);
            assert_eq!(h.to_string(), s);
        }
    }

    #[test]
    fn unknown_harness_names_do_not_fail() {
        let h: Harness = serde_json::from_str("\"antigravity\"").unwrap();
        assert_eq!(h, Harness::Unknown);
        assert_eq!("Claude".parse::<Harness>().unwrap(), Harness::Unknown);
        assert_eq!("".parse::<Harness>().unwrap(), Harness::Unknown);
    }
}
