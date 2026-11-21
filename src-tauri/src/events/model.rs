//! The core's internal model of agent activity: what an adapter produces from a
//! hook [`Envelope`] and what an [`EventStore`](super::EventStore) keeps.
//!
//! Internal types, not IPC contracts: they use `u64` milliseconds and keep
//! fields the UI never sees (the dedupe key, the transcript path). The UI gets
//! [`SessionSummary`](crate::contracts::SessionSummary) /
//! [`SessionTimeline`](crate::contracts::SessionTimeline), built from these by
//! [`super::timeline`].
//!
//! # Keys (from the WHY-45 fixtures, payloads.md §5)
//!
//! | Key | Claude Code | Codex | Used for |
//! |---|---|---|---|
//! | session | `(harness, session_id)` | same | [`SessionKey`]; stable across resume |
//! | turn | `prompt_id` | `turn_id` | [`NormalizedEvent::turn_id`] |
//! | tool call | `tool_use_id` | `tool_use_id` | **correlation**: pairs Pre/Post |
//! | subagent | `agent_id` | `agent_id` | events share the parent's session id |
//! | delivery | [`EventKey`] | same | **deduplication** |
//!
//! A correlation key is shared by events that belong together (a `PreToolUse`
//! and its `PostToolUse` have the same `tool_use_id`); a dedupe key is shared
//! only by the *same* event delivered twice. Deduplicating on `tool_use_id`
//! would throw away every `PostToolUse`.

use sha2::{Digest, Sha256};
use why_wire::{Envelope, Harness};

use crate::contracts::{EvidenceLabel, SessionId, ToolOutcome, WorktreeId};

/// Excerpts (prompts, messages, commands) are capped at this many bytes when an
/// adapter normalizes an event (decision 22): prompts and assistant messages are
/// user content, can be huge, and the full text stays in the harness's own
/// transcript (`transcript_path`), which WHY reads lazily if ever.
pub const EXCERPT_MAX_BYTES: usize = 8 * 1024;

/// Cut `text` to at most [`EXCERPT_MAX_BYTES`] bytes, on a character boundary,
/// marking the cut with `…`. Text that fits is returned unchanged.
pub fn excerpt(text: &str) -> String {
    excerpt_with_truncation(text).0
}

pub(crate) fn excerpt_with_truncation(text: &str) -> (String, bool) {
    if text.len() <= EXCERPT_MAX_BYTES {
        return (text.to_owned(), false);
    }
    const MARK: &str = "…";
    let mut end = EXCERPT_MAX_BYTES - MARK.len();
    // Slicing a `str` in the middle of a multi-byte character panics; step back
    // to the start of the character.
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}{MARK}", &text[..end]), true)
}

/// Identifies a session: the harness plus the harness's own session id. Both
/// harnesses keep the id across `--resume`/`--continue`/`exec resume`, so a
/// resumed session is the same key (an upsert, never a second session).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionKey {
    pub harness: Harness,
    pub session_id: String,
}

impl SessionKey {
    pub fn new(harness: Harness, session_id: impl Into<String>) -> Self {
        Self {
            harness,
            session_id: session_id.into(),
        }
    }

    /// The opaque id the UI gets: `<harness>:<session id>`.
    pub fn to_contract(&self) -> SessionId {
        SessionId(format!("{}:{}", self.harness, self.session_id))
    }

    /// Parse an id made by [`SessionKey::to_contract`]. `None` for anything else.
    pub fn from_contract(id: &SessionId) -> Option<Self> {
        let (harness, session_id) = id.0.split_once(':')?;
        if session_id.is_empty() {
            return None;
        }
        // Not `Harness::from_str`: it maps any unknown name to `Unknown`. Only
        // the exact spellings `to_contract` writes are accepted.
        let harness = [Harness::ClaudeCode, Harness::Codex, Harness::Unknown]
            .into_iter()
            .find(|h| h.as_str() == harness)?;
        Some(Self::new(harness, session_id))
    }
}

/// The deduplication key of one normalized event.
///
/// `<delivery digest>:<index>`, where the digest covers what makes a hook
/// delivery unique: harness, `hook_event_name`, the hook's `received_at_ms` and
/// the whole payload; `index` is the event's position in what the adapter
/// produced from that envelope (one envelope may yield several events).
///
/// Why this works: the hook stamps `received_at_ms` ONCE, so an envelope that is
/// delivered twice (sent, then re-sent from the spool because the
/// acknowledgement was lost) hashes the same and is stored once. Two distinct
/// events never share it: even byte-identical payloads (Codex's repeated
/// `Stop` in a blocked loop) arrive at different milliseconds, and a Pre/Post
/// pair differs in `hook_event_name` and payload. (Two identical payloads in the
/// same millisecond would merge; that is accepted as harmless.)
///
/// Persisted by WHY-12, so the derivation must never change silently: a golden
/// test pins it. Change the domain separator (`v1`) if it ever has to change.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventKey(pub String);

