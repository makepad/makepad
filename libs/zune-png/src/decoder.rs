/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use alloc::vec::Vec;
use alloc::{format, vec};
use core::cmp::min;

//use log::trace;
use makepad_zune_core::bit_depth::{BitDepth, ByteEndian};
use makepad_zune_core::bytestream::{ZByteReader, ZReaderTrait};
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::DecoderOptions;
use makepad_zune_core::result::DecodingResult;
use makepad_zune_inflate::DeflateOptions;

use crate::apng::{ActlChunk, FrameInfo, SingleFrame};
use crate::constants::PNG_SIGNATURE;
use crate::enums::{FilterMethod, InterlaceMethod, PngChunkType, PngColor};
use crate::error::PngDecodeErrors;
use crate::error::PngDecodeErrors::GenericStatic;
use crate::filters::de_filter::{
    handle_avg, handle_avg_first, handle_paeth, handle_paeth_first, handle_sub, handle_up
};
use crate::options::default_chunk_handler;
use crate::utils::{
    add_alpha, convert_be_to_target_endian_u16, expand_bits_to_byte, expand_palette, expand_trns,
    is_le
};

/// A palette entry.
///
/// The alpha field is used if the image has a tRNS
/// chunk and pLTE chunk.
#[derive(Copy, Clone, Debug)]
pub(crate) struct PLTEEntry {
    pub red:   u8,
    pub green: u8,
    pub blue:  u8,
    pub alpha: u8
}

impl Default for PLTEEntry {
    fn default() -> Self {
        // but a tRNS chunk may contain fewer values than there are palette entries.
        // In this case, the alpha value for all remaining palette entries is assumed to be 255
        PLTEEntry {
            red:   0,
            green: 0,
            blue:  0,
            alpha: 255
        }
    }
}

#[derive(Copy, Clone)]
pub(crate) struct PngChunk {
    pub length:     usize,
    pub chunk_type: PngChunkType,
    pub chunk:      [u8; 4],
    pub crc:        u32
}

/// Time information data
///
/// Extracted from tIME chunk
#[derive(Debug, Default, Copy, Clone)]
pub struct TimeInfo {
    pub year:   u16,
    pub month:  u8,
    pub day:    u8,
    pub hour:   u8,
    pub minute: u8,
    pub second: u8
}

/// iTXt details
///
/// UTF-8 encoded text
///
/// Extracted from iXTt chunk where present
#[derive(Clone)]
pub struct ItxtChunk {
    pub keyword: Vec<u8>,
    pub text:    Vec<u8>
}

/// tEXt chunk details
///
/// Latin-1 character set
///
/// Extracted from tEXt chunk where present
#[derive(Clone)]
pub struct TextChunk {
    pub keyword: Vec<u8>,
    pub text:    Vec<u8>
}

/// zTxt details
///
/// Extracted from zTXt chunk where present
#[derive(Clone)]
pub struct ZtxtChunk {
    pub keyword: Vec<u8>,
    /// Uncompressed text
    pub text:    Vec<u8>
}

/// Represents PNG information that can be extracted
/// from a png file.
#[derive(Default, Clone)]
pub struct PngInfo {
    /// Image width
    pub width:                usize,
    /// Image height
    pub height:               usize,
    /// Image gamma
    pub gamma:                Option<f32>,
    /// Image interlace method
    pub interlace_method:     InterlaceMethod,
    /// Image time info
    pub time_info:            Option<TimeInfo>,
    /// Image exif data
    pub exif:                 Option<Vec<u8>>,
    /// Icc profile
    pub icc_profile:          Option<Vec<u8>>,
    /// UTF-8 encoded text chunk
    pub itxt_chunk:           Vec<ItxtChunk>,
    /// ztxt chunk
    pub ztxt_chunk:           Vec<ZtxtChunk>,
    /// tEXt chunk
    pub text_chunk:           Vec<TextChunk>,
    // no need to expose these ones
    pub(crate) depth:         u8,
    // use bit_depth
    pub(crate) color:         PngColor,
    // use get_colorspace
    pub(crate) component:     u8,
    // use get_colorspace().num_components()
    pub(crate) filter_method: FilterMethod // for internal use,no need to expose
}

