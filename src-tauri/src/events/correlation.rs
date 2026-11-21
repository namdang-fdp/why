//! File ↔ turn correlation (WHY-16): which agent turns edited which files of a
//! worktree.
//!
//! This module is pure (no I/O, no clock). Both [`EventStore`](super::EventStore)
//! backends do the loading and call the SAME functions here for everything that
//! carries a rule: how a path is compared, how a Pre/Post pair becomes one
//! call, how a turn gets its index. The conformance tests in
//! `tests/correlation/` run both backends over the same cases.
//!
//! # What counts (decisions 19-28, Provenance Engine)
//!
//! Only **Observed** facts are indexed: an event of an *edit-class* tool call
//! (`Write`/`Edit`/`NotebookEdit`/`apply_patch`) that names a file. Reads and
//! shell commands never make a file "touched": a shell command's effect on the
//! tree is only known to git, so shell calls are reported separately, per turn
//! ([`ShellTurn`]), as a lead for the reviewer and never as an attribution.
//!
//! # Time is never evidence
//!
//! Nothing here compares a file's modification time with a turn's time. The
//! timestamps are only carried along so lists can be ordered and displayed.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use super::model::{EventKind, NormalizedEvent, SessionKey, ToolClass};
use super::timeline::TurnIndices;
use crate::contracts::{EvidenceLabel, ToolOutcome, WorktreeId};