impl EventKey {
    /// The key of the `index`-th event normalized from `env`.
    pub fn for_delivery(env: &Envelope, index: u32) -> Self {
        DeliveryDigest::new(env).event_key(index)
    }
}

/// The shared prefix of all event keys from one hook delivery.
pub(crate) struct DeliveryDigest(String);

impl DeliveryDigest {
    pub(crate) fn new(env: &Envelope) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"why:event-key:v1\0");
        feed(&mut hasher, env.harness.as_str().as_bytes());
        feed(&mut hasher, env.event.as_bytes());
        hasher.update(env.received_at_ms.to_le_bytes());
        // `Value` serializes deterministically for the same parsed input, which
        // is all a redelivery needs (it re-sends the same bytes).
        let payload = serde_json::to_vec(&env.payload).unwrap_or_default();
        feed(&mut hasher, &payload);
        let digest = hasher.finalize();
        // 128 bits: no realistic collision across years of one person's events.
        Self(digest[..16].iter().map(|b| format!("{b:02x}")).collect())
    }

    pub(crate) fn event_key(&self, index: u32) -> EventKey {
        EventKey(format!("{}:{index}", self.0))
    }
}

/// Length-prefixed, so `("ab", "c")` and `("a", "bc")` hash differently.
fn feed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// What happened, in WHY's vocabulary (adapters map each harness's events here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// `SessionStart`. `resumed` for `source: "resume"`: same session, not a
    /// new one.
    SessionStart {
        resumed: bool,
    },
    /// `SessionEnd`: this harness *process* ended (a later resume can follow).
    SessionEnd,
    /// `UserPromptSubmit`.
    Prompt,
    /// Something the agent said (`Stop.last_assistant_message`, transcript).
    AssistantMessage,
    /// `PreToolUse`.
    ToolStarted,
    /// `PostToolUse` (`Ok`), `PostToolUseFailure` (`Failed`), or anything else
    /// that reports a tool's result.
    ToolFinished {
        outcome: ToolOutcome,
    },
    SubagentStarted,
    SubagentStopped,
    /// `Stop`: the agent handed control back (ends a turn).
    Stop,
}

impl EventKind {
    /// Session lifecycle events decide the session's status and are not shown
    /// inside turns (Claude's `SessionEnd` carries a `prompt_id`, but it does
    /// not belong to that turn).
    pub fn is_lifecycle(&self) -> bool {
        matches!(self, EventKind::SessionStart { .. } | EventKind::SessionEnd)
    }
}

/// How the timeline should present a tool call. Chosen by the adapter, which
/// knows its harness's tool names (`Edit`/`Write` vs `apply_patch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolClass {
    Edit,
    Shell,
    TestRun,
    Subagent,
    Other,
}

/// The tool part of a `ToolStarted` / `ToolFinished` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// `tool_use_id`: the correlation key pairing start and finish. `None`
    /// when the harness gave none (the call is then shown on its own).
    pub use_id: Option<String>,
    /// The harness's tool name, verbatim.
    pub name: String,
    pub class: ToolClass,
}

/// One event, normalized from a hook payload by an adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedEvent {
    /// Deduplication key (see [`EventKey`]).
    pub key: EventKey,
    pub session: SessionKey,
    /// `hook_event_name` as received, for debugging and future re-normalizing.
    pub hook_event: String,
    pub kind: EventKind,
    /// When the hook saw it (the envelope's `received_at_ms`).
    pub at_ms: u64,
    /// The turn key: Claude `prompt_id`, Codex `turn_id`. `None` for events
    /// outside any turn (`SessionStart`).
    pub turn_id: Option<String>,
    /// Set when a subagent (not the main agent) produced the event.
    pub agent_id: Option<String>,
    /// For `ToolStarted` / `ToolFinished`.
    pub tool: Option<ToolCall>,
    /// The event's own working directory (it can differ per event: a
    /// subdirectory, another worktree). Resolved to a worktree by the ingest
    /// step, never by string prefix.
    pub cwd: String,
    /// Filled by the ingest step from `cwd` (adapters leave it `None`).
    pub worktree_id: Option<WorktreeId>,
    /// The model, when the payload names it (Codex does, Claude does not).
    pub model: Option<String>,
    /// Capped with [`excerpt`]: the prompt, the message, the command, ...
    pub text: Option<String>,
    /// Files the event touched or read. Absolute as reported by the harness;
    /// the ingest step rewrites them relative to the worktree root when the
    /// file is inside it.
    pub files: Vec<String>,
    pub evidence: EvidenceLabel,
    /// The harness transcript, for lazy reading later (never copied).
    pub transcript_path: Option<String>,
}

impl NormalizedEvent {
    /// The fields every adapter fills the same way: key, session, hook event,
    /// time and cwd from the envelope; evidence `Observed` (it came from the
    /// harness itself). Everything else starts empty for the adapter to set.
    pub fn from_envelope(
        env: &Envelope,
        index: u32,
        session_id: impl Into<String>,
        kind: EventKind,
    ) -> Self {
        Self::from_delivery_key(env, EventKey::for_delivery(env, index), session_id, kind)
    }

