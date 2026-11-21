//! Commands that read the agent's own words, and Codex shell outcomes, from the
//! harness transcript, on demand (WHY-53). Read the contract
//! (`contracts/rationale.rs`) for what the labels mean.
//!
//! # Lazy by design
//!
//! Nothing here runs during ingest or on a timer: every file read happens
//! because the UI called a command, and the text is returned as capped excerpts
//! and dropped (no copy in WHY's database). The ids come from the UI; the file
//! path does NOT: it is looked up from the events the hooks recorded, and the
//! readers (`adapters::*::{transcript,rollout}`) still refuse anything outside
//! the harness's own directory.
//!
//! # Where the work runs
//!
//! File reads block, so each command moves its owned inputs into
//! `tauri::async_runtime::spawn_blocking` (the store lookup too: SQLite can
//! wait on a lock). Every call has a byte budget: at most [`MAX_PAGES`] pages
//! of [`PAGE_BYTES`] for a Claude transcript, and the readers' own bounded
//! windows for Codex (at most [`MAX_ROLLOUT_FILES`] rollouts per call).
//!
//! # Errors vs. values
//!
//! `IpcError::InvalidInput` for an id that is not ours, `NotFound` for a valid
//! session, turn or tool call the store does not know. A file that is gone,
//! outside its root, not a regular file or lacks the anchor is a *value*
//! (`unavailable { reason }`), because it is a normal thing for the UI to show.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tauri::State;
use why_wire::Harness as WireHarness;

use crate::adapters::claude::transcript::{
    self, Limits, Query, TextBlock, TranscriptError, TranscriptScan,
};
use crate::adapters::codex::{ENCRYPTED, rollout};
use crate::contracts::{
    BlockPosition, EvidenceLabel, IpcError, Rationale, RationaleBlock, RationaleResult, SessionId,
    ShellOutcome, ShellOutcomes, ShellOutcomesResult, ToolOutcome, TranscriptSource,
    TranscriptUnavailable,
};
use crate::events::SessionKey;
use crate::events::rationale::{self as select, UnknownShellCall};
use crate::state::AppState;

/// One Claude read is at most this many pages...
pub const MAX_PAGES: u32 = 4;
/// ...of this many bytes (16 MiB in total).
pub const PAGE_BYTES: u64 = 4 * 1024 * 1024;
/// Text blocks returned for one request. `Query::Turn` has no limit of its own.
pub const MAX_BLOCKS: usize = 100;
/// Rollout files scanned by one `get_shell_outcomes` call. Each scan reads at
/// most ~18 MiB (`rollout::MAX_SCAN_BYTES` + its head), so a call reads under
/// ~72 MiB however many files a resumed session left behind.
pub const MAX_ROLLOUT_FILES: usize = 4;

/// The directories a transcript may be read from. Injected so tests use temp
/// roots; the commands build it from the process environment.
#[derive(Debug, Clone, Default)]
pub struct RationaleRoots {
    /// `$CLAUDE_CONFIG_DIR/projects` or `~/.claude/projects`.
    pub claude_projects: Option<PathBuf>,
    /// `$CODEX_HOME/sessions` or `~/.codex/sessions`.
    pub codex_sessions: Option<PathBuf>,
}

impl RationaleRoots {
    /// From `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `HOME`. Only these three
    /// variables are read; no config file (`~/.claude.json`, `config.toml`)
    /// and no credential file is opened to find a root.
    pub fn from_env() -> Self {
        Self {
            claude_projects: transcript::default_projects_root(),
            codex_sessions: rollout::sessions_root(),
        }
    }
}

// ---- commands ------------------------------------------------------------

/// What the agent said during one turn (`Declared`). Claude: all assistant text
/// of the prompt; Codex: the turn's agent messages.
#[tauri::command]
pub async fn get_turn_rationale(
    state: State<'_, AppState>,
    session_id: SessionId,
    turn_id: String,
) -> Result<RationaleResult, IpcError> {
    get_turn_rationale_impl(&state, &RationaleRoots::from_env(), &session_id, &turn_id).await
}

