// @vitest-environment jsdom

import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { saveLayout, usePanelLayout } from "./use-panel-layout";

const key = "why:test-layout";
const fallback = { top: 40, bottom: 60 };

afterEach(() => localStorage.clear());

describe("usePanelLayout (remembered split sizes)", () => {
  it("starts from the fallback and remembers a layout the user dragged", () => {
    const { result } = renderHook(() => usePanelLayout(key, fallback));
    expect(result.current.defaultLayout).toEqual(fallback);

    result.current.onLayoutChanged({ top: 30, bottom: 70 }, { isUserInteraction: true } as never);
    expect(JSON.parse(localStorage.getItem(key) ?? "{}")).toEqual({ top: 30, bottom: 70 });

    // A new mount (the next launch) starts from what was saved.
    const again = renderHook(() => usePanelLayout(key, fallback));
    expect(again.result.current.defaultLayout).toEqual({ top: 30, bottom: 70 });
  });

  it("does not save sizes the library computed itself (a window resize)", () => {
    const { result } = renderHook(() => usePanelLayout(key, fallback));
    result.current.onLayoutChanged({ top: 10, bottom: 90 }, { isUserInteraction: false } as never);
    expect(localStorage.getItem(key)).toBeNull();
  });

  it.each([
    ["not json", "{oops"],
    ["wrong panels", JSON.stringify({ a: 50, b: 50 })],
    ["does not add up", JSON.stringify({ top: 10, bottom: 10 })],
    ["out of range", JSON.stringify({ top: 0, bottom: 100 })],
  ])("ignores a saved layout that is %s", (_name, saved) => {
    localStorage.setItem(key, saved);
    const { result } = renderHook(() => usePanelLayout(key, fallback));
    expect(result.current.defaultLayout).toEqual(fallback);
  });

  it("saveLayout writes what a reset needs", () => {
    saveLayout(key, fallback);
    expect(JSON.parse(localStorage.getItem(key) ?? "{}")).toEqual(fallback);
  });
});
