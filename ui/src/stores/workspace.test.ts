import { describe, expect, it, vi } from "vitest";
import type { FileChange } from "@/contracts";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { changesByWorktree, WT_WHY_AGENT, WT_WHY_MAIN, worktreeChangedAll } from "@/fixtures";
import { createWorkspaceStore, selectChanges, selectDiffMode } from "./workspace";

describe("workspace selection", () => {
  it("wraps files in both directions starting with no selection", async () => {
    const store = createWorkspaceStore(new FixtureDataSource());
    await store.getState().loadRepositories();
    const files = changesByWorktree[WT_WHY_AGENT];
    expect(store.getState().filePath).toBeNull();
    await store.getState().moveFile(-1);
    expect(store.getState().filePath).toBe(files.at(-1)?.path);
    await store.getState().moveFile(1);
    expect(store.getState().filePath).toBe(files[0]?.path);
    await store.getState().moveFile(-1);
    expect(store.getState().filePath).toBe(files.at(-1)?.path);
  });

  it("wraps worktrees within the repository and remembers the preferred mode", async () => {
    const store = createWorkspaceStore(new FixtureDataSource());
    await store.getState().loadRepositories();
    expect(store.getState().worktreeId).toBe(WT_WHY_AGENT);
    expect(selectDiffMode(store.getState())).toBe("vsBase");

    await store.getState().moveWorktree(1);
    expect(store.getState().worktreeId).toBe(WT_WHY_MAIN);
    expect(selectDiffMode(store.getState())).toBe("uncommitted");
    await store.getState().setDiffMode("vsBase");
    expect(selectDiffMode(store.getState())).toBe("uncommitted");
    await store.getState().moveWorktree(-1);
    expect(store.getState().worktreeId).toBe(WT_WHY_AGENT);
    expect(selectDiffMode(store.getState())).toBe("vsBase");
    await store.getState().moveWorktree(-1);
    expect(store.getState().worktreeId).toBe(WT_WHY_MAIN);
  });

  it("refreshes on a worktree event and clears a file removed by the refresh", async () => {
    const source = new FixtureDataSource();
    const store = createWorkspaceStore(source);
    await store.getState().loadRepositories();
    await store.getState().moveFile(1);
    const getChanges = vi.spyOn(source, "getChanges").mockResolvedValueOnce([]);
    const stop = store.getState().watchWorktrees();
    source.emitWorktreeChanged(worktreeChangedAll);

    await vi.waitFor(() =>
      expect(selectChanges(store.getState())).toEqual({ status: "ok", data: [] }),
    );
    expect(getChanges).toHaveBeenCalledWith(WT_WHY_AGENT, "vsBase");
    expect(store.getState().filePath).toBeNull();
    stop();
  });

  it("ignores a stale response after changing modes", async () => {
    const source = new FixtureDataSource();
    const store = createWorkspaceStore(source);
    await store.getState().loadRepositories();
    let finish!: (files: FileChange[]) => void;
    const delayed = new Promise<FileChange[]>((resolve) => {
      finish = resolve;
    });
    vi.spyOn(source, "getChanges").mockReturnValueOnce(delayed);

    const pending = store.getState().setDiffMode("staged");
    await store.getState().setDiffMode("uncommitted");
    await store.getState().moveFile(1);
    finish([]);
    await pending;
    expect(selectDiffMode(store.getState())).toBe("uncommitted");
    expect(store.getState().filePath).toBe(changesByWorktree[WT_WHY_AGENT][0]?.path);
  });
});
