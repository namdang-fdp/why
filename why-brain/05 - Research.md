# Research
Up: [[00 - WHY]]

## Prior art to inspect (not a claim of unique invention)
- [LGTM](https://github.com/kunkka19xx/lgtm): live diff and line-anchored feedback, terminal-first.
- [Glassbox](https://github.com/brianwestphal/glassbox): GUI with agent-authored line-anchored rationale, proof and SARIF notes.
- [Docket](https://github.com/Dillonsmart/docket): Go engine for replaying session edits, per-line/hunk attribution, evidence/unknown. Author-reported direct added-line attribution: Claude Code 99.8%, Codex 95.7%, OpenCode 92.9%; shell-heavy cases lower. Not independent validation; whole diff coverage can be much lower.
- [Entire](https://github.com/entireio/cli): Git-native agent session checkpoints.
- [Atomic](https://docs.atomic.dev/agents/provenance): observed events and inferred causal graph.
- [Daintree](https://github.com/daintreehq/daintree): multi-project/worktree agent workspace + review.
- [Muster](https://github.com/MaraniMatias/Muster): desktop worktree discovery and review.
- [Prufs](https://prufs.ai/): decision trails and line links.
- Conductor, Crystal, Vibe Kanban: multi-worktree agent GUIs with diffs (orchestration-leaning).
- Claude Code desktop app: diff beside the conversation.

Each piece exists. WHY is not first; its bet is the integrated, harness-agnostic combination. Vendors can show "why" next to diffs in their own apps, so the moat is cross-agent scope and deterministic provenance (see [[00 - WHY]]).

WHY's proposed angle: combine passive multi-project/worktree oversight with live, evidence-backed decision-to-diff navigation built for human review, not orchestration.

## Technical references
- [Claude Code hooks](https://code.claude.com/docs/en/hooks)
- [Codex hooks](https://developers.openai.com/codex/hooks)
- [Codex App Server](https://developers.openai.com/codex/app-server)
- [Git worktrees](https://git-scm.com/docs/git-worktree)
- [Tauri architecture](https://v2.tauri.app/concept/architecture/)
- [SQLite WAL](https://sqlite.org/wal.html)
- [Tree-sitter](https://tree-sitter.github.io/tree-sitter/)

Status: design validated by available integration docs and prior art; simple synthetic line-replay proof demonstrated in chat. No real WHY implementation or real agent benchmark yet.
