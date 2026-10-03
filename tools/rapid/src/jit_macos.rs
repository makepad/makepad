//! macOS (Apple Silicon) parts of the live image runtime, a child module of jit.rs:
//! executable memory with MAP_JIT and per-thread W^X flips
//! (pthread_jit_write_protect_np + sys_icache_invalidate), the fault handler on the
//! Darwin arm64 ucontext, and symbol lookup in the default namespace.
//!
//! Same behaviour as the Linux code in jit.rs: a fault inside JIT code (or while a JIT
//! call is active) records a report with a frame-pointer backtrace and resumes at
//! `leave(ctx, 2)`, so the host continues with the next call.

use super::{collect_frames, ctx_ptr, fatal_report, in_compile, is_stack_fault, PanicInfo, PANIC, SIG_WATCHDOG, UNIT, WATCHDOG_HIT};
use std::sync::atomic::Ordering;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    fn sigaltstack(ss: *const StackT, old: *mut StackT) -> i32;
    fn pthread_jit_write_protect_np(enabled: i32);
    fn sys_icache_invalidate(start: *mut u8, len: usize);
    fn dlsym(handle: *mut u8, name: *const u8) -> *mut u8;
}

/// Darwin `struct sigaction` (user side): handler, sa_mask (u32), sa_flags.
#[repr(C)]
struct SigAction {
    handler: usize,
    mask: u32,
    flags: i32,
}

/// Darwin `stack_t`: ss_sp, ss_size, ss_flags.
#[repr(C)]
struct StackT {
    sp: *mut u8,
    size: usize,
    flags: i32,
}

pub const PROT_RW: i32 = 3;
pub const PROT_RWX: i32 = 7;
const MAP_PRIVATE: i32 = 0x2;
const MAP_ANON: i32 = 0x1000;
const MAP_JIT: i32 = 0x800;

const SA_ONSTACK: i32 = 0x1;
const SA_NODEFER: i32 = 0x10;
const SA_SIGINFO: i32 = 0x40;

const SIGILL: i32 = 4;
const SIGTRAP: i32 = 5;
const SIGFPE: i32 = 8;
const SIGBUS: i32 = 10;
const SIGSEGV: i32 = 11;

// Darwin arm64 signal frame offsets (checked against the SDK headers):
// siginfo_t.si_addr at 24; ucontext_t.uc_mcontext (pointer) at 48;
// mcontext64: __es (far, esr, exception) at 0, __ss at 16: x[0..29] at 0, fp 232,
// lr 240, sp 248, pc 256 within __ss.
const SI_ADDR: usize = 24;
const UC_MCONTEXT: usize = 48;
const SS: usize = 16;
const SS_FP: usize = SS + 232;
const SS_PC: usize = SS + 256;
const SS_SP: usize = SS + 248;

/// Anonymous memory; PROT_RWX requests MAP_JIT memory (executable, writable only
/// inside `write_code`).
pub fn os_alloc(size: usize, prot: i32) -> u64 {
    let flags = if prot == PROT_RWX { MAP_PRIVATE | MAP_ANON | MAP_JIT } else { MAP_PRIVATE | MAP_ANON };
    let p = unsafe { mmap(std::ptr::null_mut(), size, prot, flags, -1, 0) };
    if p as isize == -1 || p.is_null() {
        panic!("mmap failed");
    }
    p as u64
}

/// Writes machine code into MAP_JIT memory: flips this thread to write mode, copies,
/// flips back to execute mode and invalidates the instruction cache for the range.
pub unsafe fn write_code(addr: u64, b: &[u8]) {
    pthread_jit_write_protect_np(0);
    std::ptr::copy_nonoverlapping(b.as_ptr(), addr as *mut u8, b.len());
    pthread_jit_write_protect_np(1);
    sys_icache_invalidate(addr as *mut u8, b.len());
}

/// Symbol in any loaded image (RTLD_DEFAULT). libm lives in libSystem, always loaded.
pub unsafe fn find_symbol(name: *const u8) -> *mut u8 {
    dlsym(-2isize as *mut u8, name)
}

