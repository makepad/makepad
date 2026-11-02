//! `SmBSplineSurface` with NLib, part 1: copies of NLib curves and surfaces, offset
//! approximation, the non-rational curve copy, bicubic and bilinear Coons patches,
//! surface extension by distance, joining, n-sided patches, ruled, sweep and swung
//! surfaces, the Greville-style skinning knots, evaluation on the extended surface,
//! reparametrisation, reversal, trim boundary checks, boundary span expansion, the
//! zero-derivative fix-up and the plain evaluators.
//!
//! Upstream: `source/SMLib/src/SmBSplineSurfaceNLib.cpp`, `source/SMLib/inc/SmBSplineSurface.h`.
//!
//! | Rust                                                  | Upstream                                         |
//! |-------------------------------------------------------|--------------------------------------------------|
//! | [`build_nlib_curve`]                                  | `sm_BuildNlibCurve` (static)                     |
//! | [`create_nlib_curve`]                                 | `sm_CreateNlibCurve` (static)                    |
//! | [`create_nlib_surface`]                               | `sm_CreateNlibSurface` (static)                  |
//! | [`BSplineSurface::approximate_offset_surface`], [`BSplineSurface::approximate_offset_surface_with`], [`OffsetParamType`] | `ApproximateOffsetSurface` |
//! | [`create_non_rational_curve`]                         | `sm_CreateNonRationalCurve` (static)             |
//! | [`create_coons_srf`], [`CoonsWarning`]                | `sm_CreateCoonsSrf` (static)                     |
//! | [`BSplineSurface::create_coons_patch`]                | `CreateCoonsPatch`                               |
//! | [`BSplineSurface::create_extended_surface`]           | `CreateExtendedSurface` (distance from each side) |
//! | [`BSplineSurface::create_extended_surface_dir`]       | `CreateExtendedSurface` (with `SmSurfParamType`) |
//! | [`BSplineSurface::join_surface`]                      | `JoinSurface`                                    |
//! | [`BSplineSurface::create_n_sided_patch`]              | `CreateNSidedPatch`                              |
//! | [`BSplineSurface::create_ruled_surface`]              | `CreateRuledSurface`                             |
//! | [`make_knots`]                                        | `sm_MakeKnots` (static)                          |
//! | [`BSplineSurface::create_sweep_surface`]              | `CreateSweepSurface` (single section)            |
//! | [`BSplineSurface::create_sweep_surface_with_placements`] | `CreateSweepSurface` with the fork's `pOptPlacements` |
//! | [`BSplineSurface::create_swung_surface`]              | `CreateSwungSurface`                             |
//! | [`BSplineSurface::evaluate_point_on_extended_surface`] | `EvaluatePointOnExtendedSurface`                |
//! | [`BSplineSurface::reparameterize`]                    | `Reparameterize`                                 |
//! | [`BSplineSurface::reverse`]                           | `Reverse`                                        |
//! | [`check_trim_boundaries`]                             | `sm_CheckTrimBoundaries` (static)                |
//! | [`BSplineSurface::expand_boundary_span`]              | `ExpandBoundarySpan`                             |
//! | [`check_zero_surface_derivs`]                         | `sm_check_zero_surface_derivs` (static)          |
//! | [`BSplineSurface::evaluate_simple`], [`BSplineSurface::evaluate_simple_first_into`] | `EvaluateSimple` |
//! | [`BSplineSurface::evaluate_point`]                    | `EvaluatePoint`                                  |
//!
//! The other functions of the file (`CreateSkinnedSurface`, `CreateCornerBlend`,
//! `TrimWithDomain`, `Evaluate`, `DegreeElevate`, `DegreeReduction`, ...) belong to
//! other units.
//!
//! Deviations:
//! - `approximate_offset_surface_with` exposes the opt-in corrections of
//!   [`crate::nurbs::srf_offset::OffsetOptions`] for issue #471; the original entry
//!   point retains SMLib's behavior.
//! - **Memory and contexts.** `SmNLibStackHandler`, the `NL_STACKS` arguments and the
//!   `SmContext` arguments only manage memory and are dropped; new surfaces and curves
//!   are return values (upstream out-params), surface lists are `Vec`s.
//!   `Notify(SM_NO_PRE_EDIT / SM_NO_POST_EDIT)` only drops caches and notifies owners,
//!   which a surface here does not have; the calls are omitted (as in
//!   `nurbs::b_spline_surface::p1`).
//! - **Inputs changed in place.** Upstream hands the NURBs of its input curves and
//!   surfaces to NLib, which changes them: `CreateCoonsPatch` (curves made compatible,
//!   cross-boundary derivatives rescaled, rational curves reweighted),
//!   `CreateNSidedPatch` (see `N_FillNSidedHole`) and `JoinSurface` (both surfaces
//!   made compatible). The port takes those inputs mutably and stores the changed
//!   NURBs back. Where NLib reallocates such a NURB upstream from the local stack,
//!   the caller's object points at freed memory after the call; the port keeps the
//!   changed data instead.
//! - **Missing NURB.** `GetOrCreateGwNurbPointer` cannot make a NURB for a plain
//!   B-spline; where upstream checks the pointer with `NER` the port returns
//!   `SM_ERR_NULL_POINTER` (1006), where upstream dereferences it the port returns
//!   `SM_ERR`.
//! - **Errors.** `NL_SER` / `SER(SM_ERR)` become `SM_ERR` ([`SolidError::Upstream`]);
//!   [`SolidError::Unported`] passes through. `SM_ERR_INVALID_INPUT` checks are
//!   [`SolidError::InvalidInput`]; where upstream would read outside its arrays
//!   (`CreateCoonsPatch` without four boundary curves or with a derivative count
//!   other than 0 or 4, `CreateNSidedPatch` with fewer derivative entries than
//!   curves, `sm_MakeKnots` with `p == 0` or a too short array, a knot index past the
//!   end in `sm_CheckTrimBoundaries`) the port returns `InvalidInput` as well.
//! - **Enums and flags.** `ApproximateOffsetSurface`'s `int iParamType` is
//!   [`OffsetParamType`] (values above 3, which upstream maps to `NL_INHERITED` but
//!   still treats as "keep the domain", cannot be expressed). `JoinSurface` accepts
//!   [`SurfParamType::U`] and [`SurfParamType::V`] only (upstream computes the NLib
//!   flags arithmetically from the enum value); other values are `InvalidInput`.
//!   `sm_CheckTrimBoundaries`' negative "not set" indices are `None`.
//! - **`sm_CreateCoonsSrf`** returns its warning level (`lWarning`, only logged
//!   upstream) as [`CoonsWarning`]; `CreateCoonsPatch` ignores it as upstream.
//! - **`CreateNSidedPatch`** unitizes the caller's center normal in place, as upstream
//!   (`Unitize`'s status is ignored, so a zero vector becomes `(0, 0, 1)`).
//! - **`EvaluateSimple`** returns the derivative matrix `D[i][j]` (`i <= u`, `j <= v`
//!   derivatives) instead of writing the caller's array; entries upstream leaves
//!   untouched (the lower half with `bOnlyUpperHalf`) are zero.
//! - **`sm_check_zero_surface_derivs`** takes `Su` / `Sv` mutably and returns the
//!   flag; its unused point argument is dropped.
//! - **Back-edges.** `CreateExtendedSurface` calls `SmSurface::IsSingularity`
//!   (`surface.surface_p5`) and `SmSurface::IsClosed` (`surface::surface::p7`);
//!   `sm_CreateNonRationalCurve` drops its curve with the virtual `SmSurface::DropCurve`
//!   of its base surface, dispatched on the surface kind (the `SmPlane` override
//!   [`Plane::drop_curve`](crate::surface::plane::Plane::drop_curve) or the base body
//!   [`Surface::drop_curve_base`](crate::surface::surface::Surface::drop_curve_base)),
//!   taking the caller's kernel. The unported calls return [`SolidError::Unported`]
//!   until the units are ported. `sm_CreateNonRationalCurve` takes a
//!   [`BSplineCurve`] (upstream casts its `SmCurve` with `SM_CAST_PTR`, failing with
//!   `NER` for other curve types) and the caller's
//!   [`Surface`](crate::surface::surface::Surface) as base surface (with its kind and
//!   owner, as upstream's `SmSurface*`).
//! - **`CreateExtendedSurface`** halves the extension distance after each failed
//!   extension until it drops below `dDist / 100`, which never happens for a zero or
//!   NaN distance (upstream then loops forever); the port stops after
//!   [`MAX_EXTEND_HALVINGS`] halvings with [`SolidError::NotConverged`]. After a
//!   successful extension it doubles the distance until it reaches `dDist / 2`, which
//!   a negative distance never does (it doubles towards `-inf`, upstream HANG-16); the
//!   port stops after [`MAX_EXTEND_DOUBLINGS`] doublings with the same error.
//! - **`N_LinearMultiplyCrv`** returns nothing upstream; its port fails only for a
//!   degree-0 cross-boundary derivative (no Greville abscissae), which
//!   [`create_coons_srf`] passes on.
//! - Status values upstream ignores stay ignored: the reparametrisation of
//!   `ApproximateOffsetSurface` and `RebuildSTEPFromNURBParameters`, a no-op for a
//!   plain B-spline.

use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::tol::are_same;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::{BSplineSurface, BSplineSurfaceForm, SurfParamType};
use crate::nurbs::crv_adv::p1::CurveEnd;
use crate::nurbs::crv_basic::{curves_are_compatible, Curve};
use crate::nurbs::crv_fit::p1::ParamMethod;
use crate::nurbs::crv_offset::OffsetParam;
use crate::nurbs::crv_tool::p2::make_compatible as make_curves_compatible;
use crate::nurbs::geom::Rectangle;
use crate::nurbs::knots::{Side, MAX_DEGREE};
use crate::nurbs::math::{
    combine2_pts, power_basis_srf_eval_pts, srf_power_basis_eval_derivs, CPoint,
};
use crate::nurbs::srf_adv::p1::eval_pt_derivs_unbounded;
use crate::nurbs::srf_adv::p2::{extend_by_dist_in_place, SrfContinuity};
use crate::nurbs::srf_adv::p3::{
    create_swung_srf, create_trans_sweep_srf, extend_by_param_dist, ExtendContinuity,
};
use crate::nurbs::srf_adv::p4::create_coons_srf as bilinear_coons_srf;
use crate::nurbs::srf_adv::p6::{
    create_sweep_scale, create_sweep_scale_with_placements, fill_n_sided_hole_with,
    SweepScaleOptions,
};
use crate::nurbs::srf_basic::{size_arrays, Direction, Surface, SurfaceIndices};
use crate::nurbs::srf_common::ruled_srf;
use crate::nurbs::srf_tool::p3::{join, make_compatible as make_surfaces_compatible};
use crate::nurbs::srf_tool::p4::make_compatible_uv;

/// Largest number of distance halvings of one side of
/// [`BSplineSurface::create_extended_surface_dir`] (upstream has no bound and loops
/// forever for a zero or NaN distance; halving a positive distance reaches zero after
/// fewer than 1100 steps).
pub const MAX_EXTEND_HALVINGS: usize = 1100;

