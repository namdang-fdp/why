//! Engine unit tests: hostile and edge cases, property tests, performance.
//! The ground-truth scenarios run in `tests/provenance` (the engine never
//! sees their oracle). The small model in [`model`] is this file's own
//! reference applier for random edits, written from the README semantics.

use std::time::{Duration, Instant};

use proptest::prelude::*;
use why_wire::Harness;

use super::*;
use crate::events::{EventKey, SessionKey};
use crate::provenance::edit::{
    EditHunk, EditKind, EditOp, HunkLineHash, HunkLineKind, Observed, UnobservedReason,
};
use crate::provenance::line::{FileFingerprint, LineHasher};

// ---- Builders ---------------------------------------------------------------

const H: LineHasher = LineHasher::unkeyed_v1();

fn hl(text: &str) -> LineHash {
    H.hash_line(text.as_bytes())
}

fn snap(lines: &[&str], final_newline: bool) -> FileSnapshot {
    FileSnapshot::of_lines(&H, lines.iter().map(|l| hl(l)).collect(), final_newline)
}

fn fp(lines: &[&str], final_newline: bool) -> FileFingerprint {
    snap(lines, final_newline).fingerprint
}

fn source(i: usize) -> EditSource {
    EditSource {
        session: SessionKey::new(Harness::ClaudeCode, "s"),
        turn_id: Some(format!("t{i}")),
        agent_id: None,
        tool_use_id: Some(format!("op{i}")),
        event_key: EventKey(format!("op{i}")),
    }
}

/// A hunk from patch-style lines (`" ctx"`, `"-old"`, `"+new"`).
fn hunk(old_start: Option<u32>, header: Option<&str>, lines: &[&str]) -> EditHunk {
    EditHunk {
        old_start,
        header: header.map(hl),
        lines: lines
            .iter()
            .map(|l| {
                let (kind, text) = match l.split_at(1) {
                    (" ", t) => (HunkLineKind::Context, t),
                    ("-", t) => (HunkLineKind::Removed, t),
                    ("+", t) => (HunkLineKind::Added, t),
                    _ => panic!("test bug: {l:?}"),
                };
                HunkLineHash {
                    kind,
                    hash: hl(text),
                }
            })
            .collect(),
    }
}

fn op(kind: EditKind, before: Option<FileSnapshot>, hunks: Vec<EditHunk>) -> EditOp {
    EditOp {
        worktree_id: None,
        path: "f.txt".into(),
        kind,
        renamed_from: None,
        before,
        after: None,
        hunks,
        observed: Observed::Exact,
    }
}

fn unobserved(reason: UnobservedReason) -> EditOp {
    EditOp {
        observed: Observed::Unobserved(reason),
        ..op(EditKind::Update, None, Vec::new())
    }
}

fn input(baseline: Option<FileSnapshot>, ops: Vec<EditOp>) -> ReplayInput {
    ReplayInput {
        baseline,
        records: ops
            .into_iter()
            .enumerate()
            .map(|(i, op)| EditRecord {
                source: source(i),
                seq: i as u64 + 1,
                op_index: 0,
                at_ms: 0,
                op,
            })
            .collect(),
    }
}

/// Short labels for origins: `base`, `e0`, `e1`, …, `?noCapture`, ….
fn labels(out: &ReplayOutput) -> Vec<String> {
    out.lines
        .iter()
        .map(|l| match &l.origin {
            LineOrigin::Base => "base".to_string(),
            LineOrigin::Edit(s) => format!("e{}", &s.event_key.0[2..]),
            LineOrigin::Unknown(r) => format!("?{r:?}"),
        })
        .collect()
}

fn hashes(out: &ReplayOutput) -> Vec<LineHash> {
    out.lines.iter().map(|l| l.hash).collect()
}

fn texts(lines: &[&str]) -> Vec<LineHash> {
    lines.iter().map(|l| hl(l)).collect()
}

// ---- Edge and hostile cases -------------------------------------------------

#[test]
fn no_records_answers_the_baseline_as_base() {
    let base = FileSnapshot::of_text(&H, b"a\nb\nc");
    let out = replay(&ReplayInput {
        baseline: Some(base.clone()),
        records: Vec::new(),
    });
    assert_eq!(out.final_newline, Some(false));
    assert_eq!(hashes(&out), base.lines);
    assert_eq!(labels(&out), ["base", "base", "base"]);
    let absent = replay(&ReplayInput::default());
    assert!(absent.lines.is_empty());
    assert_eq!(absent.final_newline, None, "absent, not empty");
}

#[test]
fn an_empty_baseline_is_not_an_absent_file() {
    let grow = || {
        op(
            EditKind::Update,
            Some(snap(&[], false)),
            vec![hunk(Some(0), None, &["+first"])],
        )
    };
    // Existing empty file: the edit's `before` matches, the line is e0, and
    // nothing observed says whether the grown file ends with a newline.
    let out = replay(&input(Some(snap(&[], false)), vec![grow()]));
    assert_eq!(labels(&out), ["e0"]);
    assert_eq!(out.final_newline, None, "lines and an unknown newline");
    // Absent file: `before` shows an (empty) file WHY did not know of; there
    // are no lines to be uncaptured, so the result is the same.
    let out = replay(&input(None, vec![grow()]));
    assert_eq!(labels(&out), ["e0"]);
    // An empty existing file, untouched, is `Some(false)`.
    let out = replay(&input(Some(snap(&[], false)), vec![]));
    assert_eq!((out.lines.len(), out.final_newline), (0, Some(false)));
    // A Codex update (no `before`) of an absent file has nowhere to go.
    let codex = op(EditKind::Update, None, vec![hunk(None, None, &["+x"])]);
    let report = replay_report(&input(None, vec![codex]));
    assert_eq!(
        report.unresolved.map(|u| u.reason),
        Some(UnknownReason::ReplayMismatch)
    );
    assert!(report.output.lines.is_empty());
}

