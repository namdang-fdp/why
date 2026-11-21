import { createStore, type StoreApi } from "zustand/vanilla";
import { hasContentDiff } from "@/components/diff/diff-preview";
import type { HunkDirection, HunkResult } from "@/components/diff/hunk-navigation";
import type {
  DiffMode,
  FileChange,
  FileDiff,
  IpcError,
  RepoId,
  Repository,
  RepositoryChanged,
  RepositorySecurity,
  RevokeResult,
  StorageStatus,
  Worktree,
  WorktreeChanged,
  WorktreeId,
} from "@/contracts";
import { dataSource } from "@/data";
import { idle, resolveState, type AsyncState, type Settled } from "@/data/async-state";
import type { DataSource } from "@/data/data-source";
import { describeError } from "@/lib/error-text";
import { toastStore, type Toasts } from "./toasts";
import { sortWorktrees, worktreeLabels } from "@/components/shell/rail-model";
import { beginFileSwitch } from "@/lib/perf";
import { createDiffCache } from "./diff-cache";
import { createEvidenceStore } from "./evidence";
import { createLineOriginsStore, diffReadRevision, tagDiffRead } from "./line-origins";
import { createSessionsStore } from "./sessions";
import { sameFileChange, sameFileChanges, sameFileDiff, sameJson } from "./same";

type Queries<T> = ReadonlyMap<string, AsyncState<T>>;

export type SettingsSection = "editor" | "agent-hooks";
type SettingsView =
  { status: "closed"; lastSection: SettingsSection } | { status: "open"; section: SettingsSection };

export type DiffLayout = "split" | "unified";
type NavigateHunk = (direction: HunkDirection) => HunkResult;

type WorkspaceStore = {
  hookData: Pick<
    DataSource,
    "hookStatus" | "previewInstall" | "install" | "previewUninstall" | "uninstall"
  >;
  sessions: ReturnType<typeof createSessionsStore>;
  /** Attribution, "Why?" rationale and recovered Codex exit codes (WHY-16, WHY-53a). */
  evidence: ReturnType<typeof createEvidenceStore>;
  /** Line origins of the file open in the diff (WHY-25, v0.3 line provenance). */
  lineOrigins: ReturnType<typeof createLineOriginsStore>;
  repositories: AsyncState<Repository[]>;
  storage: AsyncState<StorageStatus>;
  worktrees: Queries<Worktree[]>;
  changes: Queries<FileChange[]>;
  /** Each repository's git filter programs and whether WHY may read it (WHY-39). */
  security: Queries<RepositorySecurity>;
  fileDiff: AsyncState<FileDiff>;
  repoId: RepoId | null;
  worktreeId: WorktreeId | null;
  filePath: string | null;
  preferredMode: DiffMode;
  diffLayout: DiffLayout;
  inspectorOpen: boolean;
  paletteOpen: boolean;
  addRepositoryOpen: boolean;
  settingsView: SettingsView;
  openSettings: (section?: SettingsSection) => void;
  closeSettings: () => void;
  /** The rail's filter text (substring match over repositories, worktrees and files). */
  railFilter: string;
  /** When the watcher last saw a file edit in a worktree (Unix seconds); newer than `lastActivity`. */
  activity: ReadonlyMap<WorktreeId, number>;
  /** Worktrees whose status dot should pulse right now: id -> a number that changes per event. */
  pulses: ReadonlyMap<WorktreeId, number>;
  /** Files whose rows should flash after a live update: path -> update number. */
  flashes: ReadonlyMap<string, number>;
  loadRepositories: () => Promise<void>;
  loadStorageStatus: () => Promise<void>;
  setAddRepositoryOpen: (open: boolean) => void;
  setRailFilter: (text: string) => void;
  pickRepositoryFolder: () => Promise<Settled<string | null>>;
  addRepository: (path: string) => Promise<Settled<Repository>>;
  removeRepository: (repo: Repository) => Promise<Settled<void>>;
  setBase: (worktree: Worktree, baseRef: string) => Promise<Settled<Worktree>>;
  retryWorktrees: (repoId: RepoId) => Promise<void>;
  /** `refresh` bypasses the cache (the panel asks for the truth, not what was cached). */
  loadSecurity: (repoId: RepoId, refresh?: boolean) => Promise<void>;
  /** Trust the filter programs shown to the user (`fingerprint` is what they saw). */
  trustFilters: (repo: Repository, fingerprint: string) => Promise<Settled<RepositorySecurity>>;
  /** Forget that trust decision. */
  revokeFilters: (repo: Repository) => Promise<Settled<RevokeResult>>;
  selectRepository: (repoId: RepoId) => Promise<void>;
  selectWorktree: (worktree: Worktree) => Promise<void>;
  jumpToFile: (worktree: Worktree, path: string, availableModes: DiffMode[]) => Promise<void>;
  selectFile: (path: string) => Promise<void>;
  moveWorktree: (step: 1 | -1) => Promise<void>;
  moveFile: (step: 1 | -1) => Promise<void>;
  moveChange: (direction: HunkDirection) => Promise<void>;
  registerDiffNavigation: (diff: FileDiff, move: NavigateHunk) => () => void;
  setDiffLayout: (layout: DiffLayout) => void;
  toggleDiffLayout: () => void;
  setDiffMode: (mode: DiffMode) => Promise<void>;
  retryChanges: () => Promise<void>;
  toggleInspector: () => void;
  showInspector: () => void;
  openSessionFile: (path: string) => Promise<void>;
  setPaletteOpen: (open: boolean) => void;
  loadPaletteFiles: (worktree: Worktree, mode: DiffMode) => Promise<AsyncState<FileChange[]>>;
  watchWorktrees: () => () => void;
};

const diffLayoutKey = "why:diff-layout:v1";

