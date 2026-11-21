import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { FileChange, FileDiff, Worktree } from "@/contracts";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { changesByWorktree, REPO_WHY, WT_WHY_AGENT, WT_WHY_MAIN } from "@/fixtures";
import { createToastStore } from "./toasts";
import {
  createWorkspaceStore,
  pulseMs,
  refreshWindowMs,
  selectChanges,
  selectWorktree,
  selectWorktrees,
} from "./workspace";

const MOD = "src-tauri/src/git/mod.rs";
const NOTES = "docs/notes.md";

async function setup() {
  const source = new FixtureDataSource();
  const store = createWorkspaceStore(source, createToastStore());
  await store.getState().loadRepositories();
  await store.getState().selectFile(MOD);
  const spies = {
    listWorktrees: vi.spyOn(source, "listWorktrees"),
    getChanges: vi.spyOn(source, "getChanges"),
    getFileDiff: vi.spyOn(source, "getFileDiff"),
  };
  const stop = store.getState().watchWorktrees();
  // Every state the visible diff goes through, to prove a refresh never shows "loading".
  const diffStatuses: string[] = [];
  store.subscribe((state) => diffStatuses.push(state.fileDiff.status));
  const settle = () => vi.advanceTimersByTimeAsync(refreshWindowMs + 50);
  return { source, store, spies, stop, diffStatuses, settle };
}

