import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { DiffEditor, type MonacoDiffEditor } from "@monaco-editor/react";
import type { FileDiff } from "@/contracts";
import { finishFileSwitch } from "@/lib/perf";
import { editorSettingsStore } from "@/stores/editor-settings";
import { selectDiffMode } from "@/stores/workspace";
import { useWorkspace } from "@/stores/workspace-context";
import { useStore } from "zustand";
import { createBufferedDiff, type DiffSlot, type DiffTarget } from "./buffered-diff";
import { detectLanguage } from "./detect-language";
import { buildEditorOptions } from "./editor-options";
import { createHunkNavigator } from "./hunk-navigation";
import { useLineOriginGutter } from "./line-origin-gutter";
import { createModelManager } from "./model-manager";
import { defineEditorThemes, monaco } from "./monaco-setup";
import { whenDiffReady } from "./ready-gate";
import { modelEdit } from "./text-edit";

/**
 * Rewrite a model's text in place with the smallest edit. `applyEdits` keeps the
 * model (so the editor keeps its scroll, cursor and expanded regions) and
 * works on a read-only editor: `readOnly` only blocks the user's typing.
 */
function replaceModelText(model: monaco.editor.ITextModel, value: string) {
  const edit = modelEdit(model.getValue(), value);
  if (!edit) return;
  const range = monaco.Range.fromPositions(
    model.getPositionAt(edit.start),
    model.getPositionAt(edit.end),
  );
  model.applyEdits([{ range, text: edit.text }]);
}

type MonacoSlot = DiffSlot & { navigator(): ReturnType<typeof createHunkNavigator> | undefined };

/** One diff editor plus the models it shows (see `createBufferedDiff` for why there are two). */
function createMonacoSlot(editor: MonacoDiffEditor): MonacoSlot {
  const registered = monaco.languages.getLanguages();
  let current: DiffTarget | null = null;
  let navigator: ReturnType<typeof createHunkNavigator> | undefined;
  let cancelGate: (() => void) | undefined;

  const models = createModelManager(
    ({ key, path, value }) =>
      monaco.editor.createModel(
        value,
        detectLanguage(path, registered),
        monaco.Uri.from({ scheme: "why-diff", path: key }),
      ),
    (pair) => editor.setModel(pair),
    replaceModelText,
  );

  const sameFile = (a: DiffTarget, b: DiffTarget) =>
    a.worktreeId === b.worktreeId &&
    a.mode === b.mode &&
    a.diff.path === b.diff.path &&
    a.diff.oldPath === b.diff.oldPath;

  return {
    navigator: () => navigator,
    isSameFile: (target) => current !== null && sameFile(current, target),
    show(target, ready) {
      const same = current !== null && sameFile(current, target);
      current = target;
      const { worktreeId, mode, diff } = target;
      if (same) {
        models.show({ worktreeId, mode, diff }); // live edit: rewritten in place
        return;
      }
      cancelGate?.();
      // Arm the gate BEFORE attaching, so no "diff computed" update can slip past.
      navigator = createHunkNavigator(editor);
      const arrived = navigator;
      cancelGate = whenDiffReady(editor, () => {
        // Land on the first change (3 lines of context above it), not on line 1.
        arrived.revealFirstChange(3);
        ready();
      });
      models.show({ worktreeId, mode, diff });
    },
    clear() {
      cancelGate?.();
      cancelGate = undefined;
      models.clear();
      current = null;
      navigator = undefined;
    },
  };
}

type Editors = [MonacoDiffEditor | null, MonacoDiffEditor | null];