/// A PNG decoder instance.
///
/// This is the main decoder for png image decoding.
///
/// Instantiate the decoder with either the [new](PngDecoder::new)
/// or [new_with_options](PngDecoder::new_with_options) and
/// using either  of the [`decode_raw`](PngDecoder::decode_into) or
/// [`decode`](PngDecoder::decode) will return pixels present in that image
///
/// # Note
/// The decoder currently expands images less than 8 bits per pixels to 8 bits per pixel
/// if this is not desired, then I'd suggest another png decoder
///
/// To get extra details such as exif data and ICC profile if present, use [`get_info`](PngDecoder::get_info)
/// and access the relevant fields exposed
pub struct PngDecoder<T>
where
    T: ZReaderTrait
{
    pub(crate) stream:          ZByteReader<T>,
    pub(crate) options:         DecoderOptions,
    pub(crate) png_info:        PngInfo,
    pub(crate) palette:         Vec<PLTEEntry>,
    pub(crate) frames:          Vec<SingleFrame>,
    pub(crate) actl_info:       Option<ActlChunk>,
    pub(crate) previous_stride: Vec<u8>,
    pub(crate) trns_bytes:      [u16; 4],
    pub(crate) seen_hdr:        bool,
    pub(crate) seen_ptle:       bool,
    pub(crate) seen_headers:    bool,
    pub(crate) seen_trns:       bool,
    pub(crate) seen_iend:       bool,
    pub(crate) current_frame:   usize
}

impl<T: ZReaderTrait> PngDecoder<T> {
    /// Create a new PNG decoder
    ///
    /// # Arguments
    ///
    /// * `data`: The raw bytes of a png encoded file
    ///
    /// returns: PngDecoder
    ///
    /// The decoder settings are set to be default which is
    ///  strict mode + intrinsics
    pub fn new(data: T) -> PngDecoder<T> {
        let default_opt = DecoderOptions::default();

        PngDecoder::new_with_options(data, default_opt)
    }
    /// Create a new decoder with the specified options
    ///
    /// # Arguments
    ///
    /// * `data`: Raw encoded jpeg file contents
    /// * `options`:  The custom options for this decoder
    ///
    /// returns: PngDecoder
    ///
    #[allow(unused_mut, clippy::redundant_field_names)]
    pub fn new_with_options(data: T, options: DecoderOptions) -> PngDecoder<T> {
        PngDecoder {
            seen_hdr:        false,
            stream:          ZByteReader::new(data),
            options:         options,
            palette:         Vec::new(),
            png_info:        PngInfo::default(),
            actl_info:       None,
            previous_stride: vec![],
            frames:          vec![],
            seen_ptle:       false,
            seen_trns:       false,
            seen_headers:    false,
            seen_iend:       false,
            trns_bytes:      [0; 4],
            current_frame:   0
        }
    }

    /// Get image dimensions or none if they aren't decoded
    ///
    /// # Returns
    /// - `Some((width,height))`
    /// - `None`: The image headers haven't been decoded
    ///   or there was an error decoding them
    pub const fn get_dimensions(&self) -> Option<(usize, usize)> {
        if !self.seen_hdr {
            return None;
        }

        Some((self.png_info.width, self.png_info.height))
    }
    /// Return the depth of the image
    ///
    /// Bit depths less than 8 will be returned as [`BitDepth::Eight`](zune_core::bit_depth::BitDepth::Eight)
    ///
    /// # Returns
    /// - `Some(depth)`:  The bit depth of the image.
    /// - `None`: The header wasn't decoded hence the depth wasn't discovered.
    pub const fn get_depth(&self) -> Option<BitDepth> {
        if !self.seen_hdr {
            return None;
        }
        match self.png_info.depth {
            1 | 2 | 4 | 8 => Some(BitDepth::Eight),
            16 => Some(BitDepth::Sixteen),
            _ => unreachable!()
        }
    }
    /// Get image colorspace
    ///
    /// If an image is a palette type, the colorspace is
    /// either RGB or RGBA depending on existence a transparency chunk
    ///
    /// If an image has a transparency chunk, the colorspace
    /// will include that
    ///
    /// # Returns
    ///  - `Some(colorspace)`: The colorspace which the decoded bytes will be in
    ///  - `None`: If the image headers haven't been decoded, or there was an error
    ///     during decoding
    pub const fn get_colorspace(&self) -> Option<ColorSpace> {
        if !self.seen_hdr {
            return None;
        }
        if self.options.png_get_add_alpha_channel() {
            return match self.png_info.color {
                PngColor::Luma | PngColor::LumaA => Some(ColorSpace::LumaA),
                PngColor::Palette | PngColor::RGB | PngColor::RGBA => Some(ColorSpace::RGBA),
                PngColor::Unknown => unreachable!()
            };
        }
        if !self.seen_trns {
            match self.png_info.color {
                PngColor::Palette => Some(ColorSpace::RGB),
                PngColor::Luma => Some(ColorSpace::Luma),
                PngColor::LumaA => Some(ColorSpace::LumaA),
                PngColor::RGB => Some(ColorSpace::RGB),
                PngColor::RGBA => Some(ColorSpace::RGBA),
                PngColor::Unknown => unreachable!()
            }
        } else {
            // for tRNS chunks, RGB=>RGBA
            // Luma=>LumaA, but if we are already in RGB and RGBA, just return
            // them
            match self.png_info.color {
                PngColor::Palette | PngColor::RGB => Some(ColorSpace::RGBA),
                PngColor::Luma => Some(ColorSpace::LumaA),
                PngColor::LumaA => Some(ColorSpace::LumaA),
                PngColor::RGBA => Some(ColorSpace::RGBA),
                _ => unreachable!()
            }
        }
    }
    /// Returns true if the image is animated
    ///
    /// # Note
    /// Png has an  unofficial specification that allows it to
    /// support Animated files, or otherwise known as
    /// APNG  (with extension .apng) supported in various capaccities
    /// in software.
    ///
    /// Such animated files can be decoded by this decoder, returning individual frames
    /// There are functions provided that allow you to further process
    /// such chunks to get the animated frames
    pub fn is_animated(&self) -> bool {
        self.actl_info.is_some() && self.frames.len() > 1
    }

