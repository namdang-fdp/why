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

export const diffTheme = "why-dark";

export function defineDiffTheme() {
  const tokens = getComputedStyle(document.documentElement);
  // Canvas converts the shell's OKLCH tokens to the sRGB hex Monaco expects.
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 1;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) return;

  const color = (token: string) => {
    context.fillStyle = tokens.getPropertyValue(token).trim();
    context.fillRect(0, 0, 1, 1);
    const [red, green, blue] = context.getImageData(0, 0, 1, 1).data;
    return (
      "#" + [red, green, blue].map((value) => (value ?? 0).toString(16).padStart(2, "0")).join("")
    );
  };

  const background = color("--background");
  const foreground = color("--foreground");
  const muted = color("--muted-foreground");
  const added = color("--git-added");
  const deleted = color("--git-deleted");
  monaco.editor.defineTheme(diffTheme, {
    base: "vs-dark",
    inherit: true,
    rules: [],
    colors: {
      "editor.background": background,
      "editor.foreground": foreground,
      "editorLineNumber.foreground": muted,
      "editorLineNumber.activeForeground": foreground,
      "diffEditor.insertedLineBackground": added + "20",
      "diffEditor.removedLineBackground": deleted + "20",
      "diffEditor.insertedTextBackground": added + "49",
      "diffEditor.removedTextBackground": deleted + "49",
      "diffEditorGutter.insertedLineBackground": added + "28",
      "diffEditorGutter.removedLineBackground": deleted + "28",
      "diffEditorOverview.insertedForeground": added + "A8",
      "diffEditorOverview.removedForeground": deleted + "A8",
    },
  });
}

export { monaco };
