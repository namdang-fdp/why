# WHY landing page: design (WHY-57)

Status: **built, awaiting the owner's review** (2026-11-21). Nothing is committed or published.
Branch `feature/WHY-57-landing-page`, worktree `why-worktrees/WHY-57-landing-page`, dev port 1590.

## 1. Intent

WHY v0.2.0 is public (Linux AppImage and `.deb`) but has no front door: the README is the only
place that explains it. The landing page is that front door for people who review code written by
coding agents. It must make one claim well, "`git diff` tells you what, not why", then show what WHY
does **today**, what it does not do yet, and why it is safe to run next to your agents.

The owner's reference is the Zen Browser site (`zen-browser.app`): calm, clean, warm, a big serif
headline with one italic accent word, a large rounded product image, short sections, quiet motion.
The page should have that **feeling**, with WHY's own identity and content.

## 2. Goals, non-goals, anti-goals

**Goals**

- A reader understands in 10 seconds: what WHY is, who it is for, that it is early and Linux only.
- A reader who is a developer can install it in under a minute from the page (links and the two
  install lines from the README, plus `SHA256SUMS`).
- Trust is the selling point: passive observer, no network, no telemetry, opt-in hooks with an exact
  preview and a restorable uninstall, fail-open hook, never reads agent credentials.
- Every claim on the page is traceable to `README.md`, `CHANGELOG.md` or `SECURITY.md`.

**Non-goals**

- No blog, docs site, pricing, sign-up, newsletter, comments or search. (Added later at the owner's
  request: `/changelog` and `/about`, see section 6b. The changelog is rendered from the root
  `CHANGELOG.md`, never copied.)
- No i18n (English only, like the README), no CMS.
- No deployment, domain or Vercel configuration in this task's build step (see section 9).

**Anti-goals (a failure even if it "works")**

- Any claim that is not shipped: line-level provenance, attention signals and closing the loop are
  **planned** (v0.3 to v0.5). No fake numbers, testimonials, "trusted by", logos or star counts.
- Looking like a copy of Zen: no Zen logo, artwork, copy, or copied source files.
- Blank or broken page with JavaScript disabled.
- Any third-party request: CDN scripts or fonts, analytics, trackers, cookies, embeds, forms.
- Any screenshot or text from the owner's real machine (paths, repos, sessions, usernames).

## 3. Constraints

Hard, from the brief and `AGENTS.md`:

- Static output, no cookies, no forms, no tracking. Works with JS disabled. Readable at 320 px.
- Semantic HTML, real headings, alt text, visible focus, contrast AA, `prefers-reduced-motion`
  respected.
- Fonts and assets self-hosted. Images optimized with `width` and `height` set.
- Fixture data only in every screenshot. The repo is public.
- Do not touch `src-tauri/` or `ui/`. Do not edit `why-brain/` without asking.
- No new dependencies in the app's root `package.json`.
- Honest copy: match README, CHANGELOG and SECURITY; promise nothing they do not.

Budgets (my calls, to be measured, not assumed):

