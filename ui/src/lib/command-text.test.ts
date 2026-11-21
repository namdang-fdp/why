import { describe, expect, it } from "vitest";
import { commandParts } from "./command-text";

describe("commandParts", () => {
  it("leaves an ordinary command as one plain run, keeping double spaces", () => {
    expect(commandParts("git-lfs  clean -- %f")).toEqual([
      { text: "git-lfs  clean -- %f", escaped: false },
    ]);
  });

  it("makes a newline and a tab visible, and still breaks the line", () => {
    expect(commandParts("a\nb\tc")).toEqual([
      { text: "a", escaped: false },
      { text: "\\n", escaped: true },
      { text: "\n", escaped: false },
      { text: "b", escaped: false },
      { text: "\\t", escaped: true },
      { text: "c", escaped: false },
    ]);
  });

  it("escapes other control characters and look-alike invisibles", () => {
    const escapes = commandParts("\u001b\u0000\u007f ​‮﻿")
      .filter((part) => part.escaped)
      .map((part) => part.text);
    expect(escapes).toEqual([
      "\\x1b",
      "\\0",
      "\\x7f",
      "\\xa0",
      "\\u{200b}",
      "\\u{202e}",
      "\\u{feff}",
    ]);
  });

  it("does not touch visible non-ASCII text", () => {
    expect(commandParts("./scripts/données-é.sh")).toEqual([
      { text: "./scripts/données-é.sh", escaped: false },
    ]);
  });
});
