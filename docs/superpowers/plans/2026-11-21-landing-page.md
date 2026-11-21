# WHY Landing Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build WHY's static landing page in `site/` (Astro + Tailwind + AnimeJS), with fixture-only screenshots, lean automated checks, and a path-aware CI, ready for the owner to review. Nothing is committed or deployed until the owner says so.

**Architecture:** A standalone Astro project in `site/` (own lockfile, not in any workspace), static output, zero client JS required to read the page. Two tiny client scripts add a theme toggle and scroll reveals (AnimeJS) as progressive enhancement. Checks are Node scripts run from `site/` against `dist/`, driven by `playwright-core` on the system Chrome.

**Tech Stack:** Astro, Tailwind CSS 4 (`@tailwindcss/vite`), `animejs` 4 (per-module imports), `sharp`, Junicode + Bricolage Grotesque (OFL, self-hosted), Prettier + `prettier-plugin-astro`, `@astrojs/check`, `playwright-core` + `@axe-core/playwright`. Pin every version exactly after confirming it with `npm view <pkg> version`.

**Spec:** `docs/superpowers/specs/2026-11-21-landing-page-design.md` (section numbers below refer to it).

## Global Constraints

- Static output, no cookies, no forms, no tracking, no third-party request of any kind (no CDN scripts or fonts, no analytics). Vercel Web Analytics and Speed Insights stay off.
- Works with JavaScript disabled; readable at 320 px; semantic HTML, real headings, alt text, visible focus, contrast AA, `prefers-reduced-motion` respected.
- Do not touch `src-tauri/` or `ui/`. Do not edit `why-brain/`. No new dependency in the root `package.json`.
- The only changes outside `site/`: one `site` entry in `ignores` of root `eslint.config.js`; `.github/workflows/ci.yml` plus `scripts/ci-scope.mjs` and its test (Task 8); this spec and plan.
- Budgets: everything except images 200 KB transferred; JavaScript 15 KB gzip; hero image 250 KB at 2x; all images 600 KB.
- Every screenshot is fixture data only (`/home/dev/...` paths); never the owner's desktop, repos, paths or sessions.
- Copy is honest: line-level provenance, attention signals and closing the loop are "planned", not shipped; no fake numbers, testimonials, "trusted by" or logos; v0.2 uses Observed, Declared and Unknown (Inferred is in the model only, unless Task 1 finds otherwise); "Why?" is always "what the agent said it was doing", labelled Declared.
- Hero headline is "Read the diff. Then ask *why*." Footer has project links only, no personal name. Canonical URL is `https://why.dorriss.com`.
- **Owner's hard rule overrides "frequent commits":** nothing is committed, pushed, stashed or branch-switched until the owner says "commit". Task 10 holds the commit list.
- Report to the owner in Vietnamese, with an explicit "not verified" list.

## Review Focus

Failure modes the spec implies that no feature task would otherwise test. Each is pinned by a scenario in `site/scripts/check-browser.mjs` (Task 7) or `check-version.mjs` (Task 2):

1. `localStorage` / `sessionStorage` throw (private window): the page renders and the toggle still changes theme for the session.
2. The reveal script fails to load or throws: all content is visible within 3 seconds, never stuck hidden.
3. JavaScript off with a dark or light OS preference: all content visible, the hero image matches the scheme.
4. 320 px wide with the long install lines and `sha256sum` command: no horizontal page scroll (code scrolls inside its own block).
5. A release is cut and `release.ts` is left behind: `check:version` fails against the root `package.json` and the README install lines.

---

### Task 1: Pre-flight (read-only): fixtures and labels

**Files:** none changed. Output is a short findings note in the report to the owner.

**Interfaces:** Produces: yes/no answers that Tasks 3 and 5 depend on.

- [ ] **Step 1:** In the worktree run `pnpm i --frozen-lockfile`, then `pnpm dev:ui` (port 1590 from `.env`). Open it in the built-in browser.
- [ ] **Step 2:** Confirm in `ui/src/fixtures/{sessions,events,evidence}.ts` and in the running UI that fixtures contain: a Claude Code session and a Codex session, a changed file marked as explained by agent turns, a "Why?" answer labelled Declared, and a Codex exit code recovered from rollout.
- [ ] **Step 3:** Grep `ui/src` for the evidence labels actually rendered (`Observed`, `Declared`, `Inferred`, `Unknown`) and record which ones v0.2 shows.
- [ ] **Step 4:** Verify: write down the three answers. **If a fixture scene is missing, stop and report it as a blocker** (the UI owner adds it; `ui/` is not ours). If "Inferred" is rendered, update the evidence copy rule in Task 5 and the Global Constraints.

