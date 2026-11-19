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

export interface DataSource {
  listRepositories(): Promise<Repository[]>;
  listWorktrees(repoId: RepoId): Promise<Worktree[]>;
  getChanges(worktreeId: WorktreeId, mode: DiffMode): Promise<FileChange[]>;
  getFileDiff(worktreeId: WorktreeId, path: string, mode: DiffMode): Promise<FileDiff>;
  onWorktreeChanged(callback: (event: WorktreeChanged) => void): () => void;
}
