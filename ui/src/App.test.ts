// @vitest-environment jsdom

import { createElement, useEffect } from "react";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { FileDiff, IpcError } from "@/contracts";
import type { HunkDirection, HunkResult } from "@/components/diff/hunk-navigation";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { ipcErrors, WT_WHY_AGENT } from "@/fixtures";
import { shortcutGroups } from "@/hooks/use-workspace-shortcuts";
import { createWorkspaceStore } from "@/stores/workspace";
import { useWorkspace } from "@/stores/workspace-context";
import { App } from "./App";

const moveHunk = vi.hoisted(() => vi.fn<(direction: HunkDirection) => HunkResult>(() => "moved"));

vi.mock("@/components/diff/monaco-diff", () => ({
  default: function MonacoDiff({
    diff,
    onReveal,
  }: {
    diff: FileDiff | null;
    onReveal?: (diff: FileDiff | null) => void;
  }) {
    const register = useWorkspace((state) => state.registerDiffNavigation);
    useEffect(() => {
      // The real editor reports a file once it has been computed and swapped in.
      onReveal?.(diff);
      if (diff) return register(diff, moveHunk);
    }, [diff, onReveal, register]);
    return createElement(
      "div",
      { "data-diff-editor": "", hidden: !diff },
      createElement("textarea", { "aria-label": "Diff editor", readOnly: true }),
      createElement("pre", { "aria-label": "Original" }, diff?.oldContent ?? ""),
      createElement("pre", { "aria-label": "Modified" }, diff?.newContent ?? ""),
    );
  },
}));

// jsdom does not lay out panels. Real resizing is checked in Chromium.
beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {
        /* no layout in jsdom */
      }
      unobserve() {
        /* no layout in jsdom */
      }
      disconnect() {
        /* no layout in jsdom */
      }
    },
  );
  vi.stubGlobal("matchMedia", () => ({ matches: false }));
  HTMLElement.prototype.scrollIntoView = vi.fn();
});
afterEach(() => {
  cleanup();
  moveHunk.mockReset();
  localStorage.clear();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(HTMLElement.prototype, "scrollIntoView");
});

