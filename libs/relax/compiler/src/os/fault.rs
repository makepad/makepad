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
    /// the hang watchdog's / RELAX_COMPILE_WATCH's interrupt (thread::kick)
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
    /// the link register (arm64 x30: the return address of a leaf); 0 on x86_64
    pub lr: u64,
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
    /// RELAX_FAULT_REGS: a register dump of a fault the policy handles
    pub debug: &'a dyn Fn(),
}

#[cfg(unix)]
mod unix {
    pub const SIGILL: i32 = 4;
    pub const SIGTRAP: i32 = 5;
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    pub const SIGABRT: i32 = 6;
    pub const SIGFPE: i32 = 8;
    pub const SIGSEGV: i32 = 11;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub const SIGBUS: i32 = 7;
    #[cfg(target_vendor = "apple")]
    pub const SIGBUS: i32 = 10;

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    pub use abort::{report_abort, start_reporter};

    /// Raises SIGABRT again on this thread, from its SIGABRT handler once the default action is
    /// back: the signal stays blocked until the handler returns, and the process ends there.
    /// abort() would raise it again itself; a SIGABRT from outside would not, and the process
    /// went on after its report. Async-signal-safe.
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    pub unsafe fn raise_abort() {
        extern "C" {
            fn raise(sig: i32) -> i32;
        }
        raise(SIGABRT);
    }

    /// SIGABRT's report, made off the aborting thread. glibc's malloc checks call abort() with
    /// the thread's arena locked, and a report on that thread reaches glibc's malloc again:
    /// besides its own blocks, its first block in the compiler arena and its first wait on the
    /// compiler's owner register thread-local destructors (glibc's __cxa_thread_atexit_impl
    /// callocs). 11-09: three test threads of a JIT waited on that lock in this handler for 10
    /// minutes, and no report came. The aborting thread hands its pc, frames and name to the
    /// reporter thread, which did all of that once at its start, and waits for the report with
    /// atomics and futex calls only.
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    mod abort {
        use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering::{AcqRel, Acquire, Relaxed, Release}};

