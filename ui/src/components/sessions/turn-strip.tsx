import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import type { ShellOutcome, Turn } from "@/contracts";
import { segmentLabel, stripSegments } from "./turn-strip-model";

/**
 * One compact row with a segment per turn (or per bucket of turns for a long
 * session). It is an overview and a way in: a segment selects its first turn
 * and the timeline reveals it. It is a single tab stop; Left/Right move
 * between segments (roving tabindex), so 80 segments are not 80 tab stops.
 */
export function TurnStrip({
  turns,
  recovered,
  selected,
  onSelect,
}: {
  turns: readonly Turn[];
  recovered: ReadonlyMap<string, ShellOutcome> | undefined;
  selected: string | null;
  onSelect: (turnId: string) => void;
}) {
  const segments = useMemo(() => stripSegments(turns, recovered), [turns, recovered]);
  const selectedIndex = turns.findIndex(({ id }) => id === selected);
  const group = useRef<HTMLDivElement>(null);
  const [roving, setRoving] = useState(0);
  const current = segments.findIndex(
    ({ from, to }) => selectedIndex >= from && selectedIndex <= to,
  );
  // The tab stop follows the selection; otherwise the last segment the keyboard visited.
  const stop = Math.min(current >= 0 ? current : roving, segments.length - 1);

  function keyDown(event: KeyboardEvent<HTMLDivElement>) {
    const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    if (step === 0) return;
    // Handled here so the inspector's own keys never see the arrows.
    event.preventDefault();
    const buttons = [...(group.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
    const index = buttons.findIndex((button) => button === document.activeElement);
    const next = Math.max(0, Math.min(buttons.length - 1, index + step));
    setRoving(next);
    buttons[next]?.focus();
  }

  if (segments.length === 0) return null;
  return (
    <div
      ref={group}
      className="turn-strip"
      role="group"
      aria-label="Turn overview"
      onKeyDown={keyDown}
    >
      {segments.map((segment, index) => {
        const turn = turns[segment.from];
        return (
          <button
            key={segment.from}
            type="button"
            className="turn-strip-segment"
            data-tone={segment.tone}
            aria-label={segmentLabel(segment)}
            aria-current={index === current || undefined}
            tabIndex={index === stop ? 0 : -1}
            title={segmentLabel(segment)}
            onClick={() => turn && onSelect(turn.id)}
          />
        );
      })}
    </div>
  );
}