describe("review shell", () => {
  it("keeps a screen-reader heading and puts Search first in the top bar's tab order", async () => {
    const user = userEvent.setup();
    render(createElement(App, { store: createWorkspaceStore(new FixtureDataSource()) }));
    const topBar = within(screen.getByRole("banner"));
    expect(
      topBar.getByRole("heading", { level: 1, name: "WHY" }).classList.contains("sr-only"),
    ).toBe(true);

    const search = topBar.getByRole("button", { name: "Search commands" });
    expect(search.id).toBe("shell-command");
    await user.tab();
    expect(document.activeElement).toBe(search);
    await user.keyboard("{Control>}k{/Control}");
    await screen.findByRole("dialog", { name: "Command palette" });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(document.activeElement).toBe(search));
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
  });

  it("shows each shortcut group once in the named dialog and closes on Escape", async () => {
    const user = userEvent.setup();
    render(createElement(App, { store: createWorkspaceStore(new FixtureDataSource()) }));
    await user.keyboard("?");

    const dialog = within(await screen.findByRole("dialog", { name: "Keyboard shortcuts" }));
    expect(
      dialog.getAllByRole("heading", { level: 3 }).map((heading) => heading.textContent),
    ).toEqual(shortcutGroups.map(({ title }) => title));
    await user.keyboard("{Escape}");
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Keyboard shortcuts" })).toBeNull(),
    );
  });

  it("opens all repositories by default and remembers independent disclosure states", async () => {
    const user = userEvent.setup();
    const renderShell = () =>
      render(createElement(App, { store: createWorkspaceStore(new FixtureDataSource()) }));
    const first = renderShell();
    await screen.findByRole("list", { name: "Worktrees in why" });
    await screen.findByRole("list", { name: "Worktrees in billing-api" });
    await user.click(screen.getByRole("button", { name: /detached @/ }));
    expect(screen.getByRole("list", { name: "Worktrees in why" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "why" }));
    // The section animates closed, then unmounts.
    await waitFor(() =>
      expect(screen.queryByRole("list", { name: "Worktrees in why" })).toBeNull(),
    );
    expect(screen.getByRole("list", { name: "Worktrees in billing-api" })).toBeTruthy();

    first.unmount();
    renderShell();
    await screen.findByRole("list", { name: "Worktrees in billing-api" });
    expect(screen.getByRole("button", { name: "why" }).getAttribute("aria-expanded")).toBe("false");
    await user.click(screen.getByRole("button", { name: "why" }));
    await screen.findByRole("list", { name: "Worktrees in why" });
  });

  it("filters the palette by contiguous text and path segments", async () => {
    const user = userEvent.setup();
    render(createElement(App, { store: createWorkspaceStore(new FixtureDataSource()) }));
    await user.click(screen.getByRole("button", { name: "Search commands" }));
    const search = await screen.findByRole("combobox");
    await user.type(search, "notes");
    await screen.findByRole("option", { name: /docs\/notes\.md/ });
    expect(screen.getAllByRole("option")).toHaveLength(1);

    await user.clear(search);
    await user.type(search, "git parser.rs");
    await screen.findByRole("option", { name: /parser\.rs.*parse\.rs/ });
    expect(screen.getAllByRole("option")).toHaveLength(1);
  });

  it("ignores shortcuts when typing in an input or using the command palette", async () => {
    const store = createWorkspaceStore(new FixtureDataSource());
    const user = userEvent.setup();
    render(createElement(App, { store }));
    await screen.findByRole("button", { name: /mod\.rs/ });

    const input = document.createElement("input");
    document.body.append(input);
    try {
      input.focus();
      await user.keyboard("ji3?[[");
      await user.keyboard("{Control>}k{/Control}");
      expect(input.value).toBe("ji3?[");
      expect(store.getState()).toMatchObject({
        filePath: null,
        inspectorOpen: true,
        preferredMode: "vsBase",
        paletteOpen: false,
      });
    } finally {
      input.remove();
    }

    await user.click(screen.getByRole("button", { name: "Search commands" }));
    const search = await screen.findByRole("combobox");
    expect(document.activeElement).toBe(search);
    await user.keyboard("ji3?");
    expect(store.getState()).toMatchObject({
      filePath: null,
      inspectorOpen: true,
      preferredMode: "vsBase",
      paletteOpen: true,
    });

    await user.keyboard("{Escape}");
    await screen.findByRole("button", { name: "Search commands" });
    await user.keyboard("j");
    expect(store.getState().filePath).toBe("src-tauri/src/git/mod.rs");
  });

  it("disables Vs base when a worktree has no base", async () => {
    const user = userEvent.setup();
    render(createElement(App, { store: createWorkspaceStore(new FixtureDataSource()) }));
    await user.click(await screen.findByRole("button", { name: /^main\b/i }));
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Vs base" }).disabled).toBe(true);
    expect(screen.getByRole("button", { name: "Uncommitted" }).getAttribute("aria-pressed")).toBe(
      "true",
    );
  });

  it("jumps to a file in another repository even if the active mode excludes it", async () => {
    const store = createWorkspaceStore(new FixtureDataSource());
    const user = userEvent.setup();
    render(createElement(App, { store }));
    await user.click(await screen.findByRole("button", { name: "Staged" }));
    await user.keyboard("{Control>}k{/Control}");
    await user.type(await screen.findByRole("combobox"), "seed.json");
    await screen.findByRole("option", { name: /seed\.json/ });
    await user.keyboard("{Enter}");

    await screen.findByRole("heading", { name: "Too large to preview (> 1 MiB)" });
    expect(store.getState()).toMatchObject({
      worktreeId: "wt-billing-detached",
      filePath: "data/seed.json",
      paletteOpen: false,
      preferredMode: "vsBase",
    });
  });

  const humanMessages = {
    notARepo: "This location is not a Git repository.",
    gitMissing: "Git is unavailable. Check that Git is installed and on your PATH.",
    gitFailed: "Git could not complete the request.",
    timeout: "The request took too long.",
    notFound: "The requested repository, worktree, or file could not be found.",
    invalidInput: "This request is unavailable for the current selection.",
    untrustedFilters:
      "This repository sets up git filter programs. WHY has not read its files and has not run them. Review them first.",
    internal: "WHY could not complete the request because of an internal error.",
  } satisfies Record<IpcError["kind"], string>;

  const titles = {
    notARepo: "Not a Git repository",
    gitMissing: "Git isn't installed",
    gitFailed: "Git couldn't read this",
    timeout: "Git took too long",
    notFound: "Not found",
    invalidInput: "That isn't available",
    untrustedFilters: "Blocked until you review it",
    internal: "Something went wrong inside WHY",
  } satisfies Record<IpcError["kind"], string>;

  it.each(ipcErrors)("renders the $kind IPC error", async (error) => {
    const source = new FixtureDataSource();
    vi.spyOn(source, "listRepositories").mockRejectedValue(error);
    render(createElement(App, { store: createWorkspaceStore(source) }));
    const alert = await screen.findByRole("alert");
    // A human headline first; the technical kind stays as secondary detail.
    expect(within(alert).getByText(titles[error.kind])).toBeTruthy();
    expect(alert.textContent).toContain(error.kind);
    expect(alert.textContent).toContain(humanMessages[error.kind]);
    if ("message" in error) expect(alert.textContent).toContain(error.message);
  });
});

