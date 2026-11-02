//! The surface/surface intersector `SmSurfaceIntersector` and its intersection point
//! `SmTsectPnt`.
//!
//! Upstream: `source/SMLib/src/SmSurfaceIntersector.cpp`,
//! `source/SMLib/inc/SmSurfaceIntersector.h`. The file is split over several plan
//! units (`intersect.surface_intersector_p1`, `_p2`, ...); each lives in its own
//! submodule and adds methods to [`SurfaceIntersector`] (or free functions).

pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
pub mod p5;
pub(crate) mod tangent_curves;
pub(crate) mod tangent_tracer;

pub use p1::{IntersectionPointType, SrfIndex, SurfaceIntersector, TsectId, TsectPnt};