pub async fn get_turn_rationale_impl(
    state: &AppState,
    roots: &RationaleRoots,
    session_id: &SessionId,
    turn_id: &str,
) -> Result<RationaleResult, IpcError> {
    let key = parse_session(session_id)?;
    require_id("turn id", turn_id)?;
    let events = state.events.clone();
    let roots = roots.clone();
    let turn_id = turn_id.to_owned();
    blocking(move || {
        let stored = events.source_events(&key)?.ok_or(IpcError::NotFound)?;
        let turn = select::locate_turn(&stored, key.harness, &turn_id).ok_or(IpcError::NotFound)?;
        Ok(read_rationale(
            &roots,
            key.harness,
            turn.transcript_path.as_deref(),
            Anchor::Turn(&turn_id),
        ))
    })
    .await
}

/// What the agent said just before (Codex: and just after) the timeline entry
/// identified by `event_id`.
#[tauri::command]
pub async fn get_tool_rationale(
    state: State<'_, AppState>,
    session_id: SessionId,
    event_id: String,
) -> Result<RationaleResult, IpcError> {
    get_tool_rationale_impl(&state, &RationaleRoots::from_env(), &session_id, &event_id).await
}

pub async fn get_tool_rationale_impl(
    state: &AppState,
    roots: &RationaleRoots,
    session_id: &SessionId,
    event_id: &str,
) -> Result<RationaleResult, IpcError> {
    let key = parse_session(session_id)?;
    require_id("event id", event_id)?;
    let events = state.events.clone();
    let roots = roots.clone();
    let event_id = event_id.to_owned();
    blocking(move || {
        let stored = events.source_events(&key)?.ok_or(IpcError::NotFound)?;
        let call =
            select::locate_tool(&stored, key.harness, &event_id).ok_or(IpcError::NotFound)?;
        Ok(read_rationale(
            &roots,
            key.harness,
            call.transcript_path.as_deref(),
            Anchor::Tool {
                turn_id: &call.turn_id,
                tool_use_id: &call.tool_use_id,
            },
        ))
    })
    .await
}

/// Codex shell and test-run calls whose outcome the hooks left `unknown`, and
/// the ones the rollout's structured `CommandExecution` record settles (exit
/// code → `ok` / `failed`, `observed`). Nothing is written to the store: the
/// UI asks when a Codex session opens. Claude's hooks report failures
/// themselves, so it gets `unavailable { notApplicable }`.
#[tauri::command]
pub async fn get_shell_outcomes(
    state: State<'_, AppState>,
    session_id: SessionId,
) -> Result<ShellOutcomesResult, IpcError> {
    get_shell_outcomes_impl(&state, &RationaleRoots::from_env(), &session_id).await
}

pub async fn get_shell_outcomes_impl(
    state: &AppState,
    roots: &RationaleRoots,
    session_id: &SessionId,
) -> Result<ShellOutcomesResult, IpcError> {
    let key = parse_session(session_id)?;
    let events = state.events.clone();
    let roots = roots.clone();
    blocking(move || {
        let stored = events.source_events(&key)?.ok_or(IpcError::NotFound)?;
        Ok(match key.harness {
            WireHarness::Codex => {
                let calls = select::unknown_shell_calls(&stored);
                codex_shell_outcomes(roots.codex_sessions.as_deref(), calls)
            }
            WireHarness::ClaudeCode => unavailable_shell(TranscriptUnavailable::NotApplicable),
            WireHarness::Unknown => unavailable_shell(TranscriptUnavailable::UnsupportedHarness),
        })
    })
    .await
}

// ---- plumbing ------------------------------------------------------------

/// Ids are opaque to the UI, which only passes back what the core gave it; one
/// that does not parse was not made by us.
fn parse_session(id: &SessionId) -> Result<SessionKey, IpcError> {
    SessionKey::from_contract(id).ok_or_else(|| IpcError::InvalidInput {
        message: format!("{:?} is not a session id", id.0),
    })
}

fn require_id(what: &str, id: &str) -> Result<(), IpcError> {
    if id.is_empty() {
        return Err(IpcError::InvalidInput {
            message: format!("{what} must not be empty"),
        });
    }
    Ok(())
}

/// Run blocking work (store read + file reads) off the async workers.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, IpcError> + Send + 'static,
) -> Result<T, IpcError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, "rationale worker failed");
            IpcError::Internal {
                message: "internal error while reading the transcript".to_string(),
            }
        })?
}

/// What a rationale read looks for.
#[derive(Clone, Copy)]
enum Anchor<'a> {
    Turn(&'a str),
    Tool {
        turn_id: &'a str,
        tool_use_id: &'a str,
    },
}

fn unavailable(reason: TranscriptUnavailable) -> RationaleResult {
    RationaleResult::Unavailable { reason }
}

