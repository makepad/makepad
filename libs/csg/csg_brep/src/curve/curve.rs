//! The curve base class `SmCurve`: the [`Curve`] object (a concrete curve kind plus
//! the owner pointer), its virtual interface dispatched over the curve kinds, and the
//! numerical methods implemented once on the base class.
//!
//! Upstream: `source/SMLib/src/SmCurve.cpp`, `source/SMLib/inc/SmCurve.h`.
//! The file is split over several plan units (`curve.curve_p1`, `_p2`, ...); each
//! lives in its own submodule and adds methods to [`Curve`] (or generic functions
//! over [`CurveEvaluate`]).

pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
pub mod p5;
pub mod p6;
pub mod p7;
pub mod p8;
pub mod p9;

pub use p2::{
    knots_ignoring_status, Curve, CurveEvaluate, CurveKind, CurvePropertyType, KnotsWithStatus,
    LineIntersection, OwnerChange, StepOff, TessStepResult,
};
