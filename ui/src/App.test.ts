// @vitest-environment jsdom

import { createElement } from "react";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { IpcError } from "@/contracts";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { createWorkspaceStore } from "@/stores/workspace";
import { App } from "./App";

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
  localStorage.clear();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(HTMLElement.prototype, "scrollIntoView");
});

describe("review shell", () => {
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
    expect(screen.queryByRole("list", { name: "Worktrees in why" })).toBeNull();
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
    await user.click(await screen.findByRole("button", { name: /^why\s*main/i }));
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

    await screen.findByText("Too large to preview");
    expect(store.getState()).toMatchObject({
      worktreeId: "wt-billing-detached",
      filePath: "data/seed.json",
      paletteOpen: false,
      preferredMode: "vsBase",
    });
  });

  it("renders an IPC error kind and its message", async () => {
    class FailingSource extends FixtureDataSource {
      override async listRepositories(): ReturnType<FixtureDataSource["listRepositories"]> {
        throw {
          kind: "gitFailed",
          message: "fatal: bad revision 'origin/nope'",
        } satisfies IpcError;
      }
    }

    render(createElement(App, { store: createWorkspaceStore(new FailingSource()) }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("gitFailed");
    expect(alert.textContent).toContain("Git could not complete the request.");
    expect(alert.textContent).toContain("fatal: bad revision 'origin/nope'");
  });
});
