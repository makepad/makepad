//! `SmCompositeCurve`, part 1: the composite curve object ([`CompositeCurve`]) with
//! its segment records ([`CompositeCurveSegment`]), construction, copying, equality,
//! the curve virtuals it overrides (natural interval, knots, bounding boxes,
//! approximation by one B-spline, transformation), connecting unordered curves into
//! composites, the projected orientation of a loop of curves, the offset corner
//! builder and classifier and the sorted break-list helpers used by the offsetters
//! (unit `curve.composite_curve_p2`), the type queries of
//! `SmCompositeCurveRegion`, and the offset bookkeeping attribute
//! ([`OffsetMapAttribute`]).
//!
//! Upstream: `source/SMLib/src/SmCompositeCurve.cpp`, `source/SMLib/inc/SmCompositeCurve.h`.
//!
//! | Rust                                                   | Upstream                                              |
//! |--------------------------------------------------------|-------------------------------------------------------|
//! | [`OffsetMapAttribute`], [`OffsetMapAttribute::new`], `Clone` | `SmOffsetMapAttribute` (constructor, copy constructor, `~`) |
//! | [`OffsetMapAttribute::corner_offset`], [`OffsetMapAttribute::curve`], [`OffsetMapAttribute::other_curve`], [`OffsetMapAttribute::orig_curve`] | `GetCornerOffset`, `GetCurve`, `GetOtherCurve`, `GetOrigCurve` |
//! | [`OFFSET_MAP_ATTRIBUTE_TYPE`], [`OffsetMapAttribute::type_string`], [`OffsetMapAttribute::is_kind_of`] | `GetType`, `GetTypeString`, `IsKindOf` (`SM_COMMON`) |
//! | [`CompositeCurveSegment`], `Drop` (implicit)           | `SmCompositeCurveSegment` (data, `~`)                 |
//! | [`CompositeCurveSegment::approx_eq`]                   | `SmCompositeCurveSegment::operator==`                 |
//! | [`CompositeCurveSegment::curve`]                       | `SmCompositeCurveSegment::GetCurve`                   |
//! | [`CompositeCurveSegment::is_kind_of`]                  | `SmCompositeCurveSegment::IsKindOf` (`SM_COMMON_ONLY_BASE`) |
//! | [`REGION_TYPE`], [`REGION_TYPE_STRING`], [`REGION_CLASS_STRING`], [`region_is_kind_of`] | `SmCompositeCurveRegion::GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`CompositeCurve`], [`CompositeCurve::new`]            | `SmCompositeCurve`, `SmCompositeCurve(ULONG)`         |
//! | [`CompositeCurve::with_curves`]                        | `SmCompositeCurve(lDim, crCurves, bIsClosed, cpSenses, cpGaps)` |
//! | [`CompositeCurve::copy`]                               | copy constructor, `Copy`                              |
//! | `Drop` (implicit)                                      | `~SmCompositeCurve`                                   |
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`], [`is_kind_of`] | `GetType`, `GetTypeString`, `GetClassString`, `IsKindOf` |
//! | [`CompositeCurve::approx_eq`]                          | `operator==`                                          |
//! | [`CompositeCurve::segment`], [`CompositeCurve::num_segments`] | `GetCurveSegment`, `GetNumSegments`            |
//! | [`CompositeCurve::is_closed`]                          | `IsClosed`                                            |
//! | [`CompositeCurve::canonical`], [`CompositeCanonical`]  | `GetCanonical`                                        |
//! | [`CompositeCurve::natural_interval`]                   | `GetNaturalInterval`                                  |
//! | [`CompositeCurve::knots`]                              | `GetKnots`                                            |
//! | [`CompositeCurve::calculate_bounding_box`]             | `CalculateBoundingBox`                                |
//! | [`CompositeCurve::approximate_curve`], [`CompositeApproximation`] | `ApproximateCurve`                         |
//! | [`CompositeCurve::transform`]                          | `Transform`                                           |
//! | [`CompositeCurve::compute_projected_loop_orientation`] | `ComputeProjectedLoopOrientation` (static)            |
//! | [`CompositeCurve::build_composites_from_curves`], [`BuiltComposites`] | `BuildCompositesFromCurves` (static)   |
//! | `CurveItem` (private)                                  | `SmCurveItem` (constructor)                           |
//! | `flush_crv` (private)                                  | `sm_FlushCrv` (static)                                |
//! | [`convert_to_bspline_curves`]                          | `sm_convert_to_BSplineCurves` (static)                |
//! | [`create_corner`], [`CornerCurves`], [`OffsetCornerType`] | `sm_CreateCorner` (static), `SmOffsetCornerType`   |
//! | [`curve_classify`]                                     | `sm_CurveClassify` (static)                           |
//! | [`trim_first_break`], [`trim_last_break`], [`insert_sorted_breaks`] | `sm_TrimFirstBreak`, `sm_TrimLastBreak`, `sm_InsertSortedBreaks` (static) |
//!
//! Deviations:
//! - **Nesting.** `with_curves` and `set_segment` return `InvalidInput` when any segment curve would nest
//!   deeper than [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING)
//!   (32; upstream fixtures nest at most 3), which bounds the recursive dispatch,
//!   cloning and dropping of nested geometry (no limit upstream).
//! - **Object model.** `SmCompositeCurve` derives from `SmCurve`; here it is the
//!   [`CurveKind::Composite`] variant of [`Curve`], whose dispatch calls the
//!   overrides ported here and runs the base-class defaults for the rest (a
//!   composite does not evaluate: `Evaluate` and `EvaluatePoint` are `SM_ERR`). The
//!   segments own their curves (`~SmCompositeCurve` deletes every segment curve);
//!   `m_bOwnsCurve` is kept as a flag. A composite made by
//!   `SmCompositeCurve(ULONG lDim)` has `m_lNumSegments = SM_UNDEF_ULONG` and no
//!   segment array (every method then reads through `NULL`); [`CompositeCurve::new`]
//!   has no segments instead.
//! - **Copies.** The copy constructor ignores the status of each segment's `Copy`
//!   and leaves a `NULL` segment curve when it fails (a Hermite curve has no `Copy`),
//!   which every later use dereferences; [`CompositeCurve::copy`] returns the error.
//!   `Clone` copies the segment curves as Rust values. `SetContext` has no
//!   counterpart (no per-object contexts).
//! - **Arguments.** Out-params become return values; `SmContext` arguments only
//!   choose where upstream allocates and are dropped, as are arguments upstream does
//!   not use (`ApproximateCurve`'s breaks and flags, the five unused tolerances of
//!   `BuildCompositesFromCurves`). `GetKnots` always computes the multiplicities and
//!   `CalculateBoundingBox` always the axis aligned box (upstream writes
//!   `*pBBox` unconditionally: a request without it is [`SolidError::InvalidInput`]).
//!   `ComputeProjectedLoopOrientation` adds to the caller's 2D box, as upstream.
//!   `BuildCompositesFromCurves` takes its curves by value: the composites own the
//!   curves they use, and the curves left out (too short) are handed back
//!   ([`BuiltComposites::unused`]).
//! - **Attributes.** A [`Curve`] carries no attribute list. `sm_CurveClassify`'s
//!   `FindAttribute(SM_AI_OFFSETMAP)` / `GetOrigCurve` becomes the caller-supplied
//!   originator curve, and `sm_convert_to_BSplineCurves`' transfer of the offset map
//!   attribute to the approximation is left to the caller. [`OffsetMapAttribute`]
//!   names its originator by [`ObjectRef`]; its `MakeCopy` and `GetMemoryUsed` belong
//!   to other units (`Clone` is the copy constructor).
//! - **Undefined behaviour upstream.** These return [`SolidError::InvalidInput`]
//!   where upstream reads or writes out of bounds: `GetKnots` of two or more segments
//!   (it reads `rKnots[GetSize()]`, one past the end, for the first transition) and
//!   any `GetKnots` with an interval (its unsigned loop `ii >= 0` never ends and
//!   reads `rKnots[ULONG_MAX]`); senses or gaps shorter than the curves and gaps for a
//!   closed composite without curves; a loop polygon without points in
//!   `ComputeProjectedLoopOrientation`; fewer senses than curves there; a chamfer
//!   corner whose trimming intersections are missing (`sSolutions[0]` of an empty
//!   array). `ApproximateCurve` dereferences the joined curve even when
//!   `CreateByJoining` failed and made none (a crash); the port returns the join's
//!   error. Approximations that are analytic curves derived from
//!   `SmBSplineCurve` (lines, ellipses, ...) join through their B-spline part.
//! - **Statuses ignored upstream.** The segments' `CalculateBoundingBox` (a failed
//!   one leaves the previous segment's boxes in place), `Transform`, `GetEnds`, the
//!   chamfer's `GlobalCurveIntersect` and `ComputeCrvsNormal`, the corner's
//!   `SetCanonical`, `Unitize` and the classifier's evaluations: the port ignores
//!   the error too, except [`SolidError::Unported`], which is always propagated.
//! - **`sm_FlushCrv`** is only reachable from `BuildCompositesFromCurves` and is
//!   ported with it (the call graph lists it as unused because another file has a
//!   static function of the same name). The linked list of `SmCurveItem`s is a
//!   vector with `next` indices.
//! - `Dump`, `Draw`, `DrawParams`, `GetMemoryUsed`, `WriteToDB`, `ReadFromDB`,
//!   `AddCurve`, `MakeCompositeNurb` and the offsetters belong to other units or are
//!   unused.

use crate::curve::curve::p1::ApproximateCurveOptions;
use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::{
    at, Curve, CurveEvaluate, CurveKind, COMPOSITE_CURVE_TYPE, CURV_BASE_TYPE,
};
use crate::curve::curve::p3::compute_crvs_normal;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve_cache::CurveCache;
use crate::error::{ignored, Result, SolidError};
use crate::geom::a_object;
use crate::geom::assert_array::ObjectKey;
use crate::geom::attribute;
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{
    line_closest_point, line_line_closest_point, vector_project_to_plane,
};
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::{self, ObjectRef};
use crate::geom::polar_box::PolarBox;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::tol::{are_same, is_zero};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, PI};
use crate::nurbs::b_spline_curve::{
    BSplineCurve, BoundingBoxRequest, BoundingBoxes, NurbCircleParam,
};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::p2::{check_geometry_nesting, NestedGeometry};

const OP: &str = "curve::composite_curve::p1::CompositeCurve";
const OP_NEW: &str = "curve::composite_curve::p1::CompositeCurve::with_curves";
const OP_KNOTS: &str = "curve::composite_curve::p1::CompositeCurve::knots";
const OP_BOX: &str = "curve::composite_curve::p1::CompositeCurve::calculate_bounding_box";
const OP_APPROX: &str = "curve::composite_curve::p1::CompositeCurve::approximate_curve";
const OP_ORIENT: &str =
    "curve::composite_curve::p1::CompositeCurve::compute_projected_loop_orientation";
const OP_BUILD: &str = "curve::composite_curve::p1::CompositeCurve::build_composites_from_curves";
const OP_CORNER: &str = "curve::composite_curve::p1::create_corner";

// ---- type codes -----------------------------------------------------------------------

/// `SmCompositeCurve_TYPE` (`CURV_BASE_TYPE + 32`).
pub const TYPE: i64 = COMPOSITE_CURVE_TYPE;
/// `SmCompositeCurve::GetTypeString`.
pub const TYPE_STRING: &str = "SmCompositeCurve_TYPE";
/// `SmCompositeCurve::GetClassString`.
pub const CLASS_STRING: &str = "SmCompositeCurve";

/// `SmCompositeCurveRegion_TYPE` (`CURV_BASE_TYPE + 38`).
pub const REGION_TYPE: i64 = CURV_BASE_TYPE + 38;
/// `SmCompositeCurveRegion::GetTypeString`.
pub const REGION_TYPE_STRING: &str = "SmCompositeCurveRegion_TYPE";
/// `SmCompositeCurveRegion::GetClassString`.
pub const REGION_CLASS_STRING: &str = "SmCompositeCurveRegion";

/// `SmCompositeCurveSegment_TYPE` (`CURV_BASE_TYPE + 40`).
pub const SEGMENT_TYPE: i64 = CURV_BASE_TYPE + 40;
/// `SmCompositeCurveSegment::GetTypeString`.
pub const SEGMENT_TYPE_STRING: &str = "SmCompositeCurveSegment_TYPE";
/// `SmCompositeCurveSegment::GetClassString`.
pub const SEGMENT_CLASS_STRING: &str = "SmCompositeCurveSegment";

/// `SmOffsetMapAttribute_TYPE` (`CURV_BASE_TYPE + 200`).
pub const OFFSET_MAP_ATTRIBUTE_TYPE: i64 = CURV_BASE_TYPE + 200;

/// `SmCompositeCurve::IsKindOf`: `SmCompositeCurve_TYPE` or a base type of
/// `SmCurve`.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || crate::curve::curve::p2::is_kind_of(t)
}

/// `SmCompositeCurveRegion::IsKindOf`: `SmCompositeCurveRegion_TYPE` or a base type
/// of `SmAObject`.
#[must_use]
pub const fn region_is_kind_of(t: i64) -> bool {
    t == REGION_TYPE || a_object::is_kind_of(t)
}

// ---- SmOffsetMapAttribute -------------------------------------------------------------

/// The attribute recording where an offset curve came from (`SmOffsetMapAttribute`,
/// attribute id `SM_AI_OFFSETMAP`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OffsetMapAttribute {
    /// `SmAttribute::m_lAttributeID`.
    pub attribute_id: u64,
    /// `m_bCornerOffset`: the curve is a corner (fills the gap between two offsets).
    pub corner_offset: bool,
    /// `m_bLeftHandOffset`: the curve is a left hand offset.
    pub left_hand_offset: bool,
    /// `m_lCompositeIndex`: index of the originating composite curve.
    pub composite_index: u64,
    /// `m_lCurve`: index of the originating curve in its composite.
    pub curve: u64,
    /// `m_lOtherCurve`: for a corner, the index of the other curve.
    pub other_curve: u64,
    /// `m_pOriginator`: the curve that was offset.
    pub originator: Option<ObjectRef>,
    /// `m_dApproxErr`: largest error of the offset approximation.
    pub approx_err: f64,
}

impl OffsetMapAttribute {
    /// `SmOffsetMapAttribute(lAttributeID, bCornerOffset, bLeftHandOffset,
    /// lCompositeIndex, lCurve, lOtherCurve, pOrigCurve, dApproxErr)`.
    #[must_use]
    pub const fn new(
        attribute_id: u64,
        corner_offset: bool,
        left_hand_offset: bool,
        composite_index: u64,
        curve: u64,
        other_curve: u64,
        originator: Option<ObjectRef>,
        approx_err: f64,
    ) -> Self {
        Self {
            attribute_id,
            corner_offset,
            left_hand_offset,
            composite_index,
            curve,
            other_curve,
            originator,
            approx_err,
        }
    }

    /// `GetCornerOffset`.
    #[must_use]
    pub const fn corner_offset(&self) -> bool {
        self.corner_offset
    }

    /// `GetCurve`.
    #[must_use]
    pub const fn curve(&self) -> u64 {
        self.curve
    }

    /// `GetOtherCurve`.
    #[must_use]
    pub const fn other_curve(&self) -> u64 {
        self.other_curve
    }

    /// `GetOrigCurve`.
    #[must_use]
    pub const fn orig_curve(&self) -> Option<ObjectRef> {
        self.originator
    }

    /// `GetType`.
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        OFFSET_MAP_ATTRIBUTE_TYPE
    }

    /// `GetTypeString`.
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmOffsetMapAttribute_TYPE"
    }

    /// `IsKindOf`: `SmOffsetMapAttribute_TYPE` or a base type of `SmAttribute`.
    #[must_use]
    pub fn is_kind_of(&self, t: i64) -> bool {
        t == OFFSET_MAP_ATTRIBUTE_TYPE
            || t == attribute::type_code::ATTRIBUTE
            || object::is_kind_of(t)
    }
}

