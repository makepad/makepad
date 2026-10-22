//! The wasm32 web target run on this machine (`MAKEPAD_RUN_CHECK_TARGET=wasm32-unknown-unknown`,
//! local/agent_state/wasm/web32.DESIGN.md): the runtime half. The program's pointers are 32-bit
//! offsets into the linear memory's 4 GB window at `BASE` (zone_alloc/web.rs), so everything it
//! can address is placed in the window (offsets):
//! - `UNIT_AT`: the program unit's reservation (code, data, statics, the symbol table);
//! - `SLOTS_AT`: the function slots' segments (thunks: a fn pointer is a code offset);
//! - `MISC_AT..ARENA`: thread-local blocks, domain static blocks, shadow stacks, runtime results;
//! - the heap arena from `zone_alloc::web::ARENA_OFFSET` (2 GB).
//! relax_codegen::web32 rewrites the program's code for it; the host functions the program calls
//! take and return offsets (`symbol`, `Rt`), the web import ABI.

include!("rt_abi.rs");

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The window's base (0 on Linux: an offset is the address).
pub const BASE: u64 = crate::web::BASE;
pub const UNIT_AT: u64 = 16 << 20;
/// the unit reservation's parts (smaller than a native unit's: the window holds all of it)
pub const UNIT_CODE: u64 = 384 << 20;
pub const UNIT_DATA: u64 = 384 << 20;
#[cfg(any(target_os = "macos", target_os = "linux"))]
const SLOTS_AT: u64 = 0x4500_0000;
#[cfg(any(target_os = "macos", target_os = "linux"))]
const SLOTS_END: u64 = 0x5500_0000;
const MISC_AT: u64 = 0x5500_0000;
#[cfg(any(target_os = "macos", target_os = "linux"))]
const MISC_END: u64 = crate::zone_alloc::web::ARENA_OFFSET as u64;
/// no window here (reserve_unit panics): alloc never runs
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const MISC_END: u64 = MISC_AT;
/// a thread's shadow stack
const STACK: u64 = 16 << 20;

static ON: AtomicBool = AtomicBool::new(false);
#[cfg(any(target_os = "macos", target_os = "linux"))]
static SLOTS_NEXT: AtomicU64 = AtomicU64::new(SLOTS_AT);
static MISC_NEXT: AtomicU64 = AtomicU64::new(0);
static FREE: relax_session::sync::Mutex<Vec<(u64, u64)>> = relax_session::sync::Mutex::new(Vec::new());
/// thread-local block offset of the shadow stack words [pointer, limit]
static SS_OFF: AtomicU64 = AtomicU64::new(0);

/// Whether a program of this session runs as wasm32 (its world: the program's, never a
/// proc-macro world, which runs on this machine).
pub fn target(sess: &crate::Session) -> bool {
    sess.options.check_target == crate::options::CheckTarget::Web(crate::options::PtrWidth::W32)
}

/// A W32 program unit exists in this process (its thread-local blocks and statics go in the window).
pub fn on() -> bool {
    ON.load(Ordering::Acquire)
}

/// The native address of a program pointer (an offset; a native address in the window stays).
#[inline(always)]
pub fn native(p: u64) -> u64 {
    BASE | (p & 0xffff_ffff)
}

/// A program pointer argument as a host pointer: 0 stays null.
#[inline(always)]
pub fn ptr(p: u64) -> *mut u8 {
    if p & 0xffff_ffff == 0 { std::ptr::null_mut() } else { native(p) as *mut u8 }
}

/// The program's view of a native address in the window (null stays 0).
#[inline(always)]
pub fn offset(a: u64) -> u64 {
    a & 0xffff_ffff
}

