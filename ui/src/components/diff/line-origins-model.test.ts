import { describe, expect, it } from "vitest";
import type {
  FileLineOrigins,
  LineOriginsResult,
  LineOriginsUnavailable,
  LineRun,
  LineUnknownReason,
  TurnRef,
} from "@/contracts";
import { buildLongFileOrigins } from "@/dev-fixtures/long-file-origins";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { WT_WHY_AGENT, WT_WHY_MAIN } from "@/fixtures";
import { coverageOf, lineOriginsByFile, unknownLineReasons } from "@/fixtures/provenance";
import {
  adjacentUnknownLine,
  buildMarks,
  coverageSegments,
  gutterGate,
  locateInTimeline,
  markedRunAt,
  runAt,
  textShape,
  turnColorIndex,
  unavailableCopy,
  unknownReasonCopy,
} from "./line-origins-model";

const turn = (over: Partial<TurnRef> = {}): TurnRef => ({
  harness: "claudeCode",
  sessionId: "s1",
  turnId: "t1",
  turnIndex: 0,
  agentId: null,
  toolUseId: "tool1",
  outcome: "ok",
  at: "2026-11-20T09:00:00Z",
  path: "a.py",
  ...over,
});

const base = (startLine: number, endLine: number): LineRun => ({
  startLine,
  endLine,
  origin: { kind: "base" },
  evidence: "observed",
});
const edit = (startLine: number, endLine: number, ref = turn()): LineRun => ({
  startLine,
  endLine,
  origin: { kind: "edit", turn: ref },
  evidence: "observed",
});
const unknown = (startLine: number, endLine: number, reason: LineUnknownReason): LineRun => ({
  startLine,
  endLine,
  origin: { kind: "unknown", reason },
  evidence: "unknown",
});

describe("buildMarks: runs become one mark each", () => {
  it("makes no mark for base runs and one per edit/unknown run, in order", () => {
    const runs = [base(1, 4), edit(5, 7), base(8, 10), unknown(11, 12, "noCapture"), edit(13, 13)];
    const marks = buildMarks(runs, 13);
    expect(
      marks.map(({ runIndex, startLine, endLine, kind }) => [runIndex, startLine, endLine, kind]),
    ).toEqual([
      [1, 5, 7, "edit"],
      [3, 11, 12, "unknown"],
      [4, 13, 13, "edit"],
    ]);
  });

  it("keeps a single 1-line run and a run on the last line", () => {
    const marks = buildMarks([base(1, 9), edit(10, 10)], 10);
    expect(marks).toHaveLength(1);
    expect(marks[0]).toMatchObject({ startLine: 10, endLine: 10 });
  });

  it("keeps unknown runs adjacent to edit runs apart and visibly different", () => {
    const marks = buildMarks([edit(1, 3), unknown(4, 5, "ambiguousMatch"), edit(6, 6)], 6);
    expect(marks.map((mark) => mark.kind)).toEqual(["edit", "unknown", "edit"]);
    expect(marks[1]?.className).toContain("og-unknown");
    expect(marks[0]?.className).not.toContain("og-unknown");
  });

  it("covers a partition of 1..lineCount without gaps or overlaps (the 2,000-line fixture)", () => {
    const result = buildLongFileOrigins(WT_WHY_AGENT);
    if (result.status !== "ok") throw new Error("fixture must be ok");
    const { runs, lineCount } = result.origins;
    const marked = new Set<number>();
    for (const mark of buildMarks(runs, lineCount)) {
      for (let line = mark.startLine; line <= mark.endLine; line++) {
        expect(marked.has(line), `line ${line} marked twice`).toBe(false);
        marked.add(line);
      }
    }
    // Every non-base line is marked, no base line is.
    for (const run of runs) {
      for (let line = run.startLine; line <= run.endLine; line++) {
        expect(marked.has(line)).toBe(run.origin.kind !== "base");
      }
    }
  });

  it("gives a file with lineCount 0 no marks and does not throw", () => {
    expect(buildMarks([], 0)).toEqual([]);
    expect(buildMarks([edit(1, 3)], 0)).toEqual([]);
  });

  it("clamps when the editor has fewer lines than the runs, and drops runs past the end", () => {
    const marks = buildMarks([base(1, 2), edit(3, 8), unknown(9, 10, "noCapture")], 5);
    expect(marks).toHaveLength(1);
    expect(marks[0]).toMatchObject({ startLine: 3, endLine: 5 });
  });

  it("skips malformed runs instead of throwing", () => {
    const bad = [edit(5, 2), edit(Number.NaN, 4), edit(0, 1)];
    expect(() => buildMarks(bad, 10)).not.toThrow();
    // `0..1` is clamped up to line 1; the reversed and NaN runs are dropped.
    expect(buildMarks(bad, 10).map(({ startLine, endLine }) => [startLine, endLine])).toEqual([
      [1, 1],
    ]);
    expect(buildMarks([edit(1, 2)], Number.NaN)).toEqual([]);
  });
});

