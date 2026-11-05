//! The process's threads as the OS sees them, and their stacks captured from another thread
//! without the owner (DESIGN.md §3).
//!
//! - `list()`: every thread: OS id, pthread handle, name, run state, CPU time.
//! - `capture(t)`: the thread's pcs, innermost first. macOS suspends it (Mach) and reads its
//!   registers; Windows suspends it too (SuspendThread + GetThreadContext); Linux and Android
//!   interrupt it with a real-time signal whose handler hands its registers over and waits until
//!   the walk is done. Either way the thread stands still while its stack is walked, and nothing
//!   is allocated or locked meanwhile (it may hold malloc's or the loader's lock): pcs go to a
//!   fixed array, names are looked up after it runs again.
//!
//! The walk: JIT code keeps frame pointers (every target); host code on macOS arm64 too (the
//! platform ABI) and on Android arm64 (frame records); host code on Linux x86_64 is stepped by
//! its DWARF CFI (cfi.rs), on Windows x64 by its PE unwind info (pe_unwind.rs). A program
//! thread's walk crosses the switch to its program stack (relax_base::stack::switch_return_pc).
//! Every memory read goes through a call that fails instead of faulting.

/// Most frames kept per stack.
pub const MAX_FRAMES: usize = 128;

#[derive(Debug)]
pub struct Thread {
    /// the OS's id: the Mach thread id (macOS), the tid (Linux), the thread id (Windows)
    pub os_id: u64,
    /// the pthread handle (what owner / waits name a thread by; Windows: the thread id); 0: not
    /// known yet
    pub handle: u64,
    pub name: String,
    /// running / runnable / waiting / stopped / uninterruptible / zombie
    pub state: &'static str,
    /// user + system CPU time
    pub cpu_ns: u64,
    /// the stack's [lo, hi) when known (0, 0 otherwise)
    pub stack: (u64, u64),
    /// Linux: the syscall it blocks in (`/proc/.../syscall`): number, first argument (a futex
    /// wait's address), user sp and pc
    pub syscall: Option<(i64, u64, u64, u64)>,
    #[cfg(target_vendor = "apple")]
    port: u32,
}

/// A copy for reports: it does not own the Mach port (only the listed original releases it).
impl Clone for Thread {
    fn clone(&self) -> Thread {
        Thread {
            os_id: self.os_id,
            handle: self.handle,
            name: self.name.clone(),
            state: self.state,
            cpu_ns: self.cpu_ns,
            stack: self.stack,
            syscall: self.syscall,
            #[cfg(target_vendor = "apple")]
            port: 0,
        }
    }
}

/// A captured stack: raw pcs (the first is the interrupted instruction, the rest return
/// addresses) and, on arm64, the link register of the top frame (the caller of a leaf that has
/// no frame record of its own; kept when it names another fn than the frames around it).
pub struct Raw {
    pub pcs: [u64; MAX_FRAMES],
    pub n: usize,
    pub lr: u64,
    pub sp: u64,
    /// the thread's pthread handle as the thread itself reported it (Linux capture), else 0
    pub handle: u64,
    /// why the walk ended at the last frame when that frame is not the stack's outermost: the
    /// step to its caller could not be proven (a walk never guesses a caller)
    pub stop: Option<&'static str>,
}

impl Raw {
    pub fn new() -> Raw {
        Raw { pcs: [0; MAX_FRAMES], n: 0, lr: 0, sp: 0, handle: 0, stop: None }
    }
    pub fn frames(&self) -> &[u64] {
        &self.pcs[..self.n]
    }
}

pub use imp::{list, regs_now, tid_of_handle};

thread_local! {
    static SELF_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The calling thread's OS id (cached per thread: no system call after the first).
pub fn self_os_id() -> u64 {
    SELF_ID
        .try_with(|c| {
            if c.get() == 0 {
                c.set(imp::self_os_id());
            }
            c.get()
        })
        .unwrap_or_else(|_| imp::self_os_id())
}

/// Captures thread `t`'s stack (see the module comment). Never the calling thread: a thread that
/// suspends itself (macOS) never runs again, one that signals itself (Linux) waits for itself.
pub fn capture(t: &Thread, raw: &mut Raw) -> Result<(), &'static str> {
    raw.n = 0;
    raw.stop = None;
    if t.os_id == self_os_id() {
        return Err("the calling thread (not captured)");
    }
    crate::inspect::activity::phase("capturing a stack of", t.os_id);
    let r = imp::capture(t, raw);
    crate::inspect::activity::phase("running", 0);
    r
}
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use imp::{stack_hi_of_self, walk_regs_with, Slot as HoldSlot};

/// Pause: holds thread `tid` in its handler until `release_hold` (Linux).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn hold_paused(tid: u64) -> Result<&'static HoldSlot, &'static str> {
    imp::hold(tid, imp::HOLD_PAUSE)
}

/// Holds thread `tid` in its handler for good (a thread with a frame in code that never runs
/// again: stale.rs `park_held`); a blocked thread is signalled too (it never returns from the
/// handler, so an interrupted system call never comes back to its caller). Linux.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn hold_for_good(tid: u64) -> Result<&'static HoldSlot, &'static str> {
    imp::hold(tid, imp::HOLD_FOREVER)
}

/// A pause's hold of a thread becomes a hold for good (`release_hold` is never called for it).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn keep_held(s: &HoldSlot) {
    imp::keep(s)
}

/// Whether thread `tid` stands still in its handler now (held by a pause or for good). Linux.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn is_held(tid: u64) -> bool {
    imp::held(tid).is_some()
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn release_hold(s: &HoldSlot) {
    imp::release(s)
}

/// Up to `len` bytes of this process's memory at `addr`, or None where it is not readable (a
/// read that fails instead of faulting: process_vm_readv / mach_vm_read_overwrite /
/// ReadProcessMemory).
pub fn read_mem(addr: u64, len: usize) -> Option<Vec<u8>> {
    #[cfg_attr(all(windows, not(target_arch = "x86_64")), allow(unused_mut))]
    let mut v = vec![0u8; len];
    if len == 0 {
        return Some(v);
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        extern "C" {
            fn syscall(n: i64, ...) -> i64;
            fn getpid() -> i32;
        }
        // process_vm_readv
        const SYS: i64 = if cfg!(target_arch = "aarch64") { 270 } else { 310 };
        let local = [v.as_mut_ptr() as usize, len];
        let remote = [addr as usize, len];
        let n = unsafe { syscall(SYS, getpid(), local.as_ptr(), 1u64, remote.as_ptr(), 1u64, 0u64) };
        (n == len as i64).then_some(v)
    }
    #[cfg(target_vendor = "apple")]
    {
        extern "C" {
            static mach_task_self_: u32;
            fn mach_vm_read_overwrite(task: u32, addr: u64, size: u64, data: u64, out: *mut u64) -> i32;
        }
        let mut got = 0u64;
        let r = unsafe { mach_vm_read_overwrite(mach_task_self_, addr, len as u64, v.as_mut_ptr() as u64, &mut got) };
        (r == 0 && got == len as u64).then_some(v)
    }
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        let mut got = 0usize;
        let ok = unsafe { makepad_windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory(makepad_windows_sys::Win32::System::Threading::GetCurrentProcess(), addr as *const std::ffi::c_void, v.as_mut_ptr() as *mut std::ffi::c_void, len, Some(&mut got)) }.is_ok();
        (ok && got == len).then_some(v)
    }
    #[cfg(all(windows, not(target_arch = "x86_64")))]
    {
        let _ = addr;
        None
    }
}

