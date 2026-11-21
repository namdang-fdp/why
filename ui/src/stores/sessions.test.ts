import { afterEach, describe, expect, it, vi } from "vitest";
import type { SessionSummary, SessionTimeline } from "@/contracts";
import { deferred } from "@/test/deferred";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { WT_WHY_AGENT, WT_WHY_MAIN } from "@/fixtures";
import { SESSION_CLAUDE, SESSION_CODEX } from "@/fixtures/sessions";
import { createSessionsStore } from "./sessions";

afterEach(() => vi.useRealTimers());

describe("session requests", () => {
  it("drops A's late list after switching to B, and versions repeat reads of A", async () => {
    const source = new FixtureDataSource();
    const read = source.listSessions.bind(source);
    const originalA = await read(WT_WHY_AGENT);
    const originalB = await read(WT_WHY_MAIN);
    const first = deferred<SessionSummary[]>();
    const second = deferred<SessionSummary[]>();
    vi.spyOn(source, "listSessions")
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => second.promise);

    const store = createSessionsStore(source);
    const a = store.getState().selectWorktree(WT_WHY_AGENT);
    const b = store.getState().selectWorktree(WT_WHY_MAIN);
    expect(store.getState().list).toEqual({ status: "loading" });
    second.resolve(originalB);
    await b;
    first.resolve(originalA);
    await a;
    expect(store.getState()).toMatchObject({
      worktreeId: WT_WHY_MAIN,
      list: { status: "ok", data: originalB },
    });

    const stale = deferred<SessionSummary[]>();
    vi.mocked(source.listSessions).mockImplementationOnce(() => stale.promise);
    const backToA = store.getState().selectWorktree(WT_WHY_AGENT);
    await store.getState().loadList();
    stale.resolve([]);
    await backToA;
    expect(store.getState().list).toEqual({ status: "ok", data: originalA });
  });

  it("drops stale timelines on session and worktree switches", async () => {
    const source = new FixtureDataSource();
    const claude = await source.getSessionTimeline(SESSION_CLAUDE);
    const codex = await source.getSessionTimeline(SESSION_CODEX);
    const first = deferred<SessionTimeline>();
    const pending = deferred<SessionTimeline>();
    vi.spyOn(source, "getSessionTimeline").mockImplementationOnce(() => first.promise);
    const store = createSessionsStore(source);

    await store.getState().selectWorktree(WT_WHY_AGENT);
    const a = store.getState().selectSession(SESSION_CLAUDE);
    await store.getState().selectSession(SESSION_CODEX);
    first.resolve(claude);
    await a;
    expect(store.getState()).toMatchObject({
      sessionId: SESSION_CODEX,
      timeline: { status: "ok", data: codex },
    });

    vi.mocked(source.getSessionTimeline).mockImplementationOnce(() => pending.promise);
    const again = store.getState().selectSession(SESSION_CLAUDE);
    await store.getState().selectWorktree(WT_WHY_MAIN);
    pending.resolve(claude);
    await again;
    expect(store.getState()).toMatchObject({ sessionId: null, timeline: { status: "idle" } });
  });
});

