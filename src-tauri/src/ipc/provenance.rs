//! `get_line_origins`: which captured edit wrote each line of a file (v0.3).
//!
//! The pipeline for one `(worktree, path)`, all read-only:
//!
//! ```text
//! store: edit_records(path [+ old path of a rename])   ─┐
//! git:   the file at the comparison base (blob by id)   ├─▶ replay_report ─▶ align ─▶ runs
//! disk:  the file in the working tree (guarded read)    ─┘   (engine)       (real file)
//! ```
//!
//! # Three reads, three moments
//!
//! The store, the base blob and the working-tree file are read one after the
//! other, while an agent may be editing. The order is chosen so that every
//! race degrades to `Unknown`, never to a wrong attribution:
//!
//! 1. the store first, 2. the base blob (immutable: addressed by object id),
//! 3. the working-tree file LAST.
//!
//! - An edit that lands after step 1 is on disk at step 3 but not in the
//!   records: its lines are "something uncaptured changed the file" and
//!   alignment makes them `Unknown(noCapture)`.
//! - A record that is newer than the file is treated the same way: the
//!   replayed line is not in the real file, so nothing is attributed to it.
//!
//! Alignment only ever copies an origin from a line it can PROVE is the same
//! line (see `provenance::align`), so a torn read costs precision, not truth.
//!
//! # Commits between edits
//!
//! The base is the merge-base with the base branch (or HEAD when the worktree
//! has no base), exactly the vs-base view of `get_file_diff`. A commit on the
//! feature branch does not move the merge-base, so the replay is unaffected and
//! earlier edits stay attributed. When there is no base branch the comparison
//! is against HEAD, and a commit DOES move it: the baseline then already holds
//! the first edit's lines. Nothing here uses time to decide what to replay.
//! The replay starts from the baseline, each op's `before` snapshot resyncs it,
//! and alignment decides: a line whose origin cannot be established becomes
//! `Unknown`, never a wrong op.

use std::collections::HashMap;
use std::io;
use std::path::Path;

use tauri::State;

use crate::contracts::{
    EvidenceLabel, FileLineOrigins, Harness, HarnessLines, IpcError, LineCoverage, LineOriginKind,
    LineOriginsResult, LineOriginsUnavailable, LineRun, LineUnknownReason, ToolOutcome, TurnRef,
    WorktreeId,
};
use crate::events::timeline::{harness_to_contract, rfc3339_from_ms};
use crate::events::{EditTouch, EventStore, canonical_path};
use crate::git::diff::{Side, commit_spec, oid_spec, read_blob_side, read_worktree_side};
use crate::git::{self, ChangeKind, ChangeSet, OldSide, Target};
use crate::ipc::changes::{Resolved, resolve_worktree};
use crate::ipc::security;
use crate::provenance::{
    EditKind, EditRecord, EditSource, FileSnapshot, LineHasher, LineOrigin, ReplayInput, align,
    has_final_newline, replay_report,
};
use crate::state::AppState;

#[tauri::command]
pub async fn get_line_origins(
    state: State<'_, AppState>,
    worktree_id: WorktreeId,
    path: String,
) -> Result<LineOriginsResult, IpcError> {
    get_line_origins_impl(&state, &worktree_id, &path).await
}

