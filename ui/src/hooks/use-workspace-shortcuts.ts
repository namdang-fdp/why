import { useEffect } from "react";
import { diffModes } from "@/stores/workspace";
import type { Workspace } from "@/stores/workspace-context";

export const paletteKeys =
  typeof navigator !== "undefined" && /mac|iphone|ipad/i.test(navigator.platform)
    ? "⌘ K"
    : "Ctrl K";

export const shortcutGroups = [
  {
    title: "Navigate",
    shortcuts: [
      { label: "Next file", keys: ["j"] },
      { label: "Previous file", keys: ["k"] },
      { label: "Previous worktree", keys: ["["] },
      { label: "Next worktree", keys: ["]"] },
    ],
  },
  {
    title: "Compare",
    shortcuts: diffModes.map(({ label, key }) => ({ label, keys: [key] })),
  },
  {
    title: "Workspace",
    shortcuts: [
      { label: "Toggle inspector", keys: ["i"] },
      { label: "Search commands", keys: paletteKeys.split(" ") },
      { label: "Keyboard shortcuts", keys: ["?"] },
    ],
  },
];

function isEditing(target: EventTarget | null) {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable ||
      target.closest("input, textarea, select, [contenteditable]:not([contenteditable=false])") !==
        null)
  );
}

export function useWorkspaceShortcuts(store: Workspace, helpOpen: boolean, openHelp: () => void) {
  useEffect(() => {
    function handleKey(event: KeyboardEvent) {
      const state = store.getState();
      if (
        event.defaultPrevented ||
        event.isComposing ||
        event.altKey ||
        state.paletteOpen ||
        helpOpen ||
        isEditing(event.target)
      )
        return;

      if (event.metaKey || event.ctrlKey) {
        if (event.key.toLowerCase() === "k" && !event.shiftKey) {
          event.preventDefault();
          if (!event.repeat) state.setPaletteOpen(true);
        }
        return;
      }

      const key = event.key;
      const mode = diffModes.find((item) => item.key === key)?.mode;
      if (mode) {
        event.preventDefault();
        void state.setDiffMode(mode);
        return;
      }

      switch (key) {
        case "j":
        case "k":
          event.preventDefault();
          void state.moveFile(key === "j" ? 1 : -1);
          break;
        case "[":
        case "]":
          event.preventDefault();
          void state.moveWorktree(key === "]" ? 1 : -1);
          break;
        case "i":
          event.preventDefault();
          if (!event.repeat) {
            if (document.activeElement?.closest("#why-inspector")) {
              document.getElementById("shell-inspector")?.focus();
            }
            state.toggleInspector();
          }
          break;
        case "?":
          event.preventDefault();
          if (!event.repeat) openHelp();
          break;
      }
    }
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [store, helpOpen, openHelp]);
}
