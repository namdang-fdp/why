// @vitest-environment jsdom

import { createElement } from "react";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ShellOutcomesResult } from "@/contracts";
import { FixtureDataSource, type FixtureOptions } from "@/data/fixture-data-source";
import { SESSION_LONG } from "@/dev-fixtures/long-session";
import { WT_WHY_AGENT, WT_WHY_MAIN } from "@/fixtures";
import { unavailableReasons } from "@/fixtures/evidence";
import { SESSION_CLAUDE, SESSION_CODEX } from "@/fixtures/sessions";
import { unavailableText } from "@/components/sessions/rationale-model";
import { deferred } from "@/test/deferred";
import { mockTimelineViewport } from "@/test/timeline-viewport";
import { inspectorViewStore, resetInspectorView, type InspectorTab } from "@/stores/inspector-view";
import { createWorkspaceStore } from "@/stores/workspace";
import { openTurn } from "@/components/sessions/session-controls";
import { selectWorktree } from "@/stores/workspace";
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

function setup(options: FixtureOptions | FixtureDataSource = {}, tab: InspectorTab = "worktree") {
  // The inspector opens on its File tab; most of these tests look at the session list.
  inspectorViewStore.getState().setTab(tab);
  const source = options instanceof FixtureDataSource ? options : new FixtureDataSource(options);
  const store = createWorkspaceStore(source);
  const user = userEvent.setup();
  render(createElement(App, { store }));
  return { source, store, user };
}

const why = () => within(screen.getByRole("complementary", { name: "Why" }));
const fileList = () => screen.findByRole("list", { name: "Changed files" });
const chip = async (name: RegExp, path: RegExp) => {
  const row = (await within(await fileList()).findByRole("button", { name: path })).closest("li");
  if (!row) throw new Error("Expected a file row");
  return within(row).findByRole("button", { name });
};
async function openSession(user: ReturnType<typeof userEvent.setup>, name: RegExp) {
  // The session list lives on the Worktree tab.
  act(() => inspectorViewStore.getState().setTab("worktree"));
  const rows = await why().findByRole("list", { name: "Worktree sessions" });
  await user.click(within(rows).getByRole("button", { name }));
  return why().findByRole("region", { name: "Session turns" });
}
const codexSession = (user: ReturnType<typeof userEvent.setup>) =>
  openSession(user, /Codex.*Create src/);

