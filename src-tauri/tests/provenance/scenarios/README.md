# Line-provenance ground truth (format 1)

Each `*.json` file in this folder is ONE scenario: a file's base content, the
edits agents made to it (as the hooks report them), the file at the end, and
the true origin of every final line. The test binary `tests/provenance` loads
and validates every file here (`cargo test --test provenance`); a file the
loader refuses fails the build.

The scenarios are written with **readable line text**. The loader hashes every
line with WHY's one line-hash policy (`src/provenance/line.rs`), so the replay
engine only ever sees hashes, exactly as in production (decision 29).

**Decision 33: the scenarios and the engine have different authors.** WHY-21
writes scenarios; WHY-22 writes the engine. The reference applier in
`../truth.rs` is the ORACLE that checks a scenario is self-consistent. The
engine must not use, copy or port it: if oracle and engine shared code, "zero
false attributions on the fixtures" would only prove the code agrees with
itself.

## Shape

```jsonc
{
  "format": 1,                      // required; any other number is refused
  "name": "claude-plain-edit",      // unique across the folder (use the file name)
  "description": "What happens and why the expected answer is right.",
  "stage": "replay",                // "replay" | "align" (see below)
  "harness": "claudeCode",          // "claudeCode" | "codex"
  "file": "src/main.rs",            // canonical relative path (no "./", no "..")
  "base": { "lines": ["..."], "finalNewline": true },   // or null: the file did not exist
  "ops": [ /* one per captured edit, in arrival order */ ],
  "final": { "lines": ["..."], "finalNewline": true },  // or null: the file is gone
  "expected": [ { "line": 1, "origin": "base" } /* , ... */ ]
}
```

Unknown fields are an error everywhere (a typo never goes unnoticed). `base`
and `final` are REQUIRED keys: write `null` for "no such file", never leave
the key out. A file with no lines has `"finalNewline": false` (`"\n"` is one
empty line, not an empty file).

A line is its text **without** a terminator. A line may not contain `\n` and
may not end with `\r`: under the CRLF policy, `a\r\n` on disk is the line `a`,
so writing `"a\r"` would describe a file that cannot exist. A lone `\r` inside
a line is fine (it is content).

### An op

```jsonc
{
  "id": "e1",                 // unique within the scenario; `expected` refers to it
  "session": "s1",            // harness session id (the harness is the scenario's)
  "turn": "t1",               // Claude prompt_id / Codex turn_id
  "kind": "update",           // "create" | "update" | "delete"
  "renamedFrom": "old.rs",    // optional, canonical path (Codex "*** Move to:")
  "observed": "exact",        // optional, default "exact"; or { "unobserved": "payloadReduced" | "unparsed" }
  "hunks": [ /* see below */ ],
  "before": "auto",           // "auto" | "none" | { "lines": [...], "finalNewline": bool }
  "after": "none",            // "auto" | "none"
  "finalNewline": true,       // optional, TRUTH ONLY: the file's final newline after this op
  "truthAfter": { ... }       // TRUTH ONLY, required for (and only for) an unobserved op
}
```

- `before` is what the harness reported as the file before the edit (Claude's
  `originalFile`, sent to the engine as the full line-hash sequence).
  - `"auto"`: the loader computes it from the replayed state (the base plus
    every earlier op).
  - `"none"`: the payload has no old file (Codex always; a created file).
  - `{ "lines": ... }`: an explicit old file that MAY DIFFER from the replayed
    state. This is how a scenario says "something changed the file between
    two edits" (a `cargo fmt`, a `sed`, a human). The op applies to this
    explicit state, and the engine is expected to resync from it.
- `after`: `"auto"` sends the fingerprint of the file after the op (Claude
  `Write` carries the whole new content); `"none"` sends nothing (Claude
  `Edit`, Codex).
- Codex scenarios must use `"none"` for both: Codex reports neither.
- `kind: "create"`: the file must not exist yet (`base: null`, or deleted by an
  earlier op). Exactly one hunk of only `+` lines, with no position: that is
  how the adapter presents a Claude `Write` create (whose `structuredPatch` is
  empty) and a Codex `*** Add File`. A create with NO hunks creates an empty
  file (zero lines, no final newline): it exists, unlike a deleted file, and
  differs from a file holding one blank line (one `"+"` line).
- `kind: "delete"`: no hunks.
- An **unobserved** op (the hook reduced the payload, or it could not be
  parsed) has no hunks and `"none"` before/after, because WHY saw no content.
  Its `truthAfter` says what the file really became; the engine never sees it.
  No line may name an unobserved op as its origin: lines it touched are
  `unknown` (usually `payloadReduced` / `unparsed`).

### A hunk

