//! Storage behind the pager.
//!
//! The pager, rollback journal and WAL all use this interface. A store set is
//! one SQLite database and its sibling files; it is deliberately synchronous
//! so a future persistent web backend can put an IndexedDB-backed cache behind
//! the same page-oriented API.

use crate::lock::ProcessLock;
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(not(target_arch = "wasm32"))]
mod file;
#[cfg(not(target_arch = "wasm32"))]
pub use file::{FilePageStore, FileStoreSet};

/// Which file belonging to one SQLite database is being opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreKind {
    Main,
    Wal,
    Journal,
    Shm,
}

/// The access and creation behavior for one store open.
#[derive(Debug, Clone, Copy)]
pub struct StoreOpenOptions {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub truncate: bool,
}

impl StoreOpenOptions {
    pub const READ_ONLY: Self = Self {
        read: true,
        write: false,
        create: false,
        truncate: false,
    };
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        create: false,
        truncate: false,
    };
    pub const CREATE: Self = Self {
        read: true,
        write: true,
        create: true,
        truncate: false,
    };
    pub const CREATE_TRUNCATE: Self = Self {
        read: true,
        write: true,
        create: true,
        truncate: true,
    };
}

/// Advisory byte-range lock flavor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreLock {
    Shared,
    Exclusive,
}

/// One random-access file as seen by SQLite's pager.
pub trait PageStore: Send + Sync {
    /// Fill `buf` from exactly `offset`, returning `UnexpectedEof` if short.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
    fn write_at(&self, offset: u64, data: &[u8]) -> io::Result<()>;
    fn len(&self) -> io::Result<u64>;
    fn truncate(&self, len: u64) -> io::Result<()>;
    fn sync(&self) -> io::Result<()>;
    fn lock(&self, kind: StoreLock, start: u64, len: u64) -> io::Result<bool>;
    fn unlock(&self, start: u64, len: u64) -> io::Result<()>;
    fn is_write_locked(&self, start: u64, len: u64) -> io::Result<bool>;
}

/// Opens the main database and its WAL/journal/SHM siblings.
pub trait PageStoreSet: Send + Sync {
    /// `Ok(None)` means the requested sibling does not exist and `create` was
    /// false.
    fn open(
        &self,
        kind: StoreKind,
        options: StoreOpenOptions,
    ) -> io::Result<Option<Arc<dyn PageStore>>>;

    /// Unlink a sibling. Existing handles may continue to use its old bytes.
    fn remove(&self, kind: StoreKind) -> io::Result<()>;

    /// Same-process write serialization complements platform file locks.
    fn process_lock(&self) -> Arc<ProcessLock>;

    /// Only the native file implementation has a meaningful filesystem path.
    fn path(&self, _kind: StoreKind) -> Option<PathBuf> {
        None
    }
}

impl<T: PageStoreSet + ?Sized> PageStoreSet for Arc<T> {
    fn open(
        &self,
        kind: StoreKind,
        options: StoreOpenOptions,
    ) -> io::Result<Option<Arc<dyn PageStore>>> {
        (**self).open(kind, options)
    }

    fn remove(&self, kind: StoreKind) -> io::Result<()> {
        (**self).remove(kind)
    }

    fn process_lock(&self) -> Arc<ProcessLock> {
        (**self).process_lock()
    }

    fn path(&self, kind: StoreKind) -> Option<PathBuf> {
        (**self).path(kind)
    }
}

pub const MEMORY_DIRTY_PAGE_BYTES: u64 = 4 * 1024;

#[derive(Default)]
struct MemoryFileState {
    bytes: Vec<u8>,
    dirty_pages: BTreeMap<u64, u64>,
    mutation_epoch: u64,
}

#[derive(Default)]
struct MemoryFile {
    state: Mutex<MemoryFileState>,
    locks: Mutex<Vec<MemoryLock>>,
}

/// A coherent copy of one in-memory SQLite sibling.
///
/// `dirty_pages` contains 4 KiB page indexes changed at or before
/// `mutation_epoch`. A durability coordinator may clear precisely those
/// changes with [`MemoryStoreSet::mark_snapshot_clean`] after its external
/// commit succeeds; writes that raced the snapshot remain dirty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryStoreSnapshot {
    pub kind: StoreKind,
    pub bytes: Vec<u8>,
    pub dirty_pages: Vec<u64>,
    pub mutation_epoch: u64,
}

impl MemoryStoreSnapshot {
    pub fn logical_len(&self) -> u64 {
        self.bytes.len() as u64
    }
}

fn dirty_page_range(start: usize, end: usize) -> std::ops::RangeInclusive<u64> {
    let first = start as u64 / MEMORY_DIRTY_PAGE_BYTES;
    let last_byte = end.saturating_sub(1).max(start) as u64;
    first..=last_byte / MEMORY_DIRTY_PAGE_BYTES
}

