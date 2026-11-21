//! Which agent turn wrote each line of a file (v0.3, WHY-64).
//!
//! The reviewer clicks a line; the core answers from the captured edits only,
//! replayed deterministically from their line hashes (decision 29):
//!
//! - `base`: the line is unchanged since the comparison base;
//! - `edit`: a captured, successful edit wrote it (`turn` points into
//!   `get_session_timeline`);
//! - `unknown`: WHY cannot prove where it came from, with the reason.
//!
//! # What these labels do NOT claim
//!
//! - **`unknown` is not "manual" and not "by a human"** (same rule as
//!   `unexplained` in `attribution.rs`): a shell command, a formatter, a lost
//!   delivery or a person all look the same from here.
//! - **The evidence is `observed` or `unknown`, never `declared` or
//!   `inferred`.** A line's origin comes from what the harness reported it
//!   did, not from what the agent said; the agent's reasons stay a separate,
//!   lazy read (`get_turn_rationale` / `get_tool_rationale`).
//!
//! # "Not available" is a value, not an error
//!
//! Like `RationaleResult`: a file whose origins cannot be computed (the engine
//! is not built yet, a binary file, a file too large) is a normal answer the UI
//! explains, tagged by `status`. [`IpcError`](super::IpcError) is for bad
//! requests and store failures.
//!
//! Wire rules as in `mod.rs`.

use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

use super::attribution::TurnRef;
use super::ids::WorktreeId;
use super::sessions::{EvidenceLabel, Harness};

/// Why a line's origin is unknown.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum LineUnknownReason {
    /// No captured edit explains the line (shell write, formatter, a person,
    /// hooks not installed yet, a lost delivery).
    NoCapture,
    /// An edit touched the file but the hook reduced its payload (over the
    /// wire cap, or redacted).
    PayloadReduced,
    /// An edit touched the file but its payload could not be read.
    Unparsed,
    /// The file WHY replayed did not match what a later edit saw, so the
    /// lines in between cannot be trusted.
    ReplayMismatch,
    /// A Codex patch's context matched in several places of the file.
    AmbiguousMatch,
    /// Another writer changed the file while an agent was editing it.
    ConcurrentWriter,
    /// The replay has not run for this file.
    NotReplayed,
}

/// Where one line came from.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LineOriginKind {
    /// Unchanged since the comparison base.
    Base,
    /// Written by this captured edit (outcome always `ok`).
    Edit {
        turn: TurnRef,
    },
    Unknown {
        reason: LineUnknownReason,
    },
}

/// A run of consecutive lines with the same origin.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct LineRun {
    /// 1-based, inclusive. The runs of a file partition `1..=lineCount` in
    /// order, without gaps or overlaps.
    pub start_line: u32,
    /// 1-based, inclusive (`endLine >= startLine`).
    pub end_line: u32,
    pub origin: LineOriginKind,
    /// `observed` for `base` and `edit`, `unknown` for `unknown`.
    pub evidence: EvidenceLabel,
}

/// How many explained lines one harness wrote.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct HarnessLines {
    pub harness: Harness,
    pub lines: u32,
}

/// Line counts by origin. `lines == base + unknown + sum(explained.lines)`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct LineCoverage {
    pub lines: u32,
    pub base: u32,
    pub unknown: u32,
    /// Lines with an `edit` origin, PER HARNESS (decision 30: Codex coverage
    /// is reported apart from Claude's). Only harnesses with at least one
    /// line appear.
    pub explained: Vec<HarnessLines>,
}

/// The origin of every line of one file of a worktree.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub struct FileLineOrigins {
    pub worktree_id: WorktreeId,
    /// Worktree-relative, `/`-separated (the canonical form of the request).
    pub path: String,
    pub line_count: u32,
    pub runs: Vec<LineRun>,
    pub coverage: LineCoverage,
}

/// Why no origins could be computed. Each is a normal outcome the UI explains.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(TS))]
#[serde(rename_all = "camelCase")]
pub enum LineOriginsUnavailable {
    /// The replay engine is not in this build yet (what WHY-64 returns).
    EngineNotReady,
    /// The file is binary (no lines to attribute).
    NotText,
    /// The file is larger than the engine handles.
    TooLarge,
}

/// Result of `get_line_origins`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(TS))]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum LineOriginsResult {
    Ok { origins: FileLineOrigins },
    Unavailable { reason: LineOriginsUnavailable },
}
