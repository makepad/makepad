//! Disk cache file helpers shared by the store (store.rs): byte buffers that map a file
//! read-only, the pending-entry type, atomic whole-file writes and the private-directory check of the cache
//! folder (dirs.rs: `<target>/relax/cache`).

use std::io::Write;
use std::path::{Path, PathBuf};

/// An entry to write: its key, kind (code, data, ...) and payload.
pub struct NewEntry {
    pub key: u128,
    pub kind: u32,
    pub payload: Vec<u8>,
}

/// A file's bytes, mapped read-only: pages are read as entries are linked, nothing is copied at open.
pub struct Bytes {
    ptr: *const u8,
    len: usize,
}

unsafe impl Send for Bytes {}
unsafe impl Sync for Bytes {}

impl std::ops::Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for Bytes {
    fn drop(&mut self) {
        forget_mapping(self.ptr as usize);
        unmap_view(self.ptr, self.len);
    }
}

impl Bytes {
    /// The file mapped read-only, shared: the page cache's pages (packs are immutable: written whole,
    /// then renamed; a store's committed payloads never change).
    pub fn map(path: &Path) -> Option<Bytes> {
        let f = std::fs::File::open(path).ok()?;
        let len = f.metadata().ok()?.len() as usize;
        if len == 0 {
            return None;
        }
        let ptr = map_view(&f, 0, len)?;
        note_mapping(ptr as usize, len, &f, 0);
        Some(Bytes { ptr, len })
    }
}

/// `len` bytes of `file` at `off` (a multiple of `view_align`), mapped read-only until `unmap_view`.
/// Shared, not private: a private file mapping costs ~1.9 ms to set up on macOS (an M4, while the
/// file is being appended to), and a large app's cold start maps ~3.5k images on the load thread (6.5 s);
/// read-only, both read the page cache's bytes, and an appended payload never changes.
#[cfg(unix)]
fn map_view(file: &std::fs::File, off: u64, len: usize) -> Option<*const u8> {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    }
    const PROT_READ: i32 = 1;
    const MAP_SHARED: i32 = 1;
    let p = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ, MAP_SHARED, file.as_raw_fd(), off as i64) };
    (!p.is_null() && p as isize != -1).then_some(p as *const u8)
}

#[cfg(unix)]
fn unmap_view(p: *const u8, len: usize) {
    extern "C" {
        fn munmap(addr: *mut u8, len: usize) -> i32;
    }
    unsafe { munmap(p as *mut u8, len) };
}

#[cfg(unix)]
fn view_align() -> u64 {
    extern "C" {
        fn getpagesize() -> i32;
    }
    unsafe { getpagesize() }.max(4096) as u64
}

#[cfg(windows)]
fn map_view(file: &std::fs::File, off: u64, len: usize) -> Option<*const u8> {
    use makepad_windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use makepad_windows_sys::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, FILE_MAP_READ, PAGE_READONLY};
    use std::os::windows::io::AsRawHandle;
    // size 0: the section ends where the file ends now, and `off + len` is written by now. The view
    // keeps the section and the section keeps the file, so neither handle stays open.
    let section = unsafe { CreateFileMappingW(HANDLE(file.as_raw_handle() as _), None, PAGE_READONLY, 0, 0, None) }.ok()?;
    let view = unsafe { MapViewOfFile(section, FILE_MAP_READ, (off >> 32) as u32, off as u32, len) };
    let _ = unsafe { CloseHandle(section) };
    Some(view.ok()?.0 as *const u8)
}

#[cfg(windows)]
fn unmap_view(p: *const u8, _len: usize) {
    use makepad_windows_sys::Win32::System::Memory::{UnmapViewOfFile, MEMORY_MAPPED_VIEW_ADDRESS};
    let _ = unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS(p as _)) };
}

/// A view starts on the allocation granularity (64 KB), not on a page.
#[cfg(windows)]
fn view_align() -> u64 {
    use makepad_windows_sys::Win32::System::SystemInformation::{GetNativeSystemInfo, SYSTEM_INFO};
    let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    unsafe { GetNativeSystemInfo(&mut info) };
    info.dwAllocationGranularity as u64
}

/// Takes `b`'s pages out of this process's working set. They stay file pages: a later read faults
/// them back without touching the disk. Unlocking pages that are not locked is how Windows does
/// this; it answers "not locked", the ordinary outcome here.
#[cfg(windows)]
pub(crate) fn release_pages(b: &[u8]) {
    use makepad_windows_sys::Win32::System::Memory::VirtualUnlock;
    if !b.is_empty() {
        let _ = unsafe { VirtualUnlock(b.as_ptr() as *const std::ffi::c_void, b.len()) };
    }
}