describe("runAt and adjacentUnknownLine", () => {
  const runs = [
    base(1, 4),
    edit(5, 7),
    base(8, 10),
    unknown(11, 11, "noCapture"),
    unknown(12, 13, "notReplayed"),
  ];

  it("finds the run holding a line, or -1 outside the file", () => {
    expect(runAt(runs, 1)).toBe(0);
    expect(runAt(runs, 7)).toBe(1);
    expect(runAt(runs, 8)).toBe(2);
    expect(runAt(runs, 13)).toBe(4);
    expect(runAt(runs, 0)).toBe(-1);
    expect(runAt(runs, 14)).toBe(-1);
    expect(runAt([], 1)).toBe(-1);
  });

  it("walks the unknown runs and stops at the ends", () => {
    expect(adjacentUnknownLine(runs, 1, "next")).toBe(11);
    expect(adjacentUnknownLine(runs, 11, "next")).toBe(12);
    expect(adjacentUnknownLine(runs, 12, "next")).toBeNull();
    expect(adjacentUnknownLine(runs, 13, "previous")).toBe(12);
    expect(adjacentUnknownLine(runs, 11, "previous")).toBeNull();
    expect(adjacentUnknownLine([edit(1, 3)], 1, "next")).toBeNull();
  });
});

describe("turn colour", () => {
  it("is stable: the same turn always gets the same slot", () => {
    const a = turn({ sessionId: "s", turnId: "x" });
    expect(turnColorIndex(a)).toBe(turnColorIndex({ ...a }));
    // The file, outcome and time do not matter.
    const elsewhere: TurnRef = { ...a, path: "other.py", outcome: "failed" };
    expect(turnColorIndex(a)).toBe(turnColorIndex(elsewhere));
  });

  it("is distinct for the fixture's turns and always inside the palette", () => {
    const refs = new Map<string, TurnRef>();
    for (const files of Object.values(lineOriginsByFile)) {
      for (const result of Object.values(files)) {
        if (result.status !== "ok") continue;
        for (const run of result.origins.runs) {
          if (run.origin.kind === "edit") {
            const ref = run.origin.turn;
            refs.set(`${ref.sessionId}/${ref.turnId}`, ref);
          }
        }
      }
    }
    expect(refs.size).toBeGreaterThanOrEqual(3);
    const slots = [...refs.values()].map(turnColorIndex);
    expect(new Set(slots).size).toBe(slots.length);
    for (const slot of slots) expect(slot).toBeGreaterThanOrEqual(0);
  });

  it("falls back to the turn index when the harness gave no turn id", () => {
    const first = turnColorIndex(turn({ turnId: "", turnIndex: 0 }));
    const second = turnColorIndex(turn({ turnId: "", turnIndex: 1 }));
    expect(Number.isInteger(first)).toBe(true);
    expect([first, second]).toHaveLength(2);
    expect(turnColorIndex(turn({ turnId: "", turnIndex: 0 }))).toBe(first);
  });
});

describe("plain-words copy", () => {
  // A literal `Record<LineUnknownReason, true>`: a new reason in the contract makes this
  // object a compile error until it is listed here, which sends the author to the copy.
  const allReasons: Record<LineUnknownReason, true> = {
    noCapture: true,
    payloadReduced: true,
    unparsed: true,
    replayMismatch: true,
    ambiguousMatch: true,
    concurrentWriter: true,
    notReplayed: true,
  };
  const reasons = Object.keys(allReasons) as LineUnknownReason[];

  it("has copy for every unknown reason, short and with detail", () => {
    for (const reason of reasons) {
      const copy = unknownReasonCopy[reason];
      expect(copy.short.trim().length, reason).toBeGreaterThan(10);
      expect(copy.detail.trim().length, reason).toBeGreaterThan(10);
    }
    // The fixture's list (what the core can send) is the same set.
    expect([...unknownLineReasons].sort()).toEqual([...reasons].sort());
  });

  it("uses the exact words the brief fixes, and never claims a person did it", () => {
    expect(unknownReasonCopy.noCapture.short).toBe("No captured agent edit wrote this line");
    expect(unknownReasonCopy.payloadReduced.short).toBe(
      "An agent edit happened here but its payload was too large to capture",
    );
    expect(unknownReasonCopy.unparsed.short).toBe(
      "An agent edit happened here but WHY could not read it",
    );
    for (const reason of reasons) {
      const { short } = unknownReasonCopy[reason];
      expect(short, reason).not.toMatch(/\bmanual(ly)?\b|\bby a human\b|\bhand-written\b/i);
    }
  });

  it("is distinct per reason", () => {
    expect(new Set(reasons.map((reason) => unknownReasonCopy[reason].short)).size).toBe(
      reasons.length,
    );
  });

  it("has copy for every unavailable reason", () => {
    const all: Record<LineOriginsUnavailable, true> = {
      engineNotReady: true,
      notText: true,
      tooLarge: true,
    };
    for (const reason of Object.keys(all) as LineOriginsUnavailable[]) {
      expect(unavailableCopy[reason].short.length).toBeGreaterThan(10);
    }
    expect(unavailableCopy.engineNotReady.short).toBe("Line origins are not available yet");
  });
});

