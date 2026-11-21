import { useMemo, useState } from "react";
import { selectChanges, selectWorktree } from "@/stores/workspace";
import { useEvidence, useSessions, useWorkspace } from "@/stores/workspace-context";
import { SessionList } from "@/components/sessions/session-details";
import { PromptExcerpt, harnessNames } from "@/components/sessions/session-labels";
import type { FileChange } from "@/contracts";
import { ErrorMessage, LoadingMessage } from "./async-message";
import { branchLabel, worktreeName } from "./change-labels";
import { coverageCounts, notExplainedCount } from "./attribution-model";
import { CoverageBar } from "./coverage-bar";
import { useCurrentAttribution } from "./use-attribution";

/** Sum of the counts git gave; `null` when none did (every file binary), so no fake zero. */
function lineTotals(files: readonly FileChange[]) {
  const known = (values: (number | null)[]) => {
    const present = values.filter((value): value is number => value !== null);
    return present.length > 0 ? present.reduce((sum, value) => sum + value, 0) : null;
  };
  return {
    additions: known(files.map((file) => file.additions)),
    deletions: known(files.map((file) => file.deletions)),
  };
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="stat">
      <dd>{value}</dd>
      <dt>{label}</dt>
    </div>
  );
}

/** The inspector's Worktree tab: a picture of the whole worktree, with sessions below. */
export function WorktreeTab({ onOpenSession }: { onOpenSession: (id: string) => void }) {
  const worktree = useWorkspace(selectWorktree);
  const changes = useWorkspace(selectChanges);
  const setOnlyUnexplained = useEvidence((state) => state.setOnlyUnexplained);
  const loadAttribution = useEvidence((state) => state.loadAttribution);
  const attribution = useCurrentAttribution();
  const list = useSessions((state) => state.list);
  const sessionsWorktree = useSessions((state) => state.worktreeId);
  const loadList = useSessions((state) => state.loadList);
  const [taskExpanded, setTaskExpanded] = useState(false);

  // The list is sorted newest activity first; the task is the verbatim first-prompt
  // excerpt of the latest session that has one. Never derived from anything else.
  const sessions = list.status === "ok" && sessionsWorktree === worktree?.id ? list.data : [];
  const taskSession = sessions.find((session) => session.task);
  const files = changes.status === "ok" ? changes.data : null;
  const totals = useMemo(() => (files ? lineTotals(files) : null), [files]);
  const counts = useMemo(
    () => (attribution.status === "ok" ? coverageCounts(attribution.data.files) : null),
    [attribution],
  );

  if (!worktree) {
    return (
      <div className="inspector-body">
        <p className="rail-note">Select a worktree to see its overview.</p>
      </div>
    );
  }

  return (
    <div className="inspector-body">
      <div className="worktree-header">
        <h3 title={worktree.path}>{worktreeName(worktree)}</h3>
        <span className="worktree-branch">{branchLabel(worktree)}</span>
      </div>

      <section className="worktree-section" aria-labelledby="overview-task">
        <h4 id="overview-task">Task</h4>
        {taskSession?.task ? (
          <>
            <PromptExcerpt
              text={taskSession.task}
              expanded={taskExpanded}
              toggle={() => setTaskExpanded(!taskExpanded)}
            />
            <p className="evidence-note">
              From the first prompt of {harnessNames[taskSession.harness]}
            </p>
          </>
        ) : (
          <p className="evidence-note">No task context captured</p>
        )}
      </section>

      <dl className="stat-row" aria-label="Worktree summary">
        <Stat label="changed files" value={files ? String(files.length) : "…"} />
        <Stat
          label="agent sessions"
          value={
            list.status === "ok" && sessionsWorktree === worktree.id ? String(sessions.length) : "…"
          }
        />
        {totals?.additions != null && (
          <Stat label="added" value={`+${totals.additions.toLocaleString()}`} />
        )}
        {totals?.deletions != null && (
          <Stat label="removed" value={`−${totals.deletions.toLocaleString()}`} />
        )}
      </dl>

      <section className="worktree-section" aria-labelledby="overview-coverage">
        <h4 id="overview-coverage">Evidence coverage</h4>
        {attribution.status === "error" ? (
          <ErrorMessage
            context="Evidence coverage"
            error={attribution.error}
            retry={() => void loadAttribution()}
          />
        ) : counts === null ? (
          <div className="coverage-skeleton" role="status" aria-label="Loading evidence coverage" />
        ) : counts.total === 0 ? (
          <p className="evidence-note">No changed files to cover.</p>
        ) : (
          <CoverageBar
            counts={counts}
            notExplained={
              attribution.status === "ok" ? notExplainedCount(attribution.data.files) : 0
            }
            onShowUnexplained={() => setOnlyUnexplained(true)}
          />
        )}
      </section>

      <section className="worktree-section" aria-label="Sessions">
        {list.status === "idle" && (
          <p className="rail-note">Select a worktree to see its sessions.</p>
        )}
        {list.status === "loading" && <LoadingMessage>Loading sessions…</LoadingMessage>}
        {list.status === "error" && (
          <ErrorMessage context="Sessions" error={list.error} retry={() => void loadList()} />
        )}
        {list.status === "ok" && <SessionList sessions={list.data} onSelect={onOpenSession} />}
      </section>
    </div>
  );
}
