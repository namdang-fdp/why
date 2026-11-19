import {
  ArrowRight,
  ChevronRight,
  GitBranch,
  GitPullRequest,
  PanelRight,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { diffModes, selectDiffMode, selectWorktree } from "@/stores/workspace";
import { useWorkspace } from "@/stores/workspace-context";
import { BranchDivergence, branchLabel, worktreeName } from "./change-labels";

export function TopBar() {
  const repository = useWorkspace((state) =>
    state.repositories.status === "ok"
      ? state.repositories.data.find((repo) => repo.id === state.repoId)
      : undefined,
  );
  const worktree = useWorkspace(selectWorktree);
  const mode = useWorkspace(selectDiffMode);
  const setMode = useWorkspace((state) => state.setDiffMode);
  const inspectorOpen = useWorkspace((state) => state.inspectorOpen);
  const toggleInspector = useWorkspace((state) => state.toggleInspector);

  return (
    <header className="top-bar">
      <div className="brand">
        <span className="brand-icon">
          <GitPullRequest size={15} aria-hidden />
        </span>
        <h1>WHY</h1>
      </div>
      <nav className="breadcrumbs" aria-label="Current location">
        {repository && (
          <span title={repository.rootPath} className="truncate">
            {repository.name}
          </span>
        )}
        {worktree && (
          <>
            <ChevronRight size={13} aria-hidden />
            <span aria-current="location" title={worktree.path} className="truncate">
              {worktreeName(worktree)}
            </span>
          </>
        )}
      </nav>
      {worktree && (
        <div className="branch-summary" aria-label="Branch comparison">
          <GitBranch size={14} aria-hidden />
          <span className="truncate" title={branchLabel(worktree)}>
            {branchLabel(worktree)}
          </span>
          {worktree.base && (
            <>
              <ArrowRight size={13} aria-hidden />
              <span className="truncate" title={worktree.base.baseRef}>
                {worktree.base.baseRef}
              </span>
              <BranchDivergence base={worktree.base} />
            </>
          )}
        </div>
      )}
      <div className="top-controls">
        <div className="mode-control" role="group" aria-label="Diff mode">
          {diffModes.map(({ mode: value, label }) => (
            <button
              key={value}
              type="button"
              aria-pressed={value === mode}
              disabled={!worktree || (value === "vsBase" && !worktree.base)}
              title={
                value === "vsBase" && worktree && !worktree.base
                  ? "This worktree has no base ref"
                  : label
              }
              onClick={() => void setMode(value)}
            >
              {label}
            </button>
          ))}
        </div>
        <Button
          id="shell-inspector"
          aria-label="Why inspector"
          aria-expanded={inspectorOpen}
          aria-controls={inspectorOpen ? "why-inspector" : undefined}
          title={inspectorOpen ? "Hide Why inspector" : "Show Why inspector"}
          variant={inspectorOpen ? "secondary" : "ghost"}
          size="icon-sm"
          onClick={toggleInspector}
        >
          <PanelRight aria-hidden />
        </Button>
      </div>
    </header>
  );
}
