//! Loads the WHY-45 golden fixtures (`tests/fixtures/agents/`) as the
//! [`Envelope`]s the hook would have sent, for adapter tests.
//!
//! Each `hooks.jsonl` line is a capture record:
//! `{event, harness, argv, env, cwd, mono_ns, wall, stdin}` where `stdin` is the
//! harness payload and `wall` the capture time in (fractional) seconds.

use std::path::{Path, PathBuf};

use serde_json::Value;
use why_wire::{Envelope, Harness, PROTOCOL_VERSION};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents")
}

/// Every `hooks.jsonl` under `dir`, recursively, sorted for stable output.
fn hook_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            hook_files(&path, out);
        } else if path.file_name().is_some_and(|n| n == "hooks.jsonl") {
            out.push(path);
        }
    }
}

/// The capture tool spells harnesses `claude` / `codex`; the hook's own
/// spelling is `claude-code` / `codex`. Mapped explicitly so a new capture
/// spelling fails here instead of silently becoming `Unknown`.
fn harness(capture_name: &str) -> Harness {
    match capture_name {
        "claude" => Harness::ClaudeCode,
        "codex" => Harness::Codex,
        other => panic!("fixture has unknown harness {other:?}"),
    }
}

fn envelope(record: &Value) -> Envelope {
    let payload = record["stdin"].clone();
    Envelope {
        v: PROTOCOL_VERSION,
        harness: harness(record["harness"].as_str().expect("harness")),
        harness_version: None,
        event: payload["hook_event_name"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        received_at_ms: (record["wall"].as_f64().expect("wall") * 1000.0) as u64,
        cwd: record["cwd"].as_str().expect("cwd").to_owned(),
        pid: 0,
        payload,
    }
}

/// `(file, envelopes)` for every fixture of one harness directory
/// (`"claude"` or `"codex"`), or of all of them with `""`.
pub fn load(harness_dir: &str) -> Vec<(PathBuf, Vec<Envelope>)> {
    let mut files = Vec::new();
    hook_files(&root().join(harness_dir), &mut files);
    assert!(!files.is_empty(), "no hooks.jsonl under {harness_dir:?}");
    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).unwrap();
            let envelopes = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| envelope(&serde_json::from_str(l).unwrap()))
                .collect();
            (path, envelopes)
        })
        .collect()
}

pub fn all() -> Vec<(PathBuf, Vec<Envelope>)> {
    load("")
}