pub async fn get_line_origins_impl(
    state: &AppState,
    worktree_id: &WorktreeId,
    path: &str,
) -> Result<LineOriginsResult, IpcError> {
    // The path comes from the webview. Two gates: the shared request check
    // refuses hostile input (absolute, `..`, NUL, empty; also `./x`, exactly
    // like `get_file_diff`, since the UI sends the paths `get_changes`
    // listed); `canonical_path` then gives the one spelling the edit index is
    // keyed by (`a//b.rs` and `a/b.rs` are the same file). The path is used
    // only as a key and as a worktree-relative name for guarded reads below
    // (`read_worktree_side` canonicalizes the parent and never follows a
    // final symlink); it is never joined to a path we open unchecked.
    git::diff::check_request_path(path).map_err(invalid)?;
    let canonical = canonical_path(path)
        .ok_or_else(|| invalid(format!("{path:?} is not a path inside the worktree")))?;

    // An unknown worktree is `NotFound`, like every other command.
    let wt = resolve_worktree(state, worktree_id).await?;
    if !tokio::fs::try_exists(&wt.entry.path)
        .await
        .map_err(io_error)?
    {
        return Err(IpcError::NotFound);
    }
    let canonical_root = tokio::fs::canonicalize(&wt.entry.path)
        .await
        .map_err(io_error)?;

    // What git says about this file: where the base is, and whether the file
    // was renamed (then the base content and the edits live under the old
    // name). One guarded git read; the filter guard may refuse here, and then
    // NOTHING of the working tree is read.
    let git_view = git_view_of(state, &wt, &canonical).await?;

    // Moment 1: the captured edits (own worktree, decision 32).
    let records = records_for(&*state.events, worktree_id, &canonical, &git_view)?;

    // Moment 2: the base content, by object id.
    let base = read_base(state, &wt, &git_view, &canonical).await?;

    // Moment 3: the file as it is on disk right now.
    let rel = Path::new(&canonical);
    let real = read_worktree_side(&canonical_root, rel)
        .await
        .map_err(io_error)?;

    // Git's own verdict counts too: a UTF-8 file marked `-diff` in
    // `.gitattributes` is binary to git, and its diff shows no text either.
    if git_view.change.as_ref().is_some_and(|c| c.binary) {
        return Ok(unavailable(LineOriginsUnavailable::NotText));
    }
    let hasher = LineHasher::default();
    let (baseline, real_bytes) = match (base, real) {
        // Not lines: refuse before comparing anything.
        (Side::Binary | Side::NotAFile, _) | (_, Side::Binary | Side::NotAFile) => {
            return Ok(unavailable(LineOriginsUnavailable::NotText));
        }
        (Side::TooLarge, _) | (_, Side::TooLarge) => {
            return Ok(unavailable(LineOriginsUnavailable::TooLarge));
        }
        (base, real) => (
            match base {
                Side::Text(text) => Some(FileSnapshot::of_text(&hasher, text.as_bytes())),
                _ => None, // absent at the base: not the same as an empty file
            },
            match real {
                Side::Text(text) => text.into_bytes(),
                _ => Vec::new(), // deleted (or never there): no lines
            },
        ),
    };

    let touches = turns_for(&*state.events, worktree_id, &canonical, &git_view)?;
    let origins = compute(baseline, records, &real_bytes, &hasher);
    Ok(LineOriginsResult::Ok {
        origins: build_origins(worktree_id, &canonical, &origins, &touches),
    })
}

/// The pure middle: records + base + real file in, an origin per real line out.
/// Split from the IO so the property and timing tests can call it directly.
pub(crate) fn compute(
    baseline: Option<FileSnapshot>,
    records: Vec<EditRecord>,
    real_bytes: &[u8],
    hasher: &LineHasher,
) -> Vec<LineOrigin> {
    let report = replay_report(&ReplayInput { baseline, records });
    let real = hasher.hash_lines(real_bytes);
    align(&report, &real, has_final_newline(real_bytes)).origins
}

// ---------------------------------------------------------------------------
// What git says about the file
// ---------------------------------------------------------------------------

/// The facts the later reads need, taken from one guarded `read_changes`.
struct GitView {
    set: ChangeSet,
    /// The file's own entry in the change list, if it changed.
    change: Option<git::Change>,
}

impl GitView {
    /// The path the file had at the base (the old name of a rename).
    fn old_name(&self) -> Option<&Path> {
        let c = self.change.as_ref()?;
        // A COPY keeps its source: the source's edits are not this file's.
        (c.kind == ChangeKind::Renamed)
            .then_some(c.old_path.as_deref())
            .flatten()
    }
}

