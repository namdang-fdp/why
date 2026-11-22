//! The "unambiguous" line matcher of rule 5 (`tests/provenance/scenarios/
//! README.md`, "The unambiguous test of rule 5"): which lines of a NEW state
//! are provably the same lines as lines of an OLD state.
//!
//! It is patience-diff-shaped, with one deliberate difference: when the
//! evidence is ambiguous it matches NOTHING instead of picking one reading.
//! A diff tool must produce some diff; provenance may answer "unknown", and
//! an unmatched line costs an `Unknown`, while a wrong match would hand a line
//! an author it never had.
//!
//! 1. Anchors are lines whose hash occurs exactly once in BOTH ranges.
//! 2. Keep only the anchors that belong to EVERY longest same-order chain of
//!    anchors (a longest increasing subsequence of their old positions, taken
//!    in new order). If two chains tie, the lines they disagree on are not
//!    evidence of anything.
//! 3. Between two kept anchors: identical gaps match line for line; other
//!    gaps repeat 1-3 inside, uniqueness counted inside the gap only.
//!    A non-identical gap with no kept anchor STOPS: its lines stay unmatched.
//!
//! No prefix/suffix trimming on purpose: in `A x B → A x x B` a diff tool
//! would match the first `x`, but nothing proves WHICH `x` survived.

use std::collections::HashMap;
use std::ops::Range;

use crate::provenance::line::LineHash;

/// For every line of `new`, the index of the line of `old` it provably is, or
/// `None`. Matched pairs are strictly increasing in both coordinates.
///
/// Cost: one level of the work list is linear in its range (plus
/// `k log k` for its `k` anchors), and ranges at one level are disjoint, so
/// typical inputs are `O(n log n)`. A hostile input can nest gaps so that
/// each level peels off only one anchor, which would be `O(n²)` (see
/// `tests::pathological`). A work BUDGET, linear in the input, caps that:
/// once it is spent, the gaps still queued stay unmatched. That only ever
/// turns would-be matches into `Unknown` (every match made so far is final
/// and correct), so a hostile file costs precision, never a wrong author or
/// a stalled app.
pub(super) fn match_lines(old: &[LineHash], new: &[LineHash]) -> Vec<Option<usize>> {
    match_lines_within(old, new, work_budget(old.len() + new.len()))
}

/// Line visits per input line the matcher may spend: a few levels of full
/// re-scans, far more than any real edit needs (real gaps shrink fast).
const WORK_PER_LINE: usize = 32;
/// A floor, so small files are never cut short by the budget.
const MIN_WORK: usize = 1 << 16;

fn work_budget(lines: usize) -> usize {
    lines.saturating_mul(WORK_PER_LINE).saturating_add(MIN_WORK)
}

fn match_lines_within(old: &[LineHash], new: &[LineHash], mut budget: usize) -> Vec<Option<usize>> {
    let mut matched = vec![None; new.len()];
    // An explicit work list instead of recursion: on hostile input the
    // nesting depth can reach the number of lines, which would overflow the
    // thread's stack (a 1M-line file is legal input).
    let mut work: Vec<(Range<usize>, Range<usize>)> = vec![(0..old.len(), 0..new.len())];
    while let Some((o, n)) = work.pop() {
        // One visit of the gap costs about its size (compare, count, scan).
        let cost = o.len() + n.len();
        if cost > budget {
            // Over budget: this gap stays unmatched (smaller ones queued may
            // still fit). Deterministic: the work order is.
            continue;
        }
        budget -= cost;
        if old[o.clone()] == new[n.clone()] {
            for (k, j) in n.enumerate() {
                matched[j] = Some(o.start + k);
            }
            continue;
        }
        let anchors = kept_anchors(&old[o.clone()], &new[n.clone()]);
        if anchors.is_empty() {
            // The stop of step 3: no surviving anchor means no progress is
            // possible (re-queuing the same range would loop forever), so
            // every line of this gap stays unmatched.
            continue;
        }
        let (mut o_from, mut n_from) = (o.start, n.start);
        for (ao, an) in anchors {
            let (ao, an) = (o.start + ao, n.start + an);
            matched[an] = Some(ao);
            work.push((o_from..ao, n_from..an));
            (o_from, n_from) = (ao + 1, an + 1);
        }
        work.push((o_from..o.end, n_from..n.end));
    }
    matched
}

