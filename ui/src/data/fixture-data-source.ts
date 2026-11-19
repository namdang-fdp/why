import type {
  BranchInfo,
  DiffMode,
  FileChange,
  FileDiff,
  IpcError,
  RepoId,
  Repository,
  RepositoryChanged,
  StorageStatus,
  Worktree,
  WorktreeChanged,
  WorktreeId,
} from "@/contracts";
import { supplementaryDiffs } from "@/dev-fixtures/file-diffs";
import { buildManyWorktrees, manyDiff } from "@/dev-fixtures/many-worktrees";
import {
  changesByWorktree,
  fileDiffs,
  repositories,
  storagePersistent,
  WT_WHY_MAIN,
  worktrees,
} from "@/fixtures";
import type { DataSource } from "./data-source";

const extraDiffs: Readonly<Partial<Record<string, FileDiff[]>>> = supplementaryDiffs;

// The sample every added repository is modelled on: a main checkout with two changes.
function findSampleMain(): Worktree {
  const found = worktrees.find((entry) => entry.id === WT_WHY_MAIN);
  if (!found) throw new Error("fixtures: the sample main worktree is missing");
  return found;
}
const sampleMain = findSampleMain();

/** Refs `setBase` accepts, so the base popover has something real to validate. */
const knownRefs = ["main", "origin/main", "develop", "origin/develop"];

export type FixtureOptions = {
  /** Artificial delay for every call, to look at loading states. */
  latencyMs?: number;
  /** Start with no repositories (the onboarding screen). */
  empty?: boolean;
  /** Add a crowded repository (12 worktrees, duplicates, detached, broken) to judge the rail. */
  many?: boolean;
  storage?: StorageStatus;
  /** What the "native" folder picker returns; `null` simulates cancelling. */
  pick?: string | null;
};

/**
 * An in-memory backend that behaves like the Rust core closely enough to design
 * and test the UI against: repositories can be added and removed, bases set,
 * and `simulateEdit` plays the role of the file watcher. Each instance owns its
 * state, so tests never share it.
 */
export class FixtureDataSource implements DataSource {
  private readonly listeners = new Set<(event: WorktreeChanged) => void>();
  private readonly repoListeners = new Set<(event: RepositoryChanged) => void>();
  private repos: Repository[];
  private worktreeList: Worktree[];
  private readonly changes: Record<string, FileChange[]>;
  private readonly editedDiffs = new Map<string, FileDiff>();
  private readonly options: FixtureOptions;
  private edits = 0;

  constructor(options: FixtureOptions = {}) {
    this.options = options;
    this.repos = options.empty ? [] : structuredClone(repositories);
    this.worktreeList = options.empty ? [] : structuredClone(worktrees);
    this.changes = structuredClone(changesByWorktree);
    if (options.many) {
      const crowd = buildManyWorktrees(Math.floor(Date.now() / 1000));
      this.repos = [...this.repos, crowd.repository].sort((a, b) => a.name.localeCompare(b.name));
      this.worktreeList = [...this.worktreeList, ...crowd.worktrees];
      Object.assign(this.changes, crowd.changes);
    }
  }

  private async delay() {
    const ms = this.options.latencyMs ?? 0;
    if (ms > 0) await new Promise<void>((resolve) => setTimeout(resolve, ms));
  }

  async listRepositories() {
    await this.delay();
    return structuredClone(this.repos);
  }

  async listWorktrees(repoId: string) {
    await this.delay();
    if (!this.repos.some((repo) => repo.id === repoId)) {
      throw { kind: "notFound" } satisfies IpcError;
    }
    return structuredClone(this.worktreeList.filter((worktree) => worktree.repoId === repoId));
  }

  private worktree(id: string) {
    const worktree = this.worktreeList.find((entry) => entry.id === id);
    if (!worktree) throw { kind: "notFound" } satisfies IpcError;
    return worktree;
  }

