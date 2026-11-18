# MVP and Validation
Up: [[00 - WHY]]

## First: technical spike
Before polishing desktop UI, create a disposable Git repo with two worktrees. Run Claude Code and Codex on different tasks with consented hooks. Record events and reconstruct each edit's origins. Validate against known ground truth, including edits shifted by another edit, replacements, deletions, formatting, shell writes, agent/manual edits and commits. Use Docket as reference (optional prototype sidecar).
Target: >90% correct coverage of supported direct edit lines in measured scenarios, zero known false attribution on fixtures. Report overall diff coverage separately; mark unknown rather than guessing. Benchmark latency/overhead with several parallel sessions.

## Then: dogfood
No static mock/HTML test first. The founder builds the MVP directly and uses it daily on their own parallel-agent work (user #1).
Success criteria:
- Founder reviews agent work in WHY instead of copy-pasting diffs into chat.
- % of agent changes reviewed in WHY per week.
- Issues/scope drifts caught via attention signals.
- Time-to-review per change vs. baseline.
Caveat: founder bias. Once daily use is stable, recruit 2-3 external users.

## MVP scope
- [ ] Repository registration and auto-discover worktrees/branches
- [ ] Live diff incl. staged/unstaged/untracked and comparison against task base
- [ ] Agent session adapters: Claude Code + Codex
- [ ] Worktree ↔ task ↔ session correlation and timeline
- [ ] Per-hunk/line provenance + contextual explanation with source events
- [ ] Decision view: highlight related edits across files
- [ ] Lightweight attention signals per hunk (scope drift, untested, Unknown, churn, large deletions) and attention-ordered review queue
- [ ] Local review comments and export feedback to original agent

## Build order
1. Core repo/worktree discovery, event schema, fixtures.
2. Collector + adapters + replay/line attribution tests.
3. GUI: multi-project sidebar, diff, context inspector with seeded fixtures.
4. Wire live data and test with real agents. Polish.
5. Attention signals, queue ordering and review feedback. Dogfood daily.

## Explicitly later
Git graph, cherry-picks, merge/conflict tools, PR integration, exporting provenance (git notes, trailers, PR checkpoint), deeper dependency graph, mutation tests, remote monitoring, analytics, agent control, team sync (reviewer without the transcript). No production hardening or SaaS infra until core value proven.
