import { describe, expect, it, vi } from "vitest";
import {
  changesByWorktree,
  fileDiffs,
  REPO_WHY,
  repositories,
  worktreeChangedAll,
  worktrees,
  WT_BILLING_DETACHED,
  WT_WHY_AGENT,
  WT_WHY_MAIN,
} from "@/fixtures";
import { LONG_FILE_PATH } from "@/dev-fixtures/long-file-origins";
import { SESSION_LONG } from "@/dev-fixtures/long-session";
import {
  SESSION_CLAUDE,
  SESSION_CODEX,
  SESSION_CODEX_QUIET,
  SESSION_SUBAGENT,
  sessionTimelines,
} from "@/fixtures/sessions";
import { FixtureDataSource, type FixtureOptions } from "./fixture-data-source";

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

  describe("git filter programs", () => {
    const filters = {
      why: [{ name: "evil", clean: "./x %f", scope: "local", origin: ".git/config" }],
    } satisfies FixtureOptions["filters"];

    it("blocks a repository with a local driver, like the core does", async () => {
      const source = new FixtureDataSource({ filters });
      const security = await source.getRepositorySecurity(REPO_WHY);
      expect(security.status).toBe("blocked");
      expect(security.drivers[0]).toMatchObject({ name: "evil", needsTrust: true, trusted: false });

      const list = await source.listWorktrees(REPO_WHY);
      expect(list.every((w) => w.loadError?.kind === "untrustedFilters")).toBe(true);
      await expect(source.getChanges(list[0]?.id ?? "", "uncommitted")).rejects.toMatchObject({
        kind: "untrustedFilters",
      });
    });

    it("trusts only the fingerprint it showed, and revoking blocks again", async () => {
      const source = new FixtureDataSource({ filters });
      await expect(source.trustRepositoryFilters(REPO_WHY, "stale")).rejects.toMatchObject({
        kind: "invalidInput",
      });
      const { fingerprint } = await source.getRepositorySecurity(REPO_WHY);
      expect((await source.trustRepositoryFilters(REPO_WHY, fingerprint)).status).toBe("trusted");
      const list = await source.listWorktrees(REPO_WHY);
      expect(list.every((w) => w.loadError === null)).toBe(true);
      expect((await source.revokeRepositoryFilters(REPO_WHY)).security?.status).toBe("blocked");
    });

    it("lets the user's own (global) drivers through as info", async () => {
      const source = new FixtureDataSource({
        filters: {
          why: [{ name: "lfs", clean: "git-lfs clean", scope: "global", origin: "~/.gitconfig" }],
        },
      });
      expect((await source.getRepositorySecurity(REPO_WHY)).status).toBe("info");
      await expect(source.listWorktrees(REPO_WHY)).resolves.toBeTruthy();
    });

    it("is clear without drivers", async () => {
      expect((await new FixtureDataSource().getRepositorySecurity(REPO_WHY)).status).toBe("clear");
    });
  });
});

describe("FixtureDataSource sessions", () => {
  it("lists a worktree's sessions, most recent first, and nothing for unknown worktrees", async () => {
    const source = new FixtureDataSource();
    const agent = await source.listSessions(WT_WHY_AGENT);
    expect(agent.map((session) => session.id)).toEqual([SESSION_CODEX, SESSION_CLAUDE]);
    expect((await source.listSessions(WT_WHY_MAIN)).map((s) => s.id)).toEqual([
      SESSION_CODEX_QUIET,
      SESSION_SUBAGENT,
    ]);
    await expect(source.listSessions("wt-nope")).resolves.toEqual([]);
  });

  it("covers both harnesses, a subagent, and failed and unknown outcomes", async () => {
    const source = new FixtureDataSource();
    const all = await Promise.all(
      [SESSION_CLAUDE, SESSION_CODEX, SESSION_SUBAGENT].map((id) => source.getSessionTimeline(id)),
    );
    expect(new Set(all.map((t) => t.session.harness))).toEqual(new Set(["claudeCode", "codex"]));
    const events = all.flatMap((t) => t.turns.flatMap((turn) => turn.events));
    expect(events.some((e) => e.agentId !== null)).toBe(true);
    expect(events.some((e) => e.outcome === "failed")).toBe(true);
    expect(events.some((e) => e.outcome === "unknown")).toBe(true);
    for (const timeline of all) {
      expect(timeline.turns.length).toBeGreaterThanOrEqual(3);
      expect(timeline.session.turnCount).toBe(timeline.turns.length);
      // Turns are numbered in order and events are in time order.
      timeline.turns.forEach((turn, index) => {
        expect(turn.index).toBe(index);
        const times = turn.events.map((e) => e.at);
        expect(times).toEqual([...times].sort());
      });
    }
  });

  it("serves a ~500-turn session for the virtualized timeline", async () => {
    const source = new FixtureDataSource();
    const [summary] = await source.listSessions(WT_BILLING_DETACHED);
    expect(summary?.id).toBe(SESSION_LONG);
    const long = await source.getSessionTimeline(SESSION_LONG);
    expect(long.turns).toHaveLength(500);
    expect(long.turns.at(-1)?.endedAt).toBeNull();
  });

  it("rejects an unknown session id with notFound", async () => {
    await expect(new FixtureDataSource().getSessionTimeline("codex:nope")).rejects.toEqual({
      kind: "notFound",
    });
  });

  it("returns copies, so callers cannot change the fixtures", async () => {
    const source = new FixtureDataSource();
    const timeline = await source.getSessionTimeline(SESSION_CLAUDE);
    timeline.turns.length = 0;
    expect((await source.getSessionTimeline(SESSION_CLAUDE)).turns).toHaveLength(3);
    expect(sessionTimelines[0]?.turns).toHaveLength(3);
  });

  it("announces simulated activity and unsubscribes cleanly", async () => {
    const source = new FixtureDataSource();
    const callback = vi.fn();
    const stop = source.onSessionsChanged(callback);
    expect(source.simulateSessionActivity(SESSION_CODEX, "pytest -k notes")).toBe(true);
    expect(callback).toHaveBeenCalledWith({ worktreeId: WT_WHY_AGENT });
    const turn = (await source.getSessionTimeline(SESSION_CODEX)).turns.at(-1);
    expect(turn?.events.at(-1)?.summary).toBe("pytest -k notes");
    stop();
    source.simulateSessionActivity(SESSION_CODEX);
    expect(callback).toHaveBeenCalledOnce();
    expect(source.simulateSessionActivity("codex:nope")).toBe(false);
  });

  it("keeps an ended session unchanged when activity is simulated", async () => {
    const source = new FixtureDataSource();
    const callback = vi.fn();
    const stop = source.onSessionsChanged(callback);
    const before = await source.getSessionTimeline(SESSION_CLAUDE);

    expect(source.simulateSessionActivity(SESSION_CLAUDE)).toBe(false);
    expect(await source.getSessionTimeline(SESSION_CLAUDE)).toEqual(before);
    expect(callback).not.toHaveBeenCalled();
    stop();
  });
});

