//! The replay state machine: one file, one op at a time.
//!
//! The state between two ops is either KNOWN (the file is absent, or a list of
//! lines with their hash and origin) or UNRESOLVED (an op happened whose
//! effect WHY cannot reproduce; only a later `before` brings the state back).
//!
//! Origins are kept as `Org` (a `Copy` index into the records) while
//! replaying, and turned into `LineOrigin`s (which clone an `EditSource` with
//! several `String`s) only once at the end: an op rebuilds the line vector,
//! and cloning sources for every line of every op would dominate the cost.
//!
//! # Uncertain lines and placement groups
//!
//! An ambiguous Codex hunk (rule 7) leaves lines whose text and origin depend
//! on which of its `k` possible placements is true. Those lines are linked: in
//! `[x, x]`, `-x +y` gives `[y, x]` OR `[x, y]`, never `[y, y]`. So each such
//! hunk becomes a GROUP with one "still possible" flag per placement, and each
//! uncertain line stores its value (hash and origin) under every placement of
//! its group. Anything later that observes those lines (a hunk's context or
//! removed lines at a known position, a `before` snapshot) must be consistent
//! with ONE placement for all of them at once; it then narrows the group,
//! which can even resolve the origins (a `before` that shows which twin
//! changed tells exactly who wrote it). A later hunk that could only be
//! placed ambiguously over uncertain lines would need placements of
//! placements: not modelled, the state becomes unknown (`AmbiguousMatch`).

use std::collections::BTreeMap;

use super::matcher::match_lines;
use super::{LineAlternatives, ReplayInput, ReplayOutput, ReplayReport, ReplayedLine, Unresolved};
use crate::provenance::edit::{
    EditHunk, EditKind, EditOp, EditRecord, FileSnapshot, HunkLineKind, Observed,
};
use crate::provenance::line::{FileFingerprint, LineHash, LineHasher};
use crate::provenance::origin::{LineOrigin, UnknownReason};

/// The ONE hasher the engine uses, to check an `after` fingerprint and a
/// `before` snapshot's own consistency (both need a digest recomputed).
/// `ReplayInput` carries only hashes, and today every hash is the unkeyed
/// `v1` recipe. When a keyed scheme arrives (see `line.rs`), `ReplayInput`
/// must carry the hasher instead and this constant goes away.
const HASHER: LineHasher = LineHasher::unkeyed_v1();

/// An ambiguous hunk with more possible places than this is not merged line
/// by line (each uncertain line stores one value per place): the state
/// becomes unknown instead. Keeps the work per op linear in the file.
const MAX_PLACEMENTS: usize = 64;

/// A line's origin while replaying. `Edit` indexes `ReplayInput::records`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Org {
    Base,
    Edit(usize),
    Unknown(UnknownReason),
}

/// One line of the replayed state. For an uncertain line (`uncertain` set),
/// `hash` is one possible text and `org` the origin common to every still
/// possible placement (or `AmbiguousMatch`); both are derived from the line's
/// `Uncertain` entry and refreshed whenever its group narrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Line {
    hash: LineHash,
    org: Org,
    /// Index into `Engine::uncertain`.
    uncertain: Option<usize>,
}

impl Line {
    fn certain(hash: LineHash, org: Org) -> Self {
        Self {
            hash,
            org,
            uncertain: None,
        }
    }
}

/// The value of one uncertain line under each placement of its group.
#[derive(Debug, Clone)]
struct Uncertain {
    group: usize,
    /// `values[c]`: (hash, origin) if placement `c` is the true one.
    values: Vec<(LineHash, Org)>,
}

/// The placements of one ambiguous hunk that are still possible.
#[derive(Debug, Clone)]
struct Group {
    alive: Vec<bool>,
}

/// An existing file.
#[derive(Debug, Clone)]
struct File {
    lines: Vec<Line>,
    /// `None`: unknown (see `ReplayOutput`).
    final_newline: Option<bool>,
}

enum State {
    /// `None`: the file is absent.
    Known(Option<File>),
    Unresolved {
        /// The last state WHY knew, just before `since`.
        last: Option<File>,
        since: usize,
        reason: UnknownReason,
    },
}

