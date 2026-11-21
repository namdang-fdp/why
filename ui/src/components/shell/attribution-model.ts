import type {
  DiffMode,
  FileAttribution,
  Harness,
  IpcError,
  SessionSummary,
  ToolOutcome,
  TurnRef,
  WorktreeAttribution,
  WorktreeId,
} from "@/contracts";
import type { AsyncState } from "@/data/async-state";

/**
 * How the rail describes a file. The names are about EVIDENCE, not people:
 * `unexplained` only means no captured applied edit explains the file. It never
 * says who changed it (a human, a shell command, a lost hook event and a change
 * from before hooks were installed all look the same here).
 */
export type FileEvidenceKind = "explained" | "attempted" | "unexplained";

export function evidenceKind(file: FileAttribution): FileEvidenceKind {
  if (file.status === "explained") return "explained";
  return file.attempts.length > 0 ? "attempted" : "unexplained";
}

export function harnessesOf(refs: readonly TurnRef[]): Harness[] {
  return [...new Set(refs.map((ref) => ref.harness))];
}

/** Distinct turns (a turn with two edits counts once). */
export function turnCount(refs: readonly TurnRef[]) {
  return new Set(refs.map((ref) => `${ref.sessionId}\u0000${ref.turnId}\u0000${ref.turnIndex}`))
    .size;
}

/** What the call did, in words that do not claim more than the harness said. */
export const refOutcomeLabel: Record<ToolOutcome, string> = {
  ok: "Applied",
  failed: "Failed",
  // A start with no result is a rejected edit OR a lost delivery: never say "rejected".
  unknown: "Not confirmed",
};

export function taskTitles(sessions: readonly SessionSummary[]) {
  return new Map(sessions.map((session) => [session.id, session.task]));
}

export const unexplainedExplanation =
  "No captured agent edit explains this file. It may be an edit made outside the agent, a " +
  "shell command (sed, a formatter), a hook event that was lost, or a change from before hooks " +
  "were installed.";

export const explainedCaveat =
  "A file listed here can still contain changes WHY did not see. Line-level provenance comes later.";

/**
 * The chip for a file with attempts but no applied edit. "Not applied" only when
 * the harness itself said every attempt failed; an attempt with no result may
 * have succeeded (its result hook may simply have been lost).
 */
export function attemptLabel(file: FileAttribution) {
  return file.attempts.every((ref) => ref.outcome === "failed")
    ? "Agent tried, not applied"
    : "Agent edit not confirmed";
}

/**
 * Attribution for ONE worktree and comparison, or why there is none yet.
 * `ok` carries the answer only when it was computed for exactly the worktree
 * and diff mode being shown; an answer for anything else is `loading`, never
 * numbers from another worktree. The rail, the File tab and the coverage bar
 * all read it through this one function so they cannot disagree.
 */
export type CurrentAttribution =
  | { status: "idle" | "loading" }
  | { status: "error"; error: IpcError }
  | { status: "ok"; data: WorktreeAttribution };

export function currentAttribution(
  state: AsyncState<WorktreeAttribution>,
  worktreeId: WorktreeId | null,
  mode: DiffMode | undefined,
): CurrentAttribution {
  if (worktreeId === null || mode === undefined) return { status: "idle" };
  if (state.status !== "ok") return state;
  return state.data.worktreeId === worktreeId && state.data.mode === mode
    ? state
    : { status: "loading" };
}

export type CoverageCounts = Record<FileEvidenceKind, number> & { total: number };

/** How many changed files fall in each evidence bucket (they always sum to `total`). */
export function coverageCounts(files: readonly FileAttribution[]): CoverageCounts {
  const counts: CoverageCounts = {
    explained: 0,
    attempted: 0,
    unexplained: 0,
    total: files.length,
  };
  for (const file of files) counts[evidenceKind(file)]++;
  return counts;
}

/** The File tab's one state chip: a label only, never a claim about who made the change. */
export function fileChipLabel(file: FileAttribution) {
  const kind = evidenceKind(file);
  if (kind === "explained") {
    const edits = file.explainedBy.length;
    return `Explained by ${edits} captured ${edits === 1 ? "edit" : "edits"}`;
  }
  return kind === "attempted" ? attemptLabel(file) : "No captured agent edit";
}

export const fileCaveat =
  "WHY may not have seen every change to this file. Line-level provenance comes later.";