/// Pause: Mach suspend / resume of one thread (macOS; suspends nest); its run state.
#[cfg(target_vendor = "apple")]
pub use imp::{resume, running, suspend};

/// Walks frame records from `fp` (fp rule: [fp] = caller's fp, [fp+8] = return address), appending
/// to `raw` until the chain ends, leaves the stack upwards order or `raw` is full.
#[cfg(target_vendor = "apple")]
fn walk_fp(raw: &mut Raw, mut fp: u64, read: &dyn Fn(u64) -> Option<u64>) {
    while raw.n < MAX_FRAMES && fp != 0 && fp % 8 == 0 {
        let (Some(ra), Some(next)) = (read(fp + 8), read(fp)) else { break };
        if ra == 0 {
            break;
        }
        // pointer authentication bits (arm64e system libraries)
        let ra = ra & 0x0000_ffff_ffff_ffff;
        raw.pcs[raw.n] = ra;
        raw.n += 1;
        // records go up the stack, except across the program stack's switch (its frame record is
        // on the thread's own stack, the callee's on the segment)
        if next <= fp && ra != crate::stack::switch_return_pc() {
            break;
        }
        fp = next;
    }
}

#[cfg(target_vendor = "apple")]
mod imp {
    use super::{Raw, Thread};

    type Port = u32;
    extern "C" {
        static mach_task_self_: Port;
        fn task_threads(task: Port, list: *mut *mut Port, count: *mut u32) -> i32;
        fn thread_info(t: Port, flavor: u32, out: *mut u32, count: *mut u32) -> i32;
        fn thread_suspend(t: Port) -> i32;
        fn thread_resume(t: Port) -> i32;
        fn thread_get_state(t: Port, flavor: i32, state: *mut u32, count: *mut u32) -> i32;
        fn mach_port_deallocate(task: Port, name: Port) -> i32;
        fn vm_deallocate(task: Port, addr: usize, size: usize) -> i32;
        fn mach_vm_read_overwrite(task: Port, addr: u64, size: u64, data: u64, out: *mut u64) -> i32;
        fn mach_thread_self() -> Port;
        fn pthread_get_stackaddr_np(t: usize) -> *mut u8;
        fn pthread_get_stacksize_np(t: usize) -> usize;
        fn pthread_from_mach_thread_np(t: Port) -> usize;
    }
    const THREAD_IDENTIFIER_INFO: u32 = 4;
    const THREAD_EXTENDED_INFO: u32 = 5;

    fn task() -> Port {
        unsafe { mach_task_self_ }
    }

    /// The calling thread's Mach thread id.
    /// (macOS lists the handle with each thread already)
    pub fn tid_of_handle(_h: u64) -> Option<u64> {
        None
    }

    pub fn self_os_id() -> u64 {
        let port = unsafe { mach_thread_self() };
        let id = ident(port).0;
        unsafe { mach_port_deallocate(task(), port) };
        id
    }

    /// (thread id, pthread handle)
    fn ident(port: Port) -> (u64, u64) {
        let mut info = [0u32; 6];
        let mut count = 6u32;
        if unsafe { thread_info(port, THREAD_IDENTIFIER_INFO, info.as_mut_ptr(), &mut count) } != 0 {
            return (0, 0);
        }
        let w = |k: usize| (info[2 * k] as u64) | ((info[2 * k + 1] as u64) << 32);
        (w(0), w(1))
    }

    pub fn list() -> Vec<Thread> {
        let mut out = Vec::new();
        let mut ports: *mut Port = std::ptr::null_mut();
        let mut count = 0u32;
        if unsafe { task_threads(task(), &mut ports, &mut count) } != 0 {
            return out;
        }
        for i in 0..count as usize {
            let port = unsafe { *ports.add(i) };
            // THREAD_IDENTIFIER_INFO's thread_handle is the thread's TSD base, not its pthread_t:
            // the handle the rest of Relax names threads by comes from libpthread
            let os_id = ident(port).0;
            let handle = unsafe { pthread_from_mach_thread_np(port) } as u64;
            // thread_extended_info: user/system ns, cpu usage, policy, run state, flags,
            // interruptible, cur/base/max priority, name[64]
            let mut ext = [0u32; 28];
            let mut n = 28u32;
            let (mut name, mut state, mut cpu_ns) = (String::new(), "?", 0u64);
            if unsafe { thread_info(port, THREAD_EXTENDED_INFO, ext.as_mut_ptr(), &mut n) } == 0 {
                let w = |k: usize| (ext[2 * k] as u64) | ((ext[2 * k + 1] as u64) << 32);
                cpu_ns = w(0) + w(1);
                state = match ext[6] {
                    1 => "running",
                    2 => "stopped",
                    3 => "waiting",
                    4 => "uninterruptible",
                    5 => "halted",
                    _ => "?",
                };
                let bytes: Vec<u8> = ext[12..28].iter().flat_map(|w| w.to_ne_bytes()).collect();
                let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                name = String::from_utf8_lossy(&bytes[..len]).into_owned();
            }
            let stack = if handle != 0 {
                let hi = unsafe { pthread_get_stackaddr_np(handle as usize) } as u64;
                let size = unsafe { pthread_get_stacksize_np(handle as usize) } as u64;
                (hi.saturating_sub(size), hi)
            } else {
                (0, 0)
            };
            out.push(Thread { os_id, handle, name, state, cpu_ns, stack, syscall: None, port });
        }
        unsafe { vm_deallocate(task(), ports as usize, count as usize * 4) };
        out
    }

    impl Drop for Thread {
        fn drop(&mut self) {
            if self.port != 0 {
                unsafe { mach_port_deallocate(task(), self.port) };
            }
        }
    }

    /// Is the thread on a CPU now (THREAD_BASIC_INFO run state)? The profiler samples only these.
    pub fn running(t: &Thread) -> bool {
        // thread_basic_info: user_time(2), system_time(2), cpu_usage, policy, run_state, ...
        let mut info = [0u32; 10];
        let mut n = 10u32;
        t.port != 0 && unsafe { thread_info(t.port, 3, info.as_mut_ptr(), &mut n) } == 0 && info[6] == 1
    }

    pub fn suspend(t: &Thread) -> bool {
        t.port != 0 && t.os_id != super::self_os_id() && unsafe { thread_suspend(t.port) } == 0
    }

    pub fn resume(t: &Thread) {
        if t.port != 0 {
            unsafe { thread_resume(t.port) };
        }
    }

