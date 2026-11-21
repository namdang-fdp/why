// Fails when the page's release data drifts from the app's real version.
// Reads files as text on purpose: no build step is needed to run it.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const here = (path) => fileURLToPath(new URL(path, import.meta.url));
const read = (path) => readFileSync(here(path), "utf8");

const releaseSrc = read("../src/data/release.ts");
const field = (name) => releaseSrc.match(new RegExp(`${name}:\\s*"([^"]+)"`))?.[1];

const errors = [];
const version = field("version");
const rootVersion = JSON.parse(read("../../package.json")).version;
if (version !== rootVersion) {
  errors.push(`release.ts version ${version} != root package.json version ${rootVersion}`);
}

const readme = read("../../README.md");
for (const [key, ext] of [
  ["appImage", "AppImage"],
  ["deb", "deb"],
]) {
  const inReadme = readme.match(new RegExp(`WHY_[\\d.]+_amd64\\.${ext}`))?.[0];
  if (field(key) !== inReadme) {
    errors.push(`release.ts ${key} ${field(key)} != README install line ${inReadme}`);
  }
}

if (errors.length > 0) {
  console.error(errors.join("\n"));
  process.exit(1);
}
console.log(`release data matches v${rootVersion} (package.json and README)`);