/// Largest number of distance doublings of one side of
/// [`BSplineSurface::create_extended_surface_dir`]. A positive distance is doubled back
/// at most once more than it was halved, so only a negative distance reaches this:
/// upstream doubles it away from the loop's exit (`dExtDist < dDist/2.0`) towards
/// `-inf` and loops forever while the extensions succeed (`plan/upstream-issues.md`
/// HANG-16).
pub const MAX_EXTEND_DOUBLINGS: usize = MAX_EXTEND_HALVINGS + 1;

const OP_BUILD_CURVE: &str = "nurbs::b_spline_surface_n_lib::p1::build_nlib_curve";
const OP_NLIB_CURVE: &str = "nurbs::b_spline_surface_n_lib::p1::create_nlib_curve";
const OP_NLIB_SURFACE: &str = "nurbs::b_spline_surface_n_lib::p1::create_nlib_surface";
const OP_OFFSET: &str =
    "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::approximate_offset_surface";
const OP_NONRAT: &str = "nurbs::b_spline_surface_n_lib::p1::create_non_rational_curve";
const OP_COONS_SRF: &str = "nurbs::b_spline_surface_n_lib::p1::create_coons_srf";
const OP_COONS: &str = "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::create_coons_patch";
const OP_EXTEND: &str =
    "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::create_extended_surface_dir";
const OP_JOIN: &str = "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::join_surface";
const OP_NSIDED: &str = "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::create_n_sided_patch";
const OP_RULED: &str = "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::create_ruled_surface";
const OP_MAKE_KNOTS: &str = "nurbs::b_spline_surface_n_lib::p1::make_knots";
const OP_SWEEP: &str = "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::create_sweep_surface";
const OP_SWUNG: &str = "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::create_swung_surface";
const OP_EXT_EVAL: &str =
    "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::evaluate_point_on_extended_surface";
const OP_REPARAM: &str = "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::reparameterize";
const OP_REVERSE: &str = "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::reverse";
const OP_TRIM_BOUNDS: &str = "nurbs::b_spline_surface_n_lib::p1::check_trim_boundaries";
const OP_EXPAND: &str = "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::expand_boundary_span";
const OP_EVAL_SIMPLE: &str = "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::evaluate_simple";
const OP_EVAL_POINT: &str = "nurbs::b_spline_surface_n_lib::p1::BSplineSurface::evaluate_point";

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// The NLib direction flag of a surface parameter (`SM_SP_U` is `NL_UDIR`, anything
/// else `NL_VDIR`).
fn nl_dir(param: SurfParamType) -> Direction {
    if param == SurfParamType::U {
        Direction::U
    } else {
        Direction::V
    }
}

/// The NURB of `curve` (`GetOrCreateGwNurbPointer`, which cannot make one for a plain
/// B-spline; upstream then dereferences `NULL`).
fn curve_nurb<'a>(curve: &'a BSplineCurve, operation: &'static str) -> Result<&'a Curve> {
    curve
        .nurb
        .as_ref()
        .ok_or_else(|| SolidError::sm_err(operation))
}

/// The NURB of `surface` (`GetOrCreateGwNurbPointer`; `NULL` dereferenced upstream).
fn surface_nurb<'a>(surface: &'a BSplineSurface, operation: &'static str) -> Result<&'a Surface> {
    surface
        .nurb
        .as_ref()
        .ok_or_else(|| SolidError::sm_err(operation))
}

// ---- unported SmSurface methods ------------------------------------------------------

/// `SmSurface::IsSingularity(uv, dir)`: whether `uv` is a singularity and its
/// direction.
fn is_singularity(k: &Kernel, srf: &BSplineSurface, uv: Point2) -> Result<(bool, SurfParamType)> {
    // the SmSurface body (surface.surface_p5), on a surface holding a copy
    let surface = crate::surface::surface::Surface::from(srf.clone_exact());
    let s = surface.is_singularity(k, uv, crate::math::EFF_ZERO, false)?;
    Ok((s.singular, s.direction))
}

/// `SmSurface::IsClosed(domain, dir)`.
fn is_closed(
    k: &Kernel,
    srf: &BSplineSurface,
    domain: &Extent2d,
    dir: SurfParamType,
) -> Result<bool> {
    Ok(crate::surface::surface::p7::base_is_closed(k, srf, domain, dir, None, false)?.0)
}

/// The curves dropped by `SmSurface::DropCurve` (with its two distance outputs, which
/// the caller here ignores).
struct DroppedCurves {
    /// `rUVCurves`.
    uv_curves: Vec<BSplineCurve>,
}

/// `SmSurface::DropCurve(context, domain, curve, interval, tol, maxDist, deviation,
/// uvCurves)` (virtual, with the default `bKeepAllDropCurves = TRUE`,
/// `pOptDropCurveFail = NULL`, `bCreatingUVTrimCurve = TRUE`), dispatched on the kind of
/// `srf` as upstream: the `SmPlane` override ([`crate::surface::plane::Plane::drop_curve`],
/// with the surface's owner) for a plane, the base body
/// ([`crate::surface::surface::Surface::drop_curve_base`]) otherwise. The surface is
/// the caller's (a face's surface keeps its owner).
fn drop_curve<O: crate::geom::solution_array::SolutionObject>(
    k: &Kernel,
    srf: &crate::surface::surface::Surface,
    domain: &Extent2d,
    curve: &BSplineCurve,
    interval: &Extent1d,
    approx_tol: f64,
    context: Option<crate::intersect::surface_drop_lib::p3::DropSolveContext<'_, O>>,
) -> Result<DroppedCurves> {
    use crate::curve::curve::{Curve as AnyCurve, CurveKind};

    let curve = AnyCurve::from(curve.clone());
    let out = match context {
        Some(context) => srf.drop_curve_with_context::<O>(
            k, domain, &curve, interval, approx_tol, true, None, true, context,
        )?,
        None => srf.drop_curve::<O>(k, domain, &curve, interval, approx_tol, true, None, true)?,
    };
    // the drop gives `SmBSplineCurve`s (for a plane a UV `SmLine` or B-spline)
    let uv_curves = out
        .uv_curves
        .into_iter()
        .map(|c| match c.kind() {
            CurveKind::BSpline(b) => Ok(b.clone()),
            CurveKind::Line(l) => Ok(l.base().clone()),
            _ => Err(invalid(OP_NONRAT, "plane drop curve is not a B-spline")),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(DroppedCurves { uv_curves })
}

// ---- NLib copies ----------------------------------------------------------------------

/// A copy of the NURB of `curve` (`sm_BuildNlibCurve`: `N_CrvInitArrays` +
/// `N_CrvCopy`).
///
/// # Errors
///
/// `SM_ERR` for a curve without NURB (upstream dereferences `NULL`).
pub fn build_nlib_curve(curve: &BSplineCurve) -> Result<Curve> {
    Ok(curve_nurb(curve, OP_BUILD_CURVE)?.clone())
}

/// A copy of the NURB of `curve` in arrays sized from the curve's counts
/// (`sm_CreateNlibCurve`: `N_AllocCrvAndArrays` + `N_CrvCopy`).
///
/// # Errors
///
/// `SM_ERR` for a curve without NURB.
pub fn create_nlib_curve(curve: &BSplineCurve) -> Result<Curve> {
    Ok(curve_nurb(curve, OP_NLIB_CURVE)?.clone())
}

/// A copy of the NURB of `surface` (`sm_CreateNlibSurface`: `N_AllocSrfAndArrays` +
/// `N_SrfCopy`).
///
/// # Errors
///
/// `SM_ERR` for a surface without NURB.
pub fn create_nlib_surface(surface: &BSplineSurface) -> Result<Surface> {
    Ok(surface_nurb(surface, OP_NLIB_SURFACE)?.clone())
}

// ---- ApproximateOffsetSurface ---------------------------------------------------------

/// The parametrisation of [`BSplineSurface::approximate_offset_surface`] (upstream
/// `int iParamType`; the values are upstream's, not NLib's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OffsetParamType {
    /// `0`: `NL_INHERITED` (default).
    #[default]
    Inherited = 0,
    /// `1`: `NL_CHORDLENGTH`; the domain is kept.
    ChordLength = 1,
    /// `2`: `NL_CENTRIPETAL`; the domain is kept.
    Centripetal = 2,
    /// `3`: `NL_UNIFORM`; the domain is kept.
    Uniform = 3,
}

/// The warning level of [`create_coons_srf`] (upstream `lWarning`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CoonsWarning {
    /// `0`: no problem found.
    #[default]
    None = 0,
    /// `1`: a cross-boundary derivative still misses the boundary derivatives at the
    /// corners after rescaling.
    CornerMismatch = 1,
    /// `2`: the twist vectors of two cross-boundary derivatives differ at a corner.
    TwistMismatch = 2,
}

// ---- sm_CreateCoonsSrf ----------------------------------------------------------------

/// `N_CrvDerivs(cur, u, NL_LEFT, 1, CD)`, `CD[1]`.
fn deriv1(cur: &Curve, u: f64) -> Result<Vec3> {
    let cd = cur.derivs(u, Side::Left, 1)?;
    cd.get(1)
        .copied()
        .ok_or_else(|| invalid(OP_COONS_SRF, "missing derivative"))
}

/// Checks a cross-boundary derivative `der` against the boundary derivatives `cd1` at
/// its start `t0` and `cd2` at its end `t1`; rescales it linearly by the magnitude
/// ratios when either misses by more than `mtol` (upstream's four "Check ..."
/// blocks). Returns `true` when it still misses after rescaling.
fn match_cross_derivative(
    der: &mut Curve,
    t0: f64,
    t1: f64,
    cd1: Vec3,
    cd2: Vec3,
    mtol: f64,
) -> Result<bool> {
    let mut tan_val1 = der.eval(t0, Side::Left)?;
    let mut tan_val2 = der.eval(t1, Side::Left)?;

    let mut mis_match = false;
    let a = tan_val1 - cd1;
    let b = tan_val2 - cd2;
    let da = a.length();
    if da > mtol {
        mis_match = true;
    }
    let db = b.length();
    if db > mtol {
        mis_match = true;
    }

    if !mis_match {
        return Ok(false);
    }

    // Scale the CBD curve by the start and end magnitude ratios.
    let da = tan_val1.length();
    let db = cd1.length();
    let scale0 = db / da;
    let da = tan_val2.length();
    let db = cd2.length();
    let scale1 = db / da;
    der.linear_multiply(scale0, scale1)?;
    mis_match = false;

    // Recheck them.
    tan_val1 = der.eval(t0, Side::Left)?;
    tan_val2 = der.eval(t1, Side::Left)?;
    let a = tan_val1 - cd1;
    let b = tan_val2 - cd2;
    let da = a.length();
    if da > mtol {
        mis_match = true;
    }
    let db = b.length();
    if db > mtol {
        mis_match = true;
    }
    Ok(mis_match)
}

