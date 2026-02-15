#![expect(
    missing_docs,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    clippy::all
)]
#![doc = include_str!("../readme.md")]
#![cfg_attr(all(not(feature = "std")), no_std)]

mod r#async;
mod bindings;

pub use bindings::*;
use r#async::*;
use windows_core::*;

#[cfg(feature = "std")]
mod future;
