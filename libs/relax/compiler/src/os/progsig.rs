//! The program's signal handlers never run inside a compile.
//!
//! A lazy compile runs on the program's own thread (jit.rs `rt_compile`), holding the compiler's
//! locks and lazy statics. A program handler that ran there would compile its own first calls
//! inside that compile and wait for what the interrupted compile holds, and the thread would
//! hang. That was an outside SIGABRT at a test child's first `thread::sleep`, 11-12. So the
//! program's `signal` and `sigaction` (domain_res.rs) keep the program's handler in a fixed
//! table, and the kernel gets this module's trampoline with the program's mask and flags. The
//! trampoline:
//! - runs the program's handler on a program thread outside a compile, as the kernel would have;
//! - on a program thread inside a compile, blocks the signal in the interrupted context and sends
//!   it again to the same thread. The compile's end unblocks it (`Compiling`), and the kernel
//!   delivers it through the trampoline once more, which now runs the handler;
//! - on a host thread (the compiler's owner, its workers, a native library's threads), blocks the
//!   signal there and hands it back to the process, which offers it to the threads still open.
//!
//! A thread is a program thread only where it registered (`program_thread`): the program's main
//! and the threads it starts. A signal with no program handler (a default SIGINT or SIGTERM, the
//! host's own signals) never comes here and nothing here blocks it. One host thread
//! ("run-signals") keeps its mask equal to the program-handled set, so a host thread's lazy
//! blocks never leave such a signal without a thread open for it once the program's handler is
//! gone.
//!
//! What the program sees: `sigaction`'s old action is the program's own (handler, mask, flags)
//! and never the trampoline. SA_RESETHAND is applied here, by the delivery that runs the handler,
//! and never given to the kernel: a host thread's delivery would use it up. A signal sent at a
//! compiling thread runs its handler on that thread once the compile ends. A signal at a host
//! thread runs it on a program thread. On Linux a signal a host thread hands back keeps its
//! siginfo, except that a kill's, a tgkill's or the kernel's own code reads SI_QUEUE (the kernel
//! takes those codes only from the main thread). On macOS the handler of a signal sent on again
//! sees this process as its sender: Darwin has no sigqueue.

use std::sync::atomic::{compiler_fence, fence, AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering::{Acquire, Relaxed, Release, SeqCst}};

#[cfg(any(target_os = "linux", target_os = "android"))]
mod abi {
    pub const NSIG: usize = 65;
    pub const SIGILL: i32 = 4;
    pub const SIGTRAP: i32 = 5;
    pub const SIGFPE: i32 = 8;
    pub const SIGSEGV: i32 = 11;
    pub const SIGKILL: i32 = 9;
    pub const SIGSTOP: i32 = 19;
    pub const SIGBUS: i32 = 7;
    pub const SIGSYS: i32 = 31;
    pub const SA_SIGINFO: i32 = 4;
    pub const SA_RESTART: i32 = 0x1000_0000;
    pub const SA_RESETHAND: i32 = 0x8000_0000u32 as i32;
    pub const SIG_BLOCK: i32 = 0;
    pub const SIG_UNBLOCK: i32 = 1;
    pub const SIG_SETMASK: i32 = 2;
    pub const CLOCK_MONOTONIC: i32 = 1;
    /// siginfo si_code values: sigqueue's, and tgkill's
    pub const SI_QUEUE: i32 = -1;
    pub const SI_TKILL: i32 = -6;
    /// `struct sigaction`: its size in words, the byte offsets of sa_handler and sa_flags
    #[cfg(target_os = "linux")]
    pub const ACT_WORDS: usize = 19;
    #[cfg(target_os = "linux")]
    pub const HANDLER: usize = 0;
    #[cfg(target_os = "linux")]
    pub const FLAGS: usize = 136;
    #[cfg(target_os = "android")]
    pub const ACT_WORDS: usize = 4;
    #[cfg(target_os = "android")]
    pub const HANDLER: usize = 8;
    #[cfg(target_os = "android")]
    pub const FLAGS: usize = 0;
    /// the kernel's `ucontext`: uc_sigmask's byte offset (the mask sigreturn restores)
    #[cfg(target_arch = "x86_64")]
    pub const UC_SIGMASK: usize = 296;
    #[cfg(target_arch = "aarch64")]
    pub const UC_SIGMASK: usize = 40;
    #[cfg(target_arch = "x86_64")]
    pub const SYS_GETTID: i64 = 186;
    #[cfg(target_arch = "x86_64")]
    pub const SYS_RT_SIGQUEUEINFO: i64 = 129;
    #[cfg(target_arch = "x86_64")]
    pub const SYS_RT_TGSIGQUEUEINFO: i64 = 297;
    #[cfg(target_arch = "x86_64")]
    pub const SYS_FUTEX: i64 = 202;
    #[cfg(target_arch = "aarch64")]
    pub const SYS_GETTID: i64 = 178;
    #[cfg(target_arch = "aarch64")]
    pub const SYS_RT_SIGQUEUEINFO: i64 = 138;
    #[cfg(target_arch = "aarch64")]
    pub const SYS_RT_TGSIGQUEUEINFO: i64 = 240;
    #[cfg(target_arch = "aarch64")]
    pub const SYS_FUTEX: i64 = 98;
    /// FUTEX_WAIT_PRIVATE, FUTEX_WAKE_PRIVATE
    pub const FUTEX_WAIT: i32 = 128;
    pub const FUTEX_WAKE: i32 = 129;
}

