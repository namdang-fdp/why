import { describe, expect, it } from "vitest";
import type { Repository, Worktree } from "@/contracts";
import {
  fileMatches,
  folderName,
  formatRelative,
  groupWorktrees,
  matchesFilter,
  repositoryMatches,
  sortWorktrees,
  worktreeLabels,
  worktreeMatches,
} from "./rail-model";

const repo: Repository = {
  id: "r",
  name: "ai-interview-practice",
  rootPath: "/home/me/ai-interview-practice",
};

let counter = 0;
function wt(overrides: Partial<Worktree> & { path: string }): Worktree {
  counter += 1;
  return {
    id: `w${counter}`,
    repoId: "r",
    branch: "feature/x",
    headSha: "abcdef0123456789",
    isMain: false,
    isDetached: false,
    isLocked: false,
    isPrunable: false,
    base: null,
    changeCount: 0,
    loadError: null,
    lastActivity: null,
    ...overrides,
  };
}

const labelOf = (list: Worktree[], target: Worktree) => worktreeLabels(list, repo).get(target.id);

describe("worktree labels", () => {
  it("titles a worktree by its branch, not its folder", () => {
    const a = wt({ path: "/work/orca/a1b2", branch: "agent/login" });
    expect(labelOf([a], a)).toEqual({ primary: "agent/login", secondary: "a1b2" });
  });

  it("titles a detached HEAD by its short commit", () => {
    const d = wt({ path: "/work/old", branch: null, isDetached: true, headSha: "1a2b3c4d5e6f" });
    expect(labelOf([d], d)?.primary).toBe("detached @ 1a2b3c4");
  });

  it("hides a folder that only repeats the branch or its last segment", () => {
    const same = wt({ path: "/work/login", branch: "login" });
    const tail = wt({ path: "/work/login-2", branch: "agent/login-2" });
    expect(labelOf([same], same)?.secondary).toBeNull();
    expect(labelOf([tail], tail)?.secondary).toBeNull();
  });

  it("hides a folder that is just the repository's name (the main checkout, orca folders)", () => {
    const main = wt({ path: "/home/me/ai-interview-practice", branch: "main", isMain: true });
    const orca = wt({ path: "/orca/x/ai-interview-practice", branch: "agent/a" });
    expect(labelOf([main, orca], main)?.secondary).toBeNull();
    expect(labelOf([main, orca], orca)?.secondary).toBeNull();
  });

  it("shows a folder that adds information", () => {
    const w = wt({ path: "/work/WHY-77-refunds", branch: "agent/billing" });
    expect(labelOf([w], w)?.secondary).toBe("WHY-77-refunds");
  });

  it("forces the folder when two worktrees would share a title", () => {
    const one = wt({ path: "/work/one", branch: null, isDetached: true, headSha: "aaaaaaa1" });
    const two = wt({ path: "/work/two", branch: null, isDetached: true, headSha: "aaaaaaa1" });
    expect(labelOf([one, two], one)).toEqual({ primary: "detached @ aaaaaaa", secondary: "one" });
    expect(labelOf([one, two], two)?.secondary).toBe("two");
  });

  it("widens to parent/folder when the folder names collide as well", () => {
    const one = wt({
      path: "/orca/aaa/ai-interview-practice",
      branch: null,
      isDetached: true,
      headSha: "bbbbbbb1",
    });
    const two = wt({
      path: "/orca/bbb/ai-interview-practice",
      branch: null,
      isDetached: true,
      headSha: "bbbbbbb1",
    });
    // The folder is just the repository's name: the parent is what tells them apart.
    expect(labelOf([one, two], one)?.secondary).toBe("aaa");
    expect(labelOf([one, two], two)?.secondary).toBe("bbb");
  });

  it("keeps parent/folder when the colliding folder is not the repository's name", () => {
    const one = wt({ path: "/a/x/scratch", branch: null, isDetached: true, headSha: "ccccccc1" });
    const two = wt({ path: "/a/y/scratch", branch: null, isDetached: true, headSha: "ccccccc1" });
    expect(labelOf([one, two], one)?.secondary).toBe("x/scratch");
    expect(labelOf([one, two], two)?.secondary).toBe("y/scratch");
  });

  it("extracts folder names from either path separator", () => {
    expect(folderName("/a/b/c/")).toBe("c");
    expect(folderName("C:\\code\\why")).toBe("why");
  });
});