    /// Return true if image has more frames available
    pub fn more_frames(&self) -> bool {
        self.frames.len() > self.current_frame
    }

    pub(crate) fn read_chunk_header(&mut self) -> Result<PngChunk, PngDecodeErrors> {
        // Format is length - chunk type - [data] -  crc chunk, load crc chunk now
        let chunk_length = self.stream.get_u32_be_err()? as usize;
        let chunk_type_int = self.stream.get_u32_be_err()?.to_be_bytes();

        let mut crc_bytes = [0; 4];

        let crc_ref = self.stream.peek_at(chunk_length, 4)?;

        crc_bytes.copy_from_slice(crc_ref);

        let crc = u32::from_be_bytes(crc_bytes);

        let chunk_type = match &chunk_type_int {
            b"IHDR" => PngChunkType::IHDR,
            b"tRNS" => PngChunkType::tRNS,
            b"PLTE" => PngChunkType::PLTE,
            b"IDAT" => PngChunkType::IDAT,
            b"IEND" => PngChunkType::IEND,
            b"pHYs" => PngChunkType::pHYs,
            b"tIME" => PngChunkType::tIME,
            b"gAMA" => PngChunkType::gAMA,
            b"acTL" => PngChunkType::acTL,
            b"fcTL" => PngChunkType::fcTL,
            b"iCCP" => PngChunkType::iCCP,
            b"iTXt" => PngChunkType::iTXt,
            b"eXIf" => PngChunkType::eXIf,
            b"zTXt" => PngChunkType::zTXt,
            b"tEXt" => PngChunkType::tEXt,
            b"fdAT" => PngChunkType::fdAT,
            _ => PngChunkType::unkn
        };

        if !self.stream.has(chunk_length + 4 /*crc stream*/) {
            let err = format!(
                "Not enough bytes for chunk {:?}, bytes requested are {}, but bytes present are {}",
                chunk_type,
                chunk_length + 4,
                self.stream.remaining()
            );

            return Err(PngDecodeErrors::Generic(err));
        }
        // Confirm the CRC here.

        if self.options.png_get_confirm_crc() {
            use crate::crc::crc32_slice8;

            // go back and point to chunk type.
            self.stream.rewind(4);
            // read chunk type + chunk data
            let bytes = self.stream.peek_at(0, chunk_length + 4).unwrap();

            // calculate crc
            let calc_crc = !crc32_slice8(bytes, u32::MAX);

            if crc != calc_crc {
                return Err(PngDecodeErrors::BadCrc(crc, calc_crc));
            }
            // go point after the chunk type
            // The other parts expect the bit-reader to point to the
            // start of the chunk data.
            self.stream.skip(4);
        }

        Ok(PngChunk {
            length: chunk_length,
            chunk: chunk_type_int,
            chunk_type,
            crc
        })
    }

    /// Decode headers from the ong stream and store information
    /// in the internal structure
    ///
    /// After calling this, header information can
    /// be accessed by public headers
    pub fn decode_headers(&mut self) -> Result<(), PngDecodeErrors> {
        if self.seen_headers && self.seen_iend {
            return Ok(());
        }
        if !self.seen_hdr {
            // READ PNG signature
            let signature = self.stream.get_u64_be_err()?;

            if signature != PNG_SIGNATURE {
                return Err(PngDecodeErrors::BadSignature);
            }
            // check if first chunk is ihdr here
            if self.stream.peek_at(4, 4)? != b"IHDR" {
                return Err(PngDecodeErrors::GenericStatic(
                    "First chunk not IHDR, Corrupt PNG"
                ));
            }
        }
        loop {
            let header = self.read_chunk_header()?;

            self.parse_header(header)?;

            if header.chunk_type == PngChunkType::IEND {
                break;
            }
            // break here, we already have content for one
            // frame, subsequent calls will fetch the next frames
            if header.chunk_type == PngChunkType::fcTL {
                break;
            }
        }
        self.seen_headers = true;
        Ok(())
    }

