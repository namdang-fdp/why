import { describe, expect, it, vi } from "vitest";
import type { FileDiff } from "@/contracts";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { REPO_WHY, WT_WHY_AGENT } from "@/fixtures";
import { createToastStore } from "./toasts";
import { createWorkspaceStore, selectWorktree } from "./workspace";

const [A, B, C, D] = [
  "src-tauri/src/git/mod.rs",
  "src-tauri/src/git/worktree.rs",
  "src-tauri/src/git/parse.rs",
  "src-tauri/src/git/legacy.rs",
] as const;

async function setup() {
  const source = new FixtureDataSource();
  const store = createWorkspaceStore(source, createToastStore());
  await store.getState().loadRepositories();
  const reads = vi.spyOn(source, "getFileDiff");
  const pathsRead = () => reads.mock.calls.map(([, path]) => path);
  const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0));
  return { source, store, reads, pathsRead, settle };
}

describe("file diff cache and prefetch", () => {
  it("prefetches the files j and k would open, once each", async () => {
    const { store, pathsRead, settle } = await setup();
    await store.getState().selectFile(B); // second of eight: neighbours A and C
    await settle();
    expect(pathsRead().sort()).toEqual([A, B, C].sort());
    await store.getState().selectFile(B);
    await settle();
    expect(pathsRead().filter((path) => path === C)).toHaveLength(1); // no second prefetch
  });

  it("paints a prefetched neighbour in the same tick, without asking git again", async () => {
    const { store, reads, settle } = await setup();
    await store.getState().selectFile(A);
    await settle();
    const before = reads.mock.calls.length;

    const switching = store.getState().selectFile(B); // NOT awaited
    // Synchronous: already the final state, never a loading one.
    expect(store.getState().filePath).toBe(B);
    const { fileDiff } = store.getState();
    expect(fileDiff.status).toBe("ok");
    expect(fileDiff.status === "ok" && fileDiff.data.path).toBe(B);
    await switching;
    // B came from the cache; only B's own neighbours may have been requested.
    expect(reads.mock.calls.slice(before).map(([, path]) => path)).not.toContain(B);
  });

  it("serves a revisit from the cache", async () => {
    const { store, pathsRead, settle } = await setup();
    await store.getState().selectFile(D); // legacy.rs, not near A
    await settle();
    await store.getState().selectFile(A);
    await settle();
    const before = pathsRead().filter((path) => path === D).length;
    await store.getState().selectFile(D);
    await settle();
    expect(pathsRead().filter((path) => path === D)).toHaveLength(before);
  });

  it("drops a cached file the moment the watcher reports it, before any refresh runs", async () => {
    const { source, store, pathsRead, settle } = await setup();
    await store.getState().selectFile(A);
    await settle();
    await store.getState().selectFile(B);
    await settle();
    const stop = store.getState().watchWorktrees();
    const before = pathsRead().filter((path) => path === A).length;

    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [A] });
    // Immediately (the 80 ms refresh window has not elapsed): A must be re-read.
    await store.getState().selectFile(A);
    expect(pathsRead().filter((path) => path === A)).toHaveLength(before + 1);
    stop();
  });

  it("an event with no paths drops every cached file of that worktree", async () => {
    const { source, store, pathsRead, settle } = await setup();
    await store.getState().selectFile(A);
    await settle();
    await store.getState().selectFile(D);
    await settle();
    const stop = store.getState().watchWorktrees();
    const count = (path: string) => pathsRead().filter((p) => p === path).length;
    const [a, d] = [count(A), count(D)];

    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [] });
    await store.getState().selectFile(A);
    await store.getState().selectFile(D);
    expect([count(A), count(D)]).toEqual([a + 1, d + 1]);
    stop();
  });

  it("a repository event drops the cache of its worktrees", async () => {
    const { source, store, pathsRead, settle } = await setup();
    await store.getState().selectFile(A);
    await settle();
    await store.getState().selectFile(D);
    await settle();
    const stop = store.getState().watchWorktrees();
    const before = pathsRead().filter((path) => path === A).length;

    source.emitRepositoryChanged({ repoId: REPO_WHY });
    await store.getState().selectFile(A);
    expect(pathsRead().filter((path) => path === A)).toHaveLength(before + 1);
    stop();
  });

  it("does not cache an answer from a read that began before the file changed", async () => {
    const { source, store, reads, pathsRead, settle } = await setup();
    await store.getState().selectFile(A); // prefetches B and the last file
    await settle();
    const stale = await source.getFileDiff(WT_WHY_AGENT, C, "vsBase");
    const stop = store.getState().watchWorktrees();
    // The next read is the prefetch of C (B's neighbour), and it is slow ...
    let release: (diff: FileDiff) => void = () => {};
    reads.mockImplementationOnce(
      () =>
        new Promise<FileDiff>((resolve) => {
          release = resolve;
        }),
    );
    await store.getState().selectFile(B);
    expect(pathsRead().filter((path) => path === C)).toHaveLength(2); // the helper + the slow prefetch
    // ... the file changes while that read is in flight ...
    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [C] });
    release({ ...stale, newContent: "STALE" });
    await settle();

    // ... so the stale answer was not cached: opening C reads it again.
    await store.getState().selectFile(C);
    expect(pathsRead().filter((path) => path === C)).toHaveLength(3);
    const { fileDiff } = store.getState();
    expect(fileDiff.status === "ok" && fileDiff.data.newContent).not.toBe("STALE");
    stop();
  });

  it("changing the base drops the worktree's cached diffs", async () => {
    const { store, pathsRead, settle } = await setup();
    await store.getState().selectFile(A);
    await settle();
    const worktree = selectWorktree(store.getState());
    if (!worktree) throw new Error("no worktree");
    const before = pathsRead().filter((path) => path === A).length;
    await store.getState().setBase(worktree, "origin/develop"); // re-selects A
    await settle();
    expect(pathsRead().filter((path) => path === A).length).toBeGreaterThan(before);
  });
});
