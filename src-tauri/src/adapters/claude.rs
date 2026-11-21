//! Claude Code adapter: hook payloads (Claude Code 2.1.x) → [`NormalizedEvent`]s.
//!
//! A pure function of the envelope: no I/O, no git, no clock (rule from
//! `adapters/mod.rs`). What each hook becomes, and why:
//!
//! | Hook | Events (in this order) | Notes |
//! |---|---|---|
//! | `SessionStart` | `SessionStart{resumed}` | `source:"resume"` is the SAME session, so `resumed`, never a new one |
//! | `SessionEnd` | `SessionEnd` | ends a harness *process*, not the conversation |
//! | `UserPromptSubmit` | `Prompt` | text = prompt excerpt; a `<task-notification>` one is synthetic, see [`extract::is_synthetic_prompt`] |
//! | `PreToolUse` | `ToolStarted` (+ `AssistantMessage` Declared for a Bash `description`) | Edit rows carry no text here: the diff arrives with the result |
//! | `PostToolUse` | `ToolFinished{Ok}` | Edit: files + rendered hunks (Observed) + the exact [`EditOp`] (see [`edit_op`]); Bash: the command again, so a lost Pre still shows what ran |
//! | `PostToolUseFailure` | `ToolFinished{Failed}` | the ONLY source of "failed"; the exit code is in `error`, see [`extract::exit_code`] |
//! | `PostToolBatch` | `ToolFinished{Failed}` per **harness-rejected edit** only | the one trace of an edit the harness refused before running it |
//! | `SubagentStart` / `SubagentStop` | `SubagentStarted` / (`AssistantMessage` Declared, `SubagentStopped`) | `agent_id` set; same `session_id` as the parent |
//! | `Stop` | (`AssistantMessage` Declared, `Stop`) | `last_assistant_message` is what the agent *said*, so it is labelled Declared; the stop itself is Observed |
//! | anything else | nothing | counted in [`ignored_events`] and logged at debug level |
//!
//! Outcomes are never inferred from text: `cmd; echo exit=$?` is a successful
//! `PostToolUse` for a shell command, whatever it printed. The timeline pairs
//! Pre/Post by `(agent_id, tool_use_id)`, so arrival order does not matter here.
//!
//! The transcript reader (lazy, Declared rationale) lives in [`transcript`].

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use why_wire::{Envelope, Reduction, reduction_of};

use self::extract::{edit_details, first_line, non_empty, str_field, target_path, tool_use_error};
use super::Adapter;
use crate::contracts::{EvidenceLabel, ToolOutcome};
use crate::events::model::DeliveryDigest;
use crate::events::{EventKind, NormalizedEvent, ToolCall, ToolClass, excerpt};
use crate::provenance::{EditOp, UnobservedReason};

pub mod edit_op;
pub mod extract;
pub mod shell;
pub mod transcript;

/// How many hook events (or batch entries) were skipped because WHY does not
/// use them or their shape was unusable. For tests and a debug counter: a new
/// Claude Code version that starts sending something new shows up here.
static IGNORED: AtomicU64 = AtomicU64::new(0);

pub fn ignored_events() -> u64 {
    IGNORED.load(Ordering::Relaxed)
}

fn ignore(what: &str) {
    IGNORED.fetch_add(1, Ordering::Relaxed);
    tracing::debug!(what, "claude adapter: ignored");
}

pub struct ClaudeAdapter;