        /// A timespec (`clock_gettime`).
        #[cfg(target_os = "linux")]
        #[repr(C)]
        struct Ts {
            s: i64,
            ns: i64,
        }
        extern "C" {
            fn pthread_self() -> usize;
            fn pthread_getname_np(t: usize, name: *mut u8, len: usize) -> i32;
            fn getpid() -> i32;
            #[cfg(target_os = "linux")]
            fn syscall(n: i64, ...) -> i64;
            #[cfg(target_os = "linux")]
            fn pthread_getcpuclockid(t: usize, clock: *mut i32) -> i32;
            #[cfg(target_os = "linux")]
            fn clock_gettime(clock: i32, t: *mut Ts) -> i32;
            #[cfg(target_vendor = "apple")]
            fn pthread_mach_thread_np(t: usize) -> u32;
            #[cfg(target_vendor = "apple")]
            fn thread_info(port: u32, flavor: u32, info: *mut u32, count: *mut u32) -> i32;
            #[cfg(target_vendor = "apple")]
            fn __ulock_wait(op: u32, addr: *mut u8, value: u64, timeout_us: u32) -> i32;
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        const SYS_FUTEX: i64 = 202;
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        const SYS_FUTEX: i64 = 98;
        /// FUTEX_WAIT_PRIVATE
        #[cfg(target_os = "linux")]
        const FUTEX_WAIT: i32 = 128;
        /// UL_COMPARE_AND_WAIT | ULF_NO_ERRNO
        #[cfg(target_vendor = "apple")]
        const UL_WAIT: u32 = 0x0100_0001;

        const FRAMES: usize = 64;
        /// How long an abort's report waits for the compiler's owner to start its lookups.
        const OWNER_WAIT_MS: u64 = 1000;
        /// How often an aborting thread looks at the report it waits for, and how long nothing
        /// may move before it ends the process without it (longer than the one bounded wait a
        /// report has, OWNER_WAIT_MS).
        const POLL_MS: u64 = 100;
        const STUCK_MS: u64 = OWNER_WAIT_MS + 2000;
        /// `STEP`: where the report is. An aborting thread reads it.
        const STEP_WAITING: u32 = 0;
        const STEP_ASK_OWNER: u32 = 1;
        const STEP_ON_OWNER: u32 = 2;
        const STEP_WRITE: u32 = 3;
        static STEP: AtomicU32 = AtomicU32::new(STEP_WAITING);
        /// The CPU clocks (`self_cpu_clock`) of the reporter and, once its lookups run there, of
        /// the compiler's owner. 0: none.
        static REPORTER_CLOCK: AtomicU64 = AtomicU64::new(0);
        static OWNER_CLOCK: AtomicU64 = AtomicU64::new(0);
        /// `HANDOVER.state`: the reporter waits; an aborting thread fills the hand-over; it is
        /// posted; the report is out (the process ends: one report per process)
        const IDLE: u32 = 0;
        const FILLING: u32 = 1;
        const POSTED: u32 = 2;
        const DONE: u32 = 3;
        struct Handover {
            state: AtomicU32,
            unit: AtomicUsize,
            pc: AtomicU64,
            n: AtomicUsize,
            frames: [AtomicU64; FRAMES],
            name: [AtomicU8; 64],
        }
        static HANDOVER: Handover = Handover {
            state: AtomicU32::new(IDLE),
            unit: AtomicUsize::new(0),
            pc: AtomicU64::new(0),
            n: AtomicUsize::new(0),
            frames: [const { AtomicU64::new(0) }; FRAMES],
            name: [const { AtomicU8::new(0) }; 64],
        };
        /// The reporter's pthread once it is ready (0 before), and its process (a forked child
        /// has no reporter).
        static REPORTER: AtomicUsize = AtomicUsize::new(0);
        static REPORTER_PID: AtomicU32 = AtomicU32::new(0);

        use crate::os::progsig::{futex_wait, futex_wake};
        /// `futex_wait` for at most `ms`.
        fn futex_wait_ms(w: &AtomicU32, v: u32, ms: u64) {
            #[cfg(target_os = "linux")]
            {
                let t = [(ms / 1000) as i64, (ms % 1000 * 1_000_000) as i64];
                unsafe { syscall(SYS_FUTEX, w.as_ptr(), FUTEX_WAIT, v, t.as_ptr()) };
            }
            #[cfg(target_vendor = "apple")]
            unsafe { __ulock_wait(UL_WAIT, w.as_ptr() as *mut u8, v as u64, (ms * 1000) as u32) };
        }
        /// The calling thread's CPU clock, for `cpu_ns` on any thread, also once this one ended.
        fn self_cpu_clock() -> u64 {
            #[cfg(target_os = "linux")]
            unsafe {
                let mut clock = 0i32;
                if pthread_getcpuclockid(pthread_self(), &mut clock) == 0 { clock as u32 as u64 } else { 0 }
            }
            #[cfg(target_vendor = "apple")]
            unsafe { pthread_mach_thread_np(pthread_self()) as u64 }
        }
        /// The CPU time a thread has used (0 when it cannot be read: it ended). Async-signal-safe.
        fn cpu_ns(clock: u64) -> u64 {
            if clock == 0 {
                return 0;
            }
            #[cfg(target_os = "linux")]
            unsafe {
                let mut t = Ts { s: 0, ns: 0 };
                if clock_gettime(clock as u32 as i32, &mut t) == 0 { t.s as u64 * 1_000_000_000 + t.ns as u64 } else { 0 }
            }
            #[cfg(target_vendor = "apple")]
            unsafe {
                // THREAD_BASIC_INFO: user_time, system_time (seconds, microseconds), then the rest
                let mut info = [0u32; 10];
                let mut count = 10u32;
                if thread_info(clock as u32, 3, info.as_mut_ptr(), &mut count) != 0 {
                    return 0;
                }
                (info[0] as u64 + info[2] as u64) * 1_000_000_000 + (info[1] as u64 + info[3] as u64) * 1000
            }
        }
        fn wait_for(w: &AtomicU32, want: u32) {
            loop {
                let s = w.load(Acquire);
                if s == want {
                    return;
                }
                futex_wait(w, s);
            }
        }

        /// Starts the reporter thread (once; with the compiler's owner).
        pub fn start_reporter() {
            static STARTED: std::sync::Once = std::sync::Once::new();
            STARTED.call_once(|| {
                let _k = crate::zone_alloc::kept();
                let _ = std::thread::Builder::new().name("run-abort-report".into()).stack_size(4 << 20).spawn(reporter);
            });
        }

        /// RELAX_ABORT_HOLD (tests/abort_heap's held and stuck cases). `locks`: once an
        /// abort's report is on the owner, a thread takes every lock it could wait for and keeps
        /// them (`holder`); the report goes on with what it reaches and names what it left out.
        /// `stuck`: the lookups on the owner never return; the aborting thread ends the process.
        static HOLD: AtomicU32 = AtomicU32::new(HOLD_OFF);
        const HOLD_OFF: u32 = 0;
        const HOLD_ARMED: u32 = 1;
        const HOLD_GO: u32 = 2;
        const HOLD_HELD: u32 = 3;
        const HOLD_STUCK: u32 = 4;

        fn reporter() {
            let _z = crate::zone_alloc::compiler_thread();
            // what a report would register in the system heap, registered now: the fault stack and
            // bounds, a block in the compiler arena, the wait row of a request to the owner
            crate::jit::ensure_altstack();
            drop(std::hint::black_box(String::from("run-abort-report")));
            let _ = crate::owner::call_within(crate::owner::Class::Job, 0, || ());
            match std::env::var_os("RELAX_ABORT_HOLD").as_deref().and_then(|v| v.to_str()) {
                Some("locks") => {
                    HOLD.store(HOLD_ARMED, Release);
                    let _ = std::thread::Builder::new().name("run-abort-hold".into()).spawn(holder);
                }
                Some("stuck") => HOLD.store(HOLD_STUCK, Release),
                _ => {}
            }
            REPORTER_CLOCK.store(self_cpu_clock(), Release);
            REPORTER_PID.store(unsafe { getpid() } as u32, Release);
            REPORTER.store(unsafe { pthread_self() }, Release);
            wait_for(&HANDOVER.state, POSTED);
            serve();
            HANDOVER.state.store(DONE, Release);
            futex_wake(&HANDOVER.state);
        }

        fn holder() {
            wait_for(&HOLD, HOLD_GO);
            let up = HANDOVER.unit.load(Acquire) as *mut crate::jit::Unit;
            crate::jit::hold_report_locks(up, || {
                let _stderr = std::io::stderr().lock();
                crate::tui::hold_tee(|| {
                    HOLD.store(HOLD_HELD, Release);
                    futex_wake(&HOLD);
                    // kept until the process ends
                    loop {
                        futex_wait(&HOLD, HOLD_HELD);
                    }
                })
            });
        }

        /// The posted abort's report, on the reporter thread: its lookups on the owner, where
        /// none waits for a lock (Unit::report_nowait), then out with what the session log's
        /// pipe still holds (glibc's "malloc(): ..." line), lock-free (tui fatal_drain). The
        /// process dies when the aborting thread's handler returns.
        fn serve() {
            use crate::jit::{PanicInfo, Unit};
            let up = HANDOVER.unit.load(Acquire) as *mut Unit;
            let pc = HANDOVER.pc.load(Relaxed);
            let n = HANDOVER.n.load(Relaxed).min(FRAMES);
            let raw: Vec<u64> = HANDOVER.frames[..n].iter().map(|f| f.load(Relaxed)).collect();
            let name: Vec<u8> = HANDOVER.name.iter().map(|c| c.load(Relaxed)).take_while(|&c| c != 0).collect();
            let tname = String::from_utf8_lossy(&name).into_owned();
            let message = format!("SIGABRT (abort() called) on thread '{}'", tname);
            // what the reporter knows without the unit: the frames by address
            let by_address = |why: String| {
                let pcs: Vec<String> = std::iter::once(pc).chain(raw.iter().map(|r| r.wrapping_sub(1))).map(|p| format!("{{\"pc\":\"{:#x}\"}}", p)).collect();
                format!("{{\"kind\":\"abort\",\"message\":{:?},\"build\":{:?},\"backtrace\":[{}],\"skipped\":[{:?}]}}", message, crate::jit::PROVENANCE, pcs.join(","), why)
            };
            let report = if up.is_null() {
                by_address("unit: no JIT unit, frames by address only".to_string())
            } else {
                let unit_here = up as usize;
                let on_owner = || {
                    OWNER_CLOCK.store(self_cpu_clock(), Release);
                    STEP.store(STEP_ON_OWNER, Release);
                    match HOLD.load(Acquire) {
                        HOLD_ARMED => {
                            HOLD.store(HOLD_GO, Release);
                            futex_wake(&HOLD);
                            wait_for(&HOLD, HOLD_HELD);
                        }
                        HOLD_STUCK => loop {
                            futex_wait(&HOLD, HOLD_STUCK);
                        },
                        _ => {}
                    }
                    let up = unit_here as *mut Unit;
                    let mut skipped = Vec::new();
                    let frames = crate::jit::frames_from_raw_in(up, pc, false, &raw, Some(&mut skipped));
                    let info = PanicInfo { kind: "abort".to_string(), message: message.clone(), site: u64::MAX, frames, fault_addr: 0, location: String::new() };
                    unsafe { (&*up).report_nowait(&info, skipped) }
                };
                // the unit's tables are the owner's: the lookups run there. A busy owner (in a
                // compile; the aborting thread itself) leaves them out.
                STEP.store(STEP_ASK_OWNER, Release);
                match crate::owner::call_within(crate::owner::Class::Job, OWNER_WAIT_MS, on_owner) {
                    Some(r) => r,
                    None => by_address(format!("owner: busy for {} ms ({}), frames by address only", OWNER_WAIT_MS, crate::owner::serving_detail())),
                }
            };
            let text = format!("relax: abort on thread '{}'\n  report: {}\n", tname, report);
            STEP.store(STEP_WRITE, Release);
            crate::tui::fatal_drain(text.as_bytes());
        }

        /// Reports SIGABRT (on the aborting thread, before its handler restores the default
        /// action): a line on the real stderr, then the reporter's report. Async-signal-safe.
        pub unsafe fn report_abort(pc: u64, fp: u64) {
            let mut name = [0u8; 64];
            pthread_getname_np(pthread_self(), name.as_mut_ptr(), name.len());
            let len = name.iter().position(|&c| c == 0).unwrap_or(0);
            let mut line = [0u8; 128];
            let mut k = 0;
            for &c in b"relax: SIGABRT on thread '".iter().chain(&name[..len]).chain(b"'\n") {
                if k < line.len() {
                    line[k] = c;
                    k += 1;
                }
            }
            crate::os::fault::raw_stderr(&line[..k]);
            let r = REPORTER.load(Acquire);
            if r == 0 || r == pthread_self() || REPORTER_PID.load(Acquire) != getpid() as u32 {
                return;
            }
            if HANDOVER.state.compare_exchange(IDLE, FILLING, AcqRel, Acquire).is_ok() {
                HANDOVER.unit.store(crate::jit::cur_unit() as usize, Relaxed);
                HANDOVER.pc.store(pc, Relaxed);
                let mut raw = [0u64; FRAMES];
                let n = crate::jit::raw_frames_here(fp, &mut raw);
                for (slot, &f) in HANDOVER.frames.iter().zip(&raw[..n]) {
                    slot.store(f, Relaxed);
                }
                HANDOVER.n.store(n, Relaxed);
                for (slot, &c) in HANDOVER.name.iter().zip(&name) {
                    slot.store(c, Relaxed);
                }
                HANDOVER.state.store(POSTED, Release);
                futex_wake(&HANDOVER.state);
            }
            // a second aborting thread waits for the first one's report too: a return ends the
            // process
            wait_for_report();
        }

        /// An aborting thread's wait for the report: until it is out, or until nothing can bring
        /// it, so that a reporter blocked for good (on something this thread holds) does not keep
        /// the process alive. Not a cap on the report: its step or the CPU time of the threads
        /// making it moving keeps the wait going, however long it takes. Async-signal-safe.
        fn wait_for_report() {
            fn progress() -> (u32, u64) {
                let step = STEP.load(Acquire);
                let owner = if step == STEP_ON_OWNER { cpu_ns(OWNER_CLOCK.load(Acquire)) } else { 0 };
                (step, cpu_ns(REPORTER_CLOCK.load(Acquire)).wrapping_add(owner))
            }
            let mut last = progress();
            let mut still = 0;
            loop {
                let s = HANDOVER.state.load(Acquire);
                if s == DONE {
                    return;
                }
                futex_wait_ms(&HANDOVER.state, s, POLL_MS);
                let now = progress();
                if now != last {
                    last = now;
                    still = 0;
                    continue;
                }
                still += POLL_MS;
                if still >= STUCK_MS {
                    report_stuck(now.0);
                    return;
                }
            }
        }

        /// What the reporter would have had: the frames by address, with where it stopped.
        fn report_stuck(step: u32) {
            let raw = crate::os::fault::raw_stderr;
            raw(b"relax: the abort's report is stuck ");
            raw(match step {
                STEP_ASK_OWNER => b"asking the compiler's owner",
                STEP_ON_OWNER => b"in its lookups on the compiler's owner",
                STEP_WRITE => b"writing itself out",
                _ => b"before it started",
            });
            raw(b", nothing moves: the process ends without it; frames by address:");
            let hex = |word: u64| {
                let mut b = *b" 0x0000000000000000";
                for (i, c) in b[3..].iter_mut().enumerate() {
                    *c = b"0123456789abcdef"[(word >> (60 - 4 * i)) as usize & 15];
                }
                raw(&b);
            };
            hex(HANDOVER.pc.load(Relaxed));
            for f in &HANDOVER.frames[..HANDOVER.n.load(Relaxed).min(FRAMES)] {
                hex(f.load(Relaxed));
            }
            raw(b"\n");
        }
    }

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
    use super::unix::{kind, raise_abort, report_abort, SIGABRT, SIGBUS, SIGFPE, SIGILL, SIGSEGV};
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
    pub(crate) struct SigAction {
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
            if sig == SIGABRT {
                report_abort(*gregs.add(16), *gregs.add(10));
                default_action(SIGABRT);
                raise_abort();
                return;
            }
            // RELAX_COMPILE_WATCH: the watcher asks a thread stuck in one step where it is
            if sig == SIG_WATCHDOG && crate::jit::WATCH_DUMP.load(std::sync::atomic::Ordering::Relaxed) {
                let (pc, fp) = (*gregs.add(16), *gregs.add(10));
                eprintln!("relax watch: stuck thread {:#x} at pc {:#x} ({}) fp {:#x}", crate::os::thread::current(), pc, crate::jit::fn_name_at(pc), fp);
                return;
            }
            let f = Fault { kind: kind(sig), sig, pc: *gregs.add(16), sp: *gregs.add(15), fp: *gregs.add(10), addr: *(info.add(16) as *const u64), lr: 0 };
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

