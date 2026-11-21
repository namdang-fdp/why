import { useCallback, useLayoutEffect, useMemo, useRef, useState, type FocusEvent } from "react";
import { ArrowDown, ChevronDown, ChevronUp } from "lucide-react";
import type { SessionTimeline as SessionTimelineData } from "@/contracts";
import { selectActiveFile } from "@/stores/workspace";
import { useEvidence, useSessions, useWorkspace } from "@/stores/workspace-context";
import { turnsTouchingFile } from "./timeline-model";
import { recoveredOutcomes } from "./outcome-overlay";
import { TurnRow } from "./turn-row";
import { useTurnWindow } from "./use-turn-window";

export function SessionTimeline({
  turns,
  moveMatch,
}: {
  turns: SessionTimelineData["turns"];
  moveMatch: (direction: 1 | -1) => void;
}) {
  const file = useWorkspace(selectActiveFile);
  const openFile = useWorkspace((state) => state.openSessionFile);
  const selected = useSessions((state) => state.turnId);
  const focusRequest = useSessions((state) => state.turnFocus);
  const selectTurn = useSessions((state) => state.selectTurn);
  const [focused, setFocused] = useState<string | null>(null);
  const focusHandled = useRef(focusRequest);
  const revealed = useRef<string | null>(null);
  const [openItems, setOpenItems] = useState<ReadonlyMap<string, ReadonlySet<string | null>>>(
    () => new Map(),
  );
  const sessionId = useSessions((state) => state.sessionId) ?? "";
  const activity = useEvidence((state) => state.activity);
  const activityPath = useEvidence((state) => state.activityPath);
  const outcomeState = useEvidence((state) => state.outcomes);
  const outcomeSession = useEvidence((state) => state.session?.id);
  // Exit codes recovered for THIS session only; another session's never overlay here.
  const outcomes = useMemo(
    () => (outcomeSession === sessionId ? recoveredOutcomes(outcomeState) : undefined),
    [outcomeSession, sessionId, outcomeState],
  );
  // A turn matches the selected file when its events name it OR the core's edit
  // history for the file (which also knows renames) points at it.
  const matching = useMemo(
    () =>
      turnsTouchingFile(
        turns,
        file,
        activity.status === "ok" ? activity.data : undefined,
        sessionId,
      ),
    [turns, file, activity, sessionId],
  );
  const firstMatch = matching.values().next().value;
  const fileKey = file ? JSON.stringify([file.path, file.oldPath]) : null;
  const selectedIndex = turns.findIndex(({ id }) => id === selected);
  const focusedIndex = turns.findIndex(({ id }) => id === focused);

  const { viewport, measure, readViewport, reveal, unseen, follow, offsets, total, start, end } =
    useTurnWindow(turns);

  // The edit history arrives after the file is selected, and it may be the only thing
  // that names the matching turn: do not call the file "handled" before it has answered.
  const historySettled =
    !file ||
    file.pathLossy ||
    (activityPath === file.path && activity.status !== "loading" && activity.status !== "idle");
  useLayoutEffect(() => {
    if (revealed.current === fileKey) return;
    if (firstMatch !== undefined) {
      revealed.current = fileKey;
      reveal(firstMatch);
    } else if (historySettled) {
      revealed.current = fileKey;
    }
  }, [fileKey, firstMatch, historySettled, reveal]);

  useLayoutEffect(() => {
    if (focusRequest === focusHandled.current) return;
    focusHandled.current = focusRequest;
    if (selectedIndex < 0) return;
    reveal(selectedIndex);
    viewport.current
      ?.querySelector<HTMLButtonElement>('[data-selected="true"] [data-turn-select]')
      ?.focus({ preventScroll: true });
  }, [focusRequest, reveal, selectedIndex, viewport]);

  const visible = new Set<number>();
  for (let index = start; index < end; index++) visible.add(index);
  // Keep a focused disclosure/file chip mounted when the reviewer scrolls
  // away from its turn. Also mount the destination of a keyboard jump now.
  if (focusedIndex >= 0) visible.add(focusedIndex);
  if (selectedIndex >= 0) visible.add(selectedIndex);

  const toggle = useCallback((turnId: string, itemId: string | null) => {
    setOpenItems((items) => {
      const next = new Set(items.get(turnId));
      if (!next.delete(itemId)) next.add(itemId);
      const updated = new Map(items);
      if (next.size > 0) updated.set(turnId, next);
      else updated.delete(turnId);
      return updated;
    });
  }, []);

  function onBlur(event: FocusEvent<HTMLDivElement>) {
    if (!event.currentTarget.contains(event.relatedTarget)) setFocused(null);
  }

  return (
    <>
      <div className="session-timeline-bar">
        <h3>
          Turns <span>{turns.length}</span>
        </h3>
        <div className="session-timeline-actions">
          {file && (
            <span className="session-match-count" title={`Turns touching ${file.path}`}>
              {matching.size} touching file
            </span>
          )}
          {matching.size > 0 && (
            <>
              <button
                type="button"
                className="session-icon-button"
                aria-label="Previous turn touching this file"
                title="Previous turn touching this file (p)"
                onClick={() => moveMatch(-1)}
              >
                <ChevronUp size={14} aria-hidden />
              </button>
              <button
                type="button"
                className="session-icon-button"
                aria-label="Next turn touching this file"
                title="Next turn touching this file (n)"
                onClick={() => moveMatch(1)}
              >
                <ChevronDown size={14} aria-hidden />
              </button>
            </>
          )}
        </div>
      </div>
      {turns.length === 0 ? (
        <p className="rail-note">No turns recorded for this session.</p>
      ) : (
        <div className="session-scroll-frame">
          <div
            ref={viewport}
            className="session-scroll"
            role="region"
            aria-label="Session turns"
            tabIndex={0}
            onScroll={readViewport}
            onBlur={onBlur}
          >
            <div className="session-window" style={{ height: total }}>
              {[...visible]
                .sort((a, b) => a - b)
                .map((index) => {
                  const turn = turns[index];
                  const top = offsets[index];
                  if (!turn || top === undefined) return null;
                  return (
                    <TurnRow
                      key={turn.id}
                      turn={turn}
                      setSize={turns.length}
                      top={top}
                      selected={turn.id === selected}
                      matching={matching.has(index)}
                      file={file}
                      measure={measure}
                      select={selectTurn}
                      onTurnFocus={setFocused}
                      openFile={openFile}
                      openItems={openItems.get(turn.id)}
                      toggle={toggle}
                      sessionId={sessionId}
                      outcomes={outcomes}
                    />
                  );
                })}
            </div>
          </div>
          <div className="session-arrivals" aria-live="polite">
            {unseen > 0 && (
              <button
                type="button"
                className="session-new"
                onClick={() => {
                  follow();
                  viewport.current?.focus({ preventScroll: true });
                }}
              >
                <ArrowDown size={13} aria-hidden />
                {unseen} new
              </button>
            )}
          </div>
        </div>
      )}
    </>
  );
}
