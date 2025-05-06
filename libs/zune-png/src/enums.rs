/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

#![allow(dead_code, unused_must_use)]
#![allow(clippy::upper_case_acronyms, non_camel_case_types)]

/// Chunk type according to table 5.3 of
/// the jpeg spec, see https://www.w3.org/TR/2003/REC-PNG-20031110/
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum PngChunkType {
    IHDR,
    PLTE,
    IDAT,
    IEND,
    eXIf,
    cHRM,
    gAMA,
    iCCP,
    sBit,
    sRGB,
    bKGD,
    hIST,
    tRNS,
    pHYs,
    sPLT,
    tIME,
    iTXt,
    tEXt,
    zTXt,
    fcTL,
    acTL,
    fdAT,
    unkn
}

impl PngChunkType {
    /// Return true if a chunk should appear
    /// before the PLTE chunk
    pub const fn should_appear_before_ptle(self) -> bool {
        matches!(
            self,
            Self::cHRM | Self::gAMA | Self::iCCP | Self::sBit | Self::sRGB
        )
    }
    /// Return true if a chunk should appear
    /// after the PLTE chunk
    pub const fn should_appear_after_ptle(self) -> bool {
        matches!(self, Self::bKGD | Self::hIST | Self::tRNS)
    }

    /// Return true if a chunk should appear
    /// before the IDAT chunk
    pub const fn should_appear_before_idat(self) -> bool {
        matches!(
            self,
            Self::PLTE
                | Self::cHRM
                | Self::gAMA
                | Self::iCCP
                | Self::sBit
                | Self::sRGB
                | Self::bKGD
                | Self::hIST
                | Self::tRNS
                | Self::pHYs
                | Self::sPLT
        )
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FilterMethod {
    None,
    Sub,
    Up,
    Average,
    Paeth,
    // First scanline, special
    PaethFirst,
    AvgFirst,
    // Unknown type of filter
    Unknown
}

impl FilterMethod {
    pub fn from_int(int: u8) -> Option<FilterMethod> {
        match int {
            0 => Some(FilterMethod::None),
            1 => Some(FilterMethod::Sub),
            2 => Some(FilterMethod::Up),
            3 => Some(FilterMethod::Average),
            4 => Some(FilterMethod::Paeth),
            _ => None
        }
    }
    pub fn to_int(self) -> u8 {
        match self {
            FilterMethod::None => 0,
            FilterMethod::Sub => 1,
            FilterMethod::Up => 2,
            FilterMethod::Average => 3,
            FilterMethod::Paeth => 4,
            _ => unimplemented!("Should not match")
        }
    }
}
#[allow(clippy::derivable_impls)]
impl Default for FilterMethod {
    fn default() -> Self {
        FilterMethod::Unknown
    }
}

/// The image interlace method
///
/// Default is `Unknown` but should change it to
/// the image interlace when headers are decoded
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum InterlaceMethod {
    /// Standard/ No interlacing
    Standard,
    /// Adam7 interlacing
    Adam7,
    /// The interlacing of the image is currently unknown
    Unknown
}

impl Default for InterlaceMethod {
    fn default() -> Self {
        Self::Unknown
    }
}

impl InterlaceMethod {
    pub fn from_int(int: u8) -> Option<InterlaceMethod> {
        match int {
            0 => Some(Self::Standard),
            1 => Some(Self::Adam7),
            _ => None
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum PngColor {
    Luma,
    Palette,
    LumaA,
    RGB,
    RGBA,
    Unknown
}

impl Default for PngColor {
    fn default() -> Self {
        Self::Unknown
    }
}

impl PngColor {
    pub(crate) fn num_components(self) -> u8 {
        match self {
            PngColor::Luma => 1,
            PngColor::Palette => 1,
            PngColor::LumaA => 2,
            PngColor::RGB => 3,
            PngColor::RGBA => 4,
            PngColor::Unknown => unreachable!()
        }
    }
    pub(crate) fn has_alpha(&self) -> bool {
        matches!(self, PngColor::RGBA | PngColor::LumaA)
    }
    pub(crate) fn from_int(int: u8) -> Option<PngColor> {
        match int {
            0 => Some(Self::Luma),
            2 => Some(Self::RGB),
            3 => Some(Self::Palette),
            4 => Some(Self::LumaA),
            6 => Some(Self::RGBA),
            _ => None
        }
    }
}
