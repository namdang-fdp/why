import { createStore } from "zustand/vanilla";
import type {
  DiffMode,
  Harness,
  RationaleResult,
  SessionId,
  ShellOutcomesResult,
  TurnRef,
  WorktreeAttribution,
  WorktreeId,
} from "@/contracts";
import { idle, resolveState, type AsyncState } from "@/data/async-state";
import type { DataSource } from "@/data/data-source";
import { sameJson } from "./same";

/**
 * The answers the Rust core gives about WHY a file or a turn looks the way it
 * does: file attribution (WHY-16), the agent's own words (WHY-53a) and the exit
 * codes Codex hooks leave out (WHY-53a). Three different freshness rules live
 * here, kept apart on purpose:
 *
 * - attribution and file activity follow the selected worktree and are refetched
 *   (coalesced) when files or sessions change;
 * - rationale reads a transcript, so it is fetched ONLY when the user opens a
 *   "Why?" disclosure, then cached for the session's lifetime;
 * - shell outcomes are one bounded file read: once when a Codex session opens,
 *   again only on a refresh or a coalesced `sessions-changed`.
 *
 * Every request is versioned: an answer that arrives after the selection moved
 * on is dropped, never shown.
 */
type EvidenceSource = Pick<
  DataSource,
  | "getWorktreeAttribution"
  | "getFileActivity"
  | "getTurnRationale"
  | "getToolRationale"
  | "getShellOutcomes"
  | "onWorktreeChanged"
  | "onSessionsChanged"
>;

export type RationaleTarget =
  | { kind: "turn"; sessionId: SessionId; turnId: string }
  | { kind: "tool"; sessionId: SessionId; eventId: string };

/** Cache key; includes the session so one session's answer can never appear in another. */
export function rationaleKey(target: RationaleTarget) {
  return JSON.stringify([
    target.kind,
    target.sessionId,
    target.kind === "turn" ? target.turnId : target.eventId,
  ]);
}

type OpenSession = { id: SessionId; harness: Harness };

type EvidenceState = {
  scope: { worktreeId: WorktreeId; mode: DiffMode } | null;
  attribution: AsyncState<WorktreeAttribution>;
  /** The rail shows only files no captured edit explains. */
  onlyUnexplained: boolean;
  activityPath: string | null;
  /** The file's previous name when it is a rename: its edits may have been made under it. */
  activityOldPath: string | null;
  activity: AsyncState<TurnRef[]>;
  session: OpenSession | null;
  outcomes: AsyncState<ShellOutcomesResult>;
  rationales: ReadonlyMap<string, AsyncState<RationaleResult>>;
  /** Keys of the "Why?" disclosures that are open in the current session. */
  whyOpen: ReadonlySet<string>;
  setScope: (worktreeId: WorktreeId | null, mode: DiffMode) => Promise<void>;
  setFile: (path: string | null, oldPath?: string | null) => Promise<void>;
  setSession: (session: OpenSession | null) => Promise<void>;
  setOnlyUnexplained: (on: boolean) => void;
  loadAttribution: () => Promise<void>;
  loadActivity: () => Promise<void>;
  loadOutcomes: () => Promise<void>;
  loadRationale: (target: RationaleTarget, refresh?: boolean) => Promise<void>;
  /** Open (or close) a "Why?" disclosure; opening is what triggers the read. */
  toggleWhy: (target: RationaleTarget, open?: boolean) => void;
  watch: () => () => void;
};

/** Calls from several lookups as one list: no call twice, newest first. */
function mergeActivity(lists: TurnRef[][]): TurnRef[] {
  const seen = new Map<string, TurnRef>();
  for (const ref of lists.flat()) {
    seen.set(
      JSON.stringify([ref.sessionId, ref.turnId, ref.agentId, ref.toolUseId, ref.at, ref.path]),
      ref,
    );
  }
  return [...seen.values()].sort((a, b) => b.at.localeCompare(a.at));
}

