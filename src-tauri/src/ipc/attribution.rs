//! Commands that say which agent turns explain a worktree's changed files
//! (WHY-16). Read the contract (`contracts/attribution.rs`) for what the labels
//! do and do not claim; the short version: git decides which files changed,
//! captured edit events decide which of them are *explained*, and everything
//! else is `unexplained`, never "manual".
//!
//! The git side is [`get_changes_impl`] reused as it is (so the filter-driver
//! guard and the read permit apply unchanged); this file adds no git call and
//! reads no file.

use std::collections::{BTreeSet, HashMap};

use tauri::State;

use crate::contracts::{
    AttributionStatus, DiffMode, EvidenceLabel, FileAttribution, FileChange, IpcError, ShellLead,
    ToolOutcome, TurnRef, WorktreeAttribution, WorktreeId,
};
use crate::events::timeline::{harness_to_contract, rfc3339_from_ms};
use crate::events::{EditTouch, ShellTurn, canonical_path};
use crate::ipc::changes::get_changes_impl;
use crate::state::AppState;

/// The worktree's changed files, each with the turns whose captured edits
/// explain it. `mode` defaults to `vsBase`, like the changed-files list.
#[tauri::command]
pub async fn get_worktree_attribution(
    state: State<'_, AppState>,
    worktree_id: WorktreeId,
    mode: Option<DiffMode>,
) -> Result<WorktreeAttribution, IpcError> {
    get_worktree_attribution_impl(&state, &worktree_id, mode.unwrap_or(DiffMode::VsBase)).await
}

pub async fn get_worktree_attribution_impl(
    state: &AppState,
    worktree_id: &WorktreeId,
    mode: DiffMode,
) -> Result<WorktreeAttribution, IpcError> {
    // Git is the source of truth for "changed": an edit the agent later undid
    // leaves no change here, so it is never listed.
    let changes = get_changes_impl(state, worktree_id, mode).await?;

    // Every spelling a file may have been edited under: its path and, for a
    // rename or copy, the old one.
    let wanted: Vec<String> = changes
        .iter()
        .flat_map(change_paths)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let touches = state.events.edit_calls(worktree_id, &wanted)?;
    let shell = state.events.shell_turns(worktree_id)?;
    Ok(attribute(worktree_id, mode, changes, &touches, &shell))
}

fn change_paths(change: &FileChange) -> impl Iterator<Item = String> + '_ {
    // Git supplies one lossiness flag for both names. Neither is a safe key
    // when the flag is set, since U+FFFD can also be part of a valid filename.
    std::iter::once(&change.path)
        .chain(change.old_path.as_ref())
        .filter(|_| !change.path_lossy)
        .filter_map(|path| canonical_path(path))
}

/// Pure: join the git changes with the captured edit calls. `touches` is
/// newest first (the store's order), and that order carries into the lists.
fn attribute(
    worktree_id: &WorktreeId,
    mode: DiffMode,
    changes: Vec<FileChange>,
    touches: &[EditTouch],
    shell: &[ShellTurn],
) -> WorktreeAttribution {
    let mut by_path: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, touch) in touches.iter().enumerate() {
        for path in &touch.paths {
            by_path.entry(path.as_str()).or_default().push(index);
        }
    }

    let files = changes
        .into_iter()
        .map(|change| {
            let names: Vec<String> = change_paths(&change).collect();
            let mut matches: Vec<_> = names
                .iter()
                .flat_map(|name| {
                    by_path
                        .get(name.as_str())
                        .into_iter()
                        .flatten()
                        .map(move |&index| (index, name))
                })
                .collect();
            // Preserve store order and keep the current name when one call
            // named both sides of a rename.
            matches.sort_by_key(|&(index, _)| index);
            matches.dedup_by_key(|entry| entry.0);

            let mut explained_by = Vec::new();
            let mut attempts = Vec::new();
            for (index, path) in matches {
                let touch = &touches[index];
                let r = turn_ref(touch, path);
                // Only an applied edit explains the file. A failed one is an
                // attempt; an unconfirmed one (rejected OR lost delivery) is
                // "maybe", which is not an explanation either.
                match touch.outcome {
                    ToolOutcome::Ok => explained_by.push(r),
                    ToolOutcome::Failed | ToolOutcome::Unknown => attempts.push(r),
                }
            }
            let (status, evidence) = if explained_by.is_empty() {
                (AttributionStatus::Unexplained, EvidenceLabel::Unknown)
            } else {
                (AttributionStatus::Explained, EvidenceLabel::Observed)
            };
            FileAttribution {
                change,
                status,
                evidence,
                explained_by,
                attempts,
            }
        })
        .collect();

    WorktreeAttribution {
        worktree_id: worktree_id.clone(),
        mode,
        files,
        shell_leads: shell.iter().map(shell_lead).collect(),
    }
}

fn turn_ref(touch: &EditTouch, path: &str) -> TurnRef {
    TurnRef {
        harness: harness_to_contract(touch.session.harness),
        session_id: touch.session.to_contract(),
        turn_id: touch.turn_id.clone(),
        turn_index: touch.turn_index,
        agent_id: touch.agent_id.clone(),
        tool_use_id: touch.tool_use_id.clone(),
        outcome: touch.outcome,
        at: rfc3339_from_ms(touch.at_ms),
        path: path.to_owned(),
    }
}

