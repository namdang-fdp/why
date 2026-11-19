import type { DiffMode, FileChange, FileDiff, IpcError, WorktreeChanged } from "@/contracts";
import { supplementaryDiffs } from "@/dev-fixtures/file-diffs";
import { changesByWorktree, fileDiffs, repositories, worktrees } from "@/fixtures";
import type { DataSource } from "./data-source";

const changes: Readonly<Partial<Record<string, FileChange[]>>> = changesByWorktree;
const extraDiffs: Readonly<Partial<Record<string, FileDiff[]>>> = supplementaryDiffs;

export class FixtureDataSource implements DataSource {
  private readonly listeners = new Set<(event: WorktreeChanged) => void>();

  constructor(private readonly latencyMs = 0) {}

  private async delay() {
    if (this.latencyMs > 0) {
      await new Promise<void>((resolve) => setTimeout(resolve, this.latencyMs));
    }
  }

  async listRepositories() {
    await this.delay();
    return structuredClone(repositories);
  }

  async listWorktrees(repoId: string) {
    await this.delay();
    if (!repositories.some((repo) => repo.id === repoId)) {
      throw { kind: "notFound" } satisfies IpcError;
    }
    return structuredClone(worktrees.filter((worktree) => worktree.repoId === repoId));
  }

  private changesForMode(worktreeId: string, mode: DiffMode) {
    const worktree = worktrees.find((entry) => entry.id === worktreeId);
    if (!worktree) throw { kind: "notFound" } satisfies IpcError;
    if (mode === "vsBase" && !worktree.base) {
      throw { kind: "invalidInput", message: "This worktree has no base ref." } satisfies IpcError;
    }

    // The snapshots are shared across modes. The staging flags describe membership.
    const files = changes[worktreeId] ?? [];
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
    const diff =
      extraDiffs[worktreeId]?.find((entry) => entry.path === path) ??
      fileDiffs.find((entry) => entry.path === path);
    if (!diff) throw { kind: "notFound" } satisfies IpcError;
    return structuredClone(diff);
  }

  onWorktreeChanged(callback: (event: WorktreeChanged) => void) {
    this.listeners.add(callback);
    return () => {
      this.listeners.delete(callback);
    };
  }

  emitWorktreeChanged(event: WorktreeChanged) {
    // Vitest's test mode also supplies DEV; production builds do not emit.
    if (!import.meta.env.DEV) return;
    for (const callback of this.listeners) callback(event);
  }
}
