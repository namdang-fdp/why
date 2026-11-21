import { describe, expect, it } from "vitest";
import type { FileAttribution, TurnRef, WorktreeAttribution } from "@/contracts";
import {
  coverageCounts,
  currentAttribution,
  fileCaveat,
  fileChipLabel,
  unexplainedExplanation,
} from "./attribution-model";

const forbidden = /manual|\byou\b|\byour\b|human|100\s?%/i;

function ref(outcome: TurnRef["outcome"]): TurnRef {
  return {
    harness: "codex",
    sessionId: "s",
    turnId: "t",
    turnIndex: 0,
    agentId: null,
    toolUseId: null,
    at: "2026-11-20T09:00:00.000Z",
    outcome,
    path: "a.ts",
  };
}

function file(
  status: FileAttribution["status"],
  explainedBy: TurnRef[] = [],
  attempts: TurnRef[] = [],
): FileAttribution {
  return {
    change: {
      path: "a.ts",
      oldPath: null,
      status: "modified",
      staged: false,
      unstaged: true,
      additions: 1,
      deletions: 0,
      binary: false,
      pathLossy: false,
    },
    status,
    evidence: status === "explained" ? "observed" : "unknown",
    explainedBy,
    attempts,
  };
}

const answer = (worktreeId: string, mode: WorktreeAttribution["mode"]): WorktreeAttribution => ({
  worktreeId,
  mode,
  files: [],
  shellLeads: [],
});

describe("currentAttribution", () => {
  it("passes an answer for exactly this worktree and comparison", () => {
    const data = answer("wt-a", "vsBase");
    expect(currentAttribution({ status: "ok", data }, "wt-a", "vsBase")).toEqual({
      status: "ok",
      data,
    });
  });

  it("treats another worktree's or another mode's answer as still loading", () => {
    const data = answer("wt-a", "vsBase");
    expect(currentAttribution({ status: "ok", data }, "wt-b", "vsBase")).toEqual({
      status: "loading",
    });
    expect(currentAttribution({ status: "ok", data }, "wt-a", "uncommitted")).toEqual({
      status: "loading",
    });
  });

  it("is idle with no worktree or no scope, and passes loading and errors through", () => {
    const data = answer("wt-a", "vsBase");
    expect(currentAttribution({ status: "ok", data }, null, "vsBase")).toEqual({ status: "idle" });
    expect(currentAttribution({ status: "ok", data }, "wt-a", undefined)).toEqual({
      status: "idle",
    });
    expect(currentAttribution({ status: "loading" }, "wt-a", "vsBase").status).toBe("loading");
    const error = { kind: "internal", message: "x" } as const;
    expect(currentAttribution({ status: "error", error }, "wt-a", "vsBase")).toEqual({
      status: "error",
      error,
    });
  });
});

describe("coverageCounts", () => {
  it("is all zeros for zero files", () => {
    expect(coverageCounts([])).toEqual({ explained: 0, attempted: 0, unexplained: 0, total: 0 });
  });

  it("splits files into three buckets that always sum to the total", () => {
    const files = [
      file("explained", [ref("ok")]),
      file("unexplained", [], [ref("failed")]),
      file("unexplained", [], [ref("unknown")]),
      file("unexplained"),
      file("unexplained"),
    ];
    expect(coverageCounts(files)).toEqual({
      explained: 1,
      attempted: 2,
      unexplained: 2,
      total: 5,
    });
  });
});

describe("evidence copy", () => {
  it("states each kind without claiming who changed the file", () => {
    const labels = [
      fileChipLabel(file("explained", [ref("ok")])),
      fileChipLabel(file("explained", [ref("ok"), ref("ok")])),
      fileChipLabel(file("unexplained", [], [ref("failed")])),
      fileChipLabel(file("unexplained", [], [ref("unknown")])),
      fileChipLabel(file("unexplained")),
      fileCaveat,
      unexplainedExplanation,
    ];
    expect(labels[0]).toBe("Explained by 1 captured edit");
    expect(labels[1]).toBe("Explained by 2 captured edits");
    expect(labels[2]).toBe("Agent tried, not applied");
    expect(labels[3]).toBe("Agent edit not confirmed");
    expect(labels[4]).toBe("No captured agent edit");
    for (const label of labels) expect(label).not.toMatch(forbidden);
  });
});
