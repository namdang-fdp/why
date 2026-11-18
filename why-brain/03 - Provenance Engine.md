# Provenance Engine
Up: [[00 - WHY]] | [[02 - Architecture and Stack]]

The defining feature is reliable two-way **decision ↔ code** mapping, not AI guessing from the final diff.

## 1 — Physical attribution (deterministic)
- Capture session ID, turn ID, tool-call ID, cwd/worktree, agent type/model, timestamp, prompt, tool input/output and status.
- Record before/after snapshots or exact patches for successful edits. Snapshot shell-generated changes where possible.
- Replay edits in event order per worktree and file. Carry origins of unchanged lines, assign new origins to changed lines, preserve deletion history.
- Align the final working-tree/commit diff with the replay using content and context; retain stable anchors (revision + file + range + hash). Later use syntax/symbol anchors for formatting and movement.
- On mismatch, manual edit, ambiguous duplicate lines or missing data, mark unknown. Never infer attribution from time alone. Detect concurrent writers; reconcile without silently overwriting provenance.

## 2 — Semantic context (non-deterministic)
Connect each physical edit to task, prior agent messages/plans, explicit rationale, file reads, alternatives and test/tool results. A decision can span many edits across files; a hunk can contain several decisions. Optional model can summarize only relevant event slices with source refs.
- **Observed:** actual tool calls/edits/test outputs captured by the harness/collector.
- **Declared:** agent's stated rationale at the time.
- **Inferred:** reconstructed explanation, clearly labeled.
- **Unknown:** insufficient evidence.

Never claim access to private chain-of-thought; summaries/messages are not guaranteed causal truth. "What if removed?" is a hypothesis unless independently verified. Tests passed is not proof of semantic correctness.

## Portable records
Reviewers are often not the person who ran the agent and lack the transcript. Design LineOrigin, Decision and Evidence as exportable, versioned, path-relative records keyed by revision + file + range + content hash, with evidence excerpts and source refs embedded or content-addressed. Target carriers: git notes, commit trailers, checkpoint file on a PR. Redact secrets/private prompts on export. Not in MVP; the schema must not block it.

## Inputs
Claude Code: SessionStart, UserPromptSubmit, MessageDisplay, PreToolUse, PostToolUse, SubagentStart/Stop, Stop; transcript_path. Codex: corresponding hooks incl. apply_patch, cwd/session/turn IDs; rollout JSONL (unstable format, version adapters). App Server JSON-RPC can stream structured items if WHY later controls sessions. Use Git worktree discovery, not Claude WorktreeCreate hook (that hook replaces default creation).
