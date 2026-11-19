/** The part of Monaco's diff editor the gate needs. */
export type DiffReadyEditor = {
  onDidUpdateDiff(listener: () => void): { dispose(): void };
  getLineChanges(): readonly unknown[] | null;
};

export type GateOptions = {
  /** Animation frames to wait after the diff arrives, so hidden regions are laid out. */
  frames?: number;
  /** Reveal anyway after this long (a broken worker must not hide the file forever). */
  timeoutMs?: number;
  requestFrame?: (callback: () => void) => unknown;
  cancelFrame?: (handle: never) => void;
};

/**
 * Calls `ready` once a diff editor has COMPUTED the diff of the models that were
 * just attached, and had a moment to collapse the unchanged regions.
 *
 * Why: attaching models shows them at once, fully expanded from line 1; Monaco
 * computes the diff on a worker a moment later and only then collapses the
 * unchanged lines, so the code visibly jumps. The caller keeps the editor out of
 * sight (behind the previous file) until this fires.
 *
 * Call it BEFORE attaching the models so no update can be missed. Only an update
 * that arrives after the call counts: `getLineChanges()` may still hold the
 * previous pair's result until then. Returns a function that cancels it.
 */
export function whenDiffReady(
  editor: DiffReadyEditor,
  ready: () => void,
  {
    frames = 2,
    timeoutMs = 1500,
    requestFrame = (callback) => requestAnimationFrame(callback),
    cancelFrame = (handle) => cancelAnimationFrame(handle as number),
  }: GateOptions = {},
) {
  let finished = false;
  let waiting = false;
  let frame: unknown;

  const finish = () => {
    if (finished) return;
    finished = true;
    subscription.dispose();
    clearTimeout(timer);
    ready();
  };

  const subscription = editor.onDidUpdateDiff(() => {
    if (finished || waiting || editor.getLineChanges() === null) return;
    waiting = true;
    let left = frames;
    const step = () => {
      if (finished) return;
      if (left-- <= 0) finish();
      else frame = requestFrame(step);
    };
    step();
  });
  const timer = setTimeout(finish, timeoutMs);

  return () => {
    if (finished) return;
    finished = true;
    subscription.dispose();
    clearTimeout(timer);
    if (frame !== undefined) cancelFrame(frame as never);
  };
}
