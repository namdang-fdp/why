//! A repository with registered, linked worktrees and a way to feed it agent
//! hook events through the real adapters, resolver and ingest step.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use why_lib::collector::GitWorktreeResolver;
use why_lib::contracts::{
    AttributionStatus, FileAttribution, RepoId, ToolOutcome, TurnRef, WorktreeAttribution,
    WorktreeId,
};
use why_lib::events::ingest::ingest;
use why_lib::events::{AppendOutcome, EventStore, MemoryEventStore, SqliteEventStore};
use why_lib::ipc::changes::list_worktrees_impl;
use why_lib::ipc::repos::add_repository_impl;
use why_lib::state::AppState;
use why_lib::store::MemoryRepoStore;
use why_wire::{Envelope, Harness, PROTOCOL_VERSION};

use crate::common::{TestRepo, test_runner, write_in};

#[derive(Debug, Clone, Copy)]
pub enum Backend {
    Memory,
    Sqlite,
}

pub const BACKENDS: [Backend; 2] = [Backend::Memory, Backend::Sqlite];

/// How an edit call ended in the hook stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// Pre then Post: the edit was applied.
    Ok,
    /// Pre then a failure result.
    Failed,
    /// Pre only: rejected, or the Post was never delivered.
    Lost,
}

pub struct Wt {
    pub path: PathBuf,
    pub id: WorktreeId,
}

pub struct Rig {
    pub repo: TestRepo,
    pub state: AppState,
    resolver: GitWorktreeResolver,
    repo_id: RepoId,
    clock: AtomicU64,
    _db: Option<tempfile::TempDir>,
}

pub fn s(p: &Path) -> String {
    p.to_str().expect("temp paths are UTF-8").to_owned()
}

impl Rig {
    /// `seed` writes and commits the base files on `main`.
    pub async fn new(backend: Backend, seed: impl FnOnce(&TestRepo)) -> Self {
        let repo = TestRepo::with_initial_commit();
        seed(&repo);
        let (events, db): (Arc<dyn EventStore>, Option<tempfile::TempDir>) = match backend {
            Backend::Memory => (Arc::new(MemoryEventStore::new()), None),
            Backend::Sqlite => {
                let dir = tempfile::tempdir().unwrap();
                let store = SqliteEventStore::open(&dir.path().join("events.db")).unwrap();
                (Arc::new(store), Some(dir))
            }
        };
        let repos = Arc::new(MemoryRepoStore::new());
        let state = AppState::new(test_runner(), repos.clone()).with_events(events);
        let added = add_repository_impl(&state, &s(&repo.path)).await.unwrap();
        Rig {
            repo,
            state,
            resolver: GitWorktreeResolver::new(test_runner(), repos),
            repo_id: added.id,
            clock: AtomicU64::new(1_000),
            _db: db,
        }
    }

    /// A linked worktree on a new branch, as the app lists it.
    pub async fn linked(&self, name: &str, branch: &str) -> Wt {
        let path = self.repo.add_worktree(name, branch);
        self.worktree_at(&path).await
    }

    pub async fn worktree_at(&self, path: &Path) -> Wt {
        let id = list_worktrees_impl(&self.state, &self.repo_id)
            .await
            .unwrap()
            .into_iter()
            .find(|w| Path::new(&w.path) == path)
            .unwrap_or_else(|| panic!("no worktree at {}", path.display()))
            .id;
        Wt {
            path: path.to_path_buf(),
            id,
        }
    }

    /// Feed one hook delivery; returns the envelope so a test can redeliver it.
    pub async fn send(&self, harness: Harness, hook: &str, cwd: &Path, payload: Value) -> Envelope {
        self.send_outcome(harness, hook, cwd, payload).await.0
    }

    /// Like [`Rig::send`], and what the store reported for the delivery.
    pub async fn send_outcome(
        &self,
        harness: Harness,
        hook: &str,
        cwd: &Path,
        payload: Value,
    ) -> (Envelope, AppendOutcome) {
        let env = Envelope {
            v: PROTOCOL_VERSION,
            harness,
            harness_version: None,
            event: hook.into(),
            // Strictly increasing, so every delivery has its own dedupe key.
            received_at_ms: self.clock.fetch_add(10, Ordering::SeqCst),
            cwd: s(cwd),
            pid: 1,
            payload,
        };
        let outcome = self.deliver(&env).await;
        (env, outcome)
    }

    pub async fn deliver(&self, env: &Envelope) -> AppendOutcome {
        ingest(env, &self.resolver, self.state.events.as_ref())
            .await
            .unwrap()
    }

    // ---- Claude Code --------------------------------------------------

    pub async fn c_prompt(&self, session: &str, turn: &str, cwd: &Path, text: &str) {
        let p = json!({"session_id": session, "hook_event_name": "UserPromptSubmit",
                       "prompt_id": turn, "prompt": text});
        self.send(Harness::ClaudeCode, "UserPromptSubmit", cwd, p)
            .await;
    }

    /// An `Edit` of the (absolute) `file`.
    pub async fn c_edit(
        &self,
        session: &str,
        turn: &str,
        use_id: &str,
        cwd: &Path,
        file: &str,
        end: End,
    ) {
        let base = |hook: &str| {
            json!({"session_id": session, "hook_event_name": hook, "prompt_id": turn,
                   "tool_name": "Edit", "tool_use_id": use_id,
                   "tool_input": {"file_path": file, "old_string": "a", "new_string": "b"}})
        };
        self.send(Harness::ClaudeCode, "PreToolUse", cwd, base("PreToolUse"))
            .await;
        match end {
            End::Ok => {
                let mut p = base("PostToolUse");
                p["tool_response"] = json!({"filePath": file});
                self.send(Harness::ClaudeCode, "PostToolUse", cwd, p).await;
            }
            End::Failed => {
                let mut p = base("PostToolUseFailure");
                p["error"] = json!("old_string not found");
                self.send(Harness::ClaudeCode, "PostToolUseFailure", cwd, p)
                    .await;
            }
            End::Lost => {}
        }
    }