| What                                              | Budget                                    |
| ------------------------------------------------- | ----------------------------------------- |
| Everything except images (HTML, CSS, JS, fonts)   | 200 KB transferred (the brief's target)   |
| JavaScript                                        | 15 KB gzip, none required to read the page |
| Hero image                                        | 250 KB at 2x, a smaller one for phones    |
| All images together                               | 600 KB                                    |
| Lighthouse (desktop and mobile)                   | Performance, Accessibility, Best Practices, SEO all 95 or more (informational, not a gate) |

## 4. Decisions the owner already made

| Topic        | Decision                                                                                       |
| ------------ | ---------------------------------------------------------------------------------------------- |
| Reference    | Zen Browser's look and feel: clean, calm                                                       |
| Stack        | Astro + Tailwind CSS + AnimeJS                                                                 |
| Hosting      | Vercel; the owner points the domain                                                            |
| Identity     | Zen's feeling, WHY's own identity (not a clone)                                                |
| Fonts        | Junicode (display, italic accent) + Bricolage Grotesque (body). Both SIL OFL 1.1, verified     |
| Animation    | AnimeJS as progressive enhancement: content always visible without JS                          |
| Hero visual  | Screenshot of WHY v0.2 on fixture data, in a rounded window frame                              |
| Theme        | Light and dark, follows the OS, plus a toggle                                                  |
| Checks       | Lean: Prettier + `astro check`, plus scripts for link check, size budget and axe               |
| Spec place   | This worktree, uncommitted until the owner says "commit"                                       |
| Domain       | `why.dorriss.com` (owner's idea, my recommendation agreed; see section 10)                      |
| Hero         | "Read the diff. Then ask *why*."                                                               |
| Footer       | Project links only, no personal name                                                           |
| CI           | Path-aware `ci.yml` in this task, keeping the required check green (section 8)                 |

## 5. Approaches considered

How to build it (the look is decided above):

1. **Astro static site in a standalone `site/` package (chosen, and the owner's stack).** Zero JS by
   default, Tailwind 4 through its Vite plugin, AnimeJS imported per module and only for reveal
   effects. Own `package.json` and lockfile, not part of the app's pnpm install.
   Trade-off: a second Node project in the repo, and a small amount of tooling outside the app's
   `pnpm lint`.
2. **Plain HTML + CSS, no build.** Smallest and most durable, but loses Tailwind and component reuse
   and does not match the owner's chosen stack. Rejected.
3. **Astro inside the app's pnpm workspace.** One install, but it changes the app's lockfile and the
   app's CI install, which the brief says to avoid. Rejected.

What Zen does that we deliberately do **not** copy:

- React integration (Zen adds React islands). We need none: the page is static and the one toggle
  is a few lines of script.
- Hero text hidden until JS runs (Zen starts it at `opacity: 0.001`). Ours is visible by default; JS
  only adds the reveal when it is available and motion is allowed.
- Dozens of dev dependencies and a Cloudflare adapter. We ship static files only.

## 6. The page

One page, English, sections in this order. Each has one message.

| #   | Section                | One-line message                                                                                   |
| --- | ---------------------- | -------------------------------------------------------------------------------------------------- |
| 1   | Header                 | Wordmark, anchor links (Problem, What it does, Evidence, Safety, Get it), GitHub, theme toggle, Download |
| 2   | Hero                   | The claim, in one line, with an italic accent word; a status pill "v0.2.0 · early · Linux only"; Download and GitHub buttons |
| 3   | Product image          | The real UI on fixture data: worktree rail, diff, session timeline in the inspector                |
| 4   | The problem            | Agents write fast; review is the bottleneck; `git diff` shows what, not the task, the reason or the drift; people paste diffs back into chat |
| 5   | What WHY does today    | Four short items: live diff of every repo and worktree; what Claude Code and Codex did, turn by turn; which turns explain each changed file; "Why?" from the agent's own transcript (Declared) with Codex exit codes recovered from its log |
| 6   | Evidence, not guesses  | Observed, Declared, Inferred, Unknown, and why "Unknown" beats a confident guess                  |
| 7   | Safe next to your agents | Passive observer, no network, no telemetry, opt-in hooks with an exact preview and restorable uninstall, fail-open, never reads agent credentials. Link to `SECURITY.md`, including its known limits |
| 8   | What it is not, yet    | Planned, not shipped: v0.3 line provenance, v0.4 attention, v0.5 close the loop. Linux only, unsigned builds, needs Git 2.45.1+, file-level attribution only |
| 9   | Get it                 | Latest release link; AppImage and `.deb` lines; `SHA256SUMS` check; requirements; links to repo, changelog, license (MIT OR Apache-2.0) |
| 10  | Footer                 | Repo, changelog, security, license, issues; one fair line on prior art with a link to the research note |

Honesty rules for copy (checked in the build task, listed in section 11):

- "Inferred" is part of the evidence model but v0.2 itself uses Observed, Declared and Unknown. The
  copy says so. **Fact-check against the v0.2 UI before writing section 6.**
- "Why?" is always "what the agent said it was doing", labelled Declared, never "the reason".
- Roadmap items use the words "planned" and "not shipped"; no dates.
- Prior art is named fairly (README and `why-brain/05 - Research.md`); the pitch is the combination:
  harness-agnostic, many repos and worktrees in one place, evidence-backed "why".

### Hero copy options (the owner picks one; italic word in `*...*`)

- **A.** "Your agents write. You ask *why*." Subcopy: "WHY shows the live diff of every repo and
  worktree your coding agents touch, and what they did, turn by turn."
- **B.** "Because `git diff` tells you *what*, not why." (README tagline; the most literal.)
- **C.** "Read the diff. Then ask *why*." (Ties to the real "Why?" action; calmest.)

**Chosen by the owner: C** for the headline, with the README tagline as the section 4 heading. Subcopy for any
option stays honest: "A local-first desktop app for reviewing code your coding agents wrote.
Read-only, early, Linux only."

### 6b. Two more pages (added at the owner's request, 2026-11-21)

- **`/changelog/`**: rendered at build time from the repo's own `CHANGELOG.md` with
  `@astrojs/markdown-remark` (no syntax highlighting, because it writes inline styles that the CSP
  forbids). One `<article>` per released version, newest first, a jump list, a link to each GitHub
  release. Relative links in the file are rewritten to GitHub; an empty "Unreleased" section is
  hidden; the build fails if no released version is found. Source of truth stays one file.
- **`/about/`**: why WHY exists, what it is and is not, five principles, a fair "not the first tool
  in this space" list taken from `why-brain/05` (with a note that it may be out of date), and "who
  is behind it" as "maintained by one person in their spare time" with no name.
- The header gains About and Changelog, with a no-JS `<details>` menu below `md`; the footer links
  both. Every `check:*` script now covers all pages.
- Deployment note: the changelog page reads `../CHANGELOG.md`, so the Vercel project must be
  allowed to see files outside the root directory `site` (Vercel's default "include source files
  outside of the Root Directory" setting).

## 7. Visual direction

Zen's calm in WHY's own colors. The product look in `why-brain/01` (Linear × Vercel, monochrome,
color only for diff, status and risk) governs the **app**; the page borrows its restraint (one accent,
no gradients, no decorative color) but is warmer and more editorial, as the owner asked.

- **Type.** Junicode (roman for display, italic for the one accent word) and Bricolage Grotesque
  (body and UI, weight 500 like Zen; one or two weights only). A small monospace is used only
  for commands and the evidence labels: a system monospace stack (no extra download).
  Fonts are subset to the Latin glyphs we use and self-hosted as `woff2` with `font-display: swap`.
- **Color (proposed tokens, final values chosen in the first mock and checked for AA).**

  | Token       | Dark (default look)  | Light (paper)       |
  | ----------- | -------------------- | ------------------- |
  | background  | warm charcoal `#1c1b19` | warm paper `#f4f1e8` |
  | text        | `#d9d5c7`            | `#2a2926`           |
  | muted text  | text at about 65%    | text at about 65%   |
  | surface     | text at 5% over bg   | text at 5% over bg  |
  | accent      | one warm hue, not Zen's coral `#f76f53` | the same hue, darker for contrast |

  The accent is used for the italic word, links and focus rings. The four evidence labels get
  quiet, distinct tints (and a text label, never color alone).
- **Layout.** Centered column, generous vertical space, max width 1024 to 1280 px, large rounded
  corners on the product image, pill buttons, sticky slim header. Mobile first; 320 px readable.
- **Motion.** Reveal on scroll = fade + small translate (+ light blur at most), 200 to 400 ms, once.
  Hero headline words may stagger in. Everything is off under `prefers-reduced-motion: reduce`.
  Without JS, nothing is hidden: the "hidden" starting state is only applied by a script that
  adds a `js` class, and only when motion is allowed.
- **Theme.** `prefers-color-scheme` decides by default (works with JS off). The toggle writes the
  choice to `localStorage` inside `try/catch` and sets `data-theme`; the page renders correctly
  if storage is unavailable. A tiny inline-free script in `<head>` avoids a flash.
- **Logo.** `assets/brand/logo.svg` (copied into `site/`, not referenced across the repo).

## 8. Stack, location and tooling

- **Location:** `site/` at the repo root, a standalone project (its own `package.json`,
  `pnpm-lock.yaml`, `.nvmrc`-equivalent in `engines`). Not added to any workspace. Node 24 and
  the same pnpm major as the app (`packageManager` pinned).
- **Dependencies (pinned exactly; versions confirmed with `npm view` at build time):**
  `astro`, `tailwindcss` + `@tailwindcss/vite`, `animejs` (MIT; imported per module), `sharp`
  for image optimization, `@fontsource/bricolage-grotesque` (OFL) for the body font. Junicode comes
  from the upstream project (OFL 1.1) and is subset once with `fonttools`, then committed as `woff2`
  with its license file. Dev only: `prettier` + `prettier-plugin-astro`, `@astrojs/check`,
  `typescript`, and `playwright-core` + `@axe-core/playwright` for the accessibility script,
  driving the system Chrome (no browser download; the script says so if Chrome is missing).
  **No React, no icon packs** (a few inline SVG icons).
- **Source layout (target):**

  ```
  site/
    astro.config.mjs        # static output; Tailwind through its Vite plugin; no Astro integrations
    package.json  pnpm-lock.yaml
    vercel.json             # headers only: CSP, cache, security headers (no deploy config)
    public/fonts/  public/favicon.svg  public/robots.txt   # no og.png yet
    src/layouts/Base.astro
    src/pages/index.astro
    src/components/         # Header, Hero, ProductShot, Problem, Features, Evidence, Safety, Roadmap, Install, Footer
    src/data/release.ts     # single source for version, file names, release URLs
    src/styles/global.css   # Tailwind import, tokens, @font-face
    src/scripts/            # theme.ts, reveal.ts (the only client JS)
    scripts/                # check-links, check-size, check-copy, check-browser (incl. axe), check-version
    README.md               # how to run, build, check; how the screenshot was made
  ```
- **Version drift.** `release.ts` holds the version and file names. `check-version` fails if it
  differs from the root `package.json` version or if a file name no longer matches the README's
  install lines. The release checklist gets one line later (not in this task).
- **Security headers.** `vercel.json` sets a strict CSP (`default-src 'self'`; `script-src 'self'`;
  `style-src 'self'`; `img-src 'self' data:`; `font-src 'self'`; `base-uri 'none'`; `form-action
  'none'`; `frame-ancestors 'none'`), `Referrer-Policy: no-referrer`, long-lived immutable cache
  for hashed assets. Astro is configured not to inline styles or scripts so the CSP needs no
  `unsafe-inline`.
- **Outside `site/`, the only repo changes:** one line in the root `eslint.config.js` `ignores`
  so `pnpm lint` skips `site/` (root Prettier only checks `ui eslint.config.js scripts`, so it
  already ignores `site/`), this spec and its plan under `docs/superpowers/`, and the CI change
  below (the owner chose to do it in this task).
- **CI path filter (owner's decision: do it here).** Constraint found while writing this: the repo
  ruleset "Protect main" **requires the status check `Lint and test (Linux)`**. A workflow-level
  `paths-ignore` would stop that check from ever being reported on a site-only PR, and GitHub
  would block the merge forever. So `ci.yml` keeps triggering on every PR and the job keeps its
  name; a first step decides whether anything outside `site/` and `docs/` changed (plain `git diff
  --name-only` against the PR base, no third-party action), and the expensive steps (Rust, pnpm
  install, lint, tests) run only when it did. A skipped-steps job still reports success, so the
  required check stays satisfied. Risks to test in the PR: a PR touching both areas still runs
  everything; a push to `main` always runs everything; a failed diff computation must fall back to
  running everything (fail toward more checking, never less). This edits shared CI, so it gets its
  own atomic commit and is easy to drop.
- **Checks (run from `site/`):** `pnpm check` (Prettier check + `astro check`), `pnpm build`,
  `pnpm check:links` (every internal and external link resolves; external ones against a list),
  `pnpm check:size` (fails over budget), `pnpm check:a11y` (axe on the built pages in both
  themes and at 320 px and desktop), `pnpm check:version`.

## 9. Screenshots and fixture data

The v0.1 screenshot is stale (no session timeline). New captures come from `pnpm dev:ui` on port
1590 with the fixture data source (no Rust core), driven by `window.whyFixtures` and the existing
fixture sessions, following `assets/screenshots/README.md`: 1440 × 900 at device scale 2, dark
scheme, pinned clock so the rail reads "7m" and "1h". The capture tool (Playwright core driving
the system Chrome) lives **outside the repo**, so nothing is added to `package.json`.

- Shots: (1) hero, a worktree with a split diff and the session timeline open in the inspector;
  (2 and 3, optional) crops for "which turns explain each file" and "Why? / Declared".
- **One dark capture only (ruling made while building).** The app's own chrome is dark-only (its
  theme setting changes the Monaco editor theme, not the app), so there is no honest light
  screenshot to take. The hero image is the same on both page themes, framed in a rounded window
  that adapts to the page theme. The `<picture>` colour-scheme swap and the toggle's `<source
  media>` rewriting from earlier drafts were dropped.
- Before committing any capture: check for real names or paths (fixtures use `/home/dev/...`).
- Output as AVIF/WebP with PNG fallback only if the budget allows; `width` and `height` set;
  responsive `srcset`; explicit alt text describing what is shown.
- Whether the fixture scenarios already contain a Codex session and a "Why?" answer is **to be
  checked** in the build task; if a scene is missing, the UI owner (Codex) adds fixtures, since
  `ui/` is not ours to edit. That would be reported as a blocker, not worked around.

## 10. Deployment (separate step, the owner's word first)

Nothing here happens in the build. When the owner says so, the exact changes are:

1. In Vercel: import the repo, Root Directory `site`, Framework Preset Astro, Build `pnpm build`,
   Output `dist`, Node 24. Turn **off** Vercel Web Analytics and Speed Insights (the page promises
   no tracking). Add an "Ignored Build Step" so app-only commits and PRs do not trigger a site
   build.
2. DNS at the domain registrar: the CNAME Vercel shows for the chosen host.
3. Set the site's canonical URL (one `site` value in `astro.config.mjs`, also used by the
   canonical link and the Open Graph tags) to the final domain. There is no sitemap: it is one page.

**Domain recommendation.** `why.dorriss.com` is a good fit for now: free, one CNAME, no new
account, and it keeps the product's own name as the first label. Costs: the product is tied to a
personal domain, and `why` is a short label someone may want elsewhere. Because the canonical URL
is one config value, moving to a product domain later is one change plus a permanent redirect.
Until then the page runs on the Vercel preview URL with the canonical set from a build variable.

## 11. Verification plan (for the build task)

- In the built-in browser pane: desktop (1440), tablet, phone (375 and 320), JS disabled, reduced
  motion, light and dark, keyboard-only traversal with visible focus.
- The checks in section 8, plus a manual contrast check of every token pair.
- A manual honesty pass: every sentence on the page against README, CHANGELOG, SECURITY.
- Page weight table (HTML, CSS, JS, fonts, images) in the report.
- Report in Vietnamese, with screenshots of the page only, link-check result, page weight and an
  explicit "not verified" list (for example: not tested on real devices, not deployed).

## 12. Left to the builder

Exact color values and spacing scale; copy polish within the honesty rules; which of the optional
crops to include; component boundaries; the precise subset of glyphs; the check scripts'
implementation.

## 13. Questions settled by the owner (2026-11-21)

1. Hero headline: **C**, "Read the diff. Then ask *why*."
2. Domain: **`why.dorriss.com`**. It is the intended canonical URL; DNS and the Vercel domain are
   still the owner's step (section 10).
3. Footer: **no name or handle**, only project links. Nothing personal on the page.
4. Prior art: included as one fair line with a link to `why-brain/05 - Research.md` on GitHub
   (my call, as the builder check suggested; the owner can drop it in review).
5. CI path filter: **in this task**, done safely because of the required check (section 8).

## 14. Small calls I made (check these while reading)

- No sitemap (one page); no Astro integrations; canonical URL and Open Graph from one `site` value.
- The hero image is one dark capture on both page themes (the app has no light chrome); no `<picture>` scheme swap.
- The axe check uses `playwright-core` + `@axe-core/playwright` on the system Chrome, as dev
  dependencies of `site/` only.
- No site CI job: the checks are scripts run by hand, as the owner chose "lean".
- Palette (final, contrast-checked, see `site/README.md`): bg `#1c1b19` / `#f4f1e8`, ink `#d9d5c7` / `#2a2926`, accent `#e3a857` / `#8f5a0f`.
- The Junicode and Bricolage fonts are subset and pinned to one weight each (about 55 KB together).
- No `og:image` yet (nothing honest to show beyond the screenshot); add one later if wanted.
