//! The ingest step: one received [`Envelope`] in, stored events out.
//!
//! This is the seam between three tasks, so each fills its own piece without
//! editing the others' code:
//!
//! 1. [`crate::adapters::normalize`] turns the envelope into events (WHY-14/15);
//! 2. a [`WorktreeResolver`] maps each event's cwd to a registered worktree
//!    (WHY-13 implements it with git + the `RepoStore`; until then
//!    [`NoResolver`] resolves nothing). Since WHY-65 it also maps every EDITED
//!    FILE to its own worktree (decision 32): the file's directory, not the
//!    session's cwd, decides which worktree an edit (and its op) belongs to;
//! 3. [`EventStore::append`] stores them (WHY-12's SQLite store).
//!
//! The collector (WHY-13) calls [`ingest`] per envelope and emits
//! `sessions-changed` for every worktree in [`AppendOutcome::changed_worktrees`].

use std::collections::HashMap;
use std::future::Future;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use why_wire::Envelope;

use super::{AppendOutcome, EditedFile, EventStore, NormalizedEvent, ToolClass};
use crate::adapters;
use crate::contracts::WorktreeId;
use crate::provenance::{EditKind, EditOp, Observed, UnobservedReason};
use crate::store::StoreError;

/// A registered worktree an event's cwd belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorktree {
    pub id: WorktreeId,
    /// Canonical root of the worktree, to make file paths relative to it.
    pub root: PathBuf,
}

/// Maps a directory (an event's cwd, or the directory of an edited file) to the
/// registered worktree that contains it.
///
/// Must use git (`rev-parse --show-toplevel` / the worktree list), never a
/// string prefix: the cwd may be a subdirectory or a linked worktree, and
/// `CLAUDE_PROJECT_DIR` is the session's start dir, not the repo root
/// (payloads.md §7). Returns `None` for a cwd outside every registered
/// repository (a cwd there should not exist: the hook's allowlist drops those
/// events). An EDITED FILE's directory is different: the allowlist only looks at
/// the session's cwd, so a file path can name any directory, and `None` simply
/// means "that file belongs to no registered worktree".
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
    // Same for the worktree that owns an edited file's directory (that
    // resolution runs git, see `owner_of`).
    let mut homes: HashMap<PathBuf, Option<Home>> = HashMap::new();
    for ev in &mut events {
        if !resolved.contains_key(&ev.cwd) {
            let wt = resolver.resolve(&ev.cwd).await;
            resolved.insert(ev.cwd.clone(), wt);
        }
        let session_wt = resolved.get(&ev.cwd).and_then(Option::as_ref);
        if let Some(wt) = session_wt {
            ev.worktree_id = Some(wt.id.clone());
        }

        if ev.tool.as_ref().is_some_and(|t| t.class == ToolClass::Edit) {
            // An edit belongs to the worktree of the EDITED FILE, which is not
            // always the session's (decision 32): resolve each file on its own,
            // through git, with no fallback to the session's root by prefix.
            let mut edited = Vec::new();
            for file in &mut ev.files {
                if let Some((wt, rel)) = owner_of(file, resolver, &mut homes).await {
                    // The shared `files` list is read as "relative to the
                    // session's worktree" (timeline chips, `turnsTouchingFile`).
                    // A bare relative path for a file of ANOTHER worktree
                    // could equal a different file of this one, so such a
                    // file keeps its absolute path there; its owner is
                    // recorded in `edited_files` (and `event_files`), which is
                    // what attributes it to the right worktree.
                    if session_wt.is_some_and(|s| s.id == wt.id) {
                        *file = rel.clone();
                    }
                    edited.push(EditedFile {
                        worktree_id: wt.id,
                        path: rel,
                    });
                }
            }
            ev.edited_files = Some(edited);
        } else if let Some(wt) = session_wt {
            for file in &mut ev.files {
                if let Some(rel) = relative_to(file, &wt.root, &mut parents).await {
                    *file = rel;
                }
            }
        }
        attribute_ops(ev, resolver, &mut homes).await;
    }

    store.append(&events)
}

/// A directory's owner: the registered worktree that contains it, and the
/// directory's canonical path (missing trailing directories included).
struct Home {
    worktree: ResolvedWorktree,
    dir: PathBuf,
}

