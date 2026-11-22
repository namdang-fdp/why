//! Alignment (WHY-23): lay the engine's replayed file over the REAL file.
//!
//! The replay says what the file would be if only the captured edits had
//! touched it. The real file may differ (a formatter, a `sed`, a person, a
//! rebase, an edit the hooks missed). Alignment decides, line of the real file
//! by line, whether it is provably a line the replay knew, and if so inherits
//! that line's origin; every other line is `Unknown`.
//!
//! # The rule: only proof moves an origin
//!
//! 1. **Matched lines keep their replayed origin.** "Matched" is exactly the
//!    rule-5 procedure of the scenarios' README (unique anchors, the chain
//!    every longest chain agrees on, identical gaps, recursion with a stop):
//!    the engine's own matcher, called again against the real file. Ambiguous
//!    evidence matches nothing.
//! 2. **Unmatched lines are `Unknown(NoCapture)`**, unless the replay itself
//!    ended `unresolved` (an unobserved op, an op it could not apply): then
//!    they get that op's reason (rule 6 beats rule 5).
//! 3. **Content equality never transfers origin.** A real line whose text
//!    equals some replayed line's text, but which the matcher could not pair
//!    with it, is `Unknown`. (`align-uncaptured-revert-to-base-text`: the file
//!    is byte for byte the base, yet the reverted line is not `Base`.)
//!
//! Why it cannot attribute a line wrongly: an `Edit`/`Base` origin is only ever
//! copied from a replayed line whose hash equals the real line's hash AND whose
//! position the matcher proved unique and order-consistent, or that sits in a
//! gap that is identical (or identical but for undecided Codex lines, below)
//! between two such proofs. Nothing else hands out an origin.
//!
//! # Ambiguous Codex placements (decision 30, rule 7)
//!
//! [`ReplayReport::alternatives`] lists replayed lines whose TEXT is undecided
//! (a Codex hunk that fits in several places). They need care:
//!
//! - they are never anchors: the matcher sees a unique stand-in hash for
//!   them that no real line can equal. Neither is any TEXT they might hold
//!   (a "contested" text): if Claude wrote `x` and an ambiguous Codex hunk may
//!   or may not have written another `x`, a lone `x` in the real file is not
//!   proof of whose it is, so such lines are hidden from the matcher on both
//!   sides;
//! - afterwards, a stretch of lines between two matches is paired line for
//!   line when it has the same length on both sides and every replayed line in
//!   it is either equal to the real line or an undecided line one of whose
//!   possible texts is the real line's. That is the matcher's "identical gap"
//!   rule, extended so that an undecided line is compatible with each of its
//!   candidate texts. A line paired this way keeps its replayed origin, which
//!   is `AmbiguousMatch`: a real line that holds one of the candidates is NOT
//!   thereby resolved (the file cannot prove which placement was true, and a
//!   hand edit could have produced the same text);
//! - an undecided line whose neighbourhood does not fit this (a real line that
//!   is none of the candidates, a gap of another length) is unmatched, so
//!   `NoCapture`.
//!
//! The lines of one ambiguous hunk are LINKED (one placement is true for all
//! of them), and this check is per line, so a combination no single placement
//! could produce is still labelled `AmbiguousMatch`. That is a harmless
//! over-approximation: the label is an `Unknown`, never an author.
//!
//! # The final newline
//!
//! A newline is not part of any line (the line policy), so a gained or lost
//! final newline never changes an origin. It is compared and exposed
//! ([`FinalNewline`]) so the claim "the replay matches the file" can be
//! checked, not assumed.
//!
//! Pure: no IO, no clock.

use std::collections::{HashMap, HashSet};

use super::line::LineHash;
use super::origin::{LineOrigin, UnknownReason};
use super::replay::{ReplayReport, match_lines};

/// The real file, line by line, with the origin WHY can prove for each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alignment {
    /// One origin per line of the real file, in order.
    pub origins: Vec<LineOrigin>,
    /// What the replay says about the final newline versus the real file.
    pub final_newline: FinalNewline,
}

