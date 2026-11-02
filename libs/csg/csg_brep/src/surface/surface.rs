//! The surface base class `SmSurface`: the [`Surface`] object (a concrete surface
//! kind plus the owner pointer), its virtual interface dispatched over the surface
//! kinds, and the numerical methods implemented once on the base class.
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`, `source/SMLib/inc/SmSurface.h`.
//! The file is split over several plan units (`surface.surface_p1`, `_p2`, ...);
//! each lives in its own submodule and adds methods to [`Surface`] (or free
//! functions).

pub mod approximate;
pub mod assert_valid;
pub mod p1;
pub mod p10;
pub mod p11;
pub mod p2;
pub mod p3;
pub mod p4;
pub mod p5;
pub mod p6;
pub mod p7;
pub mod p8;
pub mod p9;

pub use p1::StepSurface;
pub use p2::{CacheTreeState, Surface, SurfaceKind, TrimCacheSlot};
