//! Claude Code adapter. **Stub**: WHY-14 implements it.
//!
//! Notes for WHY-14 (from payloads.md §2, Claude Code 2.1.322):
//! - Every payload: `session_id`, `transcript_path`, `cwd`, `hook_event_name`;
//!   most add `prompt_id` (the turn key). No `model` field anywhere.
//! - `SessionStart.source`: `startup` | `resume` (resume = same session).
//! - `PostToolUse` for `Write`/`Edit`: `tool_response.structuredPatch` (hunks)
//!   and `originalFile`, enough to reconstruct the edit. `Bash` edits are
//!   invisible (only git knows).
//! - Failed Bash = `PostToolUseFailure` (`error` starts with `Exit code N`).
//!   Harness-rejected edits fire no Pre/Post at all, only a `PostToolBatch`
//!   entry whose `tool_response` starts with `<tool_use_error>`.
//! - Subagent events carry the parent's `session_id` plus `agent_id`.
//! - A synthetic `UserPromptSubmit` (`<task-notification>…`) follows an async
//!   subagent: a real turn, but not typed by the human.

use why_wire::Envelope;

use super::Adapter;
use crate::events::NormalizedEvent;

pub struct ClaudeAdapter;

impl Adapter for ClaudeAdapter {
    fn normalize(&self, _env: &Envelope) -> Vec<NormalizedEvent> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::fixtures;

    #[test]
    fn stub_runs_on_every_claude_fixture() {
        let mut seen = 0;
        for (path, envelopes) in fixtures::load("claude") {
            for env in &envelopes {
                // WHY-14: replace with real expectations per scenario.
                let events = ClaudeAdapter.normalize(env);
                assert!(events.is_empty(), "{}", path.display());
                seen += 1;
            }
        }
        assert!(seen > 100, "only {seen} Claude payloads");
    }
}
