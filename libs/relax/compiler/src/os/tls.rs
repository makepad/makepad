//! The OS key holding each thread's thread-local block (jit.rs rt_tls_block), and the fast
//! path compiled code reads it through (`Inst::TlsAddr`: `Env::tls_key`).
//!
//! - `key_create(dtor)`: one key per process; `dtor(block)` runs at thread exit.
//! - `get(key)` / `set(key, block)`: the calling thread's block.
//! - `jit_offset(key, emit)`: the value of `Env::tls_key` for this OS (`emit` places glue code
//!   and returns its address): Linux x86-64 the fs-relative offset of a static-TLS cell, macOS
//!   the TSD slot (checked against pthread_getspecific), Android bionic's TLS_SLOT_APP.
//! - `static_tls_*`: Linux x86-64's reserve of static TLS for programs' `#[thread_local]` statics.

include!("../rt_abi.rs");
pub use imp::*;

#[cfg(unix)]
mod unix {
    extern "C" {
        pub fn pthread_key_create(key: *mut usize, dtor: Option<extern "C" fn(*mut u8)>) -> i32;
        pub fn pthread_getspecific(key: usize) -> *mut u8;
        pub fn pthread_setspecific(key: usize, v: *mut u8) -> i32;
    }

    pub fn key_create(dtor: extern "C" fn(*mut u8)) -> u64 {
        // pthread_key_t: unsigned long on macOS, unsigned int on Linux (zero-extended here)
        let mut key: usize = 0;
        unsafe {
            pthread_key_create(&mut key, Some(dtor));
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let key = key as u32 as usize;
        key as u64
    }

    pub fn get(key: u64) -> *mut u8 {
        unsafe { pthread_getspecific(key as usize) }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub use super::unix::{get, key_create};

    thread_local! {
        /// this thread's block (static TLS: JIT code reads it at fs:[env.tls_key])
        static TLS_BLOCK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    pub fn set(key: u64, b: *mut u8) {
        unsafe {
            super::unix::pthread_setspecific(key as usize, b);
        }
        TLS_BLOCK.with(|c| c.set(b as u64));
    }

    /// Linux x86-64: the block pointer lives in a static-TLS cell of this (executable)
    /// runtime, at a fixed offset from the thread pointer that JIT code reads via fs.
    pub fn jit_offset(_key: u64, emit: &dyn Fn(&[u8]) -> u64) -> u64 {
        let reader = emit(&relax_backend::x64::fs_reader());
        let fs_base: rt_fn_ty!(fn() -> u64) = unsafe { std::mem::transmute(reader as usize) };
        let cell = TLS_BLOCK.with(|c| c as *const std::cell::Cell<u64> as u64);
        let off = cell.wrapping_sub(fs_base()) as i64;
        if off < i32::MIN as i64 || off > i32::MAX as i64 || !static_cell(cell) {
            panic!("thread-local block cell is not in static TLS (offset {:#x})", off);
        }
        off as u64
    }

    /// Under Relax the cell is the outer program's thread-local: in the outer runtime's static TLS
    /// when it placed it there (`__relax_tls_fixed`), else in a block of this thread only.
    fn static_cell(cell: u64) -> bool {
        #[cfg(relax)]
        {
            extern "C" {
                fn __relax_tls_fixed(addr: u64) -> u32;
            }
            unsafe { __relax_tls_fixed(cell) != 0 }
        }
        #[cfg(not(relax))]
        {
            let _ = cell;
            true
        }
    }
}

/// Linux x86-64: the executable's static TLS that JIT programs' `#[thread_local]` statics share
/// (jit.rs Unit::thread_local_place, `Inst::TlsStatic`): `RESERVE` bytes at one offset from the
/// thread pointer on every thread (the executable's own static TLS, as `TLS_BLOCK`), zero when a
/// thread starts (its TLS image is .tbss). Handed out once per process, never freed: a variable
/// keeps its offset for the process. Not when this runtime itself runs under Relax: its
/// thread-locals are the outer program's, which need not be at a fixed offset.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod fixed {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    const RESERVE: usize = 16 << 10;
    /// 64-aligned: the thread pointer is aligned to the executable's TLS alignment, so a variable
    /// aligned at most that within the reserve is aligned in memory
    #[repr(C, align(64))]
    struct Reserve([Cell<u64>; RESERVE / 8]);
    thread_local! {
        static PROGRAM_TLS: Reserve = const { Reserve([const { Cell::new(0) }; RESERVE / 8]) };
    }
    static BASE: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();
    static NEXT: AtomicU32 = AtomicU32::new(0);

    fn reserve() -> *const Cell<u64> {
        PROGRAM_TLS.with(|r| r.0.as_ptr())
    }

    pub fn static_tls_init(emit: &dyn Fn(&[u8]) -> u64) {
        BASE.get_or_init(|| {
            if cfg!(relax) {
                return None;
            }
            let reader = emit(&relax_backend::x64::fs_reader());
            let fs_base: rt_fn_ty!(fn() -> u64) = unsafe { std::mem::transmute(reader as usize) };
            FS_BASE.store(reader, Ordering::Release);
            let off = (reserve() as u64).wrapping_sub(fs_base()) as i64;
            (off >= i32::MIN as i64 && off + RESERVE as i64 <= i32::MAX as i64).then_some(off as i32)
        });
    }

    pub fn static_tls_base() -> Option<i32> {
        BASE.get().copied().flatten()
    }

    static FS_BASE: AtomicU64 = AtomicU64::new(0);

    /// For a runtime running under this one (`__relax_tls_fixed`, os/tls.rs static_cell):
    /// whether `addr` is a variable this runtime placed in static TLS (as seen from the calling
    /// thread), i.e. at the same offset from the thread pointer on every thread.
    pub extern "C" fn relax_tls_fixed(addr: u64) -> u32 {
        let (Some(base), r) = (static_tls_base(), FS_BASE.load(Ordering::Acquire)) else { return 0 };
        if r == 0 {
            return 0;
        }
        let fs_base: rt_fn_ty!(fn() -> u64) = unsafe { std::mem::transmute(r as usize) };
        let off = addr.wrapping_sub(fs_base()) as i64;
        (off >= base as i64 && off < base as i64 + NEXT.load(Ordering::Acquire) as i64) as u32
    }

    pub fn static_tls_alloc(size: u32, align: u32) -> Option<i32> {
        let base = static_tls_base()?;
        if align > 64 {
            return None;
        }
        let mut cur = NEXT.load(Ordering::Relaxed);
        loop {
            let at = (cur + align - 1) / align * align;
            if at as usize + size as usize > RESERVE {
                return None;
            }
            match NEXT.compare_exchange(cur, at + size, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return Some(base + at as i32),
                Err(c) => cur = c,
            }
        }
    }

    /// Exchanges the calling thread's static TLS words in use with `saved` (empty: zeros), so a
    /// scoped thread-local block (jit.rs domain_callback) starts from fresh values and the
    /// thread's own come back after it.
    pub fn static_tls_swap(saved: &mut Vec<u64>) {
        if static_tls_base().is_none() {
            return;
        }
        let n = (NEXT.load(Ordering::Acquire) as usize + 7) / 8;
        saved.resize(n, 0);
        let r = reserve();
        for (k, w) in saved.iter_mut().enumerate() {
            *w = unsafe { (*r.add(k)).replace(*w) };
        }
    }
}
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
mod fixed {
    pub fn static_tls_init(_emit: &dyn Fn(&[u8]) -> u64) {}
    pub fn static_tls_base() -> Option<i32> {
        None
    }
    pub fn static_tls_alloc(_size: u32, _align: u32) -> Option<i32> {
        None
    }
    pub fn static_tls_swap(_saved: &mut Vec<u64>) {}
    pub extern "C" fn relax_tls_fixed(_addr: u64) -> u32 {
        0
    }
}
pub use fixed::*;

#[cfg(target_os = "android")]
mod imp {
    pub use super::unix::{get, key_create};

    /// bionic TLS_SLOT_APP: word 2 of the slot array tpidr_el0 points at, "available for use by
    /// apps in API level 29 and later" (bionic libc/platform/bionic/tls_defines.h; our minSdk is
    /// 30). The process is ours (one makepad run host), so Relax owns the slot. The pthread key
    /// stays for its thread-exit destructor; JIT code reads the slot (codegen arm64 tls_addr).
    const TLS_SLOT_APP: u64 = 2;

    fn slot() -> *mut u64 {
        let tp: u64;
        unsafe { core::arch::asm!("mrs {}, tpidr_el0", out(reg) tp, options(nomem, nostack, preserves_flags)); }
        (tp as *mut u64).wrapping_add(TLS_SLOT_APP as usize)
    }

    pub fn set(key: u64, block: *mut u8) {
        unsafe {
            super::unix::pthread_setspecific(key as usize, block);
            slot().write_volatile(block as u64);
        }
    }

    /// The slot index JIT code reads (`Env::tls_key`), checked through the emitted reader.
    pub fn jit_offset(key: u64, emit: &dyn Fn(&[u8]) -> u64) -> u64 {
        let reader = emit(&relax_backend::arm64::tsd_reader());
        let read: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(reader as usize) };
        let old = get(key);
        set(key, 0x5a5a_0000 as *mut u8);
        let seen = read(TLS_SLOT_APP);
        set(key, old);
        if seen != 0x5a5a_0000 {
            panic!("thread-local fast path: bionic TLS_SLOT_APP reads {:#x}", seen);
        }
        TLS_SLOT_APP
    }
}

#[cfg(target_os = "macos")]
mod imp {
    pub use super::unix::{get, key_create};
    use super::unix::pthread_setspecific;

