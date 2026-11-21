// @vitest-environment jsdom

import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import type { IpcError } from "@/contracts";
import { repositories, storagePersistent, worktrees } from "@/fixtures";
import { HookFixtures } from "@/fixtures/hooks";
import { TauriDataSource } from "./tauri-data-source";

type Call = { cmd: string; args: unknown };

/** Answer every command with `reply(cmd, args)` and record what was invoked. */
function ipc(reply: (cmd: string, args: unknown) => unknown = () => null) {
  const calls: Call[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    return reply(cmd, args);
  });
  return calls;
}

afterEach(() => clearMocks());

describe("TauriDataSource commands", () => {
  const source = new TauriDataSource();

  it("calls the evidence commands by their Rust names with camelCase arguments", async () => {
    const calls = ipc(() => null);
    await source.getWorktreeAttribution("w1", "staged");
    await source.getWorktreeAttribution("w1");
    await source.getFileActivity("w1", "src/a.rs");
    await source.getTurnRationale("s1", "t1");
    await source.getToolRationale("s1", "e1");
    await source.getShellOutcomes("s1");
    expect(calls).toEqual([
      { cmd: "get_worktree_attribution", args: { worktreeId: "w1", mode: "staged" } },
      { cmd: "get_worktree_attribution", args: { worktreeId: "w1" } },
      { cmd: "get_file_activity", args: { worktreeId: "w1", path: "src/a.rs" } },
      { cmd: "get_turn_rationale", args: { sessionId: "s1", turnId: "t1" } },
      { cmd: "get_tool_rationale", args: { sessionId: "s1", eventId: "e1" } },
      { cmd: "get_shell_outcomes", args: { sessionId: "s1" } },
    ]);
  });

  it("calls each command by its Rust name with camelCase arguments", async () => {
    const calls = ipc((cmd) => {
      if (cmd === "list_repositories") return repositories;
      if (cmd === "list_worktrees" || cmd === "get_changes") return [];
      if (cmd === "get_storage_status") return storagePersistent;
      if (cmd === "set_base") return worktrees[0];
      if (cmd === "add_repository") return repositories[0];
      return null;
    });

    await source.listRepositories();
    await source.listWorktrees("r1");
    await source.getChanges("w1", "vsBase");
    await source.getFileDiff("w1", "src/a.rs", "staged");
    await source.addRepository("/code/a");
    await source.removeRepository("r1");
    await source.setBase("w1", "origin/main");
    await source.pickRepositoryFolder();
    await source.getStorageStatus();
    await source.getRepositorySecurity("r1");
    await source.trustRepositoryFilters("r1", "fp");
    await source.revokeRepositoryFilters("r1");

    expect(calls).toEqual([
      { cmd: "list_repositories", args: {} },
      { cmd: "list_worktrees", args: { repoId: "r1" } },
      { cmd: "get_changes", args: { worktreeId: "w1", mode: "vsBase" } },
      { cmd: "get_file_diff", args: { worktreeId: "w1", path: "src/a.rs", mode: "staged" } },
      { cmd: "add_repository", args: { path: "/code/a" } },
      { cmd: "remove_repository", args: { id: "r1" } },
      { cmd: "set_base", args: { worktreeId: "w1", baseRef: "origin/main" } },
      { cmd: "pick_repository_folder", args: {} },
      { cmd: "get_storage_status", args: {} },
      { cmd: "get_repository_security", args: { repoId: "r1" } },
      { cmd: "trust_repository_filters", args: { repoId: "r1", fingerprint: "fp" } },
      { cmd: "revoke_repository_filters", args: { repoId: "r1" } },
    ]);
  });

  it("returns the command's value unchanged, including null from a cancelled picker", async () => {
    ipc((cmd) => (cmd === "list_repositories" ? repositories : null));
    await expect(source.listRepositories()).resolves.toEqual(repositories);
    await expect(source.pickRepositoryFolder()).resolves.toBeNull();
  });

  it.each([
    { kind: "notARepo" },
    { kind: "gitMissing" },
    { kind: "timeout" },
    { kind: "notFound" },
    { kind: "gitFailed", message: "fatal: bad revision" },
    { kind: "invalidInput", message: "bare repositories are not supported" },
    {
      kind: "untrustedFilters",
      drivers: [
        {
          name: "evil",
          clean: "./x",
          smudge: null,
          process: null,
          scope: "local",
          origin: ".git/config",
          needsTrust: true,
          trusted: false,
          referenced: true,
          fingerprint: "fp",
        },
      ],
    },
    { kind: "internal", message: "database is locked" },
  ] satisfies IpcError[])("rejects with the core's $kind error as an IpcError", async (error) => {
    ipc(() => {
      throw error; // what a Rust `Err(IpcError)` looks like in `invoke`
    });
    await expect(source.addRepository("/x")).rejects.toEqual(error);
  });

  it("turns anything that is not an IpcError into an internal error, not a git failure", async () => {
    ipc(() => {
      throw new Error("Command get_changes not allowed by ACL");
    });
    await expect(source.getChanges("w", "vsBase")).rejects.toEqual({
      kind: "internal",
      message: "Command get_changes not allowed by ACL",
    });

    ipc(() => {
      throw "plain string";
    });
    await expect(source.listRepositories()).rejects.toEqual({
      kind: "internal",
      message: "plain string",
    });

    ipc(() => {
      throw { kind: "somethingNew", message: "from a newer core" };
    });
    await expect(source.listRepositories()).rejects.toMatchObject({ kind: "internal" });
  });
});

