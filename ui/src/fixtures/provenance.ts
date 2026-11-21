// Fixture answers for `get_line_origins` (WHY-64, line provenance). Everything is
// invented. Every `edit` run's TurnRef points at a real edit event of
// `./sessions.ts` (same session, turn id, turn index, tool use id, agent, time
// and path), so the gutter -> inspector flow can open the turn in the timeline.
//
// - `src/calc.py` (agent worktree): base runs, a Claude turn, a Codex turn, and
//   one `unknown` run per reason.
// - `src/helper.py` (main worktree): two Claude turns of one session (the first
//   by a subagent). The fixture sessions have no file edited by two Claude
//   turns AND a Codex turn in the same worktree, so the two files together
//   cover that case.
// - `tests/test_calc.py`: `unavailable { engineNotReady }`, what the real core
//   answers today for every file.
// The 2,000-line file for gutter performance is in `@/dev-fixtures/long-file-origins`.
import type {
  FileChange,
  FileDiff,
  FileLineOrigins,
  HarnessLines,
  LineOriginKind,
  LineOriginsResult,
  LineRun,
  LineUnknownReason,
  TurnRef,
  WorktreeId,
} from "@/contracts";
import { WT_WHY_AGENT, WT_WHY_MAIN } from "./repos";
import { SESSION_CLAUDE, SESSION_CODEX, SESSION_SUBAGENT } from "./sessions";

/** Every reason the core can give for an unknown line, for tests and scenario switches. */
export const unknownLineReasons = [
  "noCapture",
  "payloadReduced",
  "unparsed",
  "replayMismatch",
  "ambiguousMatch",
  "concurrentWriter",
  "notReplayed",
] as const satisfies readonly LineUnknownReason[];

function at(minutes: number, seconds = 0): string {
  return new Date(Date.UTC(2026, 10, 20, 9, minutes, seconds)).toISOString();
}

/** A run with the evidence label the core gives its origin. */
function run(startLine: number, endLine: number, origin: LineOriginKind): LineRun {
  return {
    startLine,
    endLine,
    origin,
    evidence: origin.kind === "unknown" ? "unknown" : "observed",
  };
}

const edit = (turn: TurnRef): LineOriginKind => ({ kind: "edit", turn });
const unknown = (reason: LineUnknownReason): LineOriginKind => ({ kind: "unknown", reason });
const base: LineOriginKind = { kind: "base" };

/** Coverage computed from the runs, the way the core sums it. */
export function coverageOf(runs: LineRun[]): FileLineOrigins["coverage"] {
  let lines = 0;
  let baseLines = 0;
  let unknownLines = 0;
  const explained = new Map<HarnessLines["harness"], number>();
  for (const r of runs) {
    const n = r.endLine - r.startLine + 1;
    lines += n;
    if (r.origin.kind === "base") baseLines += n;
    else if (r.origin.kind === "unknown") unknownLines += n;
    else {
      const h = r.origin.turn.harness;
      explained.set(h, (explained.get(h) ?? 0) + n);
    }
  }
  return {
    lines,
    base: baseLines,
    unknown: unknownLines,
    explained: [...explained].map(([harness, n]) => ({ harness, lines: n })),
  };
}

export function originsOf(
  worktreeId: WorktreeId,
  path: string,
  runs: LineRun[],
): LineOriginsResult {
  const coverage = coverageOf(runs);
  return {
    status: "ok",
    origins: { worktreeId, path, lineCount: coverage.lines, runs, coverage },
  };
}

// ---- Turn references (each matches an edit event in ./sessions.ts) --------------

const CALC = "src/calc.py";
const HELPER = "src/helper.py";

const claudeCalcEdit: TurnRef = {
  harness: "claudeCode",
  sessionId: SESSION_CLAUDE,
  turnId: "c3a1f0e2-0001",
  turnIndex: 0,
  agentId: null,
  toolUseId: "toolu_fixture_c1-edit",
  outcome: "ok",
  at: at(0, 31),
  path: CALC,
};

