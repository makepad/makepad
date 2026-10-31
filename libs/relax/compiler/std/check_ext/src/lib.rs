//! Mounts Relax's built-in crate sources (as rustc sees them, against real core/alloc).
#![allow(dead_code)]
#[path = "../../smallvec/lib.rs"]
pub mod hr_smallvec;
#[path = "../../arrayvec/lib.rs"]
pub mod hr_arrayvec;
