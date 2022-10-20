#![cfg_attr(feature = "nightly", feature(portable_simd))]
// image_formats
// by Desmond Germans, 2019

mod image;
pub use image::*;

pub mod bmp;
pub mod png;
pub mod jpeg;

