//! WHY-64: the edit-op seam of the event stores, run against BOTH stores like
//! `conformance.rs` (same fixtures, same macro pattern), in its own file so the
//! v0.2 suite stays untouched.

use why_lib::contracts::{SessionSummary, SessionTimeline, ToolOutcome, WorktreeId};
use why_lib::events::{AppendOutcome, EventStore, NormalizedEvent, SessionKey, ToolClass};
use why_lib::provenance::{
    EditHunk, EditKind, EditOp, FileSnapshot, HunkLineHash, HunkLineKind, LineHasher, Observed,
    UnobservedReason,
};
use why_lib::store::StoreError;
use why_wire::Harness;

use crate::common::*;
use crate::conformance::{Fixture, memory, sqlite};

type Make = fn(why_lib::events::Clock) -> Fixture;

fn fresh(make: Make) -> Fixture {
    make(why_lib::events::Clock::fixed(1_000))
}

fn h() -> LineHasher {
    LineHasher::default()
}

fn op(wt: Option<&str>, path: &str, marker: &str) -> EditOp {
    let hasher = h();
    EditOp {
        worktree_id: wt.map(|w| WorktreeId(w.into())),
        path: path.into(),
        kind: EditKind::Update,
        renamed_from: None,
        before: Some(FileSnapshot::of_text(&hasher, b"a\nb\n")),
        after: Some(hasher.fingerprint(format!("a\n{marker}\n").as_bytes())),
        hunks: vec![EditHunk {
            old_start: Some(1),
            header: None,
            lines: vec![
                HunkLineHash {
                    kind: HunkLineKind::Context,
                    hash: hasher.hash_line(b"a"),
                },
                HunkLineHash {
                    kind: HunkLineKind::Removed,
                    hash: hasher.hash_line(b"b"),
                },
                HunkLineHash {
                    kind: HunkLineKind::Added,
                    hash: hasher.hash_line(marker.as_bytes()),
                },
            ],
        }],
        observed: Observed::Exact,
    }
}

/// A successful edit call (`PostToolUse`, class edit) carrying `ops`.
fn edit(session: &str, use_id: &str, at: u64, ops: Vec<EditOp>) -> NormalizedEvent {
    let mut ev = tool(session, use_id, Some(ToolOutcome::Ok), at, "p1");
    ev.tool.as_mut().unwrap().class = ToolClass::Edit;
    ev.tool.as_mut().unwrap().name = "Edit".into();
    ev.files = ops.iter().map(|o| o.path.clone()).collect();
    ev.edits = ops;
    ev
}

fn markers(store: &dyn EventStore, wt_id: &str, path: &str) -> Vec<String> {
    // Identify each record by its tool_use_id and op index.
    store
        .edit_records(&wt(wt_id), path)
        .unwrap()
        .into_iter()
        .map(|r| format!("{}#{}", r.source.tool_use_id.unwrap(), r.op_index))
        .collect()
}

fn duplicate_delivery_stores_ops_once(make: Make) {
    let f = fresh(make);
    let ev = edit("s1", "t1", 1, vec![op(Some("wt-1"), "a.rs", "x")]);
    f.store.append(std::slice::from_ref(&ev)).unwrap();
    f.store.append(&[ev.clone(), ev]).unwrap();
    assert_eq!(markers(&*f.store, "wt-1", "a.rs"), ["t1#0"]);
}

