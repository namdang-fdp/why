//! WHY-23 property: whatever happens to the real file, alignment never gives
//! a line an `Edit`/`Base` origin it does not have.
//!
//! A tiny model plays the agent (Claude-style edits with `before` snapshots),
//! tracking for every line WHO wrote it. Then random "uncaptured" changes
//! (deletes, inserts, rewrites, copies) turn the model's file into the real
//! one, still tracking identities. Alignment runs on hashes only, like in
//! production, and every origin it hands out is checked against the model.

use proptest::prelude::*;
use why_lib::events::{EventKey, SessionKey};
use why_lib::provenance::{
    EditHunk, EditKind, EditOp, EditRecord, EditSource, FileSnapshot, HunkLineHash, HunkLineKind,
    LineHash, LineHasher, LineOrigin, Observed, ReplayInput, align, replay_report,
};
use why_wire::Harness;

/// Who really wrote a line (the model's truth; the engine never sees it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Truth {
    Base,
    Op(usize),
    /// Written by something the hooks did not capture.
    Foreign,
}

type ModelLine = (String, Truth);

fn h(text: &str) -> LineHash {
    LineHasher::default().hash_line(text.as_bytes())
}

fn hashes(lines: &[ModelLine]) -> Vec<LineHash> {
    lines.iter().map(|(t, _)| h(t)).collect()
}

/// A small vocabulary on purpose: duplicate lines are the hard case.
fn word(i: u8) -> String {
    format!("w{}", i % 5)
}

#[derive(Debug, Clone)]
struct Edit {
    at: u32,
    remove: u32,
    add: Vec<u8>,
}

#[derive(Debug, Clone)]
enum Mutation {
    Delete(u32),
    /// Insert a line text no captured edit ever wrote (`f<k>`).
    Insert(u32, u8),
    Rewrite(u32, u8),
    /// Insert a copy of an existing line (identical text, foreign author).
    Copy(u32, u32),
}

fn edit_strategy() -> impl Strategy<Value = Edit> {
    (
        any::<u32>(),
        0u32..3,
        prop::collection::vec(any::<u8>(), 0..3),
    )
        .prop_map(|(at, remove, add)| Edit { at, remove, add })
}

fn mutation_strategy() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        any::<u32>().prop_map(Mutation::Delete),
        (any::<u32>(), any::<u8>()).prop_map(|(p, f)| Mutation::Insert(p, f)),
        (any::<u32>(), any::<u8>()).prop_map(|(p, f)| Mutation::Rewrite(p, f)),
        (any::<u32>(), any::<u32>()).prop_map(|(p, s)| Mutation::Copy(p, s)),
    ]
}

fn source(k: usize) -> EditSource {
    EditSource {
        session: SessionKey::new(Harness::ClaudeCode, "s"),
        turn_id: Some(format!("t{k}")),
        agent_id: None,
        tool_use_id: Some(format!("e{k}")),
        event_key: EventKey(format!("k{k}")),
    }
}

/// Apply `edits` to `base`, building the records the hooks would have sent.
fn play(base: &[String], edits: &[Edit]) -> (ReplayInput, Vec<ModelLine>) {
    let hasher = LineHasher::default();
    let mut state: Vec<ModelLine> = base.iter().map(|t| (t.clone(), Truth::Base)).collect();
    let baseline = FileSnapshot::of_lines(&hasher, hashes(&state), true);
    let mut records = Vec::new();
    for (k, e) in edits.iter().enumerate() {
        let len = state.len();
        let start = e.at as usize % (len + 1);
        let remove = (e.remove as usize).min(len - start);
        if remove == 0 && e.add.is_empty() {
            continue; // an edit that changes nothing is not an edit
        }
        let before = FileSnapshot::of_lines(&hasher, hashes(&state), true);
        let mut lines: Vec<HunkLineHash> = state[start..start + remove]
            .iter()
            .map(|(t, _)| HunkLineHash {
                kind: HunkLineKind::Removed,
                hash: h(t),
            })
            .collect();
        let added: Vec<ModelLine> = e.add.iter().map(|&w| (word(w), Truth::Op(k))).collect();
        lines.extend(added.iter().map(|(t, _)| HunkLineHash {
            kind: HunkLineKind::Added,
            hash: h(t),
        }));
        // Unified-diff meaning: first removed line, or (pure insertion) the
        // line AFTER which the lines go.
        let old_start = if remove > 0 { start + 1 } else { start };
        state.splice(start..start + remove, added);
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
                    old_start: Some(old_start as u32),
                    header: None,
                    lines,
                }],
                observed: Observed::Exact,
            },
        });
    }
    (
        ReplayInput {
            baseline: Some(baseline),
            records,
        },
        state,
    )
}

