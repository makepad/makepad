//! The process's threads as the OS sees them, and their stacks captured from another thread
//! without the owner (DESIGN.md §3).
//!
//! - `list()`: every thread: OS id, pthread handle, name, run state, CPU time.
//! - `capture(t)`: the thread's pcs, innermost first. macOS suspends it (Mach) and reads its
//!   registers; Linux and Android interrupt it with a real-time signal whose handler hands its
//!   registers over and waits until the walk is done. Either way the thread stands still while its stack is
//!   walked, and nothing is allocated or locked meanwhile (it may hold malloc's or the loader's
//!   lock): pcs go to a fixed array, names are looked up after it runs again.
//!
//! The walk: JIT code keeps frame pointers (both targets); host code on macOS arm64 too (the
//! platform ABI) and on Android arm64 (frame records); host code on Linux x86_64 is stepped by
//! its DWARF CFI (cfi.rs). Every memory read goes through a call that fails instead of faulting.

/// Most frames kept per stack.
pub const MAX_FRAMES: usize = 128;

#[derive(Debug)]
pub struct Thread {
    /// the OS's id: the Mach thread id (macOS), the tid (Linux)
    pub os_id: u64,
    /// the pthread handle (what owner / waits name a thread by); 0: not known yet
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
    #[cfg(target_os = "macos")]
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
            #[cfg(target_os = "macos")]
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
}

impl Raw {
    pub fn new() -> Raw {
        Raw { pcs: [0; MAX_FRAMES], n: 0, lr: 0, sp: 0, handle: 0 }
    }
    pub fn frames(&self) -> &[u64] {
        &self.pcs[..self.n]
    }
}

pub use imp::{capture, list, self_os_id, tid_of_handle};
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use imp::{stack_hi_of_self, walk_regs_with, Slot as HoldSlot};

/// Pause: holds thread `tid` in its handler until `release_hold` (Linux).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn hold_paused(tid: u64) -> Result<&'static HoldSlot, &'static str> {
    imp::hold(tid, true)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn release_hold(s: &HoldSlot) {
    imp::release(s)
}

/// Up to `len` bytes of this process's memory at `addr`, or None where it is not readable (a
/// read that fails instead of faulting: process_vm_readv / mach_vm_read_overwrite).
pub fn read_mem(addr: u64, len: usize) -> Option<Vec<u8>> {
    #[cfg_attr(windows, allow(unused_mut))]
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
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            static mach_task_self_: u32;
            fn mach_vm_read_overwrite(task: u32, addr: u64, size: u64, data: u64, out: *mut u64) -> i32;
        }
        let mut got = 0u64;
        let r = unsafe { mach_vm_read_overwrite(mach_task_self_, addr, len as u64, v.as_mut_ptr() as u64, &mut got) };
        (r == 0 && got == len as u64).then_some(v)
    }
    #[cfg(windows)]
    {
        let _ = addr;
        None
    }
}

/// Pause: Mach suspend / resume of one thread (macOS; suspends nest); its run state.
#[cfg(target_os = "macos")]
pub use imp::{resume, running, suspend};

/// Walks frame records from `fp` (fp rule: [fp] = caller's fp, [fp+8] = return address), appending
/// to `raw` until the chain ends, leaves the stack upwards order or `raw` is full.
#[cfg(target_os = "macos")]
fn walk_fp(raw: &mut Raw, mut fp: u64, read: &dyn Fn(u64) -> Option<u64>) {
    while raw.n < MAX_FRAMES && fp != 0 && fp % 8 == 0 {
        let (Some(ra), Some(next)) = (read(fp + 8), read(fp)) else { break };
        if ra == 0 {
            break;
        }
        // pointer authentication bits (arm64e system libraries)
        raw.pcs[raw.n] = ra & 0x0000_ffff_ffff_ffff;
        raw.n += 1;
        if next <= fp {
            break;
        }
        fp = next;
    }
}

#[cfg(target_os = "macos")]
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
        t.port != 0 && unsafe { thread_suspend(t.port) } == 0
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
}

