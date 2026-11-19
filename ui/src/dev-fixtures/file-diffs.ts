import type { FileDiff } from "@/contracts";
import { WT_BILLING_DETACHED, WT_WHY_AGENT, WT_WHY_MAIN } from "@/fixtures";

export const supplementaryDiffs = {
  [WT_WHY_MAIN]: [
    {
      path: "scripts/link.sh",
      oldPath: null,
      status: "typeChanged",
      oldContent: "#!/bin/sh\nexec ./scripts/run.sh\n",
      newContent: "run.sh",
      binary: false,
      tooLarge: false,
      pathLossy: false,
    },
    {
      path: "src-tauri/src/lib.rs",
      oldPath: null,
      status: "conflicted",
      oldContent: "mod app;\n",
      newContent: "<<<<<<< HEAD\nmod app;\n=======\nmod commands;\n>>>>>>> feature\n",
      binary: false,
      tooLarge: false,
      pathLossy: false,
    },
  ],
  [WT_WHY_AGENT]: [
    {
      path: "src-tauri/src/git/testdata/porcelain_copy.txt",
      oldPath: "src-tauri/src/git/testdata/porcelain.txt",
      status: "copied",
      oldContent: "worktree /home/dev/code/why\n",
      newContent: "worktree /home/dev/code/why\n",
      binary: false,
      tooLarge: false,
      pathLossy: false,
    },
    {
      path: "docs/notes.md",
      oldPath: null,
      status: "untracked",
      oldContent: null,
      newContent: "# Worktree notes\n",
      binary: false,
      tooLarge: false,
      pathLossy: false,
    },
    {
      path: "src-tauri/Cargo.toml",
      oldPath: null,
      status: "modified",
      oldContent: '[package]\nname = "why"\n',
      newContent: '[package]\nname = "why"\ndescription = "Review agent changes"\n',
      binary: false,
      tooLarge: false,
      pathLossy: false,
    },
  ],
  [WT_BILLING_DETACHED]: [
    {
      path: "docs/caf�.txt",
      oldPath: null,
      status: "untracked",
      oldContent: null,
      newContent: "Billing notes\n",
      binary: false,
      tooLarge: false,
      pathLossy: true,
    },
  ],
} satisfies Record<string, FileDiff[]>;
