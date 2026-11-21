import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

/**
 * Which tab of the Why inspector the reviewer last chose.
 *
 * A module-level store on purpose: the inspector unmounts when it is hidden,
 * and the choice must outlive that (but not the app, so no localStorage). It is
 * a view preference, not workspace data, so it stays out of the big workspace store.
 */
export type InspectorTab = "file" | "worktree";

type InspectorViewState = {
  tab: InspectorTab;
  setTab: (tab: InspectorTab) => void;
};

export const inspectorViewStore = createStore<InspectorViewState>((set) => ({
  tab: "file",
  setTab: (tab) => set({ tab }),
}));

export function useInspectorTab() {
  return useStore(inspectorViewStore, (state) => state.tab);
}

/** Tests only: start every test from the default tab. */
export function resetInspectorView() {
  inspectorViewStore.setState({ tab: "file" });
}