/// The ONE spelling in which git paths and event paths are compared:
/// worktree-relative, `/`-separated, no empty or `.` components, `..` resolved
/// lexically. `None` when the path cannot be a place inside a worktree:
///
/// - empty, or containing a NUL byte;
/// - absolute. The ingest step already rewrote every edited file that lies in a
///   registered worktree to a path relative to ITS OWN root, so a file that is
///   still absolute lies in no registered worktree (or was recorded before the
///   worktree was registered). It is kept in the event
///   but **never matched by prefix guessing**: a `starts_with` between two
///   directories happily matches `/work/app` against `/work/app-other`;
/// - a path whose `..` climbs out of the root (`../x`).
///
/// Ingest already resolves aliases in parent directories while retaining the
/// final component, as named by Git. Compared byte for byte:
/// case-sensitive, as on Linux. On macOS (case-insensitive by default, and git
/// may report NFD file names) two spellings of one file would not match; that
/// is a known limit, not verified here.
pub fn canonical_path(raw: &str) -> Option<String> {
    if raw.is_empty() || raw.contains('\0') {
        return None;
    }
    // `starts_with('/')` as well as `is_absolute`: on Windows `/x` is not
    // absolute for `Path` (no drive), but is still not a worktree-relative path.
    if raw.starts_with('/') || Path::new(raw).is_absolute() {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            // Popping an empty stack means the path climbs above the root.
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The class of an OBSERVED tool start/finish, `None` for anything else.
///
/// "Observed only" (Provenance Engine) is enforced here, in one place: an event
/// whose evidence is anything but `Observed` (a `Declared` message, an
/// `Inferred` guess) is never a tool call we count. Today every adapter labels
/// its tool events `Observed` (they come straight from the harness), so this
/// filters nothing yet; it keeps the guarantee true if that ever changes. The
/// SQL queries in `sqlite.rs` and the backfill repeat it as `evidence = 'observed'`.
pub(crate) fn tool_event_class(ev: &NormalizedEvent) -> Option<ToolClass> {
    if ev.evidence != EvidenceLabel::Observed {
        return None;
    }
    match ev.kind {
        EventKind::ToolStarted | EventKind::ToolFinished { .. } => {
            ev.tool.as_ref().map(|t| t.class)
        }
        _ => None,
    }
}

/// The canonical form of every path in `files` that is a worktree-relative
/// path (see [`canonical_path`]), deduplicated and sorted. The one place this
/// is computed: the SQLite index writer, its migration backfill and the
/// call-event decoders all call it, so they cannot disagree.
pub fn canonical_paths(files: &[String]) -> Vec<String> {
    let unique: BTreeSet<String> = files.iter().filter_map(|f| canonical_path(f)).collect();
    unique.into_iter().collect()
}

/// Every (worktree, canonical path) an event is indexed under: only OBSERVED
/// events of edit-class tool calls. This is the single definition of "what goes
/// in the index" for new events; the migration's backfill applies the same
/// guards in SQL (`WHERE` clause) and the same [`canonical_paths`].
///
/// Where a file is indexed is decided by ingest (decision 32): each edited file
/// under the worktree that owns it, so one event can be indexed under several
/// worktrees. An event ingest did not look at (`edited_files` is `None`) falls
/// back to the v0.2 rule: all its files under the event's own worktree.
pub fn indexed_files(ev: &NormalizedEvent) -> Vec<(WorktreeId, String)> {
    if tool_event_class(ev) != Some(ToolClass::Edit) {
        return Vec::new();
    }
    let mut found: Vec<(WorktreeId, String)> = match &ev.edited_files {
        Some(files) => files
            .iter()
            .filter_map(|f| Some((f.worktree_id.clone(), canonical_path(&f.path)?)))
            .collect(),
        None => match &ev.worktree_id {
            Some(wt) => canonical_paths(&ev.files)
                .into_iter()
                .map(|path| (wt.clone(), path))
                .collect(),
            None => Vec::new(),
        },
    };
    // Sorted and deduplicated, so every caller sees one order.
    found.sort_by(|a, b| (&a.0.0, &a.1).cmp(&(&b.0.0, &b.1)));
    found.dedup();
    found
}

/// The canonical paths of `ev` that live in worktree `wt` (see
/// [`indexed_files`]), sorted.
pub fn files_in(ev: &NormalizedEvent, wt: &WorktreeId) -> Vec<String> {
    indexed_files(ev)
        .into_iter()
        .filter(|(owner, _)| owner == wt)
        .map(|(_, path)| path)
        .collect()
}

/// One start or finish event of a tool call, reduced to what correlation needs.
/// A projection (rather than the full event) so the SQLite store can fill it
/// from a few columns instead of decoding prompts and command texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallEvent {
    pub session: SessionKey,
    /// `""` when the event has no turn (matches the timeline's rule).
    pub turn_id: String,
    pub agent_id: Option<String>,
    pub use_id: Option<String>,
    /// `None` for a start (`PreToolUse`), the outcome for a finish.
    pub finished: Option<ToolOutcome>,
    pub at_ms: u64,
    /// A finish can name targets even if none are inside this worktree.
    pub reported_paths: bool,
    /// Canonical paths (see [`canonical_path`]); non-canonical ones are dropped.
    pub paths: Vec<String>,
}

impl CallEvent {
    /// `None` unless `ev` is a tool start or finish. `paths` are the files of
    /// worktree `wt` only (see [`files_in`]): a multi-file call that touched
    /// two worktrees lists in each one just its own files.
    pub fn from_event(ev: &NormalizedEvent, wt: &WorktreeId) -> Option<Self> {
        tool_event_class(ev)?;
        Some(Self {
            session: ev.session.clone(),
            turn_id: ev.turn_id.clone().unwrap_or_default(),
            agent_id: ev.agent_id.clone(),
            use_id: ev.tool.as_ref().and_then(|t| t.use_id.clone()),
            finished: match ev.kind {
                EventKind::ToolFinished { outcome } => Some(outcome),
                _ => None,
            },
            at_ms: ev.at_ms,
            reported_paths: !ev.files.is_empty(),
            paths: files_in(ev, wt),
        })
    }
}

/// One tool call: its start and finish merged, like the timeline's entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub session: SessionKey,
    pub turn_id: String,
    pub agent_id: Option<String>,
    pub use_id: Option<String>,
    /// The observed finish's outcome, if a finish was delivered.
    pub outcome: Option<ToolOutcome>,
    /// When the call started, or when it finished if no start was seen.
    pub at_ms: u64,
    /// The successful finish's targets when present, otherwise the reported paths.
    pub paths: BTreeSet<String>,
}

