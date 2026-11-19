import { describe, expect, it, vi } from "vitest";
import type { FileDiff } from "@/contracts";
import { createBufferedDiff, RETIRE_MS, type DiffSlot, type DiffTarget } from "./buffered-diff";

const diff = (path: string, newContent = "new"): FileDiff => ({
  path,
  oldPath: null,
  status: "modified",
  oldContent: "old",
  newContent,
  binary: false,
  tooLarge: false,
  pathLossy: false,
});
const target = (path: string, newContent?: string): DiffTarget => ({
  worktreeId: "w",
  mode: "vsBase",
  diff: diff(path, newContent),
});

/** A slot whose "diff computed" moment the test controls. */
function fakeSlot(name: string) {
  let current: DiffTarget | undefined;
  let ready: (() => void) | undefined;
  const slot: DiffSlot & { name: string; finish(): void; shown(): string | undefined } = {
    name,
    isSameFile: (next) =>
      current?.diff.path === next.diff.path && current.worktreeId === next.worktreeId,
    show: vi.fn((next, onReady) => {
      const same = current?.diff.path === next.diff.path;
      current = next;
      if (!same) ready = onReady;
    }),
    clear: vi.fn(() => {
      current = undefined;
      ready = undefined;
    }),
    finish: () => ready?.(),
    shown: () => current?.diff.path,
  };
  return slot;
}

function setup() {
  const a = fakeSlot("A");
  const b = fakeSlot("B");
  const timers: Array<() => void> = [];
  const hooks = {
    onFront: vi.fn(),
    onEmpty: vi.fn(),
    later: (action: () => void) => timers.push(action),
  };
  const viewer = createBufferedDiff([a, b], hooks);
  const runTimers = () => timers.splice(0).forEach((action) => action());
  return { a, b, viewer, hooks, runTimers };
}

describe("createBufferedDiff", () => {
  it("keeps the old file in front until the new one is ready, then swaps once", () => {
    const { a, b, viewer, hooks } = setup();
    viewer.show(target("one.rs"));
    // First file: built in the back slot (B), nothing is in front yet.
    expect(b.shown()).toBe("one.rs");
    expect(hooks.onFront).not.toHaveBeenCalled();
    b.finish();
    expect(hooks.onFront).toHaveBeenLastCalledWith(1, expect.anything(), true);

    viewer.show(target("two.rs"));
    // Building two.rs in A while one.rs (B) stays the front.
    expect(a.shown()).toBe("two.rs");
    expect(viewer.front).toBe(1);
    expect(hooks.onFront).toHaveBeenCalledTimes(1);
    a.finish();
    expect(viewer.front).toBe(0);
    expect(hooks.onFront).toHaveBeenCalledTimes(2);
    expect(hooks.onFront).toHaveBeenLastCalledWith(0, expect.objectContaining({}), true);
  });

  it("frees the retired editor's models after the cross-fade", () => {
    const { a, b, viewer, runTimers } = setup();
    viewer.show(target("one.rs"));
    b.finish();
    viewer.show(target("two.rs"));
    a.finish();
    expect(b.clear).not.toHaveBeenCalled(); // still under the fading-in front
    runTimers();
    expect(b.clear).toHaveBeenCalledTimes(1);
    expect(RETIRE_MS).toBeGreaterThan(80); // longer than the fade
  });

  it("builds only the last of several quick switches", () => {
    const { a, b, viewer, hooks } = setup();
    viewer.show(target("one.rs"));
    b.finish();
    viewer.show(target("two.rs")); // A starts building two.rs
    viewer.show(target("three.rs")); // supersedes it, same slot
    expect(a.shown()).toBe("three.rs");
    a.finish();
    expect(hooks.onFront).toHaveBeenCalledTimes(2);
    expect(hooks.onFront).toHaveBeenLastCalledWith(
      0,
      expect.objectContaining({ diff: expect.objectContaining({ path: "three.rs" }) }),
      true,
    );
  });

  it("a live edit of the file in front is applied in place without a swap", () => {
    const { a, b, viewer, hooks } = setup();
    viewer.show(target("one.rs"));
    b.finish();
    viewer.show(target("one.rs", "edited"));
    expect(viewer.front).toBe(1);
    expect(a.show).not.toHaveBeenCalled(); // the other editor is not touched
    expect(hooks.onFront).toHaveBeenLastCalledWith(1, expect.anything(), false);
  });

  it("going back to the front file cancels a switch that was still being prepared", () => {
    const { a, b, viewer, hooks } = setup();
    viewer.show(target("one.rs"));
    b.finish();
    viewer.show(target("two.rs")); // A preparing
    viewer.show(target("one.rs")); // changed their mind: same as front
    a.finish(); // the abandoned build completes
    expect(viewer.front).toBe(1);
    expect(hooks.onFront).toHaveBeenLastCalledWith(1, expect.anything(), false);
  });

  it("does not clear a retired editor that is already preparing the next file", () => {
    const { a, b, viewer, runTimers } = setup();
    viewer.show(target("one.rs"));
    b.finish();
    viewer.show(target("two.rs"));
    a.finish(); // B retires (clear is scheduled)
    viewer.show(target("three.rs")); // B is reused at once
    runTimers();
    expect(b.clear).not.toHaveBeenCalled();
    expect(b.shown()).toBe("three.rs");
  });

  it("clear() empties both editors and cancels pending work", () => {
    const { a, b, viewer, hooks } = setup();
    viewer.show(target("one.rs"));
    b.finish();
    viewer.show(target("two.rs"));
    viewer.clear();
    a.finish();
    expect(hooks.onEmpty).toHaveBeenCalledTimes(1);
    expect(a.clear).toHaveBeenCalled();
    expect(b.clear).toHaveBeenCalled();
    expect(hooks.onFront).toHaveBeenCalledTimes(1); // only the first swap
  });
});
