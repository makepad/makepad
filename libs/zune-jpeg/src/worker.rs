/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use alloc::format;
use core::convert::TryInto;

use makepad_zune_core::colorspace::ColorSpace;

use crate::color_convert::ycbcr_to_grayscale;
use crate::components::Components;
use crate::decoder::{ColorConvert16Ptr, MAX_COMPONENTS};
use crate::errors::DecodeErrors;

/// fast 0..255 * 0..255 => 0..255 rounded multiplication
///
/// Borrowed from stb
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
#[inline]
fn blinn_8x8(in_val: u8, y: u8) -> u8 {
    let t = i32::from(in_val) * i32::from(y) + 128;
    return ((t + (t >> 8)) >> 8) as u8;
}

#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub(crate) fn color_convert_no_sampling(
    unprocessed: &[&[i16]; MAX_COMPONENTS], color_convert_16: ColorConvert16Ptr,
    input_colorspace: ColorSpace, output_colorspace: ColorSpace, output: &mut [u8], width: usize,
    padded_width: usize
) -> Result<(), DecodeErrors> // so many parameters..
{
    // maximum sampling factors are in Y-channel, no need to pass them.

    if input_colorspace.num_components() == 3 && input_colorspace == output_colorspace {
        // sort things like RGB to RGB conversion
        copy_removing_padding(unprocessed, width, padded_width, output);
        return Ok(());
    }
    // color convert
    match (input_colorspace, output_colorspace) {
        (ColorSpace::YCbCr | ColorSpace::Luma, ColorSpace::Luma) => {
            ycbcr_to_grayscale(unprocessed[0], width, padded_width, output);
        }
        (
            ColorSpace::YCbCr,
            ColorSpace::RGB | ColorSpace::RGBA | ColorSpace::BGR | ColorSpace::BGRA
        ) => {
            color_convert_ycbcr(
                unprocessed,
                width,
                padded_width,
                output_colorspace,
                color_convert_16,
                output
            );
        }
        (ColorSpace::YCCK, ColorSpace::RGB) => {
            color_convert_ycck_to_rgb::<3>(
                unprocessed,
                width,
                padded_width,
                output_colorspace,
                color_convert_16,
                output
            );
        }

        (ColorSpace::YCCK, ColorSpace::RGBA) => {
            color_convert_ycck_to_rgb::<4>(
                unprocessed,
                width,
                padded_width,
                output_colorspace,
                color_convert_16,
                output
            );
        }
        (ColorSpace::CMYK, ColorSpace::RGB) => {
            color_convert_cymk_to_rgb::<3>(unprocessed, width, padded_width, output);
        }
        (ColorSpace::CMYK, ColorSpace::RGBA) => {
            color_convert_cymk_to_rgb::<4>(unprocessed, width, padded_width, output);
        }
        // For the other components we do nothing(currently)
        _ => {
            let msg = format!(
                    "Unimplemented colorspace mapping from {input_colorspace:?} to {output_colorspace:?}");

            return Err(DecodeErrors::Format(msg));
        }
    }
    Ok(())
}

/// Copy a block to output removing padding bytes from input
/// if necessary
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn copy_removing_padding(
    mcu_block: &[&[i16]; MAX_COMPONENTS], width: usize, padded_width: usize, output: &mut [u8]
) {
    for (((pix_w, c_w), m_w), y_w) in output
        .chunks_exact_mut(width * 3)
        .zip(mcu_block[0].chunks_exact(padded_width))
        .zip(mcu_block[1].chunks_exact(padded_width))
        .zip(mcu_block[2].chunks_exact(padded_width))
    {
        for (((pix, c), y), m) in pix_w.chunks_exact_mut(3).zip(c_w).zip(m_w).zip(y_w) {
            pix[0] = *c as u8;
            pix[1] = *y as u8;
            pix[2] = *m as u8;
        }
    }
}

/// Convert YCCK image to rgb
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn color_convert_ycck_to_rgb<const NUM_COMPONENTS: usize>(
    mcu_block: &[&[i16]; MAX_COMPONENTS], width: usize, padded_width: usize,
    output_colorspace: ColorSpace, color_convert_16: ColorConvert16Ptr, output: &mut [u8]
) {
    color_convert_ycbcr(
        mcu_block,
        width,
        padded_width,
        output_colorspace,
        color_convert_16,
        output
    );
    for (pix_w, m_w) in output
        .chunks_exact_mut(width * 3)
        .zip(mcu_block[3].chunks_exact(padded_width))
    {
        for (pix, m) in pix_w.chunks_exact_mut(NUM_COMPONENTS).zip(m_w) {
            let m = (*m) as u8;
            pix[0] = blinn_8x8(255 - pix[0], m);
            pix[1] = blinn_8x8(255 - pix[1], m);
            pix[2] = blinn_8x8(255 - pix[2], m);
        }
    }
}

