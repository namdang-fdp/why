// @vitest-environment jsdom

import { createElement } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionSummary, SessionTimeline, TimelineEvent } from "@/contracts";
import * as timelineModel from "@/components/sessions/timeline-model";
import { deferred } from "@/test/deferred";
import { mockTimelineViewport } from "@/test/timeline-viewport";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { SESSION_LONG } from "@/dev-fixtures/long-session";
import { SESSION_CODEX, SESSION_SUBAGENT } from "@/fixtures/sessions";
import { createWorkspaceStore } from "@/stores/workspace";
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
  cleanup();
  vi.restoreAllMocks();
  localStorage.clear();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(HTMLElement.prototype, "scrollIntoView");
});

function setup(source = new FixtureDataSource()) {
  const store = createWorkspaceStore(source);
  const user = userEvent.setup();
  render(createElement(App, { store }));
  return { source, store, user };
}

const why = () => within(screen.getByRole("complementary", { name: "Why" }));
const sessionRows = async () =>
  within(await why().findByRole("list", { name: "Worktree sessions" }));
const turnRegion = async () => why().findByRole("region", { name: "Session turns" });

async function openLongSession(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: /detached @ 1a2b3c4/ }));
  await user.click((await sessionRows()).getByRole("button", { name: /500 turns/ }));
  return turnRegion();
}

describe("session states in the inspector", () => {
  it("shows list loading, an IPC error with retry, and the empty worktree state", async () => {
    const source = new FixtureDataSource();
    const request = deferred<SessionSummary[]>();
    vi.spyOn(source, "listSessions").mockReturnValueOnce(request.promise).mockResolvedValueOnce([]);
    const { user } = setup(source);
    await why().findByText("Loading sessions…");
    await act(async () => request.reject({ kind: "internal", message: "Could not read events" }));
    expect(why().getByRole("alert").textContent).toContain("Could not read events");
    await user.click(why().getByRole("button", { name: "Try again" }));
    await why().findByRole("heading", { name: "No sessions recorded for this worktree" });
    expect(why().getByText(/capture session activity locally/)).toBeTruthy();
  });

  it("shows timeline loading, IPC retry, and an empty timeline", async () => {
    const source = new FixtureDataSource();
    const request = deferred<SessionTimeline>();
    const fixture = await source.getSessionTimeline(SESSION_CODEX);
    vi.spyOn(source, "getSessionTimeline")
      .mockReturnValueOnce(request.promise)
      .mockResolvedValueOnce({ ...fixture, turns: [] });
    const { user } = setup(source);
    await user.click((await sessionRows()).getByRole("button", { name: /Codex.*Create src/ }));
    await why().findByText("Loading timeline…");
    await act(async () => request.reject({ kind: "internal", message: "Timeline read failed" }));
    expect(why().getByRole("alert").textContent).toContain("Timeline read failed");
    await user.click(why().getByRole("button", { name: "Try again" }));
    expect(await why().findByText("No turns recorded for this session.")).toBeTruthy();
  });

  it("returns to the list and drops a session that disappears during a live refresh", async () => {
    const { user, source, store } = setup();
    await user.click((await sessionRows()).getByRole("button", { name: /Codex.*Create src/ }));
    await turnRegion();
    vi.spyOn(source, "getSessionTimeline").mockRejectedValueOnce({ kind: "notFound" });
    const worktreeId = store.getState().worktreeId ?? "";
    act(() => source.emitSessionsChanged({ worktreeId }));
    const rows = await sessionRows();
    expect(rows.queryByRole("button", { name: /Codex/ })).toBeNull();
    expect(rows.getByRole("button", { name: /Claude Code.*subtract/ })).toBeTruthy();
  });

  it("keeps successful reads, the scroll position and an open disclosure on refresh errors", async () => {
    const { user, store, source } = setup();
    const region = await openLongSession(user);
    mockTimelineViewport(region, 600);
    const tool = within(region).getByRole("button", { name: "Shell · Bash" });
    tool.focus();
    await user.keyboard("{Enter}");
    expect(tool.getAttribute("aria-expanded")).toBe("true");
    region.scrollTop = 260;
    fireEvent.scroll(region);
    const sessions = store.getState().sessions;
    const { worktreeId, list, timeline } = sessions.getState();
    if (!worktreeId) throw new Error("Expected the billing worktree");
    vi.spyOn(source, "listSessions").mockRejectedValueOnce({ kind: "timeout" });
    vi.spyOn(source, "getSessionTimeline").mockRejectedValueOnce({ kind: "timeout" });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

    act(() => source.emitSessionsChanged({ worktreeId }));
    await waitFor(() => expect(warn).toHaveBeenCalledTimes(2));
    expect(sessions.getState().list).toBe(list);
    expect(sessions.getState().timeline).toBe(timeline);
    expect(await turnRegion()).toBe(region);
    expect(region.scrollTop).toBe(260);
    expect(tool.isConnected).toBe(true);
    expect(tool.getAttribute("aria-expanded")).toBe("true");
    expect(why().queryByRole("alert")).toBeNull();
  });

  it("rerenders only the turn with a live event added", async () => {
    // Event grouping runs once per mounted TurnRow render.
    const renderRow = vi.spyOn(timelineModel, "eventGroups");
    const { user, source } = setup();
    await user.click((await sessionRows()).getByRole("button", { name: /Codex.*Create src/ }));
    const region = await turnRegion();
    expect(within(region).getAllByRole("article")).toHaveLength(3);
    renderRow.mockClear();
    act(() => {
      source.simulateSessionActivity(SESSION_CODEX, "refresh only this turn");
    });
    await within(region).findByText("refresh only this turn");
    expect(renderRow).toHaveBeenCalledTimes(1);
  });
});

