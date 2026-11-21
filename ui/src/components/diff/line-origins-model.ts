/**
 * Pure logic for line provenance in the UI (WHY-25): which runs get a gutter
 * mark, which colour a turn gets, what each "unknown" reason says in plain
 * words, and WHEN the marks may be drawn at all. No React, no Monaco: all of it
 * is unit-tested on its own, and the Monaco and React layers only apply it.
 */
import type {
  DiffMode,
  FileLineOrigins,
  Harness,
  LineCoverage,
  LineOriginsResult,
  LineOriginsUnavailable,
  LineRun,
  LineUnknownReason,
  SessionTimeline,
  TimelineEvent,
  Turn,
  TurnRef,
} from "@/contracts";
import type { AsyncState } from "@/data/async-state";

// ---- Runs -> gutter marks -------------------------------------------------------

export type MarkKind = "edit" | "unknown";

/** One gutter mark: a whole run, never one mark per line. */
export type RunMark = {
  /** Index into `origins.runs`, so a hover or click can find its run again. */
  runIndex: number;
  /** 1-based, inclusive, already clamped to the lines the editor really has. */
  startLine: number;
  endLine: number;
  kind: MarkKind;
  /** CSS classes for Monaco's `linesDecorationsClassName` (see `styles/line-origins.css`). */
  className: string;
};

/** How many turn colours exist (`og-turn-0` .. `og-turn-7` in the CSS). */
export const TURN_COLOR_COUNT = 8;

