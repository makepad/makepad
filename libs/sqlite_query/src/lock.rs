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
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

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
// Platform primitives
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Read,
    Write,
    Unlock,
}

#[cfg(unix)]
mod sys {
    use super::Kind;
    use std::fs::File;
    use std::os::unix::io::AsRawFd;

    #[cfg(target_os = "macos")]
    mod consts {
        pub const F_SETLK: i32 = 8;
        pub const F_RDLCK: i16 = 1;
        pub const F_UNLCK: i16 = 2;
        pub const F_WRLCK: i16 = 3;
    }
    #[cfg(not(target_os = "macos"))]
    mod consts {
        pub const F_SETLK: i32 = 6;
        pub const F_RDLCK: i16 = 0;
        pub const F_WRLCK: i16 = 1;
        pub const F_UNLCK: i16 = 2;
    }
    use consts::*;

    // struct flock has a different field order on macOS and Linux; both are
    // spelled out here rather than pulled from a bindings crate.
    #[cfg(target_os = "macos")]
    #[repr(C)]
    struct Flock {
        l_start: i64,
        l_len: i64,
        l_pid: i32,
        l_type: i16,
        l_whence: i16,
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    #[repr(C)]
    struct Flock {
        l_type: i16,
        l_whence: i16,
        l_start: i64,
        l_len: i64,
        l_pid: i32,
    }

    extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }

    /// True when another process holds a write lock on `start..start+len`.
    /// F_GETLK never reports this process's own locks, which is exactly what
    /// the caller wants: same-process coordination goes through the registry.
    pub fn is_write_locked(file: &File, start: u64, len: u64) -> std::io::Result<bool> {
        const F_GETLK: i32 = if cfg!(target_os = "macos") { 7 } else { 5 };
        #[cfg(target_os = "macos")]
        let mut fl = Flock {
            l_start: start as i64,
            l_len: len as i64,
            l_pid: 0,
            l_type: F_WRLCK,
            l_whence: 0,
        };
        #[cfg(not(target_os = "macos"))]
        let mut fl = Flock {
            l_type: F_WRLCK,
            l_whence: 0,
            l_start: start as i64,
            l_len: len as i64,
            l_pid: 0,
        };
        // Safety: same contract as `lock`; the kernel fills `fl` in place.
        let rc = unsafe { fcntl(file.as_raw_fd(), F_GETLK, &mut fl as *mut Flock) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(fl.l_type != F_UNLCK)
    }

    /// Returns Ok(true) when the lock was granted, Ok(false) when it is held by
    /// someone else, Err for anything unexpected.
    pub fn lock(file: &File, kind: Kind, start: u64, len: u64) -> std::io::Result<bool> {
        let l_type = match kind {
            Kind::Read => F_RDLCK,
            Kind::Write => F_WRLCK,
            Kind::Unlock => F_UNLCK,
        };
        #[cfg(target_os = "macos")]
        let fl = Flock {
            l_start: start as i64,
            l_len: len as i64,
            l_pid: 0,
            l_type,
            l_whence: 0, // SEEK_SET
        };
        #[cfg(not(target_os = "macos"))]
        let fl = Flock {
            l_type,
            l_whence: 0, // SEEK_SET
            l_start: start as i64,
            l_len: len as i64,
            l_pid: 0,
        };
        // Safety: `fl` is a correctly shaped `struct flock` for this platform
        // and lives for the duration of the call; fd is owned by `file`.
        let rc = unsafe { fcntl(file.as_raw_fd(), F_SETLK, &fl as *const Flock) };
        if rc == 0 {
            return Ok(true);
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            // EACCES / EAGAIN: another process holds a conflicting lock.
            Some(13) | Some(11) | Some(35) => Ok(false),
            _ => Err(err),
        }
    }
}

#[cfg(windows)]
mod sys {
    use super::Kind;
    use std::fs::File;
    use std::os::windows::io::AsRawHandle;

    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x0000_0001;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x0000_0002;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    const ERROR_IO_PENDING: i32 = 997;

    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut std::ffi::c_void,
    }

