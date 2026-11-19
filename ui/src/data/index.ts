import type { WorktreeChanged } from "@/contracts";
import type { DataSource } from "./data-source";
import { FixtureDataSource } from "./fixture-data-source";

const fixtures = new FixtureDataSource();
export const dataSource: DataSource = fixtures;

declare global {
  interface Window {
    whyFixtures?: { emitWorktreeChanged: (event: WorktreeChanged) => void };
  }
}

if (import.meta.env.DEV && typeof window !== "undefined") {
  window.whyFixtures = {
    emitWorktreeChanged: (event) => fixtures.emitWorktreeChanged(event),
  };
}
