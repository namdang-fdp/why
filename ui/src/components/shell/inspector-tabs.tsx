import { useRef, type KeyboardEvent } from "react";
import { inspectorViewStore, useInspectorTab, type InspectorTab } from "@/stores/inspector-view";

const tabs: { id: InspectorTab; label: string }[] = [
  { id: "file", label: "File" },
  { id: "worktree", label: "Worktree" },
];

/** Left/Right (and wrap) move between tabs. Home/End/Up/Down stay the inspector's own keys. */
export function InspectorTabs() {
  const current = useInspectorTab();
  const list = useRef<HTMLDivElement>(null);

  function keyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const index = tabs.findIndex((tab) => tab.id === current);
    const step = event.key === "ArrowRight" ? 1 : -1;
    const next = tabs[(index + step + tabs.length) % tabs.length];
    if (!next) return;
    inspectorViewStore.getState().setTab(next.id);
    list.current?.querySelector<HTMLButtonElement>(`#inspector-tab-${next.id}`)?.focus();
  }

  return (
    <div
      ref={list}
      className="inspector-tabs"
      role="tablist"
      aria-label="Inspector view"
      onKeyDown={keyDown}
    >
      {tabs.map((tab) => (
        <button
          key={tab.id}
          id={`inspector-tab-${tab.id}`}
          type="button"
          role="tab"
          aria-selected={tab.id === current}
          aria-controls="inspector-tabpanel"
          // Roving tabindex: one tab stop for the whole tablist.
          tabIndex={tab.id === current ? 0 : -1}
          onClick={() => inspectorViewStore.getState().setTab(tab.id)}
        >
          {tab.label}
        </button>
      ))}
    </div>
  );
}
