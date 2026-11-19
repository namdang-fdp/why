import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { DiffEditor, type MonacoDiffEditor } from "@monaco-editor/react";
import type { FileDiff } from "@/contracts";
import { selectDiffMode } from "@/stores/workspace";
import { useWorkspace } from "@/stores/workspace-context";
import { detectLanguage } from "./detect-language";
import { DiffSkeleton } from "./diff-skeleton";
import { createHunkNavigator } from "./hunk-navigation";
import { createModelManager } from "./model-manager";
import { defineDiffTheme, diffTheme, monaco } from "./monaco-setup";

const editorOptions: monaco.editor.IStandaloneDiffEditorConstructionOptions = {
  readOnly: true,
  originalEditable: false,
  automaticLayout: true,
  useInlineViewWhenSpaceIsLimited: false,
  hideUnchangedRegions: { enabled: true },
  minimap: { enabled: false },
  renderOverviewRuler: true,
  scrollBeyondLastLine: false,
  smoothScrolling: false,
  fontSize: 13,
  renderWhitespace: "boundary",
  ignoreTrimWhitespace: false,
  lightbulb: { enabled: monaco.editor.ShowLightbulbIconMode.Off },
  renderMarginRevertIcon: false,
  renderGutterMenu: false,
  contextmenu: false,
  find: { seedSearchStringFromSelection: "never", addExtraSpaceOnTop: false },
  originalAriaLabel: "Original file",
  modifiedAriaLabel: "Modified file",
};

export default function MonacoDiff({ diff }: { diff: FileDiff | null }) {
  const [editor, setEditor] = useState<MonacoDiffEditor | null>(null);
  const worktreeId = useWorkspace((state) => state.worktreeId);
  const mode = useWorkspace(selectDiffMode);
  const layout = useWorkspace((state) => state.diffLayout);
  const revealFirst = useWorkspace((state) => state.revealFirstChange);
  const registerNavigation = useWorkspace((state) => state.registerDiffNavigation);
  const options = useMemo(
    () => ({ ...editorOptions, renderSideBySide: layout === "split" }),
    [layout],
  );
  const isVisible = diff !== null;
  const models = useRef<ReturnType<typeof createModelManager<monaco.editor.ITextModel>> | null>(
    null,
  );

  useLayoutEffect(() => {
    // Monaco retains the original editor for inline layout, including its tab stop.
    editor
      ?.getOriginalEditor()
      .getContainerDomNode()
      .toggleAttribute("inert", layout === "unified");
  }, [editor, layout]);

  useLayoutEffect(() => {
    if (!editor) return;

    const registered = monaco.languages.getLanguages();
    const manager = createModelManager(
      ({ key, path, value }) =>
        monaco.editor.createModel(
          value,
          detectLanguage(path, registered),
          monaco.Uri.from({ scheme: "why-diff", path: key }),
        ),
      (pair) => editor.setModel(pair),
    );
    models.current = manager;
    return () => {
      manager.clear();
      models.current = null;
    };
  }, [editor]);

  useLayoutEffect(() => {
    const manager = models.current;
    if (!editor || !manager) return;
    if (!diff || worktreeId === null) {
      manager.clear();
      return;
    }

    manager.show({ worktreeId, mode, diff });
    const navigator = createHunkNavigator(editor);
    const unregister = registerNavigation(diff, navigator.move);

    // The worker can finish after another file has replaced this pair.
    let pending = revealFirst;
    function revealWhenReady() {
      if (!pending || !editor?.getLineChanges()) return;
      pending = false;
      navigator.revealFirst();
    }
    const subscription = editor.onDidUpdateDiff(revealWhenReady);
    revealWhenReady();
    return () => {
      subscription.dispose();
      unregister();
    };
  }, [diff, editor, mode, registerNavigation, revealFirst, worktreeId]);

  function mount(instance: MonacoDiffEditor) {
    // The React wrapper creates an empty pair during setup. From here on we own the pair.
    const placeholder = instance.getModel();
    instance.setModel(null);
    placeholder?.original.dispose();
    if (placeholder?.modified !== placeholder?.original) placeholder?.modified.dispose();
    setEditor(instance);
  }

  return (
    <div className="diff-editor-host" data-diff-editor inert={!isVisible} aria-hidden={!isVisible}>
      <DiffEditor
        theme={diffTheme}
        options={options}
        beforeMount={defineDiffTheme}
        onMount={mount}
        loading={<DiffSkeleton />}
        keepCurrentOriginalModel
        keepCurrentModifiedModel
      />
    </div>
  );
}
