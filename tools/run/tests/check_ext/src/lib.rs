//! Mounts Relax's built-in crate sources (as rustc sees them, against real core/alloc).
#![allow(dead_code)]
#[path = "../../../../../libs/relax/compiler/std/smallvec/lib.rs"]
pub mod hr_smallvec;
#[path = "../../../../../libs/relax/compiler/std/arrayvec/lib.rs"]
pub mod hr_arrayvec;
