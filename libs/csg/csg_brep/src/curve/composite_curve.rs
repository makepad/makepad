//! Composite curves (`SmCompositeCurve`): a 2D or 3D curve made of end-to-end
//! connected (within some tolerance) curve segments, the segment record
//! (`SmCompositeCurveSegment`), regions bounded by composite curves
//! (`SmCompositeCurveRegion`) and the offset bookkeeping attribute
//! (`SmOffsetMapAttribute`).
//!
//! Upstream: `source/SMLib/src/SmCompositeCurve.cpp`, `source/SMLib/inc/SmCompositeCurve.h`.
//! The file is split over several plan units (`curve.composite_curve_p1`, `_p2`); each
//! lives in its own submodule and adds methods to [`CompositeCurve`].

pub mod p1;
pub mod p2;

pub use p1::{
    BuiltComposites, CompositeApproximation, CompositeCanonical, CompositeCurve,
    CompositeCurveSegment, CornerCurves, OffsetCornerType, OffsetMapAttribute,
};
pub use p2::{OffsetDirection, OffsetParams, OffsetPiece, TrimmedOffsets};
