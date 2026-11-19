import { describe, expect, it, vi } from "vitest";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { REPO_BILLING, REPO_WHY, repositories, storageTemporary } from "@/fixtures";
import { createToastStore } from "./toasts";
import { createWorkspaceStore, selectChanges, selectDiffMode, selectWorktree } from "./workspace";

function at<T>(list: readonly T[], index: number): T {
  const item = list[index];
  if (item === undefined) throw new Error(`no item at ${index}`);
  return item;
}

async function setup(source = new FixtureDataSource()) {
  const toasts = createToastStore();
  const store = createWorkspaceStore(source, toasts);
  await store.getState().loadRepositories();
  return { source, store, toasts };
}

describe("adding repositories", () => {
  it("adds, selects the new repository and says so", async () => {
    const { store, toasts } = await setup();
    const result = await store.getState().addRepository("/home/dev/code/zeta");

    expect(result.status).toBe("ok");
    const state = store.getState();
    expect(
      state.repositories.status === "ok" && state.repositories.data.map((r) => r.name),
    ).toEqual(["billing-api", "why", "zeta"]);
    expect(state.repoId).toBe(result.status === "ok" ? result.data.id : null);
    expect(selectWorktree(state)?.path).toBe("/home/dev/code/zeta");
    expect(selectChanges(state).status).toBe("ok");
    expect(toasts.getState().toasts).toMatchObject([{ tone: "info", title: "Added zeta" }]);
  });

  it.each([
    ["/x/not-a-repo", "notARepo"],
    ["/x/no-git", "gitMissing"],
    ["/x/bare", "invalidInput"],
    ["/x/missing", "invalidInput"],
    ["   ", "invalidInput"],
  ])("returns the error for %j without touching the list", async (path, kind) => {
    const { store, toasts } = await setup();
    const before = store.getState().repositories;
    const result = await store.getState().addRepository(path);
    expect(result).toMatchObject({ status: "error", error: { kind } });
    expect(store.getState().repositories).toBe(before);
    expect(store.getState().repoId).toBe(REPO_WHY); // selection unchanged
    expect(toasts.getState().toasts).toEqual([]); // the dialog shows it inline
  });

  it("adding an existing repository again selects it and adds nothing", async () => {
    const { store } = await setup();
    const existing = at(repositories, 1);
    const result = await store.getState().addRepository(existing.rootPath);
    expect(result).toMatchObject({ status: "ok", data: { id: existing.id } });
    expect(store.getState().repoId).toBe(existing.id);
    const list = store.getState().repositories;
    expect(list.status === "ok" && list.data).toHaveLength(2);
  });

  it("still shows the new repository if re-reading the list fails", async () => {
    const { source, store } = await setup();
    vi.spyOn(source, "listRepositories").mockRejectedValueOnce({ kind: "timeout" });
    await store.getState().addRepository("/home/dev/code/zeta");
    const list = store.getState().repositories;
    expect(list.status === "ok" && list.data.map((r) => r.name)).toEqual([
      "why",
      "billing-api",
      "zeta",
    ]);
  });

  it("passes the picker's answer through, including cancel and errors", async () => {
    const { store } = await setup(new FixtureDataSource({ pick: "/p" }));
    await expect(store.getState().pickRepositoryFolder()).resolves.toEqual({
      status: "ok",
      data: "/p",
    });
    const cancelled = await setup(new FixtureDataSource({ pick: null }));
    await expect(cancelled.store.getState().pickRepositoryFolder()).resolves.toEqual({
      status: "ok",
      data: null,
    });
  });
});

describe("removing repositories", () => {
  it("removes the active repository, moves to the next and reports that disk is untouched", async () => {
    const { store, toasts } = await setup();
    expect(store.getState().repoId).toBe(REPO_WHY);
    const why = at(repositories, 0);

    await store.getState().removeRepository(why);

    const state = store.getState();
    expect(state.repositories.status === "ok" && state.repositories.data.map((r) => r.id)).toEqual([
      REPO_BILLING,
    ]);
    expect(state.repoId).toBe(REPO_BILLING);
    expect(selectWorktree(state)?.repoId).toBe(REPO_BILLING);
    expect(state.worktrees.has(REPO_WHY)).toBe(false);
    expect(toasts.getState().toasts).toMatchObject([
      { tone: "info", title: "Removed why", detail: "Nothing on disk was changed." },
    ]);
  });

  it("removing the last repository empties the workspace (onboarding)", async () => {
    const { store } = await setup();
    for (const repo of repositories) await store.getState().removeRepository(repo);
    const state = store.getState();
    expect(state.repositories).toEqual({ status: "ok", data: [] });
    expect([state.repoId, state.worktreeId, state.filePath]).toEqual([null, null, null]);
  });

  it("keeps everything and shows an error toast when removal fails", async () => {
    const { source, store, toasts } = await setup();
    vi.spyOn(source, "removeRepository").mockRejectedValueOnce({
      kind: "internal",
      message: "database is locked",
    });
    const result = await store.getState().removeRepository(at(repositories, 0));
    expect(result.status).toBe("error");
    expect(store.getState().repoId).toBe(REPO_WHY);
    expect(toasts.getState().toasts).toMatchObject([
      { tone: "error", title: "Could not remove why" },
    ]);
    expect(toasts.getState().toasts[0]?.detail).toContain("database is locked");
  });
});

describe("base override", () => {
  it("sets a base, refreshes the worktree and its list, and clears it again", async () => {
    const { store, toasts } = await setup();
    const main = store.getState().worktrees.get(REPO_WHY);
    const worktree = main?.status === "ok" ? main.data.find((w) => w.isMain) : undefined;
    expect(worktree?.base).toBeNull();
    if (!worktree) return;
    await store.getState().selectWorktree(worktree);
    expect(selectDiffMode(store.getState())).toBe("uncommitted");

    const set = await store.getState().setBase(worktree, "origin/main");
    expect(set.status).toBe("ok");
    const updated = selectWorktree(store.getState());
    expect(updated?.base?.baseRef).toBe("origin/main");
    expect(selectDiffMode(store.getState())).toBe("vsBase"); // now there is a base to compare with
    expect(toasts.getState().toasts.at(-1)?.title).toBe("Base set to origin/main");

    await store.getState().setBase(updated ?? worktree, "");
    expect(selectWorktree(store.getState())?.base).toBeNull();
    expect(selectDiffMode(store.getState())).toBe("uncommitted");
    expect(toasts.getState().toasts.at(-1)?.title).toBe("Using the detected base");
  });

  it("returns the validation error and changes nothing", async () => {
    const { store, toasts } = await setup();
    const worktree = selectWorktree(store.getState());
    if (!worktree) throw new Error("no worktree selected");
    const result = await store.getState().setBase(worktree, "nope");
    expect(result).toMatchObject({
      status: "error",
      error: { kind: "invalidInput", message: expect.stringContaining("nope") },
    });
    expect(selectWorktree(store.getState())).toBe(worktree);
    expect(toasts.getState().toasts).toEqual([]);
  });
});

describe("storage status", () => {
  it("loads once and keeps the reason for the banner", async () => {
    const source = new FixtureDataSource({ storage: storageTemporary });
    const read = vi.spyOn(source, "getStorageStatus");
    const { store } = await setup(source);
    await store.getState().loadStorageStatus();
    await store.getState().loadStorageStatus();
    expect(read).toHaveBeenCalledTimes(1);
    expect(store.getState().storage).toEqual({ status: "ok", data: storageTemporary });
  });
});
