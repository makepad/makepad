//! Generic multi-view geometry with rigid cameras and grayscale images.
//! Deterministic host references for plane-based PatchMatch stereo, multi-view depth fusion and
//! photometric pose refinement; the refinement spreads over the caller's threads with the same result at any
//! thread count.

pub mod views;
pub mod patchmatch;
pub mod program;
pub mod fusion;
pub mod alignment;
pub mod parallel;

pub mod synthetic;
#[cfg(feature = "cuda")]
pub mod cuda;
