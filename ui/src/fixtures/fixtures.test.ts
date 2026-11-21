import { describe, expect, it } from "vitest";
import type { ChangeStatus, IpcError, LineOriginsResult } from "@/contracts";
import { WORKTREE_CHANGED_EVENT } from "@/contracts";
import { buildLongFileOrigins, LONG_FILE_PATH } from "@/dev-fixtures/long-file-origins";
import {
  changesByWorktree,
  engineNotReady,
  fileDiffs,
  ipcErrors,
  lineOriginsByFile,
  repositories,
  unknownLineReasons,
  worktreeChangedAll,
  worktrees,
  WT_WHY_AGENT,
} from "./index";
import { sessionTimelines } from "./sessions";

// A `Record` over the union must list every member, so adding a ChangeStatus in Rust
// breaks `tsc` here until the new status is added (and then needs a fixture below).
const allStatuses: Record<ChangeStatus, true> = {
  added: true,
  modified: true,
  deleted: true,
  renamed: true,
  copied: true,
  typeChanged: true,
  untracked: true,
  conflicted: true,
};

const allChanges = Object.values(changesByWorktree).flat();

describe("fixtures", () => {
  it("have 2 repositories and 4 worktrees that reference them", () => {
    expect(repositories).toHaveLength(2);
    expect(worktrees).toHaveLength(4);
    const repoIds = new Set<string>(repositories.map((r) => r.id));
    for (const wt of worktrees) expect(repoIds.has(wt.repoId)).toBe(true);
  });

  it("include a main, an agent branch ahead of its base, and a detached worktree", () => {
    expect(worktrees.some((w) => w.isMain && w.branch === "main")).toBe(true);
    expect(worktrees.some((w) => !w.isMain && (w.base?.ahead ?? 0) > 0)).toBe(true);
    expect(worktrees.some((w) => w.isDetached && w.branch === null)).toBe(true);
  });

  it("include a worktree that failed to load, with the error and no measurements", () => {
    const broken = worktrees.filter((w) => w.loadError !== null);
    expect(broken).toHaveLength(1);
    expect(broken[0]).toMatchObject({ base: null, changeCount: 0 });
    // Everything else loaded fine.
    expect(worktrees.filter((w) => w.loadError === null)).toHaveLength(worktrees.length - 1);
  });

  it("have a changeCount equal to the number of listed changes", () => {
    for (const wt of worktrees) {
      const listed = changesByWorktree[wt.id as keyof typeof changesByWorktree] ?? [];
      expect(listed).toHaveLength(wt.changeCount);
    }
  });

  it("cover every ChangeStatus", () => {
    const seen = new Set<string>(allChanges.map((c) => c.status));
    for (const status of Object.keys(allStatuses)) expect(seen.has(status)).toBe(true);
  });

  it("cover binary, rename and lossy-path edge cases", () => {
    expect(allChanges.some((c) => c.binary && c.additions === null)).toBe(true);
    expect(allChanges.some((c) => c.status === "renamed" && c.oldPath !== null)).toBe(true);
    expect(allChanges.some((c) => c.pathLossy && c.path.includes("�"))).toBe(true);
  });

  it("have diffs for listed files, including binary and too-large ones", () => {
    const changedPaths = new Set<string>(allChanges.map((c) => c.path));
    for (const diff of fileDiffs) expect(changedPaths.has(diff.path)).toBe(true);
    expect(fileDiffs.some((d) => d.binary && d.newContent === null)).toBe(true);
    expect(fileDiffs.some((d) => d.tooLarge && d.newContent === null)).toBe(true);
    expect(fileDiffs.some((d) => d.status === "added" && d.oldContent === null)).toBe(true);
    expect(fileDiffs.some((d) => d.status === "deleted" && d.newContent === null)).toBe(true);
  });

  it("have one sample of every IpcError kind", () => {
    const kinds: Record<IpcError["kind"], true> = {
      notARepo: true,
      gitMissing: true,
      gitFailed: true,
      timeout: true,
      notFound: true,
      invalidInput: true,
      untrustedFilters: true,
      internal: true,
    };
    expect(new Set(ipcErrors.map((e) => e.kind))).toEqual(new Set(Object.keys(kinds)));
  });

  it("round-trip through JSON unchanged (what Tauri does on the wire)", () => {
    const all = { repositories, worktrees, changesByWorktree, fileDiffs, ipcErrors };
    expect(JSON.parse(JSON.stringify(all))).toEqual(all);
  });

  it("share the event name with Rust and use empty paths for refetch-all", () => {
    expect(WORKTREE_CHANGED_EVENT).toBe("worktree-changed");
    expect(worktreeChangedAll.paths).toEqual([]);
  });
});

