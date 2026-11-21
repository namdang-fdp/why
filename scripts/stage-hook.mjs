// Builds `why-hook` (the tiny binary agents run for every hook event) and stages it where Tauri
// bundles it from, as a sidecar ("external binary"):
//
//   node scripts/stage-hook.mjs                       # build for this machine
//   node scripts/stage-hook.mjs --target <triple>     # cross build, e.g. aarch64-apple-darwin
//
// Result: src-tauri/binaries/why-hook-<target-triple>[.exe]  (gitignored; Tauri's sidecar naming).
//
// Why a script and not `bundle.externalBin` in tauri.conf.json: tauri-build refuses to compile the
// app when a declared sidecar file is missing, which would break every plain `cargo build` /
// `cargo test` / `pnpm dev` on a checkout that has not staged the hook. So `externalBin` lives in
// src-tauri/tauri.bundle-hook.conf.json, an overlay that is passed ONLY to bundle builds:
//
//   pnpm build          = this script + `tauri build --config src-tauri/tauri.bundle-hook.conf.json`
//   release workflow    = this script + tauri-action with the same `--config`
//
// At run time the app copies the bundled `why-hook` (next to its executable) to the stable path
// ~/.local/share/why/bin/why-hook (an AppImage mounts somewhere new on every run), see
// src-tauri/src/collector/install.rs.
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const tauriDir = resolve(root, "src-tauri");

const args = process.argv.slice(2);
const at = args.indexOf("--target");
const target = at === -1 ? undefined : args[at + 1];
if (at !== -1 && !target) {
  console.error("stage-hook: --target needs a value");
  process.exit(1);
}

// Argument vectors, never a shell: nothing here is interpolated into a command line.
const run = (cmd, argv) =>
  execFileSync(cmd, argv, { cwd: root, stdio: ["ignore", "pipe", "inherit"] }).toString();

const host = /^host: (.+)$/m.exec(run("rustc", ["-vV"]))?.[1];
const triple = target ?? host;
if (!triple) {
  console.error("stage-hook: cannot tell the target triple from `rustc -vV`");
  process.exit(1);
}

const build = [
  "build",
  "--release",
  "--manifest-path",
  resolve(tauriDir, "Cargo.toml"),
  "-p",
  "why-hook",
];
if (target) build.push("--target", target);
execFileSync("cargo", build, { cwd: root, stdio: "inherit" });

const exe = triple.includes("windows") ? ".exe" : "";
const built = resolve(tauriDir, "target", ...(target ? [target] : []), "release", `why-hook${exe}`);
const staged = resolve(tauriDir, "binaries", `why-hook-${triple}${exe}`);
mkdirSync(resolve(tauriDir, "binaries"), { recursive: true });
copyFileSync(built, staged);
console.log(`stage-hook: ${built} -> ${staged}`);
