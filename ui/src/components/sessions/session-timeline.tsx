import { useCallback, useLayoutEffect, useMemo, useRef, useState, type FocusEvent } from "react";
import { ArrowDown, ChevronDown, ChevronUp } from "lucide-react";
import type { SessionTimeline as SessionTimelineData } from "@/contracts";
import { selectActiveFile } from "@/stores/workspace";
import { useSessions, useWorkspace } from "@/stores/workspace-context";
import { filesInTurn, matchesFile } from "./timeline-model";
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
  const files = useMemo(() => turns.map(filesInTurn), [turns]);
  const matching = useMemo(() => {
    const matches = new Set<number>();
    if (file)
      files.forEach((paths, index) => {
        if (matchesFile(paths, file)) matches.add(index);
      });
    return matches;
  }, [file, files]);
  const firstMatch = matching.values().next().value;
  const fileKey = file ? JSON.stringify([file.path, file.oldPath]) : null;
  const selectedIndex = turns.findIndex(({ id }) => id === selected);
  const focusedIndex = turns.findIndex(({ id }) => id === focused);

  const { viewport, measure, readViewport, reveal, unseen, follow, offsets, total, start, end } =
    useTurnWindow(turns);

  useLayoutEffect(() => {
    if (revealed.current === fileKey) return;
    revealed.current = fileKey;
    if (firstMatch !== undefined) reveal(firstMatch);
  }, [fileKey, firstMatch, reveal]);

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
