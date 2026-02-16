/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */

//! Filtering algorithms for png encoder

pub fn sub_filter(input: &[u8], output: &mut [u8], components: usize) {
    // copy leftmost byte from input to output
    output[..components].copy_from_slice(&input[..components]);

    let end = input.len().min(output.len());

    for i in components..end {
        let a = input[i - components];
        output[i] = input[i].wrapping_sub(a);
    }
}

pub fn up_filter(input: &[u8], up: &[u8], output: &mut [u8]) {
    debug_assert_eq!(input.len(), up.len());
    debug_assert_eq!(up.len(), output.len());

    for ((in_, up), x) in input.iter().zip(up).zip(output) {
        *x = (*in_).wrapping_sub(*up)
    }
}
