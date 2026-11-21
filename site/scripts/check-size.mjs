// Fails when dist/ is over the budgets in the design spec (section 3).
// "Transferred" is approximated by gzip for text files and the raw size for already-compressed
// files (fonts, images), which is what a CDN would send.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const dist = fileURLToPath(new URL("../dist", import.meta.url));
const KB = 1024;
const budgets = { nonImage: 200 * KB, js: 15 * KB, heroImage: 250 * KB, images: 600 * KB };

const imageExt = new Set([".png", ".jpg", ".jpeg", ".webp", ".avif", ".gif"]);
const textExt = new Set([".html", ".css", ".js", ".mjs", ".svg", ".json", ".txt", ".xml"]);

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* walk(path);
    else yield path;
  }
}

const totals = { html: 0, css: 0, js: 0, fonts: 0, otherText: 0, images: 0 };
let biggestHero = { path: "", size: 0 };
for (const file of walk(dist)) {
  const ext = extname(file);
  const raw = readFileSync(file);
  const rel = relative(dist, file);
  if (imageExt.has(ext)) {
    totals.images += raw.length;
    if (/hero/i.test(rel) && raw.length > biggestHero.size) {
      biggestHero = { path: rel, size: raw.length };
    }
  } else if (ext === ".woff2") totals.fonts += raw.length;
  else if (textExt.has(ext)) {
    const size = gzipSync(raw).length;
    if (ext === ".html") totals.html += size;
    else if (ext === ".css") totals.css += size;
    else if (ext === ".js" || ext === ".mjs") totals.js += size;
    else totals.otherText += size;
  }
}

const nonImage = totals.html + totals.css + totals.js + totals.fonts + totals.otherText;
const rows = [
  ["HTML (gzip)", totals.html],
  ["CSS (gzip)", totals.css],
  ["JS (gzip)", totals.js, budgets.js],
  ["Fonts", totals.fonts],
  ["Other text (gzip)", totals.otherText],
  ["Everything except images", nonImage, budgets.nonImage],
  ["Largest hero image", biggestHero.size, budgets.heroImage],
  ["All images", totals.images, budgets.images],
];
const errors = [];
for (const [label, size, budget] of rows) {
  const kb = (size / KB).toFixed(1).padStart(7);
  const limit = budget ? ` / ${(budget / KB).toFixed(0)} KB` : "";
  const over = budget && size > budget;
  if (over) errors.push(label);
  console.log(`${label.padEnd(26)} ${kb} KB${limit}${over ? "  OVER" : ""}`);
}
if (errors.length > 0) {
  console.error(`over budget: ${errors.join(", ")}`);
  process.exit(1);
}
