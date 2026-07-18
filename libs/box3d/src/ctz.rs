// Port of box3d/src/ctz.h
// Bit-scan helpers. Rust integer intrinsics are well-defined for zero inputs
// (leading_zeros(0) == 32), matching the MSVC fallback path in the C source.

use crate::b3_assert;

// https://en.wikipedia.org/wiki/Find_first_set

#[inline(always)]
pub fn ctz32(block: u32) -> u32 {
    block.trailing_zeros()
}

#[inline(always)]
pub fn clz32(value: u32) -> u32 {
    value.leading_zeros()
}

#[inline(always)]
pub fn ctz64(block: u64) -> u32 {
    block.trailing_zeros()
}

#[inline(always)]
pub fn pop_count64(block: u64) -> i32 {
    block.count_ones() as i32
}

#[inline(always)]
pub fn is_power_of_2(x: i32) -> bool {
    (x & (x - 1)) == 0
}

#[inline]
pub fn bounding_power_of_2(x: i32) -> i32 {
    if x <= 1 {
        return 1;
    }

    32 - clz32((x as u32) - 1) as i32
}

#[inline]
pub fn round_up_power_of_2(x: i32) -> i32 {
    if x <= 1 {
        return 1;
    }

    1 << (32 - clz32((x as u32) - 1))
}

#[inline]
pub fn lower_power_of_2_exponent(x: i32) -> i32 {
    b3_assert!(x > 0);
    let clz = clz32(x as u32) as i32;

    // Position of most significant bit = floor(log2(M))
    31 - clz
}