    /// Reads 8 bytes of this process's memory; None where nothing is mapped (a Mach call: it
    /// fails instead of faulting, and takes no user-space lock).
    fn read(addr: u64) -> Option<u64> {
        let mut v = 0u64;
        let mut got = 0u64;
        let r = unsafe { mach_vm_read_overwrite(task(), addr, 8, &mut v as *mut u64 as u64, &mut got) };
        (r == 0 && got == 8).then_some(v)
    }

    pub fn capture(t: &Thread, raw: &mut Raw) -> Result<(), &'static str> {
        raw.n = 0;
        // ARM_THREAD_STATE64 (flavor 6): x0..x28, fp, lr, sp, pc, cpsr, pad = 68 words
        let mut st = [0u32; 68];
        let mut count = 68u32;
        unsafe {
            if thread_suspend(t.port) != 0 {
                return Err("thread_suspend failed (the thread ended)");
            }
            let r = thread_get_state(t.port, 6, st.as_mut_ptr(), &mut count);
            if r == 0 {
                let w = |k: usize| (st[2 * k] as u64) | ((st[2 * k + 1] as u64) << 32);
                let strip = |a: u64| a & 0x0000_ffff_ffff_ffff;
                raw.pcs[0] = strip(w(32));
                raw.n = 1;
                raw.lr = strip(w(30));
                raw.sp = w(31);
                super::walk_fp(raw, w(29), &read);
            }
            thread_resume(t.port);
            if r != 0 {
                return Err("thread_get_state failed");
            }
        }
        Ok(())
    }

    /// The thread's pc, frame pointer and callee-saved integer registers (x19..x28 as the panic
    /// trampoline's block holds them: indices 2..), read while it is suspended.
    pub fn regs_now(t: &Thread) -> Option<(u64, u64, Vec<(usize, u64)>)> {
        let mut st = [0u32; 68];
        let mut count = 68u32;
        unsafe {
            if thread_suspend(t.port) != 0 {
                return None;
            }
            let r = thread_get_state(t.port, 6, st.as_mut_ptr(), &mut count);
            thread_resume(t.port);
            if r != 0 {
                return None;
            }
        }
        let w = |k: usize| (st[2 * k] as u64) | ((st[2 * k + 1] as u64) << 32);
        let ints = (19..=28).map(|r| (2 + (r - 19), w(r))).collect();
        Some((w(32) & 0x0000_ffff_ffff_ffff, w(29), ints))
    }
}

