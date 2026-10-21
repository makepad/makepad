//! Hardware faults and the watchdog's interrupt, handed to the runtime's portable policy
//! (`jit::on_fault`) as a [`Fault`]; the OS half applies the decision to the thread's context.
//!
//! - `install()`: our handlers, process-wide (after jit.rs gave the thread its fault stack).
//! - `reassert()`: our handlers back on top where a library of the program replaced them (it
//!   stays chained for what is not ours); the run monitor calls it periodically.
//! - `this_thread()`: the calling thread's fault stack (sigaltstack: a stack overflow in JIT
//!   code is still reported).

pub use imp::*;

/// What kind of fault (the OS's signal or exception, classified).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Segv,
    Bus,
    Ill,
    Fpe,
    Trap,
    Other,
    /// the hang watchdog's / MAKEPAD_RUN_COMPILE_WATCH's interrupt (thread::kick)
    Watchdog,
}

/// A fault as the policy sees it: where (pc, stack pointer, frame pointer) and what (the
/// faulting data address, the OS's number for it).
pub struct Fault {
    pub kind: Kind,
    /// the signal number (unix): reports print "signal N"
    pub sig: i32,
    pub pc: u64,
    pub sp: u64,
    pub fp: u64,
    pub addr: u64,
}

impl Fault {
    /// How reports name the OS event ("signal 11"; Windows "exception 0xc0000005").
    pub fn label(&self) -> String {
        if cfg!(windows) {
            format!("exception {:#x}", self.sig as u32)
        } else {
            format!("signal {}", self.sig)
        }
    }
}

/// What the OS half does with the fault (decided by jit::on_fault under the unit lock).
pub enum Action {
    /// nothing (an interrupt that found nothing to do)
    Ignore,
    /// not ours: the handler we replaced, else the default action
    PassOn,
    /// the process ends: the default action; the instruction faults again
    Die,
    /// continue at `pc` with the first two argument registers set (`leave(ctx, code)`)
    Resume { pc: u64, a0: u64, a1: u64 },
}

/// Hooks of the OS half the policy calls (macOS: write protection, chained handlers).
pub struct Hooks<'a> {
    /// runs once the unit lock is held, for faults the policy will look at
    pub prepare: &'a dyn Fn(),
    /// a library's handler is chained (faults outside JIT code go to it first)
    pub chained: bool,
    /// MAKEPAD_RUN_FAULT_REGS: a register dump of a fault the policy handles
    pub debug: &'a dyn Fn(),
}

#[cfg(unix)]
mod unix {
    pub const SIGILL: i32 = 4;
    pub const SIGTRAP: i32 = 5;
    pub const SIGFPE: i32 = 8;
    pub const SIGSEGV: i32 = 11;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub const SIGBUS: i32 = 7;
    #[cfg(target_os = "macos")]
    pub const SIGBUS: i32 = 10;

