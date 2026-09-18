/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use alloc::vec::Vec;

use makepad_zune_core::bytestream::{ZByteIoError, ZByteWriterTrait, ZWriter};
use makepad_zune_core::options::EncoderOptions;
#[cfg(not(feature = "std"))]
use makepad_zune_inflate::DeflateEncoder;

use crate::constants::PNG_SIGNATURE;
use crate::decoder::PngChunk;
use crate::enums::{FilterMethod, PngChunkType};
use crate::filters::{choose_compression_filter, filter_scanline};
use crate::headers::writers::{
    write_chunk, write_exif, write_gamma, write_header_fn, write_iend, write_ihdr
};

#[derive(Default)]
pub struct PngEncoder<'a> {
    pub(crate) options:         EncoderOptions,
    pub(crate) data:            &'a [u8],
    pub(crate) row_filter:      FilterMethod,
    pub(crate) encoded_chunks:  Vec<u8>,
    pub(crate) filter_scanline: Vec<u8>,
    pub(crate) gamma:           Option<f32>,
    pub(crate) exif:            Option<&'a [u8]>
}

impl<'a> PngEncoder<'a> {
    /// Create a new encoder that can encode an image into a PNG chunk
    ///
    /// # Endianness
    ///
    /// If you are encoding 16 bit data, it is expected that
    /// the data is laid  out in big endian (in order to avoid a
    /// potentially expensive clone and conversion step)
    pub fn new(data: &'a [u8], options: EncoderOptions) -> PngEncoder<'a> {
        PngEncoder {
            options,
            data,
            row_filter: FilterMethod::None,
            ..Default::default()
        }
    }

    /// Add exif data which will be encoded
    pub fn add_exif_segment(&mut self, exif: &'a [u8]) {
        self.exif = Some(exif);
    }

    pub fn encode_headers<T: ZByteWriterTrait>(
        &self, writer: &mut ZWriter<T>
    ) -> Result<(), ZByteIoError> {
        // write signature
        writer.write_u64_be(PNG_SIGNATURE);
        // write ihdr
        write_header_fn(self, writer, b"IHDR", write_ihdr)?;

        // extra headers
        // need to check their existence because  write_header_fn will do
        // some writing even if they don't exist
        if self.exif.is_some() {
            write_header_fn(self, writer, b"eXIf", write_exif)?;
        }
        if self.gamma.is_some() {
            write_header_fn(self, writer, b"gAMA", write_gamma)?;
        }
        Ok(())
    }

    pub fn encode<T: ZByteWriterTrait>(&mut self, sink: T) -> Result<usize, ZByteIoError> {
        let expected_data_size = self
            .options
            .width()
            .checked_mul(self.options.height())
            .ok_or(ZByteIoError::Generic("Overflow"))?
            .checked_mul(self.options.depth().size_of())
            .ok_or(ZByteIoError::Generic("Overflow"))?
            .checked_mul(self.options.colorspace().num_components())
            .ok_or(ZByteIoError::Generic("Overflow"))?;

        if self.data.len() != expected_data_size {
            return Err(ZByteIoError::NotEnoughBytes(
                expected_data_size,
                self.data.len()
            ));
        }
        let mut writer = ZWriter::new(sink);

        self.encode_headers(&mut writer)?;

        // encode filters
        self.add_filters();

        self.write_idat_chunks(&mut writer)?;

        write_header_fn(self, &mut writer, b"IEND", write_iend)?;

        // let position = writer.position();
        // out_size.truncate(position);

        Ok(writer.bytes_written())
    }

    const fn calculate_scanline_size(&self) -> usize {
        self.options.width()
            * self.options.depth().size_of()
            * self.options.colorspace().num_components()
    }

    fn add_filters(&mut self) {
        let scanline_length = (self.calculate_scanline_size() + 1)
            .checked_mul(self.options.height())
            .unwrap();
        let components =
            self.options.colorspace().num_components() * self.options.depth().size_of();

        // allocate space for filtered scanline
        self.filter_scanline.resize(scanline_length, 0);

        // one row above the current processing row
        let mut previous_scanline: &[u8] = &[];

        let scanline_size = self.calculate_scanline_size();

        for (i, filter_s) in self
            .filter_scanline
            .chunks_exact_mut(scanline_size + 1)
            .take(self.options.height())
            .enumerate()
        {
            let (previous, current) = self.data.split_at(i * scanline_size);

            if i > 0 {
                // previous row now becomes defined
                previous_scanline = &previous[(i - 1) * scanline_size..];
            }
            let current_scanline = &current[0..scanline_size];
            let filter = choose_compression_filter(previous_scanline, current_scanline);

            filter_scanline(
                current_scanline,
                previous_scanline,
                filter_s,
                filter,
                components
            );
        }
        // The bundled zune-inflate encoder currently only emits stored
        // DEFLATE blocks. That is a valid PNG, but makes large generated
        // atlases almost exactly raw RGBA size. Standard builds use the
        // in-repo compressor; retain the store-only fallback for no_std.
        #[cfg(feature = "std")]
        {
            self.encoded_chunks = makepad_fast_inflate::zlib_compress(&self.filter_scanline, 6);
        }
        #[cfg(not(feature = "std"))]
        {
            self.encoded_chunks = DeflateEncoder::new(&self.filter_scanline).encode_zlib();
        }
    }
    fn write_idat_chunks<T: ZByteWriterTrait>(
        &self, writer: &mut ZWriter<T>
    ) -> Result<(), ZByteIoError> {
        debug_assert!(!self.encoded_chunks.is_empty());
        // Most decoders love data in 8KB chunks, since
        // probably libpng does that by default
        // so let's try emulating that
        for chunk in self.encoded_chunks.chunks(8192) {
            let chunk_type = PngChunk {
                length:     chunk.len(),
                chunk_type: PngChunkType::IDAT, // not needed
                chunk:      *b"IDAT",
                crc:        0 // not needed
            };
            write_chunk(chunk_type, chunk, writer)?;
        }
        Ok(())
    }
}

/// The IDAT chunk size the row-band encoder writes: one deflate output
/// step per chunk (the whole-image encoder's 8 KiB chunks suit icons; a
/// print band produces megabytes).
#[cfg(feature = "std")]
const STREAM_IDAT_BYTES: usize = 1 << 16;

/// A PNG written band by band: `write_rows` filters and deflates the rows
/// it is given and streams the IDAT chunks to the sink, `finish` ends the
/// stream. The signature, headers, filters and compressor are
/// `PngEncoder`'s; the rows are never held whole (a print export is
/// gigapixel-class, so the caller hands over one tile row at a time).
#[cfg(feature = "std")]
pub struct PngStreamEncoder<T: ZByteWriterTrait> {
    writer:     ZWriter<T>,
    options:    EncoderOptions,
    scanline:   usize,
    components: usize,
    rows:       usize,
    previous:   Vec<u8>,
    filtered:   Vec<u8>,
    compressor: makepad_fast_inflate::ZlibCompressor,
    pending:    Vec<u8>
}

#[cfg(feature = "std")]
impl<T: ZByteWriterTrait> PngStreamEncoder<T> {
    /// Writes the signature and headers for `options` (8-bit depths only:
    /// the rows arrive as bytes in scanline order).
    pub fn new(sink: T, options: EncoderOptions) -> Result<Self, ZByteIoError> {
        if options.depth() != makepad_zune_core::bit_depth::BitDepth::Eight {
            return Err(ZByteIoError::Generic("the row-band encoder writes 8-bit samples"));
        }
        let mut writer = ZWriter::new(sink);
        PngEncoder::new(&[], options).encode_headers(&mut writer)?;
        let components = options.colorspace().num_components() * options.depth().size_of();
        let scanline = options.width() * components;
        if scanline == 0 || options.height() == 0 {
            return Err(ZByteIoError::Generic("an empty image"));
        }
        Ok(PngStreamEncoder {
            writer,
            options,
            scanline,
            components,
            rows: 0,
            previous: Vec::new(),
            filtered: vec![0; scanline + 1],
            compressor: makepad_fast_inflate::ZlibCompressor::new(6),
            pending: Vec::new()
        })
    }