#[test]
fn hunks_at_the_first_and_the_last_line() {
    let base = ["a", "b", "c"];
    let first = op(
        EditKind::Update,
        Some(snap(&base, true)),
        vec![hunk(Some(1), None, &["-a", "+A", " b"])],
    );
    // Pure insertion after the last line (old_start = line count).
    let last = op(
        EditKind::Update,
        Some(snap(&["A", "b", "c"], true)),
        vec![hunk(Some(3), None, &["+d"])],
    );
    // Remove the (new) last line, with context above it.
    let drop_last = op(
        EditKind::Update,
        Some(snap(&["A", "b", "c", "d"], true)),
        vec![hunk(Some(3), None, &[" c", "-d"])],
    );
    let top = op(
        EditKind::Update,
        Some(snap(&["A", "b", "c"], true)),
        vec![hunk(Some(0), None, &["+top"])],
    );
    let out = replay(&input(
        Some(snap(&base, true)),
        vec![first, last, drop_last, top],
    ));
    assert_eq!(hashes(&out), texts(&["top", "A", "b", "c"]));
    assert_eq!(labels(&out), ["e3", "e0", "base", "base"]);
    assert_eq!(out.final_newline, Some(true));
}

#[test]
fn an_old_start_past_the_end_is_never_shifted_to_fit() {
    let base = ["a", "b"];
    let past = op(
        EditKind::Update,
        Some(snap(&base, true)),
        vec![hunk(Some(5), None, &[" b", "+c"])],
    );
    let report = replay_report(&input(Some(snap(&base, true)), vec![past]));
    let unresolved = report.unresolved.expect("not applied");
    assert_eq!(unresolved.reason, UnknownReason::ReplayMismatch);
    assert_eq!(unresolved.since, source(0));
    // The output is the last known state, with its origins.
    assert_eq!(hashes(&report.output), texts(&base));
    assert_eq!(labels(&report.output), ["base", "base"]);
}

#[test]
fn removed_lines_that_do_not_match_make_the_op_unappliable() {
    let base = ["a", "b", "c"];
    // Says it removes `x` at line 2, where the file has `b`.
    let wrong = op(
        EditKind::Update,
        Some(snap(&base, true)),
        vec![hunk(Some(1), None, &[" a", "-x", "+y"])],
    );
    let report = replay_report(&input(Some(snap(&base, true)), vec![wrong]));
    assert_eq!(
        report.unresolved.map(|u| u.reason),
        Some(UnknownReason::ReplayMismatch)
    );
    // A later edit's `before` resumes: the line that changed while WHY was
    // blind gets the reason of the op that made it blind (ReplayMismatch).
    let wrong = op(
        EditKind::Update,
        Some(snap(&base, true)),
        vec![hunk(Some(1), None, &[" a", "-x", "+y"])],
    );
    let next = op(
        EditKind::Update,
        Some(snap(&["a", "y", "c"], true)),
        vec![hunk(Some(3), None, &["-c", "+C"])],
    );
    let out = replay(&input(Some(snap(&base, true)), vec![wrong, next]));
    assert_eq!(labels(&out), ["base", "?ReplayMismatch", "e1"]);
}

#[test]
fn a_before_that_differs_by_one_line_resyncs_only_that_line() {
    let base = ["a", "b", "c", "d"];
    let e0 = op(
        EditKind::Update,
        Some(snap(&base, true)),
        vec![hunk(Some(1), None, &[" a", "-b", "+B"])],
    );
    // Something uncaptured rewrote `d` to `D`.
    let e1 = op(
        EditKind::Update,
        Some(snap(&["a", "B", "c", "D"], true)),
        vec![hunk(Some(1), None, &["-a", "+A"])],
    );
    let out = replay(&input(Some(snap(&base, true)), vec![e0, e1]));
    assert_eq!(hashes(&out), texts(&["A", "B", "c", "D"]));
    assert_eq!(labels(&out), ["e1", "e0", "base", "?NoCapture"]);
}

#[test]
fn ten_unobserved_ops_in_a_row_keep_the_earliest_reason() {
    let base = ["a", "b", "c"];
    let mut ops = Vec::new();
    for k in 0..10 {
        ops.push(unobserved(if k % 2 == 0 {
            UnobservedReason::Unparsed
        } else {
            UnobservedReason::PayloadReduced
        }));
    }
    // While nothing reveals the file, the replay stays unresolved since op 0.
    let report = replay_report(&input(Some(snap(&base, true)), ops.clone()));
    let u = report.unresolved.expect("unknown state");
    assert_eq!((u.since, u.reason), (source(0), UnknownReason::Unparsed));
    // A Codex op (no `before`) cannot resume it, and is not applied.
    ops.push(op(
        EditKind::Update,
        None,
        vec![hunk(None, None, &["-a", "+z"])],
    ));
    let report = replay_report(&input(Some(snap(&base, true)), ops.clone()));
    assert_eq!(report.unresolved.map(|u| u.since), Some(source(0)));
    // A Claude op's `before` reveals the file: `b` was rewritten and `x`
    // added while WHY was blind; `a` and `c` provably survived.
    ops.push(op(
        EditKind::Update,
        Some(snap(&["a", "B", "x", "c"], true)),
        vec![hunk(Some(4), None, &["-c", "+C"])],
    ));
    let report = replay_report(&input(Some(snap(&base, true)), ops));
    assert!(report.unresolved.is_none());
    assert_eq!(
        labels(&report.output),
        ["base", "?Unparsed", "?Unparsed", "e11"]
    );
}

