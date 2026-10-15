//! Memory and wakeups shared between a host and its child: a file mapped
//! into both processes, and named semaphores one side posts and the other
//! waits on (no polling, no timeouts). The host creates both and hands their
//! names over the hosted-app channel; it removes them when it is done.
//!
//! Unix (macOS, Linux): a file under the temp dir mapped `MAP_SHARED` and POSIX
//! named semaphores. Windows: the same file mapped through a file mapping
//! object, and named kernel semaphores in the session's `Local\` namespace.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A file mapped read-write and shared.
pub struct SharedMem {
    ptr: *mut u8,
    len: usize,
    path: PathBuf,
    /// The host made it, and removes the file when dropped.
    owner: bool,
    /// Windows: the file mapping object the view is of.
    #[cfg(windows)]
    mapping: makepad_platform::windows::Win32::Foundation::HANDLE,
    /// Closed after the view (Windows cannot remove a file that is still mapped).
    _file: std::fs::File,
}

// The mapping is plain memory both sides reach only through atomics and
// the producer/consumer protocols built on it.
unsafe impl Send for SharedMem {}
unsafe impl Sync for SharedMem {}

#[cfg(unix)]
mod sys {
    use std::os::raw::{c_char, c_int, c_void};
    pub const PROT_READ: c_int = 1;
    pub const PROT_WRITE: c_int = 2;
    pub const MAP_SHARED: c_int = 1;
    #[cfg(target_os = "macos")]
    pub const O_CREAT: c_int = 0x0200;
    #[cfg(not(target_os = "macos"))]
    pub const O_CREAT: c_int = 0x0040;
    extern "C" {
        pub fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, offset: i64) -> *mut c_void;
        pub fn munmap(addr: *mut c_void, len: usize) -> c_int;
        pub fn sem_open(name: *const c_char, oflag: c_int, ...) -> *mut c_void;
        pub fn sem_close(sem: *mut c_void) -> c_int;
        pub fn sem_unlink(name: *const c_char) -> c_int;
        pub fn sem_wait(sem: *mut c_void) -> c_int;
        pub fn sem_post(sem: *mut c_void) -> c_int;
    }
    /// `SEM_FAILED`: `(sem_t *)-1` on macOS, `(sem_t *)0` on Linux.
    pub fn sem_failed(sem: *mut c_void) -> bool {
        sem.is_null() || sem as isize == -1
    }
}

impl SharedMem {
    /// Make a zeroed mapping of `len` bytes backed by a new file at `path`.
    pub fn create(path: &Path, len: usize) -> io::Result<SharedMem> {
        let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(path)?;
        file.set_len(len as u64)?;
        Self::map(file, path, len, true)
    }

