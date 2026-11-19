import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor/editor/editor.api.js";
// These entry points add tokenizers and read-only UI contributions without language workers.
import "monaco-editor/languages/definitions/register.all.js";
import "monaco-editor/features/find/register.js";
import "monaco-editor/features/codicon/register.js";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";

self.MonacoEnvironment = { getWorker: () => new EditorWorker() };
loader.config({ monaco });

monaco.languages.register({
  id: "json",
  extensions: [".json", ".jsonc", ".har"],
  filenames: [".babelrc", ".eslintrc", ".prettierrc"],
});
monaco.languages.setMonarchTokensProvider("json", {
  tokenizer: {
    root: [
      [/"(?:[^"\\]|\\.)*"(?=\s*:)/, "string.key.json"],
      [/"(?:[^"\\]|\\.)*"/, "string.value.json"],
      [/-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/, "number"],
      [/\b(?:true|false|null)\b/, "keyword"],
      [/[{}[\]]/, "@brackets"],
      [/[:,]/, "delimiter"],
    ],
  },
});

import { diffAlpha, palettes, tokenRules, type ThemePalette } from "./editor-themes";
import type { EditorTheme } from "@/stores/editor-settings";

const themeIds = ["why-dark", "tokyo-night-storm"] as const satisfies readonly EditorTheme[];

/** Monaco colour keys for one palette (every colour `#RRGGBB[AA]`). */
function themeColors(p: ThemePalette, background: string) {
  const hex = (value: string) => `#${value}`;
  return {
    "editor.background": background,
    "editor.foreground": hex(p.foreground),
    "editorGutter.background": background,
    "editorLineNumber.foreground": hex(p.lineNumber),
    "editorLineNumber.activeForeground": hex(p.activeLineNumber),
    // Read-only: no current-line band, a quiet selection, a plain cursor.
    "editor.lineHighlightBackground": "#00000000",
    "editor.lineHighlightBorder": "#00000000",
    "editor.selectionBackground": hex(p.selection),
    "editor.inactiveSelectionBackground": hex(p.selection),
    "editorCursor.foreground": hex(p.foreground),
    "editorIndentGuide.background1": hex(p.indentGuide),
    "editorIndentGuide.activeBackground1": hex(p.indentGuide),
    "editorWhitespace.foreground": hex(p.whitespace),
    "editorStickyScroll.background": background,
    "editorStickyScrollHover.background": hex(p.indentGuide),
    "editorStickyScroll.shadow": "#00000000",
    // Thin scrollbars that only announce themselves when pointed at.
    "scrollbarSlider.background": "#ffffff14",
    "scrollbarSlider.hoverBackground": "#ffffff2b",
    "scrollbarSlider.activeBackground": "#ffffff3d",
    "editorOverviewRuler.border": "#00000000",
    "diffEditor.insertedLineBackground": hex(p.addedBg + diffAlpha.line),
    "diffEditor.removedLineBackground": hex(p.removedBg + diffAlpha.line),
    "diffEditor.insertedTextBackground": hex(p.addedBg + diffAlpha.text),
    "diffEditor.removedTextBackground": hex(p.removedBg + diffAlpha.text),
    "diffEditorGutter.insertedLineBackground": hex(p.addedBg + diffAlpha.gutter),
    "diffEditorGutter.removedLineBackground": hex(p.removedBg + diffAlpha.gutter),
    "diffEditorOverview.insertedForeground": hex(p.added + diffAlpha.overview),
    "diffEditorOverview.removedForeground": hex(p.removed + diffAlpha.overview),
    "diffEditor.diagonalFill": "#ffffff0d",
    "diffEditor.unchangedRegionBackground": hex(p.indentGuide),
    "diffEditor.unchangedRegionForeground": hex(p.lineNumber),
  };
}

/**
 * Register both editor themes. "WHY Dark" takes its ground from the app's own
 * CSS token so the editor and the shell are one surface; its token colours
 * and the diff tints come from `editor-themes.ts`.
 */
export function defineEditorThemes() {
  const tokens = getComputedStyle(document.documentElement);
  // Canvas converts the shell's OKLCH tokens to the sRGB hex Monaco expects.
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 1;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  const shellBackground = (() => {
    if (!context) return `#${palettes["why-dark"].background}`;
    context.fillStyle = tokens.getPropertyValue("--background").trim();
    context.fillRect(0, 0, 1, 1);
    const [red, green, blue] = context.getImageData(0, 0, 1, 1).data;
    return (
      "#" + [red, green, blue].map((value) => (value ?? 0).toString(16).padStart(2, "0")).join("")
    );
  })();

  for (const id of themeIds) {
    const palette = palettes[id];
    monaco.editor.defineTheme(id, {
      base: "vs-dark",
      inherit: false,
      rules: tokenRules(palette),
      colors: themeColors(palette, id === "why-dark" ? shellBackground : `#${palette.background}`),
    });
  }
}

export { monaco };
