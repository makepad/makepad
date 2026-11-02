//! `SmBSplineCurve` methods implemented with NLib: arc, rational-curve and helix
//! approximation, curve extension, interpolation, knot insertion and spacing, degree
//! reduction, evenly spaced points, compatible knot vectors, end point deformation,
//! arc-length reparametrisation, scaling, the intersection-curve through-point edit and
//! trimming.
//!
//! Upstream: `source/SMLib/src/SmBSplineCurveNLib.cpp`, `source/SMLib/inc/SmBSplineCurve.h`.
//!
//! | Rust                                                   | Upstream                                       |
//! |--------------------------------------------------------|------------------------------------------------|
//! | (none: Rust owns memory)                               | `SmNLibStackHandler` (constructor, destructor) |
//! | `nlib_curve` (private)                                 | `sm_CreateNlibCurve` (static)                  |
//! | [`approx_circ_arc`], [`CircArcSpec`]                   | `sm_ApproxCircArc`                             |
//! | [`BSplineCurve::approximate_arc`]                      | `SmBSplineCurve::ApproximateArc`               |
//! | [`BSplineCurve::approximate_rational_curve`]           | `SmBSplineCurve::ApproximateRationalCurve`     |
//! | [`BSplineCurve::create_helix_segment`], [`HelixSpec`]  | `SmBSplineCurve::CreateHelixSegment`           |
//! | [`BSplineCurve::create_extended_curve`]                | `SmBSplineCurve::CreateExtendedCurve` (parameter) |
//! | [`BSplineCurve::create_extended_curve_by_distance`]    | `SmBSplineCurve::CreateExtendedCurve` (distance) |
//! | [`BSplineCurve::create_interpolating_curve`]           | `SmBSplineCurve::CreateInterpolatingCurve`     |
//! | [`BSplineCurve::create_approximating_curve`]           | `SmBSplineCurve::CreateApproximatingCurve` (unit `nlib.crv_fit_p4`) |
//! | [`BSplineCurve::dampen_knot_spacing`]                  | `SmBSplineCurve::DampenKnotSpacing`            |
//! | [`BSplineCurve::degree_reduction`]                     | `SmBSplineCurve::DegreeReduction`              |
//! | [`BSplineCurve::insert_one_knot`]                      | `SmBSplineCurve::InsertOneKnot`                |
//! | [`control_point_spacing`]                              | `sm_GetControlPointSpacing` (static)           |
//! | [`BSplineCurve::set_intersection_curve_through_point`] | `SmBSplineCurve::SetIntersectionCurveThroughPoint` |
//! | [`BSplineCurve::equally_spaced_points`]                | `SmBSplineCurve::EquallySpacedPoints`          |
//! | [`BSplineCurve::insert_knots`]                         | `SmBSplineCurve::InsertKnots(SmTArray<double>&)` |
//! | [`BSplineCurve::interpolate_points`]                   | `SmBSplineCurve::InterpolatePoints`            |
//! | [`BSplineCurve::make_curves_compatible`]               | `SmBSplineCurve::MakeCurvesCompatible`         |
//! | [`BSplineCurve::deform_to_end_point_targets`], [`InValueType`] | `SmBSplineCurve::DeformToEndPointTargets`, `SmInValueType` |
//! | [`BSplineCurve::remove_knots`]                         | `SmBSplineCurve::RemoveKnots`                  |
//! | [`BSplineCurve::reparametrize_with_arc_length`]        | `SmBSplineCurve::ReparametrizeWithArcLength`   |
//! | [`BSplineCurve::scale`]                                | `SmBSplineCurve::Scale(double)`                |
//! | [`BSplineCurve::scale_knot_vector`]                    | `SmBSplineCurve::ScaleKnotVector`              |
//! | [`BSplineCurve::synchronize_knots_of_curves`]          | `SmBSplineCurve::SyncronizeKnotsOfCurves`      |
//! | [`BSplineCurve::trim`], [`TrimMode`]                   | `SmBSplineCurve::Trim`                         |
//!
//! The other functions of the file (`ApproximatePoints`,
//! `CreateFromHermiteCurve`, `C1NonRationalCubicInterpolate`, `ForceThroughPoint`,
//! `MakeNonRational`, `Multiply*`, `SmoothJoin`, `InsertKnotsByDistance`, `RefineCurve`,
//! `Scale(SmVector3d&, SmPoint3d&)`, `TestDegenerate`, ...) belong to other units.
//!
//! Deviations:
//! - **Memory.** `SmNLibStackHandler` and the `NL_STACKS` arguments only manage NLib
//!   memory and have no counterpart. The "swap `m_pNurb` with a temporary
//!   `SmBSplineCurve(GetDim(), &curNew)`" idiom becomes "the NURB is replaced by a
//!   compact copy of the new curve with fixed-up knots"
//!   ([`BSplineCurve::from_gw_nurb`]); the borrowed flag stays as it was, as upstream.
//!   `Notify` calls only drop caches and notify owners, which a curve here does not
//!   have; they are omitted (see `nurbs::b_spline_curve::p1`).
//! - **Out-params and contexts.** New curves are return values (`Option` where upstream
//!   may hand back `NULL`); the `SmContext` arguments only choose where upstream
//!   allocates and are dropped. Unused arguments (`DeformToEndPointTargets`'
//!   `ppOptNewCurve`, `pOptSurfCrvOnSurf`, `pOptCrvOrientation`; `Trim`'s debug-only
//!   `bSkipDebugCheck`) are dropped. `Trim`'s tri-state `bNotify` is [`TrimMode`].
//! - **Enums.** `SmCurveParameterizationType` / `SmInterpolationType` are
//!   [`ParamMethod`]; the `int` end flag of the distance extension is [`CurveEnd`];
//!   `SmBoolean bRightHanded` is a `bool`.
//! - **Missing NURB.** Where upstream dereferences the `NULL` NURB of a curve without
//!   one (`GetOrCreateGwNurbPointer` cannot make it), the port returns `SM_ERR`.
//! - **Errors.** `NL_SER` / `SER(SM_ERR)` become `SM_ERR` ([`SolidError::Upstream`]),
//!   `SM_ERR_INVALID_INPUT` checks [`SolidError::InvalidInput`]; [`SolidError::Unported`]
//!   passes through. Where upstream would index outside its arrays the port returns
//!   `InvalidInput`. `sm_ApproxCircArc`'s bare `NL_YES` exits are `InvalidInput` with
//!   [`BARE_FLAG_REASON`].
//! - **Degree type.** `CreateApproximatingCurve` casts `lDegree` to `NL_DEGREE` (a
//!   `short`), so on its unchecked one-vector path a degree of 32768 or more wraps; the
//!   port takes a `usize` (no wrap), and the difftest FFI caps the degree at
//!   `NL_MAXDEG` (17).
//! - **Input arrays.** `CreateInterpolatingCurve` reinterprets its `const` vector array
//!   as the in/out `D` of `N_FitCrvFirstDeriv`, which rescales the caller's vectors in
//!   place when their magnitudes are ignored; the port works on a copy.
//!   `InterpolatePoints` copies `cpOptParams->GetSize()` parameters into an array of
//!   `m + 1`; the port requires exactly `m + 1` (`InvalidInput` otherwise).
//! - **`DampenKnotSpacing`** loops forever when a split never brings the span ratios
//!   below the factor (e.g. a factor at most 1); the port stops after
//!   `DAMPEN_SPLIT_LIMIT` splits with [`SolidError::NotConverged`]. Its
//!   `InsertAt(.., lMultiplicityOfNewKnots)` growth fails softly upstream (ignored);
//!   the port reserves it fallibly and returns `InvalidInput` when the caller's
//!   multiplicity makes the new knots too large for memory.
//! - **Tolerances of `Trim`.** The public ownerless trim uses the system zone
//!   tolerance; `trim_with_zone_tol` accepts the caller-resolved owning edge's
//!   tolerance (`SmTol::GetZoneTol3d(this)`). `SmTol::MapTo1d(tol, t, curve)` maps
//!   with the first derivative of `Evaluate(t, 1, TRUE, .., bNonZeroTangents = TRUE)`
//!   (the `SmCurve.h` default), computed by [`BSplineCurve::evaluate`].
//!   `sm_CrvTrim` returns the non-`NL_YES` flag `NL_INP_ERR` for `ur <= ul` and
//!   upstream then dereferences its `NULL` result; the port returns `InvalidInput`.
//! - **Opt-in UV trimming (#1114).** With
//!   [`crate::kernel::UpstreamFixes::bspline_trim_preserves_uv_spans`], degenerate
//!   end-span removal applies only to dimension-3 model curves. UV spans are in
//!   surface parameter units and must not be removed using a 3D zone tolerance.
//!   The default retains pinned SMLib behavior.
//! - **Back-edges.** `SetIntersectionCurveThroughPoint` starts with
//!   `IsIntersectorApprox` ([`BSplineCurve::is_intersector_approx`]) and uses
//!   `SmCurve::ApproximateLength` ([`crate::curve::curve::p2`]), `SmSurface::LocalPointSolve`
//!   (`surface.surface_p5`), `SmSurface::GetNaturalUVDomain`, `SmSurface::Evaluate` and
//!   `smsurf_DropVectors` ([`crate::surface::surface::p2`]); `DeformToEndPointTargets` calls
//!   [`BSplineCurve::degree_elevate`] for curves of degree below 3 or of even degree.
//!   The calls into unported units return [`SolidError::Unported`] until they land.
//!   Surfaces are any [`crate::surface::surface::p2::SurfaceEvaluate`] `S`.
//!   `DeformToEndPointTargets` forwards derived curve types to
//!   `SmCurve::DeformToEndPointTargets` (`curve.curve_p6`); a [`BSplineCurve`] is never
//!   a derived type, so that branch does not exist here.
//! - Status values upstream ignores stay ignored: the knot insertions and the
//!   refinement of `DeformToEndPointTargets` (a failed one leaves the curve unchanged),
//!   `ScaleKnotVector`'s `N_CrvReparam`, `CreateHelixSegment`'s `Transform`, the
//!   evaluations of `SetIntersectionCurveThroughPoint` (a failed one leaves upstream's
//!   default-constructed vectors, [`Vec3::UNDEF`]).

use crate::error::{NlibCode, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::tol::{are_same_to_tol, map_to_1d, zone_tol_3d, ScaledZero, SmallTopoType};
use crate::geom::vector2d::Vec2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{min2, EFF_ZERO};
use crate::nurbs::b_spline_curve::{BSplineCurve, BSplineCurveForm};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::crv_adv::p1::{Continuity, CurveEnd, BARE_FLAG_REASON};
use crate::nurbs::crv_adv::p2::{ConstrainedEnds, UnboundedEval};
use crate::nurbs::crv_approx::{
    approx_circ_arc_with_crv_data, approx_nurbs_with_non_rat_crv, calc_num_cpts_to_approx_arc,
    CircArcData,
};
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::crv_fit::p1::ParamMethod;
use crate::nurbs::crv_fit::p2::interp;
use crate::nurbs::crv_fit::p3::{
    approx_closed_conditions, interp_first_derivs, interp_high_derivs, interp_knots_and_tangents,
    EndTangents, ParamSource, VectorKind,
};
use crate::nurbs::crv_fit::p4::approx_end_derivs;
use crate::nurbs::crv_geom::ToleranceKind;
use crate::nurbs::crv_tool::p2::{make_compatible_adj_knots, make_compatible_knot_tol};
use crate::nurbs::geom::{normalize, PI};
use crate::nurbs::knots::{Side, MAX_DEGREE};
use crate::nurbs::math::{translate_sum2_pts, CPoint, NOW};
use crate::nurbs::nurbs_crv::{copy_nurb_curve, crv_trim};
use crate::nurbs::spiral::{approx_spiral, Hand, SpiralSpec};
use crate::surface::surface::p2::{drop_vectors, SurfaceEvaluate};

/// `lVecCount` of `SetIntersectionCurveThroughPoint`: UV values per UV curve.
const UV_VEC_COUNT: usize = 6;
/// Inner knots of the displacement curve of `DeformToEndPointTargets`.
const DISP_INSIDE_KNT_CNT: usize = 4;

const OP_ARC_FIT: &str = "nurbs::b_spline_curve_n_lib::approx_circ_arc";
const OP_ARC: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::approximate_arc";
const OP_RAT: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::approximate_rational_curve";
const OP_HELIX: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::create_helix_segment";
const OP_EXT: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::create_extended_curve";
const OP_EXT_DIST: &str =
    "nurbs::b_spline_curve_n_lib::BSplineCurve::create_extended_curve_by_distance";
const OP_INTERP_CRV: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::create_interpolating_curve";
const OP_APPROX_CRV: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::create_approximating_curve";
const OP_DAMPEN: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::dampen_knot_spacing";
const OP_DEG_RED: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::degree_reduction";
const OP_INS_ONE: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::insert_one_knot";
const OP_THROUGH: &str =
    "nurbs::b_spline_curve_n_lib::BSplineCurve::set_intersection_curve_through_point";
const OP_EVEN: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::equally_spaced_points";
const OP_INS: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::insert_knots";
const OP_INTERP: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::interpolate_points";
const OP_COMPAT: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::make_curves_compatible";
const OP_DEFORM: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::deform_to_end_point_targets";
const OP_REMOVE_KNOTS: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::remove_knots";
const OP_REPARAM: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::reparametrize_with_arc_length";
const OP_SCALE: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::scale";
const OP_SCALE_KNOTS: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::scale_knot_vector";
const OP_SYNC: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::synchronize_knots_of_curves";
const OP_TRIM: &str = "nurbs::b_spline_curve_n_lib::BSplineCurve::trim";

/// Most span splits [`BSplineCurve::dampen_knot_spacing`] makes before giving up
/// (upstream loops forever).
pub const DAMPEN_SPLIT_LIMIT: usize = 1 << 14;

/// `SmInValueType`: how an optional target value is used; discriminants are the
/// upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum InValueType {
    /// `SM_IV_SPECIFIED`: use the given value.
    Specified = 0,
    /// `SM_IV_SAME`: keep the current value.
    #[default]
    Same = 1,
    /// `SM_IV_UNCONSTRAINED`: ignore the value.
    Unconstrained = 2,
}

