import { describe, expect, it, vi } from "vitest";
import { whenDiffReady, type DiffReadyEditor } from "./ready-gate";

/** A diff editor whose worker answers when the test says so. */
function fakeEditor() {
  const listeners = new Set<() => void>();
  let changes: unknown[] | null = null;
  const editor: DiffReadyEditor = {
    onDidUpdateDiff(listener) {
      listeners.add(listener);
      return { dispose: () => listeners.delete(listener) };
    },
    getLineChanges: () => changes,
  };
  return {
    editor,
    listeners,
    /** The worker finished: `null` = an update that carries no result yet. */
    update(result: unknown[] | null) {
      changes = result;
      for (const listener of [...listeners]) listener();
    },
  };
}

/** Manual animation frames. */
function frames() {
  const queue: Array<() => void> = [];
  return {
    requestFrame: (callback: () => void) => queue.push(callback),
    cancelFrame: vi.fn(),
    tick() {
      queue.shift()?.();
    },
    pending: () => queue.length,
  };
}

describe("whenDiffReady", () => {
  it("does not fire before the diff is computed, then fires after the frames", () => {
    const { editor, update } = fakeEditor();
    const frame = frames();
    const ready = vi.fn();
    whenDiffReady(editor, ready, { frames: 2, ...frame });

    expect(ready).not.toHaveBeenCalled();
    update(null); // an update without a result is not "computed"
    expect(frame.pending()).toBe(0);
    update([]); // computed (a file whose sides are identical has no changes)
    expect(ready).not.toHaveBeenCalled();
    frame.tick();
    expect(ready).not.toHaveBeenCalled();
    frame.tick();
    expect(ready).toHaveBeenCalledTimes(1);
  });

  it("fires once however many updates follow", () => {
    const { editor, update, listeners } = fakeEditor();
    const frame = frames();
    const ready = vi.fn();
    whenDiffReady(editor, ready, { frames: 1, ...frame });
    update([{}]);
    update([{}, {}]);
    frame.tick();
    frame.tick();
    expect(ready).toHaveBeenCalledTimes(1);
    expect(listeners.size).toBe(0); // unsubscribed
  });

  it("ignores a stale result that was already there before it was armed", () => {
    const { editor, update } = fakeEditor();
    update([{}]); // the previous pair's diff is still in `getLineChanges()`
    const frame = frames();
    const ready = vi.fn();
    whenDiffReady(editor, ready, { frames: 0, ...frame });
    expect(ready).not.toHaveBeenCalled(); // no new update yet: not ready
    update([{}]);
    expect(ready).toHaveBeenCalledTimes(1);
  });

  it("can be cancelled, and then never fires", () => {
    const { editor, update, listeners } = fakeEditor();
    const frame = frames();
    const ready = vi.fn();
    const cancel = whenDiffReady(editor, ready, { frames: 2, ...frame });
    update([]);
    cancel();
    frame.tick();
    frame.tick();
    update([]);
    expect(ready).not.toHaveBeenCalled();
    expect(listeners.size).toBe(0);
  });

  it("reveals anyway after the timeout if the worker never answers", () => {
    vi.useFakeTimers();
    try {
      const { editor } = fakeEditor();
      const ready = vi.fn();
      whenDiffReady(editor, ready, { timeoutMs: 1500, ...frames() });
      vi.advanceTimersByTime(1499);
      expect(ready).not.toHaveBeenCalled();
      vi.advanceTimersByTime(2);
      expect(ready).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
