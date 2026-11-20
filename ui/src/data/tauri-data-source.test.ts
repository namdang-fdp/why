// @vitest-environment jsdom

import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import type { IpcError } from "@/contracts";
import { repositories, storagePersistent, worktrees } from "@/fixtures";
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
