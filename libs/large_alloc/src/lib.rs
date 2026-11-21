//! Large blocks straight from the OS, for a binary's global allocator.
//!
//! On macOS the system allocator keeps a freed large block mapped and dirty. The block leaves the
//! allocator's own statistics but stays in the process's footprint, `malloc_zone_pressure_relief`
//! gives none of it back, and only the system's memory pressure drains it, by compressing it. A
//! process that makes and frees blocks of hundreds of MiB (a solver's matrices, a decoded image)
//! then holds GiBs it no longer uses. [`LargeDirect`] maps each block of [`THRESHOLD`] or more
//! straight from the OS and unmaps it when it is freed, as glibc and the Windows heap already do
//! with large blocks. Everything smaller goes to the allocator it wraps, which on macOS keeps
//! freed blocks of 64 KiB to 1 MiB in the footprint too (measured): below the threshold nothing
//! changes.
//!
//! A binary opts in with its own global allocator, around the one it had:
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: makepad_large_alloc::LargeDirect<std::alloc::System> =
//!     makepad_large_alloc::LargeDirect(std::alloc::System);
//! ```
//!
//! A counting allocator puts it under its count instead (`LargeDirect(System)` where it called
//! `System`), so that the count still sees every block.
//!
//! Off macOS it is the allocator it wraps: each call goes straight there and nothing compares.
//!
//! The rule it adds: a block of [`THRESHOLD`] or more that Rust allocated is freed by Rust
//! (`dealloc`), never by C's `free` or `realloc`, and a block C allocated is never freed by Rust.
//!
//! Cost on macOS: each allocation, free and reallocation compares its size and its alignment
//! with two constants. A large block adds one `mmap`, one `munmap` and a few relaxed atomic adds,
//! and its pages are made (zero) where it is first written.

use std::alloc::{GlobalAlloc, Layout};

/// Blocks of this many bytes or more are mapped straight from the OS (macOS).
pub const THRESHOLD: usize = 4 << 20;

/// The allocator `A`, with blocks of [`THRESHOLD`] or more mapped straight from the OS on macOS.
pub struct LargeDirect<A>(pub A);

/// What the large path did since the process started (all zero off macOS).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// bytes mapped now (each block rounded up to 16 KiB)
    pub live: u64,
    /// the most bytes mapped at once
    pub peak: u64,
    /// blocks mapped
    pub maps: u64,
    /// blocks unmapped
    pub unmaps: u64,
    /// bytes mapped in total (each block rounded up to 16 KiB)
    pub mapped_total: u64,
    /// bytes `realloc` copied out of or into a mapped block
    pub realloc_copied: u64,
    /// the most bytes one `realloc` copied
    pub realloc_copy_max: u64,
}

/// The large path's counters now.
pub fn stats() -> Stats {
    #[cfg(target_os = "macos")]
    {
        direct::stats()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Stats::default()
    }
}

