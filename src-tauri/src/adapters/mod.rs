//! Harness adapters: one hook [`Envelope`] in, WHY's [`NormalizedEvent`]s out.
//!
//! One adapter per harness, each in its own file, so the two can be written in
//! parallel (WHY-14 Claude Code, WHY-15 Codex) without touching shared lines.
//! WHY-46 ships stubs that return nothing.
//!
//! An adapter is a pure function of the envelope: no I/O, no git, no clock.
//! That keeps it testable against the golden fixtures and makes
//! [`EventKey`](crate::events::EventKey)s reproducible (the `index` argument of
//! [`NormalizedEvent::from_envelope`] must be the event's position in the
//! returned vector, so the same envelope always yields the same keys).
//!
//! Adapters also own the rules that depend on the harness (from payloads.md):
//! - turn key: Claude `prompt_id`, Codex `turn_id`; subagent: `agent_id`;
//! - excerpts are capped with [`crate::events::excerpt`];
//! - edits: Claude `PostToolUse` for `Write`/`Edit` carries `structuredPatch`
//!   and `originalFile`; Codex `apply_patch` puts the patch text in
//!   `tool_input.command` (absolute paths in `*** Add/Update/Delete File:`);
//! - outcomes: Claude `PostToolUseFailure` is a failure; a Pre with no Post is
//!   left for the timeline to show as `unknown`, never "rejected";
//! - events WHY does not use (Codex `Pre/PostModelRequest`) yield nothing.

pub mod claude;
pub mod codex;

#[cfg(test)]
pub(crate) mod fixtures;

use why_wire::{Envelope, Harness};

use crate::events::NormalizedEvent;

/// Turns one harness's hook payloads into normalized events.
///
/// `Send + Sync` so the adapters can be shared `'static` values used from any
/// thread of the collector.
pub trait Adapter: Send + Sync {
    /// The events in `env`, in a deterministic order. Unknown or malformed
    /// payloads yield an empty vector, never a panic: the input comes from
    /// another program and may be anything.
    fn normalize(&self, env: &Envelope) -> Vec<NormalizedEvent>;
}

/// The adapter for `harness`, or `None` for [`Harness::Unknown`].
///
/// `&'static dyn Adapter`: the adapters are stateless unit structs, so a
/// `static` instance each is enough, and a trait object lets callers treat
/// them uniformly without knowing which harness they hold.
pub fn adapter_for(harness: Harness) -> Option<&'static dyn Adapter> {
    match harness {
        Harness::ClaudeCode => Some(&claude::ClaudeAdapter),
        Harness::Codex => Some(&codex::CodexAdapter),
        Harness::Unknown => None,
    }
}

/// Normalize with the envelope's own harness adapter (empty for unknown ones).
pub fn normalize(env: &Envelope) -> Vec<NormalizedEvent> {
    adapter_for(env.harness)
        .map(|a| a.normalize(env))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_payload_fits_the_wire_and_round_trips() {
        // The real WHY-45 payloads, wrapped as the hook would, survive the
        // framing unchanged (and none comes near the line limit).
        let mut count = 0;
        for (path, envelopes) in fixtures::all() {
            for env in envelopes {
                let line = why_wire::encode_line(&env)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert_eq!(why_wire::decode_line(&line).unwrap(), env);
                count += 1;
            }
        }
        assert!(count > 400, "only {count} fixture payloads found");
    }

    #[test]
    fn unknown_harness_has_no_adapter() {
        assert!(adapter_for(Harness::Unknown).is_none());
        let env = fixtures::all().remove(0).1.remove(0);
        let unknown = Envelope {
            harness: Harness::Unknown,
            ..env
        };
        assert!(normalize(&unknown).is_empty());
    }
}
