// Drives the built page (dist/) in the system Chrome through playwright-core and fails on any
// broken promise from the design spec: it must work with JS off, honour reduced motion, never
// stay hidden if a script fails, survive throwing storage, not overflow at 320 px, make no
// third-party request, obey its own CSP, and pass axe in both colour schemes.
//
// The static server applies the headers from vercel.json, so the CSP checked here is the one
// that will ship.
import AxeBuilder from "@axe-core/playwright";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const dist = fileURLToPath(new URL("../dist", import.meta.url));
const vercel = JSON.parse(readFileSync(fileURLToPath(new URL("../vercel.json", import.meta.url))));
const types = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css",
  ".js": "text/javascript",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".avif": "image/avif",
  ".webp": "image/webp",
  ".png": "image/png",
  ".txt": "text/plain",
};

// Minimal static server + the headers rules from vercel.json (`source` is a path pattern whose
// only wildcard is `(.*)`, which is also valid regex syntax).
const server = createServer((req, res) => {
  const path = new URL(req.url, "http://x").pathname;
  let file = normalize(join(dist, path.endsWith("/") ? `${path}index.html` : path));
  if (!file.startsWith(dist)) return res.writeHead(403).end();
  try {
    const body = readFileSync(file);
    for (const rule of vercel.headers) {
      if (new RegExp(`^${rule.source}$`).test(path)) {
        for (const { key, value } of rule.headers) res.setHeader(key, value);
      }
    }
    res.setHeader("Content-Type", types[extname(file)] ?? "application/octet-stream");
    res.end(body);
  } catch {
    res.writeHead(404).end("not found");
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

let browser;
try {
  browser = await chromium.launch({ channel: "chrome" });
} catch (error) {
  console.error(`Could not start the system Chrome (${error.message.split("\n")[0]}).`);
  console.error("Install Google Chrome, or run this check on a machine that has it.");
  server.close();
  process.exit(2);
}

const failures = [];
const fail = (scenario, message) => failures.push(`[${scenario}] ${message}`);

/** Opens the page and records requests and CSP violations for the final checks. */
async function open(name, options = {}, prepare) {
  const { block, delay, waitUntil = "load", path = "/", ...contextOptions } = options;
  const context = await browser.newContext({
    viewport: { width: 1280, height: 800 },
    ...contextOptions,
  });
  const page = await context.newPage();
  const seen = { origins: new Set(), csp: [] };
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (url.protocol === "http:" || url.protocol === "https:") seen.origins.add(url.origin);
  });
  page.on("console", (message) => {
    if (/content security policy/i.test(message.text())) seen.csp.push(message.text());
  });
  page.on("pageerror", (error) => fail(name, `page error: ${error.message}`));
  if (block) await page.route(block, (route) => route.abort());
  if (delay) {
    // A slow network: the scripts arrive late instead of never.
    await page.route(delay.pattern, async (route) => {
      await new Promise((resolve) => setTimeout(resolve, delay.ms));
      await route.continue();
    });
  }
  await context.addInitScript(() => {
    window.__csp = [];
    document.addEventListener("securitypolicyviolation", (e) =>
      window.__csp.push(e.violatedDirective),
    );
  });
  if (prepare) await prepare(context);
  await page.goto(origin + path, { waitUntil });
  return { page, context, seen };
}

async function finish(name, { page, context, seen }, { axe = true } = {}) {
  if (axe) {
    const { violations } = await new AxeBuilder({ page }).analyze();
    for (const v of violations) {
      fail(name, `axe ${v.id} (${v.impact}): ${v.nodes.length} node(s), e.g. ${v.nodes[0].target}`);
    }
  }
  for (const o of seen.origins) if (o !== origin) fail(name, `third-party request to ${o}`);
  for (const message of seen.csp) fail(name, `CSP violation: ${message}`);
  const inPage = await page.evaluate(() => window.__csp ?? []).catch(() => []);
  for (const directive of inPage) fail(name, `securitypolicyviolation: ${directive}`);
  await context.close();
}

/** Scrolls the whole page so every reveal fires, then waits for them to finish. */
async function revealAll(page) {
  await page.evaluate(async () => {
    const step = 400;
    for (let y = 0; y <= document.body.scrollHeight; y += step) {
      window.scrollTo({ top: y, behavior: "instant" });
      await new Promise((r) => setTimeout(r, 50));
    }
  });
  await page.waitForFunction(
    () =>
      !document.documentElement.classList.contains("reveal") ||
      [...document.querySelectorAll("[data-reveal]")].every((e) =>
        e.classList.contains("is-revealed"),
      ),
    undefined,
    { timeout: 10000 },
  );
  await page.evaluate(() => window.scrollTo({ top: 0, behavior: "instant" }));
}

const allVisible = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll("[data-reveal], h1, h2")].every(
      (e) => Number(getComputedStyle(e).opacity) === 1,
    ),
  );

// The pages that exist. Heading counts are minimums, so a page cannot silently lose its content.
const pages = [
  { path: "/", id: "home", minHeadings: 8, hero: true },
  { path: "/about/", id: "about", minHeadings: 5 },
  { path: "/changelog/", id: "changelog", minHeadings: 3 },
];