### Task 2: Scaffold `site/`, config, release data, version check

**Files:**
- Create: `site/package.json`, `site/astro.config.mjs`, `site/tsconfig.json`, `site/.prettierrc.json`, `site/.gitignore`, `site/vercel.json`, `site/README.md`, `site/src/data/release.ts`, `site/scripts/check-version.mjs`, `site/src/pages/index.astro` (empty page for now)
- Modify: `eslint.config.js` (root, the `ignores` array)

**Interfaces:**
- Produces: `release` from `site/src/data/release.ts`:
  `export const release = { version: "0.2.0", repo: "https://github.com/namdang-fdp/why", latest: "https://github.com/namdang-fdp/why/releases/latest", appImage: "WHY_0.2.0_amd64.AppImage", deb: "WHY_0.2.0_amd64.deb", sums: "SHA256SUMS", minGit: "2.45.1", license: "MIT OR Apache-2.0", site: "https://why.dorriss.com" } as const;`
- Produces: `pnpm` scripts in `site/package.json`: `dev`, `build`, `check` (`prettier --check . && astro check`), `check:version`, `check:size`, `check:copy`, `check:links`, `check:browser`.

- [ ] **Step 1: Write `check-version.mjs` first** (Node, no deps). It imports the values from `release.ts` by regex-reading the file, and fails (exit 1, one line per mismatch) when `release.version` differs from root `../package.json` `version`, or when the README's `WHY_<version>_amd64.AppImage` / `.deb` names differ from `release.appImage` / `release.deb`.
- [ ] **Step 2:** Verify it fails: with `release.ts` temporarily set to version `0.0.0`, run `pnpm check:version`. Expected: exit 1 naming the mismatch. Restore `0.2.0`; expected: exit 0.
- [ ] **Step 3:** Scaffold the Astro project: static output, `site` from `release.site`, Tailwind through `@tailwindcss/vite`, no Astro integrations, `build.inlineStylesheets: "never"`, `vite.build.assetsInlineLimit: 0` (so the CSP in `vercel.json` needs no `unsafe-inline`). `engines.node` ">=24", `packageManager` the same pnpm major as the root.
- [ ] **Step 4:** `vercel.json` holds only `headers`: the CSP from spec section 8, `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`, and `Cache-Control: public, max-age=31536000, immutable` for `/_astro/*` and `/fonts/*`. No build or deploy settings.
- [ ] **Step 5:** Add `"site"` to `ignores` in root `eslint.config.js`. Verify: from the repo root `pnpm lint:ui` still passes (it must not lint `site/`).
- [ ] **Step 6:** Verify: `cd site && pnpm i && pnpm build && pnpm check` pass. Write `site/README.md` (run, build, each check, how screenshots are made, "deploys are the owner's step").

### Task 3: Capture fixture screenshots and prepare images

**Files:**
- Create (outside the repo): `<why-worktrees>/prompts/WHY-57-capture/capture.mjs` (a folder next to the worktrees, outside the repo) and its own `package.json` with `playwright-core`
- Create: `site/src/assets/shots/hero-dark.png`, `hero-light.png` (+ optional `explain-*.png`, `why-declared-*.png` crops), `site/src/assets/logo.svg` (copy of `assets/brand/logo.svg`)

**Interfaces:** Produces: PNG sources 2880 × 1800 (lossless), each under 800 KB; consumed by `ProductShot.astro` in Task 5.

- [ ] **Step 1:** Write `capture.mjs` per `assets/screenshots/README.md`: UI on `http://localhost:1590`, viewport 1440 × 900, scale 2, pinned clock (`1_700_004_000` s), `colorScheme` dark then light, select the fixture worktree and file that Task 1 identified, open the session timeline in the inspector, wait for the Monaco diff, move the pointer away. System Chrome via `channel: "chrome"`.
- [ ] **Step 2:** Run it for both schemes, write the PNGs into `site/src/assets/shots/`.
- [ ] **Step 3:** Verify privacy: open each PNG and read it; no real name, path or repo appears (only `/home/dev/...` and the sample `why` and `billing-api` repos). A text search is not enough for an image: the visual read is the check.
- [ ] **Step 4:** Verify size: each PNG is under 800 KB (`ls -l`). If larger, reduce the palette losslessly or crop.

### Task 4: Fonts, tokens, base layout, size check

