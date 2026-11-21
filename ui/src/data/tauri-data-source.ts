import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ApplyResult,
  DiffMode,
  FileChange,
  FileDiff,
  HookHarness,
  HookStatusReport,
  LineOriginsResult,
  PreviewResult,
  RepoId,
  Repository,
  RepositoryChanged,
  RepositorySecurity,
  RationaleResult,
  RevokeResult,
  SessionId,
  SessionsChanged,
  SessionSummary,
  SessionTimeline,
  ShellOutcomesResult,
  StorageStatus,
  TurnRef,
  Worktree,
  WorktreeAttribution,
  WorktreeChanged,
  WorktreeId,
} from "@/contracts";
import {
  REPOSITORY_CHANGED_EVENT,
  SESSIONS_CHANGED_EVENT,
  WORKTREE_CHANGED_EVENT,
} from "@/contracts";
import { toIpcError } from "./async-state";
import type { DataSource } from "./data-source";

/**
 * The real backend: every method is one Tauri command.
 *
 * Command and argument names mirror the Rust signatures in `src-tauri/src/ipc`.
 * Tauri converts the Rust `snake_case` argument names to `camelCase` for
 * `invoke` (`repo_id` is `repoId`), which is why the keys below are camelCase
 * while the command names stay `snake_case`.
 */
export class TauriDataSource implements DataSource {
  /**
   * `invoke` rejects with whatever the Rust command returned as `Err`: for our
   * commands that is a serialized `IpcError` object, not an `Error`. Anything
   * else (the IPC layer itself failing) becomes `{ kind: "internal" }`.
   * Normalising here means callers only ever see `IpcError`.
   */
  private async call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    try {
      return await invoke<T>(command, args);
    } catch (error) {
      throw toIpcError(error);
    }
  }

  listRepositories() {
    return this.call<Repository[]>("list_repositories");
  }

  listWorktrees(repoId: RepoId) {
    return this.call<Worktree[]>("list_worktrees", { repoId });
  }

  getChanges(worktreeId: WorktreeId, mode: DiffMode) {
    return this.call<FileChange[]>("get_changes", { worktreeId, mode });
  }

  getFileDiff(worktreeId: WorktreeId, path: string, mode: DiffMode) {
    return this.call<FileDiff>("get_file_diff", { worktreeId, path, mode });
  }

  addRepository(path: string) {
    return this.call<Repository>("add_repository", { path });
  }

  removeRepository(id: RepoId) {
    return this.call<void>("remove_repository", { id });
  }

  setBase(worktreeId: WorktreeId, baseRef: string) {
    return this.call<Worktree>("set_base", { worktreeId, baseRef });
  }

  pickRepositoryFolder() {
    return this.call<string | null>("pick_repository_folder");
  }

  getStorageStatus() {
    return this.call<StorageStatus>("get_storage_status");
  }

  getRepositorySecurity(repoId: RepoId) {
    return this.call<RepositorySecurity>("get_repository_security", { repoId });
  }

  trustRepositoryFilters(repoId: RepoId, fingerprint: string) {
    return this.call<RepositorySecurity>("trust_repository_filters", { repoId, fingerprint });
  }

  revokeRepositoryFilters(repoId: RepoId) {
    return this.call<RevokeResult>("revoke_repository_filters", { repoId });
  }

  onWorktreeChanged(callback: (event: WorktreeChanged) => void) {
    return subscribe(WORKTREE_CHANGED_EVENT, callback);
  }

  onRepositoryChanged(callback: (event: RepositoryChanged) => void) {
    return subscribe(REPOSITORY_CHANGED_EVENT, callback);
  }

  listSessions(worktreeId: WorktreeId) {
    return this.call<SessionSummary[]>("list_sessions", { worktreeId });
  }

  getSessionTimeline(sessionId: SessionId) {
    return this.call<SessionTimeline>("get_session_timeline", { sessionId });
  }

  onSessionsChanged(callback: (event: SessionsChanged) => void) {
    return subscribe(SESSIONS_CHANGED_EVENT, callback);
  }
  previewInstall(harness: HookHarness) {
    return this.call<PreviewResult>("preview_install", { harness });
  }

  install(harness: HookHarness, fingerprint: string) {
    return this.call<ApplyResult>("install", { harness, fingerprint });
  }

  previewUninstall(harness: HookHarness) {
    return this.call<PreviewResult>("preview_uninstall", { harness });
  }

  uninstall(harness: HookHarness, fingerprint: string) {
    return this.call<ApplyResult>("uninstall", { harness, fingerprint });
  }

  hookStatus() {
    return this.call<HookStatusReport>("hook_status");
  }

  getWorktreeAttribution(worktreeId: WorktreeId, mode?: DiffMode) {
    return this.call<WorktreeAttribution>("get_worktree_attribution", { worktreeId, mode });
  }

  getFileActivity(worktreeId: WorktreeId, path: string) {
    return this.call<TurnRef[]>("get_file_activity", { worktreeId, path });
  }

  getTurnRationale(sessionId: SessionId, turnId: string) {
    return this.call<RationaleResult>("get_turn_rationale", { sessionId, turnId });
  }

  getToolRationale(sessionId: SessionId, eventId: string) {
    return this.call<RationaleResult>("get_tool_rationale", { sessionId, eventId });
  }

  getShellOutcomes(sessionId: SessionId) {
    return this.call<ShellOutcomesResult>("get_shell_outcomes", { sessionId });
  }

  getLineOrigins(worktreeId: WorktreeId, path: string) {
    return this.call<LineOriginsResult>("get_line_origins", { worktreeId, path });
  }
}

/**
 * `listen` is async (it registers with the Rust side and resolves to the
 * function that undoes the registration), but this must return its
 * unsubscribe function *synchronously*, because React effects clean up
 * synchronously. So there is a window where the caller can unsubscribe
 * before registration has finished (React StrictMode mounts, unmounts and
 * mounts again immediately). Handled with a flag: if we were cancelled while
 * waiting, undo the registration the moment it resolves instead of leaking
 * a listener that would deliver events to a dead subscriber forever.
 */
function subscribe<T>(eventName: string, callback: (payload: T) => void) {
  let cancelled = false;
  let unlisten: UnlistenFn | undefined;

  listen<T>(eventName, (event) => {
    if (!cancelled) callback(event.payload);
  }).then(
    (stop) => {
      if (cancelled) stop();
      else unlisten = stop;
    },
    (error: unknown) => {
      // Not fatal: the UI still works, it just stops updating on its own.
      console.error(`cannot listen for ${eventName}`, error);
    },
  );

  return () => {
    cancelled = true;
    unlisten?.();
    unlisten = undefined;
  };
}