#[cfg(target_vendor = "apple")]
mod abi {
    pub const NSIG: usize = 32;
    pub const SIGILL: i32 = 4;
    pub const SIGTRAP: i32 = 5;
    pub const SIGFPE: i32 = 8;
    pub const SIGSEGV: i32 = 11;
    pub const SIGKILL: i32 = 9;
    pub const SIGSTOP: i32 = 17;
    pub const SIGBUS: i32 = 10;
    pub const SIGSYS: i32 = 12;
    pub const SA_SIGINFO: i32 = 0x40;
    pub const SA_RESTART: i32 = 0x2;
    pub const SA_RESETHAND: i32 = 0x4;
    pub const SIG_BLOCK: i32 = 1;
    pub const SIG_UNBLOCK: i32 = 2;
    pub const SIG_SETMASK: i32 = 3;
    pub const CLOCK_MONOTONIC: i32 = 6;
    /// `struct sigaction` (user side): handler, sa_mask (u32), sa_flags
    pub const ACT_WORDS: usize = 2;
    pub const HANDLER: usize = 0;
    pub const FLAGS: usize = 12;
    /// `ucontext_t`: uc_onstack, then uc_sigmask (u32)
    pub const UC_SIGMASK: usize = 4;
    /// UL_COMPARE_AND_WAIT | ULF_NO_ERRNO, and | ULF_WAKE_ALL
    pub const UL_WAIT: u32 = 0x0100_0001;
    pub const UL_WAKE_ALL: u32 = 0x0100_0101;
}

use abi::*;

/// A `struct sigaction` as words.
type Act = [u64; ACT_WORDS];
const _: () = assert!(std::mem::size_of::<crate::os::fault::SigAction>() == ACT_WORDS * 8);
/// SIG_DFL, no mask, no flags
const DFL: Act = [0; ACT_WORDS];

mod sys {
    extern "C" {
        pub fn sigaction(sig: i32, act: *const crate::os::fault::SigAction, old: *mut crate::os::fault::SigAction) -> i32;
        pub fn pthread_sigmask(how: i32, set: *const [u64; 16], old: *mut [u64; 16]) -> i32;
        // pthread_key_t: unsigned long on macOS, unsigned int on Linux (zero-extended here, as os/tls.rs)
        pub fn pthread_key_create(key: *mut usize, dtor: Option<extern "C" fn(*mut u8)>) -> i32;
        pub fn pthread_getspecific(key: usize) -> *mut u8;
        pub fn pthread_setspecific(key: usize, v: *mut u8) -> i32;
        pub fn pthread_self() -> usize;
        pub fn pthread_kill(t: usize, sig: i32) -> i32;
        pub fn getpid() -> i32;
        pub fn kill(pid: i32, sig: i32) -> i32;
        pub fn clock_gettime(id: i32, ts: *mut super::Ts) -> i32;
        pub fn pipe(fds: *mut i32) -> i32;
        pub fn read(fd: i32, buf: *mut core::ffi::c_void, n: usize) -> isize;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        pub fn syscall(n: i64, ...) -> i64;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        pub fn getuid() -> u32;
        #[cfg(target_os = "linux")]
        pub fn __errno_location() -> *mut i32;
        #[cfg(target_os = "android")]
        pub fn __errno() -> *mut i32;
        #[cfg(target_vendor = "apple")]
        pub fn __error() -> *mut i32;
        #[cfg(target_vendor = "apple")]
        pub fn __ulock_wait(op: u32, addr: *mut u8, value: u64, timeout_us: u32) -> i32;
        #[cfg(target_vendor = "apple")]
        pub fn __ulock_wake(op: u32, addr: *mut u8, wake_value: u64) -> i32;
    }
}

/// The kernel's `sigaction` on words.
unsafe fn kernel(sig: i32, act: *const Act, old: *mut Act) -> i32 {
    sys::sigaction(sig, act.cast(), old.cast())
}

