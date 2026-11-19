import type { DiffMode, FileDiff, WorktreeId } from "@/contracts";

type Entry = { worktreeId: WorktreeId; path: string; diff: FileDiff; size: number };

/**
 * An LRU cache of file diffs, so switching back to (or onto a prefetched) file
 * paints at once instead of waiting for git.
 *
 * Keyed by worktree + mode + path: the same file reads differently in each mode.
 * Bounded by entry count AND by total text size (one diff can carry two 1 MiB
 * sides), evicting the least recently used first.
 *
 * Correctness is about invalidation, not capacity: a cached diff must never
 * outlive a change to its file. The store drops entries when the watcher
 * reports paths (or "everything") and when a worktree's base changes.
 * `epoch(worktreeId)` is bumped by every invalidation so a read that STARTED
 * before the invalidation cannot write its (possibly stale) answer afterwards.
 */
export function createDiffCache(maxEntries = 48, maxChars = 12_000_000) {
  const entries = new Map<string, Entry>(); // insertion order = recency
  const epochs = new Map<WorktreeId, number>();
  let chars = 0;

  const key = (worktreeId: WorktreeId, mode: DiffMode, path: string) =>
    JSON.stringify([worktreeId, mode, path]);

  function remove(entryKey: string) {
    const entry = entries.get(entryKey);
    if (!entry) return;
    chars -= entry.size;
    entries.delete(entryKey);
  }

  function bump(worktreeId: WorktreeId) {
    epochs.set(worktreeId, (epochs.get(worktreeId) ?? 0) + 1);
  }

  return {
    key,
    /** Read an entry and mark it most recently used. */
    get(worktreeId: WorktreeId, mode: DiffMode, path: string) {
      const entryKey = key(worktreeId, mode, path);
      const entry = entries.get(entryKey);
      if (!entry) return undefined;
      entries.delete(entryKey);
      entries.set(entryKey, entry);
      return entry.diff;
    },
    has(worktreeId: WorktreeId, mode: DiffMode, path: string) {
      return entries.has(key(worktreeId, mode, path));
    },
    /** Token for a read that is about to start; pass it back to `set`. */
    epoch(worktreeId: WorktreeId) {
      return epochs.get(worktreeId) ?? 0;
    },
    /** Store `diff` unless its worktree was invalidated since `epoch` was taken. */
    set(worktreeId: WorktreeId, mode: DiffMode, path: string, diff: FileDiff, epoch: number) {
      if (epoch !== (epochs.get(worktreeId) ?? 0)) return;
      const entryKey = key(worktreeId, mode, path);
      remove(entryKey);
      const size = (diff.oldContent?.length ?? 0) + (diff.newContent?.length ?? 0);
      if (size > maxChars) return; // never worth evicting everything else for
      entries.set(entryKey, { worktreeId, path, diff, size });
      chars += size;
      while (entries.size > maxEntries || chars > maxChars) {
        const oldest = entries.keys().next();
        if (oldest.done) break;
        remove(oldest.value);
      }
    },
    /** These files changed (in any mode). */
    invalidatePaths(worktreeId: WorktreeId, paths: Iterable<string>) {
      bump(worktreeId);
      const changed = new Set(paths);
      for (const [entryKey, entry] of entries) {
        if (entry.worktreeId === worktreeId && changed.has(entry.path)) remove(entryKey);
      }
    },
    /** Anything in this worktree may have changed. */
    invalidateWorktree(worktreeId: WorktreeId) {
      bump(worktreeId);
      for (const [entryKey, entry] of entries) {
        if (entry.worktreeId === worktreeId) remove(entryKey);
      }
    },
    get size() {
      return entries.size;
    },
  };
}

export type DiffCache = ReturnType<typeof createDiffCache>;