export default function MonacoDiff({
  diff,
  stale = false,
  onReveal,
}: {
  diff: FileDiff | null;
  /** The shown file is the previous one, waiting for the next (dimmed slightly). */
  stale?: boolean;
  /** The file that is now actually visible (or `null`: nothing). */
  onReveal?: (diff: FileDiff | null) => void;
}) {
  const [editors, setEditors] = useState<Editors>([null, null]);
  const [front, setFront] = useState<0 | 1>(0);
  const [hasFront, setHasFront] = useState(false);
  // The file the front editor really shows (not `diff`, which may be the next one still loading).
  const [frontDiff, setFrontDiff] = useState<FileDiff | null>(null);
  const worktreeId = useWorkspace((state) => state.worktreeId);
  const mode = useWorkspace(selectDiffMode);
  const layout = useWorkspace((state) => state.diffLayout);
  const registerNavigation = useWorkspace((state) => state.registerDiffNavigation);
  const settings = useStore(editorSettingsStore, (state) => state.settings);
  const options = useMemo(() => buildEditorOptions(settings, layout), [settings, layout]);
  const viewer = useRef<ReturnType<typeof createBufferedDiff> | null>(null);
  const onRevealRef = useRef(onReveal);
  const bothMounted = editors[0] !== null && editors[1] !== null;
  const { tip } = useLineOriginGutter({ editors, front, shown: hasFront ? frontDiff : null });

  useEffect(() => {
    onRevealRef.current = onReveal;
  });

  // Monaco retains the original editor for inline layout, including its tab stop.
  useLayoutEffect(() => {
    for (const editor of editors) {
      editor
        ?.getOriginalEditor()
        .getContainerDomNode()
        .toggleAttribute("inert", layout === "unified");
    }
  }, [editors, layout]);

  // The viewer exists while both editors do.
  useLayoutEffect(() => {
    const [first, second] = editors;
    if (!first || !second) return;
    const slots = [createMonacoSlot(first), createMonacoSlot(second)] as const;
    const timers = new Set<ReturnType<typeof setTimeout>>();
    let unregister: (() => void) | undefined;
    const created = createBufferedDiff(slots, {
      onFront(index, target, swapped) {
        setFront(index);
        setHasFront(true);
        setFrontDiff(target.diff);
        unregister?.();
        const navigator = slots[index].navigator();
        if (navigator) unregister = registerNavigation(target.diff, navigator.move);
        onRevealRef.current?.(target.diff);
        if (swapped) finishFileSwitch(target.diff.path);
      },
      onEmpty() {
        unregister?.();
        unregister = undefined;
        setHasFront(false);
        setFrontDiff(null);
        onRevealRef.current?.(null);
      },
      later(action, ms) {
        const timer = setTimeout(() => {
          timers.delete(timer);
          action();
        }, ms);
        timers.add(timer);
      },
    });
    viewer.current = created;
    return () => {
      viewer.current = null;
      unregister?.();
      for (const timer of timers) clearTimeout(timer);
      slots[0].clear();
      slots[1].clear();
    };
  }, [editors, registerNavigation]);

  // Feed the viewer the file to show.
  useLayoutEffect(() => {
    if (!bothMounted) return;
    if (!diff || worktreeId === null) viewer.current?.clear();
    else viewer.current?.show({ worktreeId, mode, diff });
  }, [bothMounted, diff, mode, worktreeId]);

  // Fonts that finish loading after the first measure would leave Monaco with
  // fallback metrics (misaligned selection and guides): measure again.
  useEffect(() => {
    void document.fonts?.ready.then(() => monaco.editor.remeasureFonts());
  }, []);
  useEffect(() => {
    const frame = requestAnimationFrame(() => monaco.editor.remeasureFonts());
    return () => cancelAnimationFrame(frame);
  }, [settings.fontFamily, settings.fontSize, settings.ligatures]);

  function mount(index: 0 | 1) {
    return (instance: MonacoDiffEditor) => {
      // The React wrapper creates an empty pair during setup. From here on we own the pair.
      const placeholder = instance.getModel();
      instance.setModel(null);
      placeholder?.original.dispose();
      if (placeholder?.modified !== placeholder?.original) placeholder?.modified.dispose();
      setEditors((previous) => {
        const next: Editors = [previous[0], previous[1]];
        next[index] = instance;
        return next;
      });
    };
  }

  return (
    <>
      <div className="diff-editor-stack" data-stale={stale || undefined}>
        {([0, 1] as const).map((index) => {
          const isFront = hasFront && front === index;
          return (
            <div
              key={index}
              className="diff-editor-host"
              data-diff-editor
              data-front={isFront}
              inert={!isFront}
              aria-hidden={!isFront}
            >
              <DiffEditor
                theme={settings.theme}
                options={options}
                beforeMount={defineEditorThemes}
                onMount={mount(index)}
                loading={null}
                keepCurrentOriginalModel
                keepCurrentModifiedModel
              />
            </div>
          );
        })}
      </div>
      {tip}
    </>
  );
}
