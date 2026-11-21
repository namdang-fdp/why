//! Shared helpers: fixture loading, envelopes, and the replay pipeline.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use why_lib::adapters;
use why_lib::contracts::{SessionTimeline, WorktreeId};
use why_lib::events::ingest::{ResolvedWorktree, WorktreeResolver, ingest};
use why_lib::events::{EventStore, MemoryEventStore, NormalizedEvent, SessionKey};
use why_wire::{Envelope, Harness, PROTOCOL_VERSION};

pub fn claude_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents/claude/2.1.322")
}

pub fn golden_path(scenario: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/adapters/golden/claude")
        .join(format!("{}.json", scenario.replace(['/', '\\'], "__")))
}

/// A Claude hook envelope with a payload of the test's choosing.
pub fn envelope(at_ms: u64, payload: Value) -> Envelope {
    Envelope {
        v: PROTOCOL_VERSION,
        harness: Harness::ClaudeCode,
        harness_version: None,
        event: payload["hook_event_name"].as_str().unwrap_or("").to_owned(),
        received_at_ms: at_ms,
        cwd: "/TMP/fixture-repo".into(),
        pid: 0,
        payload,
    }
}

/// `(scenario name, envelopes in arrival order)` for every Claude fixture.
/// The scenario name is the folder relative to `claude/2.1.322`.
pub fn scenarios() -> Vec<(String, Vec<Envelope>)> {
    let mut files = Vec::new();
    collect(&claude_root(), &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path
                .parent()
                .unwrap()
                .strip_prefix(claude_root())
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let text = std::fs::read_to_string(&path).unwrap();
            let envs = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| {
                    let rec: Value = serde_json::from_str(l).unwrap();
                    let payload = rec["stdin"].clone();
                    Envelope {
                        v: PROTOCOL_VERSION,
                        harness: Harness::ClaudeCode,
                        harness_version: None,
                        event: payload["hook_event_name"].as_str().unwrap_or("").to_owned(),
                        received_at_ms: (rec["wall"].as_f64().unwrap() * 1000.0) as u64,
                        cwd: rec["cwd"].as_str().unwrap().to_owned(),
                        pid: 0,
                        payload,
                    }
                })
                .collect();
            (name, envs)
        })
        .collect()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.file_name().is_some_and(|n| n == "hooks.jsonl") {
            out.push(path);
        }
    }
}

/// Resolves the fixture cwds like git would for the toy repo and its linked
/// worktree (root, not the cwd: the `subdir` scenario runs in `src/`).
pub struct FixtureResolver;

impl WorktreeResolver for FixtureResolver {
    async fn resolve(&self, cwd: &str) -> Option<ResolvedWorktree> {
        let root = if Path::new(cwd).starts_with("/TMP/fixture-wt") {
            "/TMP/fixture-wt"
        } else if Path::new(cwd).starts_with("/TMP/fixture-repo") {
            "/TMP/fixture-repo"
        } else {
            return None;
        };
        Some(ResolvedWorktree {
            id: WorktreeId(root.into()),
            root: root.into(),
        })
    }
}

/// Normalize each envelope (checking `index` = position, which the dedupe key
/// relies on) and return the events in arrival order.
pub fn normalize_all(envs: &[Envelope]) -> Vec<NormalizedEvent> {
    let mut all = Vec::new();
    for env in envs {
        let events = adapters::normalize(env);
        for (i, ev) in events.iter().enumerate() {
            assert_eq!(
                ev.key,
                why_lib::events::EventKey::for_delivery(env, i as u32),
                "event key index must be the position in the returned vector"
            );
        }
        all.extend(events);
    }
    all
}

/// Replay through the real ingest path (resolver makes files repo-relative)
/// into a memory store and return every session's timeline, sorted by id.
pub async fn replay(envs: &[Envelope]) -> Vec<SessionTimeline> {
    let store = MemoryEventStore::new();
    let mut keys: Vec<SessionKey> = Vec::new();
    for env in envs {
        for ev in adapters::normalize(env) {
            if !keys.contains(&ev.session) {
                keys.push(ev.session.clone());
            }
        }
        ingest(env, &FixtureResolver, &store).await.unwrap();
    }
    keys.sort();
    keys.iter()
        .map(|k| store.timeline(k).unwrap().expect("session stored"))
        .collect()
}

/// Minimal payload builders for hand-made scenarios.
pub fn base(hook: &str) -> Value {
    json!({
        "session_id": "s1",
        "transcript_path": "/HOME/.claude/projects/-TMP-r/s1.jsonl",
        "cwd": "/TMP/fixture-repo",
        "prompt_id": "p1",
        "hook_event_name": hook,
    })
}

pub fn with(mut v: Value, extra: Value) -> Value {
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    v
}
