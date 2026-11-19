import type { editor } from "monaco-editor";
import { cssFontFamily, type EditorSettings } from "@/stores/editor-settings";

/**
 * Monaco options for reading a diff comfortably: no chrome a read-only view does
 * not need, generous gutters and padding, thin scrollbars, sticky scope headers.
 * User-tunable parts come from `settings`; the rest is fixed.
 */
export function buildEditorOptions(
  settings: EditorSettings,
  layout: "split" | "unified",
): editor.IStandaloneDiffEditorConstructionOptions {
  return {
    readOnly: true,
    originalEditable: false,
    automaticLayout: true,
    renderSideBySide: layout === "split",
    useInlineViewWhenSpaceIsLimited: false,
    hideUnchangedRegions: { enabled: true, contextLineCount: 3, minimumLineCount: 4 },
    ignoreTrimWhitespace: false,

    fontFamily: cssFontFamily(settings.fontFamily),
    fontSize: settings.fontSize,
    lineHeight: settings.lineHeight,
    fontLigatures: settings.ligatures,
    renderWhitespace: settings.whitespace,

    // A read-only view: nothing to highlight, nothing to blink.
    renderLineHighlight: "none",
    cursorBlinking: "solid",
    guides: { indentation: true, highlightActiveIndentation: false, bracketPairs: false },
    padding: { top: 12, bottom: 24 },
    lineDecorationsWidth: 14,
    lineNumbersMinChars: 4,
    glyphMargin: false,
    smoothScrolling: false,
    scrollBeyondLastLine: false,
    scrollbar: {
      verticalScrollbarSize: 8,
      horizontalScrollbarSize: 8,
      useShadows: false,
      alwaysConsumeMouseWheel: false,
    },
    // The enclosing function/struct stays pinned while scrolling a long hunk.
    stickyScroll: { enabled: true, maxLineCount: 3 },
    minimap: { enabled: false },
    renderOverviewRuler: true,
    overviewRulerBorder: false,

    lightbulb: { enabled: "off" as editor.ShowLightbulbIconMode },
    renderMarginRevertIcon: false,
    renderGutterMenu: false,
    contextmenu: false,
    find: { seedSearchStringFromSelection: "never", addExtraSpaceOnTop: false },
    originalAriaLabel: "Original file",
    modifiedAriaLabel: "Modified file",
  };
}