describe("TauriDataSource session commands", () => {
  const source = new TauriDataSource();

  it("calls list_sessions and get_session_timeline with camelCase ids", async () => {
    const calls = ipc((cmd) => (cmd === "list_sessions" ? [] : null));
    await expect(source.listSessions("w1")).resolves.toEqual([]);
    await source.getSessionTimeline("codex:s1");
    expect(calls).toEqual([
      { cmd: "list_sessions", args: { worktreeId: "w1" } },
      { cmd: "get_session_timeline", args: { sessionId: "codex:s1" } },
    ]);
  });

  it("passes the core's notFound through for an unknown session", async () => {
    ipc(() => {
      throw { kind: "notFound" } satisfies IpcError;
    });
    await expect(source.getSessionTimeline("codex:gone")).rejects.toEqual({ kind: "notFound" });
  });
});

it("calls the five hook IPC commands and returns refusals and stale previews as values", async () => {
  const source = new TauriDataSource();
  const fixture = new HookFixtures({ stale: true });
  const ready = fixture.preview("claudeCode", "install");
  if (ready.result !== "ready") throw new Error("Expected a fixture preview");
  const { fingerprint } = ready.preview;
  const stale = fixture.apply("claudeCode", "install", fingerprint);
  const refused = { result: "refused", reason: "locked", message: "Config locked." };
  const report = fixture.status();
  const calls = ipc((cmd) => {
    if (cmd === "preview_install") return ready;
    if (cmd === "install") return stale;
    if (cmd === "hook_status") return report;
    return refused;
  });

  await expect(source.previewInstall("claudeCode")).resolves.toEqual(ready);
  await expect(source.install("claudeCode", fingerprint)).resolves.toEqual(stale);
  await expect(source.previewUninstall("codex")).resolves.toEqual(refused);
  await expect(source.uninstall("codex", "codex-fp")).resolves.toEqual(refused);
  await expect(source.hookStatus()).resolves.toEqual(report);
  expect(calls).toEqual([
    { cmd: "preview_install", args: { harness: "claudeCode" } },
    { cmd: "install", args: { harness: "claudeCode", fingerprint } },
    { cmd: "preview_uninstall", args: { harness: "codex" } },
    { cmd: "uninstall", args: { harness: "codex", fingerprint: "codex-fp" } },
    { cmd: "hook_status", args: {} },
  ]);
});
