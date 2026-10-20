//! Relax's core: Rust's core + alloc surface used by client programs, written in the Relax
//! dialect and compiled only by Relax (it defines inherent impls on primitive types and
//! declares compiler intrinsics). Physically everything lives here, including the alloc-level
//! types (Box, Vec, String, Rc, Arc, collections); the `alloc` and `std` crates re-export it at
//! their real paths (coord.md S1).
//!
//! Files shared with the rustc shim (std/check) gate Relax-only items with
//! `#[cfg(not(relax_check))]`.

// rustc library/{core,alloc}/src/lib.rs: unsized FnOnce receivers need this
// semantic feature even though Relax does not enforce nightly feature gates.
#![feature(unsized_fn_params)]

#[path = "intrinsics.rs"]
mod intrinsic_decls;
pub use intrinsic_decls::{intrinsics, intrinsics_atomic, intrinsics_int, intrinsics_mem, intrinsics_rt, intrinsics_vx};

pub mod libm;

// ---- language support
pub mod any;
pub mod arch;
// rustc's core::simd: portable-simd's core_simd (library/portable-simd/crates/core_simd/src),
// pulled in as rustc's core does, over the generic SIMD intrinsics (intrinsics_simd)
// rustc's core names itself `core` (core_simd's paths say core::)
// Relax: only in a program whose crate roots enable `portable_simd` (the loader's
// relax_portable_simd cfg): no other program can name it, and 6k lines stay out of cold starts
extern crate self as core;
#[cfg(relax_portable_simd)]
#[path = "core_simd/mod.rs"]
#[allow(missing_debug_implementations, dead_code, unsafe_op_in_unsafe_fn)]
mod core_simd;
#[cfg(relax_portable_simd)]
pub mod simd {
    pub use crate::core_simd::simd::*;
}
pub mod borrow;
pub mod cell;
pub mod clone;
#[doc(hidden)]
pub mod clone_uninit;
pub mod cmp;
pub mod convert;
pub mod default;
pub mod error;
pub mod hint;
pub mod marker;
pub mod mem;
pub mod ops;
pub mod option;
pub mod panic;
pub mod panicking;
pub mod pin;
pub mod primitive;
pub mod future;
pub mod task;
pub mod ptr;
mod ptr_methods;
pub mod result;

// ---- data
pub mod array;
pub mod ascii;
pub mod char;
pub mod fmt;
pub mod hash;
pub mod iter;
pub mod num;
pub mod slice;
pub mod str;
mod tuple;
mod fn_ptr;
pub mod unicode;

// ---- allocation (alloc's modules)
pub mod boxed;
pub mod collections;
pub mod heap;
pub mod rc;
pub mod string;
pub mod vec;

/// core::alloc / alloc::alloc
pub mod alloc {
    pub use crate::heap::{alloc, alloc_zeroed, dealloc, handle_alloc_error, realloc, GlobalAlloc, Layout, LayoutErr, LayoutError, System};
}

pub mod sync {
    // ---- std-os: `pub mod atomic;` (core/sync/atomic.rs) goes here
    pub mod atomic;
    mod arc;
    pub use arc::{Arc, Weak};
}

// ---- std-os: `pub mod time;` (core/time.rs) and `pub mod ffi;` (core/ffi/mod.rs) go here
pub mod time;
pub mod ffi;