describe("live updates", () => {
  beforeEach(() => vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] }));
  afterEach(() => vi.useRealTimers());

  it("refetches only the changed worktree's list and the open file's diff", async () => {
    const { source, store, spies, stop, diffStatuses, settle } = await setup();
    const before = store.getState().fileDiff;

    source.simulateEdit(WT_WHY_AGENT, MOD);
    await settle();

    // One list read (the visible mode doubles as the rail badge's), one diff read.
    expect(spies.getChanges.mock.calls).toEqual([[WT_WHY_AGENT, "vsBase"]]);
    expect(spies.getFileDiff.mock.calls).toEqual([[WT_WHY_AGENT, MOD, "vsBase"]]);
    // A file edit does not need the whole repository re-listed.
    expect(spies.listWorktrees).not.toHaveBeenCalled();

    const state = store.getState();
    // Selection untouched, the diff replaced IN PLACE: ok -> ok, never loading/idle.
    expect(state.worktreeId).toBe(WT_WHY_AGENT);
    expect(state.filePath).toBe(MOD);
    expect(state.fileDiff).not.toBe(before);
    expect(new Set(diffStatuses)).toEqual(new Set(["ok"]));
    expect(state.flashes.has(MOD)).toBe(true);
    stop();
  });

  it("does not refetch the open diff when the event is about another file", async () => {
    const { source, store, spies, stop, settle } = await setup();
    const open = store.getState().fileDiff;

    source.simulateEdit(WT_WHY_AGENT, NOTES);
    await settle();

    expect(spies.getChanges).toHaveBeenCalledTimes(1);
    expect(spies.getFileDiff).not.toHaveBeenCalled();
    expect(store.getState().fileDiff).toBe(open); // same object: nothing re-rendered
    expect(store.getState().filePath).toBe(MOD);
    expect([...store.getState().flashes.keys()]).toEqual([NOTES]);
    stop();
  });

  it("treats empty paths as 'refetch everything' for that worktree, including its metadata", async () => {
    const { source, spies, stop, settle } = await setup();

    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [] });
    await settle();

    expect(spies.listWorktrees.mock.calls).toEqual([[REPO_WHY]]);
    expect(spies.getChanges.mock.calls).toEqual([[WT_WHY_AGENT, "vsBase"]]);
    expect(spies.getFileDiff.mock.calls).toEqual([[WT_WHY_AGENT, MOD, "vsBase"]]);
    stop();
  });

  it("leaves a different worktree's open view alone but updates its badge", async () => {
    const { source, store, spies, stop, settle } = await setup();
    const more: FileChange[] = [
      ...changesByWorktree[WT_WHY_MAIN],
      {
        ...(changesByWorktree[WT_WHY_MAIN][0] as FileChange),
        path: "extra.txt",
        status: "untracked",
      },
    ];
    spies.getChanges.mockResolvedValueOnce(more);

    source.emitWorktreeChanged({ worktreeId: WT_WHY_MAIN, paths: ["extra.txt"] });
    await settle();

    // The base-less main worktree is counted by its uncommitted list.
    expect(spies.getChanges.mock.calls).toEqual([[WT_WHY_MAIN, "uncommitted"]]);
    expect(spies.getFileDiff).not.toHaveBeenCalled();
    const list = selectWorktrees(store.getState());
    expect(list.status === "ok" && list.data.find((w) => w.id === WT_WHY_MAIN)?.changeCount).toBe(
      3,
    );
    expect(store.getState().worktreeId).toBe(WT_WHY_AGENT);
    expect(store.getState().filePath).toBe(MOD);
    stop();
  });

  it("coalesces a burst of events into one refresh", async () => {
    const { source, spies, stop, settle } = await setup();

    for (let i = 0; i < 25; i++) {
      source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [i % 2 ? MOD : NOTES] });
      await vi.advanceTimersByTimeAsync(2); // a storm: well inside one window
    }
    await settle();

    expect(spies.getChanges).toHaveBeenCalledTimes(1);
    expect(spies.getFileDiff).toHaveBeenCalledTimes(1);
    stop();
  });

  it("runs one refresh per worktree at a time and picks up events that arrive meanwhile", async () => {
    const { source, spies, stop, settle } = await setup();
    let release: (files: FileChange[]) => void = () => {};
    spies.getChanges.mockReturnValueOnce(
      new Promise<FileChange[]>((resolve) => {
        release = resolve;
      }),
    );

    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [MOD] });
    await settle(); // first refresh now waits on its slow list
    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [NOTES] });
    await settle();
    expect(spies.getChanges).toHaveBeenCalledTimes(1); // the second waits its turn

    release(changesByWorktree[WT_WHY_AGENT]);
    await settle();
    expect(spies.getChanges).toHaveBeenCalledTimes(2);
    stop();
  });

  it("drops a diff response that arrives after the user opened another file", async () => {
    const { source, store, spies, stop, settle } = await setup();
    const stale = {
      ...(store.getState().fileDiff as { data: FileDiff }).data,
      newContent: "STALE",
    };
    let release: (diff: FileDiff) => void = () => {};
    spies.getFileDiff.mockReturnValueOnce(
      new Promise<FileDiff>((resolve) => {
        release = resolve;
      }),
    );

    source.simulateEdit(WT_WHY_AGENT, MOD);
    await settle(); // refresh asked for the diff and waits
    await store.getState().selectFile("src-tauri/src/git/worktree.rs");
    release(stale);
    await settle();

    const { fileDiff, filePath } = store.getState();
    expect(filePath).toBe("src-tauri/src/git/worktree.rs");
    expect(fileDiff.status === "ok" && fileDiff.data.path).toBe("src-tauri/src/git/worktree.rs");
    stop();
  });

  it("keeps showing the old data when a refresh fails", async () => {
    const { source, store, spies, stop, settle } = await setup();
    const files = selectChanges(store.getState());
    spies.getChanges.mockRejectedValueOnce({ kind: "timeout" });

    source.simulateEdit(WT_WHY_AGENT, MOD);
    await settle();

    expect(selectChanges(store.getState())).toBe(files);
    expect(store.getState().filePath).toBe(MOD);
    stop();
  });

  it("clears the selection when the open file is no longer changed", async () => {
    const { source, store, spies, stop, settle } = await setup();
    spies.getChanges.mockResolvedValueOnce(
      changesByWorktree[WT_WHY_AGENT].filter((file) => file.path !== MOD),
    );

    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [MOD] });
    await settle();

    expect(store.getState().filePath).toBeNull();
    expect(store.getState().fileDiff.status).toBe("idle");
    stop();
  });

  it("stops listening and drops queued work after unsubscribe", async () => {
    const { source, spies, stop, settle } = await setup();
    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [MOD] });
    stop();
    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [MOD] });
    await settle();
    expect(spies.getChanges).not.toHaveBeenCalled();
  });
});

