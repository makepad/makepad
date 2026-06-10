/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use alloc::string::String;
/// Errors possible during decoding.
use core::fmt::{Debug, Display, Formatter};

use makepad_zune_core::bytestream::ZByteIoError;
use makepad_zune_core::colorspace::ColorSpace;

/// Possible Errors that may occur during decoding
pub enum QoiErrors {
    /// The image does not start with QOI magic bytes `qoif`
    ///
    /// Indicates that image is not a qoi file
    WrongMagicBytes,
    /// The input buffer doesn't have enough bytes to fully
    /// reconstruct the image
    ///
    /// # Arguments
    /// - 1st argument is the number of bytes we expected
    /// - 2nd argument is number of bytes actually left
    InsufficientData(usize, usize),
    /// The header contains an invalid channel number
    ///
    /// The only supported types are `3` and `4`
    UnknownChannels(u8),
    /// The header contains an invalid colorspace value
    ///
    /// The should be `0` or `1`
    /// but this can be ignored if strict is set to false
    UnknownColorspace(u8),
    /// Generic message
    Generic(String),
    /// Generic message does not need heap allocation
    GenericStatic(&'static str),
    /// To small output size
    TooSmallOutput(usize, usize),
    IoErrors(ZByteIoError)
}

impl Debug for QoiErrors {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            QoiErrors::WrongMagicBytes => {
                writeln!(f, "Wrong magic bytes, expected `qoif` as image start")
            }
            QoiErrors::InsufficientData(expected, found) => {
                writeln!(
                    f,
                    "Insufficient data required {expected} but remaining stream has {found}"
                )
            }
            QoiErrors::UnknownChannels(channel) => {
                writeln!(
                    f,
                    "Unknown channel number {channel}, expected either 3 or 4"
                )
            }
            QoiErrors::UnknownColorspace(colorspace) => {
                writeln!(
                    f,
                    "Unknown colorspace number {colorspace}, expected either 0 or 1"
                )
            }
            QoiErrors::Generic(val) => {
                writeln!(f, "{val}")
            }
            QoiErrors::GenericStatic(val) => {
                writeln!(f, "{val}")
            }
            QoiErrors::TooSmallOutput(expected, found) => {
                writeln!(
                    f,
                    "Too small output size, expected {expected}, but found {found}"
                )
            }
            QoiErrors::IoErrors(value) => {
                writeln!(f, "I/O error {value:?}")
            }
        }
    }
}

impl From<&'static str> for QoiErrors {
    fn from(r: &'static str) -> Self {
        Self::GenericStatic(r)
    }
}

impl From<ZByteIoError> for QoiErrors {
    fn from(value: ZByteIoError) -> Self {
        QoiErrors::IoErrors(value)
    }
}
/// Errors encountered during encoding
pub enum QoiEncodeErrors {
    /// Unsupported colorspace
    ///
    /// The first argument is the colorspace encountered
    /// The second argument is list of supported colorspaces
    UnsupportedColorspace(ColorSpace, &'static [ColorSpace]),

    /// Too large dimensions
    /// The dimensions cannot be correctly encoded to a width
    TooLargeDimensions(usize),

    Generic(&'static str),

    IoError(ZByteIoError)
}

impl Debug for QoiEncodeErrors {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            QoiEncodeErrors::UnsupportedColorspace(found, supported) => {
                writeln!(f, "Cannot encode image with colorspace {found:?} into QOI, supported ones are {supported:?}")
            }
            QoiEncodeErrors::TooLargeDimensions(found) => {
                writeln!(
                    f,
                    "Too large image dimensions {found}, QOI can only encode images less than {}",
                    u32::MAX
                )
            }
            QoiEncodeErrors::Generic(val) => {
                writeln!(f, "{val}")
            }
            QoiEncodeErrors::IoError(v) => {
                writeln!(f, "I/O error {v:?}")
            }
        }
    }
}

impl Display for QoiEncodeErrors {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "{self:?}")
    }
}
impl Display for QoiErrors {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "{self:?}")
    }
}

#[cfg(feature = "std")]
impl std::error::Error for QoiEncodeErrors {}

#[cfg(feature = "std")]
impl std::error::Error for QoiErrors {}

impl From<ZByteIoError> for QoiEncodeErrors {
    fn from(value: ZByteIoError) -> Self {
        Self::IoError(value)
    }
}
