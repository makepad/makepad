use super::{PageStore, PageStoreSet, StoreKind, StoreLock, StoreOpenOptions};
use crate::lock::{process_lock, ProcessLock};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Native filesystem implementation of [`PageStore`].
pub struct FilePageStore {
    file: File,
}

impl FilePageStore {
    fn new(file: File) -> Self {
        Self { file }
    }
}

impl PageStore for FilePageStore {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        read_exact_at(&self.file, offset, buf)
    }

    fn write_at(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        write_all_at(&self.file, offset, data)
    }

    fn len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    fn truncate(&self, len: u64) -> io::Result<()> {
        self.file.set_len(len)
    }

    fn sync(&self) -> io::Result<()> {
        crate::sync::sync(&self.file).map_err(|error| match error {
            crate::Error::Io(error) => error,
            other => io::Error::new(io::ErrorKind::Other, other.to_string()),
        })
    }

    fn lock(&self, kind: StoreLock, start: u64, len: u64) -> io::Result<bool> {
        sys::lock(&self.file, kind, start, len)
    }

    fn unlock(&self, start: u64, len: u64) -> io::Result<()> {
        let _ = sys::unlock(&self.file, start, len)?;
        Ok(())
    }

    fn is_write_locked(&self, start: u64, len: u64) -> io::Result<bool> {
        sys::is_write_locked(&self.file, start, len)
    }
}

/// Native opener for a database path and its conventional sibling names.
pub struct FileStoreSet {
    path: PathBuf,
}

impl FileStoreSet {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    fn sibling_path(&self, kind: StoreKind) -> PathBuf {
        match kind {
            StoreKind::Main => self.path.clone(),
            StoreKind::Wal => append_suffix(&self.path, "-wal"),
            StoreKind::Journal => append_suffix(&self.path, "-journal"),
            StoreKind::Shm => append_suffix(&self.path, "-shm"),
        }
    }
}

impl PageStoreSet for FileStoreSet {
    fn open(
        &self,
        kind: StoreKind,
        options: StoreOpenOptions,
    ) -> io::Result<Option<Arc<dyn PageStore>>> {
        let path = self.sibling_path(kind);
        let opened = OpenOptions::new()
            .read(options.read)
            .write(options.write)
            .create(options.create)
            .truncate(options.truncate)
            .open(path);
        match opened {
            Ok(file) => Ok(Some(Arc::new(FilePageStore::new(file)))),
            Err(error) if error.kind() == io::ErrorKind::NotFound && !options.create => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn remove(&self, kind: StoreKind) -> io::Result<()> {
        match std::fs::remove_file(self.sibling_path(kind)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn process_lock(&self) -> Arc<ProcessLock> {
        process_lock(&self.path)
    }

    fn path(&self, kind: StoreKind) -> Option<PathBuf> {
        Some(self.sibling_path(kind))
    }
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

#[cfg(unix)]
fn read_exact_at(file: &File, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    while !buf.is_empty() {
        let read = file.read_at(buf, offset)?;
        if read == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short file read"));
        }
        offset += read as u64;
        buf = &mut buf[read..];
    }
    Ok(())
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let read = file.seek_read(buf, offset)?;
        if read == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short file read"));
        }
        offset += read as u64;
        buf = &mut buf[read..];
    }
    Ok(())
}

#[cfg(unix)]
fn write_all_at(file: &File, mut offset: u64, mut data: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    while !data.is_empty() {
        let written = file.write_at(data, offset)?;
        if written == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "short file write"));
        }
        offset += written as u64;
        data = &data[written..];
    }
    Ok(())
}

#[cfg(windows)]
fn write_all_at(file: &File, mut offset: u64, mut data: &[u8]) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !data.is_empty() {
        let written = file.seek_write(data, offset)?;
        if written == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "short file write"));
        }
        offset += written as u64;
        data = &data[written..];
    }
    Ok(())
}

#[cfg(unix)]
mod sys {
    use super::StoreLock;
    use std::fs::File;
    use std::io;
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