// ---- SmCompositeCurveSegment ----------------------------------------------------------

/// One segment of a composite curve (`SmCompositeCurveSegment`).
#[derive(Debug, Clone)]
pub struct CompositeCurveSegment {
    /// `m_eTransition`: continuity at the end of this segment (`SM_CT_C0` as built).
    pub transition: ContinuityType,
    /// `m_bSameSense`: `true` takes the curve as is, `false` reversed.
    pub same_sense: bool,
    /// `m_pParentCurve`: the curve defining the segment's shape.
    pub curve: Curve,
    /// `m_dGapStart`: gap from this curve's start to the previous curve's end.
    pub gap_start: f64,
    /// `m_dGapEnd`: gap from this curve's end to the next curve's start.
    pub gap_end: f64,
    /// `m_bOwnsCurve` (the composite owns its segment curves whatever the flag says).
    pub owns_curve: bool,
}

impl CompositeCurveSegment {
    /// `GetCurve`.
    #[must_use]
    pub const fn curve(&self) -> &Curve {
        &self.curve
    }

    /// `IsKindOf` (`SM_COMMON_ONLY_BASE`): only `SmCompositeCurveSegment_TYPE`.
    #[must_use]
    pub const fn is_kind_of(t: i64) -> bool {
        t == SEGMENT_TYPE
    }

    /// `operator==`: same transition and sense, equal curves and gaps equal within
    /// `SM_IS_ZERO`.
    ///
    /// # Errors
    ///
    /// Those of [`Curve::approx_eq`] (only reached when transition and sense
    /// agree).
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }
        Ok(self.transition == other.transition
            && self.same_sense == other.same_sense
            && self.curve.approx_eq(k, &other.curve)?
            && is_zero(self.gap_start - other.gap_start)
            && is_zero(self.gap_end - other.gap_end))
    }
}

// ---- SmCompositeCurve -----------------------------------------------------------------

/// A curve made of end-to-end connected curve segments (`SmCompositeCurve`). Its
/// parameter range stacks the segments' natural intervals end to end, starting at
/// the first segment's.
#[derive(Debug, Clone)]
pub struct CompositeCurve {
    pub(super) dim: u64,
    pub(super) segments: Vec<CompositeCurveSegment>,
    pub(super) closed: bool,
}

impl CompositeCurve {
    /// Exact evaluation copy after the caller validates the geometry depth.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        Self {
            dim: self.dim,
            segments: self
                .segments
                .iter()
                .map(|s| CompositeCurveSegment {
                    transition: s.transition,
                    same_sense: s.same_sense,
                    curve: s.curve.clone_exact_bounded(),
                    gap_start: s.gap_start,
                    gap_end: s.gap_end,
                    owns_curve: s.owns_curve,
                })
                .collect(),
            closed: self.closed,
        }
    }
}

/// [`CompositeCurve::canonical`]: the data the composite was built from
/// (`GetCanonical`).
#[derive(Debug, Clone)]
pub struct CompositeCanonical<'a> {
    /// `rlDimension`.
    pub dim: u64,
    /// `rCurves`: the segment curves (not copies).
    pub curves: Vec<&'a Curve>,
    /// `rbClosed`.
    pub closed: bool,
    /// `pSenses`: the sense of each curve.
    pub senses: Vec<bool>,
}

/// Result of [`CompositeCurve::approximate_curve`].
#[derive(Debug, Clone)]
pub struct CompositeApproximation {
    /// `rpNewBSplineCurve`: the joined approximation, `None` when the join is not a
    /// valid curve (C0 in its interior).
    pub curve: Option<BSplineCurve>,
    /// `rdMaxGap3d`: the largest interior end gap or segment approximation error.
    pub max_gap: f64,
}

/// Result of [`CompositeCurve::build_composites_from_curves`].
#[derive(Debug, Clone)]
pub struct BuiltComposites {
    /// `rComposites`: the composite curves, in the order they were closed off.
    pub composites: Vec<CompositeCurve>,
    /// The input curves no composite uses (shorter than the same point tolerance;
    /// upstream leaves them with the caller), in input order.
    pub unused: Vec<Curve>,
}

