import { describe, expect, it, vi } from "vitest";
import {
  changesByWorktree,
  fileDiffs,
  REPO_WHY,
  repositories,
  worktreeChangedAll,
  worktrees,
  WT_WHY_AGENT,
  WT_WHY_MAIN,
} from "@/fixtures";
import { FixtureDataSource } from "./fixture-data-source";

describe("FixtureDataSource", () => {
  const source = new FixtureDataSource();

  it("returns the registered repositories, worktrees and mode-specific changes", async () => {
    const changes = changesByWorktree[WT_WHY_AGENT];
    await expect(source.listRepositories()).resolves.toEqual(repositories);
    await expect(source.listWorktrees(REPO_WHY)).resolves.toEqual(
      worktrees.filter((worktree) => worktree.repoId === REPO_WHY),
    );
    await expect(source.getChanges(WT_WHY_AGENT, "vsBase")).resolves.toEqual(changes);
    await expect(source.getChanges(WT_WHY_AGENT, "staged")).resolves.toEqual(
      changes.filter((file) => file.staged),
    );
    await expect(source.getChanges(WT_WHY_MAIN, "staged")).resolves.toEqual([]);
  });

  it("returns a diff only for a file in the requested worktree and mode", async () => {
    const path = "src-tauri/src/git/mod.rs";
    await expect(source.getFileDiff(WT_WHY_AGENT, path, "vsBase")).resolves.toEqual(
      fileDiffs.find((diff) => diff.path === path),
    );
    await expect(source.getFileDiff(WT_WHY_MAIN, path, "uncommitted")).rejects.toEqual({
      kind: "notFound",
    });
    await expect(source.getFileDiff(WT_WHY_AGENT, "docs/notes.md", "staged")).rejects.toEqual({
      kind: "notFound",
    });
  });

  it("returns a diff for every listed fixture, including the supplementary samples", async () => {
    for (const worktree of worktrees.filter((entry) => entry.loadError === null)) {
      const mode = worktree.base ? "vsBase" : "uncommitted";
      for (const file of await source.getChanges(worktree.id, mode)) {
        await expect(source.getFileDiff(worktree.id, file.path, mode)).resolves.toMatchObject({
          path: file.path,
          status: file.status,
          binary: file.binary,
          pathLossy: file.pathLossy,
        });
      }
    }
  });

  it("rejects a base comparison when the worktree has no base", async () => {
    await expect(source.getChanges(WT_WHY_MAIN, "vsBase")).rejects.toMatchObject({
      kind: "invalidInput",
    });
  });

  it("unsubscribes listeners from fixture events", () => {
    const listener = vi.fn();
    const unsubscribe = source.onWorktreeChanged(listener);
    source.emitWorktreeChanged(worktreeChangedAll);
    unsubscribe();
    source.emitWorktreeChanged(worktreeChangedAll);
    expect(listener).toHaveBeenCalledExactlyOnceWith(worktreeChangedAll);
  });
});