    pub(crate) fn parse_header(&mut self, header: PngChunk) -> Result<(), PngDecodeErrors> {
        match header.chunk_type {
            PngChunkType::IHDR => {
                self.parse_ihdr(header)?;
            }
            PngChunkType::PLTE => {
                self.parse_plte(header)?;
            }
            PngChunkType::IDAT => {
                self.parse_idat(header)?;
            }
            PngChunkType::tRNS => {
                self.parse_trns(header)?;
            }
            PngChunkType::gAMA => {
                self.parse_gama(header)?;
            }
            PngChunkType::acTL => {
                self.parse_actl(header)?;
            }
            PngChunkType::tIME => {
                self.parse_time(header)?;
            }
            PngChunkType::eXIf => {
                self.parse_exif(header)?;
            }
            PngChunkType::iCCP => {
                self.parse_iccp(header);
            }
            PngChunkType::iTXt => {
                self.parse_itxt(header);
            }
            PngChunkType::zTXt => {
                self.parse_ztxt(header);
            }
            PngChunkType::tEXt => {
                self.parse_text(header);
            }
            PngChunkType::fcTL => {
                // may read more headers internally
                self.parse_fctl(header)?;
            }
            PngChunkType::IEND => self.seen_iend = true,
            _ => default_chunk_handler(header.length, header.chunk, &mut self.stream, header.crc)?
        }

        if !self.seen_hdr {
            return Err(GenericStatic("IHDR block not encountered,corrupt jpeg"));
        }

        Ok(())
    }
    /// Return the configured image byte endian which the pixels
    /// will be in if the image is in 16 bit
    ///
    /// If the image depth is less than 16 bit, then the endianness has
    /// no effect
    pub const fn byte_endian(&self) -> ByteEndian {
        self.options.get_byte_endian()
    }

    /// Return the number of bytes required to hold a decoded image frame
    /// decoded using the given input transformations
    ///
    /// # Returns
    ///  - `Some(usize)`: Minimum size for a buffer needed to decode the image
    ///  - `None`: Indicates the image headers was not decoded.
    ///
    /// # Panics
    /// In case `width*height*colorspace` calculation may overflow a usize
    pub fn output_buffer_size(&self) -> Option<usize> {
        if !self.seen_hdr {
            return None;
        }

        let info = &self.png_info;
        let bytes = if info.depth == 16 { 2 } else { 1 };

        let out_n = self.get_colorspace().unwrap().num_components();

        let new_len = info
            .width
            .checked_mul(info.height)
            .unwrap()
            .checked_mul(out_n)
            .unwrap()
            .checked_mul(bytes)
            .unwrap();

        Some(new_len)
    }

    /// Get png information which was extracted from the headers
    ///
    ///
    /// # Returns
    /// - `Some(info)` : The information present in the header
    /// - `None` : Indicates headers were not decoded
    pub const fn get_info(&self) -> Option<&PngInfo> {
        if self.seen_headers {
            Some(&self.png_info)
        } else {
            None
        }
    }
    /// Get a mutable reference to the decoder options
    /// for the decoder instance
    ///
    /// Can be used to modify options before actual decoding but after initial
    /// creation
    pub const fn get_options(&self) -> &DecoderOptions {
        &self.options
    }

    /// Overwrite decoder options with the new options
    ///
    /// Can be used to modify decoding after initialization but before
    /// decoding, it does not do anything after decoding an image
    pub fn set_options(&mut self, options: DecoderOptions) {
        self.options = options;
    }

    /// Decode PNG encoded images and write raw pixels into `out`
    ///
    /// # Arguments
    /// - `out`: The slice which we will write our values into.
    ///         If the slice length is smaller than [`output_buffer_size`](Self::output_buffer_size), it's an error
    ///
    /// # Endianness
    ///
    /// - In case the image is a 16 bit PNG, endianness of the samples may be retrieved
    ///   via [`byte_endian`](Self::byte_endian) method, which returns the configured byte
    ///   endian of the samples.
    /// - PNG uses Big Endian while most machines today are Little Endian (x86 and mainstream Arm),
    ///   hence if the configured endianness is little endian the library will implicitly convert
    ///   samples to little endian
    pub fn decode_into(&mut self, out: &mut [u8]) -> Result<(), PngDecodeErrors> {
        // decode headers
        if !self.seen_headers || !self.seen_iend {
            self.decode_headers()?;
        }

        trace!("Input Colorspace: {:?} ", self.png_info.color);
        trace!("Output Colorspace: {:?} ", self.get_colorspace().unwrap());

        if self.frames.get(self.current_frame).is_none() {
            return Err(PngDecodeErrors::GenericStatic("No more frames"));
        }
        if self.frames[self.current_frame].fctl_info.is_none() {
            return Err(PngDecodeErrors::GenericStatic("Unimplemented frame info"));
        }
        let info = self.frames[self.current_frame].fctl_info.unwrap();

        let png_info = self.png_info.clone();

        let image_len = self.output_buffer_size().unwrap();

        if out.len() < image_len {
            return Err(PngDecodeErrors::TooSmallOutput(image_len, out.len()));
        }

        let out = &mut out[..image_len];

        // go parse IDAT chunks returning the inflate
        let deflate_data = self.inflate()?;

        // then release it, we no longer need it
        self.frames[self.current_frame].fdat = vec![];
        // remove idat chunks from memory
        // we are already done with them.

        if png_info.interlace_method == InterlaceMethod::Standard {
            // allocate out to be enough to hold raw decoded bytes

            self.create_png_image_raw(&deflate_data, info.width, info.height, out, &png_info)?;
        } else if png_info.interlace_method == InterlaceMethod::Adam7 {
            self.decode_interlaced(&deflate_data, out, &png_info, &info)?;
        }

        // convert to set endian if need be
        if self.get_depth().unwrap() == BitDepth::Sixteen {
            convert_be_to_target_endian_u16(out, self.byte_endian(), self.options.use_sse41());
        }
        // one more frame decoded
        self.current_frame += 1;
        Ok(())
    }