impl CurveEvaluate for CompositeCurve {
    fn dim(&self) -> u64 {
        self.dim
    }

    fn natural_interval(&self) -> Extent1d {
        Self::natural_interval(self)
    }

    fn evaluate(
        &self,
        _k: &Kernel,
        _t: f64,
        _num_derivs: usize,
        _from_left: bool,
        _non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        // SmCurve::Evaluate (not overridden)
        Err(SolidError::sm_err(OP))
    }

    fn evaluate_point(&self, _k: &Kernel, _t: f64) -> Result<Point3> {
        // SmCurve::EvaluatePoint (not overridden)
        Err(SolidError::sm_err(OP))
    }

    fn is_closed_virtual(&self, _k: &Kernel, _interval: &Extent1d, _tol: f64) -> Result<bool> {
        Ok(Self::is_closed(self))
    }
}

impl CompositeCurve {
    /// An exact evaluation copy of every segment, including its owner and flags.
    /// `depth` is the segments' depth in the shared `MAX_GEOMETRY_NESTING` (32) geometry traversal.
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        let segments = self
            .segments
            .iter()
            .map(|s| {
                Ok(CompositeCurveSegment {
                    transition: s.transition,
                    same_sense: s.same_sense,
                    curve: s.curve.clone_for_evaluation_depth(depth)?,
                    gap_start: s.gap_start,
                    gap_end: s.gap_end,
                    owns_curve: s.owns_curve,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            dim: self.dim,
            segments,
            closed: self.closed,
        })
    }

    /// `SmCompositeCurve(ULONG lDim)`: a composite of dimension `dim` without
    /// segments (see the module docs).
    #[must_use]
    pub const fn new(dim: u64) -> Self {
        Self {
            dim,
            segments: Vec::new(),
            closed: false,
        }
    }

    /// `SmCompositeCurve(lDimension, crCurves, bIsClosed, cpSenses, cpGaps)`: one
    /// segment per curve, in order, with transition `SM_CT_C0`. `senses[i]` is the
    /// sense of curve `i` (all `true` when `None`). With `gaps` (the gap from each
    /// curve's end to the next curve's start), each segment's start gap is the
    /// previous end gap (the last one for a closed composite, `SM_BIG_DOUBLE` for the
    /// first segment of an open one); without, every gap is `0`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for fewer senses or gaps than curves, or gaps for a closed
    /// composite without curves (upstream reads past the arrays), or when a curve
    /// nests deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING) - 1.
    pub fn with_curves(
        dim: u64,
        curves: Vec<Curve>,
        closed: bool,
        senses: Option<&[bool]>,
        gaps: Option<&[f64]>,
    ) -> Result<Self> {
        let n = curves.len();
        if senses.is_some_and(|s| s.len() < n) {
            return Err(SolidError::invalid_input(
                OP_NEW,
                "fewer senses than curves",
            ));
        }
        if gaps.is_some_and(|g| g.len() < n) {
            return Err(SolidError::invalid_input(OP_NEW, "fewer gaps than curves"));
        }
        for curve in &curves {
            check_geometry_nesting(NestedGeometry::Curve(curve), 1, OP_NEW)?;
        }

        // for every curve segment - load the SmCompositeCurveSegment object
        let mut segments: Vec<CompositeCurveSegment> = curves
            .into_iter()
            .enumerate()
            .map(|(i, curve)| {
                let same_sense = senses.and_then(|s| s.get(i).copied()).unwrap_or(true);
                CompositeCurveSegment {
                    transition: ContinuityType::C0,
                    same_sense,
                    curve,
                    gap_start: 0.0,
                    gap_end: 0.0,
                    owns_curve: false,
                }
            })
            .collect();

        // when given gaps - load the gap values
        if let Some(gaps) = gaps {
            let mut prev_gap = if closed {
                *n.checked_sub(1).and_then(|i| gaps.get(i)).ok_or_else(|| {
                    SolidError::invalid_input(OP_NEW, "gaps for a closed composite without curves")
                })?
            } else {
                BIG_DOUBLE
            };
            for (seg, &gap) in segments.iter_mut().zip(gaps) {
                seg.gap_start = prev_gap;
                prev_gap = gap;
                seg.gap_end = gap;
            }
        }

        Ok(Self {
            dim,
            segments,
            closed,
        })
    }

    /// Image-space dimension (`GetDim`).
    #[must_use]
    pub const fn dim(&self) -> u64 {
        self.dim
    }

    /// The segments, in order.
    #[must_use]
    pub fn segments(&self) -> &[CompositeCurveSegment] {
        &self.segments
    }

    /// Replaces segment `index` (the checked replacement for mutable segment access).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an out-of-range index or when the segment's curve nests
    /// deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING) - 1.
    /// The composite is left unchanged on error.
    pub fn set_segment(&mut self, index: usize, segment: CompositeCurveSegment) -> Result<()> {
        const OP_SET: &str = "curve::composite_curve::p1::CompositeCurve::set_segment";
        let slot = self
            .segments
            .get_mut(index)
            .ok_or_else(|| SolidError::invalid_input(OP_SET, "segment index out of range"))?;
        check_geometry_nesting(NestedGeometry::Curve(&segment.curve), 1, OP_SET)?;
        *slot = segment;
        Ok(())
    }

    /// The B-spline curve of segment `index`, mutable in place; `None` out of range
    /// or for another curve kind. A B-spline owns no nested geometry, so editing it
    /// cannot break the nesting bound, and the segment curve keeps its owner.
    pub(crate) fn segment_b_spline_mut(&mut self, index: usize) -> Option<&mut BSplineCurve> {
        match self.segments.get_mut(index)?.curve.kind_mut() {
            CurveKind::BSpline(curve) => Some(curve),
            _ => None,
        }
    }

    /// `GetNumSegments`.
    #[must_use]
    pub fn num_segments(&self) -> usize {
        self.segments.len()
    }

    /// `GetCurveSegment`: segment `index`, `None` out of range.
    #[must_use]
    pub fn segment(&self, index: usize) -> Option<&CompositeCurveSegment> {
        self.segments.get(index)
    }

    /// `IsClosed` (the interval and tolerance are ignored): whether the last curve
    /// connects to the first.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.closed
    }

