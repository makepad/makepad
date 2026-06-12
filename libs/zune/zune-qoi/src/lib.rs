/*
 * Copyright (c) 2023.
 *
 * This software is free software; You can redistribute it or modify it under terms of the MIT, Apache License or Zlib license
 */
//! Decoding and encoding Quite Ok Image format
//!
//! [Format Specification](https://qoiformat.org/qoi-specification.pdf)
//!
//!
//! # Features
//! - Decoding and encoding
//! - `no_std`
//! - Fast
//! - Fuzz tested
//!
//! ## `no_std`
//! You can use `no_std` with alloc feature to compile for `no_std` endpoints

#![cfg_attr(not(feature = "std"), no_std)]
#![macro_use]
extern crate alloc;
extern crate core;

pub use decoder::*;
pub use encoder::*;
pub use errors::*;
pub use makepad_zune_core;
mod constants;
mod decoder;
mod encoder;
mod errors;