fn ops_round_trip_exactly_with_their_source(make: Make) {
    let f = fresh(make);
    let hasher = h();
    // A 1,000-line `before` and a Codex-style header-only locating hint.
    let text: String = (0..1000)
        .map(|i| format!("    let v{i} = {i};\r\n"))
        .collect();
    let mut big = op(Some("wt-1"), "src/big.rs", "x");
    big.before = Some(FileSnapshot::of_text(&hasher, text.as_bytes()));
    big.hunks[0].old_start = None;
    big.hunks[0].header = Some(hasher.hash_line(b"fn main() {"));
    big.renamed_from = Some("src/old.rs".into());
    let mut ev = edit("s1", "t1", 7, vec![big.clone()]);
    ev.agent_id = Some("agent-1".into());
    f.store.append(&[ev.clone()]).unwrap();

    let got = f.store.edit_records(&wt("wt-1"), "src/big.rs").unwrap();
    assert_eq!(got.len(), 1);
    let r = &got[0];
    assert_eq!(r.op, big, "the whole op, every hash in order");
    assert_eq!(r.op.before.as_ref().unwrap().lines.len(), 1000);
    assert!(r.op.before.as_ref().unwrap().is_consistent(&hasher));
    assert_eq!(r.source.session, SessionKey::new(Harness::ClaudeCode, "s1"));
    assert_eq!(r.source.turn_id.as_deref(), Some("p1"));
    assert_eq!(r.source.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(r.source.tool_use_id.as_deref(), Some("t1"));
    assert_eq!(r.source.event_key, ev.key);
    assert_eq!(r.at_ms, 7);
    assert_eq!(r.op_index, 0);
}

fn ops_are_visible_with_their_event_and_timelines_are_unchanged(make: Make) {
    let with_ops = fresh(make);
    let without = fresh(make);
    let ev = edit("s1", "t1", 1, vec![op(Some("wt-1"), "a.rs", "x")]);
    let mut bare = ev.clone();
    bare.edits.clear();
    let batch = |e: NormalizedEvent| vec![prompt("s1", 0, "p1", "go"), e, stop("s1", 2, "p1")];
    let a: AppendOutcome = with_ops.store.append(&batch(ev)).unwrap();
    let b = without.store.append(&batch(bare)).unwrap();
    assert_eq!(a, b, "ops change nothing in the append outcome");
    let key = SessionKey::new(Harness::ClaudeCode, "s1");
    let ta: Option<SessionTimeline> = with_ops.store.timeline(&key).unwrap();
    assert_eq!(ta, without.store.timeline(&key).unwrap());
    let sa: Vec<SessionSummary> = with_ops.store.sessions_for_worktree(&wt("wt-1")).unwrap();
    assert_eq!(
        sa,
        without.store.sessions_for_worktree(&wt("wt-1")).unwrap()
    );
    // And the event's ops are there as soon as the event is.
    assert_eq!(markers(&*with_ops.store, "wt-1", "a.rs"), ["t1#0"]);
}

fn the_same_path_in_two_worktrees_is_two_files(make: Make) {
    let f = fresh(make);
    f.store
        .append(&[
            edit("s1", "t1", 1, vec![op(Some("wt-1"), "a.rs", "x")]),
            edit("s1", "t2", 2, vec![op(Some("wt-2"), "a.rs", "y")]),
        ])
        .unwrap();
    assert_eq!(markers(&*f.store, "wt-1", "a.rs"), ["t1#0"]);
    assert_eq!(markers(&*f.store, "wt-2", "a.rs"), ["t2#0"]);
    assert!(markers(&*f.store, "wt-3", "a.rs").is_empty());
}

fn records_come_in_arrival_then_op_order(make: Make) {
    let f = fresh(make);
    // One patch editing a.rs twice and b.rs once, then a later delivery whose
    // timestamp is EARLIER: order is arrival, never time.
    f.store
        .append(&[edit(
            "s1",
            "t1",
            50,
            vec![
                op(Some("wt-1"), "a.rs", "1"),
                op(Some("wt-1"), "b.rs", "2"),
                op(Some("wt-1"), "a.rs", "3"),
            ],
        )])
        .unwrap();
    f.store
        .append(&[edit("s2", "t2", 10, vec![op(Some("wt-1"), "a.rs", "4")])])
        .unwrap();
    assert_eq!(markers(&*f.store, "wt-1", "a.rs"), ["t1#0", "t1#2", "t2#0"]);
    assert_eq!(markers(&*f.store, "wt-1", "b.rs"), ["t1#1"]);
    let seqs: Vec<u64> = f
        .store
        .edit_records(&wt("wt-1"), "a.rs")
        .unwrap()
        .iter()
        .map(|r| r.seq)
        .collect();
    assert!(seqs[0] == seqs[1] && seqs[1] < seqs[2], "{seqs:?}");
}

fn unobserved_ops_round_trip_without_content(make: Make) {
    let f = fresh(make);
    let mut reduced = op(Some("wt-1"), "a.rs", "x");
    reduced.observed = Observed::Unobserved(UnobservedReason::PayloadReduced);
    let mut unparsed = op(Some("wt-1"), "a.rs", "y");
    unparsed.observed = Observed::Unobserved(UnobservedReason::Unparsed);
    unparsed.hunks.clear();
    unparsed.before = None;
    unparsed.after = None;
    f.store
        .append(&[edit("s1", "t1", 1, vec![reduced, unparsed.clone()])])
        .unwrap();
    let got = f.store.edit_records(&wt("wt-1"), "a.rs").unwrap();
    assert_eq!(
        got[0].op.observed,
        Observed::Unobserved(UnobservedReason::PayloadReduced)
    );
    // Stored without the content it could not have observed.
    assert!(got[0].op.hunks.is_empty() && got[0].op.before.is_none() && got[0].op.after.is_none());
    assert_eq!(got[1].op, unparsed);
}

/// Documented on `EventStore::append`: an op that can never be looked up is
/// skipped, its event is still stored, and an op's worktree does not join
/// `changed_worktrees` (ingest's job).
fn unresolvable_ops_are_skipped_and_the_event_is_kept(make: Make) {
    let f = fresh(make);
    let out = f
        .store
        .append(&[edit(
            "s1",
            "t1",
            1,
            vec![
                op(None, "a.rs", "no worktree"),
                op(Some("wt-1"), "/abs/a.rs", "not canonical"),
                op(Some("wt-2"), "./src/../a.rs", "kept"),
            ],
        )])
        .unwrap();
    assert_eq!(out.inserted, 1);
    assert_eq!(
        out.changed_worktrees,
        vec![wt("wt-1")],
        "the EVENT's worktree only"
    );
    assert!(
        f.store
            .timeline(&SessionKey::new(Harness::ClaudeCode, "s1"))
            .unwrap()
            .is_some()
    );
    assert!(markers(&*f.store, "wt-1", "a.rs").is_empty());
    assert!(markers(&*f.store, "wt-1", "/abs/a.rs").is_empty());
    // Stored under its canonical spelling, keeping its original op index.
    assert_eq!(markers(&*f.store, "wt-2", "a.rs"), ["t1#2"]);
    assert!(markers(&*f.store, "wt-2", "./src/../a.rs").is_empty());
}

fn prune_removes_the_ops_with_their_session(make: Make) {
    let f = fresh(make);
    f.store
        .append(&[
            edit("old", "t1", 10, vec![op(Some("wt-1"), "a.rs", "x")]),
            edit("new", "t2", 500, vec![op(Some("wt-1"), "a.rs", "y")]),
        ])
        .unwrap();
    assert_eq!(f.store.prune_older_than(100).unwrap(), 1);
    assert_eq!(markers(&*f.store, "wt-1", "a.rs"), ["t2#0"]);
    // A redelivery of the pruned event is new again, ops included.
    f.store
        .append(&[edit("old", "t1", 10, vec![op(Some("wt-1"), "a.rs", "x")])])
        .unwrap();
    assert_eq!(markers(&*f.store, "wt-1", "a.rs"), ["t2#0", "t1#0"]);
}

// ---- WHY-65: the edited file's own worktree (decision 32) ------------------

use why_lib::events::EditedFile;

/// An edit call resolved to `cwd_wt` (the session's cwd) whose files ingest
/// attributed as `owned`: `(worktree, path)`.
fn edit_owned(
    session: &str,
    use_id: &str,
    at: u64,
    finished: bool,
    cwd_wt: &str,
    owned: &[(&str, &str)],
) -> NormalizedEvent {
    let mut ev = tool(
        session,
        use_id,
        finished.then_some(ToolOutcome::Ok),
        at,
        "p1",
    );
    ev.tool.as_mut().unwrap().class = ToolClass::Edit;
    ev.worktree_id = Some(wt(cwd_wt));
    ev.files = owned.iter().map(|(_, p)| (*p).to_owned()).collect();
    ev.edited_files = Some(
        owned
            .iter()
            .map(|(w, p)| EditedFile {
                worktree_id: wt(w),
                path: (*p).to_owned(),
            })
            .collect(),
    );
    ev
}

fn touch_labels(store: &dyn EventStore, wt_id: &str, paths: &[&str]) -> Vec<String> {
    let paths: Vec<String> = paths.iter().map(|p| (*p).to_owned()).collect();
    store
        .edit_calls(&wt(wt_id), &paths)
        .unwrap()
        .into_iter()
        .map(|t| {
            format!(
                "{}:{}:{:?}",
                t.session.session_id,
                t.paths.join("+"),
                t.outcome
            )
        })
        .collect()
}

fn session_ids(store: &dyn EventStore, wt_id: &str) -> Vec<String> {
    store
        .sessions_for_worktree(&wt(wt_id))
        .unwrap()
        .into_iter()
        .map(|s| s.id.0)
        .collect()
}

fn an_edited_file_lists_the_session_in_its_worktree_and_reports_it(make: Make) {
    let f = fresh(make);
    // Pre + Post of ONE call in worktree wt-1 (the cwd), editing wt-2's file.
    let pre = edit_owned("s1", "t1", 1, false, "wt-1", &[("wt-2", "src/a.rs")]);
    let post = edit_owned("s1", "t1", 2, true, "wt-1", &[("wt-2", "src/a.rs")]);
    let out = f.store.append(&[pre, post]).unwrap();
    assert_eq!(out.inserted, 2);
    assert_eq!(out.changed_worktrees, vec![wt("wt-1"), wt("wt-2")]);

    assert_eq!(session_ids(&*f.store, "wt-1"), ["claude-code:s1"]);
    assert_eq!(session_ids(&*f.store, "wt-2"), ["claude-code:s1"]);
    assert!(session_ids(&*f.store, "wt-3").is_empty());
    // The call is wt-2's; wt-1 (the cwd) has nothing of that name.
    assert_eq!(
        touch_labels(&*f.store, "wt-2", &["src/a.rs"]),
        ["s1:src/a.rs:Ok"]
    );
    assert!(touch_labels(&*f.store, "wt-1", &["src/a.rs"]).is_empty());
}

fn both_halves_are_found_even_when_the_finish_names_no_file(make: Make) {
    let f = fresh(make);
    // The finish (reduced payload) carries no files at all and sits in wt-1.
    let pre = edit_owned("s1", "t1", 1, false, "wt-1", &[("wt-2", "a.rs")]);
    let mut post = edit_owned("s1", "t1", 2, true, "wt-1", &[]);
    post.edited_files = Some(Vec::new());
    f.store.append(&[pre, post]).unwrap();
    assert_eq!(
        touch_labels(&*f.store, "wt-2", &["a.rs"]),
        ["s1:a.rs:Ok"],
        "the outcome comes from the finish found by tool_use_id"
    );
}

fn a_call_over_two_worktrees_lists_each_worktrees_own_files(make: Make) {
    let f = fresh(make);
    let files = [("wt-1", "one.rs"), ("wt-2", "two.rs"), ("wt-2", "three.rs")];
    f.store
        .append(&[
            edit_owned("s1", "t1", 1, false, "wt-1", &files),
            edit_owned("s1", "t1", 2, true, "wt-1", &files),
        ])
        .unwrap();
    assert_eq!(
        touch_labels(&*f.store, "wt-2", &["one.rs", "two.rs", "three.rs"]),
        ["s1:three.rs+two.rs:Ok"]
    );
    assert_eq!(
        touch_labels(&*f.store, "wt-1", &["one.rs", "two.rs"]),
        ["s1:one.rs:Ok"]
    );
}

fn the_same_relative_path_in_two_worktrees_keeps_two_call_lists(make: Make) {
    let f = fresh(make);
    f.store
        .append(&[
            edit_owned("s1", "t1", 1, true, "wt-1", &[("wt-1", "x.rs")]),
            edit_owned("s2", "t2", 2, true, "wt-2", &[("wt-2", "x.rs")]),
            edit_owned("s3", "t3", 3, true, "wt-1", &[("wt-2", "x.rs")]),
        ])
        .unwrap();
    assert_eq!(touch_labels(&*f.store, "wt-1", &["x.rs"]), ["s1:x.rs:Ok"]);
    let mut wt2 = touch_labels(&*f.store, "wt-2", &["x.rs"]);
    wt2.sort();
    assert_eq!(wt2, ["s2:x.rs:Ok", "s3:x.rs:Ok"]);
}

fn an_event_ingest_did_not_look_at_keeps_the_v02_rule(make: Make) {
    // `edited_files: None` (hand-built or adapter output): every file is the
    // event's own worktree's, exactly as in v0.2.
    let f = fresh(make);
    let mut ev = tool("s1", "t1", Some(ToolOutcome::Ok), 1, "p1");
    ev.tool.as_mut().unwrap().class = ToolClass::Edit;
    ev.files = vec!["./y.rs".into(), "/abs/outside.rs".into()];
    assert!(ev.edited_files.is_none());
    f.store.append(&[ev]).unwrap();
    assert_eq!(touch_labels(&*f.store, "wt-1", &["y.rs"]), ["s1:y.rs:Ok"]);
    assert!(touch_labels(&*f.store, "wt-2", &["y.rs"]).is_empty());
    assert_eq!(session_ids(&*f.store, "wt-1"), ["claude-code:s1"]);
}

fn prune_forgets_the_links_to_other_worktrees_too(make: Make) {
    let f = fresh(make);
    f.store
        .append(&[edit_owned("s1", "t1", 1, true, "wt-1", &[("wt-2", "a.rs")])])
        .unwrap();
    assert_eq!(f.store.prune_older_than(100).unwrap(), 1);
    assert!(session_ids(&*f.store, "wt-2").is_empty());
    assert!(session_ids(&*f.store, "wt-1").is_empty());
    // A later event of the same session starts from nothing.
    let out = f
        .store
        .append(&[edit_owned(
            "s1",
            "t9",
            500,
            true,
            "wt-1",
            &[("wt-1", "b.rs")],
        )])
        .unwrap();
    assert_eq!(out.changed_worktrees, vec![wt("wt-1")]);
    assert!(session_ids(&*f.store, "wt-2").is_empty());
}

macro_rules! edit_conformance {
    ($modname:ident, $make:path) => {
        mod $modname {
            use super::*;
            macro_rules! case {
                ($case:ident) => {
                    #[test]
                    fn $case() {
                        super::$case($make);
                    }
                };
            }
            case!(duplicate_delivery_stores_ops_once);
            case!(ops_round_trip_exactly_with_their_source);
            case!(ops_are_visible_with_their_event_and_timelines_are_unchanged);
            case!(the_same_path_in_two_worktrees_is_two_files);
            case!(records_come_in_arrival_then_op_order);
            case!(unobserved_ops_round_trip_without_content);
            case!(unresolvable_ops_are_skipped_and_the_event_is_kept);
            case!(prune_removes_the_ops_with_their_session);
            case!(an_edited_file_lists_the_session_in_its_worktree_and_reports_it);
            case!(both_halves_are_found_even_when_the_finish_names_no_file);
            case!(a_call_over_two_worktrees_lists_each_worktrees_own_files);
            case!(the_same_relative_path_in_two_worktrees_keeps_two_call_lists);
            case!(an_event_ingest_did_not_look_at_keeps_the_v02_rule);
            case!(prune_forgets_the_links_to_other_worktrees_too);
        }
    };
}

edit_conformance!(memory_store, memory);
edit_conformance!(sqlite_store, sqlite);

/// A store that implements only the required methods.
struct Minimal;

impl EventStore for Minimal {
    fn append(&self, _: &[NormalizedEvent]) -> Result<AppendOutcome, StoreError> {
        Ok(AppendOutcome::default())
    }
    fn sessions_for_worktree(&self, _: &WorktreeId) -> Result<Vec<SessionSummary>, StoreError> {
        Ok(Vec::new())
    }
    fn timeline(&self, _: &SessionKey) -> Result<Option<SessionTimeline>, StoreError> {
        Ok(None)
    }
    fn prune_older_than(&self, _: u64) -> Result<usize, StoreError> {
        Ok(0)
    }
}

#[test]
fn the_trait_default_for_edit_records_is_an_error_not_an_empty_list() {
    let err = Minimal.edit_records(&wt("wt-1"), "a.rs").unwrap_err();
    assert!(err.to_string().contains("edit_records"), "{err}");
}