function readDiffLayout(): DiffLayout {
  try {
    return window.localStorage.getItem(diffLayoutKey) === "unified" ? "unified" : "split";
  } catch {
    return "split";
  }
}

export const selectWorktrees = (state: WorkspaceStore) =>
  (state.repoId !== null ? state.worktrees.get(state.repoId) : undefined) ?? idle;

/** The active repository's worktrees in the order the rail lists them. */
export function selectOrderedWorktrees(state: WorkspaceStore) {
  const list = selectWorktrees(state);
  if (list.status !== "ok") return [];
  const repo =
    state.repositories.status === "ok"
      ? state.repositories.data.find((entry) => entry.id === state.repoId)
      : undefined;
  return sortWorktrees(list.data, worktreeLabels(list.data, repo), state.activity);
}

/** The selected worktree's rail title (its branch, or `detached @ sha`), or `null`. */
export function selectActiveWorktreeTitle(state: WorkspaceStore) {
  const worktree = selectWorktree(state);
  if (!worktree) return null;
  const list = selectWorktrees(state);
  const repo =
    state.repositories.status === "ok"
      ? state.repositories.data.find((entry) => entry.id === state.repoId)
      : undefined;
  const siblings = list.status === "ok" ? list.data : [worktree];
  return worktreeLabels(siblings, repo).get(worktree.id)?.primary ?? null;
}

export function selectWorktree(state: WorkspaceStore) {
  const worktrees = selectWorktrees(state);
  return worktrees.status === "ok"
    ? worktrees.data.find((worktree) => worktree.id === state.worktreeId)
    : undefined;
}

export function modeForWorktree(mode: DiffMode, worktree?: Worktree): DiffMode {
  return mode === "vsBase" && worktree?.base === null ? "uncommitted" : mode;
}

export const selectDiffMode = (state: WorkspaceStore) =>
  modeForWorktree(state.preferredMode, selectWorktree(state));

export const changesKey = (id: WorktreeId, mode: DiffMode) => `${mode}:${id}`;

export function selectChanges(state: WorkspaceStore) {
  const worktree = selectWorktree(state);
  if (!worktree) return idle;
  const mode = modeForWorktree(state.preferredMode, worktree);
  return state.changes.get(changesKey(worktree.id, mode)) ?? idle;
}

export function selectActiveFile(state: WorkspaceStore) {
  const changes = selectChanges(state);
  return changes.status === "ok"
    ? changes.data.find((file) => file.path === state.filePath)
    : undefined;
}

function moveIndex(length: number, index: number, step: 1 | -1) {
  if (index < 0) return step === 1 ? 0 : length - 1;
  return (index + step + length) % length;
}

// Share in-flight reads across the rail and palette. An invalidated request can
// finish, but its result cannot replace a newer cache entry.
//
// `refresh` re-reads a key that is already cached *silently*: the old data stays
// on screen while the request runs, and is replaced only when the answer is
// different (`same`) and a success. So a live update neither flashes a loading
// state nor swaps good data for a transient error. The returned state is
// whatever the cache holds afterwards.
function createQueryLoader<T>(read: () => Queries<T>, write: (queries: Queries<T>) => void) {
  const pending = new Map<string, Promise<AsyncState<T>>>();

  return {
    load(
      key: string,
      request: () => Promise<T>,
      refresh = false,
      same?: (previous: T, next: T) => boolean,
    ): Promise<AsyncState<T>> {
      const inFlight = pending.get(key);
      if (inFlight && !refresh) return inFlight;
      const cached = read().get(key);
      if (cached?.status === "ok" && !refresh) return Promise.resolve(cached);

      const result = resolveState(request).then((state): AsyncState<T> => {
        if (pending.get(key) !== result) return state;
        pending.delete(key);
        const current = read().get(key);
        const keep =
          current?.status === "ok" &&
          refresh &&
          (state.status === "error" || (same?.(current.data, state.data) ?? false));
        if (keep) return current;
        write(new Map(read()).set(key, state));
        return state;
      });
      pending.set(key, result);
      if (cached?.status !== "ok") write(new Map(read()).set(key, { status: "loading" }));
      return result;
    },
    /**
     * Make in-flight reads of `keys` obsolete WITHOUT touching what is cached:
     * their results will be thrown away when they arrive. Used when something
     * newer than any read in flight is already known (a saved base override).
     */
    supersede(keys: string[]) {
      for (const key of keys) pending.delete(key);
    },
    invalidate(keys: string[]) {
      const next = new Map(read());
      for (const key of keys) {
        pending.delete(key);
        next.delete(key);
      }
      write(next);
    },
  };
}

type PendingRefresh = { all: boolean; paths: Set<string> };

/** Live updates arriving within this window are merged into one refresh per worktree. */
export const refreshWindowMs = 80;

/** How long a worktree's status dot pulses after the watcher saw an edit in it. */
export const pulseMs = 2000;

export const diffModes = [
  { mode: "vsBase", label: "Vs base", key: "1" },
  { mode: "uncommitted", label: "Uncommitted", key: "2" },
  { mode: "staged", label: "Staged", key: "3" },
] as const satisfies ReadonlyArray<{ mode: DiffMode; label: string; key: string }>;

