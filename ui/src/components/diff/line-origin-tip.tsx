import type { LineRun, ToolOutcome, TurnRef } from "@/contracts";
import { Evidence, HarnessBadge } from "@/components/sessions/session-labels";
import { useTurnDetails } from "@/components/sessions/use-turn-details";
import { useSessions } from "@/stores/workspace-context";
import { unknownReasonCopy } from "./line-origins-model";

const outcomeWords: Record<ToolOutcome, string> = {
  ok: "Applied",
  failed: "Failed",
  unknown: "Outcome unknown",
};

function EditTip({ turn }: { turn: TurnRef }) {
  const details = useTurnDetails(turn);
  const title = useSessions((state) =>
    state.list.status === "ok"
      ? (state.list.data.find((session) => session.id === turn.sessionId)?.task ?? null)
      : null,
  );
  const prompt = details.status === "ok" ? details.turn?.prompt : null;
  const call = details.status === "ok" ? details.event : null;
  return (
    <>
      <div className="og-tip-head">
        <HarnessBadge harness={turn.harness} />
        <strong>Turn {turn.turnIndex + 1}</strong>
        {turn.agentId && <span className="evidence-subagent">subagent</span>}
        <Evidence label="observed" />
      </div>
      {title && title !== prompt && <p className="og-tip-session">{title}</p>}
      {prompt ? (
        <p className="og-tip-prompt">{prompt}</p>
      ) : (
        <p className="og-tip-prompt session-muted">
          {details.status === "loading"
            ? "Reading the turn…"
            : details.status === "error"
              ? "The turn could not be read."
              : "No prompt captured for this turn."}
        </p>
      )}
      <p className="og-tip-meta">
        {call?.toolName ? `${call.toolName} · ` : ""}
        {outcomeWords[turn.outcome]}
      </p>
    </>
  );
}

/**
 * The hover card for a gutter mark: who wrote the run (an edit's harness,
 * session, turn, prompt excerpt and outcome) or, for an unknown run, plainly
 * why WHY does not know. Fixed-position and pointer-transparent so it never
 * blocks the mark under it.
 */
export function LineOriginTip({
  run,
  line,
  x,
  y,
}: {
  run: LineRun;
  line: number;
  x: number;
  y: number;
}) {
  return (
    <div className="og-tip" role="tooltip" style={{ left: x, top: y }}>
      {run.origin.kind === "edit" && <EditTip turn={run.origin.turn} />}
      {run.origin.kind === "unknown" && (
        <>
          <div className="og-tip-head">
            <strong>Origin unknown</strong>
            <Evidence label="unknown" />
          </div>
          <p className="og-tip-prompt">{unknownReasonCopy[run.origin.reason].short}</p>
        </>
      )}
      <p className="og-tip-hint">
        Line {line} · click or press <kbd>o</kbd> for details
      </p>
    </div>
  );
}