#[cfg(any(target_os = "linux", all(target_os = "android", target_arch = "aarch64")))]
mod imp {
    use super::{Raw, Thread};
    use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering::{AcqRel, Acquire, Relaxed, Release}};

    extern "C" {
        fn syscall(n: i64, ...) -> i64;
        fn getpid() -> i32;
        fn pthread_self() -> usize;
        fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
        fn __libc_current_sigrtmin() -> i32;
        fn __libc_current_sigrtmax() -> i32;
    }
    #[cfg(target_arch = "x86_64")]
    const SYS_GETTID: i64 = 186;
    #[cfg(target_arch = "x86_64")]
    const SYS_TGKILL: i64 = 234;
    #[cfg(target_arch = "x86_64")]
    const SYS_FUTEX: i64 = 202;
    #[cfg(target_arch = "x86_64")]
    const SYS_PROCESS_VM_READV: i64 = 310;
    #[cfg(target_arch = "aarch64")]
    const SYS_GETTID: i64 = 178;
    #[cfg(target_arch = "aarch64")]
    const SYS_TGKILL: i64 = 131;
    #[cfg(target_arch = "aarch64")]
    const SYS_FUTEX: i64 = 98;
    #[cfg(target_arch = "aarch64")]
    const SYS_PROCESS_VM_READV: i64 = 270;

    /// glibc's struct sigaction
    #[cfg(not(target_os = "android"))]
    #[repr(C)]
    struct SigAction {
        handler: usize,
        mask: [u64; 16],
        flags: i32,
        pad: i32,
        restorer: usize,
    }
    #[cfg(not(target_os = "android"))]
    fn sig_action(handler: usize, flags: i32) -> SigAction {
        SigAction { handler, mask: [0; 16], flags, pad: 0, restorer: 0 }
    }

    /// bionic's struct sigaction (LP64)
    #[cfg(target_os = "android")]
    #[repr(C)]
    struct SigAction {
        flags: i32,
        pad: i32,
        handler: usize,
        mask: u64,
        restorer: usize,
    }
    #[cfg(target_os = "android")]
    fn sig_action(handler: usize, flags: i32) -> SigAction {
        SigAction { flags, pad: 0, handler, mask: 0, restorer: 0 }
    }

    pub fn self_os_id() -> u64 {
        unsafe { syscall(SYS_GETTID) as u64 }
    }

    /// The tid of the thread whose pthread handle is `h`: the C library keeps it in its thread
    /// descriptor (glibc `struct pthread`, bionic `pthread_internal_t`: next, prev, tid), which the
    /// handle points to, at a fixed offset, found once by checking the calling thread's own
    /// descriptor. None when the layout is not the expected one.
    pub fn tid_of_handle(h: u64) -> Option<u64> {
        static OFF: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
        #[cfg(not(target_os = "android"))]
        const OFFSETS: [u64; 4] = [0x2d0, 0x2d0 + 8, 0x2c0, 0x2e0];
        #[cfg(target_os = "android")]
        const OFFSETS: [u64; 1] = [16];
        let off = (*OFF.get_or_init(|| {
            let me = unsafe { pthread_self() } as u64;
            let tid = self_os_id();
            OFFSETS.into_iter().find(|&o| read(me + o).map_or(false, |v| v as u32 as u64 == tid))
        }))?;
        if h == 0 {
            return None;
        }
        read(h + off).map(|v| v as u32 as u64).filter(|&t| t != 0)
    }

    pub fn list() -> Vec<Thread> {
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir("/proc/self/task") else { return out };
        let tick_ns = 1_000_000_000 / 100; // USER_HZ
        for e in dir.flatten() {
            let Some(tid) = e.file_name().to_str().and_then(|s| s.parse::<u64>().ok()) else { continue };
            let base = format!("/proc/self/task/{}", tid);
            let name = std::fs::read_to_string(format!("{}/comm", base)).unwrap_or_default().trim_end().to_string();
            let stat = std::fs::read_to_string(format!("{}/stat", base)).unwrap_or_default();
            // after the ")" that ends the name: state, then fields 4.. ; utime is field 14
            let rest = stat.rsplit_once(')').map(|x| x.1).unwrap_or("");
            let f: Vec<&str> = rest.split_whitespace().collect();
            let state = match f.first().copied() {
                Some("R") => "running",
                Some("S") => "waiting",
                Some("D") => "uninterruptible",
                Some("T") | Some("t") => "stopped",
                Some("Z") => "zombie",
                _ => "?",
            };
            let num = |k: usize| f.get(k).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
            let cpu_ns = (num(11) + num(12)) * tick_ns;
            let syscall = read_syscall(tid);
            out.push(Thread { os_id: tid, handle: 0, name, state, cpu_ns, stack: (0, 0), syscall });
        }
        out.sort_by_key(|t| t.os_id);
        out
    }

    /// `/proc/self/task/<tid>/syscall`: "nr a1..a6 sp pc" while it blocks in a syscall
    fn read_syscall(tid: u64) -> Option<(i64, u64, u64, u64)> {
        let s = std::fs::read_to_string(format!("/proc/self/task/{}/syscall", tid)).ok()?;
        let w: Vec<&str> = s.split_whitespace().collect();
        if w.len() < 9 {
            return None;
        }
        let n = w[0].parse::<i64>().ok()?;
        let hex = |x: &str| u64::from_str_radix(x.trim_start_matches("0x"), 16).ok();
        Some((n, hex(w[1])?, hex(w[7])?, hex(w[8])?))
    }

    // ---- holds: a thread stands still in the capture signal's handler
    //
    // A slot per held thread. State: 0 free; 1 asked (signal sent); 5 the handler claimed it;
    // 2 registers ready, the thread waits; 3 released by the inspector; 4 the handler left.
    // A capture uses a slot for the time of one walk; `pause` keeps slots until `resume`.
    const SLOTS: usize = 256;
    pub struct Slot {
        state: AtomicU32,
        tid: AtomicI32,
        /// x86_64: rip, rsp, rbp, rbx, r12, r13, r14, r15; arm64: pc, sp, fp, lr, -, -, -, -;
        /// then the pthread handle
        regs: [AtomicU64; 9],
        /// HOLD_CAPTURE (one walk, at most 5 s), HOLD_PAUSE (until `resume`, at most 10 minutes),
        /// HOLD_FOREVER (a thread that must never run on: stale.rs `park_held`)
        mode: AtomicU32,
    }
    static HOLDS: [Slot; SLOTS] = [const { Slot { state: AtomicU32::new(0), tid: AtomicI32::new(0), regs: [const { AtomicU64::new(0) }; 9], mode: AtomicU32::new(0) } }; SLOTS];
    pub const HOLD_CAPTURE: u32 = 0;
    pub const HOLD_PAUSE: u32 = 1;
    // a hold for good: Linux x86_64 only (`hold_for_good`, `keep_held`)
    #[cfg(target_arch = "x86_64")]
    pub const HOLD_FOREVER: u32 = 2;
    static SIGNAL: AtomicI32 = AtomicI32::new(0);

    fn futex_wait(w: &AtomicU32, val: u32, ms: u64) {
        let ts = [(ms / 1000) as i64, ((ms % 1000) * 1_000_000) as i64];
        unsafe { syscall(SYS_FUTEX, w.as_ptr(), 0 | 128, val, ts.as_ptr()) }; // FUTEX_WAIT_PRIVATE
    }

    fn futex_wake(w: &AtomicU32) {
        unsafe { syscall(SYS_FUTEX, w.as_ptr(), 1 | 128, i32::MAX) }; // FUTEX_WAKE_PRIVATE
    }

    /// The capture signal's handler, on the interrupted thread: hand over the registers, stand
    /// still until released (a capture's walk, or a pause until `resume`). Async-signal-safe:
    /// atomics and futex syscalls only.
    extern "C" fn on_capture(_sig: i32, _info: *mut u8, uc: *mut u8) {
        unsafe {
            let tid = syscall(SYS_GETTID) as i32;
            // the slot asked for this thread (a late signal of a capture that gave up finds none)
            let Some(slot) = HOLDS.iter().find(|h| h.tid.load(Acquire) == tid && h.state.compare_exchange(1, 5, AcqRel, Relaxed).is_ok()) else { return };
            #[cfg(target_arch = "x86_64")]
            {
                let gregs = uc.add(40) as *const u64;
                // REG_RIP 16, REG_RSP 15, REG_RBP 10, REG_RBX 11, REG_R12..R15 4..7
                for (k, g) in [16usize, 15, 10, 11, 4, 5, 6, 7].iter().enumerate() {
                    slot.regs[k].store(*gregs.add(*g), Relaxed);
                }
            }
            #[cfg(target_arch = "aarch64")]
            {
                // uc_mcontext at 176 (flags, link, stack, 128-byte sigmask): fault_address,
                // x0..x30, sp, pc
                let mc = uc.add(176) as *const u64;
                for (k, g) in [33usize, 32, 30, 31].iter().enumerate() {
                    slot.regs[k].store(*mc.add(*g), Relaxed);
                }
            }
            slot.regs[8].store(pthread_self() as u64, Relaxed);
            slot.state.store(2, Release);
            futex_wake(&slot.state);
            // a capture's hold is bounded (an inspector that died mid-walk must not stop this
            // thread forever); a pause's lasts until resume, or 10 minutes
            let t0 = std::time::Instant::now();
            loop {
                if slot.state.load(Acquire) != 2 {
                    break;
                }
                let limit = match slot.mode.load(Relaxed) {
                    HOLD_CAPTURE => 5,
                    HOLD_PAUSE => 600,
                    _ => u64::MAX,
                };
                if t0.elapsed().as_secs() >= limit {
                    break;
                }
                futex_wait(&slot.state, 2, 100);
            }
            slot.state.store(4, Release);
            futex_wake(&slot.state);
        }
    }

    /// Asks thread `tid` to stand still in its handler; Some(slot) once its registers are in,
    /// None when it did not answer in time (signals blocked, uninterruptible, ended).
    pub fn hold(tid: u64, mode: u32) -> Result<&'static Slot, &'static str> {
        if tid == super::self_os_id() {
            return Err("the calling thread (not held)");
        }
        let sig = install();
        if sig == 0 {
            return Err("no free real-time signal for stack capture");
        }
        let Some(slot) = HOLDS.iter().find(|h| h.state.compare_exchange(0, 1, AcqRel, Relaxed).is_ok()) else { return Err("too many held threads") };
        slot.mode.store(mode, Relaxed);
        slot.tid.store(tid as i32, Release);
        if unsafe { syscall(SYS_TGKILL, getpid(), tid as i32, sig) } != 0 {
            slot.tid.store(0, Relaxed);
            slot.state.store(0, Release);
            return Err("the thread ended");
        }
        let t0 = std::time::Instant::now();
        loop {
            match slot.state.load(Acquire) {
                2 => return Ok(slot),
                1 if t0.elapsed().as_millis() >= 250 => {
                    if slot.state.compare_exchange(1, 0, AcqRel, Relaxed).is_ok() {
                        slot.tid.store(0, Relaxed);
                        return Err("no answer in 250 ms (signals blocked or uninterruptible)");
                    }
                }
                st @ (1 | 5) => futex_wait(&slot.state, st, 10),
                _ => return Err("capture handshake lost"),
            }
        }
    }

    /// Lets a held thread go and waits until it left its handler.
    pub fn release(slot: &Slot) {
        slot.state.store(3, Release);
        futex_wake(&slot.state);
        let t1 = std::time::Instant::now();
        while slot.state.load(Acquire) == 3 && t1.elapsed().as_millis() < 1000 {
            futex_wait(&slot.state, 3, 10);
        }
        slot.tid.store(0, Relaxed);
        slot.mode.store(HOLD_CAPTURE, Relaxed);
        // (a free slot keeps none of the thread's registers: they name its stack and its heap)
        for r in &slot.regs {
            r.store(0, Relaxed);
        }
        slot.state.store(0, Release);
    }

    /// A held thread stays held for good.
    #[cfg(target_arch = "x86_64")]
    pub fn keep(slot: &Slot) {
        slot.mode.store(HOLD_FOREVER, Relaxed);
    }

    /// The slot holding `tid` now (a paused thread), if any.
    pub fn held(tid: u64) -> Option<&'static Slot> {
        HOLDS.iter().find(|h| h.tid.load(Acquire) == tid as i32 && h.state.load(Acquire) == 2)
    }

    /// A paused thread's pc, frame pointer and callee-saved integer registers (x86_64: rbx,
    /// r12..r15 as the panic trampoline's block holds them: indices 2..6). None for a thread
    /// blocked in a syscall: a pause never signals one (its stack stands still while it blocks),
    /// and the syscall's sp and pc do not give its frame pointer.
    pub fn regs_now(t: &Thread) -> Option<(u64, u64, Vec<(usize, u64)>)> {
        let slot = held(t.os_id)?;
        let r = |k: usize| slot.regs[k].load(Relaxed);
        #[cfg(target_arch = "x86_64")]
        return Some((r(0), r(2), (3..8).map(|k| (k - 1, r(k))).collect()));
        #[cfg(not(target_arch = "x86_64"))]
        return Some((r(0), r(2), Vec::new()));
    }

    #[cfg(target_arch = "x86_64")]
    fn walk_slot(slot: &Slot, raw: &mut Raw) {
        let r = |k: usize| slot.regs[k].load(Relaxed);
        let mut regs = super::super::cfi::Regs { rip: r(0), rsp: r(1), rbp: r(2), rbx: r(3), r12: r(4), r13: r(5), r14: r(6), r15: r(7) };
        raw.handle = r(8);
        raw.sp = regs.rsp;
        walk(raw, &mut regs);
    }

    /// Installs the capture signal's handler (once, on the inspect thread's start): the first
    /// real-time signal from SIGRTMIN+6 nobody handles. Returns the signal number.
    pub fn install() -> i32 {
        let s = SIGNAL.load(Relaxed);
        if s != 0 {
            return s;
        }
        unsafe {
            let (lo, hi) = (__libc_current_sigrtmin(), __libc_current_sigrtmax());
            for sig in (lo + 6)..=hi {
                let mut old = sig_action(0, 0);
                if sigaction(sig, std::ptr::null(), &mut old) != 0 || old.handler != 0 {
                    continue;
                }
                // SA_SIGINFO | SA_RESTART | SA_ONSTACK: interrupted syscalls resume
                let act = sig_action(on_capture as *const () as usize, 4 | 0x1000_0000 | 0x0800_0000);
                if sigaction(sig, &act, std::ptr::null_mut()) == 0 {
                    SIGNAL.store(sig, Relaxed);
                    return sig;
                }
            }
        }
        0
    }

    /// Reads 8 bytes of this process's memory; None where nothing is mapped (process_vm_readv
    /// fails instead of faulting).
    fn read(addr: u64) -> Option<u64> {
        let mut v = 0u64;
        let local = [&mut v as *mut u64 as usize, 8usize];
        let remote = [addr as usize, 8usize];
        let n = unsafe { syscall(SYS_PROCESS_VM_READV, getpid(), local.as_ptr(), 1u64, remote.as_ptr(), 1u64, 0u64) };
        (n == 8).then_some(v)
    }

    /// Why a walk of a thread blocked in a syscall ends where its frame pointer is needed (each
    /// `Raw::stop` says why the walk ends at its last frame: "walk stops at <pc>: <why>").
    pub const BLOCKED_NO_FP: &str = "its caller is found through the frame pointer, which a thread blocked in a syscall does not give (it is walked from the syscall's sp and pc only, not signalled)";

    pub fn capture(t: &Thread, raw: &mut Raw) -> Result<(), &'static str> {
        raw.n = 0;
        // a paused thread already stands still in its handler: walked from the registers it was
        // interrupted with (its own syscall now is the handler's futex wait: a walk from there
        // has to step through the signal frame, and lost the program frames beyond it)
        if let Some(slot) = held(t.os_id) {
            walk_slot(slot, raw);
            return Ok(());
        }
        // a thread blocked in a syscall is never signalled (third-party code may not retry an
        // EINTR; an audio thread must not be held): its stack stands still while it blocks, so
        // it is walked from the syscall's sp and pc, and the walk is kept if it still blocks
        // there afterwards
        if t.state != "running" {
            if let Some((nr, a0, sp, pc)) = read_syscall(t.os_id) {
                walk_blocked(raw, sp, pc);
                if read_syscall(t.os_id) == Some((nr, a0, sp, pc)) {
                    return Ok(());
                }
                raw.n = 0;
                raw.stop = None;
            }
        }
        let slot = hold(t.os_id, HOLD_CAPTURE)?;
        walk_slot(slot, raw);
        release(slot);
        Ok(())
    }

    /// A thread blocked in a syscall: walked from the syscall's sp and pc.
    #[cfg(target_arch = "x86_64")]
    fn walk_blocked(raw: &mut Raw, sp: u64, pc: u64) {
        let mut regs = super::super::cfi::Regs { rip: pc, rsp: sp, ..Default::default() };
        raw.sp = sp;
        walk(raw, &mut regs);
    }

    /// A held thread: its pc, then its frame records from fp (lr: the caller of a leaf).
    #[cfg(target_arch = "aarch64")]
    fn walk_slot(slot: &Slot, raw: &mut Raw) {
        let r = |k: usize| slot.regs[k].load(Relaxed);
        raw.handle = r(8);
        raw.sp = r(1);
        raw.lr = r(3) & 0x0000_ffff_ffff_ffff;
        raw.pcs[0] = r(0);
        raw.n = 1;
        let ranges = super::arm64_walk::code_ranges();
        super::arm64_walk::walk_fp(raw, r(2), &ranges, &read);
    }

    /// A thread blocked in a syscall: its pc. The syscall stub keeps no frame record, and the
    /// record (x29) and return address (lr) it was called with are registers the syscall's sp
    /// and pc do not give: the caller is not known.
    #[cfg(target_arch = "aarch64")]
    fn walk_blocked(raw: &mut Raw, sp: u64, pc: u64) {
        raw.sp = sp;
        raw.pcs[0] = pc;
        raw.n = 1;
        raw.stop = Some(BLOCKED_NO_FP);
    }

    /// Walks from registers into `out` (pcs, innermost first; a running thread's own stack:
    /// the host heap tracker); returns how many.
    #[cfg(target_arch = "x86_64")]
    pub fn walk_regs_with(r: &mut super::super::cfi::Regs, out: &mut [u64], read: &dyn Fn(u64) -> Option<u64>) -> usize {
        let mut raw = Raw::new();
        super::x64_walk::walk_with(&mut raw, r, read);
        let n = raw.n.min(out.len());
        out[..n].copy_from_slice(&raw.pcs[..n]);
        n
    }

    /// Mixed walk: JIT frames by their frame records, host frames by their CFI.
    #[cfg(target_arch = "x86_64")]
    fn walk(raw: &mut Raw, r: &mut super::super::cfi::Regs) {
        super::x64_walk::walk_with(raw, r, &read)
    }

    /// The highest address of the calling thread's stack (glibc reads /proc/self/maps for the
    /// main thread: called once per thread, outside any capture).
    #[cfg(target_arch = "x86_64")]
    pub fn stack_hi_of_self() -> u64 {
        if let Some((_, hi)) = crate::stack::program_stack_bounds() {
            return hi as u64;
        }
        extern "C" {
            fn pthread_getattr_np(t: usize, attr: *mut u64) -> i32;
            fn pthread_attr_getstack(attr: *const u64, addr: *mut usize, size: *mut usize) -> i32;
            fn pthread_attr_destroy(attr: *mut u64) -> i32;
        }
        unsafe {
            let mut attr = [0u64; 16];
            if pthread_getattr_np(pthread_self(), attr.as_mut_ptr()) != 0 {
                return 0;
            }
            let (mut addr, mut size) = (0usize, 0usize);
            pthread_attr_getstack(attr.as_ptr(), &mut addr, &mut size);
            pthread_attr_destroy(attr.as_mut_ptr());
            (addr + size) as u64
        }
    }
}

