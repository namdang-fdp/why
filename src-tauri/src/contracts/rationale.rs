//! What the agent said around a turn or a tool call (*Declared*), and shell
//! outcomes the harness's own log proves (*Observed*) (WHY-53).
//!
//! The hooks WHY receives carry only a capped `last_assistant_message`. The
//! agent's full words live in the harness's own transcript file (Claude Code:
//! JSONL under `~/.claude/projects`; Codex: a "rollout" under
//! `~/.codex/sessions`). WHY reads that file **only when the user asks** (these
//! commands), never while ingesting events, and never stores the text: it
//! crosses the wire as capped excerpts and is dropped.
//!
//! # Evidence
//!
//! - Rationale text is `declared`: the agent's own words, which may be wrong or
//!   boastful. It explains, it does not prove (see the Provenance Engine note).
//! - A shell outcome is `observed`: it comes from a structured record of the
//!   harness (Codex's `CommandExecution` with an `exit_code`) matched to the
//!   hook's `tool_use_id` and turn ID exactly. It is never inferred from output text.
//!
//! # "Not available" is a value, not an error
//!
//! A transcript that was deleted, moved outside the harness directory, or never
//! recorded is a normal situation the UI shows ("the transcript is no longer on
//! disk"). The commands therefore return a tagged result
//! (`{ status: "ok" | "unavailable" }`); [`IpcError`](super::IpcError) is only
//! for ids the store does not know and for store failures.
//!
//! Wire rules as in `mod.rs`. The enums here are tagged by `status`; their
//! variants hold ONE named payload (not loose fields), so field names stay
//! camelCase without extra serde attributes.

use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

use super::sessions::{EvidenceLabel, ToolOutcome};

/// Which harness file the text or the exit code was read from.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum TranscriptSource {
    /// A Claude Code transcript (`~/.claude/projects/**.jsonl`).
    ClaudeTranscript,
    /// A Codex rollout (`~/.codex/sessions/**.jsonl`).
    CodexRollout,
}

/// Why nothing could be read. Each is a normal outcome the UI explains.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum TranscriptUnavailable {
    /// No `transcript_path` was recorded for this turn or call.
    NoTranscript,
    /// The recorded file no longer exists.
    Missing,
    /// The recorded path is not inside the harness's own directory (or it
    /// reaches it through a symlink). WHY refuses to open it.
    OutsideRoot,
    /// This platform cannot do the confined open WHY requires (needs Linux
    /// `openat2` today).
    UnsupportedPlatform,
    /// The file was read but holds nothing for this turn or call (for example
    /// after a compaction, or a subagent whose words are in another file).
    NotFound,
    /// Anything else: not a regular file (a FIFO, a directory), an I/O error,
    /// or the harness directory could not be located.
    Unreadable,
    /// The session's harness has no transcript reader in this build.
    UnsupportedHarness,
    /// Shell outcomes only: this harness's hooks already report how a command
    /// ended, so there is nothing to upgrade.
    NotApplicable,
}

/// Where a block sits relative to what the user asked about.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum BlockPosition {
    /// A turn's words (all of them, in order).
    Turn,
    /// Said just before the tool call.
    Before,
    /// Said just after the tool call (Codex only).
    After,
}

/// One piece of the agent's text.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct RationaleBlock {
    /// Capped at 8 KiB with a trailing `…` when cut. Never decrypted: an
    /// encrypted Codex message is the literal `(encrypted)` and `encrypted`
    /// is set.
    pub text: String,
    pub position: BlockPosition,
    /// RFC 3339 when the transcript records one (Claude does, Codex does not).
    pub timestamp: Option<String>,
    pub encrypted: bool,
}

/// The agent's own words for a turn or around a tool call.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct Rationale {
    /// Always `declared`.
    pub evidence: EvidenceLabel,
    pub source: TranscriptSource,
    /// In transcript order. Empty is possible (the agent said nothing there).
    pub blocks: Vec<RationaleBlock>,
    /// The harness version the transcript names (Claude `version`, Codex
    /// `cli_version`), when it names one.
    pub harness_version: Option<String>,
    /// The version is one WHY's reader was checked against. When `false` the
    /// text is still returned, but the record shapes were not verified.
    pub version_supported: bool,
    /// The read stopped at a limit (bytes read, blocks kept, text per block,
    /// or messages per turn), so there may be more words than shown, or the
    /// anchor may lie beyond what was read. With no blocks this means "not
    /// found YET", never "the agent said nothing".
    pub truncated: bool,
}

/// Result of `get_turn_rationale` / `get_tool_rationale`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum RationaleResult {
    Ok { rationale: Rationale },
    Unavailable { reason: TranscriptUnavailable },
}

/// A shell or test-run call whose outcome the harness's log settles.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct ShellOutcome {
    /// The call's `tool_use_id` (equal to the structured record's id).
    pub tool_use_id: String,
    /// The matching timeline entry's `id` (`TimelineEvent.id`), so the UI can
    /// put the outcome on the right row.
    pub event_id: String,
    pub turn_id: String,
    /// Set when a subagent made the call.
    pub agent_id: Option<String>,
    /// The process's exit code, read from the structured record's field.
    pub exit_code: i32,
    /// `ok` for exit code 0, `failed` otherwise. Never `unknown` here.
    pub outcome: ToolOutcome,
    /// Always `observed`.
    pub evidence: EvidenceLabel,
    pub source: TranscriptSource,
}

/// What a pass over the rollout settled.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct ShellOutcomes {
    /// Only calls that were `unknown` and have exactly one matching record.
    /// Oldest first. A call already `ok`/`failed` from the hooks never appears.
    pub outcomes: Vec<ShellOutcome>,
    /// Calls that stay `unknown` (no path, no matching record, an ambiguous
    /// record, an untrusted version, a file that could not be read).
    pub unresolved: u32,
    /// The version from the most recent scanned rollout, or from the most
    /// recent unsupported rollout if any scanned version was unsupported.
    pub harness_version: Option<String>,
    /// Every scanned rollout reports a Codex version whose record shape was
    /// verified. When `false`, every call stays `unknown`.
    pub version_supported: bool,
    /// A file was bigger than the scan window, or a per-call limit (files
    /// read, records kept) was hit: some calls may be missing from `outcomes`.
    pub truncated: bool,
}

/// Result of `get_shell_outcomes`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ShellOutcomesResult {
    Ok { result: ShellOutcomes },
    Unavailable { reason: TranscriptUnavailable },
}
