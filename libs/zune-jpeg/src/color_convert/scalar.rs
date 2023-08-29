/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use core::convert::TryInto;

/// Limit values to 0 and 255
#[inline]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, dead_code)]
fn clamp(a: i16) -> u8 {
    a.clamp(0, 255) as u8
}

/// YCbCr to RGBA color conversion

/// Convert YCbCr to RGB/BGR
///
/// Converts to RGB if const BGRA is false
///
/// Converts to BGR if const BGRA is true
pub fn ycbcr_to_rgba_inner_16_scalar<const BGRA: bool>(
    y: &[i16; 16], cb: &[i16; 16], cr: &[i16; 16], output: &mut [u8], pos: &mut usize
) {
    let (_, output_position) = output.split_at_mut(*pos);

    // Convert into a slice with 64 elements for Rust to see we won't go out of bounds.
    let opt: &mut [u8; 64] = output_position
        .get_mut(0..64)
        .expect("Slice to small cannot write")
        .try_into()
        .unwrap();
    for ((y, (cb, cr)), out) in y
        .iter()
        .zip(cb.iter().zip(cr.iter()))
        .zip(opt.chunks_exact_mut(4))
    {
        let cr = cr - 128;
        let cb = cb - 128;

        let r = y + ((45_i16.wrapping_mul(cr)) >> 5);
        let g = y - ((11_i16.wrapping_mul(cb) + 23_i16.wrapping_mul(cr)) >> 5);
        let b = y + ((113_i16.wrapping_mul(cb)) >> 6);

        if BGRA {
            out[0] = clamp(b);
            out[1] = clamp(g);
            out[2] = clamp(r);
            out[3] = 255;
        } else {
            out[0] = clamp(r);
            out[1] = clamp(g);
            out[2] = clamp(b);
            out[3] = 255;
        }
    }
    *pos += 64;
}

/// Convert YCbCr to RGB/BGR
///
/// Converts to RGB if const BGRA is false
///
/// Converts to BGR if const BGRA is true
pub fn ycbcr_to_rgb_inner_16_scalar<const BGRA: bool>(
    y: &[i16; 16], cb: &[i16; 16], cr: &[i16; 16], output: &mut [u8], pos: &mut usize
) {
    let (_, output_position) = output.split_at_mut(*pos);

    // Convert into a slice with 48 elements
    let opt: &mut [u8; 48] = output_position
        .get_mut(0..48)
        .expect("Slice to small cannot write")
        .try_into()
        .unwrap();

    for ((y, (cb, cr)), out) in y
        .iter()
        .zip(cb.iter().zip(cr.iter()))
        .zip(opt.chunks_exact_mut(3))
    {
        let cr = cr - 128;
        let cb = cb - 128;

        let r = y + ((45_i16.wrapping_mul(cr)) >> 5);
        let g = y - ((11_i16.wrapping_mul(cb) + 23_i16.wrapping_mul(cr)) >> 5);
        let b = y + ((113_i16.wrapping_mul(cb)) >> 6);

        if BGRA {
            out[0] = clamp(b);
            out[1] = clamp(g);
            out[2] = clamp(r);
        } else {
            out[0] = clamp(r);
            out[1] = clamp(g);
            out[2] = clamp(b);
        }
    }

    // Increment pos
    *pos += 48;
}

pub fn ycbcr_to_grayscale(y: &[i16], width: usize, padded_width: usize, output: &mut [u8]) {
    for (y_in, out) in y
        .chunks_exact(padded_width)
        .zip(output.chunks_exact_mut(width))
    {
        for (y, out) in y_in.iter().zip(out.iter_mut()) {
            *out = *y as u8;
        }
    }
}