**Files:**
- Create: `site/public/fonts/*.woff2` (Junicode roman and italic, Bricolage 500), `site/public/fonts/OFL-Junicode.txt`, `site/public/fonts/OFL-Bricolage.txt`, `site/src/styles/global.css`, `site/src/layouts/Base.astro`, `site/public/theme-init.js`, `site/public/favicon.svg`, `site/public/robots.txt`, `site/scripts/check-size.mjs`

**Interfaces:**
- Produces: CSS tokens on `:root` and `:root[data-theme="dark"]`/`[data-theme="light"]`: `--bg --text --muted --surface --accent --accent-ink`, plus four evidence tints `--ev-observed --ev-declared --ev-inferred --ev-unknown` (spec section 7).
- Produces: `Base.astro` props `{ title: string; description: string }`; renders `<html lang="en">`, canonical link and Open Graph tags from `release.site`, the head script `<script src="/theme-init.js"></script>`.
- Produces: `check-size.mjs` exit 1 over any budget in Global Constraints, printing a table (HTML, CSS, JS gzip, fonts, images).

- [ ] **Step 1: Write `check-size.mjs` first** against `dist/` and run it on the empty page. Expected: pass with tiny numbers.
- [ ] **Step 2:** Subset Junicode (roman and italic, Latin glyphs used on the page plus punctuation) and Bricolage Grotesque (weight 500, Latin) with `fonttools` (`pyftsubset --flavor=woff2`), commit the `woff2` with license files. Declare `@font-face` with `font-display: swap`; Tailwind `@theme` fonts: display = Junicode, sans = Bricolage, mono = system stack.
- [ ] **Step 3:** Define the tokens: dark default look (`#1c1b19`, `#d9d5c7`) and light paper (`#f4f1e8`, `#2a2926`) as in spec section 7; `@media (prefers-color-scheme: light)` applies light unless `data-theme` says otherwise; muted at about 65% text; one warm accent hue chosen so every text pair is AA (compute contrast; record the ratios in `site/README.md`).
- [ ] **Step 3b:** `theme-init.js` (classic script, no dependencies): in `try/catch`, read `localStorage["why-theme"]` and set `data-theme`; add class `reveal` to `<html>` only when `matchMedia("(prefers-reduced-motion: no-preference)").matches` and `IntersectionObserver` exists, and `setTimeout` 2500 ms removes `reveal` unless `window.__revealReady` is set (so a failed reveal script never leaves content hidden). The hidden start state exists only under `.reveal [data-reveal]`.
- [ ] **Step 4:** Verify: `pnpm build && pnpm check:size` passes; fonts total is within the 200 KB non-image budget with room for the page. If not, shrink the subset, not the budget.

### Task 5: Page sections and copy check

**Files:**
- Create: `site/src/components/{Header,Hero,ProductShot,Problem,Features,Evidence,Safety,Roadmap,Install,Footer}.astro`, `site/scripts/check-copy.mjs`
- Modify: `site/src/pages/index.astro`

**Interfaces:**
- Consumes: `release` (Task 2), `Base.astro` (Task 4), the PNGs (Task 3).
- Produces: `ProductShot.astro` props `{ alt: string }`; builds the hero with `getImage()` (AVIF and WebP, 1x and 2x) into `<picture>` with four `<source>` elements whose `media` is `(prefers-color-scheme: light)` or `(prefers-color-scheme: dark)`; `width="1440" height="900"` on the `<img>`.
- Produces: every block that should reveal carries `data-reveal` (consumed in Task 6). Header theme button has `id="theme-toggle"` and, without JS, is `hidden`.

- [ ] **Step 1: Write `check-copy.mjs` first.** It reads `dist/index.html` text and fails on: any of `trusted by`, `testimonial`, `★`, `stars`, `customers`, `% faster` (case-insensitive); and also fails unless the page contains each of `planned`, `Linux`, `Git 2.45.1`, `SHA256SUMS`, `Observed`, `Declared`, `Unknown`, `MIT OR Apache-2.0`. Run it on the empty page. Expected: FAIL listing the missing phrases.
- [ ] **Step 2:** Write each component with the messages in spec section 6. Copy must come from `README.md`, `CHANGELOG.md` and `SECURITY.md` (open them and reuse their wording; do not paraphrase safety claims upward). Section specifics:
  - Hero: `h1` "Read the diff. Then ask <em>why</em>." (the `em` uses the Junicode italic and the accent); status pill "v0.2.0 · early · Linux only" from `release.version`; buttons Download (`release.latest`) and GitHub (`release.repo`).
  - Features: four items exactly as spec section 6 row 5.
  - Evidence: the four labels with one line each; "Unknown beats a guess" argument; state that v0.2 uses Observed, Declared and Unknown.
  - Safety: the five claims in spec section 3 goal 3, a link to `SECURITY.md` on GitHub and one sentence naming its known limits.
  - Roadmap: v0.3, v0.4, v0.5 each tagged "Planned"; plus the "not yet" list (Linux only, unsigned, Git 2.45.1+, file-level attribution).
  - Install: the AppImage and `.deb` command blocks and the `sha256sum --check --ignore-missing SHA256SUMS` line from the README, built from `release.*`; requirements; links.
  - Footer: repo, changelog, security, license files, issues, one prior-art line linking `why-brain/05%20-%20Research.md` on GitHub. No personal name.
