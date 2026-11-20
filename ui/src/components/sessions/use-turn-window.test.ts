// @vitest-environment jsdom

import { createElement } from "react";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Turn } from "@/contracts";
import { TooltipProvider } from "@/components/ui/overlays";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { buildLongSession } from "@/dev-fixtures/long-session";
import { WT_BILLING_DETACHED } from "@/fixtures";
import { createWorkspaceStore } from "@/stores/workspace";
import { WorkspaceContext } from "@/stores/workspace-context";
import { mockTimelineViewport } from "@/test/timeline-viewport";
import { SessionTimeline } from "./session-timeline";

class LayoutObserver {
  static callbacks = new Map<Element, Set<() => void>>();
  constructor(private readonly notify: () => void) {}

  observe(node: Element) {
    const callbacks = LayoutObserver.callbacks.get(node) ?? new Set();
    callbacks.add(this.notify);
    LayoutObserver.callbacks.set(node, callbacks);
  }

  disconnect() {
    for (const callbacks of LayoutObserver.callbacks.values()) callbacks.delete(this.notify);
  }

  static resize(node: Element) {
    act(() => {
      for (const notify of LayoutObserver.callbacks.get(node) ?? []) notify();
    });
  }
}

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", LayoutObserver);
  vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame"] });
});