    /// Whole rows, top to bottom (`rows.len()` a multiple of the scanline).
    pub fn write_rows(&mut self, rows: &[u8]) -> Result<(), ZByteIoError> {
        if rows.len() % self.scanline != 0 {
            return Err(ZByteIoError::Generic("rows are not whole scanlines"));
        }
        for row in rows.chunks_exact(self.scanline) {
            if self.rows >= self.options.height() {
                return Err(ZByteIoError::Generic("more rows than the image height"));
            }
            let filter = choose_compression_filter(&self.previous, row);
            filter_scanline(row, &self.previous, &mut self.filtered, filter, self.components);
            self.compressor.write(&self.filtered, &mut self.pending);
            self.previous.clear();
            self.previous.extend_from_slice(row);
            self.rows += 1;
            self.write_idat(false)?;
        }
        Ok(())
    }

    /// Ends the stream (the last IDAT chunks and IEND) once every row of
    /// the image was written: the bytes written in all, and the sink (a
    /// buffered file the caller still flushes).
    pub fn finish(mut self) -> Result<(usize, T), ZByteIoError> {
        if self.rows != self.options.height() {
            return Err(ZByteIoError::NotEnoughBytes(self.options.height(), self.rows));
        }
        self.compressor.finish(&mut self.pending);
        self.write_idat(true)?;
        write_header_fn(&PngEncoder::new(&[], self.options), &mut self.writer, b"IEND", write_iend)?;
        Ok((self.writer.bytes_written(), self.writer.into_inner()))
    }