/// What [`BSplineCurve::trim`] does (upstream's tri-state `bNotify`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TrimMode {
    /// `TRUE` (default): trim, then notify.
    #[default]
    Notify,
    /// `FALSE`: trim without the final notification.
    Silent,
    /// `UNSURE`: only snap the trim interval, do not trim.
    IntervalOnly,
}

/// A circular arc for [`approx_circ_arc`] (the circle arguments of `sm_ApproxCircArc`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircArcSpec {
    /// Centre `cen`.
    pub center: Vec3,
    /// Local `X` axis (normalised before use).
    pub x: Vec3,
    /// Local `Y` axis (normalised before use).
    pub y: Vec3,
    /// Radius `rad`.
    pub radius: f64,
    /// Start angle `as` in degrees.
    pub start_angle: f64,
    /// End angle `ae` in degrees (`ae < as` adds 360).
    pub end_angle: f64,
    /// Exact start point `Ps`, or `None` for the point on the circle.
    pub start: Option<Vec3>,
    /// Exact end point `Pe`, or `None` for the point on the circle.
    pub end: Option<Vec3>,
}

/// A helix (or spiral) for [`BSplineCurve::create_helix_segment`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HelixSpec {
    /// Placement: the helix runs about its Z axis from `z = 0` on its X axis to
    /// `z = height`.
    pub frame: Axis2Placement,
    /// Height along Z (at least `-SM_EFF_ZERO`).
    pub height: f64,
    /// Radius at `z = 0` (at least 0).
    pub radius_start: f64,
    /// Radius at `z = height` (at least 0).
    pub radius_end: f64,
    /// Number of turns (at least `SM_EFF_ZERO`).
    pub num_turns: f64,
    /// Right-handed (`TRUE`) or left-handed.
    pub right_handed: bool,
    /// Largest allowed deviation from the exact helix.
    pub approx_tol: f64,
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

fn knot_at(u: &[f64], i: usize, operation: &'static str) -> Result<f64> {
    u.get(i)
        .copied()
        .ok_or_else(|| invalid(operation, "knot index out of range"))
}

fn cpt_at(pw: &[CPoint], i: usize, operation: &'static str) -> Result<CPoint> {
    pw.get(i)
        .copied()
        .ok_or_else(|| invalid(operation, "control point index out of range"))
}

fn cpt_mut<'a>(pw: &'a mut [CPoint], i: usize, operation: &'static str) -> Result<&'a mut CPoint> {
    pw.get_mut(i)
        .ok_or_else(|| invalid(operation, "control point index out of range"))
}

fn vec_at<T: Copy>(v: &[T], i: usize, operation: &'static str) -> Result<T> {
    v.get(i)
        .copied()
        .ok_or_else(|| invalid(operation, "index out of range"))
}

/// `Evaluate(t, 1, left, vals)` of `curve` (the `SmBSplineCurve.h` default
/// `bNonZeroTangents = FALSE`); upstream ignores the status, so a failed evaluation
/// keeps `keep`.
fn eval2(curve: &BSplineCurve, t: f64, left: bool, keep: [Vec3; 2]) -> [Vec3; 2] {
    match curve.evaluate(t, 1, left, false) {
        Ok(v) => match (v.first(), v.get(1)) {
            (Some(&a), Some(&b)) => [a, b],
            _ => keep,
        },
        Err(_) => keep,
    }
}