fn unavailable_shell(reason: TranscriptUnavailable) -> ShellOutcomesResult {
    ShellOutcomesResult::Unavailable { reason }
}

/// Pick the reader for the session's harness and run it.
fn read_rationale(
    roots: &RationaleRoots,
    harness: WireHarness,
    path: Option<&str>,
    anchor: Anchor<'_>,
) -> RationaleResult {
    let Some(path) = path else {
        return unavailable(TranscriptUnavailable::NoTranscript);
    };
    let path = Path::new(path);
    match harness {
        WireHarness::ClaudeCode => match roots.claude_projects.as_deref() {
            Some(root) => claude_rationale(root, path, anchor),
            None => unavailable(TranscriptUnavailable::Unreadable),
        },
        WireHarness::Codex => match roots.codex_sessions.as_deref() {
            Some(root) => codex_rationale(root, path, anchor),
            None => unavailable(TranscriptUnavailable::Unreadable),
        },
        WireHarness::Unknown => unavailable(TranscriptUnavailable::UnsupportedHarness),
    }
}

// ---- Claude Code ---------------------------------------------------------

fn claude_unavailable(err: &TranscriptError) -> TranscriptUnavailable {
    match err {
        TranscriptError::NotFound => TranscriptUnavailable::Missing,
        // A symlink under the root is refused like a path outside it.
        TranscriptError::OutsideAllowedRoot | TranscriptError::Symlink => {
            TranscriptUnavailable::OutsideRoot
        }
        TranscriptError::UnsupportedPlatform => TranscriptUnavailable::UnsupportedPlatform,
        // The FIFO / directory case, and plain I/O errors.
        TranscriptError::NotRegularFile | TranscriptError::Io(_) => {
            TranscriptUnavailable::Unreadable
        }
    }
}

fn claude_block(block: TextBlock, position: BlockPosition) -> RationaleBlock {
    RationaleBlock {
        text: block.text,
        position,
        timestamp: block.timestamp,
        encrypted: false,
    }
}

/// Page through the transcript until the answer is complete or the byte budget
/// ([`MAX_PAGES`] x [`PAGE_BYTES`]) or the block cap runs out.
fn claude_rationale(root: &Path, path: &Path, anchor: Anchor<'_>) -> RationaleResult {
    let (query, position) = match anchor {
        Anchor::Turn(prompt_id) => (Query::Turn(prompt_id), BlockPosition::Turn),
        Anchor::Tool { tool_use_id, .. } => {
            (Query::BeforeToolUse(tool_use_id), BlockPosition::Before)
        }
    };
    let limits = Limits {
        max_bytes: PAGE_BYTES,
        ..Limits::default()
    };
    let mut scan = match TranscriptScan::open(root, path, query, 0, limits) {
        Ok(scan) => scan,
        Err(e) => return unavailable(claude_unavailable(&e)),
    };

    let mut blocks: Vec<RationaleBlock> = Vec::new();
    let mut found = false;
    let mut truncated = false;
    let mut version = None;
    let mut supported = false;
    // The scan ended at the end of the file or at its answer (not at the budget).
    let mut complete = false;
    for _ in 0..MAX_PAGES {
        let page = match scan.read_page() {
            Ok(page) => page,
            Err(e) => return unavailable(claude_unavailable(&e)),
        };
        found |= page.found;
        truncated |= page.narration_truncated || page.texts.iter().any(|t| t.clipped);
        // The version fields are cumulative over the scan: the last page wins.
        version = page.version;
        supported = page.version_supported;
        blocks.extend(page.texts.into_iter().map(|t| claude_block(t, position)));
        if blocks.len() > MAX_BLOCKS {
            blocks.truncate(MAX_BLOCKS);
            truncated = true;
            complete = true; // stopped on purpose; the flag above says it is cut
            break;
        }
        if !page.truncated {
            complete = true;
            break;
        }
    }
    if !found {
        // Absent from a COMPLETE read is "not found"; absent from a read that
        // ran out of budget is "not found yet" (ok, empty, truncated).
        if complete {
            return unavailable(TranscriptUnavailable::NotFound);
        }
        truncated = true;
    } else if !complete {
        truncated = true;
    }
    RationaleResult::Ok {
        rationale: Rationale {
            evidence: EvidenceLabel::Declared,
            source: TranscriptSource::ClaudeTranscript,
            blocks,
            harness_version: version,
            version_supported: supported,
            truncated,
        },
    }
}

