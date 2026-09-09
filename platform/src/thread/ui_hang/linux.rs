//! SIGPROF backtrace from the interrupted context. The handler uses a bounded
//! frame-pointer walk and kernel-validated reads, not libc's allocating/lazy
//! loader unwinder. Symbolization happens on the watchdog after the handler.
use std::{ffi::{c_void, c_char, CStr}, sync::{atomic::{AtomicUsize, Ordering}, OnceLock}, time::{Duration, Instant}};

const SIGPROF: i32 = 27;
#[cfg(target_arch = "x86_64")]
const SYS_GETTID: std::ffi::c_long = 186;
#[cfg(target_arch = "x86_64")]
const SYS_TGKILL: std::ffi::c_long = 234;
#[cfg(target_arch = "aarch64")]
const SYS_GETTID: std::ffi::c_long = 178;
#[cfg(target_arch = "aarch64")]
const SYS_TGKILL: std::ffi::c_long = 131;
// Other register/ABI layouts are unsupported; an invalid syscall makes
// registration report unavailable before installing any signal handler.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const SYS_GETTID: std::ffi::c_long = -1;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const SYS_TGKILL: std::ffi::c_long = -1;
static REQUEST: AtomicUsize = AtomicUsize::new(0); // 0 idle, 1 requested, 2 sampling, 3 ready
static THREAD: AtomicUsize = AtomicUsize::new(0);
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
    fn getpid() -> i32;
    fn syscall(number: std::ffi::c_long, ...) -> std::ffi::c_long;
    fn __errno_location() -> *mut i32;
    fn dladdr(pc: *const c_void, info: *mut DlInfo) -> i32;
}

pub(super) struct Target { thread: usize, bottom: usize, top: usize }
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
        if tid <= 0 { return None; }
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
            Some(Self { thread: tid as usize, bottom: bottom as usize, top: (bottom as usize).checked_add(size)? })
        }
    }
    pub(super) fn sample(&self) -> Vec<usize> {
        // Only watchdogs use this lock, never UI/handler. Multiple headless
        // contexts cannot overwrite the fixed signal mailbox concurrently.
        let Ok(_sampler) = SAMPLER.try_lock() else { return Vec::new(); };
        if REQUEST.load(Ordering::Acquire) == 2 { return Vec::new(); }
        THREAD.store(self.thread, Ordering::Relaxed);
        BOTTOM.store(self.bottom, Ordering::Relaxed);
        TOP.store(self.top, Ordering::Relaxed);
        REQUEST.store(1, Ordering::Release);
        if unsafe { syscall(SYS_TGKILL, getpid(), self.thread as i32, SIGPROF) } != 0 {
            let _ = REQUEST.compare_exchange(1, 0, Ordering::AcqRel, Ordering::Relaxed);
            return Vec::new();
        }
        let deadline = Instant::now() + Duration::from_millis(10);
        while REQUEST.load(Ordering::Acquire) != 3 {
            if Instant::now() >= deadline {
                let _ = REQUEST.compare_exchange(1, 0, Ordering::AcqRel, Ordering::Relaxed);
                return Vec::new();
            }
            std::thread::sleep(Duration::from_micros(100));
        }
        let frames = FRAMES[..LENGTH.load(Ordering::Relaxed)].iter().map(|p| p.load(Ordering::Relaxed)).collect();
        REQUEST.store(0, Ordering::Release);
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

unsafe extern "C" fn capture(_: i32, _: *mut c_void, context: *mut c_void) {
    let saved_errno = *__errno_location();
    if syscall(SYS_GETTID) as usize != THREAD.load(Ordering::Relaxed) || REQUEST.compare_exchange(1, 2, Ordering::Acquire, Ordering::Relaxed).is_err() {
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
    *__errno_location() = saved_errno;
    REQUEST.store(3, Ordering::Release);
}
