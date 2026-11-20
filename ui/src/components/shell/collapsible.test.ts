// @vitest-environment jsdom

import { createElement, useState } from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Collapsible } from "./collapsible";

// Keep in step with CLOSE_MS in collapsible.tsx.
const CLOSE_MS = 200;

const body = () => screen.queryByText("body");
const view = (open: boolean) =>
  createElement(Collapsible, { open, children: createElement("p", null, "body") });

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("Collapsible", () => {
  it("shows the body when a click opens it just after the close timer fired", () => {
    // The flake (WHY-44). The first (closed) render sets a 200 ms timer. When it fires
    // OUTSIDE React's batch (a real timer, as under CI load) its setMounted(false) is queued
    // at default priority and not rendered yet. The click is a discrete event: React renders
    // it first, with the render-phase setMounted(true), and replays the older update after.
    function Toggle() {
      const [open, setOpen] = useState(false);
      return createElement(
        "div",
        null,
        createElement("button", { onClick: () => setOpen(true) }, "open"),
        view(open),
      );
    }
    render(createElement(Toggle));
    const flag = globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean };
    flag.IS_REACT_ACT_ENVIRONMENT = false; // let the timer's update stay queued, as in a browser
    try {
      vi.advanceTimersByTime(CLOSE_MS);
    } finally {
      flag.IS_REACT_ACT_ENVIRONMENT = true;
    }
    fireEvent.click(screen.getByRole("button", { name: "open" }));
    act(() => {
      vi.advanceTimersByTime(CLOSE_MS * 5); // let the queued update land
    });
    expect(body()).not.toBeNull();
  });

  it("opens straight after mount, long before the timer", () => {
    const { rerender } = render(view(false));
    expect(body()).toBeNull();
    rerender(view(true));
    expect(body()).not.toBeNull();
  });

  it("keeps the body for the closing animation, then unmounts it", () => {
    const { rerender } = render(view(true));
    rerender(view(false));
    expect(body()).not.toBeNull();
    act(() => {
      vi.advanceTimersByTime(CLOSE_MS - 1);
    });
    expect(body()).not.toBeNull();
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(body()).toBeNull();
  });

  it("cancels the unmount when reopened inside the closing window", () => {
    const { rerender } = render(view(true));
    rerender(view(false));
    act(() => {
      vi.advanceTimersByTime(CLOSE_MS / 2);
    });
    rerender(view(true));
    act(() => {
      vi.advanceTimersByTime(CLOSE_MS * 5);
    });
    expect(body()).not.toBeNull();
  });
});