// ---- Codex ---------------------------------------------------------------

fn codex_unavailable(err: &rollout::RolloutError) -> TranscriptUnavailable {
    use rollout::RolloutError;
    match err {
        RolloutError::OutsideRoot => TranscriptUnavailable::OutsideRoot,
        RolloutError::NotRegularFile => TranscriptUnavailable::Unreadable,
        RolloutError::Io(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                TranscriptUnavailable::Missing
            } else if is_unsupported(e) {
                TranscriptUnavailable::UnsupportedPlatform
            } else {
                TranscriptUnavailable::Unreadable
            }
        }
    }
}

/// The confined open is not available: the non-Linux stub says `Unsupported`,
/// an old Linux kernel answers `openat2` with `ENOSYS`.
fn is_unsupported(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::ENOSYS) {
        return true;
    }
    e.kind() == std::io::ErrorKind::Unsupported
}

fn codex_block(text: String, position: BlockPosition) -> RationaleBlock {
    RationaleBlock {
        encrypted: text == ENCRYPTED,
        text,
        position,
        timestamp: None,
    }
}

fn codex_rationale(root: &Path, path: &Path, anchor: Anchor<'_>) -> RationaleResult {
    let (turn_id, tool_use_id) = match anchor {
        Anchor::Turn(turn) => (turn, None),
        Anchor::Tool {
            turn_id,
            tool_use_id,
        } => (turn_id, Some(tool_use_id)),
    };
    let ctx = match rollout::read_context(path, root, turn_id, tool_use_id) {
        Ok(ctx) => ctx,
        Err(e) => return unavailable(codex_unavailable(&e)),
    };
    let seen = match anchor {
        Anchor::Turn(_) => ctx.turn_seen,
        Anchor::Tool { .. } => ctx.anchor_seen,
    };
    if !seen && !ctx.truncated {
        return unavailable(TranscriptUnavailable::NotFound);
    }
    let truncated = ctx.truncated
        || match anchor {
            Anchor::Turn(_) => ctx.turn_messages_truncated,
            Anchor::Tool { .. } => ctx.narration_clipped,
        };
    let blocks = match anchor {
        Anchor::Turn(_) => ctx
            .turn_messages
            .into_iter()
            .map(|t| codex_block(t, BlockPosition::Turn))
            .collect(),
        Anchor::Tool { .. } => ctx
            .narration_before
            .map(|t| codex_block(t, BlockPosition::Before))
            .into_iter()
            .chain(
                ctx.narration_after
                    .map(|t| codex_block(t, BlockPosition::After)),
            )
            .collect(),
    };
    RationaleResult::Ok {
        rationale: Rationale {
            evidence: EvidenceLabel::Declared,
            source: TranscriptSource::CodexRollout,
            blocks,
            harness_version: ctx.cli_version,
            version_supported: ctx.version_trusted,
            // Not seen inside a window that is only the file's tail also lands
            // here: "not found yet", not "absent".
            truncated,
        },
    }
}

