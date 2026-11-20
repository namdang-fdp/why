// Fails (exit 1) when the app's version strings disagree. Run by CI and by the release workflow:
//
//   node scripts/check-versions.mjs                 # all version strings agree
//   node scripts/check-versions.mjs --tag v0.1.0    # ...and equal the tag being released
//
// Four files carry the version, and nothing else keeps them in step:
//   package.json, src-tauri/Cargo.toml, src-tauri/Cargo.lock (the `why` entry) and
//   src-tauri/tauri.conf.json. Tauri names the bundles after tauri.conf.json (WHY_<version>_...),
//   so a stale one would ship a release whose file names disagree with its tag.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const option = (name) => {
  const i = args.indexOf(name);
  return i === -1 ? undefined : args[i + 1];
};

// `--root` lets a test point the check at a temporary copy of the files.
const root = resolve(option("--root") ?? fileURLToPath(new URL("..", import.meta.url)));
const read = (relative) => readFileSync(resolve(root, relative), "utf8");

function fail(message) {
  console.error(`check-versions: ${message}`);
  process.exit(1);
}

// Cargo.toml: the first `version = "..."` inside the [package] table.
function cargoTomlVersion(toml) {
  const pkg = /^\[package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(toml);
  const match = pkg && /^version\s*=\s*"([^"]+)"/m.exec(pkg[1]);
  if (!match) fail("no version found in the [package] table of src-tauri/Cargo.toml");
  return match[1];
}

// Cargo.lock: the `[[package]]` block named `why` (our own crate, not a dependency).
function cargoLockVersion(lock) {
  const match = /^\[\[package\]\]\nname = "why"\nversion = "([^"]+)"/m.exec(lock);
  if (!match) fail('no `why` package found in src-tauri/Cargo.lock (run "cargo metadata")');
  return match[1];
}

const versions = {
  "package.json": JSON.parse(read("package.json")).version,
  "src-tauri/Cargo.toml": cargoTomlVersion(read("src-tauri/Cargo.toml")),
  "src-tauri/Cargo.lock": cargoLockVersion(read("src-tauri/Cargo.lock")),
  "src-tauri/tauri.conf.json": JSON.parse(read("src-tauri/tauri.conf.json")).version,
};

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
const expected = versions["package.json"];
const problems = [];

for (const [file, version] of Object.entries(versions)) {
  if (typeof version !== "string" || !SEMVER.test(version)) {
    problems.push(`${file}: "${version}" is not a SemVer version`);
  } else if (version !== expected) {
    problems.push(`${file}: ${version} (package.json says ${expected})`);
  }
}

const tag = option("--tag");
if (tag !== undefined && tag !== `v${expected}`) {
  problems.push(`tag ${tag} does not match version ${expected} (expected v${expected})`);
}

if (problems.length > 0) {
  fail(`version mismatch\n  ${problems.join("\n  ")}`);
}
console.log(`check-versions: all version strings are ${expected}${tag ? ` (tag ${tag})` : ""}`);
