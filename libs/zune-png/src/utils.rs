/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

//! Utilities required by multiple implementations
//! that help to do small things
use zune_core::bit_depth::{BitDepth, ByteEndian};

use crate::decoder::PLTEEntry;
use crate::enums::PngColor;

mod avx;
mod sse;

/// scalar impl of big-endian to native endian
fn convert_be_to_ne_scalar(out: &mut [u8]) {
    out.chunks_exact_mut(2).for_each(|chunk| {
        let value: [u8; 2] = chunk.try_into().unwrap();
        let pix = u16::from_be_bytes(value);
        chunk.copy_from_slice(&pix.to_ne_bytes());
    });
}

/// Convert big endian to little endian for u16 samples
///
/// This is a no-op if the system is already in big-endian
///
/// # Arguments
///
/// * `out`:  The output array for which we will convert in place
/// * `use_sse4`:  Whether to use SSE intrinsics for conversion
///
fn convert_be_to_le_u16(out: &mut [u8], _use_sse4: bool) {
    #[cfg(feature = "std")]
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if _use_sse4 && is_x86_feature_detected!("avx2") {
            unsafe {
                return avx::convert_be_to_ne_avx(out);
            };
        }
        if _use_sse4 && is_x86_feature_detected!("ssse3") {
            unsafe {
                return sse::convert_be_to_ne_sse4(out);
            }
        }
    }
    convert_be_to_ne_scalar(out)
}

/// Convert u16 big endian samples to target endian
///
/// # Arguments
///
/// * `sample`:  The raw samples assumed to be in big endian
/// * `endian`:  The target endianness for which to convert samples
/// * `use_intrinsics`:  Whether to use sse intrinsics to speed up
///
///
/// sample array is modified in place
///
#[inline]
pub fn convert_be_to_target_endian_u16(
    sample: &mut [u8], endian: ByteEndian, use_intrinsics: bool
) {
    // if target is BE no conversion
    if endian == ByteEndian::BE {
        return;
    }
    // if system is BE, no conversion
    // poor man's check
    if u16::from_be_bytes([234, 231]) == u16::from_ne_bytes([234, 231]) {
        return;
    }
    // convert then
    convert_be_to_le_u16(sample, use_intrinsics);
}

/// Return true if the system is little endian
pub const fn is_le() -> bool {
    // see if le and be conversion return the same number
    u16::from_le_bytes([234, 231]) == u16::from_ne_bytes([234, 231])
}

