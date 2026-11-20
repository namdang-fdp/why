//! In-memory [`RepoStore`]: the backend used until the SQLite store lands, and
//! the fake that tests plug in. Nothing survives a restart.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use super::{InsertOutcome, RepoRecord, RepoStore, StoreError};
use crate::contracts::{RepoId, WorktreeId};

#[derive(Debug, Default)]
struct Inner {
    repos: HashMap<RepoId, RepoRecord>,
    base_overrides: HashMap<WorktreeId, String>,
    /// Trusted filter-driver fingerprints per repository.
    trusted_filters: HashMap<RepoId, HashSet<String>>,
}

/// A [`RepoStore`] kept in two hash maps behind one lock.
///
/// Why `std::sync::Mutex` and not `tokio::sync::Mutex`: the store's methods are
/// synchronous and never `.await` while holding the lock, so the lock is held
/// for a few microseconds at most. A std mutex is cheaper and, unlike the tokio
/// one, can be used from plain sync code and tests. A tokio mutex is only needed
/// when a guard must live across an `.await`.
#[derive(Debug, Default)]
pub struct MemoryRepoStore {
    inner: Mutex<Inner>,
}

impl MemoryRepoStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A mutex is "poisoned" if a thread panicked while holding it. We report
    /// that as a store error instead of panicking again, so one bug does not
    /// take down every later command.
    fn lock(&self) -> Result<MutexGuard<'_, Inner>, StoreError> {
        self.inner
            .lock()
            .map_err(|_| StoreError::Backend("repo store lock poisoned".to_string()))
    }
}

impl RepoStore for MemoryRepoStore {
    fn list(&self) -> Result<Vec<RepoRecord>, StoreError> {
        let mut repos: Vec<RepoRecord> = self.lock()?.repos.values().cloned().collect();
        // HashMap iteration order is random; sort so the UI list is stable.
        repos.sort_by(|a, b| (&a.name, &a.root_path).cmp(&(&b.name, &b.root_path)));
        Ok(repos)
    }

    fn get(&self, id: &RepoId) -> Result<Option<RepoRecord>, StoreError> {
        Ok(self.lock()?.repos.get(id).cloned())
    }

    fn insert(&self, rec: RepoRecord) -> Result<InsertOutcome, StoreError> {
        use std::collections::hash_map::Entry;
        // `entry` does the lookup and the insert under one lock, so two
        // concurrent inserts of the same repository cannot both win.
        Ok(match self.lock()?.repos.entry(rec.id.clone()) {
            Entry::Occupied(existing) => InsertOutcome::AlreadyPresent(existing.get().clone()),
            Entry::Vacant(slot) => InsertOutcome::Inserted(slot.insert(rec).clone()),
        })
    }

    fn remove(&self, id: &RepoId) -> Result<bool, StoreError> {
        let mut inner = self.lock()?;
        // Trust must not outlive the registration (see `RepoStore::remove`).
        inner.trusted_filters.remove(id);
        Ok(inner.repos.remove(id).is_some())
    }

    fn base_override(&self, wt: &WorktreeId) -> Result<Option<String>, StoreError> {
        Ok(self.lock()?.base_overrides.get(wt).cloned())
    }

    fn set_base_override(&self, wt: &WorktreeId, base: Option<String>) -> Result<(), StoreError> {
        let mut inner = self.lock()?;
        match base {
            Some(base) => {
                inner.base_overrides.insert(wt.clone(), base);
            }
            None => {
                inner.base_overrides.remove(wt);
            }
        }
        Ok(())
    }

    fn trusted_filters(&self, repo: &RepoId) -> Result<HashSet<String>, StoreError> {
        Ok(self
            .lock()?
            .trusted_filters
            .get(repo)
            .cloned()
            .unwrap_or_default())
    }