#[test]
fn crlf_and_lf_are_the_same_lines() {
    let lf = FileSnapshot::of_text(&H, b"a\nb\nc\n");
    let crlf = FileSnapshot::of_text(&H, b"a\r\nb\r\nc\r\n");
    assert_eq!(lf.lines, crlf.lines);
    // The baseline came from git as LF; the harness read the file as CRLF.
    // The `before` check passes: no line becomes `NoCapture`.
    let e0 = op(
        EditKind::Update,
        Some(crlf),
        vec![hunk(Some(1), None, &[" a", "-b", "+B"])],
    );
    let out = replay(&input(Some(lf), vec![e0]));
    assert_eq!(labels(&out), ["base", "e0", "base"]);
}

#[test]
fn a_damaged_before_is_ignored_not_trusted() {
    let base = ["a", "b"];
    let mut damaged = snap(&["zzz", "b"], true);
    damaged.fingerprint = fp(&["a", "b"], true); // does not describe its lines
    let e0 = op(
        EditKind::Update,
        Some(damaged),
        vec![hunk(Some(1), None, &[" a", "-b", "+c"])],
    );
    let out = replay(&input(Some(snap(&base, true)), vec![e0]));
    // Applied against the replayed state, which the hunk matches.
    assert_eq!(labels(&out), ["base", "e0"]);
}

#[test]
fn an_after_mismatch_trusts_nothing_of_that_op() {
    let base = ["a", "b"];
    let mut e0 = op(
        EditKind::Update,
        Some(snap(&base, true)),
        vec![hunk(Some(1), None, &[" a", "-b", "+c"])],
    );
    e0.after = Some(fp(&["a", "SOMETHING ELSE"], true));
    let report = replay_report(&input(Some(snap(&base, true)), vec![e0]));
    assert_eq!(
        report.unresolved.map(|u| u.reason),
        Some(UnknownReason::ReplayMismatch)
    );
    assert_eq!(hashes(&report.output), texts(&base), "the state before e0");
}

#[test]
fn an_after_fingerprint_sets_the_final_newline() {
    let base = ["a", "b"];
    let mut e0 = op(
        EditKind::Update,
        Some(snap(&base, false)),
        vec![hunk(Some(1), None, &[" a", "-b", "+c"])],
    );
    e0.after = Some(fp(&["a", "c"], true));
    let out = replay(&input(Some(snap(&base, false)), vec![e0]));
    assert_eq!(out.final_newline, Some(true));
}

#[test]
fn an_edit_strictly_inside_the_file_keeps_the_final_newline() {
    // Only the end of a file decides its final newline: hunks that neither
    // include the last line nor append after it provably keep the flag,
    // whichever it is.
    for flag in [true, false] {
        let base = ["a", "b", "c", "d"];
        let claude = op(
            EditKind::Update,
            Some(snap(&base, flag)),
            vec![
                hunk(Some(1), None, &["-a", "+A", " b"]),
                // Inserts after line 3, `c`: still above the last line.
                hunk(Some(3), None, &[" c", "+inserted"]),
            ],
        );
        let out = replay(&input(Some(snap(&base, flag)), vec![claude]));
        assert_eq!(hashes(&out), texts(&["A", "b", "c", "inserted", "d"]));
        assert_eq!(out.final_newline, Some(flag));
        // Codex, placed by context, the same.
        let out = replay(&input(
            Some(snap(&base, flag)),
            vec![codex(&["-b", "+B", " c"])],
        ));
        assert_eq!(out.final_newline, Some(flag));
    }
}

#[test]
fn an_edit_that_reaches_the_end_makes_the_final_newline_unknown() {
    let base = ["a", "b", "c"];
    let cases: Vec<(&str, EditOp)> = vec![
        // Rewrites the last line.
        (
            "rewrite last",
            op(
                EditKind::Update,
                Some(snap(&base, true)),
                vec![hunk(Some(2), None, &[" b", "-c", "+C"])],
            ),
        ),
        // Shows the last line as context only: still its end.
        (
            "context at end",
            op(
                EditKind::Update,
                Some(snap(&base, true)),
                vec![hunk(Some(1), None, &["-a", "+A", " b", " c"])],
            ),
        ),
        // Appends after the last line.
        (
            "append",
            op(
                EditKind::Update,
                Some(snap(&base, true)),
                vec![hunk(Some(3), None, &["+d"])],
            ),
        ),
        // Removes the last line.
        (
            "drop last",
            op(
                EditKind::Update,
                Some(snap(&base, true)),
                vec![hunk(Some(2), None, &[" b", "-c"])],
            ),
        ),
        // Codex, unique placement at the end.
        ("codex end", codex(&[" b", "-c", "+C"])),
    ];
    for (name, e0) in cases {
        let out = replay(&input(Some(snap(&base, true)), vec![e0]));
        assert!(!out.lines.is_empty(), "{name}");
        assert_eq!(out.final_newline, None, "{name}: lines, newline unknown");
    }
    // An ambiguous Codex hunk counts if ANY of its places reaches the end.
    let base = ["k", "v", "k", "v"];
    let out = replay(&input(
        Some(snap(&base, true)),
        vec![codex(&[" k", "-v", "+w"])],
    ));
    assert_eq!(out.final_newline, None);
    // A later `before` shows the flag again.
    let e1 = op(
        EditKind::Update,
        Some(snap(&["a", "b", "C"], false)),
        vec![hunk(Some(1), None, &["-a", "+A", " b"])],
    );
    let e0 = op(
        EditKind::Update,
        Some(snap(&["a", "b", "c"], true)),
        vec![hunk(Some(2), None, &[" b", "-c", "+C"])],
    );
    let out = replay(&input(Some(snap(&["a", "b", "c"], true)), vec![e0, e1]));
    assert_eq!(out.final_newline, Some(false));
}

