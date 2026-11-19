import { createStore } from "zustand/vanilla";
import type {
  DiffMode,
  FileChange,
  FileDiff,
  RepoId,
  Repository,
  Worktree,
  WorktreeChanged,
  WorktreeId,
} from "@/contracts";
import { dataSource } from "@/data";
import { idle, resolveState, type AsyncState } from "@/data/async-state";
import type { DataSource } from "@/data/data-source";

type Queries<T> = ReadonlyMap<string, AsyncState<T>>;

type WorkspaceStore = {
  repositories: AsyncState<Repository[]>;
  worktrees: Queries<Worktree[]>;
  changes: Queries<FileChange[]>;
  fileDiff: AsyncState<FileDiff>;
  repoId: RepoId | null;
  worktreeId: WorktreeId | null;
  filePath: string | null;
  preferredMode: DiffMode;
  inspectorOpen: boolean;
  paletteOpen: boolean;
  loadRepositories: () => Promise<void>;
  retryWorktrees: (repoId: RepoId) => Promise<void>;
  selectRepository: (repoId: RepoId) => Promise<void>;
  selectWorktree: (worktree: Worktree) => Promise<void>;
  jumpToFile: (worktree: Worktree, path: string, availableModes: DiffMode[]) => Promise<void>;
  selectFile: (path: string) => Promise<void>;
  moveWorktree: (step: 1 | -1) => Promise<void>;
  moveFile: (step: 1 | -1) => Promise<void>;
  setDiffMode: (mode: DiffMode) => Promise<void>;
  retryChanges: () => Promise<void>;
  toggleInspector: () => void;
  setPaletteOpen: (open: boolean) => void;
  loadPaletteFiles: (worktree: Worktree, mode: DiffMode) => Promise<AsyncState<FileChange[]>>;
  watchWorktrees: () => () => void;
};

