//! `SmBSplineCurve`, part 1: the curve object (an NLib curve plus the image-space
//! dimension and the STEP form flags), its constructors and copies, factories for
//! canonical, degenerate, line, circle and three-point arc curves, mirror and plane
//! projection copies, evaluation with derivatives, end point and parameterisation
//! editing, control polygon, knot and Greville queries, knot clean-up, affine
//! transformation and the static fair-blend tangent scaling.
//!
//! Upstream: `source/SMLib/src/SmBSplineCurve.cpp`, `source/SMLib/inc/SmBSplineCurve.h`.
//!
//! | Rust                                         | Upstream                                          |
//! |----------------------------------------------|---------------------------------------------------|
//! | [`BSplineCurve`]                             | `SmBSplineCurve`                                  |
//! | [`BSplineCurve::new`]                        | `SmBSplineCurve(ULONG lDim)`                      |
//! | [`BSplineCurve::from_nurb`]                  | `SmBSplineCurve(gw_CURVE*, ULONG, SmBoolean, const SmContext*)` |
//! | [`BSplineCurve::from_data`]                  | `SmBSplineCurve(ULONG, ULONG n, ULONG m, short p, gw_CPOINT*, double*, const SmContext*)` |
//! | [`BSplineCurve::from_gw_nurb`]               | `SmBSplineCurve(ULONG, const gw_CURVE*)`          |
//! | [`BSplineCurve::copy`], `Clone`              | copy constructor, `Copy` (both overloads)         |
//! | `Drop` (implicit)                            | `~SmBSplineCurve`                                 |
//! | [`BSplineCurve::approx_eq`]                  | `operator==`                                      |
//! | [`split_quadratic_bezier`]                   | `sm_SplitQuadraticBezier` (static)                |
//! | [`split_cubic_bezier`]                       | `sm_SplitCubicBezier` (static)                    |
//! | [`BSplineCurve::calculate_continuities`]     | `CalculateContinuities`                           |
//! | [`BSplineCurve::convert_to_2d`], [`BSplineCurve::convert_to_3d`] | `ConvertTo2D`, `ConvertTo3D`  |
//! | [`BSplineCurve::create_arc_from_points`]     | `CreateArcFromPoints`                             |
//! | [`BSplineCurve::create_canonical`], [`CanonicalData`] | `CreateCanonical`                        |
//! | [`BSplineCurve::create_circle_segment`]      | `CreateCircleSegment`                             |
//! | [`BSplineCurve::create_degenerate_curve`]    | `CreateDegenerateCurve`                           |
//! | [`BSplineCurve::create_line_segment`]        | `CreateLineSegment`                               |
//! | [`BSplineCurve::create_mirror_curve`]        | `CreateMirrorCurve`                               |
//! | [`BSplineCurve::create_plane_projection`]    | `CreatePlaneProjection`                           |
//! | [`BSplineCurve::edit_parameterization`]      | `EditParameterization`                            |
//! | [`BSplineCurve::edit_end_point`]             | `EditEndPoint`                                    |
//! | [`BSplineCurve::evaluate`]                   | `Evaluate`                                        |
//! | [`BSplineCurve::evaluate_point`]             | `EvaluatePoint`                                   |
//! | [`BSplineCurve::fair_blend_curve_derivatives`] | `FairBlendCurveDerivatives`                     |
//! | [`BSplineCurve::fixup_knot_vector`]          | `FixupKnotVector`                                 |
//! | [`BSplineCurve::control_polygon`]            | `GetControlPolygon`                               |
//! | [`BSplineCurve::control_points`]             | `GetControlPointsPointer`                         |
//! | [`BSplineCurve::degree`]                     | `GetDegree`                                       |
//! | [`BSplineCurve::ends`], [`Ends`]             | `GetEnds`                                         |
//! | [`BSplineCurve::knots`]                      | `GetKnots`                                        |
//! | [`BSplineCurve::number_natural_knots`]       | `GetNumberNaturalKnots`                           |
//! | [`BSplineCurve::number_control_points`]      | `GetNumberControlPoints`                          |
//! | [`BSplineCurve::greville_abscissa`]          | `GetGrevilleAbscissa`                             |
//! | [`BSplineCurve::is_rational`]                | `IsRational`                                      |
//! | [`BSplineCurve::natural_interval`]           | `GetNaturalInterval`                              |
//! | [`BSplineCurve::step_interval`]              | `GetSTEPInterval`                                 |
//! | [`BSplineCurve::make_nurb`]                  | `MakeNurb`                                        |
//! | [`BSplineCurve::remove_extra_knots`]         | `RemoveExtraKnots`                                |
//! | [`BSplineCurve::set_from_gw_nurb`]           | `SetFromGwNurb`                                   |
//! | [`BSplineCurve::tessellate`], [`Tessellation`] | `Tessellate`                                    |
//! | [`BSplineCurve::transform`]                  | `Transform`                                       |
//! | [`BSplineCurve::is_kind_of`]                 | `IsKindOf`                                        |
//! | [`BSplineCurve::TYPE`], [`BSplineCurve::type_code`], [`BSplineCurve::type_string`], [`BSplineCurve::class_string`] | `GetType`, `GetClassType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`BSplineCurve::out_of_bounds_enabled_mut`]  | `GetOutOfBoundsEnabled`                           |
//! | [`BSplineCurve::is_out_of_bounds_enabled`]   | `IsOutOfBoundsEnabled`                            |
//! | [`BSplineCurve::set_out_of_bounds_enabled`]  | `SetOutOfBoundsEnabled`                           |
//! | [`BSplineCurve::inside_out`]                 | `GetInsideOut`                                    |
//! | [`BSplineCurve::gw_nurb`]                    | `GetGwNurbPointer`                                |
//! | [`BSplineCurve::get_or_create_gw_nurb`]      | `GetOrCreateGwNurbPointer`                        |
//! | [`BSplineCurve::is_nurb_borrowed`]           | `IsNurbBorrowed`                                  |
//! | [`BSplineCurve::refresh_analytics`]          | `RefreshAnalytics`                                |
//! | [`KnotType`], [`BSplineCurveForm`], [`NurbCircleParam`], [`ProjectionType`] | `SmKnotType`, `SmBSplineCurveForm`, `SmNurbCircleParam`, `SmProjectionType` |
//!
//! Deviations:
//! - **Curve base.** `SmBSplineCurve` derives from `SmCurve` (units `curve.curve_*`).
//!   The base state it uses is the image-space dimension (`m_lDim`), kept here as
//!   [`BSplineCurve::dim`] (`SmCurve::GetDim`). `SmCurve::operator==` (same type,
//!   same dimension) is the dimension test of [`BSplineCurve::approx_eq`], the type
//!   test being the Rust type. A curve has no owner, attributes or caches here, so the
//!   `Notify(SM_NO_PRE_EDIT / SM_NO_POST_EDIT / SM_NO_CONSTRUCTION)` calls (which
//!   only drop curve caches and notify owners) have nothing to do and are omitted.
//! - **Derived classes.** Upstream `MakeNurb`, `RefreshAnalytics` and `IsKindOf` are
//!   virtual and overridden by the analytic curves (`SmLine`, `SmEllipse`,
//!   `SmParabola`, `SmHyperbola`, units `curve.line`, `curve.ellipse_*`, ...), which
//!   build their NURB lazily. Those types are not ported yet; this type is the plain
//!   B-spline, whose `MakeNurb` cannot create a NURB. `EditEndPoint`'s
//!   `IsKindOf(SmEllipse_TYPE)` check only logs a message and is false for a plain
//!   B-spline; it is omitted.
//! - **Missing NURB.** Upstream callers of `GetOrCreateGwNurbPointer` dereference
//!   the `NULL` it returns for a curve built without NURB data (the
//!   `SmBSplineCurve(ULONG)` constructor) and crash; here they return
//!   [`SolidError::Upstream`] `SM_ERR` (the status of the failed `MakeNurb`).
//!   `GetNaturalInterval` returns `[0, 0]` and `RemoveExtraKnots` succeeds there, as
//!   upstream.
//! - **Ownership.** The NURB is owned ([`Curve`]); the "borrowed" flag of the fast
//!   constructor only skips the knot fix-up and is reported by
//!   [`BSplineCurve::is_nurb_borrowed`]. `Clone` is the copy constructor (it applies
//!   the knot fix-up to the copy, as upstream) rather than a field-by-field clone.
//! - **Context.** The `SmContext` arguments only choose where upstream allocates the
//!   new object and are dropped. Arguments upstream marks `NotUsed`
//!   (`CreateArcFromPoints`' dimension, `EditEndPoint`'s end type and heal flag) are
//!   dropped too.
//! - **Errors.** Out-params become return values. A failing NLib routine becomes
//!   `SM_ERR` ([`SolidError::Upstream`], code 1001), as upstream's
//!   `if(NL_YES == ...) SER(SM_ERR)`; `SM_ERR_INVALID_INPUT` checks are
//!   [`SolidError::InvalidInput`]. Where upstream would index outside its arrays
//!   (fewer multiplicities than knots in `CreateCanonical`, too few control points
//!   or knots for the constructor) the port returns `InvalidInput`.
//! - **Canonical knots.** `CreateCanonical` expands the knots by the caller's
//!   multiplicities with `SmTArray::Add`, whose failed growth is ignored (the array is
//!   memory-dependent then). The port keeps the expansion implicit and materialises
//!   only the knots `N_Iges126Crv` reads (`0..=n + p + 1`) plus the interval knots
//!   it looks up, so an absurd multiplicity gives the same result (or the same "too
//!   few knots" error) without allocating; a total count above `usize::MAX` is
//!   `InvalidInput`.
//! - **Enums.** `SmNurbCircleParam` and `SmProjectionType` are Rust enums, so
//!   upstream's "not one of the values" branches only exist for
//!   [`ProjectionType::Rotation`] and [`ProjectionType::Unknown`].
//! - **Evaluate.** `Evaluate`'s recursive call for a zero first derivative with one
//!   derivative asked binds the `SmBSplineCurve.h` default `bNonZeroTangents =
//!   FALSE`, so it hands back the exact (zero) derivative, as here. It ignores the
//!   call's status; if it failed, upstream copies its default-constructed
//!   `SmVector3d` (`SM_UNDEF_DOUBLE`), so the port uses [`Vec3::UNDEF`] there.
//! - **Back-edges.** `CalculateContinuities` calls `SmCurve::EvaluateContinuity`
//!   and `Tessellate` calls `SmCurve::ApproximateLength` (both in
//!   [`crate::curve::curve::p2`]) and `SmCurve::TessellateByBisection`
//!   ([`crate::curve::curve::Curve::tessellate_by_bisection`], run on a
//!   [`crate::curve::curve::Curve`] made of a copy of the B-spline).
//!   `IsKindOf` adds `SmBSplineCurve_TYPE` to `SmCurve::IsKindOf`.

