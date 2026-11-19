import { describe, expect, it } from "vitest";
import { detectLanguage } from "./detect-language";

describe("diff language detection", () => {
  const registered = [
    { id: "rust", extensions: [".rs"] },
    { id: "typescript", extensions: [".ts", ".tsx"] },
    { id: "definition", extensions: [".d.ts"] },
    { id: "dockerfile", filenames: ["Dockerfile", "Dockerfile.ts"] },
    { id: "ignore", filenames: [".gitignore"] },
  ];

  it.each([
    ["src/lib.RS", "rust"],
    ["src/App.tsx", "typescript"],
    ["src/extra.d.ts", "definition"],
    ["deploy/Dockerfile", "dockerfile"],
    ["deploy/dockerfile.ts", "dockerfile"],
    [".gitignore", "ignore"],
    ["some.rs/README", "plaintext"],
    ["archive.unknown", "plaintext"],
  ])("uses the registered language for %s", (path, expected) => {
    expect(detectLanguage(path, registered)).toBe(expected);
  });
});