    pub fn kind(sig: i32) -> super::Kind {
        use super::Kind;
        match sig {
            SIGSEGV => Kind::Segv,
            SIGBUS => Kind::Bus,
            SIGILL => Kind::Ill,
            SIGFPE => Kind::Fpe,
            SIGTRAP => Kind::Trap,
            s if s == super::watchdog_signal() => Kind::Watchdog,
            _ => Kind::Other,
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::unix::{kind, SIGBUS, SIGFPE, SIGILL, SIGSEGV};
    use super::{Action, Fault, Hooks};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// SIGUSR1: the hang watchdog's signal
    pub const SIG_WATCHDOG: i32 = 10;
    /// the watchdog's signal handler runs on the looping thread itself
    pub const NEEDS_HANG_TRAMP: bool = false;

    extern "C" {
        fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
        fn sigaltstack(ss: *const StackT, old: *mut StackT) -> i32;
    }

    #[repr(C)]
    struct SigAction {
        handler: usize,
        mask: [u64; 16],
        flags: i32,
        pad: i32,
        restorer: usize,
    }

    #[repr(C)]
    struct StackT {
        sp: *mut u8,
        flags: i32,
        pad: i32,
        size: usize,
    }

    fn default_action(sig: i32) {
        let act = SigAction { handler: 0, mask: [0; 16], flags: 0, pad: 0, restorer: 0 };
        unsafe {
            sigaction(sig, &act, std::ptr::null_mut());
        }
    }

    extern "C" fn on_signal(sig: i32, info: *mut u8, uc: *mut u8) {
        unsafe {
            // the handler allocates: in the system heap, never in the faulting thread's arena (the fault may be inside its allocator)
            let _kept = crate::zone_alloc::kept();
            let gregs = uc.add(40) as *mut u64;
            // MAKEPAD_RUN_COMPILE_WATCH: the watcher asks a thread stuck in one step where it is
            if sig == SIG_WATCHDOG && crate::jit::WATCH_DUMP.load(std::sync::atomic::Ordering::Relaxed) {
                let (pc, fp) = (*gregs.add(16), *gregs.add(10));
                eprintln!("makepad-run watch: stuck thread {:#x} at pc {:#x} ({}) fp {:#x}", crate::os::thread::current(), pc, crate::jit::fn_name_at(pc), fp);
                return;
            }
            let f = Fault { kind: kind(sig), sig, pc: *gregs.add(16), sp: *gregs.add(15), fp: *gregs.add(10), addr: *(info.add(16) as *const u64) };
            let chained = CHAINED[sig as usize][0].load(Ordering::SeqCst) > 1;
            match crate::jit::on_fault(&f, &Hooks { prepare: &|| {}, chained, debug: &|| {} }) {
                Action::Ignore => {}
                Action::PassOn => {
                    if !chain(sig, info, uc) {
                        default_action(sig);
                    }
                }
                Action::Die => default_action(sig),
                Action::Resume { pc, a0, a1 } => {
                    *gregs.add(16) = pc;
                    *gregs.add(8) = a0;
                    *gregs.add(9) = a1;
                }
            }
        }
    }

    /// sigaltstack for the calling thread (64 KB)
    pub fn this_thread() {
        unsafe {
            let stack = crate::os::mem::alloc_rw(1 << 16);
            let ss = StackT { sp: stack as *mut u8, flags: 0, pad: 0, size: 1 << 16 };
            sigaltstack(&ss, std::ptr::null_mut());
        }
    }

    const OUR_SIGNALS: [i32; 5] = [SIGILL, SIGBUS, SIGFPE, SIG_WATCHDOG, SIGSEGV];
    const SA_SIGINFO: i32 = 4;
    /// SA_SIGINFO | SA_ONSTACK | SA_NODEFER
    const OUR_FLAGS: i32 = SA_SIGINFO | 0x0800_0000 | 0x4000_0000;

    pub fn install() {
        unsafe {
            for sig in OUR_SIGNALS {
                let act = SigAction { handler: on_signal as *const () as usize, mask: [0; 16], flags: OUR_FLAGS, pad: 0, restorer: 0 };
                sigaction(sig, &act, std::ptr::null_mut());
            }
        }
    }

    /// Handlers a library of the program installed over ours (Chromium's in cef_initialize, as on
    /// macOS): (handler, flags) per signal, called for signals that are not faults in JIT code.
    static CHAINED: [[AtomicUsize; 2]; 32] = [const { [const { AtomicUsize::new(0) }; 2] }; 32];

    /// Puts our handlers back on top where the program replaced them, keeping the replacement as
    /// the next handler in the chain (the run monitor calls this every 100 ms). Without it a fault
    /// in JIT code after CEF started was Chromium's (a bare core dump, no report).
    pub fn reassert() {
        unsafe {
            for sig in OUR_SIGNALS {
                let mut old = SigAction { handler: 0, mask: [0; 16], flags: 0, pad: 0, restorer: 0 };
                sigaction(sig, std::ptr::null(), &mut old);
                if old.handler != on_signal as *const () as usize {
                    CHAINED[sig as usize][0].store(old.handler, Ordering::SeqCst);
                    CHAINED[sig as usize][1].store(old.flags as usize, Ordering::SeqCst);
                    let act = SigAction { handler: on_signal as *const () as usize, mask: [0; 16], flags: OUR_FLAGS, pad: 0, restorer: 0 };
                    sigaction(sig, &act, std::ptr::null_mut());
                }
            }
        }
    }

    /// Passes a signal that is not ours to the handler we replaced, if any. False: none.
    unsafe fn chain(sig: i32, info: *mut u8, uc: *mut u8) -> bool {
        let h = CHAINED[sig as usize][0].load(Ordering::SeqCst);
        let flags = CHAINED[sig as usize][1].load(Ordering::SeqCst) as i32;
        if h <= 1 {
            // SIG_DFL / SIG_IGN
            return false;
        }
        unsafe {
            if flags & SA_SIGINFO != 0 {
                let f: extern "C" fn(i32, *mut u8, *mut u8) = std::mem::transmute(h);
                f(sig, info, uc);
            } else {
                let f: extern "C" fn(i32) = std::mem::transmute(h);
                f(sig);
            }
        }
        true
    }
}

#[cfg(target_os = "android")]
mod imp {
    use super::unix::{kind, SIGBUS, SIGFPE, SIGILL, SIGSEGV};
    use super::{Action, Fault, Hooks};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ART uses SIGUSR1. Resolve bionic's available RT range at installation.
    // NDK r28 signal.h declares __libc_current_sigrtmin / __libc_current_sigrtmax.
    static WATCHDOG: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
    pub fn watchdog_signal() -> i32 { WATCHDOG.load(Ordering::Relaxed) }

    /// LP64 bionic offsets, in bytes, derived from NDK 28.2.13676358 headers.
    /// uc_mcontext is 16-byte aligned: 40-byte prefix + 128-byte signal mask,
    /// rounded up to 176, followed by the sigcontext's fault_address.
    const OFFSETS: [usize; 16] = [
        0,   // 0 sa_flags: bits/signal_types.h:104
        8,   // 1 sa_handler: bits/signal_types.h:105-108 (pointer alignment)
        16,  // 2 sa_mask: bits/signal_types.h:109; asm-generic/signal.h:56-58
        24,  // 3 sa_restorer: bits/signal_types.h:110
        0,   // 4 ss_sp: asm-generic/signal.h:73
        8,   // 5 ss_flags: asm-generic/signal.h:74
        16,  // 6 ss_size: asm-generic/signal.h:75 (LP64 alignment)
        16,  // 7 si_addr: asm-generic/siginfo.h:49,81 (union aligned to 8)
        176, // 8 uc_mcontext: sys/ucontext.h:112-122; asm/sigcontext.h:17
        184, // 9 x0: aarch64-linux-android/asm/sigcontext.h:12-13
        192, // 10 x1: asm/sigcontext.h:13 (x0 + 8)
        416, // 11 fp (x29): asm/sigcontext.h:13 (x0 + 29*8)
        424, // 12 lr (x30): asm/sigcontext.h:13 (x0 + 30*8)
        432, // 13 sp: asm/sigcontext.h:14
        440, // 14 pc: asm/sigcontext.h:15
        32,  // 15 sizeof(sigaction): bits/signal_types.h:103-110
    ];
    /// the watchdog's signal handler runs on the looping thread itself
    pub const NEEDS_HANG_TRAMP: bool = false;

    extern "C" {
        fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
        fn sigaltstack(ss: *const StackT, old: *mut StackT) -> i32;
    }

    #[repr(C)]
    struct SigAction {
        flags: i32,
        pad: i32,
        handler: usize,
        mask: u64,
        restorer: usize,
    }

    #[repr(C)]
    struct StackT {
        sp: *mut u8,
        flags: i32,
        pad: i32,
        size: usize,
    }

    fn default_action(sig: i32) {
        let act = SigAction { handler: 0, mask: 0, flags: 0, pad: 0, restorer: 0 };
        unsafe {
            sigaction(sig, &act, std::ptr::null_mut());
        }
    }

    extern "C" fn on_signal(sig: i32, info: *mut u8, uc: *mut u8) {
        unsafe {
            // the handler allocates: in the system heap, never in the faulting thread's arena (the fault may be inside its allocator)
            let _kept = crate::zone_alloc::kept();
            let reg = |i: usize| uc.add(OFFSETS[i]) as *mut u64;
            // MAKEPAD_RUN_COMPILE_WATCH: the watcher asks a thread stuck in one step where it is
            if sig == watchdog_signal() && crate::jit::WATCH_DUMP.load(std::sync::atomic::Ordering::Relaxed) {
                let (pc, fp) = (*reg(14), *reg(11));
                eprintln!("makepad-run watch: stuck thread {:#x} at pc {:#x} ({}) fp {:#x}", crate::os::thread::current(), pc, crate::jit::fn_name_at(pc), fp);
                return;
            }
            let f = Fault { kind: kind(sig), sig, pc: *reg(14), sp: *reg(13), fp: *reg(11), addr: *(info.add(OFFSETS[7]) as *const u64) };
            let chained = CHAINED[sig as usize][0].load(Ordering::SeqCst) > 1;
            match crate::jit::on_fault(&f, &Hooks { prepare: &|| {}, chained, debug: &|| {} }) {
                Action::Ignore => {}
                Action::PassOn => {
                    if !chain(sig, info, uc) {
                        default_action(sig);
                    }
                }
                Action::Die => default_action(sig),
                Action::Resume { pc, a0, a1 } => {
                    *reg(14) = pc;
                    *reg(9) = a0;
                    *reg(10) = a1;
                }
            }
        }
    }

    /// sigaltstack for the calling thread (64 KB)
    pub fn this_thread() {
        unsafe {
            let stack = crate::os::mem::alloc_rw(1 << 16);
            let ss = StackT { sp: stack as *mut u8, flags: 0, pad: 0, size: 1 << 16 };
            sigaltstack(&ss, std::ptr::null_mut());
        }
    }

    fn our_signals() -> [i32; 5] { [SIGILL, SIGBUS, SIGFPE, watchdog_signal(), SIGSEGV] }
    const _: () = {
        assert!(std::mem::offset_of!(SigAction, flags) == OFFSETS[0]);
        assert!(std::mem::offset_of!(SigAction, handler) == OFFSETS[1]);
        assert!(std::mem::offset_of!(SigAction, mask) == OFFSETS[2]);
        assert!(std::mem::offset_of!(SigAction, restorer) == OFFSETS[3]);
        assert!(std::mem::offset_of!(StackT, sp) == OFFSETS[4]);
        assert!(std::mem::offset_of!(StackT, flags) == OFFSETS[5]);
        assert!(std::mem::offset_of!(StackT, size) == OFFSETS[6]);
        assert!(std::mem::size_of::<SigAction>() == OFFSETS[15]);
    };
    const SA_SIGINFO: i32 = 4;
    /// SA_SIGINFO | SA_ONSTACK | SA_NODEFER
    const OUR_FLAGS: i32 = SA_SIGINFO | 0x0800_0000 | 0x4000_0000;

    pub fn install() {
        // sigaction is deliberately the public libc entry: ART's libsigchain intercepts it
        // in an Activity process. Keep its chaining, then preserve prior user handlers.
        extern "C" {
            fn __libc_current_sigrtmin() -> i32;
            fn __libc_current_sigrtmax() -> i32;
        }
        unsafe {
            let signal = __libc_current_sigrtmin() + 4;
            assert!(signal <= __libc_current_sigrtmax(), "no watchdog RT signal");
            WATCHDOG.store(signal, Ordering::Relaxed);
            for sig in our_signals() {
                let act = SigAction { handler: on_signal as *const () as usize, mask: 0, flags: OUR_FLAGS, pad: 0, restorer: 0 };
                let mut old = SigAction { handler: 0, mask: 0, flags: 0, pad: 0, restorer: 0 };
                if sigaction(sig, &act, &mut old) == 0 && old.handler != on_signal as *const () as usize {
                    CHAINED[sig as usize][0].store(old.handler, Ordering::SeqCst);
                    CHAINED[sig as usize][1].store(old.flags as usize, Ordering::SeqCst);
                }
            }
        }
    }

    /// Handlers a library of the program installed over ours (Chromium's in cef_initialize, as on
    /// macOS): (handler, flags) per signal, called for signals that are not faults in JIT code.
    static CHAINED: [[AtomicUsize; 2]; 65] = [const { [const { AtomicUsize::new(0) }; 2] }; 65];

    /// Puts our handlers back on top where the program replaced them, keeping the replacement as
    /// the next handler in the chain (the run monitor calls this every 100 ms). Without it a fault
    /// in JIT code after CEF started was Chromium's (a bare core dump, no report).
    pub fn reassert() {
        unsafe {
            for sig in our_signals() {
                let mut old = SigAction { handler: 0, mask: 0, flags: 0, pad: 0, restorer: 0 };
                sigaction(sig, std::ptr::null(), &mut old);
                if old.handler != on_signal as *const () as usize {
                    CHAINED[sig as usize][0].store(old.handler, Ordering::SeqCst);
                    CHAINED[sig as usize][1].store(old.flags as usize, Ordering::SeqCst);
                    let act = SigAction { handler: on_signal as *const () as usize, mask: 0, flags: OUR_FLAGS, pad: 0, restorer: 0 };
                    sigaction(sig, &act, std::ptr::null_mut());
                }
            }
        }
    }

    /// Passes a signal that is not ours to the handler we replaced, if any. False: none.
    unsafe fn chain(sig: i32, info: *mut u8, uc: *mut u8) -> bool {
        let h = CHAINED[sig as usize][0].load(Ordering::SeqCst);
        let flags = CHAINED[sig as usize][1].load(Ordering::SeqCst) as i32;
        if h <= 1 {
            // SIG_DFL / SIG_IGN
            return false;
        }
        unsafe {
            if flags & SA_SIGINFO != 0 {
                let f: extern "C" fn(i32, *mut u8, *mut u8) = std::mem::transmute(h);
                f(sig, info, uc);
            } else {
                let f: extern "C" fn(i32) = std::mem::transmute(h);
                f(sig);
            }
        }
        true
    }
}

/// macOS (Apple Silicon): the Darwin arm64 ucontext; handlers a library installed over ours
/// (Chromium's in CefInitialize) stay chained.
#[cfg(target_os = "macos")]
mod imp {
    use super::unix::{kind, SIGBUS, SIGFPE, SIGILL, SIGSEGV, SIGTRAP};
    use super::{Action, Fault, Hooks};
    use crate::jit::{collect_frames, PanicInfo, UNIT};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// SIGUSR1 (Darwin numbering): the hang watchdog's signal
    pub const SIG_WATCHDOG: i32 = 30;
    /// the watchdog's signal handler runs on the looping thread itself
    pub const NEEDS_HANG_TRAMP: bool = false;
    const SIGABRT: i32 = 6;

    extern "C" {
        fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
        fn sigaltstack(ss: *const StackT, old: *mut StackT) -> i32;
        fn pthread_jit_write_protect_np(enabled: i32);
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

    const SA_ONSTACK: i32 = 0x1;
    const SA_NODEFER: i32 = 0x10;
    const SA_SIGINFO: i32 = 0x40;

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

    fn default_action(sig: i32) {
        let act = SigAction { handler: 0, mask: 0, flags: 0 };
        unsafe {
            sigaction(sig, &act, std::ptr::null_mut());
        }
    }

    /// SIGABRT (libc abort(): malloc's heap checks, Foundation/ObjC fatal errors, an assertion in
    /// a framework, std::process::abort) is not recoverable: report the thread and the JIT frames
    /// that led there, then let the abort proceed (default action re-raised).
    unsafe fn report_abort(uc: *mut u8) {
        let mc = *(uc.add(UC_MCONTEXT) as *const *mut u8);
        let reg = |off: usize| -> *mut u64 { mc.add(off) as *mut u64 };
        let (pc, fp) = (*reg(SS_PC), *reg(SS_FP));
        let tname = crate::os::thread::name(crate::os::thread::current());
        if UNIT.is_null() {
            eprintln!("makepad: SIGABRT on thread '{}' (no JIT unit)", tname);
            return;
        }
        let frames = collect_frames(fp, pc, false);
        let info = PanicInfo { kind: "abort".to_string(), message: format!("SIGABRT (abort() called) on thread '{}'", tname), site: u64::MAX, frames, fault_addr: 0, location: String::new() };
        // the report is a unit call: none when the compiler is busy (this very thread may be
        // the one aborting inside it)
        match crate::owner::call_within(crate::owner::Class::Job, 0, || (&*UNIT).report(&info)) {
            Some(r) => eprintln!("makepad: abort on thread '{}'\n  report: {}", tname, r),
            None => eprintln!("makepad: SIGABRT on thread '{}' while the compiler is busy", tname),
        }
    }

    /// MAKEPAD_RUN_COMPILE_WATCH: the watcher asks a thread stuck in one step where it is (no lock:
    /// that thread is the one holding it, stuck)
    unsafe fn watch_dump(uc: *mut u8) {
        let mc = *(uc.add(UC_MCONTEXT) as *const *mut u8);
        let reg = |off: usize| -> u64 { *(mc.add(off) as *const u64) };
        let (pc, lr, fp) = (reg(SS_PC), reg(SS + 240), reg(SS_FP));
        if !UNIT.is_null() {
            let nm = crate::jit::fn_name_at;
            eprintln!("makepad-run watch: stuck thread {:#x} at pc {:#x} ({}) lr {:#x} ({}) fp {:#x}; on_signal at {:#x}", crate::os::thread::current(), pc, nm(pc), lr, nm(lr), fp, on_signal as *const () as usize);
            let mut f = fp;
            for _ in 0..12 {
                if f == 0 || f % 8 != 0 {
                    break;
                }
                let ret = *((f + 8) as *const u64);
                eprintln!("makepad-run watch:   frame ret {:#x} ({})", ret, nm(ret));
                f = *(f as *const u64);
            }
        }
    }

    /// MAKEPAD_RUN_FAULT_REGS: registers and the call sequence around a fault (arm64 encodings).
    unsafe fn fault_regs(uc: *mut u8) {
        if std::env::var_os("MAKEPAD_RUN_FAULT_REGS").is_none() {
            return;
        }
        let mc = *(uc.add(UC_MCONTEXT) as *const *mut u8);
        let reg = |off: usize| -> *mut u64 { mc.add(off) as *mut u64 };
        let u = &*UNIT;
        let (pc, fp) = (*reg(SS_PC), *reg(SS_FP));
        let lr = *reg(SS + 240);
        let x16 = *reg(SS + 16 * 8);
        let x17 = *reg(SS + 17 * 8);
        let nm = |a: u64| u.find_fn(a).map(|id| format!("{}#{}", u.fns[id as usize].name, id)).unwrap_or("-".to_string());
        let mut slots = Vec::new();
        for id in 0..u.fns.len() {
            let t = *(u.slots.table(id as u32) as *const u64);
            if t == x16 || t == pc {
                slots.push(format!("{}={}", id, u.fns[id].name));
            }
        }
        let ins = |a: u64| if a >= 8 { *((a) as *const u32) } else { 0 };
        eprintln!("makepad-run fault-regs: pc {:#x} ({}) lr {:#x} ({}) fp {:#x} x16 {:#x} ({}) x17 {:#x}; table slots holding x16/pc: {:?}; insts at lr-12..lr: {:08x} {:08x} {:08x}", pc, nm(pc), lr, nm(lr), fp, x16, nm(x16), x17, slots, ins(lr - 12), ins(lr - 8), ins(lr - 4));
        if let Some(id) = u.find_fn(lr) {
            let f = &u.fns[id as usize];
            eprintln!("makepad-run fault-regs: caller {} code {:#x} len {} lr off {:#x}", f.name, f.code, f.code_len, lr.wrapping_sub(f.code));
        }
        // the call's slot address from the movz/movk sequence before `ldr x16, [x16]; blr x16`
        let mut a: u64 = 0;
        for k in (3..8).rev() {
            let w = ins(lr - 4 * k);
            if w & 0x1f == 16 {
                let hw = ((w >> 21) & 3) as u64;
                let imm = ((w >> 5) & 0xffff) as u64;
                match w >> 23 {
                    0x1a5 => a = imm << (16 * hw),
                    0x1e5 => a = (a & !(0xffff << (16 * hw))) | (imm << (16 * hw)),
                    _ => {}
                }
            }
        }
        if let Some((id, 0)) = u.slots.find(2, a).filter(|(id, _)| (*id as usize) < u.fns.len()) {
            let id = id as usize;
            let t = *(a as *const u64);
            let f = &u.fns[id];
            eprintln!("makepad-run fault-regs: called slot {} = {} (compiled {}, code {:#x}) table now {:#x} ({})", id, f.name, f.compiled, f.code, t, nm(t));
        } else {
            eprintln!("makepad-run fault-regs: slot addr {:#x} not in the table", a);
        }
    }

    extern "C" fn on_signal(sig: i32, info: *mut u8, uc: *mut u8) {
        unsafe {
            // the handler allocates: in the system heap, never in the faulting thread's arena (the fault may be inside its allocator)
            let _kept = crate::zone_alloc::kept();
            if sig == SIGABRT {
                report_abort(uc);
                // back to the default action: abort() raises again and the process ends
                default_action(SIGABRT);
                return;
            }
            if sig == SIG_WATCHDOG && crate::jit::WATCH_DUMP.load(Ordering::Relaxed) {
                watch_dump(uc);
                return;
            }
            let mc = *(uc.add(UC_MCONTEXT) as *const *mut u8);
            let reg = |off: usize| -> *mut u64 { mc.add(off) as *mut u64 };
            let f = Fault { kind: kind(sig), sig, pc: *reg(SS_PC), sp: *reg(SS_SP), fp: *reg(SS_FP), addr: *(info.add(SI_ADDR) as *const u64) };
            // the program may fault while it has its own MAP_JIT pages in write mode
            // (pthread_jit_write_protect_np(0)); recovery continues in execute mode
            let prepare = || pthread_jit_write_protect_np(1);
            let debug = || fault_regs(uc);
            let chained = CHAINED[sig as usize][0].load(Ordering::SeqCst) > 1;
            match crate::jit::on_fault(&f, &Hooks { prepare: &prepare, chained, debug: &debug }) {
                Action::Ignore => {}
                Action::PassOn => {
                    if !chain(sig, info, uc) {
                        default_action(sig);
                    }
                }
                Action::Die => default_action(sig),
                Action::Resume { pc, a0, a1 } => {
                    *reg(SS_PC) = pc;
                    *reg(SS) = a0;
                    *reg(SS + 8) = a1;
                }
            }
        }
    }

    /// sigaltstack for the calling thread (64 KB)
    pub fn this_thread() {
        unsafe {
            let size = 1 << 16;
            let stack = crate::os::mem::alloc_rw(size);
            let ss = StackT { sp: stack as *mut u8, size, flags: 0 };
            sigaltstack(&ss, std::ptr::null_mut());
        }
    }

    const OUR_SIGNALS: [i32; 7] = [SIGILL, SIGTRAP, SIGFPE, SIGBUS, SIGSEGV, SIGABRT, SIG_WATCHDOG];

    pub fn install() {
        unsafe {
            for sig in OUR_SIGNALS {
                let act = SigAction { handler: on_signal as *const () as usize, mask: 0, flags: SA_SIGINFO | SA_ONSTACK | SA_NODEFER };
                sigaction(sig, &act, std::ptr::null_mut());
            }
        }
    }

    /// Handlers a library of the program installed over ours (Chromium sets its own for SIGSEGV,
    /// SIGBUS, .. in CefInitialize): (handler, flags) per signal, called for signals that are not
    /// faults in JIT code -- the library keeps working, and faults in JIT code are still reported.
    static CHAINED: [[AtomicUsize; 2]; 32] = [const { [const { AtomicUsize::new(0) }; 2] }; 32];

    /// Puts our handlers back on top where the program replaced them, keeping the replacement as
    /// the next handler in the chain (the run monitor calls this every 100 ms).
    pub fn reassert() {
        unsafe {
            for sig in OUR_SIGNALS {
                let mut old = SigAction { handler: 0, mask: 0, flags: 0 };
                sigaction(sig, std::ptr::null(), &mut old);
                if old.handler != on_signal as *const () as usize {
                    CHAINED[sig as usize][0].store(old.handler, Ordering::SeqCst);
                    CHAINED[sig as usize][1].store(old.flags as usize, Ordering::SeqCst);
                    let act = SigAction { handler: on_signal as *const () as usize, mask: 0, flags: SA_SIGINFO | SA_ONSTACK | SA_NODEFER };
                    sigaction(sig, &act, std::ptr::null_mut());
                }
            }
        }
    }

    /// Passes a signal that is not ours to the handler we replaced, if any. False: none.
    unsafe fn chain(sig: i32, info: *mut u8, uc: *mut u8) -> bool {
        let h = CHAINED[sig as usize][0].load(Ordering::SeqCst);
        let flags = CHAINED[sig as usize][1].load(Ordering::SeqCst) as i32;
        if h <= 1 {
            // SIG_DFL / SIG_IGN
            return false;
        }
        unsafe {
            if flags & SA_SIGINFO != 0 {
                let f: extern "C" fn(i32, *mut u8, *mut u8) = std::mem::transmute(h);
                f(sig, info, uc);
            } else {
                let f: extern "C" fn(i32) = std::mem::transmute(h);
                f(sig);
            }
        }
        true
    }
}

/// Windows: a vectored exception handler (first in line) applies the policy to faults whose pc
/// is in JIT code; everything else continues the normal search, so libraries keep their own
/// first-chance handling (RPC, C++ throws, OutputDebugString). A fault outside JIT code that
/// nobody handles reaches our unhandled-exception filter, which applies the full policy (it is
/// contained when the thread has a recovery point, as on unix). The watchdog interrupt comes
/// from the watching thread (`interrupt`): it suspends the target and reads its context there.
#[cfg(windows)]
mod imp {
    use super::{Action, Fault, Hooks, Kind};
    use crate::os::win32::*;

    /// never delivered as an exception on Windows (the watchdog suspends the thread instead);
    /// kept so Fault::sig values never collide with it
    pub const SIG_WATCHDOG: i32 = -1;
    /// no signals: the watcher redirects the thread into jit::HANG_TRAMP (`interrupt`)
    pub const NEEDS_HANG_TRAMP: bool = true;

    const EXCEPTION_ACCESS_VIOLATION: u32 = 0xc000_0005;
    const EXCEPTION_IN_PAGE_ERROR: u32 = 0xc000_0006;
    const EXCEPTION_ILLEGAL_INSTRUCTION: u32 = 0xc000_001d;
    const EXCEPTION_INT_DIVIDE_BY_ZERO: u32 = 0xc000_0094;
    const EXCEPTION_INT_OVERFLOW: u32 = 0xc000_0095;
    const EXCEPTION_PRIV_INSTRUCTION: u32 = 0xc000_0096;
    const EXCEPTION_STACK_OVERFLOW: u32 = 0xc000_00fd;
    const EXCEPTION_BREAKPOINT: u32 = 0x8000_0003;

    fn kind(code: u32) -> Option<Kind> {
        Some(match code {
            EXCEPTION_ACCESS_VIOLATION | EXCEPTION_STACK_OVERFLOW => Kind::Segv,
            EXCEPTION_IN_PAGE_ERROR => Kind::Bus,
            EXCEPTION_ILLEGAL_INSTRUCTION | EXCEPTION_PRIV_INSTRUCTION => Kind::Ill,
            EXCEPTION_INT_DIVIDE_BY_ZERO | EXCEPTION_INT_OVERFLOW => Kind::Fpe,
            EXCEPTION_BREAKPOINT => Kind::Trap,
            _ => return None,
        })
    }

    unsafe fn handle(ep: *mut ExceptionPointers, chained: bool) -> i32 {
        let rec = &*(*ep).record;
        let ctx = &mut *(*ep).context;
        let k = match kind(rec.code) {
            Some(k) => k,
            None => return EXCEPTION_CONTINUE_SEARCH,
        };
        let pc = ctx.get(CTX_RIP);
        if trace() {
            trace_fault(rec.code, pc, chained);
        }
        if chained && !crate::jit::in_jit_code(pc) {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        // the data address of an access violation / in-page error (ExceptionInformation[1]);
        // a stack overflow reports the stack pointer's page
        let addr = match rec.code {
            EXCEPTION_ACCESS_VIOLATION | EXCEPTION_IN_PAGE_ERROR if rec.n_params >= 2 => rec.info[1] as u64,
            EXCEPTION_STACK_OVERFLOW => ctx.get(CTX_RSP).wrapping_sub(8),
            _ => pc,
        };
        let f = Fault { kind: k, sig: rec.code as i32, pc, sp: ctx.get(CTX_RSP), fp: ctx.get(CTX_RBP), addr };
        match crate::jit::on_fault(&f, &Hooks { prepare: &|| {}, chained: false, debug: &|| {} }) {
            Action::Ignore | Action::PassOn | Action::Die => EXCEPTION_CONTINUE_SEARCH,
            Action::Resume { pc, a0, a1 } => {
                // leave(ctx, code) is Relax's convention: rdi, rsi
                ctx.set(CTX_RIP, pc);
                ctx.set(CTX_RDI, a0);
                ctx.set(CTX_RSI, a1);
                EXCEPTION_CONTINUE_EXECUTION
            }
        }
    }

    /// MAKEPAD_RUN_FAULT_TRACE=1: every exception the handlers see, with the module holding its pc
    /// (diagnostics for faults outside JIT code; no debugger needed).
    fn trace() -> bool {
        static T: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);
        let t = T.load(std::sync::atomic::Ordering::Relaxed);
        if t != 2 {
            return t == 1;
        }
        let on = std::env::var_os("MAKEPAD_RUN_FAULT_TRACE").is_some();
        T.store(on as u8, std::sync::atomic::Ordering::Relaxed);
        on
    }

    unsafe fn trace_fault(code: u32, pc: u64, first: bool) {
        let mut m: HANDLE = 0;
        let mut name = [0u16; 260];
        let mut n = 0;
        // GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | UNCHANGED_REFCOUNT
        if GetModuleHandleExW(0x4 | 0x2, pc as *const u16, &mut m) != 0 {
            n = GetModuleFileNameW(m, name.as_mut_ptr(), 260) as usize;
        }
        let module = String::from_utf16_lossy(&name[..n]);
        let msg = format!("makepad-run fault-trace: {} exception {:#x} at pc {:#x} = {}+{:#x}{}\n", if first { "first-chance" } else { "unhandled" }, code, pc, if n > 0 { module.as_str() } else { "?" }, pc.wrapping_sub(m as u64), if crate::jit::in_jit_code(pc) { " (JIT code)" } else { "" });
        let mut w = 0u32;
        WriteFile(GetStdHandle(-12i32 as u32), msg.as_ptr(), msg.len() as u32, &mut w, std::ptr::null_mut());
    }

    extern "system" fn vectored(ep: *mut ExceptionPointers) -> i32 {
        unsafe { handle(ep, true) }
    }

    extern "system" fn unhandled(ep: *mut ExceptionPointers) -> i32 {
        unsafe { handle(ep, false) }
    }

    /// The stack-overflow handler runs in the thread's guaranteed stack area (64 KB).
    pub fn this_thread() {
        let mut size: u32 = 1 << 16;
        unsafe {
            SetThreadStackGuarantee(&mut size);
        }
    }

    pub fn install() {
        unsafe {
            AddVectoredExceptionHandler(1, vectored);
            SetUnhandledExceptionFilter(Some(unhandled));
        }
    }

    pub fn reassert() {}

    /// Suspends thread `t` and reads its context. MAKEPAD_RUN_COMPILE_WATCH: prints where it is. Hang
    /// watchdog: a thread running JIT code continues in jit::HANG_TRAMP as if the interrupted pc
    /// had called it (the return address pushed on its stack); there, on the thread itself, the
    /// policy decides as a watchdog signal's handler does.
    pub fn interrupt(t: u64) {
        unsafe {
            let h = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_SET_CONTEXT, 0, t as u32);
            if h == 0 {
                return;
            }
            let mut ctx = Context::new(CONTEXT_CONTROL | CONTEXT_INTEGER);
            if SuspendThread(h) != u32::MAX {
                if GetThreadContext(h, &mut *ctx) != 0 {
                    let (pc, fp, sp) = (ctx.get(CTX_RIP), ctx.get(CTX_RBP), ctx.get(CTX_RSP));
                    if crate::jit::WATCH_DUMP.load(std::sync::atomic::Ordering::Relaxed) {
                        eprintln!("makepad-run watch: stuck thread {:#x} at pc {:#x} ({}) fp {:#x}", t, pc, crate::jit::fn_name_at(pc), fp);
                    } else {
                        let tramp = crate::jit::HANG_TRAMP.load(std::sync::atomic::Ordering::Relaxed);
                        if tramp != 0 && crate::jit::in_jit_code(pc) && pc != tramp {
                            let nsp = sp - 8;
                            *(nsp as *mut u64) = pc;
                            ctx.set(CTX_RSP, nsp);
                            ctx.set(CTX_RIP, tramp);
                            SetThreadContext(h, &*ctx);
                        }
                    }
                }
                ResumeThread(h);
            }
            CloseHandle(h);
        }
    }
}

#[cfg(not(target_os = "android"))]
pub fn watchdog_signal() -> i32 { SIG_WATCHDOG }
