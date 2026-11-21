import { useMemo } from "react";
import { useEvidence, useWorkspace } from "@/stores/workspace-context";
import { currentAttribution } from "./attribution-model";

/** Attribution for the selected worktree and comparison only (see `currentAttribution`). */
export function useCurrentAttribution() {
  const state = useEvidence((s) => s.attribution);
  const mode = useEvidence((s) => s.scope?.mode);
  const worktreeId = useWorkspace((s) => s.worktreeId);
  return useMemo(() => currentAttribution(state, worktreeId, mode), [state, worktreeId, mode]);
}