/// The 3D distance between two control points (`sm_GetControlPointSpacing`): their
/// difference (`N_Diff2CPts`) with the weight dropped (`N_CPtToPt`), not divided.
#[must_use]
pub fn control_point_spacing(a: CPoint, b: CPoint) -> f64 {
    let v = CPoint::diff(a, b).drop_w();
    (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}

/// `MakeUnitOrthoVectors` with upstream's partial writes: on failure `x` is the
/// failed `Unitize` result `(0, 0, 1)` and `y`, `z` keep their values.
fn make_unit_ortho_into(v: Vec3, y_ref: Option<Vec3>, x: &mut Vec3, y: &mut Vec3, z: &mut Vec3) {
    match v.make_unit_ortho_vectors(y_ref) {
        Ok([a, b, c]) => {
            *x = a;
            *y = b;
            *z = c;
        }
        Err(_) => {
            // The status is ignored upstream.
            *x = v.unitize_or_z();
        }
    }
}

/// The NLib continuity flag of a curve extension (`SM_G1` -> `NL_G1`, `SM_G1G2` ->
/// `NL_G2`, `SM_CINFINITY` -> `NL_CMAX`); `SM_ERR` for any other continuity.
fn extension_continuity(continuity: ContinuityType, operation: &'static str) -> Result<Continuity> {
    match continuity {
        ContinuityType::G1 => Ok(Continuity::G1),
        ContinuityType::G1G2 => Ok(Continuity::G2),
        ContinuityType::CInfinity => Ok(Continuity::Cmax),
        _ => Err(SolidError::sm_err(operation)),
    }
}

/// The NURB of `curve` for read-only use (`GetOrCreateGwNurbPointer`).
fn nurb_of<'a>(curve: &'a BSplineCurve, operation: &'static str) -> Result<&'a Curve> {
    curve
        .nurb
        .as_ref()
        .ok_or_else(|| SolidError::sm_err(operation))
}

/// A copy of the NURB of `curve` (`sm_CreateNlibCurve`: `N_AllocCrvAndArrays` +
/// `N_CrvCopy`).
fn nlib_curve(curve: &BSplineCurve, operation: &'static str) -> Result<Curve> {
    Ok(nurb_of(curve, operation)?.clone())
}

// ---- back-edges -------------------------------------------------------------------

/// `SmBSplineCurve::IsIntersectorApprox`: degree 3 and `IsPiecewiseC0`.
fn is_intersector_approx(curve: &BSplineCurve) -> Result<bool> {
    curve.is_intersector_approx()
}

/// `SmBSplineCurve::DegreeElevate`.
fn degree_elevate(curve: &mut BSplineCurve, new_degree: usize) -> Result<()> {
    curve.degree_elevate(new_degree)
}

/// `SmCurve::ApproximateLength` of `curve` over `interval` with `samples` samples.
fn approximate_length(
    k: &Kernel,
    curve: &BSplineCurve,
    interval: &Extent1d,
    samples: usize,
) -> Result<f64> {
    Ok(crate::curve::curve::p2::approximate_length(k, curve, interval, samples, false)?.0)
}

/// `SmSurface::LocalPointSolve(domain, SM_SO_MINIMIZE, point, guess, found, sol)`:
/// whether an answer was found, the solution value and the parameters of
/// `sol.m_vStart`.
fn surface_local_point_solve<S: SurfaceEvaluate>(
    k: &Kernel,
    surface: &S,
    domain: &Extent2d,
    point: Point3,
    guess: Vec2,
) -> Result<(bool, f64, Vec2)> {
    surface.local_point_solve_minimize(k, domain, point, guess)
}

/// `SmSurface::Evaluate(uv, 1, 1, TRUE, TRUE, TRUE, vals)`: `vals[i][j]` is the
/// derivative `i` times in `u` and `j` times in `v`.
fn surface_evaluate<S: SurfaceEvaluate>(
    k: &Kernel,
    surface: &S,
    uv: Vec2,
) -> Result<[[Vec3; 2]; 2]> {
    let d = surface.evaluate(
        k,
        uv,
        1,
        1,
        EvaluateFlags {
            only_upper_half: true,
            ..EvaluateFlags::default()
        },
    )?;
    let at = |i: usize, j: usize| -> Result<Vec3> {
        d.get(i)
            .and_then(|r| r.get(j))
            .copied()
            .ok_or_else(|| invalid(OP_THROUGH, "short surface evaluation"))
    };
    // [1][1] is not computed (upper half only): upstream leaves it untouched
    Ok([[at(0, 0)?, at(0, 1)?], [at(1, 0)?, Vec3::UNDEF]])
}

/// `smsurf_DropVectors(su, sv, 1, &vec, &uv_vec)`: `vec` in the tangent plane's
/// `(su, sv)` coordinates.
fn surface_drop_vector(su: Vec3, sv: Vec3, vec: Vec3) -> Result<Vec2> {
    drop_vectors(su, sv, &[vec])?
        .first()
        .copied()
        .ok_or_else(|| invalid(OP_THROUGH, "no dropped vector"))
}

// ---- free functions -----------------------------------------------------------------

/// Non-rational curve of degree `deg` (2 to 4) approximating a circular arc
/// (`sm_ApproxCircArc`): the radial error is `tol` (`kind` [`ToleranceKind::Relative`]:
/// percent of the radius; [`ToleranceKind::Absolute`]: a distance), the number of
/// control points comes from [`calc_num_cpts_to_approx_arc`] (at least `deg`) unless
/// `num_control_points` gives it. The axes are normalised and the end points are the
/// given ones or the points on the circle.
///
/// # Errors
///
/// `InvalidInput` ([`BARE_FLAG_REASON`]) for a degree outside 2 to 4, a radius `<= 0`
/// or equal angles; the errors of [`calc_num_cpts_to_approx_arc`], [`normalize`] and
/// [`approx_circ_arc_with_crv_data`] (`NL_INP_ERR` for fewer than `deg + 1` given
/// control points).
pub fn approx_circ_arc(
    arc: &CircArcSpec,
    deg: usize,
    tol: f64,
    kind: ToleranceKind,
    num_control_points: Option<u64>,
) -> Result<Curve> {
    let bare = || invalid(OP_ARC_FIT, BARE_FLAG_REASON);
    let rad = arc.radius;
    let as_ = arc.start_angle;
    let mut ae = arc.end_angle;
    let mut tol = tol;

    // Check for errors on input
    if !(2..=4).contains(&deg) {
        return Err(bare());
    }
    if rad <= 0.0 || as_ == ae {
        return Err(bare());
    }

    // Get number of control points required for fit
    if ae < as_ {
        ae += 360.0;
    }
    let sweep = ae - as_;
    if kind == ToleranceKind::Absolute {
        tol = 100.0 * (tol / rad);
    }

    let nc: i64 = match num_control_points {
        None => {
            let nc = calc_num_cpts_to_approx_arc(tol, sweep, deg, None)?;
            if nc < deg as i64 {
                deg as i64
            } else {
                nc
            }
        }
        // ULONG - 1 as NL_INDEX: 0 gives -1
        Some(count) => count as i64 - 1,
    };

    // Now fit nonrational curve to circle data
    let (x, _) = normalize(arc.x)?;
    let (y, _) = normalize(arc.y)?;
    let cen = arc.center;

    let rs = match arc.start {
        None => {
            let dd = as_ * (PI / 180.0);
            translate_sum2_pts(cen, rad * dd.cos(), x, rad * dd.sin(), y)
        }
        Some(p) => p,
    };
    let re = match arc.end {
        None => {
            if sweep == 360.0 {
                rs
            } else {
                let dd = ae * (PI / 180.0);
                translate_sum2_pts(cen, rad * dd.cos(), x, rad * dd.sin(), y)
            }
        }
        Some(p) => p,
    };

    // N_ApproxCircArcWithCrvData rejects nc < deg first (a negative nc included)
    let nc = usize::try_from(nc).map_err(|_| {
        SolidError::nlib(
            "nurbs::crv_approx::approx_circ_arc_with_crv_data",
            NlibCode::Input,
        )
    })?;
    let data = CircArcData {
        center: cen,
        x,
        y,
        radius: rad,
        start_angle: as_,
        end_angle: ae,
        start: rs,
        end: re,
    };
    approx_circ_arc_with_crv_data(&data, deg, nc)
}

// ---- SmBSplineCurve methods -----------------------------------------------------------

impl BSplineCurve {
    /// Replaces the NURB by a compact copy of `cur` with fixed-up knots (upstream's
    /// swap with `SmBSplineCurve(GetDim(), &cur)`); the other members stay.
    fn swap_in_nurb(&mut self, cur: &Curve) {
        let tmp = Self::from_gw_nurb(self.dim, cur);
        self.nurb = tmp.nurb;
    }

    /// The NURB, mutable (`GetOrCreateGwNurbPointer`).
    fn nurb_mut(&mut self, operation: &'static str) -> Result<&mut Curve> {
        self.nurb
            .as_mut()
            .ok_or_else(|| SolidError::sm_err(operation))
    }

    /// Cubic non-rational approximation of a circular arc of `radius` from
    /// `start_angle` to `end_angle` (degrees, counter-clockwise from the X axis of
    /// `frame`, in its XY plane) within `tol_percent_radius` percent of the radius
    /// (`ApproximateArc`). A sweep within `1e-10` of 90 degrees is made exactly 90.
    /// `num_control_points` fixes the control point count. A 2D result
    /// (`dim == 2`) drops `z`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when [`approx_circ_arc`] fails.
    pub fn approximate_arc(
        dim: u64,
        num_control_points: Option<u64>,
        frame: &Axis2Placement,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
        tol_percent_radius: f64,
    ) -> Result<Self> {
        let deg = 3;
        let tol = tol_percent_radius;
        let rad = radius;
        let as_ = start_angle;
        let mut ae = end_angle;

        // GWC??? I don't see what's being done with this change in arc size
        if (ae - as_ - 90.0).abs() < 1.0e-10 {
            ae = as_ + 90.0;
        }

        // Build approximation to arc - but without specifying exact endPoint values.
        let spec = CircArcSpec {
            center: frame.origin(),
            x: frame.x_axis(),
            y: frame.y_axis(),
            radius: rad,
            start_angle: as_,
            end_angle: ae,
            start: None,
            end: None,
        };
        let mut cur = approx_circ_arc(&spec, deg, tol, ToleranceKind::Relative, num_control_points)
            .map_err(SolidError::nl_ser(OP_ARC))?;

        // preserve 2D curves
        if dim == 2 {
            cur.to_2d();
        }

        Ok(Self::from_gw_nurb(dim, &cur))
    }

    /// Cubic non-rational approximation of this rational curve within `tol`
    /// (`ApproximateRationalCurve`, `N_ApproxNurbsWithNonRatCrv` with inherited
    /// parameters and end tangents kept); a 3D curve.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB, for a non-rational curve, or when the approximation
    /// fails.
    pub fn approximate_rational_curve(&self, tol: f64) -> Result<Self> {
        // input check - only run on rational BSplineCurves
        if !self.is_rational().map_err(SolidError::nl_ser(OP_RAT))? {
            return Err(SolidError::sm_err(OP_RAT));
        }
        let cur_this = nurb_of(self, OP_RAT)?;
        let deg = 3;
        let cur_new =
            approx_nurbs_with_non_rat_crv(cur_this, tol, deg, ParamSource::Inherited, true)
                .map_err(SolidError::nl_ser(OP_RAT))?;
        Ok(Self::from_gw_nurb(3, &cur_new))
    }

    /// Cubic B-spline approximation of a helix (`CreateHelixSegment`, through
    /// `N_CrvApproxSpiral`), placed by `spec.frame`; its form is `HelicalArc`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a height below `-SM_EFF_ZERO`, a negative radius or fewer
    /// than `SM_EFF_ZERO` turns (also for NaN); `SM_ERR` when the approximation fails.
    pub fn create_helix_segment(spec: &HelixSpec) -> Result<Self> {
        // check input
        let check = |ok: bool| {
            if ok {
                Ok(())
            } else {
                Err(invalid(OP_HELIX, "bad helix dimension"))
            }
        };
        check(spec.height >= -EFF_ZERO)?;
        check(spec.radius_start >= 0.0)?;
        check(spec.radius_end >= 0.0)?;
        check(spec.num_turns >= EFF_ZERO)?;

        // Build the helical approximation
        let res = approx_spiral(&SpiralSpec {
            height: spec.height,
            rad0: spec.radius_start,
            rad1: spec.radius_end,
            num_turns: spec.num_turns,
            hand: Hand::from_flag(i64::from(spec.right_handed)),
            num_points: 0,
            tol: spec.approx_tol,
        })
        .map_err(SolidError::nl_ser(OP_HELIX))?;

        // Build SMLib Curve from NLib Curve
        let mut new_curve = Self::from_gw_nurb(3, &res.curve);

        // position helix (status ignored upstream)
        let _ = new_curve.transform(&spec.frame, None);

        new_curve.form = BSplineCurveForm::HelicalArc;
        Ok(new_curve)
    }

    /// The curve extended to the parameter `parameter` with continuity `continuity`
    /// ([`ContinuityType::G1`] linear, [`ContinuityType::G1G2`] with second
    /// derivative, [`ContinuityType::CInfinity`] polynomial) (`CreateExtendedCurve`,
    /// through `N_CrvEvalUnboundedPtsAndDerivs`). Without `precise` a parameter outside
    /// the natural interval moves one interval length further out. `None` when the
    /// parameter is inside the interval (nothing to extend); a G1-closed curve comes
    /// back unextended (it is evaluated periodically).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB, for another continuity, or when the extension fails.
    pub fn create_extended_curve(
        &self,
        parameter: f64,
        continuity: ContinuityType,
        precise: bool,
    ) -> Result<Option<Self>> {
        // get curve interval
        let ivl = self.natural_interval();
        let mut parameter = parameter;

        // when bPreciseExtension == 0 - extend dParameter value by curve's interval amount
        if !precise {
            if parameter < ivl.min() {
                parameter -= ivl.max() - ivl.min();
            } else if parameter > ivl.max() {
                parameter += ivl.max() - ivl.min();
            }
        }

        let cur = nurb_of(self, OP_EXT)?;
        let t = parameter;
        let ndr = 0;
        let gflg = extension_continuity(continuity, OP_EXT)?;

        // NLIB eval curve at t,
        // side effect: create extended curve in ecur when t is outside curve interval
        let mut state = UnboundedEval::default();
        cur.eval_unbounded_pts_and_derivs(t, ndr, gflg, &mut state)
            .map_err(SolidError::nl_ser(OP_EXT))?;

        // build output: new SMBSplineCurve object from ecur
        Ok(state.ext.map(|ecur| Self::from_gw_nurb(self.dim, &ecur)))
    }

    /// The curve extended at `end` by about the arc length `distance` with continuity
    /// `continuity` (as [`BSplineCurve::create_extended_curve`])
    /// (`CreateExtendedCurve`, distance overload, through `N_CrvExtendByDist`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB, for another continuity, or when the extension fails.
    pub fn create_extended_curve_by_distance(
        &self,
        distance: f64,
        end: CurveEnd,
        continuity: ContinuityType,
    ) -> Result<Option<Self>> {
        let cur = nurb_of(self, OP_EXT_DIST)?;
        let dist = distance;
        let gflg = extension_continuity(continuity, OP_EXT_DIST)?;

        let ecur = cur
            .extend_by_dist(dist, ToleranceKind::Absolute, end, gflg)
            .map_err(SolidError::nl_ser(OP_EXT_DIST))?;

        // build output: new SMBSplineCurve object from ecur
        Ok(Some(Self::from_gw_nurb(self.dim, &ecur)))
    }

    /// A curve of dimension `dim` (2 or 3) and degree `degree` interpolating `points`
    /// (`CreateInterpolatingCurve`), with parameters by `par`:
    /// - degree 1 and two points: the line segment;
    /// - one vector per point (more than two points; degree 2 or 3): first derivatives
    ///   at every point (`ignore_vector_magnitude`: only their directions);
    /// - two vectors: start and end first derivatives, plus the second (and third)
    ///   derivatives from `more_vectors[0..2]` (`[2..4]`), raising the degree to 3 (4);
    /// - one vector: the start first derivative, plus the second from
    ///   `more_vectors[0]`;
    /// - no vectors: plain interpolation.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for another dimension or vector count, a degree outside the branch's
    /// range, or when the fit fails; the errors of
    /// [`BSplineCurve::create_line_segment`].
    pub fn create_interpolating_curve(
        par: ParamMethod,
        dim: u64,
        degree: usize,
        points: &[Point3],
        vectors: &[Vec3],
        more_vectors: Option<&[Vec3]>,
        ignore_vector_magnitude: bool,
    ) -> Result<Self> {
        const OP: &str = OP_INTERP_CRV;
        let mut degree = degree;

        if degree == 1 && points.len() == 2 {
            let p0 = vec_at(points, 0, OP)?;
            let p1 = vec_at(points, 1, OP)?;
            return Self::create_line_segment(dim, p0, p1);
        }

        if dim != 2 && dim != 3 {
            return Err(SolidError::sm_err(OP));
        }

        let more = more_vectors.unwrap_or(&[]);

        // branch to construct new gw_CURVE
        let cur = if vectors.len() == points.len() && points.len() > 2 {
            if degree != 2 && degree != 3 {
                return Err(SolidError::sm_err(OP));
            }
            let mut d = vectors.to_vec();
            let vec = if ignore_vector_magnitude {
                VectorKind::Tangent
            } else {
                VectorKind::Derivative
            };
            interp_first_derivs(points, &mut d, degree, vec, par).map_err(SolidError::nl_ser(OP))?
        } else if vectors.len() == 2 {
            if !(2..=MAX_DEGREE).contains(&degree) {
                return Err(SolidError::sm_err(OP));
            }
            let mut ds = vec![vec_at(vectors, 0, OP)?];
            let mut de = vec![vec_at(vectors, 1, OP)?];
            if more_vectors.is_some() && more.len() > 1 {
                ds.push(vec_at(more, 0, OP)?);
                de.push(vec_at(more, 1, OP)?);
                if degree <= 2 {
                    degree = 3;
                }
                if more.len() > 3 {
                    ds.push(vec_at(more, 2, OP)?);
                    de.push(vec_at(more, 3, OP)?);
                    if degree <= 3 {
                        degree = 4;
                    }
                }
            }
            interp_high_derivs(points, degree, &ds, &de, par).map_err(SolidError::nl_ser(OP))?
        } else if vectors.len() == 1 {
            // Start Vector
            let mut ds = vec![vec_at(vectors, 0, OP)?];
            if more_vectors.is_some() && !more.is_empty() {
                ds.push(vec_at(more, 0, OP)?);
            }
            interp_high_derivs(points, degree, &ds, &[], par).map_err(SolidError::nl_ser(OP))?
        } else if vectors.is_empty() {
            interp_high_derivs(points, degree, &[], &[], par).map_err(SolidError::nl_ser(OP))?
        } else {
            return Err(SolidError::sm_err(OP));
        };

        // construct output from new gw_CURVE
        Ok(Self::from_gw_nurb(dim, &cur))
    }

    /// A curve of dimension `dim` (not checked, as upstream) and degree `degree` with
    /// `num_points` control points approximating `points` in the least-squares sense
    /// (`CreateApproximatingCurve`, through `N_FitCrvApproxDerivs`), with parameters by
    /// `par`:
    /// - two vectors: start and end first derivatives, plus the second (and third)
    ///   derivatives from `more_vectors[0..2]` (`[2..4]`);
    /// - one vector: the start first derivative, plus the second from `more_vectors[0]`
    ///   and the third from `more_vectors[2]` (the documented layout: `[0]`/`[1]` start /
    ///   end second derivatives, `[2]`/`[3]` start / end third; read only when present);
    /// - no vectors: no derivative constraints.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for more than two vectors, a degree outside `2..=MAX_DEGREE` with two or
    /// no vectors, no points or no control points, or when the fit fails.
    pub fn create_approximating_curve(
        num_points: usize,
        par: ParamMethod,
        dim: u64,
        degree: usize,
        points: &[Point3],
        vectors: &[Vec3],
        more_vectors: Option<&[Vec3]>,
    ) -> Result<Self> {
        const OP: &str = OP_APPROX_CRV;
        // n = lNumPoints - 1 is -1 for no control points: N_FitCrvApproxDerivs' p > n
        let n = num_points
            .checked_sub(1)
            .ok_or_else(|| SolidError::sm_err(OP))?;
        let p = degree;

        // option 1: given two end tangents as constraints
        let cur = if vectors.len() == 2 {
            // check input - range on degree
            if !(2..=MAX_DEGREE).contains(&degree) {
                return Err(SolidError::sm_err(OP));
            }
            // Ds[1] = vectors[0], De[1] = vectors[1]
            let mut ds = vec![vec_at(vectors, 0, OP)?];
            let mut de = vec![vec_at(vectors, 1, OP)?];
            // when given, the 2nd (and 3rd) derivatives
            if let Some(more) = more_vectors.filter(|m| m.len() > 1) {
                ds.push(vec_at(more, 0, OP)?);
                de.push(vec_at(more, 1, OP)?);
                if more.len() > 3 {
                    ds.push(vec_at(more, 2, OP)?);
                    de.push(vec_at(more, 3, OP)?);
                }
            }
            approx_end_derivs(points, &ds, &de, n, p, par).map_err(SolidError::nl_ser(OP))?
        }
        // option 2: given one Vector as constraint
        else if vectors.len() == 1 {
            let mut ds = vec![vec_at(vectors, 0, OP)?];
            if let Some(more) = more_vectors.filter(|m| !m.is_empty()) {
                ds.push(vec_at(more, 0, OP)?);
                if more.len() > 2 {
                    ds.push(vec_at(more, 2, OP)?);
                }
            }
            approx_end_derivs(points, &ds, &[], n, p, par).map_err(SolidError::nl_ser(OP))?
        }
        // option 3: no Vectors to interpolate
        else if vectors.is_empty() {
            if !(2..=MAX_DEGREE).contains(&degree) {
                return Err(SolidError::sm_err(OP));
            }
            approx_end_derivs(points, &[], &[], n, p, par).map_err(SolidError::nl_ser(OP))?
        } else {
            // unsupported option
            return Err(SolidError::sm_err(OP));
        };

        // set output
        Ok(Self::from_gw_nurb(dim, &cur))
    }

    /// Splits knot spans (at their middle, inserting the new knot `multiplicity`
    /// times) until no span is more than `max_delta_factor` times longer than a
    /// neighbouring span (`DampenKnotSpacing`); the spans are compared on the unique
    /// knots.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB or when the knot insertion fails;
    /// [`SolidError::NotConverged`] after [`DAMPEN_SPLIT_LIMIT`] splits;
    /// `InvalidInput` when the new knots do not fit in memory.
    pub fn dampen_knot_spacing(
        &mut self,
        max_delta_factor: f64,
        multiplicity: usize,
    ) -> Result<()> {
        let mut new_knots: Vec<f64> = Vec::new();
        let (mut knot_vector, _) = self.knots(None).map_err(SolidError::nl_ser(OP_DAMPEN))?;
        let mut splits = 0usize;

        let mut done = false;
        while !done {
            done = true;
            let mut i = 0usize;
            while i + 1 < knot_vector.len() {
                // note: can't say sNewKnotVector.GetSize()-1
                let mut split = false;
                let span_size =
                    knot_at(&knot_vector, i + 1, OP_DAMPEN)? - knot_at(&knot_vector, i, OP_DAMPEN)?;
                if i > 0 {
                    // Look at size of previous span
                    let last_span_size = knot_at(&knot_vector, i, OP_DAMPEN)?
                        - knot_at(&knot_vector, i - 1, OP_DAMPEN)?;
                    if span_size / last_span_size > max_delta_factor {
                        split = true;
                    }
                }
                if i + 2 < knot_vector.len() {
                    // Look at size of next span
                    let next_span_size = knot_at(&knot_vector, i + 2, OP_DAMPEN)?
                        - knot_at(&knot_vector, i + 1, OP_DAMPEN)?;
                    if span_size / next_span_size > max_delta_factor {
                        split = true;
                    }
                }
                // See if we need to split this span
                if !split {
                    i += 1;
                    continue;
                }

                // Split the span
                splits += 1;
                if splits > DAMPEN_SPLIT_LIMIT {
                    return Err(SolidError::NotConverged {
                        operation: OP_DAMPEN,
                        iterations: DAMPEN_SPLIT_LIMIT as u32,
                    });
                }
                done = false;
                let split_knot = (knot_at(&knot_vector, i + 1, OP_DAMPEN)?
                    + knot_at(&knot_vector, i, OP_DAMPEN)?)
                    / 2.0;
                knot_vector.insert(i + 1, split_knot);
                let insert_point = new_knots
                    .iter()
                    .position(|&k| k > split_knot)
                    .unwrap_or(new_knots.len());
                // (InsertAt with a count; fallible, as the caller's multiplicity sizes it)
                new_knots.try_reserve(multiplicity).map_err(|_| {
                    SolidError::invalid_input(OP_DAMPEN, "new knots too large for memory")
                })?;
                new_knots.splice(
                    insert_point..insert_point,
                    core::iter::repeat_n(split_knot, multiplicity),
                );
                i += 1;
            }
        }

        if !new_knots.is_empty() {
            self.insert_knots(&new_knots)?;
        }
        Ok(())
    }

    /// Reduces the degree by one within `tol`, or (`max_reduce`) as far as possible
    /// (`DegreeReduction`, `N_CrvReduceDegreeOnce` / `N_CrvReduceDegree`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB, when the reduction fails, or when a single reduction
    /// is not within `tol`.
    pub fn degree_reduction(&mut self, tol: f64, max_reduce: bool) -> Result<()> {
        let cur_p = nurb_of(self, OP_DEG_RED)?;
        let cur_new = if max_reduce {
            cur_p
                .reduce_degree(tol)
                .map_err(SolidError::nl_ser(OP_DEG_RED))?
        } else {
            let red = cur_p
                .reduce_degree_once(tol)
                .map_err(SolidError::nl_ser(OP_DEG_RED))?;
            match red.curve {
                Some(c) => c,
                // MSG(_T("Degree reduction failure"))
                None => return Err(SolidError::sm_err(OP_DEG_RED)),
            }
        };
        self.swap_in_nurb(&cur_new);
        Ok(())
    }

    /// Removes every knot whose removal keeps the curve within `tol` of the current
    /// one, keeping the 1st to `highest_deriv_constraint`-th derivatives at both ends
    /// when `constrain_end_deriv` (`RemoveKnots`, `N_CrvRemoveKnotsDerivConstraints`;
    /// upstream's defaults are `false` and `0`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB or when the removal fails.
    pub fn remove_knots(
        &mut self,
        tol: f64,
        constrain_end_deriv: bool,
        highest_deriv_constraint: usize,
    ) -> Result<()> {
        let (constraints, der) = if constrain_end_deriv {
            (ConstrainedEnds::Both, highest_deriv_constraint)
        } else {
            (ConstrainedEnds::None, 0)
        };
        let cur_p = nurb_of(self, OP_REMOVE_KNOTS)?;
        let cur_new = cur_p
            .remove_knots_deriv_constraints(tol, constraints, der)
            .map_err(SolidError::nl_ser(OP_REMOVE_KNOTS))?;
        self.swap_in_nurb(&cur_new);
        Ok(())
    }

    /// Inserts the interior knot `knot` `times` times (`InsertOneKnot`,
    /// `N_CrvInsertKnot`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB or when the insertion fails (a knot on or outside the
    /// ends, or a multiplicity above the degree).
    pub fn insert_one_knot(&mut self, knot: f64, times: usize) -> Result<()> {
        let cur_p = nurb_of(self, OP_INS_ONE)?;
        let cur_new = cur_p
            .insert_knot(knot, times)
            .map_err(SolidError::nl_ser(OP_INS_ONE))?;
        self.swap_in_nurb(&cur_new);
        Ok(())
    }

    /// Makes a piecewise C0 cubic intersection curve interpolate `through` (with
    /// `tangent`, or its current tangent there) at about the parameter `*new_knot`,
    /// editing the matching UV trim curves on `surfaces` the same way
    /// (`SetIntersectionCurveThroughPoint`); `*new_knot` becomes the parameter used.
    /// Returns whether the curves were modified. `uv_points[i]` is the UV point of
    /// `through` on `surfaces[i]`, or `None` to project it; spans shorter than
    /// `min_span_percent` of the span are not created (an existing knot moves).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a curve that is not a cubic intersection approximation, a failed
    /// projection or knot insertion, or an unknown case; `InvalidInput` for array
    /// lengths that do not match the curves; [`SolidError::Unported`] (see the module
    /// deviations).
    pub fn set_intersection_curve_through_point<S: SurfaceEvaluate>(
        &mut self,
        k: &Kernel,
        new_knot: &mut f64,
        through: Point3,
        tangent: Option<Vec3>,
        uv_curves: &mut [Self],
        surfaces: &[S],
        uv_points: &[Option<Vec2>],
        min_span_percent: f64,
    ) -> Result<bool> {
        const OP: &str = OP_THROUGH;

        // check state - Quit if the curve is not a piecewise C0, degree 3 curve
        if !is_intersector_approx(self)? {
            return Err(SolidError::sm_err(OP));
        }

        // locals
        let interval = self.natural_interval();
        let uv_curve_count = uv_curves.len();
        let mut case: i64;
        let mut i_spn1: usize;
        let i_spn2: usize;
        let (degree, m_knots, mut pu) = {
            let crv = nurb_of(self, OP)?;
            (crv.degree(), crv.m(), crv.knots().to_vec())
        };

        // Find the target span containing or nearest to dNewKnot
        if *new_knot < interval.min() {
            i_spn1 = degree;
            case = 1;
        } else if *new_knot > interval.max() {
            i_spn1 = m_knots
                .checked_sub(degree + 1)
                .ok_or_else(|| invalid(OP, "knot vector too short"))?;
            case = 8;
        } else {
            // find the knot span, multiplicity, and nearest distance for dNewKnot
            // (the status is ignored upstream)
            let crv = nurb_of(self, OP)?;
            let kv = crv.knot_vector()?;
            i_spn1 = kv
                .find_span_and_mult(degree, *new_knot, Side::Left)
                .map_or(0, |(s, _)| s);
            case = 4;
        }

        // get the target span's size
        let u_s1 = knot_at(&pu, i_spn1, OP)?;
        let u_s1p = knot_at(&pu, i_spn1 + 1, OP)?;
        let span_interval = Extent1d::from_min_max(u_s1, u_s1p);
        let span_size = u_s1p - u_s1;

        // find the closest span boundary to dNewKnot,
        // boundary outward tangent vector, and displacement vector
        let mut near_knot_point = [Vec3::UNDEF; 2];
        let min_dist;
        let top_node;
        if (*new_knot - u_s1).abs() < (*new_knot - u_s1p).abs() {
            min_dist = (*new_knot - u_s1).abs();
            near_knot_point = eval2(self, u_s1, true, near_knot_point);
            near_knot_point[1] = -near_knot_point[1];
            top_node = false;
        } else {
            min_dist = (*new_knot - u_s1p).abs();
            near_knot_point = eval2(self, u_s1p, true, near_knot_point);
            top_node = true;
        }
        near_knot_point[1] = near_knot_point[1].unitize_or_z();
        let disp_vec = through - near_knot_point[0];

        // get span distance and 3d distance from nearest knot to through point
        let span_length = approximate_length(k, self, &span_interval, 5)?;
        let proj_dist_3d = near_knot_point[1].dot(disp_vec);

        // Use existing knots when adding a new knot would create a too small internal segment
        let move_knot = (min_dist / span_size < min_span_percent)
            && (proj_dist_3d > -0.01 * span_length)
            && (proj_dist_3d < 0.01);

        // classify the dNewKnot value into one of the cases
        let last_span = m_knots
            .checked_sub(degree + 1)
            .ok_or_else(|| invalid(OP, "knot vector too short"))?;
        case = if case == 1 && move_knot {
            2
        } else if case == 4 && move_knot && i_spn1 == degree && !top_node {
            3
        } else if case == 4 && move_knot && i_spn1 == last_span && top_node {
            6
        } else if case == 4 && move_knot {
            5
        } else if case == 8 && move_knot {
            7
        } else {
            case
        };

        // handle special case 5 when moving the bottom knot
        if case == 5 && !top_node {
            // move iSpn1 pointer down one interval
            i_spn1 = i_spn1
                .checked_sub(degree)
                .ok_or_else(|| invalid(OP, "span index out of range"))?;
        }

        // prepare a 3 point ThroughPoint array
        let mut pos_tang = [Vec3::UNDEF; 8];
        let new_tangent_index: usize;
        let mut uv_pos_tang = vec![Vec2::UNDEF; UV_VEC_COUNT * uv_curve_count];
        let scale1: f64;
        let scale2: f64;
        let mut param_array = [0.0; 3];
        let ku = |i: usize| knot_at(&pu, i, OP);

        match case {
            1 => {
                let add_knot = ku(i_spn1)?
                    + (ku(i_spn1 + 1)? - ku(i_spn1)?) * (ku(i_spn1)? - *new_knot)
                        / (ku(i_spn1 + 1)? - *new_knot);
                *new_knot = ku(i_spn1)?;

                param_array[0] = *new_knot; // to interpolate crThrough3dPoint
                param_array[1] = add_knot; // to interpolate current endPoint
                param_array[2] = ku(i_spn1 + 1)?;

                // scale CP spacing in 1st seg by ratio of segment sizes
                scale1 = (param_array[1] - param_array[0]) / (param_array[2] - param_array[1]);
                scale2 = 1.0;
                i_spn2 = i_spn1;

                pos_tang[0] = through;
                new_tangent_index = 1;
                [pos_tang[2], pos_tang[3]] =
                    eval2(self, param_array[0], true, [pos_tang[2], pos_tang[3]]);
                [pos_tang[4], pos_tang[5]] =
                    eval2(self, param_array[2], false, [pos_tang[4], pos_tang[5]]);
            }
            2 | 3 => {
                *new_knot = ku(i_spn1)?;
                param_array[0] = *new_knot;
                param_array[1] = ku(i_spn1 + 1)?;
                param_array[2] = param_array[1]; // unused - but initialized

                // scale CP spacing by change in arc length
                scale1 = (span_length - proj_dist_3d) / span_length;
                scale2 = 1.0;
                i_spn2 = i_spn1;

                pos_tang[0] = through;
                new_tangent_index = 1;
                [pos_tang[2], pos_tang[3]] =
                    eval2(self, param_array[1], false, [pos_tang[2], pos_tang[3]]);
                pos_tang[4] = pos_tang[2];
                pos_tang[5] = pos_tang[3];
            }
            4 => {
                param_array[0] = ku(i_spn1)?;
                param_array[1] = *new_knot;
                param_array[2] = ku(i_spn1 + 1)?;

                // scale CP spacing by parameter ratios
                scale1 = (param_array[1] - param_array[0]) / (param_array[2] - param_array[0]);
                scale2 = (param_array[2] - param_array[1]) / (param_array[2] - param_array[0]);
                i_spn2 = i_spn1;

                [pos_tang[0], pos_tang[1]] =
                    eval2(self, param_array[0], true, [pos_tang[0], pos_tang[1]]);
                pos_tang[2] = through;
                new_tangent_index = 3;
                [pos_tang[4], pos_tang[5]] =
                    eval2(self, param_array[2], false, [pos_tang[4], pos_tang[5]]);
            }
            5 => {
                param_array[0] = ku(i_spn1)?;
                param_array[1] = *new_knot;
                param_array[2] = ku(i_spn1 + 1 + degree)?;

                // scale CP spacing by change in param length
                scale1 = (param_array[1] - param_array[0]) / (ku(i_spn1 + 1)? - ku(i_spn1)?);
                scale2 = (param_array[2] - param_array[1])
                    / (ku(i_spn1 + 1 + degree)? - ku(i_spn1 + 1)?);
                i_spn2 = i_spn1 + degree;

                [pos_tang[0], pos_tang[1]] =
                    eval2(self, param_array[0], true, [pos_tang[0], pos_tang[1]]);
                pos_tang[2] = through;
                new_tangent_index = 3;
                [pos_tang[4], pos_tang[5]] =
                    eval2(self, param_array[2], false, [pos_tang[4], pos_tang[5]]);
            }
            6 | 7 => {
                *new_knot = ku(i_spn1 + 1)?;
                param_array[0] = ku(i_spn1)?;
                param_array[1] = *new_knot;
                param_array[2] = param_array[1]; // unused - but initialized

                // scale CP spacing by change in arc length
                scale1 = (span_length + proj_dist_3d) / span_length;
                scale2 = 1.0;
                i_spn2 = i_spn1;

                [pos_tang[0], pos_tang[1]] =
                    eval2(self, param_array[0], true, [pos_tang[0], pos_tang[1]]);
                pos_tang[2] = through;
                new_tangent_index = 3;
                pos_tang[4] = pos_tang[0];
                pos_tang[5] = pos_tang[1];
            }
            8 => {
                let add_knot = ku(i_spn1)?
                    + (ku(i_spn1 + 1)? - ku(i_spn1)?) * (ku(i_spn1)? - *new_knot)
                        / (ku(i_spn1 + 1)? - *new_knot);
                *new_knot = ku(i_spn1 + 1)?;

                param_array[0] = ku(i_spn1)?;
                param_array[1] = add_knot; // to interpolate current endPoint
                param_array[2] = *new_knot; // to interpolate crThrough3dPoint

                // scale CP spacing in 1st seg by ratio of segment sizes
                scale1 = 1.0;
                scale2 = (param_array[2] - param_array[1]) / (param_array[1] - param_array[0]);
                i_spn2 = i_spn1;

                [pos_tang[0], pos_tang[1]] =
                    eval2(self, param_array[0], true, [pos_tang[0], pos_tang[1]]);
                [pos_tang[2], pos_tang[3]] =
                    eval2(self, param_array[2], true, [pos_tang[2], pos_tang[3]]);
                pos_tang[4] = through;
                new_tangent_index = 5;
            }
            _ => return Err(SolidError::sm_err(OP)),
        }

        // set the tangent value for the through point
        if let Some(t) = tangent {
            *pos_tang
                .get_mut(new_tangent_index)
                .ok_or_else(|| invalid(OP, "tangent index out of range"))? = t;
        } else {
            // current Tangent value at param value - allow OutOfBounds evaluations
            let saved = self.out_of_bounds_enabled;
            self.out_of_bounds_enabled = true;
            [pos_tang[6], pos_tang[7]] = eval2(self, *new_knot, true, [pos_tang[6], pos_tang[7]]);
            self.out_of_bounds_enabled = saved;
            *pos_tang
                .get_mut(new_tangent_index)
                .ok_or_else(|| invalid(OP, "tangent index out of range"))? = pos_tang[7];
        }

        // normalize the 3 tangent vectors (must have been initialised if unused)
        pos_tang[1] = pos_tang[1].unitize_or_z();
        pos_tang[3] = pos_tang[3].unitize_or_z();
        pos_tang[5] = pos_tang[5].unitize_or_z();

        // get the control point spacing for the segments being modified
        let lo1 = i_spn1
            .checked_sub(degree)
            .ok_or_else(|| invalid(OP, "span index out of range"))?;
        let lo2 = i_spn2
            .checked_sub(degree)
            .ok_or_else(|| invalid(OP, "span index out of range"))?;
        let (orig_cp_spacing1, orig_cp_spacing2) = {
            let pw = nurb_of(self, OP)?.cpts();
            (
                control_point_spacing(cpt_at(pw, lo1, OP)?, cpt_at(pw, lo1 + 1, OP)?),
                control_point_spacing(cpt_at(pw, lo2, OP)?, cpt_at(pw, lo2 + 1, OP)?),
            )
        };

        // UVCurve, segment2 spacing memory
        let mut uv_cp_spacing1: Vec<f64> = Vec::new();
        let mut uv_cp_spacing2: Vec<f64> = Vec::new();

        // for all UVCurves - set sUVPosTang array values
        for ii in 0..uv_curve_count {
            let off = UV_VEC_COUNT * ii;
            let uv_curve = uv_curves
                .get(ii)
                .ok_or_else(|| invalid(OP, "uv curve index out of range"))?;
            let surface = surfaces
                .get(ii)
                .ok_or_else(|| invalid(OP, "fewer surfaces than uv curves"))?;
            let uv_point = vec_at(uv_points, ii, OP)?;

            // for all three points
            let mut jj = 0usize;
            while jj < 3 {
                let param_jj = vec_at(&param_array, jj, OP)?;
                let pos_jj = vec_at(&pos_tang, 2 * jj, OP)?;
                let tan_jj = vec_at(&pos_tang, 2 * jj + 1, OP)?;
                // get Surface1 UV point - either as given or by projection
                let uv_slot = if let (true, Some(uv)) = (param_jj == *new_knot, uv_point) {
                    uv
                } else {
                    // evaluate UVCurve1 to get guess point for upcoming projection
                    // (the status is ignored upstream)
                    let guess_3d = uv_curve.evaluate_point(param_jj).unwrap_or(Vec3::UNDEF);

                    // project 3dPoint (sPosTang[2*jj]) to Surface1 near sUVGuessPoint
                    let guess = Vec2::new(guess_3d.x, guess_3d.y);
                    let domain = surface.natural_uv_domain_with_nurb(k)?;
                    let (_found, sol_value, sol_params) =
                        surface_local_point_solve(k, surface, &domain, pos_jj, guess)?;

                    // check and store the Surface1 answer
                    if sol_value > 1e-8 {
                        return Err(SolidError::sm_err(OP));
                    }
                    sol_params
                };
                *uv_pos_tang
                    .get_mut(off + 2 * jj)
                    .ok_or_else(|| invalid(OP, "uv index out of range"))? = uv_slot;

                // get Surface values at UVpoints
                let vals = surface_evaluate(k, surface, uv_slot)?;

                // project unit 3D vector onto Surface plane
                let dropped = surface_drop_vector(vals[1][0], vals[0][1], tan_jj)?;

                // unitize the vector (a failed Unitize leaves it unchanged)
                let tan_slot = uv_pos_tang
                    .get_mut(off + 2 * jj + 1)
                    .ok_or_else(|| invalid(OP, "uv index out of range"))?;
                *tan_slot = dropped.unitize().unwrap_or(dropped);

                // get the control point spacing for the segments being modified
                let pw_uv = nurb_of(uv_curve, OP)?.cpts();
                uv_cp_spacing1.push(control_point_spacing(
                    cpt_at(pw_uv, lo1, OP)?,
                    cpt_at(pw_uv, lo1 + 1, OP)?,
                ));
                uv_cp_spacing2.push(control_point_spacing(
                    cpt_at(pw_uv, lo2, OP)?,
                    cpt_at(pw_uv, lo2 + 1, OP)?,
                ));

                // some cases only use two points
                if jj == 1 && (case == 2 || case == 3 || case == 6 || case == 7) {
                    jj += 1;
                }
                jj += 1;
            }
        }

        // modify the knot vector so that dNewKnot is the interpolating parameter
        let modified_curve = true;

        // new curves for cases 1, 4, 8 (the control points are edited there)
        let mut new_crv: Option<Curve> = None;
        let mut new_uv_crv: Vec<Curve> = Vec::new();

        match case {
            2 | 3 => {
                // move start Interval end-point params to dNewKnot
                let first = if i_spn1 == degree {
                    0
                } else {
                    i_spn1 + 1 - degree
                };
                for ii in first..=i_spn1 {
                    *pu.get_mut(ii)
                        .ok_or_else(|| invalid(OP, "knot index out of range"))? = *new_knot;
                    for uv_curve in uv_curves.iter_mut() {
                        let u = uv_curve.nurb_mut(OP)?.knots_mut();
                        *u.get_mut(ii)
                            .ok_or_else(|| invalid(OP, "knot index out of range"))? = *new_knot;
                    }
                }
                self.nurb_mut(OP)?.knots_mut().copy_from_slice(&pu);
            }
            5..=7 => {
                // move end Interval end-point params to dNewKnot
                let last = if i_spn1 == last_span {
                    i_spn1 + degree + 1
                } else {
                    i_spn1 + degree
                };
                let mut ii = last;
                while ii > i_spn1 {
                    *pu.get_mut(ii)
                        .ok_or_else(|| invalid(OP, "knot index out of range"))? = *new_knot;
                    for uv_curve in uv_curves.iter_mut() {
                        let u = uv_curve.nurb_mut(OP)?.knots_mut();
                        *u.get_mut(ii)
                            .ok_or_else(|| invalid(OP, "knot index out of range"))? = *new_knot;
                    }
                    ii -= 1;
                }
                self.nurb_mut(OP)?.knots_mut().copy_from_slice(&pu);
            }
            _ => {
                // cases 1, 4, 8: insert a new knot at 1st span's dParamArray[1] mid-point
                let crv = nurb_of(self, OP)?;
                new_crv = Some(
                    crv.insert_knot(param_array[1], degree)
                        .map_err(SolidError::nl_ser(OP))?,
                );
                for uv_curve in uv_curves.iter() {
                    let uvc = nurb_of(uv_curve, OP)?;
                    new_uv_crv.push(
                        uvc.insert_knot(param_array[1], degree)
                            .map_err(SolidError::nl_ser(OP))?,
                    );
                }
            }
        }

        // get the control point locations - for the first segment
        let mut p_1 = pos_tang[0];
        let mut p_4 = pos_tang[2];

        // scale mid ControlPoint spacing
        let mut d_1 = pos_tang[1] * orig_cp_spacing1 * scale1;
        let mut d_2 = pos_tang[3] * orig_cp_spacing1 * scale1;

        // mid-ControlPoint positions
        let mut p_2 = p_1 + d_1;
        let mut p_3 = p_4 - d_2;

        {
            let pw = match new_crv.as_mut() {
                Some(c) => c.cpts_mut(),
                None => self.nurb_mut(OP)?.cpts_mut(),
            };
            *cpt_mut(pw, lo1, OP)? = CPoint::new(p_1.x, p_1.y, p_1.z, NOW);
            *cpt_mut(pw, lo1 + 1, OP)? = CPoint::new(p_2.x, p_2.y, p_2.z, NOW);
            *cpt_mut(pw, lo1 + 2, OP)? = CPoint::new(p_3.x, p_3.y, p_3.z, NOW);
            *cpt_mut(pw, lo1 + 3, OP)? = CPoint::new(p_4.x, p_4.y, p_4.z, NOW);
        }

        // Do the same for every UVCurve
        for ii in 0..uv_curve_count {
            let off = UV_VEC_COUNT * ii;
            let puv_1 = vec_at(&uv_pos_tang, off, OP)?;
            let puv_4 = vec_at(&uv_pos_tang, off + 2, OP)?;
            let sp1 = vec_at(&uv_cp_spacing1, ii, OP)?;

            // scale UVCurve mid-CP spacing same as 3d curve
            let duv_1 = vec_at(&uv_pos_tang, off + 1, OP)? * sp1 * scale1;
            let duv_2 = vec_at(&uv_pos_tang, off + 3, OP)? * sp1 * scale1;

            // mid-ControlPoint positions
            let puv_2 = puv_1 + duv_1;
            let puv_3 = puv_4 - duv_2;

            let pw = if new_uv_crv.is_empty() {
                uv_curves
                    .get_mut(ii)
                    .ok_or_else(|| invalid(OP, "uv curve index out of range"))?
                    .nurb_mut(OP)?
                    .cpts_mut()
            } else {
                new_uv_crv
                    .get_mut(ii)
                    .ok_or_else(|| invalid(OP, "uv curve index out of range"))?
                    .cpts_mut()
            };
            let z_val = cpt_at(pw, lo1, OP)?.z;
            *cpt_mut(pw, lo1, OP)? = CPoint::new(puv_1.x, puv_1.y, z_val, NOW);
            *cpt_mut(pw, lo1 + 1, OP)? = CPoint::new(puv_2.x, puv_2.y, z_val, NOW);
            *cpt_mut(pw, lo1 + 2, OP)? = CPoint::new(puv_3.x, puv_3.y, z_val, NOW);
            *cpt_mut(pw, lo1 + 3, OP)? = CPoint::new(puv_4.x, puv_4.y, z_val, NOW);
        }

        // for all cases which require updating the control points for a 2nd segment
        if case == 1 || case == 4 || case == 5 || case == 8 {
            // get the control point locations - for the 2nd segment
            p_1 = pos_tang[2];
            p_4 = pos_tang[4];

            // scale mid-ControlPoint spacing
            d_1 = pos_tang[3] * orig_cp_spacing2 * scale2;
            d_2 = pos_tang[5] * orig_cp_spacing2 * scale2;

            // mid-ControlPoint positions
            p_2 = p_1 + d_1;
            p_3 = p_4 - d_2;

            {
                let pw = match new_crv.as_mut() {
                    Some(c) => c.cpts_mut(),
                    None => self.nurb_mut(OP)?.cpts_mut(),
                };
                *cpt_mut(pw, lo1 + 4, OP)? = CPoint::new(p_2.x, p_2.y, p_2.z, NOW);
                *cpt_mut(pw, lo1 + 5, OP)? = CPoint::new(p_3.x, p_3.y, p_3.z, NOW);
                *cpt_mut(pw, lo1 + 6, OP)? = CPoint::new(p_4.x, p_4.y, p_4.z, NOW);
            }

            // Do the same for every UVCurve
            for ii in 0..uv_curve_count {
                let off = UV_VEC_COUNT * ii;
                let puv_1 = vec_at(&uv_pos_tang, off + 2, OP)?;
                let puv_4 = vec_at(&uv_pos_tang, off + 4, OP)?;
                let sp2 = vec_at(&uv_cp_spacing2, ii, OP)?;

                // scale mid CP spacing the same as the 3d curve
                let duv_1 = vec_at(&uv_pos_tang, off + 3, OP)? * sp2 * scale2;
                let duv_2 = vec_at(&uv_pos_tang, off + 5, OP)? * sp2 * scale2;

                // get mid-ControlPoint positions
                let puv_2 = puv_1 + duv_1;
                let puv_3 = puv_4 - duv_2;

                let pw = if new_uv_crv.is_empty() {
                    uv_curves
                        .get_mut(ii)
                        .ok_or_else(|| invalid(OP, "uv curve index out of range"))?
                        .nurb_mut(OP)?
                        .cpts_mut()
                } else {
                    new_uv_crv
                        .get_mut(ii)
                        .ok_or_else(|| invalid(OP, "uv curve index out of range"))?
                        .cpts_mut()
                };
                let z_val = cpt_at(pw, lo1 + 4, OP)?.z;
                *cpt_mut(pw, lo1 + 4, OP)? = CPoint::new(puv_2.x, puv_2.y, z_val, NOW);
                *cpt_mut(pw, lo1 + 5, OP)? = CPoint::new(puv_3.x, puv_3.y, z_val, NOW);
                *cpt_mut(pw, lo1 + 6, OP)? = CPoint::new(puv_4.x, puv_4.y, z_val, NOW);
            }
        }

        // for all cases which inserted knots: make the changes stick
        if let Some(c) = new_crv {
            self.swap_in_nurb(&c);
            for (uv_curve, c) in uv_curves.iter_mut().zip(new_uv_crv.iter()) {
                uv_curve.swap_in_nurb(c);
            }
        }

        Ok(modified_curve)
    }

    /// `n + 1` approximately evenly spaced points between the parameters `t0` and
    /// `t1` (chord lengths equal within `tol` relative) and their parameters
    /// (`EquallySpacedPoints`, `N_CrvEvalEvenSpacedPts`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB or when the NLib routine fails.
    pub fn equally_spaced_points(
        &self,
        t0: f64,
        t1: f64,
        n: usize,
        tol: f64,
    ) -> Result<(Vec<Point3>, Vec<f64>)> {
        let cur = nurb_of(self, OP_EVEN)?;
        let res = cur
            .eval_even_spaced_pts(t0, t1, n, tol)
            .map_err(SolidError::nl_ser(OP_EVEN))?;
        Ok((res.points, res.params))
    }

    /// Inserts the knots `new_knots` (non-decreasing, interior; an existing value
    /// raises that knot's multiplicity) (`InsertKnots`, `N_CrvRefine`); the NURB
    /// becomes a compact copy (`SetFromGwNurb`). Nothing happens for no knots.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB or when the refinement fails.
    pub fn insert_knots(&mut self, new_knots: &[f64]) -> Result<()> {
        if new_knots.is_empty() {
            return Ok(());
        }
        let cur = nurb_of(self, OP_INS)?;
        let newcur = cur.refine(new_knots).map_err(SolidError::nl_ser(OP_INS))?;
        self.set_from_gw_nurb(Some(&newcur))
    }

    /// A 3D curve of degree `degree` interpolating `points` (`InterpolatePoints`):
    /// closed (with the start / end tangent, if any, as the derivative at the seam,
    /// else C1 at the seam; `params` ignored), open through the parameters `params`
    /// with optional end tangent directions, open with optional end derivatives and
    /// parameters by `par`, or plain interpolation.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the fit fails; `InvalidInput` when `params` does not hold one
    /// parameter per point.
    pub fn interpolate_points(
        points: &[Point3],
        params: Option<&[f64]>,
        degree: usize,
        start_tangent: Option<Vec3>,
        end_tangent: Option<Vec3>,
        closed: bool,
        par: ParamMethod,
    ) -> Result<Self> {
        const OP: &str = OP_INTERP;
        let p = degree;
        let mut ts = Vec3::new(0.0, 0.0, 0.0);
        let mut te = Vec3::new(0.0, 0.0, 0.0);
        if let Some(t) = start_tangent {
            ts = t;
        }
        if let Some(t) = end_tangent {
            te = t;
        }

        let cur = if closed {
            // Closed curve, input parametrization (if given) is ignored.
            let mut d: Option<[Vec3; 1]> = None;
            if start_tangent.is_some() {
                d = Some([ts]);
            }
            if end_tangent.is_some() {
                d = Some([te]);
            }

            // k still has to be 1 regardless if pD is NULL or not
            let k = 1;
            approx_closed_conditions(points, p, d.as_ref().map(|d| &d[..]), k, par)
                .map_err(SolidError::nl_ser(OP))?
        } else if let Some(params) = params {
            // Open and Parameterization is given
            if params.len() != points.len() {
                return Err(invalid(OP, "one parameter per point needed"));
            }
            let mut knots: Option<Vec<f64>> = None;
            let tangents = match (start_tangent.is_some(), end_tangent.is_some()) {
                (true, true) => EndTangents::Both(&mut ts, &mut te),
                (true, false) => EndTangents::Start(&mut ts),
                (false, true) => EndTangents::End(&mut te),
                (false, false) => EndTangents::None,
            };
            interp_knots_and_tangents(
                points,
                Some(params),
                tangents,
                VectorKind::Tangent,
                &mut knots,
                1.0,
                p,
            )
            .map_err(SolidError::nl_ser(OP))?
        } else if start_tangent.is_some() || end_tangent.is_some() {
            // Open, no given Parameterization, and end derivs.
            let ds: Vec<Vec3> = if start_tangent.is_some() {
                vec![ts]
            } else {
                Vec::new()
            };
            let de: Vec<Vec3> = if end_tangent.is_some() {
                vec![te]
            } else {
                Vec::new()
            };
            interp_high_derivs(points, p, &ds, &de, par).map_err(SolidError::nl_ser(OP))?
        } else {
            // Open and no given Parameterization
            interp(points, p, par).map_err(SolidError::nl_ser(OP))?
        };

        Ok(Self::from_gw_nurb(3, &cur))
    }

    /// Makes `curves` compatible: the same degree and knot vector (knots closer than
    /// `knot_tol` merged), by degree elevation and knot insertion
    /// (`MakeCurvesCompatible`, `N_CrvsMakeCompatibleKnotTol`); the knots of every
    /// curve are fixed up with `knot_tol` before and after.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a curve without NURB or when NLib fails (the knot fix-ups made
    /// before stay); `InvalidInput` for an empty set.
    pub fn make_curves_compatible(curves: &mut [Self], knot_tol: f64) -> Result<()> {
        if curves.is_empty() {
            return Err(invalid(OP_COMPAT, "no curves"));
        }
        // First create an array of NLib curves from the input curves.
        let mut nl_curves: Vec<Curve> = Vec::with_capacity(curves.len());
        for curve in curves.iter_mut() {
            if curve.nurb.is_none() {
                curve.make_nurb().map_err(SolidError::nl_ser(OP_COMPAT))?;
            }

            // fix up knot vector before call to N_CrvsMakeCompatibleKnotTol
            curve.fixup_knot_vector(knot_tol)?;
            nl_curves.push(nlib_curve(curve, OP_COMPAT)?);
        }

        // Invoke NLib to make compatible
        make_compatible_knot_tol(&mut nl_curves, knot_tol)
            .map_err(SolidError::nl_ser(OP_COMPAT))?;

        // Put the modified NLib curves back into our curves,
        // and do the (changed) knot vectors again.
        for (curve, nl) in curves.iter_mut().zip(nl_curves.iter()) {
            curve.set_from_gw_nurb(Some(nl))?;
            curve.fixup_knot_vector(knot_tol)?;
        }
        Ok(())
    }

    /// Deforms the curve so that its ends move to `tgt_start` / `tgt_end` and (for
    /// [`InValueType::Specified`]) its end tangents point along `start_tan` /
    /// `end_tan`, keeping its general shape (`DeformToEndPointTargets`): rigid
    /// motions of the end regions (15% of the parameter range) blended by a
    /// displacement curve over the middle; knots are added near the ends as needed.
    /// Nothing happens when nothing is asked.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for [`InValueType::Specified`] without a tangent or without a NURB;
    /// `InvalidInput` where upstream would index outside its arrays (e.g. a curve over
    /// a zero-length knot interval, whose degree elevation fails; upstream ignores
    /// that status, see `plan/findings.md`).
    pub fn deform_to_end_point_targets(
        &mut self,
        tgt_start: Option<Point3>,
        tgt_end: Option<Point3>,
        start_tan: Option<Vec3>,
        start_tan_type: InValueType,
        end_tan: Option<Vec3>,
        end_tan_type: InValueType,
    ) -> Result<()> {
        const OP: &str = OP_DEFORM;

        // no work - no requested changes
        if tgt_start.is_none()
            && tgt_end.is_none()
            && (start_tan_type == InValueType::Same || start_tan_type == InValueType::Unconstrained)
            && (end_tan_type == InValueType::Same || end_tan_type == InValueType::Unconstrained)
        {
            return Ok(());
        }

        // Algorithm fixed parameters
        let break_param_percent = 0.15;
        let near_param_percent = 1.0e-3;

        // check input
        if !(start_tan_type != InValueType::Specified || start_tan.is_some()) {
            return Err(SolidError::sm_err(OP));
        }
        if !(end_tan_type != InValueType::Specified || end_tan.is_some()) {
            return Err(SolidError::sm_err(OP));
        }

        // increment CurrCrv degree when needed (the status is ignored upstream, e.g.
        // for a curve over a zero-length knot interval)
        let degree = nurb_of(self, OP)?.degree();
        let elevated = if degree < 3 {
            degree_elevate(self, 3)
        } else if degree % 2 == 0 {
            degree_elevate(self, degree + 1)
        } else {
            Ok(())
        };
        if let Err(e @ SolidError::Unported { .. }) = elevated {
            return Err(e);
        }

        // Build new curve - a copy of the current one
        let mut curr = nurb_of(self, OP)?.clone();

        let mut curr_reg1_cnt = 0usize;
        let mut curr_reg2_cnt = 0usize;
        let mut curr_reg3_cnt = 0usize;
        let p = curr.degree();
        let m = curr.m();
        let knts = curr.knots();
        let knt_min = knot_at(knts, p, OP)?;
        let knt_max = knot_at(
            knts,
            m.checked_sub(p).ok_or_else(|| invalid(OP, "short knots"))?,
            OP,
        )?;
        let knt_ivl = knt_max - knt_min;

        // DispCrv inside Knot locals
        let break1 = knt_min + break_param_percent * knt_ivl;
        let break2 = knt_max - break_param_percent * knt_ivl;
        let near_param_dist = min2(near_param_percent * (1.0 + knt_ivl), knt_ivl * 0.35);
        let mut new_knt_index: [Option<usize>; DISP_INSIDE_KNT_CNT] = [None; DISP_INSIDE_KNT_CNT];
        let mut new_knts: [f64; DISP_INSIDE_KNT_CNT] = [
            knt_min + break_param_percent / 2.0 * knt_ivl,
            knt_min + break_param_percent * knt_ivl,
            knt_max - break_param_percent * knt_ivl,
            knt_max - break_param_percent / 2.0 * knt_ivl,
        ];

        // for every interior CurrCrv Knot - count knots in each of 3 regions
        {
            for ii in p + 1..m.saturating_sub(p) {
                let k = knot_at(knts, ii, OP)?;
                if k <= break1 + near_param_dist {
                    curr_reg1_cnt += 1;
                } else if k < break2 - near_param_dist {
                    curr_reg2_cnt += 1;
                } else {
                    curr_reg3_cnt += 1;
                }

                // snap NewKnts to existing knots when near enough - remember the snap
                for (idx, knt) in new_knt_index.iter_mut().zip(new_knts.iter_mut()) {
                    if idx.is_none() && are_same_to_tol(k, *knt, near_param_dist) {
                        *knt = k;
                        *idx = Some(ii);
                        break;
                    }
                }
            }
        }

        // add knots to CurrCrv as needed (hard coded for adding 2 knots in Reg1 and Reg3)
        let mut ii = 0usize;
        while ii < DISP_INSIDE_KNT_CNT {
            if vec_at(&new_knt_index, ii, OP)?.is_none()
                && ((ii < DISP_INSIDE_KNT_CNT / 2 && curr_reg1_cnt < 2)
                    || (ii >= DISP_INSIDE_KNT_CNT / 2 && curr_reg3_cnt < 2))
            {
                let knts = curr.knots();
                let m = curr.m();
                // pick the NewKnot furthest away from the one existing knot in Reg1
                if ii == 0 && curr_reg1_cnt == 1 {
                    let first_inner = knot_at(knts, p + 1, OP)?;
                    if (new_knts[1] - first_inner).abs() > (new_knts[0] - first_inner).abs() {
                        ii += 1;
                    }
                }
                // pick the NewKnot furthest away from the one existing knot in Reg3
                else if ii == 2 && curr_reg3_cnt == 1 {
                    let last_inner = knot_at(
                        knts,
                        m.checked_sub(p + 1)
                            .ok_or_else(|| invalid(OP, "short knots"))?,
                        OP,
                    )?;
                    if (new_knts[2] - last_inner).abs() < (new_knts[3] - last_inner).abs() {
                        ii += 1;
                    }
                }

                // insert a DispCrv inside knot (in place; the status is ignored)
                let knot = vec_at(&new_knts, ii, OP)?;
                if let Ok(c) = curr.insert_knot(knot, 1) {
                    curr = c;
                }

                // keep count
                if knot <= break1 + near_param_dist / 2.0 {
                    curr_reg1_cnt += 1;
                } else {
                    curr_reg3_cnt += 1;
                }
            }
            ii += 1;
        }

        // refresh CurrCrv locals
        let curr_cpt_index = curr.n();
        let p = curr.degree();

        // build DispCrv - with all the same Reg1 and Reg3 nCurrCrv knots
        let disp_degree = p;
        let disp_knot_index = curr_reg1_cnt + curr_reg3_cnt + 2 * p + 1;
        let disp_cpt_index = disp_knot_index - disp_degree - 1;
        let mut disp_knts = vec![0.0; disp_knot_index + 1];
        let mut disp_cpts = vec![CPoint::default(); disp_cpt_index + 1];
        {
            let curr_knts = curr.knots();
            // copy Reg1 knot values into nDispCrv_Knts array
            for ii in 0..=curr_reg1_cnt + p {
                *disp_knts
                    .get_mut(ii)
                    .ok_or_else(|| invalid(OP, "knot index out of range"))? =
                    knot_at(curr_knts, ii, OP)?;
            }
            // copy Reg3 knot values into nDispCrv_Knts array
            let i0_start = curr_reg1_cnt + p + 1;
            for i0 in i0_start..=disp_knot_index {
                // i1 = CurrCrv index
                let i1 = i0 + curr_reg2_cnt;
                *disp_knts
                    .get_mut(i0)
                    .ok_or_else(|| invalid(OP, "knot index out of range"))? =
                    knot_at(curr_knts, i1, OP)?;
            }
        }

        // 1. Build StartTransform = disp + rotation to set CurveStart position and tangent
        let curr_cpts = curr.cpts();
        let start_from_origin = cpt_at(curr_cpts, 0, OP)?.to_euclid();
        let start_to_origin = tgt_start.unwrap_or(start_from_origin);
        let (mut start_from_x, mut start_to_x) =
            (Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let (mut start_from_y, mut start_to_y) =
            (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
        let (mut start_from_z, mut start_to_z) =
            (Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0));

        // when asked to manage start tangent values
        if start_tan_type == InValueType::Specified {
            let tgt = start_tan.ok_or_else(|| SolidError::sm_err(OP))?;
            let pt0 = cpt_at(curr_cpts, 0, OP)?.drop_w();
            let pt1 = cpt_at(curr_cpts, 1, OP)?.drop_w();
            let start_tan_cur = Vec3::new(pt1.x - pt0.x, pt1.y - pt0.y, pt1.z - pt0.z);
            make_unit_ortho_into(
                start_tan_cur,
                Some(tgt),
                &mut start_from_x,
                &mut start_from_y,
                &mut start_from_z,
            );
            make_unit_ortho_into(
                tgt,
                Some(start_tan_cur),
                &mut start_to_x,
                &mut start_to_y,
                &mut start_to_z,
            );
            start_to_y = -start_to_y;
        }

        // start Displacement transform
        let disp_start_transform = Axis2Placement::from_to(
            &Axis2Placement::new(start_from_origin, start_from_x, start_from_y),
            &Axis2Placement::new(start_to_origin, start_to_x, start_to_y),
        );

        // 2. Build EndTransform
        let end_from_origin = cpt_at(curr_cpts, curr_cpt_index, OP)?.to_euclid();
        let end_to_origin = tgt_end.unwrap_or(end_from_origin);
        let (mut end_from_x, mut end_to_x) = (Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let (mut end_from_y, mut end_to_y) = (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
        let (mut end_from_z, mut end_to_z) = (Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0));

        // when asked to manage end tangent values
        if end_tan_type == InValueType::Specified {
            let tgt = end_tan.ok_or_else(|| SolidError::sm_err(OP))?;
            let pt_n = cpt_at(curr_cpts, curr_cpt_index, OP)?.drop_w();
            let pt_n1 = cpt_at(
                curr_cpts,
                curr_cpt_index
                    .checked_sub(1)
                    .ok_or_else(|| invalid(OP, "control point index out of range"))?,
                OP,
            )?
            .drop_w();
            let end_tan_cur = Vec3::new(pt_n.x - pt_n1.x, pt_n.y - pt_n1.y, pt_n.z - pt_n1.z);
            make_unit_ortho_into(
                end_tan_cur,
                Some(tgt),
                &mut end_from_x,
                &mut end_from_y,
                &mut end_from_z,
            );
            make_unit_ortho_into(
                tgt,
                Some(end_tan_cur),
                &mut end_to_x,
                &mut end_to_y,
                &mut end_to_z,
            );
            end_to_y = -end_to_y;
        }

        // end Displacement transform
        let disp_end_transform = Axis2Placement::from_to(
            &Axis2Placement::new(end_from_origin, end_from_x, end_from_y),
            &Axis2Placement::new(end_to_origin, end_to_x, end_to_y),
        );

        // 3./4. Set all DispCrv Reg1 / Reg3 CPts from displacements
        let reg1_cpt_cnt = curr_reg1_cnt + disp_degree.div_ceil(2);
        let reg2_cpt_cnt = curr_cpt_index
            .checked_sub(disp_cpt_index)
            .ok_or_else(|| invalid(OP, "displacement curve larger than the curve"))?;
        let reg3_cpt_cnt = curr_reg3_cnt + disp_degree.div_ceil(2);

        // for every Reg1 Cpt
        for ii in 0..reg1_cpt_cnt {
            let in_point = cpt_at(curr_cpts, ii, OP)?.to_euclid();
            let out_point = disp_start_transform.transform_point(in_point);
            *cpt_mut(&mut disp_cpts, ii, OP)? = CPoint::new(
                out_point.x - in_point.x,
                out_point.y - in_point.y,
                out_point.z - in_point.z,
                NOW,
            );
        }

        // for every Reg3 Cpt
        for ii in 0..reg3_cpt_cnt {
            let i0 = reg1_cpt_cnt + ii;
            let i1 = reg1_cpt_cnt + reg2_cpt_cnt + ii;
            let in_point = cpt_at(curr_cpts, i1, OP)?.to_euclid();
            let out_point = disp_end_transform.transform_point(in_point);
            *cpt_mut(&mut disp_cpts, i0, OP)? = CPoint::new(
                out_point.x - in_point.x,
                out_point.y - in_point.y,
                out_point.z - in_point.z,
                NOW,
            );
        }

        let mut disp = Curve::new(disp_cpts, disp_degree, disp_knts)?;

        // KnotVector of CurrCrv Reg2 knots
        let mut reg2_knt = Vec::with_capacity(curr_reg2_cnt);
        for ii in 0..curr_reg2_cnt {
            reg2_knt.push(knot_at(curr.knots(), curr_reg1_cnt + p + 1 + ii, OP)?);
        }

        // insert CurrCrv Reg2 knots into DispCrv (in place; the status is ignored)
        if let Ok(c) = disp.refine(&reg2_knt) {
            disp = c;
        }

        // Next: pCurrCrv = pCurrCrv + pDispCrv - Add CPt positions
        let disp_cpts = disp.cpts();
        for ii in 0..=curr_cpt_index {
            let cart_disp = cpt_at(disp_cpts, ii, OP)?.drop_w();
            let slot = cpt_mut(curr.cpts_mut(), ii, OP)?;
            let curr_pt = slot.to_euclid();
            let new_pt = Vec3::new(
                cart_disp.x + curr_pt.x,
                cart_disp.y + curr_pt.y,
                cart_disp.z + curr_pt.z,
            );
            *slot = CPoint::weighted(new_pt, slot.w);
        }

        // copy into a compact block and swap it in
        self.swap_in_nurb(&curr);
        Ok(())
    }

    /// Reparametrises the curve by arc length at its knots of multiplicity at least
    /// the degree (relative tolerance `1e-3`; the shape does not change)
    /// (`ReparametrizeWithArcLength`, `N_SrfReparamMultKnots`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB or when the arc length fails.
    pub fn reparametrize_with_arc_length(&mut self) -> Result<()> {
        let relative_tol = 1.0e-3;
        let cur = nurb_of(self, OP_REPARAM)?;
        let cur_new = cur
            .reparam_mult_knots(relative_tol)
            .map_err(SolidError::nl_ser(OP_REPARAM))?;
        self.swap_in_nurb(&cur_new);
        Ok(())
    }

    /// Multiplies every control point, weight included, by `scale_factor` (not a
    /// geometric scaling for rational curves) (`Scale(double)`, `N_CrvScaleCPts`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn scale(&mut self, scale_factor: f64) -> Result<()> {
        self.nurb_mut(OP_SCALE)?.scale_cpts(scale_factor);
        Ok(())
    }

    /// Rescales the knot vector to `[min, max]` without changing the shape
    /// (`ScaleKnotVector`, `N_CrvReparam`; its status is ignored).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn scale_knot_vector(&mut self, interval_min: f64, interval_max: f64) -> Result<()> {
        let cur = self.nurb_mut(OP_SCALE_KNOTS)?;
        let _ = cur.reparam(interval_min, interval_max);
        Ok(())
    }

    /// Makes `curves` compatible with knots within `tol` counted as identical
    /// (`SyncronizeKnotsOfCurves`, `N_CrvsMakeCompatibleAdjKnots`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a curve without NURB or when NLib fails; `InvalidInput` for an
    /// empty set.
    pub fn synchronize_knots_of_curves(curves: &mut [Self], tol: f64) -> Result<()> {
        if curves.is_empty() {
            return Err(invalid(OP_SYNC, "no curves"));
        }
        let mut gw_curves: Vec<Curve> = Vec::with_capacity(curves.len());
        for curve in curves.iter_mut() {
            if curve.nurb.is_none() {
                curve.make_nurb().map_err(SolidError::nl_ser(OP_SYNC))?;
            }
            gw_curves.push(nlib_curve(curve, OP_SYNC)?);
        }

        // Syncronize the curves with knot vectors
        make_compatible_adj_knots(&mut gw_curves, tol).map_err(SolidError::nl_ser(OP_SYNC))?;

        for (curve, nl) in curves.iter_mut().zip(gw_curves.iter()) {
            curve.set_from_gw_nurb(Some(nl))?;
        }
        Ok(())
    }

    /// Trims the curve to `trim_interval` (`Trim`). Ends within `100` scaled zeros
    /// of the natural interval or of a knot snap to it, and `trim_interval` is
    /// updated to the snapped values; an interval containing the natural interval
    /// changes nothing. Degenerate end segments created by the trim (shorter than the
    /// zone tolerance mapped to parameter space) are removed. With
    /// [`crate::kernel::UpstreamFixes::bspline_trim_preserves_uv_spans`] enabled,
    /// that removal applies only to dimension-3 model curves, preserving UV spans
    /// exactly. With
    /// [`TrimMode::IntervalOnly`] only the interval is computed.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an end clearly outside the natural interval, without a NURB, or
    /// when the trim fails; `InvalidInput` for ends closer than `SM_EFF_ZERO`.
    pub fn trim(&mut self, k: &Kernel, trim_interval: &mut Extent1d, mode: TrimMode) -> Result<()> {
        self.trim_with_zone_tol(
            k,
            trim_interval,
            mode,
            zone_tol_3d(k, SmallTopoType::NormalSize),
        )
    }

    /// Trims with the caller-resolved `SmTol::GetZoneTol3d(this)` ownership context.
    /// The supplied 3D tolerance controls parameter-space end-span detection and
    /// degenerate-segment removal; snapping, modes and UV-span policy are unchanged.
    ///
    /// # Errors
    ///
    /// The same errors as [`Self::trim`].
    pub(crate) fn trim_with_zone_tol(
        &mut self,
        k: &Kernel,
        trim_interval: &mut Extent1d,
        mode: TrimMode,
        zone_tol_3d: f64,
    ) -> Result<()> {
        const OP: &str = OP_TRIM;

        // no work - the natural interval is contained by the new interval
        let nat_ivl = self.natural_interval();
        if nat_ivl.is_contained_by(trim_interval, EFF_ZERO) {
            return Ok(());
        }

        // locals
        let mut d_min = trim_interval.min();
        let mut d_max = trim_interval.max();
        let scaled_zero_min = 100.0 * d_min.scaled_zero();
        let scaled_zero_max = 100.0 * d_max.scaled_zero();
        let zone_tol_1d_min = self.map_tol_to_1d(zone_tol_3d, d_min);
        let zone_tol_1d_max = self.map_tol_to_1d(zone_tol_3d, d_max);
        let degree = self.degree().map_err(SolidError::nl_ser(OP))?;

        // when MinParam is not contained - snap within tight tolerance or quit
        if !nat_ivl.contains_value(d_min, 0.0) {
            if (nat_ivl.min() - d_min).abs() < scaled_zero_min {
                d_min = nat_ivl.clamp_value(d_min);
            } else {
                return Err(SolidError::sm_err(OP));
            }
        }

        // when MaxParam is not contained - snap within tight tolerance or quit
        if !nat_ivl.contains_value(d_max, 0.0) {
            if (nat_ivl.max() - d_max).abs() < scaled_zero_max {
                d_max = nat_ivl.clamp_value(d_max);
            } else {
                return Err(SolidError::sm_err(OP));
            }
        }

        // for every knot - snap interval min/max values within tolerance to existing knots
        let (knots, _) = self.knots(None).map_err(SolidError::nl_ser(OP))?;
        for &knot in &knots {
            // snap minParam to knots within tight tolerance
            if (knot - d_min).abs() < scaled_zero_min {
                d_min = knot;
            }

            // snap maxParam to knots within tight tolerance
            if (knot - d_max).abs() < scaled_zero_max {
                d_max = knot;
                break;
            }

            if knot > d_max + scaled_zero_max {
                break;
            }
        }

        // avoid making a degenerate interval due to snapping
        if (d_min - d_max).abs() < scaled_zero_min + scaled_zero_max {
            // Just have to default back to original trim interval
            d_min = trim_interval.min();
            d_max = trim_interval.max();
            if d_max - d_min < EFF_ZERO {
                // Error here because trim points are too close together
                return Err(invalid(OP, "trim points too close together"));
            }
        }

        // no work - both knots are within tight tol of trim boundary
        let first_knot = vec_at(&knots, 0, OP)?;
        let last_knot = knots
            .last()
            .copied()
            .ok_or_else(|| invalid(OP, "no knots"))?;
        if (d_min - first_knot).abs() < scaled_zero_min
            && (d_max - last_knot).abs() < scaled_zero_max
        {
            return Ok(());
        }

        // Reset interval if we happen to fall nearly on some knots (status ignored).
        let _ = trim_interval.set_min_max(d_min, d_max);

        // Trim when asked
        if mode != TrimMode::IntervalOnly {
            let this_cur = nurb_of(self, OP)?;
            if d_max <= d_min {
                // sm_CrvTrim's NL_INP_ERR flag is not NL_YES; upstream then uses its NULL result
                return Err(invalid(OP, "sm_CrvTrim with ur <= ul"));
            }
            let mut cur1 = crv_trim(this_cur, d_min, d_max).map_err(SolidError::nl_ser(OP))?;

            // watch out for new degenerate segments
            let p_knots = cur1.knots();
            let max_indx = cur1.m();
            let head = knot_at(p_knots, degree + 1, OP)? - knot_at(p_knots, 0, OP)?;
            let tail = knot_at(p_knots, max_indx, OP)?
                - knot_at(
                    p_knots,
                    max_indx
                        .checked_sub(degree + 1)
                        .ok_or_else(|| invalid(OP, "short knots"))?,
                    OP,
                )?;
            if (!k.fixes.bspline_trim_preserves_uv_spans || self.dim == 3)
                && ((head < zone_tol_1d_min && knots.len() > 2)
                    || (tail < zone_tol_1d_max && knots.len() > 2))
            {
                let removal = cur1
                    .remove_degen_segs(zone_tol_3d)
                    .map_err(SolidError::nl_ser(OP))?;

                // when segs removed - sCur2 is allocated and set, otherwise it's untouched
                if let Some(r) = removal {
                    cur1 = copy_nurb_curve(&r.curve);
                }
            }

            // save the new pCur as the BSplineCurve->m_pNurb curve
            self.nurb_is_borrowed = false;
            self.nurb = Some(cur1);
        }
        Ok(())
    }

    /// `SmTol::MapTo1d(tol, t, *this)`: `tol` mapped to parameter space with the first
    /// derivative at `t` (`Evaluate(t, 1, TRUE, .., TRUE)`; a failed evaluation leaves
    /// upstream's default-constructed derivative, [`Vec3::UNDEF`]).
    fn map_tol_to_1d(&self, tol: f64, t: f64) -> f64 {
        let deriv = self
            .evaluate(t, 1, true, true)
            .ok()
            .and_then(|v| v.get(1).copied())
            .unwrap_or(Vec3::UNDEF);
        map_to_1d(tol, deriv, &self.natural_interval())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::nurbs_crv::allocate_nurb_curve;

    fn cubic() -> BSplineCurve {
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 2.0, 0.0],
            [3.0, 3.0, 1.0],
            [5.0, 1.0, 0.0],
            [6.0, -1.0, 2.0],
            [8.0, 0.0, 0.0],
        ];
        let u = [0.0, 0.0, 0.0, 0.0, 0.3, 0.6, 1.0, 1.0, 1.0, 1.0];
        let mut c = allocate_nurb_curve(5, 3, 9).unwrap();
        for (d, s) in c.cpts_mut().iter_mut().zip(pts) {
            *d = CPoint::new(s[0], s[1], s[2], NOW);
        }
        c.knots_mut().copy_from_slice(&u);
        BSplineCurve::from_nurb(c, 3, false)
    }

    #[test]
    fn spacing_drops_weight() {
        let a = CPoint::new(3.0, 4.0, 0.0, 2.0);
        let b = CPoint::new(0.0, 0.0, 0.0, 1.0);
        assert_eq!(control_point_spacing(a, b), 5.0);
    }

    #[test]
    fn arc_and_helix() {
        let f = Axis2Placement::default();
        let c = BSplineCurve::approximate_arc(3, None, &f, 2.0, 0.0, 90.0, 0.01).unwrap();
        let e = c.ends().unwrap();
        assert!((e.start - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-12);
        assert!((e.end - Vec3::new(0.0, 2.0, 0.0)).length() < 1e-9);
        assert!(BSplineCurve::approximate_arc(3, None, &f, 0.0, 0.0, 90.0, 0.01).is_err());
        let h = BSplineCurve::create_helix_segment(&HelixSpec {
            frame: f,
            height: 20.0,
            radius_start: 0.25,
            radius_end: 0.25,
            num_turns: 0.25,
            right_handed: true,
            approx_tol: 1e-7,
        })
        .unwrap();
        assert_eq!(h.form(), BSplineCurveForm::HelicalArc);
        assert!(BSplineCurve::create_helix_segment(&HelixSpec {
            frame: f,
            height: -1.0,
            radius_start: 0.25,
            radius_end: 0.25,
            num_turns: 1.0,
            right_handed: true,
            approx_tol: 1e-7,
        })
        .is_err());
    }

    #[test]
    fn interpolate_and_extend() {
        let pts = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(4.0, 0.0, 0.0),
            Vec3::new(4.0, 4.0, 0.0),
            Vec3::new(8.0, 4.0, 0.0),
        ];
        let c = BSplineCurve::interpolate_points(
            &pts,
            None,
            3,
            None,
            None,
            false,
            ParamMethod::ChordLength,
        )
        .unwrap();
        assert_eq!(c.degree().unwrap(), 3);
        let ext = c
            .create_extended_curve(1.5, ContinuityType::CInfinity, false)
            .unwrap()
            .unwrap();
        assert!(ext.natural_interval().max() > 1.0);
        assert!(c
            .create_extended_curve(0.5, ContinuityType::CInfinity, true)
            .unwrap()
            .is_none());
        assert!(c
            .create_extended_curve(1.5, ContinuityType::C0, true)
            .is_err());
        let line = BSplineCurve::create_interpolating_curve(
            ParamMethod::ChordLength,
            3,
            1,
            &pts[..2],
            &[],
            None,
            true,
        )
        .unwrap();
        assert_eq!(line.degree().unwrap(), 1);
    }

    #[test]
    fn knots_edit_and_trim() {
        let mut c = cubic();
        c.insert_one_knot(0.45, 2).unwrap();
        assert_eq!(c.number_natural_knots().unwrap(), 12);
        c.insert_knots(&[0.1, 0.2]).unwrap();
        assert_eq!(c.number_natural_knots().unwrap(), 14);
        let mut d = cubic();
        d.insert_one_knot(0.45, 1).unwrap();
        d.dampen_knot_spacing(2.0, 2).unwrap();
        assert_eq!(
            d.knots(None).unwrap().0,
            vec![0.0, 0.3, 0.45, 0.6, 0.8, 1.0]
        );
        assert_eq!(d.number_natural_knots().unwrap(), 13);
        let mut e = cubic();
        assert!(matches!(
            e.dampen_knot_spacing(0.5, 1),
            Err(SolidError::NotConverged { .. })
        ));
        let k = Kernel::default();
        let mut t = cubic();
        let mut ivl = Extent1d::from_min_max(0.2, 0.8);
        t.trim(&k, &mut ivl, TrimMode::Notify).unwrap();
        assert_eq!(t.natural_interval(), Extent1d::from_min_max(0.2, 0.8));
        let mut ivl = Extent1d::from_min_max(-5.0, 0.5);
        assert!(cubic().trim(&k, &mut ivl, TrimMode::Notify).is_err());
    }

    // Two rational quarter circles. A trim just before or after the middle knot
    // leaves an end span shorter than the 3D zone tolerance in UV units, although
    // lifting it onto a plane scaled by 38 makes it longer than that tolerance.
    fn short_span_arc(dim: u64) -> BSplineCurve {
        let r = 0.14;
        let w = core::f64::consts::FRAC_1_SQRT_2;
        let points = [
            CPoint::new(r, 0.0, 0.0, NOW),
            CPoint::new(r * w, r * w, 0.0, w),
            CPoint::new(0.0, r, 0.0, NOW),
            CPoint::new(-r * w, r * w, 0.0, w),
            CPoint::new(-r, 0.0, 0.0, NOW),
        ];
        let mut nurb = allocate_nurb_curve(4, 2, 7).unwrap();
        nurb.cpts_mut().copy_from_slice(&points);
        nurb.knots_mut()
            .copy_from_slice(&[0.0, 0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0]);
        BSplineCurve::from_nurb(nurb, dim, false)
    }

    fn lifted_trim_gap(original: &BSplineCurve, trimmed: &BSplineCurve, ivl: Extent1d) -> f64 {
        (0..=64)
            .map(|i| {
                let t = ivl.evaluate(f64::from(i) / 64.0);
                let delta =
                    trimmed.evaluate_point(t).unwrap() - original.evaluate_point(t).unwrap();
                let gap = (38.0 * delta).length();
                assert!(gap.is_finite());
                gap
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn trim_uses_caller_zone_tolerance_for_short_end_span() {
        let k = Kernel::default();
        let original = short_span_arc(3);
        let interval = Extent1d::from_min_max(0.5 - 3.5e-6, 0.9);
        let mut standalone = original.clone();
        let mut standalone_interval = interval;
        standalone
            .trim(&k, &mut standalone_interval, TrimMode::Silent)
            .unwrap();

        let mut owned = original.clone();
        let mut owned_interval = interval;
        owned
            .trim_with_zone_tol(&k, &mut owned_interval, TrimMode::Silent, 1e-9)
            .unwrap();
        assert_eq!(owned_interval, interval);
        assert_eq!(owned.natural_interval(), interval);
        assert_eq!(
            owned.knots(None).unwrap().0,
            vec![interval.min(), 0.5, interval.max()]
        );
        assert_ne!(owned.nurb, standalone.nurb);
        assert!(lifted_trim_gap(&original, &owned, interval) < 1e-12);
        assert!(lifted_trim_gap(&original, &standalone, interval) > k.tolerances.zone_tol_3d);
    }

    #[test]
    fn uv_trim_keeps_short_head_and_tail_spans() {
        let mut k = Kernel::default();
        k.fixes.bspline_trim_preserves_uv_spans = true;
        for mode in [TrimMode::Notify, TrimMode::Silent] {
            for (min, max) in [(0.5 - 3.5e-6, 0.9), (0.1, 0.5 + 3.5e-6)] {
                let original = short_span_arc(2);
                let mut trimmed = original.clone();
                let mut ivl = Extent1d::from_min_max(min, max);
                trimmed.trim(&k, &mut ivl, mode).unwrap();
                assert_eq!(ivl, Extent1d::from_min_max(min, max));
                assert_eq!(trimmed.natural_interval(), ivl);
                let gap = lifted_trim_gap(&original, &trimmed, ivl);
                assert!(gap < 1e-12, "{mode:?}, {ivl:?}: lifted gap {gap}");
                // The interior knot and both pieces must survive the trim.
                assert_eq!(trimmed.knots(None).unwrap().0, vec![min, 0.5, max]);
            }
        }
    }

    #[test]
    fn uv_trim_fix_preserves_default_and_model_space_cleanup() {
        let default = Kernel::default();
        assert!(!default.fixes.bspline_trim_preserves_uv_spans);
        let mut fixed = Kernel::default();
        fixed.fixes.bspline_trim_preserves_uv_spans = true;
        for dim in [2, 3] {
            for (min, max) in [(0.5 - 3.5e-6, 0.9), (0.1, 0.5 + 3.5e-6)] {
                let original = short_span_arc(dim);
                let ivl = Extent1d::from_min_max(min, max);
                let mut old = original.clone();
                let mut old_ivl = ivl;
                old.trim(&default, &mut old_ivl, TrimMode::Notify).unwrap();
                // This is the defect reproduced by the pinned reference: the
                // removed UV piece becomes a gap larger than the edge tolerance.
                let gap = lifted_trim_gap(&original, &old, ivl);
                assert!(gap > default.tolerances.zone_tol_3d, "gap {gap}");
                if dim == 3 {
                    let mut model = original.clone();
                    let mut model_ivl = ivl;
                    model
                        .trim(&fixed, &mut model_ivl, TrimMode::Notify)
                        .unwrap();
                    assert_eq!(old.nurb, model.nurb);
                }
            }
        }
    }

    #[test]
    fn uv_trim_fix_preserves_interval_only_and_curve_dispatch() {
        let mut k = Kernel::default();
        k.fixes.bspline_trim_preserves_uv_spans = true;
        let original = short_span_arc(2);
        let mut untouched = original.clone();
        let mut ivl = Extent1d::from_min_max(0.5 - 3.5e-6, 0.9);
        untouched
            .trim(&k, &mut ivl, TrimMode::IntervalOnly)
            .unwrap();
        assert_eq!(untouched.nurb, original.nurb);

        let mut curve = crate::curve::curve::Curve::from(original.clone());
        curve.trim(&k, &mut ivl, TrimMode::Silent).unwrap();
        for i in 0..=64 {
            let t = ivl.evaluate(f64::from(i) / 64.0);
            let delta = curve.evaluate_point(&k, t).unwrap() - original.evaluate_point(t).unwrap();
            assert!((38.0 * delta).length() < 1e-12);
        }
    }

    #[test]
    fn deform_and_backedges() {
        let mut c = cubic();
        c.deform_to_end_point_targets(
            Some(Vec3::new(0.0, 0.5, 0.0)),
            None,
            None,
            InValueType::Same,
            None,
            InValueType::Same,
        )
        .unwrap();
        let s = c.ends().unwrap().start;
        assert!((s - Vec3::new(0.0, 0.5, 0.0)).length() < 1e-12);
        let mut q = cubic();
        q.degree_reduction(10.0, false).unwrap();
        assert_eq!(q.degree().unwrap(), 2);
        assert!(matches!(
            q.deform_to_end_point_targets(
                None,
                None,
                None,
                InValueType::Specified,
                None,
                InValueType::Same
            ),
            Err(SolidError::Upstream { .. })
        ));
        // degree 2: elevated to 3 first (DegreeElevate)
        q.deform_to_end_point_targets(
            Some(Vec3::new(0.0, 0.5, 0.0)),
            None,
            None,
            InValueType::Same,
            None,
            InValueType::Same,
        )
        .unwrap();
        let s = q.ends().unwrap().start;
        assert!((s - Vec3::new(0.0, 0.5, 0.0)).length() < 1e-12);
        let mut knot = 0.5;
        let r = cubic()
            .set_intersection_curve_through_point::<crate::nurbs::b_spline_surface::BSplineSurface>(
                &Kernel::default(),
                &mut knot,
                Vec3::new(4.0, 2.0, 0.0),
                None,
                &mut [],
                &[],
                &[],
                0.1,
            );
        // not an intersector-form curve
        assert!(matches!(r, Err(SolidError::Upstream { .. })));
    }

    #[test]
    fn compat_scale_and_points() {
        let mut v = vec![
            cubic(),
            BSplineCurve::approximate_arc(
                3,
                None,
                &Axis2Placement::default(),
                1.0,
                0.0,
                180.0,
                0.01,
            )
            .unwrap(),
        ];
        BSplineCurve::make_curves_compatible(&mut v, 1e-6).unwrap();
        assert_eq!(
            v[0].number_natural_knots().unwrap(),
            v[1].number_natural_knots().unwrap()
        );
        let mut w = vec![cubic(), cubic()];
        BSplineCurve::synchronize_knots_of_curves(&mut w, 1e-6).unwrap();
        let mut c = cubic();
        c.scale(2.0).unwrap();
        c.scale_knot_vector(5.0, 7.0).unwrap();
        assert_eq!(c.natural_interval(), Extent1d::from_min_max(5.0, 7.0));
        let (p, t) = cubic().equally_spaced_points(0.0, 1.0, 5, 1e-3).unwrap();
        assert_eq!((p.len(), t.len()), (6, 6));
        let mut r = cubic();
        r.reparametrize_with_arc_length().unwrap();
        assert!(BSplineCurve::new(3).scale(1.0).is_err());
    }

    #[test]
    fn dampen_absurd_multiplicity_is_an_error() {
        // fuzz regressions (nurbs.b_spline_curve_n_lib fuzz00889, fuzz01344): a huge
        // multiplicity inserted knots one at a time without end
        let mut c = cubic();
        let before = c.knots(None).unwrap();
        let r = c.dampen_knot_spacing(0.5, usize::MAX);
        assert!(r.is_err());
        assert_eq!(c.knots(None).unwrap(), before);
    }
}
