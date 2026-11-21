//! The frozen ground-truth format (WHY-64) and its loader.
//!
//! A scenario is written with READABLE line text; the loader hashes it with
//! the one [`LineHasher`] policy, so the engine only ever sees hashes (decision
//! 29). The format is documented for scenario authors in
//! `scenarios/README.md`; this file is its executable definition.
//!
//! Decision 33: this code is the ORACLE. It lives in the integration-test
//! crate, so neither production code nor the engine (WHY-22) can reach it, and
//! the engine must not copy its applier: if the oracle and the engine shared
//! code, "zero false attributions on the fixtures" would only prove the code
//! agrees with itself.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use serde::Deserialize;
use why_lib::contracts::WorktreeId;
use why_lib::events::{EventKey, SessionKey, canonical_path};
use why_lib::provenance::{
    EditHunk, EditKind, EditOp, EditRecord, EditSource, FileFingerprint, FileSnapshot,
    HunkLineHash, HunkLineKind, LineHash, LineHasher, LineOrigin, Observed, ReplayInput,
    ReplayOutput, UnknownReason, UnobservedReason,
};
use why_wire::Harness;

/// The only format version this loader reads.
pub const FORMAT: u32 = 1;

// ---- The JSON shape (serde) -------------------------------------------------
//
// `deny_unknown_fields` everywhere: a misspelled field is an error, never a
// silently ignored one.

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawScenario {
    format: u32,
    name: String,
    description: String,
    stage: Stage,
    harness: HarnessName,
    file: String,
    // `nullable`: the key is REQUIRED (`null` = no such file). Plain
    // `Option` would also accept a forgotten key as "no file".
    #[serde(deserialize_with = "nullable")]
    base: Option<FileText>,
    ops: Vec<RawOp>,
    #[serde(rename = "final", deserialize_with = "nullable")]
    final_file: Option<FileText>,
    expected: Vec<Expected>,
}

/// A required key whose value may be `null`. With `deserialize_with`, serde no
/// longer treats a missing key as `None`: it reports "missing field".
fn nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<FileText>, D::Error> {
    Option::<FileText>::deserialize(d)
}

/// Which part of the engine must pass the scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    /// `final` is exactly what a faithful replay of the ops produces (WHY-22).
    Replay,
    /// `final` differs from the replay (a human edit, a shell write, a
    /// formatter after the last op); the answer needs alignment (WHY-23).
    Align,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum HarnessName {
    ClaudeCode,
    Codex,
}