afterEach(() => {
  cleanup();
  LayoutObserver.callbacks.clear();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function setup(turns: Turn[], height = 600) {
  const workspace = createWorkspaceStore(new FixtureDataSource());
  const tree = (data: Turn[]) =>
    createElement(WorkspaceContext, {
      value: workspace,
      children: createElement(TooltipProvider, {
        children: createElement(SessionTimeline, { turns: data, moveMatch: () => {} }),
      }),
    });
  const view = render(tree(turns));
  const viewport = screen.getByRole("region", { name: "Session turns" });
  mockTimelineViewport(viewport, height);
  LayoutObserver.resize(viewport);
  return {
    viewport,
    row: (index: number) => within(viewport).getByRole("article", { name: `Turn ${index + 1}` }),
    update: (data: Turn[]) => {
      view.rerender(tree(data));
      act(() => vi.advanceTimersToNextFrame());
    },
    scroll: (top: number) => {
      viewport.scrollTop = top;
      fireEvent.scroll(viewport);
    },
    focusTurn: (id: string) =>
      act(() => workspace.getState().sessions.getState().selectTurn(id, true)),
  };
}

function rowTop(node: HTMLElement) {
  return Number.parseFloat(node.style.transform.slice("translateY(".length));
}

function resizeRow(row: HTMLElement, height: number) {
  vi.spyOn(row, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 320, height));
  LayoutObserver.resize(row);
}

const short = (count: number) => buildLongSession(WT_BILLING_DETACHED, count).turns;

function addEvent(turn: Turn): Turn {
  const event = turn.events[0];
  if (!event) throw new Error("Expected a fixture event");
  return { ...turn, events: [...turn.events, { ...event, id: "new-event" }] };
}

describe("timeline scroll and row measurement", () => {
  it("follows an appended turn at the bottom and settles at its measured height", () => {
    const turns = short(9);
    const { viewport, scroll, update, row } = setup(turns.slice(0, 8));
    scroll(viewport.scrollHeight);
    const before = viewport.scrollTop;
    update(turns);
    expect(viewport.scrollTop).toBeGreaterThan(before);
    expect(viewport.scrollTop + viewport.clientHeight).toBe(viewport.scrollHeight);
    resizeRow(row(8), 320);
    expect(viewport.scrollTop + viewport.clientHeight).toBe(viewport.scrollHeight);
    expect(screen.queryByRole("button", { name: /new$/ })).toBeNull();
  });

  it("follows a new event in a previously measured turn at the bottom", () => {
    const turns = short(8);
    const last = turns.at(-1);
    if (!last) throw new Error("Expected a last turn");
    const { viewport, scroll, update, row } = setup(turns);
    scroll(viewport.scrollHeight);
    resizeRow(row(7), 320);
    scroll(viewport.scrollHeight);
    const before = viewport.scrollTop;
    update([...turns.slice(0, -1), addEvent(last)]);
    resizeRow(row(7), 380);
    expect(viewport.scrollTop).toBe(before + 60);
    expect(viewport.scrollTop + viewport.clientHeight).toBe(viewport.scrollHeight);
    expect(screen.queryByRole("button", { name: /new$/ })).toBeNull();
  });

  it("keeps a mid-list reader in place and counts one event and one turn", () => {
    const turns = short(9);
    const seventh = turns[6];
    if (!seventh) throw new Error("Expected a fixture turn");
    const { viewport, scroll, update } = setup(turns.slice(0, 8));
    scroll(470);
    update([...turns.slice(0, 6), addEvent(seventh), ...turns.slice(7)]);
    expect(viewport.scrollTop).toBe(470);
    expect(screen.getByRole("button", { name: "2 new" })).toBeTruthy();
  });

  it("jumps to the tail and accounts for rows uncovered by earlier measurements", () => {
    const turns = short(9);
    const { viewport, row, update } = setup(turns.slice(0, 8));
    update(turns);
    fireEvent.click(screen.getByRole("button", { name: "1 new" }));
    expect(viewport.scrollTop + viewport.clientHeight).toBe(viewport.scrollHeight);
    expect(within(viewport).queryByRole("article", { name: "Turn 6" })).toBeNull();

    resizeRow(row(8), 90);
    resizeRow(row(7), 90);
    resizeRow(row(5), 450);
    expect(viewport.scrollTop + viewport.clientHeight).toBe(viewport.scrollHeight);
    expect(screen.queryByRole("button", { name: /new$/ })).toBeNull();
  });

  it("anchors the visible turn when a measured row above it grows", () => {
    const { viewport, row, scroll } = setup(short(9));
    resizeRow(row(0), 300);
    resizeRow(row(1), 280);
    scroll(345);
    const offset = rowTop(row(1)) - viewport.scrollTop;
    expect(offset).toBe(-45);

    resizeRow(row(0), 370);
    expect(viewport.scrollTop).toBe(415);
    expect(rowTop(row(1)) - viewport.scrollTop).toBe(offset);
  });

  it("anchors the visible turn instead of following a height-only change at the bottom", () => {
    const { viewport, row, scroll } = setup(short(8));
    scroll(viewport.scrollHeight);
    resizeRow(row(7), 600);
    expect(viewport.scrollTop + viewport.clientHeight).toBeLessThan(viewport.scrollHeight);
    scroll(viewport.scrollHeight);
    const before = viewport.scrollTop;
    const offset = rowTop(row(7)) - before;
    expect(offset).toBe(0);

    resizeRow(row(7), 740);
    expect(viewport.scrollTop).toBe(before);
    expect(rowTop(row(7)) - viewport.scrollTop).toBe(offset);
    expect(viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop).toBe(140);
    expect(screen.queryByRole("button", { name: /new$/ })).toBeNull();
  });

  it("bounds the 500-turn DOM at a measured 720px viewport and exposes position", () => {
    const { viewport, row, scroll, focusTurn } = setup(short(500), 720);
    // Row 4 is outside the 480px fallback range for these measured rows.
    resizeRow(row(0), 370);
    resizeRow(row(1), 360);
    resizeRow(row(2), 300);
    expect(row(3).getAttribute("aria-posinset")).toBe("4");
    expect(row(3).getAttribute("aria-setsize")).toBe("500");

    const mounted = () => within(viewport).getAllByRole("article").length;
    expect(mounted()).toBeLessThan(20);
    scroll(viewport.scrollHeight / 2);
    expect(mounted()).toBeLessThan(20);
    focusTurn("long-turn-499");
    expect(mounted()).toBeLessThan(20);
    expect(row(499).getAttribute("aria-posinset")).toBe("500");
    expect(rowTop(row(499)) - viewport.scrollTop).toBeLessThan(viewport.clientHeight);
    expect(document.activeElement).toBe(within(row(499)).getByRole("button", { name: "Turn 500" }));
  });
});
