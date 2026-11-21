import type { CSSProperties } from "react";
import { diffAlpha, palettes } from "@/components/diff/editor-themes";
import { cssFontFamily, type EditorSettings, type WhitespaceMode } from "@/stores/editor-settings";

const lines = [
  { marker: " ", text: "const visible = items.filter(" },
  { marker: "-", text: "  item => item.active," },
  { marker: "+", text: "  item => item.status !== 'archived', " },
  { marker: " ", text: ");" },
];

function showWhitespace(text: string, mode: WhitespaceMode) {
  const dots = (spaces: string) => "·".repeat(spaces.length);
  switch (mode) {
    case "all":
      return text.replace(/ +/g, dots);
    case "boundary":
      return text.replace(/^ +| {2,}| +$/g, dots);
    case "trailing":
      return text.replace(/ +$/, dots);
    case "none":
      return text;
  }
}

export function EditorSettingsPreview({ settings }: { settings: EditorSettings }) {
  const palette = palettes[settings.theme];
  const style: CSSProperties = {
    fontFamily: cssFontFamily(settings.fontFamily),
    fontSize: settings.fontSize,
    lineHeight: `${settings.lineHeight}px`,
    fontFeatureSettings: settings.ligatures ? "normal" : '"liga" off, "calt" off',
    backgroundColor: settings.theme === "why-dark" ? "var(--background)" : `#${palette.background}`,
    color: `#${palette.foreground}`,
  };

  return (
    <figure className="editor-preview">
      <figcaption id="editor-preview-label">Diff preview</figcaption>
      <pre aria-labelledby="editor-preview-label" style={style}>
        {lines.map(({ marker, text }) => {
          const tone = marker === "+" ? palette.addedBg : marker === "-" ? palette.removedBg : null;
          return (
            <span
              key={text}
              style={{ backgroundColor: tone ? `#${tone}${diffAlpha.line}` : undefined }}
            >
              {marker} {showWhitespace(text, settings.whitespace)}
            </span>
          );
        })}
      </pre>
    </figure>
  );
}