    /// Map the file another process made.
    pub fn open(path: &Path, len: usize) -> io::Result<SharedMem> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        if file.metadata()?.len() < len as u64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "shared memory file is shorter than its layout"));
        }
        Self::map(file, path, len, false)
    }

    #[cfg(unix)]
    fn map(file: std::fs::File, path: &Path, len: usize, owner: bool) -> io::Result<SharedMem> {
        use std::os::fd::AsRawFd;
        let ptr = unsafe { sys::mmap(std::ptr::null_mut(), len, sys::PROT_READ | sys::PROT_WRITE, sys::MAP_SHARED, file.as_raw_fd(), 0) };
        if ptr as isize == -1 || ptr.is_null() {
            let error = io::Error::last_os_error();
            if owner {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
        Ok(SharedMem { ptr: ptr.cast(), len, path: path.to_path_buf(), owner, _file: file })
    }

    #[cfg(windows)]
    fn map(file: std::fs::File, path: &Path, len: usize, owner: bool) -> io::Result<SharedMem> {
        use makepad_platform::windows::Win32::Foundation::{CloseHandle, HANDLE};
        use makepad_platform::windows::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, FILE_MAP_ALL_ACCESS, PAGE_READWRITE};
        use std::os::windows::io::AsRawHandle;
        let fail = |e: io::Error| {
            if owner {
                let _ = std::fs::remove_file(path);
            }
            e
        };
        let mapping = unsafe { CreateFileMappingW(HANDLE(file.as_raw_handle() as _), None, PAGE_READWRITE, 0, 0, None) }
            .map_err(|e| fail(io::Error::from_raw_os_error(e.0 & 0xffff)))?;
        let view = match unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, len) } {
            Ok(view) if !view.0.is_null() => view,
            _ => {
                let error = io::Error::last_os_error();
                unsafe {
                    let _ = CloseHandle(mapping);
                }
                return Err(fail(error));
            }
        };
        Ok(SharedMem { ptr: view.0.cast(), len, path: path.to_path_buf(), owner, mapping, _file: file })
    }

    #[cfg(not(any(unix, windows)))]
    fn map(_file: std::fs::File, path: &Path, _len: usize, owner: bool) -> io::Result<SharedMem> {
        if owner {
            let _ = std::fs::remove_file(path);
        }
        Err(io::Error::new(io::ErrorKind::Unsupported, "shared memory is not available on this platform"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// The 64-bit atomic at byte `offset` (8-aligned, inside the mapping).
    pub fn atomic_u64(&self, offset: usize) -> &AtomicU64 {
        assert!(offset % 8 == 0 && offset + 8 <= self.len);
        unsafe { &*(self.ptr.add(offset) as *const AtomicU64) }
    }

    /// The 32-bit atomic at byte `offset` (4-aligned, inside the mapping).
    pub fn atomic_u32(&self, offset: usize) -> &std::sync::atomic::AtomicU32 {
        assert!(offset % 4 == 0 && offset + 4 <= self.len);
        unsafe { &*(self.ptr.add(offset) as *const std::sync::atomic::AtomicU32) }
    }

    /// Plain bytes at `offset`. Callers own the protocol that makes the
    /// access race-free (a slot is written only while its state says so).
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn bytes(&self, offset: usize, len: usize) -> &mut [u8] {
        assert!(offset + len <= self.len);
        unsafe { std::slice::from_raw_parts_mut(self.ptr.add(offset), len) }
    }
}

impl Drop for SharedMem {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            sys::munmap(self.ptr.cast(), self.len);
        }
        #[cfg(windows)]
        unsafe {
            use makepad_platform::windows::Win32::System::Memory::{UnmapViewOfFile, MEMORY_MAPPED_VIEW_ADDRESS};
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS(self.ptr.cast()));
            let _ = makepad_platform::windows::Win32::Foundation::CloseHandle(self.mapping);
        }
        if self.owner {
            // Windows: std opens files shareable for delete, so the still-open handle does not
            // keep the file
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// A named counting semaphore shared between processes.
pub struct Semaphore {
    #[cfg(unix)]
    sem: *mut std::os::raw::c_void,
    #[cfg(windows)]
    sem: makepad_platform::windows::Win32::Foundation::HANDLE,
    name: std::ffi::CString,
    /// Unix: this side made it and unlinks its name (a Windows name lives while any handle does).
    #[cfg(unix)]
    owner: bool,
}

unsafe impl Send for Semaphore {}
unsafe impl Sync for Semaphore {}

impl Semaphore {
    /// Make a new semaphore at zero. `name` starts with `/`; macOS allows
    /// 31 bytes.
    pub fn create(name: &str) -> io::Result<Semaphore> {
        Self::open_with(name, true)
    }

    /// Open the one another process made.
    pub fn open(name: &str) -> io::Result<Semaphore> {
        Self::open_with(name, false)
    }

    #[cfg(unix)]
    fn open_with(name: &str, create: bool) -> io::Result<Semaphore> {
        let cname = std::ffi::CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "semaphore name"))?;
        let sem = unsafe {
            if create {
                // A stale one of the same name (a crashed host) is replaced.
                sys::sem_unlink(cname.as_ptr());
                sys::sem_open(cname.as_ptr(), sys::O_CREAT, 0o600 as std::os::raw::c_uint, 0 as std::os::raw::c_uint)
            } else {
                sys::sem_open(cname.as_ptr(), 0)
            }
        };
        if sys::sem_failed(sem) {
            return Err(io::Error::last_os_error());
        }
        Ok(Semaphore { sem, name: cname, owner: create })
    }

    /// The kernel object name: `/mp...` becomes `Local\mp...` (this logon session).
    #[cfg(windows)]
    fn open_with(name: &str, create: bool) -> io::Result<Semaphore> {
        use makepad_platform::windows::core::PCWSTR;
        use makepad_platform::windows::Win32::System::Threading::{CreateSemaphoreW, OpenSemaphoreW, SEMAPHORE_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE};
        let cname = std::ffi::CString::new(name).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "semaphore name"))?;
        let wide: Vec<u16> = format!("Local\\{}", name.trim_start_matches('/')).encode_utf16().chain(std::iter::once(0)).collect();
        let sem = unsafe {
            if create {
                CreateSemaphoreW(None, 0, i32::MAX, Some(PCWSTR(wide.as_ptr())))
            } else {
                OpenSemaphoreW(SEMAPHORE_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(wide.as_ptr()))
            }
        }
        .map_err(|e| io::Error::from_raw_os_error(e.0 & 0xffff))?;
        Ok(Semaphore { sem, name: cname })
    }

    #[cfg(not(any(unix, windows)))]
    fn open_with(_name: &str, _create: bool) -> io::Result<Semaphore> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "shared semaphores are not available on this platform"))
    }

    pub fn name(&self) -> &str {
        self.name.to_str().unwrap_or("")
    }

    pub fn post(&self) {
        #[cfg(unix)]
        unsafe {
            sys::sem_post(self.sem);
        }
        #[cfg(windows)]
        unsafe {
            let _ = makepad_platform::windows::Win32::System::Threading::ReleaseSemaphore(self.sem, 1, None);
        }
    }

    /// Wait until posted (no timeout: the other side posts to end a wait).
    pub fn wait(&self) {
        #[cfg(unix)]
        loop {
            if unsafe { sys::sem_wait(self.sem) } == 0 {
                return;
            }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return;
            }
        }
        #[cfg(windows)]
        unsafe {
            makepad_platform::windows::Win32::System::Threading::WaitForSingleObject(self.sem, u32::MAX);
        }
    }
}