/// Merge start/finish events (in ARRIVAL order) into calls, by
/// `(session, agent_id, tool_use_id)`: the same key the timeline pairs on. An
/// event with no `tool_use_id` cannot be paired and is a call of its own.
/// Calls come back in the order their first event arrived.
pub fn fold_calls(events: impl IntoIterator<Item = CallEvent>) -> Vec<Call> {
    // The start fixes the turn and the time; the (latest) finish fixes the outcome.
    struct Acc {
        session: SessionKey,
        agent_id: Option<String>,
        use_id: Option<String>,
        start: Option<(u64, String)>,
        finish: Option<(u64, String, ToolOutcome)>,
        executed_paths: Option<Vec<String>>,
        paths: BTreeSet<String>,
    }
    type Key = (SessionKey, Option<String>, String);

    let mut accs: Vec<Acc> = Vec::new();
    let mut by_id: HashMap<Key, usize> = HashMap::new();
    for ev in events {
        let slot = match &ev.use_id {
            Some(id) => {
                let key = (ev.session.clone(), ev.agent_id.clone(), id.clone());
                *by_id.entry(key).or_insert_with(|| {
                    accs.push(Acc::new(&ev));
                    accs.len() - 1
                })
            }
            None => {
                accs.push(Acc::new(&ev));
                accs.len() - 1
            }
        };
        let acc = &mut accs[slot];
        match ev.finished {
            None => {
                // Keep the earliest start if one is delivered twice.
                if acc.start.as_ref().is_none_or(|(at, _)| ev.at_ms < *at) {
                    acc.start = Some((ev.at_ms, ev.turn_id.clone()));
                }
            }
            Some(outcome) => {
                // Keep the latest finish; on equal times the later arrival wins.
                if acc.finish.as_ref().is_none_or(|(at, _, _)| ev.at_ms >= *at) {
                    acc.finish = Some((ev.at_ms, ev.turn_id.clone(), outcome));
                    acc.executed_paths =
                        (outcome == ToolOutcome::Ok && ev.reported_paths).then(|| ev.paths.clone());
                }
            }
        }
        acc.paths.extend(ev.paths);
    }

    impl Acc {
        fn new(ev: &CallEvent) -> Self {
            Self {
                session: ev.session.clone(),
                agent_id: ev.agent_id.clone(),
                use_id: ev.use_id.clone(),
                start: None,
                finish: None,
                executed_paths: None,
                paths: BTreeSet::new(),
            }
        }
    }

    accs.into_iter()
        .filter_map(|acc| {
            // The turn and time of a call are where it STARTED (the timeline's
            // rule); a call whose start was lost falls back to its finish.
            let (at_ms, turn_id) = match (&acc.start, &acc.finish) {
                (Some((at, turn)), _) => (*at, turn.clone()),
                (None, Some((at, turn, _))) => (*at, turn.clone()),
                (None, None) => return None, // unreachable: every acc got an event
            };
            Some(Call {
                session: acc.session,
                turn_id,
                agent_id: acc.agent_id,
                use_id: acc.use_id,
                outcome: acc.finish.map(|(_, _, o)| o),
                at_ms,
                paths: acc
                    .executed_paths
                    .map_or(acc.paths, |paths| paths.into_iter().collect()),
            })
        })
        .collect()
}

/// An edit call that named at least one of the queried paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditTouch {
    pub session: SessionKey,
    pub turn_id: String,
    /// The turn's index in `get_session_timeline`.
    pub turn_index: u32,
    pub agent_id: Option<String>,
    pub tool_use_id: Option<String>,
    pub outcome: ToolOutcome,
    pub at_ms: u64,
    /// The QUERIED paths this call named, sorted (never the call's other
    /// files: a multi-file `apply_patch` does not leak unrelated paths).
    pub paths: Vec<String>,
}

/// A turn that ran shell commands in a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellTurn {
    pub session: SessionKey,
    pub turn_id: String,
    pub turn_index: u32,
    /// Distinct shell calls in this turn (shell and test runs).
    pub command_count: u32,
    /// When the turn's first shell call started.
    pub first_at_ms: u64,
}

/// The sessions the calls belong to, once each (to compute their turn indices).
pub fn distinct_sessions(calls: &[Call]) -> Vec<SessionKey> {
    let set: HashSet<&SessionKey> = calls.iter().map(|c| &c.session).collect();
    let mut out: Vec<SessionKey> = set.into_iter().cloned().collect();
    out.sort();
    out
}