fn handler(a: &Act) -> usize {
    unsafe { *((a.as_ptr() as *const u8).add(HANDLER) as *const usize) }
}
fn set_handler(a: &mut Act, h: usize) {
    unsafe { *((a.as_mut_ptr() as *mut u8).add(HANDLER) as *mut usize) = h }
}
fn flags(a: &Act) -> i32 {
    unsafe { *((a.as_ptr() as *const u8).add(FLAGS) as *const i32) }
}
fn set_flags(a: &mut Act, f: i32) {
    unsafe { *((a.as_mut_ptr() as *mut u8).add(FLAGS) as *mut i32) = f }
}

/// The trampoline's address: what the kernel (or fault.rs's CHAINED) holds for a signal with a
/// program handler.
pub(crate) fn trampoline_addr() -> usize {
    trampoline as *const () as usize
}

// ------------------------------------------------------------ the table

/// One signal's program handler. `seq` is odd while a writer changes the entry; a writer has every
/// signal blocked on its thread, so a reader that waits for it (the trampoline, on another thread)
/// always sees it finish.
struct Entry {
    seq: AtomicU32,
    /// the program's action as it gave it; handler 0: none
    act: [AtomicU64; ACT_WORDS],
    /// what the program saw before its first change (`give_back`, `reset_statics`)
    orig: [AtomicU64; ACT_WORDS],
    orig_set: AtomicBool,
    /// the domain that changed it last (0: the process's own, which is never given back)
    domain: AtomicU64,
}

impl Entry {
    const fn new() -> Entry {
        Entry { seq: AtomicU32::new(0), act: [const { AtomicU64::new(0) }; ACT_WORDS], orig: [const { AtomicU64::new(0) }; ACT_WORDS], orig_set: AtomicBool::new(false), domain: AtomicU64::new(0) }
    }
    /// Takes the entry for a change; the caller has every signal blocked (`Blocked`).
    fn lock(&self) -> u32 {
        loop {
            let s = self.seq.load(Relaxed);
            if s & 1 == 0 && self.seq.compare_exchange_weak(s, s + 1, Acquire, Relaxed).is_ok() {
                fence(Release);
                return s + 1;
            }
            std::hint::spin_loop();
        }
    }
    fn unlock(&self, odd: u32) {
        self.seq.store(odd + 1, Release);
    }
    fn act(&self) -> Act {
        std::array::from_fn(|i| self.act[i].load(Relaxed))
    }
    fn set_act(&self, a: &Act) {
        for (w, v) in self.act.iter().zip(a) {
            w.store(*v, Relaxed);
        }
    }
    fn orig(&self) -> Act {
        std::array::from_fn(|i| self.orig[i].load(Relaxed))
    }
    /// The program's handler and flags without a lock: (the even seq they were read at, handler, flags).
    fn read(&self) -> (u32, usize, i32) {
        loop {
            let s = self.seq.load(Acquire);
            if s & 1 == 0 {
                let a = self.act();
                fence(Acquire);
                if self.seq.load(Relaxed) == s {
                    return (s, handler(&a), flags(&a));
                }
            }
            std::hint::spin_loop();
        }
    }
}

static TABLE: [Entry; NSIG] = [const { Entry::new() }; NSIG];

/// The signals with a program handler (bit sig-1), and its changes (`GEN`): the signal thread's mask.
static PROGRAM_SET: AtomicU64 = AtomicU64::new(0);
static GEN: AtomicU32 = AtomicU32::new(0);
/// The mask the signal thread last set, and how many times it set one (tests wait on it).
static APPLIED: AtomicU64 = AtomicU64::new(0);
static APPLIED_GEN: AtomicU32 = AtomicU32::new(0);

/// Per signal: deliveries that ran the program's handler, waited for a compile, went from a host
/// thread back to the process, were lost (the kernel's queue of pending signals was full).
const RAN: usize = 0;
const DEFERRED: usize = 1;
const DEFLECTED: usize = 2;
const LOST: usize = 3;
static COUNTS: [[AtomicU64; 4]; NSIG] = [const { [const { AtomicU64::new(0) }; 4] }; NSIG];

/// (ran, deferred, deflected, lost) of signal `sig` so far.
pub(crate) fn counts(sig: i32) -> [u64; 4] {
    match COUNTS.get(sig as usize) {
        Some(c) => std::array::from_fn(|i| c[i].load(Relaxed)),
        None => [0; 4],
    }
}

/// Every signal blocked on this thread while it lives (a table writer, which a delivery on its
/// own thread must not interrupt).
struct Blocked([u64; 16]);

impl Blocked {
    fn all() -> Blocked {
        let mut old = [0u64; 16];
        unsafe { sys::pthread_sigmask(SIG_BLOCK, &[!0u64; 16], &mut old) };
        Blocked(old)
    }
}

