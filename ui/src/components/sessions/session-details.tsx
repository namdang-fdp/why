import { useEffect, useId, useMemo, useState } from "react";
import { AlignLeft, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useEvidence, useWorkspace } from "@/stores/workspace-context";
import { outcomesNote, recoveredOutcomes } from "./outcome-overlay";
import { outcomesNotRecorded } from "./outcome-model";
import type { SessionSummary, Turn } from "@/contracts";
import { useNowSeconds } from "@/hooks/use-now";
import { HarnessBadge, PromptExcerpt, SessionTime, StatusBadge } from "./session-labels";

export function SessionList({
  sessions,
  onSelect,
}: {
  sessions: SessionSummary[];
  onSelect: (id: string) => void;
}) {
  const now = useNowSeconds();
  if (sessions.length === 0) return <EmptySessions />;
  return (
    <section className="session-list-view" aria-labelledby="session-list-heading">
      <div className="session-timeline-bar">
        <h3 id="session-list-heading">
          Sessions <span>{sessions.length}</span>
        </h3>
      </div>
      <ul className="session-list" aria-label="Worktree sessions">
        {sessions.map((session) => (
          <li key={session.id}>
            <button
              type="button"
              className="session-card"
              data-session-id={session.id}
              onClick={() => onSelect(session.id)}
            >
              <span className="session-card-badges">
                <HarnessBadge harness={session.harness} />
                <StatusBadge status={session.status} />
              </span>
              <span className="session-card-task">
                {session.task || "No task context captured"}
              </span>
              <span className="session-card-meta">
                {session.model && <span className="session-model">{session.model}</span>}
                <SessionTime at={session.lastEventAt} now={now} />
                <span>
                  {session.turnCount} {session.turnCount === 1 ? "turn" : "turns"}
                </span>
                <ChevronRight size={13} aria-hidden />
              </span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** True once the hook status says at least one harness has WHY's hooks installed; `null` while unknown. */
function useHooksInstalled() {
  const source = useWorkspace((state) => state.hookData);
  const [installed, setInstalled] = useState<boolean | null>(null);
  useEffect(() => {
    let current = true;
    source.hookStatus().then(
      (report) => {
        if (current) {
          setInstalled(
            report.claudeCode.state.kind === "installed" || report.codex.state.kind === "installed",
          );
        }
      },
      // Unknown stays unknown: the copy below is neutral then.
      () => undefined,
    );
    return () => {
      current = false;
    };
  }, [source]);
  return installed;
}

function EmptySessions() {
  const openSettings = useWorkspace((state) => state.openSettings);
  const installed = useHooksInstalled();
  return (
    <div className="inspector-empty">
      <span className="empty-icon">
        <AlignLeft size={16} aria-hidden />
      </span>
      <h3>No sessions recorded for this worktree</h3>
      {installed ? (
        <>
          <p>
            Sessions appear here when an agent&rsquo;s working directory is inside this worktree.
          </p>
          <button
            type="button"
            className="session-text-button"
            onClick={() => openSettings("agent-hooks")}
          >
            Hook settings
          </button>
        </>
      ) : (
        <>
          <p>Agent hooks capture session activity locally on this device.</p>
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={() => openSettings("agent-hooks")}
          >
            Set up hooks
          </Button>
        </>
      )}
    </div>
  );
}

export function SessionHeader({
  session,
  turns,
}: {
  session: SessionSummary;
  turns: readonly Turn[];
}) {
  const [expanded, setExpanded] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const detailsId = useId();
  const outcomeState = useEvidence((s) => s.outcomes);
  const forSession = useEvidence((s) => s.session?.id === session.id);
  // Exit codes recovered for THIS session only.
  const recovered = useMemo(
    () => (forSession ? recoveredOutcomes(outcomeState) : undefined),
    [forSession, outcomeState],
  );
  // Codex exit codes are still being read: do not call outcomes "not recorded" yet.
  const settling =
    session.harness === "codex" &&
    (!forSession || outcomeState.status === "idle" || outcomeState.status === "loading");
  const unrecorded = !settling && outcomesNotRecorded(turns, recovered);
  return (
    <header className="session-task">
      <div className="session-card-badges">
        <HarnessBadge harness={session.harness} />
        {session.model && (
          <span className="session-model" title={session.model}>
            {session.model}
          </span>
        )}
        <StatusBadge status={session.status} />
      </div>
      {session.task && (
        <>
          <h3>Task</h3>
          <PromptExcerpt
            text={session.task}
            expanded={expanded}
            toggle={() => setExpanded(!expanded)}
          />
        </>
      )}
      {unrecorded && <p className="session-muted">Outcomes not recorded for this session</p>}
      <button
        type="button"
        className="session-text-button session-details-toggle"
        aria-expanded={detailsOpen}
        aria-controls={detailsId}
        onClick={() => setDetailsOpen(!detailsOpen)}
      >
        <ChevronRight size={12} aria-hidden data-open={detailsOpen} /> Session details
      </button>
      {detailsOpen && (
        <div id={detailsId}>
          <OutcomesNote session={session} />
          <dl className="session-task-times">
            <div>
              <dt>Started</dt>
              <dd>
                <SessionTime at={session.startedAt} />
              </dd>
            </div>
            <div>
              <dt>Last activity</dt>
              <dd>
                <SessionTime at={session.lastEventAt} />
              </dd>
            </div>
          </dl>
        </div>
      )}
    </header>
  );
}

/**
 * One quiet line about exit-code recovery for a Codex session: why some calls
 * stay unknown, or that the rollout could not be read. Nothing for Claude (its
 * hooks report exit codes) and nothing when recovery simply worked.
 */
function OutcomesNote({ session }: { session: SessionSummary }) {
  const state = useEvidence((s) => s.outcomes);
  const forSession = useEvidence((s) => s.session?.id === session.id);
  const refresh = useEvidence((s) => s.loadOutcomes);
  if (session.harness !== "codex" || !forSession) return null;
  const note = outcomesNote(state);
  if (!note) return null;
  return (
    <p className="session-outcomes-note" role="status">
      {note}{" "}
      {state.status !== "loading" && (
        <button type="button" className="session-text-button" onClick={() => void refresh()}>
          Refresh
        </button>
      )}
    </p>
  );
}
