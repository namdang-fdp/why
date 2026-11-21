// @vitest-environment jsdom

import { createElement, StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ApplyResult,
  BinaryStatus,
  CollectorState,
  HookState,
  HookStatusReport,
  PreviewResult,
  RefusalReason,
} from "@/contracts";
import { FixtureDataSource } from "@/data/fixture-data-source";
import type { HookFixtureScenario } from "@/fixtures/hooks";
import { inspectorViewStore, resetInspectorView } from "@/stores/inspector-view";
import { createWorkspaceStore } from "@/stores/workspace";
import { deferred } from "@/test/deferred";
import { App } from "./App";

vi.mock("@/components/diff/monaco-diff", () => ({ default: () => null }));

beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  HTMLElement.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  resetInspectorView();
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(HTMLElement.prototype, "scrollIntoView");
});

const page = () => within(screen.getByRole("main", { name: "Agent hooks" }));
const card = (name: "Claude Code" | "Codex") => within(page().getByRole("region", { name }));
const gear = () => screen.getByRole("button", { name: "Settings" });
const title = () => page().getByRole("heading", { name: "Agent hooks", level: 2 });

async function openHooks(user: ReturnType<typeof userEvent.setup>) {
  await user.keyboard("{Control>},{/Control}");
  await user.click(screen.getByRole("button", { name: "Agent hooks" }));
}
const button = (name: string) => page().getByRole("button", { name });
const diff = () => page().getByLabelText("Unified config diff");

function setup(source: FixtureDataSource, strict = false) {
  // The inspector opens on its File tab; most of these tests look at the session list.
  inspectorViewStore.getState().setTab("worktree");
  const user = userEvent.setup();
  const app = createElement(App, { store: createWorkspaceStore(source) });
  render(strict ? createElement(StrictMode, null, app) : app);
  return user;
}

async function open(
  scenario?: HookFixtureScenario,
  source = new FixtureDataSource({ hooks: scenario, empty: true }),
) {
  const user = setup(source);
  await openHooks(user);
  await page().findByRole("heading", { name: "Claude Code" });
  return { user, source };
}

async function preview(
  user: ReturnType<typeof userEvent.setup>,
  action = "Preview install",
  harness: "Claude Code" | "Codex" = "Claude Code",
) {
  await user.click(card(harness).getByRole("button", { name: action }));
  await page().findByLabelText("Unified config diff");
}

const hookCases = [
  ["notInstalled", "Not installed", "Preview the changes that would add"],
  ["installed", "Installed", "hook entries match this build"],
  ["outdated", "Outdated", "hooks need an update"],
  ["needsReview", "Review in Codex", "approve WHY's hooks in `/hooks`"],
  ["disabledByUserSetting", "Disabled in settings", "Set `disableAllHooks` to false"],
  ["disabledByPolicy", "Blocked by policy", "Contact your administrator"],
  ["configInvalid", "Config needs attention", "The hooks field must be a JSON object."],
] satisfies [HookState["kind"], string, string][];

const refusalCases = [
  ["configInvalid", "Invalid config"],
  ["disabledByPolicy", "Managed policy blocks this install"],
  ["unsafeTarget", "Unsafe config path"],
  ["readOnly", "Read-only config"],
  ["locked", "Config is locked"],
  ["hookBinaryMissing", "Hook binary unavailable"],
  ["noHome", "Config location unavailable"],
  ["tooLarge", "Config file is too large"],
] satisfies [RefusalReason, string][];

const collectorCases = [
  ["starting", "Starting", "The collector is starting"],
  ["listening", "Listening", "Ready to receive hook events."],
  ["disabled", "Disabled", "The fixture socket could not be bound."],
  ["notRunning", "Not running", "No collector is running in this WHY process."],
] satisfies [CollectorState["kind"], string, string][];

const binaryCases = [
  ["current", "Current", "Matches the hook binary bundled with this build."],
  ["missing", "Missing", "This build has no bundled hook binary to install."],
  ["outdated", "Outdated", "Preview an install or update to reinstall"],
  ["unverified", "Unverified", "This build has no bundled copy to compare it with."],
] satisfies [BinaryStatus, string, string][];

