import { afterEach, describe, expect, it, vi } from "vitest";
import type {
  LineOriginsResult,
  SessionsChanged,
  SessionTimeline,
  WorktreeChanged,
} from "@/contracts";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { WT_WHY_AGENT, WT_WHY_MAIN } from "@/fixtures";
import { engineNotReady, lineOriginsByFile, originsOf } from "@/fixtures/provenance";
import { SESSION_CLAUDE } from "@/fixtures/sessions";
import { deferred } from "@/test/deferred";
import {
  createLineOriginsStore,
  diffReadRevision,
  originsBehind,
  tagDiffRead,
} from "./line-origins";

afterEach(() => vi.useRealTimers());

const CALC = "src/calc.py";
const HELPER = "src/helper.py";

function fixture(worktreeId: string, path: string): LineOriginsResult {
  const result = lineOriginsByFile[worktreeId]?.[path];
  if (!result) throw new Error(`fixture: no origins for ${worktreeId}:${path}`);
  return result;
}

/** A source whose answers the test completes by hand, plus the events the core would push. */
function controllable() {
  const requests: {
    worktreeId: string;
    path: string;
    done: ReturnType<typeof deferred<LineOriginsResult>>;
  }[] = [];
  const worktreeListeners = new Set<(event: WorktreeChanged) => void>();
  const sessionListeners = new Set<(event: SessionsChanged) => void>();
  const timelines = new FixtureDataSource();
  const source = {
    getLineOrigins: vi.fn((worktreeId: string, path: string) => {
      const done = deferred<LineOriginsResult>();
      requests.push({ worktreeId, path, done });
      return done.promise;
    }),
    getSessionTimeline: vi.fn((sessionId: string): Promise<SessionTimeline> => {
      return timelines.getSessionTimeline(sessionId);
    }),
    onWorktreeChanged: (callback: (event: WorktreeChanged) => void) => {
      worktreeListeners.add(callback);
      return () => worktreeListeners.delete(callback);
    },
    onSessionsChanged: (callback: (event: SessionsChanged) => void) => {
      sessionListeners.add(callback);
      return () => sessionListeners.delete(callback);
    },
  };
  return {
    source,
    requests,
    emitWorktree: (event: WorktreeChanged) => worktreeListeners.forEach((fn) => fn(event)),
    emitSessions: (event: SessionsChanged) => sessionListeners.forEach((fn) => fn(event)),
  };
}

const nextRequest = (requests: ReturnType<typeof controllable>["requests"], index: number) => {
  const request = requests[index];
  if (!request) throw new Error(`no request #${index}`);
  return request;
};

