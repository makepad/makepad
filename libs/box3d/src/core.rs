// Port of box3d/src/core.h + core.c
// Allocators, dump files, and the assert/log handler plumbing are not needed in
// Rust; what remains is the length-unit global, hashing, and small helpers.

use std::sync::atomic::{AtomicU32, Ordering};

/// This is used to indicate null for interfaces that work with indices instead of pointers.
pub const NULL_INDEX: i32 = -1;

// Use to validate definitions. Do not take my cookie.
pub const SECRET_COOKIE: i32 = 1152023;

/// B3_ASSERT: internal engine assertion. Active in debug/test builds.
#[macro_export]
macro_rules! b3_assert {
    ($($t:tt)*) => { debug_assert!($($t)*) };
}

/// B3_VALIDATE: floating point tolerance checks, debug builds only.
#[macro_export]
macro_rules! b3_validate {
    ($($t:tt)*) => { debug_assert!($($t)*) };
}

/// Test macro: port of ENSURE from box3d/test/test_macros.h.
#[macro_export]
macro_rules! ensure {
    ($c:expr) => {
        assert!($c, "condition false: {}", stringify!($c))
    };
}

/// Test macro: port of ENSURE_SMALL from box3d/test/test_macros.h.
/// Fails if c < -tol or tol < c.
#[macro_export]
macro_rules! ensure_small {
    ($c:expr, $tol:expr) => {{
        let c = $c;
        let tol = $tol;
        assert!(
            !(c < -tol || tol < c),
            "condition false: abs({}) < {}",
            stringify!($c),
            tol
        );
    }};
}

// This allows the user to change the length units at runtime
static LENGTH_UNITS_PER_METER: AtomicU32 = AtomicU32::new(0x3F80_0000); // 1.0f32

pub fn set_length_units_per_meter(length_units: f32) {
    b3_assert!(length_units.is_finite() && length_units > 0.0);
    LENGTH_UNITS_PER_METER.store(length_units.to_bits(), Ordering::Relaxed);
}

pub fn get_length_units_per_meter() -> f32 {
    f32::from_bits(LENGTH_UNITS_PER_METER.load(Ordering::Relaxed))
}

/// Version numbering scheme. See https://semver.org/
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: i32,
    pub minor: i32,
    pub revision: i32,
}

pub fn get_version() -> Version {
    Version { major: 0, minor: 1, revision: 0 }
}

pub fn is_double_precision() -> bool {
    cfg!(feature = "double-precision")
}

/// b3GetByteCount: the Rust port does not track allocations; kept for API parity.
pub fn get_byte_count() -> i32 {
    0
}

pub fn log(message: &str) {
    println!("Box3D: {}", message);
}

// Geometry content hashes reserve zero to mean unhashed
#[inline]
pub fn non_zero_hash(hash: u32) -> u32 {
    if hash != 0 {
        hash
    } else {
        1
    }
}

// Simple djb2 hash function for determinism testing.
// Folded 8 bytes per iteration to shorten the dependency chain, matching the C
// implementation in timer.c (little-endian word interpretation on all platforms).
pub const HASH_INIT: u32 = 5381;

pub fn hash(hash: u32, data: &[u8]) -> u32 {
    let mut result = hash;
    let count = data.len();
    let mut i = 0usize;

    while i + 8 <= count {
        let word = u64::from_le_bytes(data[i..i + 8].try_into().unwrap());
        result = (result << 5).wrapping_add(result).wrapping_add(word as u32);
        result = (result << 5).wrapping_add(result).wrapping_add((word >> 32) as u32);
        i += 8;
    }

    while i < count {
        result = (result << 5).wrapping_add(result).wrapping_add(data[i] as u32);
        i += 1;
    }

    result
}

/// Get two distinct mutable references into one slice. The C code freely takes
/// two pointers into the same array (body A and body B); this is the safe
/// equivalent. Panics if i == j or out of bounds.
#[inline]
pub fn get_two_mut<T>(v: &mut [T], i: i32, j: i32) -> (&mut T, &mut T) {
    let (i, j) = (i as usize, j as usize);
    assert!(i != j, "get_two_mut requires distinct indices");
    if i < j {
        let (lo, hi) = v.split_at_mut(j);
        (&mut lo[i], &mut hi[0])
    } else {
        let (lo, hi) = v.split_at_mut(i);
        (&mut hi[0], &mut lo[j])
    }
}
