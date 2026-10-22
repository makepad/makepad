//! The web targets run on this machine (`MAKEPAD_RUN_CHECK_TARGET=wasm32-unknown-unknown` /
//! `wasm64-unknown-unknown`, cfg.rs `web`): what Relax itself gives such a program. The page side
//! (the JS imports, `env.*`) is the embedder's: the trace host `makepad-web-trace` names its
//! functions through `set_embedder_symbols`.
//!
//! - Linear memory: zone_alloc/web.rs (`activate`): every program allocation lives in one 4 GB
//!   window. On wasm32 a pointer is a 32-bit offset into it (web32.rs: code, data, statics,
//!   thread-locals and stacks are in the window too); on wasm64 pointers are native and
//!   `u32 as *mut T` (lower.rs `cast`) adds the window's base back.
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

/// A pointer argument of the program as a host address: an offset on wasm32 (web32.rs).
fn program_addr(p: usize) -> usize {
    if crate::web32::on() { crate::web32::native(p as u64) as usize } else { p }
}

/// `relax_web_print(ptr, len)`: the bytes to stderr (test programs' output; a page has no
/// stdout). The pointer is the program's: an offset on wasm32.
extern "C" fn relax_web_print(p: usize, len: usize) {
    use std::io::Write;
    let a = if crate::web32::on() { crate::web32::native(p as u64) as usize } else { p };
    if len != 0 {
        let b = unsafe { std::slice::from_raw_parts(a as *const u8, len) };
        let _ = std::io::stderr().write_all(b);
    }
}

/// A thread blocked in `relax_web_atomic_wait32`: woken once its flag is set.
struct Waiter {
    woken: std::sync::Mutex<bool>,
    cv: std::sync::Condvar,
}

/// The threads waiting on each address (`memory.atomic.wait32` / `notify` over this process's
/// threads: the page's instructions are the real thing, this is their semantics on the trace host).
static WAITERS: std::sync::Mutex<Vec<(usize, std::sync::Arc<Waiter>)>> = std::sync::Mutex::new(Vec::new());

/// `memory.atomic.wait32`: 0 = woken by a notify, 1 = `*addr != expected`, 2 = timed out; a negative
/// timeout waits forever. The value check and the registration are atomic against `notify`.
extern "C" fn relax_web_atomic_wait32(addr: usize, expected: i32, timeout_ns: i64) -> i32 {
    let addr = program_addr(addr);
    let a = unsafe { &*(addr as *const std::sync::atomic::AtomicI32) };
    let w = std::sync::Arc::new(Waiter { woken: std::sync::Mutex::new(false), cv: std::sync::Condvar::new() });
    {
        let mut table = WAITERS.lock().unwrap();
        if a.load(Ordering::SeqCst) != expected {
            return 1;
        }
        table.push((addr, w.clone()));
    }
    let deadline = if timeout_ns < 0 { None } else { Some(std::time::Instant::now() + std::time::Duration::from_nanos(timeout_ns as u64)) };
    let mut woken = w.woken.lock().unwrap();
    while !*woken {
        match deadline {
            None => woken = w.cv.wait(woken).unwrap(),
            Some(d) => {
                let now = std::time::Instant::now();
                if now >= d {
                    break;
                }
                woken = w.cv.wait_timeout(woken, d - now).unwrap().0;
            }
        }
    }
    if *woken {
        return 0;
    }
    drop(woken);
    // timed out: leave the table unless a notify took this waiter in the meantime
    let mut table = WAITERS.lock().unwrap();
    match table.iter().position(|(_, x)| std::sync::Arc::ptr_eq(x, &w)) {
        Some(i) => {
            table.remove(i);
            2
        }
        None => 0,
    }
}

/// `memory.atomic.notify`: wakes up to `count` waiters of `addr`, oldest first; how many.
extern "C" fn relax_web_atomic_notify(addr: usize, count: u32) -> u32 {
    let addr = program_addr(addr);
    let mut table = WAITERS.lock().unwrap();
    let mut n = 0;
    let mut i = 0;
    while i < table.len() && n < count {
        if table[i].0 == addr {
            let (_, w) = table.remove(i);
            *w.woken.lock().unwrap() = true;
            w.cv.notify_one();
            n += 1;
        } else {
            i += 1;
        }
    }
    n
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
        b"relax_web_atomic_wait32" => Some(relax_web_atomic_wait32 as *const () as usize),
        b"relax_web_atomic_notify" => Some(relax_web_atomic_notify as *const () as usize),
        b"relax_web_print" => Some(relax_web_print as *const () as usize),
        _ => None,
    }
}
