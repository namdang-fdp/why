// Single source of truth for the dev-server port. Used by Vite (ui/vite.config.ts)
// and by the `pnpm dev` launcher (scripts/dev.mjs), so they can never disagree.
// Only erasable TypeScript syntax here: Node runs this file directly.
import { existsSync } from "node:fs";

const DEFAULT_PORT = 1420;

/** Load `.env` from the current directory into process.env (shell env wins). */
export function loadEnvFile(): void {
  if (existsSync(".env")) process.loadEnvFile(".env");
}

/** Dev server port from `WHY_DEV_PORT`, default 1420. */
export function devPort(): number {
  loadEnvFile();
  const raw = process.env.WHY_DEV_PORT;
  if (raw === undefined || raw === "") return DEFAULT_PORT;
  const port = Number(raw);
  if (!Number.isInteger(port) || port < 1024 || port > 65535) {
    throw new Error(`WHY_DEV_PORT must be an integer in 1024..65535, got "${raw}"`);
  }
  return port;
}
