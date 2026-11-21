import { useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { PopoverContent, PopoverRoot, PopoverTrigger } from "@/components/ui/overlays";
import type { FileAttribution, ShellLead, TurnRef } from "@/contracts";
import { openTurn } from "@/components/sessions/session-controls";
import { EventTime, HarnessBadge } from "@/components/sessions/session-labels";
import { WorkspaceContext, useSessions } from "@/stores/workspace-context";
import {
  attemptLabel,
  evidenceKind,
  explainedCaveat,
  harnessesOf,
  refOutcomeLabel,
  taskTitles,
  turnCount,
  unexplainedExplanation,
} from "./attribution-model";

/**
 * A popover that opens on click or keyboard (Enter/Space on the trigger), and
 * also when a mouse pointer rests on the trigger. A pointer-opened popover does
 * not steal focus, so hovering over the rail never moves the keyboard.
 */
function EvidencePopover({
  trigger,
  className,
  children,
}: {
  trigger: ReactNode;
  className?: string;
  children: (close: () => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const openRef = useRef(false);
  // "preview": a resting mouse pointer opened it; it closes when the pointer leaves
  // and never takes focus. "pinned": a click or the keyboard opened it; it stays
  // until dismissed (Esc, outside click, or the trigger again).
  const via = useRef<"preview" | "pinned">("pinned");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => {
    openRef.current = open;
  }, [open]);
  const hoverOpen = (event: { pointerType: string }) => {
    if (event.pointerType !== "mouse" || openRef.current) return;
    clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      via.current = "preview";
      setOpen(true);
    }, 250);
  };
  const hoverClose = (event: { pointerType: string }) => {
    if (event.pointerType !== "mouse") return;
    clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      if (via.current === "preview") setOpen(false);
    }, 150);
  };
  return (
    <PopoverRoot
      open={open}
      onOpenChange={(next) => {
        clearTimeout(timer.current);
        if (next) via.current = "pinned";
        setOpen(next);
      }}
    >
      <PopoverTrigger
        asChild
        onPointerEnter={hoverOpen}
        onPointerLeave={hoverClose}
        onClick={(event) => {
          clearTimeout(timer.current);
          // Clicking a preview keeps it open and pins it, rather than toggling it shut.
          if (openRef.current && via.current === "preview") {
            event.preventDefault();
            via.current = "pinned";
          }
        }}
      >
        {trigger}
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className={className}
        onOpenAutoFocus={(event) => {
          if (via.current === "preview") event.preventDefault();
        }}
        // A preview must not pull focus back to the trigger when it goes away.
        onCloseAutoFocus={(event) => {
          if (via.current === "preview") event.preventDefault();
        }}
        onPointerEnter={() => clearTimeout(timer.current)}
        onPointerLeave={hoverClose}
      >
        {children(() => setOpen(false))}
      </PopoverContent>
    </PopoverRoot>
  );
}

function TurnLine({
  turn,
  task,
  onOpen,
  extra,
}: {
  turn: Pick<TurnRef, "harness" | "turnIndex"> & { agentId?: string | null };
  task: string | null | undefined;
  onOpen: () => void;
  extra: ReactNode;
}) {
  return (
    <li>
      <button type="button" className="evidence-turn" onClick={onOpen}>
        <span className="evidence-turn-head">
          <HarnessBadge harness={turn.harness} />
          <span>Turn {turn.turnIndex + 1}</span>
          {turn.agentId && <span className="evidence-subagent">subagent</span>}
          {extra}
        </span>
        <span className="evidence-turn-task">{task || "No task context captured"}</span>
      </button>
    </li>
  );
}

export function RefLine({
  reference,
  file,
  titles,
  close,
}: {
  reference: TurnRef;
  file: FileAttribution["change"];
  titles: ReadonlyMap<string, string | null>;
  close: () => void;
}) {
  const store = useContext(WorkspaceContext);
  const renamed = reference.path !== file.path;
  return (
    <TurnLine
      turn={reference}
      task={titles.get(reference.sessionId)}
      onOpen={() => {
        close();
        void openTurn(store, reference);
      }}
      extra={
        <>
          <span className="session-outcome" data-outcome={reference.outcome}>
            {refOutcomeLabel[reference.outcome]}
          </span>
          <EventTime at={reference.at} />
          {renamed && (
            <span className="evidence-old-path" title={reference.path}>
              named {reference.path}
            </span>
          )}
        </>
      }
    />
  );
}

