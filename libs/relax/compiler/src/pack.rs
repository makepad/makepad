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

/// A pack file's bytes: mapped read-only (unix; pages are read as entries are linked, nothing is
/// copied at open), else read into memory.
pub struct Bytes {
    ptr: *const u8,
    len: usize,
    mapped: bool,
    owned: Vec<u8>,
}

unsafe impl Send for Bytes {}
unsafe impl Sync for Bytes {}

impl std::ops::Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        if self.mapped {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        } else {
            &self.owned
        }
    }
}

impl Drop for Bytes {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.mapped {
            extern "C" {
                fn munmap(addr: *mut u8, len: usize) -> i32;
            }
            forget_mapping(self.ptr as usize);
            unsafe { munmap(self.ptr as *mut u8, self.len) };
        }
    }
}

impl Bytes {
    pub fn from_vec(v: Vec<u8>) -> Bytes {
        Bytes { ptr: std::ptr::null(), len: v.len(), mapped: false, owned: v }
    }

    /// The file mapped read-only, shared: the page cache's pages (packs are immutable: written whole,
    /// then renamed; a store's committed payloads never change). Not MAP_PRIVATE: macOS sets up a
    /// private file mapping slowly (~1.9 ms each on an M4, ~0.002 ms shared), and read-only the two
    /// read the same bytes.
    pub fn map(path: &Path) -> Option<Bytes> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            extern "C" {
                fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
            }
            let f = std::fs::File::open(path).ok()?;
            let len = f.metadata().ok()?.len() as usize;
            if len == 0 {
                return None;
            }
            const PROT_READ: i32 = 1;
            const MAP_SHARED: i32 = 1;
            let p = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ, MAP_SHARED, f.as_raw_fd(), 0) };
            if p.is_null() || p as isize == -1 {
                return None;
            }
            note_mapping(p as usize, len, &f, 0);
            return Some(Bytes { ptr: p, len, mapped: true, owned: Vec::new() });
        }
        #[allow(unreachable_code)]
        Bytes::read_kept(path)
    }

    /// The file read into the kept heap: where it is not mapped (Windows), the bytes stand for a
    /// mapping, which outlives every compiler generation -- what is read in place from them (a
    /// startup image's pc maps, unwind trailers) is identity state of installed code. Read in
    /// the caller's phase (a warm start loads in the compiler phase) they were destroyed by the
    /// settle swap, and the O3 sampler's next pc lookup faulted on run-owner ("use after
    /// compiler-zone destroy (generation 0)", 0xC0000005, DESKTOP-5).
    fn read_kept(path: &Path) -> Option<Bytes> {
        crate::zone_alloc::kept_with(|| std::fs::read(path).ok().map(Bytes::from_vec))
    }
}

/// Every read-only cache file mapping of this process: (start, length).
#[cfg(unix)]
static MAPPINGS: relax_session::sync::Mutex<Vec<Mapping>> = relax_session::sync::Mutex::new(Vec::new());

/// One cache file mapping: where, and (macOS) what to map again over it: the file's path, its
/// identity (device, inode) and the offset mapped.
#[cfg(unix)]
struct Mapping {
    start: usize,
    len: usize,
    #[cfg(target_os = "macos")]
    again: Option<(std::path::PathBuf, u64, u64, u64)>,
}

#[cfg(unix)]
fn note_mapping(start: usize, len: usize, file: &std::fs::File, off: u64) {
    // kept heap: the list outlives every compiler generation
    let _k = crate::zone_alloc::kept();
    #[cfg(target_os = "macos")]
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
    #[cfg(not(target_os = "macos"))]
    let _ = (file, off);
    MAPPINGS.lock().unwrap_or_else(|e| e.into_inner()).push(Mapping {
        start,
        len,
        #[cfg(target_os = "macos")]
        again,
    });
}

#[cfg(unix)]
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
/// mapped before, checked by device and inode). Returns bytes released.
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
    #[cfg(target_os = "macos")]
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
    #[allow(unreachable_code)]
    0
}

/// `len` bytes of `file` at `off`, mapped read-only for the rest of the process (unix: the
/// page cache's pages, nothing copied; elsewhere a copy). The file is never truncated below
/// `off + len` while this process runs (store.rs RunStore: append-only).
#[cfg(unix)]
pub fn map_pinned(file: &std::fs::File, off: u64, len: usize) -> Option<&'static [u8]> {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn getpagesize() -> i32;
    }
    if len == 0 {
        return Some(&[]);
    }
    let page = unsafe { getpagesize() }.max(4096) as u64;
    let base = off & !(page - 1);
    let span = (off - base) as usize + len;
    const PROT_READ: i32 = 1;
    // shared, not private: a private file mapping costs ~1.9 ms to set up on macOS (an M4, while
    // the file is being appended to), and a cold Stage maps ~3.5k images on the load thread (6.5 s);
    // read-only, both read the page cache's bytes, and an appended payload never changes
    const MAP_SHARED: i32 = 1;
    let p = unsafe { mmap(std::ptr::null_mut(), span, PROT_READ, MAP_SHARED, file.as_raw_fd(), base as i64) };
    if p.is_null() || p as isize == -1 {
        return None;
    }
    note_mapping(p as usize, span, file, base);
    Some(unsafe { std::slice::from_raw_parts(p.add((off - base) as usize), len) })
}

#[cfg(not(unix))]
pub fn map_pinned(file: &std::fs::File, off: u64, len: usize) -> Option<&'static [u8]> {
    use std::os::windows::fs::FileExt;
    // the copy stands for a mapping kept for the rest of the process: the kept heap, never the
    // compiler generation the settle swap destroys (Bytes::read_kept)
    let _k = crate::zone_alloc::kept();
    // 8-byte aligned, as a store payload is
    let mut words = vec![0u64; len.div_ceil(8)];
    let buf = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, len) };
    let mut at = 0;
    while at < len {
        let n = file.seek_read(&mut buf[at..], off + at as u64).ok()?;
        if n == 0 { return None; }
        at += n;
    }
    let words = Box::leak(words.into_boxed_slice());
    Some(unsafe { std::slice::from_raw_parts(words.as_ptr() as *const u8, len) })
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

#[cfg(test)]
mod tests {
    /// A cache file read in place of a mapping, from the compiler phase (a warm start's load):
    /// its bytes are in the kept heap, not in the compiler generation the settle swap destroys.
    #[test]
    fn unmapped_cache_bytes_outlive_the_compiler_generation() {
        let path = std::env::temp_dir().join(format!("relax-pack-kept-{}", std::process::id()));
        std::fs::write(&path, vec![7u8; 4096]).unwrap();
        let _c = crate::zone_alloc::compiler();
        // the phase routes: a plain read now lands in the compiler arena
        let plain = std::fs::read(&path).unwrap();
        assert!(crate::zone_alloc::compiler_owns(plain.as_ptr()), "the compiler phase does not route to the arena here");
        let read = super::Bytes::read_kept(&path).unwrap();
        assert_eq!(read.len(), 4096);
        assert!(!crate::zone_alloc::compiler_owns(read.as_ptr()), "read into the compiler arena");
        #[cfg(not(unix))]
        {
            let file = std::fs::File::open(&path).unwrap();
            let pinned = super::map_pinned(&file, 8, 1024).unwrap();
            assert!(!crate::zone_alloc::compiler_owns(pinned.as_ptr()), "pinned copy in the compiler arena");
        }
        std::fs::remove_file(&path).unwrap();
    }

    /// The settle's release of the cache mappings' resident pages: the process's resident size
    /// falls by about the mapped size and the same bytes are read again from the file.
    #[cfg(unix)]
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