pub mod f64 {
    pub mod consts {
        pub const PI: f64 = 3.14159265358979323846264338327950288_f64;
        pub const TAU: f64 = 6.28318530717958647692528676655900577_f64;
        pub const FRAC_PI_2: f64 = 1.57079632679489661923132169163975144_f64;
        pub const FRAC_PI_3: f64 = 1.04719755119659774615421446109316763_f64;
        pub const FRAC_PI_4: f64 = 0.785398163397448309615660845819875721_f64;
        pub const FRAC_PI_6: f64 = 0.52359877559829887307710723054658381_f64;
        pub const FRAC_PI_8: f64 = 0.39269908169872415480783042290993786_f64;
        pub const FRAC_1_PI: f64 = 0.318309886183790671537767526745028724_f64;
        pub const FRAC_2_PI: f64 = 0.636619772367581343075535053490057448_f64;
        pub const FRAC_2_SQRT_PI: f64 = 1.12837916709551257389615890312154517_f64;
        pub const SQRT_2: f64 = 1.41421356237309504880168872420969808_f64;
        pub const FRAC_1_SQRT_2: f64 = 0.707106781186547524400844362104849039_f64;
        pub const E: f64 = 2.71828182845904523536028747135266250_f64;
        pub const LOG2_E: f64 = 1.44269504088896340735992468100189214_f64;
        pub const LOG2_10: f64 = 3.32192809488736234787031942948939018_f64;
        pub const LOG10_E: f64 = 0.434294481903251827651128918916605082_f64;
        pub const LOG10_2: f64 = 0.301029995663981195213738894724493027_f64;
        pub const LN_2: f64 = 0.693147180559945309417232121458176568_f64;
        pub const LN_10: f64 = 2.30258509299404568401799145468436421_f64;
    }
    pub const RADIX: u32 = 2;
    pub const MANTISSA_DIGITS: u32 = 53;
    pub const DIGITS: u32 = 15;
    pub const EPSILON: f64 = 2.2204460492503131e-16_f64;
    pub const MIN: f64 = -1.7976931348623157e+308_f64;
    pub const MIN_POSITIVE: f64 = 2.2250738585072014e-308_f64;
    pub const MAX: f64 = 1.7976931348623157e+308_f64;
    pub const MIN_EXP: i32 = -1021;
    pub const MAX_EXP: i32 = 1024;
    pub const MIN_10_EXP: i32 = -307;
    pub const MAX_10_EXP: i32 = 308;
    pub const NAN: f64 = 0.0_f64 / 0.0_f64;
    pub const INFINITY: f64 = 1.0_f64 / 0.0_f64;
    pub const NEG_INFINITY: f64 = -1.0_f64 / 0.0_f64;
}

pub mod f32 {
    pub mod consts {
        pub const PI: f32 = 3.14159265358979323846264338327950288_f32;
        pub const TAU: f32 = 6.28318530717958647692528676655900577_f32;
        pub const FRAC_PI_2: f32 = 1.57079632679489661923132169163975144_f32;
        pub const FRAC_PI_3: f32 = 1.04719755119659774615421446109316763_f32;
        pub const FRAC_PI_4: f32 = 0.785398163397448309615660845819875721_f32;
        pub const FRAC_PI_6: f32 = 0.52359877559829887307710723054658381_f32;
        pub const FRAC_PI_8: f32 = 0.39269908169872415480783042290993786_f32;
        pub const FRAC_1_PI: f32 = 0.318309886183790671537767526745028724_f32;
        pub const FRAC_2_PI: f32 = 0.636619772367581343075535053490057448_f32;
        pub const FRAC_2_SQRT_PI: f32 = 1.12837916709551257389615890312154517_f32;
        pub const SQRT_2: f32 = 1.41421356237309504880168872420969808_f32;
        pub const FRAC_1_SQRT_2: f32 = 0.707106781186547524400844362104849039_f32;
        pub const E: f32 = 2.71828182845904523536028747135266250_f32;
        pub const LOG2_E: f32 = 1.44269504088896340735992468100189214_f32;
        pub const LOG2_10: f32 = 3.32192809488736234787031942948939018_f32;
        pub const LOG10_E: f32 = 0.434294481903251827651128918916605082_f32;
        pub const LOG10_2: f32 = 0.301029995663981195213738894724493027_f32;
        pub const LN_2: f32 = 0.693147180559945309417232121458176568_f32;
        pub const LN_10: f32 = 2.30258509299404568401799145468436421_f32;
    }
    pub const RADIX: u32 = 2;
    pub const MANTISSA_DIGITS: u32 = 24;
    pub const DIGITS: u32 = 6;
    pub const EPSILON: f32 = 1.19209290e-07_f32;
    pub const MIN: f32 = -3.40282347e+38_f32;
    pub const MIN_POSITIVE: f32 = 1.17549435e-38_f32;
    pub const MAX: f32 = 3.40282347e+38_f32;
    pub const MIN_EXP: i32 = -125;
    pub const MAX_EXP: i32 = 128;
    pub const MIN_10_EXP: i32 = -37;
    pub const MAX_10_EXP: i32 = 38;
    pub const NAN: f32 = 0.0_f32 / 0.0_f32;
    pub const INFINITY: f32 = 1.0_f32 / 0.0_f32;
    pub const NEG_INFINITY: f32 = -1.0_f32 / 0.0_f32;
}

