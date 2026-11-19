import { describe, expect, it } from "vitest";
import type { FileDiff } from "@/contracts";
import { createDiffCache } from "./diff-cache";

const diff = (path: string, size = 10): FileDiff => ({
  path,
  oldPath: null,
  status: "modified",
  oldContent: "o".repeat(size),
  newContent: "n".repeat(size),
  binary: false,
  tooLarge: false,
  pathLossy: false,
});

describe("diff cache", () => {
  it("is keyed by worktree, mode and path", () => {
    const cache = createDiffCache();
    cache.set("w", "vsBase", "a.rs", diff("a.rs"), cache.epoch("w"));
    expect(cache.get("w", "vsBase", "a.rs")?.path).toBe("a.rs");
    expect(cache.get("w", "staged", "a.rs")).toBeUndefined();
    expect(cache.get("other", "vsBase", "a.rs")).toBeUndefined();
  });

  it("evicts the least recently used entry beyond the entry limit", () => {
    const cache = createDiffCache(2);
    const e = cache.epoch("w");
    cache.set("w", "vsBase", "a", diff("a"), e);
    cache.set("w", "vsBase", "b", diff("b"), e);
    cache.get("w", "vsBase", "a"); // a is now the most recent
    cache.set("w", "vsBase", "c", diff("c"), e);
    expect(cache.has("w", "vsBase", "a")).toBe(true);
    expect(cache.has("w", "vsBase", "b")).toBe(false);
    expect(cache.has("w", "vsBase", "c")).toBe(true);
  });

  it("evicts by total text size, and refuses one entry larger than the budget", () => {
    const cache = createDiffCache(100, 100);
    const e = cache.epoch("w");
    cache.set("w", "vsBase", "a", diff("a", 20), e); // 40
    cache.set("w", "vsBase", "b", diff("b", 20), e); // 80
    cache.set("w", "vsBase", "c", diff("c", 20), e); // 120 > 100: a goes
    expect(cache.has("w", "vsBase", "a")).toBe(false);
    expect(cache.size).toBe(2);
    cache.set("w", "vsBase", "huge", diff("huge", 80), e); // 160 alone: refused
    expect(cache.has("w", "vsBase", "huge")).toBe(false);
    expect(cache.size).toBe(2);
  });

  it("invalidating paths drops that file in every mode and nothing else", () => {
    const cache = createDiffCache();
    const e = cache.epoch("w");
    cache.set("w", "vsBase", "a.rs", diff("a.rs"), e);
    cache.set("w", "staged", "a.rs", diff("a.rs"), e);
    cache.set("w", "vsBase", "b.rs", diff("b.rs"), e);
    cache.set("x", "vsBase", "a.rs", diff("a.rs"), cache.epoch("x"));

    cache.invalidatePaths("w", ["a.rs"]);

    expect(cache.has("w", "vsBase", "a.rs")).toBe(false);
    expect(cache.has("w", "staged", "a.rs")).toBe(false);
    expect(cache.has("w", "vsBase", "b.rs")).toBe(true);
    expect(cache.has("x", "vsBase", "a.rs")).toBe(true);
  });

  it("invalidating a worktree drops all of it", () => {
    const cache = createDiffCache();
    const e = cache.epoch("w");
    cache.set("w", "vsBase", "a", diff("a"), e);
    cache.set("w", "staged", "b", diff("b"), e);
    cache.set("x", "vsBase", "a", diff("a"), cache.epoch("x"));
    cache.invalidateWorktree("w");
    expect(cache.size).toBe(1);
    expect(cache.has("x", "vsBase", "a")).toBe(true);
  });

  it("refuses an answer from a read that began before an invalidation", () => {
    const cache = createDiffCache();
    const epoch = cache.epoch("w"); // read starts
    cache.invalidatePaths("w", ["a.rs"]); // the file changes meanwhile
    cache.set("w", "vsBase", "a.rs", diff("a.rs"), epoch); // stale answer arrives
    expect(cache.has("w", "vsBase", "a.rs")).toBe(false);
    cache.set("w", "vsBase", "a.rs", diff("a.rs"), cache.epoch("w")); // a fresh read is fine
    expect(cache.has("w", "vsBase", "a.rs")).toBe(true);
  });
});
