//! Ground truth for line provenance (WHY-64 format, WHY-21 scenarios, WHY-22
//! engine). See `scenarios/README.md`.

mod truth;

use std::path::{Path, PathBuf};

use why_lib::provenance::{LineOrigin, ReplayOutput, ReplayedLine, UnknownReason, replay};

use truth::{Scenario, Stage};

fn scenario_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/provenance/scenarios")
}

fn all_scenarios() -> Vec<Scenario> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(scenario_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|p| Scenario::load(p).unwrap_or_else(|e| panic!("{e}")))
        .collect()
}

#[test]
fn every_scenario_in_the_folder_loads_and_validates() {
    let all = all_scenarios();
    assert!(all.len() >= 3, "the three WHY-64 examples at least");
    let mut names: Vec<&str> = all.iter().map(Scenario::name).collect();
    names.sort();
    let before = names.len();
    names.dedup();
    assert_eq!(names.len(), before, "scenario names are unique");
}

/// The engine sees hashes only, and none of the truth-only fields.
#[test]
fn inputs_carry_hashes_and_no_truth_only_positions() {
    let all = all_scenarios();
    let codex = all
        .iter()
        .find(|s| s.name() == "codex-ambiguous-context")
        .unwrap();
    let (input, final_lines) = codex.inputs();
    assert_eq!(input.baseline.as_ref().unwrap().lines.len(), 7);
    assert_eq!(final_lines.len(), 7);
    let op = &input.records[0].op;
    assert!(
        op.hunks.iter().all(|h| h.old_start.is_none()),
        "appliedAt is not passed"
    );
    assert!(op.before.is_none() && op.after.is_none());
    assert_eq!(input.records[0].source.event_key.0, "truth:p1");

    let shifted = all
        .iter()
        .find(|s| s.name() == "claude-shifted-edit")
        .unwrap();
    let (input, _) = shifted.inputs();
    let seqs: Vec<u64> = input.records.iter().map(|r| r.seq).collect();
    assert_eq!(seqs, [1, 2]);
    // `before: "auto"` is the FULL replayed state before the op.
    let second_before = input.records[1].op.before.as_ref().unwrap();
    assert_eq!(second_before.lines.len(), 7);
    assert!(input.records[1].op.after.is_some(), "after: auto");
}

#[test]
fn check_reports_a_readable_line_by_line_diff() {
    let all = all_scenarios();
    let plain = all
        .iter()
        .find(|s| s.name() == "claude-plain-edit")
        .unwrap();
    let (input, _) = plain.inputs();
    let err = plain.check(&replay(&input)).unwrap_err().to_string();
    assert!(err.contains("claude-plain-edit"), "{err}");
    assert!(err.contains("line    2: expected op e1"), "{err}");
    assert!(err.contains("let x = 2;"), "shows the line text: {err}");

    // And a correct answer passes.
    let (_, final_lines) = plain.inputs();
    let mut good = ReplayOutput {
        lines: final_lines
            .iter()
            .map(|&hash| ReplayedLine {
                hash,
                origin: LineOrigin::Base,
            })
            .collect(),
        final_newline: Some(true),
    };
    good.lines[1].origin = LineOrigin::Edit(input.records[0].source.clone());
    plain.check(&good).unwrap();
    // A wrong reason is a mismatch, not "close enough".
    let mut wrong = good.clone();
    wrong.lines[1].origin = LineOrigin::Unknown(UnknownReason::NoCapture);
    assert!(plain.check(&wrong).is_err());
    // Right origins on the wrong CONTENT is a mismatch too (replay stage).
    let mut moved = good.clone();
    moved.lines.swap(0, 2);
    moved.lines[0].origin = LineOrigin::Base;
    moved.lines[2].origin = LineOrigin::Base;
    let err = plain.check(&moved).unwrap_err().to_string();
    assert!(err.contains("content is not the final file's"), "{err}");
    let mut no_newline = good;
    no_newline.final_newline = Some(false);
    assert!(
        plain
            .check(&no_newline)
            .unwrap_err()
            .to_string()
            .contains("final newline")
    );
}

