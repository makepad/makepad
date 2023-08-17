/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

#![cfg(any(target_arch = "x86", target_arch = "x86_64"))]
//! SSE optimized utility functions

/// Convert from big endian to native endian
///
/// # Safety
/// - Responsibility of the caller to ensure the system
///  supports executing ssse3 instructions or higher
#[target_feature(enable = "ssse3")]
#[allow(dead_code)]
pub unsafe fn convert_be_to_ne_sse4(out: &mut [u8]) {
    #[cfg(target_arch = "x86")]
    use core::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::*;

    // chunk in type of u16
    for chunk in out.chunks_exact_mut(16) {
        let data = _mm_loadu_si128(chunk.as_ptr().cast());
        let mask = _mm_set_epi8(14, 15, 12, 13, 10, 11, 8, 9, 6, 7, 4, 5, 2, 3, 0, 1);
        let converted = _mm_shuffle_epi8(data, mask);
        _mm_storeu_si128(chunk.as_mut_ptr().cast(), converted);
    }
    // deal with remainder
    crate::utils::convert_be_to_ne_scalar(out.chunks_exact_mut(16).into_remainder());
}
