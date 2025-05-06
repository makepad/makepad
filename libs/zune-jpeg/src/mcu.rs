/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use alloc::{format, vec};
use core::cmp::min;

use makepad_zune_core::bytestream::ZReaderTrait;
use makepad_zune_core::colorspace::ColorSpace;

use crate::bitstream::BitStream;
use crate::components::SampleRatios;
use crate::decoder::MAX_COMPONENTS;
use crate::errors::DecodeErrors;
use crate::marker::Marker;
use crate::misc::{calculate_padded_width, setup_component_params};
use crate::worker::{
    color_convert_no_sampling, upsample_and_color_convert_h, upsample_and_color_convert_v
};
use crate::JpegDecoder;

/// The size of a DC block for a MCU.

pub const DCT_BLOCK: usize = 64;

impl<T: ZReaderTrait> JpegDecoder<T> {
    /// Check for existence of DC and AC Huffman Tables
    pub(crate) fn check_tables(&self) -> Result<(), DecodeErrors> {
        // check that dc and AC tables exist outside the hot path
        for component in &self.components {
            let _ = &self
                .dc_huffman_tables
                .get(component.dc_huff_table)
                .as_ref()
                .ok_or_else(|| {
                    DecodeErrors::HuffmanDecode(format!(
                        "No Huffman DC table for component {:?} ",
                        component.component_id
                    ))
                })?
                .as_ref()
                .ok_or_else(|| {
                    DecodeErrors::HuffmanDecode(format!(
                        "No DC table for component {:?}",
                        component.component_id
                    ))
                })?;

            let _ = &self
                .ac_huffman_tables
                .get(component.ac_huff_table)
                .as_ref()
                .ok_or_else(|| {
                    DecodeErrors::HuffmanDecode(format!(
                        "No Huffman AC table for component {:?} ",
                        component.component_id
                    ))
                })?
                .as_ref()
                .ok_or_else(|| {
                    DecodeErrors::HuffmanDecode(format!(
                        "No AC table for component {:?}",
                        component.component_id
                    ))
                })?;
        }
        Ok(())
    }

