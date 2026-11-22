//! `get_line_origins` for real (WHY-23): real temp git repositories, the real
//! guarded reads, edits recorded in an event store, the answer the gutter shows.
//!
//! Every test builds its own repository (isolated git config, no `$HOME`).
//! Edits are recorded by hand as the hooks would send them: a `ToolFinished`
//! event of an edit tool carrying one op with line hashes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::json;
use why_lib::contracts::{
    FileLineOrigins, Harness as ContractHarness, IpcError, LineOriginKind, LineOriginsResult,
    LineOriginsUnavailable, LineRun, RepoId, SessionSummary, SessionTimeline, ToolOutcome,
    WorktreeId,
};
use why_lib::events::{
    EditTouch, EventKind, EventStore, MemoryEventStore, NormalizedEvent, SessionKey, ToolCall,
    ToolClass,
};
use why_lib::ipc::changes::list_worktrees_impl;
use why_lib::ipc::provenance::get_line_origins_impl;
use why_lib::ipc::repos::add_repository_impl;
use why_lib::provenance::{
    EditHunk, EditKind, EditOp, EditRecord, FileSnapshot, HunkLineHash, HunkLineKind, LineHasher,
    Observed,
};
use why_lib::state::AppState;
use why_lib::store::{MemoryRepoStore, StoreError};
use why_wire::{Envelope, Harness, PROTOCOL_VERSION};

use crate::common::{TestRepo, test_runner, write_executable_script, write_in};

// ---------------------------------------------------------------------------
// An event store whose reads can trigger a "concurrent" change
// ---------------------------------------------------------------------------