```jsonc
{
  "oldStart": 3,              // Claude only: the harness's own position (sent to the engine)
  "appliedAt": 3,             // Codex only, TRUTH ONLY: where the hunk really applied
  "header": "def total(xs):", // Codex only, optional: the `@@ <text>` hint; must be a line above the hunk
  "lines": [" context", "-removed", "+added"]
}
```

- Lines are in **patch order** (context, `-` and `+` interleaved as the patch
  has them), each prefixed with `" "`, `"-"` or `"+"`. `" "` alone is an empty
  context line.
- Positions have unified-diff meaning, in the file BEFORE this op: the 1-based
  line number of the hunk's first context-or-removed line; for a hunk of only
  `+` lines, the line AFTER which they are inserted (`0` = at the top). Hunks
  of one op are in file order and do not overlap. (The pure-insertion meaning
  is WHY's own definition: every Claude hunk observed so far has context
  lines; the adapter converts if Claude turns out to send something else.)
- A create's hunk has no `oldStart`, `appliedAt` or `header`.
- Claude hunks need `oldStart` and must not have `appliedAt`. Codex hunks must
  not have `oldStart` (Codex gives none) and **must** have `appliedAt`: the
  author always knows where the patch really went, even when the context is
  ambiguous for the engine.
- The context and removed lines must match the file at that position exactly,
  or the scenario is refused.

### Stages

- `replay`: `final` equals what applying the ops produces (the loader checks
  it). WHY-22's engine must reproduce `expected` exactly.
- `align`: the engine needs the real final file to be judged, either because
  something uncaptured changed it after the last op (a person, a shell write, a
  formatter, a rebase), or because the ops alone do not determine its text
  (ambiguous placement, an unobserved op that no later `before` reveals, a
  final newline only the file shows). The right answer needs alignment with the
  real file (WHY-23). `final` may therefore equal the oracle's replay: the
  oracle knows `appliedAt` / `truthAfter`, the engine does not.

### Expected

One entry per line of `final`, `line` 1..=n in order. `origin` is one of:

- `"base"`: unchanged since the base;
- `{ "op": "e1" }`: written by that op (which must be observed);
- `{ "unknown": "<reason>" }` with reason one of `noCapture`, `payloadReduced`,
  `unparsed`, `replayMismatch`, `ambiguousMatch`, `concurrentWriter`.
  (`notReplayed` is the stub's answer and is refused as ground truth.)

There is no "manual" or "human" origin: a line no captured edit explains is
`unknown`, because WHY cannot tell a person from a shell command.

## What the engine receives

`Scenario::inputs()` returns a `ReplayInput` and the final file's hashes:

- `baseline`: a snapshot of `base` (line hashes + fingerprint, so its final
  newline is known), or `None` when `base` is null (absent, not empty);
- one `EditRecord` per op, in file order: `seq` = 1, 2, ...; `op_index` = 0;
  `source.event_key` = `"truth:<op id>"`, `source.tool_use_id` = the op id,
  `source.turn_id` = `turn`, `source.session` = (harness, `session`);
- each `EditOp` carries hashes only: `before` (full snapshot), `after`
  (fingerprint), hunks with `oldStart` and the hashed `header`. Never
  `appliedAt`, `truthAfter` or `finalNewline`.

`Scenario::check(&ReplayOutput)` compares the engine's answer line by line and
returns a readable diff (expected vs got, with the line's text). The engine
returns each replayed line's HASH with its origin (WHY-23 aligns on the
hashes); for a `replay` scenario `check` also requires those hashes, and the
final-newline flag, to equal the final file's.

## The examples

| File | Shows |
|---|---|
| `claude-plain-edit.json` | one Claude edit of one line |
| `claude-shifted-edit.json` | a second edit whose position an earlier edit shifted |
| `codex-ambiguous-context.json` | a Codex hunk whose context matches twice: the changed line and its twin are `unknown(ambiguousMatch)` |

## Semantics (v0.3)

The rules every `expected` list is derived from (decided for v0.3, WHY-21). Each
scenario's `description` says which rule it exercises. Examples are tiny: the
base file is on the left of `→`, the final file on the right, `e1` is an op.

1. **An origin is the op that wrote the line.** Content equality never
   transfers origin.
   `[x, y] → e1 changes y to z, e2 changes z back to y → y is e2, not base.`
2. **Untouched lines keep their origin.** Context lines and every line outside
   an op's hunks keep what they had; base lines no op touched are `base`. This
   also holds for a Claude `Write` overwrite: lines its patch shows as context
   keep their origin, because the harness's own patch is the observed fact.
   `[a, b, c] → e1 changes b → a and c stay base. A rename keeps origins too.`
3. **Removed is gone.** A removed line has no origin in `final`; deleting then
   re-adding identical text gives the re-adding op (rule 1).
   `[a, b] → e1 deletes b, e2 adds b back → b is e2.`
4. **Order is arrival order** (the `ops` order). Two sessions editing one file
   interleave by arrival; each line belongs to the op that last wrote it.
   `[a] → e1 (session A) writes a1, e2 (session B) writes a2 → a2 is e2.`
5. **Something uncaptured changed the file** (an explicit `before: { lines }`
   that differs from the replayed state: a formatter, a `sed`, a person, a lost
   event: WHY cannot tell which). Lines that exist only because of that change
   are `{ unknown: "noCapture" }`. Lines it did not change keep their origin
   **only if they match unambiguously**; the exact test is below.
   `[a, b] (e1 wrote b) → uncaptured adds c → [a, b, c]: a base, b e1, c noCapture.`
6. **An unobserved op** (`observed: { unobserved: ... }`, true effect in
   `truthAfter`): every line that differs between the state before it and the
   state after it gets the op's reason, `payloadReduced` (the hook reduced the
   payload) or `unparsed` (the payload could not be parsed). WHY knows an edit
   happened, not what it wrote. This beats rule 5 when the next op's `before`
   reveals the change. The engine never sees what the unobserved op wrote, and
   an `after: "auto"` fingerprint can verify a state but cannot supply line
   hashes. After an unobserved op a replay can only continue faithfully from a
   later op's `before` snapshot (the full hash sequence): a `replay` scenario
   needs a later Claude op with `before: "auto"`. Otherwise (a trailing
   unobserved op, any Codex op, a later op without `before`) the scenario is
   `align`.
   `[a, b] → unobserved op turns b into c → c is unknown(payloadReduced), a stays base.`
