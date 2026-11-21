import { memo, useLayoutEffect, useRef, type ReactNode } from "react";
import {
  ChevronDown,
  CircleStop,
  FilePenLine,
  FlaskConical,
  GitFork,
  MessageSquare,
  SquareTerminal,
  Wrench,
} from "lucide-react";
import type {
  TimelineEvent,
  TimelineEventKind,
  Turn,
  FileChange,
  SessionId,
  ShellOutcome,
} from "@/contracts";
import { Evidence, EventTime, Outcome, PromptExcerpt, RecoveredOutcome } from "./session-labels";
import { canExplainCall } from "./rationale-model";
import { WhyPanel, WhyToggle } from "./why-panel";
import type { RationaleTarget } from "@/stores/evidence";
import { eventGroups, foldRuns, isWorktreePath, runLabel } from "./timeline-model";
import { effectiveOutcome } from "./outcome-model";

const eventKind = {
  prompt: { name: "Prompt", Icon: MessageSquare },
  assistantMessage: { name: "Agent message", Icon: MessageSquare },
  toolCall: { name: "Tool call", Icon: Wrench },
  edit: { name: "Edit", Icon: FilePenLine },
  shell: { name: "Shell", Icon: SquareTerminal },
  testRun: { name: "Test run", Icon: FlaskConical },
  subagent: { name: "Subagent", Icon: GitFork },
  stop: { name: "Stop", Icon: CircleStop },
} satisfies Record<TimelineEventKind, { name: string; Icon: typeof Wrench }>;

