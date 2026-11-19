import { describe, expect, it, vi } from "vitest";
import type { editor } from "monaco-editor";
import { createHunkNavigator } from "./hunk-navigation";

vi.mock("monaco-editor/editor/editor.api.js", () => ({ editor: { ScrollType: { Immediate: 1 } } }));

function fakeDiff() {
  let line = 1;
  const lastLine = 8;
  const getLineChanges = vi.fn<() => editor.ILineChange[] | null>(() => null);
  const modified = {
    getPosition: () => ({ lineNumber: line }),
    getModel: () => ({ getLineCount: () => lastLine }),
    setPosition: vi.fn(({ lineNumber }: { lineNumber: number }) => {
      line = lineNumber;
    }),
    revealLineInCenter: vi.fn(),
  };
  const goToDiff = vi.fn((direction: "next" | "previous") => {
    const lines =
      getLineChanges()?.map((change) =>
        change.modifiedEndLineNumber === 0
          ? change.modifiedStartLineNumber + 1
          : change.modifiedStartLineNumber,
      ) ?? [];
    const target =
      direction === "next"
        ? line === lastLine
          ? lines[0]
          : (lines.find((hunk) => hunk > line) ?? lines[0])
        : (lines.findLast((hunk) => hunk < line) ?? lines.at(-1));
    if (target !== undefined) line = Math.min(target, lastLine);
  });
  const navigator = createHunkNavigator({
    getModifiedEditor: () => modified,
    getLineChanges,
    goToDiff,
  });
  return { navigator, modified, getLineChanges, goToDiff };
}

function change(line: number, deleted = false): editor.ILineChange {
  return {
    originalStartLineNumber: line,
    originalEndLineNumber: line,
    modifiedStartLineNumber: line,
    modifiedEndLineNumber: deleted ? 0 : line,
    charChanges: undefined,
  };
}

describe("Monaco hunk navigation", () => {
  it("waits for the worker and visits a change on the first line before advancing", () => {
    const { navigator, modified, getLineChanges, goToDiff } = fakeDiff();
    expect(navigator.move("next")).toBe("pending");
    getLineChanges.mockReturnValue([change(1), change(4)]);
    expect(navigator.move("next")).toBe("moved");
    expect(modified.revealLineInCenter).toHaveBeenCalledWith(1, 1);
    expect(navigator.move("next")).toBe("moved");
    expect(goToDiff).toHaveBeenLastCalledWith("next");
    expect(modified.getPosition().lineNumber).toBe(4);
    expect(navigator.move("next")).toBe("boundary");
    expect(navigator.move("previous")).toBe("moved");
    expect(goToDiff).toHaveBeenLastCalledWith("previous");
    expect(navigator.move("previous")).toBe("boundary");
  });

  it("reveals the first hunk on entry and distinguishes an EOF deletion from the last line", () => {
    const { navigator, modified, getLineChanges } = fakeDiff();
    getLineChanges.mockReturnValue([change(8), change(8, true)]);
    navigator.revealFirst();
    expect(modified.getPosition().lineNumber).toBe(8);
    expect(navigator.move("next")).toBe("moved");
    expect(navigator.move("next")).toBe("boundary");
    expect(navigator.move("previous")).toBe("moved");
    expect(navigator.move("previous")).toBe("boundary");
  });
});
