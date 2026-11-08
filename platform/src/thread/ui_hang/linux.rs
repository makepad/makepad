//! SIGPROF backtrace from the interrupted context. The handler uses a bounded
//! frame-pointer walk and kernel-validated reads, not libc's allocating/lazy
//! loader unwinder. Symbolization happens on the watchdog after the handler.
//! A sample waits for its handler however long the UI thread takes to run again: the kernel
//! runs the handler before that thread runs anything else, so a stall cannot end before its
//! sample is taken, however loaded the box. The one way the signal is never taken, SIGPROF
//! blocked on the thread, is refused at registration; should it be blocked later, the stall's
//! end cancels the request.
use std::{ffi::{c_void, c_char, CStr}, sync::{atomic::{AtomicU32, AtomicUsize, Ordering}, OnceLock}};

const SIGPROF: i32 = 27;
#[cfg(target_arch = "x86_64")]
const SYS_GETTID: std::ffi::c_long = 186;
#[cfg(target_arch = "x86_64")]
const SYS_TGKILL: std::ffi::c_long = 234;
#[cfg(target_arch = "x86_64")]
const SYS_FUTEX: std::ffi::c_long = 202;
#[cfg(target_arch = "aarch64")]
const SYS_GETTID: std::ffi::c_long = 178;
#[cfg(target_arch = "aarch64")]
const SYS_TGKILL: std::ffi::c_long = 131;
#[cfg(target_arch = "aarch64")]
const SYS_FUTEX: std::ffi::c_long = 98;
// Other register/ABI layouts are unsupported; an invalid syscall makes
// registration report unavailable before installing any signal handler.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const SYS_GETTID: std::ffi::c_long = -1;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const SYS_TGKILL: std::ffi::c_long = -1;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const SYS_FUTEX: std::ffi::c_long = -1;
const FUTEX_WAIT_PRIVATE: usize = 128;
const FUTEX_WAKE_PRIVATE: usize = 129;
const SIG_BLOCK: i32 = 0;
/// The mailbox: `IDLE`, the kernel id of the thread a sample is asked of, `SAMPLING` (its
/// handler walks) or `READY`. One word, so a handler takes only its own thread's request, and
/// the futex the watchdog waits on.
static REQUEST: AtomicU32 = AtomicU32::new(IDLE);
const IDLE: u32 = 0;
const SAMPLING: u32 = u32::MAX - 1;
const READY: u32 = u32::MAX;
static BOTTOM: AtomicUsize = AtomicUsize::new(0);
static TOP: AtomicUsize = AtomicUsize::new(0);
static LENGTH: AtomicUsize = AtomicUsize::new(0);
static FRAMES: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
static SAMPLER: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[repr(C)]
struct Action { handler: usize, mask: [u64; 16], flags: i32, restorer: usize }
#[repr(C)]
struct IoVec { base: *mut c_void, len: usize }
#[repr(C)]
struct DlInfo { filename: *const c_char, base: *mut c_void, symbol: *const c_char, address: *mut c_void }
extern "C" {
    fn pthread_self() -> usize;
    fn pthread_getattr_np(thread: usize, attr: *mut c_void) -> i32;
    fn pthread_attr_getstack(attr: *const c_void, address: *mut *mut c_void, size: *mut usize) -> i32;
    fn pthread_attr_destroy(attr: *mut c_void) -> i32;
    fn sigaction(signal: i32, action: *const Action, old: *mut Action) -> i32;
    fn pthread_sigmask(how: i32, set: *const u64, old: *mut u64) -> i32;
    fn getpid() -> i32;
    fn syscall(number: std::ffi::c_long, ...) -> std::ffi::c_long;
    fn __errno_location() -> *mut i32;
    fn dladdr(pc: *const c_void, info: *mut DlInfo) -> i32;
}