#[test]
fn creates_and_deletes_define_the_whole_file() {
    let base = ["a", "b"];
    // A create on a file WHY thinks exists: every line is the create's.
    let create = op(
        EditKind::Create,
        None,
        vec![hunk(None, None, &["+a", "+x"])],
    );
    let out = replay(&input(Some(snap(&base, true)), vec![create]));
    assert_eq!(labels(&out), ["e0", "e0"]);
    assert_eq!(out.final_newline, None, "no `after`: newline unknown");
    // A create with no hunks is an existing empty file.
    let empty = op(EditKind::Create, None, vec![]);
    let out = replay(&input(None, vec![empty]));
    assert_eq!((out.lines.len(), out.final_newline), (0, Some(false)));
    // A delete after an unobserved op: the file is gone, nothing unknown.
    let report = replay_report(&input(
        Some(snap(&base, true)),
        vec![
            unobserved(UnobservedReason::PayloadReduced),
            op(EditKind::Delete, None, vec![]),
        ],
    ));
    assert!(report.unresolved.is_none());
    assert_eq!(report.output, ReplayOutput::default());
    // A create whose hunk shows a non-added line is not understood.
    let odd = op(EditKind::Create, None, vec![hunk(None, None, &[" a"])]);
    let report = replay_report(&input(None, vec![odd]));
    assert_eq!(
        report.unresolved.map(|u| u.reason),
        Some(UnknownReason::ReplayMismatch)
    );
}

#[test]
fn records_are_replayed_in_seq_order_whatever_the_input_order() {
    let base = ["a"];
    let mut inp = input(
        Some(snap(&base, true)),
        vec![
            op(
                EditKind::Update,
                Some(snap(&["a"], true)),
                vec![hunk(Some(1), None, &["-a", "+b"])],
            ),
            op(
                EditKind::Update,
                Some(snap(&["b"], true)),
                vec![hunk(Some(1), None, &["-b", "+c"])],
            ),
        ],
    );
    inp.records.swap(0, 1);
    let out = replay(&inp);
    assert_eq!(hashes(&out), texts(&["c"]));
    assert_eq!(labels(&out), ["e1"]);
}

// ---- Codex placement ----------------------------------------------------------

#[test]
fn codex_ambiguous_equal_size_marks_only_placement_dependent_lines() {
    let base = ["k", "v", "k", "v"];
    let p0 = op(
        EditKind::Update,
        None,
        vec![hunk(None, None, &[" k", "-v", "+w"])],
    );
    let report = replay_report(&input(Some(snap(&base, true)), vec![p0]));
    assert!(report.unresolved.is_none());
    assert_eq!(
        labels(&report.output),
        ["base", "?AmbiguousMatch", "base", "?AmbiguousMatch"]
    );
    // Each `v` line may still be `v` or now `w`.
    assert_eq!(hashes(&report.output), texts(&base));
    assert_eq!(
        report.alternatives,
        [
            LineAlternatives {
                index: 1,
                others: vec![hl("w")]
            },
            LineAlternatives {
                index: 3,
                others: vec![hl("w")]
            },
        ]
    );
}

#[test]
fn codex_header_narrows_and_a_missing_header_matches_nothing() {
    let base = ["fn a", "x", "fn b", "x"];
    let p0 = op(
        EditKind::Update,
        None,
        vec![hunk(None, Some("fn b"), &["-x", "+y"])],
    );
    let out = replay(&input(Some(snap(&base, true)), vec![p0]));
    assert_eq!(labels(&out), ["base", "base", "base", "e0"]);

    let p0 = op(
        EditKind::Update,
        None,
        vec![hunk(None, Some("fn zzz"), &["-x", "+y"])],
    );
    let report = replay_report(&input(Some(snap(&base, true)), vec![p0]));
    assert_eq!(
        report.unresolved.map(|u| u.reason),
        Some(UnknownReason::ReplayMismatch)
    );
}

#[test]
fn codex_count_changing_or_context_free_ambiguity_is_unresolved() {
    let base = ["k", "v", "k", "v"];
    // Fits twice and adds a line: every later line shifts by an unknown place.
    let grow = op(
        EditKind::Update,
        None,
        vec![hunk(None, None, &[" k", "+new", " v"])],
    );
    let report = replay_report(&input(Some(snap(&base, true)), vec![grow]));
    assert_eq!(
        report.unresolved.map(|u| u.reason),
        Some(UnknownReason::AmbiguousMatch)
    );
    // Only `+` lines: fits everywhere.
    let bare = op(EditKind::Update, None, vec![hunk(None, None, &["+x"])]);
    let report = replay_report(&input(Some(snap(&base, true)), vec![bare]));
    assert_eq!(
        report.unresolved.map(|u| u.reason),
        Some(UnknownReason::AmbiguousMatch)
    );
    // ... except in an empty file, where there is exactly one place.
    let bare = op(EditKind::Update, None, vec![hunk(None, None, &["+x"])]);
    let out = replay(&input(Some(snap(&[], false)), vec![bare]));
    assert_eq!(labels(&out), ["e0"]);
}