/// Steps 1 and 2 inside one range pair: the anchors (old index, new index),
/// relative to the slices, in increasing order.
fn kept_anchors(old: &[LineHash], new: &[LineHash]) -> Vec<(usize, usize)> {
    #[derive(Default)]
    struct Seen {
        in_old: u32,
        old_at: usize,
        in_new: u32,
    }
    // Only looked up, never iterated: HashMap's random iteration order cannot
    // leak into the result, so the output stays deterministic.
    let mut seen: HashMap<LineHash, Seen> = HashMap::with_capacity(old.len());
    for (i, h) in old.iter().enumerate() {
        let s = seen.entry(*h).or_default();
        s.in_old = s.in_old.saturating_add(1);
        s.old_at = i;
    }
    for h in new {
        if let Some(s) = seen.get_mut(h) {
            s.in_new = s.in_new.saturating_add(1);
        }
    }
    let anchors: Vec<(usize, usize)> = new
        .iter()
        .enumerate()
        .filter_map(|(j, h)| {
            let s = seen.get(h)?;
            (s.in_old == 1 && s.in_new == 1).then_some((s.old_at, j))
        })
        .collect();
    in_every_longest_chain(&anchors)
}

/// The anchors (already in new order, old positions distinct) that lie on
/// EVERY longest chain with increasing old positions.
///
/// Classic result: with `ending[i]` the longest chain ending at `i` and
/// `starting[i]` the longest one starting there, `i` lies on SOME longest
/// chain iff `ending + starting - 1 == longest`. Every longest chain takes
/// exactly one element per `ending` value, so `i` is on EVERY longest chain
/// iff it is the only on-some-chain element with its `ending` value.
/// `O(k log k)` with patience-sorting tails.
fn in_every_longest_chain(anchors: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let k = anchors.len();
    if k == 0 {
        return Vec::new();
    }
    let ending = chain_lengths(anchors.iter().map(|&(o, _)| o));
    // A chain starting at i read backwards is a chain of DEcreasing old
    // positions; flipping the values turns it into an increasing one.
    let mut starting = chain_lengths(anchors.iter().rev().map(|&(o, _)| usize::MAX - o));
    starting.reverse();
    let longest = ending.iter().copied().max().unwrap_or(0);

    let on_some: Vec<bool> = (0..k)
        .map(|i| ending[i] + starting[i] - 1 == longest)
        .collect();
    let mut per_length = vec![0u32; longest + 1];
    for i in 0..k {
        if on_some[i] {
            per_length[ending[i]] += 1;
        }
    }
    (0..k)
        .filter(|&i| on_some[i] && per_length[ending[i]] == 1)
        .map(|i| anchors[i])
        .collect()
}

/// For each value, the length of the longest strictly increasing chain that
/// ends with it (patience sorting: `tails[l]` is the smallest last value of a
/// chain of length `l + 1` seen so far).
fn chain_lengths(values: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut tails: Vec<usize> = Vec::new();
    values
        .map(|v| {
            let at = tails.partition_point(|&t| t < v);
            if at == tails.len() {
                tails.push(v);
            } else {
                tails[at] = v;
            }
            at + 1
        })
        .collect()
}

#[cfg(test)]
pub(super) mod tests {
    use std::time::Instant;

    use super::*;
    use crate::provenance::line::LineHasher;

    fn h(s: &str) -> LineHash {
        LineHasher::default().hash_line(s.as_bytes())
    }

    fn hs(s: &str) -> Vec<LineHash> {
        s.split_whitespace().map(h).collect()
    }

    /// `new` index → `old` index, or None.
    fn m(old: &str, new: &str) -> Vec<Option<usize>> {
        match_lines(&hs(old), &hs(new))
    }

    #[test]
    fn readme_example_rotation_keeps_the_longest_unique_chain() {
        // A B C D E → C D E A B: C D E keep origins, A B unmatched.
        assert_eq!(
            m("A B C D E", "C D E A B"),
            [Some(2), Some(3), Some(4), None, None]
        );
    }

    #[test]
    fn readme_example_tied_chains_match_nothing() {
        // A B C D → C D A B: chains `A B` and `C D` tie, no anchor is in both.
        assert_eq!(m("A B C D", "C D A B"), [None; 4]);
    }

    #[test]
    fn readme_example_duplicate_in_gap_stops() {
        // A x B → A x x B: A and B kept, the `x` gap differs and has no anchor.
        assert_eq!(m("A x B", "A x x B"), [Some(0), None, None, Some(2)]);
    }

    #[test]
    fn identical_states_match_line_for_line_even_with_duplicates() {
        assert_eq!(m("x x x", "x x x"), [Some(0), Some(1), Some(2)]);
        assert_eq!(m("", ""), Vec::<Option<usize>>::new());
    }

    #[test]
    fn uniqueness_is_recounted_inside_a_gap() {
        // `x` occurs twice overall, but once in each gap around anchor M.
        assert_eq!(
            m("x M x", "x y M x"),
            // Left gap `x` vs `x y`: x unique in both sides there.
            [Some(0), None, Some(1), Some(2)]
        );
    }

    #[test]
    fn one_side_empty_matches_nothing() {
        assert_eq!(m("a b", ""), Vec::<Option<usize>>::new());
        assert_eq!(m("", "a b"), [None, None]);
    }