/** A promise the test settles by hand. */
function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("repository-wide refreshes", () => {
  beforeEach(() => vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] }));
  afterEach(() => vi.useRealTimers());

  it("reads the worktree list once for N full-refresh events and refreshes every worktree from it", async () => {
    const { source, spies, stop, settle } = await setup();

    // A branch update tells every worktree of the repository to refetch everything.
    source.emitWorktreeChanged({ worktreeId: WT_WHY_MAIN, paths: [] });
    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [] });
    source.emitWorktreeChanged({ worktreeId: WT_WHY_MAIN, paths: [] });
    await settle();

    expect(spies.listWorktrees.mock.calls).toEqual([[REPO_WHY]]);
    // The SELECTED worktree is refreshed too, not dropped by a lost cache race.
    expect(spies.getChanges.mock.calls).toEqual([[WT_WHY_AGENT, "vsBase"]]);
    expect(spies.getFileDiff.mock.calls).toEqual([[WT_WHY_AGENT, MOD, "vsBase"]]);
    stop();
  });

  it("still refreshes the selected worktree when another worktree's event starts a newer read", async () => {
    const { source, store, spies, stop, settle } = await setup();
    const slow = deferred<Worktree[]>();
    const before = await source.listWorktrees(REPO_WHY);
    spies.listWorktrees.mockClear();
    spies.listWorktrees.mockReturnValueOnce(slow.promise);

    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [] });
    await settle(); // its list read is in flight
    source.emitWorktreeChanged({ worktreeId: WT_WHY_MAIN, paths: [] });
    await settle(); // a second, newer read answers at once
    expect(spies.getChanges).not.toHaveBeenCalled();

    slow.resolve(before); // the older read finally lands
    await settle();

    expect(spies.getChanges.mock.calls).toEqual([[WT_WHY_AGENT, "vsBase"]]);
    expect(spies.getFileDiff.mock.calls).toEqual([[WT_WHY_AGENT, MOD, "vsBase"]]);
    expect(store.getState().filePath).toBe(MOD);
    stop();
  });
});

describe("worktrees added and removed", () => {
  beforeEach(() => vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] }));
  afterEach(() => vi.useRealTimers());

  const names = (store: Awaited<ReturnType<typeof setup>>["store"]) => {
    const list = selectWorktrees(store.getState());
    return list.status === "ok" ? list.data.map((wt) => wt.id) : [];
  };

  it("shows a linked worktree an agent just created, leaving the selection alone", async () => {
    const { source, store, spies, stop, settle } = await setup();
    const added = source.simulateWorktreeAdded(REPO_WHY, "fresh");
    await settle();

    expect(spies.listWorktrees.mock.calls).toEqual([[REPO_WHY]]);
    expect(names(store)).toContain(added?.id);
    expect(store.getState().worktreeId).toBe(WT_WHY_AGENT);
    expect(store.getState().filePath).toBe(MOD);
    expect(store.getState().fileDiff.status).toBe("ok");
    stop();
  });

  it("drops a removed worktree from the rail without touching the selection", async () => {
    const { source, store, stop, settle } = await setup();
    source.simulateWorktreeRemoved(WT_WHY_MAIN);
    await settle();
    expect(names(store)).toEqual([WT_WHY_AGENT]);
    expect(store.getState().worktreeId).toBe(WT_WHY_AGENT);
    stop();
  });

  it("opens the neighbouring worktree’s first diff when the selected one disappears", async () => {
    const { source, store, spies, stop, settle } = await setup();
    const extra = source.simulateWorktreeAdded(REPO_WHY, "after");
    await settle();
    expect(names(store)).toEqual([WT_WHY_MAIN, WT_WHY_AGENT, extra?.id]);
    // The new fixture copies main's changed files. Give it the same diff too.
    const path = changesByWorktree[WT_WHY_MAIN][0]?.path;
    if (!path) throw new Error("fixture change missing");
    const diff = await source.getFileDiff(WT_WHY_MAIN, path, "uncommitted");
    spies.getFileDiff.mockResolvedValueOnce(diff);

    source.simulateWorktreeRemoved(WT_WHY_AGENT);
    await settle();

    const state = store.getState();
    // The worktree now at the removed one's position.
    expect(state.worktreeId).toBe(extra?.id);
    expect(state.filePath).toBe(path);
    expect(state.fileDiff).toMatchObject({ status: "ok", data: { path } });
    expect(selectWorktree(state)?.id).toBe(extra?.id);
    stop();
  });

  it("clears the selection when the last worktree goes", async () => {
    const { source, store, stop, settle } = await setup();
    source.simulateWorktreeRemoved(WT_WHY_MAIN);
    source.simulateWorktreeRemoved(WT_WHY_AGENT);
    await settle();
    expect([store.getState().worktreeId, store.getState().filePath]).toEqual([null, null]);
    expect(store.getState().fileDiff.status).toBe("idle");
    stop();
  });
});

