import type { ShellOutcome, TimelineEvent, ToolOutcome, Turn } from "@/contracts";

/** An event is a "call" when the harness gave it an outcome slot (tool calls, edits, shell, tests). */
export function isCall(event: TimelineEvent) {
  return (
    event.kind === "toolCall" ||
    event.kind === "shell" ||
    event.kind === "testRun" ||
    event.outcome !== null
  );
}

/** The outcome a row shows: the hook's word, or the recovered exit code when the hook left it unknown. */
export function effectiveOutcome(
  event: TimelineEvent,
  recovered: ReadonlyMap<string, ShellOutcome> | undefined,
): ToolOutcome | null {
  const reported = event.outcome ?? (isCall(event) ? "unknown" : null);
  if (reported === "unknown") return recovered?.get(event.id)?.outcome ?? "unknown";
  return reported;
}

/**
 * True when the session has at least one call and none of them has a known
 * outcome. `[].every()` is true, so the empty case is guarded on purpose:
 * a session with no calls has nothing to say about outcomes.
 */
export function outcomesNotRecorded(
  turns: readonly Turn[],
  recovered: ReadonlyMap<string, ShellOutcome> | undefined,
) {
  let calls = 0;
  for (const turn of turns) {
    for (const event of turn.events) {
      if (!isCall(event)) continue;
      calls++;
      if (effectiveOutcome(event, recovered) !== "unknown") return false;
    }
  }
  return calls > 0;
}
