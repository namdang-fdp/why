/**
 * Dev-only benchmark of file switching, started by `VITE_WHY_BENCH=1 pnpm dev`.
 *
 * It drives the real store the way j/k and clicks do, in the worktree with the
 * most changes, and reports how long each switch took from "selected" to "the new
 * diff is on screen" (`lib/perf.ts`). Results go to the browser console and to the
 * dev server's log (`/__why_perf`, see `vite.config.ts`), so they can be read
 * from the terminal even inside the Tauri window. Never part of a production build.
 */
import { workspaceStore, selectChanges, selectWorktrees } from "@/stores/workspace";
import { devPerf, type SwitchTiming } from "@/lib/perf";

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

async function until(condition: () => boolean, timeoutMs = 20000) {
  const started = performance.now();
  while (!condition()) {
    if (performance.now() - started > timeoutMs) throw new Error("bench: timed out waiting");
    await sleep(50);
  }
}

function summary(label: string, timings: SwitchTiming[]) {
  const sorted = timings.map((t) => t.ms).sort((a, b) => a - b);
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  return {
    label,
    n: sorted.length,
    min: sorted[0],
    median: at(0.5),
    p90: at(0.9),
    max: sorted.at(-1),
    ms: sorted,
  };
}

export async function runBench() {
  const store = workspaceStore;
  await until(() => store.getState().repositories.status === "ok");
  await until(() => selectWorktrees(store.getState()).status === "ok");

  // The worktree with the most changed files makes the longest walk.
  const list = selectWorktrees(store.getState());
  if (list.status !== "ok") return;
  const biggest = [...list.data].sort((a, b) => b.changeCount - a.changeCount)[0];
  if (biggest) await store.getState().selectWorktree(biggest);
  await until(() => selectChanges(store.getState()).status === "ok");
  const changes = selectChanges(store.getState());
  if (changes.status !== "ok") return;
  const files = changes.data.filter((file) => !file.binary).map((file) => file.path);
  if (files.length < 8) {
    const seen = list.data.map((wt) => `${wt.path} (${wt.changeCount})`).join("; ");
    throw new Error(
      `bench: only ${files.length} text files in ${biggest?.path}; worktrees: ${seen}`,
    );
  }

  const perf = devPerf();
  if (!perf) throw new Error("bench: timing API missing (dev build only)");

  async function open(path: string) {
    const done = perf?.next(15000);
    await store.getState().selectFile(path);
    return done;
  }

  // Warm-up: loads Monaco and its worker, so the numbers are about switching.
  await open(files[0] as string);
  await sleep(800);

  const cold: SwitchTiming[] = [];
  // Every third file: never a neighbour of one already shown, so never prefetched.
  for (let i = 3; i < files.length && cold.length < 8; i += 3) {
    const timing = await open(files[i] as string);
    if (timing) cold.push(timing);
    await sleep(300); // lets the prefetch of its neighbours finish, like a pause while reading
  }

  const next: SwitchTiming[] = [];
  // Then walk in order with `j`, pausing like a reader: each file was prefetched.
  const start = Math.max(0, files.length - 9);
  await open(files[start] as string);
  await sleep(500);
  for (let i = start + 1; i < files.length; i++) {
    const timing = await open(files[i] as string);
    if (timing) next.push(timing);
    await sleep(400);
  }

  const back: SwitchTiming[] = [];
  // And return to files already visited (cache hits).
  for (const index of [3, 6, 9]) {
    const path = files[index];
    if (!path) continue;
    const timing = await open(path);
    if (timing) back.push(timing);
    await sleep(300);
  }

  const report = {
    worktree: biggest?.path,
    textFiles: files.length,
    uncached: summary("uncached (not prefetched)", cold),
    prefetched: summary("j/k to a prefetched neighbour", next),
    revisit: summary("revisit (LRU hit)", back),
    all: perf.switches,
  };
  console.table(
    [report.uncached, report.prefetched, report.revisit].map((row) => ({
      ...row,
      ms: row.ms.join(" "),
    })),
  );
  void fetch("/__why_perf", { method: "POST", body: JSON.stringify(report) });
}
