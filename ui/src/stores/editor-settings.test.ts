import { afterEach, describe, expect, it } from "vitest";
import {
  createEditorSettingsStore,
  cssFontFamily,
  defaultEditorSettings,
  sanitizeEditorSettings,
} from "./editor-settings";

// A tiny localStorage stand-in (these tests run in the node environment).
function stubStorage(initial: Record<string, string> = {}, throws = false) {
  const data = { ...initial };
  const storage = {
    getItem: (key: string) => {
      if (throws) throw new Error("blocked");
      return data[key] ?? null;
    },
    setItem: (key: string, value: string) => {
      if (throws) throw new Error("blocked");
      data[key] = value;
    },
  };
  Object.defineProperty(globalThis, "window", {
    value: { localStorage: storage },
    configurable: true,
  });
  return data;
}

afterEach(() => {
  Reflect.deleteProperty(globalThis, "window");
});

describe("editor settings", () => {
  it("defaults to the calm reading setup", () => {
    expect(defaultEditorSettings).toMatchObject({
      fontSize: 14,
      lineHeight: 22,
      ligatures: true,
      whitespace: "trailing",
      theme: "why-dark",
    });
    expect(defaultEditorSettings.fontFamily).toContain("CaskaydiaCove Nerd Font Mono");
  });

  it("persists changes and restores them", () => {
    const data = stubStorage();
    const store = createEditorSettingsStore();
    store.getState().update({ fontSize: 15, lineHeight: 24, theme: "tokyo-night-storm" });
    expect(JSON.parse(data["why:editor-settings:v1"] ?? "{}")).toMatchObject({
      fontSize: 15,
      lineHeight: 24,
      theme: "tokyo-night-storm",
    });
    const again = createEditorSettingsStore();
    expect(again.getState().settings).toMatchObject({ fontSize: 15, lineHeight: 24 });
  });

  it("clamps numbers and rejects unknown enum values", () => {
    expect(
      sanitizeEditorSettings({ fontSize: 999, lineHeight: 1, whitespace: "huge", theme: "x" }),
    ).toMatchObject({
      fontSize: 28,
      lineHeight: 14,
      whitespace: "trailing",
      theme: "why-dark",
    });
    expect(sanitizeEditorSettings({ fontSize: "big", ligatures: "yes", fontFamily: "  " })).toEqual(
      defaultEditorSettings,
    );
    expect(sanitizeEditorSettings(null)).toEqual(defaultEditorSettings);
  });

  it("survives a corrupt or blocked storage", () => {
    stubStorage({ "why:editor-settings:v1": "{not json" });
    expect(createEditorSettingsStore().getState().settings).toEqual(defaultEditorSettings);
    stubStorage({}, true);
    const store = createEditorSettingsStore();
    expect(() => store.getState().update({ fontSize: 16 })).not.toThrow();
    expect(store.getState().settings.fontSize).toBe(16); // kept for the session
  });

  it("reset returns to the defaults", () => {
    stubStorage();
    const store = createEditorSettingsStore();
    store.getState().update({ fontSize: 20, ligatures: false });
    store.getState().reset();
    expect(store.getState().settings).toEqual(defaultEditorSettings);
  });

  it("always ends a font stack with monospace", () => {
    expect(cssFontFamily('"Fira Code"')).toBe('"Fira Code", monospace');
    expect(cssFontFamily("A, monospace")).toBe("A, monospace");
  });
});
