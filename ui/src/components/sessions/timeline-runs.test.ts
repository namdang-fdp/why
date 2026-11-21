import { describe, expect, it } from "vitest";
import type { ShellOutcome, TimelineEvent, ToolOutcome, Turn } from "@/contracts";
import { eventGroups, foldRuns, runLabel } from "./timeline-model";
import { outcomesNotRecorded } from "./outcome-model";

function event(
  id: string,
  kind: TimelineEvent["kind"],
  outcome: ToolOutcome | null = "unknown",
  agentId: string | null = null,
  summary = `echo ${id}`,
): TimelineEvent {
  return {
    id,
    kind,
    at: "2026-11-20T09:00:00.000Z",
    toolName: null,
    summary,
    files: [],
    evidence: "observed",
    outcome,
    agentId,
    toolUseId: null,
  };
}

const shapes = (events: TimelineEvent[]) =>
  foldRuns(events).map((item) => (item.type === "run" ? item.events.length : item.event.kind));

describe("folding consecutive calls into a run", () => {
  it("does not fold two shell calls, and folds three or more", () => {
    expect(shapes([event("a", "shell"), event("b", "shell")])).toEqual(["shell", "shell"]);
    expect(shapes([event("a", "shell"), event("b", "shell"), event("c", "shell")])).toEqual([3]);
    expect(shapes(Array.from({ length: 12 }, (_, i) => event(`s${i}`, "shell")))).toEqual([12]);
  });

  it("splits the run at an edit or a test run, and never folds them", () => {
    const events = [
      event("a", "shell"),
      event("b", "shell"),
      event("c", "shell"),
      event("e", "edit", "ok"),
      event("d", "shell"),
      event("f", "shell"),
      event("g", "shell"),
      event("t", "testRun", "ok"),
      event("h", "shell"),
    ];
    expect(shapes(events)).toEqual([3, "edit", 3, "testRun", "shell"]);
  });

  it("never folds messages, subagents or stops, and they split a run", () => {
    for (const kind of ["assistantMessage", "subagent", "stop", "prompt"] as const) {
      const events = [
        event("a", "shell"),
        event("b", "shell"),
        event("m", kind, null),
        event("c", "shell"),
        event("d", "shell"),
      ];
      expect(shapes(events)).toEqual(["shell", "shell", kind, "shell", "shell"]);
    }
  });

  it("counts generic tool calls with shell calls and labels a mix honestly", () => {
    const mixed = [event("a", "shell"), event("b", "toolCall"), event("c", "shell")];
    expect(shapes(mixed)).toEqual([3]);
    expect(runLabel(mixed)).toBe("3 shell and tool calls");
    expect(runLabel([event("a", "shell"), event("b", "shell"), event("c", "shell")])).toBe(
      "3 shell commands",
    );
    expect(runLabel([event("a", "toolCall"), event("b", "toolCall"), event("c", "toolCall")])).toBe(
      "3 tool calls",
    );
  });

  it("never merges a subagent's calls with the main agent's (after eventGroups)", () => {
    const events = [
      event("a", "shell"),
      event("b", "shell"),
      event("x", "shell", "unknown", "agent-1"),
      event("y", "shell", "unknown", "agent-1"),
      event("c", "shell"),
    ];
    // Each agent's neighbors are folded separately: 2 + 2 + 1, so nothing reaches 3.
    const folded = eventGroups(events).map((group) => shapes(group.events));
    expect(folded).toEqual([["shell", "shell"], ["shell", "shell"], ["shell"]]);
    // And three calls in a row for the same subagent do fold, on their own.
    const busy = [
      event("a", "shell"),
      ...["x", "y", "z"].map((id) => event(id, "shell", "unknown", "agent-1")),
    ];
    expect(eventGroups(busy).map((group) => shapes(group.events))).toEqual([["shell"], [3]]);
  });

  it("keys a run by its first event, so the key survives new calls being appended", () => {
    const events = [event("a", "shell"), event("b", "shell"), event("c", "shell")];
    const [before] = foldRuns(events);
    const [after] = foldRuns([...events, event("d", "shell")]);
    expect(before?.type === "run" && before.id).toBe("run:a");
    expect(after?.type === "run" && after.id).toBe("run:a");
  });

  it("does not decide anything from the command text", () => {
    // `cat` looks read-only and `rm` does not: the grouping must not care.
    const events = [
      event("a", "shell", "ok", null, "cat a"),
      event("b", "shell", "ok", null, "rm -rf x"),
      event("c", "shell", "ok", null, "ls"),
    ];
    expect(shapes(events)).toEqual([3]);
  });
});

function turn(events: TimelineEvent[]): Turn {
  return { id: "t", index: 0, prompt: null, startedAt: "x", endedAt: null, events };
}

describe("when outcomes are not recorded for the whole session", () => {
  const recovered = (id: string, outcome: "ok" | "failed"): ReadonlyMap<string, ShellOutcome> =>
    new Map([
      [
        id,
        {
          toolUseId: id,
          eventId: id,
          turnId: "t",
          agentId: null,
          exitCode: outcome === "ok" ? 0 : 1,
          outcome,
          evidence: "observed",
          source: "codexRollout",
        },
      ],
    ]);

  it("is true only when there is at least one call and every one is unknown", () => {
    expect(outcomesNotRecorded([], undefined)).toBe(false);
    expect(outcomesNotRecorded([turn([event("p", "prompt", null)])], undefined)).toBe(false);
    expect(outcomesNotRecorded([turn([event("a", "shell"), event("b", "edit")])], undefined)).toBe(
      true,
    );
  });

  it("is false when one call failed, even in an otherwise unknown session", () => {
    expect(
      outcomesNotRecorded([turn([event("a", "shell"), event("b", "shell", "failed")])], undefined),
    ).toBe(false);
  });

  it("is false when a recovered exit code settles a call", () => {
    const turns = [turn([event("a", "shell"), event("b", "shell")])];
    expect(outcomesNotRecorded(turns, recovered("b", "ok"))).toBe(false);
    expect(outcomesNotRecorded(turns, recovered("zzz", "ok"))).toBe(true);
  });
});