/// The average twist of two cross-boundary derivative derivatives at a corner; sets
/// `warning` to [`CoonsWarning::TwistMismatch`] when they differ by more than
/// `twist_tol`.
fn twist(cd1: Vec3, cd2: Vec3, twist_tol: f64, warning: &mut CoonsWarning) -> Vec3 {
    let a = cd1 - cd2; // These derivs should be the same.
    let mag = a.length();
    if mag > twist_tol {
        *warning = CoonsWarning::TwistMismatch;
    }
    combine2_pts(0.5, cd1, 0.5, cd2) // Average of two vectors
}

/// `cur[i]`'s control point `j`.
fn cpt_of(cur: &Curve, j: usize) -> Result<CPoint> {
    cur.cpts()
        .get(j)
        .copied()
        .ok_or_else(|| invalid(OP_COONS_SRF, "control point index out of range"))
}

/// Writes `pw` into `sur[i][j]`.
fn put(sur: &mut Surface, i: usize, j: usize, pw: CPoint) -> Result<()> {
    *sur.cpt_mut(i, j)? = pw;
    Ok(())
}

/// The clamped cubic Bezier knots `[0, 0, 0, 0, 1, 1, 1, 1]`.
fn bezier_knots() -> Vec<f64> {
    vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]
}

/// Bicubic Coons surface of four non-rational boundary curves and their
/// cross-boundary derivative curves (upstream `sm_CreateCoonsSrf`).
///
/// `lr` is `[left, right, d_left, d_right]` and `bt` is `[bottom, top, d_bottom,
/// d_top]`; each set is made compatible in place when it is not. Every
/// cross-boundary derivative whose ends miss the boundary derivatives at the corners
/// by more than `mtol` is rescaled in place by the linear function of the magnitude
/// ratios. The corner twists are the averages of the two cross-boundary derivative
/// derivatives meeting there. The surface is the sum of the u- and v-direction
/// blends minus their tensor product, all made compatible.
///
/// Returns the surface and the warning level.
///
/// # Errors
///
/// `NL_INP_ERR` for a rational curve; the errors of the compatibility, evaluation,
/// rescaling and surface compatibility routines; `InvalidInput` for curves whose
/// control point counts do not match after the compatibility step.
pub fn create_coons_srf(
    mtol: f64,
    lr: &mut [Curve; 4],
    bt: &mut [Curve; 4],
) -> Result<(Surface, CoonsWarning)> {
    const OP: &str = OP_COONS_SRF;
    let twist_tol = mtol * 1.0e4;
    let mut warning = CoonsWarning::None;

    // Check rationality and initialize constants
    for c in [
        &lr[0], &lr[1], &bt[0], &bt[1], &lr[2], &lr[3], &bt[2], &bt[3],
    ] {
        if c.is_rational() {
            return Err(SolidError::nlib(OP, crate::error::NlibCode::Input));
        }
    }

    let one_3 = 1.0 / 3.0;
    let one_9 = 1.0 / 9.0;

    // Make input data compatible in the B-spline sense
    if !curves_are_compatible(lr) {
        make_curves_compatible(lr)?;
    }
    if !curves_are_compatible(bt) {
        make_curves_compatible(bt)?;
    }

    // Check twist compatibility: cross-boundary derivative curves must match the
    // first derivatives of the boundary curves at the corners; otherwise correct their
    // magnitudes, warning when that does not work.
    let uk = bt[0].knots().to_vec();
    let vk = lr[0].knots().to_vec();
    let first = |k: &[f64]| k.first().copied().ok_or_else(|| invalid(OP, "no knots"));
    let last = |k: &[f64]| k.last().copied().ok_or_else(|| invalid(OP, "no knots"));
    let (u0, ur) = (first(&uk)?, last(&uk)?);
    let (v0, vs) = (first(&vk)?, last(&vk)?);

    // Check curLR[2]: derL.
    let cd1 = deriv1(&bt[0], u0)?;
    let cd2 = deriv1(&bt[1], u0)?;
    if match_cross_derivative(&mut lr[2], v0, vs, cd1, cd2, mtol)? {
        warning = CoonsWarning::CornerMismatch;
    }

    // Check curLR[3]: derR.
    let cd1 = deriv1(&bt[0], ur)?;
    let cd2 = deriv1(&bt[1], ur)?;
    if match_cross_derivative(&mut lr[3], v0, vs, cd1, cd2, mtol)? {
        warning = CoonsWarning::CornerMismatch;
    }

    // Check curBT[2]: derB.
    let cd1 = deriv1(&lr[0], v0)?;
    let cd2 = deriv1(&lr[1], v0)?;
    if match_cross_derivative(&mut bt[2], u0, ur, cd1, cd2, mtol)? {
        warning = CoonsWarning::CornerMismatch;
    }

    // Check curBT[3]: derT.
    let cd1 = deriv1(&lr[0], vs)?;
    let cd2 = deriv1(&lr[1], vs)?;
    if match_cross_derivative(&mut bt[3], u0, ur, cd1, cd2, mtol)? {
        warning = CoonsWarning::CornerMismatch;
    }

    // Now compute the twist vectors to use: the mixed partials at the four corners,
    // the averages of the derivatives of the two cross-boundary derivative curves
    // meeting there.
    let t00 = twist(
        deriv1(&lr[2], v0)?,
        deriv1(&bt[2], u0)?,
        twist_tol,
        &mut warning,
    );
    let t10 = twist(
        deriv1(&bt[2], ur)?,
        deriv1(&lr[3], v0)?,
        twist_tol,
        &mut warning,
    );
    let t11 = twist(
        deriv1(&lr[3], vs)?,
        deriv1(&bt[3], ur)?,
        twist_tol,
        &mut warning,
    );
    let t01 = twist(
        deriv1(&bt[3], u0)?,
        deriv1(&lr[2], vs)?,
        twist_tol,
        &mut warning,
    );

    // ---- Compute the various surfaces ----

    // Allocate memory
    let (m, q, s, va) = (lr[0].n(), lr[0].degree(), lr[0].m(), lr[0].knots().to_vec());
    let (n, p, r, ua) = (bt[0].n(), bt[0].degree(), bt[0].m(), bt[0].knots().to_vec());

    let mut sur_u: Option<Surface> = None;
    size_arrays(
        &mut sur_u,
        SurfaceIndices {
            n: 3,
            m,
            p: 3,
            q,
            r: 7,
            s,
        },
    )?;
    let mut sur_v: Option<Surface> = None;
    size_arrays(
        &mut sur_v,
        SurfaceIndices {
            n,
            m: 3,
            p,
            q: 3,
            r,
            s: 7,
        },
    )?;
    let mut sur_t: Option<Surface> = None;
    size_arrays(
        &mut sur_t,
        SurfaceIndices {
            n: 3,
            m: 3,
            p: 3,
            q: 3,
            r: 7,
            s: 7,
        },
    )?;
    let (Some(mut sur_u), Some(mut sur_v), Some(mut sur_t)) = (sur_u, sur_v, sur_t) else {
        return Err(invalid(OP, "surface not sized"));
    };

    // Compute u-blend
    for j in 0..=m {
        put(&mut sur_u, 0, j, cpt_of(&lr[0], j)?)?;
    }
    for j in 0..=m {
        put(&mut sur_u, 3, j, cpt_of(&lr[1], j)?)?;
    }
    for j in 0..=m {
        let pw = CPoint::combine2(1.0, sur_u.cpt(0, j)?, one_3, cpt_of(&lr[2], j)?);
        put(&mut sur_u, 1, j, pw)?;
    }
    for j in 0..=m {
        let pw = CPoint::combine2(1.0, sur_u.cpt(3, j)?, -one_3, cpt_of(&lr[3], j)?);
        put(&mut sur_u, 2, j, pw)?;
    }
    for (dst, src) in sur_u.u_knots_mut().iter_mut().zip(bezier_knots()) {
        *dst = src;
    }
    for (dst, src) in sur_u.v_knots_mut().iter_mut().zip(&va) {
        *dst = *src;
    }

    // Compute v-blend
    for i in 0..=n {
        put(&mut sur_v, i, 0, cpt_of(&bt[0], i)?)?;
    }
    for i in 0..=n {
        put(&mut sur_v, i, 3, cpt_of(&bt[1], i)?)?;
    }
    for i in 0..=n {
        let pw = CPoint::combine2(1.0, sur_v.cpt(i, 0)?, one_3, cpt_of(&bt[2], i)?);
        put(&mut sur_v, i, 1, pw)?;
    }
    for i in 0..=n {
        let pw = CPoint::combine2(1.0, sur_v.cpt(i, 3)?, -one_3, cpt_of(&bt[3], i)?);
        put(&mut sur_v, i, 2, pw)?;
    }
    for (dst, src) in sur_v.u_knots_mut().iter_mut().zip(&ua) {
        *dst = *src;
    }
    for (dst, src) in sur_v.v_knots_mut().iter_mut().zip(bezier_knots()) {
        *dst = src;
    }

    // Compute tensor product surface
    for i in 0..=3 {
        put(&mut sur_t, i, 0, sur_u.cpt(i, 0)?)?;
        put(&mut sur_t, i, 3, sur_u.cpt(i, m)?)?;
    }
    for j in 1..=2 {
        put(&mut sur_t, 0, j, sur_v.cpt(0, j)?)?;
        put(&mut sur_t, 3, j, sur_v.cpt(n, j)?)?;
    }

    let t00w = CPoint::from_point(t00);
    let t10w = CPoint::from_point(t10);
    let t01w = CPoint::from_point(t01);
    let t11w = CPoint::from_point(t11);
    let tw = |s: &Surface, i: usize, j: usize| s.cpt(i, j);

    let c11 = CPoint::combine4(
        one_9,
        t00w,
        1.0,
        tw(&sur_t, 1, 0)?,
        1.0,
        tw(&sur_t, 0, 1)?,
        -1.0,
        tw(&sur_t, 0, 0)?,
    );
    put(&mut sur_t, 1, 1, c11)?;
    let c21 = CPoint::combine4(
        -one_9,
        t10w,
        1.0,
        tw(&sur_t, 3, 1)?,
        1.0,
        tw(&sur_t, 2, 0)?,
        -1.0,
        tw(&sur_t, 3, 0)?,
    );
    put(&mut sur_t, 2, 1, c21)?;
    let c12 = CPoint::combine4(
        -one_9,
        t01w,
        1.0,
        tw(&sur_t, 1, 3)?,
        1.0,
        tw(&sur_t, 0, 2)?,
        -1.0,
        tw(&sur_t, 0, 3)?,
    );
    put(&mut sur_t, 1, 2, c12)?;
    let c22 = CPoint::combine4(
        one_9,
        t11w,
        1.0,
        tw(&sur_t, 2, 3)?,
        1.0,
        tw(&sur_t, 3, 2)?,
        -1.0,
        tw(&sur_t, 3, 3)?,
    );
    put(&mut sur_t, 2, 2, c22)?;

    for (dst, src) in sur_t.u_knots_mut().iter_mut().zip(bezier_knots()) {
        *dst = src;
    }
    for (dst, src) in sur_t.v_knots_mut().iter_mut().zip(bezier_knots()) {
        *dst = src;
    }

    // Make surfaces compatible
    let mut sur_a = [sur_u, sur_v, sur_t];
    make_compatible_uv(&mut sur_a)?;
    let [sur_u, sur_v, sur_t] = sur_a;

    // Compute output surface
    let idx = sur_u.indices();
    let mut out: Option<Surface> = None;
    let sur = size_arrays(&mut out, idx)?;
    for i in 0..=idx.n {
        for j in 0..=idx.m {
            let pw = CPoint::translate_sum2(
                sur_u.cpt(i, j)?,
                1.0,
                sur_v.cpt(i, j)?,
                -1.0,
                sur_t.cpt(i, j)?,
            );
            put(sur, i, j, pw)?;
        }
    }
    for (dst, src) in sur.u_knots_mut().iter_mut().zip(sur_u.u_knots()) {
        *dst = *src;
    }
    for (dst, src) in sur.v_knots_mut().iter_mut().zip(sur_u.v_knots()) {
        *dst = *src;
    }
    let sur = out.ok_or_else(|| invalid(OP, "surface not sized"))?;
    Ok((sur, warning))
}