impl Drop for Blocked {
    fn drop(&mut self) {
        unsafe { sys::pthread_sigmask(SIG_SETMASK, &self.0, std::ptr::null_mut()) };
    }
}

/// Is `h` the program's code (a handler the trampoline runs), not the system's or the host's?
fn program_code(h: usize) -> bool {
    h > 1 && h != trampoline_addr() && crate::jit::in_jit_code(h as u64)
}

/// Sets signal `sig` (its entry locked) to `a`: a program handler goes in the entry first and the
/// trampoline to the kernel second; anything else goes to the kernel first and the entry is
/// emptied second. Then the signal thread learns the new set.
unsafe fn set(e: &Entry, sig: i32, a: &Act) -> i32 {
    let bit = 1u64 << (sig - 1);
    let r = if program_code(handler(a)) {
        e.set_act(a);
        let mut k = *a;
        set_handler(&mut k, trampoline_addr());
        set_flags(&mut k, (flags(a) | SA_SIGINFO) & !SA_RESETHAND);
        let r = kernel(sig, &k, std::ptr::null_mut());
        if r == 0 {
            PROGRAM_SET.fetch_or(bit, Relaxed);
        } else {
            e.set_act(&DFL);
        }
        r
    } else {
        let mut a = *a;
        if handler(&a) == trampoline_addr() {
            set_handler(&mut a, 0);
        }
        let r = kernel(sig, &a, std::ptr::null_mut());
        if r == 0 {
            e.set_act(&DFL);
            PROGRAM_SET.fetch_and(!bit, Relaxed);
        }
        r
    };
    resync();
    r
}

/// Does the kernel's action `cur` for `sig` run the trampoline (itself, or the host's fault
/// handler with the trampoline chained behind it)?
fn shows_trampoline(sig: i32, cur: &Act) -> bool {
    let h = handler(cur);
    h == trampoline_addr() || (h == crate::os::fault::host_handler() && crate::os::fault::chained_handler(sig) == trampoline_addr())
}

/// The program's `sigaction` (domain_res.rs), for the domain at `domain` (0: the process's own).
/// Async-signal-safe, as the system's is: a program may change a disposition in its handler.
pub(crate) unsafe fn sigaction(sig: i32, act: *const u8, old: *mut u8, domain: u64) -> i32 {
    if sig <= 0 || sig as usize >= NSIG || sig == SIGKILL || sig == SIGSTOP {
        return sys::sigaction(sig, act.cast(), old.cast());
    }
    let e = &TABLE[sig as usize];
    let (r, prev) = {
        let _blocked = Blocked::all();
        let odd = e.lock();
        let mut cur = DFL;
        let mut r = kernel(sig, std::ptr::null(), &mut cur);
        let entry = e.act();
        // what the program sees: its own action while the kernel runs the trampoline for it
        let prev = if handler(&entry) != 0 && shows_trampoline(sig, &cur) { entry } else { cur };
        if r == 0 && !act.is_null() {
            let a = std::ptr::read_unaligned(act as *const Act);
            if !e.orig_set.load(Relaxed) {
                for (w, v) in e.orig.iter().zip(&prev) {
                    w.store(*v, Relaxed);
                }
                e.orig_set.store(true, Relaxed);
            }
            if domain != 0 {
                e.domain.store(domain, Relaxed);
            }
            r = set(e, sig, &a);
            // the program took inspect's capture signal: the capture moves to a free one
            #[cfg(any(target_os = "linux", all(target_os = "android", target_arch = "aarch64")))]
            if r == 0 && handler(&a) != 0 {
                crate::inspect::threads::capture_taken(sig);
            }
        }
        e.unlock(odd);
        (r, prev)
    };
    if r == 0 && !old.is_null() {
        std::ptr::write_unaligned(old as *mut Act, prev);
    }
    r
}

/// The program's `signal`: `sigaction` with SA_RESTART; the previous handler, SIG_ERR on failure.
pub(crate) unsafe fn signal(sig: i32, h: usize, domain: u64) -> usize {
    let mut a = DFL;
    set_handler(&mut a, h);
    set_flags(&mut a, SA_RESTART);
    let mut old = DFL;
    if sigaction(sig, a.as_ptr().cast(), old.as_mut_ptr().cast(), domain) != 0 {
        return usize::MAX;
    }
    handler(&old)
}