/// `calls` → [`EditTouch`]es for those that name a queried path, newest first.
/// `indices` contains each session's merged timeline turns.
pub(crate) fn build_edit_touches(
    calls: Vec<Call>,
    wanted: &HashSet<&str>,
    indices: &HashMap<SessionKey, TurnIndices>,
) -> Vec<EditTouch> {
    let mut out: Vec<EditTouch> = calls
        .into_iter()
        .filter_map(|c| {
            let paths: Vec<String> = c
                .paths
                .iter()
                .filter(|p| wanted.contains(p.as_str()))
                .cloned()
                .collect();
            if paths.is_empty() {
                return None;
            }
            let (turn_id, turn_index) = locate_turn(indices, &c)?;
            let turn_id = turn_id.to_owned();
            Some(EditTouch {
                session: c.session,
                turn_id,
                turn_index,
                agent_id: c.agent_id,
                tool_use_id: c.use_id,
                outcome: c.outcome.unwrap_or(ToolOutcome::Unknown),
                at_ms: c.at_ms,
                paths,
            })
        })
        .collect();
    // Newest first. The time ONLY orders the list; the other keys make the
    // order total, so both stores and every run agree.
    out.sort_by(|a, b| {
        b.at_ms
            .cmp(&a.at_ms)
            .then_with(|| a.session.cmp(&b.session))
            .then_with(|| a.turn_id.cmp(&b.turn_id))
            .then_with(|| a.tool_use_id.cmp(&b.tool_use_id))
            .then_with(|| a.agent_id.cmp(&b.agent_id))
    });
    out
}

/// Shell calls → one [`ShellTurn`] per `(session, turn)`, newest first.
pub(crate) fn build_shell_turns(
    calls: Vec<Call>,
    indices: &HashMap<SessionKey, TurnIndices>,
) -> Vec<ShellTurn> {
    let mut grouped: HashMap<(SessionKey, String, u32), (u32, u64)> = HashMap::new();
    // A start is an unconfirmed attempt. The timeline still shows it as Unknown.
    for c in calls.into_iter().filter(|c| c.outcome.is_some()) {
        let Some((turn_id, turn_index)) = locate_turn(indices, &c) else {
            continue;
        };
        let turn_id = turn_id.to_owned();
        let slot = grouped
            .entry((c.session, turn_id, turn_index))
            .or_insert((0, c.at_ms));
        slot.0 = slot.0.saturating_add(1);
        slot.1 = slot.1.min(c.at_ms);
    }
    let mut out: Vec<ShellTurn> = grouped
        .into_iter()
        .map(
            |((session, turn_id, turn_index), (command_count, first_at_ms))| ShellTurn {
                session,
                turn_id,
                turn_index,
                command_count,
                first_at_ms,
            },
        )
        .collect();
    out.sort_by(|a, b| {
        b.first_at_ms
            .cmp(&a.first_at_ms)
            .then_with(|| a.session.cmp(&b.session))
            .then_with(|| a.turn_id.cmp(&b.turn_id))
    });
    out
}

/// The timeline can pair halves even if attribution only sees one half in this
/// worktree. Use the timeline's merged entry as the link target.
fn locate_turn<'a>(
    indices: &'a HashMap<SessionKey, TurnIndices>,
    call: &Call,
) -> Option<(&'a str, u32)> {
    indices
        .get(&call.session)?
        .locate(&call.turn_id, &call.agent_id, call.use_id.as_deref())
}

#[cfg(test)]
mod tests {
    use why_wire::Harness;

    use super::*;

    fn s(id: &str) -> SessionKey {
        SessionKey::new(Harness::ClaudeCode, id)
    }

