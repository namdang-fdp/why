import { createStore } from "zustand/vanilla";

export type WhitespaceMode = "none" | "boundary" | "trailing" | "all";
export type EditorTheme = "why-dark" | "tokyo-night-storm";

export type EditorSettings = {
  fontFamily: string;
  /** In px. */
  fontSize: number;
  /** In px. */
  lineHeight: number;
  ligatures: boolean;
  whitespace: WhitespaceMode;
  theme: EditorTheme;
};

export const defaultEditorSettings: EditorSettings = {
  // The first installed face wins; `monospace` is appended when a stack is used.
  fontFamily:
    '"CaskaydiaCove Nerd Font Mono", "Cascadia Code", "JetBrains Mono", "Geist Mono", ui-monospace, monospace',
  fontSize: 14,
  lineHeight: 22,
  ligatures: true,
  // Trailing whitespace is a review-relevant mistake; `····` for every indent is noise.
  whitespace: "trailing",
  theme: "why-dark",
};

export const themeLabels = {
  "why-dark": "WHY Dark",
  "tokyo-night-storm": "Tokyo Night Storm",
} satisfies Record<EditorTheme, string>;

export const whitespaceLabels = {
  none: "None",
  trailing: "Trailing only",
  boundary: "Boundaries",
  all: "All",
} satisfies Record<WhitespaceMode, string>;

export const fontSizeRange = { min: 10, max: 28 } as const;
export const lineHeightRange = { min: 14, max: 44 } as const;

const storageKey = "why:editor-settings:v1";

const clamp = (value: number, min: number, max: number) =>
  Math.min(max, Math.max(min, Math.round(value)));

/** Accept only values of the right type and range; anything else falls back to the default. */
export function sanitizeEditorSettings(input: unknown): EditorSettings {
  const base = defaultEditorSettings;
  if (typeof input !== "object" || input === null) return base;
  const read = (key: string): unknown => Reflect.get(input, key);
  const family = read("fontFamily");
  const size = read("fontSize");
  const height = read("lineHeight");
  const ligatures = read("ligatures");
  const whitespace = read("whitespace");
  const theme = read("theme");
  return {
    fontFamily: typeof family === "string" && family.trim() ? family.trim() : base.fontFamily,
    fontSize:
      typeof size === "number" && Number.isFinite(size)
        ? clamp(size, fontSizeRange.min, fontSizeRange.max)
        : base.fontSize,
    lineHeight:
      typeof height === "number" && Number.isFinite(height)
        ? clamp(height, lineHeightRange.min, lineHeightRange.max)
        : base.lineHeight,
    ligatures: typeof ligatures === "boolean" ? ligatures : base.ligatures,
    whitespace:
      typeof whitespace === "string" && Object.hasOwn(whitespaceLabels, whitespace)
        ? (whitespace as WhitespaceMode)
        : base.whitespace,
    theme:
      typeof theme === "string" && Object.hasOwn(themeLabels, theme)
        ? (theme as EditorTheme)
        : base.theme,
  };
}

function load(): EditorSettings {
  try {
    const raw = window.localStorage.getItem(storageKey);
    return raw === null ? defaultEditorSettings : sanitizeEditorSettings(JSON.parse(raw));
  } catch {
    return defaultEditorSettings; // blocked storage or a corrupt value
  }
}

function save(settings: EditorSettings) {
  try {
    window.localStorage.setItem(storageKey, JSON.stringify(settings));
  } catch {
    // Keep the change for this session when storage is unavailable.
  }
}

export type EditorSettingsStore = {
  settings: EditorSettings;
  update: (patch: Partial<EditorSettings>) => void;
  reset: () => void;
};

export function createEditorSettingsStore() {
  return createStore<EditorSettingsStore>((set, get) => ({
    settings: load(),
    update: (patch) => {
      const settings = sanitizeEditorSettings({ ...get().settings, ...patch });
      set({ settings });
      save(settings);
    },
    reset: () => {
      set({ settings: defaultEditorSettings });
      save(defaultEditorSettings);
    },
  }));
}

/** The app-wide store. Tests build their own. */
export const editorSettingsStore = createEditorSettingsStore();

/**
 * A font stack for CSS/Monaco: the user's list, with a generic `monospace` at the
 * end so a missing face never falls back to a proportional one.
 */
export function cssFontFamily(fontFamily: string) {
  return /(^|,)\s*monospace\s*$/i.test(fontFamily) ? fontFamily : `${fontFamily}, monospace`;
}
