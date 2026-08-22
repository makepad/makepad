//! Read-only memory-mapped file region for weight data.
//!
//! Weights served from a file-backed `PROT_READ` mapping stay CLEAN pages
//! (evictable, re-faultable from disk) instead of jetsam-dirty malloc
//! memory — the llama.cpp iOS pattern. This is the ONLY platform-gated
//! module: everything else treats the region as an opaque `&[u8]`, and on
//! non-unix targets `map_file` simply fails so callers fall back to the
//! owned-arena path.

use std::path::Path;

#[cfg(unix)]
mod imp {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    const O_RDONLY: c_int = 0;
    const SEEK_END: c_int = 2;
    const PROT_READ: c_int = 1;
    const MAP_SHARED: c_int = 1;
    /// macOS: issue an async advisory read with no copy to user space.
    #[cfg(target_os = "macos")]
    const F_RDADVISE: c_int = 44;

    /// macOS `struct radvisory` — the argument to `fcntl(F_RDADVISE)`.
    #[cfg(target_os = "macos")]
    #[repr(C)]
    struct Radvisory {
        ra_offset: i64,
        ra_count: c_int,
    }

    extern "C" {
        fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        fn close(fd: c_int) -> c_int;
        fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64;
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            offset: i64,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> c_int;
        fn getpagesize() -> c_int;
        fn mincore(addr: *const c_void, len: usize, vec: *mut c_char) -> c_int;
        #[cfg(target_os = "macos")]
        fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    }

    /// Read-only mapping of a whole file. The mapped length is the file
    /// size rounded UP to a page multiple (the tail stays within the
    /// file's zero-filled last partial page), so the base pointer and
    /// length both satisfy page granularity — required by the Metal
    /// no-copy buffer path.
    pub struct MappedRegion {
        ptr: *mut c_void,
        len: usize,
        /// Kept open for the lifetime of the mapping so `prefault` can ask
        /// the kernel to read ahead into the page cache. Costs one fd.
        fd: c_int,
    }

    // Safety: the mapping is PROT_READ and never written or remapped;
    // sharing immutable bytes across threads is safe.
    unsafe impl Send for MappedRegion {}
    unsafe impl Sync for MappedRegion {}

    impl MappedRegion {
        pub fn map_file(path: &Path) -> Result<Self, String> {
            let page = unsafe { getpagesize() };
            if page <= 0 {
                return Err("mmap: getpagesize failed".to_string());
            }
            let page = page as usize;

            let c_path = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| format!("mmap: path {:?} contains a NUL byte", path))?;
            let fd = unsafe { open(c_path.as_ptr(), O_RDONLY) };
            if fd < 0 {
                return Err(format!("mmap: failed to open {:?}", path));
            }

            let file_size = unsafe { lseek(fd, 0, SEEK_END) };
            if file_size <= 0 {
                unsafe { close(fd) };
                return Err(format!("mmap: failed to size {:?}", path));
            }
            let Some(len) = usize::try_from(file_size)
                .ok()
                .and_then(|size| size.checked_next_multiple_of(page))
            else {
                unsafe { close(fd) };
                return Err(format!("mmap: file size {} overflows usize", file_size));
            };

