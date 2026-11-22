//! WHY-22 engine checks beyond the `replay` stage (the engine never reads the
//! oracle; only `Scenario`'s public loader API is used).

use why_lib::provenance::replay::replay_report;

use crate::all_scenarios;

/// WHY-23 will run the engine on every file, including the ones only the
/// real file can settle: it must never panic (or loop) on any scenario, and
/// `replay()` must be exactly `replay_report().output`.
#[test]
fn the_engine_runs_on_every_scenario_including_align() {
    for s in all_scenarios() {
        let (input, _) = s.inputs();
        let report = replay_report(&input);
        assert_eq!(why_lib::provenance::replay(&input), report.output);
    }
}

/// Align scenarios whose ORIGINS the engine alone already answers (the align
/// check compares origins only; text and the final newline are WHY-23's):
/// rule 7's equal-size ambiguity, and an Edit whose newline only the file
/// shows.
#[test]
fn align_scenarios_whose_origins_the_replay_already_answers() {
    let wanted = [
        "codex-ambiguous-context",
        "codex-update-ambiguous-three-twins",
        "codex-update-header-still-ambiguous",
        "claude-edit-gains-final-newline",
    ];
    let all = all_scenarios();
    let mut failures = Vec::new();
    for name in wanted {
        let s = all
            .iter()
            .find(|s| s.name() == name)
            .unwrap_or_else(|| panic!("scenario {name} is missing"));
        let (input, _) = s.inputs();
        if let Err(m) = s.check(&replay_report(&input).output) {
            failures.push(m.to_string());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
