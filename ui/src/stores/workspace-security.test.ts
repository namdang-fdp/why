import { describe, expect, it, vi } from "vitest";
import { FixtureDataSource, sampleFilters } from "@/data/fixture-data-source";
import { REPO_WHY, WT_WHY_MAIN } from "@/fixtures";
import { createToastStore } from "./toasts";
import { createWorkspaceStore, selectChanges } from "./workspace";

async function setup(source: FixtureDataSource) {
  const toasts = createToastStore();
  const store = createWorkspaceStore(source, toasts);
  await store.getState().loadRepositories();
  return { store, toasts };
}

const statusOf = (store: Awaited<ReturnType<typeof setup>>["store"]) => {
  const entry = store.getState().security.get(REPO_WHY);
  return entry?.status === "ok" ? entry.data.status : entry?.status;
};

describe("git filter security in the store", () => {
  it("loads each repository's report when the list loads", async () => {
    const { store } = await setup(new FixtureDataSource({ filters: sampleFilters }));
    await vi.waitFor(() => expect(statusOf(store)).toBe("blocked"));
  });

  it("refetches the report, past the cache, when the core refuses a read", async () => {
    const source = new FixtureDataSource();
    const { store } = await setup(source);
    await vi.waitFor(() => expect(statusOf(store)).toBe("clear"));

    // A driver arrives unseen: the cached report is now wrong.
    source.setFilters("why", sampleFilters.why ?? []);
    const main = store.getState().worktrees.get(REPO_WHY);
    const worktree = main?.status === "ok" ? main.data.find((w) => w.id === WT_WHY_MAIN) : null;
    if (!worktree) throw new Error("fixture worktree missing");
    await store.getState().selectWorktree(worktree);

    await vi.waitFor(() => expect(statusOf(store)).toBe("blocked"));
  });

  it("a successful revoke drops what was read, even when the re-check failed", async () => {
    const source = new FixtureDataSource({ filters: sampleFilters });
    const { store, toasts } = await setup(source);
    const repo = { id: REPO_WHY, name: "why", rootPath: "/home/dev/code/why" };
    const report = await source.getRepositorySecurity(REPO_WHY);
    await store.getState().trustFilters(repo, report.fingerprint);
    expect(statusOf(store)).toBe("trusted");

    vi.spyOn(source, "revokeRepositoryFilters").mockImplementation(async () => {
      await FixtureDataSource.prototype.revokeRepositoryFilters.call(source, REPO_WHY);
      return { security: null, rescanError: { kind: "timeout" } };
    });
    const result = await store.getState().revokeFilters(repo);

    expect(result.status).toBe("ok");
    expect(toasts.getState().toasts.map((t) => t.title)).toContain(
      "Stopped trusting why, but could not re-check it",
    );
    // The stale "trusted" report did not survive: it was dropped and read again,
    // and what was read under the old decision (the changes) was read again too.
    await vi.waitFor(() => expect(statusOf(store)).toBe("blocked"));
    await vi.waitFor(() => {
      const changes = selectChanges(store.getState());
      expect(changes.status === "error" && changes.error.kind).toBe("untrustedFilters");
    });
  });
});