/// The x86_64 walk (Linux and Windows): JIT frames by their frame records, host frames by the
/// module's unwind tables (Linux: DWARF CFI, cfi.rs; Windows: PE unwind info, pe_unwind.rs, as
/// `cfi`), across the program stack's switch by its switch record.
#[cfg(all(target_arch = "x86_64", any(target_os = "linux", windows)))]
mod x64_walk {
    use super::{Raw, MAX_FRAMES};

    /// Each step is proven or the walk ends there, saying why (`raw.stop`): a JIT frame by its
    /// frame record (JIT code keeps one in rbp; where rbp is not known, at its unwind info's
    /// distance above sp), a host frame by its module's unwind tables, the stack switch
    /// (relax_base::stack, a program thread's stack below its start) by the sp the switching frame
    /// keeps on the segment. Never a caller guessed from stack contents: a scan for something
    /// shaped like a frame record finds return addresses that earlier calls left on the stack (a
    /// CI hang's main thread, blocked in poll, read as called from the feature resolver that had
    /// run there before).
    pub fn walk_with(raw: &mut Raw, r: &mut super::super::cfi::Regs, read: &dyn Fn(u64) -> Option<u64>) {
        use super::super::cfi;
        let mut top = true;
        while raw.n < MAX_FRAMES && r.rip != 0 {
            // the lookup pc: a return address points past its call
            let look = if top { r.rip } else { r.rip.wrapping_sub(1) };
            let jit = crate::jit::in_jit_code(look);
            if !top && !jit && !cfi::covers(look) {
                raw.stop = Some("its return address is in neither JIT code nor a module with unwind tables");
                break;
            }
            raw.pcs[raw.n] = r.rip;
            raw.n += 1;
            // the stack switch's frame: it runs on the thread's own stack (its sp is in the
            // switch record on the segment), its callee on the segment
            if !top && r.rip == crate::stack::switch_return_pc() {
                match read(r.rsp.wrapping_add(crate::stack::SWITCH_RECORD)) {
                    Some(own) => r.rsp = own,
                    None => {
                        raw.stop = Some("it switched to the program stack and its switch record is unreadable");
                        break;
                    }
                }
            }
            let sp0 = r.rsp;
            if jit {
                // [rbp] = the caller's rbp, [rbp + 8] = the return address. A thread blocked in a
                // syscall is walked from its sp and pc: when no frame below saved rbp, the frame
                // pointer at this call comes from the function's frame size and the call's
                // staged arguments (its unwind info), which need sp only
                if r.rbp == 0 {
                    if top {
                        raw.stop = Some("it is JIT code at no call (it makes the syscall itself) and its frame pointer is not known");
                        break;
                    }
                    match crate::jit::frame_pointer_at(r.rip, sp0) {
                        Ok(fp) => r.rbp = fp,
                        Err(why) => {
                            raw.stop = Some(why);
                            break;
                        }
                    }
                }
                if r.rbp < sp0 || r.rbp - sp0 > 1 << 20 {
                    raw.stop = Some("it is JIT code and its frame pointer is not in its stack");
                    break;
                }
                let (Some(ra), Some(next)) = (read(r.rbp.wrapping_add(8)), read(r.rbp)) else {
                    raw.stop = Some("it is JIT code and its frame record is unreadable");
                    break;
                };
                r.rsp = r.rbp + 16;
                r.rip = ra;
                r.rbp = next;
            } else {
                match cfi::step(r, top, read) {
                    cfi::Step::Done => {}
                    cfi::Step::End => break,
                    #[cfg(target_os = "linux")]
                    cfi::Step::NeedRbp => {
                        raw.stop = Some(super::imp::BLOCKED_NO_FP);
                        break;
                    }
                    cfi::Step::Fail(why) => {
                        raw.stop = Some(why);
                        break;
                    }
                }
            }
            if r.rsp <= sp0 {
                raw.stop = Some("its caller's frame is not above it");
                break;
            }
            top = false;
        }
    }
}