/// Before the code of the domains at `bases` ends: each disposition one of them changed last goes
/// back to what the program saw before its first change. Returns how many.
pub(crate) fn give_back(bases: &[u64]) -> u32 {
    let mut n = 0;
    for sig in 1..NSIG as i32 {
        let e = &TABLE[sig as usize];
        if !e.orig_set.load(Acquire) || !bases.contains(&e.domain.load(Acquire)) {
            continue;
        }
        let _blocked = Blocked::all();
        let odd = e.lock();
        if e.orig_set.load(Relaxed) && bases.contains(&e.domain.load(Relaxed)) {
            unsafe { set(e, sig, &e.orig()) };
            e.orig_set.store(false, Relaxed);
            e.domain.store(0, Relaxed);
            n += 1;
        }
        e.unlock(odd);
    }
    n
}

/// The run ended (`run_end`): a program handler in its code goes. Where the kernel still runs the
/// trampoline for it, the disposition goes back to what the program saw before its first change
/// (SIG_DFL when that was code of an ended run too).
pub(crate) unsafe fn reset_statics() {
    let stale = |h: usize| h > 1 && crate::stale::lookup(h as u64).is_some();
    for sig in 1..NSIG as i32 {
        let e = &TABLE[sig as usize];
        if !stale(handler(&e.act())) {
            continue;
        }
        let _blocked = Blocked::all();
        let odd = e.lock();
        let mut cur = DFL;
        let mut back = if e.orig_set.load(Relaxed) { e.orig() } else { DFL };
        if stale(handler(&back)) {
            back = DFL;
        }
        if kernel(sig, std::ptr::null(), &mut cur) == 0 && shows_trampoline(sig, &cur) {
            set(e, sig, &back);
        } else {
            e.set_act(&DFL);
            PROGRAM_SET.fetch_and(!(1u64 << (sig - 1)), Relaxed);
            resync();
        }
        e.orig_set.store(false, Relaxed);
        e.domain.store(0, Relaxed);
        e.unlock(odd);
    }
}

// ------------------------------------------------------------ thread class

/// A program thread's signal state, reached through the thread's `KEY` value: a pthread key, not a
/// thread-local, since the trampoline reads it in a signal handler (a never-seen thread reads NULL
/// without allocating; a first touch of a thread-local block on macOS allocates).
pub(crate) struct SigState {
    /// compiles the thread is in (`Compiling`)
    depth: AtomicU32,
    /// signals that waited for the compile to end (bit sig-1), and since when (monotonic ns)
    deferred: AtomicU64,
    since: AtomicU64,
    /// the last signal the trampoline took on this thread (the re-entry report)
    last: AtomicI32,
}

static KEY: AtomicUsize = AtomicUsize::new(usize::MAX);

fn key() -> Option<usize> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let mut k = 0usize;
        if unsafe { sys::pthread_key_create(&mut k, None) } == 0 {
            KEY.store(k as usize, Release);
        }
    });
    let k = KEY.load(Acquire);
    (k != usize::MAX).then_some(k)
}

/// The calling thread's state, NULL on a host or foreign thread. Async-signal-safe.
fn state() -> *const SigState {
    let k = KEY.load(Acquire);
    if k == usize::MAX {
        return std::ptr::null();
    }
    unsafe { sys::pthread_getspecific(k) as *const SigState }
}

/// Is the calling thread a program thread?
pub(crate) fn on_program_thread() -> bool {
    !state().is_null()
}

/// The last signal the trampoline took on this thread (0: none, or not a program thread).
pub(crate) fn last_signal() -> i32 {
    let s = state();
    if s.is_null() { 0 } else { unsafe { (*s).last.load(Relaxed) } }
}

/// While it lives, the calling thread is a program thread: the program's handlers run on it. Taken
/// where a thread starts running the program's code (its main, the threads it starts), never in
/// `call_jit_in`: the compiler's owner runs constant initializers through that and stays a host
/// thread. Nested ones keep the outermost's state.
pub(crate) struct ProgramThread(*mut SigState);

pub(crate) fn program_thread() -> ProgramThread {
    let Some(k) = key() else { return ProgramThread(std::ptr::null_mut()) };
    if !state().is_null() {
        return ProgramThread(std::ptr::null_mut());
    }
    let s = {
        let _kept = crate::zone_alloc::kept();
        Box::into_raw(Box::new(SigState { depth: AtomicU32::new(0), deferred: AtomicU64::new(0), since: AtomicU64::new(0), last: AtomicI32::new(0) }))
    };
    unsafe { sys::pthread_setspecific(k, s as *mut u8) };
    ProgramThread(s)
}

impl Drop for ProgramThread {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        if let Some(k) = key() {
            unsafe { sys::pthread_setspecific(k, std::ptr::null_mut()) };
        }
        compiler_fence(SeqCst);
        let _kept = crate::zone_alloc::kept();
        drop(unsafe { Box::from_raw(self.0) });
    }
}

/// While it lives, the calling thread is in a compile (or another host runtime entry): a signal
/// for a program handler that comes meanwhile waits for its end. The outermost one's end unblocks
/// what waited, and the kernel delivers it again.
pub(crate) struct Compiling(*const SigState, u32);

