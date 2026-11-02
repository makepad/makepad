//! B-spline (NURBS) surfaces as kernel geometry: the `SmBSplineSurface` class wrapping
//! an NLib surface, with its constructors, factories (bilinear, canonical, cone patch,
//! linear sweep), queries, transformation and database reading.
//!
//! Upstream: `source/SMLib/src/SmBSplineSurface.cpp`, `source/SMLib/inc/SmBSplineSurface.h`.
//! The file is split over several plan units (`nurbs.b_spline_surface_p1`, `_p2`, ...);
//! each lives in its own submodule and adds methods to [`BSplineSurface`].

pub mod p1;
pub mod p2;

pub use p1::{
    BSplineSurface, BSplineSurfaceForm, BoundingBoxRequest, BoundingBoxes, CanonicalSurfaceData,
    ControlPointNet, LocationType, SurfParamType, SurfaceCanonical,
};
pub use p2::{
    ControlPointBlock, IsoCurveOptions, IsoParametricCurve, SphereFromArcs, TrimmedPlane,
};