    /// `GetCanonical`: dimension, segment curves, closedness and senses.
    #[must_use]
    pub fn canonical(&self) -> CompositeCanonical<'_> {
        CompositeCanonical {
            dim: self.dim,
            curves: self.segments.iter().map(|s| &s.curve).collect(),
            closed: self.closed,
            senses: self.segments.iter().map(|s| s.same_sense).collect(),
        }
    }

    /// The copy constructor (and `Copy`): every segment curve copied with
    /// [`Curve::copy`].
    ///
    /// # Errors
    ///
    /// Those of [`Curve::copy`] (upstream ignores them and keeps a `NULL` segment).
    pub fn copy(&self, k: &Kernel) -> Result<Self> {
        let mut segments = Vec::with_capacity(self.segments.len());
        for s in &self.segments {
            segments.push(CompositeCurveSegment {
                transition: s.transition,
                same_sense: s.same_sense,
                curve: s.curve.copy(k)?,
                gap_start: s.gap_start,
                gap_end: s.gap_end,
                owns_curve: false,
            });
        }
        Ok(Self {
            dim: self.dim,
            segments,
            closed: self.closed,
        })
    }

    /// `operator==` (after the base check of type and dimension): same segment
    /// count and closedness, and equal segments.
    ///
    /// # Errors
    ///
    /// Those of [`CompositeCurveSegment::approx_eq`] (every segment is compared).
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }

        // first check the base
        let mut rtn = self.dim == other.dim;

        if rtn {
            // check equivalence of these objects
            rtn = self.segments.len() == other.segments.len() && self.closed == other.closed;

            // check segments (upstream ANDs every comparison, no short cut)
            if rtn {
                for (a, b) in self.segments.iter().zip(&other.segments) {
                    let same = a.approx_eq(k, b)?;
                    rtn &= same;
                }
            }
        }
        Ok(rtn)
    }

    /// `GetNaturalInterval`: the first segment's natural interval lengthened by the
    /// lengths of the others; the uninitialised interval without segments.
    #[must_use]
    pub fn natural_interval(&self) -> Extent1d {
        // no work - no segments
        let Some((first, rest)) = self.segments.split_first() else {
            return Extent1d::new();
        };

        let seg_ivl = first.curve.natural_interval();
        let mut ivl = Extent1d::from_min_max(seg_ivl.min(), seg_ivl.max());

        // for every subsequent interval
        for s in rest {
            let seg_ivl = s.curve.natural_interval();

            // increment the effective interval (status ignored upstream)
            let _ = ivl.set_min_max(ivl.min(), ivl.max() + seg_ivl.length());
        }
        ivl
    }

    /// `GetKnots`: the first segment's unique knots and multiplicities; without
    /// segments, none.
    ///
    /// # Errors
    ///
    /// Those of the segments' [`Curve::knots`]; `InvalidInput` for two or more
    /// segments or with `interval` (upstream reads out of bounds, see the module
    /// docs).
    pub fn knots(&self, k: &Kernel, interval: Option<&Extent1d>) -> Result<(Vec<f64>, Vec<usize>)> {
        // no work - no segments
        let Some((first, rest)) = self.segments.split_first() else {
            return Ok((Vec::new(), Vec::new()));
        };

        let (knots, mults) = first.curve.knots(k, interval)?;

        // for every subsequent interval
        if let Some(this) = rest.first() {
            // next segment knots
            this.curve.knots(k, None)?;

            // knot offset: upstream reads rKnots[GetSize()], one past the end
            return Err(SolidError::invalid_input(
                OP_KNOTS,
                "two or more segments: upstream reads the knot array past its end",
            ));
        }

        // trim output to OptIvl: the unsigned loop `ii >= 0` never ends
        if interval.is_some() {
            return Err(SolidError::invalid_input(
                OP_KNOTS,
                "interval given: upstream's unsigned loop reads past the knot array",
            ));
        }
        Ok((knots, mults))
    }

    /// `CalculateBoundingBox`: the union of the segments' boxes over `interval`. Each
    /// segment's natural interval is shifted by the summed lengths of the previous
    /// segments (starting at `0`, as upstream) and intersected with `interval`; the
    /// segment's boxes are taken over that shifted interval. The axis aligned box
    /// starts uninitialised, the pseudo box initialised and the polar box reset; the
    /// first pseudo union takes the segment box's basis.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `req.normal` is false (upstream writes `*pBBox`
    /// regardless); those of the polar box union; [`SolidError::Unported`] from the
    /// segments.
    pub fn calculate_bounding_box(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        req: BoundingBoxRequest,
        expand: bool,
    ) -> Result<BoundingBoxes> {
        if !req.normal {
            return Err(SolidError::invalid_input(
                OP_BOX,
                "the axis aligned box is always written upstream",
            ));
        }

        // init output
        let mut bbox = Extent3d::new();
        bbox.init();
        let mut pseudo = PseudoBox::new();
        pseudo.init();
        let mut polar = PolarBox::new();
        polar.reset();

        // locals
        let mut first_union = true;
        let mut this_bbox = Extent3d::new();
        let mut this_pseudo = PseudoBox::new();
        let mut this_polar = PolarBox::new();
        let mut ivl_offset = 0.0;

        // for every interval
        for s in &self.segments {
            let mut seg_ivl = s.curve.natural_interval();

            // get target Ivl for this segment
            seg_ivl.translate(ivl_offset);
            let tgt_ivl = seg_ivl
                .intersect(interval)
                .unwrap_or_else(|_| Extent1d::new());

            // when TgtIvl is not negative
            if !tgt_ivl.has_negative_length() {
                // get its bounding boxes (status ignored upstream)
                if let Some(b) = ignored(s.curve.calculate_bounding_box(k, &tgt_ivl, req, expand))?
                {
                    if let Some(n) = b.normal {
                        this_bbox = n;
                    }
                    if let Some(p) = b.pseudo {
                        this_pseudo = p;
                    }
                    if let Some(p) = b.polar {
                        this_polar = p;
                    }
                }

                // union bounding boxes
                bbox = bbox.union(&this_bbox);
                if req.pseudo {
                    let basis = if first_union {
                        *this_pseudo.basis()
                    } else {
                        *pseudo.basis()
                    };
                    let current = pseudo;
                    current.union_into(&this_pseudo, &basis, &mut pseudo);
                }
                if req.polar {
                    polar.union_assign(&this_polar)?;
                }

                // mark state - done with first union
                first_union = false;
            }

            // (the loop increment)
            ivl_offset += seg_ivl.length();
        }

        Ok(BoundingBoxes {
            normal: Some(bbox),
            pseudo: req.pseudo.then_some(pseudo),
            polar: req.polar.then_some(polar),
        })
    }

    /// `ApproximateCurve`: every segment approximated by a B-spline
    /// ([`convert_to_bspline_curves`] with `approx_tol_3d`, B-splines copied, not
    /// matching the parameterisation), joined into one curve
    /// ([`BSplineCurve::create_by_joining`]); a joined curve that is not valid
    /// (typically C0 in its interior) is dropped. The maximum gap is the largest of
    /// the interior end gaps and the segment approximation errors.
    ///
    /// # Errors
    ///
    /// Those of the join (upstream then dereferences a `NULL` curve) and of the
    /// joined curve's `AssertValid`; [`SolidError::Unported`] from the segment
    /// approximations.
    pub fn approximate_curve(
        &self,
        k: &Kernel,
        approx_tol_3d: f64,
    ) -> Result<CompositeApproximation> {
        // init output
        let mut max_gap = 0.0;

        // locals
        let n = self.segments.len();
        let mut min_cont_type = ContinuityType::CInfinity;
        let mut orig_segments: Vec<&Curve> = Vec::with_capacity(n);

        // gather the segments
        for (ii, s) in self.segments.iter().enumerate() {
            // build segment array
            orig_segments.push(&s.curve);

            // min continuity
            if ii + 1 < n && (s.transition as i32) < (min_cont_type as i32) {
                min_cont_type = s.transition;
            }

            // max gap
            if ii + 1 < n && s.gap_end > max_gap {
                max_gap = s.gap_end;
            }
        }

        // approximate every segment as a BSpline
        let (mut approx_segments, gaps) =
            convert_to_bspline_curves(k, &orig_segments, approx_tol_3d, false, true)?;

        // max achieved tol (entries past the approximations made read the zeros
        // upstream's array was cleared to)
        for ii in 0..n {
            let g = gaps.get(ii).copied().unwrap_or(0.0);
            if g > max_gap {
                max_gap = g;
            }
        }

        // join the Approximations as a single curve
        let mut bsplines = Vec::with_capacity(approx_segments.len());
        for c in approx_segments.drain(..) {
            bsplines.push(bspline_part(c)?);
        }
        let joined = BSplineCurve::create_by_joining(&mut bsplines, None)?;

        // check for valid curve - many might be C0 in their interiors
        let valid = joined.assert_valid(k, None, false, ObjectKey(0))?;
        Ok(CompositeApproximation {
            curve: valid.then_some(joined),
            max_gap,
        })
    }

    /// `Transform`: every segment curve rotated and moved by `rotate_n_move` after
    /// the optional scale; nothing for the identity.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the segments (their other failures are ignored,
    /// as upstream).
    pub fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        self.transform_at_depth(k, rotate_n_move, scale, 0)
    }

    pub(crate) fn transform_at_depth(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        depth: usize,
    ) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        for s in &mut self.segments {
            ignored(
                s.curve
                    .transform_at_depth(k, rotate_n_move, scale, depth + 1),
            )?;
        }
        Ok(())
    }

    /// `ComputeProjectedLoopOrientation`: the orientation of the loop of `curves`
    /// (curve `i` taken reversed when `senses[i]` is false) projected along
    /// `normal`. Each of three passes samples the curves (linear curves by their end
    /// points, others by tessellation to 10, 1 and 0.1 degrees; curves that fail to
    /// tessellate are skipped), sums the signed area of the polygon about its first
    /// point and compares it with a rough area of the polygon's box: a ratio above
    /// `0.01` (`1e-6` in the last pass) is counter-clockwise ([`OrientType::Same`]),
    /// below its negative clockwise ([`OrientType::Opposite`]); a ratio changing sign
    /// between passes, or no decision, is [`OrientType::Unknown`] (zero area). With
    /// `bounds`, every sample's `(x, y)` is added to it.
    ///
    /// # Errors
    ///
    /// Those of the end point evaluations; `InvalidInput` for fewer senses than
    /// curves or an empty polygon (upstream reads past the arrays);
    /// [`SolidError::Unported`] from the linearity test and the tessellation.
    pub fn compute_projected_loop_orientation(
        k: &Kernel,
        curves: &[Curve],
        senses: &[bool],
        normal: Vec3,
        mut bounds: Option<&mut Extent2d>,
    ) -> Result<OrientType> {
        // locals
        let mut bsc_poly: Vec<Point3> = Vec::new();
        let mut angle_deg = 100.0;

        // (see below)
        let mut prev_ratio = 0.0;

        // Do 3 iterations, getting a more accurate tessellation each time.
        for iter in 0..3u32 {
            // init locals - reduce angle limit
            let mut polygon: Vec<Point3> = Vec::new();
            bsc_poly.clear();
            angle_deg /= 10.0; // For a more accurate tessellation.

            // Tessellate every curve.
            for (this_curve, bsc) in curves.iter().enumerate() {
                // For lines, we need only the two end points.
                if bsc.is_linear(k, EFF_ZERO, false)?.linear {
                    let ivl = bsc.natural_interval();
                    let p0 = bsc.evaluate_point(k, ivl.min())?;
                    bsc_poly.clear();
                    bsc_poly.push(p0);
                    let p1 = bsc.evaluate_point(k, ivl.max())?;
                    bsc_poly.push(p1);
                } else {
                    // tesselate curve to current dAngleDeg - skip failures
                    match bsc.tessellate(k, &bsc.natural_interval(), 0.0, angle_deg, 3) {
                        Ok(t) => bsc_poly = t.points,
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => continue,
                    }
                }

                // when asked - build UVBoundingBox of all SamplePoints
                if let Some(b) = bounds.as_deref_mut() {
                    for p in &bsc_poly {
                        b.add_point(Point2::new(p.x, p.y));
                    }
                }

                // add every CurveSamplePoints found to growing polygon
                let sense = *senses.get(this_curve).ok_or_else(|| {
                    SolidError::invalid_input(OP_ORIENT, "fewer senses than curves")
                })?;
                if sense {
                    polygon.extend_from_slice(&bsc_poly);
                } else {
                    // Opposite orientation of curve
                    polygon.extend(bsc_poly.iter().rev());
                }
            }

            // Check polygon has more than 3 points (an assertion upstream)
            let num_points = polygon.len();
            let Some((&first, rest)) = polygon.split_first() else {
                return Err(SolidError::invalid_input(OP_ORIENT, "empty loop polygon"));
            };

            // We're going to calculate a 3d bounding box, to compare against our
            // calculated loop area (to make a relative check instead of absolute).
            let mut bbox = Extent3d::new();

            // We'll also need to know which is the smallest component of the
            // direction vector.
            let mut smallest_component = 0;
            let mut smallest_val = normal.x.abs();
            if normal.y.abs() < smallest_val {
                smallest_component = 1;
                smallest_val = normal.y.abs();
            }
            if normal.z.abs() < smallest_val {
                smallest_component = 2;
            }

            // init polygon walk parameters
            let mut total_area = 0.0;
            let point1 = first;

            // Accumulate bounding box.
            bbox.add_point(point1);

            // use a point near the polygon: the start point
            let base_pt = first;

            let vec1 = point1 - base_pt;
            let mut vec1_proj = vector_project_to_plane(vec1, normal);

            // For every pair of polygon points - total the polygon area.
            for &point2 in rest {
                bbox.add_point(point2);
                let vec2 = point2 - base_pt;
                let vec2_proj = vector_project_to_plane(vec2, normal);

                // twice the signed area of the triangle base point, point1, point2
                let cross = vec1_proj.cross(vec2_proj);
                total_area += cross.dot(normal);

                // continue the walk
                vec1_proj = vec2_proj;
            }

            // the box 'area': extent along the direction most nearly perpendicular
            // to the normal times the diagonal of the other two (as upstream, the
            // third case pairs X with Z)
            let (extent, diagonal) = match smallest_component {
                0 => (bbox.x_length(), Vec2::new(bbox.y_length(), bbox.z_length())),
                1 => (bbox.y_length(), Vec2::new(bbox.z_length(), bbox.x_length())),
                _ => (bbox.z_length(), Vec2::new(bbox.x_length(), bbox.z_length())),
            };

            // If either dimension of the bounding box is 'zero', then the area is zero.
            let tol1 = EFF_ZERO;
            let diag_len = diagonal.length();
            let box_area = if extent > tol1 && diag_len > tol1 {
                extent * diag_len
            } else {
                0.0
            };

            let tol2 = EFF_ZERO_SQ * num_points as f64;
            let ratio = if box_area > total_area.abs() * tol2 {
                total_area / box_area
            } else {
                0.0
            };

            // the sampling is noisy: a big tolerance first, a tight one at the end
            let mut tol3 = 0.01;
            if iter >= 2 {
                tol3 = 0.000_001;
            }

            // a sign change between iterations means zero area
            if iter > 0 && ratio * prev_ratio < 0.0 {
                return Ok(OrientType::Unknown);
            }
            prev_ratio = ratio;

            if ratio > tol3 {
                return Ok(OrientType::Same); // Positive means CCW.
            } else if ratio < -tol3 {
                return Ok(OrientType::Opposite);
            }
        }

        // the loop has zero area
        Ok(OrientType::Unknown)
    }

    /// `BuildCompositesFromCurves`: connects unordered `curves` into composite
    /// curves (each 3D) by their closest end points. Starting from the first unused
    /// curve, the curve with the closest end to the chain's end is appended (taken
    /// reversed when its end is the closer), then to the chain's start; a gap above
    /// the largest tolerance (`distance_to_create_line` when positive, else
    /// `same_point_tolerance`) is not bridged. A finished chain becomes one or more
    /// composites (`sm_FlushCrv`): gaps below `same_point_tolerance` are kept as
    /// they are, gaps below `distance_to_create_line` (when not `0`) get a line
    /// segment, a chain ends at a gap of at least the largest tolerance and is
    /// closed when its end is within that tolerance of its start; a single curve
    /// shorter than `same_point_tolerance` (5-sample length) is left out.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for `same_point_tolerance <= 0` or a non-zero
    /// `distance_to_create_line` not above it; those of the lengths and the gap
    /// lines; [`SolidError::Unported`] from the end points.
    pub fn build_composites_from_curves(
        k: &Kernel,
        curves: Vec<Curve>,
        same_point_tolerance: f64,
        distance_to_create_line: f64,
    ) -> Result<BuiltComposites> {
        // check input
        if same_point_tolerance <= 0.0 {
            return Err(SolidError::sm_err(OP_BUILD));
        }
        if !(distance_to_create_line == 0.0 || distance_to_create_line > same_point_tolerance) {
            return Err(SolidError::sm_err(OP_BUILD));
        }

        // Get the max input tolerance.
        let tol = if distance_to_create_line > 0.0 {
            distance_to_create_line
        } else {
            same_point_tolerance
        };

        // init output
        let mut composites: Vec<CompositeCurve> = Vec::new();

        let total = curves.len();
        if total == 0 {
            return Ok(BuiltComposites {
                composites,
                unused: Vec::new(),
            });
        }

        // create an array of link-list curve objects
        let mut items: Vec<CurveItem> = Vec::with_capacity(total);
        for c in curves {
            let mut item = CurveItem::new(c);
            // (status ignored upstream: GetEnds returns void)
            if let Some(e) = ignored(
                item.curve
                    .as_ref()
                    .map_or_else(|| Err(SolidError::sm_err(OP_BUILD)), |curve| curve.ends(k)),
            )? {
                item.start = e.start;
                item.end = e.end;
            }
            items.push(item);
        }

        // while loop locals
        let mut l_end = 0u32; // 0 = append at the end, 1 = at the start
        let mut head: Option<usize> = None; // sListHead.m_pNext
        let mut insert_before = false;
        let mut reverse = false;
        let mut done = false;

        // Point ListHead->Next to pList[0] and set while pointers
        append_after_head(&mut items, &mut head, 0)?;
        item_mut(&mut items, 0)?.used = true;
        let mut start = 0usize;
        let mut end = 0usize;

        // while curves are not yet placed into an ordered link list
        while !done {
            let mut min_dist = BIG_DOUBLE;
            let mut found: Option<usize> = None;

            let start_pt = item(&items, start)?.start;
            let end_pt = item(&items, end)?.end;

            // for every curve - find the best curve (smallest end-gap)
            for (i, ci) in items.iter().enumerate() {
                // skip marked items
                if ci.used {
                    continue;
                }

                if l_end == 0 {
                    // this curve's start closer to the list end
                    let d = ci.start.distance(end_pt);
                    if d < min_dist {
                        insert_before = false;
                        min_dist = d;
                        reverse = false;
                        found = Some(i);
                    }
                    // this curve's end closer to the list end
                    let d = ci.end.distance(end_pt);
                    if d < min_dist {
                        insert_before = false;
                        min_dist = d;
                        reverse = true;
                        found = Some(i);
                    }
                } else {
                    // this curve's start closer to the list start
                    let d = ci.start.distance(start_pt);
                    if d < min_dist {
                        insert_before = true;
                        min_dist = d;
                        reverse = true;
                        found = Some(i);
                    }
                    // this curve's end closer to the list start
                    let d = ci.end.distance(start_pt);
                    if d < min_dist {
                        insert_before = true;
                        min_dist = d;
                        reverse = false;
                        found = Some(i);
                    }
                }
            }

            // skip out of tolerance ends
            if min_dist > tol {
                found = None;
            }

            // no curve at the end boundary - try the start boundary
            if found.is_none() && l_end == 0 {
                l_end = 1;
                continue;
            }

            let Some(f) = found else {
                // Flush the curve and reinitialize the list
                flush_crv(
                    k,
                    &mut items,
                    &mut head,
                    tol,
                    same_point_tolerance,
                    distance_to_create_line,
                    &mut composites,
                )?;

                // Now look for 1st remaining curve segment to start another composite
                if let Some(kk) = items.iter().position(|c| !c.used) {
                    head = Some(kk);
                    item_mut(&mut items, kk)?.used = true;
                    start = kk;
                    end = kk;
                }

                // exit case - no new segments found to start another composite curve
                done = head.is_none();

                l_end = 0;
                continue;
            };

            // remember the segment has been used
            let fi = item_mut(&mut items, f)?;
            fi.used = true;

            // When curve segment needs to be reversed - reverse its data
            if reverse {
                fi.sense = false;
                core::mem::swap(&mut fi.start, &mut fi.end);
            }

            if insert_before {
                // appended to start - insert segment after the list head
                fi.dist_to_next = min_dist;
                append_after_head(&mut items, &mut head, f)?;
                start = f;
            } else {
                // appended to end - insert segment after pEnd
                item_mut(&mut items, end)?.dist_to_next = min_dist;
                append_after(&mut items, end, f)?;
                end = f;
            }
        }

        // the curves no composite took stay with the caller
        let unused = items.into_iter().filter_map(|c| c.curve).collect();
        Ok(BuiltComposites { composites, unused })
    }
}