    /// Decode data returning it into `Vec<u8>`, endianness of
    /// returned bytes in case of image being 16 bits is given
    /// [`byte_endian()`](Self::byte_endian) method
    ///
    ///
    /// returns: `Result<Vec<u8, Global>, PngErrors>`
    ///
    pub fn decode_raw(&mut self) -> Result<Vec<u8>, PngDecodeErrors> {
        if !self.seen_headers {
            self.decode_headers()?;
        }

        // allocate
        let new_len = self.output_buffer_size().unwrap();
        let mut out: Vec<u8> = vec![0; new_len];
        //decode
        self.decode_into(&mut out)?;

        Ok(out)
    }

    fn decode_interlaced(
        &mut self, deflate_data: &[u8], out: &mut [u8], info: &PngInfo, frame_info: &FrameInfo
    ) -> Result<(), PngDecodeErrors> {
        const XORIG: [usize; 7] = [0, 4, 0, 2, 0, 1, 0];
        const YORIG: [usize; 7] = [0, 0, 4, 0, 2, 0, 1];

        const XSPC: [usize; 7] = [8, 8, 4, 4, 2, 2, 1];
        const YSPC: [usize; 7] = [8, 8, 8, 4, 4, 2, 2];

        let bytes = if info.depth == 16 { 2 } else { 1 };

        let out_n = self.get_colorspace().unwrap().num_components();

        let new_len = frame_info.width * frame_info.height * out_n * bytes;

        // A mad idea would be to make this multithreaded :)
        // They called me a mad man - Thanos
        let out_bytes = out_n * bytes;

        // temporary space for  holding interlaced images
        let mut final_out = vec![0_u8; new_len];

        let mut image_offset = 0;

        // get the maximum height and width for the whole interlace part
        for p in 0..7 {
            let x = (frame_info
                .width
                .saturating_sub(XORIG[p])
                .saturating_add(XSPC[p])
                .saturating_sub(1))
                / XSPC[p];

            let y = (frame_info
                .height
                .saturating_sub(YORIG[p])
                .saturating_add(YSPC[p])
                .saturating_sub(1))
                / YSPC[p];

            if x != 0 && y != 0 {
                let mut image_len = usize::from(info.color.num_components()) * x;

                image_len *= usize::from(info.depth);
                image_len += 7;
                image_len /= 8;
                image_len += 1; // filter byte
                image_len *= y;

                if image_offset + image_len > deflate_data.len() {
                    return Err(PngDecodeErrors::GenericStatic("Too short data"));
                }

                let deflate_slice = &deflate_data[image_offset..image_offset + image_len];

                self.create_png_image_raw(deflate_slice, x, y, &mut final_out, info)?;

                for j in 0..y {
                    for i in 0..x {
                        let out_y = j * YSPC[p] + YORIG[p];
                        let out_x = i * XSPC[p] + XORIG[p];

                        let final_start = out_y * info.width * out_bytes + out_x * out_bytes;
                        let out_start = (j * x + i) * out_bytes;

                        out[final_start..final_start + out_bytes]
                            .copy_from_slice(&final_out[out_start..out_start + out_bytes]);
                    }
                }
                image_offset += image_len;
            }
        }
        Ok(())
    }

