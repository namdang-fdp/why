import type { Turn } from "@/contracts";
import { selectActiveFile } from "@/stores/workspace";
import type { Workspace } from "@/stores/workspace-context";
import { filesInTurn, matchesFile } from "./timeline-model";

export function focusInspector(store: Workspace, onFocus?: () => void) {
  store.getState().showInspector();
  // The pane can be mounting after the workspace changed its visible panels.
  requestAnimationFrame(() => {
    document.getElementById("why-inspector")?.focus();
    onFocus?.();
  });
}

export function moveTurn(store: Workspace, direction: 1 | -1 | "first" | "last", matching = false) {
  const workspace = store.getState();
  const state = workspace.sessions.getState();
  if (state.timeline.status !== "ok") return;
  const { turns } = state.timeline.data;
  if (turns.length === 0) return;
  const current = turns.findIndex(({ id }) => id === state.turnId);
  if (matching) {
    const file = selectActiveFile(workspace);
    if (!file) return;
    const touchesFile = (turn: Turn) => matchesFile(filesInTurn(turn), file);
    const forward = direction === 1 || direction === "first";
    const wrap = forward ? turns.find(touchesFile) : turns.findLast(touchesFile);
    const turn =
      current === -1 || typeof direction !== "number"
        ? wrap
        : ((forward
            ? turns.find((turn, index) => index > current && touchesFile(turn))
            : turns.findLast((turn, index) => index < current && touchesFile(turn))) ?? wrap);
    if (turn) state.selectTurn(turn.id, true);
    return;
  }

  const index =
    direction === "first"
      ? 0
      : direction === "last"
        ? turns.length - 1
        : current === -1
          ? direction === 1
            ? 0
            : turns.length - 1
          : Math.min(turns.length - 1, Math.max(0, current + direction));
  const turn = turns[index];
  if (turn) state.selectTurn(turn.id, true);
}
