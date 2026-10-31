//! Trace linear memory. Program allocations belong to program dlmalloc. The host
//! reserves its window and commits zero-filled pages. The lower half holds JIT
//! code, statics, TLS and stacks; the upper half grows through memory.grow.
use std::sync::Mutex;
use std::ffi::c_void;

#[cfg(target_os = "linux")]
pub const BASE: usize = 0;
#[cfg(target_os = "macos")]
pub const BASE: usize = 0x3000_0000_0000;
pub const ARENA_OFFSET: usize = 0x8000_0000;
const PAGE: usize = 65536;
const END: usize = 1usize << 32;
static MEMORY: Mutex<Option<usize>> = Mutex::new(None);

unsafe extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
    fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
}

pub fn activate() -> bool {
    let mut end = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    if end.is_some() { return true; }
    #[cfg(target_os = "linux")]
    const FLAGS: i32 = 0x22 | 0x100000;
    #[cfg(target_os = "macos")]
    const FLAGS: i32 = 0x1002;
    let at = BASE + ARENA_OFFSET;
    let len = END - ARENA_OFFSET;
    let p = unsafe { mmap(at as *mut c_void, len, 0, FLAGS, -1, 0) };
    if p as isize == -1 { return false; }
    if p as usize != at {
        unsafe { munmap(p, len); }
        return false;
    }
    crate::memstat::name_mapping(at, len, c"makepad-run-web-memory");
    *end = Some(ARENA_OFFSET);
    true
}

pub fn active() -> bool {
    MEMORY.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

pub fn memory_size() -> usize {
    MEMORY.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or(0) / PAGE
}

/// Old page count, or usize::MAX without changing memory on failure.
pub fn memory_grow(delta: usize) -> usize {
    let mut state = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    let Some(old) = *state else { return usize::MAX };
    let Some(bytes) = delta.checked_mul(PAGE) else { return usize::MAX };
    let Some(end) = old.checked_add(bytes).filter(|n| *n <= END) else { return usize::MAX };
    if bytes != 0 && unsafe { mprotect((BASE + old) as *mut c_void, bytes, 3) } != 0 {
        return usize::MAX;
    }
    *state = Some(end);
    old / PAGE
}

/// Committed heap bytes; live allocation size belongs to dlmalloc and is not
/// inferred from host mappings (zero means unavailable).
pub fn stats() -> (usize, usize) {
    let end = MEMORY.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or(ARENA_OFFSET);
    (end - ARENA_OFFSET, 0)
}
