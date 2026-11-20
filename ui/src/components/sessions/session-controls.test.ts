import { describe, expect, it, vi } from "vitest";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { buildLongSession, SESSION_LONG } from "@/dev-fixtures/long-session";
import { WT_BILLING_DETACHED, worktrees } from "@/fixtures";
import { createWorkspaceStore } from "@/stores/workspace";
import { moveTurn } from "./session-controls";

async function setup() {
  const fixture = new FixtureDataSource();
  const store = createWorkspaceStore(fixture);
  const worktree = worktrees.find(({ id }) => id === WT_BILLING_DETACHED);
  if (!worktree) throw new Error("Expected the detached billing fixture");
  vi.spyOn(fixture, "getSessionTimeline").mockResolvedValue(buildLongSession(worktree.id, 7));
  const workspace = store.getState();
  const sessions = workspace.sessions.getState();
  await workspace.loadRepositories();
  await workspace.selectWorktree(worktree);
  await workspace.selectFile("src/billing.ts");
  await sessions.selectWorktree(worktree.id);
  await sessions.selectSession(SESSION_LONG);
  return { store, getSession: workspace.sessions.getState };
}

describe("navigation between file matches", () => {
  it.each([
    ["fresh n selects the first match", null, 1, 0],
    ["fresh p selects the last match", null, -1, 6],
    ["n uses a non-matching turn's full timeline position", 3, 1, 4],
    ["p uses a non-matching turn's full timeline position", 3, -1, 2],
    ["n wraps after the last match", 6, 1, 0],
    ["p wraps before the first match", 0, -1, 6],
  ] as const)("%s", async (_label, initial, direction, expected) => {
    const { store, getSession } = await setup();
    if (initial !== null) getSession().selectTurn(`long-turn-${initial}`);
    moveTurn(store, direction, true);
    expect(getSession().turnId).toBe(`long-turn-${expected}`);
    expect(getSession().turnFocus).toBe(1);
  });
});
