//! HotRust's std: re-exports of HotRust's core/alloc at std's paths, plus the OS modules.
//! Module wiring here is the std-core lane's; std-os adds its modules only inside the
//! marked block below (coord.md "Ownership").

pub use core::any;
pub use core::array;
pub use core::cell;
pub use core::char;
pub use core::clone;
pub use core::cmp;
pub use core::convert;
pub use core::default;
pub use core::f32;
pub use core::f64;
pub use core::hash;
pub use core::hint;
pub use core::iter;
pub use core::marker;
pub use core::mem;
pub use core::ops;
pub use core::option;
pub use core::prelude;
pub use core::ptr;
pub use core::result;
pub use core::slice;
pub use core::str;
pub use core::i8;
pub use core::i16;
pub use core::i32;
pub use core::i64;
pub use core::i128;
pub use core::isize;
pub use core::u8;
pub use core::u16;
pub use core::u32;
pub use core::u64;
pub use core::u128;
pub use core::usize;

pub use core::borrow;
pub use core::error;
pub use core::pin;
pub use alloc::alloc;
pub use ::alloc::boxed;
pub use core::fmt;
pub use ::alloc::rc;
pub use ::alloc::string;
pub use ::alloc::vec;

pub mod collections {
    pub use core::collections::binary_heap;
    pub use core::collections::btree_map;
    pub use core::collections::btree_set;
    pub use core::collections::hash_map;
    pub use core::collections::hash_set;
    pub use core::collections::vec_deque;
    pub use core::collections::BTreeMap;
    pub use core::collections::BTreeSet;
    pub use core::collections::BinaryHeap;
    pub use core::collections::HashMap;
    pub use core::collections::HashSet;
    pub use core::collections::TryReserveError;
    pub use core::collections::VecDeque;
}

pub mod num {
    pub use core::num::FpCategory;
    pub use core::num::IntErrorKind;
    pub use core::num::NonZero;
    pub use core::num::NonZeroI128;
    pub use core::num::NonZeroI16;
    pub use core::num::NonZeroI32;
    pub use core::num::NonZeroI64;
    pub use core::num::NonZeroI8;
    pub use core::num::NonZeroIsize;
    pub use core::num::NonZeroU128;
    pub use core::num::NonZeroU16;
    pub use core::num::NonZeroU32;
    pub use core::num::NonZeroU64;
    pub use core::num::NonZeroU8;
    pub use core::num::NonZeroUsize;
    pub use core::num::ParseFloatError;
    pub use core::num::ParseIntError;
    pub use core::num::TryFromIntError;
    pub use core::num::Wrapping;
}

// ---- std-os modules ----
// (std-os lane: `pub mod <name>;` lines for std/std/<name>.rs or <name>/mod.rs go here)
pub mod env;
pub mod ffi;
pub mod fs;
pub mod io;
pub mod os;
pub mod panic;
pub mod path;
pub mod process;
pub mod sync;
pub(crate) mod sys;
pub mod thread;
pub mod time;
// ---- end std-os modules ----
