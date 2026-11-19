import type { DiffMode, FileDiff } from "@/contracts";

export type DiffTarget = { worktreeId: string; mode: DiffMode; diff: FileDiff };

/** One of the two diff editors the viewer alternates between. */
export interface DiffSlot {
  /** Is `target` this slot's current file (so a change to it is a live edit)? */
  isSameFile(target: DiffTarget): boolean;
  /**
   * Make this slot show `target`. A different file gets new models, and `ready`
   * fires once its diff is computed and laid out; the same file is rewritten in
   * place and `ready` is NOT called.
   */
  show(target: DiffTarget, ready: () => void): void;
  clear(): void;
}

export type BufferedDiffHooks = {
  /** `slot` now shows `target` in front. `swapped` is false for an in-place live update. */
  onFront(slot: 0 | 1, target: DiffTarget, swapped: boolean): void;
  /** Nothing to show any more (a binary file, no selection). */
  onEmpty(): void;
  /** Run `action` after `ms` (injected so tests need no real timers). */
  later(action: () => void, ms: number): void;
};

/** How long the retired editor stays populated, covering the cross-fade. */
export const RETIRE_MS = 160;

/**
 * Double buffering for the diff editor: the file on screen stays on screen
 * while the next one is prepared in the other editor, and the two swap only
 * when the new one is ready. Nobody ever sees a half-built diff.
 *
 * ```text
 *   show(B) while A is front:   A [front, visible]      B [back, loading models]
 *   B's diff is ready:          A [back, retired]       B [front, fades in]
 * ```
 *
 * A newer `show` supersedes one still waiting (rapid j j j builds only the last
 * file). The same file as the front one is updated in place, never swapped.
 */
export function createBufferedDiff(slots: readonly [DiffSlot, DiffSlot], hooks: BufferedDiffHooks) {
  let front: 0 | 1 = 0;
  let hasFront = false;
  let pending: { slot: 0 | 1; token: number } | undefined;
  let token = 0;

  const other = (slot: 0 | 1): 0 | 1 => (slot === 0 ? 1 : 0);

  return {
    get front() {
      return front;
    },
    show(target: DiffTarget) {
      const current = slots[front];
      if (hasFront && current.isSameFile(target)) {
        // The file on screen changed on disk (or nothing changed): no swap, and
        // any switch still waiting is no longer wanted.
        token++;
        pending = undefined;
        current.show(target, () => {});
        hooks.onFront(front, target, false);
        return;
      }

      const mine = ++token;
      const back = other(front);
      pending = { slot: back, token: mine };
      slots[back].show(target, () => {
        if (token !== mine) return; // superseded while computing
        const retired = front;
        front = back;
        hasFront = true;
        pending = undefined;
        hooks.onFront(front, target, true);
        hooks.later(() => {
          // Free the old models, unless the old slot is already preparing the next file.
          if (front !== retired && pending?.slot !== retired) slots[retired].clear();
        }, RETIRE_MS);
      });
    },
    clear() {
      token++;
      pending = undefined;
      hasFront = false;
      slots[0].clear();
      slots[1].clear();
      hooks.onEmpty();
    },
  };
}
