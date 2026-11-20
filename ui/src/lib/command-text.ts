/**
 * Show a git filter command EXACTLY, so the person deciding to trust it reads
 * what git would run. Git hands the string to a shell, where a newline, a tab
 * or a double space inside quotes changes the meaning, and an invisible
 * character (zero-width space, no-break space, a right-to-left override) can
 * make two different commands look identical. Plain text in a `<code>` hides
 * all of that, so every character that is not ordinary and visible is replaced
 * by an escape the reader can see (`\n`, `\t`, `\x1b`, `\u{a0}`).
 */
export type CommandPart = {
  text: string;
  /** An escape sequence standing in for an invisible or control character. */
  escaped: boolean;
};

// Inclusive code point ranges: C0 controls (tab and newline included: they are
// escaped too), DEL and the C1 controls, no-break and soft hyphen, and the
// invisible or direction-changing format characters. Numbers, not a regex
// literal: a literal would need the raw characters (U+2028 even ends a line).
const INVISIBLE: ReadonlyArray<readonly [number, number]> = [
  [0x0000, 0x001f],
  [0x007f, 0x009f],
  [0x00a0, 0x00a0],
  [0x00ad, 0x00ad],
  [0x061c, 0x061c],
  [0x180e, 0x180e],
  [0x200b, 0x200f],
  [0x2028, 0x202e],
  [0x2060, 0x206f],
  [0xfeff, 0xfeff],
];

function isInvisible(char: string) {
  const code = char.codePointAt(0) ?? 0;
  return INVISIBLE.some(([from, to]) => code >= from && code <= to);
}

const NAMED: Readonly<Record<string, string>> = {
  "\n": "\\n",
  "\t": "\\t",
  "\r": "\\r",
  "\0": "\\0",
};

function escapeOf(char: string) {
  const named = NAMED[char];
  if (named) return named;
  const code = char.codePointAt(0) ?? 0;
  return code < 0x100 ? `\\x${code.toString(16).padStart(2, "0")}` : `\\u{${code.toString(16)}}`;
}

/**
 * Split `command` into runs of ordinary text and escapes. A newline is shown as
 * `\n` AND still breaks the line (the following plain `"\n"` part), so a
 * multi-line command keeps its shape. Spaces are left alone: the caller shows
 * them with `white-space: pre-wrap`, which keeps every one.
 */
export function commandParts(command: string): CommandPart[] {
  const parts: CommandPart[] = [];
  let plain = "";
  const flush = () => {
    if (plain) parts.push({ text: plain, escaped: false });
    plain = "";
  };
  for (const char of command) {
    if (!isInvisible(char)) {
      plain += char;
      continue;
    }
    flush();
    parts.push({ text: escapeOf(char), escaped: true });
    if (char === "\n") parts.push({ text: "\n", escaped: false });
  }
  flush();
  return parts;
}