/// Does the replay agree with the real file about its final newline?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalNewline {
    /// The replay knew the flag and the real file has the same one.
    Agrees,
    /// The replay knew the flag and the real file differs (an edit that gained
    /// a newline without telling, a formatter, a person).
    Differs { replayed: bool },
    /// The replay could not know (Codex creates the file, a hunk touches the
    /// last line, or the replay ended `unresolved`). Only the real file says.
    ReplayUnknown,
}

/// Align `report` (see [`replay_report`](super::replay::replay_report)) with
/// the real file's line hashes and final-newline flag.
pub fn align(report: &ReplayReport, real: &[LineHash], real_final_newline: bool) -> Alignment {
    let old = &report.output.lines;

    // index of a replayed line -> the OTHER texts it may have.
    let undecided: HashMap<usize, &[LineHash]> = report
        .alternatives
        .iter()
        .map(|a| (a.index, a.others.as_slice()))
        .collect();

    // The matcher sees a stand-in for each undecided line: unique per index,
    // equal to no real line (see `stand_in`), so it can never be an anchor
    // and never be matched by text. Pairing them is `pair_compatible` below.
    //
    // Hiding them has a catch: a text an undecided line MIGHT hold would look
    // unique elsewhere (Claude wrote `x`, a Codex hunk may or may not have
    // written another `x`). Such "contested" texts are hidden too, on both
    // sides, so they can never be anchors either; they can still be paired by
    // `pair_compatible`, which compares positions, not uniqueness.
    let contested: HashSet<LineHash> = report
        .alternatives
        .iter()
        .flat_map(|a| {
            a.others
                .iter()
                .copied()
                .chain(old.get(a.index).map(|l| l.hash))
        })
        .collect();
    let old_hashes: Vec<LineHash> = old
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if undecided.contains_key(&i) || contested.contains(&line.hash) {
                stand_in(i, 0xFF)
            } else {
                line.hash
            }
        })
        .collect();
    let real_hashes: Vec<LineHash> = real
        .iter()
        .enumerate()
        .map(|(j, h)| {
            if contested.contains(h) {
                stand_in(j, 0xFE) // a different family from the replay's
            } else {
                *h
            }
        })
        .collect();
    let mut matched = match_lines(&old_hashes, &real_hashes);
    pair_compatible(old, &undecided, real, &mut matched);

    // Rule 6 beats rule 5 for lines the replay could not follow.
    let unmatched = report
        .unresolved
        .as_ref()
        .map_or(UnknownReason::NoCapture, |u| u.reason);
    let origins = matched
        .iter()
        .map(|m| match m {
            Some(i) => old[*i].origin.clone(),
            None => LineOrigin::Unknown(unmatched),
        })
        .collect();

    let final_newline = match (&report.unresolved, report.output.final_newline) {
        (Some(_), _) | (None, None) => FinalNewline::ReplayUnknown,
        (None, Some(flag)) if flag == real_final_newline => FinalNewline::Agrees,
        (None, Some(flag)) => FinalNewline::Differs { replayed: flag },
    };
    Alignment {
        origins,
        final_newline,
    }
}

/// A hash no real line has: a fill byte (`family`) in the first half, the
/// index after it. Replay and real lines use different families so a stand-in
/// can never match the other side's.
/// A real line's hash is a truncated SHA-256, so equality is a 2^-64 event
/// per pair at worst, and it could only ever cost a line its (unproven) match.
fn stand_in(index: usize, family: u8) -> LineHash {
    let mut bytes = [family; 16];
    bytes[8..].copy_from_slice(&(index as u64).to_le_bytes());
    LineHash(bytes)
}

