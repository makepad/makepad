/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */
#![cfg_attr(feature = "portable-simd", feature(portable_simd))]

//! A png decoder
//!
//! This features a simple PNG reader in Rust which supports decoding of valid
//! ISO/IEC 15948:2003 (E) or PNG images
//!
//!
//! # Features
//! - Fast inflate decoder
//! - Platform specific intrinsics for accelerated decoding on x86
//! - Endian aware decoding support.
//! - Support for animated PNG and post processing of the same
//!
//!
//! ## portable-simd
//!  The crate supports using portable-simd to accelerate decoding of images, this can be used
//! in favour of platform specific intrinsics especially where those intrinsics haven't been written, e.g aarch64, wasm
//!
//! Though portable-simd is a nightly only feature, hence it is hidden under a flag `portable-simd` and can only compile
//! on rust nightly
//!
//! To enable it add
//! ``` toml
//! zune-png = {version="0.4",feature=["portable-simd"]}
//!```
//! and compile on nightly
//! # Usage
//! Add the library to `Cargo.toml`
//!
//! ```toml
//! zune_png="0.4"
//! ```
//!
//! #### Decode to 8-bit(1 byte) per pixel always
//!
//! PNG supports both 8-bit and 16 bit images, but people mainly expect the
//! images to be in 8 bit, the library can implicitly convert to 8 bit images when
//! requested in case one doesn't want to handle it  at the cost
//! of an extra allocation
//!
//! The below example shows how to do that
//!
//!```no_run
//! use zune_core::options::DecoderOptions;
//! use zune_png::PngDecoder;
//! // tell the png decoder to always strip 16 bit images to 8 bits
//! let options = DecoderOptions::default().png_set_strip_to_8bit(true);
//! let mut decoder = PngDecoder::new_with_options(&[],options);
//!
//! let pixels = decoder.decode_raw();
//! ```
//!
//!  Above, we set the  [`DecoderOptions::png_set_strip_to_8bit`](zune_core::options::DecoderOptions::png_get_strip_to_8bit)
//! to be true in order to indicate to the decoder that it should strip 16 bit images to 8 bit.
//!
//!
//! #### Decode to raw bytes.
//!
//! This is a simple decode operation which returns raw
//! bytes of the image.
//!
//! - **Note**: The interpretation of the data varies depending
//! on the endianness of the source image, for 16 bit depth images
//! each two bytes represent a single pixel in a configurable endian.
//! So one should inspect `PngDecoder::get_bit_depth` to get bit depth
//! of image in order to understand the raw bytes layout.
//!
//! A more convenient API is given below, using `decode`
//!
//!```no_run
//! use zune_png::PngDecoder;
//! let mut decoder = PngDecoder::new(&[]);
//!
//! let pixels = decoder.decode_raw();
//! ```
//!
//! ### Decode to u8 or u16 depending on depth
//!
//! From above limitation, there are needs to treat result
//! types differently depending on the image's bit depth.
//!
//! That's what the `decode` api for the PngDecoder does.
//!
//!```no_run
//! use zune_png::PngDecoder;
//! use zune_core::result::DecodingResult;
//! let mut decoder = PngDecoder::new(&[]);
//!
//! let pixels = decoder.decode().unwrap();
//!
//! match pixels {
//!    DecodingResult::U8(px)=>{
//!        // do something with images with 8 bit depths
//!    }
//!    DecodingResult::U16(px)=>{
//!        // do something with images with 16 bit depths
//!    }
//!    _=>unreachable!(),
//!}
//!```
//! The above has a more complicated API, but it ensures that you
//! handle any image depth correctly.
//!
//! E.g one can make it that 16 bit images are scaled to 8 bit images.
//!
//! # Endian aware decoding support
//!
//! One can set the target endianness of bits for 16 bit images by using
//! [`DecoderOptions::set_endian`](zune_core::options::DecoderOptions::set_byte_endian) which
//! will be respected by [`decode_raw`](decoder::PngDecoder::decode_raw) and [`decode_into`](decoder::PngDecoder::decode_into) functions
//!
//!
//! ### Decoding from RGB to RGBA
//!
//! Some endpoints may require data to be in RGBA such as GPUs but not all pngs have
//! the alpha channel.
//!
//! - Note: When input is in Luma, the transform will convert it to Luma+Alpha and not RGB+Alpha
//! to convert it to such types use the zune-image crate which provides efficient transforms for that
//!
//!```no_run
//! use zune_core::options::DecoderOptions;
//! use zune_png::PngDecoder;
//! // set option to add alpha channel
//! let options = DecoderOptions::default().png_set_add_alpha_channel(true);
//! // use the above option to decode
//! let mut decoder = PngDecoder::new_with_options(&[],options);
//!
//! decoder.decode().unwrap();
//! // the colorspace will always be have an alpha
//! assert!(decoder.get_colorspace().unwrap().has_alpha());
//! ```
//!
//! # Extracting metadata
//!
//! Once headers have been decoded, image metadata can be accessed via [`get_info()`](PngDecoder::get_info) method
//!
//! Some data is usually borrowed from the underlying reader, so the lifetime of the [`PngInfo`] struct is tied
//! to the lifetime of the [`PngDecoder`] struct from which it was derived
//!
//!
//! # Animated images decoding support.
//!
//! The library supports animated images decoding, up to post processing for 8-bit images.
//!
//! To understand more see [post_process_image]
//!
//! # Alternatives
//! - [png](https://crates.io/crates/png) crate
//!
//!
//!
#![cfg_attr(not(feature = "std"), no_std)]
#![allow(clippy::op_ref, clippy::identity_op)]
extern crate alloc;
extern crate core;

#[cfg(feature = "std")]
pub use apng::post_process_image;
pub use apng::{BlendOp, DisposeOp};
pub use decoder::{ItxtChunk, PngDecoder, PngInfo, TextChunk, TimeInfo, ZtxtChunk};
pub use encoder::PngEncoder;
pub use enums::InterlaceMethod;
pub use zune_core;

mod apng;
mod constants;
mod crc;
mod decoder;
mod encoder;
mod enums;
pub mod error;
mod filters;
mod headers;
mod options;
mod utils;