use crate::curve::curve::p2::BSPLINE_CURVE_TYPE;
use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p1::line_plane_intersect;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::polynomial::solve_quadratic;
use crate::geom::tol::{are_same, is_zero};
use crate::geom::vector3d::{Point3, Vec3};
use crate::math::{min2, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::basis_basic::reparam;
use crate::nurbs::crv_basic::{ConicType, Curve};
use crate::nurbs::crv_common::{circ_arc, Arc};
use crate::nurbs::crv_nurb::{line_from_pt_and_vector, Projection};
use crate::nurbs::geom::Interval;
use crate::nurbs::iges::{iges_126_curve, Entity126};
use crate::nurbs::knots::Side;
use crate::nurbs::math::{CPoint, MatrixType, RealMatrix, NOW, NOZ};
use crate::nurbs::nurbs_crv::{
    allocate_nurb_curve, copy_nurb_curve, crv_derivs, crv_derivs_into, crv_eval, crv_remove_knots,
    extract_frame, fixup_knot_vector, get_knots, to_euclid, MAX_DERIV,
};

const OP_DATA: &str = "nurbs::b_spline_curve::p1::BSplineCurve::from_data";
const OP_NURB: &str = "nurbs::b_spline_curve::p1::BSplineCurve::make_nurb";
const OP_CONT: &str = "nurbs::b_spline_curve::p1::BSplineCurve::calculate_continuities";
const OP_ARC: &str = "nurbs::b_spline_curve::p1::BSplineCurve::create_arc_from_points";
const OP_CANON: &str = "nurbs::b_spline_curve::p1::BSplineCurve::create_canonical";
const OP_CIRCLE: &str = "nurbs::b_spline_curve::p1::BSplineCurve::create_circle_segment";
const OP_DEGEN: &str = "nurbs::b_spline_curve::p1::BSplineCurve::create_degenerate_curve";
const OP_LINE: &str = "nurbs::b_spline_curve::p1::BSplineCurve::create_line_segment";
const OP_PROJ: &str = "nurbs::b_spline_curve::p1::BSplineCurve::create_plane_projection";
const OP_REPARAM: &str = "nurbs::b_spline_curve::p1::BSplineCurve::edit_parameterization";
const OP_EDIT_END: &str = "nurbs::b_spline_curve::p1::BSplineCurve::edit_end_point";
const OP_EVAL: &str = "nurbs::b_spline_curve::p1::BSplineCurve::evaluate";
const OP_EVAL_PT: &str = "nurbs::b_spline_curve::p1::BSplineCurve::evaluate_point";
const OP_FAIR: &str = "nurbs::b_spline_curve::p1::BSplineCurve::fair_blend_curve_derivatives";
const OP_GREVILLE: &str = "nurbs::b_spline_curve::p1::BSplineCurve::greville_abscissa";
const OP_XFORM: &str = "nurbs::b_spline_curve::p1::BSplineCurve::transform";

/// `SmKnotType`: the STEP knot type of a B-spline curve; discriminants are the
/// upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum KnotType {
    /// `SM_KT_UNIFORM_KNOTS`.
    UniformKnots = 0,
    /// `SM_KT_UNSPECIFIED`.
    #[default]
    Unspecified = 1,
    /// `SM_KT_QUASI_UNIFORM_KNOTS`.
    QuasiUniformKnots = 2,
    /// `SM_KT_PIECEWISE_BEZIER_KNOTS`.
    PiecewiseBezierKnots = 3,
    /// `SM_KT_UNKOWN`.
    Unknown = 4,
}

/// `SmBSplineCurveForm`: the STEP form of a B-spline curve; discriminants are the
/// upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BSplineCurveForm {
    /// `SM_CF_POLYLINE_FORM`.
    PolylineForm = 0,
    /// `SM_CF_CIRCULAR_ARC`.
    CircularArc = 1,
    /// `SM_CF_ELLIPTIC_ARC`.
    EllipticArc = 2,
    /// `SM_CF_PARABOLIC_ARC`.
    ParabolicArc = 3,
    /// `SM_CF_HYPERBOLIC_ARC`.
    HyperbolicArc = 4,
    /// `SM_CF_HELICAL_ARC`.
    HelicalArc = 5,
    /// `SM_CF_UNSPECIFIED`.
    #[default]
    Unspecified = 6,
}

/// `SmNurbCircleParam`: the NURBS form of a circle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NurbCircleParam {
    /// `SM_CO_QUADRATIC` (0): degree 2.
    Quadratic = 0,
    /// `SM_CO_QUINTIC` (1): degree 5.
    Quintic = 1,
}

/// `SmProjectionType`; discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectionType {
    /// `SM_PT_PARALLEL`: project along a direction.
    Parallel = 0,
    /// `SM_PT_PERSPECTIVE`: project from an eye point.
    Perspective = 1,
    /// `SM_PT_ROTATION` (not supported by [`BSplineCurve::create_plane_projection`]).
    Rotation = 2,
    /// `SM_PT_UNKNOWN` (not supported by [`BSplineCurve::create_plane_projection`]).
    Unknown = 3,
}

impl ProjectionType {
    /// The enumerator of a stored value (`(SmProjectionType) lValue`), `None` outside
    /// the enum.
    #[must_use]
    pub const fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            0 => Self::Parallel,
            1 => Self::Perspective,
            2 => Self::Rotation,
            3 => Self::Unknown,
            _ => return None,
        })
    }
}

/// End points and end weights of a curve ([`BSplineCurve::ends`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ends {
    /// Start point (Euclidean).
    pub start: Point3,
    /// End point (Euclidean).
    pub end: Point3,
    /// Start weight (`1` for a non-rational curve).
    pub start_weight: f64,
    /// End weight (`1` for a non-rational curve).
    pub end_weight: f64,
}

/// Tessellation of a curve ([`BSplineCurve::tessellate`]): parameters, points and
/// tangents of the sample points.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tessellation {
    /// Parameters of the sample points.
    pub parameters: Vec<f64>,
    /// Sample points.
    pub points: Vec<Point3>,
    /// Tangents at the sample points.
    pub tangents: Vec<Vec3>,
}

/// The STEP-style data of [`BSplineCurve::create_canonical`].
#[derive(Debug, Clone, Copy)]
pub struct CanonicalData<'a> {
    /// Image-space dimension, 1 to 3.
    pub dim: u64,
    /// Degree, 1 to 32.
    pub degree: usize,
    /// Euclidean control points.
    pub control_points: &'a [Point3],
    /// STEP form of the new curve.
    pub form: BSplineCurveForm,
    /// Multiplicity of each unique knot.
    pub knot_multiplicities: &'a [usize],
    /// Unique knots in ascending order.
    pub knots: &'a [f64],
    /// STEP knot type of the new curve.
    pub knot_type: KnotType,
    /// Optional weight of each control point.
    pub weights: Option<&'a [f64]>,
    /// Optional trimming interval.
    pub trim: Option<Extent1d>,
}

/// A B-spline (NURBS) curve (`SmBSplineCurve`): an NLib curve with its image-space
/// dimension and STEP form flags.
///
/// Knot vectors are clamped: `U = {a,...,a, u[p+1], ..., b,...,b}` with end
/// multiplicity `p + 1`, `m = n + p + 1`.
#[derive(Debug)]
pub struct BSplineCurve {
    /// Image-space dimension (`SmCurve::m_lDim`).
    pub(crate) dim: u64,
    /// The NURB (`m_pNurb`); `None` is `NULL`.
    pub(crate) nurb: Option<Curve>,
    /// `m_bNurbIsBorrowed`.
    pub(crate) nurb_is_borrowed: bool,
    /// `m_bOutOfBoundsEnabled`: evaluate beyond the natural interval.
    pub(crate) out_of_bounds_enabled: bool,
    /// `m_eKnotType`.
    pub(crate) knot_type: KnotType,
    /// `m_eBSplineCurveForm`.
    pub(crate) form: BSplineCurveForm,
}

impl Clone for BSplineCurve {
    /// The copy constructor (see [`BSplineCurve::copy`]).
    fn clone(&self) -> Self {
        self.copy()
    }
}

impl Default for BSplineCurve {
    /// A 3D curve without NURB data (`SmBSplineCurve()`).
    fn default() -> Self {
        Self::new(3)
    }
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// `InvalidInput` unless `ok` (upstream's `*_ER` input checks, which fail for NaN).
fn check(ok: bool, operation: &'static str, reason: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(invalid(operation, reason))
    }
}

/// `SM_CPOINT_AVE`: the midpoint of two control points; `z` / `w` stay missing when
/// the first point's are.
fn cpoint_ave(a1: CPoint, a2: CPoint) -> CPoint {
    let z = if a1.z == NOZ {
        NOZ
    } else {
        (a1.z + a2.z) / 2.0
    };
    let w = if a1.w == NOW {
        NOW
    } else {
        (a1.w + a2.w) / 2.0
    };
    CPoint::new((a1.x + a2.x) / 2.0, (a1.y + a2.y) / 2.0, z, w)
}

/// Splits a quadratic Bezier control polygon at its middle (`sm_SplitQuadraticBezier`).
#[must_use]
pub fn split_quadratic_bezier(cp1: CPoint, cp2: CPoint, cp3: CPoint) -> [CPoint; 5] {
    let p1 = cpoint_ave(cp1, cp2);
    let p3 = cpoint_ave(cp2, cp3);
    let p2 = cpoint_ave(p1, p3);
    [cp1, p1, p2, p3, cp3]
}

