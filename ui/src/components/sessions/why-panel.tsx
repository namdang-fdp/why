import { useId, useState } from "react";
import { HelpCircle, RefreshCw } from "lucide-react";
import { ErrorMessage } from "@/components/shell/async-message";
import { rationaleKey, type RationaleTarget } from "@/stores/evidence";
import { useEvidence } from "@/stores/workspace-context";
import type { Rationale, RationaleBlock } from "@/contracts";
import { Evidence, EventTime, sourceNames } from "./session-labels";
import { caveats, isClipped, isLong, positionText, unavailableText } from "./rationale-model";

/** The "Why?" button on a turn or a call. Opening it (and only that) reads the transcript. */
export function WhyToggle({ target, subject }: { target: RationaleTarget; subject: string }) {
  const key = rationaleKey(target);
  const open = useEvidence((state) => state.whyOpen.has(key));
  const toggle = useEvidence((state) => state.toggleWhy);
  return (
    <button
      type="button"
      className="session-why-toggle"
      aria-expanded={open}
      aria-controls={open ? `why-${key}` : undefined}
      aria-label={`Why? ${subject}`}
      title="What the agent said it was doing (read from its transcript when opened)"
      data-why-toggle={target.kind}
      onClick={() => toggle(target)}
    >
      <HelpCircle size={12} aria-hidden />
      Why?
    </button>
  );
}

function Block({ block }: { block: RationaleBlock }) {
  const [expanded, setExpanded] = useState(false);
  const id = useId();
  const long = !block.encrypted && isLong(block.text);
  return (
    <li className="why-block" data-position={block.position}>
      <div className="why-block-meta">
        <span>{positionText[block.position]}</span>
        {block.timestamp && <EventTime at={block.timestamp} />}
        {isClipped(block) && <span className="why-hint">excerpt</span>}
      </div>
      {block.encrypted ? (
        <p className="why-text session-muted" data-encrypted>
          Encrypted message: the harness did not record readable text here.
        </p>
      ) : (
        <p id={id} className="why-text" data-expanded={expanded || !long}>
          {block.text}
        </p>
      )}
      {long && (
        <button
          type="button"
          className="session-text-button"
          aria-expanded={expanded}
          aria-controls={id}
          onClick={() => setExpanded(!expanded)}
        >
          {expanded ? "Show less" : "Show more"}
        </button>
      )}
    </li>
  );
}

function Words({ rationale }: { rationale: Rationale }) {
  const notes = caveats(rationale);
  return (
    <>
      <div className="why-source">
        <Evidence label="declared" />
        <span>{sourceNames[rationale.source]}</span>
        {rationale.harnessVersion && <code>{rationale.harnessVersion}</code>}
      </div>
      <p className="why-declared">The agent&rsquo;s own words. WHY has not verified them.</p>
      {rationale.blocks.length > 0 ? (
        <ul className="why-blocks">
          {rationale.blocks.map((block, index) => (
            <Block key={index} block={block} />
          ))}
        </ul>
      ) : rationale.truncated ? (
        // Not the same as silence: the read ran out of budget before finding the place.
        <p className="why-empty">
          Not found within the read limit. This does not mean the agent said nothing.
        </p>
      ) : (
        <p className="why-empty">The transcript records no text from the agent here.</p>
      )}
      {notes.map((note) => (
        <p key={note} className="why-caveat">
          {note}
        </p>
      ))}
    </>
  );
}

/** What a "Why?" shows once opened: loading, the agent's words, or a plain reason there are none. */
export function WhyPanel({ target }: { target: RationaleTarget }) {
  const key = rationaleKey(target);
  const open = useEvidence((state) => state.whyOpen.has(key));
  const state = useEvidence((s) => s.rationales.get(key));
  const load = useEvidence((s) => s.loadRationale);
  if (!open) return null;
  const refresh = () => void load(target, true);
  return (
    <div className="why-panel" id={`why-${key}`} data-why-panel>
      {(state === undefined || state.status === "idle" || state.status === "loading") && (
        <p className="why-empty" role="status">
          Reading the transcript…
        </p>
      )}
      {state?.status === "error" && (
        <ErrorMessage context="Why?" error={state.error} retry={refresh} />
      )}
      {state?.status === "ok" && (
        <>
          {state.data.status === "ok" ? (
            <Words rationale={state.data.rationale} />
          ) : (
            <>
              <div className="why-source">
                <Evidence label="unknown" />
              </div>
              <p className="why-empty" data-reason={state.data.reason}>
                {unavailableText[state.data.reason]}
              </p>
            </>
          )}
          <button type="button" className="session-text-button why-refresh" onClick={refresh}>
            <RefreshCw size={11} aria-hidden /> Refresh
          </button>
        </>
      )}
    </div>
  );
}
