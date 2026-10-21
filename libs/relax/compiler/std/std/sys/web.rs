//! The web layer: rustc's std for wasm32-unknown-unknown / wasm64-unknown-unknown with
//! `+atomics` (sys/pal/wasm + pal/unsupported + the unsupported halves of fs, process, env, net,
//! pipe, stdio, time, thread; selected by `cfg(target_family = "wasm")`). A thread cannot be
//! started by std; blocking is the futex over `memory.atomic.wait32` / `notify`; there is no
//! clock, no file system, no processes, no sockets; standard output is a sink. The sys API is
//! posix.rs's / windows.rs's (the `portable` module, re-exported as `sys::*`); the thread-local
//! keys are the Relax runtime's, as there.

use alloc::string::String;
use core::ffi::{c_char, c_void};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use core::time::Duration;

#[cfg(target_arch = "wasm32")]
use core::arch::wasm32 as wasm;
#[cfg(target_arch = "wasm64")]
use core::arch::wasm64 as wasm;

// ---- items generic std code reaches as sys::os::*

pub type clockid_t = u32;
pub const CLOCK_REALTIME: clockid_t = 0;
pub const CLOCK_INSTANT: clockid_t = 1;

/// rustc's sys/time/unsupported.rs: `Instant::now()` and `SystemTime::now()` panic.
pub fn now(_clock: clockid_t) -> super::time::Timespec {
    panic!("time not implemented on this platform")
}

/// rustc's sys/thread/wasm.rs sleep: an atomic wait on a private word that nothing notifies, so
/// only its timeout ends it.
pub fn sleep(dur: Duration) {
    let mut nanos = dur.as_nanos();
    while nanos > 0 {
        let amt = core::cmp::min(i64::MAX as u128, nanos);
        let mut x = 0;
        let val = unsafe { wasm::memory_atomic_wait32(&mut x, 0, amt as i64) };
        debug_assert_eq!(val, 2);
        nanos -= amt;
    }
}

/// Waits while `*futex == expected` (rustc pal/wasm/atomics/futex.rs); false = timed out.
pub fn futex_wait(futex: &AtomicU32, expected: u32, timeout: Option<Duration>) -> bool {
    let timeout = timeout.and_then(|t| t.as_nanos().try_into().ok()).unwrap_or(-1);
    unsafe { wasm::memory_atomic_wait32(futex.as_ptr() as *mut i32, expected as i32, timeout) < 2 }
}

/// Wakes one thread blocked in `futex_wait`; true if there was one.
pub fn futex_wake(futex: &AtomicU32) -> bool {
    unsafe { wasm::memory_atomic_notify(futex.as_ptr() as *mut i32, 1) > 0 }
}

pub fn futex_wake_all(futex: &AtomicU32) {
    unsafe {
        wasm::memory_atomic_notify(futex.as_ptr() as *mut i32, i32::MAX as u32);
    }
}

/// rustc's `thread::current_os_id()` on wasm has no OS id: it falls back to the Rust thread id
/// (the first thread to ask is 1).
pub fn thread_os_id() -> u64 {
    portable::current_thread() as u64
}

/// The first thread to ask is the main thread (the runtime does not say which thread started).
static MAIN_THREAD: AtomicUsize = AtomicUsize::new(0);

pub fn is_main_thread() -> bool {
    let me = portable::current_thread();
    match MAIN_THREAD.compare_exchange(0, me, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => true,
        Err(t) => t == me,
    }
}

/// rustc's sys/thread/unsupported.rs `set_name`: "nope".
pub fn set_thread_name(_name: *const c_char) {}

pub mod portable {
    use super::*;

    /// rustc's sys/io/error/generic.rs.
    pub fn errno() -> i32 {
        0
    }

    pub fn error_string(_code: i32) -> String {
        String::from("operation successful")
    }

    /// rustc's thread::yield_now is a no-op here.
    pub fn thread_yield() {}

    /// 0 = unknown (rustc: `io::Error::UNKNOWN_THREAD_COUNT`).
    pub fn available_parallelism() -> usize {
        0
    }

    static ID_KEY: AtomicU32 = AtomicU32::new(0);
    static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

    extern "C" fn id_dtor(_: *mut c_void) {}

    /// The calling thread's id (nonzero, unique among live threads): a counter value kept in a
    /// thread-local key.
    pub fn current_thread() -> usize {
        let mut k = ID_KEY.load(Ordering::Acquire);
        if k == 0 {
            let new = key_create(id_dtor, true).unwrap();
            k = match ID_KEY.compare_exchange(0, new + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => new + 1,
                Err(other) => {
                    key_delete(new);
                    other
                }
            };
        }
        let key = k - 1;
        let id = key_get(key) as usize;
        if id != 0 {
            return id;
        }
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        key_set(key, id as *const c_void);
        id
    }

    /// std cannot start a thread here (rustc's sys/thread/unsupported.rs: `Thread(!)`).
    pub struct NativeThread(!);

    pub fn spawn_thread(_stack_size: usize, _f: fn(*mut u8), _arg: *mut u8) -> Result<NativeThread, i32> {
        Err(0)
    }

    pub fn join_thread(t: NativeThread) {
        t.0
    }

    /// A thread-local storage key whose destructor runs with the thread's value at thread exit.
    pub type Key = u32;
    extern "C" {
        fn __relax_key_create(dtor: extern "C" fn(*mut c_void), late: u32) -> u32;
        fn __relax_key_delete(key: u32);
        fn __relax_key_get(key: u32) -> *mut c_void;
        fn __relax_key_set(key: u32, value: *const c_void);
    }
    /// `late`: the value type has no drop glue; its slot stays readable through every destructor
    /// round and is freed (by `dtor`) after them.
    pub fn key_create(dtor: extern "C" fn(*mut c_void), late: bool) -> Option<Key> { Some(unsafe { __relax_key_create(dtor, late as u32) }) }
    pub fn key_delete(key: Key) { unsafe { __relax_key_delete(key); } }
    pub fn key_get(key: Key) -> *mut c_void { unsafe { __relax_key_get(key) } }
    pub fn key_set(key: Key, value: *const c_void) { unsafe { __relax_key_set(key, value); } }

    /// rustc's sys/stdio/unsupported.rs: standard output and error are a sink that takes every byte.
    pub fn stdio_write(_fd: i32, buf: &[u8]) -> Result<usize, i32> {
        Ok(buf.len())
    }

    /// Standard input is at end of file.
    pub fn stdio_read(_fd: i32, _buf: &mut [u8]) -> Result<usize, i32> {
        Ok(0)
    }

    pub fn stdio_is_terminal(_fd: i32) -> bool {
        false
    }
}