async fn git_view_of(
    state: &AppState,
    wt: &Resolved,
    canonical: &str,
) -> Result<GitView, IpcError> {
    // The same base the vs-base view of `get_file_diff` compares against:
    // the stored override, else the detected base's merge-base; with no base
    // at all, HEAD (the "uncommitted" comparison).
    let stored = state.repos.base_override(&wt.id)?;
    let base = git::detect_base(&state.git, &wt.entry.path, stored.as_deref()).await?;
    let old = match base {
        Some(b) => OldSide::Commit(b.merge_base),
        None => OldSide::Head,
    };
    // The filter guard, asked last, right before the read it protects.
    let permit = security::permit_for(state, &wt.repo.id, &wt.entry.path).await?;
    let set = git::read_changes(
        &state.git,
        &permit,
        &old,
        Target::WorkingTree {
            include_untracked: true,
        },
    )
    .await?;
    let change = set
        .changes
        .iter()
        .find(|c| git::oid::path_bytes(&c.path) == canonical.as_bytes())
        .cloned();
    Ok(GitView { set, change })
}

/// The file at the comparison base, read by blob id (never by parsing a path
/// as revision syntax), through the same size and binary limits as the diff.
async fn read_base(
    state: &AppState,
    wt: &Resolved,
    view: &GitView,
    canonical: &str,
) -> Result<Side, IpcError> {
    let Some(change) = &view.change else {
        // Unchanged (or a file that exists nowhere): look it up at the old
        // revision; "no such path" there simply means "no baseline".
        return read_by_commit(state, wt, view, Path::new(canonical)).await;
    };
    if change.is_gitlink() {
        return Ok(Side::NotAFile);
    }
    match change.kind {
        ChangeKind::Added | ChangeKind::Untracked => Ok(Side::Absent),
        _ => match change.old_oid.as_deref().and_then(oid_spec) {
            Some(spec) => read_blob_side(&state.git, &wt.entry.path, spec, false)
                .await
                .map_err(blob_error),
            // A conflicted file has no blob id: by commit and path.
            None => {
                let name = change
                    .old_path
                    .clone()
                    .unwrap_or_else(|| change.path.clone());
                read_by_commit(state, wt, view, &name).await
            }
        },
    }
}