describe("attribution in the changed-files rail", () => {
  it("says what is known for every status and never calls anything manual, human or yours", async () => {
    setup();
    const files = await fileList();
    await within(files).findByRole("button", { name: "Explained by 2 turns" });
    const text = files.textContent ?? "";
    const names = within(files)
      .getAllByRole("button")
      .map((button) => button.getAttribute("aria-label"));
    expect(names).toContain("Explained by 2 turns"); // both harnesses
    expect(names).toContain("Explained by 1 turn");
    expect(text).toContain("Agent tried, not applied");
    expect(text).toContain("No captured agent edit");
    expect(text).not.toMatch(/manual|human|\byou\b|\byours?\b|100\s?%/i);
    // Both agents are named on the file two of them edited.
    const mod = (await chip(/Explained by 2 turns/, /mod\.rs/)).closest("li");
    expect(mod?.textContent).toContain("Claude Code");
    expect(mod?.textContent).toContain("Codex");
  });

  it("explains an unexplained file without guessing who changed it", async () => {
    const { user } = setup();
    await user.click(await chip(/No captured agent edit/, /notes\.md/));
    const detail = await screen.findByText(/No captured agent edit explains this file/);
    expect(detail.textContent).not.toMatch(/manual|human|\byou\b/i);
    expect(detail.textContent).toMatch(/shell command/);
    expect(detail.textContent).toMatch(/before hooks were installed/);
  });

  it("calls an unconfirmed edit 'not confirmed' and only a failed one 'not applied'", async () => {
    const { user } = setup();
    // Cargo.toml: the harness said the edit failed. legacy.rs: a start with no result.
    const files = await fileList();
    const cargo = (await within(files).findByRole("button", { name: /Cargo\.toml/ })).closest("li");
    expect(cargo?.textContent).toContain("Agent tried, not applied");
    const legacy = (await within(files).findByRole("button", { name: /legacy\.rs/ })).closest("li");
    expect(legacy?.textContent).toContain("Agent edit not confirmed");
    expect(legacy?.textContent).not.toContain("not applied");
    await user.click(await chip(/Agent edit not confirmed/, /legacy\.rs/));
    const outcomeGroup = (await screen.findByText("Not applied or not confirmed")).parentElement;
    expect(outcomeGroup && within(outcomeGroup).getByText("Not confirmed")).toBeTruthy();
    expect(screen.queryByText(/rejected/i)).toBeNull();
  });

  it("shows a rename's attribution on the new path and mentions the old one", async () => {
    const { user } = setup();
    await user.click(await chip(/Explained by 1 turn/, /parse\.rs/));
    expect(await screen.findByText(/Renamed from/)).toBeTruthy();
    // The call itself named the old path.
    expect(screen.getByText(/named src-tauri\/src\/git\/parser\.rs/)).toBeTruthy();
    expect(screen.getByText(/Edits are listed under the new path/)).toBeTruthy();
  });

  it("lists the shell turns once and opens one in the timeline", async () => {
    const { user, store } = setup();
    await fileList();
    const line = await screen.findByRole("button", {
      name: "2 turns also ran shell commands here (their file changes cannot be attributed)",
    });
    expect(screen.getAllByText(/also ran shell commands here/)).toHaveLength(1);
    await user.click(line);
    await user.click(await screen.findByRole("button", { name: /Turn 2.*Add a subtract/s }));
    const sessions = store.getState().sessions.getState();
    await waitFor(() => expect(sessions.sessionId).toBe(SESSION_CLAUDE));
    await waitFor(() => expect(store.getState().sessions.getState().turnId).toBe("c3a1f0e2-0002"));
  });

  it("opens an edit's turn in the timeline", async () => {
    const { user, store } = setup();
    await user.click(await chip(/Explained by 2 turns/, /mod\.rs/));
    const popover = await screen.findByText("Applied edits (newest first)");
    const list = popover.nextElementSibling as HTMLElement;
    await user.click(within(list).getAllByRole("button")[1] as HTMLElement); // the older, Claude edit
    await waitFor(() =>
      expect(store.getState().sessions.getState().sessionId).toBe(SESSION_CLAUDE),
    );
    await waitFor(() => expect(store.getState().sessions.getState().turnId).toBe("c3a1f0e2-0001"));
  });

  it("narrows to unexplained files from the rail and from the palette", async () => {
    const { user } = setup();
    const files = await fileList();
    await within(files).findByRole("button", { name: "Explained by 2 turns" });
    const rows = () => files.querySelectorAll(".file-row").length;
    expect(rows()).toBe(8);
    await user.click(await screen.findByRole("button", { name: "Unexplained 5" }));
    expect(rows()).toBe(5); // no explained file stays
    await user.click(screen.getByRole("button", { name: "Unexplained 5" }));
    expect(rows()).toBe(8);

    await user.keyboard("{Control>}k{/Control}");
    await user.click(await screen.findByText("Show unexplained files"));
    await waitFor(() => expect(rows()).toBe(5));
  });

  it("highlights turns from the core's edit history even when events name another path", async () => {
    const { user } = setup({}, "file");
    // The Claude turn wrote the file under its OLD name; its events list only src/calc.py.
    await user.click(await within(await fileList()).findByRole("button", { name: /parse\.rs/ }));
    const edits = await why().findByRole("region", { name: /Captured edits to/ });
    expect(await within(edits).findByText("Applied")).toBeTruthy();
    const region = await openSession(user, /Claude Code.*subtract/);
    const turn = region.querySelector('[data-turn-id="c3a1f0e2-0001"] .session-turn-body');
    expect(turn?.getAttribute("data-file-match")).toBe("true");
    expect(
      region
        .querySelector('[data-turn-id="c3a1f0e2-0003"] .session-turn-body')
        ?.hasAttribute("data-file-match"),
    ).toBe(false);
  });

  it("lists the selected file's turns in the inspector, newest first, with outcome chips", async () => {
    const { user } = setup({}, "file");
    await user.click(await within(await fileList()).findByRole("button", { name: /mod\.rs/ }));
    const edits = await why().findByRole("region", { name: /Captured edits to/ });
    const lines = await within(edits).findAllByRole("button");
    const text = lines.map((line) => line.textContent ?? "");
    expect(text).toHaveLength(3);
    // Newest first: Codex's applied retry, its earlier unconfirmed start, then Claude's edit.
    expect(text[0]).toMatch(/Codex.*Applied/);
    expect(text[1]).toMatch(/Codex.*Not confirmed/);
    expect(text[2]).toMatch(/Claude Code.*Applied/);
  });

  it("drops attribution after switching worktree while it is in flight", async () => {
    const source = new FixtureDataSource();
    const slow = deferred<Awaited<ReturnType<FixtureDataSource["getWorktreeAttribution"]>>>();
    const real = source.getWorktreeAttribution.bind(source);
    vi.spyOn(source, "getWorktreeAttribution").mockImplementationOnce(() => slow.promise);
    const { user, store } = setup(source);
    await user.click(await screen.findByRole("button", { name: /^main/ }));
    await waitFor(() => expect(store.getState().worktreeId).toBe(WT_WHY_MAIN));
    slow.resolve(await real(WT_WHY_AGENT, "vsBase"));
    await act(async () => {});
    const files = await fileList();
    expect(files.textContent).not.toContain("Explained by 2 turns");
  });
});

