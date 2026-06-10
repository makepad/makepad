/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use alloc::string::String;
use core::fmt::{Debug, Formatter};

use makepad_zune_core::bytestream::ZByteIoError;

/// BMP errors that can occur during decoding
#[non_exhaustive]
pub enum BmpDecoderErrors {
    /// The file/bytes do not start with `BM`
    InvalidMagicBytes,
    /// The output buffer is too small, expected at least
    /// a size but got another size
    TooSmallBuffer(usize, usize),
    /// Generic message
    GenericStatic(&'static str),
    /// Generic allocated message
    Generic(String),
    /// Too large dimensions for a given width or
    /// height
    TooLargeDimensions(&'static str, usize, usize),
    /// A calculation overflowed
    OverFlowOccurred,
    IoErrors(ZByteIoError)
}

impl Debug for BmpDecoderErrors {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidMagicBytes => {
                writeln!(f, "Invalid magic bytes, file does not start with BM")
            }
            Self::TooSmallBuffer(expected, found) => {
                writeln!(
                    f,
                    "Too small of buffer, expected {expected} but found {found}"
                )
            }
            Self::GenericStatic(header) => {
                writeln!(f, "{header}")
            }
            Self::TooLargeDimensions(dimension, expected, found) => {
                writeln!(
                    f,
                    "Too large dimensions for {dimension} , {found} exceeds {expected}"
                )
            }
            Self::Generic(message) => {
                writeln!(f, "{message}")
            }
            Self::OverFlowOccurred => {
                writeln!(f, "Overflow occurred")
            }
            Self::IoErrors(err) => {
                writeln!(f, "{err:?}")
            }
        }
    }
}

impl From<ZByteIoError> for BmpDecoderErrors {
    fn from(value: ZByteIoError) -> Self {
        BmpDecoderErrors::IoErrors(value)
    }
}
