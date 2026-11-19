// @vitest-environment jsdom

import { createElement } from "react";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FixtureDataSource, type FixtureOptions } from "@/data/fixture-data-source";
import { toastStore } from "@/stores/toasts";
import { createWorkspaceStore } from "@/stores/workspace";
import { App } from "./App";

vi.mock("@/components/diff/monaco-diff", () => ({ default: () => null }));

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
  toastStore.setState({ toasts: [] });
  localStorage.clear();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(HTMLElement.prototype, "scrollIntoView");
});

function setup(options: FixtureOptions = { many: true }) {
  const source = new FixtureDataSource(options);
  const store = createWorkspaceStore(source, toastStore);
  const user = userEvent.setup();
  const view = render(createElement(App, { store }));
  return { source, store, user, view };
}

const crowd = () => screen.findByRole("list", { name: "Worktrees in ai-interview-practice" });
/** A worktree row (not the top bar's branch readout, which has the same text). */
const worktreeRow = async (name: RegExp) => within(await crowd()).findByRole("button", { name });
const rowNames = async () =>
  within(await crowd())
    .getAllByRole("button")
    .map((row) => row.textContent ?? "");

describe("worktree names", () => {
  it("titles rows by branch and shows the folder only when it helps", async () => {
    const { user } = setup();
    await crowd();
    await user.click(screen.getByRole("button", { name: "5 clean" })); // the detached ones are clean
    await screen.findByRole("list", { name: "Clean worktrees in ai-interview-practice" });
    const rows = await rowNames(); // includes the clean group's rows once it is open
    // Folders named after the repository are not repeated under each title.
    const login = rows.find((text) => text.startsWith("agent/login-flow"));
    expect(login).toBeTruthy();
    expect(login).not.toContain("ai-interview-practice");
    // A folder that differs from the branch is shown.
    expect(rows.find((text) => text.startsWith("fix/timer"))).toContain("fix-timer");
    // Two detached HEADs at one commit are told apart by their parent folder.
    const detached = rows.filter((text) => text.startsWith("detached @ 1a2b3c4"));
    expect(detached).toHaveLength(2);
    expect(detached.some((text) => text.includes("e5f6"))).toBe(true);
    expect(detached.some((text) => text.includes("0a1b"))).toBe(true);
  });
});