pub(crate) fn compiling(slot: u32) -> Compiling {
    let s = state();
    if !s.is_null() {
        unsafe { (*s).depth.fetch_add(1, Relaxed) };
        compiler_fence(SeqCst);
    }
    Compiling(s, slot)
}

impl Drop for Compiling {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        let s = unsafe { &*self.0 };
        compiler_fence(SeqCst);
        let depth = s.depth.load(Relaxed);
        if depth > 1 {
            s.depth.store(depth - 1, Relaxed);
            return;
        }
        // in this order there is no window: once depth is 0 the trampoline runs a handler at once
        // and defers nothing more, then what was deferred is taken, then it is let through
        s.depth.store(0, Relaxed);
        compiler_fence(SeqCst);
        let bits = s.deferred.swap(0, Relaxed);
        if bits == 0 {
            return;
        }
        let waited_us = now_ns().saturating_sub(s.since.load(Relaxed)) / 1000;
        let mut set = [0u64; 16];
        set[0] = bits;
        unsafe { sys::pthread_sigmask(SIG_UNBLOCK, &set, std::ptr::null_mut()) };
        let _kept = crate::zone_alloc::kept();
        crate::inspect::flightrec::event("signal.deferred", &format!("signals {:#x} waited {} us for the compile of slot {}", bits, waited_us, self.1), bits, waited_us);
    }
}

/// A timespec (`clock_gettime`).
#[repr(C)]
struct Ts {
    s: i64,
    ns: i64,
}

fn now_ns() -> u64 {
    let mut t = Ts { s: 0, ns: 0 };
    unsafe { sys::clock_gettime(CLOCK_MONOTONIC, &mut t) };
    t.s as u64 * 1_000_000_000 + t.ns as u64
}

// ------------------------------------------------------------ delivery

/// The kernel's handler for every signal with a program handler. Never JIT code.
extern "C" fn trampoline(sig: i32, info: *mut u8, uc: *mut u8) {
    unsafe { deliver(sig, info, uc, false) };
}

/// fault.rs `chain()`: the host's fault handler passes on a signal it found the trampoline chained
/// for. False: not handled (the host's default action).
pub(crate) unsafe fn chained(sig: i32, info: *mut u8, uc: *mut u8) -> bool {
    deliver(sig, info, uc, true)
}

unsafe fn errno() -> *mut i32 {
    #[cfg(target_os = "linux")]
    return sys::__errno_location();
    #[cfg(target_os = "android")]
    return sys::__errno();
    #[cfg(target_vendor = "apple")]
    return sys::__error();
}

/// No lock, no allocation, no thread-local: only the table, the thread's key value and system calls.
unsafe fn deliver(sig: i32, info: *mut u8, uc: *mut u8, chained: bool) -> bool {
    if sig <= 0 || sig as usize >= NSIG {
        return false;
    }
    let errno = errno();
    let saved = *errno;
    let handled = route(sig, info, uc, chained);
    *errno = saved;
    handled
}

/// A fault the kernel raised at the instruction (not one sent with kill): never deferred.
unsafe fn synchronous(sig: i32, info: *const u8) -> bool {
    if info.is_null() || !(sig == SIGSEGV || sig == SIGBUS || sig == SIGILL || sig == SIGFPE || sig == SIGTRAP || sig == SIGSYS) {
        return false;
    }
    // si_code: > 0 the kernel's own (Darwin's sender codes start at SI_USER = 0x10001)
    let code = *(info.add(8) as *const i32);
    code > 0 && (cfg!(not(target_vendor = "apple")) || code < 0x10000)
}

unsafe fn route(sig: i32, info: *mut u8, uc: *mut u8, chained: bool) -> bool {
    let e = &TABLE[sig as usize];
    let bit = 1u64 << (sig - 1);
    loop {
        let (seq, h, f) = e.read();
        if h == 0 {
            // a disposition change raced this delivery: the kernel runs something else for it now
            if chained {
                return false;
            }
            if !resend_process(sig, info) {
                lost(sig);
            }
            return true;
        }
        let st = state();
        if synchronous(sig, info) {
            if !st.is_null() && (*st).depth.load(Relaxed) == 0 {
                run(sig, h, info, uc, st);
                return true;
            }
            // the host's crash: its default action, at the instruction that faults again
            if !chained {
                kernel(sig, &DFL, std::ptr::null_mut());
            }
            return false;
        }
        if st.is_null() {
            hold(bit, uc);
            if resend_process(sig, info) {
                COUNTS[sig as usize][DEFLECTED].fetch_add(1, Relaxed);
            } else {
                lost(sig);
            }
            return true;
        }
        let st = &*st;
        if st.depth.load(Relaxed) > 0 {
            hold(bit, uc);
            if st.deferred.fetch_or(bit, Relaxed) == 0 {
                st.since.store(now_ns(), Relaxed);
            }
            st.last.store(sig, Relaxed);
            if resend_self(sig, info) {
                COUNTS[sig as usize][DEFERRED].fetch_add(1, Relaxed);
            } else {
                lost(sig);
            }
            return true;
        }
        if f & SA_RESETHAND != 0 && !reset_hand(e, seq, sig, bit) {
            // a writer came first: read it again
            continue;
        }
        run(sig, h, info, uc, st);
        return true;
    }
}