    extern "system" {
        fn LockFileEx(
            handle: *mut std::ffi::c_void,
            flags: u32,
            reserved: u32,
            bytes_low: u32,
            bytes_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn UnlockFileEx(
            handle: *mut std::ffi::c_void,
            reserved: u32,
            bytes_low: u32,
            bytes_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }

    /// Windows has no F_GETLK: probe by taking the lock and giving it back.
    pub fn is_write_locked(file: &File, start: u64, len: u64) -> std::io::Result<bool> {
        if lock(file, Kind::Write, start, len)? {
            lock(file, Kind::Unlock, start, len)?;
            return Ok(false);
        }
        Ok(true)
    }

    pub fn lock(file: &File, kind: Kind, start: u64, len: u64) -> std::io::Result<bool> {
        let mut ov = Overlapped {
            internal: 0,
            internal_high: 0,
            offset: start as u32,
            offset_high: (start >> 32) as u32,
            event: std::ptr::null_mut(),
        };
        let handle = file.as_raw_handle() as *mut std::ffi::c_void;
        let low = len as u32;
        let high = (len >> 32) as u32;
        // Safety: handle is owned by `file`, `ov` outlives the call.
        let rc = unsafe {
            match kind {
                Kind::Unlock => UnlockFileEx(handle, 0, low, high, &mut ov),
                Kind::Read => LockFileEx(handle, LOCKFILE_FAIL_IMMEDIATELY, 0, low, high, &mut ov),
                Kind::Write => LockFileEx(
                    handle,
                    LOCKFILE_FAIL_IMMEDIATELY | LOCKFILE_EXCLUSIVE_LOCK,
                    0,
                    low,
                    high,
                    &mut ov,
                ),
            }
        };
        if rc != 0 {
            return Ok(true);
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(ERROR_LOCK_VIOLATION) | Some(ERROR_IO_PENDING) => Ok(false),
            // Unlocking a range that is not locked is not an error for us.
            _ if kind == Kind::Unlock => Ok(true),
            _ => Err(err),
        }
    }
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
    pub fn acquire(&mut self, file: &File, want: LockLevel, timeout: Duration) -> Result<bool> {
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

    /// One attempt at reaching `want`.
    pub fn try_acquire(&mut self, file: &File, want: LockLevel) -> Result<bool> {
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
                if !sys::lock(file, Kind::Read, PENDING_BYTE, 1).map_err(Error::Io)? {
                    return Ok(false);
                }
                let got = sys::lock(file, Kind::Read, SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
                sys::lock(file, Kind::Unlock, PENDING_BYTE, 1).map_err(Error::Io)?;
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
                let got = sys::lock(file, Kind::Write, RESERVED_BYTE, 1).map_err(Error::Io)?;
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
                    if !sys::lock(file, Kind::Write, PENDING_BYTE, 1).map_err(Error::Io)? {
                        return Ok(false);
                    }
                    self.level = LockLevel::Pending;
                }
                if want == LockLevel::Pending {
                    return Ok(true);
                }
                // Waits for every reader to drop its shared lock.
                let got = sys::lock(file, Kind::Write, SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
                if got {
                    self.level = LockLevel::Exclusive;
                }
                Ok(got)
            }
        }
    }

    /// Step back down to SHARED, keeping the read snapshot.
    pub fn downgrade_to_shared(&mut self, file: &File) -> Result<()> {
        if self.level <= LockLevel::Shared {
            return Ok(());
        }
        // Re-take the shared range as a read lock, then drop the write bytes.
        sys::lock(file, Kind::Unlock, SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
        let got = sys::lock(file, Kind::Read, SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
        sys::lock(file, Kind::Unlock, PENDING_BYTE, 1).map_err(Error::Io)?;
        sys::lock(file, Kind::Unlock, RESERVED_BYTE, 1).map_err(Error::Io)?;
        self.level = if got {
            LockLevel::Shared
        } else {
            LockLevel::None
        };
        Ok(())
    }

    pub fn release(&mut self, file: &File) -> Result<()> {
        if self.level == LockLevel::None {
            return Ok(());
        }
        sys::lock(file, Kind::Unlock, SHARED_FIRST, SHARED_SIZE).map_err(Error::Io)?;
        sys::lock(file, Kind::Unlock, PENDING_BYTE, 1).map_err(Error::Io)?;
        sys::lock(file, Kind::Unlock, RESERVED_BYTE, 1).map_err(Error::Io)?;
        self.level = LockLevel::None;
        Ok(())
    }
}

/// Take (or release) an arbitrary byte range: used for the WAL locking slots
/// in a `-shm` file, which live outside the database's own locking region.
pub fn try_lock_range(file: &File, start: u64, len: u64, exclusive: bool) -> Result<bool> {
    let kind = if exclusive { Kind::Write } else { Kind::Read };
    sys::lock(file, kind, start, len).map_err(Error::Io)
}

pub fn unlock_range(file: &File, start: u64, len: u64) -> Result<()> {
    sys::lock(file, Kind::Unlock, start, len).map_err(Error::Io)?;
    Ok(())
}

/// True when another process is in the middle of a write transaction on this
/// file (it holds RESERVED). A journal file is only "hot" — i.e. left behind by
/// a crash — when nobody holds it.
pub fn reserved_lock_held(file: &File) -> Result<bool> {
    sys::is_write_locked(file, RESERVED_BYTE, 1).map_err(Error::Io)
}

impl Default for FileLock {
    fn default() -> Self {
        FileLock::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(tag: &str) -> (std::path::PathBuf, File) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-sqlite-lock-{tag}-{}-{nonce}",
            std::process::id()
        ));
        let file = File::options()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        (path, file)
    }

    #[test]
    fn lock_ladder_moves_up_and_down() {
        let (path, file) = temp_file("ladder");
        let mut lock = FileLock::new();
        assert_eq!(lock.level(), LockLevel::None);
        assert!(lock.try_acquire(&file, LockLevel::Shared).unwrap());
        assert_eq!(lock.level(), LockLevel::Shared);
        assert!(lock.try_acquire(&file, LockLevel::Reserved).unwrap());
        assert!(lock.try_acquire(&file, LockLevel::Exclusive).unwrap());
        assert_eq!(lock.level(), LockLevel::Exclusive);
        lock.downgrade_to_shared(&file).unwrap();
        assert_eq!(lock.level(), LockLevel::Shared);
        lock.release(&file).unwrap();
        assert_eq!(lock.level(), LockLevel::None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn locks_are_advisory_and_do_not_touch_the_file() {
        let (path, file) = temp_file("size");
        let mut lock = FileLock::new();
        lock.try_acquire(&file, LockLevel::Exclusive).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 0, "locking grew the file");
        lock.release(&file).unwrap();
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
    fn new() -> ProcessLock {
        ProcessLock {
            state: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    /// True while some connection in this process holds the write slot.
    pub fn is_write_held(&self) -> bool {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Take the in-process write slot, waiting up to `timeout`.
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

fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<ProcessLock>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<ProcessLock>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The process-wide lock for one database file.
pub fn process_lock(path: &Path) -> Arc<ProcessLock> {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut map = registry().lock().unwrap_or_else(|e| e.into_inner());
    map.entry(key)
        .or_insert_with(|| Arc::new(ProcessLock::new()))
        .clone()
}