describe("order and clean grouping", () => {
  it("lists main first, then the most recently active, and folds clean ones under 'N clean'", async () => {
    const { user } = setup();
    const rows = await rowNames();
    expect(rows.slice(0, 4).map((text) => text.split(/\s/)[0])).toEqual([
      "main",
      "agent/login-flow", // 2 minutes ago
      "agent/scoring", // 18 minutes
      "fix/timer", // 40 minutes
    ]);
    // Clean, unselected worktrees are not in the list ...
    expect(rows.some((text) => text.startsWith("docs/pass"))).toBe(false);
    // ... but behind a disclosure that says how many.
    const toggle = screen.getByRole("button", { name: "5 clean" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    await user.click(toggle);
    const clean = await screen.findByRole("list", {
      name: "Clean worktrees in ai-interview-practice",
    });
    expect(within(clean).getAllByRole("button")).toHaveLength(5);
    expect(within(clean).getByRole("button", { name: /^docs\/pass/ })).toBeTruthy();
  });

  it("keeps a worktree with a load error visible, and the selected one", async () => {
    const { user } = setup();
    const rows = await rowNames();
    expect(rows.some((text) => text.startsWith("agent/broken"))).toBe(true);
    await user.click(screen.getByRole("button", { name: "5 clean" }));
    await user.click(await screen.findByRole("button", { name: /^docs\/pass/ }));
    // Selected: promoted out of the folded group.
    const list = await crowd();
    await waitFor(() =>
      expect(
        within(list)
          .getByRole("button", { name: /^docs\/pass/ })
          .getAttribute("aria-current"),
      ).toBe("true"),
    );
  });

  it("moves with [ and ] in the order the rail shows", async () => {
    const { user, store } = setup();
    await crowd();
    await user.click(await worktreeRow(/^agent\/login-flow/));
    await user.keyboard("]");
    await waitFor(() => expect(store.getState().worktreeId).toBe("wt-aip-3")); // agent/scoring
    await user.keyboard("[[");
    await waitFor(() => expect(store.getState().worktreeId).toBe("wt-aip-2"));
    await user.keyboard("[[");
    await waitFor(() => expect(store.getState().worktreeId).toBe("wt-aip-1")); // main
  });
});

describe("which worktree the files belong to", () => {
  it("names the selected worktree in the Changed files header", async () => {
    const { user } = setup();
    await user.click(await worktreeRow(/^agent\/scoring/));
    const section = screen.getByRole("region", { name: /Changed files/ });
    await waitFor(() => expect(within(section).getByText("agent/scoring")).toBeTruthy());
  });
});

describe("filter", () => {
  it("narrows repositories, worktrees and files by substring; / focuses it; Esc clears it", async () => {
    const { user } = setup();
    await crowd();
    await user.keyboard("/");
    const filter = screen.getByRole("textbox", { name: /Filter/ });
    expect(document.activeElement).toBe(filter);

    await user.keyboard("scoring");
    await waitFor(async () => expect(within(await crowd()).getAllByRole("button")).toHaveLength(1));
    // Other repositories have no match and disappear.
    expect(screen.queryByRole("button", { name: "billing-api" })).toBeNull();

    // Files are filtered too (by path), and nothing is hidden behind "N clean" while filtering.
    await user.keyboard("{Control>}a{/Control}{Backspace}docs");
    expect(await screen.findByRole("button", { name: /^docs\/pass/ })).toBeTruthy();

    await user.keyboard("{Control>}a{/Control}{Backspace}zzzz-no-such-thing");
    expect(await screen.findByText(/Nothing matches/)).toBeTruthy();

    await user.keyboard("{Escape}");
    expect((filter as HTMLInputElement).value).toBe("");
    await user.keyboard("{Escape}");
    expect(document.activeElement).not.toBe(filter);
  });

  it("filters the changed files list", async () => {
    const { user } = setup();
    await user.click(await worktreeRow(/^agent\/login-flow/));
    const files = await screen.findByRole("list", { name: "Changed files" });
    expect(within(files).getAllByRole("button")).toHaveLength(7);
    await user.keyboard("/");
    await user.keyboard("feature-3");
    await waitFor(() => expect(within(files).getAllByRole("button")).toHaveLength(1));
  });
});

describe("the two sections", () => {
  it("folds each section to its header, remembers it, and never folds both", async () => {
    const { user, view } = setup();
    await crowd();
    const repos = screen.getByRole("button", { name: "Repositories" });
    const files = screen.getByRole("button", { name: "Changed files" });
    expect([repos, files].map((b) => b.getAttribute("aria-expanded"))).toEqual(["true", "true"]);

    await user.click(files);
    expect(files.getAttribute("aria-expanded")).toBe("false");
    expect(JSON.parse(localStorage.getItem("why:rail-collapsed:v1") ?? "{}")).toEqual({
      repos: false,
      files: true,
    });

    // Folding the other one opens this one instead of leaving two bare headers.
    await user.click(repos);
    expect(repos.getAttribute("aria-expanded")).toBe("false");
    expect(files.getAttribute("aria-expanded")).toBe("true");

    // The next launch restores the fold.
    view.unmount();
    setup();
    await crowd();
    expect(screen.getByRole("button", { name: "Repositories" }).getAttribute("aria-expanded")).toBe(
      "false",
    );
    expect(
      screen.getByRole("button", { name: "Changed files" }).getAttribute("aria-expanded"),
    ).toBe("true");
  });

  it("has a keyboard-reachable separator that double-click resets", async () => {
    const { user } = setup();
    await crowd();
    const handle = screen.getByRole("separator", { name: /Resize repositories and changed files/ });
    expect(handle.getAttribute("tabindex")).not.toBeNull();
    localStorage.setItem("why:rail-sections:v1", JSON.stringify({ repos: 20, files: 80 }));
    await user.dblClick(handle);
    expect(JSON.parse(localStorage.getItem("why:rail-sections:v1") ?? "{}")).toEqual({
      repos: 46,
      files: 54,
    });
  });
});

describe("live activity", () => {
  it("pulses the dot of a worktree the watcher just saw edited, and moves it to the top", async () => {
    const { source, store } = setup();
    await crowd();
    expect(document.querySelector(".wt-pulse")).toBeNull();

    act(() => source.simulateEdit("wt-aip-8", "src/modules/wt-aip-8/feature-1.ts")); // refactor/ui, 3 h ago
    await waitFor(() => expect(store.getState().pulses.has("wt-aip-8")).toBe(true));
    const row = screen.getByRole("button", { name: /^refactor\/ui/ });
    await waitFor(() => expect(row.querySelector(".wt-pulse")).not.toBeNull());

    // Now the most recently active: right after main.
    const rows = await rowNames();
    expect(rows[1]?.startsWith("refactor/ui")).toBe(true);
  });
});
