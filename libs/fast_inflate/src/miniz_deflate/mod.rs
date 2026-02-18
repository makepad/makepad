// Vendored from miniz_oxide, adapted to remove external dependencies.
// Original: Copyright miniz_oxide contributors, MIT license.

mod buffer;
pub mod core;
pub(crate) mod shared;
mod stored;
mod zlib;

use self::core::*;

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
