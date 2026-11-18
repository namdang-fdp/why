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