impl MemoryFile {
    fn mutate(&self, range_start: usize, range_end: usize, update: impl FnOnce(&mut Vec<u8>)) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.mutation_epoch = state.mutation_epoch.wrapping_add(1).max(1);
        let epoch = state.mutation_epoch;
        for page in dirty_page_range(range_start, range_end) {
            state.dirty_pages.insert(page, epoch);
        }
        update(&mut state.bytes);
    }
}

#[derive(Clone, Copy)]
struct MemoryLock {
    owner: u64,
    kind: StoreLock,
    start: u64,
    len: u64,
}

impl MemoryLock {
    fn overlaps(self, start: u64, len: u64) -> bool {
        self.start < start.saturating_add(len) && start < self.start.saturating_add(self.len)
    }
}

/// A handle onto one file in a [`MemoryStoreSet`].
pub struct MemoryPageStore {
    file: Arc<MemoryFile>,
    owner: u64,
    writable: bool,
}

impl Drop for MemoryPageStore {
    fn drop(&mut self) {
        self.file
            .locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|held| held.owner != self.owner);
    }
}

impl PageStore for MemoryPageStore {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let start = usize::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset is too large"))?;
        let end = start
            .checked_add(buf.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "read is too large"))?;
        let state = self.file.state.lock().unwrap_or_else(|e| e.into_inner());
        let src = state.bytes
            .get(start..end)
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short page-store read"))?;
        buf.copy_from_slice(src);
        Ok(())
    }

    fn write_at(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        if !self.writable {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "page store is read-only",
            ));
        }
        let start = usize::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset is too large"))?;
        let end = start
            .checked_add(data.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "write is too large"))?;
        self.file.mutate(start, end, |bytes| {
            if bytes.len() < end {
                bytes.resize(end, 0);
            }
            bytes[start..end].copy_from_slice(data);
        });
        Ok(())
    }

    fn len(&self) -> io::Result<u64> {
        Ok(self
            .file
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .bytes
            .len() as u64)
    }

    fn truncate(&self, len: u64) -> io::Result<()> {
        if !self.writable {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "page store is read-only",
            ));
        }
        let len = usize::try_from(len)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "length is too large"))?;
        let old_len = self.len()? as usize;
        self.file.mutate(len.min(old_len), len.max(old_len), |bytes| {
            bytes.resize(len, 0);
        });
        Ok(())
    }

    fn sync(&self) -> io::Result<()> {
        Ok(())
    }

    fn lock(&self, kind: StoreLock, start: u64, len: u64) -> io::Result<bool> {
        let mut locks = self.file.locks.lock().unwrap_or_else(|e| e.into_inner());
        let conflict = locks.iter().copied().any(|held| {
            held.owner != self.owner
                && held.overlaps(start, len)
                && (kind == StoreLock::Exclusive || held.kind == StoreLock::Exclusive)
        });
        if conflict {
            return Ok(false);
        }
        locks.push(MemoryLock {
            owner: self.owner,
            kind,
            start,
            len,
        });
        Ok(true)
    }

    fn unlock(&self, start: u64, len: u64) -> io::Result<()> {
        self.file
            .locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|held| held.owner != self.owner || !held.overlaps(start, len));
        Ok(())
    }

    fn is_write_locked(&self, start: u64, len: u64) -> io::Result<bool> {
        Ok(self
            .file
            .locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .any(|held| {
                held.owner != self.owner
                    && held.kind == StoreLock::Exclusive
                    && held.overlaps(start, len)
            }))
    }
}

struct MemoryStoreSetInner {
    files: Mutex<HashMap<StoreKind, Arc<MemoryFile>>>,
    next_owner: AtomicU64,
    process_lock: Arc<ProcessLock>,
}

/// A complete in-memory SQLite database, including journal/WAL siblings.
#[derive(Clone)]
pub struct MemoryStoreSet {
    inner: Arc<MemoryStoreSetInner>,
}