// ---- sm_MakeKnots ---------------------------------------------------------------------

/// Knot vector `U[0..=n+p+1]` of degree `p` for the parameters `u[0..=n]`
/// (upstream `sm_MakeKnots`): full multiplicity `p + 1` at `u[0]` and `u[n]`, the
/// interior knots the averages of `p` consecutive parameters (Greville abscissae),
/// each then snapped to the parameter it lies within a third of a parameter span of,
/// and moved to half the span length when it would form a triple knot.
///
/// # Errors
///
/// `InvalidInput` for `p == 0`, fewer than `n + 1` parameters or fewer than
/// `n + p + 2` knots (upstream reads or writes outside its arrays).
pub fn make_knots(u: &[f64], n: usize, p: usize, knots: &mut [f64]) -> Result<()> {
    const OP: &str = OP_MAKE_KNOTS;
    if p == 0 {
        return Err(invalid(OP, "degree 0"));
    }
    if u.len() <= n
        || n.checked_add(p)
            .and_then(|l| l.checked_add(2))
            .is_none_or(|l| knots.len() < l)
    {
        return Err(invalid(OP, "array too short"));
    }
    let oob = || invalid(OP, "index out of range");
    let at = |a: &[f64], i: usize| a.get(i).copied().ok_or_else(oob);
    let set = |a: &mut [f64], i: usize, v: f64| -> Result<()> {
        *a.get_mut(i).ok_or_else(oob)? = v;
        Ok(())
    };

    // Compute knot vector: full multiplicity (p+1) on the ends, and Greville
    // abscissae on the interior.
    for i in 0..=p {
        set(knots, i, at(u, 0)?)?;
        set(knots, n + i + 1, at(u, n)?)?;
    }

    let mut i = 1;
    while i + p <= n {
        let mut sum = 0.0;
        for j in i..i + p {
            sum += at(u, j)?;
        }
        set(knots, i + p, sum / p as f64)?;
        // GAC - added following loop to clamp U to knots
        for k in 1..=n {
            let ui = at(knots, i + p)?;
            if ui < at(u, k)? {
                let size = at(u, k)? - at(u, k - 1)?;
                let back = ui - at(u, k - 1)?;
                let forward = at(u, k)? - ui;
                let mut test = ui;
                if back < size / 3.0 {
                    test = at(u, k - 1)?;
                } else if forward < size / 3.0 {
                    test = at(u, k)?;
                }
                // Try our best to avoid triple knots
                if are_same(test, at(knots, i + p - 1)?) && are_same(test, at(knots, i + p - 2)?) {
                    test = (at(u, k)? - at(u, k - 1)?) / 2.0;
                }
                set(knots, i + p, test)?;
                break;
            }
        }
        i += 1;
    }
    Ok(())
}

// ---- sm_CheckTrimBoundaries -----------------------------------------------------------

/// Moves proposed trim boundaries out of the interior of `domain` (upstream
/// `sm_CheckTrimBoundaries`, a helper of `TrimWithDomain`).
///
/// When `min_idx` is set and `t_min` lies above the domain's minimum in `dir`'s
/// direction ([`SurfParamType::U`]: u, anything else v), `t_min` becomes the last
/// knot at or below that minimum searching down from `min_idx` (or `knots[0]`);
/// likewise `t_max` becomes the first knot at or above the maximum searching up from
/// `max_idx` (or the last knot). Returns the new `(t_min, t_max)`.
///
/// # Errors
///
/// `InvalidInput` when a search starts past the end of `knots` (upstream reads
/// outside the array) or `knots` is empty where a knot is read.
pub fn check_trim_boundaries(
    knots: &[f64],
    t_min: f64,
    min_idx: Option<usize>,
    t_max: f64,
    max_idx: Option<usize>,
    domain: &Extent2d,
    dir: SurfParamType,
) -> Result<(f64, f64)> {
    const OP: &str = OP_TRIM_BOUNDS;
    let oob = || invalid(OP, "knot index out of range");
    let (mut t_min, mut t_max) = (t_min, t_max);

    // pick knot direction
    let dom = if dir == SurfParamType::U {
        domain.u_interval()
    } else {
        domain.v_interval()
    };

    // When min knot index and TMin are in the interior of sDom
    if let Some(min_idx) = min_idx {
        if t_min > dom.min() {
            if min_idx >= knots.len() {
                return Err(oob());
            }
            // iter every knot looking for 1st knot not within current sDom interval
            let found = knots
                .get(..=min_idx)
                .ok_or_else(oob)?
                .iter()
                .rev()
                .find(|&&k| k <= dom.min());
            match found {
                Some(&k) => t_min = k,
                // before the beginning
                None => t_min = knots.first().copied().ok_or_else(oob)?,
            }
        }
    }

    // when max knot index and tMax are in the interior of sDom
    if let Some(max_idx) = max_idx {
        if t_max < dom.max() {
            // iter every knot looking for 1st knot not within curren sDom interval
            let found = knots.iter().skip(max_idx).find(|&&k| k >= dom.max());
            match found {
                Some(&k) => t_max = k,
                // past the end
                None => t_max = knots.last().copied().ok_or_else(oob)?,
            }
        }
    }

    Ok((t_min, t_max))
}

// ---- sm_check_zero_surface_derivs -----------------------------------------------------

/// Tries to find a direction for zero-length first derivatives (upstream
/// `sm_check_zero_surface_derivs`, a helper of `Evaluate`).
///
/// A first derivative shorter than `scaled_zero` is replaced by the mixed partial
/// `suv` (or, when that is zero too, `suu` for `su` / `svv` for `sv`) scaled to
/// `1.1 * scaled_zero`, negated when the parameter the replacement differentiates by
/// lies above the middle of `domain`. Returns `false` when the last zero derivative
/// examined could not be replaced (upstream's flag: a failed `su` followed by a good
/// `sv` still returns `false`).
pub fn check_zero_surface_derivs(
    uv: Point2,
    domain: &Extent2d,
    su: &mut Vec3,
    sv: &mut Vec3,
    suu: Vec3,
    suv: Vec3,
    svv: Vec3,
    scaled_zero: f64,
) -> bool {
    let mut ret_val = true;

    let new_length = 1.1 * scaled_zero; // length of output vectors.

    // The mixed partial (Suv) is the first derivative of both Su and Sv, with respect
    // to the other, and hence can be used for either one if it's not zero. Try that
    // first. At the 'top' of the domain the degenerate derivative is shrinking to
    // zero, so its change is in the opposite direction of its derivative.
    let mut flip = false;

    if su.length() < scaled_zero {
        ret_val = false;

        // first check how Su is changing with v
        let len = suv.length();
        if len > scaled_zero {
            *su = suv * (new_length / len);
            flip = uv.y > domain.v_interval().mid(); // v-deriv of Su
            ret_val = true;
        } else {
            // try change w.r.t. u
            let len = suu.length();
            if len > scaled_zero {
                *su = suu * (new_length / len);
                flip = uv.x > domain.u_interval().mid(); // u-deriv of Su
                ret_val = true;
            }
        }

        if ret_val && flip {
            *su *= -1.0;
        }
    }

    if sv.length() < scaled_zero {
        ret_val = false;

        // first check how Sv is changing with u
        let len = suv.length();
        if len > scaled_zero {
            *sv = suv * (new_length / len);
            flip = uv.x > domain.u_interval().mid(); // u-deriv of Sv
            ret_val = true;
        } else {
            // try change w.r.t. v
            let len = svv.length();
            if len > scaled_zero {
                *sv = svv * (new_length / len);
                flip = uv.y > domain.v_interval().mid(); // v-deriv of Sv
                ret_val = true;
            }
        }

        if ret_val && flip {
            *sv *= -1.0;
        }
    }

    ret_val
}

// ---- sm_CreateNonRationalCurve --------------------------------------------------------

/// A non-rational copy of `curve`, with its UV trim curve (upstream
/// `sm_CreateNonRationalCurve`).
///
/// A non-rational `curve` is copied, together with `opt_uv_trim_curve` when given. A
/// rational one is approximated within `approx_tol` by a non-rational cubic
/// ([`BSplineCurve::approximate_rational_curve`]) and reparametrised by arc length;
/// with `opt_base_surface` the new curve is dropped onto it (over its natural
/// domain) and the first dropped curve is the UV trim curve (a given
/// `opt_uv_trim_curve` is ignored then).
///
/// # Errors
///
/// The errors of the rationality test, the approximation, the reparametrisation and
/// the drop; `InvalidInput` when the drop gives no
/// curve (upstream asserts exactly one).
pub fn create_non_rational_curve(
    k: &Kernel,
    curve: &BSplineCurve,
    opt_uv_trim_curve: Option<&BSplineCurve>,
    opt_base_surface: Option<&crate::surface::surface::Surface>,
    approx_tol: f64,
) -> Result<(BSplineCurve, Option<BSplineCurve>)> {
    create_non_rational_curve_with_context::<crate::surface::surface::p4::NoObject>(
        k,
        curve,
        opt_uv_trim_curve,
        opt_base_surface,
        approx_tol,
        None,
    )
}

/// [`create_non_rational_curve`] whose drop onto `opt_base_surface` runs under the
/// caller's face context.
///
/// # Errors
///
/// As [`create_non_rational_curve`], and those of the context.
pub fn create_non_rational_curve_with_context<O: crate::geom::solution_array::SolutionObject>(
    k: &Kernel,
    curve: &BSplineCurve,
    opt_uv_trim_curve: Option<&BSplineCurve>,
    opt_base_surface: Option<&crate::surface::surface::Surface>,
    approx_tol: f64,
    context: Option<crate::intersect::surface_drop_lib::p3::DropSolveContext<'_, O>>,
) -> Result<(BSplineCurve, Option<BSplineCurve>)> {
    // for rational curves
    if curve.is_rational()? {
        // Build arc-length parameterized nonRational curve approximation
        let mut new_curve = curve.approximate_rational_curve(approx_tol)?;
        new_curve.reparametrize_with_arc_length()?;

        // When given optional UVTrimCurve - build arc-length nonRational UVTrimCurve
        let mut new_uv_trim_curve = None;
        if let Some(base) = opt_base_surface {
            let trim_ivl = new_curve.natural_interval();
            let domain = base.natural_uv_domain_with_nurb(k)?;
            let dropped = drop_curve(k, base, &domain, &new_curve, &trim_ivl, approx_tol, context)?;
            new_uv_trim_curve = Some(
                dropped
                    .uv_curves
                    .into_iter()
                    .next()
                    .ok_or_else(|| invalid(OP_NONRAT, "no dropped curve"))?,
            );
        }
        Ok((new_curve, new_uv_trim_curve))
    } else {
        // make a curve copy and optional UVTrimCurve copy
        Ok((curve.copy(), opt_uv_trim_curve.map(BSplineCurve::copy)))
    }
}

