//! Storage behind the pager.
//!
//! The pager, rollback journal and WAL all use this interface. A store set is
//! one SQLite database and its sibling files; it is deliberately synchronous
//! so a future persistent web backend can put an IndexedDB-backed cache behind
//! the same page-oriented API.

use crate::lock::ProcessLock;
use std::collections::HashMap;
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

#[derive(Default)]
struct MemoryFile {
    bytes: Mutex<Vec<u8>>,
    locks: Mutex<Vec<MemoryLock>>,
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
        let bytes = self.file.bytes.lock().unwrap_or_else(|e| e.into_inner());
        let src = bytes
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
        let mut bytes = self.file.bytes.lock().unwrap_or_else(|e| e.into_inner());
        if bytes.len() < end {
            bytes.resize(end, 0);
        }
        bytes[start..end].copy_from_slice(data);
        Ok(())
    }

    fn len(&self) -> io::Result<u64> {
        Ok(self
            .file
            .bytes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
        self.file
            .bytes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .resize(len, 0);
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
            file.bytes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
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
}
