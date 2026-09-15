//! SQLite's file-locking protocol, byte for byte.
//!
//! Databases are shared with the `sqlite3` CLI and with the C library, so the
//! locks must live in exactly the same places: advisory byte-range locks on the
//! reserved region that starts one gigabyte into the file (see
//! <https://www.sqlite.org/lockingv3.html>). The region is past the end of
//! every real database, so the bytes themselves are never read or written.
//!
//! The FFI here is the whole unsafe surface of this crate: `fcntl` on unix,
//! `LockFileEx` on Windows. Both are wrapped so callers only see `Result`.

use crate::error::{Error, Result};
use crate::storage::{PageStore, StoreLock};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, OnceLock};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

/// First byte of the reserved locking region (1 GiB).
pub const PENDING_BYTE: u64 = 0x4000_0000;
pub const RESERVED_BYTE: u64 = PENDING_BYTE + 1;
pub const SHARED_FIRST: u64 = PENDING_BYTE + 2;
pub const SHARED_SIZE: u64 = 510;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LockLevel {
    None,
    /// Reading is allowed; no writer may commit.
    Shared,
    /// This connection intends to write, readers continue.
    Reserved,
    /// No new readers may start.
    Pending,
    /// Sole access: the database file may be modified.
    Exclusive,
}

// ---------------------------------------------------------------------------
// The protocol
// ---------------------------------------------------------------------------

/// Lock state of one open database file.
pub struct FileLock {
    level: LockLevel,
}

impl FileLock {
    pub fn new() -> FileLock {
        FileLock {
            level: LockLevel::None,
        }
    }

    pub fn level(&self) -> LockLevel {
        self.level
    }

    /// Move to `want`, retrying until `timeout` elapses. Returns false when the
    /// lock could not be taken in time (SQLITE_BUSY).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn acquire(&mut self, file: &dyn PageStore, want: LockLevel, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.try_acquire(file, want)? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// In the browser a store has exactly one owner, so there is nobody to
    /// wait for: one attempt is the whole story (and the web has no clock or
    /// sleep to wait with).
    #[cfg(target_arch = "wasm32")]
    pub fn acquire(&mut self, file: &dyn PageStore, want: LockLevel, _timeout: Duration) -> Result<bool> {
        self.try_acquire(file, want)
    }