describe("FixtureDataSource line origins (WHY-64)", () => {
  it("serves runs for the sample files and engineNotReady for everything else", async () => {
    const source = new FixtureDataSource();
    const calc = await source.getLineOrigins(WT_WHY_AGENT, "src/calc.py");
    expect(calc.status).toBe("ok");
    const helper = await source.getLineOrigins(WT_WHY_MAIN, "src/helper.py");
    expect(helper.status).toBe("ok");
    // The same path in another worktree is another file.
    expect(await source.getLineOrigins(WT_WHY_MAIN, "src/calc.py")).toEqual({
      status: "unavailable",
      reason: "engineNotReady",
    });
    expect(await source.getLineOrigins("no-such-worktree", "a.rs")).toEqual({
      status: "unavailable",
      reason: "engineNotReady",
    });
  });

  it("serves the 2,000-line file for gutter performance", async () => {
    const result = await new FixtureDataSource().getLineOrigins(WT_WHY_AGENT, LONG_FILE_PATH);
    if (result.status !== "ok") throw new Error("expected origins");
    expect(result.origins.lineCount).toBe(2000);
    expect(result.origins.runs.length).toBeGreaterThan(50);
  });

  it("rejects request paths the core would refuse, with invalidInput", async () => {
    const source = new FixtureDataSource();
    for (const path of ["", "/etc/passwd", "../x", "a/../../x", "./a.rs", ".", "a\0b"]) {
      await expect(source.getLineOrigins(WT_WHY_AGENT, path)).rejects.toMatchObject({
        kind: "invalidInput",
      });
    }
  });

  it("returns copies, so callers cannot change the fixtures", async () => {
    const source = new FixtureDataSource();
    const first = await source.getLineOrigins(WT_WHY_AGENT, "src/calc.py");
    if (first.status !== "ok") throw new Error("expected origins");
    first.origins.runs.length = 0;
    const again = await source.getLineOrigins(WT_WHY_AGENT, "src/calc.py");
    if (again.status !== "ok") throw new Error("expected origins");
    expect(again.origins.runs.length).toBeGreaterThan(0);
  });
});

describe("FixtureDataSource line origins edge cases (WHY-64)", () => {
  it("accepts what the core accepts on Unix and never answers with a prototype member", async () => {
    const source = new FixtureDataSource();
    const notReady = { status: "unavailable", reason: "engineNotReady" };
    for (const path of ["C:/x", "toString", "constructor", "__proto__", "a\\..\\b"]) {
      expect(await source.getLineOrigins(WT_WHY_AGENT, path), path).toEqual(notReady);
    }
    expect(await source.getLineOrigins("toString", "src/calc.py")).toEqual(notReady);
  });

  it("lists the samples as changes with matching diffs only when asked to", async () => {
    const plain = new FixtureDataSource();
    const plainPaths = (await plain.getChanges(WT_WHY_AGENT, "uncommitted")).map((c) => c.path);
    expect(plainPaths).not.toContain("src/calc.py");

    const source = new FixtureDataSource({ provenance: true });
    for (const [worktreeId, path] of [
      [WT_WHY_AGENT, "src/calc.py"],
      [WT_WHY_MAIN, "src/helper.py"],
      [WT_WHY_AGENT, LONG_FILE_PATH],
    ] as const) {
      const paths = (await source.getChanges(worktreeId, "uncommitted")).map((c) => c.path);
      expect(paths).toContain(path);
      const origins = await source.getLineOrigins(worktreeId, path);
      if (origins.status !== "ok") throw new Error(`expected origins for ${path}`);
      const diff = await source.getFileDiff(worktreeId, path, "uncommitted");
      const lines = (diff.newContent ?? "").split("\n").slice(0, -1);
      expect(lines.length, path).toBe(origins.origins.lineCount);
    }
  });
});