/// A whole file as text: one string per line, no terminators.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileText {
    pub lines: Vec<String>,
    pub final_newline: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawOp {
    id: String,
    session: String,
    turn: String,
    kind: OpKind,
    renamed_from: Option<String>,
    #[serde(default = "exact")]
    observed: ObservedSpec,
    #[serde(default)]
    hunks: Vec<RawHunk>,
    before: BeforeSpec,
    after: Keyword,
    /// Truth only: the file's final-newline flag after this op. Default:
    /// unchanged (`true` for a created file).
    final_newline: Option<bool>,
    /// Truth only, required for an unobserved op: what the file really
    /// became. Never passed to the engine.
    truth_after: Option<FileText>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OpKind {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ObservedSpec {
    Exact,
    Unobserved(UnobservedName),
}

fn exact() -> ObservedSpec {
    ObservedSpec::Exact
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum UnobservedName {
    PayloadReduced,
    Unparsed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Keyword {
    Auto,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum BeforeSpec {
    Keyword(Keyword),
    /// What the harness really saw: may deliberately differ from the replayed
    /// state ("something changed the file between two edits").
    Explicit(FileText),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawHunk {
    /// What the harness gives (Claude): unified-diff `oldStart`.
    old_start: Option<u32>,
    /// Truth only (Codex): where the hunk really applied, same meaning as
    /// `oldStart`. Never passed to the engine.
    applied_at: Option<u32>,
    /// Codex `@@ <text>` header.
    header: Option<String>,
    /// `" text"` context, `"-text"` removed, `"+text"` added.
    lines: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Expected {
    line: u32,
    origin: OriginSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OriginSpec {
    Base,
    Op(String),
    Unknown(ReasonName),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ReasonName {
    NoCapture,
    PayloadReduced,
    Unparsed,
    ReplayMismatch,
    AmbiguousMatch,
    ConcurrentWriter,
    NotReplayed,
}

impl ReasonName {
    fn to_reason(self) -> UnknownReason {
        // Exhaustive: a new engine reason must be added here to be usable.
        match self {
            ReasonName::NoCapture => UnknownReason::NoCapture,
            ReasonName::PayloadReduced => UnknownReason::PayloadReduced,
            ReasonName::Unparsed => UnknownReason::Unparsed,
            ReasonName::ReplayMismatch => UnknownReason::ReplayMismatch,
            ReasonName::AmbiguousMatch => UnknownReason::AmbiguousMatch,
            ReasonName::ConcurrentWriter => UnknownReason::ConcurrentWriter,
            ReasonName::NotReplayed => UnknownReason::NotReplayed,
        }
    }
}

// ---- Errors -------------------------------------------------------------------

#[derive(Debug)]
pub struct ScenarioError(pub String);

impl fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScenarioError {}

fn err<T>(msg: impl Into<String>) -> Result<T, ScenarioError> {
    Err(ScenarioError(msg.into()))
}

/// The engine's answer differed from the ground truth.
#[derive(Debug)]
pub struct Mismatch(pub String);

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ---- The validated scenario -------------------------------------------------

/// A loaded, validated scenario.
#[derive(Debug)]
pub struct Scenario {
    raw: RawScenario,
    /// The `EditOp` handed to the engine for each op (hashes only).
    ops: Vec<EditOp>,
}

impl Scenario {
    /// Read, parse and validate one scenario file.
    pub fn load(path: &Path) -> Result<Self, ScenarioError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ScenarioError(format!("{}: {e}", path.display())))?;
        Self::from_json(&text).map_err(|e| ScenarioError(format!("{}: {e}", path.display())))
    }

    /// Parse and validate a scenario given as JSON text.
    pub fn from_json(text: &str) -> Result<Self, ScenarioError> {
        // The format number first, so a future format gets a clear message
        // instead of a confusing field error.
        #[derive(Deserialize)]
        struct Version {
            format: Option<u32>,
        }
        let version: Version =
            serde_json::from_str(text).map_err(|e| ScenarioError(format!("not JSON: {e}")))?;
        match version.format {
            Some(FORMAT) => {}
            Some(other) => {
                return err(format!(
                    "unsupported format {other} (this loader reads {FORMAT})"
                ));
            }
            None => return err("missing `format`"),
        }
        let raw: RawScenario =
            serde_json::from_str(text).map_err(|e| ScenarioError(format!("bad scenario: {e}")))?;
        let mut scenario = Scenario {
            raw,
            ops: Vec::new(),
        };
        scenario.ops = scenario.validate()?;
        Ok(scenario)
    }

    pub fn name(&self) -> &str {
        &self.raw.name
    }

    pub fn stage(&self) -> Stage {
        self.raw.stage
    }

    /// Check internal consistency by running the reference applier (the
    /// oracle) over the text, and build the ops the engine will see. A loader
    /// that accepts a malformed scenario is a bug: every rule here is an error.
    pub fn validate(&self) -> Result<Vec<EditOp>, ScenarioError> {
        let s = &self.raw;
        let hasher = LineHasher::default();
        if s.format != FORMAT {
            return err(format!("unsupported format {}", s.format));
        }
        if s.name.trim().is_empty() || s.description.trim().is_empty() {
            return err("`name` and `description` must not be empty");
        }
        if canonical_path(&s.file).as_deref() != Some(s.file.as_str()) {
            return err(format!(
                "`file` {:?} must be a canonical relative path",
                s.file
            ));
        }
        if s.ops.is_empty() {
            return err("a scenario needs at least one op");
        }
        for text in [&s.base, &s.final_file].into_iter().flatten() {
            check_text(text, "file")?;
        }

        let mut ids: HashMap<&str, ObservedSpec> = HashMap::new();
        let mut state: Option<FileText> = s.base.clone();
        let mut ops = Vec::with_capacity(s.ops.len());
        for op in &s.ops {
            if op.id.is_empty() || op.session.is_empty() || op.turn.is_empty() {
                return err("op `id`, `session` and `turn` must not be empty");
            }
            if ids.insert(op.id.as_str(), op.observed).is_some() {
                return err(format!("duplicate op id {:?}", op.id));
            }
            let ctx = |m: String| ScenarioError(format!("op {:?}: {m}", op.id));
            let (next, engine_op) =
                apply_op(s.harness, &s.file, op, &state, &hasher).map_err(|e| ctx(e.0))?;
            state = next;
            ops.push(engine_op);
        }

        match (s.stage, state == s.final_file) {
            (Stage::Replay, false) => {
                return err(format!(
                    "stage `replay`, but `final` is not what the ops produce; the ops give {:?}",
                    state.map(|t| t.lines)
                ));
            }
            (Stage::Align, true) => {
                return err("stage `align`, but `final` equals the replay: use stage `replay`");
            }
            _ => {}
        }

        let final_len = s.final_file.as_ref().map_or(0, |f| f.lines.len());
        if s.expected.len() != final_len {
            return err(format!(
                "`expected` has {} entries but `final` has {final_len} lines",
                s.expected.len()
            ));
        }
        for (i, e) in s.expected.iter().enumerate() {
            let want = i as u32 + 1;
            if e.line != want {
                return err(format!(
                    "`expected` must list lines 1..={final_len} in order; entry {i} is line {}",
                    e.line
                ));
            }
            match &e.origin {
                OriginSpec::Base => {}
                OriginSpec::Op(id) => match ids.get(id.as_str()) {
                    None => return err(format!("line {want}: no op {id:?}")),
                    Some(ObservedSpec::Unobserved(_)) => {
                        return err(format!(
                            "line {want}: op {id:?} is unobserved and cannot be an origin"
                        ));
                    }
                    Some(ObservedSpec::Exact) => {}
                },
                OriginSpec::Unknown(ReasonName::NotReplayed) => {
                    return err(format!(
                        "line {want}: `notReplayed` is the stub's answer, never ground truth"
                    ));
                }
                OriginSpec::Unknown(_) => {}
            }
        }
        Ok(ops)
    }

    /// What the engine receives, and the final file as line hashes. The
    /// truth-only fields (`appliedAt`, `truthAfter`, `finalNewline`) are not
    /// in it.
    pub fn inputs(&self) -> (ReplayInput, Vec<LineHash>) {
        let hasher = LineHasher::default();
        let harness = harness(self.raw.harness);
        let records = self
            .raw
            .ops
            .iter()
            .zip(&self.ops)
            .enumerate()
            .map(|(k, (raw, op))| EditRecord {
                source: EditSource {
                    session: SessionKey::new(harness, raw.session.clone()),
                    turn_id: Some(raw.turn.clone()),
                    agent_id: None,
                    tool_use_id: Some(raw.id.clone()),
                    event_key: event_key(&raw.id),
                },
                seq: k as u64 + 1,
                op_index: 0,
                at_ms: 1_000 * (k as u64 + 1),
                op: op.clone(),
            })
            .collect();
        let baseline = self.raw.base.as_ref().map(|b| snapshot(&hasher, b));
        let final_lines = self
            .raw
            .final_file
            .as_ref()
            .map_or_else(Vec::new, |f| hash_text(&hasher, &f.lines));
        (ReplayInput { baseline, records }, final_lines)
    }

    /// Compare the engine's answer with `expected`, line by line.
    pub fn check(&self, output: &ReplayOutput) -> Result<(), Mismatch> {
        let expected = &self.raw.expected;
        let lines = self
            .raw
            .final_file
            .as_ref()
            .map_or(&[][..], |f| &f.lines[..]);
        let final_hashes = self.inputs().1;
        let mut problems = Vec::new();
        let want_newline = self.raw.final_file.as_ref().map(|f| f.final_newline);
        if self.raw.stage == Stage::Replay && output.final_newline != want_newline {
            problems.push(format!(
                "  final newline: expected {want_newline:?}, got {:?}",
                output.final_newline
            ));
        }
        if output.lines.len() != expected.len() {
            problems.push(format!(
                "  the engine returned {} lines, the final file has {}",
                output.lines.len(),
                expected.len()
            ));
        }
        for (i, want) in expected.iter().enumerate() {
            let got = output.lines.get(i).map(|l| &l.origin);
            // A `replay` scenario's final file IS the replay: the engine's line
            // content must match too, not only its origin. (An `align`
            // scenario's replay differs from `final` by definition.)
            if self.raw.stage == Stage::Replay
                && let Some(line) = output.lines.get(i)
                && Some(&line.hash) != final_hashes.get(i)
            {
                problems.push(format!(
                    "  line {:>4}: the replayed line's content is not the final file's | {}",
                    want.line,
                    lines.get(i).map_or("", String::as_str),
                ));
            }
            let ok = match (&want.origin, got) {
                (OriginSpec::Base, Some(LineOrigin::Base)) => true,
                (OriginSpec::Op(id), Some(LineOrigin::Edit(src))) => src.event_key == event_key(id),
                (OriginSpec::Unknown(r), Some(LineOrigin::Unknown(g))) => r.to_reason() == *g,
                _ => false,
            };
            if !ok {
                problems.push(format!(
                    "  line {:>4}: expected {:<28} got {:<28} | {}",
                    want.line,
                    describe_spec(&want.origin),
                    got.map_or_else(|| "(missing)".to_owned(), describe_origin),
                    lines.get(i).map_or("", String::as_str),
                ));
            }
        }
        if problems.is_empty() {
            return Ok(());
        }
        let shown = problems.len().min(40);
        Err(Mismatch(format!(
            "scenario {:?} ({:?}): {} problem(s)\n{}{}",
            self.raw.name,
            self.raw.stage,
            problems.len(),
            problems[..shown].join("\n"),
            if shown < problems.len() {
                "\n  ..."
            } else {
                ""
            }
        )))
    }
}

fn event_key(op_id: &str) -> EventKey {
    EventKey(format!("truth:{op_id}"))
}

fn harness(h: HarnessName) -> Harness {
    match h {
        HarnessName::ClaudeCode => Harness::ClaudeCode,
        HarnessName::Codex => Harness::Codex,
    }
}

fn describe_spec(o: &OriginSpec) -> String {
    match o {
        OriginSpec::Base => "base".into(),
        OriginSpec::Op(id) => format!("op {id}"),
        OriginSpec::Unknown(r) => format!("unknown({r:?})"),
    }
}

fn describe_origin(o: &LineOrigin) -> String {
    match o {
        LineOrigin::Base => "base".into(),
        LineOrigin::Edit(src) => match src.event_key.0.strip_prefix("truth:") {
            Some(id) => format!("op {id}"),
            None => format!("edit {}", src.event_key.0),
        },
        LineOrigin::Unknown(r) => format!("unknown({r:?})"),
    }
}

/// A line in a scenario is content only: no terminator inside it, and no
/// trailing `\r` (on disk `a\r\n` IS the line `a` under the CRLF policy, so
/// writing `a\r` would describe a file that cannot exist).
fn check_text(text: &FileText, what: &str) -> Result<(), ScenarioError> {
    // `"\n"` is one empty line, so a file with no lines has no final newline.
    if text.lines.is_empty() && text.final_newline {
        return err(format!(
            "{what}: a file with no lines cannot end with a newline (\"\\n\" is one empty line)"
        ));
    }
    for (i, line) in text.lines.iter().enumerate() {
        if line.contains('\n') {
            return err(format!("{what} line {}: contains a newline", i + 1));
        }
        if line.ends_with('\r') {
            return err(format!("{what} line {}: ends with \\r", i + 1));
        }
    }
    Ok(())
}

fn hash_text(hasher: &LineHasher, lines: &[String]) -> Vec<LineHash> {
    lines
        .iter()
        .map(|l| hasher.hash_line(l.as_bytes()))
        .collect()
}

fn snapshot(hasher: &LineHasher, text: &FileText) -> FileSnapshot {
    FileSnapshot::of_lines(hasher, hash_text(hasher, &text.lines), text.final_newline)
}

fn fingerprint(hasher: &LineHasher, text: &FileText) -> FileFingerprint {
    hasher.fingerprint_of(&hash_text(hasher, &text.lines), text.final_newline)
}

/// A hunk line split into its kind and text.
fn parse_line(raw: &str) -> Result<(HunkLineKind, &str), ScenarioError> {
    let mut chars = raw.chars();
    let kind = match chars.next() {
        Some(' ') => HunkLineKind::Context,
        Some('-') => HunkLineKind::Removed,
        Some('+') => HunkLineKind::Added,
        _ => return err(format!("hunk line {raw:?} must start with ' ', '-' or '+'")),
    };
    Ok((kind, chars.as_str()))
}

/// The reference applier: apply one op to the TRUE file state and build the
/// hash-only op the engine sees. Returns the state after the op.
fn apply_op(
    harness_name: HarnessName,
    file: &str,
    op: &RawOp,
    running: &Option<FileText>,
    hasher: &LineHasher,
) -> Result<(Option<FileText>, EditOp), ScenarioError> {
    let codex = harness_name == HarnessName::Codex;
    if let Some(from) = &op.renamed_from
        && canonical_path(from).as_deref() != Some(from.as_str())
    {
        return err("`renamedFrom` must be a canonical relative path");
    }
    if codex && (op.before != BeforeSpec::Keyword(Keyword::None) || op.after != Keyword::None) {
        return err("Codex gives no before/after file: use \"none\" for both");
    }
    if let BeforeSpec::Explicit(text) = &op.before {
        check_text(text, "before")?;
    }

    // The state the harness really edited: an explicit `before` overrides the
    // replayed state (that is how a scenario says "something changed the file
    // between two edits").
    let pre: Option<FileText> = match &op.before {
        BeforeSpec::Explicit(text) => Some(text.clone()),
        BeforeSpec::Keyword(_) => running.clone(),
    };
    let before = match &op.before {
        BeforeSpec::Keyword(Keyword::None) => None,
        BeforeSpec::Keyword(Keyword::Auto) => running.as_ref().map(|t| snapshot(hasher, t)),
        BeforeSpec::Explicit(text) => Some(snapshot(hasher, text)),
    };

    let observed = match op.observed {
        ObservedSpec::Exact => Observed::Exact,
        ObservedSpec::Unobserved(UnobservedName::PayloadReduced) => {
            Observed::Unobserved(UnobservedReason::PayloadReduced)
        }
        ObservedSpec::Unobserved(UnobservedName::Unparsed) => {
            Observed::Unobserved(UnobservedReason::Unparsed)
        }
    };

    let (post, hunks) = match op.observed {
        ObservedSpec::Unobserved(_) => {
            if !op.hunks.is_empty()
                || op.before != BeforeSpec::Keyword(Keyword::None)
                || op.after != Keyword::None
            {
                return err("an unobserved op has no hunks and \"none\" before/after");
            }
            let Some(truth) = &op.truth_after else {
                return err("an unobserved op needs `truthAfter` (what the file really became)");
            };
            check_text(truth, "truthAfter")?;
            (Some(truth.clone()), Vec::new())
        }
        ObservedSpec::Exact => {
            if op.truth_after.is_some() {
                return err("`truthAfter` is only for unobserved ops");
            }
            apply_exact(harness_name, op, pre.as_ref(), hasher)?
        }
    };
    let after = match op.after {
        Keyword::None => None,
        Keyword::Auto => post.as_ref().map(|t| fingerprint(hasher, t)),
    };
    let kind = match op.kind {
        OpKind::Create => EditKind::Create,
        OpKind::Update => EditKind::Update,
        OpKind::Delete => EditKind::Delete,
    };
    let engine_op = EditOp {
        worktree_id: Some(WorktreeId("truth-worktree".into())),
        path: file.to_owned(),
        kind,
        renamed_from: op.renamed_from.clone(),
        before,
        after,
        hunks,
        observed,
    };
    Ok((post, engine_op))
}

fn apply_exact(
    harness_name: HarnessName,
    op: &RawOp,
    pre: Option<&FileText>,
    hasher: &LineHasher,
) -> Result<(Option<FileText>, Vec<EditHunk>), ScenarioError> {
    let codex = harness_name == HarnessName::Codex;
    // The hunks as the engine sees them: hashes, the harness's own position
    // (never `appliedAt`), the hashed header.
    let mut engine_hunks = Vec::with_capacity(op.hunks.len());
    let mut parsed = Vec::with_capacity(op.hunks.len());
    for (n, h) in op.hunks.iter().enumerate() {
        let ctx = |m: &str| ScenarioError(format!("hunk {}: {m}", n + 1));
        if h.header.is_some() && !codex {
            return Err(ctx("only Codex hunks have a `header`"));
        }
        let lines = h
            .lines
            .iter()
            .map(|l| parse_line(l))
            .collect::<Result<Vec<_>, _>>()?;
        for (_, text) in &lines {
            if text.contains('\n') || text.ends_with('\r') {
                return Err(ctx("a hunk line contains a newline or ends with \\r"));
            }
        }
        if lines.is_empty() {
            return Err(ctx("a hunk has at least one line"));
        }
        engine_hunks.push(EditHunk {
            old_start: h.old_start,
            header: h.header.as_ref().map(|t| hasher.hash_line(t.as_bytes())),
            lines: lines
                .iter()
                .map(|(kind, text)| HunkLineHash {
                    kind: *kind,
                    hash: hasher.hash_line(text.as_bytes()),
                })
                .collect(),
        });
        parsed.push((h, lines));
    }

    match op.kind {
        OpKind::Delete => {
            if pre.is_none() {
                return err("deletes a file that does not exist");
            }
            if !op.hunks.is_empty() {
                return err("a delete has no hunks");
            }
            Ok((None, engine_hunks))
        }
        OpKind::Create => {
            if pre.is_some() {
                return err("creates a file that already exists");
            }
            // A created EMPTY file (a `Write` of ""): no hunks, no lines, no
            // final newline. Not a delete (the file exists) and not one blank
            // line (that would be one `+` line).
            if parsed.is_empty() {
                if op.final_newline == Some(true) {
                    return err("an empty created file has no final newline");
                }
                let text = FileText {
                    lines: Vec::new(),
                    final_newline: false,
                };
                return Ok((Some(text), engine_hunks));
            }
            let [(h, lines)] = parsed.as_slice() else {
                return err("a create has at most one hunk, of added lines only");
            };
            if h.applied_at.is_some()
                || h.old_start.is_some()
                || h.header.is_some()
                || lines.iter().any(|(k, _)| *k != HunkLineKind::Added)
            {
                return err(
                    "a create's hunk has only added lines and no `oldStart`, `appliedAt` or `header`",
                );
            }
            let text = FileText {
                lines: lines.iter().map(|(_, t)| (*t).to_owned()).collect(),
                final_newline: op.final_newline.unwrap_or(true),
            };
            Ok((Some(text), engine_hunks))
        }
        OpKind::Update => {
            let Some(pre) = pre else {
                return err("updates a file that does not exist");
            };
            if parsed.is_empty() {
                return err("an update has at least one hunk");
            }
            let mut out: Vec<String> = Vec::with_capacity(pre.lines.len());
            let mut cursor = 0usize; // next unread line of `pre` (0-based)
            for (n, (h, lines)) in parsed.iter().enumerate() {
                let ctx = |m: String| ScenarioError(format!("hunk {}: {m}", n + 1));
                let position = match (codex, h.old_start, h.applied_at) {
                    (false, Some(p), None) => p,
                    (false, _, _) => {
                        return Err(ctx("a Claude hunk has `oldStart` and no `appliedAt`".into()));
                    }
                    (true, None, Some(p)) => p,
                    (true, _, _) => {
                        return Err(ctx(
                            "a Codex hunk has no `oldStart`; `appliedAt` is required".into(),
                        ));
                    }
                } as usize;
                let touches_old = lines.iter().any(|(k, _)| *k != HunkLineKind::Added);
                // Unified-diff meaning: the first context/removed line is line
                // `position`; a pure insertion goes AFTER line `position`.
                let start = if touches_old {
                    if position == 0 {
                        return Err(ctx("position 0 is only for a pure insertion".into()));
                    }
                    position - 1
                } else {
                    position
                };
                if start < cursor || start > pre.lines.len() {
                    return Err(ctx(format!(
                        "position {position} is out of order or past the end of the file"
                    )));
                }
                // A Codex header names a line ABOVE the hunk (the search for the
                // context starts after it); one that is not there is a typo.
                if let Some(header) = &h.header
                    && !pre.lines[..start].iter().any(|l| l == header)
                {
                    return Err(ctx(format!(
                        "header {header:?} is not a line above position {position}"
                    )));
                }
                out.extend_from_slice(&pre.lines[cursor..start]);
                let mut i = start;
                for (kind, text) in lines {
                    match kind {
                        HunkLineKind::Added => out.push((*text).to_owned()),
                        HunkLineKind::Context | HunkLineKind::Removed => {
                            match pre.lines.get(i) {
                                Some(have) if have == text => {}
                                have => {
                                    return Err(ctx(format!(
                                        "line {} of the file is {have:?}, the hunk says {text:?}",
                                        i + 1
                                    )));
                                }
                            }
                            if *kind == HunkLineKind::Context {
                                out.push((*text).to_owned());
                            }
                            i += 1;
                        }
                    }
                }
                cursor = i;
            }
            out.extend_from_slice(&pre.lines[cursor..]);
            let text = FileText {
                lines: out,
                final_newline: op.final_newline.unwrap_or(pre.final_newline),
            };
            Ok((Some(text), engine_hunks))
        }
    }
}
