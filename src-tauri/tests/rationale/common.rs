//! Shared rig: temp harness roots, fixture replay and both event stores.

use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use tempfile::TempDir;
use why_lib::adapters;
use why_lib::contracts::{
    IpcError, RationaleBlock, RationaleResult, SessionId, ToolOutcome, WorktreeId,
};
use why_lib::events::{
    EventKind, EventStore, MemoryEventStore, NormalizedEvent, SessionKey, SqliteEventStore,
    ToolCall, ToolClass,
};
use why_lib::git::GitRunner;
use why_lib::ipc::rationale::RationaleRoots;
use why_lib::state::AppState;
use why_lib::store::MemoryRepoStore;
use why_wire::{Envelope, Harness, PROTOCOL_VERSION};

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents")
}

pub fn claude_fixture(scenario: &str) -> PathBuf {
    fixtures().join("claude/2.1.322").join(scenario)
}

pub fn codex_fixture(scenario: &str) -> PathBuf {
    fixtures().join("codex/0.170.0").join(scenario)
}

/// Fake harness directories: `<tmp>/claude/projects` and `<tmp>/codex/sessions`.
pub struct Rig {
    pub dir: TempDir,
    pub roots: RationaleRoots,
}

impl Rig {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join("claude/projects");
        let codex = dir.path().join("codex/sessions");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::create_dir_all(&codex).unwrap();
        Self {
            dir,
            roots: RationaleRoots {
                claude_projects: Some(claude),
                codex_sessions: Some(codex),
            },
        }
    }

    pub fn claude_root(&self) -> &Path {
        self.roots.claude_projects.as_deref().unwrap()
    }

    pub fn codex_root(&self) -> &Path {
        self.roots.codex_sessions.as_deref().unwrap()
    }

    /// Write `bytes` as `<claude root>/-p/<name>`.
    pub fn claude_file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.claude_root().join("-p").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// Write `bytes` as `<codex root>/2026/11/20/<name>`.
    pub fn codex_file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.codex_root().join("2026/11/20").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

/// Replay a fixture's `hooks.jsonl` through the real adapters, in arrival
/// order. Events come back in ONE vector (a fixture may hold several sessions).
pub fn replay(hooks: &Path) -> Vec<NormalizedEvent> {
    std::fs::read_to_string(hooks)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .flat_map(|line| {
            let rec: Value = serde_json::from_str(line).unwrap();
            let wall = rec["wall"].as_f64().unwrap();
            let payload = rec["stdin"].clone();
            let harness = match rec["harness"].as_str().unwrap() {
                "codex" => Harness::Codex,
                _ => Harness::ClaudeCode,
            };
            let env = Envelope {
                v: PROTOCOL_VERSION,
                harness,
                harness_version: None,
                event: payload["hook_event_name"].as_str().unwrap().to_owned(),
                received_at_ms: (wall * 1000.0) as u64,
                cwd: payload["cwd"].as_str().unwrap_or("/repo").to_owned(),
                pid: 0,
                payload,
            };
            adapters::normalize(&env)
        })
        .collect()
}

/// Point every event's `transcript_path` at the temp copy `map` names for the
/// recorded (scrubbed) path; a path `map` does not know is left alone.
pub fn rewrite_paths(events: &mut [NormalizedEvent], map: impl Fn(&str) -> Option<PathBuf>) {
    for ev in events {
        if let Some(path) = &ev.transcript_path
            && let Some(new) = map(path)
        {
            ev.transcript_path = Some(new.to_string_lossy().into_owned());
        }
    }
}

/// A hand-built Codex or Claude event. `id` makes it a tool event.
pub struct Ev<'a> {
    pub harness: Harness,
    pub session: &'a str,
    pub hook: &'a str,
    pub kind: EventKind,
    pub at: u64,
    pub turn: Option<&'a str>,
    pub agent: Option<&'a str>,
    pub tool: Option<(&'a str, ToolClass)>,
    pub path: Option<&'a str>,
}

impl<'a> Ev<'a> {
    pub fn new(harness: Harness, session: &'a str, hook: &'a str, kind: EventKind) -> Self {
        Self {
            harness,
            session,
            hook,
            kind,
            at: 1_000,
            turn: None,
            agent: None,
            tool: None,
            path: None,
        }
    }

    pub fn at(mut self, at: u64) -> Self {
        self.at = at;
        self
    }

    pub fn turn(mut self, turn: &'a str) -> Self {
        self.turn = Some(turn);
        self
    }

    pub fn path(mut self, path: &'a str) -> Self {
        self.path = Some(path);
        self
    }

    pub fn tool(mut self, id: &'a str, class: ToolClass) -> Self {
        self.tool = Some((id, class));
        self
    }

