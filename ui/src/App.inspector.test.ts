// @vitest-environment jsdom

import { createElement } from "react";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import type { TimelineEvent } from "@/contracts";
import { FixtureDataSource, type FixtureOptions } from "@/data/fixture-data-source";
import { buildLongSession } from "@/dev-fixtures/long-session";
import { WT_BILLING_DETACHED, WT_WHY_AGENT, WT_WHY_MAIN } from "@/fixtures";
import { SESSION_CODEX, SESSION_CODEX_QUIET } from "@/fixtures/sessions";
import { inspectorViewStore, resetInspectorView, type InspectorTab } from "@/stores/inspector-view";
import { createWorkspaceStore } from "@/stores/workspace";
import { deferred } from "@/test/deferred";
import { mockTimelineViewport } from "@/test/timeline-viewport";
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
  vi.stubGlobal("matchMedia", () => ({ matches: false }));
  HTMLElement.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  resetInspectorView();
  cleanup();
  vi.restoreAllMocks();
  localStorage.clear();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(HTMLElement.prototype, "scrollIntoView");
});

function setup(source: FixtureDataSource | FixtureOptions = {}, tab: InspectorTab = "file") {
  const data = source instanceof FixtureDataSource ? source : new FixtureDataSource(source);
  inspectorViewStore.getState().setTab(tab);
  const store = createWorkspaceStore(data);
  const user = userEvent.setup();
  render(createElement(App, { store }));
  return { source: data, store, user };
}

const why = () => within(screen.getByRole("complementary", { name: "Why" }));
const fileList = () => screen.findByRole("list", { name: "Changed files" });
const selectFile = async (user: ReturnType<typeof userEvent.setup>, name: RegExp) =>
  user.click(await within(await fileList()).findByRole("button", { name }));
const selectWorktree = async (user: ReturnType<typeof userEvent.setup>, name: RegExp) =>
  user.click(await screen.findByRole("button", { name }));

/** Words an evidence statement must never use: it is about records, not about people. */
const forbidden = /manual|\byou\b|\byour\b|human|100\s?%/i;