impl Drop for Semaphore {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            sys::sem_close(self.sem);
            if self.owner {
                sys::sem_unlink(self.name.as_ptr());
            }
        }
        // Windows: the name lives while any process holds a handle
        #[cfg(windows)]
        unsafe {
            let _ = makepad_platform::windows::Win32::Foundation::CloseHandle(self.sem);
        }
    }
}

/// A short name unique to this process for a new semaphore (`/mp-<pid>-<n><tag>`).
pub fn unique_name(tag: char) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("/mp{:x}-{:x}{tag}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Where this process puts the files of shared mappings.
pub fn file_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("makepad-shm{}", name.replace('/', "-")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_is_shared_and_removed() {
        let path = file_path(&unique_name('t'));
        let a = SharedMem::create(&path, 4096).unwrap();
        let b = SharedMem::open(&path, 4096).unwrap();
        a.atomic_u64(8).store(42, Ordering::Release);
        assert_eq!(b.atomic_u64(8).load(Ordering::Acquire), 42);
        drop(b);
        drop(a);
        assert!(!path.exists());
    }

    #[test]
    fn semaphore_wakes_a_waiter() {
        let name = unique_name('s');
        let a = Semaphore::create(&name).unwrap();
        let b = Semaphore::open(&name).unwrap();
        let waiter = std::thread::spawn(move || b.wait());
        a.post();
        waiter.join().unwrap();
    }
}