describe("Why? on demand", () => {
  it("shows the agent's words as Declared, with source, excerpt, encrypted and a long-text fold", async () => {
    const { user } = setup();
    const region = await codexSession(user);
    await user.click(within(region).getByRole("button", { name: "Why? Turn 2" }));
    const panel = await within(region).findByText(/The agent.s own words/);
    const box = panel.closest("[data-why-panel]") as HTMLElement;
    expect(within(box).getByText("Declared")).toBeTruthy();
    expect(within(box).getByText("Codex rollout")).toBeTruthy();
    expect(within(box).getByText("excerpt")).toBeTruthy();
    expect(within(box).getByText(/Some text is an excerpt/)).toBeTruthy();
    expect(box.querySelector("[data-encrypted]")?.textContent).toMatch(/Encrypted message/);
    const more = within(box).getByRole("button", { name: "Show more" });
    await user.click(more);
    expect(within(box).getByRole("button", { name: "Show less" })).toBeTruthy();
  });

  it("explains an unsupported harness version quietly and keeps the text", async () => {
    const { user } = setup();
    const region = await codexSession(user);
    await user.click(within(region).getByRole("button", { name: "Why? Turn 3" }));
    expect(await within(region).findByText(/Running the unit tests/)).toBeTruthy();
    expect(within(region).getByText(/newer than the ones WHY was checked against/)).toBeTruthy();
  });

  it("does not read a transcript for a tool call until its Why? opens, and says when the read stopped early", async () => {
    const { user, source } = setup();
    const tool = vi.spyOn(source, "getToolRationale");
    const region = await codexSession(user);
    expect(tool).not.toHaveBeenCalled();
    const buttons = within(region).getAllByRole("button", { name: /^Why\? Edit/ });
    // The second Codex edit is the rejected-or-lost patch: its read stopped at a limit.
    await user.click(buttons[2] as HTMLElement);
    await within(region).findByText(/There may be more/);
    expect(tool).toHaveBeenCalledTimes(1);
  });

  it("says a truncated read with no words is 'not found', not silence", async () => {
    const { user, source } = setup();
    vi.spyOn(source, "getTurnRationale").mockResolvedValue({
      status: "ok",
      rationale: {
        evidence: "declared",
        source: "codexRollout",
        blocks: [],
        harnessVersion: "0.9.2",
        versionSupported: true,
        truncated: true,
      },
    });
    const region = await codexSession(user);
    await user.click(within(region).getByRole("button", { name: "Why? Turn 2" }));
    expect(await within(region).findByText(/Not found within the read limit/)).toBeTruthy();
    expect(region.textContent).not.toMatch(/said nothing here/);
  });

  it("says plainly when the transcript records no text, without claiming the agent was silent by choice", async () => {
    const { user, source } = setup();
    vi.spyOn(source, "getTurnRationale").mockResolvedValue({
      status: "ok",
      rationale: {
        evidence: "declared",
        source: "claudeTranscript",
        blocks: [],
        harnessVersion: "2.1.150",
        versionSupported: true,
        truncated: false,
      },
    });
    const region = await codexSession(user);
    await user.click(within(region).getByRole("button", { name: "Why? Turn 2" }));
    expect(await within(region).findByText(/records no text from the agent here/)).toBeTruthy();
  });

  it.each(unavailableReasons)(
    "explains an unavailable transcript (%s) as a normal state",
    async (reason) => {
      const { user } = setup({ evidence: { rationale: reason } });
      const region = await codexSession(user);
      await user.click(within(region).getByRole("button", { name: "Why? Turn 2" }));
      const message = await within(region).findByText(unavailableText[reason]);
      expect(message.getAttribute("data-reason")).toBe(reason);
      expect(within(region).queryByRole("alert")).toBeNull();
      // Missing evidence is Unknown, never dressed up as an answer.
      const panel = message.closest("[data-why-panel]") as HTMLElement;
      expect(within(panel).getByText("Unknown")).toBeTruthy();
      expect(within(panel).queryByText("Declared")).toBeNull();
    },
  );

  it("opens the same Why? twice with one read, and a refresh reads again", async () => {
    const { user, source } = setup();
    const read = vi.spyOn(source, "getTurnRationale");
    const region = await codexSession(user);
    const toggle = within(region).getByRole("button", { name: "Why? Turn 2" });
    await user.click(toggle);
    await within(region).findByText(/The agent.s own words/);
    await user.click(toggle);
    await user.click(toggle);
    expect(read).toHaveBeenCalledTimes(1);
    await user.click(within(region).getByRole("button", { name: /Refresh/ }));
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  });

  it("does not reopen a Why? closed before its answer arrives", async () => {
    const { user, source } = setup();
    const late = deferred<Awaited<ReturnType<FixtureDataSource["getTurnRationale"]>>>();
    vi.spyOn(source, "getTurnRationale").mockReturnValueOnce(late.promise);
    const region = await codexSession(user);
    const toggle = within(region).getByRole("button", { name: "Why? Turn 2" });
    await user.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    await user.click(toggle);
    await act(async () => late.resolve({ status: "unavailable", reason: "missing" }));
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(within(region).queryByText(unavailableText.missing)).toBeNull();
  });

  it("makes no rationale call while rendering a 500-turn timeline", async () => {
    const { user, source } = setup();
    const turn = vi.spyOn(source, "getTurnRationale");
    const tool = vi.spyOn(source, "getToolRationale");
    await user.click(await screen.findByRole("button", { name: /detached @ 1a2b3c4/ }));
    const rows = await why().findByRole("list", { name: "Worktree sessions" });
    await user.click(within(rows).getByRole("button", { name: /500 turns/ }));
    const region = await why().findByRole("region", { name: "Session turns" });
    mockTimelineViewport(region, 600);
    expect(within(region).getAllByRole("article").length).toBeGreaterThan(0);
    expect(turn).not.toHaveBeenCalled();
    expect(tool).not.toHaveBeenCalled();
    void SESSION_LONG;
  });

  it("opens why for the selected turn from the palette, and w toggles it", async () => {
    const { user, source } = setup();
    const read = vi.spyOn(source, "getTurnRationale");
    const region = await codexSession(user);
    await user.click(within(region).getByRole("button", { name: "Turn 2" }));
    expect(read).not.toHaveBeenCalled();
    await user.keyboard("{Control>}k{/Control}");
    await user.click(await screen.findByText("Show why for this turn"));
    await within(region).findByText(/The agent.s own words/);
    expect(read).toHaveBeenCalledTimes(1);
  });
});