describe("a live edit while the open diff is loading", () => {
  beforeEach(() => vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] }));
  afterEach(() => vi.useRealTimers());

  it("is replayed once when the load settles", async () => {
    const source = new FixtureDataSource();
    const store = createWorkspaceStore(source, createToastStore());
    await store.getState().loadRepositories();
    const stale = await source.getFileDiff(WT_WHY_AGENT, MOD, "vsBase");
    // Force a new read of the default file without keeping a queued refresh.
    const unwatch = store.getState().watchWorktrees();
    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [MOD] });
    unwatch();
    const slow = deferred<FileDiff>();
    const getFileDiff = vi.spyOn(source, "getFileDiff").mockReturnValueOnce(slow.promise);
    const stop = store.getState().watchWorktrees();

    const loading = store.getState().selectFile(MOD); // read begins here
    source.simulateEdit(WT_WHY_AGENT, MOD); // the file changes after that read
    await vi.advanceTimersByTimeAsync(refreshWindowMs + 50);
    expect(getFileDiff).toHaveBeenCalledTimes(1); // still waiting; nothing dropped

    slow.resolve(stale); // the old text arrives
    await loading;
    await vi.advanceTimersByTimeAsync(10);

    expect(getFileDiff).toHaveBeenCalledTimes(2); // exactly one re-read
    const { fileDiff } = store.getState();
    expect(fileDiff.status === "ok" && fileDiff.data.newContent).toContain("// live edit 1");
    stop();
  });

  it("is not replayed if the user opened another file meanwhile", async () => {
    const source = new FixtureDataSource();
    const store = createWorkspaceStore(source, createToastStore());
    await store.getState().loadRepositories();
    const stale = await source.getFileDiff(WT_WHY_AGENT, MOD, "vsBase");
    // Force a new read of the default file without keeping a queued refresh.
    const unwatch = store.getState().watchWorktrees();
    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [MOD] });
    unwatch();
    const slow = deferred<FileDiff>();
    const getFileDiff = vi.spyOn(source, "getFileDiff").mockReturnValueOnce(slow.promise);
    const stop = store.getState().watchWorktrees();

    const first = store.getState().selectFile(MOD);
    source.simulateEdit(WT_WHY_AGENT, MOD);
    await vi.advanceTimersByTimeAsync(refreshWindowMs + 50);
    await store.getState().selectFile("src-tauri/src/git/worktree.rs"); // reads fresh content anyway
    slow.resolve(stale);
    await first;
    await vi.advanceTimersByTimeAsync(refreshWindowMs + 50);

    // MOD was read twice: the original (still pending) and the prefetch of the
    // neighbour of the file the user moved to. A replay would be a third read.
    const reads = getFileDiff.mock.calls.filter(([, path]) => path === MOD);
    expect(reads).toHaveLength(2);
    expect(store.getState().filePath).toBe("src-tauri/src/git/worktree.rs");
    stop();
  });
});