// ---- BuildCompositesFromCurves helpers ------------------------------------------------

/// A linked-list node of `BuildCompositesFromCurves` (`SmCurveItem`); the curve
/// moves into a composite when one is built from it.
#[derive(Debug)]
struct CurveItem {
    /// `m_pCurve`.
    curve: Option<Curve>,
    /// `m_bSense`.
    sense: bool,
    /// `m_Start`.
    start: Point3,
    /// `m_End`.
    end: Point3,
    /// `m_pNext`.
    next: Option<usize>,
    /// `m_DistToNext`.
    dist_to_next: f64,
    /// `m_bUsed`.
    used: bool,
}

impl CurveItem {
    /// `SmCurveItem()` holding `curve`; the end points are undefined.
    const fn new(curve: Curve) -> Self {
        Self {
            curve: Some(curve),
            sense: true,
            start: Vec3::UNDEF,
            end: Vec3::UNDEF,
            next: None,
            dist_to_next: BIG_DOUBLE,
            used: false,
        }
    }
}

fn item(items: &[CurveItem], i: usize) -> Result<&CurveItem> {
    items
        .get(i)
        .ok_or_else(|| SolidError::invalid_input(OP_BUILD, "curve item index out of range"))
}

fn item_mut(items: &mut [CurveItem], i: usize) -> Result<&mut CurveItem> {
    items
        .get_mut(i)
        .ok_or_else(|| SolidError::invalid_input(OP_BUILD, "curve item index out of range"))
}

/// `AppendSeg(&sListHead, new)`.
fn append_after_head(items: &mut [CurveItem], head: &mut Option<usize>, new: usize) -> Result<()> {
    item_mut(items, new)?.next = *head;
    *head = Some(new);
    Ok(())
}

/// `AppendSeg(seg, new)`.
fn append_after(items: &mut [CurveItem], seg: usize, new: usize) -> Result<()> {
    let after = item(items, seg)?.next;
    item_mut(items, new)?.next = after;
    item_mut(items, seg)?.next = Some(new);
    Ok(())
}

/// A curve moved out of its list item (each item is flushed once).
fn take_curve(items: &mut [CurveItem], i: usize) -> Result<Curve> {
    item_mut(items, i)?
        .curve
        .take()
        .ok_or_else(|| SolidError::invalid_input(OP_BUILD, "curve listed twice"))
}