describe("sorting", () => {
  const labels = (list: Worktree[]) => worktreeLabels(list, repo);

  it("puts main first, then the most recently active, then by name", () => {
    const main = wt({ path: "/m", branch: "main", isMain: true, lastActivity: 1 });
    const old = wt({ path: "/o", branch: "agent/old", lastActivity: 100 });
    const recent = wt({ path: "/r", branch: "agent/recent", lastActivity: 900 });
    const bravo = wt({ path: "/b", branch: "agent/bravo", lastActivity: null });
    const alpha = wt({ path: "/a", branch: "agent/alpha", lastActivity: null });
    const list = [alpha, old, main, bravo, recent];
    expect(sortWorktrees(list, labels(list), new Map()).map((w) => w.branch)).toEqual([
      "main",
      "agent/recent",
      "agent/old",
      "agent/alpha",
      "agent/bravo",
    ]);
  });

  it("lets a live watcher event move a worktree to the top (but never above main)", () => {
    const main = wt({ path: "/m", branch: "main", isMain: true });
    const a = wt({ path: "/a", branch: "agent/a", lastActivity: 500 });
    const b = wt({ path: "/b", branch: "agent/b", lastActivity: 100 });
    const list = [main, a, b];
    const order = sortWorktrees(list, labels(list), new Map([[b.id, 9999]])).map((w) => w.branch);
    expect(order).toEqual(["main", "agent/b", "agent/a"]);
  });

  it("orders names naturally (agent-2 before agent-10) and does not mutate the input", () => {
    const ten = wt({ path: "/10", branch: "agent-10" });
    const two = wt({ path: "/2", branch: "agent-2" });
    const list = [ten, two];
    expect(sortWorktrees(list, labels(list), new Map()).map((w) => w.branch)).toEqual([
      "agent-2",
      "agent-10",
    ]);
    expect(list[0]).toBe(ten);
  });
});

describe("clean grouping", () => {
  const main = wt({ path: "/m", branch: "main", isMain: true });
  const dirty = wt({ path: "/d", branch: "agent/d", changeCount: 3 });
  const broken = wt({ path: "/x", branch: "agent/x", loadError: { kind: "timeout" } });
  const cleanA = wt({ path: "/c1", branch: "agent/c1" });
  const cleanB = wt({ path: "/c2", branch: "agent/c2" });
  const all = [main, dirty, cleanA, broken, cleanB];

  it("folds clean worktrees, keeping main, changed, broken and selected ones visible", () => {
    const { shown, clean } = groupWorktrees(all, cleanB.id, false);
    expect(shown.map((w) => w.branch)).toEqual(["main", "agent/d", "agent/x", "agent/c2"]);
    expect(clean.map((w) => w.branch)).toEqual(["agent/c1"]);
  });

  it("folds everything clean when nothing is selected", () => {
    const { clean } = groupWorktrees(all, null, false);
    expect(clean).toHaveLength(2);
  });

  it("never folds while filtering", () => {
    const { shown, clean } = groupWorktrees(all, null, true);
    expect(shown).toHaveLength(5);
    expect(clean).toEqual([]);
  });
});

describe("relative time", () => {
  const now = 1_700_000_000;
  it.each([
    [0, "now"],
    [30, "now"],
    [60, "1m"],
    [120, "2m"],
    [59 * 60, "59m"],
    [3600, "1h"],
    [3 * 3600, "3h"],
    [23 * 3600, "23h"],
    [86400, "1d"],
    [6 * 86400, "6d"],
    [14 * 86400, "2w"],
    [400 * 86400, "1y"],
  ])("%i seconds ago is %s", (age, text) => {
    expect(formatRelative(now, now - age)).toBe(text);
  });

  it("has no text when the time is unknown, and never goes negative", () => {
    expect(formatRelative(now, null)).toBeNull();
    expect(formatRelative(now, now + 5000)).toBe("now");
  });
});

describe("filter", () => {
  it("matches case-insensitive substrings, every term somewhere", () => {
    expect(matchesFilter(["src/Git/mod.rs"], "git MOD")).toBe(true);
    expect(matchesFilter(["src/git/mod.rs"], "git nope")).toBe(false);
    expect(matchesFilter(["a"], "   ")).toBe(true);
  });

  it("matches worktrees by branch, folder, path and short sha", () => {
    const w = wt({ path: "/orca/zz9/thing", branch: "agent/login", headSha: "9f8e7d6c5b" });
    const label = worktreeLabels([w], repo).get(w.id);
    for (const query of ["login", "zz9", "orca", "9f8e7d6"]) {
      expect(worktreeMatches(w, label, query)).toBe(true);
    }
    expect(worktreeMatches(w, label, "unrelated")).toBe(false);
  });

  it("matches files by path or old path, and repositories by name or path", () => {
    expect(
      fileMatches(
        {
          path: "a/new.rs",
          oldPath: "a/old.rs",
          status: "renamed",
          staged: true,
          unstaged: false,
          additions: 1,
          deletions: 1,
          binary: false,
          pathLossy: false,
        },
        "old",
      ),
    ).toBe(true);
    expect(repositoryMatches(repo, "interview")).toBe(true);
    expect(repositoryMatches(repo, "zzz")).toBe(false);
  });
});
