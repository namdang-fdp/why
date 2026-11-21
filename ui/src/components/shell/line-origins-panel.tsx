import { useContext, useMemo, type ReactNode } from "react";
import type { FileChange, FileLineOrigins, LineUnknownReason, TurnRef } from "@/contracts";
import {
  baseCopy,
  coverageSegments,
  gutterGate,
  hiddenCopy,
  lossyPathCopy,
  runAt,
  textShape,
  unavailableCopy,
  unknownReasonCopy,
  type CoverageSegment,
} from "@/components/diff/line-origins-model";
import { openTurn } from "@/components/sessions/session-controls";
import {
  Evidence,
  EventTime,
  HarnessBadge,
  harnessNames,
} from "@/components/sessions/session-labels";
import { useTurnDetails } from "@/components/sessions/use-turn-details";
import { WhyPanel, WhyToggle } from "@/components/sessions/why-panel";
import { rationaleKey, type RationaleTarget } from "@/stores/evidence";
import { diffReadRevision } from "@/stores/line-origins";
import { selectDiffMode } from "@/stores/workspace";
import {
  WorkspaceContext,
  useLineOrigins,
  useSessions,
  useWorkspace,
} from "@/stores/workspace-context";
import { ErrorMessage } from "./async-message";
import { ShellLeadsLine } from "./rail-attribution";
import { useCurrentAttribution } from "./use-attribution";

function segmentLabel(segment: CoverageSegment) {
  if (segment.kind === "harness") return harnessNames[segment.harness];
  return segment.kind === "base" ? "Unchanged" : "Unknown";
}

/**
 * How the file's lines divide: each harness that wrote lines (Claude and Codex
 * SEPARATE: Codex patches carry no line numbers, so its coverage is measured
 * apart, decision 30), the unchanged lines, and the lines whose origin is not
 * known. The parts always sum to the file's lines; if the core ever says
 * otherwise, the panel says so instead of drawing a whole bar.
 */
