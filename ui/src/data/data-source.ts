import type {
  DiffMode,
  FileChange,
  FileDiff,
  RepoId,
  Repository,
  RepositoryChanged,
  StorageStatus,
  Worktree,
  WorktreeChanged,
  WorktreeId,
} from "@/contracts";

/**
 * Everything the UI needs from the backend. Components never reach past this
 * seam: the real implementation talks to the Rust core, the fixture one feeds
 * the UI in a plain browser (`pnpm dev:ui`) and in tests.
 *
 * Every method rejects with an `IpcError`-shaped value; callers normalise with
 * `toIpcError` / `resolveState`.
 */
export interface DataSource {
  listRepositories(): Promise<Repository[]>;
  listWorktrees(repoId: RepoId): Promise<Worktree[]>;
  getChanges(worktreeId: WorktreeId, mode: DiffMode): Promise<FileChange[]>;
  getFileDiff(worktreeId: WorktreeId, path: string, mode: DiffMode): Promise<FileDiff>;
  /** Register the repository containing `path` (idempotent). */
  addRepository(path: string): Promise<Repository>;
  /** Unregister a repository. Never touches the repository on disk. */
  removeRepository(id: RepoId): Promise<void>;
  /** Set the base ref a worktree is compared against; "" clears the override. */
  setBase(worktreeId: WorktreeId, baseRef: string): Promise<Worktree>;
  /** The native folder picker. `null` means the user cancelled. */
  pickRepositoryFolder(): Promise<string | null>;
  getStorageStatus(): Promise<StorageStatus>;
  /** Subscribe to file-system changes; returns the function that unsubscribes. */
  onWorktreeChanged(callback: (event: WorktreeChanged) => void): () => void;
  /** Subscribe to a repository gaining or losing worktrees; returns the unsubscribe. */
  onRepositoryChanged(callback: (event: RepositoryChanged) => void): () => void;
}
