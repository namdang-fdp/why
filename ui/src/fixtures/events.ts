import type { IpcError, WorktreeChanged } from "@/contracts";
import { WT_WHY_AGENT } from "./repos";

/** A `worktree-changed` event for two known files. */
export const worktreeChangedSome = {
  worktreeId: WT_WHY_AGENT,
  paths: ["src-tauri/src/git/mod.rs", "docs/notes.md"],
} satisfies WorktreeChanged;

/** Empty `paths` means "refetch everything". */
export const worktreeChangedAll = {
  worktreeId: WT_WHY_AGENT,
  paths: [],
} satisfies WorktreeChanged;

/** One sample of every `IpcError` kind, for building error states. */
export const ipcErrors = [
  { kind: "notARepo" },
  { kind: "gitMissing" },
  { kind: "gitFailed", message: "fatal: bad revision 'origin/nope'" },
  { kind: "timeout" },
  { kind: "notFound" },
  { kind: "invalidInput", message: "base ref must not be empty" },
  {
    kind: "untrustedFilters",
    drivers: [
      {
        name: "secrets-scan",
        clean: "./scripts/scan-secrets.sh %f",
        smudge: null,
        process: null,
        scope: "local",
        origin: ".git/config",
        needsTrust: true,
        trusted: false,
        referenced: true,
        fingerprint: "fp:secrets-scan",
      },
    ],
  },
  { kind: "internal", message: "internal storage error" },
] satisfies IpcError[];
