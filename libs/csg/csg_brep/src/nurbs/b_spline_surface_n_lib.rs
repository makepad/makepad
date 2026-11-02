//! `SmBSplineSurface` methods implemented with NLib (`SmBSplineSurfaceNLib.cpp`): offset
//! approximation, Coons, n-sided, ruled, sweep and swung surface construction, surface
//! extension and joining, reversal, reparametrisation, corner blends, skinning, trimming
//! to a domain and evaluation.
//!
//! Upstream: `source/SMLib/src/SmBSplineSurfaceNLib.cpp`, `source/SMLib/inc/SmBSplineSurface.h`.
//! The file is split over several plan units (`nurbs.b_spline_surface_n_lib_p1`, `_p2`);
//! each lives in its own submodule and adds methods to
//! [`BSplineSurface`](crate::nurbs::b_spline_surface::BSplineSurface).

pub mod approx;
pub mod p1;
pub mod p2;
