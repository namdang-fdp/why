//! Shared helpers (Codex): load the Codex golden fixtures and replay them.
//!
//! `adapters::fixtures` is `cfg(test)` inside the library, invisible from an
//! integration test, so this is a small copy of that loader.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use why_lib::adapters;
use why_lib::contracts::SessionTimeline;
use why_lib::events::timeline::build_timeline;
use why_lib::events::{NormalizedEvent, SessionKey};
use why_wire::{Envelope, Harness, PROTOCOL_VERSION};

pub fn codex_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents/codex/0.170.0")
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, out);
        } else if p.file_name().is_some_and(|n| n == "hooks.jsonl") {
            out.push(p);
        }
    }
}

/// `(scenario name, hooks.jsonl path)`; the name is the folder path under the
/// fixture root with `/` as `__` (`behavior__timeout`).
pub fn scenarios() -> Vec<(String, PathBuf)> {
    let root = codex_root();
    let mut files = Vec::new();
    collect(&root, &mut files);
    files
        .into_iter()
        .map(|p| {
            let rel = p.parent().unwrap().strip_prefix(&root).unwrap();
            (rel.to_string_lossy().replace('/', "__"), p)
        })
        .collect()
}

pub fn envelope(payload: Value, at_ms: u64) -> Envelope {
    Envelope {
        v: PROTOCOL_VERSION,
        harness: Harness::Codex,
        harness_version: None,
        event: payload["hook_event_name"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        received_at_ms: at_ms,
        cwd: payload["cwd"].as_str().unwrap_or("/repo").to_owned(),
        pid: 0,
        payload,
    }
}

pub fn load(path: &Path) -> Vec<Envelope> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let rec: Value = serde_json::from_str(l).unwrap();
            let wall = rec["wall"].as_f64().unwrap();
            envelope(rec["stdin"].clone(), (wall * 1000.0) as u64)
        })
        .collect()
}

/// Normalize every envelope (in arrival order), grouped by session.
pub fn replay(envelopes: &[Envelope]) -> BTreeMap<SessionKey, Vec<NormalizedEvent>> {
    let mut sessions: BTreeMap<SessionKey, Vec<NormalizedEvent>> = BTreeMap::new();
    for env in envelopes {
        for ev in adapters::normalize(env) {
            sessions.entry(ev.session.clone()).or_default().push(ev);
        }
    }
    sessions
}

pub fn timelines(envelopes: &[Envelope]) -> Vec<SessionTimeline> {
    replay(envelopes)
        .into_iter()
        .filter_map(|(key, events)| build_timeline(&key, &events, None))
        .collect()
}
