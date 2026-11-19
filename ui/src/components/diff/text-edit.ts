/** One replacement in a string: `text` replaces `old.slice(start, end)`. */
export type TextEdit = { start: number; end: number; text: string };

/**
 * The smallest single replacement that turns `before` into `after`, found by
 * trimming the common prefix and suffix; `null` when they are equal.
 *
 * Used to update a Monaco model in place after a live file change. Replacing
 * only the part that differs (instead of the whole text) keeps Monaco's
 * line tracking, decorations, cursor and scroll position intact for
 * everything outside the edit, so the open diff does not jump or flicker.
 */
export function minimalEdit(before: string, after: string): TextEdit | null {
  if (before === after) return null;
  const limit = Math.min(before.length, after.length);
  let start = 0;
  while (start < limit && before.charCodeAt(start) === after.charCodeAt(start)) start++;
  // Do not split a surrogate pair: back up if the match ended after a high surrogate.
  if (start > 0 && isHighSurrogate(before.charCodeAt(start - 1))) start--;

  let endBefore = before.length;
  let endAfter = after.length;
  while (
    endBefore > start &&
    endAfter > start &&
    before.charCodeAt(endBefore - 1) === after.charCodeAt(endAfter - 1)
  ) {
    endBefore--;
    endAfter--;
  }
  if (endBefore < before.length && isLowSurrogate(before.charCodeAt(endBefore))) {
    endBefore++;
    endAfter++;
  }
  return { start, end: endBefore, text: after.slice(start, endAfter) };
}

const isHighSurrogate = (code: number) => code >= 0xd800 && code <= 0xdbff;
const isLowSurrogate = (code: number) => code >= 0xdc00 && code <= 0xdfff;

/** Monaco drops a leading U+FEFF (byte order mark) from a model's text. */
export function stripBom(text: string) {
  return text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
}

/**
 * The edit that turns a Monaco model's text into `incoming`, compared the way
 * Monaco stores text: `model.getValue()` never contains the BOM, but git's text
 * may. Without stripping it here, a live edit of a BOM file would insert the BOM
 * as a real character on the first line and the diff would show a change that is
 * not there (and differ from what opening the file afresh shows).
 */
export function modelEdit(modelText: string, incoming: string): TextEdit | null {
  return minimalEdit(modelText, stripBom(incoming));
}