/// Splits a cubic Bezier control polygon at its middle (`sm_SplitCubicBezier`).
#[must_use]
pub fn split_cubic_bezier(cp1: CPoint, cp2: CPoint, cp3: CPoint, cp4: CPoint) -> [CPoint; 7] {
    let p1 = cpoint_ave(cp1, cp2);
    let p5 = cpoint_ave(cp3, cp4);
    let mid = cpoint_ave(cp2, cp3);
    let p2 = cpoint_ave(p1, mid);
    let p4 = cpoint_ave(mid, p5);
    let p3 = cpoint_ave(p2, p4);
    [cp1, p1, p2, p3, p4, p5, cp4]
}

/// `SmCurve::EvaluateContinuity` of `this` at `this_param` against `other` at
/// `other_param`.
fn evaluate_continuity(
    k: &crate::kernel::Kernel,
    this: &BSplineCurve,
    this_param: f64,
    this_from_left: bool,
    other: &BSplineCurve,
    other_param: f64,
    other_from_left: bool,
    angle_tol: f64,
) -> Result<ContinuityType> {
    Ok(crate::curve::curve::p2::evaluate_continuity(
        k,
        this,
        this_param,
        this_from_left,
        other,
        other_param,
        other_from_left,
        angle_tol,
    )?
    .continuity)
}

/// `SmCurve::ApproximateLength` of `curve` over `interval` with `samples` samples.
fn approximate_length(
    k: &crate::kernel::Kernel,
    curve: &BSplineCurve,
    interval: &Extent1d,
    samples: usize,
) -> Result<f64> {
    Ok(crate::curve::curve::p2::approximate_length(k, curve, interval, samples, false)?.0)
}

/// `SmCurve::TessellateByBisection` of `curve` (through the [`crate::curve::curve::Curve`]
/// dispatch of its virtual queries, all B-spline overrides).
fn tessellate_by_bisection(
    k: &crate::kernel::Kernel,
    curve: &BSplineCurve,
    interval: &Extent1d,
    chord_height_tol: f64,
    angle_tol_deg: f64,
    max_length: f64,
) -> Result<Tessellation> {
    crate::curve::curve::Curve::from(curve.clone()).tessellate_by_bisection(
        k,
        interval,
        chord_height_tol,
        angle_tol_deg,
        max_length,
    )
}

impl BSplineCurve {
    /// `SmBSplineCurve_TYPE` (`CURV_BASE_TYPE + 33`).
    pub const TYPE: i64 = BSPLINE_CURVE_TYPE;

    // ---- construction -----------------------------------------------------------------

    /// A curve of dimension `dim` without NURB data (`SmBSplineCurve(ULONG lDim)`);
    /// only the set methods can give it one.
    #[must_use]
    pub const fn new(dim: u64) -> Self {
        Self {
            dim,
            nurb: None,
            nurb_is_borrowed: false,
            out_of_bounds_enabled: false,
            knot_type: KnotType::Unspecified,
            form: BSplineCurveForm::Unspecified,
        }
    }

    /// The curve over an existing NURB (`SmBSplineCurve(gw_CURVE*, lDim,
    /// bNurbIsBorrowed)`). An owned (not borrowed) NURB gets its knots fixed up
    /// (`sm_FixupKnotVector` with `SM_EFF_ZERO`).
    #[must_use]
    pub fn from_nurb(mut nurb: Curve, dim: u64, borrowed: bool) -> Self {
        // when m_pNurb is locally owned - tune it
        if !borrowed {
            fixup_knot_vector(nurb.knots_mut(), EFF_ZERO);
        }
        Self {
            nurb: Some(nurb),
            nurb_is_borrowed: borrowed,
            ..Self::new(dim)
        }
    }

    /// The curve over a copy of the control points `pw[0..=n]` and knots `u[0..=m]` of
    /// degree `p` (`SmBSplineCurve(lDim, n, m, p, Pw, U)`); the knots are fixed up.
    /// `m != n + p + 1` only draws a warning upstream and is accepted.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `pw` holds fewer than `n + 1` points or `u` fewer than
    /// `m + 1` knots (upstream reads past them).
    pub fn from_data(
        dim: u64,
        n: usize,
        m: usize,
        p: usize,
        pw: &[CPoint],
        u: &[f64],
    ) -> Result<Self> {
        let mut new_cur = allocate_nurb_curve(n, p, m)?;
        let src_u = u
            .get(..=m)
            .ok_or_else(|| invalid(OP_DATA, "fewer than m + 1 knots"))?;
        let src_pw = pw
            .get(..=n)
            .ok_or_else(|| invalid(OP_DATA, "fewer than n + 1 control points"))?;
        new_cur.knots_mut().copy_from_slice(src_u);
        new_cur.cpts_mut().copy_from_slice(src_pw);

        // Edit knot values: close knots become same and near knots are separated.
        fixup_knot_vector(new_cur.knots_mut(), EFF_ZERO);
        Ok(Self {
            nurb: Some(new_cur),
            ..Self::new(dim)
        })
    }

    /// The curve over a compact copy of `gw_nurb`, knots fixed up
    /// (`SmBSplineCurve(lDim, const gw_CURVE*)`).
    #[must_use]
    pub fn from_gw_nurb(dim: u64, gw_nurb: &Curve) -> Self {
        let mut nurb = copy_nurb_curve(gw_nurb);
        fixup_knot_vector(nurb.knots_mut(), EFF_ZERO);
        Self {
            nurb: Some(nurb),
            ..Self::new(dim)
        }
    }

    /// A copy that keeps the NURB exactly as it is: [`BSplineCurve::copy`] fixes the
    /// knot vector up, which can move near-coincident knots. For wrappers that stand in
    /// for upstream using the original object.
    #[must_use]
    pub fn clone_exact(&self) -> Self {
        Self {
            dim: self.dim,
            nurb: self.nurb.as_ref().map(copy_nurb_curve),
            nurb_is_borrowed: false,
            out_of_bounds_enabled: self.out_of_bounds_enabled,
            knot_type: self.knot_type,
            form: self.form,
        }
    }

    /// A copy (copy constructor, `Copy`): dimension, out-of-bounds flag, knot type and
    /// form are copied; the NURB (if any) is copied compactly and its knots are fixed
    /// up; the copy owns its NURB.
    #[must_use]
    pub fn copy(&self) -> Self {
        let nurb = self.nurb.as_ref().map(|src| {
            let mut nurb = copy_nurb_curve(src);
            fixup_knot_vector(nurb.knots_mut(), EFF_ZERO);
            nurb
        });
        Self {
            dim: self.dim,
            nurb,
            nurb_is_borrowed: false,
            out_of_bounds_enabled: self.out_of_bounds_enabled,
            knot_type: self.knot_type,
            form: self.form,
        }
    }

    /// Upstream `operator==`: same dimension and either both without NURB or NURBs
    /// equal within `SM_EFF_ZERO` (`N_CrvsAreEqual`).
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        // low work
        if core::ptr::eq(self, other) {
            return true;
        }

        // first check the base
        if self.dim != other.dim {
            return false;
        }

