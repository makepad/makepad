//! rustc shim root for the std-os lane (see Cargo.toml). The root mirrors HotRust std's
//! root for the OS modules; `crate::{cell, fmt, ops, ..}` are real core/alloc so the core-side
//! files (mounted as core_time / core_atomic / core_ffi) resolve their `crate::` paths.
#![allow(dead_code, unused_imports, unused_unsafe, unused_mut, unused_variables, non_camel_case_types)]
#![allow(clippy::all)]

extern crate alloc;

// ---- real core/alloc under the names HotRust core files use
pub use alloc::borrow;
pub use alloc::boxed;
pub use alloc::fmt;
pub use alloc::string;
pub use alloc::vec;
pub use core::cell;
pub use core::cmp;
pub use core::error;
pub use core::hash;
pub use core::iter;
pub use core::mem;
pub use core::ops;
pub use core::ptr;
pub use core::slice;
pub use core::str;

// ---- the lane's core files
#[path = "../../core/time.rs"]
pub mod core_time;
#[path = "../../core/sync/atomic.rs"]
pub mod core_atomic;
#[path = "../../core/ffi/mod.rs"]
pub mod core_ffi;

/// `intrinsics_atomic` over real atomics (by width), the same signatures as HotRust core's.
pub mod intrinsics_atomic {
    use core::mem::{size_of, transmute_copy};
    use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};

    fn ord(o: u8) -> Ordering {
        match o {
            0 => Ordering::Relaxed,
            1 => Ordering::Release,
            2 => Ordering::Acquire,
            3 => Ordering::AcqRel,
            _ => Ordering::SeqCst,
        }
    }
    fn fail_ord(o: u8) -> Ordering {
        match o {
            0 => Ordering::Relaxed,
            2 => Ordering::Acquire,
            _ => Ordering::SeqCst,
        }
    }
    unsafe fn to<T, U>(v: T) -> U {
        transmute_copy(&core::mem::ManuallyDrop::new(v))
    }

    pub unsafe fn atomic_load<T>(p: *const T, o: u8) -> T {
        match size_of::<T>() {
            1 => to((*(p as *const AtomicU8)).load(ord(o))),
            2 => to((*(p as *const AtomicU16)).load(ord(o))),
            4 => to((*(p as *const AtomicU32)).load(ord(o))),
            _ => to((*(p as *const AtomicU64)).load(ord(o))),
        }
    }
    pub unsafe fn atomic_store<T>(p: *mut T, v: T, o: u8) {
        match size_of::<T>() {
            1 => (*(p as *const AtomicU8)).store(to(v), ord(o)),
            2 => (*(p as *const AtomicU16)).store(to(v), ord(o)),
            4 => (*(p as *const AtomicU32)).store(to(v), ord(o)),
            _ => (*(p as *const AtomicU64)).store(to(v), ord(o)),
        }
    }
    pub unsafe fn atomic_swap<T>(p: *mut T, v: T, o: u8) -> T {
        match size_of::<T>() {
            1 => to((*(p as *const AtomicU8)).swap(to(v), ord(o))),
            2 => to((*(p as *const AtomicU16)).swap(to(v), ord(o))),
            4 => to((*(p as *const AtomicU32)).swap(to(v), ord(o))),
            _ => to((*(p as *const AtomicU64)).swap(to(v), ord(o))),
        }
    }
    pub unsafe fn atomic_cxchg<T>(p: *mut T, old: T, new: T, s: u8, f: u8) -> (T, bool) {
        let r: Result<u64, u64> = match size_of::<T>() {
            1 => match (*(p as *const AtomicU8)).compare_exchange(to(old), to(new), ord(s), fail_ord(f)) {
                Ok(v) => Ok(v as u64),
                Err(v) => Err(v as u64),
            },
            2 => match (*(p as *const AtomicU16)).compare_exchange(to(old), to(new), ord(s), fail_ord(f)) {
                Ok(v) => Ok(v as u64),
                Err(v) => Err(v as u64),
            },
            4 => match (*(p as *const AtomicU32)).compare_exchange(to(old), to(new), ord(s), fail_ord(f)) {
                Ok(v) => Ok(v as u64),
                Err(v) => Err(v as u64),
            },
            _ => (*(p as *const AtomicU64)).compare_exchange(to(old), to(new), ord(s), fail_ord(f)),
        };
        match r {
            Ok(v) => (from_u64::<T>(v), true),
            Err(v) => (from_u64::<T>(v), false),
        }
    }
    unsafe fn from_u64<T>(v: u64) -> T {
        match size_of::<T>() {
            1 => to(v as u8),
            2 => to(v as u16),
            4 => to(v as u32),
            _ => to(v),
        }
    }
    pub unsafe fn atomic_cxchg_weak<T>(p: *mut T, old: T, new: T, s: u8, f: u8) -> (T, bool) {
        atomic_cxchg(p, old, new, s, f)
    }
    /// read-modify-write through a CAS loop on the integer bits; `op` works on u64 values
    /// sign- or zero-extended per `signed`.
    unsafe fn rmw<T>(p: *mut T, v: T, o: u8, op: fn(u64, u64, usize, bool) -> u64, signed: bool) -> T {
        let size = size_of::<T>();
        let vb = bits(&v, size);
        loop {
            let cur: T = atomic_load(p as *const T, 0);
            let cb = bits(&cur, size);
            let nb = op(cb, vb, size, signed);
            let new: T = from_u64(nb);
            let (_, ok) = atomic_cxchg(p, cur, new, o, 0);
            if ok {
                return from_u64(cb);
            }
        }
    }
    unsafe fn bits<T>(v: &T, size: usize) -> u64 {
        match size {
            1 => *(v as *const T as *const u8) as u64,
            2 => *(v as *const T as *const u16) as u64,
            4 => *(v as *const T as *const u32) as u64,
            _ => *(v as *const T as *const u64),
        }
    }
    fn mask(size: usize) -> u64 {
        if size >= 8 { u64::MAX } else { (1u64 << (size * 8)) - 1 }
    }
    fn sext(x: u64, size: usize) -> i64 {
        let sh = 64 - size * 8;
        ((x << sh) as i64) >> sh
    }
    pub unsafe fn atomic_fetch_add<T>(p: *mut T, v: T, o: u8) -> T {
        rmw(p, v, o, |a, b, s, _| a.wrapping_add(b) & mask(s), false)
    }
    pub unsafe fn atomic_fetch_sub<T>(p: *mut T, v: T, o: u8) -> T {
        rmw(p, v, o, |a, b, s, _| a.wrapping_sub(b) & mask(s), false)
    }
    pub unsafe fn atomic_fetch_and<T>(p: *mut T, v: T, o: u8) -> T {
        rmw(p, v, o, |a, b, _, _| a & b, false)
    }
    pub unsafe fn atomic_fetch_or<T>(p: *mut T, v: T, o: u8) -> T {
        rmw(p, v, o, |a, b, _, _| a | b, false)
    }
    pub unsafe fn atomic_fetch_xor<T>(p: *mut T, v: T, o: u8) -> T {
        rmw(p, v, o, |a, b, _, _| a ^ b, false)
    }
    pub unsafe fn atomic_fetch_nand<T>(p: *mut T, v: T, o: u8) -> T {
        rmw(p, v, o, |a, b, s, _| !(a & b) & mask(s), false)
    }
    /// HotRust takes signedness from T; the shim learns it from the type name.
    fn is_signed<T>() -> bool {
        let n = core::any::type_name::<T>();
        n.starts_with('i')
    }
    pub unsafe fn atomic_fetch_max<T>(p: *mut T, v: T, o: u8) -> T {
        let signed = is_signed::<T>();
        rmw(p, v, o, |a, b, s, sg| if sg { if sext(a, s) >= sext(b, s) { a } else { b } } else if a >= b { a } else { b }, signed)
    }
    pub unsafe fn atomic_fetch_min<T>(p: *mut T, v: T, o: u8) -> T {
        let signed = is_signed::<T>();
        rmw(p, v, o, |a, b, s, sg| if sg { if sext(a, s) <= sext(b, s) { a } else { b } } else if a <= b { a } else { b }, signed)
    }
    pub unsafe fn atomic_fence(o: u8) {
        core::sync::atomic::fence(ord(o))
    }
    pub unsafe fn atomic_compiler_fence(o: u8) {
        core::sync::atomic::compiler_fence(ord(o))
    }
    pub unsafe fn spin_loop_hint() {
        core::hint::spin_loop()
    }
}

// ---- the lane's std modules, at HotRust std's root paths
#[path = "../../std/env.rs"]
pub mod env;
#[path = "../../std/ffi/mod.rs"]
pub mod ffi;
#[path = "../../std/fs.rs"]
pub mod fs;
#[path = "../../std/io/mod.rs"]
pub mod io;
#[path = "../../std/net/mod.rs"]
pub mod net;
#[path = "../../std/os/mod.rs"]
pub mod os;
#[path = "../../std/panic.rs"]
pub mod panic;
#[path = "../../std/path.rs"]
pub mod path;
#[path = "../../std/process.rs"]
pub mod process;
#[path = "../../std/sync/mod.rs"]
pub mod sync;
#[path = "../../std/sys/mod.rs"]
pub mod sys;
#[path = "../../std/thread/mod.rs"]
pub mod thread;
#[path = "../../std/time.rs"]
pub mod time;
