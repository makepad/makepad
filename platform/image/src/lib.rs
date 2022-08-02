pub mod lode_png;

use crate::lode_png::lode_png::lodepng_decode_memory;
use crate::lode_png::lode_png_types::PNGError;
use crate::lode_png::lode_png_types::ColorType;

// lets load a 32 bit png
pub fn load_png_rgba32(input:&[u8])->Result<(Vec<u8>, usize, usize), PNGError>{
    lodepng_decode_memory(input, ColorType::RGBA, 8)
}