    /// Decode PNG encoded images and return the vector of raw pixels but for 16-bit images
    /// represent them in a `Vec<u16>`
    ///
    ///
    /// This returns an enum type [`DecodingResult`](zune_core::result::DecodingResult) which
    /// one can de-sugar to extract actual values.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zune_core::result::DecodingResult;
    /// use zune_png::PngDecoder;
    /// let mut decoder = PngDecoder::new(&[]);
    ///
    /// match decoder.decode().unwrap(){
    ///     DecodingResult::U16(value)=>{
    ///         // deal with 16 bit images
    ///     }
    ///     DecodingResult::U8(value)=>{
    ///         // deal with <8 bit image
    ///     }
    ///     _=>{}
    /// }
    /// ```
    #[rustfmt::skip]
    pub fn decode(&mut self) -> Result<DecodingResult, PngDecodeErrors>
    {
        // Here we want to either return a `u8` or a `u16` depending on the
        // headers, so we pull two tricks
        //  1 - We either allocate u8 or u16 depending on the output
        //      We actually allocate both, but one of the vectors ends up being
        //      zero, and in creating an empty vec nothing is allocated on the heap
        //  2 - We convert samples to native endian, so that transmuting is a no-op in case of
        //      16 bit images in the next step
        //  3 - We use bytemuck to to safe align, hence keeping the no unsafe mantra except
        //      for platform specific intrinsics

        if !self.seen_headers || !self.seen_iend {
            self.decode_headers()?;
        }
        // configure that the decoder converts samples to native endian
        if is_le()
        {
            self.options = self.options.set_byte_endian(ByteEndian::LE);
        } else {
            self.options = self.options.set_byte_endian(ByteEndian::BE);
        }

        let info = &self.png_info;
        let bytes = if info.depth == 16 { 2 } else { 1 };

        let out_n = self.get_colorspace().unwrap().num_components();
        let new_len = info.width * info.height * out_n;

        let mut out_u8: Vec<u8> = vec![0; new_len * usize::from(info.depth != 16)];
        //let mut out_u16: Vec<u16> = vec![0; new_len * usize::from(info.depth == 16)];

        // use either out_u8 or out_u16 depending on the expected type for the output
        let out = if bytes == 1
        {
            &mut out_u8
        } else {
            /*let (a, b, c) = bytemuck::pod_align_to_mut::<u16, u8>(&mut out_u16);

            // a and c should be empty since we do not expect slop bytes on either edge
            assert!(a.is_empty());
            assert!(c.is_empty());
            assert_eq!(b.len(), new_len * 2); // length should be twice that of u8
            b*/
            panic!()
        };
        self.decode_into(out)?;

        if self.png_info.depth <= 8
        {
            return Ok(DecodingResult::U8(out_u8));
        }

        if self.png_info.depth == 16
        {
            panic!();
            //return Ok(DecodingResult::U16(out_u16));
        }

        Err(PngDecodeErrors::GenericStatic("Not implemented"))
    }
    /// Create the png data from post deflated data
    ///
    /// `out` needs to have enough space to hold data, otherwise
    /// this will panic
    ///
    /// This is to allow reuse e.g interlaced images use one big allocation
    /// to and since that ends up calling this multiple times, allocation was moved
    /// away from this method to the caller of this method
    #[allow(clippy::manual_memcpy, clippy::comparison_chain)]
    fn create_png_image_raw(
        &mut self, deflate_data: &[u8], width: usize, height: usize, out: &mut [u8], info: &PngInfo
    ) -> Result<(), PngDecodeErrors> {
        let use_sse4 = self.options.use_sse41();
        let use_sse2 = self.options.use_sse2();

        let bytes = if info.depth == 16 { 2 } else { 1 };

        let out_colorspace = self.get_colorspace().unwrap();

        let mut img_width_bytes;

        img_width_bytes = usize::from(info.component) * width;
        img_width_bytes *= usize::from(info.depth);
        img_width_bytes += 7;
        img_width_bytes /= 8;

        let out_n = usize::from(info.color.num_components());

        let image_len = img_width_bytes * height;

        if deflate_data.len() < image_len + height
        // account for filter bytes
        {
            let msg = format!(
                "Not enough pixels, expected {} but found {}",
                image_len,
                deflate_data.len()
            );
            return Err(PngDecodeErrors::Generic(msg));
        }
        // do png  un-filtering
        let mut chunk_size;
        let mut components = usize::from(info.color.num_components()) * bytes;

        if info.depth < 8 {
            // if the bit depth is 8, the spec says the byte before
            // X to be used by the filter
            components = 1;
        }

        // add width plus colour component, this gives us number of bytes per every scan line
        chunk_size = width * out_n;
        chunk_size *= usize::from(info.depth);
        chunk_size += 7;
        chunk_size /= 8;
        // filter type
        chunk_size += 1;

        let out_chunk_size = width * out_colorspace.num_components() * bytes;

        // each chunk is a width stride of unfiltered data
        let chunks = deflate_data.chunks_exact(chunk_size);

        // Begin doing loop un-filtering.
        let width_stride = chunk_size - 1;

        let mut prev_row_start = 0;
        let mut first_row = true;
        let mut out_position = 0;

        let mut will_post_process = self.seen_trns | self.seen_ptle | (info.depth < 8);

        let add_alpha_channel =
            self.options.png_get_add_alpha_channel() && (!self.png_info.color.has_alpha());

        will_post_process |= add_alpha_channel;

        if will_post_process && self.previous_stride.len() < out_chunk_size {
            self.previous_stride.resize(out_chunk_size, 0);
        }
        let n_components = usize::from(info.color.num_components());

        for (i, in_stride) in chunks.take(height).enumerate() {
            // Split output into current and previous
            // current points to the start of the row where we are writing de-filtered output to
            // prev is all rows we already wrote output to.

            let (prev, mut current) = out.split_at_mut(out_position);

            current = &mut current[0..out_chunk_size];

            // get the previous row.
            //Set this to a dummy to handle special case of first row, if we aren't in the first
            // row, we actually take the real slice a line down
            let mut prev_row: &[u8] = &[0_u8];

            if !first_row {
                // normal bit depth, use the previous row as normal
                prev_row = &prev[prev_row_start..prev_row_start + out_chunk_size];
                prev_row_start += out_chunk_size;
            }

            out_position += out_chunk_size;

            // take filter
            let filter_byte = in_stride[0];
            // raw image bytes
            let raw = &in_stride[1..];

            // get it's type
            let mut filter = FilterMethod::from_int(filter_byte)
                .ok_or_else(|| PngDecodeErrors::Generic(format!("Unknown filter {filter_byte}")))?;

            if first_row {
                // match our filters to special filters for first row
                // these special filters do not need the previous scanline and treat it
                // as zero

                if filter == FilterMethod::Paeth {
                    filter = FilterMethod::PaethFirst;
                }
                if filter == FilterMethod::Up {
                    // up for the first row becomes a memcpy
                    filter = FilterMethod::None;
                }
                if filter == FilterMethod::Average {
                    filter = FilterMethod::AvgFirst;
                }

                first_row = false;
            }

            match filter {
                FilterMethod::None => current[0..width_stride].copy_from_slice(raw),

                FilterMethod::Average => handle_avg(prev_row, raw, current, components, use_sse4),

                FilterMethod::Sub => handle_sub(raw, current, components, use_sse2),

                FilterMethod::Up => handle_up(prev_row, raw, current),

                FilterMethod::Paeth => handle_paeth(prev_row, raw, current, components, use_sse4),

                FilterMethod::PaethFirst => handle_paeth_first(raw, current, components),

                FilterMethod::AvgFirst => handle_avg_first(raw, current, components),

                FilterMethod::Unknown => unreachable!()
            }

            if will_post_process && i > 0 {
                // run the post processor two scanlines behind so that we
                // don't mess with any filters that require previous row

                // read the row we are about to filter
                let to_filter_row = &mut prev[(i - 1) * out_chunk_size..(i) * out_chunk_size];

                if info.depth < 8 {
                    // check if we will run any other transform
                    let extra_transform = self.seen_ptle | self.seen_trns | add_alpha_channel;

                    if extra_transform {
                        // input data is  in_to_filter_row,
                        // we write output to previous_stride
                        // since other parts use previous_stride
                        expand_bits_to_byte(
                            width,
                            usize::from(info.depth),
                            n_components,
                            self.seen_ptle,
                            to_filter_row,
                            &mut self.previous_stride
                        )
                    } else {
                        // no extra transform, just depth upscaling, so let's
                        // do that,

                        // copy the row to a temporary space
                        self.previous_stride[..width_stride]
                            .copy_from_slice(&to_filter_row[..width_stride]);

                        expand_bits_to_byte(
                            width,
                            usize::from(info.depth),
                            n_components,
                            self.seen_ptle,
                            &self.previous_stride,
                            to_filter_row
                        )
                    }
                } else {
                    // copy the row to a temporary space
                    self.previous_stride[..width_stride]
                        .copy_from_slice(&to_filter_row[..width_stride]);
                }

                if self.seen_trns && self.png_info.color != PngColor::Palette {
                    // the expansion is a trns expansion
                    // bytes are already in position, so finish the business

                    if info.depth <= 8 {
                        expand_trns::<false>(
                            &self.previous_stride,
                            to_filter_row,
                            info.color,
                            self.trns_bytes,
                            info.depth
                        );
                    } else if info.depth == 16 {
                        // Tested by test_palette_trns_16bit.
                        expand_trns::<true>(
                            &self.previous_stride,
                            to_filter_row,
                            info.color,
                            self.trns_bytes,
                            info.depth
                        );
                    }
                }

                if self.seen_ptle && self.png_info.color == PngColor::Palette {
                    if self.palette.is_empty() {
                        return Err(PngDecodeErrors::EmptyPalette);
                    }
                    let plte_entry: &[PLTEEntry; 256] = self.palette[..256].try_into().unwrap();

                    // so now we have two things
                    // the palette entries stored in self.previous_stride
                    // the row to fill the palette sored in to_filter row,
                    // so we can finally expand the entries

                    if self.seen_trns | add_alpha_channel {
                        // if tRNS chunk is present in paletted images, it contains
                        // alpha byte values, so that means we create alpha data from
                        // raw bytes

                        // if we are to add alpha channel for palette images , we simply just
                        // read four entries from the palette.
                        //
                        // The palette is set that the alpha channel is initialized as 255 for non alpha
                        // images,
                        expand_palette(&self.previous_stride, to_filter_row, plte_entry, 4);
                    } else {
                        // Normal expansion
                        expand_palette(&self.previous_stride, to_filter_row, plte_entry, 3);
                    }
                } else if add_alpha_channel {
                    // the image is a normal RGB/ Luma image, which we need to add the alpha channel
                    // do it here
                    add_alpha(
                        &self.previous_stride,
                        to_filter_row,
                        self.png_info.color,
                        self.get_depth().unwrap()
                    );
                }
            }
        }

        if will_post_process {
            for i in height..height + min(height, 1) {
                let to_filter_row = &mut out[(i - 1) * out_chunk_size..i * out_chunk_size];

                // check if we will run any other transform
                let extra_transform = self.seen_ptle | self.seen_trns;

                if info.depth < 8 {
                    if extra_transform {
                        // input data is  in_to_filter_row,
                        // we write output to previous_stride
                        // since other parts use previous_stride
                        expand_bits_to_byte(
                            width,
                            usize::from(info.depth),
                            n_components,
                            self.seen_ptle,
                            to_filter_row,
                            &mut self.previous_stride
                        )
                    } else {
                        // no extra transform, just depth upscaling, so let's
                        // do that,

                        // copy the row to a temporary space
                        self.previous_stride[..width_stride]
                            .copy_from_slice(&to_filter_row[..width_stride]);

                        expand_bits_to_byte(
                            width,
                            usize::from(info.depth),
                            n_components,
                            self.seen_ptle,
                            &self.previous_stride,
                            to_filter_row
                        )
                    }
                } else {
                    // copy the row to a temporary space
                    self.previous_stride[..width_stride]
                        .copy_from_slice(&to_filter_row[..width_stride]);
                }
                if self.seen_trns && self.png_info.color != PngColor::Palette {
                    // the expansion is a trns expansion
                    // bytes are already in position, so finish the business

                    if info.depth <= 8 {
                        expand_trns::<false>(
                            &self.previous_stride,
                            to_filter_row,
                            info.color,
                            self.trns_bytes,
                            info.depth
                        );
                    } else if info.depth == 16 {
                        // Tested by test_palette_trns_16bit.
                        expand_trns::<true>(
                            &self.previous_stride,
                            to_filter_row,
                            info.color,
                            self.trns_bytes,
                            info.depth
                        );
                    }
                }
                if self.seen_ptle && self.png_info.color == PngColor::Palette {
                    if self.palette.is_empty() {
                        return Err(PngDecodeErrors::EmptyPalette);
                    }

                    let plte_entry: &[PLTEEntry; 256] = self.palette[..256].try_into().unwrap();

                    if self.seen_trns | add_alpha_channel {
                        expand_palette(&self.previous_stride, to_filter_row, plte_entry, 4);
                    } else {
                        expand_palette(&self.previous_stride, to_filter_row, plte_entry, 3);
                    }
                } else if add_alpha_channel {
                    add_alpha(
                        &self.previous_stride,
                        to_filter_row,
                        self.png_info.color,
                        self.get_depth().unwrap()
                    );
                }
            }
        }
        Ok(())
    }