impl MemoryStoreSet {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MemoryStoreSetInner {
                files: Mutex::new(HashMap::new()),
                next_owner: AtomicU64::new(1),
                process_lock: Arc::new(ProcessLock::new()),
            }),
        }
    }

    /// Snapshot one sibling without exposing its mutable backing allocation.
    pub fn snapshot(&self, kind: StoreKind) -> io::Result<Option<MemoryStoreSnapshot>> {
        let file = self
            .inner
            .files
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&kind)
            .cloned();
        let Some(file) = file else { return Ok(None) };
        let state = file.state.lock().unwrap_or_else(|error| error.into_inner());
        Ok(Some(MemoryStoreSnapshot {
            kind,
            bytes: state.bytes.clone(),
            dirty_pages: state.dirty_pages.keys().copied().collect(),
            mutation_epoch: state.mutation_epoch,
        }))
    }

    /// Hydrate or replace one sibling with externally verified bytes.
    /// Restored bytes are clean: they describe the durable baseline.
    pub fn restore(&self, kind: StoreKind, bytes: Vec<u8>) -> io::Result<()> {
        let file = {
            let mut files = self
                .inner
                .files
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            files
                .entry(kind)
                .or_insert_with(|| Arc::new(MemoryFile::default()))
                .clone()
        };
        let mut state = file.state.lock().unwrap_or_else(|error| error.into_inner());
        state.bytes = bytes;
        state.dirty_pages.clear();
        state.mutation_epoch = state.mutation_epoch.wrapping_add(1).max(1);
        Ok(())
    }

    /// Clear only dirtiness represented by `snapshot`.
    pub fn mark_snapshot_clean(&self, snapshot: &MemoryStoreSnapshot) -> io::Result<()> {
        let file = self
            .inner
            .files
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&snapshot.kind)
            .cloned();
        let Some(file) = file else { return Ok(()) };
        let mut state = file.state.lock().unwrap_or_else(|error| error.into_inner());
        state
            .dirty_pages
            .retain(|_, epoch| *epoch > snapshot.mutation_epoch);
        Ok(())
    }
}

impl Default for MemoryStoreSet {
    fn default() -> Self {
        Self::new()
    }
}

impl PageStoreSet for MemoryStoreSet {
    fn open(
        &self,
        kind: StoreKind,
        options: StoreOpenOptions,
    ) -> io::Result<Option<Arc<dyn PageStore>>> {
        let mut files = self.inner.files.lock().unwrap_or_else(|e| e.into_inner());
        let file = match files.get(&kind) {
            Some(file) => file.clone(),
            None if options.create => {
                let file = Arc::new(MemoryFile::default());
                files.insert(kind, file.clone());
                file
            }
            None => return Ok(None),
        };
        if options.truncate {
            let old_len = file
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .bytes
                .len();
            file.mutate(0, old_len, Vec::clear);
        }
        let owner = self.inner.next_owner.fetch_add(1, Ordering::Relaxed);
        Ok(Some(Arc::new(MemoryPageStore {
            file,
            owner,
            writable: options.write,
        })))
    }

    fn remove(&self, kind: StoreKind) -> io::Result<()> {
        self.inner
            .files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&kind);
        Ok(())
    }

    fn process_lock(&self) -> Arc<ProcessLock> {
        self.inner.process_lock.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_handles_share_bytes_and_observe_locks() {
        let stores = MemoryStoreSet::new();
        let first = stores
            .open(StoreKind::Main, StoreOpenOptions::CREATE)
            .unwrap()
            .unwrap();
        let second = stores
            .open(StoreKind::Main, StoreOpenOptions::READ_WRITE)
            .unwrap()
            .unwrap();
        first.write_at(7, b"page").unwrap();
        let mut got = [0; 4];
        second.read_at(7, &mut got).unwrap();
        assert_eq!(&got, b"page");
        let lock_byte = crate::lock::PENDING_BYTE;
        assert!(first.lock(StoreLock::Exclusive, lock_byte, 3).unwrap());
        assert!(!second.lock(StoreLock::Shared, lock_byte, 3).unwrap());
        assert_eq!(first.len().unwrap(), 11, "locking at 1 GiB grew the store");
        first.unlock(lock_byte, 3).unwrap();
        assert!(second.lock(StoreLock::Shared, lock_byte, 3).unwrap());
    }

    #[test]
    fn snapshots_track_pages_epochs_restore_and_precise_cleaning() {
        let stores = MemoryStoreSet::new();
        let main = stores
            .open(StoreKind::Main, StoreOpenOptions::CREATE)
            .unwrap()
            .unwrap();
        main.write_at(0, b"header").unwrap();
        main.write_at(MEMORY_DIRTY_PAGE_BYTES + 7, b"page-one").unwrap();
        let first = stores.snapshot(StoreKind::Main).unwrap().unwrap();
        assert_eq!(first.dirty_pages, vec![0, 1]);
        assert_eq!(first.logical_len(), MEMORY_DIRTY_PAGE_BYTES + 15);

        main.write_at(MEMORY_DIRTY_PAGE_BYTES * 2, b"newer").unwrap();
        stores.mark_snapshot_clean(&first).unwrap();
        let second = stores.snapshot(StoreKind::Main).unwrap().unwrap();
        assert_eq!(second.dirty_pages, vec![2]);
        assert!(second.mutation_epoch > first.mutation_epoch);

        let restored = MemoryStoreSet::new();
        restored.restore(StoreKind::Main, first.bytes.clone()).unwrap();
        let snapshot = restored.snapshot(StoreKind::Main).unwrap().unwrap();
        assert_eq!(snapshot.bytes, first.bytes);
        assert!(snapshot.dirty_pages.is_empty());
    }
}