/// Reserves the program unit at `UNIT_AT` and makes this process's W32 state (the window's misc
/// region). Panics when the window is taken (one wasm32 program per process).
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn reserve_unit(_code: usize, _rest: usize) -> u64 {
    panic!("makepad run: the wasm32 web target runs on macOS and Linux only")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn reserve_unit(code: usize, rest: usize) -> u64 {
    if !crate::zone_alloc::web::active() {
        if let Err(e) = crate::web::activate() {
            panic!("makepad run: {}", e);
        }
    }
    let at = crate::os::mem::reserve_at(BASE + UNIT_AT, code, rest);
    if at == 0 {
        panic!("makepad run: the wasm32 window's unit range {:#x}+{:#x} is taken (one wasm32 program per process)", BASE + UNIT_AT, code + rest);
    }
    assert!(UNIT_AT + (code + rest) as u64 <= SLOTS_AT, "web32: unit reservation overlaps the slots");
    let misc = map_rw(BASE + MISC_AT, (MISC_END - MISC_AT) as usize);
    if misc == 0 {
        panic!("makepad run: the wasm32 window's misc range {:#x} is taken", BASE + MISC_AT);
    }
    MISC_NEXT.store(BASE + MISC_AT, Ordering::Release);
    ON.store(true, Ordering::Release);
    at
}

/// Reserves a slot segment (thunks and stubs, then table words and tier records) in the window.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn reserve_slots(code: usize, rest: usize) -> u64 {
    let len = ((code + rest) as u64 + 0xffff) & !0xffff;
    let at = SLOTS_NEXT.fetch_add(len, Ordering::AcqRel);
    if at + len > SLOTS_END {
        panic!("makepad run: the wasm32 window's function slots are full");
    }
    let r = crate::os::mem::reserve_at(BASE + at, code, rest);
    if r == 0 {
        panic!("makepad run: the wasm32 window's slot range {:#x} is taken", BASE + at);
    }
    r
}

/// No W32 unit exists off macOS / Linux (reserve_unit panics), so none asks for slots.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn reserve_slots(_code: usize, _rest: usize) -> u64 {
    unreachable!("web32: slots without a unit")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn map_rw(at: u64, len: usize) -> u64 {
    extern "C" {
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn munmap(addr: *mut u8, len: usize) -> i32;
    }
    #[cfg(target_os = "linux")]
    const FLAGS: i32 = 0x22 | 0x100000;
    #[cfg(not(target_os = "linux"))]
    const FLAGS: i32 = 0x1002;
    unsafe {
        let p = mmap(at as *mut u8, len, 3, FLAGS, -1, 0);
        if p as isize == -1 {
            return 0;
        }
        if p as u64 != at {
            munmap(p, len);
            return 0;
        }
    }
    crate::memstat::name_mapping(at as usize, len, c"makepad-run-web32-misc");
    at
}

/// Zeroed read-write memory in the window's misc region (native address; page granular).
pub fn alloc(size: usize) -> u64 {
    let len = (size as u64 + 0xfff) & !0xfff;
    {
        let mut f = FREE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = f.iter().position(|&(_, l)| l == len) {
            let (a, _) = f.swap_remove(i);
            unsafe { std::ptr::write_bytes(a as *mut u8, 0, len as usize) };
            return a;
        }
    }
    let a = MISC_NEXT.fetch_add(len, Ordering::AcqRel);
    if a + len > BASE + MISC_END {
        // called from runtime entries (no unwinding): fatal, said plainly
        eprintln!("makepad run: the wasm32 window's misc region is full ({} bytes asked)", size);
        std::process::abort();
    }
    a
}

/// Gives `alloc` memory back.
pub fn free(a: u64, size: usize) {
    let len = (size as u64 + 0xfff) & !0xfff;
    FREE.lock().unwrap_or_else(|e| e.into_inner()).push((a, len));
}

/// Names the thread-local words of the shadow stack (Unit::new).
pub fn set_ss_off(off: u32) {
    SS_OFF.store(off as u64, Ordering::Release);
}

fn ss_words() -> *mut u64 {
    (crate::jit::tls_block() as u64 + SS_OFF.load(Ordering::Acquire)) as *mut u64
}

rt_fn! {
/// relax_codegen::web32's slow path: the calling thread's shadow stack has no room for `frame`
/// bytes (or none yet). Returns the new stack pointer (the caller stores it).
pub fn rt_w32_stack(frame: u64) -> u64 {
    let w = ss_words();
    unsafe {
        // no stack yet; or the outermost frame returned (it stored back the 0 it found)
        if *w.add(1) == 0 {
            *w.add(1) = alloc(STACK as usize);
        }
        if *w == 0 {
            *w = *w.add(1) + STACK;
        }
        let (sp, lim) = (*w, *w.add(1));
        if sp < lim + frame {
            // as a native stack overflow: fatal
            eprintln!("makepad run: wasm32 shadow stack overflow (recursion too deep)");
            std::process::abort();
        }
        sp - frame
    }
}
}

/// A thread's block is freed: its shadow stack too.
pub fn thread_exit(block: u64) {
    let w = (block + SS_OFF.load(Ordering::Acquire)) as *mut u64;
    unsafe {
        let lo = *w.add(1);
        if lo != 0 {
            free(lo, STACK as usize);
        }
    }
}

/// The calling thread's shadow stack pointer and limit (saved around a recovery point).
pub fn ss_save() -> (u64, u64) {
    let w = ss_words();
    unsafe { (*w, *w.add(1)) }
}

