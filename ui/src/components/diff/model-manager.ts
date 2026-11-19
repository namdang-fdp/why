import type { DiffMode, FileDiff, WorktreeId } from "@/contracts";

type DiffPreview = { worktreeId: WorktreeId; mode: DiffMode; diff: FileDiff };
type ModelSpec = { key: string; path: string; value: string };
type ModelPair<Model> = { original: Model; modified: Model };

/**
 * Owns the one pair of Monaco models behind the diff editor.
 *
 * `show` returns `true` when it attached a NEW pair (a different file, mode or
 * worktree) and `false` when it kept the pair: either nothing changed, or the
 * same file's content changed and `update` rewrote the models in place. Keeping
 * the pair is what makes a live update invisible: `setModel` would reset the
 * scroll position, the cursor and the expanded unchanged regions.
 */
export function createModelManager<Model extends { dispose(): void }>(
  create: (spec: ModelSpec) => Model,
  attach: (models: ModelPair<Model> | null) => void,
  update?: (model: Model, value: string) => void,
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
    show(preview: DiffPreview): boolean {
      const sameTarget =
        current?.preview.worktreeId === preview.worktreeId &&
        current.preview.mode === preview.mode &&
        current.preview.diff.path === preview.diff.path &&
        current.preview.diff.oldPath === preview.diff.oldPath;
      if (current && sameTarget) {
        const { diff: previous } = current.preview;
        if (previous === preview.diff) return false;
        if (update) {
          // Same file, new content (a live update): rewrite the models in place.
          if (previous.oldContent !== preview.diff.oldContent) {
            update(current.models.original, preview.diff.oldContent ?? "");
          }
          if (previous.newContent !== preview.diff.newContent) {
            update(current.models.modified, preview.diff.newContent ?? "");
          }
          current.preview = preview;
          return false;
        }
      }

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
      return true;
    },
  };
}