        // check equivalence of these objects
        match (&self.nurb, &other.nurb) {
            (None, None) => true,
            (Some(a), Some(b)) => core::ptr::eq(a, b) || a.equals(b, EFF_ZERO, EFF_ZERO),
            _ => false,
        }
    }

    // ---- simple access ----------------------------------------------------------------

    /// Image-space dimension (`SmCurve::GetDim`).
    #[must_use]
    pub const fn dim(&self) -> u64 {
        self.dim
    }

    /// The STEP knot type (`m_eKnotType`).
    #[must_use]
    pub const fn knot_type(&self) -> KnotType {
        self.knot_type
    }

    /// The STEP form (`m_eBSplineCurveForm`).
    #[must_use]
    pub const fn form(&self) -> BSplineCurveForm {
        self.form
    }

    /// The NURB, if any (`GetGwNurbPointer`).
    #[must_use]
    pub const fn gw_nurb(&self) -> Option<&Curve> {
        self.nurb.as_ref()
    }

    /// The NURB, made first when missing (`GetOrCreateGwNurbPointer`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when there is no NURB ([`BSplineCurve::make_nurb`] cannot make one).
    pub fn get_or_create_gw_nurb(&mut self) -> Result<&mut Curve> {
        if self.nurb.is_none() {
            // A plain B-spline cannot make its NURB; the status is ignored upstream.
            let _ = self.make_nurb();
        }
        self.nurb
            .as_mut()
            .ok_or_else(|| SolidError::sm_err(OP_NURB))
    }

    /// The NURB for read-only callers (`GetOrCreateGwNurbPointer` through a const
    /// cast), or `SM_ERR` without one (upstream dereferences `NULL`).
    pub(super) fn nurb_ref(&self) -> Result<&Curve> {
        self.nurb
            .as_ref()
            .ok_or_else(|| SolidError::sm_err(OP_NURB))
    }

    /// The NURB for editing, or `SM_ERR` without one (upstream dereferences `NULL`).
    pub(super) fn nurb_ref_mut(&mut self) -> Result<&mut Curve> {
        self.nurb
            .as_mut()
            .ok_or_else(|| SolidError::sm_err(OP_NURB))
    }

    /// Makes the NURB from the analytic data (`MakeNurb`); a plain B-spline has none.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when there is no NURB.
    pub fn make_nurb(&mut self) -> Result<()> {
        if self.nurb.is_none() {
            return Err(SolidError::sm_err(OP_NURB));
        }
        Ok(())
    }

    /// Whether the NURB is borrowed (`IsNurbBorrowed`).
    #[must_use]
    pub const fn is_nurb_borrowed(&self) -> bool {
        self.nurb_is_borrowed
    }

    /// Whether evaluation beyond the natural interval is enabled
    /// (`IsOutOfBoundsEnabled`).
    #[must_use]
    pub const fn is_out_of_bounds_enabled(&self) -> bool {
        self.out_of_bounds_enabled
    }

    /// The out-of-bounds flag, mutable (`GetOutOfBoundsEnabled`).
    pub fn out_of_bounds_enabled_mut(&mut self) -> &mut bool {
        &mut self.out_of_bounds_enabled
    }

    /// Enables or disables evaluation beyond the natural interval
    /// (`SetOutOfBoundsEnabled`).
    pub fn set_out_of_bounds_enabled(&mut self, enabled: bool) {
        self.out_of_bounds_enabled = enabled;
    }

    /// Always `false`: only lines and conics have an inside-out bit (`GetInsideOut`).
    #[must_use]
    pub const fn inside_out(&self) -> bool {
        false
    }

    /// Refreshes analytic data after a NURB edit (`RefreshAnalytics`); nothing to do
    /// for a plain B-spline.
    ///
    /// # Errors
    ///
    /// None for a plain B-spline.
    pub fn refresh_analytics(&mut self) -> Result<()> {
        Ok(())
    }

    /// `SmBSplineCurve_TYPE` (`GetType`, `GetClassType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmBSplineCurve_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmBSplineCurve_TYPE"
    }

    /// `"SmBSplineCurve"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmBSplineCurve"
    }

    /// True for `SmBSplineCurve_TYPE` and its base types (`IsKindOf`).
    ///
    /// # Errors
    ///
    /// None (kept fallible for callers written against the unported base).
    pub fn is_kind_of(&self, t: i64) -> Result<bool> {
        Ok(t == Self::TYPE || crate::curve::curve::p2::is_kind_of(t))
    }

    // ---- queries ----------------------------------------------------------------------

    /// `[U[0], U[m]]`, or `[0, 0]` without a NURB (`GetNaturalInterval`).
    #[must_use]
    pub fn natural_interval(&self) -> Extent1d {
        match &self.nurb {
            None => Extent1d::from_min_max(0.0, 0.0),
            Some(cur) => {
                let (u0, um) = cur.param_bounds();
                Extent1d::from_min_max(u0, um)
            }
        }
    }

    /// The natural interval (`GetSTEPInterval`).
    #[must_use]
    pub fn step_interval(&self) -> Extent1d {
        self.natural_interval()
    }

    /// The degree (`GetDegree`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn degree(&self) -> Result<usize> {
        Ok(self.nurb_ref()?.degree())
    }

    /// `m + 1`, the number of knots with repeats (`GetNumberNaturalKnots`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn number_natural_knots(&self) -> Result<usize> {
        Ok(self.nurb_ref()?.m() + 1)
    }

    /// `n + 1`, the number of control points (`GetNumberControlPoints`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn number_control_points(&self) -> Result<usize> {
        Ok(self.nurb_ref()?.n() + 1)
    }

    /// Whether the first control point has a weight (`IsRational`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn is_rational(&self) -> Result<bool> {
        let cur = self.nurb_ref()?;
        let first = cur
            .cpts()
            .first()
            .ok_or_else(|| invalid(OP_NURB, "empty control polygon"))?;
        Ok(first.w != NOW)
    }

    /// The control points as stored: `x y z w`, homogeneous for rational points, `z`
    /// [`NOZ`] for 2D points, `w` [`NOW`] for non-rational points
    /// (`GetControlPointsPointer`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn control_points(&self) -> Result<&[CPoint]> {
        Ok(self.nurb_ref()?.cpts())
    }

    /// The Euclidean control points (a missing `z` is `0`) and, for a rational curve,
    /// the weights (empty otherwise) (`GetControlPolygon`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn control_polygon(&self) -> Result<(Vec<Point3>, Vec<f64>)> {
        let pw = self.nurb_ref()?.cpts();
        let mut is_rational = false;
        let mut points = Vec::with_capacity(pw.len());
        let mut weights = vec![0.0; pw.len()];

        // For every Control Point
        for (p, w) in pw.iter().zip(weights.iter_mut()) {
            let mut pnt = Point3::new(p.x, p.y, if p.z == NOZ { 0.0 } else { p.z });

            // convert from homogeneous to cartesian coordinates
            if p.w != NOW {
                is_rational = true;
                *w = p.w;
                let w_reciprocal = 1.0 / p.w;
                pnt.x *= w_reciprocal;
                pnt.y *= w_reciprocal;
                pnt.z *= w_reciprocal;
            }
            points.push(pnt);
        }
        if !is_rational {
            weights.clear();
        }
        Ok((points, weights))
    }

    /// First and last control points (Euclidean) and their weights (`1` when
    /// non-rational) (`GetEnds`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn ends(&self) -> Result<Ends> {
        let pw = self.nurb_ref()?.cpts();
        let (Some(&first), Some(&last)) = (pw.first(), pw.last()) else {
            return Err(invalid(OP_NURB, "empty control polygon"));
        };
        let weight = |p: CPoint| if p.w == NOW { 1.0 } else { p.w };
        Ok(Ends {
            start: to_euclid(first),
            end: to_euclid(last),
            start_weight: weight(first),
            end_weight: weight(last),
        })
    }

    /// The unique knots and their multiplicities, optionally only those in `ivl`
    /// (`GetKnots`, through `sm_GetKnots`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn knots(&self, ivl: Option<&Extent1d>) -> Result<(Vec<f64>, Vec<usize>)> {
        let cur = self.nurb_ref()?;
        Ok(get_knots(&cur.knot_vector()?, ivl))
    }

    /// The Greville abscissa of control point `index`: the average of knots
    /// `index+1 ..= index+p`; the first knot for index 0, the last knot for the last
    /// index and above (`GetGrevilleAbscissa`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB; `InvalidInput` when the knots needed are missing
    /// (a malformed curve; upstream reads past its array).
    pub fn greville_abscissa(&self, index: usize) -> Result<f64> {
        let cur = self.nurb_ref()?;
        let num_pts = cur.n() + 1;
        let knots = cur.knots();
        let deg = cur.degree();
        let knot = |i: usize| {
            knots
                .get(i)
                .copied()
                .ok_or_else(|| invalid(OP_GREVILLE, "knot index out of range"))
        };

        if index < 1 {
            return knot(0); // at or below the beginning
        }
        if index >= num_pts - 1 {
            return knot(cur.m()); // at or beyond the end
        }

        let mut ret_val = 0.0;
        for i in 1..=deg {
            ret_val += knot(index + i)?;
        }
        ret_val /= deg as f64;
        Ok(ret_val)
    }

    /// The continuity at every unique knot (the ends are `SM_CT_DISCONTINUOUS`) and
    /// the lowest interior one (`SM_CT_CINFINITY` without interior knots)
    /// (`CalculateContinuities`; upstream default `angle_tol` is
    /// `SM_CONTINUITY_ANGLE`, 1 degree).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB; those of `SmCurve::EvaluateContinuity`.
    pub fn calculate_continuities(
        &self,
        k: &crate::kernel::Kernel,
        angle_tol: f64,
    ) -> Result<(ContinuityType, Vec<ContinuityType>)> {
        let (knots, _) = self.knots(None)?;
        let knot_count = knots.len();
        if knot_count == 0 {
            return Err(invalid(OP_CONT, "no knots"));
        }

        // init output - 1 continuity for each knot, end continuity values discontinuous
        let mut continuities = vec![ContinuityType::Discontinuous; knot_count];
        let mut min_continuity = ContinuityType::CInfinity;

        // for every internal knot
        for (i, &knot_val) in knots.iter().enumerate().take(knot_count - 1).skip(1) {
            let c = evaluate_continuity(k, self, knot_val, true, self, knot_val, false, angle_tol)?;
            if let Some(slot) = continuities.get_mut(i) {
                *slot = c;
            }
            // save the minimum internal knot continuity
            if (c as i32) < (min_continuity as i32) {
                min_continuity = c;
            }
        }
        Ok((min_continuity, continuities))
    }

    // ---- evaluation -------------------------------------------------------------------

    /// The point at `t` (`EvaluatePoint`). Outside the natural interval `t` is clamped
    /// to it unless out-of-bounds evaluation is enabled; the interval ends give the
    /// end control points exactly.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB or when the NLib evaluation fails.
    pub fn evaluate_point(&self, t: f64) -> Result<Point3> {
        let cur = self.nurb_ref()?;
        let (u0, um) = cur.param_bounds();
        let natural_interval = Extent1d::from_min_max(u0, um);
        let mut t = t;

        // when t is out-of-bounds and not allowed to evaluate out-of-bounds
        if !natural_interval.contains_value(t, 0.0) && !self.is_out_of_bounds_enabled() {
            t = natural_interval.clamp_value(t);
        }

        let pw = cur.cpts();
        // low work - begin point evaluation
        if t == natural_interval.min() {
            return pw
                .first()
                .map(|&p| to_euclid(p))
                .ok_or_else(|| SolidError::sm_err(OP_EVAL_PT));
        }
        // low work - end point evaluation
        else if t == natural_interval.max() {
            return pw
                .last()
                .map(|&p| to_euclid(p))
                .ok_or_else(|| SolidError::sm_err(OP_EVAL_PT));
        }

        // LEFT/RIGHT doesn't matter on C0 curves for point evals
        crv_eval(cur, t, Side::Left).map_err(SolidError::nl_ser(OP_EVAL_PT))
    }

    /// The point and derivatives `0..=num_derivs` at `t` (`Evaluate`), from the left
    /// (`from_left`) or right at a knot. Outside the natural interval `t` is clamped
    /// unless out-of-bounds evaluation is enabled. With `non_zero_tangents` (default
    /// `FALSE` in `SmBSplineCurve.h`, `TRUE` in `SmCurve.h`) a first derivative shorter than `1.1 * SM_EFF_ZERO * (1 +
    /// max|P|)` is replaced by the second derivative scaled to that length (negated
    /// past the middle of the interval).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB or when the NLib evaluation fails (including
    /// `num_derivs >= GW_MAX_DERIV`).
    pub fn evaluate(
        &self,
        t: f64,
        num_derivs: usize,
        from_left: bool,
        non_zero_tangents: bool,
    ) -> Result<Vec<Vec3>> {
        crate::limits::probe();
        // pass simple calls along to EvaluatePoint
        if num_derivs == 0 {
            return Ok(vec![self.evaluate_point(t)?]);
        }
        if num_derivs >= MAX_DERIV {
            // (NLib's order error, after the NURB check, without allocating an output
            // of that size)
            let cur = self.nurb_ref()?;
            let side = if from_left { Side::Left } else { Side::Right };
            return crv_derivs(cur, t, side, num_derivs).map_err(SolidError::nl_ser(OP_EVAL));
        }
        let mut pv = vec![Vec3::ZERO; num_derivs + 1];
        self.evaluate_into(t, from_left, non_zero_tangents, &mut pv)?;
        Ok(pv)
    }

    /// [`BSplineCurve::evaluate`] into the caller's slice: `out[k]` is derivative `k`,
    /// `num_derivs = out.len() - 1` (below `GW_MAX_DERIV`), without a heap allocation
    /// (except for the rare zero-tangent re-evaluation).
    ///
    /// # Errors
    ///
    /// As [`BSplineCurve::evaluate`]; `SM_ERR` for an empty `out`.
    pub fn evaluate_into(
        &self,
        t: f64,
        from_left: bool,
        non_zero_tangents: bool,
        out: &mut [Vec3],
    ) -> Result<()> {
        let num_derivs = out
            .len()
            .checked_sub(1)
            .ok_or_else(|| SolidError::sm_err(OP_EVAL))?;
        // pass simple calls along to EvaluatePoint
        if num_derivs == 0 {
            if let Some(slot) = out.first_mut() {
                *slot = self.evaluate_point(t)?;
            }
            return Ok(());
        }

        // natural extent
        let cur = self.nurb_ref()?;
        let (u0, um) = cur.param_bounds();
        let natural_interval = Extent1d::from_min_max(u0, um);
        let mut t = t;

        // when t is out-of-bounds and not allowed to evaluate out-of-bounds
        if !natural_interval.contains_value(t, 0.0) && !self.is_out_of_bounds_enabled() {
            t = natural_interval.clamp_value(t);
        }

        let side = if from_left { Side::Left } else { Side::Right };
        crv_derivs_into(cur, t, side, out).map_err(SolidError::nl_ser(OP_EVAL))?;

        // When asked, find a direction for zero-length first derivatives from the
        // 2nd derivative value.
        if non_zero_tangents {
            let p0 = out.first().copied().unwrap_or(Vec3::UNDEF);
            let scaled_zero = EFF_ZERO * (1.0 + p0.max_dimension());
            let new_len = 1.1 * scaled_zero;

            let d1 = out.get(1).copied().unwrap_or(Vec3::UNDEF);
            if num_derivs >= 1 && d1.length_squared() < new_len * new_len {
                if num_derivs < 2 {
                    // recurse so the next section sets the 1st derivative; the call
                    // takes the `SmBSplineCurve.h` default `bNonZeroTangents = FALSE`,
                    // so it returns the exact (zero) first derivative
                    let vals = self
                        .evaluate(t, 2, from_left, false)
                        .unwrap_or_else(|_| vec![Vec3::UNDEF; 3]);
                    if let (Some(slot), Some(&v)) = (out.get_mut(1), vals.get(1)) {
                        *slot = v;
                    }
                } else {
                    let d2 = out.get(2).copied().unwrap_or(Vec3::UNDEF);
                    let len = d2.length();
                    if len > new_len {
                        let mut d1 = (new_len / len) * d2;
                        // at the 'top' of the domain the degenerate derivative is
                        // shrinking, so it points against the 2nd derivative
                        if t > self.natural_interval().mid() {
                            d1 *= -1.0;
                        }
                        if let Some(slot) = out.get_mut(1) {
                            *slot = d1;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Tessellates the curve over `interval` (`Tessellate`): bisection with the chord
    /// height and angle tolerances, and a maximum segment length of the approximate
    /// length over `min_segments` when `min_segments > 1`.
    ///
    /// # Errors
    ///
    /// Those of `SmCurve::TessellateByBisection`
    /// ([`crate::curve::curve::Curve::tessellate_by_bisection`]).
    pub fn tessellate(
        &self,
        k: &crate::kernel::Kernel,
        interval: &Extent1d,
        chord_height_tol: f64,
        angle_tol_deg: f64,
        min_segments: usize,
    ) -> Result<Tessellation> {
        // init MaxLength control parameter
        let max_length = if min_segments > 1 {
            approximate_length(k, self, interval, 5)? / (min_segments as f64)
        } else {
            0.0
        };

        // pass the call along
        tessellate_by_bisection(
            k,
            self,
            interval,
            chord_height_tol,
            angle_tol_deg,
            max_length,
        )
    }

    // ---- editing ----------------------------------------------------------------------

    /// Sets the dimension to 2 and every control point's `z` to [`NOZ`]
    /// (`ConvertTo2D`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB (the dimension is already changed).
    pub fn convert_to_2d(&mut self) -> Result<()> {
        self.dim = 2;
        let cur = self.get_or_create_gw_nurb()?;
        for pw in cur.cpts_mut() {
            pw.z = NOZ;
        }
        Ok(())
    }

    /// Sets the dimension to 3 and every control point's `z` to `0` (`ConvertTo3D`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB (the dimension is already changed).
    pub fn convert_to_3d(&mut self) -> Result<()> {
        self.dim = 3;
        let cur = self.get_or_create_gw_nurb()?;
        for pw in cur.cpts_mut() {
            pw.z = 0.0;
        }
        Ok(())
    }

    /// Reparameterises the knots to `new_param` (`EditParameterization`; its `notify`
    /// flag only controls the omitted post-edit notification). Nothing changes when
    /// the interval ends are already the same within the scaled zero.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn edit_parameterization(&mut self, new_param: &Extent1d) -> Result<()> {
        // get current interval
        let old = self.natural_interval();

        // low work: current interval == new interval
        if are_same(old.min(), new_param.min()) && are_same(old.max(), new_param.max()) {
            return Ok(());
        }

        // MakeNurb() can fail for degenerate inputs: fail gracefully.
        let cur = self
            .get_or_create_gw_nurb()
            .map_err(|_| SolidError::sm_err(OP_REPARAM))?;
        let ivl = Interval {
            ul: new_param.min(),
            ur: new_param.max(),
        };
        let p = cur.degree();
        reparam(cur.knots_mut(), p, ivl)
    }

    /// Moves the first (`edit_start`) or last control point to `new_end_point`
    /// (`EditEndPoint`). With `weight` the point is taken as homogeneous and the
    /// weight is set (rational curves only); otherwise a rational end point is
    /// projected into homogeneous space with its old weight. A 2D control point keeps
    /// its missing `z`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB, or when `weight` is given for a non-rational curve
    /// (the coordinates are already moved then, as upstream).
    pub fn edit_end_point(
        &mut self,
        new_end_point: Point3,
        edit_start: bool,
        weight: Option<f64>,
    ) -> Result<()> {
        let cur = self.get_or_create_gw_nurb()?;
        let n = cur.n();
        let idx = if edit_start { 0 } else { n };
        let pw = cur
            .cpts_mut()
            .get_mut(idx)
            .ok_or_else(|| invalid(OP_EDIT_END, "empty control polygon"))?;

        // set coordinates
        pw.x = new_end_point.x;
        pw.y = new_end_point.y;
        if pw.z != NOZ {
            pw.z = new_end_point.z;
        }

        // handle weights
        if let Some(w) = weight {
            if pw.w == NOW {
                return Err(SolidError::sm_err(OP_EDIT_END));
            }
            pw.w = w;
        }
        // assume point needs to be projected into homogeneous space
        else if pw.w != NOW {
            pw.x *= pw.w;
            pw.y *= pw.w;
            if pw.z != NOZ {
                pw.z *= pw.w;
            }
        }

        // Give derived Analytic curves a chance to update their 'step' data
        self.refresh_analytics()
    }

    /// Fixes up the knots with tolerance `tol` (`FixupKnotVector`; upstream default
    /// `SM_EFF_ZERO`): close knots become equal, near knots are spread apart.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn fixup_knot_vector(&mut self, tol: f64) -> Result<()> {
        let cur = self.get_or_create_gw_nurb()?;
        fixup_knot_vector(cur.knots_mut(), tol);
        Ok(())
    }

    /// Removes every interior knot whose removal moves the curve less than `tol`
    /// (`RemoveExtraKnots`); when one was removed the NURB is replaced by a compact
    /// copy with fixed-up knots. Nothing happens without a NURB.
    ///
    /// # Errors
    ///
    /// The `InvalidInput` errors of [`crv_remove_knots`] for a malformed curve.
    pub fn remove_extra_knots(&mut self, tol: f64) -> Result<()> {
        let dim = self.dim;
        let Some(crv) = self.nurb.as_mut() else {
            return Ok(());
        };
        let old_m = crv.m();
        // The returned flag is ignored upstream.
        let _ = crv_remove_knots(crv, tol)?;

        // If no knots needed removing then just return here
        if crv.m() == old_m {
            return Ok(());
        }

        // A shrunken copy of the curve takes the place of the NURB.
        let tmp = Self::from_gw_nurb(dim, crv);
        self.nurb = tmp.nurb;
        Ok(())
    }

    /// Replaces the NURB by a compact copy of `gw_nurb` (`SetFromGwNurb`; no knot
    /// fix-up). The old NURB is dropped unless borrowed.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `gw_nurb` is `None` (a borrowed NURB is then kept, an owned one
    /// is gone).
    pub fn set_from_gw_nurb(&mut self, gw_nurb: Option<&Curve>) -> Result<()> {
        // free existing m_pNurb
        if !self.nurb_is_borrowed {
            self.nurb = None;
        }
        let Some(src) = gw_nurb else {
            return Err(SolidError::sm_err(
                "nurbs::b_spline_curve::p1::BSplineCurve::set_from_gw_nurb",
            ));
        };
        self.nurb = Some(copy_nurb_curve(src));
        self.nurb_is_borrowed = false;
        Ok(())
    }

    /// Scales the control points by `scale` about the origin (optional), then rotates
    /// and moves them by `rotate_n_move` (`Transform`). The identity placement with
    /// no or unit scale changes nothing. A 2D NURB is made 3D (`N_CrvTransform`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn transform(&mut self, rotate_n_move: &Axis2Placement, scale: Option<Vec3>) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        let mut rma = matrix_of(&rotate_n_move.matrix())?;
        let s = scale.unwrap_or(identity_scale);
        for i in 0..3 {
            for (j, f) in [s.x, s.y, s.z].into_iter().enumerate() {
                let v = rma.get(i, j)?;
                rma.set(i, j, v * f)?;
            }
        }

        let cur = self
            .get_or_create_gw_nurb()
            .map_err(|_| SolidError::sm_err(OP_XFORM))?;
        cur.transform(&rma)
    }

    // ---- factories --------------------------------------------------------------------

    /// A copy mirrored about the XY plane of `mirror_plane` (`CreateMirrorCurve`). The
    /// copy keeps this curve's dimension; a 2D NURB is made 3D (`N_CrvTransform`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn create_mirror_curve(&self, mirror_plane: &Axis2Placement) -> Result<Self> {
        let rma = matrix_of(&mirror_plane.mirror_matrix())?;
        let mut bsc = self.copy();
        let cur = bsc.get_or_create_gw_nurb()?;
        cur.transform(&rma)?;
        Ok(bsc)
    }

    /// The 3D projection of this curve onto the plane through `plane_point` with
    /// normal `plane_normal` (`CreatePlaneProjection`): along the direction
    /// `dir_or_center` ([`ProjectionType::Parallel`]) or from the eye point
    /// `dir_or_center` ([`ProjectionType::Perspective`]).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for another projection type, a (near) zero normal, a direction
    /// parallel to the plane or an eye point on the plane; `SM_ERR` without a NURB or
    /// when the NLib projection fails.
    pub fn create_plane_projection(
        &self,
        projection: ProjectionType,
        plane_point: Point3,
        plane_normal: Vec3,
        dir_or_center: Vec3,
    ) -> Result<Self> {
        let prj = match projection {
            ProjectionType::Parallel | ProjectionType::Perspective => projection,
            ProjectionType::Rotation | ProjectionType::Unknown => {
                return Err(invalid(OP_PROJ, "unsupported projection type"));
            }
        };

        // ZERO_VEC_ER
        check(
            plane_normal.length_squared() >= EFF_ZERO_SQ,
            OP_PROJ,
            "zero plane normal",
        )?;

        // check for bad projection vector
        if is_zero(plane_normal.length_squared()) {
            return Err(invalid(OP_PROJ, "zero plane normal"));
        }

        // Check for bad projection direction
        let nl_prj = if prj == ProjectionType::Parallel {
            if is_zero(plane_normal.dot(dir_or_center)) {
                return Err(invalid(
                    OP_PROJ,
                    "projection direction parallel to the plane",
                ));
            }
            Projection::Parallel
        } else {
            // Make sure center of projection is not on the plane
            let pnt_on_plane = dir_or_center.project_point_to_plane(plane_point, plane_normal);
            if is_zero(pnt_on_plane.distance(dir_or_center)) {
                return Err(invalid(OP_PROJ, "center of projection on the plane"));
            }
            Projection::Perspective
        };

        let cur = self.nurb_ref()?;
        let projected = cur
            .project_onto_plane(plane_point, plane_normal, dir_or_center, nl_prj)
            .map_err(SolidError::nl_ser(OP_PROJ))?;
        Ok(Self::from_gw_nurb(3, &projected))
    }

    /// A non-rational curve from STEP-style data (`CreateCanonical`): Euclidean
    /// control points, unique knots with multiplicities, optional weights and an
    /// optional trimming interval (default `[U[p], U[n+1]]`), through
    /// `N_Iges126Crv`. A 3D curve of 2D points is made 3D.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a dimension outside 1..=3, a degree outside 1..=32, a
    /// weight count different from the control point count, fewer multiplicities
    /// than knots, no control points or too few knots for the interval; `SM_ERR`
    /// when `N_Iges126Crv` fails.
    pub fn create_canonical(data: &CanonicalData<'_>) -> Result<Self> {
        let CanonicalData {
            dim,
            degree,
            control_points,
            form,
            knot_multiplicities,
            knots,
            knot_type,
            weights,
            trim,
        } = *data;

        // Do some simple input checking for valid numbers
        if !(1..=3).contains(&dim) {
            return Err(invalid(OP_CANON, "dimension outside 1..=3"));
        }
        if !(1..=32).contains(&degree) {
            return Err(invalid(OP_CANON, "degree outside 1..=32"));
        }
        if let Some(w) = weights {
            if w.len() != control_points.len() {
                return Err(invalid(
                    OP_CANON,
                    "weight count differs from control point count",
                ));
            }
        }

        // Make compatible knots array (only the part `N_Iges126Crv` and the interval
        // read is materialised; its full length comes from the multiplicities)
        let expanded = ExpandedKnots::new(knots, knot_multiplicities, OP_CANON)?;

        // Make compatible weight array
        let tmp_weights;
        let w = if let Some(w) = weights {
            w
        } else {
            tmp_weights = vec![1.0; control_points.len()];
            &tmp_weights
        };

        let n = control_points
            .len()
            .checked_sub(1)
            .ok_or_else(|| invalid(OP_CANON, "no control points"))?;
        let p = degree;

        // set curve ivl
        let v = if let Some(t) = trim {
            [t.min(), t.max()]
        } else {
            let short = || invalid(OP_CANON, "too few knots");
            let last = expanded.len().checked_sub(p + 1).ok_or_else(short)?;
            [
                expanded.get(p).ok_or_else(short)?,
                expanded.get(last).ok_or_else(short)?,
            ]
        };

        // `N_Iges126Crv` reads knots `0..=n + p + 1` only
        let new_knots = expanded.prefix(n.saturating_add(p).saturating_add(2), OP_CANON)?;

        // NLIB curve construction
        let mut cur = iges_126_curve(&Entity126 {
            k: n,
            m: p,
            t: &new_knots,
            w,
            pts: control_points,
            v,
        })
        .map_err(SolidError::nl_ser(OP_CANON))?;

        // when asked - bump 2d curves up to 3d curves
        if dim == 3 && !cur.is_3d() {
            cur.to_3d();
        }

        // make the curve - side effect: packs the memory
        let mut bsc = Self::from_gw_nurb(dim, &cur);
        bsc.knot_type = knot_type;
        bsc.form = form;
        Ok(bsc)
    }

    /// A circular arc of `radius` in the XY plane of `frame` from `start_deg` to
    /// `end_deg` (degrees from the X axis, within `[-360, 360]`), degree 2 or 5
    /// (`CreateCircleSegment`). A 2D result has its `z` removed.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a radius below `SM_EFF_ZERO`, a dimension outside 2..=3 or
    /// an angle outside `[-360, 360]`; `SM_ERR` when `N_CreateCircArc` fails.
    pub fn create_circle_segment(
        dim: u64,
        frame: &Axis2Placement,
        radius: f64,
        start_deg: f64,
        end_deg: f64,
        param: NurbCircleParam,
    ) -> Result<Self> {
        // check input
        check(radius >= EFF_ZERO, OP_CIRCLE, "radius below SM_EFF_ZERO")?;
        if !(2..=3).contains(&dim) {
            return Err(invalid(OP_CIRCLE, "dimension outside 2..=3"));
        }
        check(
            (-360.0..=360.0).contains(&start_deg),
            OP_CIRCLE,
            "start angle outside [-360, 360]",
        )?;
        check(
            (-360.0..=360.0).contains(&end_deg),
            OP_CIRCLE,
            "end angle outside [-360, 360]",
        )?;

        let (center, x, y) = extract_frame(frame);
        let ctp = match param {
            NurbCircleParam::Quadratic => ConicType::Quadratic,
            NurbCircleParam::Quintic => ConicType::Quintic,
        };

        // create circle start and stop specified by angles
        let arc = Arc {
            center,
            x,
            y,
            r: radius,
            start: start_deg,
            end: end_deg,
        };
        let mut cur = circ_arc(&arc, ctp).map_err(SolidError::nl_ser(OP_CIRCLE))?;
        if dim == 2 {
            cur.to_2d();
        }

        // copy curve into
        let mut bsc = Self::from_gw_nurb(dim, &cur);
        bsc.form = BSplineCurveForm::CircularArc;
        Ok(bsc)
    }

    /// The counter-clockwise 3D arc about `center` from `point1` to `point2`
    /// (`CreateArcFromPoints`), through [`BSplineCurve::create_circle_segment`].
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the points are not at the same distance from the center, the
    /// sweep is (nearly) zero or 180 degrees; the errors of `Unitize`,
    /// `CCWAngleBetween` and `SetCanonical` for degenerate input.
    pub fn create_arc_from_points(
        center: Point3,
        point1: Point3,
        point2: Point3,
        param: NurbCircleParam,
    ) -> Result<Self> {
        // radii 0 and 1
        let v0 = point1 - center;
        let v1 = point2 - center;
        let radius = v0.length();
        let radius2 = v1.length();

        // Z vec and arc angular sweep
        let mut z_vec = v0.cross(v1).unitize()?;
        let mut angle_rad = z_vec.ccw_angle_between(v0, v1)?;

        // tolerance
        let tol = EFF_ZERO * (1.0 + point1.max_dimension() + center.max_dimension());

        // both radii must have same length
        if (radius - radius2).abs() > tol {
            return Err(SolidError::sm_err(OP_ARC));
        }

        // angular sweep must be nonDegenerate
        if angle_rad.abs() < EFF_ZERO_SQRT {
            return Err(SolidError::sm_err(OP_ARC));
        }

        // center, start, and end points can't be colinear
        if angle_rad.abs() > PI - EFF_ZERO_SQRT {
            return Err(SolidError::sm_err(OP_ARC));
        }

        // make sure Z vec is oriented for a counterClockwise arc
        if angle_rad < 0.0 {
            z_vec = -z_vec;
            angle_rad = z_vec.ccw_angle_between(v0, v1)?;
        }

        // arc angular sweep should be positive
        if angle_rad < EFF_ZERO {
            return Err(SolidError::sm_err(OP_ARC));
        }

        // define arc origin, x axis, and y axis placement
        let mut placement = Axis2Placement::default();
        let y_vec = z_vec.cross(v0);
        let x_vec = y_vec.cross(z_vec);
        let x_vec = x_vec.unitize()?;
        let y_vec = y_vec.unitize()?;
        placement.set_canonical(center, x_vec, y_vec)?;

        // pass the call along
        Self::create_circle_segment(3, &placement, radius, 0.0, angle_rad * 180.0 / PI, param)
    }

    /// A degree-1 curve with both control points at `point`, parameterised on
    /// `[0, 1]` (`CreateDegenerateCurve`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a dimension outside 2..=3; the errors of
    /// [`BSplineCurve::create_canonical`].
    pub fn create_degenerate_curve(dim: u64, point: Point3) -> Result<Self> {
        // check input
        if !(2..=3).contains(&dim) {
            return Err(invalid(OP_DEGEN, "dimension outside 2..=3"));
        }
        let cntrl_poly = [point, point];
        let knots = [0.0, 1.0];
        let knot_mult = [2, 2];
        Self::create_canonical(&CanonicalData {
            dim,
            degree: 1,
            control_points: &cntrl_poly,
            form: BSplineCurveForm::Unspecified,
            knot_multiplicities: &knot_mult,
            knots: &knots,
            knot_type: KnotType::Unspecified,
            weights: None,
            trim: None,
        })
    }

    /// The line segment from `start` to `end` on `[0, 1]` (`CreateLineSegment`); a 2D
    /// result has its `z` removed.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a dimension outside 2..=3 or points closer than
    /// `SM_EFF_ZERO`.
    pub fn create_line_segment(dim: u64, start: Point3, end: Point3) -> Result<Self> {
        // check input
        if !(2..=3).contains(&dim) {
            return Err(invalid(OP_LINE, "dimension outside 2..=3"));
        }
        check(
            start.distance(end) >= EFF_ZERO,
            OP_LINE,
            "same start and end point",
        )?;

        let line_vec = end - start;
        let mut new_cur =
            line_from_pt_and_vector(start, line_vec).map_err(SolidError::nl_ser(OP_LINE))?;
        if dim == 2 {
            new_cur.to_2d();
        }

        // construct the curve
        let mut bsc = Self::from_nurb(new_cur, dim, false);
        bsc.form = BSplineCurveForm::PolylineForm;
        Ok(bsc)
    }

    /// Scales the end tangents of a Hermite-style blend between `points[0]` and
    /// `points[1]` so the cubic control polygon is equally spaced, optionally
    /// shortened so the blend has no inflection, and scales the optional higher
    /// derivatives (2 entries: second derivatives; 4 entries: second and third) to
    /// match (`FairBlendCurveDerivatives`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` unless there are exactly two points and two tangents; the errors of
    /// `Unitize` (zero tangent) and `smgu_LinePlaneIntersect`.
    pub fn fair_blend_curve_derivatives(
        points: &[Point3],
        tangents: &mut [Vec3],
        higher_order_derivs: Option<&mut [Vec3]>,
        prevent_inflection: bool,
    ) -> Result<()> {
        // check input
        let (&[p0, p1], &mut [t0, t1]) = (points, &mut *tangents) else {
            return Err(SolidError::sm_err(OP_FAIR));
        };

        let mut scale1 = 1.0;
        let mut scale2 = 1.0;

        // Solve for two lines parameterized by T where ||L1(t)-L2(t)|| = t:
        // 0 = t^2 (b^2-1) + t (2ab) + a^2
        let a = p1 - p0;
        let v1 = (-t1).unitize()?;
        let v0 = t0.unitize()?;
        let b = v1 - v0;

        let coeff = [a.dot(a), 2.0 * a.dot(b), b.dot(b) - 1.0];
        let sol = solve_quadratic(&coeff, EFF_ZERO);
        let mut dt = 0.0;
        match *sol.as_slice() {
            [s0] => dt = 3.0 * s0,
            [s0, s1] => {
                if s0 < 0.0 {
                    dt = 3.0 * s1;
                } else if s1 < 0.0 {
                    dt = 3.0 * s0;
                } else {
                    dt = 3.0 * min2(s0, s1);
                }
            }
            _ => {}
        }

        if dt > 0.0 {
            scale1 = dt / t0.length();
            scale2 = dt / t1.length();
        }

        if prevent_inflection {
            let z = t0.cross(t1);
            let normal1 = z.cross(t1);
            let vec = p0 - p1;
            let normal1 = normal1.unitize()?;
            let vec = vec.unitize()?;
            let dot1 = vec.dot(normal1);
            let ctrl_pnt1 = p0 + t0 * scale1 / 3.0;
            let line_vec = ctrl_pnt1 - p0;
            let vec1 = ctrl_pnt1 - p1;
            let dot2 = vec1.dot(normal1);
            let mut dt1 = 1.0;
            let mut dt2 = 1.0;

            if dot1 * dot2 < 0.0 {
                dt1 = line_plane_intersect(p0, line_vec, p1, normal1)?;
                if dt1 < 0.03 {
                    dt1 = 1.0;
                }
                // upstream signals (but ignores) dt1 outside [0, 1]
            }

            let normal2 = z.cross(t0).unitize()?;
            let vec = -vec;
            let dot3 = vec.dot(normal2);
            let ctrl_pnt2 = p1 - t1 * scale2 / 3.0;
            let vec2 = ctrl_pnt2 - p0;
            let dot4 = vec2.dot(normal2);
            let line_vec2 = ctrl_pnt2 - p1;

            if dot3 * dot4 < 0.0 {
                dt2 = line_plane_intersect(p1, line_vec2, p0, normal2)?;
                if dt2 < 0.03 {
                    dt2 = 1.0;
                }
                // upstream signals (but ignores) dt2 outside [0, 1]
            }

            dt = min2(dt1, dt2);
            scale1 *= dt;
            scale2 *= dt;
        }

        // set output - new Tangent values
        if let [r0, r1] = tangents {
            *r0 = t0 * scale1;
            *r1 = t1 * scale2;
        }

        // If we don't have any higher order just quit with results for simple Hermite
        let Some(higher) = higher_order_derivs else {
            return Ok(());
        };
        if let [h0, h1] = higher {
            *h0 = *h0 * scale1 * scale1;
            *h1 = *h1 * scale2 * scale2;
        }
        if let [h0, h1, h2, h3] = higher {
            *h0 = *h0 * scale1 * scale1;
            *h1 = *h1 * scale2 * scale2;
            *h2 = *h2 * scale1 * scale1 * scale1;
            *h3 = *h3 * scale2 * scale2 * scale2;
        }
        Ok(())
    }
}

/// A 4x4 [`RealMatrix`] (`N_SetRealMatrix(.., 3, 3, NL_MT_FULL, 3)`) filled from `m`.
fn matrix_of(m: &[[f64; 4]; 4]) -> Result<RealMatrix> {
    let mut rma = RealMatrix::new(3, 3, MatrixType::Full, 3)?;
    for (i, row) in m.iter().enumerate() {
        for (j, &v) in row.iter().enumerate() {
            rma.set(i, j, v)?;
        }
    }
    Ok(rma)
}

/// Upstream's expanded knot array of `CreateCanonical` (`sNewKnots`): `knots[i]`
/// repeated `mults[i]` times, kept implicit because the multiplicities come from the
/// caller (an absurd multiplicity must not allocate).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ExpandedKnots<'a> {
    knots: &'a [f64],
    mults: &'a [usize],
    len: usize,
}

