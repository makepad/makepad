//! The web target run on this machine (`MAKEPAD_RUN_CHECK_TARGET=wasm32-unknown-unknown`, cfg.rs
//! `web`): what Relax itself gives such a program. The page side (the JS imports, `env.*`) is the
//! embedder's: the trace host `makepad-web-trace` names its functions through `set_embedder_symbols`.
//!
//! - Linear memory: zone_alloc/web.rs (`activate`). Every program allocation lives in one 4 GB
//!   window, so `ptr as u32` is a wasm address and `u32 as *mut T` (lower.rs `cast`) adds the
//!   window's base back.
//! - `core::arch::wasm32::{memory_size, memory_grow}` (Relax std core/arch/wasm32.rs): the linear
//!   memory's committed size in pages; it grows by itself (an allocator over the arena), so an
//!   explicit grow only succeeds for 0 pages.

use std::sync::atomic::{AtomicUsize, Ordering};

/// The window's base: `u32 as *mut T` ORs it in (0 on Linux, where the window is the first 4 GB).
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub const BASE: u64 = crate::zone_alloc::web::BASE as u64;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub const BASE: u64 = 0;

/// Reserves the linear memory (before the program is linked: its allocator symbols name it).
pub fn activate() -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        if crate::zone_alloc::web::activate() {
            return Ok(());
        }
        Err(format!("the web target's linear memory ({:#x}+{:#x}) could not be reserved", crate::zone_alloc::web::BASE, crate::zone_alloc::web::ARENA_OFFSET))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    Err("the web target runs on macOS and Linux only".to_string())
}

const PAGE: usize = 64 << 10;

extern "C" fn relax_web_memory_size(_mem: u32) -> usize {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        crate::zone_alloc::web::stats().0.div_ceil(PAGE)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    0
}

extern "C" fn relax_web_memory_grow(mem: u32, delta: usize) -> usize {
    if delta == 0 { relax_web_memory_size(mem) } else { usize::MAX }
}

/// The embedder's page-side symbols (the program's `extern "C"` imports of `env`), asked first.
static EMBEDDER: AtomicUsize = AtomicUsize::new(0);

/// Names the embedder's lookup: `name` -> address of the function the program's import binds to.
pub fn set_embedder_symbols(f: fn(&[u8]) -> Option<usize>) {
    EMBEDDER.store(f as usize, Ordering::Release);
}

/// The symbols of the web target's runtime and of the embedder (jit.rs `loader_symbol`).
pub fn symbol(name: &[u8]) -> Option<usize> {
    let e = EMBEDDER.load(Ordering::Acquire);
    if e != 0 {
        let f: fn(&[u8]) -> Option<usize> = unsafe { std::mem::transmute(e) };
        if let Some(a) = f(name) {
            return Some(a);
        }
    }
    match name {
        b"relax_web_memory_size" => Some(relax_web_memory_size as *const () as usize),
        b"relax_web_memory_grow" => Some(relax_web_memory_grow as *const () as usize),
        _ => None,
    }
}
