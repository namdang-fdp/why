import { describe, expect, it } from "vitest";
import type { ShellOutcome, TimelineEvent, ToolOutcome, Turn } from "@/contracts";
import { maxSegments, segmentLabel, stripSegments } from "./turn-strip-model";

function call(id: string, kind: TimelineEvent["kind"], outcome: ToolOutcome | null): TimelineEvent {
  return {
    id,
    kind,
    at: "2026-11-20T09:00:00.000Z",
    toolName: null,
    summary: id,
    files: [],
    evidence: "observed",
    outcome,
    agentId: null,
    toolUseId: null,
  };
}

function turn(index: number, events: TimelineEvent[] = []): Turn {
  return {
    id: `t${index}`,
    index,
    prompt: `Prompt ${index}`,
    startedAt: "2026-11-20T09:00:00.000Z",
    endedAt: null,
    events,
  };
}

const plain = (count: number) => Array.from({ length: count }, (_, index) => turn(index));

describe("turn strip segments", () => {
  it.each([0, 1, 3, 79, 80])("gives one segment per turn for %i turns", (count) => {
    const segments = stripSegments(plain(count), undefined);
    expect(segments).toHaveLength(count);
    segments.forEach((segment, index) => expect(segment).toMatchObject({ from: index, to: index }));
  });

  it.each([81, 160, 161, 1000])(
    "buckets %i turns into at most 80 segments covering every turn once",
    (count) => {
      const segments = stripSegments(plain(count), undefined);
      expect(segments.length).toBeLessThanOrEqual(maxSegments);
      // Contiguous, non-overlapping, and together every turn: each turn is reachable.
      expect(segments[0]?.from).toBe(0);
      expect(segments.at(-1)?.to).toBe(count - 1);
      segments.slice(1).forEach((segment, index) => {
        expect(segment.from).toBe((segments[index]?.to ?? -2) + 1);
      });
    },
  );

  it("tones a segment by the most informative member: failed, then edit, then test run", () => {
    const turns = [
      turn(0, [call("a", "edit", "ok")]),
      turn(1, [call("b", "testRun", "ok")]),
      turn(2, [call("c", "shell", "failed")]),
      turn(3, [call("d", "shell", "ok")]),
      turn(4, [call("e", "edit", "ok"), call("f", "testRun", "ok"), call("g", "shell", "failed")]),
    ];
    expect(stripSegments(turns, undefined).map((segment) => segment.tone)).toEqual([
      "edit",
      "test",
      "failed",
      "neutral",
      "failed",
    ]);
  });

  it("never styles an unknown outcome as success", () => {
    const turns = [
      turn(0, [call("a", "edit", "unknown"), call("b", "shell", "unknown")]),
      turn(1, [call("c", "toolCall", null)]),
    ];
    const segments = stripSegments(turns, undefined);
    expect(segments.map((segment) => segment.tone)).toEqual(["neutral", "neutral"]);
    expect(segments[0]).toMatchObject({ edits: 0, failed: 0 });
  });

  it("lets a recovered exit code tone the strip, like the rows", () => {
    const recovered = new Map<string, ShellOutcome>([
      [
        "b",
        {
          toolUseId: "tb",
          eventId: "b",
          turnId: "t0",
          agentId: null,
          exitCode: 2,
          outcome: "failed",
          evidence: "observed",
          source: "codexRollout",
        },
      ],
    ]);
    const turns = [turn(0, [call("b", "shell", "unknown")])];
    expect(stripSegments(turns, recovered)[0]?.tone).toBe("failed");
  });

  it("sums a bucket and keeps the most informative tone", () => {
    // 160 turns => buckets of 2. The failure in the second bucket wins its tone.
    const turns = plain(160).map((item) => item);
    turns[2] = turn(2, [call("e1", "edit", "ok")]);
    turns[3] = turn(3, [call("e2", "edit", "ok"), call("f1", "shell", "failed")]);
    const [first, second] = stripSegments(turns, undefined);
    expect(first).toMatchObject({ from: 0, to: 1, tone: "neutral" });
    expect(second).toMatchObject({ from: 2, to: 3, edits: 2, failed: 1, tone: "failed" });
  });

  it("writes counts into the aria label", () => {
    const turns = [
      turn(0),
      turn(1),
      turn(2),
      turn(3),
      turn(4, [call("a", "edit", "ok"), call("b", "edit", "ok")]),
      turn(5, [call("c", "testRun", "ok")]),
      turn(6),
      turn(7),
    ];
    const [segment] = stripSegments(turns, undefined, 2);
    expect(segment && segmentLabel(segment)).toBe("Turns 1 to 4: 0 edits, 0 test runs, 0 failed");
    const second = stripSegments(turns, undefined, 2)[1];
    expect(second && segmentLabel(second)).toBe("Turns 5 to 8: 2 edits, 1 test run, 0 failed");
    const single = stripSegments([turn(0, [call("a", "edit", "ok")])], undefined)[0];
    expect(single && segmentLabel(single)).toBe("Turn 1: 1 edit, 0 test runs, 0 failed");
  });
});
