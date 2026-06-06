/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use makepad_zune_core::bytestream::{ZByteWriterTrait, ZWriter};
use makepad_zune_core::colorspace::{ColorCharacteristics, ColorSpace};
use makepad_zune_core::options::EncoderOptions;

use crate::constants::{
    QOI_HEADER_SIZE, QOI_MAGIC, QOI_OP_DIFF, QOI_OP_INDEX, QOI_OP_LUMA, QOI_OP_RGB, QOI_OP_RGBA,
    QOI_OP_RUN, QOI_PADDING
};
use crate::QoiEncodeErrors;

const SUPPORTED_COLORSPACES: [ColorSpace; 2] = [ColorSpace::RGB, ColorSpace::RGBA];
/// Quite Ok Image Encoder
///
///
/// # Example
/// - Encode a 100 by 100 RGB image
///
/// ```
/// use makepad_zune_core::bit_depth::BitDepth;
/// use makepad_zune_core::colorspace::ColorSpace;
/// use makepad_zune_core::options::EncoderOptions;
/// use zune_qoi::QoiEncoder;
/// use zune_qoi::QoiEncodeErrors;
///
/// const W:usize=100;
/// const H:usize=100;
///
/// fn main()->Result<(), QoiEncodeErrors>{
///     let pixels = std::array::from_fn::<u8,{W * H * 3},_>(|i| (i%256) as u8);
///     let mut encoder = QoiEncoder::new(&pixels,EncoderOptions::new(W,H,ColorSpace::RGB,BitDepth::Eight));
///     let mut sink = vec![];
///     let pix = encoder.encode(&mut sink)?;
///     // write pixels, or do something
///     Ok(())
///}
/// ```
pub struct QoiEncoder<'a> {
    // raw pixels, in RGB or RBGA
    pixel_data:            &'a [u8],
    options:               EncoderOptions,
}

