//! The tier-up sampler's per-thread sources (jit.rs sampler_thread): every SAMPLE_MS a thread
//! running JIT code contributes its pc and the return addresses of its next SAMPLE_CALLERS
//! frames.
//!
//! - `open_this_thread(me)` -> a `Handle` for the calling thread (`me` = thread::current()).
//! - `read(h, pcs, selfs, lo, hi)`: appends the samples taken since the last read whose pcs lie
//!   in [lo, hi) (pcs: the pc and callers; selfs: the pc only), without allocating.
//! - `close(h)`.
//! - `set_period(h, ms)`: the source's own sampling period (Linux: the kernel's task-clock period;
//!   the suspending samplers sample when the sampler thread wakes, so it is theirs: no-op).
//!
//! Android: no sampler (the app sandbox gives no perf_event_open).
//! macOS suspends the thread for a Mach thread_get_state; Linux reads a perf_event task-clock
//! ring the kernel fills.

pub use imp::*;

/// One observed PC and the CPU/time exposure it represents in default-period
/// units. Exposure must not be mistaken for independent observations.
#[derive(Clone, Copy)]
pub struct Sample {
    pub pc: u64,
    pub weight: u32,
}

#[cfg(target_os = "macos")]
mod imp {
    use crate::jit::SAMPLE_CALLERS;
    /// the thread's Mach port, its stack bounds (for the frame-pointer walk) and its CPU time at
    /// the last read (µs)
    pub type Handle = (u32, u64, u64, std::cell::Cell<u64>);
    extern "C" {
        fn pthread_get_stackaddr_np(t: usize) -> *mut u8;
        fn pthread_get_stacksize_np(t: usize) -> usize;
        fn pthread_mach_thread_np(t: usize) -> u32;
        fn thread_suspend(t: u32) -> i32;
        fn thread_resume(t: u32) -> i32;
        fn thread_get_state(t: u32, flavor: i32, state: *mut u32, count: *mut u32) -> i32;
        fn thread_info(t: u32, flavor: u32, out: *mut u32, count: *mut u32) -> i32;
    }
    pub fn open_this_thread(me: u64) -> Option<Handle> {
        let me = me as usize;
        unsafe {
            let hi = pthread_get_stackaddr_np(me) as u64;
            let lo = hi - pthread_get_stacksize_np(me) as u64;
            Some((pthread_mach_thread_np(me), lo, hi, std::cell::Cell::new(0)))
        }
    }
    pub fn close(_h: Handle) {}
    pub fn set_period(_h: &Handle, _ms: u64) {}
    /// A thread on a CPU now: suspend, read the pc and the return addresses of the next CALLERS
    /// frames (the frame record chain, within the thread's stack), resume. No allocation while
    /// the thread is stopped (it may hold the allocator's lock): pcs go to a fixed array first.
    /// A thread that is not on a CPU is not suspended and gives no sample (CPU samples only, as
    /// Linux's task clock); a sample weighs at most the CPU time the thread used since the last
    /// read (`weight` is the wall time since then). Returns whether the thread was on a CPU.
    pub fn read(h: &Handle, out: &mut Vec<super::Sample>, selfs: &mut Vec<super::Sample>, lo: u64, hi: u64, weight: u32) -> bool {
        // thread_basic_info: user_time (s, µs), system_time (s, µs), cpu_usage, policy, run_state, ...
        let mut info = [0u32; 10];
        let mut icount = 10u32;
        if unsafe { thread_info(h.0, 3, info.as_mut_ptr(), &mut icount) } != 0 {
            return false;
        }
        let cpu_us = (info[0] as u64 + info[2] as u64) * 1_000_000 + info[1] as u64 + info[3] as u64;
        let used_us = cpu_us.saturating_sub(h.3.replace(cpu_us));
        if info[6] != 1 {
            return false;
        }
        let unit_us = crate::jit::SAMPLE_MS * 1000;
        let weight = (((used_us + unit_us / 2) / unit_us) as u32).clamp(1, weight.max(1));
        // ARM_THREAD_STATE64 (flavor 6): x0..x28, fp, lr, sp, pc, cpsr, pad = 68 words
        let mut st = [0u32; 68];
        let mut count = 68u32;
        let mut got = [0u64; 1 + SAMPLE_CALLERS];
        let mut n = 0;
        unsafe {
            if thread_suspend(h.0) != 0 {
                return false;
            }
            let r = thread_get_state(h.0, 6, st.as_mut_ptr(), &mut count);
            if r == 0 {
                let w = |k: usize| (st[2 * k] as u64) | ((st[2 * k + 1] as u64) << 32);
                got[0] = w(32);
                n = 1;
                let mut fp = w(29);
                while n < got.len() && fp >= h.1 && fp + 16 <= h.2 && fp % 8 == 0 {
                    got[n] = *((fp + 8) as *const u64);
                    n += 1;
                    let next = *(fp as *const u64);
                    if next <= fp {
                        break;
                    }
                    fp = next;
                }
            }
            thread_resume(h.0);
        }
        if n > 0 && got[0] >= lo && got[0] < hi && selfs.len() < selfs.capacity() {
            selfs.push(super::Sample { pc: got[0], weight });
        }
        for &pc in &got[..n] {
            if pc >= lo && pc < hi && out.len() < out.capacity() {
                out.push(super::Sample { pc, weight });
            }
        }
        true
    }
}

