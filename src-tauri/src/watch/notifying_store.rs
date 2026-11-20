//! [`NotifyingRepoStore`]: a [`RepoStore`] decorator that wakes the watcher
//! whenever the set of registered repositories changes.
//!
//! The IPC commands (`add_repository`, `remove_repository`) only know the
//! `RepoStore` trait. Wrapping the real store in this decorator (in `lib.rs`)
//! makes every successful registration change trigger a watcher resync within
//! ~300 ms, without the commands knowing the watcher exists, instead of
//! waiting up to 30 s for the periodic reconcile. (Decorator pattern: same
//! interface, extra behaviour, inner object unchanged.)

use std::collections::HashSet;
use std::sync::Arc;

use super::hub::ResyncHandle;
use crate::contracts::{RepoId, WorktreeId};
use crate::store::{InsertOutcome, RepoRecord, RepoStore, StoreError};

pub struct NotifyingRepoStore {
    inner: Arc<dyn RepoStore>,
    resync: ResyncHandle,
}

impl NotifyingRepoStore {
    pub fn new(inner: Arc<dyn RepoStore>, resync: ResyncHandle) -> Self {
        Self { inner, resync }
    }
}

impl RepoStore for NotifyingRepoStore {
    fn list(&self) -> Result<Vec<RepoRecord>, StoreError> {
        self.inner.list()
    }

    fn get(&self, id: &RepoId) -> Result<Option<RepoRecord>, StoreError> {
        self.inner.get(id)
    }

    fn insert(&self, rec: RepoRecord) -> Result<InsertOutcome, StoreError> {
        let outcome = self.inner.insert(rec)?;
        // Only a real change: re-adding a known repository changes nothing.
        if matches!(outcome, InsertOutcome::Inserted(_)) {
            self.resync.request();
        }
        Ok(outcome)
    }

    fn remove(&self, id: &RepoId) -> Result<bool, StoreError> {
        let removed = self.inner.remove(id)?;
        if removed {
            self.resync.request();
        }
        Ok(removed)
    }

    fn base_override(&self, wt: &WorktreeId) -> Result<Option<String>, StoreError> {
        self.inner.base_override(wt)
    }

    fn set_base_override(&self, wt: &WorktreeId, base: Option<String>) -> Result<(), StoreError> {
        self.inner.set_base_override(wt, base)
    }

    fn trusted_filters(&self, repo: &RepoId) -> Result<HashSet<String>, StoreError> {
        self.inner.trusted_filters(repo)
    }

    // Trust changes need no watcher signal: the UI calls the trust command and
    // then re-reads, and the guard consults the store on every read.
    fn set_trusted_filters(
        &self,
        repo: &RepoId,
        fingerprints: &[String],
    ) -> Result<bool, StoreError> {
        self.inner.set_trusted_filters(repo, fingerprints)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryRepoStore;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn signals_only_on_real_registration_changes() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let store = NotifyingRepoStore::new(
            Arc::new(MemoryRepoStore::new()),
            ResyncHandle::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let rec = RepoRecord {
            id: RepoId("a".into()),
            name: "a".into(),
            root_path: PathBuf::from("/a"),
            common_dir: PathBuf::from("/a/.git"),
        };
        let n = || count.load(Ordering::SeqCst);

        store.insert(rec.clone()).unwrap();
        assert_eq!(n(), 1, "inserted");
        store.insert(rec.clone()).unwrap();
        assert_eq!(n(), 1, "already present: no signal");
        store.list().unwrap();
        store
            .set_base_override(&WorktreeId("w".into()), Some("main".into()))
            .unwrap();
        assert_eq!(n(), 1, "reads and base overrides: no signal");
        assert!(store.remove(&rec.id).unwrap());
        assert_eq!(n(), 2, "removed");
        assert!(!store.remove(&rec.id).unwrap());
        assert_eq!(n(), 2, "nothing removed: no signal");
    }
}
