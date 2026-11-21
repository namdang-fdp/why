//! Agent sessions as the UI sees them: a session summary per worktree, and a
//! session's timeline of turns and events.
//!
//! Built by the core from hook events (`events/` module); the UI only reads
//! them. Wire rules as everywhere (see `mod.rs`), plus two specific to this file:
//!
//! - **Timestamps are RFC 3339 strings** in UTC with milliseconds
//!   (`2026-11-20T15:28:47.123Z`). Internally they are `u64` milliseconds, and
//!   a `u64` cannot cross the boundary (ts-rs would type it `bigint`, which
//!   `JSON.parse` never produces). Strings also sort correctly as text.
//! - **Ids are opaque strings.** The UI never builds or parses them; it only
//!   passes them back (`get_session_timeline(session.id)`).

use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

use super::ids::WorktreeId;

/// Opaque identifier of an agent session (it encodes the harness and the
/// harness's own session id; the two harnesses' id spaces could overlap).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(test, derive(TS))]
pub struct SessionId(pub String);

/// The coding agent that ran a session.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum Harness {
    ClaudeCode,
    Codex,
    /// A harness this build has no adapter for. Kept so a newer hook's events
    /// never make the whole list fail; the UI shows it generically.
    Unknown,
}

/// Whether the session is still running, as far as the hooks can tell.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    /// Started (or resumed) and not ended since.
    Live,
    /// The last lifecycle event was `SessionEnd`. Note: each `claude -p` /
    /// `codex exec` process ends with its own `SessionEnd`, and a resume makes
    /// the same session live again.
    Ended,
    /// Not enough evidence to say: no lifecycle event was seen (for example the
    /// hook was installed mid-session), OR the session looked live but no event
    /// of any kind arrived for 30 minutes (`STALE_AFTER_MS` in
    /// `events/timeline.rs`), which is what an agent killed without a
    /// `SessionEnd` looks like. Computed when the list is read; any new event
    /// makes a stale session live again.
    Unknown,
}

/// How much to believe a timeline entry (see `why-brain/03 - Provenance Engine.md`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum EvidenceLabel {
    /// Captured by the harness: a real tool call, edit or output.
    Observed,
    /// The agent's own words at the time (a message, a command description).
    Declared,
    /// Reconstructed by WHY; clearly labelled as such in the UI.
    Inferred,
    /// Not enough evidence to say.
    Unknown,
}

/// How a tool call ended.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum ToolOutcome {
    Ok,
    Failed,
    /// No result was seen: a `PreToolUse` without a `PostToolUse`. That can be
    /// a call the harness rejected OR a lost delivery, so it is never shown as
    /// "rejected" (payloads.md §3, fact 2).
    Unknown,
}

/// One agent session in a worktree's list.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: SessionId,
    pub harness: Harness,
    /// The model, when the harness reports it (Codex does; Claude Code hooks
    /// carry no model).
    pub model: Option<String>,
    /// The worktree the session's most recent event resolved to, if any.
    pub worktree_id: Option<WorktreeId>,
    /// Working directory of the session's most recent event (it may be a
    /// subdirectory of the worktree).
    pub cwd: String,
    /// Excerpt of the first prompt, as a title for the session.
    pub task: Option<String>,
    /// RFC 3339.
    pub started_at: String,
    /// RFC 3339.
    pub last_event_at: String,
    pub status: SessionStatus,
    pub turn_count: u32,
}

/// A session and everything that happened in it, turn by turn.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionTimeline {
    pub session: SessionSummary,
    /// In order (`index` 0, 1, ...).
    pub turns: Vec<Turn>,
}

/// One user prompt and what the agent did about it.
///
/// Note: a turn's prompt is not always typed by a human (Claude Code injects a
/// `<task-notification>` prompt when a background subagent finishes).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    /// Opaque; unique within its session.
    pub id: String,
    /// 0-based position in the session.
    pub index: u32,
    /// Excerpt of the prompt, when the prompt event was seen.
    pub prompt: Option<String>,
    /// RFC 3339.
    pub started_at: String,
    /// RFC 3339 of the turn's last `Stop`; `null` while running (or if the
    /// `Stop` was not delivered).
    pub ended_at: Option<String>,
    /// In time order.
    pub events: Vec<TimelineEvent>,
}

/// What kind of thing a timeline entry is.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum TimelineEventKind {
    Prompt,
    AssistantMessage,
    /// A tool call that is not one of the more specific kinds below.
    ToolCall,
    /// A tool call that changed files (Write/Edit, `apply_patch`).
    Edit,
    /// A shell command.
    Shell,
    /// A shell command recognised as running tests.
    TestRun,
    /// A subagent started or finished.
    Subagent,
    /// The agent stopped and handed control back.
    Stop,
}

/// One entry in a turn.
///
/// A tool call is ONE entry even though the harness sends two events for it
/// (`PreToolUse` and `PostToolUse`): the core pairs them by agent and `tool_use_id`,
/// whatever order they arrived in, and the entry's `outcome` comes from the
/// result (or is `unknown` when no result arrived).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct TimelineEvent {
    /// Opaque; unique within its session. For tool rows with a `tool_use_id`,
    /// pass this as `event_id` to `get_tool_rationale`.
    pub id: String,
    pub kind: TimelineEventKind,
    /// RFC 3339 (for a tool call: when it started, if that was seen).
    pub at: String,
    /// The harness's tool name (`Bash`, `Edit`, `apply_patch`, ...).
    pub tool_name: Option<String>,
    /// One line for the list (a command, a file name, a message excerpt).
    pub summary: String,
    /// Files involved, relative to the worktree root when the core could
    /// resolve them, else as the harness reported them (absolute).
    pub files: Vec<String>,
    pub evidence: EvidenceLabel,
    /// Tool calls only.
    pub outcome: Option<ToolOutcome>,
    /// Set when a subagent (not the main agent) did this.
    pub agent_id: Option<String>,
    /// The harness's ID for a tool call. `null` for other entries and for calls
    /// the harness gave no ID.
    pub tool_use_id: Option<String>,
}
