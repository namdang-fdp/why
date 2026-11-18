---
tags: [why, project-brain]
---
# WHY

> Your agents write. You ask why.

WHY is a **human-first review workspace for agentic coding**. A local desktop app to observe and review many coding agents across multiple repositories and Git worktrees.

## The problem
Agent output is fast, but understanding it is slow. Git diff shows what changed, not the task context, reason for a change, necessity of a block, or whether the agent drifted from scope. Humans currently copy diffs into agent chats to recover that context.

## Product identity
- Orca-like multi-project/worktree navigation + LGTM-like live diff + evidence-backed rationale mapped to code.
- Not an IDE, agent harness, automated code reviewer, or orchestrator. Complement existing tools.
- The human owns approval. WHY observes, explains, and helps reviewers challenge decisions.
- Product name: **WHY**. Working tagline: "Because git diff tells you what, not why."
- Three questions: What changed? How did it change? Why did it change?
- **Routes review attention, explains where it matters.** Not "explain every line": an agent's stated reason can be post-hoc confabulation, so "why" never replaces reading code. The bottleneck relief is telling the reviewer where to look (scope drift, untested, Unknown provenance, churn, unexplained deletions).
- Not first-to-market: each piece exists (see [[05 - Research]]). The bet is the integrated, harness-agnostic combination.

## Moat and platform risk
Agent vendors (Anthropic, OpenAI, Cursor) own transcripts and can cheaply show "why" beside diffs in their own apps. Defensible core:
- Harness-agnostic: many agents, repos and worktrees in one place.
- Deterministic line provenance that survives concurrent/multi-agent edits to the same file.
- Invest in the engine; the UI is the shell. Provenance should be portable (see [[03 - Provenance Engine]]).

## Read next
- [[01 - Product and UX]] — core screens, interaction, visual direction.
- [[02 - Architecture and Stack]] — data flow and stack decisions.
- [[03 - Provenance Engine]] — technical crux and semantics.
- [[04 - MVP and Validation]] — first implementation plan and success criteria.
- [[05 - Research]] — relevant projects and evidence.
- [[06 - Decisions]] — dated decision log.

## Principles
Local-first · harness-agnostic · evidence-first · passive-first · beautiful & fast.
No fake reasoning. Mark missing evidence unknown. No premature infrastructure or feature creep.
