import type {
  ApplyResult,
  DiffMode,
  FileChange,
  FileDiff,
  HookHarness,
  HookStatusReport,
  PreviewResult,
  RepoId,
  Repository,
  RepositoryChanged,
  RepositorySecurity,
  RevokeResult,
  SessionId,
  SessionsChanged,
  SessionSummary,
  SessionTimeline,
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
  /** The git filter programs a repository configures, and whether WHY may read it (see SECURITY.md). */
  getRepositorySecurity(repoId: RepoId): Promise<RepositorySecurity>;
  /** Trust the filter programs the user was shown; `fingerprint` must still match what the core finds. */
  trustRepositoryFilters(repoId: RepoId, fingerprint: string): Promise<RepositorySecurity>;
  /**
   * Forget a trust decision. Resolving means trust IS forgotten; the fresh report
   * is missing (with `rescanError` set) when re-checking the repository failed.
   */
  revokeRepositoryFilters(repoId: RepoId): Promise<RevokeResult>;
  /** Subscribe to file-system changes; returns the function that unsubscribes. */
  onWorktreeChanged(callback: (event: WorktreeChanged) => void): () => void;
  /** Subscribe to a repository gaining or losing worktrees; returns the unsubscribe. */
  onRepositoryChanged(callback: (event: RepositoryChanged) => void): () => void;
  /** Agent sessions seen in a worktree, most recently active first (empty when none). */
  listSessions(worktreeId: WorktreeId): Promise<SessionSummary[]>;
  /** One session's turns and events; rejects with `notFound` for an unknown id. */
  getSessionTimeline(sessionId: SessionId): Promise<SessionTimeline>;
  /** Subscribe to new hook events for a worktree's sessions; returns the unsubscribe. */
  onSessionsChanged(callback: (event: SessionsChanged) => void): () => void;
  /** Read-only preview of the exact user-level config change. */
  previewInstall(harness: HookHarness): Promise<PreviewResult>;
  install(harness: HookHarness, fingerprint: string): Promise<ApplyResult>;
  previewUninstall(harness: HookHarness): Promise<PreviewResult>;
  uninstall(harness: HookHarness, fingerprint: string): Promise<ApplyResult>;
  hookStatus(): Promise<HookStatusReport>;
}
