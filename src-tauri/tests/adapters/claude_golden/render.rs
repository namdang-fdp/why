//! Shared snapshot format for the replay test and the regeneration command.

use serde_json::{Value, json};
use why_lib::adapters::claude::extract::is_synthetic_prompt;
use why_lib::contracts::{SessionTimeline, TimelineEvent, ToolOutcome};

fn kind_name(e: &TimelineEvent) -> String {
    serde_json::to_value(e.kind)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

fn outcome_name(o: Option<ToolOutcome>) -> &'static str {
    match o {
        None => "-",
        Some(ToolOutcome::Ok) => "ok",
        Some(ToolOutcome::Failed) => "failed",
        Some(ToolOutcome::Unknown) => "unknown",
    }
}

/// `kind tool outcome files [agent]`, stable across runs (no ids, no times).
fn event_line(e: &TimelineEvent) -> String {
    let evidence = serde_json::to_value(e.evidence).unwrap();
    format!(
        "{} {} {} [{}] {}{}",
        kind_name(e),
        e.tool_name.as_deref().unwrap_or("-"),
        outcome_name(e.outcome),
        e.files.join(","),
        evidence.as_str().unwrap(),
        e.agent_id
            .as_deref()
            .map(|a| format!(" agent={}", &a[..a.len().min(4)]))
            .unwrap_or_default(),
    )
}

pub fn render(timelines: &[SessionTimeline]) -> Value {
    json!(
        timelines
            .iter()
            .map(|t| json!({
                "status": serde_json::to_value(t.session.status).unwrap(),
                "turns": t.turns.iter().map(|turn| json!({
                    "prompt": turn.prompt.as_deref().map(|p| p.chars().take(40).collect::<String>()),
                    "synthetic": turn.prompt.as_deref().is_some_and(is_synthetic_prompt),
                    "closed": turn.ended_at.is_some(),
                    "events": turn.events.iter().map(event_line).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>()
    )
}