fn mutate(mut lines: Vec<ModelLine>, mutations: &[Mutation]) -> (Vec<ModelLine>, bool) {
    let mut copied = false;
    for m in mutations {
        let len = lines.len();
        match *m {
            Mutation::Delete(p) if len > 0 => {
                lines.remove(p as usize % len);
            }
            Mutation::Insert(p, f) => {
                lines.insert(
                    p as usize % (len + 1),
                    (format!("f{}", f % 4), Truth::Foreign),
                );
            }
            Mutation::Rewrite(p, f) if len > 0 => {
                lines[p as usize % len] = (format!("f{}", f % 4), Truth::Foreign);
            }
            Mutation::Copy(p, s) if len > 0 => {
                let text = lines[s as usize % len].0.clone();
                lines.insert(p as usize % (len + 1), (text, Truth::Foreign));
                copied = true;
            }
            _ => {}
        }
    }
    (lines, copied)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 3000,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    #[test]
    fn alignment_never_attributes_a_line_it_cannot_prove(
        base in prop::collection::vec(any::<u8>(), 0..10),
        edits in prop::collection::vec(edit_strategy(), 0..8),
        mutations in prop::collection::vec(mutation_strategy(), 0..6),
    ) {
        let base: Vec<String> = base.into_iter().map(word).collect();
        let (input, replayed_truth) = play(&base, &edits);
        let (real, copied) = mutate(replayed_truth, &mutations);

        let report = replay_report(&input);
        let real_hashes = hashes(&real);
        let aligned = align(&report, &real_hashes, true);
        prop_assert_eq!(aligned.origins.len(), real.len());

        let written_by = |k: usize| -> Vec<&str> {
            edits_added(&input, k)
        };
        for (i, origin) in aligned.origins.iter().enumerate() {
            let (text, truth) = &real[i];
            match origin {
                LineOrigin::Unknown(_) => {}
                LineOrigin::Base => {
                    // Weak (always): a base origin needs base text.
                    prop_assert!(base.contains(text), "line {i} {text:?} is not base text");
                    // Strict (no copies: content equality cannot fool us).
                    if !copied {
                        prop_assert_eq!(*truth, Truth::Base, "line {} {:?}", i, text);
                    }
                }
                LineOrigin::Edit(src) => {
                    let k: usize = src.tool_use_id.as_ref().unwrap()[1..].parse().unwrap();
                    prop_assert!(
                        written_by(k).contains(&text.as_str()),
                        "line {i} {text:?} was not written by e{k}"
                    );
                    if !copied {
                        prop_assert_eq!(*truth, Truth::Op(k), "line {} {:?}", i, text);
                    }
                }
            }
        }
    }
}

/// The texts op `k` added, read back from its hunk hashes via the words.
fn edits_added(input: &ReplayInput, k: usize) -> Vec<&'static str> {
    const WORDS: [&str; 5] = ["w0", "w1", "w2", "w3", "w4"];
    let Some(rec) = input
        .records
        .iter()
        .find(|r| r.source.tool_use_id.as_deref() == Some(&format!("e{k}")))
    else {
        return Vec::new();
    };
    rec.op.hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == HunkLineKind::Added)
        .filter_map(|l| WORDS.iter().copied().find(|w| h(w) == l.hash))
        .collect()
}