/// Every read-only cache file mapping of this process: (start, length).
static MAPPINGS: relax_session::sync::Mutex<Vec<Mapping>> = relax_session::sync::Mutex::new(Vec::new());

/// One cache file mapping: where, and (macOS) what to map again over it: the file's path, its
/// identity (device, inode) and the offset mapped.
struct Mapping {
    start: usize,
    len: usize,
    #[cfg(target_vendor = "apple")]
    again: Option<(std::path::PathBuf, u64, u64, u64)>,
}

fn note_mapping(start: usize, len: usize, file: &std::fs::File, off: u64) {
    // kept heap: the list outlives every compiler generation
    let _k = crate::zone_alloc::kept();
    #[cfg(target_vendor = "apple")]
    let again = {
        use std::os::unix::fs::MetadataExt;
        extern "C" {
            fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        }
        use std::os::unix::io::AsRawFd;
        const F_GETPATH: i32 = 50;
        let mut buf = [0u8; 1024];
        let ok = unsafe { fcntl(file.as_raw_fd(), F_GETPATH, buf.as_mut_ptr()) } == 0;
        let n = buf.iter().position(|b| *b == 0).unwrap_or(0);
        match (ok, file.metadata()) {
            (true, Ok(m)) if n > 0 => Some((std::path::PathBuf::from(String::from_utf8_lossy(&buf[..n]).into_owned()), m.dev(), m.ino(), off)),
            _ => None,
        }
    };
    #[cfg(not(target_vendor = "apple"))]
    let _ = (file, off);
    MAPPINGS.lock().unwrap_or_else(|e| e.into_inner()).push(Mapping {
        start,
        len,
        #[cfg(target_vendor = "apple")]
        again,
    });
}

fn forget_mapping(start: usize) {
    let mut m = MAPPINGS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = m.iter().position(|x| x.start == start) {
        m.swap_remove(i);
    }
}

/// Drops this process's resident pages of the cache file mappings (settle). Opening a warm
/// cache touches pages all over the images (headers, indexes, and the kernel's fault-around
/// maps up to 16 neighbouring page-cache pages per fault), and they stay counted in RSS/PSS for
/// the rest of the run though few are read again. The mappings are read-only file pages, so
/// nothing is lost: a later read faults the page back from the page cache (a minor fault).
/// Linux/Android: madvise(DONTNEED). macOS: that only deactivates the pages, so the same file is
/// mapped again over the range (MAP_FIXED replaces the mapping atomically; the file is the one
/// mapped before, checked by device and inode). Windows: release_pages. Returns bytes released.
pub fn release_resident() -> usize {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        extern "C" {
            fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
        }
        const MADV_DONTNEED: i32 = 4;
        // the lock is held across the calls: a mapping cannot be unmapped (and its range reused) meanwhile
        let m = MAPPINGS.lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0;
        for x in m.iter() {
            if unsafe { madvise(x.start as *mut u8, x.len, MADV_DONTNEED) } == 0 {
                n += x.len;
            }
        }
        return n;
    }
    #[cfg(target_vendor = "apple")]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::io::AsRawFd;
        extern "C" {
            fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        }
        const PROT_READ: i32 = 1;
        const MAP_SHARED: i32 = 1;
        const MAP_FIXED: i32 = 0x10;
        let m = MAPPINGS.lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0;
        for x in m.iter() {
            let Some((path, dev, ino, off)) = &x.again else { continue };
            let Ok(f) = std::fs::File::open(path) else { continue };
            let Ok(md) = f.metadata() else { continue };
            if md.dev() != *dev || md.ino() != *ino || md.len() < *off + x.len as u64 {
                continue;
            }
            let p = unsafe { mmap(x.start as *mut u8, x.len, PROT_READ, MAP_SHARED | MAP_FIXED, f.as_raw_fd(), *off as i64) };
            if p as usize == x.start {
                n += x.len;
            }
        }
        return n;
    }
    #[cfg(windows)]
    {
        let m = MAPPINGS.lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0;
        for x in m.iter() {
            release_pages(unsafe { std::slice::from_raw_parts(x.start as *const u8, x.len) });
            n += x.len;
        }
        return n;
    }
    #[allow(unreachable_code)]
    0
}

/// `len` bytes of `file` at `off`, mapped read-only (the page cache's pages, nothing copied) for as
/// long as the `Bytes` lives: the caller keeps it past every reader of the slice. The file is never
/// truncated below `off + len` while it is mapped (store.rs RunStore: append-only).
pub fn map_range(file: &std::fs::File, off: u64, len: usize) -> Option<(Option<Bytes>, &'static [u8])> {
    if len == 0 {
        return Some((None, &[]));
    }
    let align = view_align();
    let base = off & !(align - 1);
    let span = (off - base) as usize + len;
    let p = map_view(file, base, span)?;
    note_mapping(p as usize, span, file, base);
    let bytes = unsafe { std::slice::from_raw_parts(p.add((off - base) as usize), len) };
    Some((Some(Bytes { ptr: p, len: span }), bytes))
}