#[cfg(target_os = "macos")]
mod direct {
    use super::{Stats, THRESHOLD};
    use std::alloc::Layout;
    use std::ffi::{c_int, c_void};
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    extern "C" {
        fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, offset: i64) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> c_int;
    }

    /// PROT_READ | PROT_WRITE
    const PROT: c_int = 0x1 | 0x2;
    /// MAP_PRIVATE | MAP_ANON
    const FLAGS: c_int = 0x0002 | 0x1000;
    /// VM_MAKE_TAG(240), passed as an anonymous mapping's fd: the first tag for applications, by
    /// which the VM tools name these bytes
    const TAG: c_int = 240 << 24;
    /// the alignment a new mapping has: the target's page
    #[cfg(target_arch = "aarch64")]
    const PAGE: usize = 16 << 10;
    #[cfg(not(target_arch = "aarch64"))]
    const PAGE: usize = 4 << 10;
    /// the length a block maps rounds up to, on every target
    const ROUND: usize = 16 << 10;

    static LIVE: AtomicU64 = AtomicU64::new(0);
    static PEAK: AtomicU64 = AtomicU64::new(0);
    static MAPS: AtomicU64 = AtomicU64::new(0);
    static UNMAPS: AtomicU64 = AtomicU64::new(0);
    static MAPPED: AtomicU64 = AtomicU64::new(0);
    static COPIED: AtomicU64 = AtomicU64::new(0);
    static COPY_MAX: AtomicU64 = AtomicU64::new(0);

    #[inline]
    pub fn large(l: Layout) -> bool {
        l.size() >= THRESHOLD && l.align() <= PAGE
    }

    /// A layout's size is at most `isize::MAX`, so this does not overflow.
    #[inline]
    pub fn round(n: usize) -> usize {
        (n + (ROUND - 1)) & !(ROUND - 1)
    }

    /// A new zero block of `size` bytes, or null when the OS gives none.
    pub unsafe fn map(size: usize) -> *mut u8 {
        let len = round(size);
        let p = unsafe { mmap(std::ptr::null_mut(), len, PROT, FLAGS, TAG, 0) };
        if p as isize == -1 {
            return std::ptr::null_mut();
        }
        MAPS.fetch_add(1, Relaxed);
        MAPPED.fetch_add(len as u64, Relaxed);
        let live = LIVE.fetch_add(len as u64, Relaxed) + len as u64;
        PEAK.fetch_max(live, Relaxed);
        p as *mut u8
    }

    /// The block `map(size)` gave.
    pub unsafe fn unmap(p: *mut u8, size: usize) {
        let len = round(size);
        unsafe { munmap(p as *mut c_void, len) };
        UNMAPS.fetch_add(1, Relaxed);
        LIVE.fetch_sub(len as u64, Relaxed);
    }

    /// A mapped block of `have` bytes (rounded) keeps its first `keep` (rounded, smaller).
    pub unsafe fn unmap_tail(p: *mut u8, have: usize, keep: usize) {
        unsafe { munmap(p.add(keep) as *mut c_void, have - keep) };
        LIVE.fetch_sub((have - keep) as u64, Relaxed);
    }

    pub fn copied(n: usize) {
        COPIED.fetch_add(n as u64, Relaxed);
        COPY_MAX.fetch_max(n as u64, Relaxed);
    }

    pub fn stats() -> Stats {
        Stats {
            live: LIVE.load(Relaxed),
            peak: PEAK.load(Relaxed),
            maps: MAPS.load(Relaxed),
            unmaps: UNMAPS.load(Relaxed),
            mapped_total: MAPPED.load(Relaxed),
            realloc_copied: COPIED.load(Relaxed),
            realloc_copy_max: COPY_MAX.load(Relaxed),
        }
    }
}

#[cfg(target_os = "macos")]
unsafe impl<A: GlobalAlloc> GlobalAlloc for LargeDirect<A> {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if direct::large(l) {
            return unsafe { direct::map(l.size()) };
        }
        unsafe { self.0.alloc(l) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if direct::large(l) {
            // a new mapping is zero, and makes its pages only where it is written
            return unsafe { direct::map(l.size()) };
        }
        unsafe { self.0.alloc_zeroed(l) }
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if direct::large(l) {
            return unsafe { direct::unmap(p, l.size()) };
        }
        unsafe { self.0.dealloc(p, l) }
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        // SAFETY: the caller promises `new` rounded up to `l.align()` fits an `isize`
        let to = unsafe { Layout::from_size_align_unchecked(new, l.align()) };
        let (was, will) = (direct::large(l), direct::large(to));
        if !was && !will {
            return unsafe { self.0.realloc(p, l, new) };
        }
        if was && will {
            let (have, want) = (direct::round(l.size()), direct::round(new));
            if want <= have {
                // the same mapping; a smaller one gives its tail back
                if want < have {
                    unsafe { direct::unmap_tail(p, have, want) };
                }
                return p;
            }
        }
        let q = unsafe { self.alloc(to) };
        if !q.is_null() {
            let n = l.size().min(new);
            unsafe { std::ptr::copy_nonoverlapping(p, q, n) };
            direct::copied(n);
            unsafe { self.dealloc(p, l) };
        }
        q
    }
}

#[cfg(not(target_os = "macos"))]
unsafe impl<A: GlobalAlloc> GlobalAlloc for LargeDirect<A> {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { self.0.alloc(l) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        unsafe { self.0.alloc_zeroed(l) }
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { self.0.dealloc(p, l) }
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        unsafe { self.0.realloc(p, l, new) }
    }
}
