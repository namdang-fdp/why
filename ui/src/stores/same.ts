import type { FileChange, FileDiff } from "@/contracts";

/** Structural equality for the shapes the core sends (plain JSON values). */
export function sameFileChange(a: FileChange, b: FileChange) {
  return (
    a.path === b.path &&
    a.oldPath === b.oldPath &&
    a.status === b.status &&
    a.staged === b.staged &&
    a.unstaged === b.unstaged &&
    a.additions === b.additions &&
    a.deletions === b.deletions &&
    a.binary === b.binary &&
    a.pathLossy === b.pathLossy
  );
}

export function sameFileChanges(a: readonly FileChange[], b: readonly FileChange[]) {
  return (
    a.length === b.length &&
    a.every((file, index) => {
      const other = b[index];
      return other !== undefined && sameFileChange(file, other);
    })
  );
}

export function sameFileDiff(a: FileDiff, b: FileDiff) {
  return (
    a.path === b.path &&
    a.oldPath === b.oldPath &&
    a.status === b.status &&
    a.oldContent === b.oldContent &&
    a.newContent === b.newContent &&
    a.binary === b.binary &&
    a.tooLarge === b.tooLarge &&
    a.pathLossy === b.pathLossy
  );
}

/** Deep equality for plain JSON values (what `invoke` returns). */
export function sameJson(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const left = Object.entries(a);
  if (left.length !== Object.keys(b).length) return false;
  return left.every(
    ([key, value]) => Object.hasOwn(b, key) && sameJson(value, Reflect.get(b, key)),
  );
}