describe("stale responses never paint", () => {
  it("switching file A -> B before A resolves: B's origins win, whichever answer comes last", async () => {
    const { source, requests } = controllable();
    const store = createLineOriginsStore(source);
    const loadingA = store.getState().setTarget(WT_WHY_MAIN, HELPER);
    const loadingB = store.getState().setTarget(WT_WHY_MAIN, "src/other.py");
    // The old file's answer is gone from the state the moment the target moves.
    expect(store.getState().origins).toEqual({ status: "loading" });
    expect(store.getState().target).toEqual({ worktreeId: WT_WHY_MAIN, path: "src/other.py" });

    const b = originsOf(WT_WHY_MAIN, "src/other.py", [
      { startLine: 1, endLine: 2, origin: { kind: "base" }, evidence: "observed" },
    ]);
    nextRequest(requests, 1).done.resolve(b);
    await loadingB;
    expect(store.getState().origins).toEqual({ status: "ok", data: b });

    // A answers late: it must change nothing.
    nextRequest(requests, 0).done.resolve(fixture(WT_WHY_MAIN, HELPER));
    await loadingA;
    expect(store.getState().origins).toEqual({ status: "ok", data: b });
  });

  it("an answer for A that lands while B is still loading leaves B loading, not A", async () => {
    const { source, requests } = controllable();
    const store = createLineOriginsStore(source);
    const loadingA = store.getState().setTarget(WT_WHY_MAIN, HELPER);
    void store.getState().setTarget(WT_WHY_MAIN, "src/other.py");
    nextRequest(requests, 0).done.resolve(fixture(WT_WHY_MAIN, HELPER));
    await loadingA;
    expect(store.getState().origins).toEqual({ status: "loading" });
  });

  it("switching worktree mid-request, same path, never shows the other worktree's runs", async () => {
    const { source, requests } = controllable();
    const store = createLineOriginsStore(source);
    // The same relative path exists in both worktrees with different origins.
    const loadingMain = store.getState().setTarget(WT_WHY_MAIN, CALC);
    const loadingAgent = store.getState().setTarget(WT_WHY_AGENT, CALC);
    const agentAnswer = fixture(WT_WHY_AGENT, CALC);
    nextRequest(requests, 1).done.resolve(agentAnswer);
    await loadingAgent;
    nextRequest(requests, 0).done.resolve(engineNotReady);
    await loadingMain;
    expect(store.getState().origins).toEqual({ status: "ok", data: agentAnswer });
    expect(store.getState().target?.worktreeId).toBe(WT_WHY_AGENT);
  });

  it("clearing the target drops an in-flight answer and the selected line", async () => {
    const { source, requests } = controllable();
    const store = createLineOriginsStore(source);
    const loading = store.getState().setTarget(WT_WHY_MAIN, HELPER);
    store.getState().selectLine(4);
    await store.getState().setTarget(null, null);
    nextRequest(requests, 0).done.resolve(fixture(WT_WHY_MAIN, HELPER));
    await loading;
    expect(store.getState()).toMatchObject({
      target: null,
      origins: { status: "idle" },
      line: null,
    });
  });

  it("a new file forgets the previous file's selected line", async () => {
    const { source, requests } = controllable();
    const store = createLineOriginsStore(source);
    const first = store.getState().setTarget(WT_WHY_MAIN, HELPER);
    nextRequest(requests, 0).done.resolve(fixture(WT_WHY_MAIN, HELPER));
    await first;
    store.getState().selectLine(7);
    void store.getState().setTarget(WT_WHY_AGENT, CALC);
    expect(store.getState().line).toBeNull();
  });

  it("asks again for the same file only when it was left and re-opened", async () => {
    const { source, requests } = controllable();
    const store = createLineOriginsStore(source);
    void store.getState().setTarget(WT_WHY_MAIN, HELPER);
    void store.getState().setTarget(WT_WHY_MAIN, HELPER);
    expect(requests).toHaveLength(1);
  });
});

describe("answers", () => {
  it("keeps engineNotReady as a normal answer, not an error", async () => {
    const source = new FixtureDataSource();
    const store = createLineOriginsStore(source);
    await store.getState().setTarget(WT_WHY_AGENT, "tests/test_calc.py");
    expect(store.getState().origins).toEqual({ status: "ok", data: engineNotReady });
  });

  it("turns a rejected request into an error state and retries on load()", async () => {
    const { source, requests } = controllable();
    const store = createLineOriginsStore(source);
    const first = store.getState().setTarget(WT_WHY_MAIN, HELPER);
    nextRequest(requests, 0).done.reject({ kind: "timeout" });
    await first;
    expect(store.getState().origins).toEqual({ status: "error", error: { kind: "timeout" } });
    const retry = store.getState().load();
    nextRequest(requests, 1).done.resolve(fixture(WT_WHY_MAIN, HELPER));
    await retry;
    expect(store.getState().origins.status).toBe("ok");
  });
});

