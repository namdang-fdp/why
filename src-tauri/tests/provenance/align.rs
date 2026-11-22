//! WHY-23: the engine's replay, aligned with the real final file, must
//! reproduce every `align` scenario (and change nothing for a `replay` one).
//!
//! The scenarios' `inputs()` give the replay input and the final line hashes
//! but not the final newline, so this file reads `final.finalNewline` from the
//! scenario JSON itself (plain data; `truth.rs` is the oracle and stays
//! untouched, decision 33).

use std::path::Path;

use why_lib::provenance::{
    FinalNewline, LineHash, LineOrigin, ReplayOutput, ReplayedLine, align, replay_report,
};

use crate::truth::{Scenario, Stage};
use crate::{all_scenarios, scenario_dir};

/// The real file's final-newline flag, from the scenario's `final` (`null`:
/// the file is gone, which has no final newline).
fn final_newline(name: &str) -> Option<bool> {
    let path = Path::new(&scenario_dir()).join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["final"]["finalNewline"].as_bool()
}

/// Engine, then alignment, in the shape `Scenario::check` takes.
fn run(s: &Scenario) -> (ReplayOutput, FinalNewline) {
    let (input, final_hashes) = s.inputs();
    // A gone file has no lines and no flag; alignment gets `false`.
    let flag = final_newline(s.name());
    let report = replay_report(&input);
    let aligned = align(&report, &final_hashes, flag.unwrap_or(false));
    let lines = final_hashes
        .iter()
        .zip(aligned.origins)
        .map(|(&hash, origin): (&LineHash, LineOrigin)| ReplayedLine { hash, origin })
        .collect();
    (
        ReplayOutput {
            lines,
            final_newline: flag,
        },
        aligned.final_newline,
    )
}

#[test]
fn alignment_reproduces_every_align_scenario() {
    let mut failures = Vec::new();
    let mut count = 0;
    for s in all_scenarios().iter().filter(|s| s.stage() == Stage::Align) {
        count += 1;
        if let Err(m) = s.check(&run(s).0) {
            failures.push(m.to_string());
        }
    }
    assert!(count >= 14, "the align scenarios exist: {count}");
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// On a faithful replay, alignment must be a no-op: same origins, and the
/// replay's final newline never disagrees with the real file's.
#[test]
fn alignment_changes_nothing_for_replay_scenarios() {
    let mut failures = Vec::new();
    for s in all_scenarios()
        .iter()
        .filter(|s| s.stage() == Stage::Replay)
    {
        let (out, newline) = run(s);
        if let Err(m) = s.check(&out) {
            failures.push(m.to_string());
        }
        if matches!(newline, FinalNewline::Differs { .. }) {
            failures.push(format!("{}: final newline {newline:?}", s.name()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The README leaves the final newline of `align` scenarios to WHY-23: check
/// the ones whose flag is the point.
#[test]
fn the_final_newline_is_compared_not_just_the_origins() {
    let all = all_scenarios();
    let verdict = |name: &str| run(all.iter().find(|s| s.name() == name).unwrap()).1;

    // The base has no final newline, a Claude Edit rewrote line 1 and the
    // file gained one: the replay (strictly inside the file) keeps `false`.
    assert_eq!(
        verdict("claude-edit-gains-final-newline"),
        FinalNewline::Differs { replayed: false }
    );
    // An op that reaches the last line with no `after` cannot know: the real
    // file is the only witness.
    assert_eq!(
        verdict("claude-uncaptured-change-between-edits"),
        FinalNewline::ReplayUnknown
    );
    // A Write carries the flag in its `after` fingerprint.
    assert_eq!(
        verdict("claude-write-gains-final-newline"),
        FinalNewline::Agrees
    );
}