    /// The pending compressed bytes as whole chunks (every byte with `all`).
    fn write_idat(&mut self, all: bool) -> Result<(), ZByteIoError> {
        let mut done = 0;
        while self.pending.len() - done >= STREAM_IDAT_BYTES || (all && done < self.pending.len()) {
            let end = (done + STREAM_IDAT_BYTES).min(self.pending.len());
            let chunk = PngChunk {
                length:     end - done,
                chunk_type: PngChunkType::IDAT,
                chunk:      *b"IDAT",
                crc:        0
            };
            write_chunk(chunk, &self.pending[done..end], &mut self.writer)?;
            done = end;
        }
        self.pending.drain(..done);
        Ok(())
    }
}

#[test]
fn test_simple_write() {
    use makepad_zune_core::bit_depth::BitDepth;
    use makepad_zune_core::bytestream::ZCursor;
    use makepad_zune_core::colorspace::ColorSpace;

    use crate::PngDecoder;

    let width = 40;
    let height = 10;
    let data = vec![100; width * height];

    let options = EncoderOptions::default()
        .set_colorspace(ColorSpace::Luma)
        .set_width(40)
        .set_height(10)
        .set_depth(BitDepth::Eight);

    let mut encoder = PngEncoder::new(&data, options);
    let mut sink = vec![];

    let _ = encoder.encode(&mut sink).unwrap();
    let mut hello = PngDecoder::new(ZCursor::new(&sink));
    let bytes = hello.decode_raw().unwrap();
    assert_eq!(&data, &bytes);
}

#[test]
#[cfg(feature = "std")]
fn repetitive_rgba_is_actually_compressed() {
    use makepad_zune_core::bit_depth::BitDepth;
    use makepad_zune_core::colorspace::ColorSpace;

    let width = 256;
    let height = 256;
    let data = vec![127u8; width * height * 4];
    let options = EncoderOptions::default()
        .set_colorspace(ColorSpace::RGBA)
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight);
    let mut encoder = PngEncoder::new(&data, options);
    let mut sink = vec![];
    encoder.encode(&mut sink).unwrap();
    assert!(sink.len() < data.len() / 20, "png bytes {}", sink.len());
}