describe("textShape", () => {
  it("counts lines the way the core does", () => {
    expect(textShape("")).toEqual({ lineCount: 0, loneCarriageReturn: false });
    expect(textShape("\n").lineCount).toBe(1);
    expect(textShape("a").lineCount).toBe(1);
    expect(textShape("a\n").lineCount).toBe(1);
    expect(textShape("a\nb").lineCount).toBe(2);
    expect(textShape("a\r\nb\r\n").lineCount).toBe(2);
  });

  it("flags a lone carriage return, which Monaco would split on but WHY does not", () => {
    expect(textShape("a\rb\n").loneCarriageReturn).toBe(true);
    expect(textShape("a\r\nb\n").loneCarriageReturn).toBe(false);
  });
});

describe("gutterGate: marks only on text they describe", () => {
  const calc = lineOriginsByFile[WT_WHY_AGENT]?.["src/calc.py"];
  if (calc?.status !== "ok") throw new Error("fixture: calc.py must have origins");
  const ok = { status: "ok", data: calc } as const;
  const shape = { lineCount: calc.origins.lineCount, loneCarriageReturn: false };
  const input = {
    origins: ok,
    revision: 0,
    originsRevision: 0,
    diffRevision: 0,
    mode: "uncommitted",
    worktreeId: WT_WHY_AGENT,
    path: "src/calc.py",
    text: shape,
  } as const;

  it("shows for the right file, working-tree mode and matching line count", () => {
    expect(gutterGate(input)).toEqual({ status: "shown", origins: calc.origins });
    expect(gutterGate({ ...input, mode: "vsBase" }).status).toBe("shown");
  });

  it("hides, with a reason, for everything else", () => {
    const hidden = (over: Partial<Parameters<typeof gutterGate>[0]>) => {
      const gate = gutterGate({ ...input, ...over });
      if (gate.status !== "hidden") throw new Error("expected hidden");
      return gate.hidden;
    };
    expect(hidden({ mode: "staged" })).toEqual({ kind: "notWorkingTree" });
    // The file changed (revision 1): the origins and the diff must BOTH have been read after it.
    expect(hidden({ revision: 1 })).toEqual({ kind: "stale" }); // both behind
    expect(hidden({ revision: 1, originsRevision: 1 })).toEqual({ kind: "stale" }); // diff behind
    expect(hidden({ revision: 1, diffRevision: 1 })).toEqual({ kind: "stale" }); // origins behind
    expect(hidden({ diffRevision: undefined })).toEqual({ kind: "stale" }); // an untagged diff is never trusted
    expect(hidden({ path: "src/other.py" })).toEqual({ kind: "otherFile" });
    expect(hidden({ worktreeId: WT_WHY_MAIN })).toEqual({ kind: "otherFile" });
    expect(hidden({ text: { ...shape, lineCount: shape.lineCount - 1 } })).toMatchObject({
      kind: "textMismatch",
    });
    expect(hidden({ text: { ...shape, loneCarriageReturn: true } })).toMatchObject({
      kind: "textMismatch",
    });
    expect(hidden({ text: null })).toEqual({ kind: "noFile" });
    expect(hidden({ path: null })).toEqual({ kind: "noFile" });
    expect(hidden({ origins: { status: "loading" } })).toEqual({ kind: "loading" });
    expect(hidden({ origins: { status: "idle" } })).toEqual({ kind: "loading" });
    expect(hidden({ origins: { status: "error", error: { kind: "timeout" } } })).toEqual({
      kind: "error",
    });
  });

  it("shows again once both reads are at the file's revision", () => {
    const caughtUp = { ...input, revision: 3, originsRevision: 3, diffRevision: 3 };
    expect(gutterGate(caughtUp).status).toBe("shown");
  });

  it("explains engineNotReady rather than painting anything", () => {
    const notReady: LineOriginsResult = { status: "unavailable", reason: "engineNotReady" };
    expect(gutterGate({ ...input, origins: { status: "ok", data: notReady } })).toEqual({
      status: "hidden",
      hidden: { kind: "unavailable", reason: "engineNotReady" },
    });
  });
});

