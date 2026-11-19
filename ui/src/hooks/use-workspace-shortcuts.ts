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
      { label: "Next change, then next file", keys: ["n"] },
      { label: "Previous change", keys: ["p"] },
      { label: "Previous worktree", keys: ["["] },
      { label: "Next worktree", keys: ["]"] },
    ],
  },
  {
    title: "Compare",
    shortcuts: [
      ...diffModes.map(({ label, key }) => ({ label, keys: [key] })),
      { label: "Toggle unified/split", keys: ["u"] },
      { label: "Focus file list from editor", keys: ["Esc"] },
    ],
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
      target.closest(
        "input, textarea, select, [contenteditable]:not([contenteditable=false]), [data-diff-editor]",
      ) !== null)
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
        case "n":
        case "p":
          event.preventDefault();
          void state.moveChange(key === "n" ? "next" : "previous");
          break;
        case "u":
          event.preventDefault();
          if (!event.repeat) state.toggleDiffLayout();
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
    // Capture Escape before Monaco consumes it. Other keys follow the normal input guard.
    function leaveEditor(event: KeyboardEvent) {
      if (
        event.key !== "Escape" ||
        event.isComposing ||
        event.altKey ||
        event.ctrlKey ||
        event.metaKey ||
        event.shiftKey ||
        helpOpen ||
        store.getState().paletteOpen ||
        !(event.target instanceof Element) ||
        !event.target.closest("[data-diff-editor]")
      )
        return;
      const row = document.querySelector<HTMLButtonElement>(
        '#changed-file-list button[aria-current="true"]',
      );
      if (!row) return;
      event.preventDefault();
      event.stopPropagation();
      row.focus();
    }
    window.addEventListener("keydown", leaveEditor, true);
    window.addEventListener("keydown", handleKey);
    return () => {
      window.removeEventListener("keydown", leaveEditor, true);
      window.removeEventListener("keydown", handleKey);
    };
  }, [store, helpOpen, openHelp]);
}
