//! Codex adapter: hook payloads (Codex 0.170.0) → [`NormalizedEvent`]s.
//!
//! Everything here is a pure function of the envelope (no I/O, no clock). The
//! only file access of the Codex integration lives in [`rollout`], which is a
//! separate, explicitly lazy reader that nothing in ingest calls yet.
//!
//! # Mapping (payloads.md §3) and why
//!
//! | Hook event | Event(s) | Notes / why |
//! |---|---|---|
//! | `SessionStart` | `SessionStart{resumed}` | `source: resume` is the SAME session (Codex keeps the id and rollout across `exec resume`) |
//! | `SessionEnd` | `SessionEnd` | one per process, not "conversation over" |
//! | `UserPromptSubmit` | `Prompt` | text capped; `turn_id` is the turn key |
//! | `PreToolUse` | `ToolStarted` | correlation key = `tool_use_id` |
//! | `PostToolUse` | `ToolFinished` | outcome: see below |
//! | `Stop` | `AssistantMessage` (Declared) + `Stop` | Codex narrates before acting: the message is the closest thing to a stated rationale, so it is labelled *Declared* |
//! | `SubagentStart` / `SubagentStop` | `SubagentStarted` / `SubagentStopped` | `agent_id`; the hook reuses the parent's `session_id` |
//! | `Pre/PostModelRequest` | nothing | one pair per model call: chatty, not needed |
//! | anything else | nothing | unknown or not-yet-seen events must never break ingest |
//!
//! # Outcomes: only what the payload proves
//!
//! * `apply_patch` + `PostToolUse` whose response starts `Exit code: 0` and
//!   contains `Success` → `Ok`; a different explicit `Exit code: N` → `Failed`
//!   (the tool itself said so); anything unparseable → `Unknown`.
//! * `Bash` + `PostToolUse` → **`Unknown`**. The hook has no exit code and
//!   Codex has no failure event (verified for `exit 3` and a failing
//!   `unittest`), so a successful-looking `PostToolUse` proves nothing. Output
//!   text is never used to guess (a test that prints "FAILED" can exit 0 and
//!   `cmd; echo done` hides a failure). The rollout reader can upgrade this
//!   later with real evidence.
//! * Any other tool's `PostToolUse` → `Unknown` too: no field says it worked.
//! * `PreToolUse` with no `PostToolUse` is `Unknown` (rejected patch OR lost
//!   delivery); the timeline builder applies that, not this adapter.
//!
//! # Not decrypted, ever
//!
//! `collaborationspawn_agent.message` is an encrypted blob. It is replaced by
//! the literal `(encrypted)`; WHY neither decrypts nor tries to guess it.

pub mod classify;
pub mod patch;
pub mod rollout;

use std::path::Path;

use serde_json::Value;
use why_wire::Envelope;

use super::Adapter;
use crate::contracts::{EvidenceLabel, ToolOutcome};
use crate::events::{EventKind, NormalizedEvent, ToolCall, ToolClass, excerpt};
use patch::{FileEdit, ParsedPatch, PatchOp};

/// Shown instead of any encrypted field.
pub const ENCRYPTED: &str = "(encrypted)";

pub struct CodexAdapter;

fn str_field<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key)?.as_str()
}