/// `sm_FlushCrv`: turns the linked list after `head` into composite curves (see
/// [`CompositeCurve::build_composites_from_curves`]) and empties it.
fn flush_crv(
    k: &Kernel,
    items: &mut [CurveItem],
    head: &mut Option<usize>,
    tol: f64,
    same_point_tolerance: f64,
    distance_to_create_line: f64,
    composites: &mut Vec<CompositeCurve>,
) -> Result<()> {
    // locals: the curves (by item), senses and gaps of the composite being built,
    // and the line segments filling gaps (by position)
    let mut curves: Vec<Pending> = Vec::new();
    let mut senses: Vec<bool> = Vec::new();
    let mut gaps: Vec<f64> = Vec::new();
    let mut first: Option<usize> = None;
    let mut curr: Option<usize> = None; // None is the list head

    loop {
        let next_of_curr = match curr {
            None => *head,
            Some(c) => item(items, c)?.next,
        };
        // while curve remains open
        let Some(c) = next_of_curr else {
            break;
        };

        let mut next: Option<usize> = None;
        let mut create_composite = false;
        let mut is_closed = false;

        // increment curr: curr is the last curve in the linked-list
        curr = Some(c);

        // add next curve segment to arrays
        curves.push(Pending::Item(c));
        senses.push(item(items, c)?.sense);

        // remember the first curve segment
        if curves.len() == 1 {
            first = Some(c);
        }

        let dist_to_next = item(items, c)?.dist_to_next;
        if dist_to_next < tol {
            // let next be the next input curve
            next = item(items, c)?.next;
        } else {
            // this composite curve is done
            create_composite = true;

            // a single curve composite
            if first == Some(c) {
                // Calculate Length of the segment
                let crv = item(items, c)?
                    .curve
                    .as_ref()
                    .ok_or_else(|| SolidError::invalid_input(OP_BUILD, "curve listed twice"))?;
                let ivl = crv.natural_interval();
                let (len, _) = crv.approximate_length(k, &ivl, 5, false)?;

                // skip 1st curves too small
                if len < same_point_tolerance {
                    curves.clear();
                    senses.clear();
                    gaps.clear();
                    continue;
                }
                gaps.push(dist_to_next);
            }

            // Check if it closes the composite curve.
            let f = first.ok_or_else(|| SolidError::invalid_input(OP_BUILD, "no first curve"))?;
            let d = item(items, c)?.end.distance(item(items, f)?.start);
            if d < tol {
                item_mut(items, c)?.dist_to_next = d;
                is_closed = true;
                next = Some(f);
            }
        }

        let dist_to_next = item(items, c)?.dist_to_next;
        if dist_to_next < same_point_tolerance {
            // curves will be connected as is with this gap
            gaps.push(dist_to_next);
        } else if distance_to_create_line != 0.0 && dist_to_next < distance_to_create_line {
            // fill gap with line segment
            let n = next.ok_or_else(|| {
                SolidError::invalid_input(OP_BUILD, "gap line without a next curve")
            })?;
            let line =
                BSplineCurve::create_line_segment(3, item(items, c)?.end, item(items, n)?.start)?;
            gaps.push(0.0);
            curves.push(Pending::Line(line));
            senses.push(true);
            gaps.push(0.0);
        } else {
            // the end of an open composite curve
            gaps.push(dist_to_next);
        }

        // when a composite curve needs to be created
        if create_composite {
            let use_gaps = gaps.len() == curves.len();
            let mut cs = Vec::with_capacity(curves.len());
            for p in curves.drain(..) {
                cs.push(match p {
                    Pending::Item(i) => take_curve(items, i)?,
                    Pending::Line(l) => Curve::from(l),
                });
            }
            let comp = CompositeCurve::with_curves(
                3,
                cs,
                is_closed,
                Some(&senses),
                use_gaps.then_some(gaps.as_slice()),
            )?;
            composites.push(comp);

            // reset scratch arrays
            senses.clear();
            gaps.clear();
        }
    }

    // all done - empty input list
    *head = None;
    Ok(())
}

/// A curve waiting for its composite: an input item or a new gap line.
#[derive(Debug)]
enum Pending {
    Item(usize),
    Line(BSplineCurve),
}

// ---- approximation ------------------------------------------------------------------

/// The `SmBSplineCurve` an approximation is: a B-spline, or the B-spline part of
/// an analytic curve derived from it.
fn bspline_part(c: Curve) -> Result<BSplineCurve> {
    match c.into_kind() {
        CurveKind::BSpline(b) => Ok(b),
        CurveKind::Line(l) => Ok(l.base_with_nurb()?.into_owned()),
        CurveKind::Ellipse(e) => Ok(e.base().clone()),
        CurveKind::Hyperbola(h) => Ok(h.base().clone()),
        CurveKind::Parabola(p) => Ok(p.base().clone()),
        _ => Err(SolidError::invalid_input(
            OP_APPROX,
            "approximation is not an SmBSplineCurve",
        )),
    }
}

/// `sm_convert_to_BSplineCurves`: each curve approximated by a B-spline over its
/// natural interval (`ApproximateCurve` with the interval ends as breaks,
/// analytics made when possible, `match_pz`, `just_copy_bsplines`), with the
/// largest sampled error of each. Curves whose approximation fails are skipped. A
/// curve that reports no error repeats the previous curve's (`0` for the first).
///
/// # Errors
///
/// [`SolidError::Unported`] from the approximations.
pub fn convert_to_bspline_curves(
    k: &Kernel,
    curves: &[&Curve],
    approx_tol_3d: f64,
    match_pz: bool,
    just_copy_bsplines: bool,
) -> Result<(Vec<Curve>, Vec<f64>)> {
    let (out, gaps, _) =
        convert_to_bspline_curves_indexed(k, curves, approx_tol_3d, match_pz, just_copy_bsplines)?;
    Ok((out, gaps))
}

/// [`convert_to_bspline_curves`], also returning the index in `curves` each
/// approximation was made from (the offsetters move the source curve's offset map
/// attribute to it, as upstream's `RemoveAttribute` / `AddAttribute`).
///
/// # Errors
///
/// [`SolidError::Unported`] from the approximations.
pub(super) fn convert_to_bspline_curves_indexed(
    k: &Kernel,
    curves: &[&Curve],
    approx_tol_3d: f64,
    match_pz: bool,
    just_copy_bsplines: bool,
) -> Result<(Vec<Curve>, Vec<f64>, Vec<usize>)> {
    // init output
    let mut out = Vec::with_capacity(curves.len());
    let mut gaps = Vec::with_capacity(curves.len());
    let mut sources = Vec::with_capacity(curves.len());

    // locals
    let mut max_gap = 0.0;
    let opts = ApproximateCurveOptions {
        create_analytics: true,
        match_parameterization: match_pz,
        just_copy_bsplines,
    };

    // for every curve - approx with BSplineCurve
    for (index, c) in curves.iter().enumerate() {
        let ivl = c.natural_interval();
        let breaks = [ivl.min(), ivl.max()];

        // approximate the curve; delete anything a failure made, but don't quit
        if let Some(a) = ignored(c.approximate_curve(k, &breaks, approx_tol_3d, &opts))? {
            if let Some(g) = a.achieved_tol {
                max_gap = g;
            }
            out.push(a.curve);
            gaps.push(max_gap);
            sources.push(index);
        }
    }
    Ok((out, gaps, sources))
}

// ---- offset corners -------------------------------------------------------------------

/// The corner shapes of the offsetters (`SmOffsetCornerType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OffsetCornerType {
    /// `SM_OC_LINEAR_EXTENSION`: two lines from the offset ends to the
    /// intersection of their linear extensions.
    LinearExtension = 0,
    /// `SM_OC_FILLET_CORNER`: an arc centred on the original vertex.
    FilletCorner = 1,
    /// `SM_OC_LINEAR_CHAMFER`: the extension lines trimmed by a chamfer line.
    LinearChamfer = 2,
    /// `SM_OC_CURVE_EXTENSION` (source change, source fork 54148db): each
    /// offset continues as its own curve to where the two meet. Only
    /// `PrimitiveCreation::create_taper_extrude` takes it: there each wall continues as
    /// its own surface (a line's plane, an arc's cone) and neighbouring walls meet along
    /// their intersection. The offsetters refuse it.
    CurveExtension = 3,
}

/// The curves of [`create_corner`] (`rpCornerCurve1..3`).
#[derive(Debug, Clone, Default)]
pub struct CornerCurves {
    /// Extension line from `p1` (extension and chamfer corners) or the fillet arc.
    pub curve1: Option<Curve>,
    /// Extension line to (extension) or from (chamfer) `p2`.
    pub curve2: Option<Curve>,
    /// The chamfer line.
    pub curve3: Option<Curve>,
}

/// The two curves of a chamfer intersection, named by their solutions.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CornerHandle(u8, Extent1d);

impl SolutionObject for CornerHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.1)
    }
}

/// The parameters (this curve's, the other's) of the first intersection of `a`
/// with `b` (`GlobalCurveIntersect`, status ignored).
fn first_intersection(k: &Kernel, a: &Curve, b: &Curve, tol: f64) -> Result<(f64, f64)> {
    let ra = a.natural_interval();
    let rb = b.natural_interval();
    let opts = CurveIntersectOptions {
        handles: [Some(CornerHandle(0, ra)), Some(CornerHandle(1, rb))],
        analytic_handles: [Some(CornerHandle(2, ra)), Some(CornerHandle(3, rb))],
        ..CurveIntersectOptions::default()
    };
    let sols = ignored(a.global_curve_intersect(k, &ra, b, &rb, tol, &opts))?;
    let s = sols.as_ref().and_then(|s| s.get(0)).ok_or_else(|| {
        SolidError::invalid_input(
            OP_CORNER,
            "no chamfer intersection (upstream reads sSolutions[0])",
        )
    })?;
    Ok((s.start.parameters[0], s.start.parameters[1]))
}