    /// Decode MCUs and carry out post processing.
    ///
    /// This is the main decoder loop for the library, the hot path.
    ///
    /// Because of this, we pull in some very crazy optimization tricks hence readability is a pinch
    /// here.
    #[allow(
        clippy::similar_names,
        clippy::too_many_lines,
        clippy::cast_possible_truncation
    )]
    #[inline(never)]
    pub(crate) fn decode_mcu_ycbcr_baseline(
        &mut self, pixels: &mut [u8]
    ) -> Result<(), DecodeErrors> {
        setup_component_params(self)?;

        // check dc and AC tables
        self.check_tables()?;

        let (mut mcu_width, mut mcu_height);

        if self.is_interleaved {
            // set upsampling functions
            self.set_upsampling()?;

            mcu_width = self.mcu_x;
            mcu_height = self.mcu_y;
        } else {
            // For non-interleaved images( (1*1) subsampling)
            // number of MCU's are the widths (+7 to account for paddings) divided bu 8.
            mcu_width = ((self.info.width + 7) / 8) as usize;
            mcu_height = ((self.info.height + 7) / 8) as usize;
        }
        if self.is_interleaved
            && self.input_colorspace.num_components() > 1
            && self.options.jpeg_get_out_colorspace().num_components() == 1
        {
            // For a specific set of images, e.g interleaved,
            // when converting from YcbCr to grayscale, we need to
            // take into account mcu height since the MCU decoding needs to take
            // it into account for padding purposes and the post processor
            // parses two rows per mcu width.
            //
            //TODO: Check if this test works over time
            mcu_height /= self.h_max;
        }

        if self.input_colorspace.num_components() > self.components.len() {
            let msg = format!(
                " Expected {} number of components but found {}",
                self.input_colorspace.num_components(),
                self.components.len()
            );
            return Err(DecodeErrors::Format(msg));
        }

        if self.input_colorspace == ColorSpace::Luma && self.is_interleaved {
            warn!("Grayscale image with down-sampled component, resetting component details");

            self.reset_params();

            mcu_width = ((self.info.width + 7) / 8) as usize;
            mcu_height = ((self.info.height + 7) / 8) as usize;
        }
        let width = usize::from(self.info.width);

        let padded_width = calculate_padded_width(width, self.sub_sample_ratio);

        let mut stream = BitStream::new();
        let mut tmp = [0_i32; DCT_BLOCK];

        for (pos, comp) in self.components.iter_mut().enumerate() {
            // Allocate only needed components.
            //
            // For special colorspaces i.e YCCK and CMYK, just allocate all of the needed
            // components.
            if min(
                self.options.jpeg_get_out_colorspace().num_components() - 1,
                pos
            ) == pos
                || self.input_colorspace == ColorSpace::YCCK
                || self.input_colorspace == ColorSpace::CMYK
            {
                // allocate enough space to hold a whole MCU width
                // this means we should take into account sampling ratios
                // `*8` is because each MCU spans 8 widths.
                let len = comp.width_stride * comp.vertical_sample * 8;

                comp.needed = true;
                comp.raw_coeff = vec![0; len];
            } else {
                comp.needed = false;
            }
        }

        let mut pixels_written = 0;

        let is_hv = usize::from(self.sub_sample_ratio == SampleRatios::HV);
        let upsampler_scratch_size = is_hv * self.components[0].width_stride;
        let mut upsampler_scratch_space = vec![0; upsampler_scratch_size];

        for i in 0..mcu_height {
            // Report if we have no more bytes
            // This may generate false negatives since we over-read bytes
            // hence that why 37 is chosen(we assume if we over-read more than 37 bytes, we have a problem)
            if stream.overread_by > 37
            // favourite number :)
            {
                if self.options.get_strict_mode() {
                    return Err(DecodeErrors::FormatStatic("Premature end of buffer"));
                };

                error!("Premature end of buffer");
                break;
            }
            // decode a whole MCU width,
            // this takes into account interleaved components.
            self.decode_mcu_width(mcu_width, &mut tmp, &mut stream)?;
            // process that width up until it's impossible
            self.post_process(
                pixels,
                i,
                mcu_height,
                width,
                padded_width,
                &mut pixels_written,
                &mut upsampler_scratch_space
            )?;
        }

        trace!("Finished decoding image");

        Ok(())
    }
    fn decode_mcu_width(
        &mut self, mcu_width: usize, tmp: &mut [i32; 64], stream: &mut BitStream
    ) -> Result<(), DecodeErrors> {
        for j in 0..mcu_width {
            // iterate over components
            for component in &mut self.components {
                let dc_table = self.dc_huffman_tables[component.dc_huff_table % MAX_COMPONENTS]
                    .as_ref()
                    .unwrap();

                let ac_table = self.ac_huffman_tables[component.ac_huff_table % MAX_COMPONENTS]
                    .as_ref()
                    .unwrap();

                let qt_table = &component.quantization_table;
                let channel = &mut component.raw_coeff;

                // If image is interleaved iterate over scan components,
                // otherwise if it-s non-interleaved, these routines iterate in
                // trivial scanline order(Y,Cb,Cr)
                for v_samp in 0..component.vertical_sample {
                    for h_samp in 0..component.horizontal_sample {
                        // Fill the array with zeroes, decode_mcu_block expects
                        // a zero based array.
                        tmp.fill(0);

                        stream.decode_mcu_block(
                            &mut self.stream,
                            dc_table,
                            ac_table,
                            qt_table,
                            tmp,
                            &mut component.dc_pred
                        )?;

                        if component.needed {
                            let idct_position = {
                                // derived from stb and rewritten for my tastes
                                let c2 = v_samp * 8;
                                let c3 = ((j * component.horizontal_sample) + h_samp) * 8;

                                component.width_stride * c2 + c3
                            };

                            let idct_pos = channel.get_mut(idct_position..).unwrap();
                            //  call idct.
                            (self.idct_func)(tmp, idct_pos, component.width_stride);
                        }
                    }
                }
            }
            self.todo = self.todo.saturating_sub(1);
            // After all interleaved components, that's an MCU
            // handle stream markers
            //
            // In some corrupt images, it may occur that header markers occur in the stream.
            // The spec EXPLICITLY FORBIDS this, specifically, in
            // routine F.2.2.5  it says
            // `The only valid marker which may occur within the Huffman coded data is the RSTm marker.`
            //
            // But libjpeg-turbo allows it because of some weird reason. so I'll also
            // allow it because of some weird reason.
            if let Some(m) = stream.marker {
                if m == Marker::EOI {
                    // acknowledge and ignore EOI marker.
                    stream.marker.take();
                    trace!("Found EOI marker");
                } else if let Marker::RST(_) = m {
                    if self.todo == 0 {
                        self.handle_rst(stream)?;
                    }
                } else {
                    if self.options.get_strict_mode() {
                        return Err(DecodeErrors::Format(format!(
                            "Marker {m:?} found where not expected"
                        )));
                    }
                    error!(
                        "Marker `{:?}` Found within Huffman Stream, possibly corrupt jpeg",
                        m
                    );

                    self.parse_marker_inner(m)?;
                }
            }
        }
        Ok(())
    }
    // handle RST markers.
    // No-op if not using restarts
    // this routine is shared with mcu_prog
    #[cold]
    pub(crate) fn handle_rst(&mut self, stream: &mut BitStream) -> Result<(), DecodeErrors> {
        self.todo = self.restart_interval;

        if let Some(marker) = stream.marker {
            // Found a marker
            // Read stream and see what marker is stored there
            match marker {
                Marker::RST(_) => {
                    // reset stream
                    stream.reset();
                    // Initialize dc predictions to zero for all components
                    self.components.iter_mut().for_each(|x| x.dc_pred = 0);
                    // Start iterating again. from position.
                }
                Marker::EOI => {
                    // silent pass
                }
                _ => {
                    return Err(DecodeErrors::MCUError(format!(
                        "Marker {marker:?} found in bitstream, possibly corrupt jpeg"
                    )));
                }
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub(crate) fn post_process(
        &mut self, pixels: &mut [u8], i: usize, mcu_height: usize, width: usize,
        padded_width: usize, pixels_written: &mut usize, upsampler_scratch_space: &mut [i16]
    ) -> Result<(), DecodeErrors> {
        let out_colorspace_components = self.options.jpeg_get_out_colorspace().num_components();

        if self.is_interleaved && self.options.jpeg_get_out_colorspace() != ColorSpace::Luma {
            if self.sub_sample_ratio == SampleRatios::H {
                // H sample has it easy since it doesn't require the rows below or above

                upsample_and_color_convert_h(
                    &mut self.components,
                    self.color_convert_16,
                    self.input_colorspace,
                    self.options.jpeg_get_out_colorspace(),
                    &mut pixels[*pixels_written..],
                    width,
                    padded_width
                )?;

                // increment pointer to number of pixels written
                *pixels_written += width * out_colorspace_components * 8;
            } else {
                // an abomination this one ...
                upsample_and_color_convert_v(
                    &mut self.components,
                    self.color_convert_16,
                    self.input_colorspace,
                    self.options.jpeg_get_out_colorspace(),
                    pixels,
                    width,
                    padded_width,
                    pixels_written,
                    upsampler_scratch_space,
                    i,
                    mcu_height
                )?;
            }
        } else {
            let mut channels_ref: [&[i16]; MAX_COMPONENTS] = [&[]; MAX_COMPONENTS];

            self.components
                .iter()
                .enumerate()
                .for_each(|(pos, x)| channels_ref[pos] = &x.raw_coeff);

            color_convert_no_sampling(
                &channels_ref,
                self.color_convert_16,
                self.input_colorspace,
                self.options.jpeg_get_out_colorspace(),
                &mut pixels[*pixels_written..],
                width,
                padded_width
            )?;

            // increment pointer to number of pixels written
            *pixels_written += width * out_colorspace_components * 8 * self.h_max;
        }

        Ok(())
    }
}
