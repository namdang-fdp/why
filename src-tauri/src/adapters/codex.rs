//! Codex adapter. **Stub**: WHY-15 implements it.
//!
//! Notes for WHY-15 (from payloads.md §3, Codex 0.170.0):
//! - Every payload: `session_id`, `transcript_path`, `cwd`, `hook_event_name`,
//!   `model`; turn-scoped events add `turn_id` (the turn key).
//! - `Pre/PostModelRequest` fire per model call: not needed, yield nothing.
//! - Edits: `tool_name == "apply_patch"`, the whole patch in
//!   `tool_input.command` (`*** Add File:` / `*** Update File:` /
//!   `*** Delete File:` with absolute paths). Parse the hook, not the rollout's
//!   JavaScript `exec` wrapper.
//! - A `PreToolUse` with no `PostToolUse` is outcome **unknown** (a rejected
//!   patch OR a lost delivery). Shell exit codes are not in hooks at all.
//! - Subagents: the parent's `session_id`, plus `agent_id`.

use why_wire::Envelope;

use super::Adapter;
use crate::events::NormalizedEvent;

pub struct CodexAdapter;

impl Adapter for CodexAdapter {
    fn normalize(&self, _env: &Envelope) -> Vec<NormalizedEvent> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::fixtures;

    #[test]
    fn stub_runs_on_every_codex_fixture() {
        let mut seen = 0;
        for (path, envelopes) in fixtures::load("codex") {
            for env in &envelopes {
                // WHY-15: replace with real expectations per scenario.
                let events = CodexAdapter.normalize(env);
                assert!(events.is_empty(), "{}", path.display());
                seen += 1;
            }
        }
        assert!(seen > 100, "only {seen} Codex payloads");
    }
}
