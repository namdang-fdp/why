import { afterEach, describe, expect, it, vi } from "vitest";
import type {
  RationaleResult,
  ShellOutcomesResult,
  TurnRef,
  WorktreeAttribution,
} from "@/contracts";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { WT_WHY_AGENT, WT_WHY_MAIN } from "@/fixtures";
import { SESSION_CLAUDE, SESSION_CODEX } from "@/fixtures/sessions";
import { deferred } from "@/test/deferred";
import { createEvidenceStore, rationaleKey, type RationaleTarget } from "./evidence";

afterEach(() => vi.useRealTimers());

const turn = (sessionId: string, turnId: string): RationaleTarget => ({
  kind: "turn",
  sessionId,
  turnId,
});
const CLAUDE_T0 = "c3a1f0e2-0001";

describe("rationale is read only when a disclosure opens", () => {
  it("makes no call until opened, reads once for repeated opens, and refreshes on demand", async () => {
    const source = new FixtureDataSource();
    const read = vi.spyOn(source, "getTurnRationale");
    const store = createEvidenceStore(source);
    await store.getState().setSession({ id: SESSION_CLAUDE, harness: "claudeCode" });
    expect(read).not.toHaveBeenCalled();

    const target = turn(SESSION_CLAUDE, CLAUDE_T0);
    store.getState().toggleWhy(target);
    await vi.waitFor(() =>
      expect(store.getState().rationales.get(rationaleKey(target))?.status).toBe("ok"),
    );
    store.getState().toggleWhy(target); // close
    store.getState().toggleWhy(target); // open again: served from the cache
    await store.getState().loadRationale(target);
    expect(read).toHaveBeenCalledTimes(1);

    await store.getState().loadRationale(target, true);
    expect(read).toHaveBeenCalledTimes(2);
  });

  it("does not reopen a disclosure that was closed before its answer arrived", async () => {
    const source = new FixtureDataSource();
    const pending = deferred<RationaleResult>();
    vi.spyOn(source, "getTurnRationale").mockReturnValueOnce(pending.promise);
    const store = createEvidenceStore(source);
    await store.getState().setSession({ id: SESSION_CLAUDE, harness: "claudeCode" });
    const target = turn(SESSION_CLAUDE, CLAUDE_T0);

    store.getState().toggleWhy(target);
    expect(store.getState().whyOpen.has(rationaleKey(target))).toBe(true);
    store.getState().toggleWhy(target); // closed while the read is in flight
    pending.resolve({ status: "unavailable", reason: "missing" });
    await vi.waitFor(() =>
      expect(store.getState().rationales.get(rationaleKey(target))?.status).toBe("ok"),
    );
    // The answer is cached for next time but the disclosure stays closed.
    expect(store.getState().whyOpen.has(rationaleKey(target))).toBe(false);
  });

  it("drops an answer that arrives after the session changed", async () => {
    const source = new FixtureDataSource();
    const pending = deferred<RationaleResult>();
    vi.spyOn(source, "getTurnRationale").mockReturnValueOnce(pending.promise);
    const store = createEvidenceStore(source);
    await store.getState().setSession({ id: SESSION_CLAUDE, harness: "claudeCode" });
    const target = turn(SESSION_CLAUDE, CLAUDE_T0);
    store.getState().toggleWhy(target);

    await store.getState().setSession({ id: SESSION_CODEX, harness: "codex" });
    pending.resolve({ status: "unavailable", reason: "notFound" });
    await Promise.resolve();
    await Promise.resolve();
    expect(store.getState().rationales.size).toBe(0);
    expect(store.getState().whyOpen.size).toBe(0);
  });

  it("keeps an error as an error so reopening retries", async () => {
    const source = new FixtureDataSource();
    vi.spyOn(source, "getToolRationale")
      .mockRejectedValueOnce({ kind: "internal", message: "boom" })
      .mockResolvedValueOnce({ status: "unavailable", reason: "missing" });
    const store = createEvidenceStore(source);
    await store.getState().setSession({ id: SESSION_CLAUDE, harness: "claudeCode" });
    const target: RationaleTarget = { kind: "tool", sessionId: SESSION_CLAUDE, eventId: "c1-edit" };
    await store.getState().loadRationale(target);
    expect(store.getState().rationales.get(rationaleKey(target))?.status).toBe("error");
    await store.getState().loadRationale(target);
    expect(store.getState().rationales.get(rationaleKey(target))?.status).toBe("ok");
  });
});