describe("recovered Codex exit codes", () => {
  it("overlays a failed and a passed exit code from the rollout and leaves unresolved calls unknown", async () => {
    const { user, source } = setup();
    const read = vi.spyOn(source, "getShellOutcomes");
    const region = await codexSession(user);
    await within(region).findByText("Failed · exit 1");
    expect(within(region).getByText("OK · exit 0")).toBeTruthy();
    expect(within(region).getAllByText("Observed · from rollout")).toHaveLength(2);
    // The third shell call has no matching record: still unknown, so no outcome chip and no code.
    const mystery = within(region).getByText("./scripts/check.sh --all").closest("li");
    expect(within(mystery as HTMLElement).queryByText("Unknown")).toBeNull();
    expect(mystery?.textContent).not.toMatch(/exit/);
    await user.click(why().getByRole("button", { name: /Session details/ }));
    expect(why().getByText(/1 shell call stays unknown/)).toBeTruthy();
    // Calls the hooks DID report keep the hook's word and get no rollout label.
    const patch = within(region).getAllByText("OK");
    expect(patch.length).toBeGreaterThan(0);
    expect(read).toHaveBeenCalledTimes(1);
  });

  it("never asks for exit codes of a Claude session", async () => {
    const { user, source } = setup();
    const read = vi.spyOn(source, "getShellOutcomes");
    await openSession(user, /Claude Code.*subtract/);
    await waitFor(() => expect(read).not.toHaveBeenCalled());
    expect(screen.queryByText(/rollout/)).toBeNull();
  });

  it("changes nothing for an outcome whose eventId matches no row", async () => {
    const { user, source } = setup();
    const stray: ShellOutcomesResult = {
      status: "ok",
      result: {
        outcomes: [
          {
            toolUseId: "toolu_nope",
            eventId: "no-such-event",
            turnId: "no-such-turn",
            agentId: null,
            exitCode: 9,
            outcome: "failed",
            evidence: "observed",
            source: "codexRollout",
          },
        ],
        unresolved: 0,
        harnessVersion: "0.9.2",
        versionSupported: true,
        truncated: false,
      },
    };
    vi.spyOn(source, "getShellOutcomes").mockResolvedValue(stray);
    const region = await codexSession(user);
    await within(region).findByText("Run the tests.");
    await act(async () => {});
    expect(region.textContent).not.toMatch(/exit 9|from rollout/);
    // Unknown is never a chip per row.
    expect(within(region).queryAllByText("Unknown")).toHaveLength(0);
  });

  it.each([
    ["unavailable", /could not be recovered/],
    ["unsupportedVersion", /not a version WHY has checked/],
    ["truncated", /larger than the read limit/],
  ] as const)(
    "says once, quietly, in the header when the rollout is %s",
    async (outcomes, text) => {
      const { user } = setup({ evidence: { outcomes } });
      await codexSession(user);
      // The recovery note sits behind the collapsed "Session details" disclosure.
      await user.click(await why().findByRole("button", { name: /Session details/ }));
      const note = await why().findByText(text);
      expect(note.closest(".session-outcomes-note")).toBeTruthy();
      expect(why().getAllByText(text)).toHaveLength(1);
      // Not recovered means no invented code (a truncated read still recovered some).
      if (outcomes !== "truncated") expect(why().queryByText(/exit \d/)).toBeNull();
    },
  );

  it("drops exit codes that arrive after another session was opened", async () => {
    const { user, source } = setup();
    const late = deferred<ShellOutcomesResult>();
    vi.spyOn(source, "getShellOutcomes").mockReturnValueOnce(late.promise);
    const region = await codexSession(user);
    await user.click(await why().findByRole("button", { name: /Sessions/ }));
    await openSession(user, /Claude Code.*subtract/);
    await act(async () =>
      late.resolve(await new FixtureDataSource().getShellOutcomes(SESSION_CODEX)),
    );
    expect(why().queryByText(/exit \d/)).toBeNull();
    expect(region.isConnected).toBe(false);
  });
});