function AttributionDetail({
  file,
  titles,
  close,
}: {
  file: FileAttribution;
  titles: ReadonlyMap<string, string | null>;
  close: () => void;
}) {
  const kind = evidenceKind(file);
  return (
    <div className="evidence-detail">
      <p className="evidence-detail-title" title={file.change.path}>
        {file.change.path}
      </p>
      {file.change.oldPath && (
        <p className="evidence-note">
          Renamed from <code>{file.change.oldPath}</code>. Edits are listed under the new path.
        </p>
      )}
      {kind === "unexplained" && <p className="evidence-note">{unexplainedExplanation}</p>}
      {file.explainedBy.length > 0 && (
        <>
          <h4>Applied edits (newest first)</h4>
          <ul>
            {file.explainedBy.map((reference, index) => (
              <RefLine
                key={`${reference.turnId}-${reference.toolUseId ?? index}-${index}`}
                reference={reference}
                file={file.change}
                titles={titles}
                close={close}
              />
            ))}
          </ul>
          <p className="evidence-note">{explainedCaveat}</p>
        </>
      )}
      {file.attempts.length > 0 && (
        <>
          <h4>Not applied or not confirmed</h4>
          <ul>
            {file.attempts.map((reference, index) => (
              <RefLine
                key={`${reference.turnId}-${reference.toolUseId ?? index}-${index}`}
                reference={reference}
                file={file.change}
                titles={titles}
                close={close}
              />
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

export function useTitles() {
  const list = useSessions((state) => state.list);
  return useMemo(() => taskTitles(list.status === "ok" ? list.data : []), [list]);
}

/** One line under a file: who the captured evidence points at, or that there is none. */
export function AttributionMarker({ file }: { file: FileAttribution }) {
  const titles = useTitles();
  const kind = evidenceKind(file);
  const turns = turnCount(file.explainedBy);
  const label =
    kind === "explained"
      ? `Explained by ${turns} ${turns === 1 ? "turn" : "turns"}`
      : kind === "attempted"
        ? attemptLabel(file)
        : "No captured agent edit";
  return (
    <EvidencePopover
      className="evidence-popover"
      trigger={
        <button type="button" className="evidence-chip" data-kind={kind} aria-label={label}>
          {kind === "explained" && (
            <>
              {harnessesOf(file.explainedBy).map((harness) => (
                <HarnessBadge key={harness} harness={harness} />
              ))}
              <span>
                {turns} {turns === 1 ? "turn" : "turns"}
              </span>
            </>
          )}
          {kind !== "explained" && <span>{label}</span>}
        </button>
      }
    >
      {(close) => <AttributionDetail file={file} titles={titles} close={close} />}
    </EvidencePopover>
  );
}

/** Shown once per worktree: shell turns are leads, never attributions. */
export function ShellLeadsLine({ leads }: { leads: readonly ShellLead[] }) {
  const store = useContext(WorkspaceContext);
  const titles = useTitles();
  if (leads.length === 0) return null;
  return (
    <EvidencePopover
      className="evidence-popover"
      trigger={
        <button type="button" className="shell-leads">
          {leads.length} {leads.length === 1 ? "turn" : "turns"} also ran shell commands here (their
          file changes cannot be attributed)
        </button>
      }
    >
      {(close) => (
        <div className="evidence-detail">
          <h4>Turns that ran shell commands</h4>
          <ul>
            {leads.map((lead) => (
              <TurnLine
                key={`${lead.sessionId}-${lead.turnId}-${lead.turnIndex}`}
                turn={lead}
                task={titles.get(lead.sessionId)}
                onOpen={() => {
                  close();
                  void openTurn(store, lead);
                }}
                extra={
                  <span className="evidence-count">
                    {lead.commandCount} {lead.commandCount === 1 ? "command" : "commands"}
                  </span>
                }
              />
            ))}
          </ul>
          <p className="evidence-note">
            What a command changed is only known to git. Check these turns when a file is
            unexplained.
          </p>
        </div>
      )}
    </EvidencePopover>
  );
}