/// What an op claims to see on uncertain lines: (`Engine::uncertain` index,
/// the hash it says that line has).
type Observations = Vec<(usize, LineHash)>;

struct Engine<'a> {
    records: &'a [EditRecord],
    /// Append-only; `Line::uncertain` indexes it.
    uncertain: Vec<Uncertain>,
    groups: Vec<Group>,
}

pub(super) fn run(input: &ReplayInput) -> ReplayReport {
    // Stable sort: equal keys (which the store never produces) keep input
    // order, so the result is still a function of the input.
    let mut order: Vec<usize> = (0..input.records.len()).collect();
    order.sort_by_key(|&i| (input.records[i].seq, input.records[i].op_index));

    let mut engine = Engine {
        records: &input.records,
        uncertain: Vec::new(),
        groups: Vec::new(),
    };
    let mut state = State::Known(input.baseline.as_ref().map(|b| {
        File {
            lines: b
                .lines
                .iter()
                .map(|&hash| Line::certain(hash, Org::Base))
                .collect(),
            final_newline: Some(b.fingerprint.final_newline),
        }
    }));
    for i in order {
        state = engine.step(state, i);
    }
    engine.report(state)
}

impl Engine<'_> {
    fn step(&mut self, state: State, i: usize) -> State {
        let op = &self.records[i].op;
        match op.observed {
            // Rule 6: an edit happened, its effect is unknown. `kind` is not
            // trusted for an unobserved op (see `EditOp::kind`), so even
            // whether the file still exists is unknown now.
            Observed::Unobserved(reason) => match state {
                State::Known(last) => State::Unresolved {
                    last,
                    since: i,
                    reason: reason.into(),
                },
                // Already unknown: the earliest op and reason stay.
                unresolved @ State::Unresolved { .. } => unresolved,
            },
            Observed::Exact => match op.kind {
                // A delete or a create defines the whole file by itself, so it
                // also ends an unresolved stretch (no line hash is recovered:
                // nothing of the old state survives either op).
                EditKind::Delete => State::Known(None),
                EditKind::Create => self.create(state, i, op),
                EditKind::Update => self.update(state, i, op),
            },
        }
    }

    fn create(&mut self, state: State, i: usize, op: &EditOp) -> State {
        let mut lines = Vec::new();
        for line in op.hunks.iter().flat_map(|h| &h.lines) {
            if line.kind != HunkLineKind::Added {
                // A create shows only added lines; anything else means the
                // record is not what WHY thinks it is.
                return fail(state, i, UnknownReason::ReplayMismatch);
            }
            lines.push(Line::certain(line.hash, Org::Edit(i)));
        }
        if op
            .after
            .is_some_and(|after| !fingerprint_matches(&lines, after))
        {
            return fail(state, i, UnknownReason::ReplayMismatch);
        }
        let final_newline = newline_after(None, &lines, op.after, false);
        State::Known(Some(File {
            lines,
            final_newline,
        }))
    }

    fn update(&mut self, state: State, i: usize, op: &EditOp) -> State {
        // A snapshot whose fingerprint does not describe its own lines is a
        // damaged row: not evidence of anything, treated as absent.
        let before = op.before.as_ref().filter(|b| b.is_consistent(&HASHER));
        let pre = match (state, before) {
            (State::Known(file), Some(before)) => {
                Some(self.sync(file, before, UnknownReason::NoCapture))
            }
            // Rule 6: the `before` shows what the unobserved op (or the op
            // WHY could not apply) left; what changed gets ITS reason.
            (State::Unresolved { last, reason, .. }, Some(before)) => {
                Some(self.sync(last, before, reason))
            }
            (State::Known(file), None) => file,
            // Only a full `before` can resume the replay; an `after`
            // fingerprint could verify a state but never supply its lines.
            (unresolved @ State::Unresolved { .. }, None) => return unresolved,
        };
        let Some(pre) = pre else {
            // An update of a file WHY believes absent, and no `before` that
            // shows what it held: there is nothing to place the hunks in.
            return State::Unresolved {
                last: None,
                since: i,
                reason: UnknownReason::ReplayMismatch,
            };
        };
        let (lines, reaches_end) = match self.apply_hunks(&pre.lines, i, &op.hunks) {
            Ok(applied) => applied,
            // Never shift a hunk to make it fit: the state is unknown now.
            Err(reason) => {
                return State::Unresolved {
                    last: Some(pre),
                    since: i,
                    reason,
                };
            }
        };
        if op
            .after
            .is_some_and(|after| !fingerprint_matches(&lines, after))
        {
            // The harness says the file became something else than what the
            // replay computed. A fingerprint cannot say which lines differ,
            // so this op's result is not trusted at all.
            return State::Unresolved {
                last: Some(pre),
                since: i,
                reason: UnknownReason::ReplayMismatch,
            };
        }
        let final_newline = newline_after(Some(&pre), &lines, op.after, reaches_end);
        State::Known(Some(File {
            lines,
            final_newline,
        }))
    }

    /// The state the harness reported in `before`, with the origins the
    /// replay can prove for it. `reason` is what lines the replay cannot
    /// explain get: `NoCapture` (rule 5), or an unobserved op's (rule 6).
    fn sync(&mut self, file: Option<File>, before: &FileSnapshot, reason: UnknownReason) -> File {
        let mut old = file.map(|f| f.lines).unwrap_or_default();
        // The normal case: `before` is a state the replay allows. Certain
        // lines must be equal; uncertain ones must fit ONE placement of each
        // group together (then they become certain, origins resolved).
        let compatible = old.len() == before.lines.len()
            && old
                .iter()
                .zip(&before.lines)
                .all(|(l, &h)| l.uncertain.is_some() || l.hash == h)
            && self.narrow(
                &old.iter()
                    .zip(&before.lines)
                    .filter_map(|(l, &h)| l.uncertain.map(|u| (u, h)))
                    .collect::<Vec<_>>(),
            );
        let lines = if compatible {
            self.refresh(&mut old);
            old
        } else {
            // Something uncaptured changed the file (or `before` contradicts
            // the placements): rule 5. Uncertain lines take part with their
            // displayed text; a match keeps the origin they show, which is
            // `AmbiguousMatch` unless every placement agrees.
            let old_hashes: Vec<LineHash> = old.iter().map(|l| l.hash).collect();
            match_lines(&old_hashes, &before.lines)
                .into_iter()
                .zip(&before.lines)
                .map(|(m, &hash)| match m {
                    Some(k) => Line::certain(hash, old[k].org),
                    None => Line::certain(hash, Org::Unknown(reason)),
                })
                .collect()
        };
        // A gained or lost final newline is not a changed line: only the
        // flag follows `before`.
        File {
            lines,
            final_newline: Some(before.fingerprint.final_newline),
        }
    }

    /// The lines after `hunks` (of record `i`) are applied to `pre`, and
    /// whether any hunk (under any possible placement) reaches the end of the
    /// file; or why that cannot be done faithfully.
    fn apply_hunks(
        &mut self,
        pre: &[Line],
        i: usize,
        hunks: &[EditHunk],
    ) -> Result<Applied, UnknownReason> {
        // A hunk without lines changes nothing and has no place to check.
        let hunks: Vec<&EditHunk> = hunks.iter().filter(|h| !h.lines.is_empty()).collect();
        if hunks.is_empty() {
            return Ok((pre.to_vec(), false));
        }
        let positioned = hunks.iter().filter(|h| h.old_start.is_some()).count();
        if positioned == hunks.len() {
            let starts = self
                .positioned_starts(pre, &hunks)
                .ok_or(UnknownReason::ReplayMismatch)?;
            let reaches = any_reaches_end(pre.len(), &hunks, &starts);
            let pre = self.settle(pre, &hunks, &starts)?;
            Ok((splice(&pre, i, &hunks, &starts, &BTreeMap::new()), reaches))
        } else if positioned == 0 {
            self.apply_by_context(pre, i, &hunks)
        } else {
            // One op mixing positioned and position-less hunks fits neither
            // harness: not something WHY can interpret.
            Err(UnknownReason::ReplayMismatch)
        }
    }

    /// Claude: every hunk goes exactly where the harness says, or nowhere.
    fn positioned_starts(&self, pre: &[Line], hunks: &[&EditHunk]) -> Option<Vec<usize>> {
        let mut starts = Vec::with_capacity(hunks.len());
        let mut cursor = 0;
        for hunk in hunks {
            let len = old_len(hunk);
            let old_start = usize::try_from(hunk.old_start?).ok()?;
            // Unified-diff meaning: the 1-based first old line, or for a
            // pure insertion the line AFTER which it goes (0 = top). An
            // `old_start` of 0 with old lines is malformed: `checked_sub`.
            let start = if len == 0 {
                old_start
            } else {
                old_start.checked_sub(1)?
            };
            let end = start.checked_add(len)?;
            // In file order, not overlapping, inside the file, and the
            // context and removed lines really are there.
            if start < cursor || end > pre.len() || !self.fits(&pre[start..end], hunk) {
                return None;
            }
            starts.push(start);
            cursor = end;
        }
        Some(starts)
    }

    /// Hunks placed for certain at `starts` observe the lines under them;
    /// where those are uncertain, the observations must fit one placement of
    /// each group TOGETHER. Narrow the groups and return `pre` refreshed, or
    /// `ReplayMismatch`: no possible state of the file matches the hunks.
    fn settle(
        &mut self,
        pre: &[Line],
        hunks: &[&EditHunk],
        starts: &[usize],
    ) -> Result<Vec<Line>, UnknownReason> {
        let mut seen = Observations::new();
        for (hunk, &s) in hunks.iter().zip(starts) {
            seen.extend(observations(&pre[s..], hunk));
        }
        if !self.narrow(&seen) {
            return Err(UnknownReason::ReplayMismatch);
        }
        let mut pre = pre.to_vec();
        self.refresh(&mut pre);
        Ok(pre)
    }

    /// Codex: locate every hunk by its context (and header); apply it where
    /// it fits uniquely, mark what depends on the placement where it does not.
    fn apply_by_context(
        &mut self,
        pre: &[Line],
        i: usize,
        hunks: &[&EditHunk],
    ) -> Result<Applied, UnknownReason> {
        let mut candidates: Vec<Vec<usize>> = hunks
            .iter()
            .map(|h| {
                // `fits` looks at each line alone; drop the places whose
                // uncertain lines no single placement of their group allows.
                self.candidates(pre, h)
                    .into_iter()
                    .filter(|&s| self.consistent(&observations(&pre[s..], h)))
                    .collect()
            })
            .collect();

        // Hunks are in file order and do not overlap, so keep only the
        // candidates that fit in SOME complete ordered placement. Forward: a
        // hunk cannot start before the earliest possible end of the one
        // before it. Backward: it cannot end after the latest possible start
        // of the one after it. A candidate passing both lies on a full chain.
        let mut lower = 0;
        for (hunk, cands) in hunks.iter().zip(candidates.iter_mut()) {
            cands.retain(|&s| s >= lower);
            let len = old_len(hunk);
            lower = cands
                .iter()
                .map(|&s| s + len)
                .min()
                .ok_or(UnknownReason::ReplayMismatch)?;
        }
        let mut upper = pre.len();
        for (hunk, cands) in hunks.iter().zip(candidates.iter_mut()).rev() {
            let len = old_len(hunk);
            cands.retain(|&s| s + len <= upper);
            upper = *cands.iter().max().ok_or(UnknownReason::ReplayMismatch)?;
        }

        let (unique, ambiguous): (Vec<usize>, Vec<usize>) =
            (0..hunks.len()).partition(|&k| candidates[k].len() == 1);
        let unique_hunks: Vec<&EditHunk> = unique.iter().map(|&k| hunks[k]).collect();
        let unique_starts: Vec<usize> = unique.iter().map(|&k| candidates[k][0]).collect();
        let unique_reaches = any_reaches_end(pre.len(), &unique_hunks, &unique_starts);
        if ambiguous.is_empty() {
            let pre = self.settle(pre, &unique_hunks, &unique_starts)?;
            let lines = splice(&pre, i, &unique_hunks, &unique_starts, &BTreeMap::new());
            return Ok((lines, unique_reaches));
        }

        // Rule 7, for every ambiguous hunk at once, when each is independent:
        // - equal size, so placements are comparable line by line (a
        //   count-changing one, a pure insertion included, shifts every later
        //   line by an unknown amount);
        // - not too many places (one value per place is stored per line);
        // - its places never cover an uncertain line (a placement of a
        //   placement is not modelled);
        // - all places of each hunk lie between the places of its neighbours,
        //   so every combination of choices is a valid patch and the hunks'
        //   choices do not constrain each other.
        for &a in &ambiguous {
            let (hunk, len) = (hunks[a], old_len(hunks[a]));
            if removed_count(hunk) != added_count(hunk)
                || candidates[a].len() > MAX_PLACEMENTS
                || candidates[a]
                    .iter()
                    .any(|&s| pre[s..s + len].iter().any(|l| l.uncertain.is_some()))
            {
                return Err(UnknownReason::AmbiguousMatch);
            }
        }
        for k in 1..hunks.len() {
            let prev_end = candidates[k - 1]
                .iter()
                .map(|&s| s + old_len(hunks[k - 1]))
                .max()
                .unwrap_or(0);
            let next_start = candidates[k].iter().copied().min().unwrap_or(0);
            if prev_end > next_start {
                return Err(UnknownReason::AmbiguousMatch);
            }
        }

        // An ambiguous hunk may reach the end if ANY of its places does.
        let reaches = unique_reaches
            || ambiguous.iter().any(|&a| {
                candidates[a]
                    .iter()
                    .any(|&s| reaches_end(pre.len(), s, old_len(hunks[a])))
            });
        // The unique hunks are certain wherever the ambiguous ones went.
        let pre = self.settle(pre, &unique_hunks, &unique_starts)?;
        let mut overrides = BTreeMap::new();
        for &a in &ambiguous {
            self.ambiguous_lines(&pre, i, hunks[a], &candidates[a], &mut overrides);
        }
        Ok((
            splice(&pre, i, &unique_hunks, &unique_starts, &overrides),
            reaches,
        ))
    }

    /// Where `hunk` could go in `pre`: every start whose old lines match and
    /// that lies below a line equal to the header (anywhere above, as
    /// ruled). `O(n · hunk length)`.
    fn candidates(&self, pre: &[Line], hunk: &EditHunk) -> Vec<usize> {
        let len = old_len(hunk);
        let Some(last_start) = pre.len().checked_sub(len) else {
            return Vec::new();
        };
        let first = match hunk.header {
            None => 0,
            // Not found: Codex would have refused the patch, so the state is
            // not the file the patch was made for (no candidate at all). A
            // header on an uncertain line counts if any placement allows it
            // (more candidates, never fewer: the safe direction).
            Some(header) => match pre.iter().position(|l| self.is(l, header)) {
                Some(at) => at + 1,
                None => return Vec::new(),
            },
        };
        (first..=last_start)
            .filter(|&s| self.fits(&pre[s..s + len], hunk))
            .collect()
    }

    /// Rule 7 for one equal-size hunk with several possible `starts` (none
    /// covering an uncertain line): every line it may touch, merged over its
    /// placements, as a new group. A line whose value is the same under every
    /// placement stays certain; otherwise it records its value per placement.
    /// `O(touched lines × placements)`.
    fn ambiguous_lines(
        &mut self,
        pre: &[Line],
        i: usize,
        hunk: &EditHunk,
        starts: &[usize],
        overrides: &mut BTreeMap<usize, Line>,
    ) {
        let len = old_len(hunk);
        // The new side, once: each line either comes from the old side at
        // some offset (context: keeps that line's origin) or is added.
        let template: Vec<Result<usize, LineHash>> = {
            let mut old_offset = 0;
            hunk.lines
                .iter()
                .filter_map(|l| match l.kind {
                    HunkLineKind::Context => {
                        old_offset += 1;
                        Some(Ok(old_offset - 1))
                    }
                    HunkLineKind::Removed => {
                        old_offset += 1;
                        None
                    }
                    HunkLineKind::Added => Some(Err(l.hash)),
                })
                .collect()
        };
        let group = self.groups.len();
        self.groups.push(Group {
            alive: vec![true; starts.len()],
        });
        let touched: std::collections::BTreeSet<usize> =
            starts.iter().flat_map(|&s| s..s + len).collect();
        for j in touched {
            let values: Vec<(LineHash, Org)> = starts
                .iter()
                .map(|&s| {
                    if (s..s + len).contains(&j) {
                        match template.get(j - s) {
                            Some(&Ok(offset)) => (pre[s + offset].hash, pre[s + offset].org),
                            Some(&Err(hash)) => (hash, Org::Edit(i)),
                            // Equal size makes this unreachable; the
                            // untouched line is the harmless fallback.
                            None => (pre[j].hash, pre[j].org),
                        }
                    } else {
                        (pre[j].hash, pre[j].org)
                    }
                })
                .collect();
            if values.iter().all(|v| *v == values[0]) {
                // Certain, but NOT necessarily the old line: when every
                // placement covers j (overlapping places), every placement
                // may rewrite it the same way (`[w w w w]`, `-w -w +w +w  w`
                // makes line 2 the op's under both places).
                let (hash, org) = values[0];
                overrides.insert(j, Line::certain(hash, org));
                continue;
            }
            self.uncertain.push(Uncertain { group, values });
            let u = self.uncertain.len() - 1;
            overrides.insert(j, self.derive(u, pre[j].hash));
        }
    }

    /// Whether `seen` fits one still-possible placement of every group.
    fn consistent(&self, seen: &[(usize, LineHash)]) -> bool {
        self.survivors(seen).is_some()
    }

    /// Narrow every group to the placements `seen` allows; `false` (and no
    /// change) if some group would have none left.
    fn narrow(&mut self, seen: &[(usize, LineHash)]) -> bool {
        match self.survivors(seen) {
            Some(narrowed) => {
                for (group, alive) in narrowed {
                    self.groups[group].alive = alive;
                }
                true
            }
            None => false,
        }
    }

    fn survivors(&self, seen: &[(usize, LineHash)]) -> Option<BTreeMap<usize, Vec<bool>>> {
        let mut narrowed: BTreeMap<usize, Vec<bool>> = BTreeMap::new();
        for &(u, hash) in seen {
            let entry = &self.uncertain[u];
            let alive = narrowed
                .entry(entry.group)
                .or_insert_with(|| self.groups[entry.group].alive.clone());
            for (c, flag) in alive.iter_mut().enumerate() {
                *flag &= entry.values[c].0 == hash;
            }
        }
        narrowed
            .values()
            .all(|alive| alive.contains(&true))
            .then_some(narrowed)
    }

    /// Re-derive every uncertain line from its group's current placements.
    fn refresh(&self, lines: &mut [Line]) {
        for line in lines.iter_mut() {
            if let Some(u) = line.uncertain {
                *line = self.derive(u, line.hash);
            }
        }
    }

    /// An uncertain line as its group now allows: shown text `prefer` if
    /// still possible, origin common to all live placements or
    /// `AmbiguousMatch`. Certain again once one value is left.
    fn derive(&self, u: usize, prefer: LineHash) -> Line {
        let entry = &self.uncertain[u];
        let alive = &self.groups[entry.group].alive;
        let live: Vec<(LineHash, Org)> = entry
            .values
            .iter()
            .zip(alive)
            .filter(|&(_, &a)| a)
            .map(|(&v, _)| v)
            .collect();
        let Some(&(first_hash, first_org)) = live.first() else {
            // A group always keeps one placement (`narrow` refuses to empty
            // it); answer honestly if that invariant ever breaks.
            return Line::certain(prefer, Org::Unknown(UnknownReason::AmbiguousMatch));
        };
        let hash = if live.iter().any(|&(h, _)| h == prefer) {
            prefer
        } else {
            first_hash
        };
        let org = if live.iter().all(|&(_, o)| o == first_org) {
            first_org
        } else {
            Org::Unknown(UnknownReason::AmbiguousMatch)
        };
        let settled = live.iter().all(|&v| v == (first_hash, first_org));
        Line {
            hash,
            org,
            uncertain: (!settled).then_some(u),
        }
    }

    /// `line` may be (one of) the text `hash`.
    fn is(&self, line: &Line, hash: LineHash) -> bool {
        line.hash == hash
            || line
                .uncertain
                .is_some_and(|u| self.possible(u).any(|h| h == hash))
    }

    /// The texts an uncertain line may still have.
    fn possible(&self, u: usize) -> impl Iterator<Item = LineHash> + '_ {
        let entry = &self.uncertain[u];
        entry
            .values
            .iter()
            .zip(&self.groups[entry.group].alive)
            .filter(|&(_, &a)| a)
            .map(|(&(h, _), _)| h)
    }

    /// The hunk's context and removed lines are exactly `window` (each line
    /// on its own; joint consistency is `consistent`'s job).
    fn fits(&self, window: &[Line], hunk: &EditHunk) -> bool {
        let mut window = window.iter();
        old_side(hunk).all(|want| window.next().is_some_and(|l| self.is(l, want)))
            && window.next().is_none()
    }

    fn report(&self, state: State) -> ReplayReport {
        let (file, unresolved) = match state {
            State::Known(file) => (file, None),
            State::Unresolved {
                last,
                since,
                reason,
            } => (
                last,
                Some(Unresolved {
                    since: self.records[since].source.clone(),
                    reason,
                }),
            ),
        };
        let mut alternatives = Vec::new();
        let output = match file {
            None => ReplayOutput::default(),
            Some(file) => ReplayOutput {
                lines: file
                    .lines
                    .iter()
                    .enumerate()
                    .map(|(index, line)| {
                        if let Some(u) = line.uncertain {
                            let mut others: Vec<LineHash> = Vec::new();
                            for h in self.possible(u) {
                                if h != line.hash && !others.contains(&h) {
                                    others.push(h);
                                }
                            }
                            // An uncertain ORIGIN with a certain text has no
                            // alternative texts to report.
                            if !others.is_empty() {
                                alternatives.push(LineAlternatives { index, others });
                            }
                        }
                        ReplayedLine {
                            hash: line.hash,
                            origin: self.origin(line.org),
                        }
                    })
                    .collect(),
                final_newline: file.final_newline,
            },
        };
        ReplayReport {
            output,
            alternatives,
            unresolved,
        }
    }

    fn origin(&self, org: Org) -> LineOrigin {
        match org {
            Org::Base => LineOrigin::Base,
            Org::Edit(i) => LineOrigin::Edit(self.records[i].source.clone()),
            Org::Unknown(reason) => LineOrigin::Unknown(reason),
        }
    }
}

