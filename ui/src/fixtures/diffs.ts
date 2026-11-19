import type { FileDiff } from "@/contracts";

// Small samples for the diff viewer: modified, added (no old side), deleted (no new
// side), renamed, binary (no text) and too large (no text).
export const fileDiffs = [
  {
    path: "src-tauri/src/git/mod.rs",
    oldPath: null,
    status: "modified",
    oldContent: `//! Git CLI wrapper.

pub fn version_arg() -> &'static str {
    "--version"
}
`,
    newContent: `//! Git CLI wrapper.

pub mod worktree;

pub fn version_arg() -> &'static str {
    "--version"
}

/// Executable name looked up on PATH.
pub const GIT: &str = "git";
`,
    binary: false,
    tooLarge: false,
    pathLossy: false,
  },
  {
    path: "src-tauri/src/git/worktree.rs",
    oldPath: null,
    status: "added",
    oldContent: null,
    newContent: `/// One entry of \`git worktree list --porcelain\`.
pub struct RawWorktree {
    pub path: String,
    pub branch: Option<String>,
}
`,
    binary: false,
    tooLarge: false,
    pathLossy: false,
  },
  {
    path: "src-tauri/src/git/legacy.rs",
    oldPath: null,
    status: "deleted",
    oldContent: `pub fn old_helper() -> u32 {
    42
}
`,
    newContent: null,
    binary: false,
    tooLarge: false,
    pathLossy: false,
  },
  {
    path: "src-tauri/src/git/parse.rs",
    oldPath: "src-tauri/src/git/parser.rs",
    status: "renamed",
    oldContent: `pub fn parse(line: &str) -> Option<&str> {
    line.strip_prefix("worktree ")
}
`,
    newContent: `/// Returns the path from a \`worktree <path>\` porcelain line.
pub fn parse(line: &str) -> Option<&str> {
    line.strip_prefix("worktree ")
}
`,
    binary: false,
    tooLarge: false,
    pathLossy: false,
  },
  {
    path: "src/billing.ts",
    oldPath: null,
    status: "modified",
    oldContent: `export function total(items: number[]): number {
  let sum = 0;
  for (const item of items) sum += item;
  return sum;
}
`,
    newContent: `export function total(items: readonly number[]): number {
  return items.reduce((sum, item) => sum + item, 0);
}
`,
    binary: false,
    tooLarge: false,
    pathLossy: false,
  },
  {
    path: "ui/public/icon.png",
    oldPath: null,
    status: "added",
    oldContent: null,
    newContent: null,
    binary: true,
    tooLarge: false,
    pathLossy: false,
  },
  {
    path: "data/seed.json",
    oldPath: null,
    status: "modified",
    oldContent: null,
    newContent: null,
    binary: false,
    tooLarge: true,
    pathLossy: false,
  },
] satisfies FileDiff[];