- [ ] **Step 3:** Verify: `pnpm build && pnpm check && pnpm check:copy && pnpm check:size`. Expected: all pass. Then read the built page top to bottom against the README, CHANGELOG and SECURITY once, and list any sentence not backed by them (expect none).

### Task 6: Theme toggle and reveal motion (progressive enhancement)

**Files:**
- Create: `site/src/scripts/theme.ts`, `site/src/scripts/reveal.ts`
- Modify: `site/src/layouts/Base.astro` (load both as module scripts), `site/src/styles/global.css` (hidden start state only under `.reveal`)

**Interfaces:**
- Consumes: `#theme-toggle`, `[data-reveal]`, `window.__revealReady` contract from Task 4.
- Produces: `theme.ts` — on click flips `data-theme` between `dark` and `light`, stores it in `try/catch` storage under `why-theme`, shows the button (removes `hidden`), and rewrites each hero `<source media>` to `all` or `not all` to match the chosen theme.
- Produces: `reveal.ts` — sets `window.__revealReady = true` first, then uses `IntersectionObserver` and `animate` from `animejs` (opacity 0→1, translateY 20→0, optional blur 4px→0, 200–400 ms, once, small stagger for hero words); on any error removes the `reveal` class.

- [ ] **Step 1:** Implement both scripts; import only the needed AnimeJS module (`import { animate } from "animejs"`), and keep the page JS within the 15 KB gzip budget.
- [ ] **Step 2:** Verify: `pnpm build && pnpm check:size` passes; then in the built-in browser toggle theme, scroll through, and confirm the reveal plays once. (Scenario tests are Task 7.)

### Task 7: Browser and link checks

**Files:**
- Create: `site/scripts/check-browser.mjs`, `site/scripts/check-links.mjs`