7. **Ambiguous Codex placement**: when a hunk's context (and `header`, if any:
   a line ABOVE the hunk) matches more than one place, every final line whose
   origin depends on which placement is true is `{ unknown: "ambiguousMatch" }`;
   lines that are the same under every placement keep their origin. The
   truth-only `appliedAt` says where it really went (Codex searches forward and
   takes the first match; this is for realism only, the engine never picks a
   placement). v0.3 scenarios keep ambiguous hunks to equal-size replacements;
   a count-changing ambiguous hunk has no defined answer yet.
   `[k, v, k, v] → hunk "k, -v, +w" fits twice → both v lines are ambiguousMatch, both k stay base.`
8. **The `align` stage** (the final file differs from a faithful replay: a
   change after the last captured op): apply rule 5 between the replayed state
   and `final`.
   `replay [a, b] (e1 wrote b), final [a, B] → a base, B noCapture.`
9. **Reserved reasons**: never expect `notReplayed` or `concurrentWriter`.
   Descriptions never say "manual" or "human": an uncaptured change may be a
   person, a shell command or a lost event.

A line is its text without terminator, so CRLF and LF files have the same lines
(`a\r\n` is the line `a`) and a gained or lost final newline is not a changed
line. A lone `\r` inside a line is content. Newline handling at the `align`
stage is checked by WHY-23, which gets the real file: the `align` check here
compares origins only (it skips hashes and the final-newline flag).

### The "unambiguous" test of rule 5

Compare the state BEFORE the uncaptured change with the state AFTER it, as
lines of text:

1. **Anchors** are lines whose text occurs exactly once in BOTH states.
2. Take the longest chain of anchors that keeps the same relative order in both
   states. If more than one chain has that maximum length, only anchors that
   belong to EVERY maximal chain count; the rest are not matched.
3. Between two consecutive matched anchors (and before the first, after the
   last): if the two gaps are identical sequences, every line in the gap keeps
   its origin. Otherwise repeat steps 1-3 inside that gap, with uniqueness
   counted inside the gap only. **Termination:** if a non-identical gap yields
   no anchor that survives step 2 (no line unique in both sides of the gap, or
   none common to every longest chain), STOP: the recursion makes no progress,
   and every line of the new state in that gap is unmatched. The same stop
   applies to the whole file at the top level.
4. Any line of the new state that is never matched is `noCapture` (under rule 6
   it takes the unobserved op's reason instead).

Three examples (letters are distinct lines); each ends with the stop of step 3:

- `A B C D E → C D E A B`: the longest chain is `C D E` (unique), so C, D, E
  keep their origins. The gap before `C` is `A B` versus nothing and the gap
  after `E` is nothing versus `A B`: non-identical, no anchor in them, so step 3
  stops; A and B are `noCapture`.
- `A B C D → C D A B`: step 2 finds two maximal chains (`A B` and `C D`) and no
  anchor is in both, so no anchor survives. The whole file is one non-identical
  gap with no surviving anchor: step 3 stops at once and all four lines are
  `noCapture`.
- `A x B → A x x B`: `x` is not unique in the new state, so the anchors are A
  and B. The gap between them, `x` versus `x x`, is non-identical; inside it
  `x` is not unique on both sides, so no anchor survives, step 3 stops and both
  `x` lines are `noCapture`.