describe("Agent hooks status", () => {
  it.each(hookCases)("renders %s with its explanation", async (state, label, explanation) => {
    const harness = state === "needsReview" ? "codex" : "claudeCode";
    await open({ states: { [harness]: state } });
    const section = card(harness === "codex" ? "Codex" : "Claude Code");
    expect(section.getByText(label).dataset.kind).toBe(state);
    expect(section.getByText(explanation, { exact: false })).toBeTruthy();
    expect(
      section.getByText(
        harness === "codex" ? "/home/you/.codex/hooks.json" : "/home/you/.claude/settings.json",
      ),
    ).toBeTruthy();
  });

  it.each(collectorCases)("renders collector %s", async (collector, label, explanation) => {
    await open({ collector });
    const health = within(page().getByRole("region", { name: "Collector and binary" }));
    expect(health.getByText(label)).toBeTruthy();
    expect(health.getByText(explanation, { exact: false })).toBeTruthy();
  });

  it.each(binaryCases)("renders binary %s", async (binary, label, explanation) => {
    await open({ binary });
    const health = within(page().getByRole("region", { name: "Collector and binary" }));
    expect(health.getByText(label)).toBeTruthy();
    expect(health.getByText(explanation, { exact: false })).toBeTruthy();
  });

  it("prints unknown for unavailable facts, preserves a real zero, and copies the reported path", async () => {
    const { user } = await open({ instance: "dev:1550" });
    const health = within(page().getByRole("region", { name: "Collector and binary" }));
    expect(health.getByText("dev:1550")).toBeTruthy();
    expect(health.getByText("Last event").parentElement?.textContent).toBe("Last eventunknown");
    expect(health.getByText("Spool files waiting").parentElement?.textContent).toBe(
      "Spool files waiting0",
    );
    await user.click(health.getByRole("button", { name: "Copy socket path" }));
    expect(await navigator.clipboard.readText()).toBe(
      "/home/you/.local/share/why-dev/run/collector-dev-1550.sock",
    );
    const summary = card("Claude Code").getByText("Events WHY registers (10)");
    expect(summary.closest("details")?.open).toBe(false);
    await user.click(summary);
    expect(summary.closest("details")?.open).toBe(true);
    expect(card("Claude Code").getByText("PostToolBatch")).toBeTruthy();
  });

  it("fetches status on open and on manual refresh, and discards a closed visit's status", async () => {
    const source = new FixtureDataSource({ empty: true });
    const old = deferred<HookStatusReport>();
    const listened = await source.hookStatus();
    const status = vi.spyOn(source, "hookStatus").mockReturnValueOnce(old.promise);
    const user = setup(source);
    expect(status).not.toHaveBeenCalled();
    await openHooks(user);
    await page().findByText("Checking agent hooks…");
    await user.keyboard("{Escape}");
    await openHooks(user);
    await page().findByText("Listening");
    await act(async () => old.resolve({ ...listened, instance: "installed" }));
    expect(page().queryByText("installed", { selector: ".hook-instance code" })).toBeNull();
    source.setHookScenario({ collector: "disabled" });
    await user.click(button("Refresh status"));
    await page().findByText("The fixture socket could not be bound.");
    expect(status).toHaveBeenCalledTimes(3);
  });

  it("recovers the status request after StrictMode remount", async () => {
    const user = setup(new FixtureDataSource({ empty: true }), true);
    await openHooks(user);
    await page().findByText("Listening");
  });

  it("keeps keyboard focus on Refresh status and ignores repeats while refreshing", async () => {
    const { user, source } = await open();
    const pending = deferred<HookStatusReport>();
    const report = await source.hookStatus();
    const status = vi.spyOn(source, "hookStatus").mockReturnValueOnce(pending.promise);
    const refresh = button("Refresh status");
    await user.tab();
    expect(document.activeElement).toBe(refresh);
    await user.keyboard("{Enter}");
    expect(refresh.getAttribute("aria-disabled")).toBe("true");
    expect(refresh.getAttribute("aria-busy")).toBe("true");
    expect(document.activeElement).toBe(refresh);
    await user.keyboard("{Enter}");
    expect(status).toHaveBeenCalledTimes(1);
    await act(async () => pending.resolve(report));
    expect(refresh.getAttribute("aria-disabled")).toBe("false");
    expect(document.activeElement).toBe(refresh);
  });
});