// ---- SmBSplineSurface methods ---------------------------------------------------------

impl BSplineSurface {
    /// Cubic approximation of the offset of the surface by `offset_distance` along its
    /// normal (negative for insets) within `approx_tol_3d` (`ApproximateOffsetSurface`).
    /// With a parametrisation other than [`OffsetParamType::Inherited`] the result is
    /// reparametrised to the surface's domain (to approximately match trim curves).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without NURB; `SM_ERR` when `N_SrfOffset` fails.
    pub fn approximate_offset_surface(
        &self,
        offset_distance: f64,
        approx_tol_3d: f64,
        param_type: OffsetParamType,
    ) -> Result<BSplineSurface> {
        self.approximate_offset_surface_with(
            offset_distance,
            approx_tol_3d,
            param_type,
            crate::nurbs::srf_offset::OffsetOptions::default(),
        )
    }

    /// [`BSplineSurface::approximate_offset_surface`] with explicit corrections for
    /// extrusion recognition and sliver-span sampling (issue #471).
    /// The default [`crate::nurbs::srf_offset::OffsetOptions`] preserves SMLib.
    pub fn approximate_offset_surface_with(
        &self,
        offset_distance: f64,
        approx_tol_3d: f64,
        param_type: OffsetParamType,
        options: crate::nurbs::srf_offset::OffsetOptions,
    ) -> Result<BSplineSurface> {
        let nl_surf_p = self
            .nurb
            .as_ref()
            .ok_or_else(|| SolidError::null_pointer(OP_OFFSET))?;

        // cubic degree
        let ln_deg = 3;

        // in case input was from a face, capture the domain
        let mut bounds = Rectangle::new(0.0, 0.0, 0.0, 0.0);
        if param_type != OffsetParamType::Inherited {
            bounds = nl_surf_p.parameter_bounds();
        }

        // map SMLib Param vals to NLib Param vals
        let nl_param = match param_type {
            OffsetParamType::ChordLength => OffsetParam::Fit(ParamMethod::ChordLength),
            OffsetParamType::Centripetal => OffsetParam::Fit(ParamMethod::Centripetal),
            OffsetParamType::Uniform => OffsetParam::Fit(ParamMethod::Uniform),
            OffsetParamType::Inherited => OffsetParam::Inherited,
        };

        // pass the call along to NLib
        let mut nl_surf = nl_surf_p
            .offset_with(
                offset_distance,
                ln_deg,
                ln_deg,
                nl_param,
                10.0 * approx_tol_3d,
                approx_tol_3d,
                options,
            )
            .map_err(SolidError::nl_ser(OP_OFFSET))?;

        // reset domain to original ... to approx match trim.curves if face
        if param_type != OffsetParamType::Inherited {
            // The status is ignored upstream.
            let _ = nl_surf.reparam(bounds.ul, bounds.ur, bounds.vb, bounds.vt);
        }

        // use the new Nurb to make a new BSpline
        Ok(Self::from_gw_nurb(&nl_surf))
    }

    /// A Coons patch through four boundary curves `bdry_crvs` (left, right, bottom,
    /// top; left and right running the same way over the same domain, likewise bottom
    /// and top, meeting at the corners) (`CreateCoonsPatch`).
    ///
    /// With `create_bilinear_patch` the patch is the bilinear Coons surface of the
    /// boundaries (`N_CreateCoonsSrf`; `deriv_crvs` are ignored). Otherwise it is the
    /// bicubic patch matching the four cross-boundary derivative curves `deriv_crvs`
    /// ([`create_coons_srf`] with tolerance `tol`). All curves must be non-rational for
    /// the bicubic patch. The input curves are changed in place (made compatible,
    /// derivatives rescaled), as upstream.
    ///
    /// # Errors
    ///
    /// `InvalidInput` unless there are four boundary curves and zero or four
    /// derivative curves; `SM_ERR` for a bicubic patch without derivative curves, for
    /// curves without NURB, or when the NLib construction fails.
    pub fn create_coons_patch(
        tol: f64,
        create_bilinear_patch: bool,
        bdry_crvs: &mut [BSplineCurve],
        deriv_crvs: &mut [BSplineCurve],
    ) -> Result<BSplineSurface> {
        const OP: &str = OP_COONS;
        // check input - 4 curves
        let [cur_l, cur_r, cur_b, cur_t] = bdry_crvs else {
            return Err(invalid(OP, "need four boundary curves"));
        };
        if !(deriv_crvs.is_empty() || deriv_crvs.len() == 4) {
            return Err(invalid(OP, "need zero or four derivative curves"));
        }

        // Get boundary curves and derivatives
        let l = curve_nurb(cur_l, OP)?.clone();
        let r = curve_nurb(cur_r, OP)?.clone();
        let b = curve_nurb(cur_b, OP)?.clone();
        let t = curve_nurb(cur_t, OP)?.clone();

        // when given crossTangent functions (the bilinear patch does not use them)
        let ders = if create_bilinear_patch {
            None
        } else if let [der_l, der_r, der_b, der_t] = deriv_crvs {
            Some([
                curve_nurb(der_l, OP)?.clone(),
                curve_nurb(der_r, OP)?.clone(),
                curve_nurb(der_b, OP)?.clone(),
                curve_nurb(der_t, OP)?.clone(),
            ])
        } else {
            return Err(SolidError::sm_err(OP)); // Missing derivative curves
        };

        // create patch branching on bCreateBilinearPatch flag
        let sur = if create_bilinear_patch {
            // DOES NOT USE DERIVS
            let mut cur_u = [l, r];
            let mut cur_v = [b, t];
            let res = bilinear_coons_srf(&mut cur_u, &mut cur_v);
            let [l, r] = cur_u;
            let [b, t] = cur_v;
            cur_l.nurb = Some(l);
            cur_r.nurb = Some(r);
            cur_b.nurb = Some(b);
            cur_t.nurb = Some(t);
            res.map_err(SolidError::nl_ser(OP))?
        } else {
            let [der_l, der_r, der_b, der_t] = ders.ok_or_else(|| SolidError::sm_err(OP))?;
            let mut lr = [l, r, der_l, der_r];
            let mut bt = [b, t, der_b, der_t];
            let res = create_coons_srf(tol, &mut lr, &mut bt);
            let [l, r, der_l, der_r] = lr;
            let [b, t, der_b, der_t] = bt;
            cur_l.nurb = Some(l);
            cur_r.nurb = Some(r);
            cur_b.nurb = Some(b);
            cur_t.nurb = Some(t);
            if let [dl, dr, db, dt] = deriv_crvs {
                dl.nurb = Some(der_l);
                dr.nurb = Some(der_r);
                db.nurb = Some(der_b);
                dt.nurb = Some(der_t);
            }
            // A warning only prints a debug message upstream.
            let (sur, _warning) = res.map_err(SolidError::nl_ser(OP))?;
            sur
        };

        // copy the new surface into an SMLib object
        Ok(Self::from_gw_nurb(&sur))
    }

    /// The surface extended by `dist` across each side ([`SurfParamType::Both`]) with
    /// continuity `extension_continuity` (`CreateExtendedSurface(crContext, dDist,
    /// eExtensionContinuity, rpExtended)`).
    ///
    /// # Errors
    ///
    /// The errors of [`BSplineSurface::create_extended_surface_dir`].
    pub fn create_extended_surface(
        &self,
        k: &Kernel,
        dist: f64,
        extension_continuity: ContinuityType,
    ) -> Result<BSplineSurface> {
        // pass the call along
        self.create_extended_surface_dir(k, SurfParamType::Both, dist, extension_continuity)
    }

    /// The surface extended by `dist` across the sides named by `ext_direction`
    /// (`CreateExtendedSurface` with `SmSurfParamType`): [`SurfParamType::U`] /
    /// [`SurfParamType::V`] both sides of one direction, `UMin`, `VMin`, `UMax`,
    /// `VMax` one side, `Both` all four, `Neither` none.
    ///
    /// The continuity is `NL_G1` for [`ContinuityType::G1`] (and any type not listed),
    /// `NL_G1R` for `G1R`, `NL_G2` for `G1G2` and `NL_CMAX` for `CInfinity`. Sides at a
    /// singularity and directions in which the surface is closed are not extended; a
    /// surface closed in both directions is copied. A failing extension is retried with
    /// half the distance (down to `dist / 100`), then repeated with doubled distances
    /// until about `dist` is reached.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB, when no extension succeeds, a repeated extension fails
    /// or an extended surface is invalid; [`SolidError::Unported`] from the
    /// singularity and closedness tests.
    pub fn create_extended_surface_dir(
        &self,
        k: &Kernel,
        ext_direction: SurfParamType,
        dist: f64,
        extension_continuity: ContinuityType,
    ) -> Result<BSplineSurface> {
        self.create_extended_surface_dispatched(
            ext_direction,
            dist,
            extension_continuity,
            &|uv| is_singularity(k, self, uv),
            &|domain, dir| is_closed(k, self, domain, dir),
        )
    }