describe("line-origin fixtures (WHY-64)", () => {
  const results: { label: string; result: LineOriginsResult }[] = [
    ...Object.entries(lineOriginsByFile).flatMap(([wt, files]) =>
      Object.entries(files).map(([path, result]) => ({ label: `${wt}:${path}`, result })),
    ),
    { label: "long file", result: buildLongFileOrigins(WT_WHY_AGENT) },
  ];
  const ok = results.flatMap(({ label, result }) =>
    result.status === "ok" ? [{ label, origins: result.origins }] : [],
  );
  const timelines = new Map(sessionTimelines.map((t) => [t.session.id, t]));

  it("cover base, two Claude turns, a Codex turn, every unknown reason and unavailable", () => {
    const runs = ok.flatMap((o) => o.origins.runs);
    expect(runs.some((r) => r.origin.kind === "base")).toBe(true);
    const turns = new Set(
      runs.flatMap((r) =>
        r.origin.kind === "edit" ? [`${r.origin.turn.harness}:${r.origin.turn.turnId}`] : [],
      ),
    );
    expect([...turns].filter((t) => t.startsWith("claudeCode:")).length).toBeGreaterThanOrEqual(2);
    expect([...turns].some((t) => t.startsWith("codex:"))).toBe(true);
    const reasons = new Set(
      runs.flatMap((r) => (r.origin.kind === "unknown" ? [r.origin.reason] : [])),
    );
    expect([...reasons].sort()).toEqual([...unknownLineReasons].sort());
    expect(results.some(({ result }) => result.status === "unavailable")).toBe(true);
    expect(engineNotReady).toEqual({ status: "unavailable", reason: "engineNotReady" });
  });

  it("partition each file into runs without gaps or overlaps", () => {
    for (const { label, origins } of ok) {
      let next = 1;
      for (const r of origins.runs) {
        expect(r.startLine, label).toBe(next);
        expect(r.endLine, label).toBeGreaterThanOrEqual(r.startLine);
        next = r.endLine + 1;
      }
      expect(next - 1, label).toBe(origins.lineCount);
    }
  });

  it("label evidence observed for base and edit, unknown for unknown", () => {
    for (const { origins } of ok) {
      for (const r of origins.runs) {
        expect(r.evidence).toBe(r.origin.kind === "unknown" ? "unknown" : "observed");
      }
    }
  });

  it("have coverage that adds up, per harness", () => {
    for (const { label, origins } of ok) {
      const c = origins.coverage;
      const explained = c.explained.reduce((sum, h) => sum + h.lines, 0);
      expect(c.lines, label).toBe(origins.lineCount);
      expect(c.base + c.unknown + explained, label).toBe(c.lines);
      const count = (pred: (r: (typeof origins.runs)[number]) => boolean) =>
        origins.runs.filter(pred).reduce((sum, r) => sum + r.endLine - r.startLine + 1, 0);
      expect(c.base, label).toBe(count((r) => r.origin.kind === "base"));
      expect(c.unknown, label).toBe(count((r) => r.origin.kind === "unknown"));
      for (const h of c.explained) {
        expect(h.lines, label).toBe(
          count((r) => r.origin.kind === "edit" && r.origin.turn.harness === h.harness),
        );
      }
    }
  });

  it("point every edit run at a real edit event of the fixture sessions", () => {
    for (const { label, origins } of ok) {
      for (const r of origins.runs) {
        if (r.origin.kind !== "edit") continue;
        const ref = r.origin.turn;
        const timeline = timelines.get(ref.sessionId);
        expect(timeline, label).toBeDefined();
        if (!timeline) continue;
        expect(timeline.session.harness, label).toBe(ref.harness);
        const turn = timeline.turns[ref.turnIndex];
        expect(turn?.id, label).toBe(ref.turnId);
        const event = turn?.events.find((e) => e.toolUseId === ref.toolUseId);
        expect(event, `${label}: ${ref.toolUseId}`).toBeDefined();
        expect(event?.kind).toBe("edit");
        expect(event?.agentId ?? null).toBe(ref.agentId);
        expect(event?.at).toBe(ref.at);
        expect(ref.outcome).toBe("ok");
        if (origins.path !== LONG_FILE_PATH) expect(event?.files).toContain(ref.path);
        expect(ref.path).toBe(origins.path);
      }
    }
  });
});
