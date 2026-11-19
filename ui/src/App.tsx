import { useEffect } from "react";
import { TopBar } from "@/components/shell/top-bar";
import { WorkspacePanels } from "@/components/shell/workspace-panels";
import { workspaceStore } from "@/stores/workspace";
import { WorkspaceContext, type Workspace } from "@/stores/workspace-context";

export function App({ store = workspaceStore }: { store?: Workspace }) {
  useEffect(() => {
    const workspace = store.getState();
    const unsubscribe = workspace.watchWorktrees();
    void workspace.loadRepositories();
    return unsubscribe;
  }, [store]);

  return (
    <WorkspaceContext value={store}>
      <div className="app-shell">
        <TopBar />
        <WorkspacePanels />
      </div>
    </WorkspaceContext>
  );
}
