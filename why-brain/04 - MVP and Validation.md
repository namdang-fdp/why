# MVP and Validation
Up: [[00 - WHY]]

## User #1 (founder) profile
- Agents: Claude Code ~50% (backend logic), Codex ~40% (UI), Antigravity ~10% (when token-limited).
- Workflow: one strong coordinator agent plans and orchestrates; worker sessions run lighter models, each owning one worktree with its own branch, env and port.
- Today's review: `git diff` inside Claude Code or VS Code branch extensions. Pain: nothing centralized, context lost across PRs, repeated "why did you do X" turns and copy-pasting code back to agents.

## Roadmap: vertical slices, usable from the first one
Turn/file-level "why" (v0.2) answers most questions cheaply; line-level replay (v0.3) is the precision/moat layer. Task state lives in Kaneo (project WHY), not here.
- **v0.1 Workspace + live diff:** repo registration, worktree discovery, diff vs base incl. untracked, live updates, Monaco diff. **Released 2026-11-20 as v0.1.0** (Linux AppImage + .deb), incl. the git filter-driver guard.
- **v0.2 Session context:** payload-capture spike, wiring, event store (`events.db`), fail-open `why-hook` + collector (user-level hooks, registered-repo allowlist), Claude Code and Codex adapters, worktree↔session↔task correlation, session timeline in inspector, hook setup UI. Plan approved 2026-11-20 (decisions 19–28). **Released 2026-11-21 as v0.2.0**; v0.2.1 (UI polish: settings page, top bar, inspector phase A) tagged the same day.
- **v0.3 Line provenance (released 2026-11-22 as v0.3.0, before the real-session benchmark: decision 36):** exact-edit capture (per-line hashes, decision 29), ground-truth fixtures, replay engine, alignment + stable anchors, gutter origins, accuracy benchmark; shell-write capture was a timeboxed spike (decision 31) and moves to v0.3.1 (decision 34). This is the former "technical spike". Target: >90% correct coverage of supported direct-edit lines (Claude `Edit`/`Write`; Codex reported separately, decision 30), zero known false attribution on fixtures, overall diff coverage reported separately, overhead measured with parallel sessions. Plan: decisions 29–33.
- **v0.4 Attention:** signal engine (Unknown, churn, untested, big deletion, scope drift), attention-ordered review queue, per-hunk review state, decision view.
- **v0.5 Close the loop:** anchored comments, feedback export to originating agent, Antigravity feasibility spike, portable provenance export design, external users.

## Dogfood
No static mock first. WHY is built by agents in parallel worktrees and reviewed in WHY from v0.1 on.
Success criteria:
- Founder reviews agent work in WHY instead of copy-pasting diffs into chat.
- % of agent changes reviewed in WHY per week.
- Issues/scope drifts caught via attention signals.
- Time-to-review per change vs. baseline.
Caveat: founder bias. Once daily use is stable, recruit 2-3 external users.

## Explicitly later
Git graph, cherry-picks, merge/conflict tools, PR integration, exporting provenance (git notes, trailers, PR checkpoint) beyond the v0.5 design, deeper dependency graph, mutation tests, remote monitoring, analytics, agent control, team sync (reviewer without the transcript). No production hardening or SaaS infra until core value proven.