function FileChips({
  files,
  file,
  openFile,
}: {
  files: string[];
  file: FileChange | undefined;
  openFile: (path: string) => void;
}) {
  if (files.length === 0) return null;
  return (
    <ul className="session-files" aria-label="Recorded files">
      {[...new Set(files)].map((path) => (
        <li key={path}>
          {isWorktreePath(path) ? (
            <button
              type="button"
              className="session-file"
              title={path}
              data-file-match={path === file?.path || path === file?.oldPath || undefined}
              onClick={() => openFile(path)}
            >
              {path}
            </button>
          ) : (
            <span className="session-file" data-external title={`Outside the worktree: ${path}`}>
              {path}
              <span className="sr-only"> (outside the worktree)</span>
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}

function EventRow({
  event,
  sessionId,
  recovered,
  file,
  openFile,
  expanded,
  toggle,
}: {
  event: TimelineEvent;
  sessionId: SessionId;
  /** The exit code the rollout settled for this call, when the hooks left it unknown. */
  recovered: ShellOutcome | undefined;

  file: FileChange | undefined;
  openFile: (path: string) => void;
  expanded: boolean;
  toggle: () => void;
}) {
  const { name, Icon } = eventKind[event.kind];
  const disclosure = event.kind === "toolCall" || event.kind === "shell";
  const fileMatch = event.files.some((path) => path === file?.path || path === file?.oldPath);
  const heading = event.toolName ? `${name} · ${event.toolName}` : name;
  const outcome = effectiveOutcome(event, undefined);
  // Only a call the hooks left unknown takes the recovered code; a hook-reported outcome stays as it is.
  const overlay = recovered && outcome === "unknown" ? recovered : undefined;
  // A chip carries information only when it differs from "unknown": an unknown
  // outcome is the default, said once in the session header, not on every row.
  const showOutcome = outcome !== null && outcome !== "unknown";
  const why: RationaleTarget | null =
    canExplainCall(event.kind, event.toolUseId) && sessionId !== ""
      ? { kind: "tool", sessionId, eventId: event.id }
      : null;
  return (
    <li className="session-event" data-kind={event.kind} data-file-match={fileMatch || undefined}>
      <div className="session-event-head">
        <Icon size={13} aria-hidden />
        {disclosure ? (
          <button
            type="button"
            className="session-tool-toggle"
            aria-expanded={expanded}
            title={heading}
            onClick={toggle}
          >
            <span>{heading}</span>
            <ChevronDown size={12} aria-hidden />
          </button>
        ) : (
          <span className="session-event-name" title={heading}>
            {heading}
          </span>
        )}
        {overlay ? (
          <RecoveredOutcome outcome={overlay} test={event.kind === "testRun"} />
        ) : (
          showOutcome && <Outcome outcome={outcome} test={event.kind === "testRun"} />
        )}
        {why && <WhyToggle target={why} subject={`${heading}: ${event.summary}`} />}
        {/* The recovered chip is already the Observed label for this outcome. Observed is
            the default (the legend says so); only a different label is worth a chip. */}
        {!overlay && event.evidence !== "observed" && <Evidence label={event.evidence} />}
      </div>
      {why && <WhyPanel target={why} />}
      {event.summary && (
        <p
          className="session-event-summary"
          data-clamped={disclosure && !expanded}
          title={disclosure && !expanded ? event.summary : undefined}
        >
          {event.summary}
        </p>
      )}
      <FileChips files={event.files} file={file} openFile={openFile} />
      {expanded && (
        <div className="session-event-time">
          <EventTime at={event.at} />
        </div>
      )}
    </li>
  );
}

/**
 * 3+ consecutive shell/tool calls from one agent, folded into one row. It is
 * closed by default; its open state lives in the timeline's `openItems` (keyed
 * by turn id + `run:<first event id>`), so it survives the row being unmounted
 * by the virtualized window. The summary says how many failed, so a failure is
 * never hidden inside a closed group.
 */
function RunRow({
  id,
  events,
  open,
  toggleRun,
  file,
  outcomes,
  children,
}: {
  id: string;
  events: TimelineEvent[];
  open: boolean;
  toggleRun: () => void;
  file: FileChange | undefined;
  outcomes: ReadonlyMap<string, ShellOutcome> | undefined;
  children: ReactNode;
}) {
  const failed = events.filter((event) => effectiveOutcome(event, outcomes) === "failed").length;
  const fileMatch = events.some((event) =>
    event.files.some((path) => path === file?.path || path === file?.oldPath),
  );
  const label = runLabel(events);
  return (
    <li
      className="session-event session-run"
      data-run={id}
      data-file-match={fileMatch || undefined}
    >
      <div className="session-event-head">
        <SquareTerminal size={13} aria-hidden />
        <button
          type="button"
          className="session-tool-toggle"
          aria-expanded={open}
          onClick={toggleRun}
        >
          <span>{label}</span>
          <ChevronDown size={12} aria-hidden />
        </button>
        {failed > 0 && (
          <span className="session-outcome" data-outcome="failed">
            {failed} failed
          </span>
        )}
      </div>
      {open && <ul className="session-run-events">{children}</ul>}
    </li>
  );
}

export const TurnRow = memo(function TurnRow({
  turn,
  setSize,
  top,
  selected,
  matching,
  file,
  measure,
  select,
  onTurnFocus,
  openFile,
  openItems,
  toggle,
  sessionId,
  outcomes,
}: {
  sessionId: SessionId;
  outcomes: ReadonlyMap<string, ShellOutcome> | undefined;
  turn: Turn;
  setSize: number;
  top: number;
  selected: boolean;
  matching: boolean;
  file: FileChange | undefined;
  measure: (turn: Turn, height: number) => void;
  select: (turnId: string) => void;
  onTurnFocus: (turnId: string) => void;
  openFile: (path: string) => void;
  openItems: ReadonlySet<string | null> | undefined;
  toggle: (turnId: string, itemId: string | null) => void;
}) {
  const row = useRef<HTMLElement>(null);
  useLayoutEffect(() => {
    const node = row.current;
    if (!node) return;
    const observer = new ResizeObserver(() => measure(turn, node.getBoundingClientRect().height));
    observer.observe(node);
    return () => observer.disconnect();
  }, [turn, measure]);

  // Use the matching prompt event as the turn's prompt, preserving any
  // additional prompt events in chronological order below it.
  const promptEvent = turn.events.find(
    (event) => event.kind === "prompt" && event.agentId === null && event.summary === turn.prompt,
  );
  const synthetic = /^\s*<task-notification(?:\s|>)/.test(turn.prompt ?? "");
  const promptExpanded = openItems?.has(null) ?? false;

  return (
    <article
      ref={row}
      className="session-turn"
      data-turn-id={turn.id}
      aria-label={`Turn ${turn.index + 1}`}
      aria-setsize={setSize}
      aria-posinset={turn.index + 1}
      style={{ transform: `translateY(${top}px)` }}
      onFocus={() => onTurnFocus(turn.id)}
    >
      <div
        className="session-turn-body"
        data-selected={selected || undefined}
        data-file-match={matching || undefined}
      >
        <div className="session-turn-head">
          <button
            type="button"
            className="session-turn-select"
            data-turn-select
            aria-pressed={selected}
            onClick={() => select(turn.id)}
          >
            Turn {turn.index + 1}
          </button>
          <EventTime at={turn.startedAt} />
          {/* A turn the harness gave no id cannot be looked up, so it offers no Why?. */}
          {turn.id !== "" && (
            <WhyToggle
              target={{ kind: "turn", sessionId, turnId: turn.id }}
              subject={`Turn ${turn.index + 1}`}
            />
          )}
          {(promptEvent?.evidence ?? "unknown") !== "observed" && (
            <Evidence label={promptEvent?.evidence ?? "unknown"} />
          )}
        </div>
        {turn.id !== "" && <WhyPanel target={{ kind: "turn", sessionId, turnId: turn.id }} />}
        {synthetic && <p className="session-synthetic">Task notification · not typed by you</p>}
        {turn.prompt ? (
          <PromptExcerpt
            text={turn.prompt}
            expanded={promptExpanded}
            toggle={() => toggle(turn.id, null)}
          />
        ) : (
          <p className="session-prompt session-muted">Prompt unavailable</p>
        )}
        {eventGroups(turn.events.filter((event) => event !== promptEvent)).map((group) => (
          <div
            key={group.events[0]?.id}
            className="session-event-group"
            data-subagent={group.agentId ?? undefined}
          >
            {group.agentId && (
              <div className="session-subagent" title={group.agentId}>
                <GitFork size={12} aria-hidden />
                <span>
                  Subagent <code>{group.agentId}</code>
                </span>
              </div>
            )}
            <ul>
              {foldRuns(group.events).map((item) =>
                item.type === "event" ? (
                  <EventRow
                    key={item.event.id}
                    event={item.event}
                    sessionId={sessionId}
                    recovered={outcomes?.get(item.event.id)}
                    file={file}
                    openFile={openFile}
                    expanded={openItems?.has(item.event.id) ?? false}
                    toggle={() => toggle(turn.id, item.event.id)}
                  />
                ) : (
                  <RunRow
                    key={item.id}
                    id={item.id}
                    events={item.events}
                    open={openItems?.has(item.id) ?? false}
                    toggleRun={() => toggle(turn.id, item.id)}
                    file={file}
                    outcomes={outcomes}
                  >
                    {item.events.map((event) => (
                      <EventRow
                        key={event.id}
                        event={event}
                        sessionId={sessionId}
                        recovered={outcomes?.get(event.id)}
                        file={file}
                        openFile={openFile}
                        expanded={openItems?.has(event.id) ?? false}
                        toggle={() => toggle(turn.id, event.id)}
                      />
                    ))}
                  </RunRow>
                ),
              )}
            </ul>
          </div>
        ))}
      </div>
    </article>
  );
});
