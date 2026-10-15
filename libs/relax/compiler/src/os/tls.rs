//! The OS key holding each thread's thread-local block (jit.rs rt_tls_block), and the fast
//! path compiled code reads it through (`Inst::TlsAddr`: `Env::tls_key`).
//!
//! - `key_create(dtor)`: one key per process; `dtor(block)` runs at thread exit.
//! - `get(key)` / `set(key, block)`: the calling thread's block.
//! - `jit_offset(key, emit)`: the value of `Env::tls_key` for this OS (`emit` places glue code
//!   and returns its address): Linux x86-64 the fs-relative offset of a static-TLS cell, macOS
//!   the TSD slot (checked against pthread_getspecific), Android bionic's TLS_SLOT_APP.

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
        let reader = emit(&crate::x64::fs_reader());
        let fs_base: rt_fn_ty!(fn() -> u64) = unsafe { std::mem::transmute(reader as usize) };
        let cell = TLS_BLOCK.with(|c| c as *const std::cell::Cell<u64> as u64);
        let off = cell.wrapping_sub(fs_base()) as i64;
        if off < i32::MIN as i64 || off > i32::MAX as i64 {
            panic!("thread-local block cell is not in static TLS (offset {:#x})", off);
        }
        off as u64
    }
}

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
        let reader = emit(&crate::arm64::tsd_reader());
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
        let reader = emit(&crate::arm64::tsd_reader());
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
        let reader = emit(&crate::x64::gs_reader());
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