    pub fn build(self) -> NormalizedEvent {
        let env = Envelope {
            v: PROTOCOL_VERSION,
            harness: self.harness,
            harness_version: None,
            event: self.hook.into(),
            received_at_ms: self.at,
            cwd: "/repo".into(),
            pid: 1,
            payload: json!({
                "session_id": self.session, "hook_event_name": self.hook,
                "turn_id": self.turn, "tool_use_id": self.tool.map(|t| t.0),
                "kind": format!("{:?}", self.kind), "agent": self.agent,
            }),
        };
        let mut ev = NormalizedEvent::from_envelope(&env, 0, self.session, self.kind);
        ev.turn_id = self.turn.map(str::to_owned);
        ev.agent_id = self.agent.map(str::to_owned);
        ev.worktree_id = Some(WorktreeId("wt-1".into()));
        ev.transcript_path = self.path.map(str::to_owned);
        ev.tool = self.tool.map(|(id, class)| ToolCall {
            use_id: Some(id.into()),
            name: "Bash".into(),
            class,
        });
        ev
    }
}

/// A started-and-finished shell call (a Pre and a Post).
pub fn shell_call(
    harness: Harness,
    session: &str,
    turn: &str,
    id: &str,
    outcome: Option<ToolOutcome>,
    at: u64,
    path: Option<&str>,
) -> Vec<NormalizedEvent> {
    let mut start = Ev::new(harness, session, "PreToolUse", EventKind::ToolStarted)
        .at(at)
        .turn(turn)
        .tool(id, ToolClass::Shell);
    start.path = path;
    let mut out = vec![start.build()];
    if let Some(outcome) = outcome {
        let mut end = Ev::new(
            harness,
            session,
            "PostToolUse",
            EventKind::ToolFinished { outcome },
        )
        .at(at + 1)
        .turn(turn)
        .tool(id, ToolClass::Shell);
        end.path = path;
        out.push(end.build());
    }
    out
}

/// A prompt event of `turn` (so the turn exists), carrying `path`.
pub fn prompt(
    harness: Harness,
    session: &str,
    turn: &str,
    at: u64,
    path: Option<&str>,
) -> NormalizedEvent {
    let mut ev = Ev::new(harness, session, "UserPromptSubmit", EventKind::Prompt)
        .at(at)
        .turn(turn);
    ev.path = path;
    ev.build()
}

/// One state per event store, labelled; the SQLite one keeps its temp dir.
pub struct Stores {
    pub states: Vec<(&'static str, AppState)>,
    _dir: TempDir,
}

pub fn stores(events: &[NormalizedEvent]) -> Stores {
    let dir = tempfile::tempdir().unwrap();
    let memory: Arc<dyn EventStore> = Arc::new(MemoryEventStore::new());
    let sqlite: Arc<dyn EventStore> =
        Arc::new(SqliteEventStore::open(&dir.path().join("events.db")).unwrap());
    let states = [("memory", memory), ("sqlite", sqlite)]
        .into_iter()
        .map(|(label, store)| {
            store.append(events).unwrap();
            let state = AppState::new(
                GitRunner::new().with_isolated_config(),
                Arc::new(MemoryRepoStore::new()),
            )
            .with_events(store);
            (label, state)
        })
        .collect();
    Stores { states, _dir: dir }
}

/// Run `ask` on every store and assert the answers are identical; return one.
pub async fn on_both<T, F, Fut>(events: &[NormalizedEvent], ask: F) -> T
where
    T: PartialEq + Debug,
    F: Fn(AppState) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    // `AppState` is not Clone, so the closure gets it by reference through a
    // fresh state per store (stores are cheap).
    let all = stores(events);
    let mut answers = Vec::new();
    for (label, state) in all.states {
        answers.push((label, ask(state).await));
    }
    let (first_label, first) = answers.remove(0);
    for (label, other) in &answers {
        assert_eq!(&first, other, "{first_label} and {label} stores disagree");
    }
    first
}

pub fn session_id(harness: Harness, id: &str) -> SessionId {
    SessionKey::new(harness, id).to_contract()
}

pub fn tool_event_id<'a>(events: &'a [NormalizedEvent], use_id: &str) -> &'a str {
    events
        .iter()
        .find(|e| {
            e.kind == EventKind::ToolStarted
                && e.tool.as_ref().and_then(|t| t.use_id.as_deref()) == Some(use_id)
        })
        .expect("fixture tool start")
        .key
        .0
        .as_str()
}

pub fn texts(result: &RationaleResult) -> Vec<String> {
    match result {
        RationaleResult::Ok { rationale } => rationale
            .blocks
            .iter()
            .map(|b: &RationaleBlock| b.text.clone())
            .collect(),
        RationaleResult::Unavailable { .. } => Vec::new(),
    }
}

pub fn invalid_input(err: &IpcError) -> bool {
    matches!(err, IpcError::InvalidInput { .. })
}