    /// Undo deflate decoding
    #[allow(clippy::manual_memcpy)]
    fn inflate(&mut self) -> Result<Vec<u8>, PngDecodeErrors> {
        let flat_data = &self.frames[self.current_frame];

        // An annoying thing is that deflate doesn't
        // store its uncompressed size,
        // so we can't pre-allocate storage and pass that willy nilly
        //
        // Meaning we are left with some design choices
        // 1. Have deflate resize at will
        // 2. Have deflate return incomplete, to indicate we need to extend
        // the vec, extend and go back to inflate.
        //
        //
        // so choose point 1.
        //
        // This allows the zlib decoder to optimize its own paths(which it does)
        // because it controls the allocation and doesn't have to check for near EOB
        // runs.
        //
        let depth_scale = if self.png_info.depth == 16 { 2 } else { 1 };

        let size_hint = (self.png_info.width + 1)
            * self.png_info.height
            * depth_scale
            * usize::from(self.png_info.color.num_components());

        let option = DeflateOptions::default()
            .set_size_hint(size_hint)
            .set_limit(size_hint + 4 * (self.png_info.height))
            .set_confirm_checksum(self.options.inflate_get_confirm_adler());

        let mut decoder = makepad_zune_inflate::DeflateDecoder::new_with_options(&flat_data.fdat, option);

        decoder
            .decode_zlib()
            .map_err(PngDecodeErrors::ZlibDecodeErrors)
    }
}