/// The parameters along the linear extensions (`dT1` from `p1` along `v1`, `dT2`
/// from `p2` along `v2`) of the corner's extension lines.
fn extension_params(p1: Point3, v1: Vec3, p2: Point3, v2: Vec3) -> Result<(f64, f64)> {
    let mut t1 = 0.0;
    let mut t2 = 0.0;
    if v1.length_squared() > EFF_ZERO_SQ && v2.length_squared() > EFF_ZERO_SQ {
        (t1, t2) = line_line_closest_point(p1, v1, p2, v2)?;
    } else if v1.length_squared() > EFF_ZERO_SQ {
        t1 = line_closest_point(p1, v1, p2);
    } else {
        t2 = line_closest_point(p2, v2, p1);
    }
    Ok((t1, t2))
}

/// `sm_CreateCorner`: the curves filling the corner between an offset ending at
/// `p1` (tangent `v1`) and the next one beginning at `p2` (tangent `v2`), for
/// `vertex_point` the original curves' common end.
///
/// - [`OffsetCornerType::LinearExtension`]: lines from `p1` to `p1 + t1 v1` and from
///   `p2 + t2 v2` to `p2`, where the extensions come closest (a line is missing when
///   its parameter is zero, e.g. a zero tangent).
/// - [`OffsetCornerType::FilletCorner`]: the quadratic arc about `vertex_point` from
///   `p1` through the angle to `p2` (in the plane of `p1 - vertex_point` and `v1`
///   when the two radius vectors are parallel).
/// - [`OffsetCornerType::LinearChamfer`]: the extension lines (the second from `p2`
///   outwards), and when both exist a chamfer line perpendicular to the direction
///   from `vertex_point` to the extensions' meeting point at `offset_distance` from
///   the vertex; the three are trimmed at their intersections (within
///   `xsect_tol_3d`).
///
/// # Errors
///
/// Those of the closest points (parallel extensions), the lines, the frame and
/// angle of the fillet, the arc and the trims; `InvalidInput` when a chamfer
/// intersection is missing; [`SolidError::Unported`] from the intersections.
pub fn create_corner(
    k: &Kernel,
    vertex_point: Point3,
    p1: Point3,
    v1: Vec3,
    p2: Point3,
    v2: Vec3,
    corner: OffsetCornerType,
    offset_distance: f64,
    xsect_tol_3d: f64,
) -> Result<CornerCurves> {
    // init output
    let mut out = CornerCurves::default();

    match corner {
        OffsetCornerType::LinearExtension => {
            // find linear extension xsect point - handle zero length tangents
            let (t1, t2) = extension_params(p1, v1, p2, v2)?;

            // when crV1 is zero length - no linear extension
            if !is_zero(t1) {
                let end1 = p1 + t1 * v1;
                out.curve1 = Some(Curve::from(BSplineCurve::create_line_segment(3, p1, end1)?));
            }

            // when crV2 is zero length - no linear extension
            if !is_zero(t2) {
                let end2 = p2 + t2 * v2;
                out.curve2 = Some(Curve::from(BSplineCurve::create_line_segment(3, end2, p2)?));
            }
        }
        OffsetCornerType::FilletCorner => {
            // Create a fillet in the corner.
            let sv1 = p1 - vertex_point;
            let sv2 = p2 - vertex_point;
            let cross = sv1.cross(sv2);

            // Handle the 180 degree case by using crV1 as the direction on the side
            // of the curve to keep.
            let [sx, sy, sz] = if cross.length_squared() < EFF_ZERO {
                sv1.make_unit_ortho_vectors(Some(v1))?
            } else {
                sv1.make_unit_ortho_vectors(Some(sv2))?
            };
            let angle = sz.ccw_angle_between(sv1, sv2)?.abs();
            let mut a2p = Axis2Placement::IDENTITY;
            // (status ignored upstream)
            let _ = a2p.set_canonical(vertex_point, sx, sy);
            let arc = BSplineCurve::create_circle_segment(
                3,
                &a2p,
                sv1.length(),
                0.0,
                angle * 180.0 / PI,
                NurbCircleParam::Quadratic,
            )?;
            out.curve1 = Some(Curve::from(arc));
        }
        OffsetCornerType::LinearChamfer => {
            // find linear extension xsect point - handle zero length tangents
            let (t1, t2) = extension_params(p1, v1, p2, v2)?;

            // Keep track of intersection between offset extensions
            let mut x_pnt = Vec3::UNDEF;

            let mut c1 = None;
            if !is_zero(t1) {
                let end1 = p1 + t1 * v1;
                c1 = Some(Curve::from(BSplineCurve::create_line_segment(3, p1, end1)?));
                x_pnt = end1;
            }

            let mut c2 = None;
            if !is_zero(t2) {
                let end2 = p2 + t2 * v2;
                c2 = Some(Curve::from(BSplineCurve::create_line_segment(3, p2, end2)?));
                x_pnt = end2;
            }

            // Both offset extensions need to exist to calculate chamfer
            let (mut c1, mut c2) = match (c1, c2) {
                (Some(a), Some(b)) => (a, b),
                (a, b) => {
                    out.curve1 = a;
                    out.curve2 = b;
                    return Ok(out);
                }
            };

            // create vector between vertex and extension intersection
            let vertex_to_x = (x_pnt - vertex_point).unitize_or_z();

            // the plane normal of the two extensions (status ignored upstream)
            let pair = [c1, c2];
            let plane_normal =
                ignored(compute_crvs_normal(k, &pair))?.unwrap_or(Vec3::new(0.0, 0.0, 0.0));
            [c1, c2] = pair;

            let chamfer_vector = vertex_to_x.cross(plane_normal);
            let chamfer_pt = vertex_point + offset_distance * vertex_to_x;

            // Now add and subtract along chamferVector
            let chamfer_start = chamfer_pt + offset_distance * chamfer_vector;
            let chamfer_end = chamfer_pt - offset_distance * chamfer_vector;
            let mut c3 = Curve::from(BSplineCurve::create_line_segment(
                3,
                chamfer_start,
                chamfer_end,
            )?);

            // trim off unnecessary pieces
            let range1 = c1.natural_interval();
            let range2 = c2.natural_interval();

            // Intersect the first offset with the chamfer; trim it
            let (s1, p31) = first_intersection(k, &c1, &c3, xsect_tol_3d)?;
            let mut trim1 = Extent1d::from_min_max(range1.min(), s1);
            c1.trim(k, &mut trim1, TrimMode::Notify)?;

            // Intersect the second offset with the chamfer; trim it
            let (s2, p32) = first_intersection(k, &c2, &c3, xsect_tol_3d)?;
            let mut trim2 = Extent1d::from_min_max(range2.min(), s2);
            c2.trim(k, &mut trim2, TrimMode::Notify)?;

            // Trim the chamfer
            let mut trim3 = Extent1d::new();
            if p31 < p32 {
                let _ = trim3.set_min_max(p31, p32);
            } else {
                let _ = trim3.set_min_max(p32, p31);
            }
            c3.trim(k, &mut trim3, TrimMode::Notify)?;

            out.curve1 = Some(c1);
            out.curve2 = Some(c2);
            out.curve3 = Some(c3);
        }
        OffsetCornerType::CurveExtension => {
            // No other corner cases implemented yet
            return Err(SolidError::sm_err(OP));
        }
    }
    Ok(out)
}

/// The curve handle of [`curve_classify`]'s point solves.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OrigHandle(Extent1d);

impl SolutionObject for OrigHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

/// `sm_CurveClassify`: whether to keep the trimmed offset curve `trimmed_offset`.
/// With its `originator` (the offset map attribute's original curve), a curve whose
/// mid-parameter tangent opposes the originator's there is a bow tie (discard).
/// Otherwise the curve is discarded when its mid point lies closer than
/// `offset_distance * (1 - SM_EFF_ZERO)` to any of `orig_curves` (minimum distance
/// solves within `offset_distance + 2 xsect_tol_3d`).
///
/// # Errors
///
/// Those of the mid point evaluation and the point solves; [`SolidError::Unported`]
/// from the tangent evaluations (their other failures are ignored, as upstream).
pub fn curve_classify(
    k: &Kernel,
    trimmed_offset: &Curve,
    offset_distance: f64,
    xsect_tol_3d: f64,
    orig_curves: &[&Curve],
    originator: Option<&Curve>,
) -> Result<bool> {
    curve_classify_cached(
        k,
        trimmed_offset,
        offset_distance,
        xsect_tol_3d,
        orig_curves,
        originator,
        &mut OrigCurveCaches::default(),
    )
}

/// The curve caches of the original curves of [`curve_classify`]'s point solves,
/// kept across calls for the same original curves (performance only: upstream keeps
/// each cache on its curve; see [`Curve::global_point_solve_cached`]). Slot `i`
/// belongs to `orig_curves[i]`, which must not change while the slots are used.
#[derive(Debug, Default)]
pub(crate) struct OrigCurveCaches(Vec<Option<CurveCache<OrigHandle>>>);