unsafe fn run(sig: i32, h: usize, info: *mut u8, uc: *mut u8, st: *const SigState) {
    (*st).last.store(sig, Relaxed);
    COUNTS[sig as usize][RAN].fetch_add(1, Relaxed);
    // three arguments always (a one-argument handler ignores the others)
    let h: extern "C" fn(i32, *mut u8, *mut u8) = std::mem::transmute(h);
    h(sig, info, uc);
}

/// SA_RESETHAND, applied by the delivery that runs the handler: the entry empties and the kernel's
/// action is SIG_DFL, as the entry read at `seq` still says. False: the entry changed since.
unsafe fn reset_hand(e: &Entry, seq: u32, sig: i32, bit: u64) -> bool {
    let _blocked = Blocked::all();
    if e.seq.compare_exchange(seq, seq + 1, Acquire, Relaxed).is_err() {
        return false;
    }
    fence(Release);
    kernel(sig, &DFL, std::ptr::null_mut());
    e.set_act(&DFL);
    PROGRAM_SET.fetch_and(!bit, Relaxed);
    e.unlock(seq + 1);
    resync();
    true
}

/// The signal stays blocked on this thread: in the interrupted context once the handler returns
/// (uc_sigmask), and in the handler's own from now (SA_NODEFER would take the copy sent on here).
unsafe fn hold(bit: u64, uc: *mut u8) {
    if !uc.is_null() {
        #[cfg(not(target_vendor = "apple"))]
        {
            *(uc.add(UC_SIGMASK) as *mut u64) |= bit;
        }
        #[cfg(target_vendor = "apple")]
        {
            *(uc.add(UC_SIGMASK) as *mut u32) |= bit as u32;
        }
    }
    let mut set = [0u64; 16];
    set[0] = bit;
    sys::pthread_sigmask(SIG_BLOCK, &set, std::ptr::null_mut());
}

/// The signal again, at this thread (Linux: with its siginfo).
unsafe fn resend_self(sig: i32, info: *const u8) -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let tid = sys::syscall(SYS_GETTID);
        sys::syscall(SYS_RT_TGSIGQUEUEINFO, sys::getpid() as i64, tid, sig as i64, info) == 0
    }
    #[cfg(target_vendor = "apple")]
    {
        let _ = info;
        sys::pthread_kill(sys::pthread_self(), sig) == 0
    }
}

/// The signal again, at the process (Linux: with its siginfo). The kernel takes a code a sender
/// could not claim itself (a kill's, a tgkill's, its own) only from the process's main thread:
/// from any other the signal goes on as SI_QUEUE, with its sender, value and other fields kept.
unsafe fn resend_process(sig: i32, info: *const u8) -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let pid = sys::getpid();
        if info.is_null() {
            return sys::kill(pid, sig) == 0;
        }
        let mut copy = [0u64; 16];
        std::ptr::copy_nonoverlapping(info, copy.as_mut_ptr().cast::<u8>(), 128);
        let code = copy.as_mut_ptr().cast::<u8>().add(8) as *mut i32;
        if (*code >= 0 || *code == SI_TKILL) && sys::syscall(SYS_GETTID) != pid as i64 {
            *code = SI_QUEUE;
        }
        sys::syscall(SYS_RT_SIGQUEUEINFO, pid as i64, sig as i64, copy.as_ptr()) == 0
    }
    #[cfg(target_vendor = "apple")]
    {
        let _ = info;
        sys::kill(sys::getpid(), sig) == 0
    }
}

/// A signal sent on that the kernel did not take (its queue of pending signals is full): counted
/// and said, never retried.
fn lost(sig: i32) {
    COUNTS[sig as usize][LOST].fetch_add(1, Relaxed);
    let mut line = *b"relax: signal 00 for the program was lost: the queue of pending signals is full\n";
    line[20] = b'0' + (sig / 10) as u8;
    line[21] = b'0' + (sig % 10) as u8;
    crate::os::fault::raw_stderr(&line);
}

// ------------------------------------------------------------ the signal thread

