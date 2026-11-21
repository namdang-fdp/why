// @vitest-environment jsdom

import { createElement } from "react";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FixtureDataSource, type FixtureOptions } from "@/data/fixture-data-source";
import { storageTemporary } from "@/fixtures";
import { editorSettingsStore } from "@/stores/editor-settings";
import { createToastStore, toastStore } from "@/stores/toasts";
import { createWorkspaceStore } from "@/stores/workspace";
import { App } from "./App";

// The editor itself is covered elsewhere.
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
  editorSettingsStore.getState().reset();
  localStorage.clear();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(HTMLElement.prototype, "scrollIntoView");
});

function setup(options: FixtureOptions = {}) {
  const source = new FixtureDataSource(options);
  const store = createWorkspaceStore(source, toastStore);
  const user = userEvent.setup();
  render(createElement(App, { store }));
  return { source, store, user };
}

/** Radix opens a menu on Enter from its trigger (its pointer path needs real layout). */
async function openRemoveMenu(repo: string) {
  const trigger = screen.getByRole("button", { name: `Actions for ${repo}` });
  trigger.focus();
  await userEvent.keyboard("{Enter}");
  await userEvent.click(await screen.findByRole("menuitem", { name: /Remove from WHY/ }));
}

const dialog = () => screen.findByRole("dialog", { name: "Add repository" });

