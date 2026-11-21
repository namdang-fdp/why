import { useId, useLayoutEffect, useRef, useState } from "react";
import type {
  EvidenceLabel,
  Harness,
  SessionStatus,
  ShellOutcome,
  ToolOutcome,
  TranscriptSource,
} from "@/contracts";
import { Tip } from "@/components/ui/overlays";

const harnessNames: Record<Harness, string> = {
  claudeCode: "Claude Code",
  codex: "Codex",
  unknown: "Unknown harness",
};

const statuses: Record<SessionStatus, string> = {
  live: "Live",
  ended: "Ended",
  unknown: "Unknown",
};

const evidence = {
  observed: { label: "Observed", detail: "Captured directly from the harness or collector." },
  declared: { label: "Declared", detail: "Stated by the agent. Its claim hasn’t been verified." },
  inferred: { label: "Inferred", detail: "Reconstructed from available records." },
  unknown: { label: "Unknown", detail: "The available records do not establish this." },
} satisfies Record<EvidenceLabel, { label: string; detail: string }>;

export function Evidence({ label }: { label: EvidenceLabel }) {
  const item = evidence[label];
  return (
    <Tip label={item.detail} side="left">
      <span className="session-evidence" data-evidence={label}>
        {item.label}
        <span className="sr-only">. {item.detail}</span>
      </span>
    </Tip>
  );
}

export function Outcome({ outcome, test }: { outcome: ToolOutcome; test: boolean }) {
  return (
    <span className="session-outcome" data-outcome={outcome}>
      {outcome === "ok" ? (test ? "Passed" : "OK") : outcome === "failed" ? "Failed" : "Unknown"}
    </span>
  );
}

export const sourceNames: Record<TranscriptSource, string> = {
  claudeTranscript: "Claude transcript",
  codexRollout: "Codex rollout",
};

/**
 * An exit code the hooks did not report, recovered from the harness's own log.
 * Shown next to its source so it never looks like the hook's word: the label
 * says where it came from, and the chip is the Observed one.
 */
export function RecoveredOutcome({ outcome, test }: { outcome: ShellOutcome; test: boolean }) {
  const word = outcome.outcome === "ok" ? (test ? "Passed" : "OK") : "Failed";
  return (
    <>
      <span
        className="session-outcome"
        data-outcome={outcome.outcome}
        data-recovered
        title={`Exit code ${outcome.exitCode}, read from the ${sourceNames[outcome.source]}`}
      >
        {word} · exit {outcome.exitCode}
      </span>
      <span className="session-evidence" data-evidence="observed" data-recovered>
        Observed · from rollout
      </span>
    </>
  );
}

export function HarnessBadge({ harness }: { harness: Harness }) {
  return <span className="session-harness">{harnessNames[harness]}</span>;
}

export function StatusBadge({ status }: { status: SessionStatus }) {
  return (
    <span className="session-status" data-status={status}>
      <span aria-hidden />
      {statuses[status]}
    </span>
  );
}

const relativeTime = new Intl.RelativeTimeFormat(undefined, { numeric: "always" });
const fullTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
const clockTime = new Intl.DateTimeFormat(undefined, { timeStyle: "medium" });

export function SessionTime({ at, now }: { at: string; now?: number }) {
  const epoch = Date.parse(at);
  if (!Number.isFinite(epoch)) return <span>Unknown</span>;
  let label = fullTime.format(epoch);
  if (now !== undefined) {
    const seconds = Math.max(0, now - Math.floor(epoch / 1000));
    label =
      seconds < 45
        ? "Just now"
        : seconds < 3600
          ? relativeTime.format(-Math.max(1, Math.floor(seconds / 60)), "minute")
          : seconds < 86400
            ? relativeTime.format(-Math.floor(seconds / 3600), "hour")
            : relativeTime.format(-Math.floor(seconds / 86400), "day");
  }
  return (
    <time dateTime={at} title={fullTime.format(epoch)}>
      {label}
    </time>
  );
}

export function EventTime({ at }: { at: string }) {
  const epoch = Date.parse(at);
  if (!Number.isFinite(epoch)) return <span>Unknown time</span>;
  return (
    <time dateTime={at} title={fullTime.format(epoch)}>
      {clockTime.format(epoch)}
    </time>
  );
}

export function PromptExcerpt({
  text,
  expanded,
  toggle,
}: {
  text: string;
  expanded: boolean;
  toggle: () => void;
}) {
  const ref = useRef<HTMLParagraphElement>(null);
  const id = useId();
  const [overflows, setOverflows] = useState(false);
  useLayoutEffect(() => {
    const node = ref.current;
    if (expanded || !node) return;
    const measure = () => setOverflows(node.scrollHeight > node.clientHeight);
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, [expanded, text]);

  return (
    <>
      <p ref={ref} id={id} className="session-prompt" data-expanded={expanded}>
        {text}
      </p>
      {(expanded || overflows) && (
        <button
          type="button"
          className="session-text-button"
          aria-expanded={expanded}
          aria-controls={id}
          onClick={toggle}
        >
          {expanded ? "Show less" : "Show full prompt"}
        </button>
      )}
    </>
  );
}
