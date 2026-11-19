// Sample repositories and worktrees for building the UI without the Rust core.
// Every value is checked against the generated contract types with `satisfies`, so
// if a Rust type changes and the bindings are regenerated, `tsc -b ui` flags stale fixtures.
import type { Repository, StorageStatus, Worktree } from "@/contracts";

export const REPO_WHY = "repo-why";
export const REPO_BILLING = "repo-billing-api";

export const WT_WHY_MAIN = "wt-why-main";
export const WT_WHY_AGENT = "wt-why-agent-git";
export const WT_BILLING_DETACHED = "wt-billing-detached";
export const WT_BILLING_BROKEN = "wt-billing-broken";

export const repositories = [
  { id: REPO_WHY, name: "why", rootPath: "/home/dev/code/why" },
  { id: REPO_BILLING, name: "billing-api", rootPath: "/home/dev/code/billing-api" },
] satisfies Repository[];

// Every repository has a main worktree, so two of the three worktrees below are
// `isMain`; the billing one is the main checkout parked on a detached HEAD.
export const worktrees = [
  {
    // Main checkout on the base branch itself: no base to compare against.
    id: WT_WHY_MAIN,
    repoId: REPO_WHY,
    path: "/home/dev/code/why",
    branch: "main",
    headSha: "4be1a7c93d0f5e28a6b9c1d7e4f30a528b6d9e10",
    isMain: true,
    isDetached: false,
    isLocked: false,
    isPrunable: false,
    base: null,
    changeCount: 2,
    loadError: null,
    lastActivity: 1_700_000_000,
  },
  {
    // An agent's linked worktree, three commits ahead of origin/main and one behind.
    id: WT_WHY_AGENT,
    repoId: REPO_WHY,
    path: "/home/dev/code/why-worktrees/WHY-4-git",
    branch: "agent/WHY-4-git-cli",
    headSha: "c8d2f4a1b6e90375d4c2a8f1e6b3097d5a4c8e21",
    isMain: false,
    isDetached: false,
    isLocked: false,
    isPrunable: false,
    base: {
      baseRef: "origin/main",
      mergeBase: "9f3a6e2d1c8b4057a6e9d3f2c1b80475e6a9d3c2",
      ahead: 3,
      behind: 1,
    },
    changeCount: 8,
    loadError: null,
    lastActivity: 1_700_003_600,
  },
  {
    // Detached HEAD (for example a checked-out tag): no branch name.
    id: WT_BILLING_DETACHED,
    repoId: REPO_BILLING,
    path: "/home/dev/code/billing-api",
    branch: null,
    headSha: "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d",
    isMain: true,
    isDetached: true,
    isLocked: false,
    isPrunable: false,
    base: {
      baseRef: "origin/main",
      mergeBase: "7e6d5c4b3a291807f6e5d4c3b2a190807f6e5d4c",
      ahead: 0,
      behind: 5,
    },
    changeCount: 3,
    loadError: null,
    lastActivity: 1_699_990_000,
  },
  {
    // A linked worktree whose `.git` file is corrupt: it is still listed, with
    // no base and no changes, and says why (`loadError`).
    id: WT_BILLING_BROKEN,
    repoId: REPO_BILLING,
    path: "/home/dev/code/billing-api-worktrees/WHY-77-refunds",
    branch: "agent/WHY-77-refunds",
    headSha: null,
    isMain: false,
    isDetached: false,
    isLocked: false,
    isPrunable: false,
    base: null,
    changeCount: 0,
    loadError: { kind: "gitFailed", message: "fatal: not a git repository: /nonexistent/place" },
    lastActivity: null,
  },
] satisfies Worktree[];

/** Where the fixture data pretends to be stored. */
export const storagePersistent = {
  persistent: true,
  path: "/home/dev/.local/share/io.github.namdang-fdp.why/why.db",
  reason: null,
} satisfies StorageStatus;

/** The state after the database failed to open (an in-memory store is used). */
export const storageTemporary = {
  persistent: false,
  path: null,
  reason: "cannot create /home/dev/.local/share/io.github.namdang-fdp.why: permission denied",
} satisfies StorageStatus;
