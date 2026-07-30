extern crate alloc;

#[cfg(feature = "core")]
pub mod adapter;
#[cfg(feature = "core")]
pub mod int;
#[cfg(feature = "core")]
pub mod triangle;

#[cfg(feature = "float_pt")]
pub mod float;

pub mod integration;