impl Adapter for ClaudeAdapter {
    fn normalize(&self, env: &Envelope) -> Vec<NormalizedEvent> {
        let p = &env.payload;
        if !p.is_object() {
            ignore("payload is not an object");
            return Vec::new();
        }
        // Without a session id the event cannot be filed anywhere.
        let Some(session) = non_empty(p, "session_id") else {
            ignore("no session_id");
            return Vec::new();
        };
        // The payload's own name wins; the envelope's is the fallback.
        let hook = non_empty(p, "hook_event_name").unwrap_or(&env.event);

        let mut b = Builder {
            env,
            digest: None,
            session,
            out: Vec::new(),
        };
        match hook {
            "SessionStart" => {
                let resumed = str_field(p, "source") == Some("resume");
                b.push(EventKind::SessionStart { resumed });
            }
            "SessionEnd" => {
                b.push(EventKind::SessionEnd);
            }
            "UserPromptSubmit" => {
                let e = b.push(EventKind::Prompt);
                e.text = str_field(p, "prompt").map(excerpt);
            }
            "PreToolUse" => b.tool_event(p, Phase::Start),
            "PostToolUse" => b.tool_event(p, Phase::Done),
            "PostToolUseFailure" => b.tool_event(p, Phase::Failed),
            "PostToolBatch" => b.rejected_edits(p),
            "SubagentStart" => {
                let e = b.push(EventKind::SubagentStarted);
                e.text = non_empty(p, "agent_type").map(str::to_owned);
            }
            "SubagentStop" => {
                b.declared_message(p);
                let e = b.push(EventKind::SubagentStopped);
                // The tools themselves report the parent transcript.
                e.transcript_path = non_empty(p, "agent_transcript_path").map(str::to_owned);
            }
            "Stop" => {
                b.declared_message(p);
                b.push(EventKind::Stop);
            }
            other => ignore(other),
        }
        b.out
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    Done,
    Failed,
}

/// Collects the events of one envelope; `push` fills the fields every Claude
/// event shares so each hook arm only sets what is specific to it.
struct Builder<'a> {
    env: &'a Envelope,
    digest: Option<DeliveryDigest>,
    session: &'a str,
    out: Vec<NormalizedEvent>,
}

impl Builder<'_> {
    fn push(&mut self, kind: EventKind) -> &mut NormalizedEvent {
        let p = &self.env.payload;
        // `index` must be the event's position in the returned vector: it is
        // part of the dedupe key.
        let index = u32::try_from(self.out.len()).unwrap_or(u32::MAX);
        let key = self
            .digest
            .get_or_insert_with(|| DeliveryDigest::new(self.env))
            .event_key(index);
        let mut ev = NormalizedEvent::from_delivery_key(self.env, key, self.session, kind);
        ev.turn_id = non_empty(p, "prompt_id").map(str::to_owned);
        ev.agent_id = non_empty(p, "agent_id").map(str::to_owned);
        ev.transcript_path = non_empty(p, "transcript_path").map(str::to_owned);
        self.out.push(ev);
        let last = self.out.len() - 1;
        &mut self.out[last]
    }

    /// `last_assistant_message` as a Declared event (the agent's own words).
    fn declared_message(&mut self, p: &Value) {
        if let Some(msg) = non_empty(p, "last_assistant_message") {
            let e = self.push(EventKind::AssistantMessage);
            e.text = Some(excerpt(msg));
            e.evidence = EvidenceLabel::Declared;
        }
    }

    /// `PreToolUse` / `PostToolUse` / `PostToolUseFailure`.
    fn tool_event(&mut self, p: &Value, phase: Phase) {
        let Some(name) = non_empty(p, "tool_name") else {
            ignore("tool event without tool_name");
            return;
        };
        // `tool_input` may be anything; a non-object reads as "no fields".
        let input = p.get("tool_input").unwrap_or(&Value::Null);
        let class = classify(name, input);
        let kind = match phase {
            Phase::Start => EventKind::ToolStarted,
            Phase::Done => EventKind::ToolFinished {
                outcome: ToolOutcome::Ok,
            },
            Phase::Failed => EventKind::ToolFinished {
                outcome: ToolOutcome::Failed,
            },
        };
        let response = p.get("tool_response").unwrap_or(&Value::Null);
        let error_line = str_field(p, "error").map(first_line);

        // Files: only edits count as "touched". A read is not an edit, and the
        // timeline's file list means "files this turn changed".
        let mut files = Vec::new();
        if class == ToolClass::Edit {
            // A successful tool response identifies the file actually edited.
            let executed = match phase {
                Phase::Done => non_empty(response, "filePath"),
                _ => None,
            };
            files.extend(executed.or_else(|| target_path(input)).map(str::to_owned));
        }

        // What the row says. NOTE the timeline keeps the *start* text over the
        // finish text, so anything only known at the result (the diff) must
        // not be set at start.
        let text = match (class, phase) {
            (ToolClass::Edit, Phase::Start) => None,
            (ToolClass::Edit, Phase::Done) => {
                edit_details(input, response).map(|d| excerpt(&d.render()))
            }
            (ToolClass::Edit, Phase::Failed) => error_line.map(excerpt),
            (ToolClass::Shell | ToolClass::TestRun, _) => non_empty(input, "command").map(excerpt),
            (ToolClass::Subagent, _) => non_empty(input, "description").map(excerpt),
            (ToolClass::Other, _) => {
                // A read's path is useful context; other tools say nothing.
                non_empty(input, "file_path").map(str::to_owned)
            }
        };

        // v0.3: the exact edit, from a SUCCESSFUL finish only. A failure, a
        // rejected edit (no Pre/Post at all) and a bare start change nothing, so
        // they carry no op. The path is the one `files` already holds.
        let edits = if class == ToolClass::Edit && phase == Phase::Done {
            edit_ops(p, name, response, files.first().map(String::as_str))
        } else {
            Vec::new()
        };

        let use_id = non_empty(p, "tool_use_id").map(str::to_owned);
        let ev = self.push(kind);
        ev.tool = Some(ToolCall {
            use_id,
            name: name.to_owned(),
            class,
        });
        ev.files = files;
        ev.text = text;
        ev.edits = edits;

        // The agent's own explanation of a command, once, at the start. It is
        // the model's words (Declared), so it is its own event rather than a
        // label on the Observed command.
        if phase == Phase::Start
            && matches!(class, ToolClass::Shell | ToolClass::TestRun)
            && let Some(desc) = non_empty(input, "description")
        {
            let e = self.push(EventKind::AssistantMessage);
            e.text = Some(excerpt(desc));
            e.evidence = EvidenceLabel::Declared;
        }
    }

    /// `PostToolBatch` carries every call's result as "the string the model
    /// saw". We read ONLY the edits the harness rejected before running
    /// (`<tool_use_error>…`): they fire no Pre/Post at all, so this is their
    /// only trace. Every other entry is already covered by its own Post event,
    /// and emitting it too would overwrite the real outcome in the timeline.
    fn rejected_edits(&mut self, p: &Value) {
        let Some(calls) = p.get("tool_calls").and_then(Value::as_array) else {
            ignore("PostToolBatch without tool_calls");
            return;
        };
        for call in calls {
            let Some(name) = non_empty(call, "tool_name") else {
                continue;
            };
            let input = call.get("tool_input").unwrap_or(&Value::Null);
            if classify(name, input) != ToolClass::Edit {
                continue;
            }
            let Some(error) = call.get("tool_response").and_then(tool_use_error) else {
                continue;
            };
            let use_id = non_empty(call, "tool_use_id").map(str::to_owned);
            let files: Vec<String> = target_path(input).map(str::to_owned).into_iter().collect();
            let e = self.push(EventKind::ToolFinished {
                outcome: ToolOutcome::Failed,
            });
            e.tool = Some(ToolCall {
                use_id,
                name: name.to_owned(),
                class: ToolClass::Edit,
            });
            e.files = files;
            e.text = Some(excerpt(&format!(
                "rejected by Claude Code: {}",
                first_line(error)
            )));
        }
    }
}

