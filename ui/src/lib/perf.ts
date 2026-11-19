/**
 * Dev-only timing of "I asked for a file" -> "its diff is on screen".
 *
 * `begin` is called when a file is selected, `finish` when the editor reveals it.
 * Each pair becomes a `performance.measure` (visible in the browser's
 * Performance panel) and a row in `window.whyPerf.switches`. All of it is a
 * no-op outside `vite dev`: `import.meta.env.DEV` is a constant the bundler
 * folds away, so production builds carry none of this.
 */

export type SwitchTiming = { path: string; cached: boolean; ms: number };

type PerfApi = {
  switches: SwitchTiming[];
  /** Resolves with the next completed switch (used by the dev benchmark). */
  next(timeoutMs?: number): Promise<SwitchTiming | undefined>;
};

declare global {
  interface Window {
    whyPerf?: PerfApi;
  }
}

const started = new Map<string, { at: number; cached: boolean }>();
const waiters = new Set<(timing: SwitchTiming) => void>();

/** The timing API (created on first use); `undefined` outside `vite dev`. */
export function devPerf(): PerfApi | undefined {
  if (!import.meta.env.DEV || typeof window === "undefined") return undefined;
  window.whyPerf ??= {
    switches: [],
    next: (timeoutMs = 5000) =>
      new Promise((resolve) => {
        const timer = setTimeout(() => {
          waiters.delete(done);
          resolve(undefined);
        }, timeoutMs);
        const done = (timing: SwitchTiming) => {
          clearTimeout(timer);
          resolve(timing);
        };
        waiters.add(done);
      }),
  };
  return window.whyPerf;
}

export function beginFileSwitch(path: string, cached: boolean) {
  if (!devPerf()) return;
  started.set(path, { at: performance.now(), cached });
  performance.mark(`file-switch:${path}`);
}

export function finishFileSwitch(path: string) {
  const perf = devPerf();
  const begin = started.get(path);
  if (!perf || !begin) return;
  started.delete(path);
  const ms = performance.now() - begin.at;
  performance.measure(`file-switch ${begin.cached ? "cached" : "uncached"}: ${path}`, {
    start: begin.at,
    end: begin.at + ms,
  });
  const timing = { path, cached: begin.cached, ms: Math.round(ms * 10) / 10 };
  perf.switches.push(timing);
  for (const waiter of waiters) waiter(timing);
  waiters.clear();
}
