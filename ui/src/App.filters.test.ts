// @vitest-environment jsdom

import { createElement } from "react";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FixtureDataSource, sampleFilters, type FixtureOptions } from "@/data/fixture-data-source";
import { editorSettingsStore } from "@/stores/editor-settings";
import { toastStore } from "@/stores/toasts";
import { createWorkspaceStore } from "@/stores/workspace";
import { App } from "./App";

// WHY-39: a repository whose own git config defines filter programs is blocked
// until the user has seen the programs and trusted them.

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

function setup(options: FixtureOptions = { filters: sampleFilters }) {
  const source = new FixtureDataSource(options);
  const store = createWorkspaceStore(source, toastStore);
  const user = userEvent.setup();
  render(createElement(App, { store }));
  return { source, store, user };
}

const panel = () => screen.findByRole("region", { name: "Blocked until you review it" });

describe("a repository with untrusted git filter programs", () => {
  it("is blocked: the diff area explains it and lists the programs, with the shield on the row", async () => {
    const { source } = setup();
    const read = vi.spyOn(source, "getChanges");
    const diff = vi.spyOn(source, "getFileDiff");

    const region = within(await panel());
    // What would run, and where it came from: the user decides on facts.
    expect(region.getByText("secrets-scan")).toBeTruthy();
    expect(region.getByText("./scripts/scan-secrets.sh %f")).toBeTruthy();
    expect(region.getByText(/from \.git\/config \(this repository's config\)/)).toBeTruthy();
    expect(region.getByText(/Only trust it if you know where it came from/)).toBeTruthy();
    expect(region.getByRole("button", { name: "Trust and continue" })).toBeTruthy();
    expect(region.getByRole("button", { name: "Keep blocked" })).toBeTruthy();

    // The shield marks the repository in the rail, not just the pane.
    expect(screen.getByRole("img", { name: /^Blocked: defines git filter programs/ })).toBeTruthy();
    // The rail's file list does not offer a pointless retry.
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    expect(read).toHaveBeenCalledTimes(1); // the core was asked, and it refused
    expect(diff).not.toHaveBeenCalled();
  });

  it("trusting asks for confirmation, then reads the repository", async () => {
    const { user, source } = setup();
    const trust = vi.spyOn(source, "trustRepositoryFilters");
    await user.click(within(await panel()).getByRole("button", { name: "Trust and continue" }));

    // Nothing is trusted by the first click: a confirmation names what runs.
    const confirm = await screen.findByRole("alertdialog");
    expect(trust).not.toHaveBeenCalled();
    // The dialog itself shows the program in full, with where it came from.
    expect(within(confirm).getByText("./scripts/scan-secrets.sh %f")).toBeTruthy();
    expect(
      within(confirm).getByText(/from \.git\/config \(this repository's config\)/),
    ).toBeTruthy();
    await user.click(within(confirm).getByRole("button", { name: "Trust and continue" }));

    // The panel gives way to the changes, and a quiet notice remembers the decision.
    await screen.findByRole("list", { name: "Changed files" });
    expect(screen.queryByRole("region", { name: "Blocked until you review it" })).toBeNull();
    expect(trust).toHaveBeenCalledWith("repo-why", expect.stringContaining("secrets-scan"));
    expect(
      screen
        .getByText(/You trusted this repository's git filter programs \(secrets-scan\)/)
        .closest('[role="status"]'),
    ).toBeTruthy();
    expect(screen.getByRole("img", { name: /^Git filter programs trusted/ })).toBeTruthy();
  });

  it("cancelling the confirmation trusts nothing", async () => {
    const { user, source } = setup();
    const trust = vi.spyOn(source, "trustRepositoryFilters");
    await user.click(within(await panel()).getByRole("button", { name: "Trust and continue" }));
    const confirm = await screen.findByRole("alertdialog");
    await user.click(within(confirm).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(trust).not.toHaveBeenCalled();
    expect(screen.getByRole("region", { name: "Blocked until you review it" })).toBeTruthy();
  });

  it("keep blocked leaves it blocked, and it can be reviewed again", async () => {
    const { user, source } = setup();
    const trust = vi.spyOn(source, "trustRepositoryFilters");
    await user.click(within(await panel()).getByRole("button", { name: "Keep blocked" }));

    expect(await screen.findByRole("heading", { name: "Kept blocked" })).toBeTruthy();
    expect(screen.getByText(/nothing from it was run/)).toBeTruthy();
    expect(trust).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Review again" }));
    expect(await panel()).toBeTruthy();
  });

  it("trust can be revoked from the notice, and the repository is blocked again", async () => {
    const { user } = setup();
    await user.click(within(await panel()).getByRole("button", { name: "Trust and continue" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Trust and continue",
      }),
    );
    await screen.findByRole("list", { name: "Changed files" });

    await user.click(screen.getByRole("button", { name: "Stop trusting" }));
    await panel();
    expect(screen.getByRole("img", { name: /^Blocked: defines git filter programs/ })).toBeTruthy();
  });

  it("a configuration that changed after the user looked is not trusted blind", async () => {
    const { user, source, store } = setup();
    const region = within(await panel());
    // The core finds a different fingerprint than the one the panel displays.
    vi.spyOn(source, "trustRepositoryFilters").mockRejectedValue({
      kind: "invalidInput",
      message: "the repository's filter configuration changed; review it again",
    });
    await user.click(region.getByRole("button", { name: "Trust and continue" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Trust and continue",
      }),
    );

    await waitFor(() =>
      expect(
        within(screen.getByRole("region", { name: "Notifications" })).getByText(
          /Could not trust why's filter programs/,
        ),
      ).toBeTruthy(),
    );
    expect(store.getState().security.get("repo-why")?.status).toBe("ok");
    expect(screen.getByRole("region", { name: "Blocked until you review it" })).toBeTruthy();
  });
});

describe("a repository that only uses the user's own filter programs", () => {
  it("is not blocked; it shows an info shield and a quiet note", async () => {
    const { user } = setup();
    await screen.findByRole("list", { name: "Worktrees in why" });
    // `billing-api` configures git-lfs in the USER's config.
    const shield = await screen.findByRole("img", { name: /^Uses your own git filter programs/ });
    expect(shield.getAttribute("aria-label")).toContain("lfs");

    await user.click(screen.getByRole("button", { name: /^billing-api/ }));
    const detached = await screen.findByRole("button", { name: /detached/ });
    await user.click(detached);
    await screen.findByRole("list", { name: "Changed files" });
    expect(screen.queryByRole("region", { name: "Blocked until you review it" })).toBeNull();
    expect(
      screen
        .getByText(/uses git filter programs from your own git config \(lfs\)/)
        .closest('[role="status"]'),
    ).toBeTruthy();
  });
});

describe("a repository without filter programs", () => {
  it("shows no shield and no notice", async () => {
    setup({});
    await screen.findByRole("list", { name: "Worktrees in why" });
    expect(screen.queryByRole("img", { name: /filter programs/ })).toBeNull();
    expect(screen.queryByRole("region", { name: "Blocked until you review it" })).toBeNull();
    await screen.findByRole("list", { name: "Changed files" });
  });
});

/** Open the confirmation for the blocked repository and return it. */
async function openConfirmation(user: ReturnType<typeof userEvent.setup>) {
  await user.click(within(await panel()).getByRole("button", { name: "Trust and continue" }));
  return await screen.findByRole("alertdialog");
}

describe("the confirmation is bound to what the user reviewed", () => {
  it("shows every command exactly: double spaces kept, control characters spelled out", async () => {
    const { user } = setup({
      filters: {
        why: [
          {
            name: "tricky",
            clean: "sh -c 'a  b\nrm x'\u00a0",
            scope: "local",
            origin: ".git/config",
          },
        ],
      },
    });
    const confirm = within(await openConfirmation(user));
    const code = confirm.getByLabelText("Programs you are trusting").querySelector("code");
    if (!code) throw new Error("no command shown");
    // Both spaces survive, the newline is visible as `\n` (and still breaks the
    // line), and the no-break space at the end is spelled out, not invisible.
    expect(code.textContent).toBe("sh -c 'a  b\\n\nrm x'\\xa0");
    expect(code.querySelectorAll(".filter-escape")).toHaveLength(2);
  });

  it("trusts the fingerprint that was shown, and a change while open closes the dialog", async () => {
    const { user, source, store } = setup();
    const trust = vi.spyOn(source, "trustRepositoryFilters");
    const shown = (await source.getRepositorySecurity("repo-why")).fingerprint;
    await openConfirmation(user);

    // The configuration changes under the same name while the dialog is open,
    // and a refresh delivers the new report.
    source.setFilters("why", [{ ...sampleFilters.why![0]!, clean: "./scripts/exfiltrate.sh %f" }]);
    await store.getState().loadSecurity("repo-why", true);

    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(trust).not.toHaveBeenCalled();
    expect(screen.getByText(/changed while you were reviewing them/)).toBeTruthy();
    // The panel now lists the NEW command, so it can be read again.
    expect(screen.getByText("./scripts/exfiltrate.sh %f")).toBeTruthy();

    // Reviewing again and confirming trusts the NEW fingerprint, not the old one.
    const again = within(await openConfirmation(user));
    expect(again.getByText("./scripts/exfiltrate.sh %f")).toBeTruthy();
    await user.click(again.getByRole("button", { name: "Trust and continue" }));
    await waitFor(() => expect(trust).toHaveBeenCalledTimes(1));
    const sent = trust.mock.calls[0]?.[1];
    expect(sent).not.toBe(shown);
    expect(sent).toBe((await source.getRepositorySecurity("repo-why")).fingerprint);
  });

  it("confirming sends the snapshot even if the live report already moved on", async () => {
    const { user, source } = setup();
    const trust = vi.spyOn(source, "trustRepositoryFilters");
    const shown = (await source.getRepositorySecurity("repo-why")).fingerprint;
    const confirm = within(await openConfirmation(user));
    // The configuration changes but no refresh has arrived yet: the core's own
    // re-check is the backstop, and it must be given the fingerprint the user saw.
    source.setFilters("why", [{ ...sampleFilters.why![0]!, clean: "./other.sh" }]);
    await user.click(confirm.getByRole("button", { name: "Trust and continue" }));
    await waitFor(() => expect(trust).toHaveBeenCalledWith("repo-why", shown));
    // ...which refuses it, and nothing is trusted.
    await waitFor(() =>
      expect(
        within(screen.getByRole("region", { name: "Notifications" })).getByText(
          /Could not trust why's filter programs/,
        ),
      ).toBeTruthy(),
    );
  });
});

describe("a refusal the security cache did not expect", () => {
  it("refetches the report so the blocked panel appears instead of 'Reading changes…'", async () => {
    const { user, source } = setup({});
    await screen.findByRole("list", { name: "Changed files" });
    expect(screen.queryByRole("region", { name: "Blocked until you review it" })).toBeNull();

    // A driver arrives through a file the watcher does not route: the cached
    // report still says "clear".
    source.setFilters("why", sampleFilters.why ?? []);
    const list = await screen.findByRole("list", { name: "Worktrees in why" });
    await user.click(within(list).getByRole("button", { name: /^main/ }));

    await panel();
    expect(screen.queryByText("Reading changes…")).toBeNull();
  });
});

describe("stopping trust", () => {
  async function trustWhy(user: ReturnType<typeof userEvent.setup>) {
    const confirm = within(await openConfirmation(user));
    await user.click(confirm.getByRole("button", { name: "Trust and continue" }));
    await screen.findByRole("list", { name: "Changed files" });
  }

  async function openMenuItem(name: RegExp) {
    const trigger = screen.getByRole("button", { name: "Actions for why" });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    return await screen.findByRole("menuitem", { name });
  }

  it("is still offered when only some approvals remain", async () => {
    const { user, source, store } = setup();
    await trustWhy(user);
    // The repository gains a second, untrusted driver: blocked as a whole, but
    // the first approval is still in force.
    source.setFilters("why", [
      ...(sampleFilters.why ?? []),
      { name: "second", clean: "./second.sh", scope: "local", origin: ".git/config" },
    ]);
    await store.getState().loadSecurity("repo-why", true);
    await waitFor(() =>
      expect(store.getState().security.get("repo-why")).toMatchObject({
        status: "ok",
        data: { status: "blocked" },
      }),
    );

    expect(await openMenuItem(/Stop trusting filter programs/)).toBeTruthy();
  });

  it("is not offered when nothing is trusted", async () => {
    setup();
    await panel();
    const trigger = screen.getByRole("button", { name: "Actions for why" });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    await screen.findByRole("menuitem", { name: /Remove from WHY/ });
    expect(screen.queryByRole("menuitem", { name: /Stop trusting/ })).toBeNull();
  });

  it("counts as done when the re-check fails: stale state is dropped and the failure is reported", async () => {
    const { user, source } = setup();
    await trustWhy(user);
    vi.spyOn(source, "revokeRepositoryFilters").mockImplementation(async () => {
      // Trust is gone in the core, but rescanning a worktree failed.
      await FixtureDataSource.prototype.revokeRepositoryFilters.call(source, "repo-why");
      return {
        security: null,
        rescanError: { kind: "gitFailed", message: "cannot read worktree" },
      };
    });

    await user.click(screen.getByRole("button", { name: "Stop trusting" }));

    await waitFor(() =>
      expect(
        within(screen.getByRole("region", { name: "Notifications" })).getByText(
          /Stopped trusting why, but could not re-check it/,
        ),
      ).toBeTruthy(),
    );
    // The old "you trusted this" notice is gone (the cached report was dropped),
    // and what was read under the old decision was read again: the core now
    // refuses, so the panel is back.
    expect(screen.queryByText(/You trusted this repository's git filter programs/)).toBeNull();
    await panel();
  });
});