/// Non-empty string field, owned.
fn id_field(payload: &Value, key: &str) -> Option<String> {
    str_field(payload, key)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Does `s` look like a Fernet token (what Codex uses for encrypted content:
/// `gAAAA…`, URL-safe base64, long)? A heuristic used ONLY to decide whether a
/// string is shown; it never touches the content.
pub fn looks_encrypted(s: &str) -> bool {
    s.len() >= 40
        && s.starts_with("gAAAA")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'=')
}

/// `path` as an absolute path: a relative one (seen in a linked worktree:
/// `*** Add File: src/cx_wt.py`) is joined to the event's cwd, the directory
/// the patch is applied against. Plain string work: no filesystem access and
/// no `..` resolution (the ingest step resolves paths with git).
fn absolutize(path: &str, cwd: &str) -> String {
    Path::new(cwd).join(path).to_string_lossy().into_owned()
}

/// `A src/x.py (+3)`, `M a.py (+2 -1)`, `D b.py`, `R a.py -> b.py (+1)`, one
/// line per file. Paths are shown as the patch wrote them (short, readable).
fn summarize_edit(edit: &FileEdit) -> String {
    let counts = match (edit.added(), edit.removed()) {
        (0, 0) => String::new(),
        (a, 0) => format!(" (+{a})"),
        (0, r) => format!(" (-{r})"),
        (a, r) => format!(" (+{a} -{r})"),
    };
    match (edit.op, &edit.move_to) {
        (PatchOp::Add, _) => format!("A {}{counts}", edit.path),
        (PatchOp::Delete, _) => format!("D {}", edit.path),
        (PatchOp::Update, Some(to)) => format!("R {} -> {to}{counts}", edit.path),
        (PatchOp::Update, None) => format!("M {}{counts}", edit.path),
    }
}

fn summarize_patch(parsed: &ParsedPatch) -> String {
    if parsed.edits.is_empty() {
        // The attempt happened, but nothing in it could be read.
        return "apply_patch (patch not understood)".to_owned();
    }
    let lines: Vec<String> = parsed.edits.iter().map(summarize_edit).collect();
    excerpt(&lines.join("\n"))
}

/// Files a patch touches (both ends of a rename), absolute, without
/// duplicates, in patch order.
fn patch_files(parsed: &ParsedPatch, cwd: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for e in &parsed.edits {
        for p in std::iter::once(&e.path).chain(e.move_to.as_ref()) {
            if p.is_empty() {
                continue;
            }
            let abs = absolutize(p, cwd);
            if !files.contains(&abs) {
                files.push(abs);
            }
        }
    }
    files
}

fn classify_tool(name: &str, input: &Value) -> ToolClass {
    match name {
        "apply_patch" | "Edit" | "Write" => ToolClass::Edit,
        "Bash" => {
            let command = input.get("command").and_then(Value::as_str).unwrap_or("");
            if classify::is_test_command(command) {
                ToolClass::TestRun
            } else {
                ToolClass::Shell
            }
        }
        // Sic: Codex 0.170 spells it with no separator.
        "collaborationspawn_agent" | "spawn_agent" => ToolClass::Subagent,
        _ => ToolClass::Other,
    }
}

/// Outcome of an `apply_patch` result string (`tool_response`).
///
/// Observed shape: `Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess. Updated
/// the following files:\nA <path>\n`. Only an explicit `Exit code:` line is
/// believed.
fn patch_outcome(response: Option<&Value>) -> ToolOutcome {
    let Some(text) = response.and_then(Value::as_str) else {
        return ToolOutcome::Unknown;
    };
    let Some(rest) = text.strip_prefix("Exit code:") else {
        return ToolOutcome::Unknown;
    };
    let code_text = rest.lines().next().unwrap_or("").trim();
    match code_text.parse::<i64>() {
        Ok(0) if text.contains("Success") => ToolOutcome::Ok,
        // Exit 0 without the success line is not what we know; do not guess.
        Ok(0) => ToolOutcome::Unknown,
        Ok(_) => ToolOutcome::Failed,
        Err(_) => ToolOutcome::Unknown,
    }
}

/// Text and files shown for a tool call, by tool.
fn describe_tool(name: &str, input: &Value, cwd: &str) -> (Option<String>, Vec<String>) {
    match name {
        "apply_patch" => match input.get("command").and_then(Value::as_str) {
            Some(patch_text) => {
                let parsed = patch::parse_patch(patch_text);
                (Some(summarize_patch(&parsed)), patch_files(&parsed, cwd))
            }
            None => (Some("apply_patch (no patch text)".to_owned()), Vec::new()),
        },
        "Bash" => (
            input.get("command").and_then(Value::as_str).map(excerpt),
            Vec::new(),
        ),
        "collaborationspawn_agent" | "spawn_agent" => {
            let task = input
                .get("task_name")
                .and_then(Value::as_str)
                .unwrap_or("subagent");
            // The message is an encrypted blob in 0.170. If a future version
            // sends plain text it is shown (capped); a blob never is.
            let message = match input.get("message").and_then(Value::as_str) {
                Some(m) if looks_encrypted(m) => Some(ENCRYPTED.to_owned()),
                Some(m) => Some(excerpt(m)),
                None => None,
            };
            let text = match message {
                Some(m) => format!("{task}: {m}"),
                None => task.to_owned(),
            };
            (Some(excerpt(&text)), Vec::new())
        }
        // Other tools (wait_agent, MCP, ...): the name says enough; their
        // inputs may hold anything, so nothing is copied.
        _ => (None, Vec::new()),
    }
}

impl CodexAdapter {
    /// An event with the fields common to every Codex payload filled in.
    fn base(env: &Envelope, index: u32, session_id: &str, kind: EventKind) -> NormalizedEvent {
        let p = &env.payload;
        let mut ev = NormalizedEvent::from_envelope(env, index, session_id, kind);
        ev.turn_id = id_field(p, "turn_id");
        ev.agent_id = id_field(p, "agent_id");
        ev.model = id_field(p, "model");
        ev.transcript_path = id_field(p, "transcript_path");
        // The payload's own cwd wins (it is the session's), the envelope's is
        // the hook process's (equal in every fixture).
        if let Some(cwd) = id_field(p, "cwd") {
            ev.cwd = cwd;
        }
        ev
    }

    fn tool_event(env: &Envelope, session_id: &str, finished: bool) -> Vec<NormalizedEvent> {
        let p = &env.payload;
        // Without a tool name there is nothing to classify or show.
        let Some(name) = id_field(p, "tool_name") else {
            return Vec::new();
        };
        let empty = Value::Null;
        let input = p.get("tool_input").unwrap_or(&empty);
        let kind = if finished {
            let outcome = if name == "apply_patch" {
                patch_outcome(p.get("tool_response"))
            } else {
                ToolOutcome::Unknown
            };
            EventKind::ToolFinished { outcome }
        } else {
            EventKind::ToolStarted
        };
        let mut ev = Self::base(env, 0, session_id, kind);
        let (text, files) = describe_tool(&name, input, &ev.cwd);
        ev.text = text;
        ev.files = files;
        ev.tool = Some(ToolCall {
            use_id: id_field(p, "tool_use_id"),
            class: classify_tool(&name, input),
            name,
        });
        vec![ev]
    }
}

impl Adapter for CodexAdapter {
    fn normalize(&self, env: &Envelope) -> Vec<NormalizedEvent> {
        let p = &env.payload;
        // Prefer the payload's own event name; the envelope's copy is the
        // fallback (the hook fills both).
        let hook = str_field(p, "hook_event_name").unwrap_or(&env.event);
        // Every Codex payload carries `session_id`; without it the event
        // cannot be filed anywhere.
        let Some(session_id) = id_field(p, "session_id") else {
            return Vec::new();
        };
        let sid = session_id.as_str();

        match hook {
            "SessionStart" => {
                let resumed = str_field(p, "source") == Some("resume");
                vec![Self::base(env, 0, sid, EventKind::SessionStart { resumed })]
            }
            "SessionEnd" => vec![Self::base(env, 0, sid, EventKind::SessionEnd)],
            "UserPromptSubmit" => {
                let mut ev = Self::base(env, 0, sid, EventKind::Prompt);
                ev.text = str_field(p, "prompt").map(excerpt);
                vec![ev]
            }
            "PreToolUse" => Self::tool_event(env, sid, false),
            "PostToolUse" => Self::tool_event(env, sid, true),
            "Stop" => {
                let mut out = Vec::new();
                // The agent's closing words, in its own voice: Declared.
                if let Some(msg) = str_field(p, "last_assistant_message").filter(|m| !m.is_empty())
                {
                    let mut said = Self::base(env, 0, sid, EventKind::AssistantMessage);
                    said.text = Some(excerpt(msg));
                    said.evidence = EvidenceLabel::Declared;
                    out.push(said);
                }
                // `index` = position in the returned vector (EventKey rule).
                out.push(Self::base(env, out.len() as u32, sid, EventKind::Stop));
                out
            }
            "SubagentStart" => {
                let mut ev = Self::base(env, 0, sid, EventKind::SubagentStarted);
                ev.text = str_field(p, "agent_type")
                    .filter(|s| !s.is_empty())
                    .map(excerpt);
                vec![ev]
            }
            "SubagentStop" => {
                let mut ev = Self::base(env, 0, sid, EventKind::SubagentStopped);
                // `transcript_path` points to the parent, while `turn_id` belongs to the child.
                ev.transcript_path = id_field(p, "agent_transcript_path");
                if let Some(msg) = str_field(p, "last_assistant_message").filter(|m| !m.is_empty())
                {
                    ev.text = Some(excerpt(msg));
                    ev.evidence = EvidenceLabel::Declared;
                }
                vec![ev]
            }
            // Pre/PostModelRequest (chatty), PermissionRequest, Pre/PostCompact,
            // Interrupt and whatever a newer Codex adds: ignored without error.
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use why_wire::{Harness, PROTOCOL_VERSION};

    use super::*;

    fn env(payload: Value) -> Envelope {
        Envelope {
            v: PROTOCOL_VERSION,
            harness: Harness::Codex,
            harness_version: None,
            event: payload["hook_event_name"].as_str().unwrap_or("").to_owned(),
            received_at_ms: 1,
            cwd: "/repo".into(),
            pid: 1,
            payload,
        }
    }

    #[test]
    fn patch_outcome_only_trusts_an_explicit_exit_code() {
        let ok = json!(
            "Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess. Updated the following files:\nA x\n"
        );
        assert_eq!(patch_outcome(Some(&ok)), ToolOutcome::Ok);
        let failed = json!("Exit code: 1\nWall time: 0 seconds\nOutput:\nerror\n");
        assert_eq!(patch_outcome(Some(&failed)), ToolOutcome::Failed);
        for unknown in [
            json!("Exit code: 0\nOutput:\nnothing to say"),
            json!("Success"),
            json!("Exit code: x"),
            json!(""),
            json!(7),
            Value::Null,
        ] {
            assert_eq!(
                patch_outcome(Some(&unknown)),
                ToolOutcome::Unknown,
                "{unknown}"
            );
        }
        assert_eq!(patch_outcome(None), ToolOutcome::Unknown);
    }

    #[cfg(unix)]
    #[test]
    fn relative_patch_paths_join_the_cwd() {
        assert_eq!(absolutize("src/a.py", "/wt"), "/wt/src/a.py");
        assert_eq!(absolutize("/abs/a.py", "/wt"), "/abs/a.py");
        assert_eq!(absolutize("C:\\x\\a.py", "/wt"), "/wt/C:\\x\\a.py");
        assert_eq!(absolutize("a:notes.py", "/wt/src"), "/wt/src/a:notes.py");
        assert_eq!(absolutize("a.py", "/wt/"), "/wt/a.py");
        assert_eq!(absolutize("a.py", ""), "a.py");
    }

    #[test]
    fn empty_patch_paths_do_not_attribute_the_working_directory() {
        let events = CodexAdapter.normalize(&env(json!({
            "hook_event_name": "PreToolUse", "session_id": "s", "tool_name": "apply_patch",
            "tool_input": {"command": "*** Begin Patch\n*** Add File:\n+x\n*** Update File: a.py\n*** Move to:   \n-x\n+y\n*** End Patch"},
        })));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::ToolStarted);
        assert_eq!(events[0].files, [absolutize("a.py", "/repo")]);
    }

    #[test]
    fn stop_without_a_message_yields_only_stop() {
        let events = CodexAdapter.normalize(&env(
            json!({"hook_event_name":"Stop","session_id":"s","turn_id":"t","last_assistant_message":null}),
        ));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Stop);
    }

    #[test]
    fn encrypted_blobs_are_recognised_but_plain_text_is_not() {
        let blob = format!("gAAAAAB{}", "x".repeat(60));
        assert!(looks_encrypted(&blob));
        assert!(!looks_encrypted("gAAAA"));
        assert!(!looks_encrypted("please create the file"));
    }
}
