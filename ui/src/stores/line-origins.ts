import { createStore } from "zustand/vanilla";
import type {
  FileDiff,
  IpcError,
  LineOriginsResult,
  SessionId,
  SessionTimeline,
  WorktreeId,
} from "@/contracts";
import { idle, resolveState, type AsyncState } from "@/data/async-state";
import type { DataSource } from "@/data/data-source";
import { sameJson } from "./same";

/**
 * Line provenance for the ONE file open in the diff (WHY-25).
 *
 * Origins are read lazily, only for the file the reviewer is looking at, never
 * for every changed file. Three freshness rules, all versioned so an answer that
 * arrives after the selection moved on is dropped, never painted:
 *
 * - a different file or worktree starts a new request and shows nothing until
 *   ITS answer is in (the old file's origins are gone in the same `set`);
 * - when the file changes on disk its REVISION is bumped (one counter per file,
 *   shared with the diff side, see `tagDiffRead`). The origins remember the
 *   revision their read started at, so an answer that was already in flight when
 *   the file changed is recognisably old. Marks need the origins AND the shown
 *   diff to be at the file's current revision (`gutterGate`); the store reads
 *   the origins again after a short quiet window;
 * - the turns behind the runs (prompt, tool call) come from per-session
 *   timelines, fetched only when a mark is hovered or a line is opened.
 */
type LineOriginsSource = Pick<
  DataSource,
  "getLineOrigins" | "getSessionTimeline" | "onWorktreeChanged" | "onSessionsChanged"
>;

type Target = { worktreeId: WorktreeId; path: string };

/**
 * The revision of the file each shown diff was READ at. A WeakMap keyed by the
 * diff object (the workspace store tags every read it makes), so a cached diff
 * keeps its tag and nothing needs to be threaded through the diff cache.
 */
const diffReadRevisions = new WeakMap<FileDiff, number>();

/** Record that `diff` was read starting at `revision` of its file. */
export function tagDiffRead(diff: FileDiff, revision: number) {
  diffReadRevisions.set(diff, revision);
}

/** The revision `diff` was read at, or `undefined` when nobody tagged it. */
export function diffReadRevision(diff: FileDiff | null | undefined) {
  return diff ? diffReadRevisions.get(diff) : undefined;
}

const fileKey = (worktreeId: WorktreeId, path: string) => JSON.stringify([worktreeId, path]);

type LineOriginsState = {
  target: Target | null;
  origins: AsyncState<LineOriginsResult>;
  /** How many changes to the target file have been reported (see the note above). */
  revision: number;
  /** The `revision` the read behind `origins` started at. */
  originsRevision: number;
  /** A background re-read failed while `origins` is behind `revision`: say so, offer a retry. */
  refreshError: IpcError | null;
  /** Bumped when a diff re-read confirmed unchanged text, so views re-check their tags. */
  diffTick: number;
  /** The line the inspector explains (1-based), or `null`. Belongs to `target`. */
  line: number | null;
  /** Timelines of the sessions behind the runs, by session id. */
  timelines: ReadonlyMap<SessionId, AsyncState<SessionTimeline>>;
  setTarget: (worktreeId: WorktreeId | null, path: string | null) => Promise<void>;
  load: () => Promise<void>;
  /** The revision of any file (changes reported for it, or for its whole worktree). */
  revisionOf: (worktreeId: WorktreeId, path: string) => number;
  /** A re-read of the shown diff finished with the same text: retag it and tell the views. */
  confirmDiff: (diff: FileDiff, revision: number) => void;
  selectLine: (line: number | null) => void;
  loadTimeline: (sessionId: SessionId, refresh?: boolean) => Promise<void>;
  watch: () => () => void;
};

