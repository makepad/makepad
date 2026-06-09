/*
 * Copyright (c) 2023.
 *
 * This software is free software;
 *
 * You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

use makepad_zune_core::colorspace::ColorSpace;

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
#[allow(clippy::upper_case_acronyms)]
pub enum BmpCompression {
    RGB,
    RLE8,
    RLE4,
    BITFIELDS,
    Unknown
}

impl BmpCompression {
    pub fn from_u32(num: u32) -> Option<BmpCompression> {
        match num {
            0 => Some(BmpCompression::RGB),
            1 => Some(BmpCompression::RLE8),
            2 => Some(BmpCompression::RLE4),
            3 => Some(BmpCompression::BITFIELDS),
            _ => None
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[allow(clippy::upper_case_acronyms)]
pub enum BmpPixelFormat {
    None,
    RGBA,
    PAL8,
    GRAY8,
    RGB
}

impl BmpPixelFormat {
    pub fn num_components(&self) -> usize {
        match self {
            BmpPixelFormat::None => 0,
            BmpPixelFormat::RGBA => 4,
            BmpPixelFormat::PAL8 => 3,
            BmpPixelFormat::GRAY8 => 1,
            BmpPixelFormat::RGB => 3
        }
    }
    pub fn into_colorspace(self) -> ColorSpace {
        match self {
            BmpPixelFormat::None => ColorSpace::Unknown,
            BmpPixelFormat::RGBA => ColorSpace::RGBA,
            BmpPixelFormat::PAL8 => ColorSpace::RGB,
            BmpPixelFormat::GRAY8 => ColorSpace::Luma,
            BmpPixelFormat::RGB => ColorSpace::RGB
        }
    }
}

/// This value indicates that bV5ProfileData points to the file name of the profile to use (gamma and endpoints values are ignored).
pub(crate) const PROFILE_LINKED: u32 = 0x4c494e4b;

/// This value indicates that bV5ProfileData points to a memory buffer that contains the profile to be used (gamma and endpoints values are ignored).
pub(crate) const PROFILE_EMBEDDED: u32 = 0x4D424544;
