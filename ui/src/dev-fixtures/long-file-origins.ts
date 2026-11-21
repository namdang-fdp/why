// A 2,000-line file's line origins for judging the gutter's performance (WHY-25).
// Generated and deterministic: the same `lines` always gives the same runs. Runs
// are short (1-40 lines) and alternate between base, two edits and unknown, so
// a virtualized gutter has many boundaries to draw. The TurnRefs reuse real
// fixture turns so the inspector flow still resolves.
import type { LineOriginKind, LineOriginsResult, LineRun, TurnRef, WorktreeId } from "@/contracts";
import { originsOf, unknownLineReasons } from "@/fixtures/provenance";
import { SESSION_CLAUDE, SESSION_CODEX } from "@/fixtures/sessions";

export const LONG_FILE_PATH = "src/generated/big_table.py";

function turn(partial: Omit<TurnRef, "path" | "outcome" | "agentId">): TurnRef {
  return { ...partial, agentId: null, outcome: "ok", path: LONG_FILE_PATH };
}

const turns: TurnRef[] = [
  turn({
    harness: "claudeCode",
    sessionId: SESSION_CLAUDE,
    turnId: "c3a1f0e2-0001",
    turnIndex: 0,
    toolUseId: "toolu_fixture_c1-edit",
    at: new Date(Date.UTC(2026, 10, 20, 9, 0, 31)).toISOString(),
  }),
  turn({
    harness: "codex",
    sessionId: SESSION_CODEX,
    turnId: "01a1f812-9f04-7c11-8d2e-5b0a7e3c1d22",
    turnIndex: 1,
    toolUseId: "toolu_fixture_x2-retry",
    at: new Date(Date.UTC(2026, 10, 20, 9, 12, 29)).toISOString(),
  }),
];

/** Build the origins of a `lines`-line file in `worktreeId`. */
export function buildLongFileOrigins(worktreeId: WorktreeId, lines = 2000): LineOriginsResult {
  const runs: LineRun[] = [];
  let start = 1;
  let i = 0;
  while (start <= lines) {
    // A small linear congruential sequence: deterministic, no Math.random.
    const length = 1 + ((i * 37 + 11) % 40);
    const end = Math.min(lines, start + length - 1);
    const pick = i % 5;
    let origin: LineOriginKind;
    if (pick === 0 || pick === 3) origin = { kind: "base" };
    else if (pick === 4) {
      const reason = unknownLineReasons[i % unknownLineReasons.length] ?? "noCapture";
      origin = { kind: "unknown", reason };
    } else {
      const ref = turns[pick - 1] ?? turns[0];
      if (!ref) throw new Error("no fixture turn");
      origin = { kind: "edit", turn: ref };
    }
    runs.push({
      startLine: start,
      endLine: end,
      origin,
      evidence: origin.kind === "unknown" ? "unknown" : "observed",
    });
    start = end + 1;
    i++;
  }
  return originsOf(worktreeId, LONG_FILE_PATH, runs);
}