export function createLineOriginsStore(source: LineOriginsSource, refreshWindowMs = 300) {
  return createStore<LineOriginsState>((set, get) => {
    // Bumped by every request and every target change: only the newest answer lands.
    let version = 0;
    // Change counters: per file, and per worktree for "everything changed" (`paths: []`).
    const fileRevisions = new Map<string, number>();
    const worktreeRevisions = new Map<WorktreeId, number>();
    const revisionOf = (worktreeId: WorktreeId, path: string) =>
      (fileRevisions.get(fileKey(worktreeId, path)) ?? 0) +
      (worktreeRevisions.get(worktreeId) ?? 0);
    const timelineVersions = new Map<SessionId, number>();

    async function load(background = false) {
      const { target, origins } = get();
      if (target === null) return;
      const mine = ++version;
      // A read that starts now covers every change reported so far, and no later one.
      const seen = revisionOf(target.worktreeId, target.path);
      if (!background || origins.status !== "ok") set({ origins: { status: "loading" } });

      const result = await resolveState(() =>
        source.getLineOrigins(target.worktreeId, target.path),
      );
      const now = get().target;
      if (mine !== version || now?.worktreeId !== target.worktreeId || now.path !== target.path) {
        return;
      }
      const current = get().origins;
      if (result.status === "error") {
        if (background && current.status === "ok") {
          // Keep the last answer (it may still be right), but if it is behind the file the
          // views must say the refresh failed rather than promise the marks will return.
          console.warn("Could not refresh line origins", result.error);
          set({ refreshError: result.error });
        } else {
          set({ origins: result, refreshError: null });
        }
        return;
      }
      // Skip a write that changes nothing so the gutter is not rebuilt per event.
      if (current.status === "ok" && sameJson(current.data, result.data)) {
        set({ originsRevision: seen, refreshError: null });
        return;
      }
      set({ origins: result, originsRevision: seen, refreshError: null });
    }

    async function loadTimeline(sessionId: SessionId, refresh = false) {
      const current = get().timelines.get(sessionId);
      if (!refresh && (current?.status === "ok" || current?.status === "loading")) return;
      const mine = (timelineVersions.get(sessionId) ?? 0) + 1;
      timelineVersions.set(sessionId, mine);
      const put = (state: AsyncState<SessionTimeline>) =>
        set((s) => ({ timelines: new Map(s.timelines).set(sessionId, state) }));
      // A refresh keeps showing the old turns until the new ones arrive.
      if (!(refresh && current?.status === "ok")) put({ status: "loading" });
      const result = await resolveState(() => source.getSessionTimeline(sessionId));
      if (timelineVersions.get(sessionId) !== mine) return;
      put(result);
    }

    return {
      target: null,
      origins: idle,
      revision: 0,
      originsRevision: 0,
      refreshError: null,
      diffTick: 0,
      line: null,
      timelines: new Map(),

      setTarget: async (worktreeId, path) => {
        const { target } = get();
        const next = worktreeId !== null && path !== null ? { worktreeId, path } : null;
        if (target?.worktreeId === next?.worktreeId && target?.path === next?.path) return;
        // Drop whatever is in flight and clear the old file's answer in the same write.
        version++;
        set({
          target: next,
          origins: next ? { status: "loading" } : idle,
          revision: next ? revisionOf(next.worktreeId, next.path) : 0,
          originsRevision: next ? revisionOf(next.worktreeId, next.path) : 0,
          refreshError: null,
          line: null,
        });
        await load();
      },

      load: () => load(),

      revisionOf,

      confirmDiff: (diff, revision) => {
        tagDiffRead(diff, revision);
        set((state) => ({ diffTick: state.diffTick + 1 }));
      },

      selectLine: (line) => set({ line }),

      loadTimeline,

      watch: () => {
        let stopped = false;
        let timer: ReturnType<typeof setTimeout> | undefined;
        let running = false;
        let dirty = false;

        // One read per quiet window; an event during a read marks the next window dirty.
        function schedule() {
          if (stopped || running || timer !== undefined || !dirty) return;
          timer = setTimeout(() => {
            timer = undefined;
            dirty = false;
            running = true;
            const referenced = new Set<SessionId>();
            const { origins } = get();
            if (origins.status === "ok" && origins.data.status === "ok") {
              for (const run of origins.data.origins.runs) {
                if (run.origin.kind === "edit") referenced.add(run.origin.turn.sessionId);
              }
            }
            const reads = [load(true)];
            for (const sessionId of referenced) {
              if (get().timelines.has(sessionId)) reads.push(loadTimeline(sessionId, true));
            }
            void Promise.all(reads).finally(() => {
              running = false;
              schedule();
            });
          }, refreshWindowMs);
        }

        const stopWorktrees = source.onWorktreeChanged(({ worktreeId, paths }) => {
          // Every file's revision moves, target or not: the diff side tags the diffs it
          // reads (prefetched neighbours too) with these numbers, so they must stay true.
          if (paths.length === 0) {
            worktreeRevisions.set(worktreeId, (worktreeRevisions.get(worktreeId) ?? 0) + 1);
          } else {
            for (const path of paths) {
              const key = fileKey(worktreeId, path);
              fileRevisions.set(key, (fileRevisions.get(key) ?? 0) + 1);
            }
          }
          const { target } = get();
          if (target?.worktreeId !== worktreeId) return;
          // `paths: []` means "unknown, assume everything" (a branch switch).
          if (paths.length > 0 && !paths.includes(target.path)) return;
          set({ revision: revisionOf(target.worktreeId, target.path) });
          dirty = true;
          schedule();
        });
        const stopSessions = source.onSessionsChanged(({ worktreeId }) => {
          if (worktreeId !== get().target?.worktreeId) return;
          dirty = true;
          schedule();
        });
        return () => {
          stopped = true;
          clearTimeout(timer);
          timer = undefined;
          stopWorktrees();
          stopSessions();
        };
      },
    };
  });
}

/** The origins on show were read before the file's latest reported change. */
export function originsBehind(state: Pick<LineOriginsState, "revision" | "originsRevision">) {
  return state.originsRevision !== state.revision;
}
