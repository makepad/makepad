/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

//! Image Colorspace information and manipulation utilities.

/// All possible image colorspaces
/// Some of them aren't yet supported exist here.
#[allow(clippy::upper_case_acronyms)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ColorSpace {
    /// Red, Green , Blue
    RGB,
    /// Red, Green, Blue, Alpha
    RGBA,
    /// YUV colorspace
    YCbCr,
    /// Grayscale colorspace
    Luma,
    /// Grayscale with alpha colorspace
    LumaA,
    YCCK,
    /// Cyan , Magenta, Yellow, Black
    CMYK,
    /// Blue, Green, Red
    BGR,
    /// Blue, Green, Red, Alpha
    BGRA,
    /// The colorspace is unknown
    Unknown
}

impl ColorSpace {
    pub const fn num_components(&self) -> usize {
        match self {
            Self::RGB | Self::YCbCr | Self::BGR => 3,
            Self::RGBA | Self::YCCK | Self::CMYK | Self::BGRA => 4,
            Self::Luma => 1,
            Self::LumaA => 2,
            Self::Unknown => 0
        }
    }

    pub const fn has_alpha(&self) -> bool {
        matches!(self, Self::RGBA | Self::LumaA | Self::BGRA)
    }

    pub const fn is_grayscale(&self) -> bool {
        matches!(self, Self::LumaA | Self::Luma)
    }
}

/// Encapsulates all colorspaces supported by
/// the library
pub static ALL_COLORSPACES: [ColorSpace; 9] = [
    ColorSpace::RGB,
    ColorSpace::RGBA,
    ColorSpace::LumaA,
    ColorSpace::Luma,
    ColorSpace::CMYK,
    ColorSpace::BGRA,
    ColorSpace::BGR,
    ColorSpace::YCCK,
    ColorSpace::YCbCr
];

/// Color characteristics
///
/// Gives more information about values in a certain
/// colorspace
#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ColorCharacteristics {
    /// Normal default gamma setting
    /// The float contains gamma present
    ///
    /// The default gamma value is 2.2 but for
    /// decoders that allow specifying gamma values,e.g PNG,
    /// the gamma value becomes the specified value by the decoder
    sRGB,
    /// Linear transfer characteristics
    /// The image is in linear colorspace
    Linear
}