/// What `hunk`, placed at the start of `window`, says about its uncertain
/// lines (context and removed lines alike).
fn observations(window: &[Line], hunk: &EditHunk) -> Observations {
    window
        .iter()
        .zip(old_side(hunk))
        .filter_map(|(l, want)| l.uncertain.map(|u| (u, want)))
        .collect()
}

/// The state cannot follow an op: it becomes unknown from `i` on, unless it
/// already was (then the earlier op stays the explanation).
fn fail(state: State, i: usize, reason: UnknownReason) -> State {
    match state {
        State::Known(last) => State::Unresolved {
            last,
            since: i,
            reason,
        },
        unresolved @ State::Unresolved { .. } => unresolved,
    }
}

/// Apply hunks at known starts (`starts[k]` for `hunks[k]`, in order,
/// already checked and settled). Lines outside every hunk are copied,
/// through `overrides` (rule 7's merged lines) when present.
fn splice(
    pre: &[Line],
    i: usize,
    hunks: &[&EditHunk],
    starts: &[usize],
    overrides: &BTreeMap<usize, Line>,
) -> Vec<Line> {
    let added: usize = hunks.iter().map(|h| added_count(h)).sum();
    let mut out = Vec::with_capacity(pre.len() + added);
    let copy = |out: &mut Vec<Line>, from: usize, to: usize| {
        out.extend((from..to).map(|j| overrides.get(&j).copied().unwrap_or(pre[j])));
    };
    let mut cursor = 0;
    for (hunk, &start) in hunks.iter().zip(starts) {
        copy(&mut out, cursor, start);
        let mut k = start;
        for line in &hunk.lines {
            match line.kind {
                // Rule 2: context keeps its origin. (`settle` made it
                // certain, so its text is the hunk's.)
                HunkLineKind::Context => {
                    out.push(Line::certain(line.hash, pre[k].org));
                    k += 1;
                }
                // Rule 3: removed is gone.
                HunkLineKind::Removed => k += 1,
                // Rule 1: the op that wrote the line.
                HunkLineKind::Added => out.push(Line::certain(line.hash, Org::Edit(i))),
            }
        }
        cursor = k;
    }
    copy(&mut out, cursor, pre.len());
    out
}

