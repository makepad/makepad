//! Shim `crate::hash` (collections area): real std's Hash/Hasher traits, so test derives
//! work, plus Relax's default hasher from core/hash/fast.rs (DefaultHasher::new's SipHash-1-3 from sip.rs)
//! and RandomState's keys (random.rs).
pub use std::hash::{BuildHasher, BuildHasherDefault, Hash, Hasher};
#[path = "../../core/hash/fast.rs"]
mod fast;
#[path = "../../core/hash/random.rs"]
mod random;
#[path = "../../core/hash/sip.rs"]
mod sip;
pub use fast::{DefaultHasher, FastHasher, RandomState};
pub use sip::SipHasher;
