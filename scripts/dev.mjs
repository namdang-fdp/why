// `pnpm dev`: run `tauri dev` with the dev URL pointing at WHY_DEV_PORT.
// tauri.conf.json holds a static devUrl (1420); `--config` merges a JSON patch
// over it, so each worktree can pick its own port.
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { devPort } from "../ui/dev-port.ts";

const port = devPort();
// Exported so the `beforeDevCommand` (Vite) child process sees the same value.
process.env.WHY_DEV_PORT = String(port);
const configPatch = JSON.stringify({ build: { devUrl: `http://localhost:${port}` } });

// Run the local Tauri CLI through Node itself: no shell, no `pnpm`, and it works
// with Windows `.cmd` shims too.
const tauriCli = createRequire(import.meta.url).resolve("@tauri-apps/cli/tauri.js");
const isWindows = process.platform === "win32";

console.log(`[why] dev server port ${port}`);
const child = spawn(
  process.execPath,
  [tauriCli, "dev", "--config", configPatch, ...process.argv.slice(2)],
  // On POSIX the child leads its own process group so one `kill(-pid)` reaches
  // the whole tree (Tauri CLI, Vite, the app).
  { stdio: "inherit", detached: !isWindows },
);

let stopping = false;
function stopTree(signal) {
  if (stopping || child.exitCode !== null) return;
  stopping = true;
  try {
    if (isWindows) {
      // No process groups on Windows: `taskkill /T` ends the child and its descendants.
      spawn("taskkill", ["/pid", String(child.pid), "/T", "/F"], { stdio: "ignore" });
    } else {
      process.kill(-child.pid, signal);
    }
  } catch {
    // Already gone.
  }
}

for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => stopTree(signal));
// Ctrl+C in a terminal signals the whole foreground group, which now excludes the
// detached child, so the handler above is what stops it.
child.on("exit", (code) => process.exit(code ?? 1));
