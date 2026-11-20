//! Snapshot formatting shared by the read-only test and the update command.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use why_lib::contracts::SessionTimeline;

use crate::codex_common::{load, timelines};

pub fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/adapters/codex_golden")
        .join(format!("{name}.txt"))
}

pub fn snapshot(hooks: &Path) -> String {
    render(&timelines(&load(hooks)))
}

/// Stable text form of timelines (no timestamps: they are fixture-recorded and
/// say nothing about the adapter).
fn render(timelines: &[SessionTimeline]) -> String {
    let mut out = String::new();
    for tl in timelines {
        let s = &tl.session;
        writeln!(
            out,
            "session {} model={:?} status={:?} turns={}",
            s.id.0, s.model, s.status, s.turn_count
        )
        .unwrap();
        for turn in &tl.turns {
            writeln!(
                out,
                "  turn {} id={} prompt={:?} ended={}",
                turn.index,
                turn.id,
                turn.prompt
                    .as_deref()
                    .map(|p| p.chars().take(50).collect::<String>()),
                turn.ended_at.is_some()
            )
            .unwrap();
            for e in &turn.events {
                let summary: String = e.summary.replace('\n', " | ").chars().take(110).collect();
                writeln!(
                    out,
                    "    {:?} {:?} tool={:?} agent={} evidence={:?} files={:?} :: {}",
                    e.kind,
                    e.outcome,
                    e.tool_name,
                    e.agent_id
                        .as_deref()
                        .map(|a| &a[a.len() - 4..])
                        .unwrap_or("-"),
                    e.evidence,
                    e.files,
                    summary
                )
                .unwrap();
            }
        }
    }
    out
}