describe("refetch on file and session changes", () => {
  async function opened() {
    vi.useFakeTimers();
    const harness = controllable();
    const store = createLineOriginsStore(harness.source, 300);
    const stop = store.getState().watch();
    const first = store.getState().setTarget(WT_WHY_MAIN, HELPER);
    nextRequest(harness.requests, 0).done.resolve(fixture(WT_WHY_MAIN, HELPER));
    await first;
    return { ...harness, store, stop };
  }

  it("marks the answer stale at once, then reads again after the quiet window", async () => {
    const { store, requests, emitWorktree, stop } = await opened();
    emitWorktree({ worktreeId: WT_WHY_MAIN, paths: [HELPER] });
    expect(originsBehind(store.getState())).toBe(true);
    // The old answer is still there (no flicker) but flagged, and nothing was asked yet.
    expect(store.getState().origins.status).toBe("ok");
    expect(requests).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(300);
    expect(requests).toHaveLength(2);
    nextRequest(requests, 1).done.resolve(fixture(WT_WHY_MAIN, HELPER));
    await vi.advanceTimersByTimeAsync(0);
    expect(originsBehind(store.getState())).toBe(false);
    stop();
  });

  it("coalesces a burst of events into one read", async () => {
    const { requests, emitWorktree, emitSessions, stop } = await opened();
    for (let i = 0; i < 5; i++) emitWorktree({ worktreeId: WT_WHY_MAIN, paths: [HELPER] });
    emitSessions({ worktreeId: WT_WHY_MAIN });
    await vi.advanceTimersByTimeAsync(300);
    expect(requests).toHaveLength(2);
    stop();
  });

  it("an answer already in flight when the file changed does not clear stale", async () => {
    const { store, requests, emitWorktree, stop } = await opened();
    emitWorktree({ worktreeId: WT_WHY_MAIN, paths: [HELPER] });
    await vi.advanceTimersByTimeAsync(300); // read #2 starts
    // The file changes again while read #2 runs: its answer may predate that change.
    emitWorktree({ worktreeId: WT_WHY_MAIN, paths: [HELPER] });
    nextRequest(requests, 1).done.resolve(originsOf(WT_WHY_MAIN, HELPER, []));
    await vi.advanceTimersByTimeAsync(0);
    expect(originsBehind(store.getState())).toBe(true);
    // The trailing window then reads once more and settles it.
    await vi.advanceTimersByTimeAsync(300);
    expect(requests).toHaveLength(3);
    nextRequest(requests, 2).done.resolve(fixture(WT_WHY_MAIN, HELPER));
    await vi.advanceTimersByTimeAsync(0);
    expect(originsBehind(store.getState())).toBe(false);
    stop();
  });

  it("ignores other worktrees and other files, but an empty path list means everything", async () => {
    const { store, requests, emitWorktree, emitSessions, stop } = await opened();
    emitWorktree({ worktreeId: WT_WHY_AGENT, paths: [HELPER] });
    emitWorktree({ worktreeId: WT_WHY_MAIN, paths: ["src/unrelated.py"] });
    emitSessions({ worktreeId: WT_WHY_AGENT });
    await vi.advanceTimersByTimeAsync(1000);
    expect(requests).toHaveLength(1);
    expect(originsBehind(store.getState())).toBe(false);

    emitWorktree({ worktreeId: WT_WHY_MAIN, paths: [] });
    expect(originsBehind(store.getState())).toBe(true);
    stop();
  });

  it("a session event refreshes without marking the text stale", async () => {
    const { store, requests, emitSessions, stop } = await opened();
    emitSessions({ worktreeId: WT_WHY_MAIN });
    expect(originsBehind(store.getState())).toBe(false);
    await vi.advanceTimersByTimeAsync(300);
    expect(requests).toHaveLength(2);
    stop();
  });

  it("a failed background refresh keeps showing the last good answer", async () => {
    const { store, requests, emitSessions, stop } = await opened();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    emitSessions({ worktreeId: WT_WHY_MAIN });
    await vi.advanceTimersByTimeAsync(300);
    nextRequest(requests, 1).done.reject({ kind: "timeout" });
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getState().origins.status).toBe("ok");
    expect(warn).toHaveBeenCalled();
    stop();
  });

  it("of two reads of the same file, only the newer one lands, whatever the order", async () => {
    const { store, requests, emitSessions, stop } = await opened();
    emitSessions({ worktreeId: WT_WHY_MAIN });
    await vi.advanceTimersByTimeAsync(300); // background read #2 starts
    const newer = store.getState().load(); // a retry starts read #3 on the same file
    const fresh = fixture(WT_WHY_MAIN, HELPER);
    const old = originsOf(WT_WHY_MAIN, HELPER, []);
    nextRequest(requests, 2).done.resolve(fresh);
    await newer;
    nextRequest(requests, 1).done.resolve(old); // the older read answers last
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getState().origins).toEqual({ status: "ok", data: fresh });
    stop();
  });

  it("an answer in flight when the file changed is behind the file, whichever read finishes first", async () => {
    const { store, requests, emitWorktree, emitSessions, stop } = await opened();
    emitSessions({ worktreeId: WT_WHY_MAIN });
    await vi.advanceTimersByTimeAsync(300); // read #2 starts, covering revision 0
    emitWorktree({ worktreeId: WT_WHY_MAIN, paths: [HELPER] }); // the file changes: revision 1
    expect(store.getState().revision).toBe(1);
    nextRequest(requests, 1).done.resolve(fixture(WT_WHY_MAIN, HELPER));
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getState().originsRevision).toBe(0);
    expect(originsBehind(store.getState())).toBe(true);
    stop();
  });

  it("keeps a revision per file: an event for one file never moves another's", async () => {
    const { store, emitWorktree, stop } = await opened();
    emitWorktree({ worktreeId: WT_WHY_MAIN, paths: ["src/unrelated.py"] });
    expect(store.getState().revision).toBe(0);
    expect(store.getState().revisionOf(WT_WHY_MAIN, "src/unrelated.py")).toBe(1);
    // "Everything changed" moves every file of that worktree, and only that worktree.
    emitWorktree({ worktreeId: WT_WHY_MAIN, paths: [] });
    expect(store.getState().revision).toBe(1);
    expect(store.getState().revisionOf(WT_WHY_MAIN, "src/unrelated.py")).toBe(2);
    expect(store.getState().revisionOf(WT_WHY_AGENT, HELPER)).toBe(0);
    stop();
  });

  it("tags a diff with the revision it was read at, and retags on confirmation", async () => {
    const { store, stop } = await opened();
    const diff = {
      path: HELPER,
      oldPath: null,
      status: "modified",
      oldContent: "a\n",
      newContent: "b\n",
      binary: false,
      tooLarge: false,
      pathLossy: false,
    } as const;
    expect(diffReadRevision(diff)).toBeUndefined(); // an untagged diff is never trusted
    tagDiffRead(diff, 0);
    expect(diffReadRevision(diff)).toBe(0);
    const ticks = store.getState().diffTick;
    store.getState().confirmDiff(diff, 2);
    expect(diffReadRevision(diff)).toBe(2);
    expect(store.getState().diffTick).toBe(ticks + 1);
    stop();
  });

  it("says a re-read failed while the answer is behind, and clears it on a good read", async () => {
    const { store, requests, emitWorktree, stop } = await opened();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    emitWorktree({ worktreeId: WT_WHY_MAIN, paths: [HELPER] });
    await vi.advanceTimersByTimeAsync(300);
    nextRequest(requests, 1).done.reject({ kind: "timeout" });
    await vi.advanceTimersByTimeAsync(0);
    expect(warn).toHaveBeenCalled();
    expect(originsBehind(store.getState())).toBe(true);
    expect(store.getState().refreshError).toEqual({ kind: "timeout" });
    // The last good answer is still there, but nothing is promised: a retry is the way out.
    expect(store.getState().origins.status).toBe("ok");

    const retry = store.getState().load();
    nextRequest(requests, 2).done.resolve(fixture(WT_WHY_MAIN, HELPER));
    await retry;
    expect(store.getState().refreshError).toBeNull();
    expect(originsBehind(store.getState())).toBe(false);
    stop();
  });

  it("stops listening when the watcher is stopped", async () => {
    const { requests, emitWorktree, stop } = await opened();
    stop();
    emitWorktree({ worktreeId: WT_WHY_MAIN, paths: [HELPER] });
    await vi.advanceTimersByTimeAsync(1000);
    expect(requests).toHaveLength(1);
  });
});

describe("timelines are fetched lazily and cached per session", () => {
  it("reads nothing until asked, reads a session once, and refreshes on demand", async () => {
    const { source } = controllable();
    const store = createLineOriginsStore(source);
    expect(source.getSessionTimeline).not.toHaveBeenCalled();

    await store.getState().loadTimeline(SESSION_CLAUDE);
    await store.getState().loadTimeline(SESSION_CLAUDE);
    expect(source.getSessionTimeline).toHaveBeenCalledTimes(1);
    expect(store.getState().timelines.get(SESSION_CLAUDE)?.status).toBe("ok");

    await store.getState().loadTimeline(SESSION_CLAUDE, true);
    expect(source.getSessionTimeline).toHaveBeenCalledTimes(2);
  });

  it("keeps sessions apart and surfaces a failure as an error state", async () => {
    const { source } = controllable();
    const store = createLineOriginsStore(source);
    await store.getState().loadTimeline("no-such-session");
    expect(store.getState().timelines.get("no-such-session")?.status).toBe("error");
    expect(store.getState().timelines.get(SESSION_CLAUDE)).toBeUndefined();
  });
});
