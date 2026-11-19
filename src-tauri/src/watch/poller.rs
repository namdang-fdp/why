//! Polling fallback for directories the OS watcher cannot cover (inotify watch
//! limit reached, or the OS watcher could not be created at all).
//!
//! Why not notify's own `PollWatcher`: it compares modification times in whole
//! seconds, so a file edited twice within one second (typical for an agent)
//! shows only the first edit. This poller fingerprints every entry with the
//! full-resolution mtime (nanoseconds on Linux), the size and, on Unix, the
//! inode, so a same-second edit or a file replaced by rename is detected.
//!
//! It also walks less: the scan honours `.gitignore` (so `node_modules/` and
//! `target/` are not stat'ed every interval), skips git's object store and
//! never follows symlinks. The cost: a tracked file inside an ignored
//! directory is not polled.
//!
//! Changes are reported as synthetic notify [`Event`]s to the same handler the
//! OS watcher uses, so routing and filtering are shared.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use ignore::WalkBuilder;
use notify::event::{CreateKind, DataChange, EventKind, ModifyKind, RemoveKind};
use notify::{Event, EventHandler};

/// What we remember about one directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    is_dir: bool,
    mtime: Option<SystemTime>,
    size: u64,
    /// Inode number: changes when a file is replaced (atomic save) even if
    /// size and mtime happen to match. 0 where unavailable.
    ino: u64,
}

type Snapshot = HashMap<PathBuf, Entry>;

/// Polls a set of root directories on its own thread.
pub struct Poller {
    /// Root -> last snapshot. Shared with the thread.
    roots: Arc<Mutex<HashMap<PathBuf, Snapshot>>>,
    /// Dropping this sender stops the thread (its `recv_timeout` then reports
    /// `Disconnected`); sending wakes it early.
    wake: mpsc::Sender<()>,
}

impl Poller {
    /// Start the polling thread.
    pub fn new<H: EventHandler>(handler: H, interval: Duration) -> std::io::Result<Self> {
        let roots: Arc<Mutex<HashMap<PathBuf, Snapshot>>> = Arc::default();
        let (wake, wake_rx) = mpsc::channel();
        let shared = Arc::clone(&roots);
        std::thread::Builder::new()
            .name("why-poller".into())
            .spawn(move || run(shared, handler, interval, wake_rx))?;
        Ok(Self { roots, wake })
    }

    /// Start polling `root`. Takes the baseline snapshot now (blocking: it
    /// walks the tree), so a change made right after this call is detected.
    pub fn watch(&self, root: PathBuf) {
        let snapshot = scan(&root);
        lock(&self.roots).insert(root, snapshot);
        let _ = self.wake.send(());
    }

    pub fn unwatch(&self, root: &Path) {
        lock(&self.roots).remove(root);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // The data is a cache of snapshots; a panic mid-update leaves it merely
    // stale, which the next scan fixes.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn run<H: EventHandler>(
    roots: Arc<Mutex<HashMap<PathBuf, Snapshot>>>,
    mut handler: H,
    interval: Duration,
    wake: mpsc::Receiver<()>,
) {
    loop {
        match wake.recv_timeout(interval) {
            Ok(()) => continue, // a root was added: its baseline is fresh, wait a full interval
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return, // the `Poller` was dropped
        }
        let list: Vec<PathBuf> = lock(&roots).keys().cloned().collect();
        for root in list {
            // Scan without holding the lock: walking a tree takes a while.
            let new = scan(&root);
            let old = {
                let mut map = lock(&roots);
                match map.get_mut(&root) {
                    Some(old) => std::mem::replace(old, new.clone()),
                    None => continue, // unwatched meanwhile
                }
            };
            for event in diff(&old, &new) {
                handler.handle_event(Ok(event));
            }
        }
    }
}

/// Every entry under `root` (not `root` itself) that git would care about.
fn scan(root: &Path) -> Snapshot {
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_global(false)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .parents(false)
        .follow_links(false)
        .filter_entry(|e| !is_git_noise(e.path()))
        .build();
    let mut out = Snapshot::new();
    for entry in walker.flatten() {
        if entry.depth() == 0 {
            continue;
        }
        // `metadata()` does not follow symlinks here (follow_links is off), so a
        // link is fingerprinted as the link itself.
        let Ok(meta) = entry.metadata() else {
            continue; // vanished between listing and stat
        };
        out.insert(
            entry.into_path(),
            Entry {
                is_dir: meta.is_dir(),
                mtime: meta.modified().ok(),
                size: meta.len(),
                ino: inode(&meta),
            },
        );
    }
    out
}

/// Big, constantly churning parts of a git dir that never matter to us.
fn is_git_noise(path: &Path) -> bool {
    let in_git_dir = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == ".git");
    in_git_dir
        && path
            .file_name()
            .is_some_and(|n| n == "objects" || n == "logs" || n == "lfs" || n == "hooks")
}