/// WHY-22 removes the ignore: the engine must reproduce every `replay`
/// scenario exactly. (`align` scenarios are WHY-23's.)
#[test]
#[ignore = "WHY-22"]
fn the_engine_reproduces_every_replay_scenario() {
    let mut failures = Vec::new();
    for s in all_scenarios()
        .iter()
        .filter(|s| s.stage() == Stage::Replay)
    {
        let (input, _) = s.inputs();
        if let Err(m) = s.check(&replay(&input)) {
            failures.push(m.to_string());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

// ---- The loader refuses malformed scenarios ---------------------------------

const VALID: &str = r#"{
  "format": 1,
  "name": "n",
  "description": "d",
  "stage": "replay",
  "harness": "claudeCode",
  "file": "a.rs",
  "base": { "lines": ["a", "b"], "finalNewline": true },
  "ops": [
    { "id": "e1", "session": "s", "turn": "t", "kind": "update",
      "hunks": [ { "oldStart": 1, "lines": [" a", "-b", "+c"] } ],
      "before": "auto", "after": "none" }
  ],
  "final": { "lines": ["a", "c"], "finalNewline": true },
  "expected": [
    { "line": 1, "origin": "base" },
    { "line": 2, "origin": { "op": "e1" } }
  ]
}"#;

fn variant(from: &str, to: &str) -> String {
    assert!(VALID.contains(from), "test bug: {from:?} not in VALID");
    VALID.replacen(from, to, 1)
}

fn refused(json: &str, needle: &str) {
    match Scenario::from_json(json) {
        Ok(_) => panic!("accepted a malformed scenario (expected {needle:?})"),
        Err(e) => assert!(e.to_string().contains(needle), "{needle:?} not in: {e}"),
    }
}

#[test]
fn the_valid_template_is_valid() {
    Scenario::from_json(VALID).unwrap();
}

#[test]
fn refuses_a_bad_or_missing_format() {
    refused(
        &variant(r#""format": 1"#, r#""format": 2"#),
        "unsupported format 2",
    );
    refused(&variant(r#""format": 1,"#, ""), "missing `format`");
    refused("not json", "not JSON");
}

#[test]
fn refuses_unknown_fields_everywhere() {
    refused(
        &variant(r#""name": "n","#, r#""name": "n", "nmae": 1,"#),
        "unknown field",
    );
    refused(
        &variant(r#""kind": "update","#, r#""kind": "update", "oops": 1,"#),
        "unknown field",
    );
    refused(
        &variant(r#""oldStart": 1,"#, r#""oldStart": 1, "x": 1,"#),
        "unknown field",
    );
    refused(
        &variant(
            r#""line": 1, "origin": "base""#,
            r#""line": 1, "origin": "base", "why": 1"#,
        ),
        "unknown field",
    );
}

#[test]
fn refuses_a_reference_to_a_missing_op() {
    refused(
        &variant(r#"{ "op": "e1" }"#, r#"{ "op": "e9" }"#),
        "no op \"e9\"",
    );
}

#[test]
fn refuses_expected_that_does_not_cover_the_final_file() {
    refused(
        &variant(
            r#",
    { "line": 2, "origin": { "op": "e1" } }"#,
            "",
        ),
        "`expected` has 1 entries but `final` has 2 lines",
    );
    refused(&variant(r#""line": 2"#, r#""line": 3"#), "in order");
}

#[test]
fn refuses_unknown_or_stub_reasons() {
    refused(
        &variant(r#"{ "op": "e1" }"#, r#"{ "unknown": "manual" }"#),
        "unknown variant",
    );
    refused(
        &variant(r#"{ "op": "e1" }"#, r#"{ "unknown": "notReplayed" }"#),
        "never ground truth",
    );
}

#[test]
fn refuses_duplicate_op_ids() {
    let dup = variant(
        r#""before": "auto", "after": "none" }"#,
        r#""before": "auto", "after": "none" },
    { "id": "e1", "session": "s", "turn": "t", "kind": "update",
      "hunks": [ { "oldStart": 2, "lines": ["-c", "+d"] } ],
      "before": "auto", "after": "none" }"#,
    );
    refused(&dup, "duplicate op id");
}

#[test]
fn refuses_a_hunk_that_does_not_match_the_file() {
    refused(
        &variant(r#"" a", "-b""#, r#"" a", "-x""#),
        "the hunk says \"x\"",
    );
    refused(
        &variant(r#""lines": [" a", "-b", "+c"]"#, r#""lines": ["*a"]"#),
        "must start with",
    );
}

#[test]
fn refuses_a_replay_scenario_whose_final_is_not_the_replay() {
    refused(
        &variant(r#"["a", "c"]"#, r#"["a", "z"]"#),
        "not what the ops produce",
    );
    // The same file marked `align` is fine (that is what align means)...
    let align = variant(r#""stage": "replay""#, r#""stage": "align""#).replacen(
        r#"["a", "c"]"#,
        r#"["a", "z"]"#,
        1,
    );
    Scenario::from_json(&align).unwrap();
    // ...but an `align` scenario equal to the replay is mislabelled.
    refused(
        &variant(r#""stage": "replay""#, r#""stage": "align""#),
        "use stage `replay`",
    );
}

#[test]
fn refuses_positions_that_do_not_fit_the_harness() {
    // Claude hunks carry `oldStart`; `appliedAt` is Codex's truth-only field.
    refused(
        &variant(r#""oldStart": 1,"#, r#""appliedAt": 1,"#),
        "Claude hunk",
    );
    // A Codex hunk without `appliedAt`: the oracle could not place it.
    let codex = variant(r#""claudeCode""#, r#""codex""#).replacen(
        r#""before": "auto""#,
        r#""before": "none""#,
        1,
    );
    refused(&codex, "`appliedAt` is required");
    refused(
        &variant(r#""claudeCode""#, r#""codex""#),
        "Codex gives no before/after",
    );
}

#[test]
fn refuses_unobserved_ops_that_carry_content_or_attribute_lines() {
    let unobserved = variant(
        r#""kind": "update","#,
        r#""kind": "update", "observed": { "unobserved": "payloadReduced" },"#,
    );
    refused(&unobserved, "unobserved op has no hunks");
    let bare = VALID
        .replacen(r#""kind": "update","#, r#""kind": "update", "observed": { "unobserved": "payloadReduced" }, "truthAfter": { "lines": ["a", "c"], "finalNewline": true },"#, 1)
        .replacen(r#""hunks": [ { "oldStart": 1, "lines": [" a", "-b", "+c"] } ],"#, "", 1)
        .replacen(r#""before": "auto""#, r#""before": "none""#, 1);
    refused(&bare, "is unobserved and cannot be an origin");
    let honest = bare.replacen(r#"{ "op": "e1" }"#, r#"{ "unknown": "payloadReduced" }"#, 1);
    Scenario::from_json(&honest).unwrap();
}

#[test]
fn refuses_lines_with_terminators() {
    refused(
        &variant(r#"["a", "b"]"#, r#"["a\r", "b"]"#),
        "ends with \\r",
    );
    refused(
        &variant(r#"["a", "b"]"#, r#"["a\nb"]"#),
        "contains a newline",
    );
}

#[test]
fn an_explicit_before_is_the_state_the_harness_really_saw() {
    // Something changed `b` to `B` between the base and the edit: the edit
    // applies to the explicit `before`, and the replay stage holds.
    let resync = VALID
        .replacen(
            r#""before": "auto""#,
            r#""before": { "lines": ["a", "B"], "finalNewline": true }"#,
            1,
        )
        .replacen(r#""-b""#, r#""-B""#, 1);
    let s = Scenario::from_json(&resync).unwrap();
    let (input, _) = s.inputs();
    let before = input.records[0].op.before.as_ref().unwrap();
    assert_ne!(
        before.lines,
        input.baseline.as_ref().unwrap().lines,
        "deliberately not the replayed state"
    );
}

#[test]
fn refuses_unknown_fields_in_file_texts() {
    refused(
        &variant(
            r#""base": { "lines": ["a", "b"], "finalNewline": true }"#,
            r#""base": { "lines": ["a", "b"], "finalNewline": true, "eol": "crlf" }"#,
        ),
        "unknown field",
    );
    refused(
        &variant(
            r#""final": { "lines": ["a", "c"], "finalNewline": true }"#,
            r#""final": { "lines": ["a", "c"], "finalNewline": true, "x": 1 }"#,
        ),
        "unknown field",
    );
    // An explicit `before` with a stray field matches neither "auto"/"none"
    // nor the file shape: refused, not read as "auto".
    refused(
        &variant(
            r#""before": "auto""#,
            r#""before": { "lines": ["a", "b"], "finalNewline": true, "x": 1 }"#,
        ),
        "bad scenario",
    );
}

#[test]
fn refuses_create_hunks_with_positions_and_misplaced_codex_headers() {
    let create = r#"{
      "format": 1, "name": "n", "description": "d", "stage": "replay",
      "harness": "claudeCode", "file": "a.rs", "base": null,
      "ops": [ { "id": "e1", "session": "s", "turn": "t", "kind": "create",
                 "hunks": [ { "oldStart": 0, "lines": ["+a"] } ],
                 "before": "none", "after": "auto" } ],
      "final": { "lines": ["a"], "finalNewline": true },
      "expected": [ { "line": 1, "origin": { "op": "e1" } } ]
    }"#;
    refused(create, "a create's hunk");
    Scenario::from_json(&create.replacen(r#""oldStart": 0, "#, "", 1)).unwrap();

    let codex = r#"{
      "format": 1, "name": "n", "description": "d", "stage": "replay",
      "harness": "codex", "file": "a.py",
      "base": { "lines": ["def f():", "    return 1"], "finalNewline": true },
      "ops": [ { "id": "p1", "session": "s", "turn": "t", "kind": "update",
                 "hunks": [ { "appliedAt": 2, "header": "def f():", "lines": ["-    return 1", "+    return 2"] } ],
                 "before": "none", "after": "none" } ],
      "final": { "lines": ["def f():", "    return 2"], "finalNewline": true },
      "expected": [ { "line": 1, "origin": "base" }, { "line": 2, "origin": { "op": "p1" } } ]
    }"#;
    Scenario::from_json(codex).unwrap();
    refused(
        &codex.replacen(r#""header": "def f():""#, r#""header": "def g():""#, 1),
        "not a line above",
    );
}

#[test]
fn a_created_empty_file_is_neither_a_delete_nor_a_blank_line() {
    let json = r#"{
      "format": 1, "name": "n", "description": "d", "stage": "replay",
      "harness": "claudeCode", "file": "empty.txt", "base": null,
      "ops": [
        { "id": "e1", "session": "s", "turn": "t", "kind": "create",
          "before": "none", "after": "auto" },
        { "id": "e2", "session": "s", "turn": "t2", "kind": "update",
          "hunks": [ { "oldStart": 0, "lines": ["+first"] } ],
          "before": "auto", "after": "none", "finalNewline": true }
      ],
      "final": { "lines": ["first"], "finalNewline": true },
      "expected": [ { "line": 1, "origin": { "op": "e2" } } ]
    }"#;
    let s = Scenario::from_json(json).unwrap();
    let (input, _) = s.inputs();
    assert!(input.baseline.is_none(), "absent, not empty");
    let created = &input.records[0].op;
    assert!(created.hunks.is_empty());
    let after = created.after.unwrap();
    assert_eq!((after.line_count, after.final_newline), (0, false));
    // The second edit saw an existing, EMPTY file.
    let before = input.records[1].op.before.as_ref().unwrap();
    assert!(before.lines.is_empty());

    refused(
        &json.replacen(
            r#""kind": "create","#,
            r#""kind": "create", "finalNewline": true,"#,
            1,
        ),
        "no final newline",
    );
    refused(
        &variant(
            r#""base": { "lines": ["a", "b"], "finalNewline": true }"#,
            r#""base": { "lines": [], "finalNewline": true }"#,
        ),
        "cannot end with a newline",
    );
}

#[test]
fn base_and_final_keys_are_required_even_when_null() {
    refused(
        &variant(
            r#""base": { "lines": ["a", "b"], "finalNewline": true },"#,
            "",
        ),
        "missing field `base`",
    );
    refused(
        &variant(
            r#",
  "final": { "lines": ["a", "c"], "finalNewline": true }"#,
            "",
        ),
        "missing field `final`",
    );
}