/// The op of a successful edit-class `PostToolUse` (empty when the path is unknown).
///
/// - A payload the hook reduced says an edit happened but not what it did:
///   `Unobserved(PayloadReduced)`, and only when the hook kept the path
///   (`why_wire::reduced` keeps `tool_input.file_path` for a truncated payload,
///   never for a redacted one, which may touch a credential file).
/// - Otherwise `Write`/`Edit` are read exactly; any other edit-class tool
///   (`NotebookEdit`) or an unreadable response is `Unobserved(Unparsed)`.
fn edit_ops(p: &Value, tool_name: &str, response: &Value, path: Option<&str>) -> Vec<EditOp> {
    let Some(path) = path else {
        return Vec::new();
    };
    match reduction_of(p) {
        Some(Reduction::Redacted) => Vec::new(),
        Some(Reduction::Truncated) => {
            vec![edit_op::unobserved(path, UnobservedReason::PayloadReduced)]
        }
        None => vec![edit_op::from_response(tool_name, path, response)],
    }
}

/// How the timeline should show a tool, from its name and (for Bash) command.
/// Identical for every phase, because the timeline fixes a row's kind from
/// whichever half arrives first.
pub fn classify(tool_name: &str, tool_input: &Value) -> ToolClass {
    match tool_name {
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => ToolClass::Edit,
        "Bash" => {
            if non_empty(tool_input, "command").is_some_and(shell::is_test_run) {
                ToolClass::TestRun
            } else {
                ToolClass::Shell
            }
        }
        "Agent" | "Task" => ToolClass::Subagent,
        _ => ToolClass::Other,
    }
}
