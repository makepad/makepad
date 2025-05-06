/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

//! Errors possible during png operations
use alloc::string::String;
use core::fmt::{Debug, Display, Formatter};

/// Errors possible during decoding
pub enum PngDecodeErrors {
    /// Image signature is not png signature
    BadSignature,
    /// Generic message
    GenericStatic(&'static str),
    /// Generic message
    Generic(String),
    /// Calculated CRC does not match expected crc
    BadCrc(u32, u32),
    /// error decoding zlib stream
    ZlibDecodeErrors(zune_inflate::errors::InflateDecodeErrors),
    /// Palette is empty yet was expected
    EmptyPalette,
    /// Unsupported Animated PNG
    UnsupportedAPNGImage,
    /// Too small output slice
    TooSmallOutput(usize, usize)
}

impl Display for PngDecodeErrors {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for PngDecodeErrors {}

impl Debug for PngDecodeErrors {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadSignature => writeln!(f, "Bad PNG signature, not a png"),
            Self::GenericStatic(val) => writeln!(f, "{val:?}"),
            Self::Generic(val) => writeln!(f, "{val:?}"),
            Self::BadCrc(expected, found) => writeln!(
                f,
                "CRC does not match, expected {expected} but found {found}",
            ),
            Self::ZlibDecodeErrors(err) => {
                writeln!(f, "Error decoding idat chunks {err:?}")
            }
            Self::EmptyPalette => {
                writeln!(f, "Empty palette but image is indexed")
            }
            Self::UnsupportedAPNGImage => {
                writeln!(f, "Unsupported APNG format")
            }
            Self::TooSmallOutput(expected, found) => {
                write!(f, "Too small output, expected buffer with at least {expected} bytes but got one with {found} bytes")
            }
        }
    }
}

impl From<&'static str> for PngDecodeErrors {
    fn from(val: &'static str) -> Self {
        Self::GenericStatic(val)
    }
}

impl From<String> for PngDecodeErrors {
    fn from(val: String) -> Self {
        Self::Generic(val)
    }
}

impl From<zune_inflate::errors::InflateDecodeErrors> for PngDecodeErrors {
    fn from(val: zune_inflate::errors::InflateDecodeErrors) -> Self {
        Self::ZlibDecodeErrors(val)
    }
}