/// The "identical gap" rule for undecided lines (see the module docs): in each
/// stretch between two matched pairs, if both sides have the same number of
/// lines and every replayed line is compatible with the real line at the same
/// offset, pair them all.
fn pair_compatible(
    old: &[super::replay::ReplayedLine],
    undecided: &HashMap<usize, &[LineHash]>,
    real: &[LineHash],
    matched: &mut [Option<usize>],
) {
    if undecided.is_empty() {
        // Nothing the matcher could not already decide.
        return;
    }
    let compatible = |old_at: usize, new_hash: &LineHash| {
        old[old_at].hash == *new_hash
            || undecided
                .get(&old_at)
                .is_some_and(|others| others.contains(new_hash))
    };
    // Walk the matched pairs in order; `*_from` is the first line after the
    // previous pair. A final pair at the ends closes the last stretch.
    let pairs: Vec<(usize, usize)> = matched
        .iter()
        .enumerate()
        .filter_map(|(n, m)| m.map(|o| (o, n)))
        .chain(std::iter::once((old.len(), real.len())))
        .collect();
    let (mut old_from, mut new_from) = (0, 0);
    for (o, n) in pairs {
        let (old_len, new_len) = (o - old_from, n - new_from);
        if old_len == new_len
            && old_len > 0
            && (0..old_len).all(|k| compatible(old_from + k, &real[new_from + k]))
        {
            for k in 0..old_len {
                matched[new_from + k] = Some(old_from + k);
            }
        }
        (old_from, new_from) = (o + 1, n + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{EventKey, SessionKey};
    use crate::provenance::EditSource;
    use crate::provenance::line::LineHasher;
    use crate::provenance::replay::{LineAlternatives, ReplayOutput, ReplayedLine, Unresolved};
    use why_wire::Harness;

    fn h(s: &str) -> LineHash {
        LineHasher::default().hash_line(s.as_bytes())
    }

    fn hs(s: &str) -> Vec<LineHash> {
        s.split_whitespace().map(h).collect()
    }

    fn edit(id: &str) -> LineOrigin {
        LineOrigin::Edit(EditSource {
            session: SessionKey::new(Harness::ClaudeCode, "s"),
            turn_id: None,
            agent_id: None,
            tool_use_id: Some(id.into()),
            event_key: EventKey(id.into()),
        })
    }

    /// `base` lines are `Base`, a word in `e1`'s list is its edit's.
    fn report(text: &str, edited: &[&str]) -> ReplayReport {
        ReplayReport {
            output: ReplayOutput {
                lines: text
                    .split_whitespace()
                    .map(|w| ReplayedLine {
                        hash: h(w),
                        origin: if edited.contains(&w) {
                            edit("e1")
                        } else {
                            LineOrigin::Base
                        },
                    })
                    .collect(),
                final_newline: Some(true),
            },
            ..Default::default()
        }
    }

    const NO_CAPTURE: LineOrigin = LineOrigin::Unknown(UnknownReason::NoCapture);

    #[test]
    fn matched_lines_keep_their_origin_and_new_ones_are_no_capture() {
        let r = report("a b c", &["b"]);
        let got = align(&r, &hs("a b c d"), true);
        assert_eq!(
            got.origins,
            [LineOrigin::Base, edit("e1"), LineOrigin::Base, NO_CAPTURE]
        );
        assert_eq!(got.final_newline, FinalNewline::Agrees);
    }

    #[test]
    fn equal_text_never_transfers_origin() {
        // `b` is e1's. The real file has the same text twice: which one is
        // e1's? Neither is provable.
        let r = report("a b c", &["b"]);
        let got = align(&r, &hs("a b b c"), true);
        assert_eq!(got.origins[1], NO_CAPTURE);
        assert_eq!(got.origins[2], NO_CAPTURE);
    }

    #[test]
    fn an_unresolved_replay_gives_its_reason_to_unmatched_lines() {
        let mut r = report("a b", &[]);
        r.unresolved = Some(Unresolved {
            since: edit_source("u"),
            reason: UnknownReason::PayloadReduced,
        });
        r.output.final_newline = Some(true);
        let got = align(&r, &hs("a X"), true);
        assert_eq!(
            got.origins,
            [
                LineOrigin::Base,
                LineOrigin::Unknown(UnknownReason::PayloadReduced)
            ]
        );
        // The flag of a last-known state is not the file's.
        assert_eq!(got.final_newline, FinalNewline::ReplayUnknown);
    }

    fn edit_source(id: &str) -> EditSource {
        match edit(id) {
            LineOrigin::Edit(s) => s,
            _ => unreachable!(),
        }
    }

    #[test]
    fn final_newline_is_compared() {
        let mut r = report("a", &[]);
        r.output.final_newline = Some(false);
        assert_eq!(
            align(&r, &hs("a"), true).final_newline,
            FinalNewline::Differs { replayed: false }
        );
        assert_eq!(
            align(&r, &hs("a"), false).final_newline,
            FinalNewline::Agrees
        );
        r.output.final_newline = None;
        assert_eq!(
            align(&r, &hs("a"), true).final_newline,
            FinalNewline::ReplayUnknown
        );
    }

    /// Replay of `[k v k v]` where a hunk `-v +w` may have hit either `v`.
    fn ambiguous() -> ReplayReport {
        let amb = LineOrigin::Unknown(UnknownReason::AmbiguousMatch);
        let line = |w: &str, origin: LineOrigin| ReplayedLine { hash: h(w), origin };
        ReplayReport {
            output: ReplayOutput {
                lines: vec![
                    line("k", LineOrigin::Base),
                    line("v", amb.clone()),
                    line("k", LineOrigin::Base),
                    line("v", amb),
                ],
                final_newline: Some(true),
            },
            alternatives: vec![
                LineAlternatives {
                    index: 1,
                    others: vec![h("w")],
                },
                LineAlternatives {
                    index: 3,
                    others: vec![h("w")],
                },
            ],
            unresolved: None,
        }
    }

    #[test]
    fn undecided_lines_stay_ambiguous_even_when_the_file_shows_a_candidate() {
        let amb = LineOrigin::Unknown(UnknownReason::AmbiguousMatch);
        let b = LineOrigin::Base;
        // The first placement was true...
        let got = align(&ambiguous(), &hs("k w k v"), true);
        assert_eq!(
            got.origins,
            [b.clone(), amb.clone(), b.clone(), amb.clone()]
        );
        // ...or the second: the file cannot prove it, so both stay ambiguous.
        let got = align(&ambiguous(), &hs("k v k w"), true);
        assert_eq!(got.origins, [b.clone(), amb.clone(), b, amb]);
    }

    #[test]
    fn a_text_an_undecided_line_might_hold_is_not_an_anchor() {
        // Claude wrote `x` (e1); `v` at index 2 or 3 may have become `x`
        // (Codex). An uncaptured edit then removed Claude's `x`, leaving the
        // Codex `x` alone: it looks unique, but it must not be credited to e1.
        let line = |w: &str, origin: LineOrigin| ReplayedLine { hash: h(w), origin };
        let amb = LineOrigin::Unknown(UnknownReason::AmbiguousMatch);
        let r = ReplayReport {
            output: ReplayOutput {
                lines: vec![
                    line("x", edit("e1")),
                    line("v", amb.clone()),
                    line("v", amb),
                ],
                final_newline: Some(true),
            },
            alternatives: vec![
                LineAlternatives {
                    index: 1,
                    others: vec![h("x")],
                },
                LineAlternatives {
                    index: 2,
                    others: vec![h("x")],
                },
            ],
            unresolved: None,
        };
        let got = align(&r, &hs("x v"), true);
        assert!(!got.origins.contains(&edit("e1")), "{:?}", got.origins);
    }

    #[test]
    fn an_undecided_line_that_is_none_of_its_texts_is_no_capture() {
        let got = align(&ambiguous(), &hs("k ZZ k v"), true);
        // Not a compatible stretch: nothing in it is provable (`k` repeats).
        assert!(
            got.origins.iter().all(|o| *o == NO_CAPTURE),
            "{:?}",
            got.origins
        );
    }
}
