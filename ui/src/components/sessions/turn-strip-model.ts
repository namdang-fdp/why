import type { ShellOutcome, Turn } from "@/contracts";
import { effectiveOutcome } from "./outcome-model";

export type StripTone = "failed" | "edit" | "test" | "neutral";

export type StripSegment = {
  /** Inclusive turn indexes this segment covers. */
  from: number;
  to: number;
  edits: number;
  tests: number;
  failed: number;
  tone: StripTone;
};

export const maxSegments = 80;

type TurnFacts = Pick<StripSegment, "edits" | "tests" | "failed">;

/** What one turn contains. `edits` counts applied edits only; `unknown` is never counted as success. */
export function turnFacts(
  turn: Turn,
  recovered: ReadonlyMap<string, ShellOutcome> | undefined,
): TurnFacts {
  const facts = { edits: 0, tests: 0, failed: 0 };
  for (const event of turn.events) {
    const outcome = effectiveOutcome(event, recovered);
    if (event.kind === "edit" && outcome === "ok") facts.edits++;
    if (event.kind === "testRun") facts.tests++;
    if (outcome === "failed") facts.failed++;
  }
  return facts;
}

/** The most informative thing in the range wins: failed, then edit, then test run. */
function toneOf({ edits, tests, failed }: TurnFacts): StripTone {
  return failed > 0 ? "failed" : edits > 0 ? "edit" : tests > 0 ? "test" : "neutral";
}

/**
 * One segment per turn, or, past `limit` turns, buckets of consecutive turns so
 * the strip never mounts more than `limit` elements however long the session is.
 */
export function stripSegments(
  turns: readonly Turn[],
  recovered: ReadonlyMap<string, ShellOutcome> | undefined,
  limit = maxSegments,
): StripSegment[] {
  const size = Math.max(1, Math.ceil(turns.length / limit));
  const segments: StripSegment[] = [];
  for (let from = 0; from < turns.length; from += size) {
    const to = Math.min(turns.length, from + size) - 1;
    const total = { edits: 0, tests: 0, failed: 0 };
    for (let index = from; index <= to; index++) {
      const turn = turns[index];
      if (!turn) continue;
      const facts = turnFacts(turn, recovered);
      total.edits += facts.edits;
      total.tests += facts.tests;
      total.failed += facts.failed;
    }
    segments.push({ from, to, ...total, tone: toneOf(total) });
  }
  return segments;
}

const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

/** "Turns 5 to 8: 2 edits, 1 test run, 0 failed" (or "Turn 5: ..." for one turn). */
export function segmentLabel(segment: StripSegment) {
  const range =
    segment.from === segment.to
      ? `Turn ${segment.from + 1}`
      : `Turns ${segment.from + 1} to ${segment.to + 1}`;
  return `${range}: ${plural(segment.edits, "edit", "edits")}, ${plural(segment.tests, "test run", "test runs")}, ${segment.failed} failed`;
}