describe("recovered shell outcomes", () => {
  it("reads once when a Codex session opens, never for Claude", async () => {
    const source = new FixtureDataSource();
    const read = vi.spyOn(source, "getShellOutcomes");
    const store = createEvidenceStore(source);

    await store.getState().setSession({ id: SESSION_CLAUDE, harness: "claudeCode" });
    expect(read).not.toHaveBeenCalled();
    expect(store.getState().outcomes.status).toBe("idle");

    await store.getState().setSession({ id: SESSION_CODEX, harness: "codex" });
    await store.getState().setSession({ id: SESSION_CODEX, harness: "codex" });
    expect(read).toHaveBeenCalledExactlyOnceWith(SESSION_CODEX);
    expect(store.getState().outcomes.status).toBe("ok");
  });

  it("drops outcomes that arrive after another session opened", async () => {
    const source = new FixtureDataSource();
    const pending = deferred<ShellOutcomesResult>();
    vi.spyOn(source, "getShellOutcomes").mockReturnValueOnce(pending.promise);
    const store = createEvidenceStore(source);

    const open = store.getState().setSession({ id: SESSION_CODEX, harness: "codex" });
    await store.getState().setSession({ id: SESSION_CLAUDE, harness: "claudeCode" });
    pending.resolve(await new FixtureDataSource().getShellOutcomes(SESSION_CODEX));
    await open;
    expect(store.getState().outcomes.status).toBe("idle");
  });

  it("coalesces a burst of 50 sessions-changed into a couple of refetches", async () => {
    vi.useFakeTimers();
    const source = new FixtureDataSource();
    const outcomes = vi.spyOn(source, "getShellOutcomes");
    const attribution = vi.spyOn(source, "getWorktreeAttribution");
    const store = createEvidenceStore(source);
    const stop = store.getState().watch();
    await store.getState().setScope(WT_WHY_AGENT, "vsBase");
    await store.getState().setSession({ id: SESSION_CODEX, harness: "codex" });
    outcomes.mockClear();
    attribution.mockClear();

    for (let i = 0; i < 50; i++) source.emitSessionsChanged({ worktreeId: WT_WHY_AGENT });
    source.emitSessionsChanged({ worktreeId: WT_WHY_MAIN }); // another worktree: ignored
    await vi.advanceTimersByTimeAsync(1000);
    expect(outcomes.mock.calls.length).toBeLessThanOrEqual(2);
    expect(attribution.mock.calls.length).toBeLessThanOrEqual(2);
    expect(outcomes).toHaveBeenCalled();
    stop();
  });

  it("does not poll: nothing is read without an event", async () => {
    vi.useFakeTimers();
    const source = new FixtureDataSource();
    const outcomes = vi.spyOn(source, "getShellOutcomes");
    const store = createEvidenceStore(source);
    const stop = store.getState().watch();
    await store.getState().setSession({ id: SESSION_CODEX, harness: "codex" });
    outcomes.mockClear();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(outcomes).not.toHaveBeenCalled();
    stop();
  });
});

