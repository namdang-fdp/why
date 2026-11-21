# WHY landing page (`site/`)

A static page built with Astro, Tailwind CSS 4 and AnimeJS. It is a standalone project: its own
`package.json` and lockfile, not part of the app's pnpm install, and skipped by the app's
`pnpm lint`. Design: `docs/superpowers/specs/2026-11-21-landing-page-design.md`.

## Run

```bash
cd site
pnpm i
pnpm dev        # http://localhost:1591
pnpm build      # static files in dist/
pnpm preview
```

## Checks (run from `site/`, after `pnpm build`)

| Command              | What it checks                                                                 |
| -------------------- | ------------------------------------------------------------------------------ |
| `pnpm check`         | Prettier and `astro check`                                                     |
| `pnpm check:version` | `src/data/release.ts` matches the root `package.json` and the README's install lines |
| `pnpm check:size`    | the budgets in the spec (everything except images under 200 KB, and so on)     |
| `pnpm check:copy`    | no banned claims ("trusted by"...), and the honest phrases are present         |
| `pnpm check:links`   | every internal and external link resolves (needs network)                      |
| `pnpm check:browser` | JS off, reduced motion, blocked scripts, throwing storage, 320 px, no third party, CSP, axe |

`check:browser` drives the system Chrome through `playwright-core`; it downloads no browser.

## Colour contrast

Every text colour is at least 4.5:1 on both `--bg` and `--surface` (computed from the token
values in `src/styles/global.css`):

| Token (light / dark)  | on bg         | on surface    |
| --------------------- | ------------- | ------------- |
| ink                   | 12.9 / 11.7   | 11.8 / 10.4   |
| muted                 | 6.2 / 7.0     | 5.7 / 6.2     |
| accent                | 5.1 / 8.2     | 4.7 / 7.3     |
| Observed              | 5.7 / 8.6     | 5.2 / 7.7     |
| Declared              | 6.0 / 8.1     | 5.5 / 7.2     |
| Inferred              | 6.8 / 7.7     | 6.2 / 6.8     |
| Unknown               | 6.2 / 7.0     | 5.7 / 6.2     |

`pnpm check:browser` also runs axe (which includes colour contrast) in both schemes.

## Fonts

Junicode (display) and Bricolage Grotesque (body) are SIL OFL 1.1; the licence texts are in
`public/fonts/`. They are subset to Basic Latin plus the punctuation the page uses and pinned to
one weight, with `fonttools` (`pyftsubset` and `fonttools varLib.instancer`), from the upstream
Junicode release v2.226 and `@fontsource/bricolage-grotesque` 5.3.0. Add a glyph to the page that
is not in the subset and it falls back to the system serif or sans.

## Releases

When a release is cut, update `src/data/release.ts`; `pnpm check:version` fails until it matches.

## Screenshots

They come from the app's UI running on fixture data, never from a real machine. See
`assets/screenshots/README.md` for the method (1440 x 900 at device scale 2, pinned clock).

## Deploys

The site is hosted on Vercel as the project `why`. Nothing in this repo deploys it; the project
settings live in the Vercel dashboard, and `vercel.json` only holds headers.

| Setting                                  | Value                                                                  |
| ---------------------------------------- | ---------------------------------------------------------------------- |
| Git repository                           | `namdang-fdp/why`, production branch `main`                            |
| Root Directory                           | `site`                                                                 |
| Framework, build, output                 | Astro, `pnpm build`, `dist`                                            |
| Node.js                                  | 24.x                                                                   |
| Include source files outside Root Dir    | **on**: `/changelog` reads `../CHANGELOG.md` at build time             |
| Ignored Build Step                       | `git diff --quiet HEAD^ HEAD -- . ../CHANGELOG.md`                     |
| Web Analytics, Speed Insights            | **off**: the page promises no tracking                                 |
| Production domain                        | `why.dorriss.com`                                                      |

- Vercel deploys a commit **pushed** to `main`. A project created after a commit does not deploy
  that commit by itself, so the first deploy needs a new commit on `main`.
- A push that changes neither `site/` nor `CHANGELOG.md` skips the build (the Ignored Build Step
  exits 0). If the command fails, Vercel builds anyway.
- DNS is at Cloudflare: a `CNAME` for `why` pointing at the value Vercel shows for the domain, with
  the proxy **off** (DNS only, grey cloud). A proxied record in front of Vercel breaks its
  certificates.
- The `*.vercel.app` URLs sit behind Vercel Authentication; the custom domain is public.
- The canonical URL and Open Graph tags come from `site` in `src/data/release.ts`.