#[cfg(unix)]
fn inode(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino()
}

#[cfg(not(unix))]
fn inode(_meta: &std::fs::Metadata) -> u64 {
    0
}

/// The events that turn `old` into `new`. Directory "modifications" (their
/// mtime changes whenever a child does) are skipped: the child's own event
/// says more.
fn diff(old: &Snapshot, new: &Snapshot) -> Vec<Event> {
    let mut events = Vec::new();
    for (path, e) in new {
        let kind = match old.get(path) {
            None if e.is_dir => EventKind::Create(CreateKind::Folder),
            None => EventKind::Create(CreateKind::File),
            Some(prev) if !e.is_dir && (prev != e) => {
                EventKind::Modify(ModifyKind::Data(DataChange::Any))
            }
            Some(prev) if prev.is_dir != e.is_dir => EventKind::Create(CreateKind::Any),
            Some(_) => continue,
        };
        events.push(Event::new(kind).add_path(path.clone()));
    }
    for (path, e) in old {
        if !new.contains_key(path) {
            let kind = if e.is_dir {
                RemoveKind::Folder
            } else {
                RemoveKind::File
            };
            events.push(Event::new(EventKind::Remove(kind)).add_path(path.clone()));
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_reports_create_modify_remove_and_skips_dir_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("d")).unwrap();
        std::fs::write(root.join("d/a"), "1").unwrap();
        std::fs::write(root.join("b"), "1").unwrap();
        let before = scan(root);

        std::fs::write(root.join("d/a"), "22").unwrap(); // modify
        std::fs::remove_file(root.join("b")).unwrap(); // remove
        std::fs::write(root.join("d/c"), "1").unwrap(); // create (also bumps d's mtime)
        let after = scan(root);

        let mut got: Vec<(String, String)> = diff(&before, &after)
            .into_iter()
            .map(|e| {
                let p = e.paths[0].strip_prefix(root).unwrap().display().to_string();
                (p, format!("{:?}", e.kind))
            })
            .collect();
        got.sort();
        assert_eq!(got.len(), 3, "{got:?}");
        assert!(got[0].0 == "b" && got[0].1.starts_with("Remove"));
        assert!(got[1].0 == "d/a" && got[1].1.starts_with("Modify"));
        assert!(got[2].0 == "d/c" && got[2].1.starts_with("Create"));
    }

    #[test]
    fn same_size_edit_within_one_second_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("f");
        std::fs::write(&f, "x1").unwrap();
        let before = scan(tmp.path());
        // Same size, a few ms later: only the sub-second mtime differs.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&f, "x2").unwrap();
        let after = scan(tmp.path());
        assert_eq!(diff(&before, &after).len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directories_are_not_followed_and_git_noise_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".git/objects/ab")).unwrap();
        std::fs::write(tmp.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let snap = scan(tmp.path());
        assert!(snap.contains_key(&tmp.path().join("link")));
        assert!(!snap.contains_key(&tmp.path().join("link/secret")));
        assert!(snap.contains_key(&tmp.path().join(".git/HEAD")));
        assert!(!snap.contains_key(&tmp.path().join(".git/objects")));
    }
}