    fn set_trusted_filters(
        &self,
        repo: &RepoId,
        fingerprints: &[String],
    ) -> Result<bool, StoreError> {
        let mut inner = self.lock()?;
        // Same lock for the check and the write: see the trait docs.
        if !inner.repos.contains_key(repo) {
            return Ok(false);
        }
        if fingerprints.is_empty() {
            inner.trusted_filters.remove(repo);
        } else {
            inner
                .trusted_filters
                .insert(repo.clone(), fingerprints.iter().cloned().collect());
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn rec(id: &str, name: &str, root: &str) -> RepoRecord {
        RepoRecord {
            id: RepoId(id.to_string()),
            name: name.to_string(),
            root_path: PathBuf::from(root),
            common_dir: PathBuf::from(root).join(".git"),
        }
    }

    #[test]
    fn insert_is_idempotent_and_keeps_the_original() {
        let store = MemoryRepoStore::new();
        let first = rec("a", "app", "/code/app");
        assert_eq!(
            store.insert(first.clone()).unwrap(),
            InsertOutcome::Inserted(first.clone())
        );

        // Same id, different name: the stored record wins, nothing is replaced.
        let again = rec("a", "renamed", "/elsewhere");
        assert_eq!(
            store.insert(again).unwrap(),
            InsertOutcome::AlreadyPresent(first.clone())
        );
        assert_eq!(store.list().unwrap(), vec![first]);
    }

    #[test]
    fn get_and_remove() {
        let store = MemoryRepoStore::new();
        let r = rec("a", "app", "/code/app");
        store.insert(r.clone()).unwrap();

        assert_eq!(store.get(&r.id).unwrap(), Some(r.clone()));
        assert_eq!(store.get(&RepoId("zzz".into())).unwrap(), None);

        assert!(store.remove(&r.id).unwrap());
        assert!(!store.remove(&r.id).unwrap()); // already gone
        assert_eq!(store.get(&r.id).unwrap(), None);
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn list_is_ordered_by_name_then_path() {
        let store = MemoryRepoStore::new();
        // Inserted out of order, with two repos sharing a name.
        for r in [
            rec("3", "zeta", "/a/zeta"),
            rec("2", "app", "/b/app"),
            rec("1", "app", "/a/app"),
        ] {
            store.insert(r).unwrap();
        }
        let ids: Vec<String> = store.list().unwrap().into_iter().map(|r| r.id.0).collect();
        assert_eq!(ids, ["1", "2", "3"]);
    }

    #[test]
    fn base_override_set_replace_clear() {
        let store = MemoryRepoStore::new();
        let wt = WorktreeId("w1".into());
        assert_eq!(store.base_override(&wt).unwrap(), None);

        store
            .set_base_override(&wt, Some("origin/main".into()))
            .unwrap();
        assert_eq!(
            store.base_override(&wt).unwrap().as_deref(),
            Some("origin/main")
        );

        store
            .set_base_override(&wt, Some("develop".into()))
            .unwrap();
        assert_eq!(
            store.base_override(&wt).unwrap().as_deref(),
            Some("develop")
        );

        store.set_base_override(&wt, None).unwrap();
        assert_eq!(store.base_override(&wt).unwrap(), None);
        // Overrides are per worktree.
        assert_eq!(store.base_override(&WorktreeId("w2".into())).unwrap(), None);
    }

    #[test]
    fn filter_trust_is_replaced_per_repo_and_dies_with_the_registration() {
        let store = MemoryRepoStore::new();
        let (a, b) = (RepoId("a".into()), RepoId("b".into()));
        store.insert(rec("a", "a", "/a")).unwrap();
        store.insert(rec("b", "b", "/b")).unwrap();
        assert!(store.trusted_filters(&a).unwrap().is_empty());

        store
            .set_trusted_filters(&a, &["f1".into(), "f2".into()])
            .unwrap();
        store.set_trusted_filters(&b, &["x".into()]).unwrap();
        assert_eq!(store.trusted_filters(&a).unwrap().len(), 2);

        // Replace, not add.
        store.set_trusted_filters(&a, &["f3".into()]).unwrap();
        let set = store.trusted_filters(&a).unwrap();
        assert!(set.contains("f3") && !set.contains("f1"));
        assert!(store.trusted_filters(&b).unwrap().contains("x"));

        // Removing the repository drops its trust: a re-add starts untrusted.
        assert!(store.remove(&a).unwrap());
        assert!(store.trusted_filters(&a).unwrap().is_empty());

        store.set_trusted_filters(&b, &[]).unwrap();
        assert!(store.trusted_filters(&b).unwrap().is_empty());
    }

    #[test]
    fn a_late_trust_write_for_a_removed_repository_is_refused() {
        // The trust command scans first and writes later; the user may have
        // removed the repository in between.
        let store = MemoryRepoStore::new();
        let a = RepoId("a".into());
        // Never registered: nothing is written.
        assert!(!store.set_trusted_filters(&a, &["late".into()]).unwrap());
        assert!(store.trusted_filters(&a).unwrap().is_empty());

        store.insert(rec("a", "a", "/a")).unwrap();
        assert!(store.set_trusted_filters(&a, &["f".into()]).unwrap());
        assert!(store.remove(&a).unwrap());
        // The "in-flight" write lands after the removal: refused, and a
        // re-registration of the same path (same id) starts untrusted.
        assert!(!store.set_trusted_filters(&a, &["late".into()]).unwrap());
        store.insert(rec("a", "a", "/a")).unwrap();
        assert!(store.trusted_filters(&a).unwrap().is_empty());
    }

    #[test]
    fn to_contract_carries_id_name_and_path() {
        let c = rec("a", "app", "/code/app").to_contract();
        assert_eq!(c.id, RepoId("a".into()));
        assert_eq!(c.name, "app");
        assert_eq!(c.root_path, "/code/app");
    }

    #[test]
    fn poisoned_lock_is_a_store_error_not_a_panic() {
        let store = std::sync::Arc::new(MemoryRepoStore::new());
        let s = store.clone();
        let _ = std::thread::spawn(move || {
            let _guard = s.inner.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(matches!(store.list(), Err(StoreError::Backend(_))));
    }
}