/// Android arm64: JIT code and host code both keep frame records (x29 = fp: [fp] = the caller's
/// fp, [fp+8] = the return address); a frame is kept while its return address is code.
#[cfg(all(target_os = "android", target_arch = "aarch64"))]
mod arm64_walk {
    use super::{Raw, MAX_FRAMES};

    /// The executable mappings of the process (JIT code included), from /proc/self/maps; read on
    /// the inspecting thread before a walk, never in the signal handler.
    pub fn code_ranges() -> Vec<(u64, u64)> {
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
        let mut out = Vec::new();
        for line in maps.lines() {
            let mut w = line.split_whitespace();
            let (Some(range), Some(perms)) = (w.next(), w.next()) else { continue };
            if perms.as_bytes().get(2) != Some(&b'x') {
                continue;
            }
            let Some((lo, hi)) = range.split_once('-') else { continue };
            if let (Ok(lo), Ok(hi)) = (u64::from_str_radix(lo, 16), u64::from_str_radix(hi, 16)) {
                out.push((lo, hi));
            }
        }
        out
    }

    fn is_code(ranges: &[(u64, u64)], a: u64) -> bool {
        ranges.iter().any(|&(lo, hi)| a >= lo && a < hi)
    }

    /// pointer authentication / tag bits off a saved return address
    fn strip(a: u64) -> u64 {
        a & 0x0000_ffff_ffff_ffff
    }

