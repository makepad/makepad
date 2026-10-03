//! rustc-side shim for Rapid's std (see Cargo.toml). The crate root mirrors Rapid's
//! `core` root: modules that are Rapid std *implementations* are mounted from
//! ../../core (and std-side ones from ../../std) with #[path]; lang-level modules
//! (Option, ops, closures, Box, mem, ptr, iter traits) are real std re-exported, and the
//! intrinsics/panicking modules are shims backed by real std.
//!
//! Each area has its own section below; add a module to your section only.
#![allow(dead_code, unused_imports, unused_unsafe, unused_mut, unused_variables)]
#![allow(clippy::all)]
#![allow(dangerous_implicit_autorefs)]

extern crate alloc;

// ---------------------------------------------------------------- lang: real std
pub use std::any;
pub use std::borrow;
pub use std::error;
pub use std::clone;
pub use std::cmp;
pub use std::convert;
pub use std::default;
pub use std::hint;
pub use std::iter;
pub use std::marker;
pub use std::mem;
pub use std::ops;
pub use std::option;
pub use std::panic;
pub use std::ptr;
pub use std::result;
pub use std::boxed;

// ---------------------------------------------------------------- intrinsics shims
pub mod intrinsics {
    pub unsafe fn sqrt_f64(x: f64) -> f64 { x.sqrt() }
    pub unsafe fn sqrt_f32(x: f32) -> f32 { x.sqrt() }
    pub unsafe fn floor_f64(x: f64) -> f64 { x.floor() }
    pub unsafe fn floor_f32(x: f32) -> f32 { x.floor() }
    pub unsafe fn ceil_f64(x: f64) -> f64 { x.ceil() }
    pub unsafe fn ceil_f32(x: f32) -> f32 { x.ceil() }
    pub unsafe fn trunc_f64(x: f64) -> f64 { x.trunc() }
    pub unsafe fn trunc_f32(x: f32) -> f32 { x.trunc() }
    pub unsafe fn rint_f64(x: f64) -> f64 { x.round_ties_even() }
    pub unsafe fn rint_f32(x: f32) -> f32 { x.round_ties_even() }
    pub unsafe fn fabs_f64(x: f64) -> f64 { x.abs() }
    pub unsafe fn fabs_f32(x: f32) -> f32 { x.abs() }
    pub unsafe fn fmin_f64(x: f64, y: f64) -> f64 { x.min(y) }
    pub unsafe fn fmax_f64(x: f64, y: f64) -> f64 { x.max(y) }
    pub unsafe fn fmin_f32(x: f32, y: f32) -> f32 { x.min(y) }
    pub unsafe fn fmax_f32(x: f32, y: f32) -> f32 { x.max(y) }
    pub unsafe fn to_bits_f64(x: f64) -> u64 { x.to_bits() }
    pub unsafe fn to_bits_f32(x: f32) -> u32 { x.to_bits() }
    pub unsafe fn from_bits_f64(x: u64) -> f64 { f64::from_bits(x) }
    pub unsafe fn from_bits_f32(x: u32) -> f32 { f32::from_bits(x) }
}

