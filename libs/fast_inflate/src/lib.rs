mod adler32;
mod crc32;
mod decompress;

pub use adler32::{adler32, Adler32};
pub use crc32::{crc32, Crc32};
pub use decompress::{
    deflate_decompress, deflate_decompress_vec, zlib_decompress, zlib_decompress_vec,
    zlib_decompress_vec_with_hint, DecompressError,
};