  private changesForMode(worktreeId: string, mode: DiffMode) {
    const worktree = this.worktree(worktreeId);
    // The real core runs git in the worktree, which fails the same way.
    if (worktree.loadError) throw worktree.loadError;
    if (mode === "vsBase" && !worktree.base) {
      throw { kind: "invalidInput", message: "This worktree has no base ref." } satisfies IpcError;
    }

    // The snapshots are shared across modes. The staging flags describe membership.
    const files = this.changes[worktreeId] ?? [];
    if (mode === "staged") return files.filter((file) => file.staged);
    if (mode === "uncommitted") return files.filter((file) => file.staged || file.unstaged);
    return files;
  }

  async getChanges(worktreeId: string, mode: DiffMode) {
    await this.delay();
    return structuredClone(this.changesForMode(worktreeId, mode));
  }

  async getFileDiff(worktreeId: string, path: string, mode: DiffMode) {
    await this.delay();
    if (!this.changesForMode(worktreeId, mode).some((file) => file.path === path)) {
      throw { kind: "notFound" } satisfies IpcError;
    }
    const file = this.changesForMode(worktreeId, mode).find((entry) => entry.path === path);
    const diff =
      this.editedDiffs.get(`${worktreeId}:${path}`) ??
      (worktreeId.startsWith("wt-aip-") && file ? manyDiff(path, file.status) : undefined) ??
      extraDiffs[worktreeId]?.find((entry) => entry.path === path) ??
      fileDiffs.find((entry) => entry.path === path);
    if (!diff) throw { kind: "notFound" } satisfies IpcError;
    return structuredClone(diff);
  }

  /**
   * Mirrors `add_repository`'s failure modes, chosen by words in the path so
   * every error state can be provoked by hand: "not-a-repo", "bare", "no-git",
   * "missing". Anything else registers a sample repository named after the
   * folder.
   */
  async addRepository(path: string) {
    await this.delay();
    const text = path.trim();
    if (!text) throw { kind: "invalidInput", message: "path is empty" } satisfies IpcError;
    if (text.includes("missing")) {
      throw {
        kind: "invalidInput",
        message: `cannot open "${text}": No such file or directory (os error 2)`,
      } satisfies IpcError;
    }
    if (text.includes("no-git")) throw { kind: "gitMissing" } satisfies IpcError;
    if (text.includes("not-a-repo")) throw { kind: "notARepo" } satisfies IpcError;
    if (text.includes("bare")) {
      throw {
        kind: "invalidInput",
        message: "bare repositories are not supported",
      } satisfies IpcError;
    }

    // Exactly the path given (only trailing slashes are not part of a name).
    const root = path.replace(/\/+$/, "");
    const existing = this.repos.find((repo) => repo.rootPath === root);
    if (existing) return structuredClone(existing);

    const name = root.split("/").filter(Boolean).at(-1) ?? root;
    const repo: Repository = { id: `repo-${name}-${this.repos.length}`, name, rootPath: root };
    const main: Worktree = {
      ...structuredClone(sampleMain),
      id: `wt-${repo.id}-main`,
      repoId: repo.id,
      path: root,
    };
    this.changes[main.id] = structuredClone(changesByWorktree[WT_WHY_MAIN]);
    this.repos = [...this.repos, repo].sort((a, b) => a.name.localeCompare(b.name));
    this.worktreeList = [...this.worktreeList, main];
    return structuredClone(repo);
  }

  async removeRepository(id: RepoId) {
    await this.delay();
    if (!this.repos.some((repo) => repo.id === id)) {
      throw { kind: "notFound" } satisfies IpcError;
    }
    this.repos = this.repos.filter((repo) => repo.id !== id);
    this.worktreeList = this.worktreeList.filter((worktree) => worktree.repoId !== id);
  }

