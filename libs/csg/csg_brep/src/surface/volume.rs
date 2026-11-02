//! Volumes (`SmVolume` and its derived classes): maps of a 3d parameter space
//! into 3d space, used as volumetric shapes and as space deformations.
//!
//! Upstream: `source/SMLib/src/SmVolume.cpp`, `source/SMLib/inc/SmVolume.h`.
//! The file is split over the plan units `surface.volume_p1` and `_p2`; each lives
//! in its own submodule and adds methods to [`Volume`].

pub mod p1;
pub mod p2;

pub use p1::{DirectionDiscontinuities, Discontinuities3d, EvalOptions, Volume, VolumeKind};
pub use p2::InvPoints;