describe("inspector tabs", () => {
  it("opens on File and exposes a tablist with aria-selected", async () => {
    setup();
    const tabs = await why().findByRole("tablist", { name: "Inspector view" });
    const [file, worktree] = within(tabs).getAllByRole("tab");
    expect(file?.textContent).toBe("File");
    expect(file?.getAttribute("aria-selected")).toBe("true");
    expect(worktree?.getAttribute("aria-selected")).toBe("false");
    expect(why().getByRole("tabpanel")).toBeTruthy();
  });

  it("moves between tabs with the arrow keys and wraps", async () => {
    const { user } = setup();
    const file = await why().findByRole("tab", { name: "File" });
    file.focus();
    await user.keyboard("{ArrowRight}");
    const worktree = why().getByRole("tab", { name: "Worktree" });
    expect(worktree.getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(worktree);
    await user.keyboard("{ArrowRight}");
    expect(why().getByRole("tab", { name: "File" }).getAttribute("aria-selected")).toBe("true");
    await user.keyboard("{ArrowLeft}");
    expect(why().getByRole("tab", { name: "Worktree" }).getAttribute("aria-selected")).toBe("true");
  });

  it("keeps one tab stop (roving tabindex)", async () => {
    setup();
    const tabs = within(await why().findByRole("tablist")).getAllByRole("tab");
    expect(tabs.map((tab) => tab.tabIndex)).toEqual([0, -1]);
  });

  it("points a reader with no file selected at the Worktree tab", async () => {
    const { user, store } = setup();
    await why().findByRole("tab", { name: "File" });
    act(() => store.setState({ filePath: null }));
    const pointer = await why().findByText(/Select a changed file/);
    expect(pointer.textContent).toMatch(/Worktree tab/);
    await user.click(why().getByRole("button", { name: "Worktree tab" }));
    expect(why().getByRole("tab", { name: "Worktree" }).getAttribute("aria-selected")).toBe("true");
  });

  it("remembers the chosen tab when the inspector is hidden and shown again", async () => {
    const { user } = setup();
    await user.click(await why().findByRole("tab", { name: "Worktree" }));
    await user.click(why().getByRole("button", { name: "Hide Why inspector" }));
    expect(screen.queryByRole("complementary", { name: "Why" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Why inspector" }));
    expect(why().getByRole("tab", { name: "Worktree" }).getAttribute("aria-selected")).toBe("true");
  });

  it("keeps j, k, Home and End working on the session list inside the Worktree tab", async () => {
    const { user } = setup({}, "worktree");
    await selectWorktree(user, /^agent\/WHY-4-git-cli/);
    const list = await why().findByRole("list", { name: "Worktree sessions" });
    const cards = within(list).getAllByRole("button");
    cards[0]?.focus();
    await user.keyboard("j");
    expect(document.activeElement).toBe(cards[1]);
    await user.keyboard("k");
    expect(document.activeElement).toBe(cards[0]);
    await user.keyboard("{End}");
    expect(document.activeElement).toBe(cards.at(-1));
    await user.keyboard("{Home}");
    expect(document.activeElement).toBe(cards[0]);
  });
});

describe("File tab", () => {
  it("says an explained file is explained by its captured edits, with the caveat once", async () => {
    const { user } = setup();
    await selectFile(user, /mod\.rs/);
    const chip = await why().findByText("Explained by 2 captured edits");
    expect(chip.getAttribute("data-kind")).toBe("explained");
    expect(why().getAllByText(/may not have seen every change/)).toHaveLength(1);
    // The captured edits are listed, newest first, and no session log is dumped here.
    expect(await why().findAllByText("Applied")).toHaveLength(2);
    expect(why().queryByRole("list", { name: "Worktree sessions" })).toBeNull();
    expect(why().queryByRole("region", { name: "Session turns" })).toBeNull();
  });

  it("uses the attempt labels for a file the agent tried to change", async () => {
    const { user } = setup();
    await selectFile(user, /legacy\.rs/);
    expect((await why().findByText("Agent edit not confirmed")).getAttribute("data-kind")).toBe(
      "attempted",
    );
  });

  it("says one short sentence, not a log, for a file with no captured edit", async () => {
    const { user } = setup();
    await selectFile(user, /notes\.md/);
    expect((await why().findByText("No captured agent edit")).getAttribute("data-kind")).toBe(
      "unexplained",
    );
    expect(await why().findByText("No captured agent edit names this file.")).toBeTruthy();
    // No edit lines and no session log: the sentence (plus an optional lead) is all there is.
    expect(why().queryByText("Applied")).toBeNull();
    expect(screen.getByRole("tabpanel").querySelectorAll("li")).toHaveLength(0);
  });

  it("adds one shell lead line, never an attribution", async () => {
    const { user } = setup();
    await selectWorktree(user, /^main/);
    await selectFile(user, /link\.sh/);
    const lead = await why().findByText(
      "1 turn ran shell commands here; WHY cannot attribute file changes made by shell commands yet.",
    );
    expect(lead.textContent).not.toMatch(/edited|changed this/);
  });

  it("never uses manual, you, human or 100% in any evidence state", async () => {
    const { user } = setup();
    for (const name of [/mod\.rs/, /legacy\.rs/, /notes\.md/, /worktree\.rs/]) {
      await selectFile(user, name);
      const panel = screen.getByRole("tabpanel");
      await waitFor(() => {
        const kind = panel.querySelector(".file-chip")?.getAttribute("data-kind");
        expect(kind && kind !== "pending").toBe(true);
      });
      expect(panel.textContent ?? "").not.toMatch(forbidden);
    }
  });

  it("shows a neutral chip, not an absence claim, while attribution is loading", async () => {
    const source = new FixtureDataSource();
    const slow = deferred<Awaited<ReturnType<FixtureDataSource["getWorktreeAttribution"]>>>();
    vi.spyOn(source, "getWorktreeAttribution").mockReturnValue(slow.promise);
    const { user } = setup(source);
    await selectFile(user, /notes\.md/);
    const chip = await why().findByText("Checking captured evidence…");
    expect(chip.getAttribute("data-kind")).toBe("pending");
    expect(why().queryByText("No captured agent edit")).toBeNull();
  });

  it("offers a link to the sessions from a compact activity line", async () => {
    const { user } = setup();
    await selectFile(user, /mod\.rs/);
    await why().findByText(/Session activity: 2 sessions\./);
    await user.click(why().getByRole("button", { name: "View sessions" }));
    expect(why().getByRole("tab", { name: "Worktree" }).getAttribute("aria-selected")).toBe("true");
    expect(await why().findByRole("list", { name: "Worktree sessions" })).toBeTruthy();
  });

  it("opens a captured edit's turn in the session explorer, then Sessions goes back to the File tab", async () => {
    const { user } = setup();
    await selectFile(user, /mod\.rs/);
    const edits = await why().findByRole("region", { name: /Captured edits to/ });
    await user.click((await within(edits).findAllByRole("button"))[0] as HTMLElement);
    await why().findByRole("region", { name: "Session turns" });
    expect(why().queryByRole("tablist")).toBeNull();
    await user.click(why().getByRole("button", { name: /Sessions/ }));
    expect(why().getByRole("tab", { name: "File" }).getAttribute("aria-selected")).toBe("true");
  });
});

describe("Worktree tab", () => {
  const legend = (name: string) => why().getByText(name).closest("li") as HTMLElement;

  it("shows the coverage counts the rail shows (same source)", async () => {
    setup({}, "worktree");
    const rail = await fileList();
    await within(rail).findByRole("button", { name: "Explained by 2 turns" });
    const unexplainedButton = await screen.findByRole("button", { name: /^Unexplained \d+$/ });
    // The rail's "Unexplained" count is everything that is not explained: attempted + none.
    const railUnexplained = Number(/\d+/.exec(unexplainedButton.textContent ?? "")?.[0]);
    const rows = rail.querySelectorAll(".file-row").length;
    expect(await why().findByText("Evidence coverage")).toBeTruthy();
    const count = (name: string) => Number(within(legend(name)).getByText(/^\d+$/).textContent);
    const explained = count("Explained");
    const attempted = count("Attempted");
    const none = count("No captured edit");
    expect(attempted + none).toBe(railUnexplained);
    expect(explained + attempted + none).toBe(rows);
  });

  it("turns on the rail's unexplained filter from the 'No captured edit' item", async () => {
    const { user } = setup({}, "worktree");
    await screen.findByRole("button", { name: /^Unexplained \d+$/ });
    await user.click(await why().findByRole("button", { name: /No captured edit/ }));
    expect(
      (await screen.findByRole("button", { name: /^Unexplained \d+$/ })).getAttribute(
        "aria-pressed",
      ),
    ).toBe("true");
  });

  it("never shows another worktree's attribution, even when it arrives late", async () => {
    const source = new FixtureDataSource();
    const real = source.getWorktreeAttribution.bind(source);
    const slow = deferred<Awaited<ReturnType<FixtureDataSource["getWorktreeAttribution"]>>>();
    vi.spyOn(source, "getWorktreeAttribution").mockImplementationOnce(() => slow.promise);
    const { user, store } = setup(source, "worktree");
    await selectWorktree(user, /^main/);
    await waitFor(() => expect(store.getState().worktreeId).toBe(WT_WHY_MAIN));
    // The first worktree's answer arrives after the switch: it must not be drawn.
    slow.resolve(await real(WT_WHY_AGENT, "vsBase"));
    await act(async () => {});
    await why().findByRole("heading", { name: "why" });
    const coverage = why().getByRole("img", { name: /Evidence coverage/ });
    expect(coverage.getAttribute("aria-label")).toBe(
      "Evidence coverage: 1 explained, 0 attempted, 1 no captured edit",
    );
  });

  it("shows a neutral skeleton, never numbers, while attribution loads", async () => {
    const source = new FixtureDataSource();
    vi.spyOn(source, "getWorktreeAttribution").mockReturnValue(
      new Promise(() => undefined) as ReturnType<FixtureDataSource["getWorktreeAttribution"]>,
    );
    setup(source, "worktree");
    await why().findByRole("status", { name: "Loading evidence coverage" });
    expect(why().queryByRole("img", { name: /Evidence coverage/ })).toBeNull();
    expect(why().queryByText("Explained")).toBeNull();
  });

  it("says an attribution error honestly and retries", async () => {
    const source = new FixtureDataSource();
    const real = source.getWorktreeAttribution.bind(source);
    vi.spyOn(source, "getWorktreeAttribution")
      .mockRejectedValueOnce({ kind: "internal", message: "Could not read attribution" })
      .mockImplementation(real);
    const { user } = setup(source, "worktree");
    expect((await why().findByRole("alert")).textContent).toContain("Could not read attribution");
    await user.click(why().getByRole("button", { name: "Try again" }));
    await why().findByRole("img", { name: /Evidence coverage/ });
  });

  it("shows the latest session's task verbatim, labelled with its source", async () => {
    setup({}, "worktree");
    await why().findByText("Evidence coverage");
    // agent worktree: the Codex session is the most recently active one.
    expect(
      why().getAllByText("Create src/notes.py containing NOTE = 1, then delete src/old.py.").length,
    ).toBeGreaterThan(0);
    expect(why().getByText("From the first prompt of Codex")).toBeTruthy();
  });

  it("says 'No task context captured' when there is no task, and never derives one from the branch", async () => {
    const source = new FixtureDataSource();
    const real = source.listSessions.bind(source);
    vi.spyOn(source, "listSessions").mockImplementation(async (id) =>
      (await real(id)).map((session) => ({ ...session, task: null })),
    );
    setup(source, "worktree");
    await why().findByText("Evidence coverage");
    // The selected branch is agent/WHY-4-git-cli: a task-like name that must not become a task.
    expect(why().getAllByText("No task context captured").length).toBeGreaterThan(0);
    expect(why().queryByText(/From the first prompt/)).toBeNull();
    const overview = screen.getByRole("tabpanel").textContent ?? "";
    expect(overview).not.toMatch(/Task unknown/);
    expect(why().getByRole("heading", { name: "WHY-4-git" })).toBeTruthy();
  });

  it("says there is nothing to cover when the worktree has no changed files", async () => {
    const source = new FixtureDataSource();
    const real = source.getWorktreeAttribution.bind(source);
    vi.spyOn(source, "getWorktreeAttribution").mockImplementation(async (...args) => ({
      ...(await real(...args)),
      files: [],
      shellLeads: [],
    }));
    setup(source, "worktree");
    expect(await why().findByText("No changed files to cover.")).toBeTruthy();
    expect(why().queryByRole("img", { name: /Evidence coverage/ })).toBeNull();
  });

  it("omits a line total that is not meaningful when every file is binary", async () => {
    const source = new FixtureDataSource();
    const real = source.getChanges.bind(source);
    vi.spyOn(source, "getChanges").mockImplementation(async (...args) =>
      (await real(...args)).map((file) => ({
        ...file,
        additions: null,
        deletions: null,
        binary: true,
      })),
    );
    setup(source, "worktree");
    await why().findByText("Evidence coverage");
    expect(why().queryByText("added")).toBeNull();
    expect(why().queryByText("removed")).toBeNull();
    expect(why().getByText("changed files")).toBeTruthy();
  });

  it("explains the empty sessions state without lying about hooks", async () => {
    const source = new FixtureDataSource({ hooks: { states: { claudeCode: "installed" } } });
    vi.spyOn(source, "listSessions").mockResolvedValue([]);
    const { user, store } = setup(source, "worktree");
    await why().findByRole("heading", { name: "No sessions recorded for this worktree" });
    expect(await why().findByText(/appear here when an agent.s working directory/)).toBeTruthy();
    await user.click(why().getByRole("button", { name: "Hook settings" }));
    await waitFor(() => expect(store.getState().settingsView.status).toBe("open"));
  });
});

// ---- session explorer -----------------------------------------------------------------------

async function openSession(user: ReturnType<typeof userEvent.setup>, name: RegExp) {
  const list = await why().findByRole("list", { name: "Worktree sessions" });
  await user.click(within(list).getByRole("button", { name }));
  return why().findByRole("region", { name: "Session turns" });
}

/** Open the fixture Codex session whose every outcome is unknown (worktree `main`). */
async function openQuietSession(prepare?: (source: FixtureDataSource) => void) {
  const source = new FixtureDataSource();
  prepare?.(source);
  const ctx = setup(source, "worktree");
  await selectWorktree(ctx.user, /^main/);
  const list = await why().findByRole("list", { name: "Worktree sessions" });
  await ctx.user.click(within(list).getByRole("button", { name: /Survey the repository/ }));
  return { ...ctx, region: await why().findByRole("region", { name: "Session turns" }) };
}

describe("session header", () => {
  it("shows no Task unknown row, and keeps times and the Codex note behind Session details", async () => {
    const source = new FixtureDataSource();
    const fixture = await source.getSessionTimeline(SESSION_CODEX);
    const noTask = { ...fixture, session: { ...fixture.session, task: null } };
    vi.spyOn(source, "getSessionTimeline").mockResolvedValue(noTask);
    const real = source.listSessions.bind(source);
    vi.spyOn(source, "listSessions").mockImplementation(async (id) =>
      (await real(id)).map((s) => (s.id === SESSION_CODEX ? { ...s, task: null } : s)),
    );
    const { user } = setup(source, "worktree");
    await selectWorktree(user, /^agent\/WHY-4-git-cli/);
    await openSession(user, /Codex/);
    expect(why().queryByText("Task unknown")).toBeNull();
    expect(why().queryByRole("heading", { name: "Task" })).toBeNull();
    const toggle = why().getByRole("button", { name: /Session details/ });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(why().queryByText("Started")).toBeNull();
    expect(why().queryByText(/shell call/)).toBeNull();
    await user.click(toggle);
    expect(why().getByText("Started")).toBeTruthy();
    expect(why().getByText("Last activity")).toBeTruthy();
    expect(await why().findByText(/1 shell call stays unknown/)).toBeTruthy();
  });
});

describe("outcome chips", () => {
  it("says outcomes were not recorded once, with no Unknown chip on any row", async () => {
    const { region } = await openQuietSession();
    await why().findByText("Outcomes not recorded for this session");
    expect(why().getAllByText("Outcomes not recorded for this session")).toHaveLength(1);
    expect(within(region).queryAllByText("Unknown")).toHaveLength(0);
    expect(region.querySelector("[data-outcome]")).toBeNull();
  });

  it("still shows a failed call in an otherwise unknown session, and drops the note", async () => {
    const source = new FixtureDataSource();
    const fixture = await source.getSessionTimeline(SESSION_CODEX_QUIET);
    const failing = structuredClone(fixture);
    const shell = failing.turns[1]?.events.find((e) => e.id === "q2-lint");
    if (!shell) throw new Error("fixture changed");
    shell.outcome = "failed";
    vi.spyOn(source, "getSessionTimeline").mockResolvedValue(failing);
    const { user } = setup(source, "worktree");
    await selectWorktree(user, /^main/);
    const list = await why().findByRole("list", { name: "Worktree sessions" });
    await user.click(within(list).getByRole("button", { name: /Survey the repository/ }));
    const region = await why().findByRole("region", { name: "Session turns" });
    expect(await within(region).findByText("Failed")).toBeTruthy();
    expect(why().queryByText("Outcomes not recorded for this session")).toBeNull();
  });

  it("keeps a recovered Codex exit code, labelled from the rollout, apart from a hook outcome", async () => {
    const { user } = setup({}, "worktree");
    const list = await why().findByRole("list", { name: "Worktree sessions" });
    await user.click(within(list).getByRole("button", { name: /Codex.*Create src/ }));
    const region = await why().findByRole("region", { name: "Session turns" });
    const failed = await within(region).findByText("Failed · exit 1");
    expect(failed.hasAttribute("data-recovered")).toBe(true);
    expect(within(region).getAllByText("Observed · from rollout")).toHaveLength(2);
    // A hook-reported outcome has no rollout label.
    const hook = within(region)
      .getAllByText("OK")
      .find((chip) => !chip.hasAttribute("data-recovered"));
    expect(hook?.textContent).toBe("OK");
  });

  it("shows an observed row without an Observed chip but still marks declared and unknown evidence", async () => {
    const { region } = await openQuietSession();
    expect(region.querySelector('[data-evidence="observed"]')).toBeNull();
    expect(region.querySelector('[data-evidence="declared"]')).toBeTruthy();
    expect(why().getByText(/Rows are observed by the agent.s hooks unless marked/)).toBeTruthy();
  });
});

describe("shell grouping in the explorer", () => {
  it("folds 3+ shell calls into one closed row, and leaves two alone", async () => {
    const { user, region } = await openQuietSession();
    const turn1 = within(within(region).getByRole("article", { name: "Turn 1" }));
    const toggle = turn1.getByRole("button", { name: /5 shell commands/ });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(turn1.queryByText("ls -la")).toBeNull();
    await user.click(toggle);
    expect(turn1.getByText("ls -la")).toBeTruthy();
    expect(turn1.getByText("git log --oneline -5")).toBeTruthy();
    // Turn 2 has only two shell calls: plain rows, no run.
    const turn2 = within(within(region).getByRole("article", { name: "Turn 2" }));
    expect(turn2.queryByRole("button", { name: /shell commands/ })).toBeNull();
    expect(turn2.getByText("ruff format .")).toBeTruthy();
  });

  it("does not read a transcript when a group or a strip segment is used", async () => {
    // The spies are in place before the session renders, so a render-time read would be seen.
    let turn: MockInstance | undefined;
    let tool: MockInstance | undefined;
    const { user, region } = await openQuietSession((source) => {
      turn = vi.spyOn(source, "getTurnRationale");
      tool = vi.spyOn(source, "getToolRationale");
    });
    expect(turn).not.toHaveBeenCalled();
    expect(tool).not.toHaveBeenCalled();
    await user.click(within(region).getByRole("button", { name: /5 shell commands/ }));
    await user.click(why().getByRole("button", { name: /Turn 2: 0 edits/ }));
    expect(turn).not.toHaveBeenCalled();
    expect(tool).not.toHaveBeenCalled();
  });

  it("keeps a group open after its turn is scrolled out of the virtualized window and back", async () => {
    const source = new FixtureDataSource();
    const long = buildLongSession(WT_BILLING_DETACHED, 300);
    const first = long.turns[0];
    if (!first) throw new Error("expected a turn");
    first.events = [
      first.events[0] as TimelineEvent,
      ...["a", "b", "c"].map((id): TimelineEvent => ({
        ...(first.events[0] as TimelineEvent),
        id: `run-${id}`,
        kind: "shell",
        summary: `echo ${id}`,
        outcome: "ok",
        toolUseId: `tool-${id}`,
      })),
    ];
    vi.spyOn(source, "getSessionTimeline").mockResolvedValue(long);
    const { user } = setup(source, "worktree");
    await selectWorktree(user, /detached @ 1a2b3c4/);
    const list = await why().findByRole("list", { name: "Worktree sessions" });
    await user.click(within(list).getAllByRole("button")[0] as HTMLElement);
    const region = await why().findByRole("region", { name: "Session turns" });
    mockTimelineViewport(region, 600);
    await user.click(await why().findByRole("button", { name: /3 shell commands/ }));
    expect(why().getByText("echo a")).toBeTruthy();

    // Jump to the end: turn 1 leaves the window (its DOM is gone, not just hidden).
    // A focused row stays mounted by design, so move focus to the scrollport first.
    act(() => region.focus());
    await user.keyboard("{End}");
    await waitFor(() => expect(why().queryByRole("article", { name: "Turn 1" })).toBeNull());
    expect(region.querySelectorAll("[data-turn-id]").length).toBeLessThan(20);

    await user.keyboard("{Home}");
    await why().findByRole("article", { name: "Turn 1" });
    const again = why().getByRole("button", { name: /3 shell commands/ });
    expect(again.getAttribute("aria-expanded")).toBe("true");
    expect(why().getByText("echo a")).toBeTruthy();
  });

  it("still virtualizes the 500-turn session and renders no rationale call", async () => {
    const { user, source } = setup({}, "worktree");
    const turn = vi.spyOn(source, "getTurnRationale");
    const tool = vi.spyOn(source, "getToolRationale");
    await selectWorktree(user, /detached @ 1a2b3c4/);
    const list = await why().findByRole("list", { name: "Worktree sessions" });
    await user.click(within(list).getByRole("button", { name: /500 turns/ }));
    const region = await why().findByRole("region", { name: "Session turns" });
    expect(region.querySelectorAll("[data-turn-id]").length).toBeLessThan(20);
    expect(turn).not.toHaveBeenCalled();
    expect(tool).not.toHaveBeenCalled();
  });
});

describe("turn strip", () => {
  async function openLong(turns: number) {
    const source = new FixtureDataSource();
    vi.spyOn(source, "getSessionTimeline").mockResolvedValue(
      buildLongSession(WT_BILLING_DETACHED, turns),
    );
    const real = source.listSessions.bind(source);
    vi.spyOn(source, "listSessions").mockImplementation(async (id) =>
      (await real(id)).map((s) => ({ ...s, turnCount: turns })),
    );
    const ctx = setup(source, "worktree");
    await selectWorktree(ctx.user, /detached @ 1a2b3c4/);
    const list = await why().findByRole("list", { name: "Worktree sessions" });
    await ctx.user.click(within(list).getAllByRole("button")[0] as HTMLElement);
    const region = await why().findByRole("region", { name: "Session turns" });
    mockTimelineViewport(region, 600);
    return { ...ctx, region, strip: await why().findByRole("group", { name: "Turn overview" }) };
  }

  it.each([1, 3, 79, 80])("has one segment per turn for %i turns", async (turns) => {
    const { strip } = await openLong(turns);
    expect(within(strip).getAllByRole("button")).toHaveLength(turns);
  });

  it.each([81, 1000])("has at most 80 segments for %i turns, no per-turn DOM", async (turns) => {
    const { strip, region } = await openLong(turns);
    const buttons = within(strip).getAllByRole("button");
    expect(buttons.length).toBeLessThanOrEqual(80);
    expect(buttons.length).toBeGreaterThan(40);
    expect(document.querySelectorAll("[data-turn-id]").length).toBeLessThan(25);
    expect(region.querySelectorAll("[data-turn-id]").length).toBeLessThan(25);
    // The last segment reaches the last turn.
    expect(buttons.at(-1)?.getAttribute("aria-label")).toMatch(new RegExp(`(to |Turn )${turns}:`));
  });

  it("selects the first turn of a segment and reveals it in the virtualized list", async () => {
    const { user, strip, store, region } = await openLong(1000);
    const target = within(strip).getAllByRole("button")[40] as HTMLElement;
    const label = target.getAttribute("aria-label") ?? "";
    const from = Number(/Turns (\d+) to/.exec(label)?.[1]);
    await user.click(target);
    const sessions = store.getState().sessions.getState();
    expect(sessions.turnId).toBe(`long-turn-${from - 1}`);
    await why().findByRole("article", { name: `Turn ${from}` });
    // Revealed: the scrollport moved down to it (a selected turn is always mounted, so check the scroll).
    expect(region.scrollTop).toBeGreaterThan(0);
    expect(target.getAttribute("aria-current")).toBe("true");
  });

  it("is a single tab stop and moves with the arrow keys", async () => {
    const { user, strip } = await openLong(30);
    const buttons = within(strip).getAllByRole("button");
    expect(buttons.filter((b) => b.tabIndex === 0)).toHaveLength(1);
    buttons[0]?.focus();
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(buttons[1]);
    await user.keyboard("{ArrowLeft}{ArrowLeft}");
    expect(document.activeElement).toBe(buttons[0]);
  });

  it("describes a segment's counts in its aria-label", async () => {
    const { strip } = await openLong(10);
    const labels = within(strip)
      .getAllByRole("button")
      .map((b) => b.getAttribute("aria-label"));
    expect(labels[0]).toMatch(/^Turn 1: \d+ edits?, \d+ test runs?, \d+ failed$/);
  });
});

describe("explorer keys", () => {
  it("keeps j, k, Home, End, n, p, w and Esc working", async () => {
    // Codex session on the agent worktree; mod.rs (auto-selected) was edited in turn 2 only.
    const { user } = setup({}, "worktree");
    const list = await why().findByRole("list", { name: "Worktree sessions" });
    await user.click(within(list).getByRole("button", { name: /Codex.*Create src/ }));
    const region = await why().findByRole("region", { name: "Session turns" });
    mockTimelineViewport(region, 600);
    const turn = (n: number) => within(region).getByRole("button", { name: `Turn ${n}` });
    await why().findByText("1 touching file");
    await user.keyboard("{Home}");
    expect(document.activeElement).toBe(turn(1));
    await user.keyboard("j");
    expect(document.activeElement).toBe(turn(2));
    await user.keyboard("k");
    expect(document.activeElement).toBe(turn(1));
    await user.keyboard("{End}");
    expect(document.activeElement).toBe(turn(3));
    // n and p jump between turns that touch the selected file, wrapping: from turn 3 both land on turn 2.
    await user.keyboard("n");
    expect(document.activeElement).toBe(turn(2));
    await user.keyboard("{End}p");
    expect(document.activeElement).toBe(turn(2));
    // w opens "Why?" for the selected turn.
    await user.keyboard("{End}w");
    await waitFor(() =>
      expect(why().getByRole("button", { name: "Why? Turn 3" }).getAttribute("aria-expanded")).toBe(
        "true",
      ),
    );
    // Esc leaves the session and the tabs come back.
    await user.keyboard("{Escape}");
    expect(await why().findByRole("tablist")).toBeTruthy();
    expect(why().queryByRole("region", { name: "Session turns" })).toBeNull();
  });
});