#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn color_convert_cymk_to_rgb<const NUM_COMPONENTS: usize>(
    mcu_block: &[&[i16]; MAX_COMPONENTS], width: usize, padded_width: usize, output: &mut [u8]
) {
    for ((((pix_w, c_w), m_w), y_w), k_w) in output
        .chunks_exact_mut(width * NUM_COMPONENTS)
        .zip(mcu_block[0].chunks_exact(padded_width))
        .zip(mcu_block[1].chunks_exact(padded_width))
        .zip(mcu_block[2].chunks_exact(padded_width))
        .zip(mcu_block[3].chunks_exact(padded_width))
    {
        for ((((pix, c), m), y), k) in pix_w
            .chunks_exact_mut(3)
            .zip(c_w)
            .zip(m_w)
            .zip(y_w)
            .zip(k_w)
        {
            let c = *c as u8;
            let m = *m as u8;
            let y = *y as u8;
            let k = *k as u8;

            pix[0] = blinn_8x8(c, k);
            pix[1] = blinn_8x8(m, k);
            pix[2] = blinn_8x8(y, k);
        }
    }
}

/// Do color-conversion for interleaved MCU
#[allow(
    clippy::similar_names,
    clippy::too_many_arguments,
    clippy::needless_pass_by_value,
    clippy::unwrap_used
)]
fn color_convert_ycbcr(
    mcu_block: &[&[i16]; MAX_COMPONENTS], width: usize, padded_width: usize,
    output_colorspace: ColorSpace, color_convert_16: ColorConvert16Ptr, output: &mut [u8]
) {
    let num_components = output_colorspace.num_components();

    let stride = width * num_components;

    // Allocate temporary buffer for small widths less than  16.
    let mut temp = [0; 64];
    // We need to chunk per width to ensure we can discard extra values at the end of the width.
    // Since the encoder may pad bits to ensure the width is a multiple of 8.
    for (((y_width, cb_width), cr_width), out) in mcu_block[0]
        .chunks_exact(padded_width)
        .zip(mcu_block[1].chunks_exact(padded_width))
        .zip(mcu_block[2].chunks_exact(padded_width))
        .zip(output.chunks_exact_mut(stride))
    {
        if width < 16 {
            // allocate temporary buffers for the values received from idct
            let mut y_out = [0; 16];
            let mut cb_out = [0; 16];
            let mut cr_out = [0; 16];
            // copy those small widths to that buffer
            y_out[0..y_width.len()].copy_from_slice(y_width);
            cb_out[0..cb_width.len()].copy_from_slice(cb_width);
            cr_out[0..cr_width.len()].copy_from_slice(cr_width);
            // we handle widths less than 16 a bit differently, allocating a temporary
            // buffer and writing to that and then flushing to the out buffer
            // because of the optimizations applied below,
            (color_convert_16)(&y_out, &cb_out, &cr_out, &mut temp, &mut 0);
            // copy to stride
            out[0..width * num_components].copy_from_slice(&temp[0..width * num_components]);
            // next
            continue;
        }

        // Chunk in outputs of 16 to pass to color_convert as an array of 16 i16's.
        for (((y, cb), cr), out_c) in y_width
            .chunks_exact(16)
            .zip(cb_width.chunks_exact(16))
            .zip(cr_width.chunks_exact(16))
            .zip(out.chunks_exact_mut(16 * num_components))
        {
            (color_convert_16)(
                y.try_into().unwrap(),
                cb.try_into().unwrap(),
                cr.try_into().unwrap(),
                out_c,
                &mut 0
            );
        }
        //we have more pixels in the end that can't be handled by the main loop.
        //move pointer back a little bit to get last 16 bytes,
        //color convert, and overwrite
        //This means some values will be color converted twice.
        for ((y, cb), cr) in y_width[width - 16..]
            .chunks_exact(16)
            .zip(cb_width[width - 16..].chunks_exact(16))
            .zip(cr_width[width - 16..].chunks_exact(16))
            .take(1)
        {
            (color_convert_16)(
                y.try_into().unwrap(),
                cb.try_into().unwrap(),
                cr.try_into().unwrap(),
                &mut temp,
                &mut 0
            );
        }

        let rem = out[(width - 16) * num_components..]
            .chunks_exact_mut(16 * num_components)
            .next()
            .unwrap();

        rem.copy_from_slice(&temp[0..rem.len()]);
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn upsample_and_color_convert_h(
    component_data: &mut [Components], color_convert_16: ColorConvert16Ptr,
    input_colorspace: ColorSpace, output_colorspace: ColorSpace, output: &mut [u8], width: usize,
    padded_width: usize
) -> Result<(), DecodeErrors> {
    let v_samp = component_data[0].vertical_sample;

    let out_stride = width * output_colorspace.num_components() * v_samp;
    // Width of image which takes into account fill bytes
    let width_stride = component_data[0].width_stride * v_samp;

    let (y, remainder) = component_data.split_at_mut(1);
    for ((pos, out), y_stride) in output
        .chunks_mut(out_stride)
        .enumerate()
        .zip(y[0].raw_coeff.chunks(width_stride))
    {
        for component in remainder.iter_mut() {
            let raw_data = &component.raw_coeff;

            let comp_stride_start = pos * component.width_stride;
            let comp_stride_stop = comp_stride_start + component.width_stride;

            let comp_stride = &raw_data[comp_stride_start..comp_stride_stop];
            let out_stride = &mut component.upsample_dest;

            // upsample using the fn pointer, should only be H, so no need for
            // row up and row down
            (component.up_sampler)(comp_stride, &[], &[], &mut [], out_stride);
        }

        // by here, each component has been up-sampled, so let's color convert a row(s)
        let cb_stride = &remainder[0].upsample_dest;
        let cr_stride = &remainder[1].upsample_dest;

        let iq_stride: &[i16] = if let Some(component) = remainder.get(2) {
            &component.upsample_dest
        } else {
            &[]
        };

        color_convert_no_sampling(
            &[y_stride, cb_stride, cr_stride, iq_stride],
            color_convert_16,
            input_colorspace,
            output_colorspace,
            out,
            width,
            padded_width
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn upsample_and_color_convert_v(
    component_data: &mut [Components], color_convert_16: ColorConvert16Ptr,
    input_colorspace: ColorSpace, output_colorspace: ColorSpace, output: &mut [u8], width: usize,
    padded_width: usize, pixels_written: &mut usize, upsampler_scratch_space: &mut [i16], i: usize,
    mcu_height: usize
) -> Result<(), DecodeErrors> {
    // HV and V sampling are a bust.
    // They suck because we need top row and bottom row.
    // but we haven't decoded the whole image, we only did a single
    // MCU width.
    //
    // So to make it work we upsample and color convert in two steps
    //
    // These are the special conditions
    // 1. First row of image
    //  row_up points to the current row, since there is no row above it
    //
    // 2. Last row of MCU
    //    We can't upsample yet since we don't have row_down , we haven't decoded it
    //    - Save the row above us, which we will use
    //    - Save the current row.
    //
    // 3. Before decoding a new  MCU.
    //   We already had saved the last row, we now currently have the row_down
    //   which is the first row of this MCU, so we can upsample the last row of the previous
    //   MCU.
    //
    // 4. Decoding a new line for MCU
    //      Previous row is provided by component.prev_row, the rest can be accessed
    //      via stride calculations.
    //  5. Decoding normal lines with no MCU boundary handling.
    //     Use `component.raw_coeffs` to get raw coefficient data stride wise

    // Most of the logic flows in the following:
    // 1. Handle the previous MCU
    // 2. Handle current MCU.
    // 3. Prepare for future MCU

    let (y_component, remainder) = component_data.split_at_mut(1);

    let out_stride = width * output_colorspace.num_components() * 2;

    let (max_h_sample, max_v_sample) = (
        y_component[0].horizontal_sample,
        y_component[0].vertical_sample
    );

    let width_stride = y_component[0].width_stride * 2;
    let stop_offset = y_component[0].raw_coeff.len() / width_stride;

    if i > 0 {
        // Handle the last MCU of the previous row
        // This wasn't up-sampled as we didn't have the row_down
        // so we do it now
        for c in remainder.iter_mut() {
            if c.horizontal_sample == max_h_sample
                && c.vertical_sample == max_v_sample
                && c.horizontal_sample == 2
            {
                // This one solves the following case

                //  Component ID:Y    HS:2 VS:2 QT:0
                //  Component ID:Cb   HS:2 VS:2 QT:1
                //  Component ID:Cr   HS:1 VS:1 QT:2
                //
                // Cb component should not be up-sampled, since it contains
                // the same sampling factor as Y,so if it is the case do not
                // try to sample it.
                // Ideally, it should be using self.h_max, and self.v_max, but
                // that's a story for another day.
                continue;
            }

            let stride = c.width_stride;

            let dest = &mut c.upsample_dest;

            // get current row
            let row = &c.current_row[..];
            let row_up = &c.prev_row[..];
            let row_down = &c.raw_coeff[0..stride];

            // upsample
            (c.up_sampler)(row, row_up, row_down, upsampler_scratch_space, dest);
        }
        // by here, each component has been up-sampled, so let's color convert a row(s)
        let cb_stride = &remainder[0].upsample_dest;
        let cr_stride = &remainder[1].upsample_dest;

        let iq_stride: &[i16] = if let Some(component) = remainder.get(2) {
            &component.upsample_dest
        } else {
            &[]
        };
        // color convert row
        color_convert_no_sampling(
            &[&y_component[0].current_row, cb_stride, cr_stride, iq_stride],
            color_convert_16,
            input_colorspace,
            output_colorspace,
            &mut output[*pixels_written..*pixels_written + out_stride],
            width,
            padded_width
        )?;
        *pixels_written += out_stride;
    }

    'top: for ((pos, out), y_stride) in output[*pixels_written..]
        .chunks_mut(out_stride)
        .enumerate()
        .zip(y_component[0].raw_coeff.chunks_exact(width_stride))
    {
        // we have the Y component width stride.
        // this may be higher than the actual width,(2x because vertical sampling)
        //
        // This will not upsample the last row

        // if false, do not upsample.
        // set to false on the last row of an mcu
        let mut upsample = true;

        for c in remainder.iter_mut() {
            if c.horizontal_sample == max_h_sample
                && c.vertical_sample == max_v_sample
                && c.horizontal_sample == 2
            {
                // see comment inside the same block when i > 0
                continue;
            }

            let stride = c.width_stride * c.vertical_sample;
            let mut row_up: &[i16] = &[];
            // row below current sample
            let mut row_down: &[i16] = &[];
            let dest = &mut c.upsample_dest;

            // get current row
            let row = &c.raw_coeff[pos * stride..(pos + 1) * stride];

            if i == 0 && pos == 0 {
                // first IMAGE row, row_up is the same as current row
                // row_down is the row below.
                row_up = &c.raw_coeff[pos * stride..(pos + 1) * stride];
                row_down = &c.raw_coeff[(pos + 1) * stride..(pos + 2) * stride];
            } else if pos == 0 {
                // first row of a new mcu, previous row was copied so use that
                row_up = &c.prev_row;
                row_down = &c.raw_coeff[(pos + 1) * stride..(pos + 2) * stride];
            } else if pos > 0 && pos < stop_offset - 1 {
                // other rows, get row up and row down relative to our current row
                row_up = &c.raw_coeff[(pos - 1) * stride..pos * stride];
                row_down = &c.raw_coeff[(pos + 1) * stride..(pos + 2) * stride];
            } else if i == mcu_height.saturating_sub(1) && pos == stop_offset - 1 {
                // last IMAGE row, adjust pointer to use previous row and current row

                // other rows, get row up and row down relative to our current row
                row_up = &c.raw_coeff[(pos - 1) * stride..pos * stride];
                row_down = &c.raw_coeff[pos * stride..(pos + 1) * stride];
            } else {
                // the only fallthrough to this point is the last MCU in a row
                // we need a row at the next MCU but we haven't decoded that MCU yet
                // so we should save this and when we have the next MCU,
                // do the due diligence

                // store the current row and previous row in a buffer
                let prev_row = &c.raw_coeff[(pos - 1) * stride..pos * stride];

                c.prev_row.copy_from_slice(prev_row);
                c.current_row.copy_from_slice(row);
                upsample = false;
            }

            if upsample {
                // upsample
                (c.up_sampler)(row, row_up, row_down, upsampler_scratch_space, dest);
            }
            if pos == stop_offset - 1 {
                // copy current last MCU row to be used in the next mcu row
                c.prev_row.copy_from_slice(row);
            }
        }
        // if we didn't upsample,means we are in the last row, so then there is no need
        // to color convert
        if !upsample {
            break 'top;
        }
        // by here, each component has been up-sampled, so let's color convert a row(s)
        let cb_stride = &remainder[0].upsample_dest;
        let cr_stride = &remainder[1].upsample_dest;

        let iq_stride: &[i16] = if let Some(component) = remainder.get(2) {
            &component.upsample_dest
        } else {
            &[]
        };
        // color convert row
        color_convert_no_sampling(
            &[y_stride, cb_stride, cr_stride, iq_stride],
            color_convert_16,
            input_colorspace,
            output_colorspace,
            out,
            width,
            padded_width
        )?;
        *pixels_written += out_stride;
    }
    // copy last row of current y to be used in the next invocation
    if i < mcu_height - 1 {
        let last_row = y_component[0]
            .raw_coeff
            .rchunks_exact(width_stride)
            .next()
            .unwrap();
        let start = last_row
            .len()
            .saturating_sub(y_component[0].current_row.len());

        y_component[0]
            .current_row
            .copy_from_slice(&last_row[start..]);
    }
    Ok(())
}
