//! The calling thread's secondary stack: one large address reservation, used as a bump region by
//! frames (lib.rs). Pages are committed as they are touched (Linux: MAP_NORESERVE; macOS: lazy
//! anonymous memory; Windows: committed in steps as `limit` rises). A reservation that cannot be
//! made leaves the region empty: every allocation then takes the heap fallback.
//!
//! Under `makepad run` (`cfg(relax)`) the region is the JIT thread's, owned by the runtime (the
//! compiler's own frame-scoped allocations share it); a rustc build keeps one per thread here.

/// The region's state. Shared layout with the runtime (`__relax_sstack`), so the JIT's own
/// frame-scoped allocations and this crate's frames nest on the same stack.
#[repr(C)]
pub struct RawRegion {
    /// next free byte
    pub top: usize,
    /// end of the fast path: an allocation past it asks `grow`
    pub limit: usize,
    /// the innermost open frame's id (0: none)
    pub depth: usize,
    /// the reservation [base, end)
    pub base: usize,
    pub end: usize,
}

/// `limit` after a trim: the part of the region kept committed between uses
pub(crate) const KEEP: usize = 1 << 20;
/// `limit` steps (Windows commits this much at a time)
const STEP: usize = 1 << 20;

#[cfg(relax)]
mod imp {
    use super::RawRegion;
    extern "C" {
        fn __relax_sstack() -> *mut RawRegion;
        fn __relax_sstack_grow(r: *mut RawRegion, need: usize) -> u32;
        fn __relax_sstack_trim(r: *mut RawRegion);
    }
    /// The JIT thread's region (null: none, every frame uses the heap).
    #[inline]
    pub fn region() -> *mut RawRegion {
        unsafe { __relax_sstack() }
    }
    pub fn grow(r: *mut RawRegion, need: usize) -> bool {
        unsafe { __relax_sstack_grow(r, need) != 0 }
    }
    pub fn trim(r: *mut RawRegion) {
        unsafe { __relax_sstack_trim(r) }
    }
}

#[cfg(not(relax))]
mod imp {
    use super::*;
    use std::cell::UnsafeCell;

    /// Reservation per thread (halved until the OS grants one)
    #[cfg(target_pointer_width = "64")]
    const RESERVE: usize = 8 << 30;
    #[cfg(not(target_pointer_width = "64"))]
    const RESERVE: usize = 16 << 20;
    const MIN_RESERVE: usize = 1 << 20;

    struct Owned(UnsafeCell<RawRegion>);
    impl Drop for Owned {
        fn drop(&mut self) {
            let r = self.0.get_mut();
            if r.end > r.base {
                os::release(r.base, r.end - r.base);
            }
            *r = RawRegion { top: 0, limit: 0, depth: 0, base: 0, end: 0 };
        }
    }

    thread_local! {
        static REGION: Owned = Owned(UnsafeCell::new(new_region()));
    }

    fn new_region() -> RawRegion {
        let mut len = RESERVE;
        while len >= MIN_RESERVE {
            if let Some(base) = os::reserve(len) {
                let limit = os::commit(base, base, (base + KEEP).min(base + len)).unwrap_or(base);
                return RawRegion { top: base, limit, depth: 0, base, end: base + len };
            }
            len /= 2;
        }
        RawRegion { top: 0, limit: 0, depth: 0, base: 0, end: 0 }
    }

    /// The thread's region; null once its destructor ran (every frame then uses the heap).
    #[inline]
    pub fn region() -> *mut RawRegion {
        REGION.try_with(|r| r.0.get()).unwrap_or(std::ptr::null_mut())
    }

    /// Raises `limit` to cover `need` (within the reservation).
    pub fn grow(r: *mut RawRegion, need: usize) -> bool {
        let r = unsafe { &mut *r };
        if need > r.end || r.end == 0 {
            return false;
        }
        let to = (need.div_ceil(STEP) * STEP).min(r.end);
        match os::commit(r.base, r.limit, to) {
            Some(l) => {
                r.limit = l;
                true
            }
            None => false,
        }
    }

    /// Gives back the pages above KEEP (the outermost frame closed after a large use).
    pub fn trim(r: *mut RawRegion) {
        let r = unsafe { &mut *r };
        let keep = (r.base + KEEP).min(r.end);
        if r.limit > keep {
            os::decommit(keep, r.limit - keep);
            r.limit = keep;
        }
    }

    #[cfg(unix)]
    mod os {
        use std::ffi::c_void;
        extern "C" {
            fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
            fn munmap(addr: *mut c_void, len: usize) -> i32;
            fn madvise(addr: *mut c_void, len: usize, advice: i32) -> i32;
        }
        const PROT_RW: i32 = 3;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const FLAGS: i32 = 0x22 | 0x4000; // MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        const FLAGS: i32 = 0x1002; // MAP_PRIVATE | MAP_ANON
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const DONTNEED: i32 = 4;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        const DONTNEED: i32 = 5; // MADV_FREE: the pages go when memory is needed

        pub fn reserve(len: usize) -> Option<usize> {
            let p = unsafe { mmap(std::ptr::null_mut(), len, PROT_RW, FLAGS, -1, 0) };
            (p as isize != -1 && !p.is_null()).then_some(p as usize)
        }
        /// The whole reservation is usable (pages commit on touch): the limit only marks use.
        pub fn commit(_base: usize, _from: usize, to: usize) -> Option<usize> {
            Some(to)
        }
        pub fn decommit(at: usize, len: usize) {
            unsafe { madvise(at as *mut c_void, len, DONTNEED) };
        }
        pub fn release(at: usize, len: usize) {
            unsafe { munmap(at as *mut c_void, len) };
        }
    }

    #[cfg(windows)]
    mod os {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        extern "system" {
            fn VirtualAlloc(addr: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
            fn VirtualFree(addr: *mut c_void, size: usize, kind: u32) -> i32;
        }
        const MEM_COMMIT: u32 = 0x1000;
        const MEM_RESERVE: u32 = 0x2000;
        const MEM_DECOMMIT: u32 = 0x4000;
        const MEM_RELEASE: u32 = 0x8000;
        const PAGE_NOACCESS: u32 = 1;
        const PAGE_READWRITE: u32 = 4;

        pub fn reserve(len: usize) -> Option<usize> {
            let p = unsafe { VirtualAlloc(std::ptr::null_mut(), len, MEM_RESERVE, PAGE_NOACCESS) };
            (!p.is_null()).then_some(p as usize)
        }
        /// Commits [from, to): the reservation's commit charge follows use.
        pub fn commit(_base: usize, from: usize, to: usize) -> Option<usize> {
            if to <= from {
                return Some(from);
            }
            let p = unsafe { VirtualAlloc(from as *mut c_void, to - from, MEM_COMMIT, PAGE_READWRITE) };
            (!p.is_null()).then_some(to)
        }
        pub fn decommit(at: usize, len: usize) {
            unsafe { VirtualFree(at as *mut c_void, len, MEM_DECOMMIT) };
        }
        pub fn release(at: usize, _len: usize) {
            unsafe { VirtualFree(at as *mut c_void, 0, MEM_RELEASE) };
        }
    }

    #[cfg(not(any(unix, windows)))]
    mod os {
        pub fn reserve(_len: usize) -> Option<usize> {
            None
        }
        pub fn commit(_base: usize, _from: usize, _to: usize) -> Option<usize> {
            None
        }
        pub fn decommit(_at: usize, _len: usize) {}
        pub fn release(_at: usize, _len: usize) {}
    }
}

pub(crate) use imp::{grow, region, trim};
