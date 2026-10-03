//! Shim `crate::hash` (collections area): real std's Hash/Hasher traits, so test derives
//! work, plus Rapid's default hasher from core/hash/fast.rs.
pub use std::hash::{BuildHasher, BuildHasherDefault, Hash, Hasher};
#[path = "../../core/hash/fast.rs"]
mod fast;
pub use fast::{DefaultHasher, FastHasher, RandomState};