pub(crate) fn expand_palette(
    input: &[u8], out: &mut [u8], palette: &[PLTEEntry; 256], components: usize
) {
    if components == 0 {
        return;
    }

    if components == 3 {
        for (in_px, px) in input.iter().zip(out.chunks_exact_mut(3)) {
            let entry = palette[usize::from(*in_px) % 256];

            px[0] = entry.red;
            px[1] = entry.green;
            px[2] = entry.blue;
        }
    } else if components == 4 {
        for (in_px, px) in input.iter().zip(out.chunks_exact_mut(4)) {
            let entry = palette[usize::from(*in_px) % 256];

            px[0] = entry.red;
            px[1] = entry.green;
            px[2] = entry.blue;
            px[3] = entry.alpha;
        }
    }
}
/// Expand an image filling the tRNS chunks
///
/// # Arguments
///
/// * `out`:  The output we are to expand
/// * `color`: Input color space
/// * `trns_bytes`:  The tRNS bytes present for the images
/// * `depth`:  The depth of the image
///
pub fn expand_trns<const SIXTEEN_BITS: bool>(
    input: &[u8], out: &mut [u8], color: PngColor, trns_bytes: [u16; 4], depth: u8
) {
    const DEPTH_SCALE_TABLE: [u8; 9] = [0, 0xff, 0x55, 0, 0x11, 0, 0, 0, 0x01];

    // for images whose color types are not paletted
    // presence of a tRNS chunk indicates that the image
    // has transparency.
    //
    // When the pixel specified  in the tRNS chunk is encountered in the resulting stream,
    // it is to be treated as fully transparent.
    // We indicate that by replacing the pixel with pixel+alpha and setting alpha to be zero;
    // to indicate fully transparent.
    if SIXTEEN_BITS {
        match color {
            PngColor::Luma => {
                let trns_byte = trns_bytes[0].to_ne_bytes();

                for (in_chunk, chunk) in input.chunks_exact(2).zip(out.chunks_exact_mut(4)) {
                    chunk[..2].copy_from_slice(in_chunk);

                    if trns_byte != &in_chunk[0..2] {
                        chunk[2] = 255;
                        chunk[3] = 255;
                    } else {
                        chunk[2] = 0;
                        chunk[3] = 0;
                    }
                }
            }
            PngColor::RGB => {
                let r = trns_bytes[0].to_ne_bytes();
                let g = trns_bytes[1].to_ne_bytes();
                let b = trns_bytes[2].to_ne_bytes();

                // copy all trns chunks into one big vector
                let mut all: [u8; 6] = [0; 6];

                all[0..2].copy_from_slice(&r);
                all[2..4].copy_from_slice(&g);
                all[4..6].copy_from_slice(&b);

                for (in_chunk, chunk) in input.chunks_exact(6).zip(out.chunks_exact_mut(8)) {
                    chunk[..6].copy_from_slice(in_chunk);

                    // the read does not match the bytes
                    // so set it to opaque
                    if all != &in_chunk[..6] {
                        chunk[6] = 255;
                        chunk[7] = 255;
                    } else {
                        chunk[6] = 0;
                        chunk[7] = 0;
                    }
                }
            }
            _ => unreachable!()
        }
    } else {
        match color {
            PngColor::Luma => {
                let scale = DEPTH_SCALE_TABLE[usize::from(depth)];

                let depth_mask = (1_u16 << depth) - 1;
                // BUG: This overflowing is indicative of a wrong tRNS value
                let trns_byte = (((trns_bytes[0]) & 255 & depth_mask) as u8) * scale;

                for (in_byte, chunk) in input.iter().zip(out.chunks_exact_mut(2)) {
                    chunk[0] = *in_byte;
                    chunk[1] = u8::from(*in_byte != trns_byte) * 255;
                }
            }
            PngColor::RGB => {
                let depth_mask = (1_u16 << depth) - 1;

                let scale = DEPTH_SCALE_TABLE[usize::from(depth)];

                let r = (trns_bytes[0] & 255 & depth_mask) as u8 * scale;
                let g = (trns_bytes[1] & 255 & depth_mask) as u8 * scale;
                let b = (trns_bytes[2] & 255 & depth_mask) as u8 * scale;

                let r_matrix = [r, g, b];

                for (in_chunk, chunk) in input.chunks_exact(3).zip(out.chunks_exact_mut(4)) {
                    let mask = &in_chunk[0..3] != &r_matrix;

                    chunk[0..3].copy_from_slice(in_chunk);
                    chunk[3] = 255 * u8::from(mask);
                }
            }
            _ => unreachable!()
        }
    }
}

