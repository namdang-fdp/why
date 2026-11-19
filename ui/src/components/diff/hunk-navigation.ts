import { editor } from "monaco-editor/editor/editor.api.js";

export type HunkDirection = "next" | "previous";
export type HunkResult = "pending" | "moved" | "boundary";

type ModifiedEditor = Pick<editor.ICodeEditor, "setPosition" | "revealLineInCenter"> & {
  // Optional so a minimal editor (a test double) still satisfies the type.
  setScrollTop?: editor.ICodeEditor["setScrollTop"];
  getTopForLineNumber?: editor.ICodeEditor["getTopForLineNumber"];
  getPosition(): { lineNumber: number } | null;
  getModel(): Pick<editor.ITextModel, "getLineCount"> | null;
};

type DiffNavigationEditor = Pick<editor.IDiffEditor, "getLineChanges" | "goToDiff"> & {
  getModifiedEditor(): ModifiedEditor;
};

export function createHunkNavigator(diffEditor: DiffNavigationEditor) {
  const modified = diffEditor.getModifiedEditor();
  let lastJump: number | null = null;

  function hunkLines() {
    // Monaco's legacy line-change API reports a deletion after the preceding line.
    return diffEditor
      .getLineChanges()
      ?.map((change) =>
        change.modifiedEndLineNumber === 0
          ? change.modifiedStartLineNumber + 1
          : change.modifiedStartLineNumber,
      );
  }

  function jump(line: number, cursor: number, direction: HunkDirection) {
    const lastLine = modified.getModel()?.getLineCount() ?? 1;
    if (direction === "next" ? cursor < line && cursor < lastLine : cursor > line) {
      diffEditor.goToDiff(direction);
    } else {
      // goToDiff skips the current line and wraps at EOF.
      const target = Math.min(line, lastLine);
      modified.setPosition({ lineNumber: target, column: 1 });
      modified.revealLineInCenter(target, editor.ScrollType.Immediate);
    }
    lastJump = line;
  }

  return {
    move(direction: HunkDirection): HunkResult {
      const lines = hunkLines();
      if (!lines) return "pending";
      const cursor = modified.getPosition()?.lineNumber ?? 1;
      const lastLine = modified.getModel()?.getLineCount() ?? 1;
      const current =
        lastJump !== null && Math.min(lastJump, lastLine) === cursor ? lastJump : null;
      const target =
        direction === "next"
          ? lines.find((line) => line > (current ?? (cursor === 1 ? 0 : cursor)))
          : lines.findLast((line) => line < (current ?? cursor));
      if (target === undefined) return "boundary";
      jump(target, cursor, direction);
      return "moved";
    },
    revealFirst() {
      const first = hunkLines()?.[0];
      if (first !== undefined) jump(first, modified.getPosition()?.lineNumber ?? 1, "next");
    },
    /**
     * Put the top of the first change at the top of the editor with `context`
     * lines above it, like opening a review: you land ON the change, not in the
     * middle of the file. Returns false while the diff is not computed yet.
     */
    revealFirstChange(context = 3) {
      const first = hunkLines()?.[0];
      if (first === undefined) return false;
      modified.setPosition({ lineNumber: first, column: 1 });
      if (modified.setScrollTop && modified.getTopForLineNumber) {
        const top = modified.getTopForLineNumber(Math.max(1, first - context));
        modified.setScrollTop(top, editor.ScrollType.Immediate);
      } else {
        modified.revealLineInCenter(first, editor.ScrollType.Immediate);
      }
      lastJump = first;
      return true;
    },
  };
}