impl<'a> QoiEncoder<'a> {
    /// Create a new encoder which will encode the pixels
    ///
    /// # Arguments
    /// - data: Pixel data, size must be equal to `width*height*colorspace channels`
    /// - options: Encoder details for data, this contains eidth, height and number of color components
    #[allow(clippy::redundant_field_names)]
    pub const fn new(data: &'a [u8], options: EncoderOptions) -> QoiEncoder<'a> {
        QoiEncoder {
            pixel_data:            data,
            options:               options,
        }
    }
    pub fn set_color_characteristics(&mut self, _characteristics: ColorCharacteristics) {
        //self.color_characteristics = characteristics;
    }

    /// Return the maximum size for which the encoder can safely
    /// encode the image without fearing for an out of space error
    pub fn max_size(&self) -> usize {
        self.options.width()
            * self.options.height()
            * (self.options.colorspace().num_components() + 1)
            + QOI_HEADER_SIZE
            + QOI_PADDING
    }
    fn encode_headers<T: ZByteWriterTrait>(
        &self, writer: &mut ZWriter<T>
    ) -> Result<(), QoiEncodeErrors> {
        let expected_len = self.options.width()
            * self.options.height()
            * self.options.colorspace().num_components();

        if self.pixel_data.len() != expected_len {
            return Err(QoiEncodeErrors::Generic(
                "Expected length doesn't match pixels length"
            ));
        }

        // qoif
        writer.write_all(&QOI_MAGIC.to_be_bytes())?;

        let options = &self.options;
        if (options.width() as u64) > u64::from(u32::MAX) {
            // error out
            return Err(QoiEncodeErrors::TooLargeDimensions(options.width()));
        }
        if (options.height() as u64) > u64::from(u32::MAX) {
            return Err(QoiEncodeErrors::TooLargeDimensions(options.height()));
        }
        // it's safe to convert to u32 here. since we checked
        // the number can be safely encoded.

        // width
        writer.write_u32_be_err(options.width() as u32)?;
        // height
        writer.write_u32_be_err(options.height() as u32)?;
        //channel
        let channel = match self.options.colorspace() {
            ColorSpace::RGB => 3,
            ColorSpace::RGBA => 4,

            _ => {
                return Err(QoiEncodeErrors::UnsupportedColorspace(
                    self.options.colorspace(),
                    &SUPPORTED_COLORSPACES
                ))
            }
        };

        writer.write_u8_err(channel)?;
        // colorspace, gamma
        let xtic = 0;
        writer.write_u8_err(xtic)?;

        Ok(())
    }
    /// Encode into a pre-allocated buffer and error out if
    /// the buffer provided is too small
    ///
    /// # Arguments.
    /// - buf: The buffer to write encoded content to
    ///
    /// # Returns
    /// - Ok(size): Actual bytes used for encoding
    /// - Err: The error encountered during encoding
    pub fn encode<T: ZByteWriterTrait>(&mut self, sink: T) -> Result<usize, QoiEncodeErrors> {
        let mut stream = ZWriter::new(sink);

        self.encode_headers(&mut stream)?;

        let mut index = [[0_u8; 4]; 64];
        // starting pixel
        let mut px = [0, 0, 0, 255];
        let mut px_prev = [0, 0, 0, 255];

        let mut run = 0;

        let channel_count = self.options.colorspace().num_components();

        for pix_chunk in self.pixel_data.chunks_exact(channel_count) {
            px[0..channel_count].copy_from_slice(pix_chunk);

            if px == px_prev {
                run += 1;

                if run == 62 {
                    stream.write_u8_err(QOI_OP_RUN | (run - 1))?;
                    run = 0;
                }
            } else {
                if run > 0 {
                    stream.write_u8_err(QOI_OP_RUN | (run - 1))?;
                    run = 0;
                }

                let index_pos = (usize::from(px[0]) * 3
                    + usize::from(px[1]) * 5
                    + usize::from(px[2]) * 7
                    + usize::from(px[3]) * 11)
                    % 64;

                if index[index_pos] == px {
                    stream.write_u8_err(QOI_OP_INDEX | (index_pos as u8))?;
                } else {
                    index[index_pos] = px;

                    if px[3] == px_prev[3] {
                        let vr = px[0].wrapping_sub(px_prev[0]);
                        let vg = px[1].wrapping_sub(px_prev[1]);
                        let vb = px[2].wrapping_sub(px_prev[2]);

                        let vg_r = vr.wrapping_sub(vg);
                        let vg_b = vb.wrapping_sub(vg);

                        if !(2..=253).contains(&vr)
                            && !(2..=253).contains(&vg)
                            && !(2..=253).contains(&vb)
                        {
                            stream.write_u8(
                                QOI_OP_DIFF
                                    | vr.wrapping_add(2) << 4
                                    | vg.wrapping_add(2) << 2
                                    | vb.wrapping_add(2)
                            );
                        } else if !(8..=247).contains(&vg_r)
                            && !(32..=223).contains(&vg)
                            && !(8..=247).contains(&vg_b)
                        {
                            stream.write_u8_err(QOI_OP_LUMA | vg.wrapping_add(32))?;
                            stream
                                .write_u8_err(vg_r.wrapping_add(8) << 4 | vg_b.wrapping_add(8))?;
                        } else {
                            stream.write_u8_err(QOI_OP_RGB)?;
                            stream.write_const_bytes(&[px[0], px[1], px[2]])?;
                        }
                    } else {
                        stream.write_u8_err(QOI_OP_RGBA)?;
                        stream.write_u32_be_err(u32::from_be_bytes(px))?;
                    }
                }
            }

            px_prev.copy_from_slice(&px);
        }
        if run > 0 {
            stream.write_u8_err(QOI_OP_RUN | (run - 1))?;
        }
        // write trailing bytes
        stream.write_u64_be_err(0x01)?;
        // done
        let len = stream.bytes_written();

        Ok(len)
    }
}

#[cfg(test)]
mod tests {
    use makepad_zune_core::bytestream::ZCursor;
    use makepad_zune_core::colorspace::ColorSpace;
    use makepad_zune_core::options::EncoderOptions;

    use crate::QoiEncoder;

    #[test]
    fn test_qoi_encode_rgb() {
        use makepad_zune_core::bit_depth::BitDepth;
        const W: usize = 100;
        const H: usize = 100;

        let pixels = std::array::from_fn::<u8, { W * H * 3 }, _>(|i| (i % 256) as u8);
        let mut encoder = QoiEncoder::new(
            &pixels,
            EncoderOptions::new(W, H, ColorSpace::RGB, BitDepth::Eight)
        );
        let mut output = vec![];
        encoder.encode(&mut output).unwrap();
        // write pixels, do something
    }

    #[test]
    fn test_qoi_encode_rgba() {
        use makepad_zune_core::bit_depth::BitDepth;
        const W: usize = 100;
        const H: usize = 100;

        let pixels = std::array::from_fn::<u8, { W * H * 4 }, _>(|i| (i % 256) as u8);
        let mut encoder = QoiEncoder::new(
            &pixels,
            EncoderOptions::new(W, H, ColorSpace::RGBA, BitDepth::Eight)
        );

        let mut output = vec![];
        encoder.encode(&mut output).unwrap();
        // write pixels, do something
        let mut decoder = crate::QoiDecoder::new(ZCursor::new(&output));
        let decoded_pixels = decoder.decode().unwrap();
        assert_eq!(&pixels[..], &decoded_pixels[..]);
    }
}