/// One bounded pass per distinct rollout file, newest call's file first. A call
/// is upgraded only when its id has exactly one structured record with a code
/// in a file whose `cli_version` is trusted (`ExitCodes::exit_code`). Anything
/// else stays `unknown`; nothing is guessed and no output text is parsed.
fn codex_shell_outcomes(root: Option<&Path>, calls: Vec<UnknownShellCall>) -> ShellOutcomesResult {
    if calls.is_empty() {
        // Nothing to settle, so no file is opened (and no version was seen).
        return ShellOutcomesResult::Ok {
            result: ShellOutcomes {
                outcomes: Vec::new(),
                unresolved: 0,
                harness_version: None,
                version_supported: false,
                truncated: false,
            },
        };
    }
    let Some(root) = root else {
        return unavailable_shell(TranscriptUnavailable::Unreadable);
    };

    // An id shared by two unknown calls cannot be told apart: leave both out.
    let mut uses: HashMap<&str, u32> = HashMap::new();
    for call in &calls {
        *uses.entry(call.use_id.as_str()).or_default() += 1;
    }

    // Distinct files, the most recent call's file first, capped.
    let mut paths: Vec<&str> = Vec::new();
    for call in calls.iter().rev() {
        if let Some(path) = call.transcript_path.as_deref()
            && !paths.contains(&path)
        {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return unavailable_shell(TranscriptUnavailable::NoTranscript);
    }
    let mut truncated = paths.len() > MAX_ROLLOUT_FILES;
    paths.truncate(MAX_ROLLOUT_FILES);

    let mut scanned: HashMap<&str, rollout::ExitCodes> = HashMap::new();
    let mut first_error = None;
    for path in paths {
        match rollout::read_exit_codes(Path::new(path), root) {
            Ok(exits) => {
                truncated |= exits.truncated || exits.overflowed;
                scanned.insert(path, exits);
            }
            Err(e) => {
                first_error.get_or_insert_with(|| codex_unavailable(&e));
            }
        }
    }
    if scanned.is_empty() {
        return unavailable_shell(first_error.unwrap_or(TranscriptUnavailable::Unreadable));
    }
    // A file that could not be read leaves its calls unresolved.
    truncated |= first_error.is_some();

    let version_supported = scanned.values().all(|e| e.version_trusted);
    // In a mixed session, report the newest scanned rollout that blocked recovery.
    let harness_version = calls
        .iter()
        .rev()
        .filter_map(|c| c.transcript_path.as_deref())
        .filter_map(|p| scanned.get(p))
        .find(|e| e.version_trusted == version_supported)
        .and_then(|e| e.cli_version.clone());

    let outcomes: Vec<ShellOutcome> = calls
        .iter()
        .filter(|call| version_supported && uses.get(call.use_id.as_str()) == Some(&1))
        .filter_map(|call| {
            let exits = scanned.get(call.transcript_path.as_deref()?)?;
            let code = exits.exit_code(&call.use_id, &call.turn_id)?;
            Some(ShellOutcome {
                tool_use_id: call.use_id.clone(),
                event_id: call.event_id.clone(),
                turn_id: call.turn_id.clone(),
                agent_id: call.agent_id.clone(),
                exit_code: code,
                outcome: if code == 0 {
                    ToolOutcome::Ok
                } else {
                    ToolOutcome::Failed
                },
                evidence: EvidenceLabel::Observed,
                source: TranscriptSource::CodexRollout,
            })
        })
        .collect();
    let unresolved = u32::try_from(calls.len() - outcomes.len()).unwrap_or(u32::MAX);
    ShellOutcomesResult::Ok {
        result: ShellOutcomes {
            outcomes,
            unresolved,
            harness_version,
            version_supported,
            truncated,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn claude_errors_map_to_reasons() {
        use TranscriptUnavailable as U;
        let io_err = || TranscriptError::Io(io::Error::other("boom"));
        for (err, want) in [
            (TranscriptError::NotFound, U::Missing),
            (TranscriptError::OutsideAllowedRoot, U::OutsideRoot),
            (TranscriptError::Symlink, U::OutsideRoot),
            (TranscriptError::NotRegularFile, U::Unreadable),
            (TranscriptError::UnsupportedPlatform, U::UnsupportedPlatform),
            (io_err(), U::Unreadable),
        ] {
            assert_eq!(claude_unavailable(&err), want, "{err}");
        }
    }

    #[test]
    fn codex_errors_map_to_reasons() {
        use TranscriptUnavailable as U;
        use rollout::RolloutError as E;
        let io = |kind| E::Io(io::Error::new(kind, "x"));
        for (err, want) in [
            (E::OutsideRoot, U::OutsideRoot),
            (E::NotRegularFile, U::Unreadable),
            (io(io::ErrorKind::NotFound), U::Missing),
            // The non-Linux stub of the confined open.
            (io(io::ErrorKind::Unsupported), U::UnsupportedPlatform),
            // An old kernel without `openat2`.
            #[cfg(unix)]
            (
                E::Io(io::Error::from_raw_os_error(libc::ENOSYS)),
                U::UnsupportedPlatform,
            ),
            (io(io::ErrorKind::PermissionDenied), U::Unreadable),
        ] {
            assert_eq!(codex_unavailable(&err), want, "{err}");
        }
    }

    #[test]
    fn the_block_cap_constants_keep_one_call_bounded() {
        // 16 MiB of Claude pages, and under ~72 MiB of rollouts per call.
        assert_eq!(MAX_PAGES as u64 * PAGE_BYTES, 16 * 1024 * 1024);
        let per_file = rollout::MAX_SCAN_BYTES + 2 * 1024 * 1024;
        assert!(MAX_ROLLOUT_FILES as u64 * per_file < 80 * 1024 * 1024);
    }
}
