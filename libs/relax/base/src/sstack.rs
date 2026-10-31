//! The JIT program threads' secondary stacks (makepad_stack under `makepad run`; the codegen's
//! frame-scoped allocations later share them): per thread one large reservation used as a bump
//! region, committed as it is touched, made on the thread's first use and released at its exit.
//! The layout of `RawRegion` is makepad_stack's (libs/stack/src/region.rs): keep them equal.

/// One thread's secondary stack (makepad_stack::RawRegion).
#[repr(C)]
pub struct RawRegion {
    pub top: usize,
    pub limit: usize,
    pub depth: usize,
    pub base: usize,
    pub end: usize,
}

/// committed (Windows) / kept between uses
const KEEP: usize = 1 << 20;
const STEP: usize = 1 << 20;
#[cfg(target_pointer_width = "64")]
const RESERVE: usize = 8 << 30;
#[cfg(not(target_pointer_width = "64"))]
const RESERVE: usize = 16 << 20;

struct Owned(std::cell::UnsafeCell<RawRegion>);
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
    static REGION: Owned = Owned(std::cell::UnsafeCell::new(new_region()));
}

fn new_region() -> RawRegion {
    let mut len = RESERVE;
    while len >= (1 << 20) {
        if let Some(base) = os::reserve(len) {
            let limit = os::commit(base, (base + KEEP).min(base + len)).unwrap_or(base);
            return RawRegion { top: base, limit, depth: 0, base, end: base + len };
        }
        len /= 2;
    }
    RawRegion { top: 0, limit: 0, depth: 0, base: 0, end: 0 }
}

/// `__relax_sstack`: the calling thread's region (null after its exit began: frames use the heap).
pub extern "C" fn relax_sstack() -> *mut RawRegion {
    REGION.try_with(|r| r.0.get()).unwrap_or(std::ptr::null_mut())
}

/// `__relax_sstack_grow`: raises the region's limit to cover `need` (1) or says it is full (0).
pub extern "C" fn relax_sstack_grow(r: *mut RawRegion, need: usize) -> u32 {
    let r = unsafe { &mut *r };
    if r.end == 0 || need > r.end {
        return 0;
    }
    let to = (need.div_ceil(STEP) * STEP).min(r.end);
    match os::commit(r.limit, to) {
        Some(l) => {
            r.limit = l;
            1
        }
        None => 0,
    }
}

/// `__relax_sstack_trim`: gives back the pages above KEEP (the outermost frame closed).
pub extern "C" fn relax_sstack_trim(r: *mut RawRegion) {
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
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const FLAGS: i32 = 0x22 | 0x4000; // MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const FLAGS: i32 = 0x1002;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const DONTNEED: i32 = 4;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const DONTNEED: i32 = 5;

    pub fn reserve(len: usize) -> Option<usize> {
        let p = unsafe { mmap(std::ptr::null_mut(), len, 3, FLAGS, -1, 0) };
        if p as isize == -1 || p.is_null() {
            return None;
        }
        crate::memstat::name_mapping(p as usize, len, c"makepad-run-secondary-stack");
        Some(p as usize)
    }
    pub fn commit(_from: usize, to: usize) -> Option<usize> {
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
    use relax_session::win32::*;
    pub fn reserve(len: usize) -> Option<usize> {
        let p = unsafe { VirtualAlloc(std::ptr::null_mut(), len, MEM_RESERVE, PAGE_NOACCESS) };
        (!p.is_null()).then_some(p as usize)
    }
    pub fn commit(from: usize, to: usize) -> Option<usize> {
        if to <= from {
            return Some(from);
        }
        let p = unsafe { VirtualAlloc(from as *mut u8, to - from, MEM_COMMIT, PAGE_READWRITE) };
        (!p.is_null()).then_some(to)
    }
    pub fn decommit(at: usize, len: usize) {
        unsafe { VirtualFree(at as *mut u8, len, MEM_DECOMMIT) };
    }
    pub fn release(at: usize, _len: usize) {
        unsafe { VirtualFree(at as *mut u8, 0, MEM_RELEASE) };
    }
}

#[cfg(not(any(unix, windows)))]
mod os {
    pub fn reserve(_len: usize) -> Option<usize> {
        None
    }
    pub fn commit(_from: usize, _to: usize) -> Option<usize> {
        None
    }
    pub fn decommit(_at: usize, _len: usize) {}
    pub fn release(_at: usize, _len: usize) {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn region_grows_and_trims() {
        let r = super::relax_sstack();
        assert!(!r.is_null());
        let (base, end) = unsafe { ((*r).base, (*r).end) };
        assert!(end - base >= 1 << 20);
        assert_eq!(super::relax_sstack_grow(r, base + (64 << 20)), 1);
        unsafe { *((base + (64 << 20) - 8) as *mut u64) = 7 };
        super::relax_sstack_trim(r);
        assert_eq!(unsafe { (*r).limit }, base + super::KEEP);
    }
}
