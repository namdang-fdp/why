# Product and UX
Up: [[00 - WHY]]

## Primary workflow
1. Register local Git repositories. Discover their existing/new worktrees (including those created outside WHY).
2. See active agents, branch/base, original task, live change counts and review status across projects.
3. Select a worktree to open its live diff with session/task context.
4. Click a line/hunk to see its origin, purpose, rationale, relevant evidence and uncertainty.
5. Click a decision to highlight the related edits across files. Ask questions or leave feedback for the originating agent.
6. Review by attention: a queue orders worktrees/hunks by attention signals, so the reviewer reads the riskiest code first.

## Workspace layout (desktop)
- Left rail: projects → worktrees → changed files; branch status and activity.
- Top: task, branch/base, diff mode, review status.
- Center: split/unified diff, code syntax, inline annotations, live updates.
- Right: contextual inspector with reason, edit timeline, source events, alternatives and evidence.
- Review queue: hunks/files ordered by attention score; skip low-signal, well-evidenced changes.
- Diff gutter: compact attention markers per hunk (see signals below); click opens the reason in the inspector.
- Optional Decision view groups multiple file changes by logical decision; File view remains available.

## Reviewer questions
- What does this code do?
- Why did the agent add it, based on what context?
- What happens if it is removed? Is it necessary or overengineered?
- Does it implement the task or introduce possible scope drift?
- What tests did the agent run? What is unverified?

## Attention signals (per hunk)
- Scope drift vs. task.
- Not touched by any test the agent ran.
- Unknown provenance.
- High churn (edited/reverted repeatedly).
- Large unexplained deletions.
Signals are hints to route attention, not verdicts. The agent's declared reason is shown labeled as Declared, never as proof.

## Design direction
Linear × Vercel × macOS: monochrome, dark-first, compact type, restrained borders, excellent spacing, polished interactions. Color reserved for diff, agent status and risk. Keyboard-first, command palette; optional Vim motions later. Avoid dashboard clutter.
Do not hide actual changes behind summaries or promise definitive hallucination detection.