pub fn ss_restore(s: (u64, u64)) {
    let w = ss_words();
    unsafe {
        *w = s.0;
        *w.add(1) = s.1;
    }
}

// ---------------------------------------------------------------- the runtime entries (offsets)

rt_fn! {
pub fn w32_fmt_str(p: u64, len: u64, spec: u64) {
    crate::jit::rt_fmt_str(ptr(p), len, spec)
}
}

rt_fn! {
pub fn w32_str_eq(p1: u64, l1: u64, p2: u64, l2: u64) -> u64 {
    crate::jit::rt_str_eq(ptr(p1), l1, ptr(p2), l2)
}
}

rt_fn! {
pub fn w32_panic_impl(msg: u64, len: u64, file: u64, file_len: u64, line: u64, col: u64, regs: *const u64, a7: u64) {
    crate::jit::rt_panic_impl(ptr(msg), len, ptr(file), file_len, line, col, regs, a7)
}
}

rt_fn! {
pub fn w32_catch_panic(f: u64, data: u64) -> u64 {
    // an unwind skips the frames' returns: the shadow stack is put back at the recovery point
    let s = ss_save();
    let r = crate::jit::rt_catch_panic(native(f), data);
    ss_restore(s);
    r
}
}

rt_fn! {
pub fn w32_thread_run(entry: u64, arg: u64) -> u64 {
    let s = ss_save();
    let r = crate::jit::rt_thread_run(native(entry), arg);
    ss_restore(s);
    r
}
}

rt_fn! {
/// wasm32-unknown-unknown has no argv
pub fn w32_args() -> crate::jit::ArgsRet {
    crate::jit::ArgsRet { argc: 0, argv: 0 }
}
}

rt_fn! {
pub fn w32_memcpy(d: u64, s: u64, n: u64) -> u64 {
    if n != 0 {
        unsafe { std::ptr::copy_nonoverlapping(native(s) as *const u8, native(d) as *mut u8, n as usize) };
    }
    offset(d)
}
}

rt_fn! {
pub fn w32_memmove(d: u64, s: u64, n: u64) -> u64 {
    if n != 0 {
        unsafe { std::ptr::copy(native(s) as *const u8, native(d) as *mut u8, n as usize) };
    }
    offset(d)
}
}

rt_fn! {
pub fn w32_memset(d: u64, c: u64, n: u64) -> u64 {
    if n != 0 {
        unsafe { std::ptr::write_bytes(native(d) as *mut u8, c as u8, n as usize) };
    }
    offset(d)
}
}

thread_local! {
    /// the window copy of this thread's last formatted text (w32_fmt_pop): (address, capacity)
    static POPPED: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
}

rt_fn! {
/// rt_fmt_pop's text, copied into the window (the program reads it)
pub fn w32_fmt_pop() -> crate::jit::StrRet {
    let r = crate::jit::rt_fmt_pop();
    let (p, len) = r.parts();
    POPPED.with(|c| {
        let (mut at, mut cap) = c.get();
        if cap < len.max(1) {
            if at != 0 {
                free(at, cap as usize);
            }
            cap = len.max(64).next_power_of_two();
            at = alloc(cap as usize);
            c.set((at, cap));
        }
        unsafe { std::ptr::copy_nonoverlapping(p as *const u8, at as *mut u8, len as usize) };
        crate::jit::StrRet::new(offset(at), len)
    })
}
}

/// The host functions a wasm32 program may import, by name: the web runtime's, its embedder's
/// page side (web.rs), the linear memory's allocator in the offset ABI, the memory functions and
/// the Relax runtime keys. None: not available on the wasm32 target (never called with offsets).
pub fn symbol(name: &[u8]) -> Option<usize> {
    if let Some(a) = crate::web::symbol(name) {
        return Some(a);
    }
    Some(match name {
        b"memcpy" => w32_memcpy as *const () as usize,
        b"memmove" => w32_memmove as *const () as usize,
        b"memset" => w32_memset as *const () as usize,
        b"__relax_key_create" => w32_key_create as *const () as usize,
        b"__relax_key_delete" => crate::domain_threads::key_delete as *const () as usize,
        b"__relax_key_get" => crate::domain_threads::key_get as *const () as usize,
        b"__relax_key_set" => crate::domain_threads::key_set as *const () as usize,
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        _ => return crate::zone_alloc::web::symbol32(name),
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        _ => return None,
    })
}

/// A runtime key whose destructor is program code (a code offset).
extern "C" fn w32_key_create(dtor: u64, late: u32) -> u32 {
    crate::domain_threads::key_create(native(dtor) as usize, late)
}
