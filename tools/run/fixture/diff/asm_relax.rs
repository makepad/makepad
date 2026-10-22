// diff_fast corpus program: the shapes of Relax's own assembly -- relax-base stack.rs
// call_on_segment (switch sp, call, clobber_abi("C")), zone_alloc arena.rs (thread pointer), inspect/heap.rs
// naked veneers (jmp through a static, sym), codegen interp.rs global_asm! (pool of numbered entries via
// .rept/.set, local labels, a call back into Rust through sym), compiler os/mem.rs cache maintenance (arm64).
// Every result line is `D ...`.
use std::arch::{asm, global_asm};
use std::sync::atomic::{AtomicUsize, Ordering};
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

// x86_64: the asm below passes arguments in SysV registers (rdi, rsi) and leaves no Win64 shadow
// space, so the functions it reaches are `extern "sysv64"` there (the C ABI on Linux and macOS,
// not on Windows); aarch64 keeps the C ABI.
#[cfg(target_arch = "x86_64")]
type SegFn = extern "sysv64" fn(*mut u8);
#[cfg(target_arch = "aarch64")]
type SegFn = extern "C" fn(*mut u8);
#[cfg(target_arch = "x86_64")]
type U64Fn = extern "sysv64" fn(u64) -> u64;
#[cfg(target_arch = "aarch64")]
type U64Fn = extern "C" fn(u64) -> u64;

// ---- stack.rs call_on_segment: run `f(data)` with sp at the top of `stack`
#[cfg(target_arch = "x86_64")]
extern "sysv64" fn on_segment(data: *mut u8) { on_segment_body(data) }
#[cfg(target_arch = "aarch64")]
extern "C" fn on_segment(data: *mut u8) { on_segment_body(data) }
fn on_segment_body(data: *mut u8) {
    let probe = 0u8;
    let v = unsafe { &mut *(data as *mut (usize, usize, u64)) };
    let sp = &probe as *const u8 as usize;
    v.2 = (sp > v.0 && sp <= v.1) as u64 * 1000 + deep(20);
}
#[inline(never)]
fn deep(n: u64) -> u64 { if n == 0 { 0 } else { std::hint::black_box(n) + deep(n - 1) } }

#[cfg(target_arch = "x86_64")]
unsafe fn call_on_segment(sp: usize, f: SegFn, data: *mut u8) {
    asm!("mov r12, rsp", "mov rsp, {sp}", "call {f}", "mov rsp, r12", sp = in(reg) sp, f = in(reg) f, in("rdi") data, out("r12") _, clobber_abi("sysv64"));
}
#[cfg(target_arch = "aarch64")]
unsafe fn call_on_segment(sp: usize, f: SegFn, data: *mut u8) {
    asm!("mov x20, sp", "mov sp, {sp}", "blr {f}", "mov sp, x20", sp = in(reg) sp, f = in(reg) f, in("x0") data, out("x20") _, clobber_abi("C"));
}

// ---- heap.rs: a naked veneer that jumps through a word another thread may change
static TARGET: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "x86_64")]
extern "sysv64" fn twice(x: u64) -> u64 { x * 2 }
#[cfg(target_arch = "x86_64")]
extern "sysv64" fn thrice(x: u64) -> u64 { x * 3 }
#[cfg(target_arch = "aarch64")]
extern "C" fn twice(x: u64) -> u64 { x * 2 }
#[cfg(target_arch = "aarch64")]
extern "C" fn thrice(x: u64) -> u64 { x * 3 }
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
extern "sysv64" fn veneer(_x: u64) -> u64 {
    core::arch::naked_asm!("jmp qword ptr [rip + {t}]", t = sym TARGET)
}
#[cfg(all(target_arch = "aarch64", target_vendor = "apple"))]
#[unsafe(naked)]
extern "C" fn veneer(_x: u64) -> u64 {
    core::arch::naked_asm!("adrp x16, {t}@PAGE", "ldr x16, [x16, {t}@PAGEOFF]", "br x16", t = sym TARGET)
}
#[cfg(all(target_arch = "aarch64", not(target_vendor = "apple")))]
#[unsafe(naked)]
extern "C" fn veneer(_x: u64) -> u64 {
    core::arch::naked_asm!("adrp x16, {t}", "ldr x16, [x16, :lo12:{t}]", "br x16", t = sym TARGET)
}
// a naked fn that adds the caller-visible frame pointer into a tracked call (heap.rs track_*)
#[cfg(target_arch = "x86_64")]
extern "sysv64" fn tracked(x: u64, fp_nonzero: u64) -> u64 { x + 100 * (fp_nonzero != 0) as u64 }
#[cfg(target_arch = "aarch64")]
extern "C" fn tracked(x: u64, fp_nonzero: u64) -> u64 { x + 100 * (fp_nonzero != 0) as u64 }
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
extern "sysv64" fn track(_x: u64) -> u64 {
    core::arch::naked_asm!("mov rsi, rbp", "jmp {f}", f = sym tracked)
}
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
extern "C" fn track(_x: u64) -> u64 {
    core::arch::naked_asm!("mov x1, x29", "b {f}", f = sym tracked)
}