    pub fn set(key: u64, b: *mut u8) {
        unsafe {
            pthread_setspecific(key as usize, b);
        }
    }

    /// The inline fast path reads the TSD slot directly: check it sees pthread's value.
    pub fn jit_offset(key: u64, emit: &dyn Fn(&[u8]) -> u64) -> u64 {
        let reader = emit(&relax_backend::arm64::tsd_reader());
        let read: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(reader as usize) };
        let old = get(key);
        set(key, 0x5a5a_0000 as *mut u8);
        let seen = read(key);
        set(key, old);
        if seen != 0x5a5a_0000 {
            panic!("thread-local fast path: TSD slot {} reads {:#x}", key, seen);
        }
        key
    }
}

/// Windows: a TlsAlloc index; compiled code reads the TEB's slot for it, gs:[0x1480 + 8 index]
/// (TEB.TlsSlots, the 64 directly addressable indexes; TlsGetValue reads the same word). The
/// OS has no per-key destructor: a host thread-local guard frees the block at thread exit.
#[cfg(windows)]
mod imp {
    use crate::os::win32::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DTOR: AtomicUsize = AtomicUsize::new(0);
    const TLS_SLOTS: u64 = 0x1480;

    struct Guard(std::cell::Cell<(u64, usize)>);
    impl Drop for Guard {
        fn drop(&mut self) {
            let (key, b) = self.0.get();
            let d = DTOR.load(Ordering::Acquire);
            if b != 0 && d != 0 {
                let dtor: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(d) };
                unsafe { TlsSetValue(key as u32, std::ptr::null_mut()) };
                dtor(b as *mut u8);
            }
        }
    }
    thread_local! {
        static GUARD: Guard = const { Guard(std::cell::Cell::new((0, 0))) };
    }

    pub fn key_create(dtor: extern "C" fn(*mut u8)) -> u64 {
        DTOR.store(dtor as *const () as usize, Ordering::Release);
        let k = unsafe { TlsAlloc() };
        if k == u32::MAX {
            panic!("TlsAlloc failed");
        }
        k as u64
    }

    pub fn get(key: u64) -> *mut u8 {
        unsafe { TlsGetValue(key as u32) }
    }

    pub fn set(key: u64, b: *mut u8) {
        unsafe {
            TlsSetValue(key as u32, b);
        }
        let _ = GUARD.try_with(|g| g.0.set((key, b as usize)));
    }

    pub fn jit_offset(key: u64, emit: &dyn Fn(&[u8]) -> u64) -> u64 {
        if key >= 64 {
            panic!("thread-local fast path: TlsAlloc index {} is not one of the TEB's 64 direct slots", key);
        }
        let off = TLS_SLOTS + 8 * key;
        let reader = emit(&relax_backend::x64::gs_reader());
        let read: rt_fn_ty!(fn(u64) -> u64) = unsafe { std::mem::transmute(reader as usize) };
        let old = get(key);
        unsafe { TlsSetValue(key as u32, 0x5a5a_0000 as *mut u8) };
        let seen = read(off);
        unsafe { TlsSetValue(key as u32, old) };
        if seen != 0x5a5a_0000 {
            panic!("thread-local fast path: gs:[{:#x}] reads {:#x}", off, seen);
        }
        off
    }
}