impl<'a> ExpandedKnots<'a> {
    /// The expansion of `knots` by `mults`; `InvalidInput` for fewer multiplicities
    /// than knots (upstream reads past the array) or a total count above `usize::MAX`.
    pub(crate) fn new(knots: &'a [f64], mults: &'a [usize], op: &'static str) -> Result<Self> {
        let mut len = 0usize;
        for i in 0..knots.len() {
            let mult = *mults
                .get(i)
                .ok_or_else(|| invalid(op, "fewer multiplicities than knots"))?;
            len = len
                .checked_add(mult)
                .ok_or_else(|| invalid(op, "knot count overflow"))?;
        }
        Ok(Self { knots, mults, len })
    }

    /// Number of expanded knots (`sNewKnots.GetSize()`).
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Expanded knot `idx` (`sNewKnots[idx]`), `None` past the end.
    pub(crate) fn get(&self, idx: usize) -> Option<f64> {
        let mut start = 0usize;
        for (&knot, &mult) in self.knots.iter().zip(self.mults) {
            let end = start.saturating_add(mult);
            if idx < end {
                return Some(knot);
            }
            start = end;
        }
        None
    }

    /// The first `min(count, len)` expanded knots, allocated fallibly
    /// (`InvalidInput` when they do not fit in memory).
    pub(crate) fn prefix(&self, count: usize, op: &'static str) -> Result<Vec<f64>> {
        let count = count.min(self.len);
        let mut out = Vec::new();
        out.try_reserve_exact(count)
            .map_err(|_| invalid(op, "knot array too large"))?;
        for (&knot, &mult) in self.knots.iter().zip(self.mults) {
            let room = count.saturating_sub(out.len());
            if room == 0 {
                break;
            }
            out.extend(core::iter::repeat_n(knot, mult.min(room)));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kernel;

    fn cubic() -> BSplineCurve {
        let pw = vec![
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(1.0, 2.0, 0.0, NOW),
            CPoint::new(3.0, 2.0, 1.0, NOW),
            CPoint::new(4.0, 0.0, 0.0, NOW),
            CPoint::new(5.0, -1.0, 2.0, NOW),
        ];
        let u = vec![0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0];
        BSplineCurve::from_nurb(Curve::new(pw, 3, u).unwrap(), 3, false)
    }

    #[test]
    fn evaluate_into_matches_evaluate_bit_for_bit() {
        let bits = |v: &[Vec3]| -> Vec<[u64; 3]> {
            v.iter()
                .map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()])
                .collect()
        };
        // the second curve has a zero first derivative at t = 0 (repeated point)
        let pw = vec![
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(3.0, 2.0, 1.0, NOW),
            CPoint::new(4.0, 0.0, 0.0, NOW),
        ];
        let u = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let cusp = BSplineCurve::from_nurb(Curve::new(pw, 3, u).unwrap(), 3, false);
        for c in [cubic(), cusp] {
            for t in [-0.25, 0.0, 0.3, 0.5, 0.75, 1.0, 1.5] {
                for from_left in [true, false] {
                    for nzt in [true, false] {
                        for nd in 0..MAX_DERIV + 2 {
                            let full = c.evaluate(t, nd, from_left, nzt);
                            let mut out = vec![Vec3::new(7.0, 7.0, 7.0); nd + 1];
                            let r = c.evaluate_into(t, from_left, nzt, &mut out);
                            match full {
                                Ok(full) => {
                                    assert!(r.is_ok());
                                    assert_eq!(bits(&full), bits(&out));
                                }
                                Err(e) => assert_eq!(r.err(), Some(e)),
                            }
                        }
                    }
                }
            }
        }
        assert!(cubic().evaluate_into(0.5, true, false, &mut []).is_err());
    }

