# Decisions
Up: [[00 - WHY]]

## 2026-11-18 — Strategy review
1. **Differentiate on the combination, not novelty.** Conductor, Crystal, Vibe Kanban, Claude Code desktop app already cover pieces; claiming "first" is false and fragile. See [[05 - Research]].
2. **Moat = engine, UI = shell.** Vendors own transcripts and can show "why" cheaply; harness-agnostic multi-agent view plus deterministic provenance under concurrent edits is what they won't build. See [[00 - WHY]].
3. **Route attention instead of explaining every line.** Stated reasons can be post-hoc confabulation; the real relief is telling the reviewer where to look. Attention signals per hunk. See [[01 - Product and UX]].
4. **Provenance schema is portable.** Reviewers often lack the transcript; design for git notes / trailers / PR checkpoint. Export is later, local-only assumptions are not allowed now. See [[02 - Architecture and Stack]], [[03 - Provenance Engine]].
5. **Validate by dogfooding.** Keep the provenance spike, then build the MVP and use it daily; track usage metrics; add external users after stable use to counter founder bias. See [[04 - MVP and Validation]].

## 2026-11-18 — Roadmap and working model
6. **Coarse provenance before fine.** Ship turn/file-level "why" (v0.2) before line-level replay (v0.3); the spike moves into v0.3. Reason: daily usefulness tonight, precision later.
7. **Codex adapter in v0.2, not later.** Founder's mix is Claude Code ~50% / Codex ~40%; one adapter would leave half the work unexplained. Antigravity (~10%) is a v0.5 feasibility spike.
8. **Agents write all code; founder learns via deep PR review.** Coordinator + founder plan and review; worker sessions (one task = one worktree/branch/port) implement. No worker starts without founder go-ahead. Kaneo holds task state; brain holds decisions only.
9. **Contracts generated from Rust (ts-rs).** Lets Rust core (agent:claude) and UI (agent:codex) work in parallel against fixtures with no drift.

## 2026-11-19/20 — v0.1 build and release
10. **Wiring task before parallel work.** Shared `AppState`, `RepoStore` trait, single error mapping and per-task IPC files (WHY-40) let two workers run without touching the same lines. Reuse the pattern whenever tasks share wiring.
11. **Branches are `feature/WHY-N-slug`.** Owner's convention from 2026-11-19 (was `why/…`).
12. **`main` is protected.** PR + green "Lint and test (Linux)" required, no force-push or deletion; the owner (admin) can bypass. Docs/brain changes also go through PRs.
13. **Two review layers per PR.** Coordinator review + Codex review; they find different bug classes.
14. **Minimum git 2.45.1.** Needed for `GIT_NO_LAZY_FETCH` (no network or object writes on partial clones). Accepted cost: Ubuntu 22.04/24.04 and Debian 12 ship older git.
15. **Repo-local git filter drivers are blocked until trusted** (WHY-39). Per-driver fingerprints re-prompt on change; global/system drivers (e.g. git-lfs) are allowed and shown. Fixed before the first public release because "observe without executing" is the product's promise.
16. **The webview gets no dialog powers.** The folder picker runs from the core (`pick_repository_folder`); `dialog:allow-open` cannot be narrowed.
17. **v0.1.0 shipped as an official release, no RC** (2026-11-20): Linux AppImage + .deb + SHA256SUMS, unsigned, no auto-update. Screenshots use fixture data only.
18. **Working agreement written down** in [[07 - Working Agreement]] so a new coordinator session can continue v0.2 with the same rules.

