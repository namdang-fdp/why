// A very long agent session (default 500 turns, a few thousand events) for
// building and judging a virtualized timeline. Generated, deterministic: the
// same `turns` always gives the same data, so tests and screenshots are stable.
import type {
  SessionTimeline,
  TimelineEvent,
  TimelineEventKind,
  ToolOutcome,
  Turn,
  WorktreeId,
} from "@/contracts";

export const SESSION_LONG = "codex:01a1f900-long-0000-0000-000000000500";

const START_MS = Date.UTC(2026, 10, 19, 8, 0, 0);
const FILES = ["src/calc.py", "src/notes.py", "src/helper.py", "tests/test_calc.py", "README.md"];

function iso(ms: number): string {
  return new Date(ms).toISOString();
}

/** Build a session of `turns` turns in `worktreeId`. */
export function buildLongSession(worktreeId: WorktreeId, turns = 500): SessionTimeline {
  const list: Turn[] = [];
  // Each turn takes 40 s, spaced one minute apart.
  for (let i = 0; i < turns; i++) {
    const startMs = START_MS + i * 60_000;
    const file = FILES[i % FILES.length] ?? "src/calc.py";
    const events: TimelineEvent[] = [
      {
        id: `long-${i}-prompt`,
        kind: "prompt",
        at: iso(startMs),
        toolName: null,
        summary: `Step ${i + 1}: adjust ${file}`,
        files: [],
        evidence: "observed",
        outcome: null,
        agentId: null,
      },
    ];
    // 1 to 4 tool calls per turn, cycling through the kinds a reviewer sees.
    const toolCount = 1 + (i % 4);
    for (let t = 0; t < toolCount; t++) {
      const kinds: TimelineEventKind[] = ["edit", "shell", "testRun", "toolCall"];
      const kind = kinds[(i + t) % kinds.length] ?? "toolCall";
      const outcomes: ToolOutcome[] = ["ok", "ok", "ok", "failed", "unknown"];
      events.push({
        id: `long-${i}-tool-${t}`,
        kind,
        at: iso(startMs + (t + 1) * 7_000),
        toolName: kind === "edit" ? "apply_patch" : kind === "toolCall" ? "Read" : "Bash",
        summary: kind === "edit" ? `Update File: ${file}` : `command ${t + 1} of turn ${i + 1}`,
        files: kind === "edit" || kind === "toolCall" ? [file] : [],
        evidence: "observed",
        outcome: outcomes[(i * 3 + t) % outcomes.length] ?? "ok",
        agentId: null,
      });
    }
    const last = i === turns - 1;
    if (!last) {
      events.push({
        id: `long-${i}-stop`,
        kind: "stop",
        at: iso(startMs + 40_000),
        toolName: null,
        summary: `Finished step ${i + 1}.`,
        files: [],
        evidence: "declared",
        outcome: null,
        agentId: null,
      });
    }
    list.push({
      id: `long-turn-${i}`,
      index: i,
      prompt: `Step ${i + 1}: adjust ${file}`,
      startedAt: iso(startMs),
      // The last turn is still running.
      endedAt: last ? null : iso(startMs + 40_000),
      events,
    });
  }
  const lastEvent = list.at(-1)?.events.at(-1);
  return {
    session: {
      id: SESSION_LONG,
      harness: "codex",
      model: "gpt-6-luna",
      worktreeId,
      cwd: "/home/dev/code/billing-api",
      task: "Step 1: adjust src/calc.py",
      startedAt: iso(START_MS),
      lastEventAt: lastEvent?.at ?? iso(START_MS),
      status: "live",
      turnCount: list.length,
    },
    turns: list,
  };
}
