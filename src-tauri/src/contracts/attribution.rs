//! Which agent turns explain a worktree's changed files (WHY-16).
//!
//! The reviewer sees the changed files from git. For each one the core answers
//! two questions from what the agents' hooks reported, nothing else:
//!
//! - which turns **edited** it (`explained_by`, and failed or unconfirmed
//!   `attempts`);
//! - whether no captured edit explains it (`unexplained`).
//!
//! # What these labels do NOT claim
//!
//! - **`unexplained` is not "manual" and not "by a human".** It means no edit
//!   was captured. The change may come from a person, from a shell command the
//!   agent ran (`sed -i`, `cargo fmt`, a code generator), from a hook delivery
//!   that was lost, or from before the hooks were installed. WHY cannot tell
//!   these apart, so it does not guess. The evidence label is `unknown`.
//! - **`explained` is not "fully explained".** It means at least one captured
//!   edit named the file. A file an agent edited may ALSO hold changes WHY never
//!   saw (a later `sed`, a human touch-up). File-level attribution cannot tell;
//!   splitting a file's lines between authors is line provenance (v0.3). There
//!   is deliberately no "partial" status and no score.
//! - **Time is never used.** A file's modification time and a turn's time are
//!   not compared; time only orders lists. The same rule keeps shell commands
//!   out of the attribution: they are listed per turn (`shell_leads`) as a lead,
//!   never as having touched a file.
//!
//! Wire rules as in `mod.rs`; timestamps are RFC 3339 strings.

use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

use super::changes::{DiffMode, FileChange};
use super::ids::WorktreeId;
use super::sessions::{EvidenceLabel, Harness, SessionId, ToolOutcome};

/// One edit-class tool call, as a pointer into a session's timeline.
///
/// `session_id` + `turn_id` locate the turn in `get_session_timeline`
/// (`turn_index` is its position there); `tool_use_id` (+ `agent_id` for a
/// subagent) locates the call inside the turn.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct TurnRef {
    pub harness: Harness,
    pub session_id: SessionId,
    /// The turn's opaque id; `""` for events that carried none.
    pub turn_id: String,
    /// The turn's 0-based index in the session's timeline.
    pub turn_index: u32,
    /// Set when a subagent (not the main agent) made the call.
    pub agent_id: Option<String>,
    /// The harness's id for the call; `null` when the harness gave none.
    pub tool_use_id: Option<String>,
    /// `ok`: the edit was applied. `failed`: the harness reported the edit
    /// failed or rejected it. `unknown`: a start with no result (rejected OR a
    /// lost delivery: never shown as "rejected").
    pub outcome: ToolOutcome,
    /// RFC 3339: when the call started (when only its result was seen, when
    /// that arrived). For display and ordering only.
    pub at: String,
    /// The worktree-relative path the call named (for a renamed file this may
    /// be its OLD path).
    pub path: String,
}

/// Whether a captured edit explains a changed file.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum AttributionStatus {
    /// At least one `ok` edit of the file (under its path, or its previous
    /// path for a rename) was captured. The file may hold other changes too.
    Explained,
    /// No `ok` edit was captured. NOT "manual": see the module docs.
    Unexplained,
}

/// One changed file and the turns that explain it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct FileAttribution {
    /// The git side: path, status, counts (as `get_changes` returns it).
    pub change: FileChange,
    pub status: AttributionStatus,
    /// `observed` when explained (a captured edit); `unknown` otherwise.
    pub evidence: EvidenceLabel,
    /// Calls whose outcome is `ok`, newest first.
    pub explained_by: Vec<TurnRef>,
    /// Calls that do NOT explain the file: `failed` or `unknown`, newest first.
    pub attempts: Vec<TurnRef>,
}

/// A turn that ran shell commands in this worktree: "look here" for changes no
/// edit explains. Never an attribution: what a command changed is only known to
/// git. Listed for the whole worktree, not per file.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct ShellLead {
    pub harness: Harness,
    pub session_id: SessionId,
    pub turn_id: String,
    pub turn_index: u32,
    /// Distinct shell calls (commands and test runs) in the turn.
    pub command_count: u32,
    /// RFC 3339: when the turn's first command started.
    pub first_at: String,
}

/// A worktree's changed files with the turns that explain each, plus the turns
/// that ran shell commands.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct WorktreeAttribution {
    pub worktree_id: WorktreeId,
    /// The comparison the file list was computed for.
    pub mode: DiffMode,
    /// In the order `get_changes` lists them.
    pub files: Vec<FileAttribution>,
    /// Newest first.
    pub shell_leads: Vec<ShellLead>,
}