describe("recorded events", () => {
  it("renders every kind, outcome and evidence label, plus synthetic prompts and absolute paths", async () => {
    const source = new FixtureDataSource();
    const fixture = await source.getSessionTimeline(SESSION_SUBAGENT);
    const first = fixture.turns[0];
    const synthetic = fixture.turns[1];
    if (!first || !synthetic) throw new Error("Expected the subagent fixture");
    synthetic.events = synthetic.events.filter(({ kind }) => kind !== "prompt");
    const event: Omit<TimelineEvent, "id" | "kind" | "summary" | "evidence"> = {
      at: first.startedAt,
      toolName: null,
      files: [],
      outcome: null,
      agentId: null,
    };
    first.events.push(
      {
        ...event,
        kind: "prompt",
        id: "test-inferred",
        summary: "Prompt excerpt reconstructed",
        evidence: "inferred",
      },
      {
        ...event,
        kind: "assistantMessage",
        id: "test-declared",
        summary: "I chose to keep it simple",
        evidence: "declared",
      },
      {
        ...event,
        kind: "assistantMessage",
        id: "test-inferred-message",
        summary: "Inferred assistant excerpt",
        evidence: "inferred",
      },
      {
        ...event,
        kind: "assistantMessage",
        id: "test-unknown-message",
        summary: "Unverified assistant excerpt",
        evidence: "unknown",
      },
      {
        ...event,
        kind: "toolCall",
        id: "test-missing",
        summary: "Tool result missing",
        evidence: "unknown",
        toolName: "Read",
        files: ["../x", "~/x"],
      },
    );
    vi.spyOn(source, "getSessionTimeline").mockResolvedValueOnce(fixture);

    const { user } = setup(source);
    await user.click(await screen.findByRole("button", { name: /^main Main/ }));
    await user.click((await sessionRows()).getByRole("button", { name: /Use a subagent/ }));
    const region = await turnRegion();
    const firstTurn = within(why().getByRole("article", { name: "Turn 1" }));
    expect(
      new Set(
        [...region.querySelectorAll("[data-kind]")].map((node) => node.getAttribute("data-kind")),
      ),
    ).toEqual(
      new Set([
        "prompt",
        "assistantMessage",
        "toolCall",
        "edit",
        "shell",
        "testRun",
        "subagent",
        "stop",
      ]),
    );
    const evidenceFor = (summary: string) =>
      firstTurn.getByText(summary).closest("li")?.querySelector("[data-evidence]");
    expect(evidenceFor("I chose to keep it simple")?.getAttribute("data-evidence")).toBe(
      "declared",
    );
    expect(evidenceFor("Inferred assistant excerpt")?.getAttribute("data-evidence")).toBe(
      "inferred",
    );
    expect(evidenceFor("Unverified assistant excerpt")?.getAttribute("data-evidence")).toBe(
      "unknown",
    );
    expect(evidenceFor("Prompt excerpt reconstructed")?.getAttribute("data-evidence")).toBe(
      "inferred",
    );
    expect(evidenceFor("I chose to keep it simple")?.textContent).toContain("Stated by the agent");
    expect(
      [...region.querySelectorAll<HTMLElement>("[data-evidence]")].every(
        (chip) => chip.tabIndex < 0,
      ),
    ).toBe(true);
    expect(firstTurn.getByText("Failed").getAttribute("data-outcome")).toBe("failed");
    expect(
      firstTurn.getAllByText("Unknown").map((node) => node.getAttribute("data-outcome")),
    ).toContain("unknown");
    expect(
      firstTurn.getAllByText("Unknown").map((node) => node.getAttribute("data-evidence")),
    ).toContain("unknown");
    expect(
      firstTurn.getAllByText("OK").every((node) => node.getAttribute("data-outcome") === "ok"),
    ).toBe(true);
    expect(firstTurn.getByText("Observed", { selector: ".session-turn-head span" })).toBeTruthy();
    for (const path of ["/tmp/why-agent/test-helper.log", "../x", "~/x"]) {
      expect(firstTurn.getByText(path).hasAttribute("data-external")).toBe(true);
      expect(firstTurn.queryByRole("button", { name: path })).toBeNull();
    }
    expect(region.querySelector("[data-subagent]")?.textContent).toContain("Subagent started");

    const tool = firstTurn.getByRole("button", { name: "Tool call · Read" });
    expect(tool.getAttribute("aria-expanded")).toBe("false");
    tool.focus();
    await user.keyboard("{Enter}");
    expect(tool.getAttribute("aria-expanded")).toBe("true");
    await user.keyboard("{End}");
    expect(await why().findByText(/not typed by you/)).toBeTruthy();
    expect(
      why()
        .getByRole("article", { name: "Turn 2" })
        .querySelector(".session-turn-head [data-evidence]")
        ?.getAttribute("data-evidence"),
    ).toBe("unknown");
    expect(why().getByText("Passed").getAttribute("data-outcome")).toBe("ok");
  });
});