    /// sigaltstack for the calling thread (64 KB above a guard page); its stack pointer for
    /// `this_thread_done`.
    pub fn this_thread() -> u64 {
        unsafe {
            let stack = super::guarded_altstack();
            let ss = StackT { sp: stack as *mut u8, flags: 0, pad: 0, size: super::ALTSTACK };
            sigaltstack(&ss, std::ptr::null_mut());
            stack
        }
    }

    /// The calling thread ends: its fault stack `sp` (this_thread's) is switched off if it is still
    /// on and given back (`free_altstack`). It can be off already: std's thread end switches off
    /// whichever stack is on, and something else may have replaced it; either way nothing uses it.
    pub fn this_thread_done(sp: u64) {
        const SS_DISABLE: i32 = 2;
        unsafe {
            let mut old = StackT { sp: std::ptr::null_mut(), flags: 0, pad: 0, size: 0 };
            if sigaltstack(std::ptr::null(), &mut old) != 0 {
                return;
            }
            let on = old.sp as u64 == sp && old.flags & SS_DISABLE == 0;
            let off = StackT { sp: std::ptr::null_mut(), flags: SS_DISABLE, pad: 0, size: 0 };
            if !on || sigaltstack(&off, std::ptr::null_mut()) == 0 {
                super::free_altstack(sp);
            }
        }
    }

