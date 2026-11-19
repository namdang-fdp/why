// @vitest-environment jsdom

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useDelayed } from "./diff-pane";

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("useDelayed (skeleton and dim delays)", () => {
  it("turns on only after the delay has fully elapsed", () => {
    const { result } = renderHook(({ active }) => useDelayed(active, 200), {
      initialProps: { active: true },
    });
    expect(result.current).toBe(false);
    act(() => vi.advanceTimersByTime(199));
    expect(result.current).toBe(false);
    act(() => vi.advanceTimersByTime(2));
    expect(result.current).toBe(true);
  });

  it("never turns on for a load that finishes first (a fast switch shows nothing)", () => {
    const { result, rerender } = renderHook(({ active }) => useDelayed(active, 200), {
      initialProps: { active: true },
    });
    act(() => vi.advanceTimersByTime(150));
    rerender({ active: false });
    act(() => vi.advanceTimersByTime(1000));
    expect(result.current).toBe(false);
  });

  it("starts counting from zero again for the next load", () => {
    const { result, rerender } = renderHook(({ active }) => useDelayed(active, 200), {
      initialProps: { active: true },
    });
    act(() => vi.advanceTimersByTime(250));
    expect(result.current).toBe(true);
    rerender({ active: false });
    expect(result.current).toBe(false);
    rerender({ active: true });
    expect(result.current).toBe(false); // not left over from the previous load
    act(() => vi.advanceTimersByTime(201));
    expect(result.current).toBe(true);
  });
});