    /// Appends the frames of the record chain from `fp` (the top pc is already in `raw`).
    pub fn walk_fp(raw: &mut Raw, mut fp: u64, ranges: &[(u64, u64)], read: &dyn Fn(u64) -> Option<u64>) {
        while raw.n < MAX_FRAMES && fp != 0 && fp % 8 == 0 {
            let (Some(next), Some(ra)) = (read(fp), read(fp + 8)) else {
                raw.stop = Some("its frame record is unreadable");
                break;
            };
            let ra = strip(ra);
            if ra == 0 {
                break;
            }
            if !is_code(ranges, ra) {
                raw.stop = Some("its frame record's return address is not code");
                break;
            }
            raw.pcs[raw.n] = ra;
            raw.n += 1;
            // up the stack, except across the program stack's switch (relax_base::stack)
            if next <= fp && ra != crate::stack::switch_return_pc() {
                break;
            }
            fp = next;
        }
    }
}

/// Windows x64: every thread from the toolhelp snapshot; a capture suspends the thread
/// (SuspendThread, then GetThreadContext, which waits until the suspension took effect) and walks
/// it from its full registers, blocked or not, then resumes it. The walk reads through
/// ReadProcessMemory (fails instead of faulting) and takes no lock: the suspended thread may hold
/// the loader's or the heap's.
#[cfg(all(windows, target_arch = "x86_64"))]
mod imp {
    use super::{Raw, Thread};
    use makepad_windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use makepad_windows_sys::Win32::System::Diagnostics::Debug::{GetThreadContext, ReadProcessMemory, CONTEXT, CONTEXT_CONTROL_AMD64, CONTEXT_INTEGER_AMD64};
    use makepad_windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32};
    use makepad_windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, GetThreadDescription, GetThreadTimes, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME};

    /// (no pause on Windows: nothing holds a thread whose registers this could give)
    pub fn regs_now(_t: &Thread) -> Option<(u64, u64, Vec<(usize, u64)>)> {
        None
    }

    pub fn self_os_id() -> u64 {
        unsafe { GetCurrentThreadId() as u64 }
    }

    /// A thread's handle is its thread id on Windows (os::thread::current).
    pub fn tid_of_handle(h: u64) -> Option<u64> {
        (h != 0).then_some(h)
    }

    pub fn list() -> Vec<Thread> {
        let mut out = Vec::new();
        let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }) else { return out };
        let pid = unsafe { GetCurrentProcessId() };
        let mut e = THREADENTRY32::default();
        e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut more = unsafe { Thread32First(snap, &mut e) }.is_ok();
        while more {
            if e.th32OwnerProcessID == pid {
                let tid = e.th32ThreadID as u64;
                let (name, cpu_ns) = name_and_cpu(tid);
                // the run state is not known without NtQuerySystemInformation: "?"
                out.push(Thread { os_id: tid, handle: tid, name, state: "?", cpu_ns, stack: (0, 0), syscall: None });
            }
            e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            more = unsafe { Thread32Next(snap, &mut e) }.is_ok();
        }
        unsafe {
            let _ = CloseHandle(snap);
        }
        out.sort_by_key(|t| t.os_id);
        out
    }

    /// The thread's description (SetThreadDescription; std names its threads with it) and its
    /// user + kernel CPU time.
    fn name_and_cpu(tid: u64) -> (String, u64) {
        let Ok(h) = (unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid as u32) }) else { return (String::new(), 0) };
        let mut name = String::new();
        unsafe {
            if let Ok(p) = GetThreadDescription(h) {
                if !p.0.is_null() {
                    let mut n = 0;
                    while *p.0.add(n) != 0 {
                        n += 1;
                    }
                    name = String::from_utf16_lossy(std::slice::from_raw_parts(p.0, n));
                    let _ = makepad_windows_sys::Win32::Foundation::LocalFree(Some(makepad_windows_sys::Win32::Foundation::HLOCAL(p.0 as *mut std::ffi::c_void)));
                }
            }
        }
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let ticks = |f: &FILETIME| (f.dwLowDateTime as u64) | ((f.dwHighDateTime as u64) << 32);
        let cpu_ns = match unsafe { GetThreadTimes(h, &mut created, &mut exited, &mut kernel, &mut user) } {
            Ok(()) => (ticks(&kernel) + ticks(&user)) * 100,
            Err(_) => 0,
        };
        unsafe {
            let _ = CloseHandle(h);
        }
        (name, cpu_ns)
    }

    /// Reads 8 bytes of this process's memory; None where nothing is readable (a system call:
    /// it fails instead of faulting, and takes no user-space lock).
    pub fn read(addr: u64) -> Option<u64> {
        let mut v = 0u64;
        let mut got = 0usize;
        let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), addr as *const std::ffi::c_void, &mut v as *mut u64 as *mut std::ffi::c_void, 8, Some(&mut got)) }.is_ok();
        (ok && got == 8).then_some(v)
    }

    /// GetThreadContext needs its record 16-byte aligned (the generated CONTEXT says 8).
    #[repr(C, align(16))]
    struct Aligned(CONTEXT);
    const _: () = assert!(std::mem::size_of::<CONTEXT>() == 0x4d0, "the x64 CONTEXT record");

    pub fn capture(t: &Thread, raw: &mut Raw) -> Result<(), &'static str> {
        raw.n = 0;
        let h: HANDLE = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_LIMITED_INFORMATION, false, t.os_id as u32) }.map_err(|_| "OpenThread failed (the thread ended)")?;
        let mut ctx = Aligned(CONTEXT::default());
        ctx.0.ContextFlags = CONTEXT_CONTROL_AMD64 | CONTEXT_INTEGER_AMD64;
        let r = unsafe {
            if SuspendThread(h) == u32::MAX {
                Err("SuspendThread failed (the thread ended)")
            } else {
                // the suspension takes effect asynchronously: GetThreadContext returns once it has
                let got = GetThreadContext(h, &mut ctx.0).is_ok();
                if got {
                    let c = &ctx.0;
                    let mut regs = super::super::cfi::Regs { rip: c.Rip, rsp: c.Rsp, rbp: c.Rbp, rbx: c.Rbx, rsi: c.Rsi, rdi: c.Rdi, r12: c.R12, r13: c.R13, r14: c.R14, r15: c.R15 };
                    raw.sp = regs.rsp;
                    super::x64_walk::walk_with(raw, &mut regs, &read);
                }
                ResumeThread(h);
                if got { Ok(()) } else { Err("GetThreadContext failed") }
            }
        };
        unsafe {
            let _ = CloseHandle(h);
        }
        r
    }
}