#[test]
fn codex_multi_hunk_order_removes_impossible_placements() {
    // Hunk 1 `-x +y` alone fits lines 2 and 4; hunk 2 `-z +w` fits only line
    // 3. Hunks are in file order, so hunk 1 must be at line 2.
    let base = ["a", "x", "z", "x"];
    let p0 = op(
        EditKind::Update,
        None,
        vec![
            hunk(None, None, &["-x", "+y"]),
            hunk(None, None, &["-z", "+w"]),
        ],
    );
    let out = replay(&input(Some(snap(&base, true)), vec![p0]));
    assert_eq!(hashes(&out), texts(&["a", "y", "w", "x"]));
    assert_eq!(labels(&out), ["base", "e0", "e0", "base"]);
}

fn codex(lines: &[&str]) -> EditOp {
    op(EditKind::Update, None, vec![hunk(None, None, lines)])
}

#[test]
fn an_ambiguous_hunk_over_uncertain_lines_is_unresolved() {
    let base = ["k", "v", "k", "v", "end"];
    // The real file now has exactly one `w`, at line 2 or line 4.
    // `-w +W` could go on either: a placement of a placement, and `W`'s
    // twin would keep showing `w` as possible after the only `w` is gone.
    let report = replay_report(&input(
        Some(snap(&base, true)),
        vec![codex(&[" k", "-v", "+w"]), codex(&["-w", "+W"])],
    ));
    let u = report.unresolved.expect("not modelled: unknown");
    assert_eq!(
        (u.since, u.reason),
        (source(1), UnknownReason::AmbiguousMatch)
    );
    // The last known state is the one after p0, with its uncertainty.
    assert_eq!(
        labels(&report.output),
        ["base", "?AmbiguousMatch", "base", "?AmbiguousMatch", "base"]
    );
    assert_eq!(report.alternatives.len(), 2);
}

#[test]
fn placements_stay_linked_across_later_hunks() {
    // `-x +y` on `[x, x]` gives `[y, x]` or `[x, y]`, never `[y, y]`.
    let base = ["x", "x"];
    let p0 = || codex(&["-x", "+y"]);
    // Each line alone may be `y`, but not both: no state allows this hunk.
    let report = replay_report(&input(
        Some(snap(&base, true)),
        vec![p0(), codex(&["-y", "-y", "+z"])],
    ));
    assert_eq!(
        report.unresolved.map(|u| u.reason),
        Some(UnknownReason::ReplayMismatch)
    );
    // `-y +z` fits either line on its own: ambiguous over uncertain lines.
    let report = replay_report(&input(
        Some(snap(&base, true)),
        vec![p0(), codex(&["-y", "+z"])],
    ));
    assert_eq!(
        report.unresolved.map(|u| u.reason),
        Some(UnknownReason::AmbiguousMatch)
    );
}

#[test]
fn a_unique_hunk_over_an_uncertain_line_narrows_its_twins() {
    // `[a, x, b, x]`, `-x +y` fits lines 2 and 4. Then ` a -y +Y` can only
    // be at the top (only line 1 is `a`), so line 2 WAS `y`: the patch went
    // there, and line 4 is still the base `x`.
    let base = ["a", "x", "b", "x"];
    let report = replay_report(&input(
        Some(snap(&base, true)),
        vec![codex(&["-x", "+y"]), codex(&[" a", "-y", "+Y"])],
    ));
    assert!(report.unresolved.is_none());
    assert_eq!(hashes(&report.output), texts(&["a", "Y", "b", "x"]));
    assert_eq!(labels(&report.output), ["base", "e1", "base", "base"]);
    assert!(report.alternatives.is_empty(), "nothing is uncertain now");
}

#[test]
fn a_before_that_shows_the_placement_resolves_the_origins() {
    let base = ["x", "x"];
    let p0 = codex(&["-x", "+y"]);
    // A later Claude edit read the file: `y` is on line 2.
    let e1 = op(
        EditKind::Update,
        Some(snap(&["x", "y"], true)),
        vec![hunk(Some(0), None, &["+top"])],
    );
    let out = replay(&input(Some(snap(&base, true)), vec![p0, e1]));
    assert_eq!(labels(&out), ["e1", "base", "e0"]);
    // A `before` no placement allows (`[y, y]`) is an uncaptured change:
    // rule 5, and the lines it cannot match are `NoCapture`.
    let e1 = op(
        EditKind::Update,
        Some(snap(&["y", "y"], true)),
        vec![hunk(Some(0), None, &["+top"])],
    );
    let out = replay(&input(
        Some(snap(&base, true)),
        vec![codex(&["-x", "+y"]), e1],
    ));
    assert_eq!(labels(&out)[0], "e1");
    assert!(labels(&out)[1..].iter().all(|l| l.starts_with('?')));
}

#[test]
fn several_independent_ambiguous_hunks_in_one_patch() {
    // Each hunk fits two places, and every place of the first lies above
    // every place of the second: four valid combinations, merged per hunk.
    let base = ["x", "x", "sep", "y", "y"];
    let p0 = op(
        EditKind::Update,
        None,
        vec![
            hunk(None, None, &["-x", "+X"]),
            hunk(None, None, &["-y", "+Y"]),
        ],
    );
    let report = replay_report(&input(Some(snap(&base, true)), vec![p0]));
    assert!(report.unresolved.is_none());
    assert_eq!(
        labels(&report.output),
        [
            "?AmbiguousMatch",
            "?AmbiguousMatch",
            "base",
            "?AmbiguousMatch",
            "?AmbiguousMatch"
        ]
    );
    let lines_with_alternatives: Vec<usize> = report.alternatives.iter().map(|a| a.index).collect();
    assert_eq!(lines_with_alternatives, [0, 1, 3, 4]);
    // Interleaved places (the hunks' choices constrain each other) are not
    // modelled: unknown.
    let base = ["x", "y", "x", "y"];
    let p0 = op(
        EditKind::Update,
        None,
        vec![
            hunk(None, None, &["-x", "+X"]),
            hunk(None, None, &["-y", "+Y"]),
        ],
    );
    let report = replay_report(&input(Some(snap(&base, true)), vec![p0]));
    assert_eq!(
        report.unresolved.map(|u| u.reason),
        Some(UnknownReason::AmbiguousMatch)
    );
}