    /// The body of [`BSplineSurface::create_extended_surface_dir`] with the virtual
    /// `IsSingularity(uv, dir)` and `IsClosed(domain, dir)` of the calling object
    /// passed in (an `SmPlane` overrides both), as upstream's virtual dispatch.
    pub(crate) fn create_extended_surface_dispatched(
        &self,
        ext_direction: SurfParamType,
        dist: f64,
        extension_continuity: ContinuityType,
        is_singularity: &dyn Fn(Point2) -> Result<(bool, SurfParamType)>,
        is_closed: &dyn Fn(&Extent2d, SurfParamType) -> Result<bool>,
    ) -> Result<BSplineSurface> {
        const OP: &str = OP_EXTEND;

        // copy input surface into local object
        let mut esur = surface_nurb(self, OP)?.clone();

        let uv_domain = self.natural_uv_domain();
        let mut ext_dist = dist;

        // pick continuity level
        let gflg = match extension_continuity {
            ContinuityType::G1R => SrfContinuity::G1R,
            ContinuityType::G1G2 => SrfContinuity::G2,
            ContinuityType::CInfinity => SrfContinuity::Cmax,
            _ => SrfContinuity::G1, // SM_CT_G1 and anything else
        };

        // sort out where extension is needed
        let u_dir = matches!(
            ext_direction,
            SurfParamType::U | SurfParamType::UMin | SurfParamType::UMax
        );
        let v_dir = matches!(
            ext_direction,
            SurfParamType::V | SurfParamType::VMin | SurfParamType::VMax
        );
        let ext_u_or_v = if u_dir {
            SurfParamType::U
        } else if v_dir {
            SurfParamType::V
        } else {
            SurfParamType::Both
        };
        let neither = ext_direction == SurfParamType::Neither;
        let mut skip_umin = neither || v_dir || ext_direction == SurfParamType::UMax;
        let mut skip_vmin = neither || u_dir || ext_direction == SurfParamType::VMax;
        let mut skip_umax = neither || v_dir || ext_direction == SurfParamType::UMin;
        let mut skip_vmax = neither || u_dir || ext_direction == SurfParamType::VMin;

        // Don't extend through singularities in U or V
        let (sing, sing_direction) = is_singularity(uv_domain.min())?;
        if sing {
            if sing_direction == SurfParamType::U {
                skip_vmin = true;
            } else {
                skip_umin = true;
            }
        }
        let (sing, sing_direction) = is_singularity(uv_domain.max())?;
        if sing {
            if sing_direction == SurfParamType::U {
                skip_vmax = true;
            } else {
                skip_umax = true;
            }
        }

        // when asked and allowed - extend in U direction
        if (ext_u_or_v == SurfParamType::U || ext_u_or_v == SurfParamType::Both)
            && !is_closed(&uv_domain, SurfParamType::U)?
        {
            // when extending in the UMin direction
            if !skip_umin {
                extend_side(
                    &mut esur,
                    dist,
                    &mut ext_dist,
                    Direction::U,
                    CurveEnd::Start,
                    gflg,
                )?;
            }

            // reset the extension distance
            ext_dist = dist;

            // when asked - extend in the UMax direction
            if !skip_umax {
                extend_side(
                    &mut esur,
                    dist,
                    &mut ext_dist,
                    Direction::U,
                    CurveEnd::End,
                    gflg,
                )?;
            }
        }

        // when asked - extend in the V direction
        if (ext_u_or_v == SurfParamType::V || ext_u_or_v == SurfParamType::Both)
            && !is_closed(&uv_domain, SurfParamType::V)?
        {
            // reset extension distance
            ext_dist = dist;

            // when asked - extend in the V Min direction
            if !skip_vmin {
                extend_side(
                    &mut esur,
                    dist,
                    &mut ext_dist,
                    Direction::V,
                    CurveEnd::Start,
                    gflg,
                )?;
            }

            // reset the extension distance
            ext_dist = dist;

            // when asked - extend in the V Max direction
            if !skip_vmax {
                extend_side(
                    &mut esur,
                    dist,
                    &mut ext_dist,
                    Direction::V,
                    CurveEnd::End,
                    gflg,
                )?;
            }
        }

        // build output SmBSplineSurface from esur SURFACE object
        if is_closed(&uv_domain, SurfParamType::U)? && is_closed(&uv_domain, SurfParamType::V)? {
            // surfaces closed in both directions - don't get extended - copy input
            Ok(self.copy())
        } else {
            Ok(Self::from_gw_nurb(&esur))
        }
    }

    /// Joins the surface with `surface_to_join` along a common boundary
    /// (`JoinSurface`): with [`SurfParamType::U`] the v-maximum isocurve of `self`
    /// meets the v-minimum isocurve of `surface_to_join` (made compatible in v), with
    /// [`SurfParamType::V`] the u-maximum meets the u-minimum (made compatible in u).
    /// The joint knot (multiplicity the degree) is removed within `tolerance`. The
    /// joined domain in the join direction is `[this min, this max + other max - other
    /// min]`.
    ///
    /// Both surfaces are made compatible in place, as upstream (whose documentation
    /// says they are not modified).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a direction other than `U` or `V`; `SM_ERR` without NURBs,
    /// when the surfaces cannot be made compatible or the join fails.
    pub fn join_surface(
        &mut self,
        surface_to_join: &mut BSplineSurface,
        dir_flag: SurfParamType,
        tolerance: f64,
    ) -> Result<BSplineSurface> {
        const OP: &str = OP_JOIN;
        // UorV = 2 - eDirFlag for the compatibility, eDirFlag + 1 for the join.
        let (compat_dir, join_dir) = match dir_flag {
            SurfParamType::U => (Direction::V, Direction::U),
            SurfParamType::V => (Direction::U, Direction::V),
            _ => return Err(invalid(OP, "join direction must be U or V")),
        };
        let this_sur = surface_nurb(self, OP)?.clone();
        let sur_2_join = surface_nurb(surface_to_join, OP)?.clone();

        // Make surfaces compatible.
        let mut sur_list = [this_sur, sur_2_join];
        let res = make_surfaces_compatible(&mut sur_list, compat_dir);
        let [this_sur, sur_2_join] = sur_list;
        self.nurb = Some(this_sur).into();
        surface_to_join.nurb = Some(sur_2_join).into();
        // surfaces not compatible, so not joined
        res.map_err(SolidError::nl_ser(OP))?;

        let (Some(this_sur), Some(sur_2_join)) =
            (self.nurb.as_ref(), surface_to_join.nurb.as_ref())
        else {
            return Err(SolidError::sm_err(OP));
        };
        // join operation failed -> SM_ERR
        let joined =
            join(this_sur, sur_2_join, join_dir, tolerance).map_err(SolidError::nl_ser(OP))?;

        Ok(Self::from_gw_nurb(&joined))
    }

    /// Fills an n-sided hole bounded by the non-rational curves `bdry_crvs` (joined
    /// end to start, all running the same way) with `bdry_crvs.len()` patches
    /// (`CreateNSidedPatch`, `N_FillNSidedHole`). `deriv_crvs` holds a cross-boundary
    /// derivative curve per boundary curve (`None` where not specified); the patch
    /// derivatives approximate them within `ang_tol_deg` degrees. With
    /// `opt_center` (point and normal of the tangent plane at the patch center) the
    /// center is given, else computed; the normal is unitized in place. The input
    /// curves are changed in place as `N_FillNSidedHole` changes them.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for fewer derivative entries than boundary curves; `SM_ERR` for
    /// curves without NURB or when NLib fails (fewer than three curves, rational
    /// curves, boundaries that do not join, ...).
    pub fn create_n_sided_patch(
        bdry_crvs: &mut [BSplineCurve],
        deriv_crvs: &mut [Option<BSplineCurve>],
        ang_tol_deg: f64,
        opt_center: Option<(Point3, &mut Vec3)>,
    ) -> Result<Vec<BSplineSurface>> {
        Self::create_n_sided_patch_with(bdry_crvs, deriv_crvs, ang_tol_deg, opt_center, false)
    }

    /// [`BSplineSurface::create_n_sided_patch`] with the opt-in fix of WR-461 (#1170)
    /// when `stay_inside`: the cross-boundary derivatives across the inner curves are
    /// shortened where their blend would leave the neighbour's tangent plane
    /// (`Kernel::fixes.corner_blend_stays_inside_neighbours`).
    ///
    /// # Errors
    ///
    /// As [`BSplineSurface::create_n_sided_patch`].
    pub fn create_n_sided_patch_with(
        bdry_crvs: &mut [BSplineCurve],
        deriv_crvs: &mut [Option<BSplineCurve>],
        ang_tol_deg: f64,
        opt_center: Option<(Point3, &mut Vec3)>,
        stay_inside: bool,
    ) -> Result<Vec<BSplineSurface>> {
        const OP: &str = OP_NSIDED;
        if deriv_crvs.len() < bdry_crvs.len() {
            return Err(invalid(OP, "fewer derivative entries than boundary curves"));
        }

        // for every input pos/crossTan input pair - set NLib call input
        let mut cur = Vec::with_capacity(bdry_crvs.len());
        let mut der = Vec::with_capacity(bdry_crvs.len());
        for (b, d) in bdry_crvs.iter().zip(deriv_crvs.iter()) {
            cur.push(curve_nurb(b, OP)?.clone());
            der.push(match d {
                Some(d) => Some(curve_nurb(d, OP)?.clone()),
                None => None,
            });
        }

        // use input tangent plane - when given
        let center = opt_center.map(|(ci, ni)| {
            *ni = ni.unitize_or_z();
            (ci, *ni)
        });

        // pass the call along to NLib
        let res = fill_n_sided_hole_with(
            &mut cur,
            Some(der.as_mut_slice()),
            ang_tol_deg,
            center,
            stay_inside,
        );
        for (b, c) in bdry_crvs.iter_mut().zip(cur) {
            b.nurb = Some(c);
        }
        for (d, c) in deriv_crvs.iter_mut().zip(der) {
            if let (Some(d), Some(c)) = (d.as_mut(), c) {
                d.nurb = Some(c);
            }
        }
        let fill = res.map_err(SolidError::nl_ser(OP))?;

        // for every surface - build an SmBSplineSurface output object
        Ok(fill.surfaces.iter().map(Self::from_gw_nurb).collect())
    }

    /// The ruled surface between `curve1` (the minimum isocurve) and `curve2` (the
    /// maximum isocurve), linear in u for `linear_surf_param` [`SurfParamType::U`] and
    /// in v otherwise (`CreateRuledSurface`). The curves are not changed (NLib works on
    /// copies).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for curves without NURB or when `N_CreateRuledSrf` fails.
    pub fn create_ruled_surface(
        curve1: &BSplineCurve,
        curve2: &BSplineCurve,
        linear_surf_param: SurfParamType,
    ) -> Result<BSplineSurface> {
        let dir = nl_dir(linear_surf_param);

        // NLib curves from input curves
        let mut cur1 = build_nlib_curve(curve1).map_err(|_| SolidError::sm_err(OP_RULED))?;
        let mut cur2 = build_nlib_curve(curve2).map_err(|_| SolidError::sm_err(OP_RULED))?;

        // pass the call along to NLib
        let sur = ruled_srf(&mut cur1, &mut cur2, dir).map_err(SolidError::nl_ser(OP_RULED))?;

        // BSplineSurface from NLib SURFACE
        Ok(Self::from_gw_nurb(&sur))
    }

    /// A swept surface of `section_curve` (u) along `trajectory_curve` (v)
    /// (`CreateSweepSurface`).
    ///
    /// With `do_translational_sweep` the surface is the exact translational sweep
    /// `S(u) + T(v)` (`N_CreateTransSweepSrf`). Otherwise it approximates within
    /// `approx_tol_3d` the sweep of the section, scaled by `opt_scale_curve(v)` in x,
    /// y, z, positioned along the trajectory (`N_CreateSweepScale` with the z axis a
    /// unit vector orthogonal to the trajectory's start tangent; cubic in the sweep
    /// direction unless the trajectory has a lower degree and no scale curve).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for curves without NURB, a zero start tangent, or when NLib fails.
    pub fn create_sweep_surface(
        section_curve: &BSplineCurve,
        trajectory_curve: &BSplineCurve,
        opt_scale_curve: Option<&BSplineCurve>,
        do_translational_sweep: bool,
        approx_tol_3d: f64,
    ) -> Result<BSplineSurface> {
        Self::sweep_surface(
            section_curve,
            trajectory_curve,
            opt_scale_curve,
            do_translational_sweep,
            approx_tol_3d,
            None,
        )
    }