describe("first run", () => {
  it("invites to add a repository, then shows the repository once added", async () => {
    const { user, store } = setup({ empty: true });
    await screen.findByRole("heading", { name: "Review what your agents wrote" });
    expect(screen.queryByRole("navigation", { name: "Review navigation" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Add repository" }));
    const form = within(await dialog());
    await user.type(form.getByLabelText("Folder path"), "/home/dev/code/zeta");
    await user.click(form.getByRole("button", { name: "Add repository" }));

    await screen.findByRole("list", { name: "Worktrees in zeta" });
    expect(screen.queryByRole("heading", { name: "Review what your agents wrote" })).toBeNull();
    expect(store.getState().repoId).toMatch(/zeta/);
    // Announced in the live region.
    expect(
      within(screen.getByRole("region", { name: "Notifications" })).getByText("Added zeta"),
    ).toBeTruthy();
  });

  it("opens the add dialog from the a shortcut, and Esc closes it", async () => {
    const { user } = setup({ empty: true });
    await screen.findByRole("heading", { name: "Review what your agents wrote" });
    await user.keyboard("a");
    await dialog();
    // Typing an "a" (or any shortcut key) in the field is text, not a command.
    await user.type(screen.getByLabelText("Folder path"), "aj");
    expect((screen.getByLabelText("Folder path") as HTMLInputElement).value).toBe("aj");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});

describe("add repository", () => {
  it.each([
    ["/x/not-a-repo", /not inside a Git repository/],
    ["/x/no-git", /Git was not found/],
    ["/x/bare", /Bare repositories are not supported/],
    ["/x/missing", /No such file or directory/],
  ])("explains %s inline and keeps the dialog open", async (path, message) => {
    const { user } = setup();
    await screen.findByRole("list", { name: "Worktrees in why" });
    await user.click(screen.getByRole("button", { name: "Add repository" }));
    const form = within(await dialog());
    await user.type(form.getByLabelText("Folder path"), path);
    await user.keyboard("{Enter}");
    expect((await form.findByRole("alert")).textContent).toMatch(message);
    expect(form.getByLabelText("Folder path").getAttribute("aria-invalid")).toBe("true");
  });

  it("browse picks a folder and adds it at once; cancelling does nothing", async () => {
    const { user } = setup({ pick: "/home/dev/code/picked" });
    await screen.findByRole("list", { name: "Worktrees in why" });
    await user.click(screen.getByRole("button", { name: "Add repository" }));
    await user.click(within(await dialog()).getByRole("button", { name: "Browse…" }));
    await screen.findByRole("list", { name: "Worktrees in picked" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("does not add anything when the picker is cancelled", async () => {
    const { user, source } = setup({ pick: null });
    const add = vi.spyOn(source, "addRepository");
    await screen.findByRole("list", { name: "Worktrees in why" });
    await user.click(screen.getByRole("button", { name: "Add repository" }));
    await user.click(within(await dialog()).getByRole("button", { name: "Browse…" }));
    expect(add).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("warns inside the dialog when repositories will not be saved", async () => {
    const { user } = setup({ storage: storageTemporary });
    await screen.findByRole("list", { name: "Worktrees in why" });
    await user.click(screen.getByRole("button", { name: "Add repository" }));
    expect(within(await dialog()).getByRole("note").textContent).toMatch(/won't be saved/);
  });
});

describe("add repository: the path is passed through unchanged", () => {
  it("does not trim a directory name that ends with a space", async () => {
    const { user, source } = setup({ pick: "/home/dev/code/spacey " });
    const add = vi.spyOn(source, "addRepository");
    await screen.findByRole("list", { name: "Worktrees in why" });
    await user.click(screen.getByRole("button", { name: "Add repository" }));
    await user.click(within(await dialog()).getByRole("button", { name: "Browse…" }));
    await waitFor(() => expect(add).toHaveBeenCalledTimes(1));
    expect(add).toHaveBeenCalledWith("/home/dev/code/spacey ");
  });

  it("still refuses blank input", async () => {
    const { user, source } = setup();
    const add = vi.spyOn(source, "addRepository");
    await screen.findByRole("list", { name: "Worktrees in why" });
    await user.click(screen.getByRole("button", { name: "Add repository" }));
    const form = within(await dialog());
    await user.type(form.getByLabelText("Folder path"), "   ");
    expect(form.getByRole<HTMLButtonElement>("button", { name: "Add repository" }).disabled).toBe(
      true,
    );
    await user.keyboard("{Enter}");
    expect(add).not.toHaveBeenCalled();
  });
});

describe("add repository: a slow request and a re-opened dialog", () => {
  it("an earlier request finishing does not close or reset the new dialog", async () => {
    const { user, source } = setup();
    await screen.findByRole("list", { name: "Worktrees in why" });
    let finish: () => void = () => {};
    const real = source.addRepository.bind(source);
    vi.spyOn(source, "addRepository").mockImplementationOnce(
      (path) =>
        new Promise((resolve, reject) => {
          finish = () => real(path).then(resolve, reject);
        }),
    );

    await user.click(screen.getByRole("button", { name: "Add repository" }));
    await user.type(within(await dialog()).getByLabelText("Folder path"), "/home/dev/code/slow");
    await user.keyboard("{Enter}"); // request 1 starts and hangs
    await user.keyboard("{Escape}"); // the user gives up on that dialog
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await user.click(screen.getByRole("button", { name: "Add repository" }));
    const second = within(await dialog()).getByLabelText("Folder path");
    await user.type(second, "/home/dev/code/typed-later");

    finish(); // request 1 completes
    await screen.findByText("Added slow"); // it did register

    // The new dialog is untouched.
    expect(screen.getByRole("dialog", { name: "Add repository" })).toBeTruthy();
    expect((second as HTMLInputElement).value).toBe("/home/dev/code/typed-later");
  });
});

describe("storage banner", () => {
  it("is calm, persistent and gives the reason when storage is temporary", async () => {
    setup({ storage: storageTemporary });
    const banner = (await screen.findByText(/Repositories won't be saved/)).closest("p");
    expect(banner?.textContent).toContain(storageTemporary.reason);
    expect(banner?.closest("[role=status]")).toBeTruthy();
  });

  it("is absent when storage is persistent", async () => {
    setup();
    await screen.findByRole("list", { name: "Worktrees in why" });
    expect(screen.queryByText(/won't be saved/)).toBeNull();
  });
});

describe("worktrees that failed to load", () => {
  it("shows a warning with the reason, and the worktree stays selectable", async () => {
    const { user, store } = setup();
    const warning = await screen.findByRole("img", { name: /Could not be read/ });
    expect(warning.getAttribute("aria-label")).toContain("fatal: not a git repository");

    const row = screen.getByRole("button", { name: /WHY-77-refunds/ });
    await user.click(row);
    expect(store.getState().worktreeId).toBe("wt-billing-broken");
    // Where the changes would be: the error, with a retry.
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("fatal: not a git repository");
    expect(
      within(screen.getByRole("main", { name: "Diff" })).getByText("Select a file"),
    ).toBeTruthy();
    expect(store.getState().filePath).toBeNull();
    expect(store.getState().fileDiff.status).toBe("idle");
  });

  it("does not put error cards in the command palette", async () => {
    const { user } = setup();
    await screen.findByRole("img", { name: /Could not be read/ });
    await user.click(screen.getByRole("button", { name: "Search commands" }));
    await screen.findByRole("combobox");
    await screen.findByRole("option", { name: /WHY-77-refunds/ });
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("remove repository", () => {
  it("confirms, says nothing on disk changes, then removes it", async () => {
    const { user, store } = setup();
    await screen.findByRole("list", { name: "Worktrees in why" });
    await openRemoveMenu("why");

    const confirm = await screen.findByRole("alertdialog");
    expect(confirm.textContent).toContain("Remove why from WHY?");
    expect(confirm.textContent).toContain("not touched");
    await user.click(within(confirm).getByRole("button", { name: "Remove" }));

    await waitFor(() =>
      expect(screen.queryByRole("list", { name: "Worktrees in why" })).toBeNull(),
    );
    expect(store.getState().repoId).toBe("repo-billing-api");
    expect(
      within(screen.getByRole("region", { name: "Notifications" })).getByText("Removed why"),
    ).toBeTruthy();
  });

  it.each([
    [
      "Cancel",
      (user: ReturnType<typeof userEvent.setup>) =>
        user.click(screen.getByRole("button", { name: "Cancel" })),
    ],
    ["Escape", (user: ReturnType<typeof userEvent.setup>) => user.keyboard("{Escape}")],
  ])("%s returns focus to the repository's Actions button", async (_name, dismiss) => {
    const { user } = setup();
    await screen.findByRole("list", { name: "Worktrees in why" });
    await openRemoveMenu("why");
    await screen.findByRole("alertdialog");
    await dismiss(user);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole("button", { name: "Actions for why" })),
    );
  });

  it("cancel keeps the repository", async () => {
    const { user, source } = setup();
    const remove = vi.spyOn(source, "removeRepository");
    await screen.findByRole("list", { name: "Worktrees in why" });
    await openRemoveMenu("why");
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }),
    );
    expect(remove).not.toHaveBeenCalled();
    expect(screen.getByRole("list", { name: "Worktrees in why" })).toBeTruthy();
  });
});

describe("base override", () => {
  it("keeps worktree and branch names out of the top bar while supporting base overrides", async () => {
    const { user } = setup();
    const topBar = within(await screen.findByRole("banner"));
    const base = await topBar.findByRole("button", { name: "Change the base branch" });
    expect(topBar.queryByText("WHY-4-git")).toBeNull();
    expect(topBar.queryByText("agent/WHY-4-git-cli")).toBeNull();
    expect(topBar.queryByText("origin/main")).toBeNull();
    await user.click(base);
    const input = await screen.findByLabelText("Compare against");

    await user.clear(input);
    await user.type(input, "nope{Enter}");
    expect((await screen.findByRole("alert")).textContent).toContain("`nope` is not a commit");

    await user.click(await screen.findByRole("button", { name: "origin/main" }));
    await waitFor(() => expect(screen.queryByLabelText("Compare against")).toBeNull());
    expect(
      within(screen.getByRole("region", { name: "Notifications" })).getByText(
        "Base set to origin/main",
      ),
    ).toBeTruthy();
  });

  it("offers to set a base when the worktree has none", async () => {
    const { user } = setup();
    await user.click(await screen.findByRole("button", { name: /^main\b/i }));
    const topBar = within(screen.getByRole("banner"));
    const base = await topBar.findByRole("button", { name: "Set base" });
    await user.click(base);
    await user.type(await screen.findByLabelText("Compare against"), "origin/main{Enter}");
    await waitFor(() => expect(screen.queryByLabelText("Compare against")).toBeNull());
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Vs base" }).disabled).toBe(false);
  });
});

describe("toasts", () => {
  it("dismisses on request and expires by itself", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const store = createToastStore();
      const first = store.getState().push({ tone: "info", title: "One" });
      store.getState().push({ tone: "error", title: "Two" });
      store.getState().dismiss(first);
      expect(store.getState().toasts.map((t) => t.title)).toEqual(["Two"]);
      await vi.advanceTimersByTimeAsync(4001);
      expect(store.getState().toasts).toHaveLength(1); // errors stay longer
      await vi.advanceTimersByTimeAsync(4001);
      expect(store.getState().toasts).toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("caps the pile at four", () => {
    const store = createToastStore();
    for (let i = 0; i < 7; i++) store.getState().push({ tone: "info", title: `t${i}` });
    expect(store.getState().toasts.map((t) => t.title)).toEqual(["t3", "t4", "t5", "t6"]);
    vi.useRealTimers();
  });
});

describe("editor settings", () => {
  it("changes font, theme, ligatures and whitespace, and remembers them", async () => {
    const { user } = setup();
    await screen.findByRole("list", { name: "Worktrees in why" });
    await user.click(screen.getByRole("button", { name: "Editor settings" }));

    const size = await screen.findByLabelText("Size");
    await user.clear(size);
    await user.type(size, "15");
    const lineHeight = screen.getByLabelText("Line height");
    await user.clear(lineHeight);
    await user.type(lineHeight, "24");
    await user.selectOptions(screen.getByLabelText("Theme"), "tokyo-night-storm");
    await user.selectOptions(screen.getByLabelText("Show whitespace"), "none");
    await user.click(screen.getByRole("checkbox", { name: "Font ligatures" }));

    expect(editorSettingsStore.getState().settings).toMatchObject({
      fontSize: 15,
      lineHeight: 24,
      theme: "tokyo-night-storm",
      whitespace: "none",
      ligatures: false,
    });
    expect(JSON.parse(localStorage.getItem("why:editor-settings:v1") ?? "{}")).toMatchObject({
      fontSize: 15,
      lineHeight: 24,
    });

    await user.click(screen.getByRole("button", { name: "Reset to defaults" }));
    expect(editorSettingsStore.getState().settings).toMatchObject({ fontSize: 14, lineHeight: 22 });
  });

  it("applies the font family when the field is left, not on every keystroke", async () => {
    const { user } = setup();
    await screen.findByRole("list", { name: "Worktrees in why" });
    await user.click(screen.getByRole("button", { name: "Editor settings" }));
    const family = await screen.findByLabelText("Font");
    await user.clear(family);
    await user.type(family, "Fira Code");
    expect(editorSettingsStore.getState().settings.fontFamily).toContain("CaskaydiaCove");
    await user.tab();
    expect(editorSettingsStore.getState().settings.fontFamily).toBe("Fira Code");
  });

  it("is reachable from the command palette, which can also tune the editor", async () => {
    const { user } = setup();
    await screen.findByRole("list", { name: "Worktrees in why" });
    await user.keyboard("{Control>}k{/Control}");
    await user.type(await screen.findByRole("combobox"), "increase font");
    await user.keyboard("{Enter}");
    expect(editorSettingsStore.getState().settings.fontSize).toBe(15);

    await user.keyboard("{Control>}k{/Control}");
    await user.type(await screen.findByRole("combobox"), "Editor settings");
    await user.keyboard("{Enter}");
    expect(await screen.findByLabelText("Line height")).toBeTruthy();
  });
});