/// [`curve_classify`] with the original curves' caches kept in `caches` (see
/// [`OrigCurveCaches`]): a cache is built where a solve first needs it and reused by
/// later calls with the same original curves (a cache's handle is the curve's
/// natural interval, constant while the curve is).
pub(crate) fn curve_classify_cached(
    k: &Kernel,
    trimmed_offset: &Curve,
    offset_distance: f64,
    xsect_tol_3d: f64,
    orig_curves: &[&Curve],
    originator: Option<&Curve>,
    caches: &mut OrigCurveCaches,
) -> Result<bool> {
    if caches.0.len() < orig_curves.len() {
        caches.0.resize_with(orig_curves.len(), || None);
    }

    // locals
    let ivl = trimmed_offset.natural_interval();

    // Quick check of the mid point for being in a bow-tie.
    if let Some(orig) = originator {
        let mid = ivl.evaluate(0.5);
        let pv_this = ignored(trimmed_offset.evaluate(k, mid, 1, false, true))?.unwrap_or_default();
        let pv_orig = ignored(orig.evaluate(k, mid, 1, false, true))?.unwrap_or_default();
        let d_this = at(&pv_this, 1);
        let d_orig = at(&pv_orig, 1);
        if d_this.dot(d_orig) < 0.0 {
            return Ok(false);
        }
    }

    // Use something that is plenty big enough to cover numerical noise.
    let cutoff = offset_distance * (1.0 - EFF_ZERO);

    // For GlobalPointSolve:
    let min_dist = offset_distance + 2.0 * xsect_tol_3d;

    // drop the mid point onto every original curve
    let pnt = trimmed_offset.evaluate_point(k, ivl.evaluate(0.5))?;
    let samples = [pnt];

    for (orig, cache) in orig_curves.iter().zip(caches.0.iter_mut()) {
        for &p in &samples {
            let oi = orig.natural_interval();
            let opts = GlobalPointSolveOptions {
                target_distance: Some(min_dist),
                handle: Some(OrigHandle(oi)),
                ..GlobalPointSolveOptions::default()
            };
            let sols = orig.global_point_solve_cached(
                k,
                &oi,
                SolverOperation::Minimize,
                p,
                xsect_tol_3d,
                SolutionRequested::Single,
                &opts,
                cache,
            )?;
            if let Some(s) = sols.get(0) {
                if s.start.solution_value < cutoff {
                    // Within offset distance of this curve.
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

// ---- sorted break lists ---------------------------------------------------------------

/// `sm_TrimFirstBreak`: puts `value` into the increasing `breaks` as the first
/// entry, removing the entries before it (an entry `SM_ARE_SAME` as `value` is
/// replaced); a `value` above every entry leaves only itself.
pub fn trim_first_break(breaks: &mut Vec<f64>, value: f64) {
    for ii in 0..breaks.len() {
        let Some(&b) = breaks.get(ii) else {
            break;
        };
        // replace repeated values
        if are_same(value, b) {
            if let Some(slot) = breaks.get_mut(ii) {
                *slot = value;
            }
            breaks.drain(..ii);
            return;
        }

        // insert prior to first value larger than NewDouble
        if value < b {
            breaks.insert(ii, value);
            breaks.drain(..ii);
            return;
        }
    }

    // larger than all entries - replace array
    breaks.clear();
    breaks.push(value);
}

/// `sm_TrimLastBreak`: puts `value` into the increasing `breaks` as the last entry,
/// removing the entries after it (an entry `SM_ARE_SAME` as `value` is replaced); a
/// `value` above every entry is appended.
pub fn trim_last_break(breaks: &mut Vec<f64>, value: f64) {
    for ii in 0..breaks.len() {
        let Some(&b) = breaks.get(ii) else {
            break;
        };
        // replace repeated values
        if are_same(value, b) {
            if let Some(slot) = breaks.get_mut(ii) {
                *slot = value;
            }
            breaks.truncate(ii + 1);
            return;
        }

        // insert prior to first value larger than NewDouble
        if value < b {
            breaks.insert(ii, value);
            breaks.truncate(ii + 1);
            return;
        }
    }

    // larger than all entries - add to end of array
    breaks.push(value);
}

/// `sm_InsertSortedBreaks`: inserts `value` into the increasing `breaks` before the
/// first larger entry, unless an entry is `SM_ARE_SAME` as it.
pub fn insert_sorted_breaks(breaks: &mut Vec<f64>, value: f64) {
    for ii in 0..breaks.len() {
        let Some(&b) = breaks.get(ii) else {
            break;
        };
        // skip repeated values
        if are_same(value, b) {
            return;
        }

        if value < b {
            breaks.insert(ii, value);
            return;
        }
    }

    // larger than all entries - add to end of array
    breaks.push(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::hermite_curve::HermiteCurve;

    fn line(a: [f64; 3], b: [f64; 3]) -> Curve {
        Curve::from(
            BSplineCurve::create_line_segment(3, Vec3::from_array(a), Vec3::from_array(b)).unwrap(),
        )
    }

    #[test]
    fn set_segment_rejects_excessive_nesting_without_changing_composite() {
        use crate::surface::surface::p2::MAX_GEOMETRY_NESTING;

        let base = line([0.0; 3], [1.0, 0.0, 0.0]);
        let mut composite =
            CompositeCurve::with_curves(3, vec![base.clone()], false, None, None).unwrap();
        let mut curve = base;
        for _ in 0..MAX_GEOMETRY_NESTING {
            curve = Curve::from(
                CompositeCurve::with_curves(3, vec![curve], false, None, None).unwrap(),
            );
        }
        let mut segment = composite.segment(0).unwrap().clone();
        segment.curve = curve;
        let before = format!("{composite:?}");
        assert!(matches!(
            composite.set_segment(0, segment),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(format!("{composite:?}"), before);
        let mut segment = composite.segment(0).unwrap().clone();
        segment.same_sense = false;
        assert!(composite.set_segment(1, segment.clone()).is_err());
        composite.set_segment(0, segment).unwrap();
        assert!(!composite.segment(0).unwrap().same_sense);
    }

    #[test]
    fn segment_b_spline_edits_in_place_and_keeps_the_owner() {
        let owner = ObjectRef {
            type_code: 7,
            key: attribute::ObjectKey(3),
        };
        let mut owned = line([0.0; 3], [1.0, 0.0, 0.0]);
        let _ = owned.set_owner(Some(owner), None);
        let mut composite = CompositeCurve::with_curves(3, vec![owned], false, None, None).unwrap();
        *composite.segment_b_spline_mut(0).unwrap() =
            BSplineCurve::create_line_segment(3, Vec3::ZERO, Vec3::Y).unwrap();
        let curve = &composite.segment(0).unwrap().curve;
        assert_eq!(curve.owner(), Some(owner));
        let CurveKind::BSpline(b) = curve.kind() else {
            panic!("segment kind changed");
        };
        assert_eq!(b.evaluate_point(1.0).unwrap(), Vec3::Y);
        assert!(composite.segment_b_spline_mut(1).is_none());
    }

    #[test]
    fn natural_interval_stacks_segments() {
        let c = CompositeCurve::with_curves(
            3,
            vec![
                line([0.0; 3], [1.0, 0.0, 0.0]),
                line([1.0, 0.0, 0.0], [1.0, 2.0, 0.0]),
            ],
            false,
            None,
            None,
        )
        .unwrap();
        let a = c.segments()[0].curve.natural_interval();
        let b = c.segments()[1].curve.natural_interval();
        let ivl = c.natural_interval();
        assert_eq!(ivl.min(), a.min());
        assert_eq!(ivl.max(), a.max() + b.length());
        assert!(CompositeCurve::new(3).natural_interval().is_init());
    }

    #[test]
    fn gaps_follow_upstream() {
        let curves = vec![
            line([0.0; 3], [1.0, 0.0, 0.0]),
            line([1.0, 0.0, 0.0], [2.0, 0.0, 0.0]),
        ];
        let c = CompositeCurve::with_curves(
            3,
            curves.clone(),
            true,
            Some(&[true, false]),
            Some(&[0.1, 0.2]),
        )
        .unwrap();
        assert_eq!(c.segments()[0].gap_start, 0.2);
        assert_eq!(c.segments()[1].gap_start, 0.1);
        assert!(!c.segments()[1].same_sense);
        let o = CompositeCurve::with_curves(3, curves, false, None, Some(&[0.1, 0.2])).unwrap();
        assert_eq!(o.segments()[0].gap_start, BIG_DOUBLE);
        assert!(CompositeCurve::with_curves(3, vec![], true, None, Some(&[])).is_err());
    }

    #[test]
    fn breaks() {
        let mut v = vec![0.0, 1.0, 2.0, 3.0];
        trim_first_break(&mut v, 1.5);
        assert_eq!(v, vec![1.5, 2.0, 3.0]);
        let mut v = vec![0.0, 1.0, 2.0, 3.0];
        trim_last_break(&mut v, 2.0);
        assert_eq!(v, vec![0.0, 1.0, 2.0]);
        let mut v = vec![0.0, 1.0];
        insert_sorted_breaks(&mut v, 0.5);
        insert_sorted_breaks(&mut v, 1.0);
        assert_eq!(v, vec![0.0, 0.5, 1.0]);
        let mut v = vec![0.0, 1.0];
        trim_first_break(&mut v, 5.0);
        assert_eq!(v, vec![5.0]);
    }

    #[test]
    fn build_square() {
        let k = &Kernel::default();
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let curves = vec![
            line(pts[0], pts[1]),
            line(pts[2], pts[3]),
            line(pts[2], pts[1]),
            line(pts[3], pts[0]),
        ];
        let r = CompositeCurve::build_composites_from_curves(k, curves, 1e-6, 0.0).unwrap();
        assert_eq!(r.composites.len(), 1);
        assert!(r.composites[0].is_closed());
        assert_eq!(r.composites[0].num_segments(), 4);
        assert!(r.unused.is_empty());
    }

    #[test]
    fn orientation_of_square() {
        let k = &Kernel::default();
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let curves = vec![
            line(pts[0], pts[1]),
            line(pts[1], pts[2]),
            line(pts[2], pts[3]),
            line(pts[3], pts[0]),
        ];
        let o = CompositeCurve::compute_projected_loop_orientation(
            k,
            &curves,
            &[true; 4],
            Vec3::new(0.0, 0.0, 1.0),
            None,
        )
        .unwrap();
        assert_eq!(o, OrientType::Same);
        let o = CompositeCurve::compute_projected_loop_orientation(
            k,
            &curves,
            &[true; 4],
            Vec3::new(0.0, 0.0, -1.0),
            None,
        )
        .unwrap();
        assert_eq!(o, OrientType::Opposite);
    }

    #[test]
    fn hermite_copy_fails() {
        let h = Curve::from(HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            3,
        ));
        let c = CompositeCurve::with_curves(3, vec![h], false, None, None).unwrap();
        assert!(c.copy(&Kernel::default()).is_err());
        assert!(is_kind_of(TYPE));
        assert!(region_is_kind_of(REGION_TYPE));
    }
}
