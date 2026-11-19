// A repository that looks like a real agent-heavy one: twelve worktrees, several
// folders named after the repository (orca-style), two detached HEADs at the same
// commit, one that fails to load, one locked. For judging the rail with a crowd:
// `pnpm dev:ui` then open `/?many=1`.
import type { FileChange, FileDiff, Repository, Worktree } from "@/contracts";

const REPO_ID = "repo-ai-interview-practice";
const ROOT = "/home/dev/code/ai-interview-practice";

export const manyRepository = {
  id: REPO_ID,
  name: "ai-interview-practice",
  rootPath: ROOT,
} satisfies Repository;

type Spec = {
  folder: string;
  branch: string | null;
  sha?: string;
  changes: number;
  /** Seconds ago the worktree was last touched. */
  ago: number;
  ahead?: number;
  behind?: number;
  main?: boolean;
  locked?: boolean;
  broken?: boolean;
};

const MIN = 60;
const HOUR = 3600;
const DAY = 86400;

const specs: Spec[] = [
  { folder: ROOT, branch: "main", changes: 0, ago: 3 * DAY, main: true },
  {
    folder: "/home/dev/orca/a1b2/ai-interview-practice",
    branch: "agent/login-flow",
    changes: 7,
    ago: 2 * MIN,
    ahead: 4,
  },
  {
    folder: "/home/dev/orca/c3d4/ai-interview-practice",
    branch: "agent/scoring",
    changes: 3,
    ago: 18 * MIN,
    ahead: 2,
    behind: 1,
  },
  {
    folder: "/home/dev/orca/e5f6/ai-interview-practice",
    branch: null,
    sha: "1a2b3c4d5e6f",
    changes: 0,
    ago: 2 * HOUR,
  },
  {
    folder: "/home/dev/orca/0a1b/ai-interview-practice",
    branch: null,
    sha: "1a2b3c4d5e6f",
    changes: 0,
    ago: 5 * HOUR,
  },
  {
    folder: "/home/dev/code/ai-interview-practice-worktrees/fix-timer",
    branch: "fix/timer",
    changes: 2,
    ago: 40 * MIN,
    ahead: 1,
  },
  {
    folder: "/home/dev/code/ai-interview-practice-worktrees/feature-a",
    branch: "feature/question-bank",
    changes: 0,
    ago: DAY,
    ahead: 9,
    behind: 3,
  },
  {
    folder: "/home/dev/code/ai-interview-practice-worktrees/refactor-ui",
    branch: "refactor/ui",
    changes: 12,
    ago: 3 * HOUR,
    ahead: 6,
    behind: 6,
  },
  {
    folder: "/home/dev/tmp/review-pr-42",
    branch: null,
    sha: "9f8e7d6c5b4a",
    changes: 1,
    ago: DAY + 3 * HOUR,
  },
  {
    folder: "/home/dev/orca/9z9z/ai-interview-practice",
    branch: "agent/broken",
    changes: 0,
    ago: 0,
    broken: true,
  },
  {
    folder: "/home/dev/code/ai-interview-practice-worktrees/locked-wt",
    branch: "chore/deps",
    changes: 0,
    ago: 4 * DAY,
    locked: true,
    ahead: 1,
  },
  {
    folder: "/home/dev/code/ai-interview-practice-worktrees/docs-pass",
    branch: "docs/pass",
    changes: 0,
    ago: 6 * DAY,
    ahead: 2,
  },
];

const statuses = ["modified", "added", "modified", "deleted", "renamed", "modified"] as const;

function changesFor(id: string, count: number): FileChange[] {
  return Array.from({ length: count }, (_, i) => ({
    path: `src/modules/${id}/feature-${i + 1}.ts`,
    oldPath: null,
    status: statuses[i % statuses.length] ?? "modified",
    staged: i % 3 === 0,
    unstaged: i % 3 !== 0,
    additions: 3 + ((i * 7) % 40),
    deletions: (i * 3) % 9,
    binary: false,
    pathLossy: false,
  }));
}

/** Everything the fixture source needs to add the crowd, with times relative to `nowSeconds`. */
export function buildManyWorktrees(nowSeconds: number) {
  const worktrees: Worktree[] = [];
  const changes: Record<string, FileChange[]> = {};
  specs.forEach((spec, index) => {
    const id = `wt-aip-${index + 1}`;
    changes[id] = spec.broken ? [] : changesFor(id, spec.changes);
    worktrees.push({
      id,
      repoId: REPO_ID,
      path: spec.folder,
      branch: spec.branch,
      headSha: spec.sha
        ? spec.sha.padEnd(40, "0")
        : `${(index + 1).toString(16).padStart(2, "0")}c0ffee`.padEnd(40, "0"),
      isMain: spec.main === true,
      isDetached: spec.branch === null,
      isLocked: spec.locked === true,
      isPrunable: false,
      base: spec.main
        ? null
        : {
            baseRef: "origin/main",
            mergeBase: "9f3a6e2d1c8b4057a6e9d3f2c1b80475e6a9d3c2",
            ahead: spec.ahead ?? 0,
            behind: spec.behind ?? 0,
          },
      changeCount: changes[id]?.length ?? 0,
      loadError: spec.broken
        ? { kind: "gitFailed", message: "fatal: not a git repository: /nonexistent/place" }
        : null,
      lastActivity: spec.broken ? null : nowSeconds - spec.ago,
    });
  });
  return { repository: manyRepository, worktrees, changes };
}

/** A small modified-file diff for any synthetic path (the crowd has no real files). */
export function manyDiff(path: string, status: FileChange["status"]): FileDiff {
  const name = path.split("/").at(-1)?.replace(/\.ts$/, "") ?? "value";
  return {
    path,
    oldPath: null,
    status,
    oldContent: status === "added" ? null : `export function ${name}() {\n  return 1;\n}\n`,
    newContent:
      status === "deleted"
        ? null
        : `export function ${name}() {\n  // reworked\n  return 2;\n}\n\nexport const ${name}Label = "${name}";\n`,
    binary: false,
    tooLarge: false,
    pathLossy: false,
  };
}