const codexCalcRetry: TurnRef = {
  harness: "codex",
  sessionId: SESSION_CODEX,
  turnId: "01a1f812-9f04-7c11-8d2e-5b0a7e3c1d22",
  turnIndex: 1,
  agentId: null,
  toolUseId: "toolu_fixture_x2-retry",
  outcome: "ok",
  at: at(12, 29),
  path: CALC,
};

const subagentHelperWrite: TurnRef = {
  harness: "claudeCode",
  sessionId: SESSION_SUBAGENT,
  turnId: "d41d8cd9-0001",
  turnIndex: 0,
  agentId: "a7f3c2e1b9d84f06",
  toolUseId: "toolu_fixture_s1-agent-write",
  outcome: "ok",
  at: at(20, 21),
  path: HELPER,
};

const claudeHelperEdit: TurnRef = {
  harness: "claudeCode",
  sessionId: SESSION_SUBAGENT,
  turnId: "d41d8cd9-0003",
  turnIndex: 2,
  agentId: null,
  toolUseId: "toolu_fixture_s3-edit",
  outcome: "ok",
  at: at(23, 16),
  path: HELPER,
};

// ---- Answers --------------------------------------------------------------------

const calcRuns: LineRun[] = [
  run(1, 4, base),
  run(5, 7, edit(claudeCalcEdit)),
  run(8, 10, base),
  run(11, 12, edit(codexCalcRetry)),
  run(13, 20, base),
  // One run per reason, in contract order, so the UI can show every label.
  ...unknownLineReasons.map((reason, i) => run(21 + i, 21 + i, unknown(reason))),
];

const helperRuns: LineRun[] = [
  run(1, 6, edit(subagentHelperWrite)),
  run(7, 9, edit(claudeHelperEdit)),
  run(10, 12, edit(subagentHelperWrite)),
];

/** What the real core answers for a file it has nothing for (today: every file). */
export const engineNotReady: LineOriginsResult = {
  status: "unavailable",
  reason: "engineNotReady",
};

/** By worktree, then by path. A path not listed answers `engineNotReady`. */
export const lineOriginsByFile: Record<WorktreeId, Record<string, LineOriginsResult>> = {
  [WT_WHY_AGENT]: {
    [CALC]: originsOf(WT_WHY_AGENT, CALC, calcRuns),
    "tests/test_calc.py": engineNotReady,
  },
  [WT_WHY_MAIN]: {
    [HELPER]: originsOf(WT_WHY_MAIN, HELPER, helperRuns),
  },
};

// ---- Opt-in diff-flow entries ---------------------------------------------------
//
// With `FixtureOptions.provenance`, the sample files above are also listed as
// changes with a matching diff, so the gutter can be exercised through the
// normal "pick a changed file" flow. Opt-in, so the default change lists the
// rest of the UI tests rely on stay as they are. The text is generated: one line
// per line of the origins, labelled with that line's origin.

function labelOf(origin: LineOriginKind): string {
  if (origin.kind === "base") return "base";
  if (origin.kind === "unknown") return `unknown (${origin.reason})`;
  return `${origin.turn.harness} turn ${origin.turn.turnIndex}`;
}

/** A changed file and its diff whose new side has exactly `lineCount` lines. */
export function changeForOrigins(result: LineOriginsResult): {
  change: FileChange;
  diff: FileDiff;
} {
  if (result.status !== "ok") throw new Error("fixtures: only ok results have content");
  const { path, runs } = result.origins;
  const newLines: string[] = [];
  const oldLines: string[] = [];
  for (const r of runs) {
    for (let n = r.startLine; n <= r.endLine; n++) {
      const line = `# line ${n}: ${labelOf(r.origin)}`;
      newLines.push(line);
      if (r.origin.kind === "base") oldLines.push(line);
    }
  }
  const added = oldLines.length === 0;
  const status = added ? "added" : "modified";
  return {
    change: {
      path,
      oldPath: null,
      status,
      staged: false,
      unstaged: true,
      additions: newLines.length - oldLines.length,
      deletions: 0,
      binary: false,
      pathLossy: false,
    },
    diff: {
      path,
      oldPath: null,
      status,
      oldContent: added ? null : `${oldLines.join("\n")}\n`,
      newContent: `${newLines.join("\n")}\n`,
      binary: false,
      tooLarge: false,
      pathLossy: false,
    },
  };
}
