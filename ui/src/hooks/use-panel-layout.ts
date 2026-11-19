import { useState } from "react";
import type { Layout, LayoutChangedMeta } from "react-resizable-panels";

/**
 * A resizable group's layout, remembered in localStorage under `key`.
 *
 * Only sizes the user dragged are saved (`isUserInteraction`); a layout the
 * library computed itself (a window resize) is not. A saved layout that does
 * not match `fallback`'s panels, or does not add up to 100, is ignored.
 */
export function usePanelLayout(key: string, fallback: Layout) {
  const [layout, setLayout] = useState(() => {
    try {
      const raw = window.localStorage.getItem(key);
      if (raw === null) return fallback;
      const saved: unknown = JSON.parse(raw);
      if (!saved || typeof saved !== "object") return fallback;

      const restored: Layout = {};
      for (const id of Object.keys(fallback)) {
        const size: unknown = Reflect.get(saved, id);
        if (typeof size !== "number" || !Number.isFinite(size) || size <= 0 || size >= 100) {
          return fallback;
        }
        restored[id] = size;
      }
      const sum = Object.values(restored).reduce((total, size) => total + size, 0);
      return Math.abs(sum - 100) < 0.1 ? restored : fallback;
    } catch {
      return fallback;
    }
  });

  return {
    defaultLayout: layout,
    onLayoutChanged(next: Layout, meta: LayoutChangedMeta) {
      if (!meta.isUserInteraction) return;
      const saved = meta.requestedLayout ?? next;
      setLayout(saved);
      try {
        window.localStorage.setItem(key, JSON.stringify(saved));
      } catch {
        // The panels remain resizable when webview storage is unavailable.
      }
    },
  };
}

/** Save a layout the same way `usePanelLayout` does (for a "reset" that is not a drag). */
export function saveLayout(key: string, layout: Layout) {
  try {
    window.localStorage.setItem(key, JSON.stringify(layout));
  } catch {
    // Storage unavailable: the layout still applies for this session.
  }
}
