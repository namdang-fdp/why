import type { FileDiff } from "@/contracts";

export function hasContentDiff(diff: FileDiff) {
  return !diff.binary && !diff.tooLarge && (diff.oldContent ?? "") !== (diff.newContent ?? "");
}
