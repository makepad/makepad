//! Surfaces of extrusion (`SmSurfOfExtrusion`): a generating curve swept along an
//! extrusion vector, kept together with its B-spline surface.
//!
//! Upstream: `source/SMLib/src/SmSurfOfExtrusion.cpp`,
//! `source/SMLib/inc/SmSurfOfExtrusion.h`. The file is split over two plan units
//! (`surface.surf_of_extrusion_p1`, `_p2`); each lives in its own submodule and adds
//! methods to [`SurfOfExtrusion`] (or free functions).

pub mod p1;
pub mod p2;

pub use p1::{CurveOrientType, GeneratorIntersection, SurfOfExtrusion};
pub use p2::{ExtrusionIntersection, IntersectTarget};