#[cfg(target_os = "linux")]
mod imp {
    #[cfg(target_arch = "x86_64")]
    const SYS_GETTID: i64 = 186;
    #[cfg(target_arch = "x86_64")]
    const SYS_PERF_EVENT_OPEN: i64 = 298;
    // asm-generic/unistd.h (aarch64): __NR_gettid / __NR_perf_event_open.
    #[cfg(target_arch = "aarch64")]
    const SYS_GETTID: i64 = 178;
    #[cfg(target_arch = "aarch64")]
    const SYS_PERF_EVENT_OPEN: i64 = 241;
    use crate::jit::{sample_periods, SAMPLE_CALLERS, SAMPLE_MS};
    /// perf_event fd and its mapped sample ring (header page + RING_PAGES data pages)
    pub type Handle = (i32, usize, std::cell::Cell<(u64, u64, usize)>);
    const PAGE: usize = 4096;
    const RING_PAGES: usize = 8;
    extern "C" {
        fn syscall(n: i64, ...) -> i64;
        fn mmap(a: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn munmap(a: *mut u8, len: usize) -> i32;
        #[link_name = "close"]
        fn close_fd(fd: i32) -> i32;
        fn ioctl(fd: i32, req: u64, ...) -> i32;
    }
    /// task-clock sampling of the calling thread every SAMPLE_MS of its CPU time, user ips only
    pub fn open_this_thread(_me: u64) -> Option<Handle> {
        unsafe {
            let tid = syscall(SYS_GETTID) as i32; // gettid
            let mut attr = [0u8; 128];
            attr[0..4].copy_from_slice(&1u32.to_ne_bytes()); // PERF_TYPE_SOFTWARE
            attr[4..8].copy_from_slice(&128u32.to_ne_bytes()); // size
            attr[8..16].copy_from_slice(&1u64.to_ne_bytes()); // PERF_COUNT_SW_TASK_CLOCK
            let periods = sample_periods();
            let period = if periods.len() == 1 { periods[0] } else { periods.iter().copied().min().unwrap().min(SAMPLE_MS) };
            // a thread that starts running JIT code while the sampler is settled samples at its cadence
            let period = if periods.len() == 1 { period.max(crate::jit::sampler_period_floor()) } else { period };
            attr[16..24].copy_from_slice(&(period * 1_000_000).to_ne_bytes()); // sample_period (ns)
            attr[24..32].copy_from_slice(&(1u64 | 32 | 256).to_ne_bytes()); // PERF_SAMPLE_IP | PERF_SAMPLE_CALLCHAIN | PERF_SAMPLE_PERIOD
            attr[108..110].copy_from_slice(&(SAMPLE_CALLERS as u16 + 2).to_ne_bytes()); // sample_max_stack
            attr[40..48].copy_from_slice(&((1u64 << 5) | (1u64 << 6)).to_ne_bytes()); // exclude_kernel | exclude_hv
            let fd = syscall(SYS_PERF_EVENT_OPEN, attr.as_ptr(), tid, -1i32, -1i32, 0u64) as i32; // perf_event_open
            if fd < 0 {
                return None;
            }
            let len = (1 + RING_PAGES) * PAGE;
            let p = mmap(std::ptr::null_mut(), len, 3, 1, fd, 0); // PROT_READ|WRITE, MAP_SHARED
            if p as isize == -1 {
                close_fd(fd);
                return None;
            }
            Some((fd, p as usize, std::cell::Cell::new((0, 0, 0))))
        }
    }
    pub fn close(h: Handle) {
        unsafe {
            munmap(h.1 as *mut u8, (1 + RING_PAGES) * PAGE);
            close_fd(h.0);
        }
    }
    /// PERF_EVENT_IOC_PERIOD (_IOW('$', 4, u64)): the kernel samples every `ms` of the thread's
    /// CPU from its next overflow on (each record carries its period: weights stay right).
    /// Only for the single default cadence (the irregular diagnostic one is `exposure`'s).
    pub fn set_period(h: &Handle, ms: u64) {
        if sample_periods().len() != 1 {
            return;
        }
        let ns: u64 = ms * 1_000_000;
        unsafe {
            ioctl(h.0, 0x4008_2404, &ns as *const u64);
        }
    }
    /// Diagnostic irregular sampling selects observations from a fixed-period
    /// stream. Resetting PERF_EVENT_IOC_PERIOD at each wall-clock wake can
    /// restart the counter before it overflows and accidentally starve samples.
    fn exposure(h: &Handle, ns: u64) -> Option<u64> {
        let periods = sample_periods();
        if periods.len() == 1 { return Some(ns); }
        let (mut phase, mut span, mut index) = h.2.get();
        phase += ns;
        span += ns;
        let target = periods[index] * 1_000_000;
        let out = if phase >= target {
            phase -= target;
            index = (index + 1) % periods.len();
            let out = Some(span);
            span = 0;
            out
        } else { None };
        h.2.set((phase, span, index));
        out
    }
    /// drains PERF_RECORD_SAMPLE records (header, ip) from the ring
    /// Returns true (the ring holds CPU samples only: the sampler does not back off on Linux).
    pub fn read(h: &Handle, out: &mut Vec<super::Sample>, selfs: &mut Vec<super::Sample>, lo: u64, hi: u64, _weight: u32) -> bool {
        use std::sync::atomic::{AtomicU64, Ordering};
        unsafe {
            let base = h.1;
            let head = (*((base + 1024) as *const AtomicU64)).load(Ordering::Acquire);
            let tail_p = (base + 1032) as *const AtomicU64;
            let mut tail = (*tail_p).load(Ordering::Relaxed);
            let data = base + PAGE;
            let size = (RING_PAGES * PAGE) as u64;
            let rd = |off: u64| -> u64 {
                let mut b = [0u8; 8];
                for k in 0..8u64 {
                    b[k as usize] = *((data + ((off + k) % size) as usize) as *const u8);
                }
                u64::from_ne_bytes(b)
            };
            while tail + 8 <= head {
                let hdr = rd(tail);
                let (ty, sz) = (hdr as u32, (hdr >> 48) as u16 as u64);
                if sz < 8 {
                    break;
                }
                if ty == 9 && sz >= 24 {
                    let ip = rd(tail + 8);
                    // PERF_SAMPLE_PERIOD precedes CALLCHAIN in the record.
                    // The kernel already accounts for delayed ring draining;
                    // multiplying by sampler wake latency would count it twice.
                    let Some(ns) = exposure(h, rd(tail + 16)) else { tail += sz; continue; };
                    let unit = SAMPLE_MS * 1_000_000;
                    let weight = ((ns + unit / 2) / unit).clamp(1, 500) as u32;
                    if ip >= lo && ip < hi && out.len() < out.capacity() {
                        out.push(super::Sample { pc: ip, weight });
                    }
                    if ip >= lo && ip < hi && selfs.len() < selfs.capacity() {
                        selfs.push(super::Sample { pc: ip, weight });
                    }
                    // callchain: nr, then ips (context markers are outside the code region; the
                    // first entry repeats ip)
                    if sz >= 32 {
                        let nr = rd(tail + 24).min((sz - 32) / 8);
                        let mut taken = 0;
                        for k in 0..nr {
                            let a = rd(tail + 32 + 8 * k);
                            if a == ip || a < lo || a >= hi {
                                continue;
                            }
                            if taken < SAMPLE_CALLERS && out.len() < out.capacity() {
                                out.push(super::Sample { pc: a, weight });
                                taken += 1;
                            }
                        }
                    }
                }
                tail += sz;
            }
            (*tail_p).store(tail, Ordering::Release);
        }
        true
    }
}

/// Windows: the sampler suspends the thread (SuspendThread; GetThreadContext completes the
/// suspension), reads rip and walks the rbp chain within the thread's stack, then resumes it --
/// the macOS scheme. No allocation while the thread is stopped (the context buffer is the
/// handle's own).
#[cfg(windows)]
mod imp {
    use crate::jit::SAMPLE_CALLERS;
    use crate::os::win32::*;

