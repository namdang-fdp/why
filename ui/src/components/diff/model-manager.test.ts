import { describe, expect, it, vi } from "vitest";
import type { FileDiff } from "@/contracts";
import { createModelManager } from "./model-manager";

const modifiedFile: FileDiff = {
  path: "src/file.ts",
  oldPath: null,
  status: "modified",
  oldContent: "before",
  newContent: "after",
  binary: false,
  tooLarge: false,
  pathLossy: false,
};

function fakeMonaco() {
  const live = new Set<string>();
  const dispose = vi.fn((key: string) => {
    expect(live.delete(key)).toBe(true);
  });
  const createModel = (spec: { key: string; path: string; value: string }) => {
    expect(live.has(spec.key)).toBe(false);
    live.add(spec.key);
    return { ...spec, dispose: () => dispose(spec.key) };
  };
  const create = vi.fn(createModel);
  const attach =
    vi.fn<
      (
        models: { original: ReturnType<typeof create>; modified: ReturnType<typeof create> } | null,
      ) => void
    >();
  return { create, createModel, attach, dispose, live };
}

describe("diff model ownership", () => {
  it("keeps one pair alive through 50 file switches and disposes the final pair", () => {
    const { create, attach, dispose, live } = fakeMonaco();
    const manager = createModelManager(create, attach);
    for (let index = 0; index < 50; index++) {
      const diff = { ...modifiedFile, path: `src/file-${index}.ts` };
      manager.show({ worktreeId: "worktree", mode: "uncommitted", diff });
      manager.show({ worktreeId: "worktree", mode: "uncommitted", diff });
      expect(live.size).toBe(2);
    }
    expect(create).toHaveBeenCalledTimes(100);
    expect(dispose).toHaveBeenCalledTimes(98);
    manager.clear();
    manager.clear();
    expect(attach).toHaveBeenLastCalledWith(null);
    expect(dispose).toHaveBeenCalledTimes(100);
    expect(live.size).toBe(0);
  });

  it("includes worktree, Git mode, side and path in model identity", () => {
    const { create, attach, live } = fakeMonaco();
    const manager = createModelManager(create, attach);
    manager.show({ worktreeId: "tree/a", mode: "staged", diff: modifiedFile });
    const firstPair = [...live];
    manager.show({ worktreeId: "tree/a", mode: "vsBase", diff: modifiedFile });
    manager.show({ worktreeId: "tree%2Fa", mode: "vsBase", diff: modifiedFile });
    expect(new Set(create.mock.calls.map(([spec]) => spec.key)).size).toBe(6);
    expect(firstPair.every((key) => !live.has(key))).toBe(true);
  });

  it.each([
    { diff: { ...modifiedFile, status: "added", oldContent: null }, old: "", new: "after" },
    { diff: { ...modifiedFile, status: "deleted", newContent: null }, old: "before", new: "" },
    {
      diff: { ...modifiedFile, status: "renamed", oldPath: "src/file.js" },
      old: "before",
      new: "after",
    },
  ] as const)("preserves both sides of a $diff.status file", ({ diff, old, new: updated }) => {
    const { create, attach } = fakeMonaco();
    createModelManager(create, attach).show({ worktreeId: "worktree", mode: "vsBase", diff });
    expect(attach).toHaveBeenCalledWith({
      original: expect.objectContaining({ path: diff.oldPath ?? diff.path, value: old }),
      modified: expect.objectContaining({ path: diff.path, value: updated }),
    });
  });

  it("disposes the original if the modified model cannot be created", () => {
    const { create, createModel, attach, live } = fakeMonaco();
    create.mockImplementationOnce(createModel).mockImplementationOnce(() => {
      throw new Error("Cannot create the modified model");
    });
    const manager = createModelManager(create, attach);
    expect(() =>
      manager.show({ worktreeId: "worktree", mode: "vsBase", diff: modifiedFile }),
    ).toThrow("Cannot create the modified model");
    expect(live.size).toBe(0);
    expect(attach).not.toHaveBeenCalled();
  });
});