describe("Agent hooks consent", () => {
  it.each(refusalCases)(
    "shows %s as a refusal value with its original message",
    async (reason, title) => {
      const { user, source } = await open({ refusal: { reason } });
      const response = await source.previewInstall("claudeCode");
      if (response.result !== "refused") throw new Error("Expected a fixture refusal");
      const install = vi.spyOn(source, "install");
      await user.click(card("Claude Code").getByRole("button", { name: "Preview install" }));
      expect(await page().findByText(title)).toBeTruthy();
      expect(page().getByText(response.message)).toBeTruthy();
      expect(page().getByText("No change was written.")).toBeTruthy();
      expect(page().queryByRole("alert")).toBeNull();
      expect(page().queryByRole("button", { name: /try again|confirm/i })).toBeNull();
      expect(install).not.toHaveBeenCalled();
    },
  );

  it("renders the exact diff and notes, and requires a second action with the new fingerprint after stale", async () => {
    const { user, source } = await open({ stale: true });
    const first = await source.previewInstall("claudeCode");
    if (first.result !== "ready") throw new Error("Expected a fixture preview");
    const install = vi.spyOn(source, "install");
    await preview(user);
    expect(diff().textContent).toBe(first.preview.diff);
    const notes = within(page().getByRole("list", { name: "Preview notes" }));
    expect(notes.getAllByRole("listitem").map((item) => item.textContent)).toEqual(
      first.preview.notes,
    );
    expect(notes.getByText(/keeps a private copy/)).toBeTruthy();
    expect(document.activeElement).toBe(
      page().getByRole("heading", { name: "Install Claude Code hooks" }),
    );
    await user.keyboard("{Enter}{Enter}");
    expect(install).not.toHaveBeenCalled();
    const firstDiff = diff();
    firstDiff.scrollTop = 2495;
    await user.click(button("Confirm install"));
    await page().findByText("The file changed since your preview.");
    const second = await source.previewInstall("claudeCode");
    if (second.result !== "ready") throw new Error("Expected a refreshed fixture preview");
    expect(second.preview.fingerprint).not.toBe(first.preview.fingerprint);
    expect(diff()).not.toBe(firstDiff);
    expect(diff().scrollTop).toBe(0);
    expect(diff().textContent).toBe(second.preview.diff);
    expect(install.mock.calls).toEqual([["claudeCode", first.preview.fingerprint]]);
    await user.keyboard("{Enter}");
    expect(install).toHaveBeenCalledTimes(1);
    await user.click(button("Confirm install"));
    await page().findByText("WHY’s hook entries were written.", { exact: false });
    expect(card("Claude Code").getByText("Installed")).toBeTruthy();
    expect(install.mock.calls).toEqual([
      ["claudeCode", first.preview.fingerprint],
      ["claudeCode", second.preview.fingerprint],
    ]);
  });

  it("displays the Codex note verbatim and returns the separate Codex review step after install", async () => {
    const { user, source } = await open();
    const response = await source.previewInstall("codex");
    if (response.result !== "ready") throw new Error("Expected a Codex preview");
    const { notes } = response.preview;
    await preview(user, "Preview install", "Codex");
    const previewNotes = page().getByRole("list", { name: "Preview notes" });
    expect(
      within(previewNotes)
        .getAllByRole("listitem")
        .map((item) => item.textContent),
    ).toEqual(notes);
    expect(within(previewNotes).getByText(/approve WHY's hooks in `\/hooks`/)).toBeTruthy();
    await user.click(button("Confirm install"));
    await card("Codex").findByText("Review in Codex");
    expect(card("Codex").getByText(/approve WHY's hooks in `\/hooks`/)).toBeTruthy();
    expect(card("Codex").queryByRole("alert")).toBeNull();
  });

  it.each(["click", "Enter"] as const)(
    "blocks duplicate %s while confirm is in flight",
    async (kind) => {
      const { user, source } = await open();
      const held = deferred<ApplyResult>();
      const install = vi.spyOn(source, "install").mockReturnValueOnce(held.promise);
      await preview(user);
      const confirm = button("Confirm install");
      if (kind === "click") {
        await user.dblClick(confirm);
      } else {
        confirm.focus();
        await user.keyboard("{Enter}{Enter}");
      }
      expect(install).toHaveBeenCalledTimes(1);
      expect(page().getByText("Applying the reviewed change…")).toBeTruthy();
      await act(async () =>
        held.resolve({
          result: "refused",
          reason: "locked",
          message: "Lock acquired by another process.",
        }),
      );
      expect(page().getByText("Lock acquired by another process.")).toBeTruthy();
      expect(page().queryByRole("alert")).toBeNull();
      expect(page().queryByRole("button", { name: /try again|confirm/i })).toBeNull();
      expect(install).toHaveBeenCalledTimes(1);
    },
  );

  it("discards a late preview after leaving and reopens with fresh status", async () => {
    const source = new FixtureDataSource({ empty: true });
    const original = await source.previewInstall("claudeCode");
    const held = deferred<PreviewResult>();
    const status = vi.spyOn(source, "hookStatus");
    const install = vi.spyOn(source, "install");
    const { user } = await open(undefined, source);
    vi.spyOn(source, "previewInstall").mockReturnValueOnce(held.promise);
    await user.click(card("Claude Code").getByRole("button", { name: "Preview install" }));
    await user.click(screen.getByRole("button", { name: "Editor" }));
    expect(screen.queryByRole("main", { name: "Agent hooks" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Agent hooks" }));
    await page().findByRole("heading", { name: "Codex" });
    await preview(user, "Preview install", "Codex");
    const codexDiff = diff().textContent;
    await act(async () => held.resolve(original));

    expect(page().getByRole("heading", { name: "Install Codex hooks" })).toBeTruthy();
    expect(diff().textContent).toBe(codexDiff);
    expect(status).toHaveBeenCalledTimes(2);
    expect(install).not.toHaveBeenCalled();
  });

  it("waits for a confirmed apply on return, rereads the written state and never resubmits", async () => {
    const source = new FixtureDataSource({ empty: true });
    const { user } = await open(undefined, source);
    const status = vi.spyOn(source, "hookStatus");
    const original = source.install.bind(source);
    const held = deferred<void>();
    const install = vi.spyOn(source, "install").mockImplementation(async (...args) => {
      await held.promise;
      return original(...args);
    });
    await preview(user);
    await user.click(button("Confirm install"));
    await user.click(screen.getByRole("button", { name: "Back to workspace" }));
    expect(screen.queryByRole("main", { name: "Agent hooks" })).toBeNull();

    await openHooks(user);
    expect(page().getByText("Checking agent hooks…")).toBeTruthy();
    expect(page().queryByRole("button", { name: /preview install|confirm install/i })).toBeNull();
    expect(status).not.toHaveBeenCalled();

    await act(async () => held.resolve());
    await card("Claude Code").findByText("Installed");
    expect(status).toHaveBeenCalledTimes(1);
    expect(install).toHaveBeenCalledTimes(1);
    expect(page().queryByText("WHY’s hook entries were written.", { exact: false })).toBeNull();
  });

  it.each([
    ["restoreOriginal", "Restore the original file exactly."],
    ["removeEntries", "Remove only WHY’s hook entries and keep your edits."],
    ["deleteFile", "Delete the unchanged config file created by WHY."],
  ] as const)(
    "previews %s and only uninstalls the reviewed fingerprint",
    async (uninstall, summary) => {
      const { user, source } = await open({ states: { claudeCode: "installed" }, uninstall });
      const expected = await source.previewUninstall("claudeCode");
      if (expected.result !== "ready") throw new Error("Expected an uninstall preview");
      expect(expected.preview.outcome).toBe(uninstall);
      const apply = vi.spyOn(source, "uninstall");
      await preview(user, "Preview uninstall");
      expect(page().getByText(summary)).toBeTruthy();
      expect(diff().textContent).toBe(expected.preview.diff);
      expect(apply).not.toHaveBeenCalled();
      await user.click(button("Confirm uninstall"));
      await card("Claude Code").findByText("Not installed");
      expect(apply.mock.calls).toEqual([["claudeCode", expected.preview.fingerprint]]);
    },
  );

  it("confirms a binary-only update with an empty config diff", async () => {
    const { user, source } = await open({
      states: { claudeCode: "installed" },
      binary: "outdated",
    });
    const install = vi.spyOn(source, "install");
    await user.click(card("Claude Code").getByRole("button", { name: "Preview update" }));
    expect(await page().findByText("No config diff to show.")).toBeTruthy();
    expect(page().getByText(/reinstall the bundled hook binary/)).toBeTruthy();
    expect(install).not.toHaveBeenCalled();
    await user.click(button("Confirm install"));
    expect(
      await page().findByText("Matches the hook binary bundled with this build."),
    ).toBeTruthy();
    expect(install).toHaveBeenCalledTimes(1);
  });

  it("offers retry on preview/apply I/O failures, and obtains fresh consent before retrying a write", async () => {
    const source = new FixtureDataSource({ empty: true });
    const read = vi
      .spyOn(source, "previewInstall")
      .mockRejectedValueOnce({ kind: "internal", message: "Could not read config" });
    const install = vi
      .spyOn(source, "install")
      .mockRejectedValueOnce({ kind: "internal", message: "Could not write config" });
    const { user } = await open(undefined, source);
    await user.click(card("Claude Code").getByRole("button", { name: "Preview install" }));
    expect((await page().findByRole("alert")).textContent).toContain("Could not read config");
    await user.click(button("Try again"));
    await page().findByLabelText("Unified config diff");
    await user.click(button("Confirm install"));
    expect((await page().findByRole("alert")).textContent).toContain("Could not write config");
    await user.click(button("Try again"));
    await page().findByLabelText("Unified config diff");
    expect(read).toHaveBeenCalledTimes(3);
    expect(install).toHaveBeenCalledTimes(1);
    await user.click(button("Confirm install"));
    await card("Claude Code").findByText("Installed");
    expect(install).toHaveBeenCalledTimes(2);
  });
});

it("opens from Settings, the palette and the session empty state, with focus restored on Escape", async () => {
  const source = new FixtureDataSource();
  vi.spyOn(source, "listSessions").mockResolvedValue([]);
  const install = vi.spyOn(source, "install");
  const user = setup(source);
  await screen.findByRole("button", { name: "Set up hooks" });

  gear().focus();
  await user.keyboard("{Enter}");
  await user.click(screen.getByRole("button", { name: "Agent hooks" }));
  expect(document.activeElement).toBe(title());
  await user.keyboard("{Escape}");
  await waitFor(() => expect(document.activeElement).toBe(gear()));

  const search = screen.getByRole("button", { name: "Search commands" });
  search.focus();
  await user.keyboard("{Enter}");
  await user.type(screen.getByRole("combobox"), "Set up agent hooks");
  await user.keyboard("{Enter}");
  await page().findByRole("heading", { name: "Claude Code" });
  await user.keyboard("{Escape}");
  await waitFor(() => expect(document.activeElement).toBe(search));

  (await screen.findByRole("button", { name: "Set up hooks" })).focus();
  await user.keyboard("{Enter}");
  await preview(user);
  await user.keyboard("{Escape}");
  await waitFor(() => expect(document.activeElement).toBe(gear()));
  expect(install).not.toHaveBeenCalled();

  fireEvent.keyDown(window, { key: "?" });
  expect(await screen.findByRole("dialog", { name: "Keyboard shortcuts" })).toBeTruthy();
});
