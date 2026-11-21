//! The ingest step: one received [`Envelope`] in, stored events out.
//!
//! This is the seam between three tasks, so each fills its own piece without
//! editing the others' code:
//!
//! 1. [`crate::adapters::normalize`] turns the envelope into events (WHY-14/15);
//! 2. a [`WorktreeResolver`] maps each event's cwd to a registered worktree
//!    (WHY-13 implements it with git + the `RepoStore`; until then
//!    [`NoResolver`] resolves nothing);
//! 3. [`EventStore::append`] stores them (WHY-12's SQLite store).
//!
//! The collector (WHY-13) calls [`ingest`] per envelope and emits
//! `sessions-changed` for every worktree in [`AppendOutcome::changed_worktrees`].

use std::collections::HashMap;
use std::future::Future;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use why_wire::Envelope;

use super::{AppendOutcome, EventStore, NormalizedEvent};
use crate::adapters;
use crate::contracts::WorktreeId;
use crate::store::StoreError;

/// A registered worktree an event's cwd belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorktree {
    pub id: WorktreeId,
    /// Canonical root of the worktree, to make file paths relative to it.
    pub root: PathBuf,
}

/// Maps a cwd to the registered worktree that contains it.
///
/// Must use git (`rev-parse --show-toplevel` / the worktree list), never a
/// string prefix: the cwd may be a subdirectory or a linked worktree, and
/// `CLAUDE_PROJECT_DIR` is the session's start dir, not the repo root
/// (payloads.md §7). Returns `None` for a cwd outside every registered
/// repository (those events should not exist: the hook's allowlist drops them).
///
/// Written as `fn -> impl Future + Send` rather than `async fn` so the future
/// is guaranteed `Send` (the collector runs on Tauri's multi-threaded runtime).
/// A generic parameter, not a trait object: async trait methods cannot be
/// called through `dyn`.
pub trait WorktreeResolver: Send + Sync {
    fn resolve(&self, cwd: &str) -> impl Future<Output = Option<ResolvedWorktree>> + Send;
}

/// Resolves nothing. The stub until WHY-13; events are stored without a
/// worktree and so appear in no worktree's session list.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoResolver;

impl WorktreeResolver for NoResolver {
    async fn resolve(&self, _cwd: &str) -> Option<ResolvedWorktree> {
        None
    }
}

/// Normalize, resolve and store one envelope.
pub async fn ingest<R: WorktreeResolver>(
    env: &Envelope,
    resolver: &R,
    store: &dyn EventStore,
) -> Result<AppendOutcome, StoreError> {
    store_events(adapters::normalize(env), resolver, store).await
}

/// Steps 2 and 3 of [`ingest`], for events that are already normalized.
pub async fn store_events<R: WorktreeResolver>(
    mut events: Vec<NormalizedEvent>,
    resolver: &R,
    store: &dyn EventStore,
) -> Result<AppendOutcome, StoreError> {
    if events.is_empty() {
        return Ok(AppendOutcome::default());
    }

    // Usually every event of an envelope shares one cwd: resolve each distinct
    // cwd once (each resolution may run git).
    let mut resolved: HashMap<String, Option<ResolvedWorktree>> = HashMap::new();
    // Multi-file edits often share a parent. Resolve it once per envelope.
    let mut parents: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    for ev in &mut events {
        if !resolved.contains_key(&ev.cwd) {
            let wt = resolver.resolve(&ev.cwd).await;
            resolved.insert(ev.cwd.clone(), wt);
        }
        if let Some(Some(wt)) = resolved.get(&ev.cwd) {
            ev.worktree_id = Some(wt.id.clone());
            for file in &mut ev.files {
                if let Some(rel) = relative_to(file, &wt.root, &mut parents).await {
                    *file = rel;
                }
            }
        }
    }

    store.append(&events)
}

/// Remove dot components for an event whose parent no longer resolves through
/// the filesystem, such as a patch path containing a deleted directory and `..`.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                if normalized.file_name().is_some() {
                    normalized.pop();
                }
            }
            Component::CurDir => {}
            part => normalized.push(part),
        }
    }
    normalized
}

/// Canonicalize the parent directory, even if an edit removed some of its
/// directories. Keep missing suffixes under their nearest existing ancestor.
async fn resolve_parent(mut parent: &Path) -> Option<PathBuf> {
    let mut missing = Vec::new();
    let mut resolved = loop {
        match tokio::fs::canonicalize(parent).await {
            Ok(resolved) => break resolved,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let Some(dir) = parent.file_name() else {
                    break tokio::fs::canonicalize(normalize_lexically(parent))
                        .await
                        .ok()?;
                };
                missing.push(dir);
                parent = parent.parent()?;
            }
            Err(_) => return None,
        }
    };
    resolved.extend(missing.into_iter().rev());
    Some(resolved)
}

