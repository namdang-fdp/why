import { describe, expect, it } from "vitest";
import { diffAlpha, palettes, type ThemePalette } from "./editor-themes";

function luminance(hex: string) {
  const channel = (offset: number) => {
    const value = Number.parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
}

function contrast(a: string, b: string) {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return ((light ?? 0) + 0.05) / ((dark ?? 0) + 0.05);
}

/** The colour of `hex` at `alpha` (0-255) painted over `base`. */
function over(base: string, hex: string, alpha: number) {
  const mix = (offset: number) => {
    const fg = Number.parseInt(hex.slice(offset, offset + 2), 16);
    const bg = Number.parseInt(base.slice(offset, offset + 2), 16);
    return Math.round(fg * (alpha / 255) + bg * (1 - alpha / 255))
      .toString(16)
      .padStart(2, "0");
  };
  return mix(0) + mix(2) + mix(4);
}

const textTokens = [
  "foreground",
  "keyword",
  "string",
  "number",
  "type",
  "function",
  "variable",
  "operator",
  "constant",
] as const satisfies ReadonlyArray<keyof ThemePalette>;

describe.each(Object.entries(palettes))("%s palette", (_name, palette) => {
  it.each(textTokens)("%s is readable on the editor background (>= 4.5:1)", (token) => {
    expect(contrast(palette[token], palette.background)).toBeGreaterThanOrEqual(4.5);
  });

  it("keeps comments dim but legible (>= 3.5:1) and below the main text", () => {
    expect(contrast(palette.comment, palette.background)).toBeGreaterThanOrEqual(3.5);
    expect(contrast(palette.comment, palette.background)).toBeLessThan(
      contrast(palette.foreground, palette.background),
    );
  });

  it("stays readable (>= 4:1) on top of the diff backgrounds, even on a wholly added line", () => {
    const line = over(palette.background, palette.addedBg, Number.parseInt(diffAlpha.line, 16));
    // Changed text is painted over the line colour on a new line.
    const stacked = over(line, palette.addedBg, Number.parseInt(diffAlpha.text, 16));
    // 4:1, not 4.5: on a wholly new line the line tint and the changed-text tint
    // stack (the worst case; elsewhere only one applies).
    for (const token of textTokens) {
      expect(contrast(palette[token], stacked)).toBeGreaterThanOrEqual(4);
    }
    const removed = over(
      over(palette.background, palette.removedBg, Number.parseInt(diffAlpha.line, 16)),
      palette.removedBg,
      Number.parseInt(diffAlpha.text, 16),
    );
    for (const token of textTokens) {
      expect(contrast(palette[token], removed)).toBeGreaterThanOrEqual(4);
    }
  });
});

describe("diff tint strength", () => {
  it("keeps line backgrounds at 12-16% and changed text at 25% or less", () => {
    const percent = (hex: string) => (Number.parseInt(hex, 16) / 255) * 100;
    expect(percent(diffAlpha.line)).toBeGreaterThanOrEqual(12);
    expect(percent(diffAlpha.line)).toBeLessThanOrEqual(16);
    expect(percent(diffAlpha.text)).toBeLessThanOrEqual(25);
    expect(percent(diffAlpha.gutter)).toBeLessThanOrEqual(25);
  });
});