/// Delegates to a memory store; after the FIRST `edit_records` call returns it
/// runs a hook, to play "an agent edit arrives between the three reads".
struct HookStore {
    inner: Arc<MemoryEventStore>,
    hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl EventStore for HookStore {
    fn append(
        &self,
        events: &[NormalizedEvent],
    ) -> Result<why_lib::events::AppendOutcome, StoreError> {
        self.inner.append(events)
    }
    fn sessions_for_worktree(&self, wt: &WorktreeId) -> Result<Vec<SessionSummary>, StoreError> {
        self.inner.sessions_for_worktree(wt)
    }
    fn timeline(&self, s: &SessionKey) -> Result<Option<SessionTimeline>, StoreError> {
        self.inner.timeline(s)
    }
    fn prune_older_than(&self, cutoff: u64) -> Result<usize, StoreError> {
        self.inner.prune_older_than(cutoff)
    }
    fn edit_calls(&self, wt: &WorktreeId, paths: &[String]) -> Result<Vec<EditTouch>, StoreError> {
        self.inner.edit_calls(wt, paths)
    }
    fn edit_records(&self, wt: &WorktreeId, path: &str) -> Result<Vec<EditRecord>, StoreError> {
        let records = self.inner.edit_records(wt, path)?;
        if let Some(hook) = self.hook.lock().unwrap().take() {
            hook();
        }
        Ok(records)
    }
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Fx {
    repo: TestRepo,
    /// The linked worktree `wt` on branch `feature` (base: `main`).
    wt: PathBuf,
    state: AppState,
    wt_id: WorktreeId,
    store: Arc<HookStore>,
}

const BASE_TEXT: &str = "one\ntwo\nthree\n";

impl Fx {
    async fn new() -> Self {
        let repo = TestRepo::with_initial_commit();
        repo.write("edit.txt", BASE_TEXT);
        repo.write("crlf.txt", "a\r\nb\r\nc\r\n");
        repo.write("ren.txt", "alpha\nbeta\ngamma\ndelta\n");
        repo.write("del.txt", "gone\nsoon\n");
        write_in(&repo.path, "bin.dat", b"\x00\x01\x02binary\x00");
        repo.commit_all("base files");
        let wt = repo.add_worktree("wt", "feature");

        let store = Arc::new(HookStore {
            inner: Arc::new(MemoryEventStore::new()),
            hook: Mutex::new(None),
        });
        let state = AppState::new(test_runner(), Arc::new(MemoryRepoStore::new()))
            .with_events(store.clone());
        let added = add_repository_impl(&state, repo.path.to_str().unwrap())
            .await
            .unwrap();
        let wt_id = id_at(&state, &added.id, &wt).await;
        Fx {
            repo,
            wt,
            state,
            wt_id,
            store,
        }
    }

    fn write(&self, rel: &str, text: &str) {
        write_in(&self.wt, rel, text.as_bytes());
    }

    fn git(&self, args: &[&str]) -> String {
        crate::common::git(&self.wt, args)
    }

    async fn origins(&self, path: &str) -> FileLineOrigins {
        match self.answer(path).await.unwrap() {
            LineOriginsResult::Ok { origins } => origins,
            other => panic!("{path}: {other:?}"),
        }
    }

    async fn answer(&self, path: &str) -> Result<LineOriginsResult, IpcError> {
        get_line_origins_impl(&self.state, &self.wt_id, path).await
    }

    /// Record a successful edit call.
    fn record(&self, who: &Who, op: EditOp) {
        record_in(&self.store.inner, &self.wt_id, who, op);
    }
}

async fn id_at(state: &AppState, repo_id: &RepoId, path: &Path) -> WorktreeId {
    list_worktrees_impl(state, repo_id)
        .await
        .unwrap()
        .into_iter()
        .find(|w| Path::new(&w.path) == path)
        .unwrap_or_else(|| panic!("no worktree at {}", path.display()))
        .id
}

/// Who made an edit: harness, session, call id, arrival time.
struct Who {
    harness: Harness,
    session: &'static str,
    call: &'static str,
    at: u64,
}

fn claude(session: &'static str, call: &'static str, at: u64) -> Who {
    Who {
        harness: Harness::ClaudeCode,
        session,
        call,
        at,
    }
}

fn record_in(store: &MemoryEventStore, wt: &WorktreeId, who: &Who, mut op: EditOp) {
    op.worktree_id = Some(wt.clone());
    let env = Envelope {
        v: PROTOCOL_VERSION,
        harness: who.harness,
        harness_version: None,
        event: "PostToolUse".into(),
        received_at_ms: who.at,
        cwd: "/x".into(),
        pid: 1,
        payload: json!({"session_id": who.session, "prompt_id": "p1", "tool_use_id": who.call}),
    };
    let mut ev = NormalizedEvent::from_envelope(
        &env,
        0,
        who.session,
        EventKind::ToolFinished {
            outcome: ToolOutcome::Ok,
        },
    );
    ev.turn_id = Some("p1".into());
    ev.worktree_id = Some(wt.clone());
    ev.tool = Some(ToolCall {
        use_id: Some(who.call.into()),
        name: "Edit".into(),
        class: ToolClass::Edit,
    });
    ev.files = vec![op.path.clone()];
    ev.edits = vec![op];
    store.append(&[ev]).unwrap();
}

fn hasher() -> LineHasher {
    LineHasher::default()
}

fn hunk_line(kind: HunkLineKind, text: &str) -> HunkLineHash {
    HunkLineHash {
        kind,
        hash: hasher().hash_line(text.as_bytes()),
    }
}

/// A Claude `Edit`: `removed` lines starting at 0-based `start` become `added`.
fn claude_op(path: &str, before: &str, start: usize, removed: &[&str], added: &[&str]) -> EditOp {
    let mut lines: Vec<_> = removed
        .iter()
        .map(|t| hunk_line(HunkLineKind::Removed, t))
        .collect();
    lines.extend(added.iter().map(|t| hunk_line(HunkLineKind::Added, t)));
    EditOp {
        worktree_id: None,
        path: path.into(),
        kind: EditKind::Update,
        renamed_from: None,
        before: Some(FileSnapshot::of_text(&hasher(), before.as_bytes())),
        after: None,
        hunks: vec![EditHunk {
            old_start: Some(if removed.is_empty() { start } else { start + 1 } as u32),
            header: None,
            lines,
        }],
        observed: Observed::Exact,
    }
}

/// A file created with `lines` (Claude `Write` / Codex add).
fn create_op(path: &str, lines: &[&str]) -> EditOp {
    EditOp {
        worktree_id: None,
        path: path.into(),
        kind: EditKind::Create,
        renamed_from: None,
        before: None,
        after: None,
        hunks: vec![EditHunk {
            old_start: None,
            header: None,
            lines: lines
                .iter()
                .map(|t| hunk_line(HunkLineKind::Added, t))
                .collect(),
        }],
        observed: Observed::Exact,
    }
}

/// A Codex hunk (no positions): context, removed, added in patch order.
fn codex_op(path: &str, hunk: &[(HunkLineKind, &str)]) -> EditOp {
    EditOp {
        worktree_id: None,
        path: path.into(),
        kind: EditKind::Update,
        renamed_from: None,
        before: None,
        after: None,
        hunks: vec![EditHunk {
            old_start: None,
            header: None,
            lines: hunk.iter().map(|(k, t)| hunk_line(*k, t)).collect(),
        }],
        observed: Observed::Exact,
    }
}

/// `1-1 base`, `2-2 e:call`, `3-3 u:noCapture`: one token per run.
fn shape(o: &FileLineOrigins) -> Vec<String> {
    o.runs.iter().map(describe).collect()
}

fn describe(run: &LineRun) -> String {
    let what = match &run.origin {
        LineOriginKind::Base => "base".to_string(),
        LineOriginKind::Edit { turn } => {
            format!("e:{}", turn.tool_use_id.as_deref().unwrap_or("?"))
        }
        LineOriginKind::Unknown { reason } => format!("u:{reason:?}"),
    };
    format!("{}-{} {what}", run.start_line, run.end_line)
}

/// The partition law of the contract, for every answer.
fn assert_well_formed(o: &FileLineOrigins) {
    let mut next = 1;
    for run in &o.runs {
        assert_eq!(run.start_line, next, "gap or overlap: {:?}", o.runs);
        assert!(run.end_line >= run.start_line);
        next = run.end_line + 1;
    }
    assert_eq!(next - 1, o.line_count, "runs cover 1..=lineCount");
    let c = &o.coverage;
    let explained: u32 = c.explained.iter().map(|e| e.lines).sum();
    assert_eq!(c.lines, o.line_count);
    assert_eq!(c.lines, c.base + c.unknown + explained);
}

// ---------------------------------------------------------------------------
// The cases of the brief
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_plain_agent_edit() {
    let fx = Fx::new().await;
    fx.write("edit.txt", "one\nTWO\nthree\n");
    fx.record(
        &claude("s1", "t1", 5_000),
        claude_op("edit.txt", BASE_TEXT, 1, &["two"], &["TWO"]),
    );
    let o = fx.origins("edit.txt").await;
    assert_well_formed(&o);
    assert_eq!(shape(&o), ["1-1 base", "2-2 e:t1", "3-3 base"]);
    assert_eq!(o.coverage.explained.len(), 1);
    assert_eq!(o.coverage.explained[0].harness, ContractHarness::ClaudeCode);
    // The run carries the turn the timeline knows, at the record's time.
    let LineOriginKind::Edit { turn } = &o.runs[1].origin else {
        panic!()
    };
    assert_eq!(turn.turn_id, "p1");
    assert_eq!(turn.outcome, ToolOutcome::Ok);
    assert_eq!(turn.path, "edit.txt");
    assert_eq!(turn.at, "1970-01-01T00:00:05.000Z");
    assert!(o.runs[1].evidence == why_lib::contracts::EvidenceLabel::Observed);
    assert!(o.runs[0].evidence == why_lib::contracts::EvidenceLabel::Observed);
}

#[tokio::test]
async fn an_agent_edit_then_a_hand_edit_afterwards() {
    let fx = Fx::new().await;
    fx.record(
        &claude("s1", "t1", 10),
        claude_op("edit.txt", BASE_TEXT, 1, &["two"], &["TWO"]),
    );
    // Someone (a person, a formatter, a shell command) rewrites line 3 later.
    fx.write("edit.txt", "one\nTWO\nTHREE\n");
    let o = fx.origins("edit.txt").await;
    assert_well_formed(&o);
    assert_eq!(shape(&o), ["1-1 base", "2-2 e:t1", "3-3 u:NoCapture"]);
    assert_eq!(
        o.runs[2].evidence,
        why_lib::contracts::EvidenceLabel::Unknown
    );
}

#[tokio::test]
async fn a_crlf_file_has_the_same_lines_as_its_lf_twin() {
    let fx = Fx::new().await;
    fx.write("crlf.txt", "a\r\nB\r\nc\r\n");
    fx.record(
        &claude("s1", "t1", 10),
        claude_op("crlf.txt", "a\r\nb\r\nc\r\n", 1, &["b"], &["B"]),
    );
    let o = fx.origins("crlf.txt").await;
    assert_well_formed(&o);
    assert_eq!(shape(&o), ["1-1 base", "2-2 e:t1", "3-3 base"]);
}

#[tokio::test]
async fn a_new_untracked_file_is_written_by_its_create() {
    let fx = Fx::new().await;
    fx.write("fresh.txt", "x\ny\n");
    fx.record(&claude("s1", "t1", 10), create_op("fresh.txt", &["x", "y"]));
    let o = fx.origins("fresh.txt").await;
    assert_well_formed(&o);
    assert_eq!(shape(&o), ["1-2 e:t1"]);

    // The same file with nothing recorded: no base, no edit, so all unknown.
    fx.write("unrecorded.txt", "p\nq\n");
    let o = fx.origins("unrecorded.txt").await;
    assert_eq!(shape(&o), ["1-2 u:NoCapture"]);
}

#[tokio::test]
async fn a_deleted_file_has_no_lines() {
    let fx = Fx::new().await;
    std::fs::remove_file(fx.wt.join("del.txt")).unwrap();
    let o = fx.origins("del.txt").await;
    assert_well_formed(&o);
    assert_eq!((o.line_count, o.runs.len()), (0, 0));
    // A path that never existed answers the same way (not an error).
    let o = fx.origins("never/was.txt").await;
    assert_eq!(o.line_count, 0);
}

#[tokio::test]
async fn a_renamed_file_keeps_the_edits_made_under_its_old_name() {
    let fx = Fx::new().await;
    // The agent edits `ren.txt`, then the file is renamed (and staged, so git
    // sees a rename).
    fx.write("ren.txt", "alpha\nBETA\ngamma\ndelta\n");
    fx.record(
        &claude("s1", "t1", 10),
        claude_op(
            "ren.txt",
            "alpha\nbeta\ngamma\ndelta\n",
            1,
            &["beta"],
            &["BETA"],
        ),
    );
    fx.git(&["mv", "ren.txt", "renamed.txt"]);
    let o = fx.origins("renamed.txt").await;
    assert_well_formed(&o);
    assert_eq!(shape(&o), ["1-1 base", "2-2 e:t1", "3-4 base"]);
    // The turn's path is the file's CURRENT path.
    let LineOriginKind::Edit { turn } = &o.runs[1].origin else {
        panic!()
    };
    assert_eq!(turn.path, "renamed.txt");
}

#[tokio::test]
async fn a_binary_file_is_not_text() {
    let fx = Fx::new().await;
    assert_eq!(
        fx.answer("bin.dat").await.unwrap(),
        LineOriginsResult::Unavailable {
            reason: LineOriginsUnavailable::NotText
        }
    );
    // Untracked binary too.
    write_in(&fx.wt, "new.bin", b"\x00\x00\x00");
    assert_eq!(
        fx.answer("new.bin").await.unwrap(),
        LineOriginsResult::Unavailable {
            reason: LineOriginsUnavailable::NotText
        }
    );
}

#[tokio::test]
async fn a_file_over_the_read_limit_is_too_large() {
    let fx = Fx::new().await;
    let big = "line of text\n".repeat(100_000); // 1.3 MB > 1 MiB
    fx.write("big.txt", &big);
    assert_eq!(
        fx.answer("big.txt").await.unwrap(),
        LineOriginsResult::Unavailable {
            reason: LineOriginsUnavailable::TooLarge
        }
    );
}

#[tokio::test]
async fn two_sessions_editing_one_file_keep_their_own_lines() {
    let fx = Fx::new().await;
    // Claude rewrites line 1; Codex (no positions, no `before`) rewrites line 3.
    fx.write("edit.txt", "ONE\ntwo\nTHREE\n");
    fx.record(
        &claude("s1", "t1", 10),
        claude_op("edit.txt", BASE_TEXT, 0, &["one"], &["ONE"]),
    );
    record_in(
        &fx.store.inner,
        &fx.wt_id,
        &Who {
            harness: Harness::Codex,
            session: "c1",
            call: "c-call",
            at: 20,
        },
        codex_op(
            "edit.txt",
            &[
                (HunkLineKind::Context, "two"),
                (HunkLineKind::Removed, "three"),
                (HunkLineKind::Added, "THREE"),
            ],
        ),
    );
    let o = fx.origins("edit.txt").await;
    assert_well_formed(&o);
    assert_eq!(shape(&o), ["1-1 e:t1", "2-2 base", "3-3 e:c-call"]);
    // Decision 30: coverage per harness, never one blended number.
    let per: Vec<_> = o
        .coverage
        .explained
        .iter()
        .map(|e| (e.harness, e.lines))
        .collect();
    assert_eq!(
        per,
        [
            (ContractHarness::ClaudeCode, 1),
            (ContractHarness::Codex, 1)
        ]
    );
}

#[tokio::test]
async fn no_recorded_edits_changed_lines_are_no_capture_and_the_rest_base() {
    let fx = Fx::new().await;
    fx.write("edit.txt", "one\nTWO\nthree\nfour\n");
    let o = fx.origins("edit.txt").await;
    assert_well_formed(&o);
    assert_eq!(
        shape(&o),
        ["1-1 base", "2-2 u:NoCapture", "3-3 base", "4-4 u:NoCapture"]
    );
    // An untouched tracked file is all base.
    let o = fx.origins("ren.txt").await;
    assert_eq!(shape(&o), ["1-4 base"]);
}

#[tokio::test]
async fn an_unknown_worktree_and_a_bad_path_are_errors_not_answers() {
    let fx = Fx::new().await;
    let err = get_line_origins_impl(&fx.state, &WorktreeId("nope".into()), "edit.txt")
        .await
        .unwrap_err();
    assert!(matches!(err, IpcError::NotFound), "{err:?}");
    for bad in ["../outside.txt", "/etc/passwd", "a/../../b", ""] {
        let err = fx.answer(bad).await.unwrap_err();
        assert!(
            matches!(err, IpcError::InvalidInput { .. }),
            "{bad:?}: {err:?}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_directory_cannot_lead_out_of_the_worktree() {
    let fx = Fx::new().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "top secret\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), fx.wt.join("link")).unwrap();
    let err = fx.answer("link/secret.txt").await.unwrap_err();
    assert!(matches!(err, IpcError::InvalidInput { .. }), "{err:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn an_untrusted_filter_driver_is_refused_and_never_run() {
    let fx = Fx::new().await;
    let marker = fx.repo.root.path().join("evil.marker");
    let script = fx.repo.root.path().join("evil.sh");
    write_executable_script(
        &script,
        &format!("echo \"$@\" >> '{}'\ncat\n", marker.display()),
    );
    fx.repo.git([
        "config",
        "filter.evil.clean",
        &format!("'{}' clean", script.display()),
    ]);
    fx.write(".gitattributes", "* filter=evil\n");
    // A same-length rewrite: git must hash the file to know it changed.
    fx.write("edit.txt", "one\nTWO\nthree\n");

    let err = fx.answer("edit.txt").await.unwrap_err();
    assert!(matches!(err, IpcError::UntrustedFilters { .. }), "{err:?}");
    assert!(!marker.exists(), "the untrusted driver ran");
}

// ---------------------------------------------------------------------------
// Commits between edits
// ---------------------------------------------------------------------------

/// vs-base (the worktree has a base branch): the merge-base does not move
/// when the agent commits on its branch, so both edits stay attributed.
#[tokio::test]
async fn a_commit_between_two_edits_keeps_both_when_there_is_a_base_branch() {
    let fx = Fx::new().await;
    fx.write("edit.txt", "one\nTWO\nthree\n");
    fx.record(
        &claude("s1", "t1", 10),
        claude_op("edit.txt", BASE_TEXT, 1, &["two"], &["TWO"]),
    );
    fx.git(&["add", "edit.txt"]);
    fx.git(&["commit", "-q", "-m", "agent commit"]);

    fx.write("edit.txt", "one\nTWO\nTHREE\n");
    fx.record(
        &claude("s1", "t2", 20),
        claude_op("edit.txt", "one\nTWO\nthree\n", 2, &["three"], &["THREE"]),
    );
    let o = fx.origins("edit.txt").await;
    assert_well_formed(&o);
    assert_eq!(shape(&o), ["1-1 base", "2-2 e:t1", "3-3 e:t2"]);
}

/// No base branch: the comparison is against HEAD, which the commit moved.
/// The first edit's line is now part of the base; the second is still the
/// agent's or `unknown`, and NEVER a wrong op.
#[tokio::test]
async fn a_commit_between_two_edits_without_a_base_never_names_a_wrong_op() {
    // The main checkout of a repository with a single branch has no base.
    let repo = TestRepo::with_initial_commit();
    repo.write("edit.txt", BASE_TEXT);
    repo.commit_all("base");
    let store = Arc::new(HookStore {
        inner: Arc::new(MemoryEventStore::new()),
        hook: Mutex::new(None),
    });
    let state =
        AppState::new(test_runner(), Arc::new(MemoryRepoStore::new())).with_events(store.clone());
    let added = add_repository_impl(&state, repo.path.to_str().unwrap())
        .await
        .unwrap();
    let wt_id = id_at(&state, &added.id, &repo.path.canonicalize().unwrap()).await;

    write_in(&repo.path, "edit.txt", b"one\nTWO\nthree\n");
    record_in(
        &store.inner,
        &wt_id,
        &claude("s1", "t1", 10),
        claude_op("edit.txt", BASE_TEXT, 1, &["two"], &["TWO"]),
    );
    repo.commit_all("agent commit");
    write_in(&repo.path, "edit.txt", b"one\nTWO\nTHREE\n");
    record_in(
        &store.inner,
        &wt_id,
        &claude("s1", "t2", 20),
        claude_op("edit.txt", "one\nTWO\nthree\n", 2, &["three"], &["THREE"]),
    );
    let o = match get_line_origins_impl(&state, &wt_id, "edit.txt")
        .await
        .unwrap()
    {
        LineOriginsResult::Ok { origins } => origins,
        other => panic!("{other:?}"),
    };
    assert_well_formed(&o);
    eprintln!("after a commit, no base branch: {:?}", shape(&o));
    // Line 1 never changed. Line 2 was written by t1 but is committed now:
    // `base` (HEAD has it) or `t1`, never `t2`. Line 3 is t2's or unknown.
    let kind = |line: u32| {
        o.runs
            .iter()
            .find(|r| r.start_line <= line && line <= r.end_line)
            .map(describe)
            .unwrap()
    };
    assert!(kind(1).ends_with("base"), "{}", kind(1));
    assert!(
        kind(2).ends_with("base") || kind(2).ends_with("e:t1") || kind(2).contains("u:"),
        "{}",
        kind(2)
    );
    assert!(
        kind(3).ends_with("e:t2") || kind(3).contains("u:"),
        "{}",
        kind(3)
    );
}

// ---------------------------------------------------------------------------
// Reads at three moments
// ---------------------------------------------------------------------------

/// An agent edit lands on disk AFTER the store was read: its line is not in
/// the records, so it must come out unknown, not base and not the old op.
#[tokio::test]
async fn an_edit_arriving_after_the_store_read_degrades_to_unknown() {
    let fx = Fx::new().await;
    fx.write("edit.txt", "one\nTWO\nthree\n");
    fx.record(
        &claude("s1", "t1", 10),
        claude_op("edit.txt", BASE_TEXT, 1, &["two"], &["TWO"]),
    );
    let (store, wt, wt_id) = (fx.store.inner.clone(), fx.wt.clone(), fx.wt_id.clone());
    *fx.store.hook.lock().unwrap() = Some(Box::new(move || {
        // The agent's next edit hits the disk, then its record arrives.
        write_in(&wt, "edit.txt", b"one\nTWO\nTHREE\n");
        record_in(
            &store,
            &wt_id,
            &claude("s1", "t2", 20),
            claude_op("edit.txt", "one\nTWO\nthree\n", 2, &["three"], &["THREE"]),
        );
    }));
    let o = fx.origins("edit.txt").await;
    assert_well_formed(&o);
    assert_eq!(shape(&o), ["1-1 base", "2-2 e:t1", "3-3 u:NoCapture"]);
    // A second request sees the record and attributes the line.
    let o = fx.origins("edit.txt").await;
    assert_eq!(shape(&o), ["1-1 base", "2-2 e:t1", "3-3 e:t2"]);
}

/// The opposite tear: a record that is newer than the file we read. The
/// replayed line is not on disk, so nothing may be attributed to it. (The
/// record is stored before the request; the working tree stays untouched.)
#[tokio::test]
async fn a_record_newer_than_the_file_attributes_nothing_to_it() {
    let fx = Fx::new().await;
    fx.record(
        &claude("s1", "t1", 10),
        claude_op("edit.txt", BASE_TEXT, 1, &["two"], &["TWO"]),
    );
    // Disk still has the base text.
    let o = fx.origins("edit.txt").await;
    assert_well_formed(&o);
    assert_eq!(shape(&o), ["1-1 base", "2-2 u:NoCapture", "3-3 base"]);
}

/// A UTF-8 file that git treats as binary (`-diff`) is not text for us either.
#[tokio::test]
async fn a_file_git_calls_binary_is_not_text() {
    let fx = Fx::new().await;
    fx.write(".gitattributes", "*.txt -diff\n");
    fx.write("edit.txt", "one\nTWO\nthree\n");
    assert_eq!(
        fx.answer("edit.txt").await.unwrap(),
        LineOriginsResult::Unavailable {
            reason: LineOriginsUnavailable::NotText
        }
    );
}

/// A path emptied and reused after a rename: the new file's history must not
/// be credited to the renamed one.
#[tokio::test]
async fn a_reused_old_path_does_not_lend_its_history_to_a_renamed_file() {
    let fx = Fx::new().await;
    fx.git(&["mv", "ren.txt", "renamed.txt"]);
    // A brand-new file at the old path, written by another call, whose first
    // line equals a line of the renamed file.
    fx.write("ren.txt", "alpha\nnew\n");
    fx.record(
        &claude("s1", "t0", 5),
        create_op("ren.txt", &["alpha", "new"]),
    );
    let o = fx.origins("renamed.txt").await;
    assert!(
        !shape(&o).iter().any(|s| s.contains("e:t0")),
        "{:?}",
        shape(&o)
    );
}

// ---------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------

/// 1,000 lines, 200 recorded edits, a real repository: the whole command,
/// git reads included. (The pure part is measured in the unit tests.)
#[tokio::test]
async fn timing_a_thousand_lines_and_two_hundred_edits_end_to_end() {
    let fx = Fx::new().await;
    let base: Vec<String> = (0..1000).map(|i| format!("let v{i} = {i};")).collect();
    let base_text = base.join("\n") + "\n";
    fx.repo.write("big.rs", &base_text);
    fx.repo.commit_all("big");
    // A new worktree commit is needed so `feature` sees the file at its base:
    // merge main into the feature branch's checkout.
    fx.git(&["merge", "-q", "main"]);

    let mut lines = base.clone();
    for k in 0..200usize {
        let at = (k * 5) % 1000;
        let before = lines.join("\n") + "\n";
        let old = lines[at].clone();
        let new = format!("let v{at} = {};", 10_000 + k);
        lines[at] = new.clone();
        // Different call ids per edit; `&'static str` needs leaking.
        let call: &'static str = Box::leak(format!("c{k}").into_boxed_str());
        fx.record(
            &claude("s1", call, 1_000 + k as u64),
            claude_op("big.rs", &before, at, &[old.as_str()], &[new.as_str()]),
        );
    }
    fx.write("big.rs", &(lines.join("\n") + "\n"));

    let start = Instant::now();
    let o = fx.origins("big.rs").await;
    let took = start.elapsed();
    eprintln!(
        "get_line_origins, 1000 lines, 200 edits, real repo: {took:?} (debug build: {})",
        cfg!(debug_assertions)
    );
    assert_well_formed(&o);
    assert_eq!(o.coverage.explained[0].lines, 200);
    assert!(took.as_secs() < 5, "{took:?}");
}