    const OUR_SIGNALS: [i32; 6] = [SIGILL, SIGBUS, SIGFPE, SIG_WATCHDOG, SIGSEGV, SIGABRT];
    const SA_SIGINFO: i32 = 4;
    /// SA_SIGINFO | SA_ONSTACK. No SA_NODEFER: the signal stays blocked while its handler runs,
    /// so a fault inside the handler is not handled again (the kernel kills the process).
    const OUR_FLAGS: i32 = SA_SIGINFO | 0x0800_0000;

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

    /// A handler the ended run's program chained goes; a library's stays.
    pub(crate) unsafe fn reset_statics() {
        for handler in CHAINED.iter().flatten() {
            if crate::stale::lookup(handler.load(Ordering::Relaxed) as u64).is_some() {
                handler.store(0, Ordering::Relaxed);
            }
        }
        // the program's handlers behind the trampoline are given back by progsig.rs
        for row in CHAINED.iter() {
            if row[0].load(Ordering::Relaxed) == crate::os::progsig::trampoline_addr() {
                row[0].store(0, Ordering::Relaxed);
            }
        }
    }

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

    /// The host's fault handler (progsig.rs: where the kernel shows it, the trampoline may be chained behind it).
    pub(crate) fn host_handler() -> usize {
        on_signal as *const () as usize
    }

    /// The handler chained behind ours for `sig` (0: none).
    pub(crate) fn chained_handler(sig: i32) -> usize {
        CHAINED.get(sig as usize).map_or(0, |c| c[0].load(Ordering::SeqCst))
    }

