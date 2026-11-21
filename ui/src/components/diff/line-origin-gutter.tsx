import {
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { MonacoDiffEditor } from "@monaco-editor/react";
import type { FileDiff } from "@/contracts";
import { diffReadRevision } from "@/stores/line-origins";
import { selectDiffMode } from "@/stores/workspace";
import { WorkspaceContext, useLineOrigins, useWorkspace } from "@/stores/workspace-context";
import { openLineOrigin } from "./line-origin-actions";
import { LineOriginTip } from "./line-origin-tip";
import {
  adjacentUnknownLine,
  buildMarks,
  gutterGate,
  markedRunAt,
  runAt,
  textShape,
  type GutterGate,
} from "./line-origins-model";
import { monaco } from "./monaco-setup";

type Editors = readonly [MonacoDiffEditor | null, MonacoDiffEditor | null];
/** A hovered mark. It remembers the file it was over, so a different file clears it. */
type Tip = { line: number; x: number; y: number; diff: FileDiff };

/** Monaco's overview ruler (the strip beside the scrollbar) wants a literal colour. */
const unknownRulerColor = "#d4a72c";

function decorationOf(mark: ReturnType<typeof buildMarks>[number]) {
  return {
    range: new monaco.Range(mark.startLine, 1, mark.endLine, 1),
    options: {
      // One decoration spans the whole run; Monaco paints it per visible line.
      isWholeLine: true,
      linesDecorationsClassName: mark.className,
      // Unknown runs also show in the ruler, so a long file's unknowns are findable.
      ...(mark.kind === "unknown" && {
        overviewRuler: {
          color: unknownRulerColor,
          position: monaco.editor.OverviewRulerLane.Right,
        },
      }),
    },
  };
}

/**
 * Draws line origins in the modified side's gutter and handles hover, click and
 * keys. Marks are Monaco decorations built from the RUNS (a handful per file),
 * never one React element per line, so a 2,000-line file costs the same as 20.
 *
 * What it will NOT do is paint on text the origins do not describe: `gutterGate`
 * decides, and the marks are cleared the moment the file is stale, the
 * comparison is not the working tree, or the line counts disagree.
 */
export function useLineOriginGutter({
  editors,
  front,
  shown,
}: {
  editors: Editors;
  front: 0 | 1;
  /** The file in the front editor, or `null` while nothing is shown. */
  shown: FileDiff | null;
}): { tip: ReactNode } {
  const workspace = useContext(WorkspaceContext);
  const worktreeId = useWorkspace((state) => state.worktreeId);
  const mode = useWorkspace(selectDiffMode);
  const origins = useLineOrigins((state) => state.origins);
  const revision = useLineOrigins((state) => state.revision);
  const originsRevision = useLineOrigins((state) => state.originsRevision);
  // Re-check the shown diff's revision tag when a re-read confirmed it unchanged.
  useLineOrigins((state) => state.diffTick);
  const diffRevision = diffReadRevision(shown);
  const [tip, setTip] = useState<Tip | null>(null);

  const shape = useMemo(
    () => (shown?.newContent != null ? textShape(shown.newContent) : null),
    [shown],
  );
  const gate = useMemo(
    () =>
      gutterGate({
        origins,
        revision,
        originsRevision,
        diffRevision,
        mode,
        worktreeId,
        path: shown?.path ?? null,
        text: shape,
      }),
    [origins, revision, originsRevision, diffRevision, mode, worktreeId, shown?.path, shape],
  );

  // Handlers registered once per editor read the current state from here.
  const live = useRef<{ gate: GutterGate; front: 0 | 1; diff: FileDiff | null }>({
    gate,
    front,
    diff: shown,
  });
  const collections = useRef<
    [
      monaco.editor.IEditorDecorationsCollection | null,
      monaco.editor.IEditorDecorationsCollection | null,
    ]
  >([null, null]);

  // Paint (or clear) the front editor's marks. The other editor never shows marks.
  useLayoutEffect(() => {
    live.current = { gate, front, diff: shown };
    editors.forEach((diffEditor, index) => {
      if (!diffEditor) return;
      collections.current[index]?.clear();
      collections.current[index] = null;
      if (index !== front || gate.status !== "shown") return;
      const modified = diffEditor.getModifiedEditor();
      const started = performance.now();
      const marks = buildMarks(gate.origins.runs, modified.getModel()?.getLineCount() ?? 0);
      collections.current[index] = modified.createDecorationsCollection(marks.map(decorationOf));
      if (import.meta.env.DEV) {
        // Dev-only: read it with `performance.getEntriesByName("why:origin-marks")`.
        performance.measure("why:origin-marks", {
          start: started,
          end: performance.now(),
          detail: { runs: gate.origins.runs.length, marks: marks.length },
        });
      }
    });
  }, [editors, front, gate, shown]);

  // Mouse and keys, on each editor's modified side.
  useEffect(() => {
    const disposables: monaco.IDisposable[] = [];
    editors.forEach((diffEditor, index) => {
      if (!diffEditor) return;
      const modified = diffEditor.getModifiedEditor();
      const isFront = () => live.current.front === index;

      /** The run under a gutter mouse event, if the gutter is live and this editor is in front. */
      function runUnder(event: monaco.editor.IEditorMouseEvent) {
        const state = live.current;
        if (!isFront() || state.gate.status !== "shown") return null;
        if (event.target.type !== monaco.editor.MouseTargetType.GUTTER_LINE_DECORATIONS)
          return null;
        const line = event.target.position?.lineNumber;
        if (line === undefined) return null;
        const run = state.gate.origins.runs[runAt(state.gate.origins.runs, line)];
        return run ? { line, run } : null;
      }

      disposables.push(
        modified.onMouseMove((event) => {
          const hit = runUnder(event);
          // Base lines carry no mark, so there is nothing to explain on hover.
          if (!hit || hit.run.origin.kind === "base") return setTip(null);
          const info = modified.getLayoutInfo();
          const box = modified.getDomNode()?.getBoundingClientRect();
          const file = live.current.diff;
          if (!box || !file) return;
          const top = modified.getTopForLineNumber(hit.line) - modified.getScrollTop();
          setTip((previous) =>
            previous?.line === hit.line && previous.diff === file
              ? previous
              : {
                  line: hit.line,
                  diff: file,
                  x: box.left + info.decorationsLeft + info.decorationsWidth + 6,
                  y: box.top + top,
                },
          );
        }),
        modified.onMouseLeave(() => setTip(null)),
        modified.onDidScrollChange(() => setTip(null)),
        modified.onMouseDown((event) => {
          // A click on any line's gutter (a base line too) explains that line.
          if (!event.event.leftButton) return;
          const hit = runUnder(event);
          if (hit) openLineOrigin(workspace, hit.line);
        }),
        modified.addAction({
          id: "why.openLineOrigin",
          label: "Show where this line came from",
          keybindings: [monaco.KeyCode.KeyO],
          run: (editor) => {
            const line = editor.getPosition()?.lineNumber;
            if (line === undefined || live.current.gate.status !== "shown") return;
            openLineOrigin(workspace, line);
          },
        }),
        ...(["next", "previous"] as const).map((direction) =>
          modified.addAction({
            id: `why.${direction}UnknownLine`,
            label: `Go to the ${direction} line with an unknown origin`,
            keybindings: [direction === "next" ? monaco.KeyCode.Period : monaco.KeyCode.Comma],
            run: (editor) => {
              const { gate: current } = live.current;
              if (current.status !== "shown") return;
              const from = editor.getPosition()?.lineNumber ?? 1;
              const target = adjacentUnknownLine(current.origins.runs, from, direction);
              if (target === null) return;
              editor.setPosition({ lineNumber: target, column: 1 });
              editor.revealLineInCenter(target, monaco.editor.ScrollType.Immediate);
              // The inspector follows the cursor but is not forced open by a jump.
              workspace.getState().lineOrigins.getState().selectLine(target);
            },
          }),
        ),
      );
    });
    return () => disposables.forEach((disposable) => disposable.dispose());
  }, [editors, workspace]);

  // Resolved on EVERY render from the hovered line and the current runs (never from a
  // stored run index): if a refresh splits or merges runs under a resting pointer, the
  // card follows the line. A different file clears it.
  const hovered =
    tip && tip.diff === shown && gate.status === "shown"
      ? markedRunAt(gate.origins.runs, tip.line)
      : undefined;
  return {
    tip:
      tip && hovered ? <LineOriginTip run={hovered} line={tip.line} x={tip.x} y={tip.y} /> : null,
  };
}