pub mod intrinsics_mem {
    pub unsafe fn size_of<T>() -> usize { core::mem::size_of::<T>() }
    pub unsafe fn align_of<T>() -> usize { core::mem::align_of::<T>() }
    pub unsafe fn size_of_val<T: ?Sized>(v: &T) -> usize { core::mem::size_of_val(v) }
    pub unsafe fn align_of_val<T: ?Sized>(v: &T) -> usize { core::mem::align_of_val(v) }
    pub unsafe fn needs_drop<T>() -> bool { core::mem::needs_drop::<T>() }
    pub unsafe fn drop_in_place<T: ?Sized>(p: *mut T) { core::ptr::drop_in_place(p) }
    pub unsafe fn ptr_read<T>(p: *const T) -> T { core::ptr::read(p) }
    pub unsafe fn ptr_write<T>(p: *mut T, v: T) { core::ptr::write(p, v) }
    pub unsafe fn ptr_add<T>(p: *const T, n: usize) -> *const T { p.add(n) }
    pub unsafe fn ptr_add_mut<T>(p: *mut T, n: usize) -> *mut T { p.add(n) }
    pub unsafe fn slice_from_raw<'a, T>(p: *const T, len: usize) -> &'a [T] { core::slice::from_raw_parts(p, len) }
    pub unsafe fn slice_from_raw_mut<'a, T>(p: *mut T, len: usize) -> &'a mut [T] { core::slice::from_raw_parts_mut(p, len) }
    pub unsafe fn slice_ptr<T>(s: &[T]) -> *const T { s.as_ptr() }
    pub unsafe fn str_from_raw<'a>(p: *const u8, len: usize) -> &'a str { core::str::from_utf8_unchecked(core::slice::from_raw_parts(p, len)) }
    pub unsafe fn copy_nonoverlapping<T>(src: *const T, dst: *mut T, n: usize) { core::ptr::copy_nonoverlapping(src, dst, n) }
    pub unsafe fn copy<T>(src: *const T, dst: *mut T, n: usize) { core::ptr::copy(src, dst, n) }
    pub unsafe fn write_bytes<T>(dst: *mut T, b: u8, n: usize) { core::ptr::write_bytes(dst, b, n) }
    pub unsafe fn forget<T>(v: T) { core::mem::forget(v) }
    pub unsafe fn null_mut<T>() -> *mut T { core::ptr::null_mut() }
    pub unsafe fn zeroed<T>() -> T { core::mem::zeroed() }
    pub unsafe fn transmute<T, U>(v: T) -> U { core::mem::transmute_copy(&core::mem::ManuallyDrop::new(v)) }
    pub unsafe fn discriminant_value<T>(_v: &T) -> i64 { 0 }
    pub unsafe fn black_box<T>(v: T) -> T { core::hint::black_box(v) }
    pub unsafe fn slice_len<T>(s: &[T]) -> usize { s.len() }
    pub unsafe fn str_as_bytes(s: &str) -> &[u8] { s.as_bytes() }
}

pub mod intrinsics_atomic {
    use core::sync::atomic::{AtomicU64, Ordering};
    // Only what the shimmed implementation modules need; extend as areas need more.
    pub unsafe fn atomic_fence(_ord: u8) { core::sync::atomic::fence(Ordering::SeqCst) }
    pub unsafe fn spin_loop_hint() { core::hint::spin_loop() }
    // collections area (Arc): usize counters
    fn ord(o: u8) -> Ordering { match o { 0 => Ordering::Relaxed, 1 => Ordering::Release, 2 => Ordering::Acquire, 3 => Ordering::AcqRel, _ => Ordering::SeqCst } }
    fn load_ord(o: u8) -> Ordering { match o { 1 => Ordering::Relaxed, 3 => Ordering::Acquire, _ => ord(o) } }
    fn fail_ord(o: u8) -> Ordering { match o { 1 => Ordering::Relaxed, 3 => Ordering::Acquire, _ => ord(o) } }
    unsafe fn au<'a>(p: *const usize) -> &'a core::sync::atomic::AtomicUsize { core::sync::atomic::AtomicUsize::from_ptr(p as *mut usize) }
    pub unsafe fn atomic_load(p: *const usize, o: u8) -> usize { au(p).load(load_ord(o)) }
    pub unsafe fn atomic_store(p: *mut usize, v: usize, o: u8) { au(p).store(v, match o { 2 | 3 => Ordering::SeqCst, _ => ord(o) }) }
    pub unsafe fn atomic_fetch_add(p: *mut usize, v: usize, o: u8) -> usize { au(p).fetch_add(v, ord(o)) }
    pub unsafe fn atomic_fetch_sub(p: *mut usize, v: usize, o: u8) -> usize { au(p).fetch_sub(v, ord(o)) }
    pub unsafe fn atomic_cxchg(p: *mut usize, old: usize, new: usize, s: u8, f: u8) -> (usize, bool) {
        match au(p).compare_exchange(old, new, ord(s), fail_ord(f)) { Ok(v) => (v, true), Err(v) => (v, false) }
    }
    pub unsafe fn atomic_cxchg_weak(p: *mut usize, old: usize, new: usize, s: u8, f: u8) -> (usize, bool) {
        match au(p).compare_exchange_weak(old, new, ord(s), fail_ord(f)) { Ok(v) => (v, true), Err(v) => (v, false) }
    }
}