/// The worktree that owns `file`, and the file's path relative to its root.
///
/// The file's parent directory is resolved through the filesystem and then
/// through git (`resolver`), never by comparing strings: symlinks are followed,
/// and a nested repository or submodule is its OWN worktree (or, when it is not
/// registered, nobody's: `None`), not part of the outer one. For a file in a
/// directory that does not exist yet (a new file, or a directory the edit
/// removed) git is asked about the nearest directory that exists, and the
/// missing part is kept in the path. The final component is not resolved: git
/// may track a symlink in its own right.
async fn owner_of<R: WorktreeResolver>(
    file: &str,
    resolver: &R,
    homes: &mut HashMap<PathBuf, Option<Home>>,
) -> Option<(ResolvedWorktree, String)> {
    let file = Path::new(file);
    if !file.is_absolute() {
        return None;
    }
    let (parent, name) = (file.parent()?, file.file_name()?);
    if !homes.contains_key(parent) {
        let home = async {
            let located = locate_parent(parent).await?;
            // Ask about the directory as given first: when it exists the
            // resolver answers for it (following symlinks itself). When it
            // does not (a new directory, or one the edit removed) that costs
            // nothing and the nearest existing ancestor is asked instead.
            let worktree = match resolver.resolve(parent.to_str()?).await {
                Some(worktree) => worktree,
                // Only worth asking again when that was not already the
                // directory in question (a missing part, or a `..` that only
                // resolves lexically): a second ask about the SAME existing
                // directory cannot answer differently and can cost a failing
                // git call.
                None if Path::new(parent) != located.existing => {
                    resolver.resolve(located.existing.to_str()?).await?
                }
                None => return None,
            };
            Some(Home {
                worktree,
                dir: located.dir,
            })
        }
        .await;
        homes.insert(parent.to_path_buf(), home);
    }
    let home = homes.get(parent)?.as_ref()?;
    let rel = home.dir.join(name);
    let rel = rel.strip_prefix(&home.worktree.root).ok()?;
    Some((home.worktree.clone(), relative_string(rel)?))
}

/// Give each edit op of `ev` the worktree that owns its file and the path
/// relative to that worktree's root. An op whose file is in no registered
/// worktree keeps `worktree_id: None`, so the stores skip it (the event itself
/// is still stored).
///
/// A rename's old path (`renamed_from`) is kept only when it is in the SAME
/// worktree as the new one: the engine follows renames inside one file
/// history, and a file that moved between worktrees has no single history.
/// For a move ACROSS worktrees the destination's hunks describe the OLD file,
/// not whatever B holds, so replaying them against B would be wrong: the
/// destination op becomes `Unobserved(Unparsed)` with no hunks, and when the
/// old path resolves to a worktree its removal is recorded there as a
/// separate `Delete` op (appended after the event's own ops, so their
/// `op_index` does not move).
async fn attribute_ops<R: WorktreeResolver>(
    ev: &mut NormalizedEvent,
    resolver: &R,
    homes: &mut HashMap<PathBuf, Option<Home>>,
) {
    let mut removals = Vec::new();
    for op in &mut ev.edits {
        let Some((wt, rel)) = owner_of(&op.path, resolver, homes).await else {
            op.worktree_id = None;
            op.renamed_from = None;
            continue;
        };
        op.worktree_id = Some(wt.id.clone());
        op.path = rel.clone();
        if let Some(old) = op.renamed_from.take() {
            match owner_of(&old, resolver, homes).await {
                Some((old_wt, old_rel)) if old_wt.id == wt.id => op.renamed_from = Some(old_rel),
                from => {
                    // A move across worktrees (or from outside every one).
                    op.observed = Observed::Unobserved(UnobservedReason::Unparsed);
                    op.hunks.clear();
                    op.before = None;
                    op.after = None;
                    if let Some((old_wt, old_rel)) = from {
                        removals.push(EditOp {
                            worktree_id: Some(old_wt.id.clone()),
                            path: old_rel.clone(),
                            kind: EditKind::Delete,
                            renamed_from: None,
                            before: None,
                            after: None,
                            hunks: Vec::new(),
                            observed: Observed::Exact,
                        });
                        if let Some(edited) = &mut ev.edited_files {
                            let file = EditedFile {
                                worktree_id: old_wt.id,
                                path: old_rel,
                            };
                            if !edited.contains(&file) {
                                edited.push(file);
                            }
                        }
                    }
                }
            }
        }
        // The session is listed in the file's worktree even if the event's
        // own file list missed this path.
        if let Some(edited) = &mut ev.edited_files {
            let file = EditedFile {
                worktree_id: wt.id,
                path: rel,
            };
            if !edited.contains(&file) {
                edited.push(file);
            }
        }
    }
    ev.edits.extend(removals);
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

/// A directory resolved through the filesystem.
struct LocatedDir {
    /// The directory's canonical path, missing trailing directories included.
    dir: PathBuf,
    /// The nearest ancestor (or the directory itself) that exists, canonical:
    /// what git can be asked about.
    existing: PathBuf,
}

/// Canonicalize the parent directory, even if an edit removed some of its
/// directories (or has not created them yet). Keep missing suffixes under
/// their nearest existing ancestor.
async fn locate_parent(mut parent: &Path) -> Option<LocatedDir> {
    let mut missing = Vec::new();
    let existing = loop {
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
    let mut dir = existing.clone();
    dir.extend(missing.into_iter().rev());
    Some(LocatedDir { dir, existing })
}

async fn resolve_parent(parent: &Path) -> Option<PathBuf> {
    locate_parent(parent).await.map(|located| located.dir)
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
    relative_string(resolved.strip_prefix(root).ok()?)
}

/// A relative path as `/`-joined UTF-8 components; `None` if it is empty or
/// not valid UTF-8.
fn relative_string(rel: &Path) -> Option<String> {
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