describe("coverageSegments: the numbers add up, Claude and Codex apart", () => {
  it("adds up for every fixture file, including the 2,000-line one", () => {
    const results: LineOriginsResult[] = [
      ...Object.values(lineOriginsByFile).flatMap((files) => Object.values(files)),
      buildLongFileOrigins(WT_WHY_AGENT),
    ];
    let checked = 0;
    for (const result of results) {
      if (result.status !== "ok") continue;
      const { segments, total, addsUp } = coverageSegments(result.origins.coverage);
      expect(addsUp).toBe(true);
      expect(total).toBe(result.origins.lineCount);
      expect(segments.reduce((sum, segment) => sum + segment.lines, 0)).toBe(total);
      checked++;
    }
    expect(checked).toBeGreaterThanOrEqual(3);
  });

  it("lists each harness on its own, never as one merged agent total", () => {
    const calc = lineOriginsByFile[WT_WHY_AGENT]?.["src/calc.py"];
    if (calc?.status !== "ok") throw new Error("fixture");
    const harnesses = coverageSegments(calc.origins.coverage)
      .segments.filter((segment) => segment.kind === "harness")
      .map((segment) => (segment.kind === "harness" ? segment.harness : null));
    expect(harnesses).toEqual(["claudeCode", "codex"]);
  });

  it("notices a core whose numbers do not add up", () => {
    const origins: FileLineOrigins = {
      worktreeId: "w",
      path: "a",
      lineCount: 10,
      runs: [],
      coverage: { lines: 10, base: 3, unknown: 3, explained: [{ harness: "codex", lines: 3 }] },
    };
    expect(coverageSegments(origins.coverage).addsUp).toBe(false);
    expect(coverageOf([base(1, 4), edit(5, 6)]).lines).toBe(6);
  });
});

describe("locateInTimeline resolves fixture TurnRefs", () => {
  it("finds the turn, the prompt and the tool call for every edit run of the fixtures", async () => {
    const source = new FixtureDataSource();
    let located = 0;
    for (const files of Object.values(lineOriginsByFile)) {
      for (const result of Object.values(files)) {
        if (result.status !== "ok") continue;
        for (const run of result.origins.runs) {
          if (run.origin.kind !== "edit") continue;
          const ref = run.origin.turn;
          const timeline = await source.getSessionTimeline(ref.sessionId);
          const { turn: found, event } = locateInTimeline(timeline, ref);
          expect(found?.index, ref.toolUseId ?? "").toBe(ref.turnIndex);
          expect(event?.toolUseId).toBe(ref.toolUseId);
          located++;
        }
      }
    }
    expect(located).toBeGreaterThanOrEqual(4);
  });

  it("returns no event when the harness gave no tool id, and no turn for an unknown id", () => {
    const timeline = { session: {}, turns: [] } as never;
    expect(locateInTimeline(timeline, { turnId: "x", toolUseId: null, agentId: null })).toEqual({
      turn: null,
      event: null,
    });
  });
});

describe("markedRunAt: the hover card follows the current runs", () => {
  it("resolves the same line against whichever runs are current", () => {
    const before = [base(1, 4), edit(5, 9), base(10, 12)];
    // A refresh split the edit run in two and added an unknown run, under a resting pointer.
    const after = [base(1, 4), edit(5, 6), unknown(7, 7, "noCapture"), edit(8, 9), base(10, 12)];
    expect(markedRunAt(before, 7)?.origin.kind).toBe("edit");
    expect(markedRunAt(after, 7)?.origin.kind).toBe("unknown");
    expect(markedRunAt(after, 8)?.origin.kind).toBe("edit");
  });

  it("explains nothing on a base line or outside the file", () => {
    const runs = [base(1, 4), edit(5, 6)];
    expect(markedRunAt(runs, 2)).toBeUndefined();
    expect(markedRunAt(runs, 9)).toBeUndefined();
  });
});