    /// One attempt at reaching `want`.
    pub fn try_acquire(&mut self, file: &dyn PageStore, want: LockLevel) -> Result<bool> {
        if self.level >= want {
            return Ok(true);
        }
        match want {
            LockLevel::None => {
                self.release(file)?;
                Ok(true)
            }
            LockLevel::Shared => {
                // A pending read lock first, so a writer waiting to become
                // EXCLUSIVE is not starved by a stream of new readers.
                if !file.lock(StoreLock::Shared, PENDING_BYTE, 1).map_err(Error::Io)? {
                    return Ok(false);
                }
                let got = file.lock(StoreLock::Shared, SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
                file.unlock(PENDING_BYTE, 1).map_err(Error::Io)?;
                if got {
                    self.level = LockLevel::Shared;
                }
                Ok(got)
            }
            LockLevel::Reserved => {
                if self.level != LockLevel::Shared {
                    if !self.try_acquire(file, LockLevel::Shared)? {
                        return Ok(false);
                    }
                }
                let got = file.lock(StoreLock::Exclusive, RESERVED_BYTE, 1).map_err(Error::Io)?;
                if got {
                    self.level = LockLevel::Reserved;
                }
                Ok(got)
            }
            LockLevel::Pending | LockLevel::Exclusive => {
                if self.level < LockLevel::Reserved {
                    if !self.try_acquire(file, LockLevel::Reserved)? {
                        return Ok(false);
                    }
                }
                if self.level < LockLevel::Pending {
                    if !file.lock(StoreLock::Exclusive, PENDING_BYTE, 1).map_err(Error::Io)? {
                        return Ok(false);
                    }
                    self.level = LockLevel::Pending;
                }
                if want == LockLevel::Pending {
                    return Ok(true);
                }
                // Waits for every reader to drop its shared lock.
                //
                // This handle is one of those readers: reaching SHARED took a
                // READ lock on exactly this range. Whether asking for the
                // WRITE lock on top of it is an upgrade or a collision is the
                // one place the two lock APIs genuinely disagree.
                //
                // POSIX `fcntl` locks belong to the PROCESS, and a second
                // request over a range it already holds simply CONVERTS it —
                // so unix reaches EXCLUSIVE by asking, and that is what this
                // code did everywhere.
                //
                // Windows `LockFileEx` locks belong to the HANDLE, and ranges
                // may not overlap: the request is refused with
                // ERROR_LOCK_VIOLATION against our OWN read lock, forever, no
                // matter how long the busy timeout is. That is what made
                // `PRAGMA journal_mode=WAL` on a brand-new database answer
                // SQLITE_BUSY on Windows and only on Windows — with the
                // embedded asset store unable to open a catalog at all.
                //
                // So do what SQLite's own Windows VFS does in `winLock`: drop
                // the shared range first, then take it exclusively. Losing
                // that race means another process got in between, and the
                // read lock has to go back so this handle is still the SHARED
                // (+PENDING) holder it claims to be.
                #[cfg(windows)]
                {
                    file.unlock(SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
                    let got = file.lock(StoreLock::Exclusive, SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
                    if got {
                        self.level = LockLevel::Exclusive;
                    } else {
                        file.lock(StoreLock::Shared, SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
                    }
                    return Ok(got);
                }
                #[cfg(not(windows))]
                {
                    let got = file.lock(StoreLock::Exclusive, SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
                    if got {
                        self.level = LockLevel::Exclusive;
                    }
                    Ok(got)
                }
            }
        }
    }

    /// Step back down to SHARED, keeping the read snapshot.
    pub fn downgrade_to_shared(&mut self, file: &dyn PageStore) -> Result<()> {
        if self.level <= LockLevel::Shared {
            return Ok(());
        }
        // Re-take the shared range as a read lock, then drop the write bytes.
        file.unlock(SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
        let got = file.lock(StoreLock::Shared, SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
        file.unlock(PENDING_BYTE, 1).map_err(Error::Io)?;
        file.unlock(RESERVED_BYTE, 1).map_err(Error::Io)?;
        self.level = if got {
            LockLevel::Shared
        } else {
            LockLevel::None
        };
        Ok(())
    }

    pub fn release(&mut self, file: &dyn PageStore) -> Result<()> {
        if self.level == LockLevel::None {
            return Ok(());
        }
        file.unlock(SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
        file.unlock(PENDING_BYTE, 1).map_err(Error::Io)?;
        file.unlock(RESERVED_BYTE, 1).map_err(Error::Io)?;
        self.level = LockLevel::None;
        Ok(())
    }
}

/// Take (or release) an arbitrary byte range: used for the WAL locking slots
/// in a `-shm` file, which live outside the database's own locking region.
pub fn try_lock_range(file: &dyn PageStore, start: u64, len: u64, exclusive: bool) -> Result<bool> {
    let kind = if exclusive { StoreLock::Exclusive } else { StoreLock::Shared };
    file.lock(kind, start, len).map_err(Error::Io)
}

pub fn unlock_range(file: &dyn PageStore, start: u64, len: u64) -> Result<()> {
    file.unlock(start, len).map_err(Error::Io)
}

/// True when another process is in the middle of a write transaction on this
/// file (it holds RESERVED). A journal file is only "hot" — i.e. left behind by
/// a crash — when nobody holds it.
pub fn reserved_lock_held(file: &dyn PageStore) -> Result<bool> {
    file.is_write_locked(RESERVED_BYTE, 1).map_err(Error::Io)
}

impl Default for FileLock {
    fn default() -> Self {
        FileLock::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{FileStoreSet, PageStoreSet, StoreKind, StoreOpenOptions};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(tag: &str) -> (std::path::PathBuf, Arc<dyn PageStore>) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-sqlite-lock-{tag}-{}-{nonce}",
            std::process::id()
        ));
        let file = FileStoreSet::new(&path)
            .open(StoreKind::Main, StoreOpenOptions::CREATE)
            .unwrap()
            .unwrap();
        (path, file)
    }

    #[test]
    fn lock_ladder_moves_up_and_down() {
        let (path, file) = temp_file("ladder");
        let mut lock = FileLock::new();
        assert_eq!(lock.level(), LockLevel::None);
        assert!(lock.try_acquire(file.as_ref(), LockLevel::Shared).unwrap());
        assert_eq!(lock.level(), LockLevel::Shared);
        assert!(lock.try_acquire(file.as_ref(), LockLevel::Reserved).unwrap());
        assert!(lock.try_acquire(file.as_ref(), LockLevel::Exclusive).unwrap());
        assert_eq!(lock.level(), LockLevel::Exclusive);
        lock.downgrade_to_shared(file.as_ref()).unwrap();
        assert_eq!(lock.level(), LockLevel::Shared);
        lock.release(file.as_ref()).unwrap();
        assert_eq!(lock.level(), LockLevel::None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn locks_are_advisory_and_do_not_touch_the_file() {
        let (path, file) = temp_file("size");
        let mut lock = FileLock::new();
        lock.try_acquire(file.as_ref(), LockLevel::Exclusive).unwrap();
        assert_eq!(file.len().unwrap(), 0, "locking grew the file");
        lock.release(file.as_ref()).unwrap();
        let _ = std::fs::remove_file(path);
    }
}


// ---------------------------------------------------------------------------
// In-process coordination
// ---------------------------------------------------------------------------

/// POSIX advisory locks are owned by the *process*, so two connections to the
/// same file inside one process never see each other's locks — SQLite solves
/// this with a per-inode registry and so do we. Writers register here and
/// serialize; cross-process exclusion is still the byte-range locks above.
pub struct ProcessLock {
    state: Mutex<bool>,
    changed: Condvar,
}

impl ProcessLock {
    pub fn new() -> ProcessLock {
        ProcessLock {
            state: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    /// One owner in the browser: the slot is free or it is not, nobody to wait for.
    #[cfg(target_arch = "wasm32")]
    pub fn acquire_write(&self, _timeout: Duration) -> bool {
        let mut held = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *held {
            return false;
        }
        *held = true;
        true
    }

    /// True while some connection in this process holds the write slot.
    pub fn is_write_held(&self) -> bool {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Take the in-process write slot, waiting up to `timeout`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn acquire_write(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut held = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while *held {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (guard, wait) = self
                .changed
                .wait_timeout(held, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            held = guard;
            if wait.timed_out() && *held {
                return false;
            }
        }
        *held = true;
        true
    }

    pub fn release_write(&self) {
        let mut held = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *held = false;
        self.changed.notify_one();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<ProcessLock>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<ProcessLock>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The process-wide lock for one database file.
#[cfg(not(target_arch = "wasm32"))]
pub fn process_lock(path: &Path) -> Arc<ProcessLock> {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut map = registry().lock().unwrap_or_else(|e| e.into_inner());
    map.entry(key)
        .or_insert_with(|| Arc::new(ProcessLock::new()))
        .clone()
}