    #[test]
    fn empty_curve_has_no_nurb() {
        let mut c = BSplineCurve::new(2);
        assert!(c.gw_nurb().is_none());
        assert_eq!(c.natural_interval(), Extent1d::from_min_max(0.0, 0.0));
        assert!(c.make_nurb().is_err());
        assert!(c.evaluate_point(0.5).is_err());
        assert!(c.remove_extra_knots(1e-3).is_ok());
        assert_eq!(c.type_code(), 14033);
        assert_eq!(c.class_string(), "SmBSplineCurve");
        assert!(c.is_kind_of(BSplineCurve::TYPE).unwrap());
        assert!(!c.is_kind_of(1).unwrap());
        assert!(c.is_kind_of(14001).unwrap());
    }

    #[test]
    fn evaluate_ends_and_clamping() {
        let c = cubic();
        assert_eq!(c.evaluate_point(0.0).unwrap(), Point3::new(0.0, 0.0, 0.0));
        assert_eq!(c.evaluate_point(1.0).unwrap(), Point3::new(5.0, -1.0, 2.0));
        assert_eq!(c.evaluate_point(-3.0).unwrap(), Point3::new(0.0, 0.0, 0.0));
        let pv = c.evaluate(0.25, 2, true, true).unwrap();
        assert_eq!(pv.len(), 3);
        assert_eq!(pv[0], c.evaluate_point(0.25).unwrap());
        assert!(c.evaluate(0.25, 6, true, true).is_err());
    }

