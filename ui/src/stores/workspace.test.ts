import { assert, describe, expect, it, vi } from "vitest";
import type { FileChange, FileDiff } from "@/contracts";
import { FixtureDataSource } from "@/data/fixture-data-source";
import {
  changesByWorktree,
  worktrees,
  WT_WHY_AGENT,
  WT_WHY_MAIN,
  worktreeChangedAll,
} from "@/fixtures";
import { deferred } from "@/test/deferred";
import { createWorkspaceStore, selectChanges, selectDiffMode, selectWorktree } from "./workspace";

describe("workspace selection", () => {
  it("opens the first file on repository selection and wraps in both directions", async () => {
    const store = createWorkspaceStore(new FixtureDataSource());
    await store.getState().loadRepositories();
    const files = changesByWorktree[WT_WHY_AGENT];
    expect(store.getState().filePath).toBe(files[0]?.path);
    await store.getState().moveFile(-1);
    expect(store.getState().filePath).toBe(files.at(-1)?.path);
    await store.getState().moveFile(1);
    expect(store.getState().filePath).toBe(files[0]?.path);
    await store.getState().moveFile(-1);
    expect(store.getState().filePath).toBe(files.at(-1)?.path);
  });

  it.each([false, true])("restores the last file when reverted=%s", async (reverted) => {
    const source = new FixtureDataSource();
    const store = createWorkspaceStore(source);
    const files = changesByWorktree[WT_WHY_AGENT];
    const [first, second] = files;
    assert(first && second);
    await store.getState().loadRepositories();
    await store.getState().selectFile(second.path);
    await store.getState().moveWorktree(1);
    if (reverted) {
      const remaining = files.filter((file) => file.path !== second.path);
      const stop = store.getState().watchWorktrees();
      vi.spyOn(source, "getChanges").mockResolvedValueOnce(remaining);
      source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [second.path] });
      await vi.waitFor(() =>
        expect(store.getState().changes.get(`vsBase:${WT_WHY_AGENT}`)).toMatchObject({
          status: "ok",
          data: remaining,
        }),
      );
      stop();
    }
    await store.getState().moveWorktree(-1);
    const path = reverted ? first.path : second.path;
    expect(store.getState().filePath).toBe(path);
    expect(store.getState().fileDiff).toMatchObject({ status: "ok", data: { path } });
  });

  it("lets a session link win while the first file is opening", async () => {
    const source = new FixtureDataSource();
    const store = createWorkspaceStore(source);
    const worktree = worktrees.find(({ id }) => id === WT_WHY_AGENT);
    const files = changesByWorktree[WT_WHY_AGENT];
    const third = files[2];
    assert(worktree && third);
    const staged = await source.getChanges(worktree.id, "staged");
    const vsBase = deferred<FileChange[]>();
    const pendingStaged = deferred<FileChange[]>();
    const changesStarted = deferred<void>();
    vi.spyOn(source, "getChanges").mockImplementation((_, mode) => {
      if (mode === "vsBase") {
        changesStarted.resolve(undefined);
        return vsBase.promise;
      }
      if (mode === "staged") return pendingStaged.promise;
      return Promise.resolve([]);
    });
    const selecting = store.getState().loadRepositories();
    await changesStarted.promise;
    const opening = store.getState().openSessionFile(third.path);
    vsBase.resolve(files.filter((file) => file.path !== third.path));
    await selecting;
    expect(store.getState().filePath).toBe(files[0]?.path);
    pendingStaged.resolve(staged);
    await opening;
    expect(selectDiffMode(store.getState())).toBe("staged");
    expect(store.getState().fileDiff).toMatchObject({
      status: "ok",
      data: { path: third.path },
    });
  });

  it("opens an explicit third file and falls back to the first file of a new mode", async () => {
    const source = new FixtureDataSource();
    const store = createWorkspaceStore(source);
    const worktree = worktrees.find(({ id }) => id === WT_WHY_AGENT);
    const third = changesByWorktree[WT_WHY_AGENT][2];
    assert(worktree && third);
    await store.getState().loadRepositories();
    await store.getState().moveWorktree(1);
    await store.getState().jumpToFile(worktree, third.path, ["vsBase"]);
    expect(store.getState().filePath).toBe(third.path);
    expect(store.getState().fileDiff).toMatchObject({ status: "ok", data: { path: third.path } });

    const unstaged = changesByWorktree[WT_WHY_AGENT].find((file) => !file.staged);
    assert(unstaged);
    await store.getState().selectFile(unstaged.path);
    const staged = await source.getChanges(worktree.id, "staged");
    expect(staged.some((file) => file.path === unstaged.path)).toBe(false);
    const firstStaged = staged[0];
    assert(firstStaged);
    await store.getState().setDiffMode("staged");
    expect(store.getState().filePath).toBe(firstStaged.path);
    expect(store.getState().fileDiff).toMatchObject({
      status: "ok",
      data: { path: firstStaged.path },
    });
  });

  it("shows only the last worktree's file and diff after twenty rapid selections", async () => {
    const source = new FixtureDataSource();
    const store = createWorkspaceStore(source);
    const template = worktrees.find(({ id }) => id === WT_WHY_AGENT);
    const first = changesByWorktree[WT_WHY_AGENT][0];
    assert(template && first);
    const diff = await source.getFileDiff(template.id, first.path, "vsBase");
    const requests = Array.from({ length: 20 }, (_, index) => ({
      worktree: { ...template, id: `rapid-${index}`, changeCount: 1 },
      file: { ...first, path: `file-${index}.ts` },
      changes: deferred<FileChange[]>(),
    }));
    const [oldest] = requests;
    const last = requests.at(-1);
    assert(oldest && last);
    vi.spyOn(source, "listWorktrees").mockImplementation(async () => [
      template,
      ...requests.map(({ worktree }) => worktree),
    ]);
    await store.getState().loadRepositories();
    vi.spyOn(source, "getChanges").mockImplementation((id) => {
      const request = requests.find(({ worktree }) => worktree.id === id);
      assert(request);
      return request.changes.promise;
    });
    const staleDiff = deferred<FileDiff>();
    const diffStarted = deferred<void>();
    const readDiff = vi.spyOn(source, "getFileDiff").mockImplementation(async (id, path) => {
      if (id === oldest.worktree.id) {
        diffStarted.resolve(undefined);
        return staleDiff.promise;
      }
      return { ...diff, path };
    });

    const firstNavigation = store.getState().selectWorktree(oldest.worktree);
    oldest.changes.resolve([oldest.file]);
    await diffStarted.promise;
    const laterNavigations = requests
      .slice(1)
      .map(({ worktree }) => store.getState().selectWorktree(worktree));
    expect(store.getState().filePath).toBeNull();
    for (const { changes, file } of requests.slice(1).reverse()) changes.resolve([file]);
    await Promise.all(laterNavigations);
    staleDiff.resolve({ ...diff, path: oldest.file.path });
    await firstNavigation;
    expect(selectWorktree(store.getState())?.id).toBe(last.worktree.id);
    expect(store.getState().filePath).toBe(last.file.path);
    expect(store.getState().fileDiff).toMatchObject({
      status: "ok",
      data: { path: last.file.path },
    });
    expect(readDiff.mock.calls.map(([id]) => id)).toEqual([oldest.worktree.id, last.worktree.id]);
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
    finish([]);
    await pending;
    expect(selectDiffMode(store.getState())).toBe("uncommitted");
    expect(store.getState().filePath).toBe(changesByWorktree[WT_WHY_AGENT][0]?.path);
  });

  it("keeps the current models' navigation after a slower file request resolves", async () => {
    const source = new FixtureDataSource();
    const store = createWorkspaceStore(source);
    await store.getState().loadRepositories();
    const [first, second] = changesByWorktree[WT_WHY_AGENT];
    assert(first && second);
    const firstDiff = await source.getFileDiff(WT_WHY_AGENT, first.path, "vsBase");
    // The default file is cached. Invalidate it before exercising a slow read.
    const stop = store.getState().watchWorktrees();
    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [first.path] });
    stop();
    let finish: (value: typeof firstDiff) => void = () => {};
    vi.spyOn(source, "getFileDiff").mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const pending = store.getState().selectFile(first.path);
    await store.getState().selectFile(second.path);
    const { fileDiff } = store.getState();
    assert(fileDiff.status === "ok");
    const move = vi.fn(() => "moved" as const);
    store.getState().registerDiffNavigation(fileDiff.data, move);
    finish(firstDiff);
    await pending;
    await store.getState().moveChange("next");
    expect(move).toHaveBeenCalledExactlyOnceWith("next");
    expect(store.getState().filePath).toBe(second.path);
    expect(store.getState().fileDiff).toBe(fileDiff);
  });

  it("stops at the last file and keeps previous change in the current file", async () => {
    const store = createWorkspaceStore(new FixtureDataSource());
    await store.getState().loadRepositories();
    await store.getState().moveFile(-1);
    const { fileDiff, filePath } = store.getState();
    assert(fileDiff.status === "ok");
    const move = vi.fn(() => "boundary" as const);
    store.getState().registerDiffNavigation(fileDiff.data, move);
    await store.getState().moveChange("next");
    await store.getState().moveChange("previous");
    expect(move.mock.calls).toEqual([["next"], ["previous"]]);
    expect(store.getState().filePath).toBe(filePath);
  });
});