describe("review follow-ups", () => {
  it("says nothing about the version when there was nothing to recover", async () => {
    const { user, source } = setup();
    vi.spyOn(source, "getShellOutcomes").mockResolvedValue({
      status: "ok",
      result: {
        outcomes: [],
        unresolved: 0,
        harnessVersion: null,
        versionSupported: false,
        truncated: false,
      },
    });
    await codexSession(user);
    await why().findByText("Run the tests.");
    await act(async () => {});
    expect(why().queryByText(/WHY has checked/)).toBeNull();
    expect(why().queryByRole("status")).toBeNull();
  });

  it("asks for attribution again when the base changes in place", async () => {
    const { source, store } = setup();
    await fileList();
    await screen.findByRole("button", { name: "Explained by 2 turns" });
    const read = vi.spyOn(source, "getWorktreeAttribution");
    const worktree = selectWorktree(store.getState());
    if (!worktree) throw new Error("Expected a selected worktree");
    await act(async () => void (await store.getState().setBase(worktree, "main")));
    await waitFor(() => expect(read).toHaveBeenCalled());
  });

  it("keeps lossy paths out of evidence: no marker, no history lookup", async () => {
    const { user, source } = setup({}, "file");
    const history = vi.spyOn(source, "getFileActivity");
    await user.click(await screen.findByRole("button", { name: /detached @ 1a2b3c4/ }));
    const files = await fileList();
    const lossy = (await within(files).findByRole("button", { name: /caf/ })).closest("li");
    await within(files).findAllByRole("button", { name: "No captured agent edit" });
    expect(lossy?.querySelector(".evidence-chip")).toBeNull();
    await user.click(within(lossy as HTMLElement).getByRole("button", { name: /caf/ }));
    await act(async () => {});
    expect(history).not.toHaveBeenCalledWith("wt-billing-detached", "docs/caf�.txt");
    expect(screen.queryByRole("region", { name: /Captured edits to/ })).toBeNull();
  });

  it("keeps the unexplained toggle while it hides everything, and says why the list is empty", async () => {
    const { user, source } = setup();
    const files = await fileList();
    await user.click(await screen.findByRole("button", { name: "Unexplained 5" }));
    // A name filter that only explained files match: the toggle stays, and the message is exact.
    await user.keyboard("/mod.rs");
    expect(await screen.findByText("No unexplained file matches the filter.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Unexplained 5" })).toBeTruthy();
    expect(files.querySelectorAll(".file-row")).toHaveLength(0);

    // Every file explained: the toggle is still there to turn the filter off.
    const real = await source.getWorktreeAttribution(WT_WHY_AGENT, "vsBase");
    vi.spyOn(source, "getWorktreeAttribution").mockResolvedValue({
      ...real,
      files: real.files.map((file) => ({ ...file, status: "explained" as const })),
    });
    await user.keyboard("{Escape}");
    act(() => source.emitWorktreeChanged({ worktreeId: WT_WHY_AGENT, paths: [] }));
    expect(await screen.findByText("Every changed file has a captured agent edit.")).toBeTruthy();
    expect(screen.getByRole("button", { name: /^Unexplained/ })).toBeTruthy();
  });

  it("opens a turn link that was clicked while its timeline was still loading", async () => {
    const source = new FixtureDataSource();
    const store = createWorkspaceStore(source);
    const sessions = store.getState().sessions;
    const real = await source.getSessionTimeline(SESSION_CLAUDE);
    const late = deferred<typeof real>();
    vi.spyOn(source, "getSessionTimeline").mockReturnValueOnce(late.promise);
    await sessions.getState().selectWorktree(WT_WHY_AGENT);
    const opening = sessions.getState().selectSession(SESSION_CLAUDE);
    const jump = openTurn(store, { sessionId: SESSION_CLAUDE, turnId: "c3a1f0e2-0002" });
    late.resolve(real);
    await Promise.all([opening, jump]);
    expect(sessions.getState().turnId).toBe("c3a1f0e2-0002");
  });

  it("selects a turn recorded without an id but offers no Why? for it", async () => {
    const source = new FixtureDataSource();
    const real = await source.getSessionTimeline(SESSION_CODEX);
    const [first, ...rest] = real.turns;
    if (!first) throw new Error("Expected turns");
    vi.spyOn(source, "getSessionTimeline").mockResolvedValue({
      ...real,
      turns: [{ ...first, id: "" }, ...rest],
    });
    const read = vi.spyOn(source, "getTurnRationale");
    const { user, store } = setup(source);
    const region = await codexSession(user);
    expect(within(region).queryByRole("button", { name: "Why? Turn 1" })).toBeNull();
    expect(within(region).getByRole("button", { name: "Why? Turn 2" })).toBeTruthy();
    await openTurn(store, { sessionId: SESSION_CODEX, turnId: "" });
    expect(store.getState().sessions.getState().turnId).toBe("");
    store
      .getState()
      .evidence.getState()
      .toggleWhy({ kind: "turn", sessionId: SESSION_CODEX, turnId: "" });
    expect(read).not.toHaveBeenCalled();
  });

  it("previews on hover, pins on click, and closes a preview when the pointer leaves", async () => {
    const { user } = setup();
    const marker = await chip(/No captured agent edit/, /notes\.md/);
    const text = /No captured agent edit explains this file/;

    await user.hover(marker);
    expect(await screen.findByText(text)).toBeTruthy();
    await user.unhover(marker);
    await waitFor(() => expect(screen.queryByText(text)).toBeNull());

    // Click after the preview opened: it stays, and leaving does not dismiss it.
    await user.hover(marker);
    await screen.findByText(text);
    await user.click(marker);
    await user.unhover(marker);
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(screen.queryByText(text)).not.toBeNull();
  });
});