    #[test]
    fn zero_tangent_replaced() {
        // Two coincident start control points: zero first derivative at 0.
        let pw = vec![
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(1.0, 1.0, 0.0, NOW),
        ];
        let c = BSplineCurve::from_nurb(
            Curve::new(pw, 2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).unwrap(),
            3,
            false,
        );
        let exact = c.evaluate(0.0, 1, false, false).unwrap();
        assert_eq!(exact[1], Vec3::ZERO);
        // One derivative: the recursion binds bNonZeroTangents = FALSE (as upstream).
        let one = c.evaluate(0.0, 1, false, true).unwrap();
        assert_eq!(one[1], Vec3::ZERO);
        let fixed = c.evaluate(0.0, 2, false, true).unwrap();
        assert!(fixed[1].length() > 0.0);
    }

    #[test]
    fn copies_and_equality() {
        let c = cubic();
        let d = c.clone();
        assert!(c.approx_eq(&d));
        let mut e = c.copy();
        e.edit_end_point(Point3::new(9.0, 9.0, 9.0), true, None)
            .unwrap();
        assert!(!c.approx_eq(&e));
        assert!(e.edit_end_point(Point3::ZERO, false, Some(2.0)).is_err());
        let mut f = BSplineCurve::new(3);
        assert!(!c.approx_eq(&f));
        f.dim = 2;
        assert!(!BSplineCurve::new(3).approx_eq(&f));
    }

