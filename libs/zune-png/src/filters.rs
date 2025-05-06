/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

#![allow(unused_variables)]
//! A set of optimized filter functions for de-filtering png
//! scanlines.
//!

use crate::enums::FilterMethod;

pub mod de_filter;
mod filter;
mod portable_simd;
mod sse4;

pub fn choose_compression_filter(_previous_row: &[u8], _current_row: &[u8]) -> FilterMethod {
    if _previous_row.is_empty() {
        // first row
        return FilterMethod::None;
    }
    FilterMethod::Up
}

pub fn filter_scanline(
    input: &[u8], previous_row: &[u8], output: &mut [u8], filter: FilterMethod, components: usize
) {
    let (filter_byte, filter_scanline) = output.split_at_mut(1);
    // add
    filter_byte[0] = filter.to_int();

    match filter {
        FilterMethod::None => filter_scanline.copy_from_slice(input),
        FilterMethod::Sub => filter::sub_filter(input, filter_scanline, components),
        FilterMethod::Up => filter::up_filter(input, previous_row, filter_scanline),

        _ => unreachable!("Unexpected input")
    }
}