## 2026-11-20 — v0.2 plan (Session context)
19. **Capture real payloads before designing the schema** (WHY-45 spike). Hook fields, worktree settings lookup and Codex's transcript source (rollout JSONL vs thread-history SQLite in 0.170) are verified on golden fixtures, not assumed.
20. **Wiring task again** (WHY-46): Cargo workspace (`crates/why-hook`, `crates/why-wire`), `EventStore` trait, contracts and stubs before WHY-12/13/17 run in parallel. Contracts keep the no-`u64` rule: timestamps are RFC 3339 strings, ids opaque strings.
21. **Hooks are installed at user level** (`~/.claude/settings.json`, `~/.codex/hooks.json`), never into repositories: a file in the worktree would show up in the diff (`.claude/settings.local.json` is not gitignored) and agents could commit it. Installers merge beside existing user hooks and restore the file byte-identical.
22. **Capture only registered repositories.** `why-hook` reads an allowlist written by the app and drops everything else, even when the app is down. Excerpts ≤ ~8 KB, best-effort secret redaction, spool `0600`, 30-day retention.
23. **v0.2 edits come from harness payloads only**; no pre-read before/after snapshots. Full snapshots arrive with shell-write capture (WHY-24, v0.3).
24. **Events live in their own `events.db`** (WAL, single writer), separate from the repository registry `why.db`.
25. **`why-hook` is a separate tiny binary** (no Tauri), shipped as `externalBin` and copied to `~/.local/share/why/bin` so hook configs point at a stable path (an AppImage mounts somewhere new every run).
26. **Dev instances never share the installed build's collector socket or data dir.** WHY-19 dogfoods a locally built v0.2.0 bundle; v0.2.0 is released after that checkpoint.
27. **Review bandwidth caps parallelism**: at most 2 Claude workers + 1 Codex UI task at once.
28. **glib 0.18 alert dismissed as tolerable risk**: pinned by Tauri 2.12's GTK3 stack, `VariantStrIter` unused; revisit when Tauri moves to gtk-rs ≥ 0.20. DOMPurify is forced to a patched version with a pnpm override (WHY-47).

## 2026-11-21 — v0.3 plan (Line provenance)
Context: v0.2.0 and v0.2.1 (UI polish: settings page, top bar, inspector phase A) are tagged. The v0.3 tickets (WHY-21..27) were written on 2026-11-18, before v0.2's code existed; reading the v0.2 code showed that the replay engine would have had no exact input to replay. Plan reconciled with the owner on 2026-11-21 (Kaneo WHY-63 gate, WHY-64 wiring, WHY-65 capture).

29. **Replay inputs are per-line hashes, not line text.** v0.2 keeps an edit only as a rendered excerpt capped at 8 KB (`text`) plus file paths, which cannot drive a deterministic replay. v0.3 persists, per edit, the hashes of removed, added and context lines with line counts and positions where the harness gives them (Claude `structuredPatch`, plus a whole old-file hash from `originalFile`) in `events.db` schema v3 (a new table; shipped migrations are never edited). Line text is never stored: replay only has to *match* lines, and display text still comes from git and the existing excerpts. This keeps decision 22's privacy promise, leaves the 30-day retention and the SECURITY.md wording intact, and content hashes keep the records portable (decision 4). Cost: stored rows cannot be read by eye when debugging (fixtures may keep line text for readability). Payloads the hook reduced (over 1 MiB, or redacted) are recorded as unobserved, never guessed. One documented line-normalization policy (CRLF, trailing newline) lives in the `provenance` module (WHY-64).
30. **Codex coverage is reported on its own.** `apply_patch` carries context and changed lines but no line numbers and no original file, so hunks are located by context; ambiguous or missing context is Unknown, never a guess. The >90% target applies to Claude `Edit`/`Write` direct-edit lines; Codex is measured and published separately in the WHY-26 report.
31. **Shell-write capture is a spike, not a commitment.** WHY-24 starts with a 2 h timeboxed spike after the replay engine works: can a pre/post snapshot run inside the hook's fail-open latency budget, or only app-side (async, racy, blind while WHY is closed, needs a `ReadPermit` and `--no-filters`)? If it does not fit, it moves to v0.3.1 and shell writes stay leads, as in v0.2. v0.3 ships direct edits.
32. **An edit belongs to the edited file's own worktree; no v0.2.2.** The WHY-19 dogfood finding (a session in the main checkout editing a file inside another worktree by absolute path) is fixed in v0.3 wave 2 (WHY-65): ingest resolves each edited file's worktree with the git resolver (never by prefix guessing), stores the repo-relative path, and lists the session in both worktrees. Line origins would be wrong without it. WHY-55's core items (last event per harness, a missing spool directory counts as 0) ride in the same task. v0.2.1 stayed UI-only; WHY-58 a–d stay backlog.
33. **The engine and its ground truth have different authors.** WHY-64 freezes the fixture format; WHY-21 (scenarios, expected origins, expected Unknowns) and WHY-22 (engine) are written by different workers. If one worker wrote both, "zero known false attribution on fixtures" would only prove the code agrees with itself.

Order: WHY-63 gate → WHY-64 wiring (alone) → WHY-65 ‖ WHY-21 ‖ WHY-25 (Codex UI, on fixtures) → WHY-22 engine → WHY-23 alignment and anchors → WHY-24 spike → WHY-26 benchmark → WHY-27 dogfood → release v0.3.0.
