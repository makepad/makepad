//! `RandomState`'s keys, as real std makes them (library/std/src/hash/random.rs): each thread's
//! first `RandomState::new` reads 16 bytes of OS randomness (sys::random::hashmap_random_keys),
//! and every later one on that thread takes the same keys with k0 incremented by one. So two
//! processes get different keys, and maps made one after another on a thread differ too.
//! The OS sources are real std's (library/std/src/sys/random/{linux,apple,mod}.rs), except
//! Windows, which uses RtlGenRandom like the rest of Relax's std instead of ProcessPrng.

#[cfg(all(not(relax_check), not(relax_web)))]
use crate::cell::Cell;

// The web profile: every RandomState takes the same fixed keys (hashmap_random_keys below), with
// no per-thread k0 + 1. A wasm export's trace runs the program natively, where its code can run on
// more than one OS thread (each with its own count of RandomStates made) and the host can make maps
// the page never makes; any difference in those counts gave a later map other keys on the page
// than in the trace, and so another iteration order (P(DOOM) nodegate a1 with fixed base keys:
// first difference still the freed texture order at frame 16727). Iteration orders then depend
// on each map's contents and history only, the same in the trace and on the page.
#[cfg(all(not(relax_check), relax_web))]
pub(crate) fn next_keys() -> (u64, u64) {
    hashmap_random_keys()
}

/// The calling thread's next keys (std's `KEYS.with(|keys| ..)` in RandomState::new).
#[cfg(all(not(relax_check), not(relax_web)))]
pub(crate) fn next_keys() -> (u64, u64) {
    // None until the thread's first RandomState (std: the thread_local's lazy initialiser)
    #[thread_local]
    static KEYS: Cell<Option<(u64, u64)>> = Cell::new(None);
    let (k0, k1) = match KEYS.get() {
        Some(k) => k,
        None => hashmap_random_keys(),
    };
    KEYS.set(Some((k0.wrapping_add(1), k1)));
    (k0, k1)
}

/// The rustc-compiled shim (std/check): the host std's keys, one pair per thread, stepped as above.
#[cfg(relax_check)]
pub(crate) fn next_keys() -> (u64, u64) {
    use std::hash::BuildHasher;
    std::thread_local!(static KEYS: std::cell::Cell<(u64, u64)> = {
        let s = std::hash::RandomState::new();
        std::cell::Cell::new((s.hash_one(0u8), s.hash_one(1u8)))
    });
    KEYS.with(|keys| {
        let (k0, k1) = keys.get();
        keys.set((k0.wrapping_add(1), k1));
        (k0, k1)
    })
}

#[cfg(not(relax_check))]
fn keys_from(buf: &[u8; 16]) -> (u64, u64) {
    let mut a = [0u8; 8];
    let mut b = [0u8; 8];
    a.copy_from_slice(&buf[..8]);
    b.copy_from_slice(&buf[8..]);
    (u64::from_ne_bytes(a), u64::from_ne_bytes(b))
}

// sys/random/linux.rs: getrandom(bytes, insecure = true) -- GRND_INSECURE, else GRND_NONBLOCK;
// when the call is unavailable or would block, /dev/urandom
#[cfg(all(not(relax_check), not(relax_web), any(target_os = "linux", target_os = "android")))]
fn hashmap_random_keys() -> (u64, u64) {
    use crate::ffi::{c_char, c_int, c_void};
    use crate::sync::atomic::{AtomicBool, Ordering::Relaxed};
    extern "C" {
        fn getrandom(buf: *mut c_void, len: usize, flags: u32) -> isize;
        fn __errno_location() -> *mut c_int;
        fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
        fn close(fd: c_int) -> c_int;
    }
    const GRND_NONBLOCK: u32 = 1;
    const GRND_INSECURE: u32 = 4;
    const EINTR: c_int = 4;
    const EAGAIN: c_int = 11;
    const EPERM: c_int = 1;
    const EINVAL: c_int = 22;
    const ENOSYS: c_int = 38;
    const O_RDONLY: c_int = 0;
    const O_CLOEXEC: c_int = 0o2000000;
    static GETRANDOM_AVAILABLE: AtomicBool = AtomicBool::new(true);
    static GRND_INSECURE_AVAILABLE: AtomicBool = AtomicBool::new(true);
    let mut buf = [0u8; 16];
    let mut off = 0;
    if GETRANDOM_AVAILABLE.load(Relaxed) {
        while off < buf.len() {
            let flags = if GRND_INSECURE_AVAILABLE.load(Relaxed) { GRND_INSECURE } else { GRND_NONBLOCK };
            let r = unsafe { getrandom(buf.as_mut_ptr().add(off) as *mut c_void, buf.len() - off, flags) };
            if r != -1 {
                off += r as usize;
                continue;
            }
            match unsafe { *__errno_location() } {
                EINTR => {}
                EINVAL if flags == GRND_INSECURE => GRND_INSECURE_AVAILABLE.store(false, Relaxed),
                EAGAIN if flags == GRND_NONBLOCK => break,
                ENOSYS | EPERM => {
                    GETRANDOM_AVAILABLE.store(false, Relaxed);
                    break;
                }
                _ => panic!("failed to generate random data"),
            }
        }
    }
    if off < buf.len() {
        // (std reads the rest from /dev/urandom: read_exact on the device)
        let fd = unsafe { open(b"/dev/urandom\0".as_ptr() as *const c_char, O_RDONLY | O_CLOEXEC) };
        if fd < 0 {
            panic!("failed to generate random data");
        }
        while off < buf.len() {
            let r = unsafe { read(fd, buf.as_mut_ptr().add(off) as *mut c_void, buf.len() - off) };
            if r > 0 {
                off += r as usize;
            } else if r == 0 || unsafe { *__errno_location() } != EINTR {
                panic!("failed to generate random data");
            }
        }
        unsafe { close(fd) };
    }
    keys_from(&buf)
}

// sys/random/apple.rs: CCRandomGenerateBytes
#[cfg(all(not(relax_check), not(relax_web), target_os = "macos"))]
fn hashmap_random_keys() -> (u64, u64) {
    use crate::ffi::{c_int, c_void};
    extern "C" {
        fn CCRandomGenerateBytes(bytes: *mut c_void, count: usize) -> c_int;
    }
    let mut buf = [0u8; 16];
    let ret = unsafe { CCRandomGenerateBytes(buf.as_mut_ptr() as *mut c_void, buf.len()) };
    assert!(ret == 0, "failed to generate random data");
    keys_from(&buf)
}

#[cfg(all(not(relax_check), not(relax_web), windows))]
fn hashmap_random_keys() -> (u64, u64) {
    #[link(name = "advapi32")]
    extern "system" {
        #[link_name = "SystemFunction036"]
        fn RtlGenRandom(buf: *mut u8, len: u32) -> u8;
    }
    let mut buf = [0u8; 16];
    assert!(unsafe { RtlGenRandom(buf.as_mut_ptr(), buf.len() as u32) } != 0, "failed to generate random data");
    keys_from(&buf)
}

// The web profile: fixed keys. Real std's wasm32-unknown-unknown keys (sys/random/unsupported.rs)
// are the addresses of a stack and a heap value: the same on every load of a page, so its
// iteration orders are fixed too -- but a wasm export's trace runs the program natively, where
// those addresses are the host process's, and the page must replay the trace exactly (the
// export's frame gate: HashMap iteration orders reach what the page draws and frees)
#[cfg(all(not(relax_check), relax_web))]
fn hashmap_random_keys() -> (u64, u64) {
    (0x082e_fa98_ec4e_6c89, 0x082e_fa98_ec4e_6c89)
}
