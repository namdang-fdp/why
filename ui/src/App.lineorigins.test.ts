// @vitest-environment jsdom

import { createElement } from "react";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { openLineOrigin } from "@/components/diff/line-origin-actions";
import { openTurn } from "@/components/sessions/session-controls";
import { deferred } from "@/test/deferred";
import { lossyPathCopy, unknownReasonCopy } from "@/components/diff/line-origins-model";
import { FixtureDataSource } from "@/data/fixture-data-source";
import { WT_WHY_AGENT, WT_WHY_MAIN } from "@/fixtures";
import { lineOriginsByFile } from "@/fixtures/provenance";
import { SESSION_CLAUDE } from "@/fixtures/sessions";
import { originsBehind } from "@/stores/line-origins";
import { inspectorViewStore, resetInspectorView } from "@/stores/inspector-view";
import { createWorkspaceStore } from "@/stores/workspace";
import { App } from "./App";

// The Monaco editor itself is not run in jsdom: a click on a gutter mark is `openLineOrigin`,
// which these tests call directly (the Monaco side is covered by the pure model tests).
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

async function open(path: RegExp, provenance = true) {
  const source = new FixtureDataSource({ provenance });
  const store = createWorkspaceStore(source);
  const user = userEvent.setup();
  render(createElement(App, { store }));
  const files = await screen.findByRole("list", { name: "Changed files" });
  await user.click(await within(files).findByRole("button", { name: path }));
  return { source, store, user };
}

const section = () => screen.findByRole("region", { name: "Line origins" });
const lineSection = (line: number) =>
  screen.findByRole("region", { name: `Origin of line ${line}` });

