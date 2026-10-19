//! Compiler intrinsics. Relax lowers every function declared in a core module whose name
//! starts with `intrinsics` itself (by name); nothing here is a real symbol.
//! Std code calls these only through the safe wrappers in mem/ptr/sync::atomic/panicking.

/// Float operations that become single instructions.
pub mod intrinsics {
    extern "relax-intrinsic" {
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
        /// a * b + c with one rounding (hardware fused multiply-add); only valid to call when
        /// `fma_native()` is true
        pub fn fma_f64(a: f64, b: f64, c: f64) -> f64;
        pub fn fma_f32(a: f32, b: f32, c: f32) -> f32;
        /// true when this target (arm64; x86-64 with FMA3) fuses in hardware: a constant per run
        pub fn fma_native() -> bool;
    }
}

/// Memory, pointers and type information.
pub mod intrinsics_mem {
    extern "relax-intrinsic" {
        pub fn size_of<T>() -> usize;
        pub fn align_of<T>() -> usize;
        /// Size/alignment of the value behind a (possibly fat) reference: slices, str, dyn.
        pub fn size_of_val<T: ?Sized>(v: &T) -> usize;
        pub fn align_of_val<T: ?Sized>(v: &T) -> usize;
        pub fn needs_drop<T>() -> bool;
        /// `T: Copy` for this instantiation (a constant per instance): std's Copy fast paths
        /// (real std specializes Clone-for-Copy into memcpy)
        pub fn is_copy<T>() -> bool;
        /// `T: Freeze` for this instantiation: no UnsafeCell outside indirection (rustc's auto
        /// trait)
        pub fn is_freeze<T>() -> bool;
        pub fn drop_in_place<T: crate::marker::PointeeSized>(p: *mut T);
        pub fn ptr_read<T>(p: *const T) -> T;
        pub fn ptr_write<T>(p: *mut T, v: T);
        /// volatile access (never removed, merged, forwarded or reordered with other volatile ops)
        pub fn volatile_load<T>(p: *const T) -> T;
        pub fn volatile_store<T>(p: *mut T, v: T);
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
        /// reinterpret the bits; Relax checks size equality and R14 (repr(C)/scalars only)
        pub fn transmute<T, U>(v: T) -> U;
        /// enum discriminant as i64 (0 for non-enums)
        pub fn discriminant_value<T>(v: &T) -> i64;
        /// stable 128-bit hash of the type (after monomorphisation): low and high halves
        pub fn type_id<T: ?Sized>() -> u64;
        pub fn type_id_hi<T: ?Sized>() -> u64;
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
    extern "relax-intrinsic" {
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
    extern "relax-intrinsic" {
        /// Ends the current panic: live mode reports and unwinds to the recovery point,
        /// AOT aborts. `msg` is the formatted message.
        pub fn panic_impl(msg: *const u8, msg_len: usize, file: *const u8, file_len: usize, line: u32, col: u32) -> !;
        /// The location of the caller of the enclosing #[track_caller] fn (or this call).
        pub fn caller_location() -> &'static crate::panic::Location<'static>;
        pub fn abort() -> !;
        /// Runs `f(data)` under a recovery point; true if it panicked or faulted (live: the
        /// panic was reported and the stack reset here; AOT: panics abort). std::thread uses it
        /// so a panicking thread ends and join() returns Err (Rt::thread_run).
        pub fn catch_panic(f: fn(*mut u8), data: *mut u8) -> bool;
        /// (argc, argv) as the process received them (Rt::args; `makepad run` passes the program's
        /// own arguments).
        pub fn args() -> (usize, *const *const crate::ffi::c_char);
        pub fn unreachable() -> !;
        /// Whether this thread is unwinding a panic (from the panic to the recovery point that
        /// catches it): std::thread::panicking.
        pub fn panicking() -> bool;
        /// The next panic on this thread continues one already reported (resume_unwind): no new
        /// inspect park (Rt::rethrow).
        pub fn rethrow();
        /// This thread's floating-point control register (x86-64: MXCSR, zero-extended;
        /// AArch64: FPCR): one instruction, no call.
        pub fn fp_env_get() -> u64;
        pub fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32);
        pub fn xgetbv(index: u32) -> u64;
        /// Writes this thread's floating-point control register (flush-to-zero,
        /// denormals-are-zero, rounding, ...).
        pub fn fp_env_set(x: u64);
    }
}

/// Integer bit operations (single instructions: popcnt/lzcnt/tzcnt/bswap/rbit). T is an
/// unsigned integer type; counts are in bits of T.
pub mod intrinsics_int {
    extern "relax-intrinsic" {
        pub fn ctpop<T>(x: T) -> u32;
        /// leading zeros; ctlz(0) = bits of T
        pub fn ctlz<T>(x: T) -> u32;
        /// trailing zeros; cttz(0) = bits of T
        pub fn cttz<T>(x: T) -> u32;
        pub fn bswap<T>(x: T) -> T;
        pub fn bitreverse<T>(x: T) -> T;
        /// high 64 bits of the 128-bit product (signed for signed T); T is 64 bits wide
        pub fn mul_hi<T>(a: T, b: T) -> T;
    }
}

/// Internal SIMD bridge. T/U are padding-free vector newtypes, exactly 16 bytes.
/// vx_ternary's S is a vector except for SHA1C/P/M, where it is a u32.
/// Selectors are compile-time constants, checked by lowering. Values use the normal
/// aggregate ABI; the bridge uses V128 loads/stores so slot forwarding can remove copies.
pub mod intrinsics_vx {
    // Binary selectors (shared with lower.rs::vx_intrinsic):
    // 0 add, 1 sub, 2/3 signed/unsigned saturating add, 4/5 saturating sub,
    // 6 and, 7 or, 8 xor, 9 a & !b, 10 equal, 11 signed greater,
    // 12/13 signed/unsigned min, 14/15 max, 16 unsigned rounded average,
    // 17/18 zip low/high, 19 pack i16->u8, 20 pack i16->i8,
    // 21 pack i32->i16, 22 pack i32->u16, 23 madd i16, 24 sad u8,
    // 25 pshufb, 26 multiply low, 27 madd u8*i8, 29 unsigned greater.
    // 40..45 f32 add/sub/mul/div/Intel min/max; 46..51 eq/ne/lt/le/gt/ge.
    // 52/53 f32 NEON min/max (NaN propagation and signed-zero ordering).
    // 60..63 NEON f32 add/sub/mul/div.
    // SHA binary (width 32): 70 x86 sha1nexte, 71 sha1msg1, 72 sha1msg2,
    // 73 sha256msg1, 74 sha256msg2; 80 Arm sha1su1, 81 sha256su0.
    // Ternary: 0 x86 sha256rnds2; 1/2/3 Arm sha1c/p/m (b is u32),
    // 4 Arm sha1su0, 5 sha256h, 6 sha256h2, 7 sha256su1 (b is a vector).
    // 8 / 9 NEON f32 fused a + b * c / a - b * c (FMLA / FMLS .4s).
    // vx_sha1rnds4: x86, immediate FUNC in 0..=3, checked at lowering.
    // vx_sha1h: Arm, scalar u32 -> u32 through lane 0 of a V128.
    // Unary: 0 abs, 1 shl, 2 unsigned shr, 3 signed shr, 4 shuffle i32,
    // 5/6 byte shift right/left, 7 sqrt f32, 8 abs f32, 9 NEON sqrt f32. Narrow: 19/20/21 as above,
    // 30 unsigned u16->u8. Narrow results are 8 bytes, all other vectors 16.

    extern "relax-intrinsic" {
        pub fn vx_binary<T, U>(a: T, b: T, op: u32, width: u32) -> U;
        pub fn vx_ternary<T, S>(a: T, b: S, c: T, op: u32) -> T;
        pub fn vx_sha1rnds4<T>(a: T, b: T, func: i32) -> T;
        pub fn vx_sha1h(a: u32) -> u32;
        pub fn vx_unary<T>(a: T, op: u32, width: u32, imm: i32) -> T;
        pub fn vx_splat<T>(a: u64, width: u32) -> T;
        pub fn vx_load<T>(p: *const u8) -> T;
        pub fn vx_store<T>(p: *mut u8, a: T);
        pub fn vx_narrow<T, U>(a: T, op: u32) -> U;
        pub fn vx_table<T, U>(a: T, b: U) -> T;
        pub fn vx_mask<T>(a: T) -> i32;
        pub fn vx_blend<T>(a: T, b: T, mask: T) -> T;
    }
}