export function createWorkspaceStore(source: DataSource, toasts: StoreApi<Toasts> = toastStore) {
  const sessions = createSessionsStore(source);
  const evidence = createEvidenceStore(source);
  const lineOrigins = createLineOriginsStore(source);
  return createStore<WorkspaceStore>((set, get) => {
    const lastFileByWorktree = new Map<WorktreeId, string>();
    const worktreeQueries = createQueryLoader(
      () => get().worktrees,
      (worktrees) => set({ worktrees }),
    );
    const changeQueries = createQueryLoader(
      () => get().changes,
      (changes) => set({ changes }),
    );
    const securityQueries = createQueryLoader(
      () => get().security,
      (security) => set({ security }),
    );
    const loadWorktrees = (id: RepoId, refresh = false) =>
      worktreeQueries.load(id, () => source.listWorktrees(id), refresh, sameJson);
    const loadSecurityOf = (id: RepoId, refresh = false) =>
      securityQueries.load(id, () => source.getRepositorySecurity(id), refresh, sameJson);
    const loadChanges = (id: WorktreeId, mode: DiffMode) =>
      changeQueries
        .load(changesKey(id, mode), () => source.getChanges(id, mode))
        .then((state) => {
          // The core just refused to read this repository. The security report we
          // may have cached says otherwise (the driver arrived through a file the
          // watcher does not route, say), and the explanation panel would then sit
          // on "Reading changes…". Ask again, past the cache.
          if (state.status === "error" && state.error.kind === "untrustedFilters") {
            const owner = ownerOf(id);
            if (owner !== undefined) void loadSecurityOf(owner, true);
          }
          return state;
        });
    const notify = (title: string, detail?: string) =>
      toasts.getState().push({ tone: "info", title, detail });
    const notifyError = (title: string, error: IpcError) =>
      toasts.getState().push({ tone: "error", title, detail: describeError(error) });

    let navigationVersion = 0;
    let sessionFileVersion = 0;
    let diffRefreshVersion = 0;
    let flashVersion = 0;
    // A live update that hit the open file while its diff was still loading.
    let pendingDiffRefresh: { worktreeId: WorktreeId; path: string } | undefined;
    let diffNavigation: { diff: FileDiff; move: NavigateHunk } | undefined;

    function takePendingDiffRefresh() {
      const pending = pendingDiffRefresh;
      pendingDiffRefresh = undefined;
      return pending;
    }

    // ---- file diffs: cache + prefetch ---------------------------------------
    //
    // Switching files must not wait for git when it can be avoided. Every diff
    // read goes through `fetchDiff`, which fills an LRU cache; the neighbours a
    // j/k press would open next are read ahead of time. A cached switch is then
    // a synchronous `set`, so the new file is painted in the same frame.
    // Entries are dropped the moment the watcher reports their file (see
    // `invalidateDiffs`), before any refresh has run.

    const diffCache = createDiffCache();
    const inflightDiffs = new Map<string, Promise<Settled<FileDiff>>>();

    function fetchDiff(worktreeId: WorktreeId, mode: DiffMode, path: string) {
      const key = diffCache.key(worktreeId, mode, path);
      const shared = inflightDiffs.get(key);
      if (shared) return shared; // a prefetch of this very file is already running
      const epoch = diffCache.epoch(worktreeId);
      // The file's change count when this read starts: the diff covers every change
      // reported up to here, which line origins compare against (see `gutterGate`).
      const revision = lineOrigins.getState().revisionOf(worktreeId, path);
      const request = resolveState(() => source.getFileDiff(worktreeId, path, mode)).then(
        (result) => {
          if (inflightDiffs.get(key) === request) inflightDiffs.delete(key);
          if (result.status === "ok") {
            tagDiffRead(result.data, revision);
            diffCache.set(worktreeId, mode, path, result.data, epoch);
          }
          return result;
        },
      );
      inflightDiffs.set(key, request);
      return request;
    }

    /** `paths` undefined = the whole worktree. */
    function invalidateDiffs(worktreeId: WorktreeId, paths?: Iterable<string>) {
      if (paths) diffCache.invalidatePaths(worktreeId, paths);
      else diffCache.invalidateWorktree(worktreeId);
      // A read already in flight began before this change: nobody may join it.
      const prefix = JSON.stringify([worktreeId]).slice(0, -1);
      for (const key of inflightDiffs.keys()) {
        if (key.startsWith(prefix)) inflightDiffs.delete(key);
      }
    }

    /** Read the files j and k would open next, if they are not at hand yet. */
    function prefetchNeighbours(path: string) {
      const state = get();
      const changes = selectChanges(state);
      if (state.worktreeId === null || changes.status !== "ok" || changes.data.length < 2) return;
      const { worktreeId } = state;
      const mode = selectDiffMode(state);
      const index = changes.data.findIndex((file) => file.path === path);
      if (index < 0) return;
      for (const step of [1, -1]) {
        const next = changes.data[(index + step + changes.data.length) % changes.data.length];
        if (!next || next.path === path) continue;
        const key = diffCache.key(worktreeId, mode, next.path);
        if (diffCache.has(worktreeId, mode, next.path) || inflightDiffs.has(key)) continue;
        void fetchDiff(worktreeId, mode, next.path);
      }
    }

    async function loadFileDiff(path: string, syncVersion?: number) {
      const state = get();
      const changes = selectChanges(state);
      if (
        state.worktreeId === null ||
        changes.status !== "ok" ||
        !changes.data.some((file) => file.path === path)
      )
        return;

      const { worktreeId } = state;
      const mode = selectDiffMode(state);
      // A synced default belongs to the navigation already in progress, so it
      // must not supersede an explicit session-file lookup in that navigation.
      const version = syncVersion ?? ++navigationVersion;
      lastFileByWorktree.set(worktreeId, path);
      // This load starts AFTER anything the watcher reported so far, so it
      // already reads those changes; only an event arriving while it runs can
      // make its answer stale.
      pendingDiffRefresh = undefined;

      const cached = diffCache.get(worktreeId, mode, path);
      beginFileSwitch(path, cached !== undefined);
      if (cached) {
        set({ filePath: path, fileDiff: { status: "ok", data: cached } });
        prefetchNeighbours(path);
        return;
      }

      set({ filePath: path, fileDiff: { status: "loading" } });
      const fileDiff = await fetchDiff(worktreeId, mode, path);
      if (version !== navigationVersion) return;
      set({ fileDiff });
      // The file changed on disk after this read began: read it once more.
      const replay = takePendingDiffRefresh();
      if (replay?.worktreeId === worktreeId && replay.path === path) void refreshOpenDiff(path);
      else if (fileDiff.status === "ok") prefetchNeighbours(path);
    }

    async function syncChanges(worktree: Worktree, explicitPath = false) {
      const version = ++navigationVersion;
      const mode = modeForWorktree(get().preferredMode, worktree);
      set({ fileDiff: idle });
      const changes = await loadChanges(worktree.id, mode);
      if (version !== navigationVersion || changes.status !== "ok") return;
      const current = get().filePath;
      const remembered = lastFileByWorktree.get(worktree.id);
      const contains = (path: string | null | undefined) =>
        changes.data.some((file) => file.path === path);
      if (remembered !== undefined && !contains(remembered)) {
        lastFileByWorktree.delete(worktree.id);
      }
      const preferred = current ?? remembered;
      const path = contains(preferred)
        ? preferred
        : explicitPath
          ? undefined
          : changes.data[0]?.path;
      if (path !== undefined) await loadFileDiff(path, version);
      else set({ filePath: null });
    }

    async function navigateTo(worktree: Worktree, path: string | null, preferredMode: DiffMode) {
      set({
        repoId: worktree.repoId,
        worktreeId: worktree.id,
        filePath: path,
        fileDiff: idle,
        flashes: new Map(),
        preferredMode,
      });
      await syncChanges(worktree, path !== null);
    }

    // ---- live updates -------------------------------------------------------
    //
    // The core sends `worktree-changed` after its own debounce, but a build or a
    // `git checkout` can still produce several events a second, and a branch or
    // tag update sends one to EVERY worktree of the repository. Events are
    // merged per worktree and handled once per `refreshWindowMs`; at most one
    // refresh per worktree runs at a time, and events that arrive meanwhile are
    // merged into the next one. A refresh re-reads only what the event touched
    // and swaps results in silently (see `createQueryLoader`), so the selection,
    // the scroll position and the Monaco view all stay where they are.
    //
    // Metadata (`list_worktrees`) is shared: one read per repository per round,
    // however many of its worktrees were reported, and every affected worktree
    // then continues from that one answer. `repository-changed` (a worktree was
    // added or removed) goes through the same read.

    const queued = new Map<WorktreeId, PendingRefresh>();
    const queuedRepos = new Set<RepoId>();
    const running = new Set<WorktreeId>();
    let flushTimer: ReturnType<typeof setTimeout> | undefined;

    function schedule(delay = refreshWindowMs) {
      // A fixed window from the FIRST event (not restarted by later ones), so a
      // continuous stream still refreshes every `refreshWindowMs`.
      flushTimer ??= setTimeout(flushRefreshes, delay);
    }

    // A file edit (not a "refetch everything", which also fans out to every
    // worktree on a branch move or a new watch) counts as activity: the rail sorts
    // by it and the worktree's dot pulses for `pulseMs`.
    const pulseTimers = new Map<WorktreeId, ReturnType<typeof setTimeout>>();
    let pulseVersion = 0;
    function noteActivity(worktreeId: WorktreeId) {
      const seq = ++pulseVersion;
      set({
        activity: new Map(get().activity).set(worktreeId, Math.floor(Date.now() / 1000)),
        pulses: new Map(get().pulses).set(worktreeId, seq),
      });
      clearTimeout(pulseTimers.get(worktreeId));
      pulseTimers.set(
        worktreeId,
        setTimeout(() => {
          pulseTimers.delete(worktreeId);
          if (get().pulses.get(worktreeId) !== seq) return;
          const pulses = new Map(get().pulses);
          pulses.delete(worktreeId);
          set({ pulses });
        }, pulseMs),
      );
    }

    function queueRefresh({ worktreeId, paths }: WorktreeChanged) {
      if (paths.length > 0) noteActivity(worktreeId);
      const entry = queued.get(worktreeId) ?? { all: false, paths: new Set<string>() };
      // Empty `paths` means "unknown, refetch everything" and wins over any list.
      if (paths.length === 0) entry.all = true;
      for (const path of paths) entry.paths.add(path);
      queued.set(worktreeId, entry);
      // Right now, not when the refresh runs: a cached diff of a changed file
      // must not be shown to a switch that happens in the next 80 ms.
      invalidateDiffs(worktreeId, paths.length === 0 ? undefined : paths);
      schedule();
    }

    function queueRepository({ repoId }: RepositoryChanged) {
      queuedRepos.add(repoId);
      const list = get().worktrees.get(repoId);
      if (list?.status === "ok") for (const wt of list.data) invalidateDiffs(wt.id);
      schedule();
    }

    function flushRefreshes() {
      flushTimer = undefined;
      const repos = new Set(queuedRepos);
      queuedRepos.clear();
      const work: Array<[WorktreeId, PendingRefresh, RepoId]> = [];
      for (const [worktreeId, entry] of queued) {
        if (running.has(worktreeId)) continue; // picked up when the running one ends
        queued.delete(worktreeId);
        const owner = ownerOf(worktreeId);
        if (owner !== undefined) work.push([worktreeId, entry, owner]);
      }

      // Which repositories need their worktree list re-read this round: those
      // named by a repository event, and those with a worktree that reported
      // "everything changed" or is currently broken.
      for (const [worktreeId, entry, owner] of work) {
        if (entry.all || findWorktree(owner, worktreeId)?.loadError) repos.add(owner);
      }
      const metadata = new Map<RepoId, Promise<void>>();
      for (const repoId of repos) {
        if (get().worktrees.has(repoId)) metadata.set(repoId, relistRepository(repoId));
      }

      for (const [worktreeId, entry, owner] of work) {
        running.add(worktreeId);
        void refreshWorktree(worktreeId, entry, metadata.get(owner))
          .catch((error: unknown) => console.error("live refresh failed", error))
          .finally(() => {
            running.delete(worktreeId);
            if (queued.has(worktreeId)) schedule(0);
          });
      }
      for (const done of metadata.values()) {
        done.catch((error: unknown) => console.error("live refresh failed", error));
      }
    }

    function ownerOf(worktreeId: WorktreeId) {
      return Array.from(get().worktrees).find(
        ([, list]) => list.status === "ok" && list.data.some((wt) => wt.id === worktreeId),
      )?.[0];
    }

    function findWorktree(repoId: RepoId, worktreeId: WorktreeId) {
      const list = get().worktrees.get(repoId);
      return list?.status === "ok" ? list.data.find((wt) => wt.id === worktreeId) : undefined;
    }

    function patchWorktree(repoId: RepoId, updated: Worktree) {
      const list = get().worktrees.get(repoId);
      if (list?.status !== "ok") return;
      const data = list.data.map((wt) => (wt.id === updated.id ? updated : wt));
      set({ worktrees: new Map(get().worktrees).set(repoId, { status: "ok", data }) });
    }

    /**
     * Re-read a repository's worktrees (silently) and, if the selected worktree
     * is gone (removed or pruned), move the selection to its neighbour: the one
     * now at its old position, or the last.
     *
     * The answer is read back from the cache, not from this call's own result:
     * a newer read may have replaced this one, and the cache is what is shown.
     */
    async function relistRepository(repoId: RepoId) {
      const previous = get().worktrees.get(repoId);
      // A re-list follows commits and config edits: the repository may just have
      // gained (or lost) a git filter program, so its security view is re-read too.
      void loadSecurityOf(repoId, true);
      await loadWorktrees(repoId, true);
      const list = get().worktrees.get(repoId);
      if (list?.status !== "ok") return;
      if (previous?.status === "ok" && previous !== list) {
        const remaining = new Set(list.data.map((worktree) => worktree.id));
        for (const worktree of previous.data) {
          if (!remaining.has(worktree.id)) lastFileByWorktree.delete(worktree.id);
        }
      }
      const state = get();
      if (
        state.repoId !== repoId ||
        state.worktreeId === null ||
        list.data.some((wt) => wt.id === state.worktreeId)
      ) {
        return;
      }
      const index =
        previous?.status === "ok"
          ? previous.data.findIndex((wt) => wt.id === state.worktreeId)
          : -1;
      const neighbour = list.data[Math.min(Math.max(index, 0), list.data.length - 1)];
      if (neighbour) await get().selectWorktree(neighbour);
      else {
        navigationVersion++;
        set({ worktreeId: null, filePath: null, fileDiff: idle });
      }
    }

    /**
     * Trust was granted or revoked: what was read (or refused) under the old
     * decision is stale, in every worktree of the repository. Drop it and read
     * again, keeping the selection.
     */
    async function rereadAfterTrustChange(repoId: RepoId) {
      const list = get().worktrees.get(repoId);
      if (list?.status === "ok") {
        for (const wt of list.data) invalidateDiffs(wt.id);
        changeQueries.invalidate(
          list.data.flatMap((wt) => diffModes.map(({ mode }) => changesKey(wt.id, mode))),
        );
      }
      await relistRepository(repoId);
      const current = selectWorktree(get());
      if (get().repoId === repoId && current) await syncChanges(current);
    }

    async function refreshWorktree(
      worktreeId: WorktreeId,
      { all, paths }: PendingRefresh,
      metadata: Promise<void> | undefined,
    ) {
      const owner = ownerOf(worktreeId);
      if (owner === undefined) return;
      const modeBefore = selectDiffMode(get());

      // 1. Worktree metadata (base, ahead/behind, error, count), shared with every
      //    other worktree of the repository (see above). A plain file edit does
      //    not need it: step 2 is enough and git runs in only one worktree.
      const relist = metadata !== undefined;
      if (metadata) await metadata;

      const latest = findWorktree(owner, worktreeId);
      if (!latest) return; // removed meanwhile
      const state = get();
      const isCurrent = state.worktreeId === worktreeId;

      // A worktree that fails to load, or whose base appeared or went away, is
      // not a silent update: the visible list changes meaning, so go through the
      // normal path (loading state, then the result or its error).
      if (latest.loadError) {
        changeQueries.invalidate(diffModes.map(({ mode }) => changesKey(worktreeId, mode)));
        if (isCurrent) await syncChanges(latest);
        return;
      }
      const visibleMode = isCurrent ? selectDiffMode(state) : undefined;
      if (isCurrent && visibleMode !== modeBefore) {
        changeQueries.invalidate(diffModes.map(({ mode }) => changesKey(worktreeId, mode)));
        await syncChanges(latest);
        return;
      }

      // 2. The changed-files lists. The list behind the rail badge (when we did
      //    not re-list) and the visible one; every other cached mode is dropped
      //    and re-read when something asks for it.
      const countMode: DiffMode = latest.base ? "vsBase" : "uncommitted";
      const wanted = new Set<DiffMode>();
      if (!relist) wanted.add(countMode);
      if (visibleMode) wanted.add(visibleMode);
      changeQueries.invalidate(
        diffModes
          .filter(({ mode }) => !wanted.has(mode))
          .map(({ mode }) => changesKey(worktreeId, mode)),
      );

      await Promise.all(
        Array.from(wanted, async (mode) => {
          const key = changesKey(worktreeId, mode);
          const previous = get().changes.get(key);
          const next = await changeQueries.load(
            key,
            () => source.getChanges(worktreeId, mode),
            true,
            sameFileChanges,
          );
          // Only a result that is still what the cache holds may have effects. A
          // read that was invalidated meanwhile (the base was changed) or
          // replaced by a newer one reflects a state that no longer exists:
          // its count or "this file is gone" must not leak into the new one.
          if (next.status !== "ok" || get().changes.get(key) !== next) return;
          if (get().worktreeId !== worktreeId && previous?.status === "ok") {
            // A background refresh may read a different mode. Forget a file
            // only when this list previously showed it and now it has gone.
            const remembered = lastFileByWorktree.get(worktreeId);
            if (
              previous.data.some((file) => file.path === remembered) &&
              !next.data.some((file) => file.path === remembered)
            ) {
              lastFileByWorktree.delete(worktreeId);
            }
          }
          if (mode === countMode && !relist && latest.changeCount !== next.data.length) {
            const fresh = findWorktree(owner, worktreeId);
            if (fresh) patchWorktree(owner, { ...fresh, changeCount: next.data.length });
          }
          if (mode === visibleMode) {
            applyVisibleChanges(worktreeId, mode, previous, next.data, paths, all);
          }
        }),
      );
    }

    /** The visible list was re-read: flash what changed, drop or reload the open file. */
    function applyVisibleChanges(
      worktreeId: WorktreeId,
      mode: DiffMode,
      previous: AsyncState<FileChange[]> | undefined,
      files: FileChange[],
      paths: ReadonlySet<string>,
      all: boolean,
    ) {
      const state = get();
      if (state.worktreeId !== worktreeId || selectDiffMode(state) !== mode) return;

      if (previous?.status === "ok") {
        const known = new Map(previous.data.map((file) => [file.path, file]));
        const flashes = new Map(state.flashes);
        for (const file of files) {
          const old = known.get(file.path);
          if (!old || paths.has(file.path) || !sameFileChange(old, file)) {
            flashes.set(file.path, ++flashVersion);
          }
        }
        set({ flashes });
      }

      const path = state.filePath;
      if (path === null) return;
      const open = files.find((file) => file.path === path);
      if (!open) {
        // The file is no longer changed (reverted, committed): nothing to show.
        navigationVersion++;
        pendingDiffRefresh = undefined;
        lastFileByWorktree.delete(worktreeId);
        set({ filePath: null, fileDiff: idle });
        return;
      }
      const old =
        previous?.status === "ok" ? previous.data.find((f) => f.path === path) : undefined;
      const touched =
        all ||
        paths.has(path) ||
        (open.oldPath !== null && paths.has(open.oldPath)) ||
        !old ||
        !sameFileChange(old, open);
      if (touched) void refreshOpenDiff(path);
    }

    async function refreshOpenDiff(path: string) {
      const state = get();
      const { worktreeId, fileDiff } = state;
      if (worktreeId === null || fileDiff.status === "idle") return; // nothing open
      if (fileDiff.status === "loading") {
        // Its read may have happened before this change: run once more when it lands.
        pendingDiffRefresh = { worktreeId, path };
        return;
      }
      if (fileDiff.status === "error") {
        await loadFileDiff(path); // retry visibly; there was nothing good to keep
        return;
      }
      const mode = selectDiffMode(state);
      const refresh = ++diffRefreshVersion;
      const version = navigationVersion;
      const next = await fetchDiff(worktreeId, mode, path);
      const now = get();
      if (
        refresh !== diffRefreshVersion || // a newer refresh superseded this one
        version !== navigationVersion || // the user moved on
        now.worktreeId !== worktreeId ||
        now.filePath !== path ||
        next.status !== "ok"
      ) {
        return;
      }
      if (now.fileDiff.status === "ok" && sameFileDiff(now.fileDiff.data, next.data)) {
        // Same text: keep the shown diff (no re-render), but it has now been verified
        // against the newer revision, which line origins are waiting to hear.
        const revision = diffReadRevision(next.data);
        if (revision !== undefined) lineOrigins.getState().confirmDiff(now.fileDiff.data, revision);
        return;
      }
      set({ fileDiff: next });
    }

    return {
      sessions,
      evidence,
      lineOrigins,
      hookData: source,
      repositories: idle,
      storage: idle,
      worktrees: new Map(),
      changes: new Map(),
      security: new Map(),
      fileDiff: idle,
      repoId: null,
      worktreeId: null,
      filePath: null,
      preferredMode: "vsBase",
      diffLayout: readDiffLayout(),
      inspectorOpen: true,
      paletteOpen: false,
      addRepositoryOpen: false,
      settingsView: { status: "closed", lastSection: "editor" },
      railFilter: "",
      activity: new Map(),
      pulses: new Map(),
      flashes: new Map(),

      loadRepositories: async () => {
        const { status } = get().repositories;
        if (status === "loading" || status === "ok") return;
        set({ repositories: { status: "loading" } });
        const repositories = await resolveState(() => source.listRepositories());
        set({ repositories });
        if (repositories.status !== "ok") return;

        const [first, ...otherRepos] = repositories.data;
        for (const repo of repositories.data) void loadSecurityOf(repo.id);
        for (const repo of otherRepos) void loadWorktrees(repo.id);
        if (first) await get().selectRepository(first.id);
      },

      loadStorageStatus: async () => {
        if (get().storage.status !== "idle") return;
        set({ storage: { status: "loading" } });
        set({ storage: await resolveState(() => source.getStorageStatus()) });
      },

      setAddRepositoryOpen: (addRepositoryOpen) => set({ addRepositoryOpen }),
      openSettings: (section) => {
        const view = get().settingsView;
        const previous = view.status === "open" ? view.section : view.lastSection;
        if (view.status === "open" && (!section || section === previous)) return;
        set({ settingsView: { status: "open", section: section ?? previous } });
      },
      closeSettings: () => {
        const view = get().settingsView;
        if (view.status === "open") {
          set({ settingsView: { status: "closed", lastSection: view.section } });
        }
      },
      setRailFilter: (railFilter) => set({ railFilter }),

      pickRepositoryFolder: () => resolveState(() => source.pickRepositoryFolder()),

      addRepository: async (path) => {
        const result = await resolveState(() => source.addRepository(path));
        if (result.status !== "ok") return result;
        const repo = result.data;
        // Re-read the list rather than patching it: the core owns the order, and
        // adding an already-registered repository changes nothing.
        const listed = await resolveState(() => source.listRepositories());
        const known = get().repositories;
        const existing = known.status === "ok" ? known.data : [];
        // If re-reading fails, still show the repository we just added, next to the others.
        set({
          repositories:
            listed.status === "ok"
              ? listed
              : {
                  status: "ok",
                  data: existing.some((entry) => entry.id === repo.id)
                    ? existing
                    : [...existing, repo],
                },
        });
        worktreeQueries.invalidate([repo.id]);
        void loadSecurityOf(repo.id, true);
        await get().selectRepository(repo.id);
        notify(`Added ${repo.name}`, repo.rootPath);
        return result;
      },

      removeRepository: async (repo) => {
        const result = await resolveState(() => source.removeRepository(repo.id));
        if (result.status !== "ok") {
          notifyError(`Could not remove ${repo.name}`, result.error);
          return result;
        }
        const owned = get().worktrees.get(repo.id);
        if (owned?.status === "ok") {
          for (const wt of owned.data) {
            lastFileByWorktree.delete(wt.id);
            invalidateDiffs(wt.id);
          }
          changeQueries.invalidate(
            owned.data.flatMap((wt) => diffModes.map(({ mode }) => changesKey(wt.id, mode))),
          );
        }
        worktreeQueries.invalidate([repo.id]);
        securityQueries.invalidate([repo.id]);
        const current = get().repositories;
        const remaining =
          current.status === "ok" ? current.data.filter((entry) => entry.id !== repo.id) : [];
        set({ repositories: { status: "ok", data: remaining } });
        if (get().repoId === repo.id) {
          navigationVersion++;
          set({
            repoId: null,
            worktreeId: null,
            filePath: null,
            fileDiff: idle,
            flashes: new Map(),
          });
          const [next] = remaining;
          if (next) await get().selectRepository(next.id);
        }
        notify(`Removed ${repo.name}`, "Nothing on disk was changed.");
        return result;
      },

      setBase: async (worktree, baseRef) => {
        const result = await resolveState(() => source.setBase(worktree.id, baseRef));
        if (result.status !== "ok") return result;
        const updated = result.data;
        // A list read that began before the override was saved may still be in
        // flight with the OLD base; make it irrelevant before patching.
        worktreeQueries.supersede([updated.repoId]);
        invalidateDiffs(updated.id);
        patchWorktree(updated.repoId, updated);
        changeQueries.invalidate(diffModes.map(({ mode }) => changesKey(updated.id, mode)));
        if (get().worktreeId === updated.id) await syncChanges(updated);
        notify(
          updated.base && baseRef.trim()
            ? `Base set to ${updated.base.baseRef}`
            : "Using the detected base",
        );
        return result;
      },

      selectRepository: async (repoId) => {
        const state = get();
        if (
          state.repoId === repoId &&
          (selectWorktree(state) || selectWorktrees(state).status === "loading")
        )
          return;

        const version = ++navigationVersion;
        set({ repoId, worktreeId: null, filePath: null, fileDiff: idle, flashes: new Map() });
        const list = await loadWorktrees(repoId);
        if (version !== navigationVersion || list.status !== "ok") return;
        const initial = list.data.find((worktree) => worktree.base && worktree.changeCount > 0);
        const worktree = initial ?? list.data[0];
        if (worktree) await get().selectWorktree(worktree);
      },

      loadSecurity: async (repoId, refresh = false) => {
        await loadSecurityOf(repoId, refresh);
      },

      trustFilters: async (repo, fingerprint) => {
        const result = await resolveState(() =>
          source.trustRepositoryFilters(repo.id, fingerprint),
        );
        if (result.status !== "ok") {
          notifyError(`Could not trust ${repo.name}'s filter programs`, result.error);
          void loadSecurityOf(repo.id, true); // what the user saw is out of date
          return result;
        }
        securityQueries.supersede([repo.id]);
        set({ security: new Map(get().security).set(repo.id, result) });
        await rereadAfterTrustChange(repo.id);
        notify(
          `Trusted the filter programs of ${repo.name}`,
          "WHY will now run exactly these while reading it. You can revoke this from its menu.",
        );
        return result;
      },

      revokeFilters: async (repo) => {
        const result = await resolveState(() => source.revokeRepositoryFilters(repo.id));
        if (result.status !== "ok") {
          notifyError(`Could not revoke trust for ${repo.name}`, result.error);
          return result;
        }
        // `ok` means trust IS forgotten, whether or not the re-check worked, so
        // everything read under the old decision is stale either way.
        securityQueries.supersede([repo.id]);
        const { security, rescanError } = result.data;
        if (security)
          set({ security: new Map(get().security).set(repo.id, { status: "ok", data: security }) });
        else securityQueries.invalidate([repo.id]);
        await rereadAfterTrustChange(repo.id);
        if (rescanError) {
          notifyError(`Stopped trusting ${repo.name}, but could not re-check it`, rescanError);
        } else {
          notify(`Stopped trusting ${repo.name}'s filter programs`);
        }
        return result;
      },

      retryWorktrees: async (repoId) => {
        if (get().repoId === repoId) await get().selectRepository(repoId);
        else await loadWorktrees(repoId);
      },

      selectWorktree: async (worktree) => {
        const state = get();
        if (state.worktreeId === worktree.id && state.repoId === worktree.repoId) return;
        await navigateTo(worktree, null, state.preferredMode);
      },

      jumpToFile: async (worktree, path, availableModes) => {
        const state = get();
        const effective = modeForWorktree(state.preferredMode, worktree);
        const preferredMode = availableModes.includes(effective)
          ? state.preferredMode
          : availableModes[0];
        if (preferredMode) await navigateTo(worktree, path, preferredMode);
      },

      selectFile: (path) => loadFileDiff(path),

      moveWorktree: async (step) => {
        // In the order the rail shows them (main, then most recent), not the core's.
        const ordered = selectOrderedWorktrees(get());
        if (ordered.length === 0) return;
        const current = ordered.findIndex((worktree) => worktree.id === get().worktreeId);
        const next = ordered[moveIndex(ordered.length, current, step)];
        if (next) await get().selectWorktree(next);
      },

      moveFile: async (step) => {
        const list = selectChanges(get());
        if (list.status !== "ok" || list.data.length === 0) return;
        const current = list.data.findIndex((file) => file.path === get().filePath);
        const next = list.data[moveIndex(list.data.length, current, step)];
        if (next) await get().selectFile(next.path);
      },

      registerDiffNavigation: (diff, move) => {
        const navigation = { diff, move };
        diffNavigation = navigation;
        return () => {
          if (diffNavigation === navigation) diffNavigation = undefined;
        };
      },

      moveChange: async (direction) => {
        const state = get();
        const diff = state.fileDiff;
        if (diff.status === "loading" || (diff.status === "idle" && state.filePath !== null))
          return;
        if (diff.status === "ok" && hasContentDiff(diff.data)) {
          // The old editor can still be registered while React commits the new selection.
          if (diffNavigation?.diff !== diff.data || diffNavigation.move(direction) !== "boundary")
            return;
        }
        if (direction !== "next") return;
        const changes = selectChanges(state);
        if (changes.status !== "ok") return;
        const current = changes.data.findIndex((file) => file.path === state.filePath);
        const next = changes.data[current + 1];
        if (next) await loadFileDiff(next.path);
      },

      setDiffLayout: (diffLayout) => {
        set({ diffLayout });
        try {
          window.localStorage.setItem(diffLayoutKey, diffLayout);
        } catch {
          // Keep the selection for this session when storage is unavailable.
        }
      },
      toggleDiffLayout: () =>
        get().setDiffLayout(get().diffLayout === "split" ? "unified" : "split"),

      setDiffMode: async (preferredMode) => {
        const state = get();
        const worktree = selectWorktree(state);
        if (!worktree || (preferredMode === "vsBase" && !worktree.base)) return;
        const previousMode = selectDiffMode(state);
        set({ preferredMode });
        if (preferredMode !== previousMode) await syncChanges(worktree);
      },

      retryChanges: async () => {
        const worktree = selectWorktree(get());
        if (worktree) await syncChanges(worktree);
      },
      toggleInspector: () => set((state) => ({ inspectorOpen: !state.inspectorOpen })),
      showInspector: () => set({ inspectorOpen: true }),
      openSessionFile: async (path) => {
        const state = get();
        const worktree = selectWorktree(state);
        if (!worktree) return;
        const findPath = (result: AsyncState<FileChange[]>) =>
          result.status === "ok"
            ? result.data.find((file) => file.path === path || file.oldPath === path)?.path
            : undefined;
        const lookup = ++sessionFileVersion;
        const changes = selectChanges(state);
        const current = findPath(changes);
        if (current) {
          await state.selectFile(current);
          return;
        }

        // A session can mention a file changed in another mode, renamed since
        // the event, or no longer changed at all.
        const version = navigationVersion;
        const sessionId = sessions.getState().sessionId;
        const modes = diffModes
          .map(({ mode }) => mode)
          .filter(
            (mode) =>
              (mode !== selectDiffMode(state) || changes.status !== "ok") &&
              (mode !== "vsBase" || worktree.base),
          );
        const lists = await Promise.all(modes.map((mode) => loadChanges(worktree.id, mode)));
        if (
          get().worktreeId !== worktree.id ||
          version !== navigationVersion ||
          lookup !== sessionFileVersion ||
          sessions.getState().sessionId !== sessionId
        )
          return;
        for (const [index, mode] of modes.entries()) {
          const result = lists[index];
          const match = result && findPath(result);
          if (match) {
            await state.jumpToFile(worktree, match, [mode]);
            return;
          }
        }
        const failure = lists.find((result) => result.status === "error");
        if (failure?.status === "error") notifyError("Could not open file diff", failure.error);
        else notify("No current diff for this file", path);
      },
      setPaletteOpen: (paletteOpen) => set({ paletteOpen }),
      loadPaletteFiles: (worktree, mode) => loadChanges(worktree.id, mode),
      watchWorktrees: () => {
        const stopWorktrees = source.onWorktreeChanged(queueRefresh);
        const stopRepositories = source.onRepositoryChanged(queueRepository);
        const stopLineOrigins = lineOrigins.getState().watch();
        return () => {
          stopLineOrigins();
          stopWorktrees();
          stopRepositories();
          clearTimeout(flushTimer);
          flushTimer = undefined;
          queued.clear();
          queuedRepos.clear();
          for (const timer of pulseTimers.values()) clearTimeout(timer);
          pulseTimers.clear();
          if (get().pulses.size > 0) set({ pulses: new Map() });
        };
      },
    };
  });
}

export const workspaceStore = createWorkspaceStore(dataSource);