// calc.py: base 1-4, claude turn 5-7, base 8-10, codex turn 11-12, base 13-20, then one
// unknown line per reason (21 noCapture, 22 payloadReduced, 23 unparsed, ...).
describe("File tab: line origins", () => {
  it("shows coverage that adds up, with Claude Code and Codex apart", async () => {
    await open(/calc\.py/);
    const origins = within(await section());
    const coverage = within(
      await origins
        .findByRole("img", { name: /Line origins:/ })
        .then((bar) => bar.parentElement as HTMLElement),
    );
    const legend = coverage.getByRole("list");
    const count = (label: RegExp) => {
      const item = within(legend).getByText(label).closest("li");
      return Number(item?.querySelector("b")?.textContent);
    };
    const claude = count(/Claude Code/);
    const codex = count(/Codex/);
    const unchanged = count(/Unchanged/);
    const unknown = count(/Unknown/);
    expect([claude, codex, unchanged, unknown]).toEqual([3, 2, 15, 7]);
    expect(claude + codex + unchanged + unknown).toBe(27);
    expect(origins.getByText("27 lines.")).toBeTruthy();
  });

  it("opening an edit line shows its origin chain with the right turn, as Observed", async () => {
    const { store } = await open(/calc\.py/);
    await section();
    act(() => openLineOrigin(store, 6));
    const line = within(await lineSection(6));
    expect(line.getByText("Claude Code")).toBeTruthy();
    expect(line.getByText(/^Turn 1\b/)).toBeTruthy();
    // The prompt excerpt and the tool call come from the session's timeline.
    await waitFor(() => expect(line.getByText(/Add a subtract function/)).toBeTruthy());
    expect(line.getByText("Edit")).toBeTruthy();
    expect(line.getByText("Observed")).toBeTruthy();
    // The agent's own words are labelled Declared and are separate from the evidence.
    expect(line.getByText("Declared")).toBeTruthy();
  });

  it("opens the File tab and the inspector when a line is opened", async () => {
    const { store } = await open(/calc\.py/);
    inspectorViewStore.getState().setTab("worktree");
    store.getState().toggleInspector();
    expect(store.getState().inspectorOpen).toBe(false);
    act(() => openLineOrigin(store, 12));
    expect(store.getState().inspectorOpen).toBe(true);
    expect(inspectorViewStore.getState().tab).toBe("file");
    const line = within(await lineSection(12));
    expect(line.getByText("Codex")).toBeTruthy();
    expect(line.getByText(/^Turn 2\b/)).toBeTruthy();
  });

  it('keeps "Why?" lazy: no rationale is read until it is opened, then once', async () => {
    const { source, store, user } = await open(/calc\.py/);
    const turnRead = vi.spyOn(source, "getTurnRationale");
    const toolRead = vi.spyOn(source, "getToolRationale");
    await section();
    act(() => openLineOrigin(store, 6));
    const line = within(await lineSection(6));
    await waitFor(() => expect(line.getByText(/Add a subtract function/)).toBeTruthy());
    expect(turnRead).not.toHaveBeenCalled();
    expect(toolRead).not.toHaveBeenCalled();

    await user.click(line.getByRole("button", { name: /Why\?/ }));
    await waitFor(() => expect(turnRead.mock.calls.length + toolRead.mock.calls.length).toBe(1));
    // The words that come back are labelled Declared and "not verified".
    await waitFor(() => expect(line.getByText(/has not verified/)).toBeTruthy());
  });

  it("selects the turn in the session timeline from the link", async () => {
    const { store, user } = await open(/calc\.py/);
    await section();
    act(() => openLineOrigin(store, 6));
    const line = within(await lineSection(6));
    await user.click(await line.findByRole("button", { name: /Open this turn in the timeline/ }));
    await waitFor(() => {
      const sessions = store.getState().sessions.getState();
      expect(sessions.sessionId).toBe(SESSION_CLAUDE);
      expect(sessions.turnId).toBe("c3a1f0e2-0001");
    });
  });

  it("keeps an open Why? open when the reviewer only opens the turn in the timeline", async () => {
    const { source, store, user } = await open(/calc\.py/);
    await section();
    act(() => openLineOrigin(store, 6));
    const line = within(await lineSection(6));
    await waitFor(() => expect(line.getByText(/Add a subtract function/)).toBeTruthy());
    await user.click(line.getByRole("button", { name: /Why\?/ }));
    await waitFor(() => expect(line.getByText(/has not verified/)).toBeTruthy());
    const evidence = store.getState().evidence;
    expect(evidence.getState().whyOpen.size).toBe(1);
    const reads = vi.spyOn(source, "getToolRationale");

    await user.click(line.getByRole("button", { name: /Open this turn in the timeline/ }));
    // Selecting a session resets the evidence store's open disclosures (they belong to one
    // session). The inspector now shows the timeline, where this call's own Why? sits under
    // the same key: it must still be open, read again once, and never left half-open.
    await waitFor(() => expect(evidence.getState().session?.id).toBe(SESSION_CLAUDE));
    await waitFor(() => expect(evidence.getState().whyOpen.size).toBe(1));
    await waitFor(() => expect(reads).toHaveBeenCalledTimes(1));
  });

  it("explains a base line without attributing it to anyone", async () => {
    const { store } = await open(/calc\.py/);
    await section();
    act(() => openLineOrigin(store, 2));
    const line = within(await lineSection(2));
    expect(line.getByText("Unchanged since the base revision")).toBeTruthy();
    expect(line.queryByText(/Claude Code|Codex/)).toBeNull();
  });

  it("explains every unknown reason in its own words, as Unknown evidence", async () => {
    const { store } = await open(/calc\.py/);
    await section();
    const calc = lineOriginsByFile[WT_WHY_AGENT]?.["src/calc.py"];
    if (calc?.status !== "ok") throw new Error("fixture");
    const unknownRuns = calc.origins.runs.filter((run) => run.origin.kind === "unknown");
    expect(unknownRuns).toHaveLength(7);
    for (const run of unknownRuns) {
      if (run.origin.kind !== "unknown") continue;
      act(() => openLineOrigin(store, run.startLine));
      const line = within(await lineSection(run.startLine));
      expect(line.getByText(unknownReasonCopy[run.origin.reason].short)).toBeTruthy();
      expect(line.getByText("Unknown")).toBeTruthy();
      // An unknown line is never given a name.
      expect(line.queryByText(/^Turn \d/)).toBeNull();
    }
  });

  it("offers the worktree's shell turns as leads for a noCapture line, never as the author", async () => {
    const { store } = await open(/calc\.py/);
    await section();
    act(() => openLineOrigin(store, 21)); // noCapture
    const line = within(await lineSection(21));
    expect(await line.findByText("Look here")).toBeTruthy();
    expect(line.getByText(/Leads only/)).toBeTruthy();
    // Another reason gets no leads.
    act(() => openLineOrigin(store, 22)); // payloadReduced
    const other = within(await lineSection(22));
    expect(other.queryByText("Look here")).toBeNull();
  });

  it("says the file changed, and draws no coverage, until the origins are read again", async () => {
    const { source, store } = await open(/calc\.py/);
    await section();
    act(() => source.simulateEdit(WT_WHY_AGENT, "src/calc.py"));
    expect(originsBehind(store.getState().lineOrigins.getState())).toBe(true);
    const origins = within(await section());
    expect(
      await origins.findByText(
        /line marks are hidden until the diff and the origins have both been read again/,
      ),
    ).toBeTruthy();
    expect(origins.queryByRole("img", { name: /Line origins:/ })).toBeNull();
  });
});

