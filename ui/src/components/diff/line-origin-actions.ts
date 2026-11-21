import { inspectorViewStore } from "@/stores/inspector-view";
import type { Workspace } from "@/stores/workspace-context";

/**
 * Show where one line came from: select it for the inspector, make sure the
 * inspector is visible and on its File tab (the "Line origin" section lives
 * there). Used by a click on a gutter mark and by the `o` key on the cursor line.
 */
export function openLineOrigin(workspace: Workspace, line: number) {
  // The inspector shows the File tab only while no session is open (the timeline replaces
  // the tabs). Leave the session view first, as its own "Sessions" back button does, or the
  // reviewer would click a mark and see nothing happen.
  void workspace.getState().sessions.getState().selectSession(null);
  workspace.getState().lineOrigins.getState().selectLine(line);
  workspace.getState().showInspector();
  inspectorViewStore.getState().setTab("file");
}
