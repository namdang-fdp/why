import type { FileChange, SessionId, TimelineEvent, Turn, TurnRef } from "@/contracts";

export function isWorktreePath(path: string) {
  return path !== "" && !/^(?:[/\\]|[a-z]:[/\\]|(?:\.\.|~)(?:[/\\]|$))/i.test(path);
}

export function filesInTurn(turn: Turn) {
  return new Set(turn.events.flatMap((event) => event.files.filter(isWorktreePath)));
}

export function matchesFile(paths: ReadonlySet<string>, file: FileChange) {
  return paths.has(file.path) || (file.oldPath !== null && paths.has(file.oldPath));
}

// Group neighboring events for the same subagent without changing recorded order.
export function eventGroups(events: TimelineEvent[]) {
  const groups: { agentId: string | null; events: TimelineEvent[] }[] = [];
  for (const event of events) {
    const group = groups.at(-1);
    if (group && group.agentId === event.agentId) group.events.push(event);
    else groups.push({ agentId: event.agentId, events: [event] });
  }
  return groups;
}

export function newActivity(previous: Turn[], next: Turn[]) {
  const known = new Map(previous.map((turn) => [turn.id, turn]));
  let count = 0;
  for (const turn of next) {
    const old = known.get(turn.id);
    if (old === turn) continue;
    const events = old && new Set(old.events.map(({ id }) => id));
    // A new turn and its initial events are one arrival.
    count += events ? turn.events.filter(({ id }) => !events.has(id)).length : 1;
  }
  return count;
}

/**
 * Indexes of the turns that touched `file`. Two sources, never time:
 * the files named in each turn's events, and the core's edit history for the
 * file (`get_file_activity`), which also knows renames and paths the events
 * only gave as absolute. A turn counts if either source names it.
 */
export function turnsTouchingFile(
  turns: readonly Turn[],
  file: FileChange | undefined,
  activity: readonly TurnRef[] | undefined,
  sessionId: SessionId | undefined,
) {
  const matches = new Set<number>();
  if (!file) return matches;
  const edited = new Set(
    (activity ?? []).filter((ref) => ref.sessionId === sessionId).map((ref) => ref.turnId),
  );
  turns.forEach((turn, index) => {
    if (matchesFile(filesInTurn(turn), file) || (turn.id !== "" && edited.has(turn.id)))
      matches.add(index);
  });
  return matches;
}

/** The recorded kinds a run of calls may be folded from. Never decided from command text. */
const foldable = new Set<TimelineEvent["kind"]>(["shell", "toolCall"]);
/** Fewer consecutive calls than this stay as plain rows. */
export const minRunLength = 3;

export type EventItem =
  { type: "event"; event: TimelineEvent } | { type: "run"; id: string; events: TimelineEvent[] };

/**
 * Fold 3+ consecutive `shell` / `toolCall` events into one run. The input is
 * already one agent's events (`eventGroups`), so a run never spans two agents.
 * An edit, test run, message, subagent or stop between calls splits the run,
 * and none of those is ever folded. The run's id (`run:<first event id>`)
 * is the key its open/closed state is stored under.
 */
export function foldRuns(events: readonly TimelineEvent[]): EventItem[] {
  const items: EventItem[] = [];
  let run: TimelineEvent[] = [];
  const flush = () => {
    const first = run[0];
    if (first && run.length >= minRunLength) {
      items.push({ type: "run", id: `run:${first.id}`, events: run });
    } else {
      for (const event of run) items.push({ type: "event", event });
    }
    run = [];
  };
  for (const event of events) {
    if (foldable.has(event.kind)) {
      run.push(event);
    } else {
      flush();
      items.push({ type: "event", event });
    }
  }
  flush();
  return items;
}

/** "5 shell commands", "4 tool calls" or, for a mix, "6 shell and tool calls". */
export function runLabel(events: readonly TimelineEvent[]) {
  const shells = events.filter((event) => event.kind === "shell").length;
  if (shells === events.length) return `${events.length} shell commands`;
  if (shells === 0) return `${events.length} tool calls`;
  return `${events.length} shell and tool calls`;
}
