/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */
#![allow(dead_code)]
#[cfg(feature = "portable-simd")]
use crate::filters::portable_simd;
#[allow(clippy::manual_memcpy)]
pub fn handle_avg(
    prev_row: &[u8], raw: &[u8], current: &mut [u8], components: usize, use_sse4: bool
) {
    if raw.len() < components || current.len() < components {
        return;
    }

    #[cfg(feature = "portable-simd")]
    {
        match components {
            3 => return portable_simd::defilter_avg_generic::<3>(prev_row, raw, current),
            4 => return portable_simd::defilter_avg_generic::<4>(prev_row, raw, current),
            6 => return portable_simd::defilter_avg_generic::<6>(prev_row, raw, current),
            8 => return portable_simd::defilter_avg_generic::<8>(prev_row, raw, current),
            _ => ()
        }
    }

    #[cfg(feature = "sse")]
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        // use sse features where applicable
        if use_sse4 {
            match components {
                3 => return crate::filters::sse4::defilter_avg_sse::<3>(prev_row, raw, current),
                4 => return crate::filters::sse4::defilter_avg_sse::<4>(prev_row, raw, current),
                6 => return crate::filters::sse4::defilter_avg_sse::<6>(prev_row, raw, current),
                8 => return crate::filters::sse4::defilter_avg_sse::<8>(prev_row, raw, current),
                _ => ()
            }
        }
    }

    // no simd, so just do it the old fashioned way

    // handle leftmost byte explicitly
    for i in 0..components {
        current[i] = raw[i].wrapping_add(prev_row[i] >> 1);
    }
    // raw length is one row,so always keep it in check
    let end = current.len().min(raw.len()).min(prev_row.len());

    if components > 8 {
        // optimizer hint to tell the compiler that we don't see this ever happening
        return;
    }

    for i in components..end {
        let a = current[i - components];
        let b = prev_row[i];

        // find average, with overflow handling
        // from standford bit-hacks.
        // This lets us keep the implementations using
        // 8 bits, hence easier to vectorize
        let c = (a & b) + ((a ^ b) >> 1);

        current[i] = raw[i].wrapping_add(c);
    }
}

#[allow(clippy::manual_memcpy)]
pub fn handle_sub(raw: &[u8], current: &mut [u8], components: usize, use_sse2: bool) {
    if current.len() < components || raw.len() < components {
        return;
    }
    #[cfg(feature = "portable-simd")]
    {
        match components {
            3 => return portable_simd::defilter_sub_generic::<3>(raw, current),
            4 => return portable_simd::defilter_sub_generic::<4>(raw, current),
            6 => return portable_simd::defilter_sub_generic::<6>(raw, current),
            8 => return portable_simd::defilter_sub_generic::<8>(raw, current),
            _ => ()
        }
    }
    #[cfg(feature = "sse")]
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if use_sse2 {
            match components {
                3 => return crate::filters::sse4::de_filter_sub_sse2::<3>(raw, current),
                4 => return crate::filters::sse4::de_filter_sub_sse2::<4>(raw, current),
                6 => return crate::filters::sse4::de_filter_sub_sse2::<6>(raw, current),
                8 => return crate::filters::sse4::de_filter_sub_sse2::<8>(raw, current),
                _ => ()
            }
        }
    }
    // handle leftmost byte explicitly
    for i in 0..components {
        current[i] = raw[i];
    }
    // raw length is one row,so always keep it in check
    let end = current.len().min(raw.len());

    for i in components..end {
        let a = current[i - components];
        current[i] = raw[i].wrapping_add(a);
    }
}