/// Writes `image` to `dir/name` atomically: a fresh temp file (O_EXCL, 0600) in the same
/// directory, then rename. A crash leaves at most a temp file, never a partial pack.
pub fn write_atomic(dir: &Path, name: &str, image: &[u8]) -> std::io::Result<PathBuf> {
    write_atomic_with(dir, name, |f| f.write_all(image))
}

/// Build an atomic file without assembling its contents in memory.
pub fn write_atomic_with(dir: &Path, name: &str, write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>) -> std::io::Result<PathBuf> {
    let tmp = dir.join(format!(".tmp-{}-{}-{}", std::process::id(), name, nonce()));
    let mut f = crate::os::proc::create_private(&tmp)?;
    let r = write(&mut f);
    drop(f);
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let dst = dir.join(name);
    if let Err(e) = std::fs::rename(&tmp, &dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(dst)
}

pub(crate) fn nonce() -> u64 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut h = crate::sip::Sip128::new();
    h.u64(t);
    h.u64(std::process::id() as u64);
    h.u64(&t as *const u64 as u64);
    h.finish128() as u64
}

/// Creates `dir` (and parents) as 0700 and checks it is safe to load code from: owned by this
/// user, not writable by group or others. Err(reason) turns the cache off.
pub fn ensure_private_dir(dir: &Path) -> Result<(), String> {
    crate::os::proc::ensure_private_dir(dir)
}

/// A run's run dir made private before the run writes into it: created owner-only when missing
/// (with the mode at its mkdir; on Windows with the owner-only DACL at its creation), checked when
/// present. Err for one that belongs to another user, which a run never uses: its `exe/`,
/// `native/` and `build/` hold what that user could swap (the run's own executable, a library it
/// loads, a build script's output). A dir of this user's that others can write only turns the
/// cache off (cache.rs says so).
pub fn own_run_dir(dir: &Path) -> Result<(), String> {
    match ensure_private_dir(dir) {
        Err(e) if e.ends_with(crate::os::proc::OTHER_USERS) => Err(format!("{e}: a run never uses another user's run dir (set CARGO_TARGET_DIR to a folder of your own)")),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    /// A run dir is created owner-only at its mkdir, with the folders above it it creates (one
    /// that create_dir_all made group-writable, under umask 002, turned the cache off for good),
    /// and is the run's; another user's (`/` is root's) is refused, naming why.
    #[cfg(unix)]
    #[test]
    fn a_run_dir_is_made_owner_only_and_another_users_is_refused() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let base = std::env::temp_dir().join(format!("relax-run-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("target").join("relax");
        super::own_run_dir(&dir).unwrap();
        for d in [&dir, &base.join("target")] {
            let mode = std::fs::metadata(d).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{} was made {mode:o}, not owner-only", d.display());
        }
        super::own_run_dir(&dir).unwrap();
        let ours = std::fs::metadata(&dir).unwrap().uid();
        let _ = std::fs::remove_dir_all(&base);
        if std::fs::metadata("/").unwrap().uid() != ours {
            let e = super::own_run_dir(std::path::Path::new("/")).unwrap_err();
            assert!(e.contains("belongs to another user") && e.contains("never uses"), "another user's run dir was taken: {e}");
        }
    }
    /// The settle's release of the cache mappings' resident pages: the process's resident size
    /// falls by about the mapped size and the same bytes are read again from the file.
    #[test]
    fn release_resident_drops_the_pages_and_keeps_the_bytes() {
        let path = std::env::temp_dir().join(format!("relax-pack-release-{}", std::process::id()));
        let len = 192 << 20;
        let data: Vec<u8> = (0..len).map(|i| (i * 31 + i / 4096) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let map = super::Bytes::map(&path).unwrap();
        // touch every page
        let sum = |b: &[u8]| b.iter().step_by(4096).fold(0u64, |a, x| a + *x as u64);
        let before = sum(&map);
        let rss0 = crate::memstat::resident() as usize;
        assert!(super::release_resident() >= len);
        let rss1 = crate::memstat::resident() as usize;
        assert!(rss0 >= rss1 + len / 2, "resident {} MB -> {} MB", rss0 >> 20, rss1 >> 20);
        assert_eq!(&map[..], &data[..]);
        assert_eq!(sum(&map), before);
        drop(map);
        std::fs::remove_file(&path).unwrap();
    }
}
