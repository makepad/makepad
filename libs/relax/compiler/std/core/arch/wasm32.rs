//! core::arch::wasm32 for the web target run on this machine (cfg.rs `web`): the linear
//! memory is the trace host's (`makepad-web-trace`, web_host.rs), which defines these symbols
//! with the semantics of the wasm instructions (memory index 0 only, sizes in 64 KiB pages).

extern "C" {
    fn relax_web_memory_size(mem: u32) -> usize;
    fn relax_web_memory_grow(mem: u32, delta: usize) -> usize;
}

/// `memory.size`: the current size of memory `MEM` in pages.
#[inline]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(0))]
pub fn memory_size<const MEM: u32>() -> usize {
    unsafe { relax_web_memory_size(MEM) }
}

/// `memory.grow`: grows memory `MEM` by `delta` pages; the previous size in pages, or
/// `usize::MAX` when it cannot grow.
#[inline]
#[cfg_attr(not(relax_check), rustc_legacy_const_generics(0))]
pub fn memory_grow<const MEM: u32>(delta: usize) -> usize {
    unsafe { relax_web_memory_grow(MEM, delta) }
}

extern "C" {
    fn relax_web_atomic_wait32(addr: *mut i32, expected: i32, timeout_ns: i64) -> i32;
    fn relax_web_atomic_notify(addr: *mut i32, count: u32) -> u32;
}

/// `memory.atomic.wait32`: blocks while `*ptr == expression` (a negative `timeout_ns` waits
/// forever). 0 = woken by a notify, 1 = the value was not `expression`, 2 = timed out.
#[inline]
pub unsafe fn memory_atomic_wait32(ptr: *mut i32, expression: i32, timeout_ns: i64) -> i32 {
    relax_web_atomic_wait32(ptr, expression, timeout_ns)
}

/// `memory.atomic.notify`: wakes up to `waiters` threads blocked on `ptr`; how many woke.
#[inline]
pub unsafe fn memory_atomic_notify(ptr: *mut i32, waiters: u32) -> u32 {
    relax_web_atomic_notify(ptr, waiters)
}

/// The 128-bit vector type (simd128): same size and alignment as wasm's `v128`.
#[derive(Copy, Clone)]
#[repr(C, align(16))]
#[allow(non_camel_case_types)]
pub struct v128(pub [i32; 4]);