/// Worktree-relative Git spelling. Resolve symlinks in the parent before
/// checking the root, but preserve the final component: Git may track that
/// component as a symlink in its own right.
async fn relative_to(
    file: &str,
    root: &Path,
    parents: &mut HashMap<PathBuf, Option<PathBuf>>,
) -> Option<String> {
    let file = Path::new(file);
    if !file.is_absolute() {
        return None;
    }
    let parent = file.parent()?;
    if !parents.contains_key(parent) {
        parents.insert(parent.to_path_buf(), resolve_parent(parent).await);
    }
    let resolved = parents.get(parent)?.as_ref()?.join(file.file_name()?);
    let rel = resolved.strip_prefix(root).ok()?;
    let parts: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::model::EventKind;
    use crate::events::test_support::event;
    use crate::events::{EventStore, MemoryEventStore, SessionKey};

    #[tokio::test]
    async fn relative_paths_are_component_wise() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("repo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let mut parents = HashMap::new();
        for (path, expected) in [
            ("repo/src/a.rs", Some("src/a.rs")),
            ("repo-other/a.rs", None),
            ("repo/../a.rs", None),
            ("repo/src/../README.md", Some("README.md")),
            ("repo", None),
        ] {
            let file = base.join(path);
            let relative = relative_to(file.to_str().unwrap(), &root, &mut parents).await;
            assert_eq!(relative.as_deref(), expected);
        }
        assert_eq!(
            relative_to("relative/a.rs", &root, &mut parents).await,
            None
        );
    }

    struct FakeResolver(PathBuf);

    impl WorktreeResolver for FakeResolver {
        async fn resolve(&self, cwd: &str) -> Option<ResolvedWorktree> {
            Path::new(cwd)
                .starts_with(&self.0)
                .then(|| ResolvedWorktree {
                    id: WorktreeId("wt-repo".into()),
                    root: self.0.clone(),
                })
        }
    }

    fn at(cwd: &Path, hook: &str, ms: u64, files: &[String]) -> NormalizedEvent {
        let mut e = event("s1", hook, EventKind::Prompt, ms, Some("p1"));
        e.worktree_id = None; // adapters never set it
        e.cwd = cwd.to_str().unwrap().into();
        e.files = files.to_vec();
        e
    }

    #[tokio::test]
    async fn events_get_their_worktree_and_relative_files() {
        let store = MemoryEventStore::new();
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("repo");
        std::fs::create_dir(&root).unwrap();
        let outside = base.join("outside.txt").to_str().unwrap().to_owned();
        let files = [
            root.join("src/a.rs").to_str().unwrap().to_owned(),
            outside.clone(),
        ];
        let events = vec![
            at(&root.join("sub"), "UserPromptSubmit", 1, &files),
            at(&root, "Stop", 2, &files),
            at(&base, "Notification", 3, &files),
        ];
        let resolver = FakeResolver(root);
        let out = store_events(events.clone(), &resolver, &store)
            .await
            .unwrap();
        assert_eq!(out.inserted, 3);
        assert_eq!(out.changed_worktrees, vec![WorktreeId("wt-repo".into())]);

        let t = store
            .timeline(&SessionKey::new(why_wire::Harness::ClaudeCode, "s1"))
            .unwrap()
            .unwrap();
        assert_eq!(t.session.worktree_id, Some(WorktreeId("wt-repo".into())));
        let first = &t.turns[0].events[0];
        assert_eq!(first.files, ["src/a.rs", outside.as_str()]);

        // Redelivery: nothing new, so nothing to announce.
        let again = store_events(events, &resolver, &store).await.unwrap();
        assert_eq!(again.duplicates, 3);
        assert!(again.changed_worktrees.is_empty());
    }

    // A harness with no adapter stores nothing: the envelope is dropped, not
    // an error.
    #[tokio::test]
    async fn envelope_of_a_harness_without_adapter_stores_nothing() {
        let store = MemoryEventStore::new();
        let env = crate::events::test_support::envelope(
            why_wire::Harness::Unknown,
            "UserPromptSubmit",
            1,
            serde_json::json!({ "session_id": "s", "hook_event_name": "UserPromptSubmit", "prompt": "hi" }),
        );
        let out = ingest(&env, &NoResolver, &store).await.unwrap();
        assert_eq!(out, AppendOutcome::default());
    }

    // Since WHY-14 the Claude adapter is real: its prompt is stored.
    #[tokio::test]
    async fn claude_prompt_is_stored_through_the_adapter() {
        let store = MemoryEventStore::new();
        let env = crate::events::test_support::envelope(
            why_wire::Harness::ClaudeCode,
            "UserPromptSubmit",
            1,
            serde_json::json!({ "session_id": "s", "hook_event_name": "UserPromptSubmit", "prompt": "hi", "prompt_id": "p1" }),
        );
        let out = ingest(&env, &NoResolver, &store).await.unwrap();
        assert_eq!(out.inserted, 1);
        let t = store
            .timeline(&SessionKey::new(why_wire::Harness::ClaudeCode, "s"))
            .unwrap()
            .unwrap();
        assert_eq!(t.session.task.as_deref(), Some("hi"));
    }
}