    #[test]
    fn queries() {
        let c = cubic();
        assert_eq!(c.degree().unwrap(), 3);
        assert_eq!(c.number_control_points().unwrap(), 5);
        assert_eq!(c.number_natural_knots().unwrap(), 9);
        assert!(!c.is_rational().unwrap());
        assert_eq!(c.greville_abscissa(0).unwrap(), 0.0);
        assert_eq!(c.greville_abscissa(4).unwrap(), 1.0);
        assert_eq!(c.greville_abscissa(1).unwrap(), (0.0 + 0.0 + 0.5) / 3.0);
        let (pts, w) = c.control_polygon().unwrap();
        assert_eq!(pts.len(), 5);
        assert!(w.is_empty());
        let (k, m) = c.knots(None).unwrap();
        assert_eq!(k, vec![0.0, 0.5, 1.0]);
        assert_eq!(m, vec![4, 1, 4]);
        let (min_c, conts) = c.calculate_continuities(&Kernel::default(), 1.0).unwrap();
        assert_eq!(conts.len(), 3);
        assert_eq!(conts[0], ContinuityType::Discontinuous);
        assert_eq!(conts[1], min_c);
        let t = c
            .tessellate(&Kernel::default(), &c.natural_interval(), 0.1, 10.0, 0)
            .unwrap();
        assert_eq!(t.parameters.first().copied(), Some(0.0));
        assert_eq!(t.parameters.last().copied(), Some(1.0));
    }

    #[test]
    fn factories() {
        let l =
            BSplineCurve::create_line_segment(2, Point3::ZERO, Point3::new(1.0, 1.0, 0.0)).unwrap();
        assert_eq!(l.form(), BSplineCurveForm::PolylineForm);
        assert_eq!(l.control_points().unwrap()[0].z, NOZ);
        assert!(BSplineCurve::create_line_segment(3, Point3::ZERO, Point3::ZERO).is_err());
        let d = BSplineCurve::create_degenerate_curve(3, Point3::new(1.0, 2.0, 3.0)).unwrap();
        assert_eq!(d.evaluate_point(0.5).unwrap(), Point3::new(1.0, 2.0, 3.0));
        let c = BSplineCurve::create_circle_segment(
            3,
            &Axis2Placement::default(),
            2.0,
            0.0,
            90.0,
            NurbCircleParam::Quadratic,
        )
        .unwrap();
        let mid = c.evaluate_point(c.natural_interval().mid()).unwrap();
        assert!((mid.length() - 2.0).abs() < 1e-12);
        let a = BSplineCurve::create_arc_from_points(
            Point3::ZERO,
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            NurbCircleParam::Quintic,
        )
        .unwrap();
        assert_eq!(a.degree().unwrap(), 5);
        assert!(BSplineCurve::create_arc_from_points(
            Point3::ZERO,
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(-1.0, 0.0, 0.0),
            NurbCircleParam::Quadratic,
        )
        .is_err());
    }

    #[test]
    fn edits() {
        let mut c = cubic();
        c.edit_parameterization(&Extent1d::from_min_max(2.0, 4.0))
            .unwrap();
        assert_eq!(c.natural_interval(), Extent1d::from_min_max(2.0, 4.0));
        let mut m = c.create_mirror_curve(&Axis2Placement::default()).unwrap();
        let p = m.evaluate_point(4.0).unwrap();
        assert_eq!(p, Point3::new(5.0, -1.0, -2.0));
        m.transform(&Axis2Placement::default(), Some(Vec3::new(2.0, 2.0, 2.0)))
            .unwrap();
        assert_eq!(
            m.evaluate_point(4.0).unwrap(),
            Point3::new(10.0, -2.0, -4.0)
        );
        c.convert_to_2d().unwrap();
        assert_eq!(c.dim(), 2);
        c.convert_to_3d().unwrap();
        assert_eq!(c.control_points().unwrap()[4].z, 0.0);
        let pr = cubic()
            .create_plane_projection(ProjectionType::Parallel, Point3::ZERO, Vec3::Z, Vec3::Z)
            .unwrap();
        assert_eq!(pr.evaluate_point(1.0).unwrap(), Point3::new(5.0, -1.0, 0.0));
        assert!(cubic()
            .create_plane_projection(ProjectionType::Rotation, Point3::ZERO, Vec3::Z, Vec3::Z)
            .is_err());
        let mut s = BSplineCurve::new(3);
        assert!(s.set_from_gw_nurb(None).is_err());
        s.set_from_gw_nurb(cubic().gw_nurb()).unwrap();
        assert_eq!(s.number_control_points().unwrap(), 5);
    }

    #[test]
    fn fair_blend() {
        let pts = [Point3::ZERO, Point3::new(4.0, 0.0, 0.0)];
        let mut tans = [Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, -1.0, 0.0)];
        let mut hi = [Vec3::X, Vec3::Y];
        BSplineCurve::fair_blend_curve_derivatives(&pts, &mut tans, Some(&mut hi), true).unwrap();
        assert!(tans[0].length() > 0.0);
        let mut bad = [Vec3::X];
        assert!(BSplineCurve::fair_blend_curve_derivatives(&pts, &mut bad, None, false).is_err());
    }

    #[test]
    fn bezier_split() {
        let a = CPoint::new(0.0, 0.0, NOZ, NOW);
        let b = CPoint::new(2.0, 2.0, NOZ, NOW);
        let c = CPoint::new(4.0, 0.0, NOZ, NOW);
        let q = split_quadratic_bezier(a, b, c);
        assert_eq!(q[2], CPoint::new(2.0, 1.0, NOZ, NOW));
        let d = CPoint::new(6.0, 2.0, NOZ, NOW);
        let k = split_cubic_bezier(a, b, c, d);
        assert_eq!(k[3], CPoint::new(3.0, 1.0, NOZ, NOW));
        assert_eq!(k[6], d);
    }

    fn canonical(mults: &[usize]) -> Result<BSplineCurve> {
        let pts = [Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 2.0, 0.0)];
        BSplineCurve::create_canonical(&CanonicalData {
            dim: 3,
            degree: 1,
            control_points: &pts,
            form: BSplineCurveForm::Unspecified,
            knot_multiplicities: mults,
            knots: &[0.0, 1.0],
            knot_type: KnotType::Unspecified,
            weights: None,
            trim: None,
        })
    }

    #[test]
    fn canonical_huge_multiplicity_does_not_allocate() {
        // fuzz regression (nurbs.b_spline_curve_p1 fuzz00528): a 2^31 multiplicity
        // aborted the knot expansion; only the knots N_Iges126Crv reads are built now
        let plain = canonical(&[2, 2]).unwrap();
        let huge = canonical(&[2, 1 << 31]).unwrap();
        assert_eq!(plain.knots(None).unwrap(), huge.knots(None).unwrap());
        assert!(canonical(&[2, usize::MAX - 2]).is_ok());
        // total above usize::MAX, and a huge leading run (knots [0, 0, 0, 0] invalid)
        assert!(canonical(&[2, usize::MAX]).is_err());
        assert!(canonical(&[1 << 62, 2]).is_err());
        assert!(canonical(&[2]).is_err());
    }

    #[test]
    fn expanded_knots_lookup() {
        let e = ExpandedKnots::new(&[0.0, 0.5, 1.0], &[2, 1, 3], "test").unwrap();
        assert_eq!(e.len(), 6);
        assert_eq!(e.get(1), Some(0.0));
        assert_eq!(e.get(2), Some(0.5));
        assert_eq!(e.get(5), Some(1.0));
        assert_eq!(e.get(6), None);
        assert_eq!(e.prefix(4, "test").unwrap(), vec![0.0, 0.0, 0.5, 1.0]);
        assert_eq!(e.prefix(99, "test").unwrap().len(), 6);
        assert!(ExpandedKnots::new(&[0.0, 1.0], &[usize::MAX, 1], "test").is_err());
    }
}