    #[cfg(target_os = "macos")]
    #[repr(C)]
    struct Flock {
        l_start: i64,
        l_len: i64,
        l_pid: i32,
        l_type: i16,
        l_whence: i16,
    }
    #[cfg(not(target_os = "macos"))]
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

    fn flock(kind: i16, start: u64, len: u64) -> Flock {
        #[cfg(target_os = "macos")]
        return Flock {
            l_start: start as i64,
            l_len: len as i64,
            l_pid: 0,
            l_type: kind,
            l_whence: 0,
        };
        #[cfg(not(target_os = "macos"))]
        return Flock {
            l_type: kind,
            l_whence: 0,
            l_start: start as i64,
            l_len: len as i64,
            l_pid: 0,
        };
    }

    fn set(file: &File, kind: i16, start: u64, len: u64) -> io::Result<bool> {
        let flock = flock(kind, start, len);
        let rc = unsafe { fcntl(file.as_raw_fd(), F_SETLK, &flock as *const Flock) };
        if rc == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(13) | Some(11) | Some(35) => Ok(false),
            _ => Err(error),
        }
    }

    pub fn lock(file: &File, kind: StoreLock, start: u64, len: u64) -> io::Result<bool> {
        set(
            file,
            match kind {
                StoreLock::Shared => F_RDLCK,
                StoreLock::Exclusive => F_WRLCK,
            },
            start,
            len,
        )
    }

    pub fn unlock(file: &File, start: u64, len: u64) -> io::Result<bool> {
        set(file, F_UNLCK, start, len)
    }

    pub fn is_write_locked(file: &File, start: u64, len: u64) -> io::Result<bool> {
        const F_GETLK: i32 = if cfg!(target_os = "macos") { 7 } else { 5 };
        let mut flock = flock(F_WRLCK, start, len);
        let rc = unsafe { fcntl(file.as_raw_fd(), F_GETLK, &mut flock as *mut Flock) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(flock_type(&flock) != F_UNLCK)
    }

    #[cfg(target_os = "macos")]
    fn flock_type(flock: &Flock) -> i16 {
        flock.l_type
    }
    #[cfg(not(target_os = "macos"))]
    fn flock_type(flock: &Flock) -> i16 {
        flock.l_type
    }
}

#[cfg(windows)]
mod sys {
    use super::StoreLock;
    use std::fs::File;
    use std::io;
    use std::os::windows::io::AsRawHandle;

    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 1;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 2;
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

    fn overlapped(start: u64) -> Overlapped {
        Overlapped {
            internal: 0,
            internal_high: 0,
            offset: start as u32,
            offset_high: (start >> 32) as u32,
            event: std::ptr::null_mut(),
        }
    }

    pub fn lock(file: &File, kind: StoreLock, start: u64, len: u64) -> io::Result<bool> {
        let mut ov = overlapped(start);
        let flags = LOCKFILE_FAIL_IMMEDIATELY
            | if kind == StoreLock::Exclusive {
                LOCKFILE_EXCLUSIVE_LOCK
            } else {
                0
            };
        let rc = unsafe {
            LockFileEx(
                file.as_raw_handle() as *mut std::ffi::c_void,
                flags,
                0,
                len as u32,
                (len >> 32) as u32,
                &mut ov,
            )
        };
        if rc != 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(ERROR_LOCK_VIOLATION) | Some(ERROR_IO_PENDING) => Ok(false),
            _ => Err(error),
        }
    }

    pub fn unlock(file: &File, start: u64, len: u64) -> io::Result<bool> {
        let mut ov = overlapped(start);
        let rc = unsafe {
            UnlockFileEx(
                file.as_raw_handle() as *mut std::ffi::c_void,
                0,
                len as u32,
                (len >> 32) as u32,
                &mut ov,
            )
        };
        if rc != 0 {
            Ok(true)
        } else {
            // Unlocking a range not held by this handle is harmless here.
            Ok(true)
        }
    }

    pub fn is_write_locked(file: &File, start: u64, len: u64) -> io::Result<bool> {
        if lock(file, StoreLock::Exclusive, start, len)? {
            unlock(file, start, len)?;
            Ok(false)
        } else {
            Ok(true)
        }
    }
}
