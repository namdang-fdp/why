import { isTauri } from "@tauri-apps/api/core";
import type { WorktreeChanged } from "@/contracts";
import type { DataSource } from "./data-source";
import { FixtureDataSource, sampleFilters, type FixtureOptions } from "./fixture-data-source";
import { TauriDataSource } from "./tauri-data-source";

/**
 * Dev-only knobs for looking at states by hand in a plain browser, e.g.
 * `http://localhost:1510/?empty=1&storage=temporary&latency=400`.
 */
function fixtureOptionsFromUrl(): FixtureOptions {
  if (!import.meta.env.DEV || typeof window === "undefined") return {};
  const params = new URLSearchParams(window.location.search);
  const latency = Number(params.get("latency"));
  return {
    empty: params.get("empty") === "1",
    many: params.get("many") === "1",
    latencyMs: Number.isFinite(latency) ? latency : 0,
    // `?filters=1` gives each sample repository a different filter-driver state:
    // `why` is blocked (its own .git/config defines one), `billing-api` only uses
    // the user's git-lfs.
    ...(params.get("filters") === "1" && { filters: sampleFilters }),
    ...(params.get("storage") === "temporary" && {
      storage: {
        persistent: false,
        path: null,
        reason: "cannot create /home/dev/.local/share/why: permission denied",
      },
    }),
  };
}

declare global {
  interface Window {
    whyFixtures?: {
      emitWorktreeChanged: (event: WorktreeChanged) => void;
      simulateSessionActivity: (sessionId: string, summary?: string, newPrompt?: string) => boolean;
      simulateEdit: (worktreeId: string, path: string) => void;
      simulateWorktreeAdded: (repoId: string) => void;
      simulateWorktreeRemoved: (worktreeId: string) => void;
    };
  }
}

function createDataSource(): DataSource {
  // Opt into fixtures in Tauri dev to review the UI before the collector is ready.
  const preview = import.meta.env.DEV && import.meta.env.VITE_WHY_FIXTURES === "1";
  if (isTauri() && !preview) return new TauriDataSource();

  const fixtures = new FixtureDataSource(fixtureOptionsFromUrl());
  if (import.meta.env.DEV && typeof window !== "undefined") {
    window.whyFixtures = {
      emitWorktreeChanged: (event) => fixtures.emitWorktreeChanged(event),
      simulateSessionActivity: (sessionId, summary, newPrompt) =>
        fixtures.simulateSessionActivity(sessionId, summary, newPrompt),
      simulateEdit: (worktreeId, path) => fixtures.simulateEdit(worktreeId, path),
      simulateWorktreeAdded: (repoId) => void fixtures.simulateWorktreeAdded(repoId),
      simulateWorktreeRemoved: (worktreeId) => fixtures.simulateWorktreeRemoved(worktreeId),
    };
  }
  return fixtures;
}

export const dataSource: DataSource = createDataSource();