// 1. JavaScript off, every page, both colour schemes: everything is there.
for (const { path, id, minHeadings, hero } of pages
  .flatMap((p) => ["dark", "light"].map((scheme) => ({ ...p, scheme })))
  .map((p) => ({ ...p, id: `${p.id}/${p.scheme}` }))) {
  const scheme = id.split("/")[1];
  const name = `jsOff/${id}`;
  const ctx = await open(name, { javaScriptEnabled: false, colorScheme: scheme, path });
  const { page } = ctx;
  // The hero's CSS entrance runs without JavaScript too; give it time to finish (about 1.1 s).
  await page.waitForTimeout(1500);
  if (!(await allVisible(page))) fail(name, "some content is hidden without JavaScript");
  const headings = await page.locator("h1, h2").count();
  if (headings < minHeadings)
    fail(name, `expected at least ${minHeadings} headings, found ${headings}`);
  if (hero) {
    const imageOk = await page
      .locator("figure img")
      .evaluate((img) => img.complete && img.naturalWidth > 0);
    if (!imageOk) fail(name, "hero image did not load");
  }
  const toggleHidden = await page.locator("#theme-toggle").isHidden();
  if (!toggleHidden) fail(name, "theme toggle should stay hidden without JavaScript");
  // axe itself needs JavaScript, so this scenario cannot run it; the layout scenarios cover contrast.
  await finish(name, ctx, { axe: false });
}

// 2. Reduced motion: the hidden start state must never be switched on.
{
  const name = "reducedMotion";
  const ctx = await open(name, { reducedMotion: "reduce" });
  if (await ctx.page.evaluate(() => document.documentElement.classList.contains("reveal"))) {
    fail(name, "html.reveal is set although the user prefers reduced motion");
  }
  if (!(await allVisible(ctx.page))) fail(name, "content hidden under reduced motion");
  await finish(name, ctx, { axe: false });
}

// 3. The reveal script is blocked: content must be visible within 3 s.
{
  const name = "revealScriptBlocked";
  const ctx = await open(name, { block: "**/_astro/*.js" });
  await ctx.page.waitForTimeout(3200);
  if (await ctx.page.evaluate(() => document.documentElement.classList.contains("reveal"))) {
    fail(name, "html.reveal still set 3 s after the reveal script failed to load");
  }
  if (!(await allVisible(ctx.page)))
    fail(name, "content still hidden after the reveal script failed");
  await finish(name, ctx, { axe: false });
}

// 3b. The scripts are slow (not blocked): the hero must not wait for them.
{
  const name = "slowScript";
  const ctx = await open(name, {
    delay: { pattern: "**/_astro/*.js", ms: 2500 },
    waitUntil: "commit",
  });
  await ctx.page.waitForTimeout(1600);
  const hidden = await ctx.page.evaluate(() =>
    [
      ...document.querySelectorAll(
        "section:first-of-type [data-reveal], section:first-of-type h1, figure",
      ),
    ]
      .filter((e) => Number(getComputedStyle(e).opacity) < 1)
      .map((e) => e.tagName),
  );
  if (hidden.length > 0) fail(name, `hero still hidden 1.6 s in while scripts are slow: ${hidden}`);
  await finish(name, ctx, { axe: false });
}

// 3c. Printing: nothing may print blank because a reveal never ran.
{
  const name = "print";
  const ctx = await open(name);
  await ctx.page.emulateMedia({ media: "print" });
  const faded = await ctx.page.evaluate(
    () =>
      [...document.querySelectorAll("[data-reveal]")].filter(
        (e) => Number(getComputedStyle(e).opacity) < 1,
      ).length,
  );
  if (faded > 0) fail(name, `${faded} element(s) would print faded or blank`);
  await finish(name, ctx, { axe: false });
}

// 4. Storage throws: the page renders and the toggle still works for the visit.
{
  const name = "storageThrows";
  const ctx = await open(name, { colorScheme: "dark" }, async (context) => {
    await context.addInitScript(() => {
      const boom = () => {
        throw new DOMException("blocked", "SecurityError");
      };
      Storage.prototype.getItem = boom;
      Storage.prototype.setItem = boom;
    });
  });
  const { page } = ctx;
  const toggle = page.locator("#theme-toggle");
  if (await toggle.isHidden()) fail(name, "theme toggle not shown with JavaScript on");
  else {
    const before = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    await toggle.click();
    const theme = await page.evaluate(() => document.documentElement.dataset.theme);
    const after = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    if (theme !== "light") fail(name, `data-theme is ${theme}, expected light after one click`);
    if (before === after) fail(name, "background did not change after toggling the theme");
  }
  await finish(name, ctx, { axe: false });
}

// 5. No horizontal page scroll and no axe violations at 320 and 1440 px, both schemes.
for (const { path, id } of pages)
  for (const width of [320, 1440]) {
    for (const scheme of ["dark", "light"]) {
      const name = `layout/${id}/${width}px/${scheme}`;
      const ctx = await open(name, { viewport: { width, height: 900 }, colorScheme: scheme, path });
      await revealAll(ctx.page);
      const overflow = await ctx.page.evaluate(
        () => document.documentElement.scrollWidth - window.innerWidth,
      );
      if (overflow > 0) fail(name, `page scrolls sideways by ${overflow}px`);
      // A scrollable block needs a role for its accessible name to be announced.
      const unnamed = await ctx.page.evaluate(
        () =>
          [...document.querySelectorAll("pre[tabindex]")].filter(
            (e) => e.getAttribute("role") !== "region",
          ).length,
      );
      if (unnamed > 0) fail(name, `${unnamed} scrollable <pre> without role="region"`);
      await finish(name, ctx);
    }
  }

await browser.close();
server.close();
if (failures.length > 0) {
  console.error(failures.join("\n"));
  console.error(`\n${failures.length} failure(s)`);
  process.exit(1);
}
console.log(
  "browser checks passed: jsOff x6, reducedMotion, revealScriptBlocked, slowScript, print, storageThrows, layout x12 (3 pages)",
);
