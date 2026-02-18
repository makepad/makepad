// Vendored from miniz_oxide, adapted to remove external dependencies.
// Original: Copyright miniz_oxide contributors, MIT license.

mod buffer;
pub mod core;
pub(crate) mod shared;
mod stored;
pub mod stream;
mod zlib;

use self::core::*;

/// How much processing the compressor should do to compress the data.
#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum CompressionLevel {
    NoCompression = 0,
    BestSpeed = 1,
    BestCompression = 9,
    UberCompression = 10,
    DefaultLevel = 6,
    DefaultCompression = -1,
}

/// A list of flush types.
#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum MZFlush {
    None = 0,
    Partial = 1,
    Sync = 2,
    Full = 3,
    Finish = 4,
    Block = 5,
}

impl MZFlush {
    pub fn new(flush: i32) -> Result<Self, MZError> {
        match flush {
            0 => Ok(MZFlush::None),
            1 | 2 => Ok(MZFlush::Sync),
            3 => Ok(MZFlush::Full),
            4 => Ok(MZFlush::Finish),
            _ => Err(MZError::Param),
        }
    }
}

/// A list of miniz successful status codes.
#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum MZStatus {
    Ok = 0,
    StreamEnd = 1,
    NeedDict = 2,
}

/// A list of miniz failed status codes.
#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum MZError {
    ErrNo = -1,
    Stream = -2,
    Data = -3,
    Mem = -4,
    Buf = -5,
    Version = -6,
    Param = -10_000,
}

/// How compressed data is wrapped.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DataFormat {
    Zlib,
    ZLibIgnoreChecksum,
    Raw,
}

impl DataFormat {
    pub fn from_window_bits(window_bits: i32) -> DataFormat {
        if window_bits > 0 {
            DataFormat::Zlib
        } else {
            DataFormat::Raw
        }
    }

    pub fn to_window_bits(self) -> i32 {
        match self {
            DataFormat::Zlib | DataFormat::ZLibIgnoreChecksum => shared::MZ_DEFAULT_WINDOW_BITS,
            DataFormat::Raw => -shared::MZ_DEFAULT_WINDOW_BITS,
        }
    }
}

pub type MZResult = Result<MZStatus, MZError>;

/// A structure containing the result of a call to the inflate or deflate streaming functions.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct StreamResult {
    pub bytes_consumed: usize,
    pub bytes_written: usize,
    pub status: MZResult,
}

impl StreamResult {
    #[inline]
    pub(crate) fn error(error: MZError) -> StreamResult {
        StreamResult {
            bytes_consumed: 0,
            bytes_written: 0,
            status: Err(error),
        }
    }
}

/// Compress the input data to a vector, using the specified compression level (0-10).
pub fn compress_to_vec(input: &[u8], level: u8) -> Vec<u8> {
    compress_to_vec_inner(input, level, 0, 0)
}

/// Compress the input data to a vector, using the specified compression level (0-10), and with a
/// zlib wrapper.
pub fn compress_to_vec_zlib(input: &[u8], level: u8) -> Vec<u8> {
    compress_to_vec_inner(input, level, 1, 0)
}

/// Simple function to compress data to a vec.
fn compress_to_vec_inner(mut input: &[u8], level: u8, window_bits: i32, strategy: i32) -> Vec<u8> {
    let flags = create_comp_flags_from_zip_params(level.into(), window_bits, strategy);
    let mut compressor = CompressorOxide::new(flags);
    let mut output = vec![0; ::core::cmp::max(input.len() / 2, 2)];

    let mut out_pos = 0;
    loop {
        let (status, bytes_in, bytes_out) = compress(
            &mut compressor,
            input,
            &mut output[out_pos..],
            TDEFLFlush::Finish,
        );
        out_pos += bytes_out;

        match status {
            TDEFLStatus::Done => {
                output.truncate(out_pos);
                break;
            }
            TDEFLStatus::Okay if bytes_in <= input.len() => {
                input = &input[bytes_in..];
                if output.len().saturating_sub(out_pos) < 30 {
                    output.resize(output.len() * 2, 0)
                }
            }
            _ => panic!("Bug! Unexpectedly failed to compress!"),
        }
    }

    output
}