#[test]
fn too_many_places_is_unresolved_not_merged() {
    // 100 identical lines: `-x +y` fits 100 places (over MAX_PLACEMENTS).
    let base = vec!["x"; 100];
    let report = replay_report(&input(Some(snap(&base, true)), vec![codex(&["-x", "+y"])]));
    assert_eq!(
        report.unresolved.map(|u| u.reason),
        Some(UnknownReason::AmbiguousMatch)
    );
}

/// Found by `codex_edits_never_guess_a_placement` when the engine was
/// deliberately broken to take the first match (as Codex itself does): kept
/// as an explicit case instead of a proptest seed file in the checkout.
#[test]
fn regression_an_ambiguous_identity_replacement_is_not_guessed() {
    let base = ["w", "x", "x", "w"];
    let out = replay(&input(Some(snap(&base, true)), vec![codex(&["-w", "+w"])]));
    // Same text either way, but WHICH `w` the patch rewrote decides the
    // origin: both `w` lines are e0 under one placement and base under the
    // other.
    assert_eq!(hashes(&out), texts(&base));
    assert_eq!(
        labels(&out),
        ["?AmbiguousMatch", "base", "base", "?AmbiguousMatch"]
    );
}

/// Found by `codex_edits_never_guess_a_placement` (a real engine bug, fixed):
/// overlapping places. `-w -w +w +w  w` fits `[w w w w]` at lines 1 and 2.
/// Line 2 is rewritten by the op under BOTH placements, so it is certainly
/// e0, not the untouched base line it replaces.
#[test]
fn regression_a_line_every_placement_rewrites_is_the_ops() {
    let base = ["w", "w", "w", "w"];
    let out = replay(&input(
        Some(snap(&base, true)),
        vec![codex(&["-w", "-w", "+w", "+w", " w"])],
    ));
    assert_eq!(hashes(&out), texts(&base));
    assert_eq!(
        labels(&out),
        // Line 4 is context under one placement, untouched under the other:
        // base either way.
        ["?AmbiguousMatch", "e0", "?AmbiguousMatch", "base"]
    );
}

// ---- Property tests -------------------------------------------------------------

/// This file's own reference applier: a file as (text, origin label) pairs.
mod model {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Tag {
        Base,
        Op(usize),
    }

    pub type File = Vec<(String, Tag)>;

    /// One single-hunk edit, described by where it really happens.
    #[derive(Debug, Clone)]
    pub struct Edit {
        pub start: usize,
        pub removed: usize,
        pub added: Vec<String>,
        pub before_ctx: usize,
        pub after_ctx: usize,
    }

    impl Edit {
        /// Clamp a random edit into the current file.
        pub fn fit(mut self, len: usize) -> Self {
            self.start = self.start.min(len);
            self.removed = self.removed.min(len - self.start);
            self.before_ctx = self.before_ctx.min(self.start);
            self.after_ctx = self.after_ctx.min(len - self.start - self.removed);
            self
        }

        pub fn apply(&self, file: &mut File, op: usize) {
            let new: Vec<(String, Tag)> = self
                .added
                .iter()
                .map(|t| (t.clone(), Tag::Op(op)))
                .collect();
            file.splice(self.start..self.start + self.removed, new);
        }

        /// The hunk as patch lines (context, removed, added, context).
        pub fn patch(&self, file: &File) -> Vec<String> {
            let mut out = Vec::new();
            for (t, _) in &file[self.start - self.before_ctx..self.start] {
                out.push(format!(" {t}"));
            }
            for (t, _) in &file[self.start..self.start + self.removed] {
                out.push(format!("-{t}"));
            }
            for t in &self.added {
                out.push(format!("+{t}"));
            }
            let end = self.start + self.removed;
            for (t, _) in &file[end..end + self.after_ctx] {
                out.push(format!(" {t}"));
            }
            out
        }

        /// Unified-diff `old_start` of the hunk in the file before it.
        pub fn old_start(&self) -> u32 {
            let old_len = self.before_ctx + self.removed + self.after_ctx;
            let s = if old_len == 0 {
                self.start
            } else {
                self.start - self.before_ctx + 1
            };
            u32::try_from(s).unwrap()
        }
    }
}

use model::{Edit, Tag};

/// A tiny alphabet on purpose: duplicates everywhere, so content equality
/// is a constant trap (rule 1).
fn word() -> impl Strategy<Value = String> {
    prop_oneof![Just("x"), Just("y"), Just("z"), Just("w")].prop_map(str::to_string)
}

fn edit() -> impl Strategy<Value = Edit> {
    (
        0usize..12,
        0usize..3,
        prop::collection::vec(word(), 0..3),
        0usize..3,
        0usize..3,
    )
        .prop_map(|(start, removed, added, before_ctx, after_ctx)| Edit {
            start,
            removed,
            added,
            before_ctx,
            after_ctx,
        })
}

fn file_of(model: &model::File) -> Vec<&str> {
    model.iter().map(|(t, _)| t.as_str()).collect()
}

fn hunk_of(e: &Edit, model: &model::File, positioned: bool) -> EditHunk {
    let patch = e.patch(model);
    let patch: Vec<&str> = patch.iter().map(String::as_str).collect();
    hunk(positioned.then(|| e.old_start()), None, &patch)
}

