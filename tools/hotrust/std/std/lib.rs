//! HotRust's std: HotRust's core under the std/alloc paths makepad uses.

pub use core::f32;
pub use core::f64;
pub use core::ops;
pub use core::prelude;
pub use core::mem;
pub use core::clone;
pub use core::cmp;
pub use core::default;
pub use core::marker;
pub use core::option;
pub use core::boxed;
pub use core::vec;
pub use core::string;
pub mod result {
    pub use core::option::Result;
}
