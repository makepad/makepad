/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

#![cfg(any(target_arch = "x86", target_arch = "x86_64"))]
//! AVX optimized utility functions

/// Convert from big endian to native endian
///
/// # Safety
/// - Responsibility of the caller to ensure the system
///  supports executing ssse3 instructions or higher
#[target_feature(enable = "avx2")]
#[allow(dead_code)]
pub unsafe fn convert_be_to_ne_avx(out: &mut [u8]) {
    #[cfg(target_arch = "x86")]
    use core::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::*;

    // chunk in type of u16
    for chunk in out.chunks_exact_mut(32) {
        let data = _mm256_loadu_si256(chunk.as_ptr().cast());
        let mask = _mm256_set_epi8(
            14, 15, 12, 13, 10, 11, 8, 9, 6, 7, 4, 5, 2, 3, 0, 1, 14, 15, 12, 13, 10, 11, 8, 9, 6,
            7, 4, 5, 2, 3, 0, 1
        );
        let converted = _mm256_shuffle_epi8(data, mask);
        _mm256_storeu_si256(chunk.as_mut_ptr().cast(), converted);
    }
    // deal with remainder
    crate::utils::convert_be_to_ne_scalar(out.chunks_exact_mut(32).into_remainder());
}
