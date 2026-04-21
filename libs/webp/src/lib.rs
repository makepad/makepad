//! Decoding and Encoding of WebP Images

#![forbid(unsafe_code)]

pub use self::decoder::{
    DecodingError, LoopCount, UpsamplingMethod, WebPDecodeOptions, WebPDecoder,
};
#[cfg(feature = "encoder")]
pub use self::encoder::{ColorType, EncoderParams, EncodingError, WebPEncoder};

mod alpha_blending;
mod decoder;
#[cfg(feature = "encoder")]
mod encoder;
mod extended;
mod huffman;
mod loop_filter;
mod lossless;
mod lossless_transform;
mod transform;
mod vp8_arithmetic_decoder;
mod yuv;

pub mod vp8;
