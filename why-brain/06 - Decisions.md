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
