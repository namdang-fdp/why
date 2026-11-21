import type { ShellOutcome, ShellOutcomesResult } from "@/contracts";
import type { AsyncState } from "@/data/async-state";

/**
 * Recovered exit codes by `TimelineEvent.id`. An id the timeline does not have
 * simply never matches a row, so a stray outcome changes nothing.
 */
export function recoveredOutcomes(
  state: AsyncState<ShellOutcomesResult>,
): ReadonlyMap<string, ShellOutcome> | undefined {
  if (state.status !== "ok" || state.data.status !== "ok") return undefined;
  return new Map(state.data.result.outcomes.map((outcome) => [outcome.eventId, outcome]));
}

/** The session-level line about recovered exit codes; `null` when there is nothing to say. */
export function outcomesNote(state: AsyncState<ShellOutcomesResult>): string | null {
  if (state.status === "idle") return null;
  if (state.status === "loading") return "Reading exit codes from the Codex rollout…";
  if (state.status === "error") return "Could not read exit codes from the Codex rollout.";
  if (state.data.status === "unavailable") {
    return "Exit codes could not be recovered, so shell calls keep the hooks' unknown outcome.";
  }
  const { versionSupported, harnessVersion, truncated, unresolved, outcomes } = state.data.result;
  // Nothing to recover (no unknown shell calls): the core never opened a rollout, so
  // "unsupported, no version" is not news about the harness.
  if (!versionSupported && harnessVersion === null && unresolved === 0 && outcomes.length === 0) {
    return null;
  }
  const notes: string[] = [];
  if (!versionSupported) {
    notes.push(
      `Codex${harnessVersion ? ` ${harnessVersion}` : ""} is not a version WHY has checked, so exit codes were not recovered.`,
    );
  } else {
    if (unresolved > 0) {
      notes.push(
        `${unresolved} shell ${unresolved === 1 ? "call stays" : "calls stay"} unknown: the rollout has no record that settles ${unresolved === 1 ? "it" : "them"}.`,
      );
    }
    if (truncated)
      notes.push("Some calls may be missing: the rollout is larger than the read limit.");
  }
  return notes.length > 0 ? notes.join(" ") : null;
}
