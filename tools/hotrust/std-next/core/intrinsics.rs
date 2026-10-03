//! Compiler intrinsics. HotRust lowers every function declared in a core module whose name
//! starts with `intrinsics` itself (by name); nothing here is a real symbol.
//! Std code calls these only through the safe wrappers in mem/ptr/sync::atomic/panicking.

/// Float operations that become single instructions.
pub mod intrinsics {
    extern "hotrust-intrinsic" {
        pub fn sqrt_f64(x: f64) -> f64;
        pub fn sqrt_f32(x: f32) -> f32;
        pub fn floor_f64(x: f64) -> f64;
        pub fn floor_f32(x: f32) -> f32;
        pub fn ceil_f64(x: f64) -> f64;
        pub fn ceil_f32(x: f32) -> f32;
        pub fn trunc_f64(x: f64) -> f64;
        pub fn trunc_f32(x: f32) -> f32;
        pub fn rint_f64(x: f64) -> f64;
        pub fn rint_f32(x: f32) -> f32;
        pub fn fabs_f64(x: f64) -> f64;
        pub fn fabs_f32(x: f32) -> f32;
        pub fn fmin_f64(x: f64, y: f64) -> f64;
        pub fn fmax_f64(x: f64, y: f64) -> f64;
        pub fn fmin_f32(x: f32, y: f32) -> f32;
        pub fn fmax_f32(x: f32, y: f32) -> f32;
        pub fn to_bits_f64(x: f64) -> u64;
        pub fn to_bits_f32(x: f32) -> u32;
        pub fn from_bits_f64(x: u64) -> f64;
        pub fn from_bits_f32(x: u32) -> f32;
    }
}

/// Memory, pointers and type information.
pub mod intrinsics_mem {
    extern "hotrust-intrinsic" {
        pub fn size_of<T>() -> usize;
        pub fn align_of<T>() -> usize;
        /// Size/alignment of the value behind a (possibly fat) reference: slices, str, dyn.
        pub fn size_of_val<T: ?Sized>(v: &T) -> usize;
        pub fn align_of_val<T: ?Sized>(v: &T) -> usize;
        pub fn needs_drop<T>() -> bool;
        pub fn drop_in_place<T: ?Sized>(p: *mut T);
        pub fn ptr_read<T>(p: *const T) -> T;
        pub fn ptr_write<T>(p: *mut T, v: T);
        /// `p + n * size_of::<T>()`
        pub fn ptr_add<T>(p: *const T, n: usize) -> *const T;
        pub fn ptr_add_mut<T>(p: *mut T, n: usize) -> *mut T;
        pub fn slice_from_raw<'a, T>(p: *const T, len: usize) -> &'a [T];
        pub fn slice_from_raw_mut<'a, T>(p: *mut T, len: usize) -> &'a mut [T];
        pub fn slice_ptr<T>(s: &[T]) -> *const T;
        pub fn str_from_raw<'a>(p: *const u8, len: usize) -> &'a str;
        /// memcpy of `n` elements
        pub fn copy_nonoverlapping<T>(src: *const T, dst: *mut T, n: usize);
        /// memmove of `n` elements
        pub fn copy<T>(src: *const T, dst: *mut T, n: usize);
        /// memset of `n` elements to byte `b`
        pub fn write_bytes<T>(dst: *mut T, b: u8, n: usize);
        pub fn forget<T>(v: T);
        pub fn null_mut<T>() -> *mut T;
        /// all-zero value of T
        pub fn zeroed<T>() -> T;
        /// reinterpret the bits; HotRust checks size equality and R14 (repr(C)/scalars only)
        pub fn transmute<T, U>(v: T) -> U;
        /// enum discriminant as i64 (0 for non-enums)
        pub fn discriminant_value<T>(v: &T) -> i64;
        /// unique per type instance (after monomorphisation)
        pub fn type_id<T: ?Sized>() -> u64;
        pub fn type_name<T: ?Sized>() -> &'static str;
        /// opaque to the optimiser
        pub fn black_box<T>(v: T) -> T;
        /// fat pointer parts of a `*const [T]` / `&[T]`
        pub fn slice_len<T>(s: &[T]) -> usize;
        pub fn str_as_bytes(s: &str) -> &[u8];
    }
}

/// Atomics. `ord`: 0 Relaxed, 1 Release, 2 Acquire, 3 AcqRel, 4 SeqCst (core::sync::atomic::Ordering
/// as u8). T is an integer (u8..u64/i8..i64/usize/isize/bool as u8) or a raw pointer.
pub mod intrinsics_atomic {
    extern "hotrust-intrinsic" {
        pub fn atomic_load<T>(p: *const T, ord: u8) -> T;
        pub fn atomic_store<T>(p: *mut T, v: T, ord: u8);
        pub fn atomic_swap<T>(p: *mut T, v: T, ord: u8) -> T;
        /// returns (previous value, success)
        pub fn atomic_cxchg<T>(p: *mut T, old: T, new: T, success: u8, failure: u8) -> (T, bool);
        /// may fail spuriously
        pub fn atomic_cxchg_weak<T>(p: *mut T, old: T, new: T, success: u8, failure: u8) -> (T, bool);
        pub fn atomic_fetch_add<T>(p: *mut T, v: T, ord: u8) -> T;
        pub fn atomic_fetch_sub<T>(p: *mut T, v: T, ord: u8) -> T;
        pub fn atomic_fetch_and<T>(p: *mut T, v: T, ord: u8) -> T;
        pub fn atomic_fetch_or<T>(p: *mut T, v: T, ord: u8) -> T;
        pub fn atomic_fetch_xor<T>(p: *mut T, v: T, ord: u8) -> T;
        pub fn atomic_fetch_nand<T>(p: *mut T, v: T, ord: u8) -> T;
        /// signedness from T
        pub fn atomic_fetch_max<T>(p: *mut T, v: T, ord: u8) -> T;
        pub fn atomic_fetch_min<T>(p: *mut T, v: T, ord: u8) -> T;
        pub fn atomic_fence(ord: u8);
        pub fn atomic_compiler_fence(ord: u8);
        /// pause/yield instruction for spin loops
        pub fn spin_loop_hint();
    }
}

/// Runtime hooks.
pub mod intrinsics_rt {
    extern "hotrust-intrinsic" {
        /// Ends the current panic: live mode reports and unwinds to the recovery point,
        /// AOT aborts. `msg` is the formatted message.
        pub fn panic_impl(msg: *const u8, msg_len: usize, file: *const u8, file_len: usize, line: u32, col: u32) -> !;
        /// The location of the caller of the enclosing #[track_caller] fn (or this call).
        pub fn caller_location() -> &'static crate::panic::Location<'static>;
        pub fn abort() -> !;
        pub fn unreachable() -> !;
    }
}

/// Integer bit operations (single instructions: popcnt/lzcnt/tzcnt/bswap/rbit). T is an
/// unsigned integer type; counts are in bits of T.
pub mod intrinsics_int {
    extern "hotrust-intrinsic" {
        pub fn ctpop<T>(x: T) -> u32;
        /// leading zeros; ctlz(0) = bits of T
        pub fn ctlz<T>(x: T) -> u32;
        /// trailing zeros; cttz(0) = bits of T
        pub fn cttz<T>(x: T) -> u32;
        pub fn bswap<T>(x: T) -> T;
        pub fn bitreverse<T>(x: T) -> T;
    }
}
