/**
 * Colours of the two editor themes, as plain data (no Monaco import, so they
 * can be unit-tested: see `editor-themes.test.ts` for the contrast checks).
 *
 * Token colours are hex `RRGGBB`. "WHY Dark" is desaturated on purpose: code is
 * read for hours, so no token should shout; the editor background itself still
 * comes from the app's CSS tokens (see `monaco-setup.ts`). "Tokyo Night Storm"
 * is written from the publicly documented palette of that style (a blue-grey
 * ground with soft purple, green, orange and cyan).
 */

export type ThemePalette = {
  /** Editor ground. WHY Dark overrides it at runtime from `--background`. */
  background: string;
  foreground: string;
  lineNumber: string;
  activeLineNumber: string;
  comment: string;
  keyword: string;
  string: string;
  number: string;
  type: string;
  function: string;
  variable: string;
  operator: string;
  constant: string;
  /** Marks (overview ruler, gutter) for added / removed lines. */
  added: string;
  removed: string;
  /** The tints painted behind changed lines and text (alpha is applied on top). */
  addedBg: string;
  removedBg: string;
  selection: string;
  indentGuide: string;
  whitespace: string;
};

export const palettes = {
  "why-dark": {
    background: "0a0a0a",
    foreground: "d8d8dc",
    lineNumber: "5f6068",
    activeLineNumber: "a1a1aa",
    comment: "7c7f8c",
    keyword: "b3a9d6",
    string: "a6c3a0",
    number: "d3b88a",
    type: "8fb3c7",
    function: "e6e6ea",
    variable: "d8d8dc",
    operator: "b8b8c0",
    constant: "c9a9c9",
    added: "6fcf97",
    removed: "ef7b7b",
    addedBg: "6fcf97",
    removedBg: "ef7b7b",
    selection: "ffffff1f",
    indentGuide: "ffffff14",
    whitespace: "ffffff26",
  },
  "tokyo-night-storm": {
    background: "24283b",
    foreground: "a9b1d6",
    lineNumber: "545c7e",
    activeLineNumber: "a9b1d6",
    comment: "7982b4",
    keyword: "bb9af7",
    string: "9ece6a",
    number: "ff9e64",
    type: "2ac3de",
    function: "7aa2f7",
    variable: "c0caf5",
    operator: "89ddff",
    constant: "ff9e64",
    added: "9ece6a",
    removed: "f7768e",
    // Diff tints in Tokyo Night's own family (a teal and a rose) but deeper than
    // the bright token colours, so stacked tints stay dark enough for the text.
    addedBg: "1f7a8c",
    removedBg: "9a3a48",
    selection: "364a8299",
    indentGuide: "3b426180",
    whitespace: "3b4261",
  },
} satisfies Record<string, ThemePalette>;

/**
 * Diff backgrounds as alpha bytes. A wholly new line gets the line colour AND the
 * changed-text colour painted over it, so both stay low: line ~12% (0x1f), changed
 * text ~22% (0x38), gutter ~18% (0x2e).
 */
export const diffAlpha = { line: "1f", text: "38", gutter: "2e", overview: "a8" } as const;

/** Monaco token rules for a palette (token prefixes match their sub-tokens). */
export function tokenRules(p: ThemePalette) {
  const rule = (token: string, color: string, fontStyle?: string) => ({
    token,
    foreground: color,
    ...(fontStyle && { fontStyle }),
  });
  return [
    rule("", p.foreground),
    rule("comment", p.comment, "italic"),
    rule("keyword", p.keyword),
    rule("keyword.operator", p.operator),
    rule("storage", p.keyword),
    rule("string", p.string),
    rule("string.key.json", p.type),
    rule("string.value.json", p.string),
    rule("number", p.number),
    rule("constant", p.constant),
    rule("type", p.type),
    rule("type.identifier", p.type),
    rule("class", p.type),
    rule("interface", p.type),
    rule("function", p.function),
    rule("support.function", p.function),
    rule("identifier", p.variable),
    rule("variable", p.variable),
    rule("operator", p.operator),
    rule("delimiter", p.operator),
    rule("tag", p.keyword),
    rule("attribute.name", p.number),
    rule("attribute.value", p.string),
    rule("regexp", p.constant),
    rule("annotation", p.constant),
    rule("meta", p.operator),
  ];
}