  async setBase(worktreeId: WorktreeId, baseRef: string) {
    await this.delay();
    const worktree = this.worktree(worktreeId);
    const wanted = baseRef.trim();
    if (!wanted) {
      // Back to auto-detection: the fixture's own base (or none).
      const detected = worktrees.find((entry) => entry.id === worktreeId)?.base ?? null;
      worktree.base = structuredClone(detected);
    } else {
      if (!knownRefs.includes(wanted)) {
        throw { kind: "invalidInput", message: `\`${wanted}\` is not a commit` } satisfies IpcError;
      }
      if (wanted === worktree.branch) {
        throw {
          kind: "invalidInput",
          message: `\`${wanted}\` is this worktree's own branch`,
        } satisfies IpcError;
      }
      const base: BranchInfo = {
        baseRef: wanted,
        mergeBase: "9f3a6e2d1c8b4057a6e9d3f2c1b80475e6a9d3c2",
        ahead: worktree.base?.ahead ?? 1,
        behind: worktree.base?.behind ?? 0,
      };
      worktree.base = base;
    }
    return structuredClone(worktree);
  }

  async pickRepositoryFolder() {
    await this.delay();
    return this.options.pick === undefined ? "/home/dev/code/sample-project" : this.options.pick;
  }

  async getStorageStatus() {
    await this.delay();
    return structuredClone(this.options.storage ?? storagePersistent);
  }

  onWorktreeChanged(callback: (event: WorktreeChanged) => void) {
    this.listeners.add(callback);
    return () => {
      this.listeners.delete(callback);
    };
  }

  onRepositoryChanged(callback: (event: RepositoryChanged) => void) {
    this.repoListeners.add(callback);
    return () => {
      this.repoListeners.delete(callback);
    };
  }

  emitRepositoryChanged(event: RepositoryChanged) {
    if (!import.meta.env.DEV) return;
    for (const callback of this.repoListeners) callback(event);
  }

  /**
   * Plays an agent running `git worktree add`: a new linked worktree appears in
   * `repoId` and the core announces it with `repository-changed`.
   */
  simulateWorktreeAdded(repoId: RepoId, name = `agent-${this.worktreeList.length + 1}`) {
    const repo = this.repos.find((entry) => entry.id === repoId);
    if (!repo) return undefined;
    const worktree: Worktree = {
      ...structuredClone(sampleMain),
      id: `wt-${repoId}-${name}`,
      repoId,
      path: `${repo.rootPath}-worktrees/${name}`,
      branch: `agent/${name}`,
      isMain: false,
      base: {
        baseRef: "origin/main",
        mergeBase: "9f3a6e2d1c8b4057a6e9d3f2c1b80475e6a9d3c2",
        ahead: 1,
        behind: 0,
      },
      changeCount: 2,
    };
    this.changes[worktree.id] = structuredClone(changesByWorktree[WT_WHY_MAIN]);
    this.worktreeList = [...this.worktreeList, worktree];
    this.emitRepositoryChanged({ repoId });
    return worktree;
  }

  /** Plays `git worktree remove` (or the directory vanishing). */
  simulateWorktreeRemoved(worktreeId: WorktreeId) {
    const worktree = this.worktreeList.find((entry) => entry.id === worktreeId);
    if (!worktree) return;
    this.worktreeList = this.worktreeList.filter((entry) => entry.id !== worktreeId);
    this.emitRepositoryChanged({ repoId: worktree.repoId });
  }

  emitWorktreeChanged(event: WorktreeChanged) {
    // Vitest's test mode also supplies DEV; production builds do not emit.
    if (!import.meta.env.DEV) return;
    for (const callback of this.listeners) callback(event);
  }

  /**
   * Plays the file watcher for design work: "edits" `path` (one more added
   * line, new content appended) and emits the event the real core would.
   */
  simulateEdit(worktreeId: WorktreeId, path: string) {
    const file = this.changes[worktreeId]?.find((entry) => entry.path === path);
    if (!file) return;
    this.edits += 1;
    file.additions = (file.additions ?? 0) + 1;
    const key = `${worktreeId}:${path}`;
    const base =
      this.editedDiffs.get(key) ??
      extraDiffs[worktreeId]?.find((entry) => entry.path === path) ??
      fileDiffs.find((entry) => entry.path === path);
    if (base && base.newContent !== null) {
      this.editedDiffs.set(key, {
        ...base,
        newContent: `${base.newContent}// live edit ${this.edits}\n`,
      });
    }
    this.emitWorktreeChanged({ worktreeId, paths: [path] });
  }
}