#[cfg(any(all(windows, not(target_arch = "x86_64")), all(target_os = "android", not(target_arch = "aarch64"))))]
mod imp {
    pub fn regs_now(_t: &super::Thread) -> Option<(u64, u64, Vec<(usize, u64)>)> {
        None
    }
    use super::{Raw, Thread};
    pub fn self_os_id() -> u64 {
        0
    }
    pub fn tid_of_handle(_h: u64) -> Option<u64> {
        None
    }
    pub fn list() -> Vec<Thread> {
        Vec::new()
    }
    pub fn capture(_t: &Thread, _raw: &mut Raw) -> Result<(), &'static str> {
        Err("stack capture is not implemented on this target")
    }
}

/// A walk proves each step or stops there and says why; it never takes a caller from stack
/// contents that look like a frame record. The CI shape: a thread blocked in poll whose own frame
/// holds what earlier calls left there (here: two chained frame records naming `fake_caller`), with
/// a garbage rbp. The walk from the syscall's sp and pc must not name `fake_caller`: it reaches
/// `park_in_poll` through the CFI (a libc without frame pointers), or stops where the frame
/// pointer is needed and says so (a libc built with them).
#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod tests {
    use super::{capture, imp::BLOCKED_NO_FP, list, Raw};
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicU64, Ordering::{Acquire, Release}};

    #[repr(C)]
    struct PollFd { fd: i32, events: i16, revents: i16 }
    extern "C" {
        fn poll(fds: *mut PollFd, n: u64, timeout: i32) -> i32;
        fn gettid() -> i32;
    }

    #[inline(never)]
    extern "C" fn fake_caller() -> u64 {
        std::hint::black_box(7)
    }

    /// poll(fd) with rbp holding `rbp` (garbage, as rustc code without frame pointers leaves it)
    #[inline(never)]
    fn park_in_poll(fd: i32, rbp: u64, tid: &AtomicU64) {
        // two chained records pointing into `fake_caller`, in this frame above poll's sp
        let mut stale = [0u64; 64];
        let base = stale.as_mut_ptr() as u64;
        let ra = fake_caller as extern "C" fn() -> u64 as usize as u64 + 4;
        stale[8] = base + 8 * 16;
        stale[9] = ra;
        stale[16] = 0;
        stale[17] = ra;
        std::hint::black_box(&mut stale);
        tid.store(unsafe { gettid() } as u64, Release);
        let mut p = PollFd { fd, events: 1, revents: 0 };
        let pp = &mut p as *mut PollFd;
        unsafe {
            std::arch::asm!(
                "push rbp", ".cfi_adjust_cfa_offset 8",
                "sub rsp, 8", ".cfi_adjust_cfa_offset 8",
                "mov rbp, {rbp}",
                "call {poll}",
                "add rsp, 8", ".cfi_adjust_cfa_offset -8",
                "pop rbp", ".cfi_adjust_cfa_offset -8",
                rbp = in(reg) rbp,
                poll = sym poll,
                inout("rdi") pp => _,
                inout("rsi") 1u64 => _,
                inout("edx") -1i32 => _,
                clobber_abi("C"),
            );
        }
        std::hint::black_box(&stale);
    }

    #[test]
    fn a_blocked_threads_walk_never_takes_a_caller_from_stack_contents() {
        super::super::cfi::refresh();
        let (reader, mut writer) = std::io::pipe().unwrap();
        static TID: AtomicU64 = AtomicU64::new(0);
        let read_fd = reader.as_raw_fd();
        let parked = std::thread::spawn(move || park_in_poll(read_fd, 0x10, &TID));
        // until it blocks in poll (syscall 7)
        let t = loop {
            let tid = TID.load(Acquire);
            let blocked = tid != 0 && std::fs::read_to_string(format!("/proc/self/task/{tid}/syscall")).is_ok_and(|s| s.split_whitespace().next() == Some("7"));
            if blocked {
                if let Some(t) = list().into_iter().find(|t| t.os_id == tid && t.state != "running") {
                    break t;
                }
            }
            std::thread::yield_now();
        };
        let mut raw = Raw::new();
        let captured = capture(&t, &mut raw);
        writer.write_all(b"x").unwrap();
        parked.join().unwrap();
        drop(reader);
        captured.unwrap();
        let names: Vec<String> = raw.frames().iter().enumerate().map(|(i, &pc)| {
            super::super::symbols::host_symbol(if i == 0 { pc } else { pc - 1 }).map_or(format!("{pc:#x}"), |s| s.0)
        }).collect();
        assert!(!names.iter().any(|n| n.contains("fake_caller")), "a caller guessed from stack contents: {names:?}");
        let reached = names.iter().any(|n| n.contains("park_in_poll"));
        assert!(reached || raw.stop == Some(BLOCKED_NO_FP), "the walk neither reached park_in_poll nor said why it stopped: {names:?} {:?}", raw.stop);
        assert!(raw.n >= 1);
    }
}

/// A program thread's walk crosses the program stack's switch (relax_base::stack): a thread
/// parked on a program stack shows the function that switched to it, which runs on the thread's
/// own stack. Host frames only (no JIT here): the module's unwind tables, both sides. So only in a
/// rustc build: under the JIT these frames are JIT code, which no loaded module's CFI covers, and
/// the walk rightly stops at its first frame and says why.
#[cfg(all(test, not(relax), target_arch = "x86_64", any(target_os = "linux", windows)))]
mod switch_tests {
    use super::{capture, list, Raw};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::{Acquire, Release}};

    static TID: AtomicU64 = AtomicU64::new(0);
    static STOP: AtomicBool = AtomicBool::new(false);

    /// Spins (on Linux a thread blocked in a syscall is walked from its sp and pc only, and host
    /// code built with frame pointers needs rbp: a running thread is walked from its registers)
    #[inline(never)]
    fn below_the_switch() {
        TID.store(super::self_os_id(), Release);
        while !STOP.load(Acquire) {
            std::hint::spin_loop();
        }
    }

    fn bounds_changed() {}

    #[inline(never)]
    fn above_the_switch() {
        crate::stack::on_program_stack(bounds_changed, below_the_switch);
        // a call, not a tail jump: this frame stays on the stack
        std::hint::black_box(());
    }

    #[test]
    fn a_walk_crosses_the_program_stack_switch() {
        super::super::cfi::refresh();
        let h = std::thread::spawn(above_the_switch);
        let t = loop {
            let tid = TID.load(Acquire);
            if let Some(t) = list().into_iter().find(|t| tid != 0 && t.os_id == tid) {
                break t;
            }
            std::thread::yield_now();
        };
        let mut raw = Raw::new();
        let captured = capture(&t, &mut raw);
        STOP.store(true, Release);
        h.join().unwrap();
        captured.unwrap();
        // by name (ELF symbols; Windows: the test binary's PDB)
        let names: Vec<String> = raw.frames().iter().enumerate().map(|(i, &pc)| {
            super::super::symbols::host_symbol(if i == 0 { pc } else { pc - 1 }).map_or(format!("{pc:#x}"), |s| s.0)
        }).collect();
        let below = names.iter().position(|n| n.contains("below_the_switch"));
        let above = names.iter().position(|n| n.contains("above_the_switch"));
        assert!(below.is_some() && above > below, "the walk does not cross the switch: {names:?}, stop {:?}", raw.stop);
    }
}