// ---- interp.rs: a global_asm! pool of numbered entries that call back into Rust
#[cfg(target_arch = "x86_64")]
#[no_mangle]
extern "sysv64" fn diff_asm_enter(k: u64, x: u64) -> u64 { k * 1000 + x }
#[cfg(target_arch = "aarch64")]
#[no_mangle]
extern "C" fn diff_asm_enter(k: u64, x: u64) -> u64 { k * 1000 + x }
#[cfg(target_arch = "x86_64")]
global_asm!(
    ".text",
    ".p2align 4",
    ".globl diff_asm_pool",
    "diff_asm_pool:",
    ".set dk, 0",
    ".rept 8",
    ".p2align 4",
    "  mov esi, edi",
    "  mov edi, dk",
    "  jmp 2f",
    ".set dk, dk + 1",
    ".endr",
    "2:",
    "  jmp {enter}",
    enter = sym diff_asm_enter,
);
#[cfg(all(target_arch = "aarch64", target_vendor = "apple"))]
macro_rules! csym { ($s:literal) => { concat!("_", $s) } }
#[cfg(all(target_arch = "aarch64", not(target_vendor = "apple")))]
macro_rules! csym { ($s:literal) => { $s } }
#[cfg(target_arch = "aarch64")]
global_asm!(
    ".text",
    ".p2align 4",
    concat!(".globl ", csym!("diff_asm_pool")),
    concat!(csym!("diff_asm_pool"), ":"),
    ".set dk, 0",
    ".rept 8",
    ".p2align 4",
    "  mov x1, x0",
    "  movz w0, #dk",
    "  b 2f",
    ".set dk, dk + 1",
    ".endr",
    "2:",
    "  b {enter}",
    enter = sym diff_asm_enter,
);
extern "C" { fn diff_asm_pool(); }
fn pool_entry(k: usize) -> U64Fn {
    // every entry starts on a 16-byte boundary (.p2align 4)
    let base = diff_asm_pool as usize;
    unsafe { std::mem::transmute::<usize, U64Fn>(base + k * 16) }
}

// ---- arena.rs / tls.rs: the thread pointer
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn thread_ptr() -> usize { let tp: usize; unsafe { asm!("mov {}, qword ptr fs:[0]", out(reg) tp, options(pure, readonly, nostack, preserves_flags)) }; tp }
// Darwin's user thread pointer is TPIDRRO_EL0 (the TSD base, cpu number in its low 3 bits);
// TPIDR_EL0 there is not per-thread state a program owns. Linux / Android: TPIDR_EL0.
#[cfg(all(target_arch = "aarch64", target_vendor = "apple"))]
fn thread_ptr() -> usize { let tp: usize; unsafe { asm!("mrs {}, tpidrro_el0", out(reg) tp, options(nomem, nostack, preserves_flags)) }; tp & !7 }
#[cfg(all(target_arch = "aarch64", not(target_vendor = "apple")))]
fn thread_ptr() -> usize { let tp: usize; unsafe { asm!("mrs {}, tpidr_el0", out(reg) tp, options(nomem, nostack, preserves_flags)) }; tp }
#[cfg(not(any(all(target_arch = "x86_64", target_os = "linux"), target_arch = "aarch64")))]
fn thread_ptr() -> usize { 1 }

// ---- os/mem.rs: instruction cache maintenance after writing code (arm64)
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn flush(a: usize, n: usize) -> u64 {
    let ctr: u64;
    unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    let line = 4usize << (ctr & 0xf);
    let mut p = a & !(line - 1);
    while p < a + n {
        unsafe { asm!("dc cvau, {}", in(reg) p, options(nostack, preserves_flags)) };
        p += line;
    }
    unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };
    let mut p = a & !(line - 1);
    while p < a + n {
        unsafe { asm!("ic ivau, {}", in(reg) p, options(nostack, preserves_flags)) };
        p += line;
    }
    unsafe { asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
    (line >= 4) as u64
}
#[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
fn flush(_a: usize, _n: usize) -> u64 { 1 }

fn work() {
    let mut stack = vec![0u8; 256 << 10];
    let lo = stack.as_mut_ptr() as usize;
    let top = (lo + stack.len()) & !15;
    let mut data = (lo, top, 0u64);
    unsafe { call_on_segment(top, on_segment, &mut data as *mut _ as *mut u8) };
    d!("segment {}", data.2);
    TARGET.store(twice as usize, Ordering::SeqCst);
    d!("veneer twice {}", veneer(21));
    TARGET.store(thrice as usize, Ordering::SeqCst);
    d!("veneer thrice {}", veneer(21));
    d!("track {}", track(5));
    for k in [0usize, 1, 7] { d!("pool {} = {}", k, pool_entry(k)(42)); }
    d!("tp {}", (thread_ptr() != 0) as u32);
    let code = [0u8; 256];
    d!("flush {}", flush(code.as_ptr() as usize, code.len()));
}