    /// [`create_sweep_surface`](Self::create_sweep_surface) that also reports where
    /// the section's first, middle and last instances lie (the source fork's
    /// `pOptPlacements`, source fork 5d3939e, #1450): the systems the section
    /// curve's own coordinates were carried onto. A translational sweep's instance at
    /// `v` is the section moved by `TrajectoryCurve(v)` (at the trajectory's start,
    /// middle and end parameters, axes unturned); otherwise they are the local systems
    /// `N_CreateSweepScale` placed the first, middle and last sections on. The surface
    /// is the same as [`create_sweep_surface`](Self::create_sweep_surface)'s.
    ///
    /// # Errors
    ///
    /// Those of [`create_sweep_surface`](Self::create_sweep_surface), and of the
    /// trajectory's point evaluations for a translational sweep.
    pub fn create_sweep_surface_with_placements(
        section_curve: &BSplineCurve,
        trajectory_curve: &BSplineCurve,
        opt_scale_curve: Option<&BSplineCurve>,
        do_translational_sweep: bool,
        approx_tol_3d: f64,
    ) -> Result<(BSplineSurface, [Axis2Placement; 3])> {
        let mut placements = [Axis2Placement::IDENTITY; 3];
        let surface = Self::sweep_surface(
            section_curve,
            trajectory_curve,
            opt_scale_curve,
            do_translational_sweep,
            approx_tol_3d,
            Some(&mut placements),
        )?;
        Ok((surface, placements))
    }

    /// `CreateSweepSurface` with the fork's optional `pOptPlacements` output.
    fn sweep_surface(
        section_curve: &BSplineCurve,
        trajectory_curve: &BSplineCurve,
        opt_scale_curve: Option<&BSplineCurve>,
        do_translational_sweep: bool,
        approx_tol_3d: f64,
        opt_placements: Option<&mut [Axis2Placement; 3]>,
    ) -> Result<BSplineSurface> {
        const OP: &str = OP_SWEEP;

        // get NurbCurve pointers for Section and trajectory curves
        let nl_curv_s = curve_nurb(section_curve, OP)?;
        let nl_curv_t = curve_nurb(trajectory_curve, OP)?;

        // when given a scaling curve - get its NurbCurve Pointer
        let nl_scale_curv = match opt_scale_curve {
            Some(c) => Some(curve_nurb(c, OP)?),
            None => None,
        };

        // branch on TranslationalSweep flag
        let nl_surf = if do_translational_sweep {
            // nlSurf(u,v) = nlCurvS(u) + nlCurvT(v)
            let surf =
                create_trans_sweep_srf(nl_curv_t, nl_curv_s).map_err(SolidError::nl_ser(OP))?;

            // source fork (5d3939e): the section's instance at v is the section moved
            // by TrajectoryCurve(v)
            if let Some(placements) = opt_placements {
                let ivl = trajectory_curve.natural_interval();
                let ts = [ivl.min(), ivl.evaluate(0.5), ivl.max()];
                for (placement, t) in placements.iter_mut().zip(ts) {
                    let at = trajectory_curve.evaluate_point(t)?;
                    *placement = Axis2Placement::new(at, Vec3::X, Vec3::Y);
                }
            }
            surf
        } else {
            // make orthogonal vector set: x = trajectory start tangent, y = any perp
            // vector to x, z = x * y
            let pv = trajectory_curve.evaluate(
                trajectory_curve.natural_interval().min(),
                1,
                true,
                false,
            )?;
            let tangent = pv.get(1).copied().ok_or_else(|| SolidError::sm_err(OP))?;
            let [_x_axis, _y_axis, z_axis] = tangent.make_unit_ortho_vectors(None)?;

            // degree would normally be cubic, except if trajectory = linear and there
            // is no scaling curve
            let mut nl_deg = 3;
            if nl_curv_t.degree() < nl_deg {
                nl_deg = nl_curv_t.degree();
            }
            if nl_scale_curv.is_some_and(|c| c.degree() > nl_deg) {
                nl_deg = 3;
            }

            let opt = SweepScaleOptions {
                cur_z: None,
                cur_s: nl_scale_curv,
                tflg: false,
                tc: 0.0,
                deg: nl_deg,
                dir: Direction::U,
                e: approx_tol_3d,
            };
            match opt_placements {
                None => create_sweep_scale(nl_curv_t, nl_curv_s, z_axis, &opt)
                    .map_err(SolidError::nl_ser(OP))?,
                Some(placements) => {
                    let (surf, placed) =
                        create_sweep_scale_with_placements(nl_curv_t, nl_curv_s, z_axis, &opt)
                            .map_err(SolidError::nl_ser(OP))?;
                    // source fork (5d3939e): each instance is the section carried from
                    // the global system onto the local one (OL, XL, YL, ZL) it was placed on
                    for (placement, frame) in placements.iter_mut().zip(placed) {
                        *placement = Axis2Placement::new(frame.origin, frame.x, frame.y);
                    }
                    surf
                }
            }
        };

        // build SmBSplineSurface from NurbSurface
        Ok(Self::from_gw_nurb(&nl_surf))
    }

    /// The swung surface of a profile `section_curve` in the `[x, z]` plane and a
    /// trajectory `trajectory_curve` in the `[x, y]` plane, scaled by `scale_factor`
    /// (`CreateSwungSurface`, `N_CreateSwungSrf`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for curves without NURB or when NLib fails.
    pub fn create_swung_surface(
        section_curve: &BSplineCurve,
        trajectory_curve: &BSplineCurve,
        scale_factor: f64,
    ) -> Result<BSplineSurface> {
        const OP: &str = OP_SWUNG;
        let nl_curv_p = curve_nurb(section_curve, OP)?;
        let nl_curv_t = curve_nurb(trajectory_curve, OP)?;
        let nl_surf =
            create_swung_srf(nl_curv_p, nl_curv_t, scale_factor).map_err(SolidError::nl_ser(OP))?;
        Ok(Self::from_gw_nurb(&nl_surf))
    }

    /// The point at `uv`, which may lie outside the domain, on the surface extended
    /// with `NL_G1R` continuity (or wrapped in closed directions)
    /// (`EvaluatePointOnExtendedSurface`, `N_SrfEvalPtDerivsUnbounded`; a failed
    /// evaluation is retried once with the extension built so far).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB or when both evaluations fail.
    pub fn evaluate_point_on_extended_surface(&self, uv: Point2) -> Result<Point3> {
        const OP: &str = OP_EXT_EVAL;
        let sur = surface_nurb(self, OP)?;
        let cflg = ExtendContinuity::G1R;
        let mut esur = None;
        let d = match eval_pt_derivs_unbounded(sur, uv.x, uv.y, 0, cflg, &mut esur) {
            Ok(d) => d,
            Err(SolidError::Unported { upstream, unit }) => {
                return Err(SolidError::Unported { upstream, unit })
            }
            Err(_) => eval_pt_derivs_unbounded(sur, uv.x, uv.y, 0, cflg, &mut esur)
                .map_err(SolidError::nl_ser(OP))?,
        };
        d.first()
            .and_then(|row| row.first())
            .copied()
            .ok_or_else(|| SolidError::sm_err(OP))
    }

    /// Maps the parameter domain linearly onto `trim_domain` (`Reparameterize`,
    /// `N_SrfReparamToInterval` in both directions).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without NURB; `InvalidInput` where NLib would write
    /// outside its knot arrays.
    pub fn reparameterize(&mut self, trim_domain: &Extent2d) -> Result<()> {
        let min = trim_domain.min();
        let max = trim_domain.max();
        let r = Rectangle::new(min.x, max.x, min.y, max.y);

        let sur_p = self
            .nurb
            .as_mut()
            .ok_or_else(|| SolidError::null_pointer(OP_REPARAM))?;
        sur_p.reparam_to_interval(r, Direction::UV)?;
        Ok(())
    }

    /// Reverses the parametrisation in v ([`SurfParamType::V`]) or u (anything else)
    /// (`Reverse`, `N_SrfReverse` in place).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB or when NLib fails.
    pub fn reverse(&mut self, surf_param: SurfParamType) -> Result<()> {
        let dir = if surf_param == SurfParamType::V {
            Direction::V
        } else {
            Direction::U
        };
        let sur = self
            .nurb
            .as_mut()
            .ok_or_else(|| SolidError::sm_err(OP_REVERSE))?;
        sur.reverse(dir).map_err(SolidError::nl_ser(OP_REVERSE))?;

        // give analytics a chance to update their nested STEP data
        self.rebuild_step_from_nurb_parameters()?;
        Ok(())
    }

    /// Moves the u ([`SurfParamType::U`]) or v (anything else) domain boundary out to
    /// `new_u_or_v_param` by extending the boundary span polynomially
    /// (`ExpandBoundarySpan`, `N_SrfExtendByParamDist` with `NL_CMAX`); nothing
    /// happens for a parameter inside the domain. The control point and knot counts
    /// do not change.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB, when the extension fails, or when it changes the counts
    /// (the surface is left unchanged then).
    pub fn expand_boundary_span(
        &mut self,
        new_u_or_v_param: f64,
        cross_section_surf_param: SurfParamType,
    ) -> Result<()> {
        const OP: &str = OP_EXPAND;
        let uv_domain = self.natural_uv_domain();

        let sur_p = surface_nurb(self, OP)?;
        let dflg = nl_dir(cross_section_surf_param);
        let (dmin, dmax) = (uv_domain.min(), uv_domain.max());
        let end = if (dflg == Direction::U && new_u_or_v_param > dmax.x)
            || (dflg == Direction::V && new_u_or_v_param > dmax.y)
        {
            CurveEnd::End
        } else if (dflg == Direction::U && new_u_or_v_param < dmin.x)
            || (dflg == Direction::V && new_u_or_v_param < dmin.y)
        {
            CurveEnd::Start
        } else {
            // no work - specified param value contained within UVDomain
            return Ok(());
        };

        // pass the call along
        let sur_q =
            extend_by_param_dist(sur_p, new_u_or_v_param, dflg, end, ExtendContinuity::Cmax)
                .map_err(SolidError::nl_ser(OP))?;

        // verify that surP is the same size as surQ
        let same_size = sur_p.n() == sur_q.n()
            && sur_p.m() == sur_q.m()
            && sur_p.r() == sur_q.r()
            && sur_p.s() == sur_q.s();

        let rtn = if same_size {
            // copy surQ values into surP
            let mut nurb = self.nurb.take();
            sur_q.copy_into(&mut nurb);
            self.nurb = nurb.into();
            Ok(())
        } else {
            // something went wrong - leave surP unmodified
            Err(SolidError::sm_err(OP))
        };

        // give analytics a chance to update their nested STEP data
        self.rebuild_step_from_nurb_parameters()?;

        rtn
    }

