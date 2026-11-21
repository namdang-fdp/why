import { createContext, useContext } from "react";
import { useStore } from "zustand";
import { workspaceStore } from "./workspace";

export type Workspace = typeof workspaceStore;
export const WorkspaceContext = createContext<Workspace>(workspaceStore);

export function useWorkspace<T>(selector: (state: ReturnType<Workspace["getState"]>) => T): T {
  const store = useContext(WorkspaceContext);
  return useStore(store, selector);
}

type SessionsState = ReturnType<ReturnType<Workspace["getState"]>["sessions"]["getState"]>;

export function useSessions<T>(selector: (state: SessionsState) => T): T {
  const store = useContext(WorkspaceContext);
  return useStore(store.getState().sessions, selector);
}

type EvidenceState = ReturnType<ReturnType<Workspace["getState"]>["evidence"]["getState"]>;

export function useEvidence<T>(selector: (state: EvidenceState) => T): T {
  const store = useContext(WorkspaceContext);
  return useStore(store.getState().evidence, selector);
}

type LineOriginsState = ReturnType<ReturnType<Workspace["getState"]>["lineOrigins"]["getState"]>;

export function useLineOrigins<T>(selector: (state: LineOriginsState) => T): T {
  const store = useContext(WorkspaceContext);
  return useStore(store.getState().lineOrigins, selector);
}