    /// Brute force: enumerate every subset of the anchors, keep the longest
    /// increasing ones, intersect them. Checks the `ending/starting` trick.
    fn brute_every(anchors: &[(usize, usize)]) -> Vec<(usize, usize)> {
        let k = anchors.len();
        let mut best = 0;
        let mut chains: Vec<u32> = Vec::new();
        for mask in 0u32..(1 << k) {
            let picked: Vec<usize> = (0..k).filter(|i| mask & (1 << i) != 0).collect();
            let increasing = picked.windows(2).all(|w| anchors[w[0]].0 < anchors[w[1]].0);
            if !increasing {
                continue;
            }
            match picked.len().cmp(&best) {
                std::cmp::Ordering::Greater => {
                    best = picked.len();
                    chains = vec![mask];
                }
                std::cmp::Ordering::Equal => chains.push(mask),
                std::cmp::Ordering::Less => {}
            }
        }
        let every = chains.iter().fold(u32::MAX, |acc, c| acc & c);
        if k == 0 {
            return Vec::new();
        }
        (0..k)
            .filter(|i| every & (1 << i) != 0)
            .map(|i| anchors[i])
            .collect()
    }

    #[test]
    fn every_longest_chain_agrees_with_brute_force() {
        // All permutations of up to 7 anchors (5040 at the largest size):
        // exhaustive, deterministic, no randomness needed.
        fn permutations(n: usize) -> Vec<Vec<usize>> {
            if n == 0 {
                return vec![Vec::new()];
            }
            let mut out = Vec::new();
            for p in permutations(n - 1) {
                for at in 0..=p.len() {
                    let mut q = p.clone();
                    q.insert(at, n - 1);
                    out.push(q);
                }
            }
            out
        }
        for n in 0..=7 {
            for perm in permutations(n) {
                let anchors: Vec<(usize, usize)> =
                    perm.iter().enumerate().map(|(j, &o)| (o, j)).collect();
                assert_eq!(
                    in_every_longest_chain(&anchors),
                    brute_every(&anchors),
                    "{perm:?}"
                );
            }
        }
    }

    /// `[u2 u1 u3 u2 u4 u3 … un u(n-1) Z un]` against the same with `Z → Z'`.
    /// Every u_i occurs twice except u1, and each level of the work list
    /// finds exactly one anchor (the u whose twin the previous level peeled
    /// into a one-line gap), leaving everything else in ONE gap: n levels of
    /// linear work.
    pub(crate) fn pathological(n: usize) -> (Vec<LineHash>, Vec<LineHash>) {
        let u = |i: usize| h(&format!("u{i}"));
        let mut old = vec![u(2), u(1)];
        for i in 2..n {
            old.push(u(i + 1));
            old.push(u(i));
        }
        let mut new = old.clone();
        old.push(h("Z"));
        new.push(h("Z'"));
        old.push(u(n));
        new.push(u(n));
        (old, new)
    }

    #[test]
    fn pathological_nesting_without_a_budget_matches_everything_but_z() {
        // The README semantics, unbounded: only the changed `Z` is unmatched.
        let (old, new) = pathological(1_000);
        let got = match_lines_within(&old, &new, usize::MAX);
        assert_eq!(got.iter().filter(|x| x.is_none()).count(), 1);
        // At 200 lines (~20k visits) the default budget is not reached, so
        // the answer is exactly the README's; at 2,000 lines it already is.
        let (old, new) = pathological(100);
        let full = match_lines_within(&old, &new, usize::MAX);
        assert_eq!(full.iter().filter(|x| x.is_none()).count(), 1);
        assert_eq!(match_lines(&old, &new), full);
    }

    #[test]
    fn the_budget_bounds_pathological_nesting_and_never_mismatches() {
        // 40,000 lines: unbounded this is ~400M line visits (minutes in a
        // debug build); the budget stops it after ~1.3M.
        let (old, new) = pathological(20_000);
        let start = Instant::now();
        let got = match_lines(&old, &new);
        let took = start.elapsed();
        eprintln!("pathological, {} lines, budgeted: {took:?}", new.len());
        // `new` equals `old` line for line except `Z`, so the only correct
        // match for line j is line j: the budget may leave lines unmatched,
        // never match one wrongly.
        for (j, m) in got.iter().enumerate() {
            assert!(m.is_none_or(|k| k == j), "line {j} matched to {m:?}");
        }
        assert!(got[new.len() - 2].is_none(), "Z' is never matched");
        assert!(took.as_secs() < 10, "took {took:?}");
    }

    #[test]
    fn the_budget_floor_leaves_small_files_alone() {
        assert!(work_budget(0) >= MIN_WORK);
        // A 100-line file may be re-scanned in full hundreds of times.
        assert!(work_budget(200) / 200 > 300);
    }
}