describe("live sessions", () => {
  it("ignores other worktrees, coalesces 50 events and retains the selected turn", async () => {
    vi.useFakeTimers();
    const source = new FixtureDataSource();
    const list = vi.spyOn(source, "listSessions");
    const timeline = vi.spyOn(source, "getSessionTimeline");
    const store = createSessionsStore(source);
    const stop = store.getState().watch();
    await store.getState().selectWorktree(WT_WHY_AGENT);
    await store.getState().selectSession(SESSION_CODEX);
    const snapshot = store.getState().timeline;
    if (snapshot.status !== "ok") throw new Error("Expected the Codex fixture");
    const turnId = snapshot.data.turns[0]?.id ?? "";
    store.getState().selectTurn(turnId);
    list.mockClear();
    timeline.mockClear();

    source.emitSessionsChanged({ worktreeId: WT_WHY_MAIN });
    await vi.advanceTimersByTimeAsync(50);
    expect(list).not.toHaveBeenCalled();
    expect(timeline).not.toHaveBeenCalled();
    for (let index = 0; index < 50; index++) source.simulateSessionActivity(SESSION_CODEX);
    await vi.advanceTimersByTimeAsync(299);
    expect(list).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(list).toHaveBeenCalledExactlyOnceWith(WT_WHY_AGENT);
    expect(timeline).toHaveBeenCalledExactlyOnceWith(SESSION_CODEX);
    const updated = store.getState();
    expect(updated.turnId).toBe(turnId);
    expect(
      updated.timeline.status === "ok" && updated.timeline.data.turns.at(-1)?.events,
      // The fixture turn already has 4 events (prompt, test run and two more shell calls) before the 50.
    ).toHaveLength(54);

    stop();
    source.emitSessionsChanged({ worktreeId: WT_WHY_AGENT });
    await vi.advanceTimersByTimeAsync(300);
    expect(list).toHaveBeenCalledTimes(1);
  });

  it("keeps unchanged row references and applies a late outcome in place", async () => {
    vi.useFakeTimers();
    const source = new FixtureDataSource();
    const store = createSessionsStore(source, 20);
    const stop = store.getState().watch();
    await store.getState().selectWorktree(WT_WHY_AGENT);
    await store.getState().selectSession(SESSION_CODEX);
    const before = store.getState().timeline;
    if (before.status !== "ok") throw new Error("Expected the Codex fixture");
    const result = structuredClone(before.data);
    const event = result.turns.at(-1)?.events.find(({ outcome }) => outcome === "unknown");
    if (!event) throw new Error("Expected an unknown outcome");
    event.outcome = "ok";
    vi.spyOn(source, "getSessionTimeline").mockResolvedValueOnce(result);

    source.emitSessionsChanged({ worktreeId: WT_WHY_AGENT });
    await vi.advanceTimersByTimeAsync(20);
    const after = store.getState().timeline;
    if (after.status !== "ok") throw new Error("Expected the refreshed timeline");
    expect(after.data.turns[0]).toBe(before.data.turns[0]);
    expect(after.data.turns[1]).toBe(before.data.turns[1]);
    expect(after.data.turns.at(-1)).not.toBe(before.data.turns.at(-1));
    expect(after.data.turns.at(-1)?.events).toContainEqual(event);
    stop();
  });

  it("queues one trailing refresh while reading and ignores an old worktree's pending round", async () => {
    vi.useFakeTimers();
    const source = new FixtureDataSource();
    const list = vi.spyOn(source, "listSessions");
    const timeline = vi.spyOn(source, "getSessionTimeline");
    const store = createSessionsStore(source, 20);
    const stop = store.getState().watch();
    await store.getState().selectWorktree(WT_WHY_AGENT);
    await store.getState().selectSession(SESSION_CODEX);
    const original = await source.listSessions(WT_WHY_AGENT);
    list.mockClear();
    timeline.mockClear();

    const slow = deferred<SessionSummary[]>();
    list.mockImplementationOnce(() => slow.promise);
    source.emitSessionsChanged({ worktreeId: WT_WHY_AGENT });
    await vi.advanceTimersByTimeAsync(20);
    for (let index = 0; index < 50; index++) {
      source.emitSessionsChanged({ worktreeId: WT_WHY_AGENT });
    }
    await vi.advanceTimersByTimeAsync(50);
    expect(list).toHaveBeenCalledTimes(1);
    slow.resolve(original);
    await vi.advanceTimersByTimeAsync(50);
    expect(list).toHaveBeenCalledTimes(2);
    expect(timeline).toHaveBeenCalledTimes(2);

    const obsolete = deferred<SessionSummary[]>();
    list.mockImplementationOnce(() => obsolete.promise);
    source.emitSessionsChanged({ worktreeId: WT_WHY_AGENT });
    await vi.advanceTimersByTimeAsync(20);
    await store.getState().selectWorktree(WT_WHY_MAIN);
    obsolete.resolve(original);
    await vi.advanceTimersByTimeAsync(50);
    expect(timeline).toHaveBeenCalledTimes(2);
    expect(store.getState()).toMatchObject({ worktreeId: WT_WHY_MAIN, sessionId: null });
    stop();
  });
});
