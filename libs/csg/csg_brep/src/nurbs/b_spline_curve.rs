//! B-spline (NURBS) curves as kernel geometry: the `SmBSplineCurve` class wrapping an
//! NLib curve, with its constructors, factories (lines, arcs, canonical data),
//! evaluation, editing and queries.
//!
//! Upstream: `source/SMLib/src/SmBSplineCurve.cpp`, `source/SMLib/inc/SmBSplineCurve.h`.
//! The file is split over several plan units (`nurbs.b_spline_curve_p1`, `_p2`, ...);
//! each lives in its own submodule and adds methods to [`BSplineCurve`].

pub mod p1;
pub mod p2;
pub mod p3;

pub use p1::{
    BSplineCurve, BSplineCurveForm, CanonicalData, Ends, KnotType, NurbCircleParam, ProjectionType,
    Tessellation,
};
pub use p2::{AnalyticCurve, BoundingBoxRequest, BoundingBoxes, ControlPointForm, CurveCanonical};
pub use p3::{GapFix, OnPlane, SegmentLine};