extern "C" fn on_signal(sig: i32, info: *mut u8, uc: *mut u8) {
    // the handler reads the unit's tables: wait out (or, for the watchdog, skip) a compile
    // on another thread so they are not changing underneath it
    if sig == SIG_WATCHDOG {
        if in_compile() || !super::try_lock_unit() {
            return;
        }
    } else {
        super::lock_unit();
    }
    super::ensure_altstack();
    unsafe { on_signal_locked(sig, info, uc) };
    super::unlock_unit();
}

unsafe fn on_signal_locked(sig: i32, info: *mut u8, uc: *mut u8) {
    unsafe {
        // signals can arrive while this thread is in JIT write mode (inside write_code)
        pthread_jit_write_protect_np(1);
        let mc = *(uc.add(UC_MCONTEXT) as *const *mut u8);
        let reg = |off: usize| -> *mut u64 { mc.add(off) as *mut u64 };
        let pc = *reg(SS_PC);
        let fp = *reg(SS_FP);
        let fault = *(info.add(SI_ADDR) as *const u64);
        if sig == SIG_WATCHDOG {
            // hang watchdog: act only when JIT code runs and the compiler is not running
            // (added by the compiler lane with the poll-free watchdog; mirrors jit.rs)
            if UNIT.is_null() || *ctx_ptr() == 0 || (&*UNIT).find_fn(pc).is_none() {
                return;
            }
            WATCHDOG_HIT.store(true, Ordering::SeqCst);
            let u = &*UNIT;
            let frames = collect_frames(fp, pc);
            PANIC.with(|p| *p.borrow_mut() = Some(PanicInfo { kind: "hang".to_string(), message: "watchdog: the call did not return in time".to_string(), site: u64::MAX, frames, fault_addr: 0, location: String::new() }));
            *reg(SS_PC) = u.leave;
            *reg(SS) = ctx_ptr() as u64;
            *reg(SS + 8) = 3;
            return;
        }
        if !UNIT.is_null() && *ctx_ptr() == 0 && (&*UNIT).find_fn(pc).is_some() {
            fatal_report(sig, fault, fp, pc, *reg(SS_SP));
        }
        if UNIT.is_null() || *ctx_ptr() == 0 {
            // not ours: restore the default action and return to crash normally
            let act = SigAction { handler: 0, mask: 0, flags: 0 };
            sigaction(sig, &act, std::ptr::null_mut());
            return;
        }
        let kind = match sig {
            SIGSEGV | SIGBUS if is_stack_fault(fault, *reg(SS_SP)) => "stack overflow",
            SIGSEGV => "segfault",
            SIGBUS => "bus error",
            SIGILL => "illegal instruction",
            SIGFPE => "arithmetic fault",
            SIGTRAP => "trap",
            _ => "signal",
        };
        let u = &*UNIT;
        let frames = collect_frames(fp, pc);
        PANIC.with(|p| *p.borrow_mut() = Some(PanicInfo { kind: kind.to_string(), message: format!("signal {} at {:#x}", sig, fault), site: u64::MAX, frames, fault_addr: fault, location: String::new() }));
        // resume at `leave(ctx, 2)`
        *reg(SS_PC) = u.leave;
        *reg(SS) = ctx_ptr() as u64;
        *reg(SS + 8) = 2;
    }
}

/// sigaltstack for the calling thread (64 KB)
pub fn altstack_this_thread() {
    unsafe {
        let size = 1 << 16;
        let stack = os_alloc(size, PROT_RW);
        let ss = StackT { sp: stack as *mut u8, size, flags: 0 };
        sigaltstack(&ss, std::ptr::null_mut());
    }
}

pub fn install_signals() {
    super::ensure_altstack();
    unsafe {
        for sig in [SIGILL, SIGTRAP, SIGFPE, SIGBUS, SIGSEGV, SIG_WATCHDOG] {
            let act = SigAction { handler: on_signal as *const () as usize, mask: 0, flags: SA_SIGINFO | SA_ONSTACK | SA_NODEFER };
            sigaction(sig, &act, std::ptr::null_mut());
        }
    }
}