async fn read_by_commit(
    state: &AppState,
    wt: &Resolved,
    view: &GitView,
    name: &Path,
) -> Result<Side, IpcError> {
    read_blob_side(
        &state.git,
        &wt.entry.path,
        commit_spec(&view.set.old_rev, name),
        true,
    )
    .await
    .map_err(blob_error)
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// The file's records, plus its old name's for a rename, in `(seq, op_index)`
/// order (the engine sorts again; this keeps the input honest).
fn records_for(
    events: &dyn EventStore,
    wt: &WorktreeId,
    canonical: &str,
    view: &GitView,
) -> Result<Vec<EditRecord>, IpcError> {
    let mut own = events.edit_records(wt, canonical)?;
    own.sort_by_key(|r| (r.seq, r.op_index));
    let Some(old_name) = names_of(canonical, view).into_iter().nth(1) else {
        return Ok(own);
    };
    let mut old = events.edit_records(wt, &old_name)?;
    old.sort_by_key(|r| (r.seq, r.op_index));
    // The old name's history ends where the file left it. A captured rename
    // (an own record naming the old path in `renamed_from`) is the exact end;
    // a `Delete` or a `Create` at the old name means the path was emptied and
    // reused (git only reports a rename of a file that existed at the base, so
    // a create there can only be a new file), and what follows is ANOTHER
    // file's history that must not be credited to this one. Without a
    // provable end we keep only what we can prove belongs to the file.
    let captured_rename = own
        .iter()
        .find(|r| {
            r.op.renamed_from
                .as_deref()
                .and_then(canonical_path)
                .as_deref()
                == Some(&old_name)
        })
        .map(|r| (r.seq, r.op_index));
    let mut kept = Vec::new();
    for rec in old {
        let reused = matches!(rec.op.kind, EditKind::Delete | EditKind::Create);
        let after_rename = captured_rename.is_some_and(|end| (rec.seq, rec.op_index) >= end);
        if reused || after_rename {
            break;
        }
        kept.push(rec);
    }
    kept.extend(own);
    kept.sort_by_key(|r| (r.seq, r.op_index));
    Ok(kept)
}

/// Every name the file's edits may be indexed under: its own and, for a
/// rename, the old one (only if it is valid UTF-8; a lossy name could equal a
/// different valid one, so it is never a lookup key).
fn names_of(canonical: &str, view: &GitView) -> Vec<String> {
    let mut names = vec![canonical.to_owned()];
    if let Some(old) = view
        .old_name()
        .and_then(Path::to_str)
        .and_then(canonical_path)
        && old != canonical
    {
        names.push(old);
    }
    names
}

/// The captured calls behind the records, for the `TurnRef`s.
fn turns_for(
    events: &dyn EventStore,
    wt: &WorktreeId,
    canonical: &str,
    view: &GitView,
) -> Result<Vec<EditTouch>, IpcError> {
    Ok(events.edit_calls(wt, &names_of(canonical, view))?)
}

// ---------------------------------------------------------------------------
// Runs, coverage, turns
// ---------------------------------------------------------------------------

/// Find the call (turn) that explains an `Edit` origin. The key is the
/// harness's own call id, plus the subagent when there was one. A call with no
/// id is matched only when exactly one call has the same session, turn and
/// subagent; two such calls cannot be told apart, so none is chosen.
fn turn_of(src: &EditSource, path: &str, touches: &[EditTouch]) -> Option<TurnRef> {
    let same_call = |t: &&EditTouch| {
        t.session == src.session
            && t.agent_id == src.agent_id
            && t.tool_use_id == src.tool_use_id
            && src.turn_id.as_deref().is_none_or(|id| id == t.turn_id)
    };
    let mut found = touches.iter().filter(same_call);
    let touch = found.next()?;
    // Two different calls with one key (an id-less pair): not provable.
    if found.next().is_some() && src.tool_use_id.is_none() {
        return None;
    }
    Some(TurnRef {
        harness: harness_to_contract(touch.session.harness),
        session_id: touch.session.to_contract(),
        turn_id: touch.turn_id.clone(),
        turn_index: touch.turn_index,
        agent_id: touch.agent_id.clone(),
        tool_use_id: touch.tool_use_id.clone(),
        // Ops exist only for successful edits (see `provenance::edit`).
        outcome: ToolOutcome::Ok,
        at: rfc3339_from_ms(touch.at_ms),
        path: path.to_owned(),
    })
}

fn build_origins(
    worktree_id: &WorktreeId,
    path: &str,
    origins: &[LineOrigin],
    touches: &[EditTouch],
) -> FileLineOrigins {
    // One lookup per distinct source: a 5,000-line file written by one edit
    // must not search the call list 5,000 times.
    let mut turns: HashMap<&EditSource, Option<TurnRef>> = HashMap::new();
    let kinds: Vec<LineOriginKind> = origins
        .iter()
        .map(|origin| match origin {
            LineOrigin::Base => LineOriginKind::Base,
            LineOrigin::Unknown(reason) => LineOriginKind::Unknown {
                reason: (*reason).into(),
            },
            LineOrigin::Edit(src) => {
                match turns
                    .entry(src)
                    .or_insert_with(|| turn_of(src, path, touches))
                {
                    Some(turn) => LineOriginKind::Edit { turn: turn.clone() },
                    // An edit WHY recorded but cannot point at in the
                    // timeline: say so, never invent a turn.
                    None => LineOriginKind::Unknown {
                        reason: LineUnknownReason::NoCapture,
                    },
                }
            }
        })
        .collect();

    let runs = runs_of(&kinds);
    let coverage = coverage_of(&kinds);
    FileLineOrigins {
        worktree_id: worktree_id.clone(),
        path: path.to_owned(),
        line_count: u32::try_from(kinds.len()).unwrap_or(u32::MAX),
        runs,
        coverage,
    }
}

/// Consecutive lines with the same origin become one run; the runs partition
/// `1..=n` in order. Two ops of ONE call (an `apply_patch` touching a file
/// twice) have equal `TurnRef`s and so merge, which is what the reviewer
/// wants: the unit they can open is the turn.
fn runs_of(kinds: &[LineOriginKind]) -> Vec<LineRun> {
    let mut runs: Vec<LineRun> = Vec::new();
    for (i, kind) in kinds.iter().enumerate() {
        let line = u32::try_from(i + 1).unwrap_or(u32::MAX);
        match runs.last_mut() {
            Some(run) if run.origin == *kind => run.end_line = line,
            _ => runs.push(LineRun {
                start_line: line,
                end_line: line,
                origin: kind.clone(),
                evidence: match kind {
                    LineOriginKind::Unknown { .. } => EvidenceLabel::Unknown,
                    // `observed` for base and edit; never declared/inferred.
                    LineOriginKind::Base | LineOriginKind::Edit { .. } => EvidenceLabel::Observed,
                },
            }),
        }
    }
    runs
}

/// `lines == base + unknown + sum(explained)`; explained is per harness
/// (decision 30), only for harnesses with at least one line.
fn coverage_of(kinds: &[LineOriginKind]) -> LineCoverage {
    let (mut base, mut unknown) = (0u32, 0u32);
    let mut explained: Vec<HarnessLines> = Vec::new();
    for kind in kinds {
        match kind {
            LineOriginKind::Base => base += 1,
            LineOriginKind::Unknown { .. } => unknown += 1,
            LineOriginKind::Edit { turn } => {
                match explained.iter_mut().find(|e| e.harness == turn.harness) {
                    Some(entry) => entry.lines += 1,
                    None => explained.push(HarnessLines {
                        harness: turn.harness,
                        lines: 1,
                    }),
                }
            }
        }
    }
    explained.sort_by_key(|e| harness_rank(e.harness));
    LineCoverage {
        lines: u32::try_from(kinds.len()).unwrap_or(u32::MAX),
        base,
        unknown,
        explained,
    }
}

/// A fixed order, so the wire answer never depends on which line came first.
fn harness_rank(h: Harness) -> u8 {
    match h {
        Harness::ClaudeCode => 0,
        Harness::Codex => 1,
        Harness::Unknown => 2,
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn unavailable(reason: LineOriginsUnavailable) -> LineOriginsResult {
    LineOriginsResult::Unavailable { reason }
}

fn invalid(message: impl Into<String>) -> IpcError {
    IpcError::InvalidInput {
        message: message.into(),
    }
}

/// Same mapping as `changes.rs` for the guarded working-tree read: our own
/// refusals are the caller's fault, anything else is a short generic error.
fn io_error(err: io::Error) -> IpcError {
    match err.kind() {
        io::ErrorKind::NotFound => IpcError::NotFound,
        io::ErrorKind::InvalidInput => invalid("path is not a plain relative path"),
        io::ErrorKind::PermissionDenied if err.raw_os_error().is_none() => {
            invalid("path leaves the worktree")
        }
        _ => {
            tracing::warn!(error = %err, "cannot read a worktree file");
            IpcError::Internal {
                message: "could not read the file".to_string(),
            }
        }
    }
}

/// A blob missing from the local object database (partial clone, corruption):
/// reported, never fetched (the runner forbids lazy fetching).
fn blob_error(err: git::GitError) -> IpcError {
    if err.is_exit_code(128) {
        tracing::warn!(error = %err, "blob is not in the local object database");
        return IpcError::GitFailed {
            message: "object not available locally (partial clone?)".to_string(),
        };
    }
    err.into()
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use why_wire::Harness as WireHarness;

    use super::*;
    use crate::events::{EventKey, SessionKey};
    use crate::provenance::{
        EditHunk, EditKind, EditOp, HunkLineHash, HunkLineKind, Observed, UnknownReason,
    };

    fn session() -> SessionKey {
        SessionKey::new(WireHarness::ClaudeCode, "s")
    }

    fn source(k: usize) -> EditSource {
        EditSource {
            session: session(),
            turn_id: Some("p".into()),
            agent_id: None,
            tool_use_id: Some(format!("c{k}")),
            event_key: EventKey(format!("k{k}")),
        }
    }

    fn touch(k: usize) -> EditTouch {
        EditTouch {
            session: session(),
            turn_id: "p".into(),
            turn_index: k as u32,
            agent_id: None,
            tool_use_id: Some(format!("c{k}")),
            outcome: ToolOutcome::Ok,
            at_ms: k as u64,
            paths: vec!["f.rs".into()],
        }
    }

    /// 1,000 lines, 200 single-line edits, each with its full `before`.
    fn thousand_lines_two_hundred_edits() -> (Option<FileSnapshot>, Vec<EditRecord>, Vec<u8>) {
        let hasher = LineHasher::default();
        let mut lines: Vec<String> = (0..1000).map(|i| format!("let v{i} = {i};")).collect();
        let text = |l: &[String]| l.join("\n") + "\n";
        let baseline = FileSnapshot::of_text(&hasher, text(&lines).as_bytes());
        let mut records = Vec::new();
        for k in 0..200 {
            let at = (k * 5) % 1000;
            let before = FileSnapshot::of_text(&hasher, text(&lines).as_bytes());
            let removed = hasher.hash_line(lines[at].as_bytes());
            lines[at] = format!("let v{at} = {};", 10_000 + k);
            let added = hasher.hash_line(lines[at].as_bytes());
            records.push(EditRecord {
                source: source(k),
                seq: k as u64 + 1,
                op_index: 0,
                at_ms: k as u64,
                op: EditOp {
                    worktree_id: None,
                    path: "f.rs".into(),
                    kind: EditKind::Update,
                    renamed_from: None,
                    before: Some(before),
                    after: None,
                    hunks: vec![EditHunk {
                        old_start: Some(at as u32 + 1),
                        header: None,
                        lines: vec![
                            HunkLineHash {
                                kind: HunkLineKind::Removed,
                                hash: removed,
                            },
                            HunkLineHash {
                                kind: HunkLineKind::Added,
                                hash: added,
                            },
                        ],
                    }],
                    observed: Observed::Exact,
                },
            });
        }
        (Some(baseline), records, text(&lines).into_bytes())
    }

    #[test]
    fn engine_alignment_and_runs_for_a_thousand_lines_and_two_hundred_edits() {
        let (baseline, records, real) = thousand_lines_two_hundred_edits();
        let touches: Vec<EditTouch> = (0..200).map(touch).collect();
        let wt = WorktreeId("w".into());

        let start = Instant::now();
        let origins = compute(baseline, records, &real, &LineHasher::default());
        let built = build_origins(&wt, "f.rs", &origins, &touches);
        let took = start.elapsed();
        eprintln!(
            "engine + alignment + runs, 1000 lines, 200 edits: {took:?} (debug build: {})",
            cfg!(debug_assertions)
        );

        assert_eq!(built.line_count, 1000);
        assert_eq!(built.coverage.explained[0].lines, 200);
        // The 50 ms budget is asserted only in optimised builds: a wall-clock
        // limit in the everyday debug suite would flake on a loaded machine
        // (measured: ~25 ms debug, ~3 ms release). The shape checks above are
        // deterministic and always run.
        if !cfg!(debug_assertions) {
            assert!(took.as_millis() < 50, "{took:?}");
        }
    }

    #[test]
    fn a_recorded_edit_the_timeline_cannot_show_is_unknown_never_an_invented_turn() {
        let origins = vec![
            LineOrigin::Edit(source(1)),
            LineOrigin::Edit(source(2)),
            LineOrigin::Base,
        ];
        // Only call 1 is known to the timeline.
        let built = build_origins(&WorktreeId("w".into()), "f.rs", &origins, &[touch(1)]);
        assert!(matches!(built.runs[0].origin, LineOriginKind::Edit { .. }));
        assert_eq!(
            built.runs[1].origin,
            LineOriginKind::Unknown {
                reason: LineUnknownReason::NoCapture
            }
        );
        assert_eq!(built.runs[1].evidence, EvidenceLabel::Unknown);
        assert_eq!(built.coverage.lines, 3);
        assert_eq!((built.coverage.base, built.coverage.unknown), (1, 1));
    }

    #[test]
    fn runs_partition_the_file_and_merge_equal_neighbours() {
        let origins = vec![
            LineOrigin::Base,
            LineOrigin::Base,
            LineOrigin::Unknown(UnknownReason::NoCapture),
            LineOrigin::Base,
        ];
        let built = build_origins(&WorktreeId("w".into()), "f.rs", &origins, &[]);
        let spans: Vec<_> = built
            .runs
            .iter()
            .map(|r| (r.start_line, r.end_line))
            .collect();
        assert_eq!(spans, [(1, 2), (3, 3), (4, 4)]);
        assert!(runs_of(&[]).is_empty(), "an empty file has no runs");
    }
}