    pub(crate) fn from_delivery_key(
        env: &Envelope,
        key: EventKey,
        session_id: impl Into<String>,
        kind: EventKind,
    ) -> Self {
        Self {
            key,
            session: SessionKey::new(env.harness, session_id),
            hook_event: env.event.clone(),
            kind,
            at_ms: env.received_at_ms,
            turn_id: None,
            agent_id: None,
            tool: None,
            cwd: env.cwd.clone(),
            worktree_id: None,
            model: None,
            text: None,
            files: Vec::new(),
            evidence: EvidenceLabel::Observed,
            transcript_path: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn envelope(event: &str, at: u64, payload: serde_json::Value) -> Envelope {
        Envelope {
            v: 1,
            harness: Harness::ClaudeCode,
            harness_version: None,
            event: event.into(),
            received_at_ms: at,
            cwd: "/r".into(),
            pid: 1,
            payload,
        }
    }

    #[test]
    fn excerpt_keeps_short_text_and_cuts_on_a_char_boundary() {
        assert_eq!(excerpt("short"), "short");
        let exact = "a".repeat(EXCERPT_MAX_BYTES);
        assert_eq!(excerpt(&exact), exact);

        // 3-byte characters, so the naive cut point lands mid-character.
        let long = "世".repeat(EXCERPT_MAX_BYTES);
        let cut = excerpt(&long);
        assert!(cut.len() <= EXCERPT_MAX_BYTES, "{} bytes", cut.len());
        assert!(cut.ends_with('…'));
        assert!(cut.trim_end_matches('…').chars().all(|c| c == '世'));
    }

    #[test]
    fn session_ids_round_trip_and_reject_garbage() {
        let key = SessionKey::new(Harness::Codex, "01a1f812-8d57-7393-87df-7cfc4fefbfb3");
        let id = key.to_contract();
        assert_eq!(id.0, "codex:01a1f812-8d57-7393-87df-7cfc4fefbfb3");
        assert_eq!(SessionKey::from_contract(&id), Some(key));
        let claude = SessionKey::new(Harness::ClaudeCode, "a:b");
        assert_eq!(
            SessionKey::from_contract(&claude.to_contract()),
            Some(claude)
        );

        for bad in ["", "codex", "codex:", "Codex:x", "gemini:x", ":x"] {
            assert_eq!(
                SessionKey::from_contract(&SessionId(bad.into())),
                None,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn redelivery_has_the_same_key_distinct_events_do_not() {
        let payload =
            json!({"session_id": "s", "tool_use_id": "t1", "hook_event_name": "PreToolUse"});
        let pre = envelope("PreToolUse", 100, payload.clone());
        assert_eq!(
            EventKey::for_delivery(&pre, 0),
            EventKey::for_delivery(&pre.clone(), 0)
        );
        // Same tool_use_id, different event: the correlation key is shared,
        // the dedupe key is not.
        let mut post_payload = payload.clone();
        post_payload["hook_event_name"] = json!("PostToolUse");
        let post = envelope("PostToolUse", 100, post_payload);
        assert_ne!(
            EventKey::for_delivery(&pre, 0),
            EventKey::for_delivery(&post, 0)
        );
        // Only `event` differs.
        let renamed = envelope("PostToolUse", 100, payload.clone());
        assert_ne!(
            EventKey::for_delivery(&pre, 0),
            EventKey::for_delivery(&renamed, 0)
        );
        // Identical payload, later millisecond (a repeated Stop).
        let later = envelope("PreToolUse", 101, payload);
        assert_ne!(
            EventKey::for_delivery(&pre, 0),
            EventKey::for_delivery(&later, 0)
        );
        // Second event from the same envelope.
        assert_ne!(
            EventKey::for_delivery(&pre, 0),
            EventKey::for_delivery(&pre, 1)
        );
    }

    #[test]
    fn event_key_derivation_is_pinned() {
        // WHY-12 persists these keys: if this value changes, stored events
        // would stop deduplicating against new deliveries. Change the domain
        // separator version instead of editing this expectation.
        let env = envelope(
            "Stop",
            1_795_163_026_192,
            json!({"session_id": "s1", "prompt_id": "p1"}),
        );
        // Recomputed independently (Python hashlib) from the documented recipe:
        // sha256("why:event-key:v1\0" + len-prefixed "claude-code", "Stop",
        // u64-le ms, len-prefixed compact payload with sorted keys)[..16].
        // It also pins that serde_json sorts object keys here; if a dependency
        // ever enables serde_json's `preserve_order` feature, this fails.
        assert_eq!(
            EventKey::for_delivery(&env, 0).0,
            "552f91e63800a4972983d7f27e78d17e:0"
        );
    }
}