function Coverage({ origins }: { origins: FileLineOrigins }) {
  const { segments, total, addsUp } = coverageSegments(origins.coverage);
  const visible = segments.filter((segment) => segment.lines > 0);
  const summary = segments.map((s) => `${s.lines} ${segmentLabel(s).toLowerCase()}`).join(", ");
  return (
    <div className="coverage" data-line-coverage>
      <div className="coverage-bar" role="img" aria-label={`Line origins: ${summary}`}>
        {visible.map((segment) => (
          <span
            key={segment.kind === "harness" ? segment.harness : segment.kind}
            data-kind={segment.kind === "harness" ? segment.harness : segment.kind}
            style={{ flexGrow: segment.lines }}
          />
        ))}
      </div>
      <ul className="coverage-legend">
        {segments
          .filter((segment) => segment.kind !== "harness" || segment.lines > 0)
          .map((segment) => (
            <li key={segment.kind === "harness" ? segment.harness : segment.kind}>
              <span
                className="coverage-swatch"
                data-kind={segment.kind === "harness" ? segment.harness : segment.kind}
                aria-hidden
              />
              <span>
                {segmentLabel(segment)} <b>{segment.lines}</b>
              </span>
            </li>
          ))}
      </ul>
      <p className="evidence-note">
        {addsUp
          ? `${origins.coverage.lines} ${origins.coverage.lines === 1 ? "line" : "lines"}.`
          : `These counts add up to ${total}, not the file's ${origins.coverage.lines} lines. Treat them with caution.`}
      </p>
    </div>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="og-row">
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

/** An edit line: the chain of records behind it (Observed), then the agent's own reason (Declared). */
function EditOrigin({ turn }: { turn: TurnRef }) {
  const workspace = useContext(WorkspaceContext);
  const details = useTurnDetails(turn);
  const title = useSessions((state) =>
    state.list.status === "ok"
      ? (state.list.data.find((session) => session.id === turn.sessionId)?.task ?? null)
      : null,
  );
  const prompt = details.status === "ok" ? details.turn?.prompt : null;
  const call = details.status === "ok" ? details.event : null;

  // Prefer the exact tool call; fall back to the whole turn. A turn the harness gave
  // no id cannot be looked up at all, so then there is no "Why?" to offer.
  const target = useMemo((): RationaleTarget | null => {
    if (call) return { kind: "tool", sessionId: turn.sessionId, eventId: call.id };
    if (details.status === "ok" && turn.turnId !== "") {
      return { kind: "turn", sessionId: turn.sessionId, turnId: turn.turnId };
    }
    return null;
  }, [call, details.status, turn.sessionId, turn.turnId]);

  async function openInTimeline() {
    const evidence = workspace.getState().evidence.getState();
    const wasOpen = target !== null && evidence.whyOpen.has(rationaleKey(target));
    await openTurn(workspace, turn);
    // Selecting the session resets the evidence store's open disclosures (they belong to
    // one session). Keep what the reviewer had open: read it again, lazily, as before.
    if (wasOpen && target) workspace.getState().evidence.getState().toggleWhy(target, true);
  }

  return (
    <>
      <dl className="og-chain">
        <Row label="Agent">
          <HarnessBadge harness={turn.harness} />
          {turn.agentId && <span className="evidence-subagent">subagent</span>}
        </Row>
        {/* A session's title is usually its first prompt: do not say it twice. */}
        {title && title !== prompt && <Row label="Session">{title}</Row>}
        <Row label="Turn">
          Turn {turn.turnIndex + 1} · <EventTime at={turn.at} />
        </Row>
        <Row label="Prompt">
          {prompt ? (
            <span className="og-prompt">{prompt}</span>
          ) : (
            <span className="session-muted">
              {details.status === "loading"
                ? "Reading the turn…"
                : details.status === "error"
                  ? "The turn could not be read."
                  : "No prompt captured for this turn."}
            </span>
          )}
        </Row>
        <Row label="Tool call">
          {call ? (
            <>
              <code>{call.toolName ?? "edit"}</code>{" "}
              <span className="og-prompt">{call.summary}</span>
            </>
          ) : (
            <span className="session-muted">
              {details.status === "ok" ? "Not found in the timeline." : "…"}
            </span>
          )}
        </Row>
      </dl>
      <button type="button" className="session-text-button" onClick={() => void openInTimeline()}>
        Open this turn in the timeline
      </button>
      <div className="og-declared">
        <div className="og-declared-head">
          <span>Agent&rsquo;s reason</span>
          <Evidence label="declared" />
          {target && (
            <WhyToggle
              target={target}
              subject={`Turn ${turn.turnIndex + 1}${call ? `, ${call.toolName ?? "edit"}` : ""}`}
            />
          )}
        </div>
        {target ? (
          <WhyPanel target={target} />
        ) : (
          <p className="evidence-note">
            {details.status === "ok"
              ? "WHY cannot look up this turn's transcript."
              : "Its words can be read once the turn is found."}
          </p>
        )}
      </div>
    </>
  );
}

/** An unknown line: the reason in plain words, and for `noCapture` the shell turns to look at. */
function UnknownOrigin({ reason }: { reason: LineUnknownReason }) {
  const attribution = useCurrentAttribution();
  const copy = unknownReasonCopy[reason];
  const leads = attribution.status === "ok" ? attribution.data.shellLeads : [];
  return (
    <>
      <p className="og-reason" data-reason={reason}>
        {copy.short}
      </p>
      <p className="evidence-note">{copy.detail}</p>
      {reason === "noCapture" && leads.length > 0 && (
        <div className="og-leads">
          <h5>Look here</h5>
          <ShellLeadsLine leads={leads} />
          <p className="evidence-note">
            Leads only. WHY cannot say any of these turns wrote this line.
          </p>
        </div>
      )}
    </>
  );
}

function ThisLine({ origins, line }: { origins: FileLineOrigins; line: number }) {
  const index = runAt(origins.runs, line);
  const run = origins.runs[index];
  if (!run) return null;
  const range =
    run.startLine === run.endLine
      ? `line ${run.startLine}`
      : `lines ${run.startLine}–${run.endLine}`;
  return (
    <section
      className="og-line"
      aria-label={`Origin of line ${line}`}
      data-origin={run.origin.kind}
    >
      <div className="og-line-head">
        <h5>Line {line}</h5>
        <span className="session-muted">in run of {range}</span>
        <Evidence label={run.evidence} />
      </div>
      {run.origin.kind === "edit" && <EditOrigin turn={run.origin.turn} />}
      {run.origin.kind === "base" && <p className="og-reason">{baseCopy}</p>}
      {run.origin.kind === "unknown" && <UnknownOrigin reason={run.origin.reason} />}
    </section>
  );
}

/**
 * The File tab's line-provenance block: file coverage, and the origin of the
 * line picked in the gutter. Every case that draws nothing says why in calm
 * words (not ready yet, wrong comparison, file changed): never an error toast.
 */
export function LineOriginsPanel({ file }: { file: FileChange }) {
  const store = useContext(WorkspaceContext);
  const worktreeId = useWorkspace((state) => state.worktreeId);
  const mode = useWorkspace(selectDiffMode);
  const diff = useWorkspace((state) => state.fileDiff);
  const origins = useLineOrigins((state) => state.origins);
  const revision = useLineOrigins((state) => state.revision);
  const originsRevision = useLineOrigins((state) => state.originsRevision);
  const refreshError = useLineOrigins((state) => state.refreshError);
  // Re-check the diff's revision tag when a re-read confirmed it unchanged.
  useLineOrigins((state) => state.diffTick);
  const line = useLineOrigins((state) => state.line);

  const shape = useMemo(
    () =>
      diff.status === "ok" && diff.data.newContent !== null
        ? textShape(diff.data.newContent)
        : null,
    [diff],
  );
  const gate = gutterGate({
    origins,
    revision,
    originsRevision,
    diffRevision: diffReadRevision(diff.status === "ok" ? diff.data : null),
    mode,
    worktreeId,
    path: file.path,
    text: shape,
  });

  // The store never asks about a lossy path (its name is shared by other files), so it would
  // sit at "loading" forever: say plainly that this file has no line origins.
  if (file.pathLossy) {
    return (
      <section className="line-origins" aria-label="Line origins" data-unavailable="lossyPath">
        <h4>Line origins</h4>
        <p className="og-reason">{lossyPathCopy.short}</p>
        <p className="evidence-note">{lossyPathCopy.detail}</p>
      </section>
    );
  }

  if (gate.status === "shown") {
    return (
      <section className="line-origins" aria-label="Line origins">
        <h4>Line origins</h4>
        <Coverage origins={gate.origins} />
        {line !== null ? (
          <ThisLine origins={gate.origins} line={line} />
        ) : (
          <p className="evidence-note">
            Click a mark in the diff gutter, or press <kbd>o</kbd> on a line, to see where it came
            from. <kbd>.</kbd> and <kbd>,</kbd> jump between lines whose origin is unknown.
          </p>
        )}
      </section>
    );
  }

  const { hidden } = gate;
  const note = hidden.kind === "unavailable" ? unavailableCopy[hidden.reason] : null;
  const words = hiddenCopy(hidden);
  if (hidden.kind === "loading") {
    return (
      <section className="line-origins" aria-label="Line origins">
        <h4>Line origins</h4>
        <p className="rail-note" role="status">
          Reading line origins…
        </p>
      </section>
    );
  }
  // A re-read failed while the answer on show is behind the file: do not promise that
  // the marks will come back, say it failed and let the reviewer try again.
  if (hidden.kind === "stale" && refreshError) {
    return (
      <section className="line-origins" aria-label="Line origins">
        <h4>Line origins</h4>
        <ErrorMessage
          context="Reading line origins again"
          error={refreshError}
          retry={() => void store.getState().lineOrigins.getState().load()}
        />
      </section>
    );
  }
  if (hidden.kind === "error" && origins.status === "error") {
    return (
      <section className="line-origins" aria-label="Line origins">
        <h4>Line origins</h4>
        <ErrorMessage
          context="Line origins"
          error={origins.error}
          retry={() => void store.getState().lineOrigins.getState().load()}
        />
      </section>
    );
  }
  if (!note && !words) return null;
  return (
    <section
      className="line-origins"
      aria-label="Line origins"
      data-unavailable={hidden.kind === "unavailable" ? hidden.reason : undefined}
    >
      <h4>Line origins</h4>
      {note && (
        <>
          <p className="og-reason">{note.short}</p>
          <p className="evidence-note">{note.detail}</p>
        </>
      )}
      {words && <p className="evidence-note">{words}</p>}
    </section>
  );
}
