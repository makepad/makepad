#![allow(unused_imports)]

pub use makepad_widgets;
pub use makepad_widgets::*;

// Keep a direct dependency edge in the dylib crate so Cargo/rustc build and link the
// wrapper around the widgets stack instead of optimizing the crate down to nothing.
use makepad_widgets as _;
