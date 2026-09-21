mod adler32;
mod crc32;
mod decompress;
mod miniz_deflate;

pub use adler32::{adler32, Adler32};
pub use crc32::{crc32, Crc32};
pub use decompress::{
    deflate_decompress, deflate_decompress_from, deflate_decompress_vec, gzip_decompress, gzip_decompress_vec,
    zlib_decompress, zlib_decompress_vec, zlib_decompress_vec_with_hint, DecompressError,
};

/// Compress data with zlib wrapping.
pub fn zlib_compress(input: &[u8], level: u32) -> Vec<u8> {
    miniz_deflate::compress_to_vec_zlib(input, level as u8)
}

/// Output is taken from the compressor in steps of this size (its own
/// block size is 64 KiB; a smaller buffer only costs extra calls).
const ZLIB_OUTPUT_STEP: usize = 1 << 16;

/// A zlib stream compressed as its input arrives (the row-band PNG
/// encoder's compressor): `write` takes any amount of input, appending
/// whatever output the compressor releases to `out`; `finish` ends the
/// stream with its trailer. Nothing after `finish` is accepted.
pub struct ZlibCompressor {
    compressor: miniz_deflate::core::CompressorOxide,
}

impl ZlibCompressor {
    /// `level` 0–10 as `zlib_compress` takes it.
    pub fn new(level: u32) -> Self {
        let flags = miniz_deflate::core::create_comp_flags_from_zip_params(level as i32, 1, 0);
        Self { compressor: miniz_deflate::core::CompressorOxide::new(flags) }
    }

    pub fn write(&mut self, input: &[u8], out: &mut Vec<u8>) {
        self.run(input, out, miniz_deflate::core::TDEFLFlush::None);
    }

    pub fn finish(&mut self, out: &mut Vec<u8>) {
        self.run(&[], out, miniz_deflate::core::TDEFLFlush::Finish);
    }

    fn run(&mut self, mut input: &[u8], out: &mut Vec<u8>, flush: miniz_deflate::core::TDEFLFlush) {
        use miniz_deflate::core::{compress, TDEFLFlush, TDEFLStatus};
        loop {
            let start = out.len();
            let room = (input.len() / 2).max(ZLIB_OUTPUT_STEP);
            out.resize(start + room, 0);
            let (status, consumed, written) = compress(&mut self.compressor, input, &mut out[start..], flush);
            out.truncate(start + written);
            match status {
                TDEFLStatus::Done => return,
                TDEFLStatus::Okay => {
                    input = &input[consumed..];
                    // the input is in and the output buffer did not fill:
                    // nothing more is released until the next write
                    if flush == TDEFLFlush::None && input.is_empty() && written < room {
                        return;
                    }
                }
                status => panic!("zlib compressor failed: {status:?}"),
            }
        }
    }
}

/// Compress raw DEFLATE data (no wrapper).
pub fn deflate_compress(input: &[u8], level: u32) -> Vec<u8> {
    miniz_deflate::compress_to_vec(input, level as u8)
}

/// Compress data with a gzip header/trailer (RFC 1952).
pub fn gzip_compress(input: &[u8], level: u32) -> Vec<u8> {
    let body = deflate_compress(input, level);
    let mut out = Vec::with_capacity(18 + body.len());
    // ID1 ID2 CM FLG MTIME[4] XFL OS=unknown
    out.extend_from_slice(&[0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff]);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(input).to_le_bytes());
    out.extend_from_slice(&(input.len() as u32).to_le_bytes());
    out
}

/// miniz_oxide-compatible inflate (decompression) API.
pub mod inflate {
    use crate::decompress::DecompressError;

    /// Decompress raw DEFLATE data (no zlib/gzip wrapper) to a Vec.
    pub fn decompress_to_vec(input: &[u8]) -> Result<Vec<u8>, DecompressError> {
        crate::decompress::deflate_decompress_vec(input)
    }

    /// Decompress zlib-wrapped data to a Vec.
    pub fn decompress_to_vec_zlib(input: &[u8]) -> Result<Vec<u8>, DecompressError> {
        crate::decompress::zlib_decompress_vec(input)
    }
}

/// miniz_oxide-compatible deflate (compression) API.
pub mod deflate {
    /// Compress data to a Vec using raw DEFLATE (no wrapper).
    pub fn compress_to_vec(input: &[u8], level: u8) -> Vec<u8> {
        crate::miniz_deflate::compress_to_vec(input, level)
    }

    /// Compress data to a Vec using zlib wrapping.
    pub fn compress_to_vec_zlib(input: &[u8], level: u8) -> Vec<u8> {
        crate::miniz_deflate::compress_to_vec_zlib(input, level)
    }
}

#[cfg(test)]
mod zlib_compressor_tests {
    use super::*;

    fn sample(len: usize) -> Vec<u8> {
        // text-like runs with a drifting pattern: compressible, not trivial
        let mut seed = 0x9e37_79b9u32;
        (0..len)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                if i % 7 == 0 { (seed >> 24) as u8 } else { b'a' + (i % 13) as u8 }
            })
            .collect()
    }

    #[test]
    fn streamed_pieces_inflate_to_the_input() {
        let input = sample(3 << 20);
        let mut out = Vec::new();
        let mut compressor = ZlibCompressor::new(6);
        for piece in input.chunks(100_003) {
            compressor.write(piece, &mut out);
        }
        compressor.finish(&mut out);
        assert!(out.len() < input.len() / 2, "compressed {} of {}", out.len(), input.len());
        assert_eq!(zlib_decompress_vec(&out).unwrap(), input);
        // the same bytes the whole-buffer path produces at this level
        assert_eq!(out, zlib_compress(&input, 6));
    }

    #[test]
    fn an_empty_stream_is_a_valid_zlib_stream() {
        let mut out = Vec::new();
        ZlibCompressor::new(6).finish(&mut out);
        assert_eq!(zlib_decompress_vec(&out).unwrap(), Vec::<u8>::new());
    }
}
