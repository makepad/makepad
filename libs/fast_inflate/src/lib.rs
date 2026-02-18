mod adler32;
mod crc32;
mod decompress;
mod miniz_deflate;

pub use adler32::{adler32, Adler32};
pub use crc32::{crc32, Crc32};
pub use decompress::{
    deflate_decompress, deflate_decompress_vec, gzip_decompress, gzip_decompress_vec,
    zlib_decompress, zlib_decompress_vec, zlib_decompress_vec_with_hint, DecompressError,
};

/// Compress data with zlib wrapping.
pub fn zlib_compress(input: &[u8], level: u32) -> Vec<u8> {
    miniz_deflate::compress_to_vec_zlib(input, level as u8)
}

/// Compress raw DEFLATE data (no wrapper).
pub fn deflate_compress(input: &[u8], level: u32) -> Vec<u8> {
    miniz_deflate::compress_to_vec(input, level as u8)
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
