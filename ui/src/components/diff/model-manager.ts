import type { DiffMode, FileDiff, WorktreeId } from "@/contracts";

type DiffPreview = { worktreeId: WorktreeId; mode: DiffMode; diff: FileDiff };
type ModelSpec = { key: string; path: string; value: string };
type ModelPair<Model> = { original: Model; modified: Model };

export function createModelManager<Model extends { dispose(): void }>(
  create: (spec: ModelSpec) => Model,
  attach: (models: ModelPair<Model> | null) => void,
) {
  let current: { preview: DiffPreview; models: ModelPair<Model> } | null = null;

  function clear() {
    if (!current) return;
    attach(null);
    current.models.original.dispose();
    current.models.modified.dispose();
    current = null;
  }

  return {
    clear,
    show(preview: DiffPreview) {
      if (
        current?.preview.diff === preview.diff &&
        current.preview.worktreeId === preview.worktreeId &&
        current.preview.mode === preview.mode
      )
        return;

      clear();
      const { worktreeId, mode, diff } = preview;
      const originalPath = diff.oldPath ?? diff.path;
      const key = (side: string, path: string) =>
        "/" + [worktreeId, mode, side, path].map(encodeURIComponent).join("/");
      const original = create({
        key: key("original", originalPath),
        path: originalPath,
        value: diff.oldContent ?? "",
      });
      let modified: Model;
      try {
        modified = create({
          key: key("modified", diff.path),
          path: diff.path,
          value: diff.newContent ?? "",
        });
      } catch (error) {
        original.dispose();
        throw error;
      }
      const models = { original, modified };
      current = { preview, models };
      attach(models);
    },
  };
}