#[cfg(any(target_os = "linux", all(target_os = "android", target_arch = "aarch64")))]
mod imp {
    use super::{Raw, Thread};
    #[cfg(target_arch = "x86_64")]
    use super::MAX_FRAMES;
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
        /// a pause holds it (released by `resume`, not after one walk)
        paused: std::sync::atomic::AtomicBool,
    }
    static HOLDS: [Slot; SLOTS] = [const { Slot { state: AtomicU32::new(0), tid: AtomicI32::new(0), regs: [const { AtomicU64::new(0) }; 9], paused: std::sync::atomic::AtomicBool::new(false) } }; SLOTS];
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
                let limit = if slot.paused.load(Relaxed) { 600 } else { 5 };
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
    pub fn hold(tid: u64, paused: bool) -> Result<&'static Slot, &'static str> {
        let sig = install();
        if sig == 0 {
            return Err("no free real-time signal for stack capture");
        }
        let Some(slot) = HOLDS.iter().find(|h| h.state.compare_exchange(0, 1, AcqRel, Relaxed).is_ok()) else { return Err("too many held threads") };
        slot.paused.store(paused, Relaxed);
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
        slot.paused.store(false, Relaxed);
        slot.state.store(0, Release);
    }

    /// The slot holding `tid` now (a paused thread), if any.
    fn held(tid: u64) -> Option<&'static Slot> {
        HOLDS.iter().find(|h| h.tid.load(Acquire) == tid as i32 && h.state.load(Acquire) == 2)
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

    pub fn capture(t: &Thread, raw: &mut Raw) -> Result<(), &'static str> {
        raw.n = 0;
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
            }
        }
        // a paused thread already stands still in its handler
        if let Some(slot) = held(t.os_id) {
            walk_slot(slot, raw);
            return Ok(());
        }
        let slot = hold(t.os_id, false)?;
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

    /// A thread blocked in a syscall: its pc, then the frame records above the syscall's sp.
    #[cfg(target_arch = "aarch64")]
    fn walk_blocked(raw: &mut Raw, sp: u64, pc: u64) {
        raw.sp = sp;
        raw.pcs[0] = pc;
        raw.n = 1;
        let ranges = super::arm64_walk::code_ranges();
        if let Some(fp) = super::arm64_walk::find_record(sp, &ranges, &read) {
            super::arm64_walk::walk_fp(raw, fp, &ranges, &read);
        }
    }

    /// Walks from registers into `out` (pcs, innermost first; a running thread's own stack:
    /// the host heap tracker); returns how many.
    #[cfg(target_arch = "x86_64")]
    pub fn walk_regs_with(r: &mut super::super::cfi::Regs, out: &mut [u64], read: &dyn Fn(u64) -> Option<u64>) -> usize {
        let mut raw = Raw::new();
        walk_with(&mut raw, r, read);
        let n = raw.n.min(out.len());
        out[..n].copy_from_slice(&raw.pcs[..n]);
        n
    }

    /// A frame record at or above `sp`: [a] = a higher record (or 0), [a+8] = code, and the record
    /// it links to looks the same (two links checked: stale data rarely chains twice).
    #[cfg(target_arch = "x86_64")]
    fn find_record(sp: u64, read: &dyn Fn(u64) -> Option<u64>) -> Option<u64> {
        let code = |a: u64| crate::jit::in_jit_code(a) || super::super::cfi::covers(a);
        let ok = |a: u64| -> Option<u64> {
            let next = read(a)?;
            let ra = read(a + 8)?;
            (code(ra) && (next == 0 || (next > a && next - a < 1 << 20))).then_some(next)
        };
        let mut a = sp;
        while a < sp + (64 << 10) {
            if let Some(next) = ok(a) {
                if next == 0 || ok(next).is_some() {
                    return Some(a);
                }
            }
            a += 8;
        }
        None
    }

    /// Mixed walk: JIT frames by their frame pointers, host frames by their CFI (frame pointers
    /// where a module has none).
    #[cfg(target_arch = "x86_64")]
    fn walk(raw: &mut Raw, r: &mut super::super::cfi::Regs) {
        walk_with(raw, r, &read)
    }

    /// The highest address of the calling thread's stack (glibc reads /proc/self/maps for the
    /// main thread: called once per thread, outside any capture).
    #[cfg(target_arch = "x86_64")]
    pub fn stack_hi_of_self() -> u64 {
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

    #[cfg(target_arch = "x86_64")]
    fn walk_with(raw: &mut Raw, r: &mut super::super::cfi::Regs, read: &dyn Fn(u64) -> Option<u64>) {
        use super::super::cfi;
        let mut top = true;
        while raw.n < MAX_FRAMES && r.rip != 0 {
            // a caller that is no code we know ends the walk (a frame-pointer guess through code
            // without frame pointers or CFI reads garbage)
            if !top && !crate::jit::in_jit_code(r.rip.wrapping_sub(1)) && !cfi::covers(r.rip.wrapping_sub(1)) {
                break;
            }
            raw.pcs[raw.n] = r.rip;
            raw.n += 1;
            let sp0 = r.rsp;
            let jit = crate::jit::in_jit_code(r.rip);
            let mut stepped = false;
            if !jit && cfi::covers(r.rip) {
                match cfi::step(r, top, read) {
                    cfi::Step::Done => stepped = true,
                    cfi::Step::NeedRbp => {
                        // a frame-pointer frame of a walk without registers: its record is the
                        // first chained one above sp
                        let Some(fp) = find_record(r.rsp, read) else { break };
                        r.rbp = fp;
                        stepped = cfi::step(r, top, read) == cfi::Step::Done;
                    }
                    cfi::Step::Fail => {}
                }
            }
            if !stepped && jit && (r.rbp < r.rsp || r.rbp - r.rsp > 1 << 20) {
                // a JIT frame reached from host frames that never saved rbp (a walk from a
                // syscall's sp/pc has no rbp): its frame record is the first one above sp that
                // chains to another record
                match find_record(r.rsp, read) {
                    Some(fp) => r.rbp = fp,
                    None => break,
                }
            }
            if !stepped {
                // frame record: [rbp] = caller's rbp, [rbp+8] = return address
                let (Some(ra), Some(next)) = (read(r.rbp.wrapping_add(8)), read(r.rbp)) else { break };
                if r.rbp == 0 || r.rbp < sp0 {
                    break;
                }
                r.rsp = r.rbp + 16;
                r.rip = ra;
                r.rbp = next;
            }
            if r.rsp <= sp0 {
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
            let (Some(next), Some(ra)) = (read(fp), read(fp + 8)) else { break };
            let ra = strip(ra);
            if ra == 0 || !is_code(ranges, ra) {
                break;
            }
            raw.pcs[raw.n] = ra;
            raw.n += 1;
            if next <= fp {
                break;
            }
            fp = next;
        }
    }

    /// A frame record at or above `sp` (a thread stopped in a syscall stub, which keeps no
    /// record of its own): [a] = a higher record (or 0), [a+8] = code, and the record it links
    /// to looks the same (two links checked: stale data rarely chains twice).
    pub fn find_record(sp: u64, ranges: &[(u64, u64)], read: &dyn Fn(u64) -> Option<u64>) -> Option<u64> {
        let ok = |a: u64| -> Option<u64> {
            let next = read(a)?;
            let ra = strip(read(a + 8)?);
            (is_code(ranges, ra) && (next == 0 || (next > a && next - a < 1 << 20))).then_some(next)
        };
        let mut a = sp;
        while a < sp + (64 << 10) {
            if let Some(next) = ok(a) {
                if next == 0 || ok(next).is_some() {
                    return Some(a);
                }
            }
            a += 8;
        }
        None
    }
}

#[cfg(any(windows, all(target_os = "android", not(target_arch = "aarch64"))))]
mod imp {
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