    /// (thread handle, stack low, stack high, context buffer)
    pub type Handle = (HANDLE, u64, u64, Box<std::cell::UnsafeCell<Context>>);

    pub fn open_this_thread(_me: u64) -> Option<Handle> {
        unsafe {
            let mut h: HANDLE = 0;
            if DuplicateHandle(GetCurrentProcess(), GetCurrentThread(), GetCurrentProcess(), &mut h, 0, 0, DUPLICATE_SAME_ACCESS) == 0 {
                return None;
            }
            let (mut lo, mut hi) = (0usize, 0usize);
            GetCurrentThreadStackLimits(&mut lo, &mut hi);
            Some((h, lo as u64, hi as u64, Box::new(std::cell::UnsafeCell::new(*Context::new(CONTEXT_CONTROL | CONTEXT_INTEGER)))))
        }
    }

    pub fn close(h: Handle) {
        unsafe {
            CloseHandle(h.0);
        }
    }
    pub fn set_period(_h: &Handle, _ms: u64) {}

    /// Returns true (the thread is sampled whatever its state: no back-off on Windows).
    pub fn read(h: &Handle, out: &mut Vec<super::Sample>, selfs: &mut Vec<super::Sample>, lo: u64, hi: u64, weight: u32) -> bool {
        let mut got = [0u64; 1 + SAMPLE_CALLERS];
        let mut n = 0;
        unsafe {
            if SuspendThread(h.0) == u32::MAX {
                return true;
            }
            let ctx = h.3.get();
            if GetThreadContext(h.0, ctx) != 0 {
                got[0] = (*ctx).get(CTX_RIP);
                n = 1;
                let mut fp = (*ctx).get(CTX_RBP);
                // frames lie between the thread's stack pointer and the top of its stack (below rsp
                // the reserved stack is not committed); a thread running on a compiler stack
                // segment has rsp outside its stack: rip only. Reads go through ReadProcessMemory,
                // which fails instead of faulting (a stale rbp must never take the process down).
                let rsp = (*ctx).get(CTX_RSP);
                let lo = if rsp >= h.1 && rsp < h.2 { rsp } else { h.2 };
                let read = |a: u64| -> Option<u64> {
                    let mut v = 0u64;
                    let mut got_n = 0usize;
                    let ok = ReadProcessMemory(GetCurrentProcess(), a as *const u8, &mut v as *mut u64 as *mut u8, 8, &mut got_n);
                    (ok != 0 && got_n == 8).then_some(v)
                };
                while n < got.len() && fp >= lo && fp + 16 <= h.2 && fp % 8 == 0 {
                    let (Some(ret), Some(next)) = (read(fp + 8), read(fp)) else { break };
                    got[n] = ret;
                    n += 1;
                    if next <= fp {
                        break;
                    }
                    fp = next;
                }
            }
            ResumeThread(h.0);
        }
        if n > 0 && got[0] >= lo && got[0] < hi && selfs.len() < selfs.capacity() {
            selfs.push(super::Sample { pc: got[0], weight });
        }
        for &pc in &got[..n] {
            if pc >= lo && pc < hi && out.len() < out.capacity() {
                out.push(super::Sample { pc, weight });
            }
        }
        true
    }
}

/// Sampler-owned wait source: high-resolution one-shot timers on Windows, ordinary sleep
/// elsewhere. Older Windows falls back to a 1 ms timer period, released with the sampler.
pub struct Wait {
    #[cfg(windows)]
    timer: crate::os::win32::HANDLE,
    #[cfg(windows)]
    period: bool,
}

impl Wait {
    pub fn new() -> Self {
        #[cfg(windows)]
        unsafe {
            use crate::os::win32::*;
            let timer = CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_MODIFY_STATE | SYNCHRONIZE);
            Self { timer, period: timer == 0 && timeBeginPeriod(1) == 0 }
        }
        #[cfg(not(windows))]
        { Self {} }
    }

