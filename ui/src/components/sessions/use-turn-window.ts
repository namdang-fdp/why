import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Turn } from "@/contracts";
import { newActivity } from "./timeline-model";

const overscan = 400;
const bottomSlack = 16;
const initialViewport = 480;

type RowMeasurement = { turn: Turn; height: number };

function rowAt(offsets: number[], y: number) {
  let low = 0;
  let high = offsets.length - 1;
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    if ((offsets[mid] ?? Infinity) <= y) low = mid;
    else high = mid - 1;
  }
  return low;
}

export function useTurnWindow(turns: Turn[]) {
  const viewport = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, height: initialViewport });
  const [heights, setHeights] = useState<ReadonlyMap<string, RowMeasurement>>(() => new Map());
  const [unseen, setUnseen] = useState(0);
  const atEnd = useRef(false);
  const settlingTail = useRef(false);

  const geometry = useMemo(() => {
    const offsets = [0];
    const indices = new Map<string, number>();
    let total = 0;
    for (const [index, turn] of turns.entries()) {
      indices.set(turn.id, index);
      total += heights.get(turn.id)?.height ?? 104 + turn.events.length * 66;
      offsets.push(total);
    }
    return { turns, heights, offsets, indices, total };
  }, [heights, turns]);
  const previous = useRef(geometry);

  const measure = useCallback((turn: Turn, height: number) => {
    if (height <= 0) return;
    setHeights((sizes) => {
      const old = sizes.get(turn.id);
      if (old?.turn === turn && old.height === height) return sizes;
      return new Map(sizes).set(turn.id, { turn, height });
    });
  }, []);

  const readViewport = useCallback(() => {
    const node = viewport.current;
    if (!node) return;
    const { clientHeight, scrollTop, scrollHeight } = node;
    atEnd.current = clientHeight > 0 && scrollHeight - clientHeight - scrollTop <= bottomSlack;
    if (atEnd.current) setUnseen(0);
    else settlingTail.current = false;
    setView((old) => {
      const height = clientHeight || initialViewport;
      return old.top === scrollTop && old.height === height ? old : { top: scrollTop, height };
    });
  }, []);

  const hasTurns = turns.length > 0;
  useLayoutEffect(() => {
    const node = viewport.current;
    if (!hasTurns || !node) return;
    const observer = new ResizeObserver(readViewport);
    observer.observe(node);
    return () => observer.disconnect();
  }, [hasTurns, readViewport]);

  const setScroll = useCallback(
    (top: number) => {
      if (!viewport.current) return;
      viewport.current.scrollTop = top;
      readViewport();
    },
    [readViewport],
  );

  const follow = useCallback(() => {
    const node = viewport.current;
    if (!node) return;
    settlingTail.current = true;
    setScroll(node.scrollHeight);
  }, [setScroll]);

  useLayoutEffect(() => {
    const old = previous.current;
    previous.current = geometry;
    const node = viewport.current;
    if (!node || old === geometry) return;
    const added = old.turns === turns ? 0 : newActivity(old.turns, turns);
    if (added > 0 && atEnd.current) {
      follow();
      return;
    }
    if (settlingTail.current) {
      // Correct estimates uncovered by the jump as those rows first measure.
      // Resizing an already measured row (e.g. opening a disclosure) still anchors.
      const start = rowAt(geometry.offsets, geometry.total - node.clientHeight - overscan);
      const tail = turns.slice(start);
      const measured = (turn: Turn) => heights.get(turn.id)?.turn === turn;
      settlingTail.current = tail.some((turn) => !measured(turn));
      if (tail.some((turn) => measured(turn) && old.heights.get(turn.id)?.turn !== turn)) {
        setScroll(node.scrollHeight);
        return;
      }
    }
    if (added > 0) {
      // Keep the badge out of the scrollport so its arrival does not resize it.
      requestAnimationFrame(() => {
        if (!atEnd.current) setUnseen((count) => count + added);
      });
    }

    // If a row above the viewport changed size, preserve the visible turn and
    // its intra-row offset. Browser anchoring is disabled on this scrollport.
    const index = rowAt(old.offsets, node.scrollTop);
    const anchor = old.turns[index];
    const nextIndex = anchor && geometry.indices.get(anchor.id);
    const nextTop = nextIndex === undefined ? undefined : geometry.offsets[nextIndex];
    const oldTop = old.offsets[index];
    if (nextTop !== undefined && oldTop !== undefined) {
      const top = nextTop + node.scrollTop - oldTop;
      if (top !== node.scrollTop) {
        setScroll(top);
        return;
      }
    }
    readViewport();
  }, [follow, geometry, heights, readViewport, setScroll, turns]);

  const reveal = useCallback(
    (index: number) => {
      const node = viewport.current;
      if (!node) return;
      const top = geometry.offsets[index];
      // Keep the turn heading visible even when the turn is taller than the viewport.
      if (
        top !== undefined &&
        (top < node.scrollTop || top + 34 > node.scrollTop + node.clientHeight)
      ) {
        setScroll(top);
      }
    },
    [geometry.offsets, setScroll],
  );

  return {
    viewport,
    measure,
    readViewport,
    reveal,
    unseen,
    follow,
    offsets: geometry.offsets,
    total: geometry.total,
    start: rowAt(geometry.offsets, Math.max(0, view.top - overscan)),
    end: Math.min(turns.length, rowAt(geometry.offsets, view.top + view.height + overscan) + 1),
  };
}
