import type { FileChange, TimelineEvent, Turn } from "@/contracts";

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