export const selectWorktrees = (state: WorkspaceStore) =>
  (state.repoId !== null ? state.worktrees.get(state.repoId) : undefined) ?? idle;

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
function createQueryLoader<T>(read: () => Queries<T>, write: (queries: Queries<T>) => void) {
  const pending = new Map<string, Promise<AsyncState<T>>>();

  return {
    load(key: string, request: () => Promise<T>, refresh = false): Promise<AsyncState<T>> {
      const inFlight = pending.get(key);
      if (inFlight && !refresh) return inFlight;
      const cached = read().get(key);
      if (cached?.status === "ok" && !refresh) return Promise.resolve(cached);

      const result = resolveState(request).then((state) => {
        if (pending.get(key) === result) {
          pending.delete(key);
          write(new Map(read()).set(key, state));
        }
        return state;
      });
      pending.set(key, result);
      if (cached?.status !== "ok") write(new Map(read()).set(key, { status: "loading" }));
      return result;
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

export const diffModes = [
  { mode: "vsBase", label: "Vs base", key: "1" },
  { mode: "uncommitted", label: "Uncommitted", key: "2" },
  { mode: "staged", label: "Staged", key: "3" },
] as const satisfies ReadonlyArray<{ mode: DiffMode; label: string; key: string }>;

export function createWorkspaceStore(source: DataSource) {
  return createStore<WorkspaceStore>((set, get) => {
    const worktreeQueries = createQueryLoader(
      () => get().worktrees,
      (worktrees) => set({ worktrees }),
    );
    const changeQueries = createQueryLoader(
      () => get().changes,
      (changes) => set({ changes }),
    );
    const loadWorktrees = (id: RepoId, refresh = false) =>
      worktreeQueries.load(id, () => source.listWorktrees(id), refresh);
    const loadChanges = (id: WorktreeId, mode: DiffMode) =>
      changeQueries.load(changesKey(id, mode), () => source.getChanges(id, mode));

    let navigationVersion = 0;

    async function syncChanges(worktree: Worktree) {
      const version = ++navigationVersion;
      const mode = modeForWorktree(get().preferredMode, worktree);
      set({ fileDiff: idle });
      const changes = await loadChanges(worktree.id, mode);
      if (version !== navigationVersion || changes.status !== "ok") return;
      const path = get().filePath;
      if (path !== null && changes.data.some((file) => file.path === path)) {
        await get().selectFile(path);
      } else {
        set({ filePath: null });
      }
    }

    async function navigateTo(worktree: Worktree, path: string | null, preferredMode: DiffMode) {
      set({
        repoId: worktree.repoId,
        worktreeId: worktree.id,
        filePath: path,
        fileDiff: idle,
        preferredMode,
      });
      await syncChanges(worktree);
    }

    async function refreshWorktree({ worktreeId }: WorktreeChanged) {
      const owner = Array.from(get().worktrees).find(
        ([, list]) => list.status === "ok" && list.data.some((wt) => wt.id === worktreeId),
      )?.[0];
      if (owner === undefined) return;

      // Invalidate changes immediately, then use refreshed metadata to pick the mode.
      if (get().worktreeId === worktreeId) {
        navigationVersion++;
        set({ fileDiff: idle });
      }
      changeQueries.invalidate(diffModes.map(({ mode }) => changesKey(worktreeId, mode)));
      const previousWorktree = selectWorktree(get());
      const list = await loadWorktrees(owner, true);
      const state = get();
      if (state.worktrees.get(owner) !== list || state.repoId !== owner || list.status !== "ok") {
        return;
      }

      const current = selectWorktree(state);
      if (current) {
        const mode = selectDiffMode(state);
        const previousMode = modeForWorktree(state.preferredMode, previousWorktree);
        if (
          current.id === worktreeId ||
          mode !== previousMode ||
          selectChanges(state).status === "idle"
        ) {
          await syncChanges(current);
        }
      } else {
        const first = list.data[0];
        if (first) await get().selectWorktree(first);
        else {
          navigationVersion++;
          set({ worktreeId: null, filePath: null, fileDiff: idle });
        }
      }
    }

    return {
      repositories: idle,
      worktrees: new Map(),
      changes: new Map(),
      fileDiff: idle,
      repoId: null,
      worktreeId: null,
      filePath: null,
      preferredMode: "vsBase",
      inspectorOpen: true,
      paletteOpen: false,

      loadRepositories: async () => {
        const { status } = get().repositories;
        if (status === "loading" || status === "ok") return;
        set({ repositories: { status: "loading" } });
        const repositories = await resolveState(() => source.listRepositories());
        set({ repositories });
        if (repositories.status !== "ok") return;

        const [first, ...otherRepos] = repositories.data;
        for (const repo of otherRepos) void loadWorktrees(repo.id);
        if (first) await get().selectRepository(first.id);
      },

      selectRepository: async (repoId) => {
        const state = get();
        if (
          state.repoId === repoId &&
          (selectWorktree(state) || selectWorktrees(state).status === "loading")
        )
          return;

        const version = ++navigationVersion;
        set({ repoId, worktreeId: null, filePath: null, fileDiff: idle });
        const list = await loadWorktrees(repoId);
        if (version !== navigationVersion || list.status !== "ok") return;
        const initial = list.data.find((worktree) => worktree.base && worktree.changeCount > 0);
        const worktree = initial ?? list.data[0];
        if (worktree) await get().selectWorktree(worktree);
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

      selectFile: async (path) => {
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
        const version = ++navigationVersion;
        set({ filePath: path, fileDiff: { status: "loading" } });
        const fileDiff = await resolveState(() => source.getFileDiff(worktreeId, path, mode));
        if (version === navigationVersion) set({ fileDiff });
      },

      moveWorktree: async (step) => {
        const list = selectWorktrees(get());
        if (list.status !== "ok" || list.data.length === 0) return;
        const current = list.data.findIndex((worktree) => worktree.id === get().worktreeId);
        const next = list.data[moveIndex(list.data.length, current, step)];
        if (next) await get().selectWorktree(next);
      },

      moveFile: async (step) => {
        const list = selectChanges(get());
        if (list.status !== "ok" || list.data.length === 0) return;
        const current = list.data.findIndex((file) => file.path === get().filePath);
        const next = list.data[moveIndex(list.data.length, current, step)];
        if (next) await get().selectFile(next.path);
      },

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
      setPaletteOpen: (paletteOpen) => set({ paletteOpen }),
      loadPaletteFiles: (worktree, mode) => loadChanges(worktree.id, mode),
      watchWorktrees: () => source.onWorktreeChanged((event) => void refreshWorktree(event)),
    };
  });
}

export const workspaceStore = createWorkspaceStore(dataSource);
