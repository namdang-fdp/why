import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  REPOSITORY_CHANGED_EVENT,
  WORKTREE_CHANGED_EVENT,
  type RepositoryChanged,
  type WorktreeChanged,
} from "@/contracts";
import { TauriDataSource } from "./tauri-data-source";

// `listen` is replaced so the test controls WHEN registration finishes, which is
// the whole point of the race being tested.
const listen = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/event", () => ({ listen }));

type Handler = (event: { payload: never }) => void;

/** A `listen` whose registration the test completes by hand. */
function deferredListen() {
  const unlisten = vi.fn();
  let handler: Handler = () => {};
  let finish: (stop: () => void) => void = () => {};
  listen.mockImplementation((_name: string, cb: Handler) => {
    handler = cb;
    return new Promise<() => void>((resolve) => {
      finish = resolve;
    });
  });
  return {
    unlisten,
    deliver: (payload: WorktreeChanged | RepositoryChanged) =>
      handler({ payload: payload as never }),
    register: async () => {
      finish(unlisten);
      await Promise.resolve(); // let the `.then` in onWorktreeChanged run
    },
  };
}

const event: WorktreeChanged = { worktreeId: "w1", paths: ["a.rs"] };

describe("TauriDataSource.onWorktreeChanged", () => {
  beforeEach(() => {
    listen.mockReset();
  });

  it("listens to the shared event name and hands the payload to the callback", async () => {
    const fake = deferredListen();
    const callback = vi.fn();
    new TauriDataSource().onWorktreeChanged(callback);
    await fake.register();

    expect(listen).toHaveBeenCalledWith(WORKTREE_CHANGED_EVENT, expect.any(Function));
    fake.deliver(event);
    expect(callback).toHaveBeenCalledExactlyOnceWith(event);
  });

  it("unsubscribing after registration stops delivery and releases the listener", async () => {
    const fake = deferredListen();
    const callback = vi.fn();
    const stop = new TauriDataSource().onWorktreeChanged(callback);
    await fake.register();

    stop();
    expect(fake.unlisten).toHaveBeenCalledTimes(1);
    fake.deliver(event);
    expect(callback).not.toHaveBeenCalled();
    stop(); // idempotent
    expect(fake.unlisten).toHaveBeenCalledTimes(1);
  });

  it("unsubscribing BEFORE registration finishes still releases the listener (no leak)", async () => {
    const fake = deferredListen();
    const callback = vi.fn();
    const stop = new TauriDataSource().onWorktreeChanged(callback);

    // React StrictMode: mount, unmount, mount; `listen` has not resolved yet.
    stop();
    expect(fake.unlisten).not.toHaveBeenCalled();
    fake.deliver(event); // an event sneaking in during the window is ignored
    expect(callback).not.toHaveBeenCalled();

    await fake.register();
    expect(fake.unlisten).toHaveBeenCalledTimes(1); // released as soon as it exists
    fake.deliver(event);
    expect(callback).not.toHaveBeenCalled();
  });

  it("survives a failed registration without throwing", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    listen.mockRejectedValue(new Error("no event system"));
    const stop = new TauriDataSource().onWorktreeChanged(vi.fn());
    await Promise.resolve();
    await Promise.resolve();
    expect(error).toHaveBeenCalled();
    expect(() => stop()).not.toThrow();
    error.mockRestore();
  });
});

describe("TauriDataSource.onRepositoryChanged", () => {
  beforeEach(() => {
    listen.mockReset();
  });

  it("listens to the repository event, not the worktree one, and delivers its payload", async () => {
    const fake = deferredListen();
    const callback = vi.fn();
    new TauriDataSource().onRepositoryChanged(callback);
    await fake.register();

    expect(listen).toHaveBeenCalledWith(REPOSITORY_CHANGED_EVENT, expect.any(Function));
    expect(REPOSITORY_CHANGED_EVENT).not.toBe(WORKTREE_CHANGED_EVENT);
    fake.deliver({ repoId: "r1" });
    expect(callback).toHaveBeenCalledExactlyOnceWith({ repoId: "r1" });
  });

  it("releases the listener even when unsubscribed before registration finishes", async () => {
    const fake = deferredListen();
    const callback = vi.fn();
    const stop = new TauriDataSource().onRepositoryChanged(callback);
    stop();
    await fake.register();
    expect(fake.unlisten).toHaveBeenCalledTimes(1);
    fake.deliver({ repoId: "r1" });
    expect(callback).not.toHaveBeenCalled();
  });
});