/// The program-handled set changed: the signal thread sets its mask again.
fn resync() {
    GEN.fetch_add(1, Release);
    futex_wake(&GEN);
}

/// Starts the signal thread (once; with the compiler's owner): a host thread whose mask is always
/// the program-handled set, so every other signal finds it open.
pub(crate) fn start() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = key();
        let _kept = crate::zone_alloc::kept();
        let _ = std::thread::Builder::new().name("run-signals".into()).spawn(|| loop {
            let g = GEN.load(Acquire);
            let set = PROGRAM_SET.load(Relaxed);
            let mut mask = [0u64; 16];
            mask[0] = set;
            unsafe { sys::pthread_sigmask(SIG_SETMASK, &mask, std::ptr::null_mut()) };
            APPLIED.store(set, Relaxed);
            APPLIED_GEN.fetch_add(1, Release);
            futex_wake(&APPLIED_GEN);
            futex_wait(&GEN, g);
        });
    });
}

/// Waits while `w` is `v` (a wake or a change ends it, or nothing at all: callers check again).
pub(crate) fn futex_wait(w: &AtomicU32, v: u32) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        sys::syscall(SYS_FUTEX, w.as_ptr(), FUTEX_WAIT, v, std::ptr::null::<u8>())
    };
    #[cfg(target_vendor = "apple")]
    unsafe {
        sys::__ulock_wait(UL_WAIT, w.as_ptr() as *mut u8, v as u64, 0)
    };
}

pub(crate) fn futex_wake(w: &AtomicU32) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        sys::syscall(SYS_FUTEX, w.as_ptr(), FUTEX_WAKE, i32::MAX)
    };
    #[cfg(target_vendor = "apple")]
    unsafe {
        sys::__ulock_wake(UL_WAKE_ALL, w.as_ptr() as *mut u8, 0)
    };
}

/// The signal thread's mask once it satisfies `want` (it resyncs after each change).
#[cfg(test)]
pub(crate) fn applied_mask(want: impl Fn(u64) -> bool) -> u64 {
    loop {
        let g = APPLIED_GEN.load(Acquire);
        let set = APPLIED.load(Relaxed);
        if want(set) {
            return set;
        }
        futex_wait(&APPLIED_GEN, g);
    }
}

// ------------------------------------------------------------ the compile hook of the tests

/// RELAX_TEST_SIGNAL_IN_COMPILE's send, from inside the compile of `name` on a program
/// thread (jit.rs `test_signal_in_compile`): signal `sig` `to` this thread (`self`), the process
/// (`process`), this thread with the program's handler put straight into the kernel (`raw-self`,
/// what the trampoline never allows: the re-entry check's case), or three times queued with the
/// values 1, 2, 3 (`rt3-self`, Linux). `block`: the compile then never ends (a hung compile).
pub(crate) unsafe fn test_send(sig: i32, to: &str, block: bool, name: &str) {
    match to {
        "self" => {
            sys::pthread_kill(sys::pthread_self(), sig);
        }
        "process" => {
            sys::kill(sys::getpid(), sig);
        }
        "raw-self" if sig > 0 && (sig as usize) < NSIG => {
            let mut k = TABLE[sig as usize].act();
            let f = flags(&k);
            set_flags(&mut k, (f | SA_SIGINFO) & !SA_RESETHAND);
            kernel(sig, &k, std::ptr::null_mut());
            sys::pthread_kill(sys::pthread_self(), sig);
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        "rt3-self" => {
            for value in 1..=3u64 {
                // siginfo: si_signo, si_errno, si_code (SI_QUEUE), then si_pid, si_uid, si_value
                let mut info = [0u64; 16];
                let p = info.as_mut_ptr() as *mut u8;
                *(p as *mut i32) = sig;
                *(p.add(8) as *mut i32) = SI_QUEUE;
                *(p.add(16) as *mut i32) = sys::getpid();
                *(p.add(20) as *mut u32) = sys::getuid();
                *(p.add(24) as *mut u64) = value;
                let tid = sys::syscall(SYS_GETTID);
                sys::syscall(SYS_RT_TGSIGQUEUEINFO, sys::getpid() as i64, tid, sig as i64, p);
            }
        }
        _ => panic!("RELAX_TEST_SIGNAL_IN_COMPILE: unknown target `{to}`"),
    }
    eprintln!("relax test: sent signal {sig} ({to}) in the compile of `{name}`; its handler ran {} times so far", counts(sig)[RAN]);
    if block {
        let mut fds = [0i32; 2];
        sys::pipe(fds.as_mut_ptr());
        let mut b = 0u8;
        // nothing writes to it; a signal that interrupts the read (inspect's capture) reads again
        loop {
            sys::read(fds[0], (&mut b as *mut u8).cast(), 1);
        }
    }
}