/// Lines after an op, and whether one of its hunks reaches the file's end.
type Applied = (Vec<Line>, bool);

/// A hunk at `start` with `old_len` old lines, in a file of `len` lines,
/// includes the last line or inserts after it. (In an empty file a pure
/// insertion at 0 does: it writes the last line.)
fn reaches_end(len: usize, start: usize, old_len: usize) -> bool {
    start + old_len == len
}

fn any_reaches_end(len: usize, hunks: &[&EditHunk], starts: &[usize]) -> bool {
    hunks
        .iter()
        .zip(starts)
        .any(|(h, &s)| reaches_end(len, s, old_len(h)))
}

/// The final-newline flag after an op that left `lines`.
///
/// Only the end of a file decides whether it ends with a newline. A hunk
/// strictly inside the file cannot change that, so such an op provably keeps
/// the flag. A hunk that includes the last line or appends after it may have
/// changed it, and the hunks cannot say: the adapter drops Claude's
/// `\ No newline at end of file` marker (`adapters/claude/edit_op.rs`) and
/// `EditHunk` has no field for it, so without an `after` fingerprint the
/// flag is unknown (rule A: a newline only the file shows).
fn newline_after(
    pre: Option<&File>,
    lines: &[Line],
    after: Option<FileFingerprint>,
    reaches_end: bool,
) -> Option<bool> {
    if let Some(after) = after {
        // Checked by the caller to describe `lines`.
        return Some(after.final_newline);
    }
    if lines.is_empty() {
        // An existing file with no lines cannot end with a newline.
        return Some(false);
    }
    match pre {
        Some(pre) if !pre.lines.is_empty() && !reaches_end => pre.final_newline,
        // Touched the end (an edit that grew an empty file included), or
        // lines appeared from nothing (a create) with no `after`: nothing
        // observed says whether the last line ends with a newline.
        _ => None,
    }
}

fn fingerprint_matches(lines: &[Line], after: FileFingerprint) -> bool {
    let hashes: Vec<LineHash> = lines.iter().map(|l| l.hash).collect();
    let fp = HASHER.fingerprint_of(&hashes, after.final_newline);
    fp.line_count == after.line_count && fp.lines_digest == after.lines_digest
}

/// The hunk's old side: context and removed lines, in order.
fn old_side(hunk: &EditHunk) -> impl Iterator<Item = LineHash> + '_ {
    hunk.lines
        .iter()
        .filter(|l| l.kind != HunkLineKind::Added)
        .map(|l| l.hash)
}

fn old_len(hunk: &EditHunk) -> usize {
    old_side(hunk).count()
}

fn added_count(hunk: &EditHunk) -> usize {
    hunk.lines
        .iter()
        .filter(|l| l.kind == HunkLineKind::Added)
        .count()
}

fn removed_count(hunk: &EditHunk) -> usize {
    hunk.lines
        .iter()
        .filter(|l| l.kind == HunkLineKind::Removed)
        .count()
}
