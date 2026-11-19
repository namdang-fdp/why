import type { ChangeStatus, FileChange, Worktree } from "@/contracts";

export const changeLabels = {
  added: { letter: "A", name: "Added" },
  modified: { letter: "M", name: "Modified" },
  deleted: { letter: "D", name: "Deleted" },
  renamed: { letter: "R", name: "Renamed" },
  copied: { letter: "C", name: "Copied" },
  typeChanged: { letter: "T", name: "Type changed" },
  untracked: { letter: "U", name: "Untracked" },
  conflicted: { letter: "!", name: "Conflicted" },
} satisfies Record<ChangeStatus, { letter: string; name: string }>;

export function worktreeName(worktree: Worktree) {
  return worktree.path.split(/[/\\]/).filter(Boolean).at(-1) ?? worktree.path;
}

export function branchLabel(worktree: Worktree) {
  if (!worktree.isDetached) return worktree.branch ?? "Unborn branch";
  return worktree.headSha ? `detached @ ${worktree.headSha.slice(0, 7)}` : "detached";
}

export function displayPath(file: FileChange) {
  return file.oldPath ? `${file.oldPath} → ${file.path}` : file.path;
}

export function ChangeStatusBadge({ status }: { status: ChangeStatus }) {
  const label = changeLabels[status];
  return (
    <span className="change-status" data-change={status} title={label.name}>
      <span aria-hidden>{label.letter}</span>
      <span className="sr-only">{label.name}</span>
    </span>
  );
}

export function LineCounts({ file }: { file: FileChange }) {
  if (file.additions === null && file.deletions === null) return null;
  return (
    <span className="line-counts">
      {file.additions !== null && (
        <span className="line-additions" aria-label={`${file.additions} added lines`}>
          +{file.additions.toLocaleString()}
        </span>
      )}
      {file.deletions !== null && (
        <span className="line-deletions" aria-label={`${file.deletions} deleted lines`}>
          −{file.deletions.toLocaleString()}
        </span>
      )}
    </span>
  );
}

export function BranchDivergence({ base }: { base: NonNullable<Worktree["base"]> }) {
  return (
    <span className="branch-divergence" title={`${base.ahead} ahead, ${base.behind} behind`}>
      <span aria-hidden>
        ↑{base.ahead} ↓{base.behind}
      </span>
      <span className="sr-only">
        {base.ahead} ahead, {base.behind} behind
      </span>
    </span>
  );
}