pub mod intrinsics_rt {
    pub unsafe fn abort() -> ! { std::process::abort() }
}

/// Panicking entry points with Rapid core's signatures, backed by real panics.
pub mod panicking {
    #[track_caller]
    pub fn panic_str(msg: &'static str) -> ! {
        panic!("{}", msg)
    }
    #[track_caller]
    pub fn panic_display<T: ?Sized + core::fmt::Display>(x: &T) -> ! {
        panic!("{}", x)
    }
    #[track_caller]
    pub fn panic_fmt(args: core::fmt::Arguments<'_>) -> ! {
        panic!("{}", args)
    }
    #[track_caller]
    pub fn panic_bounds(index: usize, len: usize) -> ! {
        panic!("index out of bounds: the len is {} but the index is {}", len, index)
    }
    #[track_caller]
    pub fn slice_index_order_fail(index: usize, end: usize) -> ! {
        panic!("slice index starts at {} but ends at {}", index, end)
    }
    #[track_caller]
    pub fn slice_start_index_len_fail(index: usize, len: usize) -> ! {
        panic!("range start index {} out of range for slice of length {}", index, len)
    }
    #[track_caller]
    pub fn slice_end_index_len_fail(index: usize, len: usize) -> ! {
        panic!("range end index {} out of range for slice of length {}", index, len)
    }
}

// ---------------------------------------------------------------- area: core misc (std lane)
#[path = "../../core/heap.rs"]
pub mod heap;
#[path = "../../core/slice/mod.rs"]
pub mod slice;
#[path = "../../core/vec/mod.rs"]
pub mod vec;
#[path = "../../core/cell.rs"]
pub mod cell;
#[path = "../../core/num/mod.rs"]
pub mod num;
/// Generated rustc-checkable copy of core/num/int_impls.rs (gen_num.rs).
pub mod num_int_check;
pub mod intrinsics_int {
    pub trait ShimInt: Copy {
        fn pop(self) -> u32;
        fn lz(self) -> u32;
        fn tz(self) -> u32;
        fn bs(self) -> Self;
        fn br(self) -> Self;
    }
    macro_rules! shim_int {
        ($($t:ty),*) => {$(
            impl ShimInt for $t {
                fn pop(self) -> u32 { self.count_ones() }
                fn lz(self) -> u32 { self.leading_zeros() }
                fn tz(self) -> u32 { self.trailing_zeros() }
                fn bs(self) -> $t { self.swap_bytes() }
                fn br(self) -> $t { self.reverse_bits() }
            }
        )*};
    }
    shim_int!(u8, u16, u32, u64, u128, usize);
    pub unsafe fn ctpop<T: ShimInt>(x: T) -> u32 { x.pop() }
    pub unsafe fn ctlz<T: ShimInt>(x: T) -> u32 { x.lz() }
    pub unsafe fn cttz<T: ShimInt>(x: T) -> u32 { x.tz() }
    pub unsafe fn bswap<T: ShimInt>(x: T) -> T { x.bs() }
    pub unsafe fn bitreverse<T: ShimInt>(x: T) -> T { x.br() }
}

// ---------------------------------------------------------------- area: fmt
#[path = "../../core/fmt/mod.rs"]
pub mod fmt;
pub use crate::num::dec2flt;


// ---------------------------------------------------------------- area: collections, hash, rc, arc
#[path = "coll_hash.rs"]
pub mod hash;
#[path = "../../core/collections/mod.rs"]
pub mod collections;
#[path = "../../core/rc.rs"]
pub mod rc;
#[path = "../../core/sync/arc.rs"]
pub mod arc;

// ---------------------------------------------------------------- area: text (char, str, string, unicode)
#[path = "../../core/unicode/mod.rs"]
pub mod unicode;
#[path = "../../core/char/mod.rs"]
pub mod char;
#[path = "../../core/str/mod.rs"]
pub mod str;
#[path = "../../core/string.rs"]
pub mod string;

// ---------------------------------------------------------------- area: std os layer