    fn ce(
        use_id: Option<&str>,
        finished: Option<ToolOutcome>,
        at: u64,
        turn: &str,
        paths: &[&str],
    ) -> CallEvent {
        CallEvent {
            session: s("s1"),
            turn_id: turn.into(),
            agent_id: None,
            use_id: use_id.map(str::to_owned),
            finished,
            at_ms: at,
            reported_paths: !paths.is_empty(),
            paths: paths.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    #[test]
    fn canonical_paths_are_one_spelling() {
        for (raw, want) in [
            ("a.rs", Some("a.rs")),
            ("./a.rs", Some("a.rs")),
            ("src//a.rs", Some("src/a.rs")),
            ("src/./a.rs", Some("src/a.rs")),
            ("./a/../b.rs", Some("b.rs")),
            ("a/b/../../c.rs", Some("c.rs")),
            ("src/", Some("src")),
            (
                "dir with spaces/né 世界.rs",
                Some("dir with spaces/né 世界.rs"),
            ),
            // Not inside the worktree, or not a path at all.
            ("", None),
            (".", None),
            ("a/..", None),
            ("../x", None),
            ("a/../../x", None),
            ("/abs/a.rs", None),
            ("/", None),
            ("a\0b", None),
        ] {
            assert_eq!(canonical_path(raw).as_deref(), want, "{raw:?}");
        }
        // Exact comparison: case matters.
        assert_ne!(canonical_path("A.rs"), canonical_path("a.rs"));
    }

    #[test]
    fn a_start_and_finish_are_one_call_and_a_lone_start_is_unknown() {
        let calls = fold_calls([
            ce(Some("t1"), None, 10, "p1", &["a.rs"]),
            ce(Some("t1"), Some(ToolOutcome::Ok), 12, "p1", &["a.rs"]),
            ce(Some("t2"), None, 20, "p1", &["b.rs"]),
        ]);
        assert_eq!(calls.len(), 2);
        assert_eq!(
            (calls[0].outcome, calls[0].at_ms),
            (Some(ToolOutcome::Ok), 10)
        );
        assert_eq!(calls[1].outcome, None);
    }

    #[test]
    fn a_finish_alone_keeps_its_outcome_and_time() {
        // Claude's PostToolBatch rejection has no start.
        let calls = fold_calls([ce(
            Some("t1"),
            Some(ToolOutcome::Failed),
            7,
            "p1",
            &["a.rs"],
        )]);
        assert_eq!(calls[0].outcome, Some(ToolOutcome::Failed));
        assert_eq!(calls[0].at_ms, 7);
    }

    #[test]
    fn calls_pair_per_agent_and_events_without_an_id_never_pair() {
        let mut sub = ce(Some("t1"), Some(ToolOutcome::Ok), 3, "p1", &["a.rs"]);
        sub.agent_id = Some("agent-9".into());
        let calls = fold_calls([
            ce(Some("t1"), None, 1, "p1", &["a.rs"]),
            sub,
            ce(None, None, 4, "p1", &["a.rs"]),
            ce(None, Some(ToolOutcome::Ok), 5, "p1", &["a.rs"]),
        ]);
        // main t1 (start only), subagent t1 (finish only), and two id-less calls.
        assert_eq!(calls.len(), 4, "{calls:?}");
    }

    #[test]
    fn a_redelivered_start_keeps_the_earliest_time() {
        let calls = fold_calls([
            ce(Some("t1"), None, 9, "p1", &["a.rs"]),
            ce(Some("t1"), None, 5, "p1", &["a.rs"]),
        ]);
        assert_eq!((calls.len(), calls[0].at_ms), (1, 5));
    }

    fn indices() -> HashMap<SessionKey, TurnIndices> {
        use super::super::timeline::{TimelinePosition, turn_indices};

        let positions = ["p1", "p2"].map(|turn_id| TimelinePosition {
            turn_id,
            at_ms: 0,
            agent_id: None,
            use_id: None,
            is_start: false,
        });
        HashMap::from([(s("s1"), turn_indices(&positions))])
    }

    #[test]
    fn only_queried_paths_are_reported_for_a_multi_file_call() {
        let calls = fold_calls([ce(
            Some("t1"),
            Some(ToolOutcome::Ok),
            1,
            "p1",
            &["a.rs", "secret/other.rs"],
        )]);
        let wanted: HashSet<&str> = HashSet::from(["a.rs"]);
        let touches = build_edit_touches(calls, &wanted, &indices());
        assert_eq!(touches[0].paths, ["a.rs"]);
    }

    #[test]
    fn shell_calls_are_counted_per_turn() {
        let calls = fold_calls([
            ce(Some("t1"), None, 5, "p1", &[]),
            ce(Some("t1"), Some(ToolOutcome::Ok), 6, "p1", &[]),
            ce(Some("t2"), None, 7, "p1", &[]),
            ce(Some("t3"), None, 9, "p2", &[]),
        ]);
        let turns = build_shell_turns(calls, &indices());
        assert_eq!(
            turns
                .iter()
                .map(|t| (t.turn_id.as_str(), t.turn_index, t.command_count))
                .collect::<Vec<_>>(),
            [("p1", 0, 1)]
        );
    }
}