pub(super) struct Target { thread: u32, bottom: usize, top: usize }
impl Target {
    pub(super) fn current() -> Option<Self> {
        // Resolve the handler's libc stubs before a signal can interrupt a
        // loader operation. Keep a kernel TID, not a pthread pointer that can
        // be freed when the UI thread exits ahead of the watchdog.
        let tid = unsafe {
            let _ = *__errno_location();
            let _ = getpid();
            syscall(SYS_GETTID)
        };
        let tid = u32::try_from(tid).ok().filter(|tid| *tid != IDLE && *tid < SAMPLING)?;
        // a thread that blocks SIGPROF would never take a request
        let mut mask = [0u64; 16];
        if unsafe { pthread_sigmask(SIG_BLOCK, std::ptr::null(), mask.as_mut_ptr()) } != 0 || mask[0] & (1 << (SIGPROF - 1)) != 0 {
            return None;
        }
        static INSTALLED: OnceLock<bool> = OnceLock::new();
        if !*INSTALLED.get_or_init(|| unsafe {
            let mut old: Action = std::mem::zeroed();
            if sigaction(SIGPROF, std::ptr::null(), &mut old) != 0 || old.handler != 0 { return false; }
            let action = Action { handler: capture as *const () as usize, mask: [0; 16], flags: 4 | 0x10000000, restorer: 0 };
            sigaction(SIGPROF, &action, std::ptr::null_mut()) == 0
        }) { return None; }
        unsafe {
            let thread = pthread_self();
            let mut attr = [0usize; 8];
            if pthread_getattr_np(thread, attr.as_mut_ptr().cast()) != 0 { return None; }
            let mut bottom = std::ptr::null_mut();
            let mut size = 0;
            let result = pthread_attr_getstack(attr.as_ptr().cast(), &mut bottom, &mut size);
            pthread_attr_destroy(attr.as_mut_ptr().cast());
            if result != 0 { return None; }
            Some(Self { thread: tid, bottom: bottom as usize, top: (bottom as usize).checked_add(size)? })
        }
    }
    pub(super) fn sample(&self) -> Vec<usize> {
        self.sample_in(&SAMPLER)
    }
    /// A sample, `sampler` held for the mailbox. Only watchdogs take it, never the UI thread or
    /// the handler: one request in the mailbox at a time (several headless contexts take turns).
    /// One that finds it taken skips this sample instead of waiting: the request there waits for
    /// its own thread, which may sit in the kernel (an uninterruptible ioctl) as long as it does.
    /// The wait for the handler ends on events only: it ran (`READY`), or the stall ended or the
    /// thread left first ([`stall_ended`]: `IDLE`).
    fn sample_in(&self, sampler: &std::sync::Mutex<()>) -> Vec<usize> {
        let _sampler = match sampler.try_lock() {
            Ok(held) => held,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return Vec::new(),
        };
        BOTTOM.store(self.bottom, Ordering::Relaxed);
        TOP.store(self.top, Ordering::Relaxed);
        REQUEST.store(self.thread, Ordering::Release);
        if unsafe { syscall(SYS_TGKILL, getpid(), self.thread as i32, SIGPROF) } != 0 {
            REQUEST.store(IDLE, Ordering::Release);
            return Vec::new();
        }
        loop {
            match REQUEST.load(Ordering::Acquire) {
                READY => break,
                // cancelled: the stall ended before the thread took the signal
                IDLE => return Vec::new(),
                pending => unsafe {
                    syscall(SYS_FUTEX, REQUEST.as_ptr(), FUTEX_WAIT_PRIVATE, pending as usize, 0usize);
                },
            }
        }
        let frames = FRAMES[..LENGTH.load(Ordering::Relaxed)].iter().map(|p| p.load(Ordering::Relaxed)).collect();
        REQUEST.store(IDLE, Ordering::Release);
        frames
    }
    pub(super) fn symbolize(&self, pc: usize) -> String {
        unsafe {
            let mut info: DlInfo = std::mem::zeroed();
            if dladdr(pc as *const c_void, &mut info) != 0 && !info.symbol.is_null() {
                format!("{}+0x{:x}", super::demangle::demangle(&CStr::from_ptr(info.symbol).to_string_lossy()), pc.saturating_sub(info.address as usize))
            } else { format!("0x{pc:x}") }
        }
    }
}

/// The calling UI thread's stall ended, or the thread ends: a request for it still pending is
/// past its stall (the thread ran on without taking the signal: SIGPROF blocked since
/// registration, or the signal still on its way), so its watchdog stops waiting for it.
pub(super) fn stall_ended() {
    let Ok(tid) = u32::try_from(unsafe { syscall(SYS_GETTID) }) else { return };
    if REQUEST.compare_exchange(tid, IDLE, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
        unsafe { syscall(SYS_FUTEX, REQUEST.as_ptr(), FUTEX_WAKE_PRIVATE, i32::MAX as usize) };
    }
}

unsafe extern "C" fn capture(_: i32, _: *mut c_void, context: *mut c_void) {
    let saved_errno = *__errno_location();
    if REQUEST.compare_exchange(syscall(SYS_GETTID) as u32, SAMPLING, Ordering::Acquire, Ordering::Relaxed).is_err() {
        *__errno_location() = saved_errno;
        return;
    }
    #[cfg(target_arch = "x86_64")]
    let (pc, mut fp, syscall_read) = {
        let registers = context.cast::<u8>().add(40).cast::<usize>();
        (*registers.add(16), *registers.add(10), 310)
    };
    #[cfg(target_arch = "aarch64")]
    let (pc, mut fp, syscall_read) = {
        // glibc ucontext: flags/link/stack, 128-byte signal mask, 8-byte
        // alignment padding, then fault_address + 31 GPRs + SP + PC.
        let registers = context.cast::<u8>().add(176).cast::<usize>();
        (*registers.add(33), *registers.add(30), 270)
    };
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let (pc, mut fp, syscall_read) = { let _ = context; (0, 0, 0) };
    let mut len = usize::from(pc != 0);
    FRAMES[0].store(pc, Ordering::Relaxed);
    let bottom = BOTTOM.load(Ordering::Relaxed);
    let top = TOP.load(Ordering::Relaxed);
    while len < FRAMES.len() && fp >= bottom && fp <= top.saturating_sub(16) && fp & 7 == 0 {
        let mut record = [0usize; 2];
        let local = IoVec { base: record.as_mut_ptr().cast(), len: 16 };
        let remote = IoVec { base: fp as *mut c_void, len: 16 };
        if syscall(syscall_read, getpid(), &local as *const IoVec, 1usize, &remote as *const IoVec, 1usize, 0usize) != 16 { break; }
        if record[1] == 0 { break; }
        FRAMES[len].store(record[1], Ordering::Relaxed);
        len += 1;
        if record[0] <= fp { break; }
        fp = record[0];
    }
    LENGTH.store(len, Ordering::Relaxed);
    REQUEST.store(READY, Ordering::Release);
    syscall(SYS_FUTEX, REQUEST.as_ptr(), FUTEX_WAKE_PRIVATE, 1usize);
    *__errno_location() = saved_errno;
}

#[cfg(test)]
mod tests {
    /// A watchdog that finds the mailbox taken skips its sample: the one waiting there waits for a
    /// thread that may not run for as long as the kernel holds it, and every watchdog waited
    /// behind it.
    #[test]
    fn a_sample_while_another_waits_is_skipped() {
        let target = super::Target::current().expect("native stack target");
        let busy = std::sync::Mutex::new(());
        let _waiting = busy.lock().unwrap();
        assert!(target.sample_in(&busy).is_empty(), "skipped, not waited for");
    }
}
