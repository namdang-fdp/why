import { useState } from "react";
import { AlignLeft, ChevronRight } from "lucide-react";
import type { SessionSummary } from "@/contracts";
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
  if (sessions.length === 0) {
    return (
      <div className="inspector-empty">
        <span className="empty-icon">
          <AlignLeft size={16} aria-hidden />
        </span>
        <h3>No sessions recorded for this worktree</h3>
        <p>Agent hooks capture session activity locally on this device.</p>
      </div>
    );
  }
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
              <span className="session-card-task">{session.task || "Task unknown"}</span>
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

export function TaskHeader({ session }: { session: SessionSummary }) {
  const [expanded, setExpanded] = useState(false);
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
      <h3>Task</h3>
      {session.task ? (
        <PromptExcerpt
          text={session.task}
          expanded={expanded}
          toggle={() => setExpanded(!expanded)}
        />
      ) : (
        <p className="session-muted">Task unknown</p>
      )}
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
    </header>
  );
}