describe("long session navigation", () => {
  it("bounds rendered rows, reaches turn 500 with the keyboard and cross-highlights both ways", async () => {
    const { user, store, source } = setup();
    const region = await openLongSession(user);
    expect(region.querySelectorAll("[data-turn-id]").length).toBeLessThan(20);
    expect(why().queryByRole("button", { name: "Turn 500" })).toBeNull();

    await user.keyboard("{End}");
    expect(document.activeElement).toBe(why().getByRole("button", { name: "Turn 500" }));
    expect(region.querySelectorAll("[data-turn-id]").length).toBeLessThan(20);
    await user.keyboard("k");
    expect(document.activeElement).toBe(why().getByRole("button", { name: "Turn 499" }));
    await user.keyboard("{Home}");

    const rail = within(screen.getByRole("list", { name: "Changed files" }));
    const billing = rail.getByRole("button", { name: /billing\.ts/ });
    const seed = rail.getByRole("button", { name: /seed\.json/ });
    expect(billing.getAttribute("data-turn-match")).toBe("true");
    expect(seed.getAttribute("data-turn-match")).toBe("true");

    await user.click(billing);
    const first = why().getByRole("article", { name: "Turn 1" });
    expect(first.querySelector(".session-turn-body")?.getAttribute("data-file-match")).toBe("true");
    expect(first.querySelector(".session-event[data-file-match]")?.getAttribute("data-kind")).toBe(
      "edit",
    );
    await user.click(within(first).getByRole("button", { name: "data/seed.json" }));
    await waitFor(() => expect(seed.getAttribute("aria-current")).toBe("true"));

    act(() => {
      source.simulateSessionActivity(SESSION_LONG, "pytest -q");
      source.simulateSessionActivity(SESSION_LONG, "pytest -q", "Recheck billing");
    });
    await why().findByRole("heading", { name: "Turns 501" });
    expect(store.getState().sessions.getState().turnId).toBe("long-turn-0");
    expect(await why().findByRole("button", { name: "2 new" })).toBeTruthy();
  });

  it("uses the same first-match and next-match path from the palette", async () => {
    const { user, store } = setup();
    const region = await openLongSession(user);
    await act(async () => store.getState().selectFile("src/billing.ts"));
    for (const turn of ["Turn 1", "Turn 3"]) {
      await user.keyboard("{Control>}k{/Control}");
      await user.type(await screen.findByRole("combobox"), "Next turn touching this file");
      await user.keyboard("{Enter}");
      await waitFor(() =>
        expect(document.activeElement).toBe(within(region).getByRole("button", { name: turn })),
      );
    }
  });

  it("consumes a focus request whose turn is missing", async () => {
    const { user, store } = setup();
    const region = await openLongSession(user);
    const sessions = store.getState().sessions.getState();
    region.focus();
    act(() => sessions.selectTurn("missing", true));
    act(() => sessions.selectTurn("long-turn-0"));
    expect(document.activeElement).toBe(region);
  });

  it("focuses a hidden inspector and opens the sessions list from the palette", async () => {
    const { user } = setup();
    await sessionRows();
    await user.keyboard("i");
    expect(screen.queryByRole("complementary", { name: "Why" })).toBeNull();
    await user.keyboard("{Shift>}i{/Shift}");
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole("complementary", { name: "Why" })),
    );
    await user.keyboard("j{Enter}");
    await turnRegion();
    await user.keyboard("{Control>}k{/Control}");
    const search = await screen.findByRole("combobox");
    await user.type(search, "Show sessions");
    await user.keyboard("{Enter}");
    await sessionRows();
  });
});