fn shell_lead(turn: &ShellTurn) -> ShellLead {
    ShellLead {
        harness: harness_to_contract(turn.session.harness),
        session_id: turn.session.to_contract(),
        turn_id: turn.turn_id.clone(),
        turn_index: turn.turn_index,
        command_count: turn.command_count,
        first_at: rfc3339_from_ms(turn.first_at_ms),
    }
}

/// Every captured edit call that named `path`, any outcome, newest first: how
/// the UI jumps from a file to the turn in the timeline
/// (`get_session_timeline`). `path` is worktree-relative; for a renamed file ask
/// for the old path too. An unknown worktree or file has no activity (not an
/// error), like `list_sessions`.
#[tauri::command]
pub async fn get_file_activity(
    state: State<'_, AppState>,
    worktree_id: WorktreeId,
    path: String,
) -> Result<Vec<TurnRef>, IpcError> {
    get_file_activity_impl(&state, &worktree_id, &path).await
}

pub async fn get_file_activity_impl(
    state: &AppState,
    worktree_id: &WorktreeId,
    path: &str,
) -> Result<Vec<TurnRef>, IpcError> {
    // The path comes from the webview: it is used only as a lookup key into the
    // index (never to touch the disk), but it must still be a worktree-relative
    // path, and the answer to "..", an absolute path or "" is an error, not an
    // empty list (which would read as "nothing happened to this file").
    let canonical = canonical_path(path).ok_or_else(|| IpcError::InvalidInput {
        message: format!("{path:?} is not a path inside the worktree"),
    })?;
    let touches = state
        .events
        .edit_calls(worktree_id, std::slice::from_ref(&canonical))?;
    Ok(touches.iter().map(|t| turn_ref(t, &canonical)).collect())
}

#[cfg(test)]
mod tests {
    use why_wire::Harness;

    use super::*;
    use crate::contracts::ChangeStatus;
    use crate::events::SessionKey;

    fn change(path: &str, old: Option<&str>) -> FileChange {
        FileChange {
            path: path.into(),
            old_path: old.map(str::to_owned),
            status: if old.is_some() {
                ChangeStatus::Renamed
            } else {
                ChangeStatus::Modified
            },
            staged: false,
            unstaged: true,
            additions: Some(1),
            deletions: Some(0),
            binary: false,
            path_lossy: false,
        }
    }

    fn touch(path: &str, outcome: ToolOutcome, at: u64, id: &str) -> EditTouch {
        EditTouch {
            session: SessionKey::new(Harness::Codex, "s1"),
            turn_id: "t1".into(),
            turn_index: 0,
            agent_id: None,
            tool_use_id: Some(id.into()),
            outcome,
            at_ms: at,
            paths: vec![path.into()],
        }
    }

    fn run(changes: Vec<FileChange>, touches: &[EditTouch]) -> Vec<FileAttribution> {
        attribute(
            &WorktreeId("w".into()),
            DiffMode::VsBase,
            changes,
            touches,
            &[],
        )
        .files
    }

    #[test]
    fn only_an_applied_edit_explains_a_file() {
        let files = run(
            vec![
                change("a.rs", None),
                change("b.rs", None),
                change("c.rs", None),
            ],
            &[
                touch("a.rs", ToolOutcome::Ok, 3, "ok"),
                touch("b.rs", ToolOutcome::Failed, 2, "bad"),
                touch("b.rs", ToolOutcome::Unknown, 1, "maybe"),
            ],
        );
        let a = &files[0];
        assert_eq!(a.status, AttributionStatus::Explained);
        assert_eq!(a.evidence, EvidenceLabel::Observed);
        assert_eq!(a.explained_by.len(), 1);
        assert!(a.attempts.is_empty());

        let b = &files[1];
        assert_eq!(b.status, AttributionStatus::Unexplained);
        assert_eq!(b.evidence, EvidenceLabel::Unknown);
        assert!(b.explained_by.is_empty());
        assert_eq!(b.attempts.len(), 2);

        let c = &files[2];
        assert_eq!(c.status, AttributionStatus::Unexplained);
        assert!(c.attempts.is_empty());
    }

    #[test]
    fn lossy_git_paths_are_display_only() {
        let mut lossy = change("caf�.txt", Some("previous.txt"));
        lossy.path_lossy = true;
        let files = run(
            vec![lossy, change("caf�.txt", None)],
            &[
                touch("caf�.txt", ToolOutcome::Ok, 2, "valid-name"),
                touch("previous.txt", ToolOutcome::Failed, 1, "old-name"),
            ],
        );
        assert_eq!(files[0].status, AttributionStatus::Unexplained);
        assert!(files[0].explained_by.is_empty() && files[0].attempts.is_empty());
        assert_eq!(files[1].status, AttributionStatus::Explained);
    }

    #[test]
    fn a_rename_matches_an_edit_of_either_name_once_per_call() {
        let mut both = touch("new.rs", ToolOutcome::Ok, 5, "both");
        both.paths = vec!["new.rs".into(), "old.rs".into()];
        let files = run(
            vec![change("new.rs", Some("old.rs"))],
            &[
                touch("old.rs", ToolOutcome::Ok, 7, "older-name-first"),
                both,
                touch("old.rs", ToolOutcome::Ok, 1, "old-name"),
                touch("elsewhere.rs", ToolOutcome::Ok, 9, "other"),
            ],
        );
        let refs = &files[0].explained_by;
        assert_eq!(refs.len(), 3, "one reference per call: {refs:?}");
        // The call that named both is reported under the new name.
        assert_eq!(
            refs.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            ["old.rs", "new.rs", "old.rs"]
        );
    }
}
