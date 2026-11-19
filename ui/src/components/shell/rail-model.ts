import type { FileChange, Repository, Worktree, WorktreeId } from "@/contracts";
import { branchLabel } from "./change-labels";

/**
 * The rail's logic, kept free of React so each rule can be tested on its own:
 * how a worktree is named, in what order worktrees are listed, which of them are
 * folded into "N clean", how the filter matches, and how "2m" is written.
 */

// ---- names --------------------------------------------------------------------

const segments = (path: string) => path.split(/[/\\]/).filter(Boolean);

/** The last path segment: the worktree's folder name. */
export function folderName(path: string) {
  return segments(path).at(-1) ?? path;
}

/**
 * A name that tells folders of the same name apart: `parent/folder`, or just the
 * parent when the folder is only the repository's name (it adds nothing).
 */
function folderWithParent(path: string, repoName: string | undefined) {
  const [parent, folder] = segments(path).slice(-2);
  if (parent === undefined || folder === undefined) return folderName(path);
  return folder === repoName ? parent : `${parent}/${folder}`;
}

export type WorktreeLabel = {
  /** What identifies the worktree: its branch, or `detached @ sha`. */
  primary: string;
  /** The folder, shown only when it adds information; `null` otherwise. */
  secondary: string | null;
};

/**
 * Names for every worktree of one repository.
 *
 * The title is the BRANCH (or `detached @ sha`), because that is what tells
 * worktrees apart: agents' worktrees are often folders with the same name as the
 * repository, so the folder makes every row look identical. The folder is shown
 * underneath only when it is informative:
 *
 * - it is not the branch (or the branch's last segment: `agent/x` in a folder `x`);
 * - it is not just the repository's name, which every such folder repeats;
 * - or two worktrees would otherwise share a title (two detached HEADs at the same
 *   commit): then the folder is always shown, widened to `parent/folder` (or just
 *   the parent, for a folder named like the repository) when folder names collide too.
 */
export function worktreeLabels(worktrees: readonly Worktree[], repo: Repository | undefined) {
  const primaries = new Map<WorktreeId, string>(worktrees.map((wt) => [wt.id, branchLabel(wt)]));
  const titleCount = new Map<string, number>();
  for (const title of primaries.values()) titleCount.set(title, (titleCount.get(title) ?? 0) + 1);

  const folders = new Map<string, number>();
  for (const wt of worktrees) {
    const folder = folderName(wt.path);
    folders.set(folder, (folders.get(folder) ?? 0) + 1);
  }

  const labels = new Map<WorktreeId, WorktreeLabel>();
  for (const wt of worktrees) {
    const primary = primaries.get(wt.id) ?? branchLabel(wt);
    const folder = folderName(wt.path);
    const ambiguous = (titleCount.get(primary) ?? 0) > 1;
    const branchTail = wt.branch ? (segments(wt.branch).at(-1) ?? wt.branch) : null;
    const redundant =
      folder === primary || folder === wt.branch || folder === branchTail || folder === repo?.name;

    let secondary: string | null = null;
    if (ambiguous) {
      // Needed to tell rows apart; widen to `parent/folder` if folders collide too.
      secondary = (folders.get(folder) ?? 0) > 1 ? folderWithParent(wt.path, repo?.name) : folder;
    } else if (!redundant) {
      secondary = folder;
    }
    labels.set(wt.id, { primary, secondary });
  }
  return labels;
}

// ---- activity and order -----------------------------------------------------------

/** The newest of what the core measured and what the watcher has seen since (Unix seconds). */
export function activityOf(worktree: Worktree, seen: ReadonlyMap<WorktreeId, number>) {
  const measured = worktree.lastActivity;
  const live = seen.get(worktree.id);
  if (measured === null && live === undefined) return null;
  return Math.max(measured ?? 0, live ?? 0);
}

/**
 * Main worktree first (the anchor), then the most recently active, then by name.
 * Never mutates its input.
 */
export function sortWorktrees(
  worktrees: readonly Worktree[],
  labels: ReadonlyMap<WorktreeId, WorktreeLabel>,
  seen: ReadonlyMap<WorktreeId, number>,
) {
  return [...worktrees].sort((a, b) => {
    if (a.isMain !== b.isMain) return a.isMain ? -1 : 1;
    const [ta, tb] = [activityOf(a, seen), activityOf(b, seen)];
    if (ta !== tb) {
      if (ta === null) return 1;
      if (tb === null) return -1;
      return tb - ta;
    }
    const [la, lb] = [labels.get(a.id)?.primary ?? "", labels.get(b.id)?.primary ?? ""];
    return la.localeCompare(lb, undefined, { numeric: true, sensitivity: "base" });
  });
}

/** Is there nothing to review in this worktree? (And nothing wrong with it.) */
export function isClean(worktree: Worktree) {
  return worktree.changeCount === 0 && worktree.loadError === null;
}

/**
 * Split a sorted list into the rows always shown and the ones folded under
 * "N clean". Shown: the main worktree (the anchor), the selected one, and
 * anything with changes or an error. While filtering, nothing is folded: the
 * filter already narrowed the list and hiding a match would be wrong.
 */
export function groupWorktrees(
  sorted: readonly Worktree[],
  selectedId: WorktreeId | null,
  filtering: boolean,
) {
  if (filtering) return { shown: [...sorted], clean: [] as Worktree[] };
  const shown: Worktree[] = [];
  const clean: Worktree[] = [];
  for (const wt of sorted) {
    if (wt.isMain || wt.id === selectedId || !isClean(wt)) shown.push(wt);
    else clean.push(wt);
  }
  return { shown, clean };
}

// ---- time ---------------------------------------------------------------------------

/** "now", "2m", "3h", "5d", "6w": the age of `then` as of `now` (both Unix seconds). */
export function formatRelative(now: number, then: number | null) {
  if (then === null) return null;
  const seconds = Math.max(0, now - then);
  if (seconds < 45) return "now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${Math.max(1, minutes)}m`;
  const hours = Math.round(seconds / 3600);
  if (hours < 24) return `${hours}h`;
  const days = Math.round(seconds / 86400);
  if (days < 7) return `${days}d`;
  const weeks = Math.round(days / 7);
  if (weeks < 53) return `${weeks}w`;
  return `${Math.round(days / 365)}y`;
}

// ---- filter ----------------------------------------------------------------------------

/** Every whitespace-separated term of `filter` appears (case-insensitive) in some haystack. */
export function matchesFilter(haystacks: ReadonlyArray<string | null | undefined>, filter: string) {
  const terms = filter.toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return true;
  const text = haystacks.filter(Boolean).join("\n").toLowerCase();
  return terms.every((term) => text.includes(term));
}

export function worktreeMatches(
  worktree: Worktree,
  label: WorktreeLabel | undefined,
  filter: string,
) {
  return matchesFilter(
    [
      label?.primary,
      label?.secondary,
      worktree.branch,
      worktree.path,
      worktree.headSha?.slice(0, 7),
    ],
    filter,
  );
}

export function fileMatches(file: FileChange, filter: string) {
  return matchesFilter([file.path, file.oldPath], filter);
}

export function repositoryMatches(repo: Repository, filter: string) {
  return matchesFilter([repo.name, repo.rootPath], filter);
}
