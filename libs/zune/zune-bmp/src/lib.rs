/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */
//! A versatile BMP decoder
//!
//! This crate features a BMP decoder capable of decoding
//! multiple BMP images fast
//!
//! # Features
//! - `no_std` by default with `alloc` feature
//! - Fast
//! - Minimal dependencies
//! - Very minimal internal allocation. (most paths do not allocate any more than output buffer)
//!
//! # Supported formats
//! - RLE (4 bit and 8 bit)
//! - Paletted images(1 bit, 2 bits, 4  bits and 8 bits)
//! - Masked images (16 bit and 32 bit formats)
//!
//! # Unsupported formats
//! - Embedded PNG and JPEGs
//! - Images with embedded color profiles. (the embedded color profile is ignored)
//!
//! # Features
//!  - `log`: Use the `log` crate features to print image information when decoding
//!  - `std`: Allow direct decoding from anything that implements `std::io::BufRead` + `std::io::Seek`
//!
//! # Usage
//!  It is recommended that if you have an in memory buffer you use
//! `ZCursor` to read as it is more optimized when compared to `std::io::Cursor` since it's
//!  methods are specialized for the [`ZByteReaderTrait`](crate::makepad_zune_core::bytestream::ZByteReaderTrait)
//!
//!
//! ```no_run
//! use zune_bmp::BmpDecoder;
//! use makepad_zune_core::bytestream::ZCursor;
//!
//! let decoder:Vec<u8> = BmpDecoder::new(ZCursor::new(b"BMP")).decode().unwrap();
//! ```
//!
//! You can also read directly from bmp files. This can be preferred when you don't have the file contents
//! in memory and just want the pixels
//!
//! It's recommended you wrap the file in a bufreader.
//!
//! This requires the `std` feature to work.
//! ```no_run
//! use std::fs::File;
//! use std::io::BufReader;
//! use zune_bmp::BmpDecoder;
//! // read from a file
//! let source = BufReader::new(File::open("./image.bmp").unwrap());
//! // only run when std is enabled, otherwise makepad_zune_core doesn't implement the ZByteReader trait
//! // on File since it doesn't exist in `no_std` land
//! #[cfg(feature = "std")]
//! let decoder = BmpDecoder::new(source);
//!
//! ```
//!
//! # Security
//!
//! The decoder is continuously fuzz tested in CI to ensure it does not crash on malicious input
//! in case a sample causes it to crash, an issue would be welcome.
//!
//! # Performance
//! BMP isn't a compute heavy image format, this crate should not be a bottleneck in any way possible,
//! benchmark just in case you think it's slowing you down in any way.
//!
#![no_std]
#![macro_use]
extern crate alloc;

extern crate core;

pub use makepad_zune_core;

pub use crate::decoder::{probe_bmp, BmpDecoder};
pub use crate::errors::BmpDecoderErrors;

mod common;
mod decoder;
mod errors;
mod utils;
