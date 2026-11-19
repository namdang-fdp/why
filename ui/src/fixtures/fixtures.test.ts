import { describe, expect, it } from "vitest";
import type { ChangeStatus, IpcError } from "@/contracts";
import { WORKTREE_CHANGED_EVENT } from "@/contracts";
import {
  changesByWorktree,
  fileDiffs,
  ipcErrors,
  repositories,
  worktreeChangedAll,
  worktrees,
} from "./index";

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
