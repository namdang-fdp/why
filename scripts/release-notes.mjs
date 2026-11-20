// Prints the CHANGELOG.md section for one version to stdout (without its `## [x.y.z]` heading).
// The release workflow uses it as the body of the GitHub release.
//
//   node scripts/release-notes.mjs 0.1.0            # also accepts v0.1.0
//   node scripts/release-notes.mjs 0.1.0 --strict   # release mode, see below
//   node scripts/release-notes.mjs 0.1.0 --changelog path/to/CHANGELOG.md
//
// Exit code 1 (with a message on stderr, and a `::error::` annotation inside GitHub Actions)
// when the section is missing or empty. `--strict` is for a real tagged release and also
// refuses a heading that still has the `YYYY-MM-DD` date placeholder and any leftover `TODO`.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const strict = args.includes("--strict");
const changelogIndex = args.indexOf("--changelog");
const changelogPath =
  changelogIndex === -1
    ? fileURLToPath(new URL("../CHANGELOG.md", import.meta.url))
    : resolve(args[changelogIndex + 1] ?? "");
const positional = args.filter(
  (arg, i) => !arg.startsWith("--") && (changelogIndex === -1 || i !== changelogIndex + 1),
);
const version = (positional[0] ?? "").replace(/^v/, "");

function fail(message) {
  // `::error::` makes GitHub show the message on the workflow summary, not just in the log.
  if (process.env.GITHUB_ACTIONS) console.error(`::error title=Release notes::${message}`);
  console.error(`release-notes: ${message}`);
  process.exit(1);
}

if (!/^\d+\.\d+\.\d+/.test(version)) {
  fail("usage: release-notes.mjs <version> [--strict] [--changelog <path>]");
}

const lines = readFileSync(changelogPath, "utf8").split(/\r?\n/);
const escaped = version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const heading = new RegExp(`^## \\[${escaped}\\](?:\\s+-\\s+(.+?))?\\s*$`);

const start = lines.findIndex((line) => heading.test(line));
if (start === -1) {
  fail(`CHANGELOG.md has no "## [${version}]" section; add it before tagging v${version}`);
}

// The section runs to the next `## ` heading or to the link definitions at the bottom of the
// file (`[0.1.0]: https://...`), which belong to the whole file and not to this version.
let end = lines.length;
for (let i = start + 1; i < lines.length; i++) {
  if (/^## /.test(lines[i]) || /^\[[^\]]+\]:\s/.test(lines[i])) {
    end = i;
    break;
  }
}
const body = lines
  .slice(start + 1, end)
  .join("\n")
  .trim();

if (body === "") {
  fail(`the "## [${version}]" section of CHANGELOG.md is empty`);
}

if (strict) {
  const date = heading.exec(lines[start])?.[1] ?? "";
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) {
    fail(`the "## [${version}]" heading needs a real release date (found "${date || "none"}")`);
  }
  const todo = body.split("\n").find((line) => /\bTODO\b/.test(line));
  if (todo) {
    fail(`the "## [${version}]" section still has a TODO: ${todo.trim()}`);
  }
}

process.stdout.write(`${body}\n`);