describe("review diff", () => {
  function setup(source = new FixtureDataSource()) {
    const store = createWorkspaceStore(source);
    const user = userEvent.setup();
    const view = render(createElement(App, { store }));
    const rail = () => within(screen.getByRole("list", { name: "Changed files" }));
    const main = () => within(screen.getByRole("main", { name: "Diff" }));
    const select = async (name: RegExp) => {
      const list = await screen.findByRole("list", { name: "Changed files" });
      await user.click(await within(list).findByRole("button", { name }));
    };
    return { store, user, view, rail, main, select };
  }

  it("keeps the editor mounted and passes added, deleted and renamed content through", async () => {
    const { select, main } = setup();
    await select(/worktree\.rs/);
    const editor = await main().findByRole("textbox", { name: "Diff editor" });
    expect(main().getByLabelText("Original").textContent).toBe("");
    expect(main().getByLabelText("Modified").textContent).toContain("pub struct RawWorktree");

    await select(/icon\.png/);
    expect(await main().findByRole("heading", { name: "Binary file" })).toBeTruthy();
    expect(main().getAllByText("ui/public/icon.png").length).toBeGreaterThan(0);
    expect(main().queryByRole("textbox")).toBeNull();

    await select(/legacy\.rs/);
    expect(await main().findByRole("textbox")).toBe(editor);
    expect(main().getByLabelText("Original").textContent).toContain("pub fn old_helper");
    expect(main().getByLabelText("Modified").textContent).toBe("");

    await select(/parser\.rs.*parse\.rs/);
    expect(
      await main().findByText("src-tauri/src/git/parser.rs → src-tauri/src/git/parse.rs"),
    ).toBeTruthy();
    expect(main().getByLabelText("Original").textContent).toContain("pub fn parse");
    expect(main().getByLabelText("Modified").textContent).toContain("/// Returns the path");

    await select(/porcelain_copy\.txt/);
    expect(await main().findByRole("heading", { name: "No content changes" })).toBeTruthy();
  });

  it("keeps the previous diff on screen during a quick switch and shows the skeleton only for a slow one", async () => {
    const source = new FixtureDataSource();
    let release: (diff: FileDiff) => void = () => {};
    const real = source.getFileDiff.bind(source);
    // One file (not a neighbour, so not prefetched) answers slowly.
    vi.spyOn(source, "getFileDiff").mockImplementation((worktree, path, mode) =>
      path === "src-tauri/src/git/legacy.rs"
        ? new Promise<FileDiff>((resolve) => {
            release = resolve;
          })
        : real(worktree, path, mode),
    );
    const { select, main } = setup(source);
    await select(/mod\.rs/);
    await waitFor(() => expect(main().getByLabelText("Modified").textContent).toContain("pub mod"));
    const old = main().getByLabelText("Modified").textContent;

    await select(/legacy\.rs/);
    // Right after the click: the OLD diff is still there, and no skeleton yet.
    expect(main().getByLabelText("Modified").textContent).toBe(old);
    expect(main().queryByRole("status")).toBeNull();

    // A genuinely slow load gets the skeleton (after ~200 ms).
    expect(await main().findByRole("status", {}, { timeout: 1500 })).toHaveProperty(
      "textContent",
      "Loading diff…",
    );

    release(await real("wt-why-agent-git", "src-tauri/src/git/legacy.rs", "vsBase"));
    await waitFor(() =>
      expect(main().getByLabelText("Original").textContent).toContain("pub fn old_helper"),
    );
    expect(main().queryByRole("status")).toBeNull();
  });

  it("shows the lossy-path notice alongside the text preview", async () => {
    const { select, user, main } = setup();
    await user.click(await screen.findByRole("button", { name: /detached @/ }));
    await select(/Lossy path/);
    expect(await main().findByText("The file path could not be decoded exactly.")).toBeTruthy();
    expect(await main().findByRole("textbox")).toBeTruthy();
  });

  it("shows loading and retries the failed diff request for the same file", async () => {
    const source = new FixtureDataSource();
    let fail = () => {};
    const delayed = new Promise<FileDiff>((_, reject) => {
      fail = () => reject({ kind: "timeout" } satisfies IpcError);
    });
    const read = vi.spyOn(source, "getFileDiff").mockReturnValueOnce(delayed);
    const { select, main, user } = setup(source);
    await select(/mod\.rs/);
    expect(await main().findByRole("status")).toHaveProperty("textContent", "Loading diff…");
    fail();
    expect(await main().findByRole("alert")).toHaveProperty(
      "textContent",
      expect.stringContaining("The request took too long."),
    );
    await user.click(main().getByRole("button", { name: "Try again" }));
    await main().findByRole("textbox");
    // Asked for twice (the failure, then the retry); neighbours are prefetched too.
    expect(read.mock.calls.filter(([, path]) => path === "src-tauri/src/git/mod.rs")).toEqual([
      [WT_WHY_AGENT, "src-tauri/src/git/mod.rs", "vsBase"],
      [WT_WHY_AGENT, "src-tauri/src/git/mod.rs", "vsBase"],
    ]);
  });

  it("navigates hunks, advances files at the end and returns editor focus to the rail", async () => {
    const { user, store, select, main, rail } = setup();
    await select(/mod\.rs/);
    await main().findByRole("textbox");
    await user.keyboard("np");
    expect(moveHunk.mock.calls).toEqual([["next"], ["previous"]]);
    moveHunk.mockReturnValueOnce("boundary");
    await user.keyboard("n");
    await waitFor(() => expect(store.getState().filePath).toBe("src-tauri/src/git/worktree.rs"));
    expect(store.getState().filePath).toBe("src-tauri/src/git/worktree.rs");

    const editor = await main().findByRole("textbox");
    editor.focus();
    expect(document.activeElement).toBe(editor);
    await user.keyboard("npujk");
    expect(moveHunk).toHaveBeenCalledTimes(3);
    expect(store.getState().diffLayout).toBe("split");
    expect(store.getState().filePath).toBe("src-tauri/src/git/worktree.rs");
    await user.keyboard("{Escape}");
    expect(document.activeElement).toBe(rail().getByRole("button", { name: /worktree\.rs/ }));
    await user.keyboard("k");
    expect(store.getState().filePath).toBe("src-tauri/src/git/mod.rs");
  });

  it("shares diff layout across u, the palette and the header, and restores it on remount", async () => {
    const { user, view, main } = setup();
    const layout = (name: string) =>
      main().getByRole("button", { name }).getAttribute("aria-pressed");
    expect(layout("Split")).toBe("true");
    await user.keyboard("u");
    expect(layout("Unified")).toBe("true");
    view.unmount();

    setup();
    expect(layout("Unified")).toBe("true");
    await user.click(screen.getByRole("button", { name: "Search commands" }));
    await user.type(await screen.findByRole("combobox"), "Toggle unified/split");
    await screen.findByRole("option", { name: /Toggle unified\/split/ });
    await user.keyboard("{Enter}");
    expect(layout("Split")).toBe("true");
    await user.click(main().getByRole("button", { name: "Unified" }));
    expect(layout("Unified")).toBe("true");
  });
});