    /// Passes a signal that is not ours to the handler we replaced, if any. False: none.
    unsafe fn chain(sig: i32, info: *mut u8, uc: *mut u8) -> bool {
        let h = CHAINED[sig as usize][0].load(Ordering::SeqCst);
        let flags = CHAINED[sig as usize][1].load(Ordering::SeqCst) as i32;
        if h <= 1 {
            // SIG_DFL / SIG_IGN
            return false;
        }
        // a program handler: the trampoline says whether it took the signal
        if h == crate::os::progsig::trampoline_addr() {
            return crate::os::progsig::chained(sig, info, uc);
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
    pub(crate) struct SigAction {
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
            // RELAX_COMPILE_WATCH: the watcher asks a thread stuck in one step where it is
            if sig == watchdog_signal() && crate::jit::WATCH_DUMP.load(std::sync::atomic::Ordering::Relaxed) {
                let (pc, fp) = (*reg(14), *reg(11));
                eprintln!("relax watch: stuck thread {:#x} at pc {:#x} ({}) fp {:#x}", crate::os::thread::current(), pc, crate::jit::fn_name_at(pc), fp);
                return;
            }
            let f = Fault { kind: kind(sig), sig, pc: *reg(14), sp: *reg(13), fp: *reg(11), addr: *(info.add(OFFSETS[7]) as *const u64), lr: *reg(12) };
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

    /// sigaltstack for the calling thread (64 KB above a guard page); its stack pointer for
    /// `this_thread_done`.
    pub fn this_thread() -> u64 {
        unsafe {
            let stack = super::guarded_altstack();
            let ss = StackT { sp: stack as *mut u8, flags: 0, pad: 0, size: super::ALTSTACK };
            sigaltstack(&ss, std::ptr::null_mut());
            stack
        }
    }

    /// The calling thread ends: its fault stack `sp` (this_thread's) is switched off if it is still
    /// on and given back (`free_altstack`). It can be off already: std's thread end switches off
    /// whichever stack is on, and something else may have replaced it; either way nothing uses it.
    pub fn this_thread_done(sp: u64) {
        const SS_DISABLE: i32 = 2;
        unsafe {
            let mut old = StackT { sp: std::ptr::null_mut(), flags: 0, pad: 0, size: 0 };
            if sigaltstack(std::ptr::null(), &mut old) != 0 {
                return;
            }
            let on = old.sp as u64 == sp && old.flags & SS_DISABLE == 0;
            let off = StackT { sp: std::ptr::null_mut(), flags: SS_DISABLE, pad: 0, size: 0 };
            if !on || sigaltstack(&off, std::ptr::null_mut()) == 0 {
                super::free_altstack(sp);
            }
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
    /// SA_SIGINFO | SA_ONSTACK. No SA_NODEFER: the signal stays blocked while its handler runs,
    /// so a fault inside the handler is not handled again (the kernel kills the process).
    const OUR_FLAGS: i32 = SA_SIGINFO | 0x0800_0000;

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

    /// A handler the ended run's program chained goes; a library's stays.
    pub(crate) unsafe fn reset_statics() {
        for handler in CHAINED.iter().flatten() {
            if crate::stale::lookup(handler.load(Ordering::Relaxed) as u64).is_some() {
                handler.store(0, Ordering::Relaxed);
            }
        }
        // the program's handlers behind the trampoline are given back by progsig.rs
        for row in CHAINED.iter() {
            if row[0].load(Ordering::Relaxed) == crate::os::progsig::trampoline_addr() {
                row[0].store(0, Ordering::Relaxed);
            }
        }
    }

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

    /// The host's fault handler (progsig.rs: where the kernel shows it, the trampoline may be chained behind it).
    pub(crate) fn host_handler() -> usize {
        on_signal as *const () as usize
    }

    /// The handler chained behind ours for `sig` (0: none).
    pub(crate) fn chained_handler(sig: i32) -> usize {
        CHAINED.get(sig as usize).map_or(0, |c| c[0].load(Ordering::SeqCst))
    }

    /// Passes a signal that is not ours to the handler we replaced, if any. False: none.
    unsafe fn chain(sig: i32, info: *mut u8, uc: *mut u8) -> bool {
        let h = CHAINED[sig as usize][0].load(Ordering::SeqCst);
        let flags = CHAINED[sig as usize][1].load(Ordering::SeqCst) as i32;
        if h <= 1 {
            // SIG_DFL / SIG_IGN
            return false;
        }
        // a program handler: the trampoline says whether it took the signal
        if h == crate::os::progsig::trampoline_addr() {
            return crate::os::progsig::chained(sig, info, uc);
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
#[cfg(target_vendor = "apple")]
mod imp {
    use super::unix::{kind, raise_abort, report_abort, SIGABRT, SIGBUS, SIGFPE, SIGILL, SIGSEGV, SIGTRAP};
    use super::{Action, Fault, Hooks};
    use crate::jit::cur_unit;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// SIGUSR1 (Darwin numbering): the hang watchdog's signal
    pub const SIG_WATCHDOG: i32 = 30;
    /// the watchdog's signal handler runs on the looping thread itself
    pub const NEEDS_HANG_TRAMP: bool = false;

    extern "C" {
        fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
        fn sigaltstack(ss: *const StackT, old: *mut StackT) -> i32;
        // macOS only: iOS has no MAP_JIT pages (no executable memory at all)
        #[cfg(target_os = "macos")]
        fn pthread_jit_write_protect_np(enabled: i32);
    }

    /// Darwin `struct sigaction` (user side): handler, sa_mask (u32), sa_flags.
    #[repr(C)]
    pub(crate) struct SigAction {
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

    /// RELAX_COMPILE_WATCH: the watcher asks a thread stuck in one step where it is (no lock:
    /// that thread is the one holding it, stuck)
    unsafe fn watch_dump(uc: *mut u8) {
        let mc = *(uc.add(UC_MCONTEXT) as *const *mut u8);
        let reg = |off: usize| -> u64 { *(mc.add(off) as *const u64) };
        let (pc, lr, fp) = (reg(SS_PC), reg(SS + 240), reg(SS_FP));
        if !cur_unit().is_null() {
            let nm = crate::jit::fn_name_at;
            eprintln!("relax watch: stuck thread {:#x} at pc {:#x} ({}) lr {:#x} ({}) fp {:#x}; on_signal at {:#x}", crate::os::thread::current(), pc, nm(pc), lr, nm(lr), fp, on_signal as *const () as usize);
            let mut f = fp;
            for _ in 0..12 {
                if f == 0 || f % 8 != 0 {
                    break;
                }
                let ret = *((f + 8) as *const u64);
                eprintln!("relax watch:   frame ret {:#x} ({})", ret, nm(ret));
                f = *(f as *const u64);
            }
        }
    }

    /// RELAX_FAULT_REGS: registers and the call sequence around a fault (arm64 encodings).
    unsafe fn fault_regs(uc: *mut u8) {
        // (read once at install: no environment lock in a signal handler)
        if !FAULT_REGS.load(Ordering::Relaxed) {
            return;
        }
        let mc = *(uc.add(UC_MCONTEXT) as *const *mut u8);
        let reg = |off: usize| -> *mut u64 { mc.add(off) as *mut u64 };
        if cur_unit().is_null() {
            return;
        }
        let u = &*cur_unit();
        let (pc, fp) = (*reg(SS_PC), *reg(SS_FP));
        let lr = *reg(SS + 240);
        let x16 = *reg(SS + 16 * 8);
        let x17 = *reg(SS + 17 * 8);
        // the code tables without waiting (the faulting thread may hold them)
        let find = |a: u64| u.code.try_read().ok().and_then(|t| t.slot_at(a));
        let nm = |a: u64| find(a).map(|id| format!("{}#{}", u.fns[id as usize].name, id)).unwrap_or("-".to_string());
        let mut slots = Vec::new();
        for id in 0..u.fns.len() {
            let t = *(u.slots.table(id as u32) as *const u64);
            if t == x16 || t == pc {
                slots.push(format!("{}={}", id, u.fns[id].name));
            }
        }
        let ins = |a: u64| if a >= 8 { *((a) as *const u32) } else { 0 };
        eprintln!("relax fault-regs: pc {:#x} ({}) lr {:#x} ({}) fp {:#x} x16 {:#x} ({}) x17 {:#x}; table slots holding x16/pc: {:?}; insts at lr-12..lr: {:08x} {:08x} {:08x}", pc, nm(pc), lr, nm(lr), fp, x16, nm(x16), x17, slots, ins(lr - 12), ins(lr - 8), ins(lr - 4));
        if let Some(id) = find(lr) {
            let f = &u.fns[id as usize];
            eprintln!("relax fault-regs: caller {} code {:#x} len {} lr off {:#x}", f.name, f.code, f.code_len, lr.wrapping_sub(f.code));
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
            eprintln!("relax fault-regs: called slot {} = {} (compiled {}, code {:#x}) table now {:#x} ({})", id, f.name, f.compiled, f.code, t, nm(t));
        } else {
            eprintln!("relax fault-regs: slot addr {:#x} not in the table", a);
        }
    }

    extern "C" fn on_signal(sig: i32, info: *mut u8, uc: *mut u8) {
        unsafe {
            // the handler allocates: in the system heap, never in the faulting thread's arena (the fault may be inside its allocator)
            let _kept = crate::zone_alloc::kept();
            if sig == SIGABRT {
                let mc = *(uc.add(UC_MCONTEXT) as *const *mut u8);
                let reg = |off: usize| -> *mut u64 { mc.add(off) as *mut u64 };
                report_abort(*reg(SS_PC), *reg(SS_FP));
                default_action(SIGABRT);
                raise_abort();
                return;
            }
            if sig == SIG_WATCHDOG && crate::jit::WATCH_DUMP.load(Ordering::Relaxed) {
                watch_dump(uc);
                return;
            }
            let mc = *(uc.add(UC_MCONTEXT) as *const *mut u8);
            let reg = |off: usize| -> *mut u64 { mc.add(off) as *mut u64 };
            let f = Fault { kind: kind(sig), sig, pc: *reg(SS_PC), sp: *reg(SS_SP), fp: *reg(SS_FP), addr: *(info.add(SI_ADDR) as *const u64), lr: *reg(SS + 240) };
            // the program may fault while it has its own MAP_JIT pages in write mode
            // (pthread_jit_write_protect_np(0)); recovery continues in execute mode
            #[cfg(target_os = "macos")]
            let prepare = || pthread_jit_write_protect_np(1);
            #[cfg(not(target_os = "macos"))]
            let prepare = || {};
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

    /// sigaltstack for the calling thread (64 KB above a guard page); its stack pointer for
    /// `this_thread_done`.
    pub fn this_thread() -> u64 {
        unsafe {
            let stack = super::guarded_altstack();
            let ss = StackT { sp: stack as *mut u8, size: super::ALTSTACK, flags: 0 };
            sigaltstack(&ss, std::ptr::null_mut());
            stack
        }
    }

    /// The calling thread ends: its fault stack `sp` (this_thread's) is switched off if it is still
    /// on and given back (`free_altstack`). It can be off already: std's thread end switches off
    /// whichever stack is on, and something else may have replaced it; either way nothing uses it.
    pub fn this_thread_done(sp: u64) {
        const SS_DISABLE: i32 = 4;
        unsafe {
            let mut old = StackT { sp: std::ptr::null_mut(), size: 0, flags: 0 };
            if sigaltstack(std::ptr::null(), &mut old) != 0 {
                return;
            }
            let on = old.sp as u64 == sp && old.flags & SS_DISABLE == 0;
            // (Darwin checks the size first, also when switching off: ENOMEM for less than its minimum)
            let off = StackT { sp: std::ptr::null_mut(), size: super::ALTSTACK, flags: SS_DISABLE };
            if !on || sigaltstack(&off, std::ptr::null_mut()) == 0 {
                super::free_altstack(sp);
            }
        }
    }

    const OUR_SIGNALS: [i32; 7] = [SIGILL, SIGTRAP, SIGFPE, SIGBUS, SIGSEGV, SIGABRT, SIG_WATCHDOG];

    static FAULT_REGS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    pub fn install() {
        FAULT_REGS.store(std::env::var_os("RELAX_FAULT_REGS").is_some(), Ordering::Relaxed);
        unsafe {
            for sig in OUR_SIGNALS {
                let act = SigAction { handler: on_signal as *const () as usize, mask: 0, flags: SA_SIGINFO | SA_ONSTACK };
                sigaction(sig, &act, std::ptr::null_mut());
            }
        }
    }

    /// Handlers a library of the program installed over ours (Chromium sets its own for SIGSEGV,
    /// SIGBUS, .. in CefInitialize): (handler, flags) per signal, called for signals that are not
    /// faults in JIT code -- the library keeps working, and faults in JIT code are still reported.
    static CHAINED: [[AtomicUsize; 2]; 32] = [const { [const { AtomicUsize::new(0) }; 2] }; 32];

    /// A handler the ended run's program chained goes; a library's stays.
    pub(crate) unsafe fn reset_statics() {
        for handler in CHAINED.iter().flatten() {
            if crate::stale::lookup(handler.load(Ordering::Relaxed) as u64).is_some() {
                handler.store(0, Ordering::Relaxed);
            }
        }
        // the program's handlers behind the trampoline are given back by progsig.rs
        for row in CHAINED.iter() {
            if row[0].load(Ordering::Relaxed) == crate::os::progsig::trampoline_addr() {
                row[0].store(0, Ordering::Relaxed);
            }
        }
    }

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
                    let act = SigAction { handler: on_signal as *const () as usize, mask: 0, flags: SA_SIGINFO | SA_ONSTACK };
                    sigaction(sig, &act, std::ptr::null_mut());
                }
            }
        }
    }

    /// The host's fault handler (progsig.rs: where the kernel shows it, the trampoline may be chained behind it).
    pub(crate) fn host_handler() -> usize {
        on_signal as *const () as usize
    }

    /// The handler chained behind ours for `sig` (0: none).
    pub(crate) fn chained_handler(sig: i32) -> usize {
        CHAINED.get(sig as usize).map_or(0, |c| c[0].load(Ordering::SeqCst))
    }

    /// Passes a signal that is not ours to the handler we replaced, if any. False: none.
    unsafe fn chain(sig: i32, info: *mut u8, uc: *mut u8) -> bool {
        let h = CHAINED[sig as usize][0].load(Ordering::SeqCst);
        let flags = CHAINED[sig as usize][1].load(Ordering::SeqCst) as i32;
        if h <= 1 {
            // SIG_DFL / SIG_IGN
            return false;
        }
        // a program handler: the trampoline says whether it took the signal
        if h == crate::os::progsig::trampoline_addr() {
            return crate::os::progsig::chained(sig, info, uc);
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
        // (the code of a run that ended and a thread of an ending run are this runtime's whatever
        // unit is current: the normal search cannot walk their frames)
        if chained && !crate::jit::in_jit_code(pc) && !crate::stale::in_ended_run(pc) && !crate::domain::current_is_ending() {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        // the data address of an access violation / in-page error (ExceptionInformation[1]);
        // a stack overflow reports the stack pointer's page
        let addr = match rec.code {
            EXCEPTION_ACCESS_VIOLATION | EXCEPTION_IN_PAGE_ERROR if rec.n_params >= 2 => rec.info[1] as u64,
            EXCEPTION_STACK_OVERFLOW => ctx.get(CTX_RSP).wrapping_sub(8),
            _ => pc,
        };
        let f = Fault { kind: k, sig: rec.code as i32, pc, sp: ctx.get(CTX_RSP), fp: ctx.get(CTX_RBP), addr, lr: 0 };
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

    /// RELAX_FAULT_TRACE=1: every exception the handlers see, with the module holding its pc
    /// (diagnostics for faults outside JIT code; no debugger needed).
    fn trace() -> bool {
        static T: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);
        let t = T.load(std::sync::atomic::Ordering::Relaxed);
        if t != 2 {
            return t == 1;
        }
        let on = std::env::var_os("RELAX_FAULT_TRACE").is_some();
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
        let msg = format!("relax fault-trace: {} exception {:#x} at pc {:#x} = {}+{:#x}{}\n", if first { "first-chance" } else { "unhandled" }, code, pc, if n > 0 { module.as_str() } else { "?" }, pc.wrapping_sub(m as u64), if crate::jit::in_jit_code(pc) { " (JIT code)" } else { "" });
        let mut w = 0u32;
        WriteFile(GetStdHandle(-12i32 as u32), msg.as_ptr(), msg.len() as u32, &mut w, std::ptr::null_mut());
    }

    extern "system" fn vectored(ep: *mut ExceptionPointers) -> i32 {
        unsafe { handle(ep, true) }
    }

    /// The end of every search, on a thread's own stack and on a program stack (its last frame):
    /// what the runtime does not resume ends the process here, with its code. The system's way on
    /// is an error report, and with no one to answer it (no desktop, a dialog) the process stays.
    extern "system" fn unhandled(ep: *mut ExceptionPointers) -> i32 {
        unsafe {
            let r = handle(ep, false);
            if r != EXCEPTION_CONTINUE_EXECUTION {
                TerminateProcess(GetCurrentProcess(), (*(*ep).record).code);
            }
            r
        }
    }

    /// The stack-overflow handler runs in the thread's guaranteed stack area (64 KB).
    pub fn this_thread() -> u64 {
        let mut size: u32 = 1 << 16;
        unsafe {
            SetThreadStackGuarantee(&mut size);
        }
        0
    }

    /// The guarantee goes with the thread.
    pub fn this_thread_done(_sp: u64) {}

    pub fn install() {
        unsafe {
            AddVectoredExceptionHandler(1, vectored);
            SetUnhandledExceptionFilter(Some(unhandled));
        }
    }

    pub fn reassert() {}

    /// Suspends thread `t` and reads its context. RELAX_COMPILE_WATCH: prints where it is. Hang
    /// A run's end (domain.rs end_run): no signal ends a wait here. A synchronous I/O call is
    /// cancelled, and the program's waits end by themselves (domain_threads.rs `waits`). Back in JIT
    /// code the thread faults and ends as on unix.
    pub fn end_interrupt(t: u64, waiting: usize) {
        crate::domain_threads::waits::wake(waiting);
        unsafe {
            let h = OpenThread(THREAD_TERMINATE, 0, t as u32);
            if h != 0 {
                CancelSynchronousIo(h);
                CloseHandle(h);
            }
        }
    }

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
                        eprintln!("relax watch: stuck thread {:#x} at pc {:#x} ({}) fp {:#x}", t, pc, crate::jit::fn_name_at(pc), fp);
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

    pub(crate) unsafe fn reset_statics() {}
}

#[cfg(not(target_os = "android"))]
pub fn watchdog_signal() -> i32 { SIG_WATCHDOG }

/// Starts the thread SIGABRT's report is made on (`unix::abort`), with the compiler's owner.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub fn start_abort_reporter() {
    unix::start_reporter()
}

/// Fault stack memory: ALTSTACK bytes above one PROT_NONE guard page, so a handler that
/// overflows its alternate stack faults on the guard (the process dies) instead of writing into
/// whatever mapping lies below it. One a finished thread left (`free_altstack`), else a new one.
#[cfg(unix)]
fn guarded_altstack() -> u64 {
    if let Some(sp) = SPARE_ALTSTACKS.take() {
        return sp;
    }
    let base = crate::os::mem::alloc_rw(ALTSTACK + ALTSTACK_GUARD);
    crate::os::mem::guard_page(base, ALTSTACK_GUARD);
    base + ALTSTACK_GUARD as u64
}

#[cfg(unix)]
const ALTSTACK: usize = 1 << 16;
#[cfg(unix)]
const ALTSTACK_GUARD: usize = 16 << 10;

/// Fault stacks no thread uses, for the next thread's `guarded_altstack`.
#[cfg(unix)]
static SPARE_ALTSTACKS: crate::stack::Spares<u64> = crate::stack::Spares::new();

/// A thread is done with what `guarded_altstack` made (`sp` above its guard): it is kept for the
/// next thread, or unmapped when enough are kept.
#[cfg(unix)]
fn free_altstack(sp: u64) {
    if let Some(sp) = SPARE_ALTSTACKS.keep(sp) {
        crate::os::mem::free(sp - ALTSTACK_GUARD as u64, ALTSTACK + ALTSTACK_GUARD);
    }
}

/// The calling thread's stack bounds (0, 0: unknown). Recorded in normal context;
/// fault handlers never query pthread attributes.
pub fn stack_bounds_of_self() -> (u64, u64) {
    // a program stack (relax_base::stack::on_program_stack) replaces the OS thread's while it runs
    if let Some((lo, hi)) = crate::stack::program_stack_bounds() {
        return (lo as u64, hi as u64);
    }
    #[cfg(target_vendor = "apple")]
    unsafe {
        extern "C" {
            fn pthread_self() -> usize;
            fn pthread_get_stackaddr_np(t: usize) -> *mut u8;
            fn pthread_get_stacksize_np(t: usize) -> usize;
        }
        let t = pthread_self();
        let hi = pthread_get_stackaddr_np(t) as u64;
        (hi.saturating_sub(pthread_get_stacksize_np(t) as u64), hi)
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        extern "C" {
            fn pthread_self() -> usize;
            fn pthread_getattr_np(t: usize, attr: *mut u64) -> i32;
            fn pthread_attr_getstack(attr: *const u64, addr: *mut usize, size: *mut usize) -> i32;
            fn pthread_attr_destroy(attr: *mut u64) -> i32;
        }
        let mut attr = [0u64; 16];
        if pthread_getattr_np(pthread_self(), attr.as_mut_ptr()) != 0 {
            return (0, 0);
        }
        let (mut addr, mut size) = (0usize, 0usize);
        let ok = pthread_attr_getstack(attr.as_ptr(), &mut addr, &mut size) == 0;
        pthread_attr_destroy(attr.as_mut_ptr());
        if ok { (addr as u64, (addr + size) as u64) } else { (0, 0) }
    }
    #[cfg(windows)]
    unsafe {
        extern "system" {
            fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
        }
        let (mut lo, mut hi) = (0usize, 0usize);
        GetCurrentThreadStackLimits(&mut lo, &mut hi);
        (lo as u64, hi as u64)
    }
}

/// The word at `addr`, None where it cannot be read: the kernel reads it, so a bad address is an
/// error and not a fault. Signal-safe (no allocation, no lock).
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub fn read_word_checked(addr: u64) -> Option<u64> {
    let mut word = 0u64;
    #[cfg(target_os = "linux")]
    unsafe {
        #[repr(C)]
        struct IoVec {
            base: *mut u8,
            len: usize,
        }
        extern "C" {
            fn getpid() -> i32;
            fn process_vm_readv(pid: i32, local: *const IoVec, n_local: u64, remote: *const IoVec, n_remote: u64, flags: u64) -> isize;
        }
        let local = IoVec { base: (&mut word as *mut u64).cast(), len: 8 };
        let remote = IoVec { base: addr as *mut u8, len: 8 };
        (process_vm_readv(getpid(), &local, 1, &remote, 1, 0) == 8).then_some(word)
    }
    #[cfg(target_vendor = "apple")]
    unsafe {
        extern "C" {
            static mach_task_self_: u32;
            fn mach_vm_read_overwrite(task: u32, addr: u64, size: u64, data: u64, out_size: *mut u64) -> i32;
        }
        let mut n = 0u64;
        (mach_vm_read_overwrite(mach_task_self_, addr, 8, &mut word as *mut u64 as u64, &mut n) == 0 && n == 8).then_some(word)
    }
}

/// Writes `b` to stderr with one system call: no allocation, no lock (a fault handler's report).
pub fn raw_stderr(b: &[u8]) {
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn write(fd: i32, buf: *const core::ffi::c_void, n: usize) -> isize;
        }
        let _ = write(crate::tui::fault_fd(), b.as_ptr().cast(), b.len());
    }
    #[cfg(windows)]
    {
        use crate::os::win32::*;
        let mut written = 0;
        unsafe { WriteFile(GetStdHandle(-12i32 as u32), b.as_ptr(), b.len() as u32, &mut written, std::ptr::null_mut()); }
    }
}

/// Ends the process by signal `sig` with its default action (exit status 128 + sig, as a
/// rustc build's fault would), from normal context: a contained fault the policy found fatal.
pub fn die_by(sig: i32) -> ! {
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
            fn raise(sig: i32) -> i32;
        }
        signal(sig, 0);
        raise(sig);
    }
    let _ = sig;
    std::process::abort()
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    imp::reset_statics();
}

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
mod abort_tests {
    use std::io::BufRead;
    use std::os::unix::process::ExitStatusExt;

    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
        fn setrlimit(resource: i32, limit: *const [u64; 2]) -> i32;
    }

    /// A chain walked with no recorded stack bounds ends at the first word it cannot read, with
    /// the frames before it, and does not fault.
    #[test]
    fn a_frame_chain_without_stack_bounds_ends_at_an_unreadable_page() {
        extern "C" {
            fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
            fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
            fn munmap(addr: *mut u8, len: usize) -> i32;
        }
        const PAGE: usize = 1 << 16;
        // PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS
        let anon = if cfg!(target_vendor = "apple") { 0x1002 } else { 0x22 };
        let base = unsafe { mmap(std::ptr::null_mut(), 2 * PAGE, 3, anon, -1, 0) };
        assert_ne!(base as isize, -1);
        assert_eq!(unsafe { mprotect(base.add(PAGE), PAGE, 0) }, 0);
        let words = base as *mut u64;
        let at = |i: usize| base as u64 + 8 * i as u64;
        for (i, w) in [at(2), 0xa, at(4), 0xb, base as u64 + PAGE as u64, 0xc].into_iter().enumerate() {
            unsafe { words.add(i).write(w) };
        }
        let mut out = [0u64; 8];
        let n = crate::jit::walk_fp_checked(at(0), &mut out);
        assert_eq!(&out[..n], &[0xa, 0xb, 0xc]);
        // the same word read by hand: the page itself answers None
        assert_eq!(super::read_word_checked(base as u64 + PAGE as u64), None);
        assert_eq!(super::read_word_checked(at(1)), Some(0xa));
        unsafe { munmap(base, 2 * PAGE) };
    }

    /// A SIGABRT sent from outside ends the process after its report line, as abort()'s does. A JIT
    /// parked at a compile error took one, reported it and stayed parked for good.
    #[test]
    fn a_sigabrt_from_outside_ends_the_process() {
        const CHILD: &str = "RELAX_OUTSIDE_SIGABRT_CHILD";
        const READY: &str = "waiting for the signal";
        if std::env::var_os(CHILD).is_some() {
            // (no core file: writing one is most of the time an abort takes; RLIMIT_CORE)
            unsafe { setrlimit(4, &[0, 0]) };
            super::install();
            super::start_abort_reporter();
            println!("{READY}");
            std::thread::sleep(std::time::Duration::from_secs(60));
            std::process::exit(7);
        }
        let _slot = crate::test_child::slot();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "os::fault::abort_tests::a_sigabrt_from_outside_ends_the_process", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.contains(READY) {
            line.clear();
            assert!(out.read_line(&mut line).unwrap() > 0, "the child ended before it waited for the signal");
        }
        assert_eq!(unsafe { kill(child.id() as i32, 6) }, 0);
        let output = child.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.signal(), Some(6), "status {:?}:\n{err}", output.status);
        assert!(err.contains("SIGABRT on thread"), "no report line:\n{err}");
    }
}