// Deprecated per-type modules (core::u32::MAX ...), still used by some vendored code.
pub mod u8 {
    pub const MIN: u8 = 0;
    pub const MAX: u8 = 255;
}
pub mod u16 {
    pub const MIN: u16 = 0;
    pub const MAX: u16 = 65535;
}
pub mod u32 {
    pub const MIN: u32 = 0;
    pub const MAX: u32 = 4294967295;
}
pub mod u64 {
    pub const MIN: u64 = 0;
    pub const MAX: u64 = 18446744073709551615;
}
pub mod u128 {
    pub const MIN: u128 = 0;
    pub const MAX: u128 = 340282366920938463463374607431768211455;
}
pub mod usize {
    pub const MIN: usize = 0;
    pub const MAX: usize = 18446744073709551615;
}
pub mod i8 {
    pub const MIN: i8 = -128;
    pub const MAX: i8 = 127;
}
pub mod i16 {
    pub const MIN: i16 = -32768;
    pub const MAX: i16 = 32767;
}
pub mod i32 {
    pub const MIN: i32 = -2147483648;
    pub const MAX: i32 = 2147483647;
}
pub mod i64 {
    pub const MIN: i64 = -9223372036854775808;
    pub const MAX: i64 = 9223372036854775807;
}
pub mod i128 {
    pub const MIN: i128 = -170141183460469231731687303715884105728;
    pub const MAX: i128 = 170141183460469231731687303715884105727;
}
pub mod isize {
    pub const MIN: isize = -9223372036854775808;
    pub const MAX: isize = 9223372036854775807;
}

/// The preludes (library/core/src/prelude): `rust_2015` is also 2018's (`prelude::v1` is a builtin path), `rust_2021` adds
/// `FromIterator`, `TryFrom` and `TryInto`, `rust_2024` adds `Future` and `IntoFuture`. The
/// resolver injects the module of the crate's edition. Relax puts std's prelude (a superset of
/// core's; harmless for no_std crates since user items shadow it) in every crate.
pub mod prelude {
    pub mod rust_2015 {
        pub use crate::borrow::ToOwned;
        pub use crate::boxed::Box;
        pub use crate::clone::Clone;
        pub use crate::cmp::{Eq, Ord, PartialEq, PartialOrd};
        pub use crate::convert::{AsMut, AsRef, From, Into};
        pub use crate::default::Default;
        pub use crate::iter::{DoubleEndedIterator, ExactSizeIterator, Extend, IntoIterator, Iterator};
        pub use crate::marker::{Copy, Send, Sized, Sync, Unpin};
        pub use crate::mem::{align_of, align_of_val, drop, size_of, size_of_val};
        pub use crate::ops::{AsyncFn, AsyncFnMut, AsyncFnOnce, Drop, Fn, FnMut, FnOnce};
        pub use crate::option::Option::{self, None, Some};
        pub use crate::result::Result::{self, Err, Ok};
        pub use crate::string::{String, ToString};
        pub use crate::vec::Vec;
    }
    /// `core::prelude::v1`: the 2015/2018 set
    pub mod v1 {
        pub use super::rust_2015::*;
    }
    pub mod rust_2018 {
        pub use super::rust_2015::*;
    }
    pub mod rust_2021 {
        pub use super::rust_2015::*;
        pub use crate::convert::{TryFrom, TryInto};
        pub use crate::iter::FromIterator;
    }
    pub mod rust_2024 {
        pub use super::rust_2015::*;
        pub use crate::convert::{TryFrom, TryInto};
        pub use crate::future::{Future, IntoFuture};
        pub use crate::iter::FromIterator;
    }
}

/// `ptr::addr_of!(place)` / `ptr::addr_of_mut!(place)` (core/src/ptr/mod.rs: raw borrows).
#[macro_export]
macro_rules! addr_of {
    ($place:expr) => {
        &raw const $place
    };
}
#[macro_export]
macro_rules! addr_of_mut {
    ($place:expr) => {
        &raw mut $place
    };
}
/// `mem::offset_of!(Type, field.path)` (core/src/mem/mod.rs): the field's byte offset, from a
/// raw borrow of the field of an uninitialized value (no reads). An inline const, as rustc's
/// builtin is a constant: `&[Field { offset: offset_of!(T, f), .. }]` is promoted to 'static.
#[macro_export]
macro_rules! offset_of {
    // the field tokens are transcribed as written (`0.1`, `0. 1.1`): the expression parser breaks up floats
    ($T:ty, $($field:tt)+) => {
        const {
            let u = $crate::mem::MaybeUninit::<$T>::uninit();
            let base = u.as_ptr();
            #[allow(unused_unsafe)]
            let f = unsafe { &raw const (*base).$($field)+ };
            f as *const u8 as usize - base as *const u8 as usize
        }
    };
}
/// `try!(e)` (2015 edition; `?` since 2018)
#[macro_export]
macro_rules! r#try {
    ($e:expr $(,)?) => {
        match $e {
            $crate::result::Result::Ok(v) => v,
            $crate::result::Result::Err(e) => return $crate::result::Result::Err($crate::convert::From::from(e)),
        }
    };
}
