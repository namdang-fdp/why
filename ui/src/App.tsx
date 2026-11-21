import { useCallback, useEffect, useState } from "react";
import { AddRepositoryDialog } from "@/components/shell/add-repository-dialog";
import { CommandPalette } from "@/components/shell/command-palette";
import { StorageBanner } from "@/components/shell/storage-banner";
import { Toaster } from "@/components/shell/toaster";
import { TooltipProvider } from "@/components/ui/overlays";
import { ShortcutHelp } from "@/components/shell/shortcut-help";
import { TopBar } from "@/components/shell/top-bar";
import { WorkspacePanels } from "@/components/shell/workspace-panels";
import { useWorkspaceShortcuts } from "@/hooks/use-workspace-shortcuts";
import {
  selectActiveFile,
  selectDiffMode,
  selectWorktree,
  workspaceStore,
} from "@/stores/workspace";
import { WorkspaceContext, type Workspace } from "@/stores/workspace-context";

export function App({ store = workspaceStore }: { store?: Workspace }) {
  const [helpOpen, setHelpOpen] = useState(false);
  const openHelp = useCallback(() => setHelpOpen(true), []);
  useWorkspaceShortcuts(store, helpOpen, openHelp);

  useEffect(() => {
    const workspace = store.getState();
    const unsubscribe = workspace.watchWorktrees();
    const sessions = workspace.sessions.getState();
    const stopSessions = sessions.watch();
    const syncSessions = (worktreeId: string | null) => void sessions.selectWorktree(worktreeId);
    const stopSync = store.subscribe((state, previous) => {
      if (state.worktreeId !== previous.worktreeId) syncSessions(state.worktreeId);
    });
    syncSessions(workspace.worktreeId);

    // Evidence follows the selection: worktree + comparison mode for attribution,
    // the selected file for its edit history, the open session for exit codes.
    const evidence = workspace.evidence.getState();
    const stopEvidence = evidence.watch();
    // What the changed-files list was computed from, beyond worktree and mode: a new
    // base ref or a filter-trust decision changes the list in place, so attribution
    // (which is computed from the same list) must be asked again.
    let inputs = "";
    const syncEvidence = (state: ReturnType<Workspace["getState"]>) => {
      const worktree = selectWorktree(state);
      const security = worktree ? state.security.get(worktree.repoId) : undefined;
      const next = JSON.stringify([
        state.worktreeId,
        selectDiffMode(state),
        worktree?.base?.baseRef ?? null,
        security?.status === "ok" ? [security.data.status, security.data.fingerprint] : null,
      ]);
      const changed = inputs !== "" && next !== inputs;
      inputs = next;
      const mode = selectDiffMode(state);
      const scope = workspace.evidence.getState().scope;
      // A new worktree or mode already refetches inside `setScope`; only an in-place change needs the nudge.
      const sameScope = scope?.worktreeId === state.worktreeId && scope.mode === mode;
      void evidence.setScope(state.worktreeId, mode);
      if (changed && sameScope) void evidence.loadAttribution();
      // A file whose name is not valid UTF-8 shares its display string with other
      // files: never look its history up (the core leaves such files unexplained).
      const file = selectActiveFile(state);
      void evidence.setFile(file && !file.pathLossy ? file.path : null, file?.oldPath ?? null);
    };
    const stopEvidenceSync = store.subscribe(syncEvidence);
    syncEvidence(store.getState());
    const syncSession = (state: ReturnType<typeof workspace.sessions.getState>) =>
      void evidence.setSession(
        state.timeline.status === "ok"
          ? { id: state.timeline.data.session.id, harness: state.timeline.data.session.harness }
          : null,
      );
    const stopSessionSync = workspace.sessions.subscribe(syncSession);
    void workspace.loadStorageStatus();
    void workspace.loadRepositories();
    return () => {
      unsubscribe();
      stopEvidenceSync();
      stopSessionSync();
      stopEvidence();
      void evidence.setSession(null);
      void evidence.setScope(null, "vsBase");
      stopSync();
      stopSessions();
      syncSessions(null);
    };
  }, [store]);

  return (
    <WorkspaceContext value={store}>
      <TooltipProvider>
        <div className="app-shell">
          <TopBar onHelp={openHelp} />
          <StorageBanner />
          <WorkspacePanels />
          <CommandPalette />
          <AddRepositoryDialog />
          <ShortcutHelp open={helpOpen} onOpenChange={setHelpOpen} />
          <Toaster />
        </div>
      </TooltipProvider>
    </WorkspaceContext>
  );
}
