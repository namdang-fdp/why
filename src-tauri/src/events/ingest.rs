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
    for ev in &mut events {
        if !resolved.contains_key(&ev.cwd) {
            let wt = resolver.resolve(&ev.cwd).await;
            resolved.insert(ev.cwd.clone(), wt);
        }
        if let Some(Some(wt)) = resolved.get(&ev.cwd) {
            ev.worktree_id = Some(wt.id.clone());
            for file in &mut ev.files {
                if let Some(rel) = relative_to(file, &wt.root) {
                    *file = rel;
                }
            }
        }
    }

    store.append(&events)
}

/// A worktree-relative Git path with `/` separators, when the normalized
/// payload path is under the canonical root.
fn relative_to(file: &str, root: &Path) -> Option<String> {
    let file = Path::new(file);
    if !file.is_absolute() {
        return None;
    }
    // Normalize lexically: a historical event's file may no longer exist,
    // and Git paths name symlinks by their location in the tree.
    let mut normalized = PathBuf::new();
    for part in file.components() {
        match part {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            part => normalized.push(part),
        }
    }
    let rel = normalized.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::model::EventKind;
    use crate::events::test_support::event;
    use crate::events::{EventStore, MemoryEventStore, SessionKey};

    #[test]
    fn relative_paths_are_component_wise() {
        let root = Path::new("/home/u/repo");
        assert_eq!(
            relative_to("/home/u/repo/src/a.rs", root).as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(relative_to("/home/u/repo-other/a.rs", root), None);
        assert_eq!(relative_to("/home/u/repo/../a.rs", root), None);
        assert_eq!(
            relative_to("/home/u/repo/src/../README.md", root).as_deref(),
            Some("README.md")
        );
        assert_eq!(relative_to("/home/u/repo", root), None);
        assert_eq!(relative_to("relative/a.rs", root), None);
    }

    /// Resolves cwds under `/home/u/repo` to `wt-repo`.
    struct FakeResolver;

    impl WorktreeResolver for FakeResolver {
        async fn resolve(&self, cwd: &str) -> Option<ResolvedWorktree> {
            Path::new(cwd)
                .starts_with("/home/u/repo")
                .then(|| ResolvedWorktree {
                    id: WorktreeId("wt-repo".into()),
                    root: PathBuf::from("/home/u/repo"),
                })
        }
    }

    fn at(cwd: &str, hook: &str, ms: u64) -> NormalizedEvent {
        let mut e = event("s1", hook, EventKind::Prompt, ms, Some("p1"));
        e.worktree_id = None; // adapters never set it
        e.cwd = cwd.into();
        e.files = vec!["/home/u/repo/src/a.rs".into(), "/etc/hosts".into()];
        e
    }

    #[tokio::test]
    async fn events_get_their_worktree_and_relative_files() {
        let store = MemoryEventStore::new();
        let events = vec![
            at("/home/u/repo/sub", "UserPromptSubmit", 1),
            at("/home/u/repo", "Stop", 2),
            at("/tmp/elsewhere", "Notification", 3),
        ];
        let out = store_events(events.clone(), &FakeResolver, &store)
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
        assert_eq!(first.files, ["src/a.rs", "/etc/hosts"]);

        // Redelivery: nothing new, so nothing to announce.
        let again = store_events(events, &FakeResolver, &store).await.unwrap();
        assert_eq!(again.duplicates, 3);
        assert!(again.changed_worktrees.is_empty());
    }

    // With the WHY-46 stub adapters, a real envelope stores nothing.
    #[tokio::test]
    async fn stub_adapters_store_nothing() {
        let store = MemoryEventStore::new();
        let env = crate::events::test_support::envelope(
            why_wire::Harness::ClaudeCode,
            "UserPromptSubmit",
            1,
            serde_json::json!({ "session_id": "s", "hook_event_name": "UserPromptSubmit", "prompt": "hi" }),
        );
        let out = ingest(&env, &NoResolver, &store).await.unwrap();
        assert_eq!(out, AppendOutcome::default());
    }
}