    pub async fn c_bash(&self, session: &str, turn: &str, use_id: &str, cwd: &Path, cmd: &str) {
        let base = |hook: &str| {
            json!({"session_id": session, "hook_event_name": hook, "prompt_id": turn,
                   "tool_name": "Bash", "tool_use_id": use_id,
                   "tool_input": {"command": cmd}})
        };
        self.send(Harness::ClaudeCode, "PreToolUse", cwd, base("PreToolUse"))
            .await;
        let mut post = base("PostToolUse");
        post["tool_response"] = json!({"stdout": "", "stderr": ""});
        self.send(Harness::ClaudeCode, "PostToolUse", cwd, post)
            .await;
    }

    // ---- Codex ----------------------------------------------------------

    /// An `apply_patch` call; `patch` is the patch text's lines.
    pub async fn x_patch(
        &self,
        session: &str,
        turn: &str,
        use_id: &str,
        cwd: &Path,
        patch: &[&str],
        end: End,
    ) {
        let text = format!("{}\n", patch.join("\n"));
        let base = |hook: &str| {
            json!({"session_id": session, "turn_id": turn, "cwd": s(cwd),
                   "hook_event_name": hook, "tool_name": "apply_patch",
                   "tool_use_id": use_id, "tool_input": {"command": text}})
        };
        self.send(Harness::Codex, "PreToolUse", cwd, base("PreToolUse"))
            .await;
        let result = match end {
            End::Ok => Some(
                "Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess. Updated the following files:\nM x\n",
            ),
            End::Failed => Some("Exit code: 1\nWall time: 0 seconds\nOutput:\nerror: no match\n"),
            End::Lost => None,
        };
        if let Some(result) = result {
            let mut p = base("PostToolUse");
            p["tool_response"] = json!(result);
            self.send(Harness::Codex, "PostToolUse", cwd, p).await;
        }
    }

    /// `*** Update File: <file>` patch lines (one hunk).
    pub fn update_patch(file: &str) -> Vec<String> {
        vec![
            "*** Begin Patch".into(),
            format!("*** Update File: {file}"),
            "@@".into(),
            "-base".into(),
            "+changed".into(),
            "*** End Patch".into(),
        ]
    }

    pub async fn x_update(
        &self,
        session: &str,
        turn: &str,
        use_id: &str,
        cwd: &Path,
        file: &str,
        end: End,
    ) {
        let lines = Self::update_patch(file);
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        self.x_patch(session, turn, use_id, cwd, &refs, end).await;
    }

    pub async fn x_bash(&self, session: &str, turn: &str, use_id: &str, cwd: &Path, cmd: &str) {
        let base = |hook: &str| {
            json!({"session_id": session, "turn_id": turn, "cwd": s(cwd),
                   "hook_event_name": hook, "tool_name": "Bash",
                   "tool_use_id": use_id, "tool_input": {"command": cmd}})
        };
        self.send(Harness::Codex, "PreToolUse", cwd, base("PreToolUse"))
            .await;
        let mut post = base("PostToolUse");
        post["tool_response"] = json!("Exit code: 0\n");
        self.send(Harness::Codex, "PostToolUse", cwd, post).await;
    }
}

/// Overwrite `rel` in the worktree (so git sees it as changed).
pub fn change(wt: &Wt, rel: &str) {
    write_in(&wt.path, rel, b"changed on disk\n");
}

pub fn file<'a>(attr: &'a WorktreeAttribution, path: &str) -> &'a FileAttribution {
    attr.files
        .iter()
        .find(|f| f.change.path == path)
        .unwrap_or_else(|| {
            panic!(
                "{path} is not a changed file; changed: {:?}",
                attr.files
                    .iter()
                    .map(|f| &f.change.path)
                    .collect::<Vec<_>>()
            )
        })
}

/// `harness:session#turn_index(turn_id)` for compact assertions.
pub fn label(r: &TurnRef) -> String {
    format!("{}#{}({})", r.session_id.0, r.turn_index, r.turn_id)
}

pub fn labels(refs: &[TurnRef]) -> Vec<String> {
    refs.iter().map(label).collect()
}

/// The result as a table: file, status, explainedBy, attempts.
pub fn table(title: &str, attr: &WorktreeAttribution) -> String {
    let mut out = format!("== {title} ==\n");
    out.push_str(&format!(
        "{:<14} {:<12} {:<34} {}\n",
        "file", "status", "explainedBy", "attempts"
    ));
    for f in &attr.files {
        let status = match f.status {
            AttributionStatus::Explained => "explained",
            AttributionStatus::Unexplained => "unexplained",
        };
        let attempts: Vec<String> = f
            .attempts
            .iter()
            .map(|r| format!("{}:{}", label(r), outcome(r.outcome)))
            .collect();
        out.push_str(&format!(
            "{:<14} {:<12} {:<34} {}\n",
            f.change.path,
            status,
            labels(&f.explained_by).join(", "),
            attempts.join(", ")
        ));
    }
    for l in &attr.shell_leads {
        out.push_str(&format!(
            "  shell lead: {}#{}({}) ran {} command(s)\n",
            l.session_id.0, l.turn_index, l.turn_id, l.command_count
        ));
    }
    out
}

fn outcome(o: ToolOutcome) -> &'static str {
    match o {
        ToolOutcome::Ok => "ok",
        ToolOutcome::Failed => "failed",
        ToolOutcome::Unknown => "unknown",
    }
}
