//! PAIR CACHE — what a producer made for one pair of frames, kept under a
//! key that says exactly which pair of which clip at which tier.
//!
//! It moved into `makepad-frametween` with the tweener that needs it; this
//! is the VJ's name for it, unchanged.

pub use makepad_frametween::pair_cache::*;
