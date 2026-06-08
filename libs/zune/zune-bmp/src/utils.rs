/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

pub(crate) fn expand_bits_to_byte(depth: usize, plte_present: bool, input: &[u8], out: &mut [u8]) {
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
