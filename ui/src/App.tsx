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
    void workspace.loadStorageStatus();
    void workspace.loadRepositories();
    return unsubscribe;
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