fn check_against_model(out: &ReplayOutput, model: &model::File) -> Result<(), TestCaseError> {
    prop_assert_eq!(hashes(out), texts(&file_of(model)));
    for (line, (_, tag)) in out.lines.iter().zip(model) {
        let want = match tag {
            Tag::Base => LineOrigin::Base,
            Tag::Op(i) => LineOrigin::Edit(source(*i)),
        };
        prop_assert_eq!(&line.origin, &want);
    }
    Ok(())
}

proptest! {
    // No seed files: a failing case must not write into the checkout (test
    // isolation). A case worth keeping becomes an explicit `regression_*`
    // test instead.
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Faithful Claude edits (true `before` and `old_start`): (a) the replay
    /// reproduces the text, (b) every origin is exactly the op that wrote
    /// the line (so never one that did not), (c) determinism.
    #[test]
    fn claude_edits_replay_exactly(
        base in prop::collection::vec(word(), 0..10),
        edits in prop::collection::vec(edit(), 0..12),
    ) {
        let mut model: model::File = base.iter().map(|t| (t.clone(), Tag::Base)).collect();
        let base_refs: Vec<&str> = base.iter().map(String::as_str).collect();
        let mut ops = Vec::new();
        for (i, e) in edits.into_iter().enumerate() {
            let e = e.fit(model.len());
            let before = snap(&file_of(&model), true);
            ops.push(op(EditKind::Update, Some(before), vec![hunk_of(&e, &model, true)]));
            e.apply(&mut model, i);
        }
        let inp = input(Some(snap(&base_refs, true)), ops);
        let report = replay_report(&inp);
        prop_assert!(report.unresolved.is_none());
        check_against_model(&report.output, &model)?;
        prop_assert_eq!(replay_report(&inp), report, "deterministic");
    }

    /// Claude edits with uncaptured rewrites and unobserved ops mixed in.
    /// Exactness is impossible (rule 5 matches by content), but (b) holds in
    /// its weak form: a line attributed to op k has a text op k really added,
    /// a `Base` line a text of the base, and no unobserved op is an origin.
    #[test]
    fn claude_edits_with_uncaptured_changes_never_invent_an_author(
        base in prop::collection::vec(word(), 0..10),
        steps in prop::collection::vec((edit(), 0u8..4, edit()), 0..10),
    ) {
        let mut model: model::File = base.iter().map(|t| (t.clone(), Tag::Base)).collect();
        let base_refs: Vec<&str> = base.iter().map(String::as_str).collect();
        let mut ops = Vec::new();
        let mut added: Vec<Vec<LineHash>> = Vec::new();
        for (e, interference, noise) in steps {
            match interference {
                // An uncaptured change right before this edit.
                0 => noise.fit(model.len()).apply(&mut model, usize::MAX),
                // An unobserved op that really did something.
                1 => {
                    noise.fit(model.len()).apply(&mut model, usize::MAX);
                    ops.push(unobserved(UnobservedReason::PayloadReduced));
                    added.push(Vec::new());
                }
                _ => {}
            }
            let i = ops.len(); // the index of the edit pushed below
            let e = e.fit(model.len());
            let before = snap(&file_of(&model), true);
            ops.push(op(EditKind::Update, Some(before), vec![hunk_of(&e, &model, true)]));
            added.push(texts(&e.added.iter().map(String::as_str).collect::<Vec<_>>()));
            e.apply(&mut model, i);
        }
        let base_hashes = texts(&base_refs);
        let inp = input(Some(snap(&base_refs, true)), ops.clone());
        let out = replay(&inp);
        prop_assert_eq!(hashes(&out), texts(&file_of(&model)));
        for line in &out.lines {
            match &line.origin {
                LineOrigin::Base => prop_assert!(base_hashes.contains(&line.hash)),
                LineOrigin::Edit(s) => {
                    let k: usize = s.event_key.0[2..].parse().unwrap();
                    prop_assert_eq!(ops[k].observed, Observed::Exact);
                    prop_assert!(added[k].contains(&line.hash));
                }
                LineOrigin::Unknown(_) => {}
            }
        }
        prop_assert_eq!(replay(&inp), out, "deterministic");
    }

    /// Codex edits (no positions, no `before`) on files full of duplicated
    /// context: an origin that is not `AmbiguousMatch` is the true one, and
    /// the true text is always among a line's possible texts.
    #[test]
    fn codex_edits_never_guess_a_placement(
        base in prop::collection::vec(word(), 1..10),
        edits in prop::collection::vec(edit(), 1..5),
    ) {
        let mut model: model::File = base.iter().map(|t| (t.clone(), Tag::Base)).collect();
        let base_refs: Vec<&str> = base.iter().map(String::as_str).collect();
        let mut ops = Vec::new();
        for e in edits {
            let mut e = e.fit(model.len());
            // Equal-size replacements with at least one line removed, so
            // rule 7 is defined (count-changing ones become unresolved).
            e.added.resize(e.removed, "w".to_string());
            if e.removed == 0 {
                continue;
            }
            ops.push(op(EditKind::Update, None, vec![hunk_of(&e, &model, false)]));
            e.apply(&mut model, ops.len() - 1);
        }
        let inp = input(Some(snap(&base_refs, true)), ops);
        let report = replay_report(&inp);
        // One equal-size hunk per op, made from the real file: the true
        // placement is always a candidate (and jointly consistent), so the
        // engine is never left with NO placement. It may stop with
        // `AmbiguousMatch` when a later hunk is ambiguous over lines an
        // earlier one left uncertain (not modelled); then `output` is a
        // last known state, not the final file, and is not compared.
        if let Some(u) = &report.unresolved {
            prop_assert_eq!(u.reason, UnknownReason::AmbiguousMatch);
        } else {
            let out = &report.output;
            prop_assert_eq!(out.lines.len(), model.len());
            for (j, (line, (text, tag))) in out.lines.iter().zip(&model).enumerate() {
                let possible: Vec<LineHash> = std::iter::once(line.hash)
                    .chain(report.alternatives.iter().filter(|a| a.index == j).flat_map(|a| a.others.clone()))
                    .collect();
                prop_assert!(possible.contains(&hl(text)), "line {j}: true text not possible");
                match &line.origin {
                    LineOrigin::Unknown(UnknownReason::AmbiguousMatch) => {}
                    LineOrigin::Base => prop_assert_eq!(tag, &Tag::Base, "line {}", j),
                    LineOrigin::Edit(s) => prop_assert_eq!(s, &source(match tag {
                        Tag::Op(k) => *k,
                        Tag::Base => usize::MAX,
                    }), "line {}", j),
                    other => prop_assert!(false, "line {j}: unexpected {other:?}"),
                }
            }
        }
        prop_assert_eq!(replay_report(&inp), report, "deterministic");
    }
}