            let ptr = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ, MAP_SHARED, fd, 0) };
            if ptr.is_null() || ptr as isize == -1 {
                unsafe { close(fd) };
                return Err(format!("mmap: mapping {:?} ({} bytes) failed", path, len));
            }
            // The mapping itself no longer needs the fd, but `prefault`
            // does, so it is held until Drop instead of closed here.
            Ok(Self { ptr, len, fd })
        }

        /// Pages of this mapping currently resident in the unified page
        /// cache, as `(resident, total)`. Straight from `mincore(2)` — the
        /// only honest way to tell a warm mapping from a cold one, since a
        /// cold mapping is indistinguishable from a warm one until it is
        /// touched.
        pub fn residency(&self) -> (usize, usize) {
            let page = unsafe { getpagesize() }.max(1) as usize;
            let pages = self.len / page;
            let mut vec = vec![0i8; pages];
            let rc =
                unsafe { mincore(self.ptr, self.len, vec.as_mut_ptr() as *mut c_char) };
            if rc != 0 {
                return (0, pages);
            }
            (vec.iter().filter(|b| **b & 1 != 0).count(), pages)
        }

        /// Force the whole mapping resident before anything reads it.
        ///
        /// Demand-paging a cold mapping is the slowest way to get weights
        /// off an Apple SSD: every 16 KB page is a synchronous fault, which
        /// measures ~0.8 GB/s against ~6.3 GB/s for the same file read
        /// sequentially. `F_RDADVISE` fixes that — it queues an async
        /// advisory read that populates the page cache with no copy to user
        /// space — so this walks the file hinting a window ahead of the
        /// cursor it is touching, and the faults land on pages that are
        /// already there. Measured 6.2 GB/s, i.e. device speed, while
        /// leaving a fully populated file-backed mapping (which is what the
        /// Metal no-copy buffer needs).
        ///
        /// Cheap to call on an already-warm mapping (~26 GB/s of soft
        /// faults), and a no-op beyond that.
        #[cfg(target_os = "macos")]
        pub fn prefault(&self) {
            const WINDOW: usize = 16 << 20;
            const AHEAD: usize = 4 * WINDOW;
            let page = unsafe { getpagesize() }.max(1) as usize;
            let mut hinted = 0usize;
            let mut touched = 0usize;
            while touched < self.len {
                // Keep the device busy ahead of where we are reading.
                while hinted < self.len && hinted < touched + AHEAD {
                    let count = WINDOW.min(self.len - hinted);
                    let ra = Radvisory {
                        ra_offset: hinted as i64,
                        ra_count: count as c_int,
                    };
                    unsafe { fcntl(self.fd, F_RDADVISE, &ra as *const Radvisory) };
                    hinted += count;
                }
                let end = (touched + WINDOW).min(self.len);
                while touched < end {
                    // Safety: `touched` stays inside the live mapping.
                    unsafe {
                        std::ptr::read_volatile((self.ptr as *const u8).add(touched));
                    }
                    touched += page;
                }
            }
        }

        /// Non-macOS unix: no `F_RDADVISE`, so just fault the pages in.
        #[cfg(not(target_os = "macos"))]
        pub fn prefault(&self) {
            let page = unsafe { getpagesize() }.max(1) as usize;
            let mut off = 0usize;
            while off < self.len {
                // Safety: `off` stays inside the live mapping.
                unsafe {
                    std::ptr::read_volatile((self.ptr as *const u8).add(off));
                }
                off += page;
            }
        }

        pub fn len(&self) -> usize {
            self.len
        }

        pub fn is_empty(&self) -> bool {
            self.len == 0
        }

        pub fn as_slice(&self) -> &[u8] {
            // Safety: ptr/len describe a live PROT_READ mapping owned by self.
            unsafe { std::slice::from_raw_parts(self.ptr as *const u8, self.len) }
        }
    }

    impl Drop for MappedRegion {
        fn drop(&mut self) {
            unsafe {
                munmap(self.ptr, self.len);
                close(self.fd);
            }
        }
    }

    impl std::fmt::Debug for MappedRegion {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MappedRegion")
                .field("ptr", &self.ptr)
                .field("len", &self.len)
                .finish()
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use std::path::Path;

    /// Stub: mapping is unavailable, so `map_file` always fails and
    /// callers take the owned-arena path.
    #[derive(Debug)]
    pub struct MappedRegion {
        _private: (),
    }

    impl MappedRegion {
        pub fn map_file(path: &Path) -> Result<Self, String> {
            Err(format!(
                "mmap is unavailable on this platform (cannot map {:?})",
                path
            ))
        }

        pub fn len(&self) -> usize {
            0
        }

        pub fn is_empty(&self) -> bool {
            true
        }

        pub fn as_slice(&self) -> &[u8] {
            &[]
        }

        pub fn residency(&self) -> (usize, usize) {
            (0, 0)
        }

        pub fn prefault(&self) {}
    }
}

pub use imp::MappedRegion;

impl MappedRegion {
    /// Convenience wrapper taking anything path-like.
    pub fn map(path: impl AsRef<Path>) -> Result<Self, String> {
        Self::map_file(path.as_ref())
    }
}