    pub fn sleep(&mut self, ms: u64) {
        #[cfg(windows)]
        unsafe {
            use crate::os::win32::*;
            if self.timer != 0 {
                let due = -(ms as i64 * 10_000); // relative time, 100 ns units
                if SetWaitableTimer(self.timer, &due, 0, None, std::ptr::null_mut(), 0) != 0
                    && WaitForSingleObject(self.timer, u32::MAX) == 0 {
                    return;
                }
                CloseHandle(self.timer);
                self.timer = 0;
                self.period = timeBeginPeriod(1) == 0;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

#[cfg(windows)]
impl Drop for Wait {
    fn drop(&mut self) {
        unsafe {
            use crate::os::win32::*;
            if self.timer != 0 { CloseHandle(self.timer); }
            if self.period { timeEndPeriod(1); }
        }
    }
}

#[cfg(target_os = "android")]
mod imp {
    pub type Handle = ();
    /// Perf sampling is unsupported in the Android app sandbox.
    pub fn open_this_thread(_me: u64) -> Option<Handle> { None }
    pub fn close(_h: Handle) {}
    pub fn set_period(_h: &Handle, _ms: u64) {}
    pub fn read(_h: &Handle, _out: &mut Vec<super::Sample>, _selfs: &mut Vec<super::Sample>, _lo: u64, _hi: u64, _weight: u32) -> bool {
        false
    }
}