// ---- Performance ------------------------------------------------------------------

/// The brief's budget: a 1,000-line file with 200 Claude edits in < 50 ms.
#[test]
fn a_thousand_lines_and_two_hundred_edits_replay_fast() {
    let mut text: Vec<String> = (0..1000).map(|i| format!("line {i}")).collect();
    let base = snap(&text.iter().map(String::as_str).collect::<Vec<_>>(), true);
    let mut ops = Vec::new();
    for k in 0..200 {
        let at = (k * 37) % 990 + 1; // 1-based line, spread over the file
        let before = snap(&text.iter().map(String::as_str).collect::<Vec<_>>(), true);
        let ctx = text[at - 1].clone();
        let old = text[at].clone();
        let new = format!("edited {k}");
        ops.push(op(
            EditKind::Update,
            Some(before),
            vec![hunk(
                Some(u32::try_from(at).unwrap()),
                None,
                &[&format!(" {ctx}"), &format!("-{old}"), &format!("+{new}")],
            )],
        ));
        text[at] = new;
    }
    let inp = input(Some(base), ops);
    let start = Instant::now();
    let out = replay(&inp);
    let took = start.elapsed();
    eprintln!("1,000 lines x 200 edits: {took:?}");
    assert_eq!(out.lines.len(), 1000);
    // The 50 ms budget is a release-build number (measured ~4 ms). A debug
    // build of this crate runs the same work in ~40 ms on an idle machine,
    // too close to the limit under a parallel `pnpm test`, so it only has to
    // stay within 10x.
    let budget = if cfg!(debug_assertions) { 500 } else { 50 };
    assert!(took < Duration::from_millis(budget), "took {took:?}");
}

#[test]
fn a_million_line_baseline_is_linear() {
    let lines: Vec<String> = (0..1_000_000).map(|i| format!("line {i}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let base = snap(&refs, true);
    // One faithful edit in the middle, then one whose `before` shows a line
    // rewritten behind WHY's back (forces the rule-5 matcher on 1M lines).
    let mut after_e0 = refs.clone();
    after_e0[500_000] = "edited";
    let mut drifted = after_e0.clone();
    drifted[10] = "uncaptured";
    let ops = vec![
        op(
            EditKind::Update,
            Some(base.clone()),
            vec![hunk(Some(500_001), None, &["-line 500000", "+edited"])],
        ),
        op(
            EditKind::Update,
            Some(snap(&drifted, true)),
            vec![hunk(Some(1), None, &["-line 0", "+first"])],
        ),
    ];
    let inp = input(Some(base), ops);
    let start = Instant::now();
    let out = replay(&inp);
    let took = start.elapsed();
    eprintln!("1,000,000-line baseline, 2 edits (one resync): {took:?}");
    assert_eq!(out.lines.len(), 1_000_000);
    let labels = labels(&out);
    assert_eq!(labels[0], "e1");
    assert_eq!(labels[10], "?NoCapture");
    assert_eq!(labels[500_000], "e0");
    assert_eq!(labels[999_999], "base");
    // Generous for a debug build; quadratic work would take hours.
    assert!(took < Duration::from_secs(30), "took {took:?}");
}

/// The rule-5 matcher's worst case, measured end to end through the engine
/// (see `matcher::tests::pathological`).
#[test]
fn the_pathological_resync_is_bounded_and_measured() {
    // Unbounded, doubling n quadrupled the time (release: ~9 ms, 36 ms,
    // 112 ms, 455 ms for 1,000 to 8,000 lines). With the matcher's budget
    // the work is linear; past it, lines stay `NoCapture` instead.
    for n in [1_000usize, 4_000, 20_000] {
        let (old, new) = super::matcher::tests::pathological(n);
        let baseline = FileSnapshot::of_lines(&H, old, true);
        let before = FileSnapshot::of_lines(&H, new, true);
        let inp = input(
            Some(baseline),
            vec![op(EditKind::Update, Some(before), vec![])],
        );
        let start = Instant::now();
        let out = replay(&inp);
        let took = start.elapsed();
        let labels = labels(&out);
        let unknown = labels.iter().filter(|l| l.starts_with('?')).count();
        eprintln!(
            "pathological resync, {} lines: {took:?}, {unknown} unknown",
            out.lines.len()
        );
        // Every line is base or `NoCapture`: the budget never invents.
        assert!(labels.iter().all(|l| l == "base" || l == "?NoCapture"));
        assert!(unknown >= 1, "the changed `Z'` line");
        assert!(took < Duration::from_secs(10), "took {took:?}");
    }
}
