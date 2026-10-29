#![cfg(target_arch = "x86_64")]

use std::arch::x86_64::*;
use std::mem::transmute;

// rustc library/stdarch/crates/core_arch/src/x86/{avx,avx2}.rs.
// These fixtures also run under rustc --test, using real stdarch as the implementation.
#[target_feature(enable = "avx2")]
unsafe fn check_selectors<const MASK: i32>(a: [i32; 8], b: [i32; 8]) {
    let va = transmute::<[i32; 8], __m256i>(a);
    let vb = transmute::<[i32; 8], __m256i>(b);
    let shuffled: [i32; 8] = transmute(_mm256_shuffle_epi32::<MASK>(va));
    let blended: [i32; 8] = transmute(_mm256_blend_epi32::<MASK>(va, vb));
    for i in 0..8 {
        let half = (i / 4) * 4;
        let selector = ((MASK >> (2 * (i % 4))) & 3) as usize;
        assert_eq!(shuffled[i], a[half + selector]);
        assert_eq!(blended[i], if MASK & (1 << i) == 0 { a[i] } else { b[i] });
    }
}

#[test]
fn avx2_selectors() {
    if !is_x86_feature_detected!("avx2") {
        return;
    }
    let a = [i32::MIN, -1, 0, 1, i32::MAX, -17, 42, 99];
    let b = [11, 12, 13, 14, 15, 16, 17, 18];
    unsafe {
        check_selectors::<0x00>(a, b);
        check_selectors::<0x55>(a, b);
        check_selectors::<0xaa>(a, b);
        check_selectors::<0xff>(a, b);
        check_selectors::<0xe4>(a, b);
        check_selectors::<0x1b>(a, b);
        check_selectors::<0x39>(a, b);
        check_selectors::<0x81>(a, b);
        let va = transmute::<[i32; 8], __m256i>(a);
        let vb = transmute::<[i32; 8], __m256i>(b);
        let legacy: [i32; 8] = transmute(_mm256_shuffle_epi32(va, 0x1b));
        assert_eq!(legacy, [a[3], a[2], a[1], a[0], a[7], a[6], a[5], a[4]]);
        let legacy: [i32; 8] = transmute(_mm256_blend_epi32(va, vb, 0xaa));
        assert_eq!(legacy, [a[0], b[1], a[2], b[3], a[4], b[5], a[6], b[7]]);
        for offset in 0..8i32 {
            // Indices cross the 128-bit boundary and ignore all but their low three bits.
            let idx = [offset, offset + 8, offset - 8, offset + 4, -1, i32::MIN, i32::MAX, 0x1234567a];
            let vi = transmute::<[i32; 8], __m256i>(idx);
            let permuted: [i32; 8] = transmute(_mm256_permutevar8x32_epi32(va, vi));
            for i in 0..8 {
                assert_eq!(permuted[i], a[(idx[i] as u32 & 7) as usize]);
            }
        }
        let set: [i32; 8] = transmute(_mm256_set_epi32(a[7], a[6], a[5], a[4], a[3], a[2], a[1], a[0]));
        assert_eq!(set, a);
    }
    println!("| AVX2 selectors");
}

#[target_feature(enable = "avx2")]
unsafe fn check_arithmetic(a: [u64; 4], b: [u64; 4]) {
    let va = transmute::<[u64; 4], __m256i>(a);
    let vb = transmute::<[u64; 4], __m256i>(b);
    let product: [u64; 4] = transmute(_mm256_mul_epu32(va, vb));
    let sum: [u64; 4] = transmute(_mm256_add_epi64(va, vb));
    let difference: [u64; 4] = transmute(_mm256_sub_epi64(va, vb));
    let and: [u64; 4] = transmute(_mm256_and_si256(va, vb));
    for i in 0..4 {
        assert_eq!(product[i], (a[i] as u32 as u64) * (b[i] as u32 as u64));
        assert_eq!(sum[i], a[i].wrapping_add(b[i]));
        assert_eq!(difference[i], a[i].wrapping_sub(b[i]));
        assert_eq!(and[i], a[i] & b[i]);
    }
    let low: [u64; 4] = transmute(_mm256_unpacklo_epi64(va, vb));
    let high: [u64; 4] = transmute(_mm256_unpackhi_epi64(va, vb));
    assert_eq!(low, [a[0], b[0], a[2], b[2]]);
    assert_eq!(high, [a[1], b[1], a[3], b[3]]);
}

#[test]
fn avx2_arithmetic() {
    if !is_x86_feature_detected!("avx2") {
        return;
    }
    let edges = [0, 1, u64::MAX, 1u64 << 63, 0x80000000, 0xffffffff, 0xffffffff00000000, 0x0123456789abcdef];
    unsafe {
        for i in 0..edges.len() {
            for j in 0..edges.len() {
                check_arithmetic(
                    [edges[i], edges[(i + 1) % 8], edges[(i + 2) % 8], edges[(i + 3) % 8]],
                    [edges[j], edges[(j + 3) % 8], edges[(j + 5) % 8], edges[(j + 7) % 8]],
                );
            }
        }
        let zero: [u64; 4] = transmute(_mm256_setzero_si256());
        assert_eq!(zero, [0; 4]);
        // rand_chacha's refill_wide: vzeroupper after the AVX2 rounds keeps values in variables
        let kept = _mm256_set1_epi64x(7);
        _mm256_zeroupper();
        let kept: [i64; 4] = transmute(kept);
        assert_eq!(kept, [7; 4]);
        let broadcast: [i64; 4] = transmute(_mm256_set1_epi64x(i64::MIN));
        assert_eq!(broadcast, [i64::MIN; 4]);
    }
    println!("| AVX2 arithmetic");
}

#[target_feature(enable = "avx2")]
unsafe fn check_shifts<const COUNT: i32>(a: [u64; 4]) {
    let va = transmute::<[u64; 4], __m256i>(a);
    let left: [u64; 4] = transmute(_mm256_slli_epi64::<COUNT>(va));
    let right: [u64; 4] = transmute(_mm256_srli_epi64::<COUNT>(va));
    for i in 0..4 {
        assert_eq!(left[i], a[i].checked_shl(COUNT as u32).unwrap_or(0));
        assert_eq!(right[i], a[i].checked_shr(COUNT as u32).unwrap_or(0));
    }
}

#[test]
fn avx2_shift_boundaries() {
    if !is_x86_feature_detected!("avx2") {
        return;
    }
    let a = [u64::MAX, 1, 1u64 << 63, 0x0123456789abcdef];
    unsafe {
        check_shifts::<0>(a);
        check_shifts::<1>(a);
        check_shifts::<25>(a);
        check_shifts::<26>(a);
        check_shifts::<32>(a);
        check_shifts::<63>(a);
        check_shifts::<64>(a);
        check_shifts::<65>(a);
        check_shifts::<255>(a);
        let va = transmute::<[u64; 4], __m256i>(a);
        let left: [u64; 4] = transmute(_mm256_slli_epi64(va, 64));
        let right: [u64; 4] = transmute(_mm256_srli_epi64(va, 63));
        assert_eq!(left, [0; 4]);
        assert_eq!(right, [1, 0, 1, 0]);
    }
    println!("| AVX2 shift boundaries");
}
