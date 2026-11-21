import type { Turn } from "@/contracts";
import { selectActiveFile } from "@/stores/workspace";
import type { Workspace } from "@/stores/workspace-context";
import { turnsTouchingFile } from "./timeline-model";

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
    const evidence = workspace.evidence.getState();
    const touching = turnsTouchingFile(
      turns,
      file,
      evidence.activity.status === "ok" ? evidence.activity.data : undefined,
      state.sessionId ?? undefined,
    );
    const touchingIds = new Set([...touching].map((index) => turns[index]?.id));
    const touchesFile = (turn: Turn) => touchingIds.has(turn.id);
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

/**
 * Show a turn in the timeline: open its session, then select the turn (the
 * timeline scrolls to it and focuses it). Used by every link that points at a
 * turn, wherever the pointer came from (the rail, the file history).
 */
export async function openTurn(
  store: Workspace,
  ref: { sessionId: string; turnId: string },
  focus = true,
) {
  store.getState().showInspector();
  const sessions = store.getState().sessions;
  await sessions.getState().selectSession(ref.sessionId);
  // Already selected and still loading: selectSession returned at once, so wait for the
  // timeline (or for the reviewer to pick another session, which ends the wait).
  await new Promise<void>((resolve) => {
    const settled = (state: ReturnType<typeof sessions.getState>) =>
      state.sessionId !== ref.sessionId || state.timeline.status !== "loading";
    if (settled(sessions.getState())) return resolve();
    const stop = sessions.subscribe((state) => {
      if (!settled(state)) return;
      stop();
      resolve();
    });
  });
  // The user may have opened something else while the timeline loaded.
  const now = sessions.getState();
  if (now.sessionId !== ref.sessionId || now.timeline.status !== "ok") return;
  // A turn the harness gave no id is still a real row, grouped under id "".
  if (now.timeline.data.turns.some(({ id }) => id === ref.turnId)) {
    now.selectTurn(ref.turnId, focus);
  }
}
