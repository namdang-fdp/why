import type { CoverageCounts } from "./attribution-model";

const segments = [
  { kind: "explained", label: "Explained" },
  { kind: "attempted", label: "Attempted" },
  { kind: "unexplained", label: "No captured edit" },
] as const;

/**
 * How many changed files have captured agent edits behind them. It counts
 * captured edit calls only (what the harness recorded), not rationale, and the
 * caption says line-level provenance is not here yet. The "No captured edit"
 * item turns on the rail's "Show unexplained files" filter.
 */
export function CoverageBar({
  counts,
  onShowUnexplained,
}: {
  counts: CoverageCounts;
  onShowUnexplained: () => void;
}) {
  const summary = segments
    .map(({ kind, label }) => `${counts[kind]} ${label.toLowerCase()}`)
    .join(", ");
  return (
    <div className="coverage">
      <div className="coverage-bar" role="img" aria-label={`Evidence coverage: ${summary}`}>
        {segments.map(({ kind }) =>
          counts[kind] > 0 ? (
            <span key={kind} data-kind={kind} style={{ flexGrow: counts[kind] }} />
          ) : null,
        )}
      </div>
      <ul className="coverage-legend">
        {segments.map(({ kind, label }) => (
          <li key={kind}>
            <span className="coverage-swatch" data-kind={kind} aria-hidden />
            {kind === "unexplained" && counts.unexplained > 0 ? (
              <button type="button" className="session-text-button" onClick={onShowUnexplained}>
                {label} <b>{counts.unexplained}</b>
              </button>
            ) : (
              <span>
                {label} <b>{counts[kind]}</b>
              </span>
            )}
          </li>
        ))}
      </ul>
      <p className="evidence-note">Captured edits only. Line-level provenance comes later.</p>
    </div>
  );
}