    /// The point and partial derivatives at `uv` straight from the NURB, without the
    /// zero-derivative corrections of `Evaluate` (`EvaluateSimple`).
    ///
    /// Returns `D[i][j] = d^(i+j) S / du^i dv^j` for `i <= highest_u_deriv`,
    /// `j <= highest_v_deriv`; with `only_upper_half` only `i + j <= highest_u_deriv`
    /// is computed when both orders are equal (the rest is zero). Unless the surface is
    /// in power-basis form ([`BSplineSurfaceForm::Polynomial`]), `uv` is clamped to the
    /// natural domain, and `u_from_left` / `v_from_left` pick the span at a knot.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an order above `NL_MAXDEG` (17); `SM_ERR` without NURB or
    /// when NLib fails (e.g. an order above `NL_MAXDER`).
    pub fn evaluate_simple(
        &self,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        u_from_left: bool,
        v_from_left: bool,
        only_upper_half: bool,
    ) -> Result<Vec<Vec<Vec3>>> {
        const OP: &str = OP_EVAL_SIMPLE;
        // check input
        if highest_u_deriv > MAX_DEGREE || highest_v_deriv > MAX_DEGREE {
            return Err(invalid(OP, "derivative order above NL_MAXDEG"));
        }

        let mut s_uv = uv;
        if self.form != BSplineSurfaceForm::Polynomial {
            let natural_uv_domain = self.natural_uv_domain();
            if !natural_uv_domain.contains_point(s_uv, 0.0) {
                s_uv = natural_uv_domain.clamp_point(s_uv);
            }
        }

        let uflg = if u_from_left { Side::Left } else { Side::Right };
        let vflg = if v_from_left { Side::Left } else { Side::Right };

        let sur = surface_nurb(self, OP)?;
        let sd = if self.form == BSplineSurfaceForm::Polynomial {
            srf_power_basis_eval_derivs(
                &sur.cpts(),
                sur.p(),
                sur.q(),
                s_uv.x,
                s_uv.y,
                only_upper_half,
                highest_u_deriv,
                highest_v_deriv,
            )
        } else {
            sur.derivs(
                s_uv.x,
                s_uv.y,
                uflg,
                vflg,
                only_upper_half,
                highest_u_deriv,
                highest_v_deriv,
            )
        }
        .map_err(SolidError::nl_ser(OP))?;
        Ok(sd)
    }

    /// [`BSplineSurface::evaluate_simple`] with first derivatives
    /// (`highest_u_deriv = highest_v_deriv = 1`) into the caller's 2x2 block `d`
    /// (every entry written), without a heap allocation per call for a non-rational,
    /// non-polynomial surface ([`Surface::first_derivs_into`]).
    ///
    /// # Errors
    ///
    /// As [`BSplineSurface::evaluate_simple`].
    pub fn evaluate_simple_first_into(
        &self,
        uv: Point2,
        u_from_left: bool,
        v_from_left: bool,
        only_upper_half: bool,
        d: &mut [[Vec3; 2]; 2],
    ) -> Result<()> {
        const OP: &str = OP_EVAL_SIMPLE;
        if self.form == BSplineSurfaceForm::Polynomial {
            let full = self.evaluate_simple(uv, 1, 1, u_from_left, v_from_left, only_upper_half)?;
            *d = [[Vec3::ZERO; 2]; 2];
            for (out, row) in d.iter_mut().zip(&full) {
                for (o, x) in out.iter_mut().zip(row) {
                    *o = *x;
                }
            }
            return Ok(());
        }

        let mut s_uv = uv;
        let natural_uv_domain = self.natural_uv_domain();
        if !natural_uv_domain.contains_point(s_uv, 0.0) {
            s_uv = natural_uv_domain.clamp_point(s_uv);
        }

        let uflg = if u_from_left { Side::Left } else { Side::Right };
        let vflg = if v_from_left { Side::Left } else { Side::Right };
        let sur = surface_nurb(self, OP)?;
        sur.first_derivs_into(s_uv.x, s_uv.y, uflg, vflg, only_upper_half, d)
            .map_err(SolidError::nl_ser(OP))
    }

    /// The point at `uv` (`EvaluatePoint`): `N_SrfEvalPt` from the left at knots, or the
    /// power-basis evaluation for [`BSplineSurfaceForm::Polynomial`]. `uv` is not
    /// clamped (upstream's clamping is disabled on purpose).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB or when NLib fails.
    pub fn evaluate_point(&self, uv: Point2) -> Result<Point3> {
        const OP: &str = OP_EVAL_POINT;
        let sur = surface_nurb(self, OP)?;
        let flg = Side::Left;
        let c = if self.form == BSplineSurfaceForm::Polynomial {
            power_basis_srf_eval_pts(&sur.cpts(), sur.n(), sur.m(), uv.x, uv.y)
        } else {
            sur.eval(uv.x, uv.y, flg, flg)
        };
        c.map_err(SolidError::nl_ser(OP))
    }
}

/// One side of `CreateExtendedSurface`: extends `esur` across `end` of `dir` by
/// `ext_dist`, halving the distance after each failure (`SM_ERR` below
/// `dist / 100`), checks validity, then repeats with doubled distances while the
/// distance is below `dist / 2` (at most [`MAX_EXTEND_DOUBLINGS`] times).
fn extend_side(
    esur: &mut Surface,
    dist: f64,
    ext_dist: &mut f64,
    dir: Direction,
    end: CurveEnd,
    gflg: SrfContinuity,
) -> Result<()> {
    const OP: &str = OP_EXTEND;

    // extend Surf - shorter dists until extension succeeds (in place: a failed
    // extension can leave its extended surface in esur, which the retry extends)
    let mut halvings = 0usize;
    loop {
        match extend_by_dist_in_place(esur, *ext_dist, dir, end, gflg) {
            Ok(()) => break,
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(_) => {
                *ext_dist /= 2.0;
                if *ext_dist < dist / 100.0 {
                    return Err(SolidError::sm_err(OP));
                }
                // upstream loops forever when dist is 0 or NaN
                halvings += 1;
                if halvings > MAX_EXTEND_HALVINGS {
                    return Err(SolidError::NotConverged {
                        operation: OP,
                        iterations: u32::try_from(MAX_EXTEND_HALVINGS).unwrap_or(u32::MAX),
                    });
                }
            }
        }
    }

    // quit - when extended surface is no good
    if esur.is_valid().is_err() {
        return Err(SolidError::sm_err(OP));
    }

    // if needed - multiple extensions until Extension gets to be near requested size
    let mut doublings = 0usize;
    while *ext_dist < dist / 2.0 {
        // upstream loops forever for a negative distance (HANG-16)
        doublings += 1;
        if doublings > MAX_EXTEND_DOUBLINGS {
            return Err(SolidError::NotConverged {
                operation: OP,
                iterations: u32::try_from(MAX_EXTEND_DOUBLINGS).unwrap_or(u32::MAX),
            });
        }
        *ext_dist *= 2.0;
        extend_by_dist_in_place(esur, *ext_dist, dir, end, gflg).map_err(SolidError::nl_ser(OP))?;
        if esur.is_valid().is_err() {
            return Err(SolidError::sm_err(OP));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SM_ERR_NULL_POINTER;
    use crate::nurbs::math::NOW;

    fn plane_surface() -> BSplineSurface {
        let net = (0..3)
            .map(|i| {
                (0..3)
                    .map(|j| CPoint::new(f64::from(i), f64::from(j), 0.1 * f64::from(i * j), NOW))
                    .collect()
            })
            .collect();
        let sur = Surface::new(
            net,
            2,
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .unwrap();
        BSplineSurface::from_nurb(sur, false)
    }

    #[test]
    fn make_knots_greville() {
        let u = [0.0, 0.1, 0.5, 0.6, 1.0];
        let mut k = [0.0; 9];
        make_knots(&u, 4, 3, &mut k).unwrap();
        assert_eq!(k[0], 0.0);
        assert_eq!(k[8], 1.0);
        // (0.1 + 0.5 + 0.6) / 3 = 0.4 -> snapped to 0.5 (forward < size / 3)
        assert_eq!(k[4], 0.5);
        assert!(make_knots(&u, 4, 0, &mut k).is_err());
    }

    #[test]
    fn trim_boundaries() {
        let knots = [0.0, 0.0, 0.25, 0.5, 0.75, 1.0, 1.0];
        let dom = Extent2d::from_bounds(0.3, 0.0, 0.6, 1.0);
        let (a, b) =
            check_trim_boundaries(&knots, 0.4, Some(3), 0.55, Some(3), &dom, SurfParamType::U)
                .unwrap();
        assert_eq!((a, b), (0.25, 0.75));
        assert!(
            check_trim_boundaries(&knots, 0.4, Some(9), 0.55, None, &dom, SurfParamType::U)
                .is_err()
        );
    }

    #[test]
    fn zero_derivs() {
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut su = Vec3::new(0.0, 0.0, 0.0);
        let mut sv = Vec3::new(0.0, 1.0, 0.0);
        let ok = check_zero_surface_derivs(
            Point2::new(0.2, 0.9),
            &dom,
            &mut su,
            &mut sv,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            1.0e-6,
        );
        assert!(ok);
        assert!(su.x < 0.0); // flipped: v above the middle
    }

    #[test]
    fn evaluate_and_reverse() {
        let mut s = plane_surface();
        let p = s.evaluate_point(Point2::new(0.25, 0.5)).unwrap();
        let d = s
            .evaluate_simple(Point2::new(0.25, 0.5), 1, 1, true, true, false)
            .unwrap();
        assert_eq!(d[0][0], p);
        s.reverse(SurfParamType::U).unwrap();
        let q = s.evaluate_point(Point2::new(0.75, 0.5)).unwrap();
        assert!((p - q).length() < 1e-12);
        s.reparameterize(&Extent2d::from_bounds(0.0, 0.0, 2.0, 2.0))
            .unwrap();
        assert_eq!(s.natural_uv_domain().max().x, 2.0);
    }

    #[test]
    fn extended_surface_runs() {
        let k = &Kernel::default();
        let s = plane_surface();
        let r = s.create_extended_surface(k, 0.1, ContinuityType::G1);
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn missing_nurb() {
        let s = BSplineSurface::new();
        assert!(matches!(
            s.approximate_offset_surface(0.1, 0.01, OffsetParamType::Inherited),
            Err(SolidError::Upstream {
                code: SM_ERR_NULL_POINTER,
                ..
            })
        ));
    }

    #[test]
    fn negative_extension_distance_stops() {
        // upstream doubles a negative distance towards -inf forever (HANG-16)
        let k = &Kernel::default();
        let r = plane_surface().create_extended_surface(k, -5.0, ContinuityType::G1R);
        assert!(matches!(r, Err(SolidError::NotConverged { .. })), "{r:?}");
        // a positive distance is unaffected
        assert!(plane_surface()
            .create_extended_surface(k, 0.5, ContinuityType::G1R)
            .is_ok());
    }

    #[test]
    fn make_knots_absurd_sizes_are_errors() {
        // fuzz regression: n + p + 2 overflowed
        let u = [0.0, 0.5, 1.0];
        let mut k = [0.0; 6];
        assert!(make_knots(&u, 2, usize::MAX, &mut k).is_err());
        assert!(make_knots(&u, 2, 1, &mut k).is_ok());
    }
}