describe("attribution and file activity", () => {
  it("never shows worktree A's answer after switching to B", async () => {
    const source = new FixtureDataSource();
    const slow = deferred<WorktreeAttribution>();
    const fast = await source.getWorktreeAttribution(WT_WHY_MAIN, "uncommitted");
    vi.spyOn(source, "getWorktreeAttribution").mockReturnValueOnce(slow.promise);
    const store = createEvidenceStore(source);

    const a = store.getState().setScope(WT_WHY_AGENT, "vsBase");
    await store.getState().setScope(WT_WHY_MAIN, "uncommitted");
    expect(store.getState().attribution).toMatchObject({
      status: "ok",
      data: { worktreeId: WT_WHY_MAIN },
    });
    slow.resolve(await new FixtureDataSource().getWorktreeAttribution(WT_WHY_AGENT, "vsBase"));
    await a;
    expect(store.getState().attribution).toEqual({ status: "ok", data: fast });
  });

  it("refetches on worktree-changed for the selected worktree only, coalesced", async () => {
    vi.useFakeTimers();
    const source = new FixtureDataSource();
    const read = vi.spyOn(source, "getWorktreeAttribution");
    const store = createEvidenceStore(source);
    const stop = store.getState().watch();
    await store.getState().setScope(WT_WHY_AGENT, "vsBase");
    read.mockClear();

    for (let i = 0; i < 20; i++)
      source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [] });
    source.emitWorktreeChanged({ worktreeId: WT_WHY_MAIN, paths: [] });
    await vi.advanceTimersByTimeAsync(1000);
    expect(read.mock.calls.length).toBeGreaterThanOrEqual(1);
    expect(read.mock.calls.length).toBeLessThanOrEqual(2);
    stop();
  });

  it("asks for both names of a renamed file and merges the answers newest first", async () => {
    const source = new FixtureDataSource();
    const read = vi.spyOn(source, "getFileActivity");
    const store = createEvidenceStore(source);
    await store.getState().setScope(WT_WHY_AGENT, "vsBase");
    const path = "src-tauri/src/git/parse.rs";
    const oldPath = "src-tauri/src/git/parser.rs";

    // Like the real core, the new name alone finds nothing: the call named the old one.
    await store.getState().setFile(path);
    expect(store.getState().activity).toEqual({ status: "ok", data: [] });

    await store.getState().setFile(path, oldPath);
    expect(read).toHaveBeenCalledWith(WT_WHY_AGENT, path);
    expect(read).toHaveBeenCalledWith(WT_WHY_AGENT, oldPath);
    const state = store.getState().activity;
    expect(state.status === "ok" && state.data.map((ref) => ref.path)).toEqual([oldPath]);
  });

  it("never looks up or opens a turn that has no id", async () => {
    const source = new FixtureDataSource();
    const read = vi.spyOn(source, "getTurnRationale");
    const store = createEvidenceStore(source);
    await store.getState().setSession({ id: SESSION_CLAUDE, harness: "claudeCode" });
    store.getState().toggleWhy({ kind: "turn", sessionId: SESSION_CLAUDE, turnId: "" });
    await store.getState().loadRationale({ kind: "turn", sessionId: SESSION_CLAUDE, turnId: "" });
    expect(read).not.toHaveBeenCalled();
    expect(store.getState().whyOpen.size).toBe(0);
  });

  it("drops a late file history after the file or worktree changed", async () => {
    const source = new FixtureDataSource();
    const late = deferred<TurnRef[]>();
    vi.spyOn(source, "getFileActivity").mockReturnValueOnce(late.promise);
    const store = createEvidenceStore(source);
    await store.getState().setScope(WT_WHY_AGENT, "vsBase");

    const first = store.getState().setFile("src-tauri/src/git/mod.rs");
    await store.getState().setFile("src-tauri/src/git/worktree.rs");
    late.resolve([]);
    await first;
    const state = store.getState();
    expect(state.activityPath).toBe("src-tauri/src/git/worktree.rs");
    expect(state.activity.status === "ok" && state.activity.data.map((r) => r.path)).toEqual([
      "src-tauri/src/git/worktree.rs",
    ]);

    await store.getState().setScope(WT_WHY_MAIN, "uncommitted");
    expect(store.getState().activityPath).toBeNull();
    expect(store.getState().activity.status).toBe("idle");
  });
});
