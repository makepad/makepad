#![doc = include_str!("../readme.md")]
#![cfg_attr(all(not(feature = "std")), no_std)]
#![expect(
    missing_docs,
    non_snake_case,
    non_camel_case_types,
    clippy::missing_transmute_annotations
)]

mod bindings;
pub use bindings::*;


