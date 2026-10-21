//! The web target's linear memory (`MAKEPAD_RUN_CHECK_TARGET=wasm32-unknown-unknown`, run by the
//! trace host `makepad-web-trace`): every allocation of the program goes to one arena inside one
//! 4 GB-aligned window at `BASE`, as a wasm program's heap lives in its linear memory. A pointer's
//! low 32 bits are then its wasm address. On wasm32 the program's pointers are those offsets
//! (compiler web32.rs; `symbol32`: this allocator in that ABI); on wasm64 they are native and a
//! `u32 as *mut T` cast gets `BASE` back (lower.rs `cast`). On Linux the window is the first 4 GB
//! of the address space (`BASE` 0: a wasm address is the pointer).
//!
//! Active only after `activate()` (the trace host, before the program is linked): the program's
//! allocator symbols (`symbol`) then name these functions instead of domain.rs's.
use super::web_arena as arena;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, Ordering};

/// The 4 GB window holding the linear memory.
#[cfg(target_os = "linux")]
pub const BASE: usize = 0;
/// macOS keeps the first 4 GB unmapped (__PAGEZERO): a window far above the system's heaps.
#[cfg(target_os = "macos")]
pub const BASE: usize = 0x3000_0000_0000;
/// Where the arena starts in the window (2 GB long): wasm addresses below it stay unmapped, so
/// a stray small address (a null-ish pointer) faults as it would trap in the browser.
pub const ARENA_OFFSET: usize = 0x8000_0000;

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Reserves the linear memory; from now on the program's allocator symbols name it. False when
/// the range is taken (nothing changes then).
pub fn activate() -> bool {
    if !arena::init() {
        return false;
    }
    ACTIVE.store(true, Ordering::Release);
    true
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

#[inline(always)]
pub fn owns(p: *mut u8) -> bool {
    arena::owns(p)
}

/// (committed bytes, live requested bytes): `memory.size` is the committed part in pages.
pub fn stats() -> (usize, usize) {
    let (committed, requested, _) = arena::stats();
    (committed, requested)
}

/// (bytes touched, bytes in live slots, bytes in large blocks, chunks in the partial list): where
/// the committed bytes are (arena.rs `detail`).
pub fn detail() -> (usize, usize, usize, usize) {
    arena::detail()
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Header {
    size: usize,
    align: usize,
    offset: usize,
}

/// A C allocation (malloc family): the layout in a header in front of the block.
unsafe fn allocate(size: usize, align: usize, zero: bool) -> *mut u8 {
    let align = align.max(std::mem::align_of::<Header>());
    let offset = (std::mem::size_of::<Header>() + align - 1) & !(align - 1);
    let Some(total) = size.max(1).checked_add(offset) else { return std::ptr::null_mut() };
    let Ok(layout) = Layout::from_size_align(total, align) else { return std::ptr::null_mut() };
    let raw = if zero { arena::alloc_zeroed(layout) } else { arena::alloc(layout) };
    if raw.is_null() {
        return raw;
    }
    let p = raw.add(offset);
    p.cast::<Header>().sub(1).write(Header { size, align, offset });
    p
}

unsafe fn release(p: *mut u8) {
    let h = p.cast::<Header>().sub(1).read();
    arena::dealloc(p.sub(h.offset), Layout::from_size_align_unchecked(h.size.max(1) + h.offset, h.align));
}

/// A C block of the linear memory freed by native code (domain.rs `dealloc`).
pub unsafe fn dealloc_c(p: *mut u8) -> bool {
    if !owns(p) {
        return false;
    }
    release(p);
    true
}

/// A C block of the linear memory resized by native code (domain.rs `realloc_owned`).
pub unsafe fn realloc_c(p: *mut u8, size: usize) -> Option<*mut u8> {
    if !owns(p) {
        return None;
    }
    let h = p.cast::<Header>().sub(1).read();
    let q = allocate(size, h.align, false);
    if !q.is_null() {
        std::ptr::copy_nonoverlapping(p, q, h.size.min(size));
        release(p);
    }
    Some(q)
}

/// A Rust block of the linear memory freed through the global allocator (domain.rs).
pub unsafe fn dealloc_layout(p: *mut u8, l: Layout) -> bool {
    if !owns(p) {
        return false;
    }
    arena::dealloc(p, l);
    true
}

pub unsafe fn realloc_layout(p: *mut u8, l: Layout, size: usize) -> Option<*mut u8> {
    if !owns(p) {
        return None;
    }
    Some(resize(p, l, size))
}

unsafe fn resize(p: *mut u8, l: Layout, size: usize) -> *mut u8 {
    if arena::resize_in_place(p, l, size) {
        return p;
    }
    let q = arena::alloc(Layout::from_size_align_unchecked(size, l.align()));
    if !q.is_null() {
        std::ptr::copy_nonoverlapping(p, q, l.size().min(size));
        arena::dealloc(p, l);
    }
    q
}

// The program's allocator ABI (Relax std heap.rs; the C allocator of linked C code). Exhaustion
// answers null, as a wasm program's allocator does when memory.grow fails.
unsafe extern "C" fn web_alloc(size: usize, align: usize) -> *mut u8 {
    arena::alloc(Layout::from_size_align_unchecked(size, align))
}
unsafe extern "C" fn web_alloc_zeroed(size: usize, align: usize) -> *mut u8 {
    arena::alloc_zeroed(Layout::from_size_align_unchecked(size, align))
}
unsafe extern "C" fn web_dealloc(p: *mut u8, size: usize, align: usize) {
    let l = Layout::from_size_align_unchecked(size, align);
    if owns(p) { arena::dealloc(p, l) } else { System.dealloc(p, l) }
}
unsafe extern "C" fn web_resize(p: *mut u8, old: usize, align: usize, size: usize) -> *mut u8 {
    let l = Layout::from_size_align_unchecked(old, align);
    if owns(p) { resize(p, l, size) } else { System.realloc(p, l, size) }
}
extern "C" {
    fn free(p: *mut u8);
    fn realloc(p: *mut u8, size: usize) -> *mut u8;
}
unsafe extern "C" fn web_malloc(size: usize) -> *mut u8 {
    allocate(size, 16, false)
}
unsafe extern "C" fn web_calloc(n: usize, size: usize) -> *mut u8 {
    match n.checked_mul(size) {
        Some(total) => allocate(total, 16, true),
        None => std::ptr::null_mut(),
    }
}
unsafe extern "C" fn web_free(p: *mut u8) {
    if !p.is_null() && !dealloc_c(p) {
        free(p);
    }
}
unsafe extern "C" fn web_realloc(p: *mut u8, size: usize) -> *mut u8 {
    if p.is_null() {
        return web_malloc(size);
    }
    if size == 0 {
        web_free(p);
        return std::ptr::null_mut();
    }
    realloc_c(p, size).unwrap_or_else(|| realloc(p, size))
}
unsafe extern "C" fn web_memalign(out: *mut *mut u8, align: usize, size: usize) -> i32 {
    if !align.is_power_of_two() || align < std::mem::size_of::<usize>() {
        return 22;
    }
    let p = allocate(size, align, false);
    if p.is_null() {
        return 12;
    }
    *out = p;
    0
}

/// The program's allocator symbols while the linear memory is active (domain.rs `symbol`).
pub fn symbol(name: &[u8]) -> Option<usize> {
    if !active() {
        return None;
    }
    Some(match name {
        b"relax_alloc" => web_alloc as *const () as usize,
        b"relax_alloc_zeroed" => web_alloc_zeroed as *const () as usize,
        b"relax_dealloc" => web_dealloc as *const () as usize,
        b"relax_realloc" => web_resize as *const () as usize,
        b"malloc" => web_malloc as *const () as usize,
        b"calloc" => web_calloc as *const () as usize,
        b"realloc" => web_realloc as *const () as usize,
        b"free" => web_free as *const () as usize,
        b"posix_memalign" => web_memalign as *const () as usize,
        _ => return None,
    })
}

// The same allocator in the wasm32 offset ABI (compiler web32.rs): a pointer argument or result is
// the block's offset in the window (0 = null).
#[inline(always)]
fn w32_ptr(p: usize) -> *mut u8 {
    if p as u32 == 0 { std::ptr::null_mut() } else { (BASE | (p & 0xffff_ffff)) as *mut u8 }
}
#[inline(always)]
fn w32_off(p: *mut u8) -> usize {
    p as usize & 0xffff_ffff
}
unsafe extern "C" fn w32_alloc(size: usize, align: usize) -> usize {
    w32_off(web_alloc(size, align))
}
unsafe extern "C" fn w32_alloc_zeroed(size: usize, align: usize) -> usize {
    w32_off(web_alloc_zeroed(size, align))
}
unsafe extern "C" fn w32_dealloc(p: usize, size: usize, align: usize) {
    web_dealloc(w32_ptr(p), size, align)
}
unsafe extern "C" fn w32_resize(p: usize, old: usize, align: usize, size: usize) -> usize {
    w32_off(web_resize(w32_ptr(p), old, align, size))
}
unsafe extern "C" fn w32_malloc(size: usize) -> usize {
    w32_off(web_malloc(size))
}
unsafe extern "C" fn w32_calloc(n: usize, size: usize) -> usize {
    w32_off(web_calloc(n, size))
}
unsafe extern "C" fn w32_free(p: usize) {
    web_free(w32_ptr(p))
}
unsafe extern "C" fn w32_realloc(p: usize, size: usize) -> usize {
    w32_off(web_realloc(w32_ptr(p), size))
}
unsafe extern "C" fn w32_memalign(out: usize, align: usize, size: usize) -> i32 {
    // `*out` is a 4-byte pointer word in the program's memory
    if !align.is_power_of_two() || align < 4 {
        return 22;
    }
    let p = allocate(size, align, false);
    if p.is_null() {
        return 12;
    }
    (w32_ptr(out) as *mut u32).write_unaligned(w32_off(p) as u32);
    0
}

/// The allocator symbols of a wasm32 program (offsets in and out); None while the linear memory
/// is not active.
pub fn symbol32(name: &[u8]) -> Option<usize> {
    if !active() {
        return None;
    }
    Some(match name {
        b"relax_alloc" => w32_alloc as *const () as usize,
        b"relax_alloc_zeroed" => w32_alloc_zeroed as *const () as usize,
        b"relax_dealloc" => w32_dealloc as *const () as usize,
        b"relax_realloc" => w32_resize as *const () as usize,
        b"malloc" => w32_malloc as *const () as usize,
        b"calloc" => w32_calloc as *const () as usize,
        b"realloc" => w32_realloc as *const () as usize,
        b"free" => w32_free as *const () as usize,
        b"posix_memalign" => w32_memalign as *const () as usize,
        _ => return None,
    })
}