/** 32-bit FNV-1a: tiny, deterministic and well spread; no crypto is needed for a colour. */
function fnv1a(text: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

/**
 * A turn's colour slot. Derived only from WHAT the turn is (harness, session,
 * turn), never from the file or the order runs arrive in, so the same turn has
 * the same colour in every file and after every reload. A turn the harness gave
 * no id falls back to its index.
 */
export function turnColorIndex(
  turn: Pick<TurnRef, "harness" | "sessionId" | "turnId" | "turnIndex">,
): number {
  const id = turn.turnId !== "" ? turn.turnId : `#${turn.turnIndex}`;
  return fnv1a(`${turn.harness}\u0000${turn.sessionId}\u0000${id}`) % TURN_COLOR_COUNT;
}

/**
 * The marks for a file's runs. Base runs get none (unchanged lines are not
 * news). Runs are clamped to `1..=maxLine` (the editor's real line count) and
 * anything unusable is skipped, so a short or empty model never throws.
 */
export function buildMarks(runs: readonly LineRun[], maxLine: number): RunMark[] {
  const marks: RunMark[] = [];
  if (!Number.isFinite(maxLine) || maxLine < 1) return marks;
  runs.forEach((run, runIndex) => {
    if (run.origin.kind === "base") return;
    const startLine = Math.max(1, Math.trunc(run.startLine));
    const endLine = Math.min(Math.trunc(run.endLine), Math.trunc(maxLine));
    if (!Number.isFinite(startLine) || !Number.isFinite(endLine) || startLine > endLine) return;
    const kind: MarkKind = run.origin.kind === "unknown" ? "unknown" : "edit";
    marks.push({
      runIndex,
      startLine,
      endLine,
      kind,
      className:
        run.origin.kind === "edit"
          ? `og-mark og-turn-${turnColorIndex(run.origin.turn)}`
          : "og-mark og-unknown",
    });
  });
  return marks;
}

/** The index of the run holding `line` (1-based), or -1. Runs are sorted and disjoint. */
export function runAt(runs: readonly LineRun[], line: number): number {
  let low = 0;
  let high = runs.length - 1;
  while (low <= high) {
    const mid = (low + high) >> 1;
    const run = runs[mid];
    if (!run) return -1;
    if (line < run.startLine) high = mid - 1;
    else if (line > run.endLine) low = mid + 1;
    else return mid;
  }
  return -1;
}

/** The run behind a hovered mark: the run holding `line`, unless it is a base run (no mark, nothing to explain). */
export function markedRunAt(runs: readonly LineRun[], line: number): LineRun | undefined {
  const run = runs[runAt(runs, line)];
  return run && run.origin.kind !== "base" ? run : undefined;
}

/**
 * The first line of the next (or previous) `unknown` run relative to `line`, or
 * `null` at the end. Stops at the ends, like the hunk navigator's "boundary".
 */
export function adjacentUnknownLine(
  runs: readonly LineRun[],
  line: number,
  direction: "next" | "previous",
): number | null {
  const unknown = runs.filter((run) => run.origin.kind === "unknown");
  const found =
    direction === "next"
      ? unknown.find((run) => run.startLine > line)
      : unknown.findLast((run) => run.startLine < line);
  return found?.startLine ?? null;
}

// ---- Is the text the one the origins describe? ------------------------------------

/** What the origins are compared against: the shown file's line count, in WHY's own counting. */
export type TextShape = { lineCount: number; loneCarriageReturn: boolean };

/**
 * WHY counts lines like the core does: the final newline does not start a new
 * line (`"a\nb\n"` is 2 lines, `""` is 0), `\r\n` ends one line, and a lone `\r`
 * is content. Monaco splits on a lone `\r` too, so a file with one cannot be
 * lined up with Monaco's rows: `loneCarriageReturn` lets the gate refuse it.
 */
export function textShape(text: string): TextShape {
  if (text === "") return { lineCount: 0, loneCarriageReturn: false };
  let newlines = 0;
  for (let at = text.indexOf("\n"); at !== -1; at = text.indexOf("\n", at + 1)) newlines++;
  return {
    lineCount: text.endsWith("\n") ? newlines : newlines + 1,
    loneCarriageReturn: /\r(?!\n)/.test(text),
  };
}

// ---- When may the marks be drawn? --------------------------------------------------

/** Why there are no marks (and so what the File tab should say). */
export type GutterHidden =
  | { kind: "noFile" }
  | { kind: "loading" }
  | { kind: "error" }
  | { kind: "unavailable"; reason: LineOriginsUnavailable }
  /** The right-hand side is the index, not the working tree the origins describe. */
  | { kind: "notWorkingTree" }
  | { kind: "otherFile" }
  /** The origins and the shown text do not cover the same revision of the file (see `gutterGate`). */
  | { kind: "stale" }
  | { kind: "textMismatch"; originsLines: number; textLines: number };

export type GutterGate =
  { status: "shown"; origins: FileLineOrigins } | { status: "hidden"; hidden: GutterHidden };

/** Comparisons whose right-hand side is the working tree (`staged` shows the index). */
export function showsWorkingTree(mode: DiffMode): boolean {
  return mode === "vsBase" || mode === "uncommitted";
}

/**
 * The single rule for painting: marks appear only when the origins are for THIS
 * file of THIS worktree, and describe exactly the text on the modified side.
 * Anything else paints nothing: a mark on text it does not describe would be a
 * false attribution, worse than no mark.
 *
 * "Describe the same text" is decided by revision tokens, not by guessing from
 * line counts: every `worktree-changed` event on the file bumps its `revision`;
 * the origins carry the revision their read STARTED at, and so does the shown
 * diff. Marks need all three equal. An edit that keeps the line count, with the
 * origins read finished before the diff read, therefore shows nothing until the
 * diff has caught up. The line-count check stays as a second, independent net.
 */
export function gutterGate(input: {
  origins: AsyncState<LineOriginsResult>;
  /** The file's current revision (how many changes to it have been reported). */
  revision: number;
  /** The revision the origins read started at. */
  originsRevision: number;
  /** The revision the shown diff's read started at; `undefined` when unknown (never trusted). */
  diffRevision: number | undefined;
  mode: DiffMode;
  worktreeId: string | null;
  path: string | null;
  /** The modified side's shape; `null` when the file has no new text (deleted, binary). */
  text: TextShape | null;
}): GutterGate {
  const { origins, revision, originsRevision, diffRevision, mode, worktreeId, path, text } = input;
  const hide = (hidden: GutterHidden): GutterGate => ({ status: "hidden", hidden });
  if (worktreeId === null || path === null) return hide({ kind: "noFile" });
  if (origins.status === "idle" || origins.status === "loading") return hide({ kind: "loading" });
  if (origins.status === "error") return hide({ kind: "error" });
  if (origins.data.status === "unavailable") {
    return hide({ kind: "unavailable", reason: origins.data.reason });
  }
  const answer = origins.data.origins;
  if (answer.worktreeId !== worktreeId || answer.path !== path) return hide({ kind: "otherFile" });
  if (!showsWorkingTree(mode)) return hide({ kind: "notWorkingTree" });
  if (originsRevision !== revision || diffRevision !== revision) return hide({ kind: "stale" });
  if (text === null) return hide({ kind: "noFile" });
  if (text.loneCarriageReturn || text.lineCount !== answer.lineCount) {
    return hide({
      kind: "textMismatch",
      originsLines: answer.lineCount,
      textLines: text.lineCount,
    });
  }
  return { status: "shown", origins: answer };
}

// ---- Words -----------------------------------------------------------------------------

/**
 * What an `unknown` run says. `short` is the one-liner (hover, headline);
 * `detail` is the honest extra sentence for the inspector. Written about
 * RECORDS, never about people: a line with no captured edit can be a person, a
 * shell command or a lost event, and WHY must not guess which. The
 * `Record<LineUnknownReason, ...>` type makes a new reason a compile error
 * until it gets words here.
 */
export const unknownReasonCopy: Record<LineUnknownReason, { short: string; detail: string }> = {
  noCapture: {
    short: "No captured agent edit wrote this line",
    detail:
      "It may have been written by a person, by a shell command, or by an edit WHY did not see. WHY does not guess which.",
  },
  payloadReduced: {
    short: "An agent edit happened here but its payload was too large to capture",
    detail: "WHY knows an edit touched this file, but not which lines it wrote.",
  },
  unparsed: {
    short: "An agent edit happened here but WHY could not read it",
    detail: "The edit was captured in a shape WHY does not understand, so it cannot place it.",
  },
  replayMismatch: {
    short: "The captured edits do not reproduce this part of the file",
    detail:
      "Replaying the recorded edits gave different text than the file has, so WHY will not name an author for these lines.",
  },
  ambiguousMatch: {
    short: "The edit matches more than one place, so WHY cannot tell which lines it changed",
    detail: "Codex patches carry context but no line numbers; here the context fits twice.",
  },
  concurrentWriter: {
    short: "More than one writer changed this file at the same time",
    detail: "With edits interleaved, WHY cannot say whose edit produced this line.",
  },
  notReplayed: {
    short: "WHY has not worked out where this line came from",
    detail: "The captured edits for this file have not been replayed for this line.",
  },
};

export const unavailableCopy: Record<LineOriginsUnavailable, { short: string; detail: string }> = {
  engineNotReady: {
    short: "Line origins are not available yet",
    detail:
      "WHY cannot yet trace individual lines to the agent turn that wrote them. Nothing is wrong with this file; the captured edits and sessions above still work.",
  },
  notText: {
    short: "Line origins are only available for text files",
    detail: "This file is binary or not valid text, so WHY has no lines to trace.",
  },
  tooLarge: {
    short: "This file is too large for line origins",
    detail: "WHY skips line origins for very large files to stay light on the repository.",
  },
};

/** A file name that is not valid UTF-8 cannot be looked up: it shares its display text with other files. */
export const lossyPathCopy = {
  short: "Line origins are not available for this file name",
  detail:
    "The name is not valid UTF-8, so WHY cannot ask about this exact file without risking another file's answer.",
};

/** Why no marks are drawn, for the File tab. `null`: nothing to say (loading, no file, error shown elsewhere). */
export function hiddenCopy(hidden: GutterHidden): string | null {
  switch (hidden.kind) {
    case "notWorkingTree":
      return "Line marks describe the working-tree file. This comparison shows the staged version, so none are drawn. Switch to Vs base or Uncommitted to see them.";
    case "stale":
      return "The file changed, so line marks are hidden until the diff and the origins have both been read again.";
    case "textMismatch":
      return "The text shown and the origins disagree about the file's lines, so no marks are drawn rather than risk marking the wrong ones.";
    case "otherFile":
    case "noFile":
    case "loading":
    case "error":
    case "unavailable":
      return null;
  }
}

/** What the inspector says for a line that no edit changed. */
export const baseCopy = "Unchanged since the base revision";

// ---- Coverage -------------------------------------------------------------------------

export type CoverageSegment =
  | { kind: "harness"; harness: Harness; lines: number }
  | { kind: "base"; lines: number }
  | { kind: "unknown"; lines: number };

const harnessOrder: Harness[] = ["claudeCode", "codex", "unknown"];

/**
 * The coverage as ordered segments: each harness that wrote lines (Claude and
 * Codex SEPARATE, decision 30), then unchanged, then unknown. `addsUp` is the
 * contract's invariant (`lines == base + unknown + sum(explained)`); a core that
 * breaks it must not be shown as if it were whole.
 */
export function coverageSegments(coverage: LineCoverage): {
  segments: CoverageSegment[];
  total: number;
  addsUp: boolean;
} {
  const explained = [...coverage.explained].sort(
    (a, b) => harnessOrder.indexOf(a.harness) - harnessOrder.indexOf(b.harness),
  );
  const segments: CoverageSegment[] = [
    ...explained.map(({ harness, lines }): CoverageSegment => ({
      kind: "harness",
      harness,
      lines,
    })),
    { kind: "base", lines: coverage.base },
    { kind: "unknown", lines: coverage.unknown },
  ];
  const total = segments.reduce((sum, segment) => sum + segment.lines, 0);
  return { segments, total, addsUp: total === coverage.lines };
}

// ---- Finding a turn in a timeline --------------------------------------------------------

/**
 * The turn a `TurnRef` points at, and the tool call inside it. The call is
 * matched by `toolUseId` AND `agentId` (a subagent's call can reuse an id
 * space), and only when the harness gave an id: otherwise it is `null` and the
 * caller falls back to the turn.
 */
export function locateInTimeline(
  timeline: SessionTimeline,
  ref: Pick<TurnRef, "turnId" | "toolUseId" | "agentId">,
): { turn: Turn | null; event: TimelineEvent | null } {
  const turn = timeline.turns.find((candidate) => candidate.id === ref.turnId) ?? null;
  const event =
    turn && ref.toolUseId !== null
      ? (turn.events.find(
          (candidate) =>
            candidate.toolUseId === ref.toolUseId && candidate.agentId === (ref.agentId ?? null),
        ) ?? null)
      : null;
  return { turn, event };
}