describe("changing the base during a live refresh", () => {
  beforeEach(() => vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] }));
  afterEach(() => vi.useRealTimers());

  it("ignores a changed-files list that was read before the new base was saved", async () => {
    const { source, store, spies, stop, settle } = await setup();
    const slow = deferred<FileChange[]>();
    spies.getChanges.mockReturnValueOnce(slow.promise);

    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [NOTES] });
    await settle(); // the live list read (old base) is in flight

    const worktree = selectWorktree(store.getState());
    if (!worktree) throw new Error("no worktree");
    await store.getState().setBase(worktree, "origin/develop");
    await settle();
    expect(store.getState().filePath).toBe(MOD);
    const count = selectWorktree(store.getState())?.changeCount;

    slow.resolve([]); // old-base answer: no files at all
    await settle();

    // It must neither clear the selection nor overwrite the badge or the list.
    expect(store.getState().filePath).toBe(MOD);
    expect(selectWorktree(store.getState())?.changeCount).toBe(count);
    const list = selectChanges(store.getState());
    expect(list.status === "ok" && list.data.length).toBe(changesByWorktree[WT_WHY_AGENT].length);
    stop();
  });

  it("does not let a worktree-list read that began before the save overwrite the new base", async () => {
    const { source, store, spies, stop, settle } = await setup();
    const old = await source.listWorktrees(REPO_WHY);
    const slow = deferred<Worktree[]>();
    spies.listWorktrees.mockReturnValueOnce(slow.promise);

    source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [] });
    await settle(); // list read (old base) in flight

    const worktree = selectWorktree(store.getState());
    if (!worktree) throw new Error("no worktree");
    const saved = await store.getState().setBase(worktree, "origin/develop");
    expect(saved.status).toBe("ok");

    slow.resolve(old); // arrives afterwards, carrying origin/main
    await settle();

    expect(selectWorktree(store.getState())?.base?.baseRef).toBe("origin/develop");
    stop();
  });
});

describe("activity pulse", () => {
  beforeEach(() => vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] }));
  afterEach(() => vi.useRealTimers());

  it("pulses a worktree's dot for two seconds after an edit and remembers when it happened", async () => {
    const { source, store, stop } = await setup();
    const before = Math.floor(Date.now() / 1000);

    source.emitWorktreeChanged({ worktreeId: WT_WHY_MAIN, paths: ["a.txt"] });
    const first = store.getState().pulses.get(WT_WHY_MAIN);
    expect(first).toBeDefined();
    expect(store.getState().activity.get(WT_WHY_MAIN)).toBeGreaterThanOrEqual(before);

    // A second edit restarts the pulse (a new number, so the ring replays) and its clock.
    await vi.advanceTimersByTimeAsync(pulseMs - 500);
    source.emitWorktreeChanged({ worktreeId: WT_WHY_MAIN, paths: ["b.txt"] });
    expect(store.getState().pulses.get(WT_WHY_MAIN)).not.toBe(first);
    await vi.advanceTimersByTimeAsync(600);
    expect(store.getState().pulses.has(WT_WHY_MAIN)).toBe(true); // 500 + 600 < 2 x pulseMs
    await vi.advanceTimersByTimeAsync(pulseMs);
    expect(store.getState().pulses.has(WT_WHY_MAIN)).toBe(false);
    // The activity time stays: the rail keeps sorting by it.
    expect(store.getState().activity.has(WT_WHY_MAIN)).toBe(true);
    stop();
  });

  it("does not count a 'refetch everything' event (branch moves fan out to every worktree)", async () => {
    const { source, store, stop } = await setup();
    source.emitWorktreeChanged({ worktreeId: WT_WHY_MAIN, paths: [] });
    expect(store.getState().pulses.size).toBe(0);
    expect(store.getState().activity.size).toBe(0);
    stop();
  });

  it("stops pulsing when the watcher is unsubscribed", async () => {
    const { source, store, stop } = await setup();
    source.emitWorktreeChanged({ worktreeId: WT_WHY_MAIN, paths: ["a.txt"] });
    stop();
    expect(store.getState().pulses.size).toBe(0);
  });
});