export function createEvidenceStore(source: EvidenceSource, refreshWindowMs = 300) {
  return createStore<EvidenceState>((set, get) => {
    let attributionVersion = 0;
    let activityVersion = 0;
    let outcomesVersion = 0;
    // Bumped when the session changes: drops the cache and every answer still in flight.
    let rationaleEpoch = 0;
    const rationaleVersions = new Map<string, number>();

    async function loadAttribution(background = false) {
      const { scope, attribution } = get();
      if (scope === null) return;
      const version = ++attributionVersion;
      if (!background || attribution.status !== "ok") set({ attribution: { status: "loading" } });

      const result = await resolveState(() =>
        source.getWorktreeAttribution(scope.worktreeId, scope.mode),
      );
      if (version !== attributionVersion) return;
      const current = get().attribution;
      if (result.status === "error") {
        if (background && current.status === "ok") {
          console.warn("Could not refresh file attribution", result.error);
        } else {
          set({ attribution: result });
        }
        return;
      }
      // Skip a write that changes nothing so the rail does not re-render per event.
      if (current.status === "ok" && sameJson(current.data, result.data)) return;
      set({ attribution: result });
    }

    async function loadActivity(background = false) {
      const { scope, activityPath, activityOldPath, activity } = get();
      if (scope === null || activityPath === null) return;
      const version = ++activityVersion;
      if (!background || activity.status !== "ok") set({ activity: { status: "loading" } });

      // The core looks a file up by the exact path it is given, so a renamed file's
      // earlier edits (made under its old name) need a second question.
      const paths = activityOldPath ? [activityPath, activityOldPath] : [activityPath];
      const result = await resolveState(async () =>
        mergeActivity(
          await Promise.all(paths.map((path) => source.getFileActivity(scope.worktreeId, path))),
        ),
      );
      if (version !== activityVersion) return;
      const current = get().activity;
      if (result.status === "error" && background && current.status === "ok") {
        console.warn("Could not refresh file activity", result.error);
        return;
      }
      if (result.status === "ok" && current.status === "ok" && sameJson(current.data, result.data))
        return;
      set({ activity: result });
    }

    async function loadOutcomes(background = false) {
      const { session, outcomes } = get();
      // Only Codex needs this: Claude hooks report exit codes themselves.
      if (session === null || session.harness !== "codex") return;
      const version = ++outcomesVersion;
      if (!background || outcomes.status !== "ok") set({ outcomes: { status: "loading" } });

      const result = await resolveState(() => source.getShellOutcomes(session.id));
      if (version !== outcomesVersion) return;
      const current = get().outcomes;
      if (result.status === "error" && background && current.status === "ok") {
        console.warn("Could not refresh recovered exit codes", result.error);
        return;
      }
      if (result.status === "ok" && current.status === "ok" && sameJson(current.data, result.data))
        return;
      set({ outcomes: result });
    }

    async function loadRationale(target: RationaleTarget, refresh = false) {
      // A turn the harness gave no id cannot be looked up: the core always refuses "".
      if (target.kind === "turn" && target.turnId === "") return;
      const key = rationaleKey(target);
      const current = get().rationales.get(key);
      // Cached or already on its way: opening the same disclosure twice reads once.
      if (!refresh && (current?.status === "ok" || current?.status === "loading")) return;

      const epoch = rationaleEpoch;
      const version = (rationaleVersions.get(key) ?? 0) + 1;
      rationaleVersions.set(key, version);
      const put = (state: AsyncState<RationaleResult>) =>
        set((s) => ({ rationales: new Map(s.rationales).set(key, state) }));
      // A refresh keeps showing the old words until the new ones arrive.
      if (!(refresh && current?.status === "ok")) put({ status: "loading" });

      const result = await resolveState(() =>
        target.kind === "turn"
          ? source.getTurnRationale(target.sessionId, target.turnId)
          : source.getToolRationale(target.sessionId, target.eventId),
      );
      if (epoch !== rationaleEpoch || rationaleVersions.get(key) !== version) return;
      put(result);
    }

    return {
      scope: null,
      attribution: idle,
      onlyUnexplained: false,
      activityPath: null,
      activityOldPath: null,
      activity: idle,
      session: null,
      outcomes: idle,
      rationales: new Map(),
      whyOpen: new Set(),

      setScope: async (worktreeId, mode) => {
        const { scope } = get();
        if (worktreeId === null) {
          if (scope === null) return;
          attributionVersion++;
          activityVersion++;
          set({
            scope: null,
            attribution: idle,
            activityPath: null,
            activityOldPath: null,
            activity: idle,
            onlyUnexplained: false,
          });
          return;
        }
        if (scope?.worktreeId === worktreeId && scope.mode === mode) return;
        const sameWorktree = scope?.worktreeId === worktreeId;
        attributionVersion++;
        activityVersion++;
        set({
          scope: { worktreeId, mode },
          attribution: idle,
          // A file selection belongs to one worktree; within it, only the comparison changed.
          ...(sameWorktree
            ? {}
            : {
                activityPath: null,
                activityOldPath: null,
                activity: idle,
                onlyUnexplained: false,
              }),
        });
        await Promise.all([loadAttribution(), sameWorktree ? loadActivity() : undefined]);
      },

      setFile: async (path, oldPath = null) => {
        if (path === get().activityPath && oldPath === get().activityOldPath) return;
        activityVersion++;
        set({
          activityPath: path,
          activityOldPath: path === null ? null : oldPath,
          activity: path === null ? idle : { status: "loading" },
        });
        await loadActivity();
      },

      setSession: async (session) => {
        if (session?.id === get().session?.id) return;
        // New session: its cache, its open disclosures and its exit codes start empty.
        rationaleEpoch++;
        rationaleVersions.clear();
        outcomesVersion++;
        set({ session, outcomes: idle, rationales: new Map(), whyOpen: new Set() });
        await loadOutcomes();
      },

      setOnlyUnexplained: (onlyUnexplained) => set({ onlyUnexplained }),

      loadAttribution: () => loadAttribution(),
      loadActivity: () => loadActivity(),
      loadOutcomes: () => loadOutcomes(),
      loadRationale,

      toggleWhy: (target, open) => {
        if (target.kind === "turn" && target.turnId === "") return;
        const key = rationaleKey(target);
        const opened = get().whyOpen.has(key);
        const next = open ?? !opened;
        if (next === opened) return;
        const whyOpen = new Set(get().whyOpen);
        if (next) whyOpen.add(key);
        else whyOpen.delete(key);
        set({ whyOpen });
        // Only opening reads the transcript. Closing leaves any in-flight answer to land in the cache.
        if (next) void loadRationale(target);
      },

      watch: () => {
        let stopped = false;
        let timer: ReturnType<typeof setTimeout> | undefined;
        let running = false;
        const dirty = { attribution: false, outcomes: false };

        // One refresh per window. An event during a read marks the next window dirty,
        // so nothing is lost behind it and a burst of events costs a couple of reads.
        function schedule() {
          if (stopped || running || timer !== undefined) return;
          if (!dirty.attribution && !dirty.outcomes) return;
          timer = setTimeout(() => {
            timer = undefined;
            const work = { ...dirty };
            dirty.attribution = dirty.outcomes = false;
            running = true;
            const reads: Promise<void>[] = [];
            if (work.attribution) reads.push(loadAttribution(true), loadActivity(true));
            if (work.outcomes) reads.push(loadOutcomes(true));
            void Promise.all(reads).finally(() => {
              running = false;
              schedule();
            });
          }, refreshWindowMs);
        }

        const stopWorktrees = source.onWorktreeChanged(({ worktreeId }) => {
          if (worktreeId !== get().scope?.worktreeId) return;
          dirty.attribution = true;
          schedule();
        });
        const stopSessions = source.onSessionsChanged(({ worktreeId }) => {
          if (worktreeId !== get().scope?.worktreeId) return;
          dirty.attribution = true;
          dirty.outcomes = true;
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