**Interfaces:**
- Consumes: `dist/`, `vercel.json` headers (the script's tiny static server applies them), system Chrome through `playwright-core` (`channel: "chrome"`; print a clear message and exit 2 if Chrome is missing).
- Produces: `check-browser.mjs` exits 1 on any failure; every scenario also runs `@axe-core/playwright` and fails on any violation (including `color-contrast`) after scrolling the whole page.

- [ ] **Step 1: Write the scenarios first** (they pin the Review Focus), then run them against the Task 6 build:
  1. `jsOff`: `javaScriptEnabled: false`, dark and light schemes: every `h1`/`h2` is visible (computed opacity 1), the hero `currentSrc` filename contains the scheme (`dark` / `light`).
  2. `reducedMotion`: `reducedMotion: "reduce"`: `<html>` has no class `reveal`.
  3. `revealScriptBlocked`: abort the request for the reveal chunk: after 3 s all `[data-reveal]` have opacity 1.
  4. `storageThrows`: `addInitScript` makes `Storage.prototype.getItem/setItem` throw: the page renders and clicking `#theme-toggle` changes `data-theme`.
  5. `noOverflow`: widths 320 and 1440, both schemes: `document.documentElement.scrollWidth <= window.innerWidth`.
  6. `noThirdParty`: every request origin equals the local server origin.
  7. `csp`: no console message or `securitypolicyviolation` event under the `vercel.json` CSP.
- [ ] **Step 2:** Verify they can fail: temporarily add `<img src="https://example.com/x.png">` to the page; expect `noThirdParty` and `csp` to fail; remove it and expect all to pass.
- [ ] **Step 3: `check-links.mjs`:** parse `dist/index.html`; internal `href`/`src` must exist in `dist/`; external links are requested (GET, 10 s timeout, follow redirects) and must return 2xx. Required targets present: releases/latest, repo, CHANGELOG, SECURITY.md, LICENSE-MIT, LICENSE-APACHE, issues, the research note. Run it. Expected: all resolve; a dead link (temporarily added) fails.

### Task 8: Path-aware CI (the owner chose to do it here)

**Files:**
- Create: `scripts/ci-scope.mjs`, `scripts/ci-scope.test.mjs`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: `export function appChanged(paths: string[]): boolean` in `scripts/ci-scope.mjs`: false only when `paths` is non-empty and every path starts with `site/` or `docs/`; true otherwise, including for an empty list.
- Produces: the CLI form `node scripts/ci-scope.mjs` reads `git diff --name-only HEAD^1 HEAD`, prints `app=true|false` to `$GITHUB_OUTPUT`, and prints `app=true` on any error, on a missing `HEAD^1`, and on any `push` event.

- [ ] **Step 1: Write `ci-scope.test.mjs` first** (`node:test`): `appChanged(["site/a.astro","docs/x.md"])` is false; `["site/a.astro","ui/src/App.tsx"]` true; `[".github/workflows/ci.yml"]` true; `["eslint.config.js"]` true; `[]` true; `["sitemap.txt"]` true (prefix is `site/`, not `site`). Run `node --test scripts/ci-scope.test.mjs`. Expected: FAIL (module missing).
- [ ] **Step 2:** Implement `ci-scope.mjs`; run the test. Expected: PASS.
- [ ] **Step 3:** In `ci.yml` keep the job name `Lint and test (Linux)` and the triggers unchanged; set `actions/checkout` `fetch-depth: 2`; add a first step `scope` running the CLI; add `if: steps.scope.outputs.app == 'true'` to every later step. **No `paths` or `paths-ignore` on the workflow** (the ruleset "Protect main" requires this exact check; a skipped workflow would block merges forever).
- [ ] **Step 4:** Verify: `pnpm lint:ui` passes (Prettier covers `scripts`). Say plainly that the workflow itself can only be proven by pushing a PR, and list the three PR scenarios to watch (mixed PR runs everything, a docs-only PR is quick and green, the push to `main` runs everything).

### Task 9: Whole-page verification and report

**Files:** none changed.

- [ ] **Step 1:** Run from `site/`: `pnpm build && pnpm check && pnpm check:version && pnpm check:size && pnpm check:copy && pnpm check:links && pnpm check:browser`. Expected: all pass. Record the size table.
- [ ] **Step 2:** In the built-in browser pane, with `pnpm preview`: desktop 1440, tablet 768, phone 375 and 320; JS off; reduced motion; light and dark; keyboard-only traversal with visible focus. Take page screenshots (of the page only).
- [ ] **Step 3:** Run the `design:accessibility-review` skill on the page and fix findings. Run `pnpm lint:ui` and the `ci-scope` test from the repo root once more.
- [ ] **Step 4:** Report to the owner **in Vietnamese**: what was built and why, screenshots, link-check result, size table, and the "not verified" list (at minimum: not tested on real devices or other browsers than Chrome, not deployed, CI workflow not proven until a PR runs it, nothing committed).

### Task 9b: Changelog and About pages (added at the owner's request, built and verified)

**Files:** `site/src/data/changelog.ts`, `site/src/pages/{changelog,about}.astro`, `site/src/components/Header.astro` (menu, anchors as `/#id`), `Footer.astro`, `Hero.astro`; `check-copy`, `check-links`, `check-browser` extended to all three pages.
**Interfaces:** `loadChangelog(): Promise<ChangelogEntry[]>` with `ChangelogEntry = { version: string; date: string | null; html: string }`, reading `../CHANGELOG.md` relative to the build's working directory (`site/`).
- [x] Extend the three checks to every page first (RED: they failed with the pages missing), then build the pages (GREEN). Verified: copy, links (20 external), browser (jsOff x6, layout x12 with axe) all pass.

### Task 10: Commit and PR (only after the owner says "commit")

**Files:** all of the above.

- [ ] **Step 1:** Stop and wait for the owner's word. Do not commit before it.
- [ ] **Step 2:** On "commit", make small atomic conventional commits, each ending with the `Co-Authored-By:` trailer of the model that wrote it: `docs(site): landing page spec and plan`; `feat(site): scaffold Astro project, config and release data`; `feat(site): fixture screenshots and fonts`; `feat(site): landing page sections and copy`; `feat(site): changelog and about pages`; `feat(site): theme toggle and reveal motion`; `test(site): link, size, copy and browser checks`; `ci: skip app jobs for site-only changes`; and the `eslint.config.js` ignore belongs to the scaffold commit.
- [ ] **Step 3:** Push `feature/WHY-57-landing-page` and open a PR against `main` titled `WHY-57: landing page`. Bind the PR with the ccd_pr tools. Do not merge: merging (with the merge commit `Merge WHY-57: landing page (#PR)`), moving Kaneo WHY-57 to Done, and every Vercel, DNS or domain step are separate steps on the owner's explicit word.
