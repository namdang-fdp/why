// Checks every link on every built page: internal files and #anchors must exist, external links
// must answer 2xx (this needs network access). The links each page must carry are listed below.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const dist = fileURLToPath(new URL("../dist", import.meta.url));
const base = "https://github.com/namdang-fdp/why";
// The site's own canonical URL only resolves after the owner deploys it, so it is skipped.
const siteOrigin = readFileSync(
  fileURLToPath(new URL("../src/data/release.ts", import.meta.url)),
  "utf8",
).match(/site:\s*"([^"]+)"/)?.[1];

const pages = {
  "index.html": [
    `${base}/releases/latest`,
    base,
    `${base}/blob/main/SECURITY.md`,
    `${base}/blob/main/LICENSE-MIT`,
    `${base}/blob/main/LICENSE-APACHE`,
    `${base}/issues`,
    `${base}/blob/main/why-brain/05%20-%20Research.md`,
    `${base}/releases/latest/download/SHA256SUMS`,
    "/about/",
    "/changelog/",
  ],
  "about/index.html": [base, "/changelog/", `${base}/blob/main/SECURITY.md`],
  "changelog/index.html": [
    `${base}/blob/main/CHANGELOG.md`,
    `${base}/releases/tag/v0.3.0`,
    `${base}/releases/tag/v0.2.1`,
    `${base}/releases/tag/v0.2.0`,
    `${base}/releases/tag/v0.1.0`,
  ],
};

const read = (file) => readFileSync(join(dist, file), "utf8");
const idsOf = (file) => new Set([...read(file).matchAll(/\sid="([^"]+)"/g)].map((m) => m[1]));
const fileFor = (path) => (path.endsWith("/") ? `${path}index.html`.slice(1) : path.slice(1));

const failures = [];
const answers = new Map(); // external URL -> status text, so each is requested once

async function request(url) {
  // GET, not HEAD: some hosts answer HEAD differently. Redirects are followed.
  const response = await fetch(url, {
    redirect: "follow",
    signal: AbortSignal.timeout(10000),
    headers: { "user-agent": "link-check" },
  });
  await response.body?.cancel();
  return response.status;
}

let total = 0;
let skipped = 0;
for (const [file, required] of Object.entries(pages)) {
  const html = read(file);
  const urls = new Set([...html.matchAll(/\s(?:href|src)="([^"]+)"/g)].map((m) => m[1]));
  total += urls.size;
  for (const url of required) {
    if (!urls.has(url)) failures.push(`[${file}] missing required link: ${url}`);
  }
  const ownIds = idsOf(file);
  for (const url of urls) {
    if (url.startsWith("#")) {
      if (!ownIds.has(url.slice(1))) failures.push(`[${file}] broken anchor: ${url}`);
    } else if (siteOrigin && url.startsWith(siteOrigin)) {
      skipped += 1;
    } else if (/^https?:\/\//.test(url)) {
      if (!answers.has(url)) {
        try {
          const status = await request(url);
          answers.set(url, status >= 200 && status < 300 ? "ok" : `${status}`);
        } catch (error) {
          answers.set(url, `unreachable (${error.message})`);
        }
      }
      if (answers.get(url) !== "ok") failures.push(`[${file}] ${answers.get(url)} ${url}`);
    } else if (url.startsWith("/")) {
      const [path, hash] = url.split("#");
      const target = fileFor(path.split("?")[0] || "/");
      if (!existsSync(join(dist, target))) failures.push(`[${file}] missing file for ${url}`);
      else if (hash && !idsOf(target).has(hash)) failures.push(`[${file}] broken anchor: ${url}`);
    }
  }
}

if (failures.length > 0) {
  console.error(failures.join("\n"));
  process.exit(1);
}
console.log(
  `links ok on ${Object.keys(pages).length} pages: ${total} total, ${answers.size} external requested, required links present, ${skipped} self link(s) skipped until deployed`,
);