describe("File tab: leaving and refreshing", () => {
  it("leaves an open session timeline when a line is opened, then shows the line", async () => {
    const { store } = await open(/calc\.py/);
    await section();
    const ref = { sessionId: SESSION_CLAUDE, turnId: "c3a1f0e2-0001" };
    await act(async () => openTurn(store, ref));
    await waitFor(() =>
      expect(store.getState().sessions.getState().sessionId).toBe(SESSION_CLAUDE),
    );
    // The timeline replaces the tabs: no File tab, no line section, until the reviewer leaves it.
    expect(screen.queryByRole("region", { name: "Line origins" })).toBeNull();

    act(() => openLineOrigin(store, 6));
    expect(store.getState().sessions.getState().sessionId).toBeNull();
    const line = within(await lineSection(6));
    expect(line.getByText("Claude Code")).toBeTruthy();
    expect(inspectorViewStore.getState().tab).toBe("file");
  });

  it("shows no marks or coverage until the diff has caught up with an edit that keeps the line count", async () => {
    const { source, store } = await open(/calc\.py/);
    await section();
    // The edit does not change the diff's text here, so the line count stays the same:
    // only the revision tokens can tell that the shown diff predates the change.
    const readDiff = source.getFileDiff.bind(source);
    const slowDiff = deferred<void>();
    vi.spyOn(source, "getFileDiff").mockImplementationOnce(async (...args) => {
      await slowDiff.promise;
      return readDiff(...args);
    });
    act(() => source.simulateEdit(WT_WHY_AGENT, "src/calc.py"));

    // The origins are read again first (about 300 ms) while the diff read is still held.
    const origins = store.getState().lineOrigins;
    await waitFor(() => expect(origins.getState().revision).toBe(1));
    await waitFor(() => expect(origins.getState().originsRevision).toBe(1), { timeout: 3000 });
    const panel = within(await section());
    expect(panel.queryByRole("img", { name: /Line origins:/ })).toBeNull();
    expect(
      panel.getByText(/hidden until the diff and the origins have both been read/),
    ).toBeTruthy();

    await act(async () => slowDiff.resolve());
    await waitFor(() =>
      expect(
        within(screen.getByRole("region", { name: "Line origins" })).getByRole("img", {
          name: /Line origins:/,
        }),
      ).toBeTruthy(),
    );
  });

  it("says a failed refresh failed, with a Retry, and brings the coverage back after one", async () => {
    const { source, store, user } = await open(/calc\.py/);
    await section();
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(source, "getLineOrigins").mockRejectedValueOnce({ kind: "timeout" });
    act(() => source.simulateEdit(WT_WHY_AGENT, "src/calc.py"));

    const alert = await screen.findByRole("alert", undefined, { timeout: 3000 });
    expect(within(alert).getByText("Reading line origins again")).toBeTruthy();
    // No promise that the marks will come back by themselves.
    expect(screen.queryByText(/Marks return/)).toBeNull();
    expect(originsBehind(store.getState().lineOrigins.getState())).toBe(true);

    await user.click(within(alert).getByRole("button", { name: "Try again" }));
    await waitFor(() =>
      expect(
        within(screen.getByRole("region", { name: "Line origins" })).getByRole("img", {
          name: /Line origins:/,
        }),
      ).toBeTruthy(),
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says plainly that a file with a lossy name has no line origins, instead of loading forever", async () => {
    const source = new FixtureDataSource({ provenance: true });
    const store = createWorkspaceStore(source);
    const user = userEvent.setup();
    render(createElement(App, { store }));
    const readOrigins = vi.spyOn(source, "getLineOrigins");
    await user.click(await screen.findByRole("button", { name: /detached @ 1a2b3c4/ }));
    const files = await screen.findByRole("list", { name: "Changed files" });
    await user.click(await within(files).findByRole("button", { name: /caf/ }));

    const panel = within(await section());
    expect(panel.getByText(lossyPathCopy.short)).toBeTruthy();
    expect(panel.queryByText("Reading line origins…")).toBeNull();
    expect(readOrigins).not.toHaveBeenCalledWith("wt-billing-detached", "docs/caf\uFFFD.txt");
  });
});

describe("fixtures: the file's evidence agrees with its line origins", () => {
  it("explains each sample file by exactly the turns its edit runs name", async () => {
    const source = new FixtureDataSource({ provenance: true });
    let checked = 0;
    for (const worktreeId of [WT_WHY_AGENT, WT_WHY_MAIN]) {
      const attribution = await source.getWorktreeAttribution(worktreeId, "uncommitted");
      for (const entry of attribution.files) {
        const origins = await source.getLineOrigins(worktreeId, entry.change.path);
        if (origins.status !== "ok") continue;
        const named = new Set(
          origins.origins.runs.flatMap((run) =>
            run.origin.kind === "edit"
              ? [
                  `${run.origin.turn.sessionId}|${run.origin.turn.turnId}|${run.origin.turn.toolUseId}`,
                ]
              : [],
          ),
        );
        const explained = new Set(
          entry.explainedBy.map((ref) => `${ref.sessionId}|${ref.turnId}|${ref.toolUseId}`),
        );
        expect(explained, entry.change.path).toEqual(named);
        expect(entry.status, entry.change.path).toBe("explained");
        checked++;
      }
    }
    // calc.py, helper.py and the 2,000-line file.
    expect(checked).toBe(3);
  });

  it("does not tell the File tab 'No captured agent edit' for a file whose gutter names turns", async () => {
    await open(/calc\.py/);
    await section();
    // The captured-edit list under the line section names the same two turns.
    const list = await screen.findByRole("region", { name: "Captured edits to src/calc.py" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(2);
    expect(screen.queryByText("No captured agent edit names this file.")).toBeNull();
  });
});

describe("File tab: line origins not available", () => {
  it("shows a calm 'not available yet' state for engineNotReady: no coverage, no error", async () => {
    // Without the opt-in, every file answers engineNotReady, like the real core today.
    await open(/mod\.rs/, false);
    const origins = within(await section());
    expect(await origins.findByText("Line origins are not available yet")).toBeTruthy();
    expect(origins.queryByRole("img")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
