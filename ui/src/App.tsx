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
import { workspaceStore } from "@/stores/workspace";
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
    void workspace.loadStorageStatus();
    void workspace.loadRepositories();
    return () => {
      unsubscribe();
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