/// Expand bits to bytes expand images with less than 8 bpp
pub(crate) fn expand_bits_to_byte(
    width: usize, depth: usize, out_n: usize, plte_present: bool, input: &[u8], out: &mut [u8]
) {
    let scale = if plte_present {
        // When a palette is used we only separate the indexes in this pass,
        // the palette pass will convert indexes to the right colors later.
        1
    } else {
        match depth {
            1 => 0xFF,
            2 => 0x55,
            4 => 0x11,
            _ => return
        }
    };

    let out = &mut out[..width * out_n];

    if depth == 1 {
        let mut in_iter = input.iter();
        let mut out_iter = out.chunks_exact_mut(8);

        // process in batches of 8 to make use of autovectorization,
        // or failing that - instruction-level parallelism.
        //
        // The ordering of the iterators is important:
        // `out_iter` must come before `in_iter` so that `in_iter` is not advanced
        // when `out_iter` is less than 8 bytes long
        (&mut out_iter)
            .zip(&mut in_iter)
            .for_each(|(out_vals, in_val)| {
                // make sure we only perform the bounds check once
                let cur: &mut [u8; 8] = out_vals.try_into().unwrap();
                // perform the actual expansion
                cur[0] = scale * ((in_val >> 7) & 0x01);
                cur[1] = scale * ((in_val >> 6) & 0x01);
                cur[2] = scale * ((in_val >> 5) & 0x01);
                cur[3] = scale * ((in_val >> 4) & 0x01);
                cur[4] = scale * ((in_val >> 3) & 0x01);
                cur[5] = scale * ((in_val >> 2) & 0x01);
                cur[6] = scale * ((in_val >> 1) & 0x01);
                cur[7] = scale * ((in_val) & 0x01);
            });

        // handle the remainder at the end where the output is less than 8 bytes long
        if let Some(in_val) = in_iter.next() {
            let remainder_iter = out_iter.into_remainder().iter_mut();
            remainder_iter.enumerate().for_each(|(pos, out_val)| {
                let shift = (7_usize).wrapping_sub(pos);
                *out_val = scale * ((in_val >> shift) & 0x01);
            });
        }
    } else if depth == 2 {
        let mut in_iter = input.iter();
        let mut out_iter = out.chunks_exact_mut(4);

        // same as above but adjusted to expand into 4 bytes instead of 8
        (&mut out_iter)
            .zip(&mut in_iter)
            .for_each(|(out_vals, in_val)| {
                let cur: &mut [u8; 4] = out_vals.try_into().unwrap();

                cur[0] = scale * ((in_val >> 6) & 0x03);
                cur[1] = scale * ((in_val >> 4) & 0x03);
                cur[2] = scale * ((in_val >> 2) & 0x03);
                cur[3] = scale * ((in_val) & 0x03);
            });

        // handle the remainder at the end where the output is less than 4 bytes long
        if let Some(in_val) = in_iter.next() {
            let remainder_iter = out_iter.into_remainder().iter_mut();
            remainder_iter.enumerate().for_each(|(pos, out_val)| {
                let shift = (6_usize).wrapping_sub(pos * 2);
                *out_val = scale * ((in_val >> shift) & 0x03);
            });
        }
    } else if depth == 4 {
        let mut in_iter = input.iter();
        let mut out_iter = out.chunks_exact_mut(2);

        // same as above but adjusted to expand into 2 bytes instead of 8
        (&mut out_iter)
            .zip(&mut in_iter)
            .for_each(|(out_vals, in_val)| {
                let cur: &mut [u8; 2] = out_vals.try_into().unwrap();

                cur[0] = scale * ((in_val >> 4) & 0x0f);
                cur[1] = scale * ((in_val) & 0x0f);
            });

        // handle the remainder at the end
        if let Some(in_val) = in_iter.next() {
            let remainder_iter = out_iter.into_remainder().iter_mut();
            remainder_iter.enumerate().for_each(|(pos, out_val)| {
                let shift = (4_usize).wrapping_sub(pos * 4);
                *out_val = scale * ((in_val >> shift) & 0x0f);
            });
        }
    }
}

/// Add an alpha channel to an image with no alpha
///
/// This feels the alpha channel with 255 or 65535 depending on image depth
pub(crate) fn add_alpha(input: &[u8], output: &mut [u8], colorspace: PngColor, depth: BitDepth) {
    match (colorspace, depth) {
        (PngColor::Luma, BitDepth::Eight) => {
            for (in_array, out_chunk) in input.iter().zip(output.chunks_exact_mut(2)) {
                out_chunk[0] = *in_array;
                out_chunk[1] = 255;
            }
        }
        (PngColor::Luma, BitDepth::Sixteen) => {
            for (in_array, out_chunk) in input.chunks_exact(2).zip(output.chunks_exact_mut(4)) {
                out_chunk[0..2].copy_from_slice(in_array);
                out_chunk[2] = 255;
                out_chunk[3] = 255;
            }
        }
        (PngColor::RGB, BitDepth::Eight) => {
            for (in_array, out_chunk) in input.chunks_exact(3).zip(output.chunks_exact_mut(4)) {
                out_chunk[0..3].copy_from_slice(in_array);
                out_chunk[3] = 255;
            }
        }
        (PngColor::RGB, BitDepth::Sixteen) => {
            for (in_array, out_chunk) in input.chunks_exact(6).zip(output.chunks_exact_mut(8)) {
                out_chunk[0..6].copy_from_slice(in_array);
                out_chunk[6] = 255;
                out_chunk[7] = 255;
            }
        }
        (a, b) => panic!("Unknown combination of depth {a:?} and color type for expand alpha {b:?}")
    }
}

pub fn convert_u16_to_u8_slice(slice: &mut [u16]) -> &mut [u8] {
    // Converting a u16 slice to a u8 slice is always correct because
    // the alignment of the target is smaller.
    unsafe {
        core::slice::from_raw_parts_mut(
            slice.as_ptr() as *mut u8,
            slice.len().checked_mul(2).unwrap()
        )
    }
}