#[allow(clippy::manual_memcpy)]
pub fn handle_paeth(
    prev_row: &[u8], raw: &[u8], current: &mut [u8], components: usize, use_sse4: bool
) {
    if raw.len() < components || current.len() < components {
        return;
    }

    #[cfg(feature = "portable-simd")]
    {
        match components {
            3 => {
                return crate::filters::portable_simd::defilter_paeth_generic::<3>(
                    prev_row, raw, current
                )
            }
            4 => {
                return crate::filters::portable_simd::defilter_paeth_generic::<4>(
                    prev_row, raw, current
                )
            }
            6 => {
                return crate::filters::portable_simd::defilter_paeth_generic::<6>(
                    prev_row, raw, current
                )
            }
            8 => {
                return crate::filters::portable_simd::defilter_paeth_generic::<8>(
                    prev_row, raw, current
                )
            }
            _ => ()
        }
    }

    #[cfg(feature = "sse")]
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if use_sse4 {
            match components {
                3 => {
                    return crate::filters::sse4::de_filter_paeth_sse41::<3>(prev_row, raw, current)
                }
                4 => {
                    return crate::filters::sse4::de_filter_paeth_sse41::<4>(prev_row, raw, current)
                }
                6 => {
                    return crate::filters::sse4::de_filter_paeth_sse41::<6>(prev_row, raw, current)
                }
                8 => {
                    return crate::filters::sse4::de_filter_paeth_sse41::<8>(prev_row, raw, current)
                }
                _ => ()
            }
        }
    }

    // handle leftmost byte explicitly
    for i in 0..components {
        current[i] = raw[i].wrapping_add(paeth(0, prev_row[i], 0));
    }
    // raw length is one row,so always keep it in check
    let end = current.len().min(raw.len()).min(prev_row.len());

    if components > 8 {
        // optimizer hint to tell the CPU that we don't see this ever happening
        return;
    }

    for i in components..end {
        let paeth_res = paeth(
            current[i - components],
            prev_row[i],
            prev_row[i - components]
        );
        current[i] = raw[i].wrapping_add(paeth_res)
    }
}

pub fn handle_up(prev_row: &[u8], raw: &[u8], current: &mut [u8]) {
    for ((filt, recon), up) in raw.iter().zip(current).zip(prev_row) {
        *recon = (*filt).wrapping_add(*up)
    }
}

/// Handle images with the first scanline as paeth scanline
///
/// Special in that the above row is treated as zero
#[allow(clippy::manual_memcpy)]
pub fn handle_paeth_first(raw: &[u8], current: &mut [u8], components: usize) {
    if raw.len() < components || current.len() < components {
        return;
    }

    // handle leftmost byte explicitly
    for i in 0..components {
        current[i] = raw[i];
    }
    // raw length is one row,so always keep it in check
    let end = current.len().min(raw.len());

    for i in components..end {
        let paeth_res = paeth(current[i - components], 0, 0);
        current[i] = raw[i].wrapping_add(paeth_res)
    }
}

/// Handle images with the fast scanline as an average scanline
///
/// The above row is treated as zero
#[allow(clippy::manual_memcpy)]
pub fn handle_avg_first(raw: &[u8], current: &mut [u8], components: usize) {
    if raw.len() < components || current.len() < components {
        return;
    }

    // handle leftmost byte explicitly
    for i in 0..components {
        current[i] = raw[i];
    }
    // raw length is one row,so always keep it in check
    let end = current.len().min(raw.len());

    for i in components..end {
        let avg = current[i - components] >> 1;
        current[i] = raw[i].wrapping_add(avg)
    }
}

#[inline(always)]
pub fn paeth(a: u8, b: u8, c: u8) -> u8 {
    // FROM STB
    // This formulation looks very different from the reference in the PNG spec, but is
    // actually equivalent and has favorable data dependencies and admits straightforward
    // generation of branch-free code, which helps performance significantly.

    let a = i32::from(a);
    let b = i32::from(b);
    let c = i32::from(c);
    let thresh = c * 3 - (a + b);
    let lo = if a < b { a } else { b };
    let hi = if a < b { b } else { a };

    let t0 = if hi <= thresh { lo } else { c };
    let t1 = if thresh <= lo { hi } else { t0 };
    return t1 as u8;
}
