//! Integration tests for the collector (WHY-13): the real socket server, spool
//! drain, git resolver and allowlist writer on throw-away repositories, plus an
//! end-to-end run with the real `why-hook` binary.
//!
//! Isolation: every test uses its own temp directory for the data dir, the
//! socket (a SHORT path under /tmp: `sun_path` holds ~108 bytes) and the spool;
//! nothing reads `$HOME` or `$XDG_*`, and no process-wide environment variable
//! is set. The hook binary is run with a cleared environment.
//!
//! The harness adapters are still stubs (WHY-14/15), so `ingest` through them
//! stores nothing. These tests plug a tiny [`TestNormalizer`] into the
//! collector's `Normalizer` seam instead: everything else (socket, queue,
//! resolver, `store_events`, memory store, notifications) is the real code.

#[path = "../git/common.rs"]
mod git_common;

mod allowlist;
mod e2e;
mod resolver;
mod retention;
mod server;
mod spool;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use git_common::TestRepo;
use tempfile::TempDir;
use why_lib::collector::allowlist_sync::{AllowlistState, wrap_store};
use why_lib::collector::server::ServerConfig;
use why_lib::collector::{CoreConfig, CoreHandle, Normalizer, Timing, spawn_core};
use why_lib::contracts::{RepoId, WorktreeId};
use why_lib::events::{EventKind, EventStore, MemoryEventStore, NormalizedEvent, SessionKey};
use why_lib::git;
use why_lib::store::{MemoryRepoStore, RepoRecord, RepoStore};
use why_wire::{Envelope, Harness, Instance, InstancePaths, PROTOCOL_VERSION};

/// One event per envelope, in a session named by the payload's `session_id`.
pub struct TestNormalizer;

impl Normalizer for TestNormalizer {
    fn normalize(&self, env: &Envelope) -> Vec<NormalizedEvent> {
        let Some(sid) = env.payload.get("session_id").and_then(|v| v.as_str()) else {
            return Vec::new();
        };
        vec![NormalizedEvent::from_envelope(
            env,
            0,
            sid,
            EventKind::Prompt,
        )]
    }
}

/// A normalizer that takes `delay` per envelope: a slow store, to fill queues.
pub struct SlowNormalizer(pub Duration);

impl Normalizer for SlowNormalizer {
    fn normalize(&self, env: &Envelope) -> Vec<NormalizedEvent> {
        std::thread::sleep(self.0);
        TestNormalizer.normalize(env)
    }
}

/// A temp dir with a short canonical path under /tmp.
pub fn short_tmp() -> TempDir {
    let tmp = tempfile::Builder::new()
        .prefix("wc")
        .tempdir_in("/tmp")
        .unwrap();
    assert!(tmp.path().as_os_str().len() < 30);
    tmp
}

pub fn envelope(session: &str, cwd: &Path, at_ms: u64) -> Envelope {
    Envelope {
        v: PROTOCOL_VERSION,
        harness: Harness::ClaudeCode,
        harness_version: None,
        event: "UserPromptSubmit".into(),
        received_at_ms: at_ms,
        cwd: cwd.to_string_lossy().into_owned(),
        pid: 1,
        payload: serde_json::json!({
            "session_id": session,
            "hook_event_name": "UserPromptSubmit",
            "cwd": cwd,
        }),
    }
}

/// Now, in Unix milliseconds (events must carry a RECENT `received_at_ms`: the
/// spool drain drops anything older than the 30-day retention window).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Poll `cond` until true or `within` passes.
pub async fn eventually(within: Duration, what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < within, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Everything one test needs: temp dirs, stores, a registered repository.
pub struct Rig {
    pub tmp: TempDir,
    pub repo: TestRepo,
    pub paths: InstancePaths,
    /// The registry as the app sees it: wrapped so the allowlist file follows
    /// registrations synchronously.
    pub repos: Arc<dyn RepoStore>,
    pub allowlist: Arc<AllowlistState>,
    pub events: Arc<MemoryEventStore>,
    pub emitted: Arc<Mutex<Vec<WorktreeId>>>,
    pub main_wt: WorktreeId,
}

impl Default for Rig {
    fn default() -> Self {
        Self::new()
    }
}

impl Rig {
    pub fn new() -> Self {
        let tmp = short_tmp();
        let data = tmp.path().join("data");
        let paths = InstancePaths::from_hook_paths(
            Instance::Installed,
            data,
            tmp.path().join("run/s.sock"),
        )
        .unwrap();
        let repo = TestRepo::with_initial_commit();
        let common = repo.path.join(".git").canonicalize().unwrap();
        let memory = Arc::new(MemoryRepoStore::new());
        memory
            .insert(RepoRecord {
                id: RepoId(git::repo_id(&common)),
                name: "repo".into(),
                root_path: repo.path.clone(),
                common_dir: common.clone(),
            })
            .unwrap();
        let main_wt = WorktreeId(git::worktree_id(&common, &repo.path));
        let (repos, allowlist) = wrap_store(memory, Some(paths.allowlist_path.clone()));
        Rig {
            tmp,
            repo,
            paths,
            repos,
            allowlist,
            events: Arc::new(MemoryEventStore::new()),
            emitted: Arc::default(),
            main_wt,
        }
    }

    pub fn config(&self, normalizer: Arc<dyn Normalizer>) -> CoreConfig {
        let emitted = Arc::clone(&self.emitted);
        CoreConfig {
            paths: self.paths.clone(),
            git: git_common::test_runner(),
            repos: Arc::clone(&self.repos),
            allowlist: Arc::clone(&self.allowlist),
            events: Arc::clone(&self.events) as Arc<dyn EventStore>,
            normalizer,
            emit: Arc::new(move |wt| emitted.lock().unwrap().push(wt)),
            timing: Timing {
                allowlist: why_lib::collector::allowlist_sync::SyncTiming {
                    refresh: Duration::from_millis(300),
                },
                spool_every: Duration::from_millis(200),
                notify_window: Duration::from_millis(100),
                retention_every: Duration::from_secs(3600),
            },
            server: ServerConfig {
                max_connections: 128,
                idle_timeout: Duration::from_millis(300),
                connection_deadline: Duration::from_secs(3),
                max_queued_bytes: 64 * 1024 * 1024,
            },
        }
    }

    /// Start with the test normalizer and wait until listening.
    pub async fn start(&self) -> CoreHandle {
        self.start_with(Arc::new(TestNormalizer)).await
    }

    pub async fn start_with(&self, normalizer: Arc<dyn Normalizer>) -> CoreHandle {
        let core = spawn_core(self.config(normalizer));
        let status = core.ready().await;
        assert_eq!(status, why_lib::collector::CollectorStatus::Listening);
        core
    }

    /// Sessions the store attributes to the registered worktree.
    pub fn sessions(&self) -> usize {
        self.events
            .sessions_for_worktree(&self.main_wt)
            .unwrap()
            .len()
    }

    pub fn has_session(&self, id: &str) -> bool {
        self.events
            .timeline(&SessionKey::new(Harness::ClaudeCode, id))
            .unwrap()
            .is_some()
    }

    pub fn socket(&self) -> PathBuf {
        self.paths.socket_path.clone()
    }
}
