//! `SmEdgeuse`, part 3: UV trim curve creation (`CreateUVTrimCurve`,
//! `GetOrCreateUVTrimCurve`), the binormal of an edge use (`EvaluateBinormal`) and the
//! normalised evaluation (`NormalizedEvaluate`).
//!
//! Upstream: `source/SMLib/src/SmEdgeuse.cpp`, `source/SMLib/inc/SmEdgeuse.h`.
//!
//! | Rust                                              | Upstream                                  |
//! |---------------------------------------------------|-------------------------------------------|
//! | [`create_uv_trim_curve`], [`CreatedUvTrimCurve`]  | `SmEdgeuse::CreateUVTrimCurve`            |
//! | [`get_or_create_uv_trim_curve`], [`UvTrimCurveFetch`] | `SmEdgeuse::GetOrCreateUVTrimCurve` (both overloads) |
//! | [`evaluate_binormal`], [`BinormalOptions`]        | `SmEdgeuse::EvaluateBinormal`             |
//! | [`normalized_evaluate`], [`NormalizedEval`]       | `SmEdgeuse::NormalizedEvaluate`           |
//!
//! Deviations:
//! - **Object model.** As [`super::p1`]: an edge use is a record in the [`Brep`]'s
//!   arenas and the methods are free functions taking the B-rep and its id. Upstream
//!   casts the `const` away to attach UV trim curves and to edit the edge, so these
//!   functions take `&mut Brep` (and the kernel, which the surface evaluators need).
//!   `GetOrCreateUVTrimCurve` returns what it did; the curve is
//!   [`super::p1::uv_trim_curve`] afterwards.
//! - **Null pointers.** Where upstream dereferences a null pointer (an edge use
//!   without face use, face, surface, edge, edge curve, primary edge use or start
//!   vertex) the port returns [`SolidError::InvalidInput`]; `NER` is
//!   [`SolidError::null_pointer`], `SER` the callee's error.
//! - **Surface owner.** `CreateUVTrimCurve` clears the surface's owner
//!   (`SmTemporaryChangeValue`) for the rest of the call, so the solvers do not trim
//!   to the face; the port works on a copy of the face's surface whose owner is
//!   cleared, and clears the owner of the B-rep's face surface too, recording the
//!   scope on the B-rep ([`Brep::begin_surface_owner_cleared`]) so that a nested
//!   `smsurf_GetSurfaceCache` of that surface (`EvaluateBinormal`'s, the seam test's)
//!   finds no face and does not run `SmFace::CreateUVTrimCurves`, as upstream.
//!   `EvaluateBinormal` clears it only around `GlobalPointSolve` and
//!   `IsOnSeam` (a copy there); every other surface call uses the face's surface as it
//!   is. The surface cache flags `EvaluateBinormal` switches (`m_bPointTestEnabled`,
//!   `m_bProcessBoundaryCurves`) have nothing to act on because the Rust surface
//!   solvers build no surface cache. `EvaluateBinormal` first gets the surface cache of
//!   the face's surface (`smsurf_GetSurfaceCache`), whose construction creates the UV
//!   trim curves of the face's edge uses (`SmFace::CreateUVTrimCurves`): the port
//!   builds the face's trimmed surface cache over the B-rep
//!   ([`crate::query::face::p2::face_surface_cache`], the `&mut Brep` the function
//!   already takes) for that side effect and drops it. Upstream keeps the cache on the
//!   surface; the port has no object caches, so it is rebuilt on every call that
//!   reaches this point. The same
//!   side effect makes `CreateUVTrimCurve` clean up a UV trim curve the cache attached;
//!   the clean-up is kept.
//! - **Out-params.** `CreateUVTrimCurve` returns a [`CreatedUvTrimCurve`]; its optional
//!   `pdMaxDeviation` is `want_deviation`. `GetOrCreateUVTrimCurve` returns its status
//!   as [`UvTrimCurveFetch::status`] (upstream's status can be a failure while the
//!   curve pointer is valid, or null) and fails with `Err` only for [`SolidError::Unported`],
//!   [`SolidError::InvalidInput`] and [`SolidError::NotFound`]. `EvaluateBinormal`
//!   always computes the optional outputs, except the parameter direction
//!   ([`BinormalOptions::want_uv_vector`]), which is a separate surface evaluation.
//! - **Drop-curve records.** The surface and curve handles in the record are the
//!   face's and edge's ids (never dereferenced). `SM_SET_CREATE_UVTRIMCURVE_FAIL` is
//!   the record `Set` with the macro's arguments.
//! - **Statuses ignored upstream** (the surface and curve edits of
//!   `CreateUVTrimCurve`, `Trim` and `EditParameterization` aside, which are `SER`):
//!   `GetEnds`, `CalculateBoundingBox`, `ConvertTo2D`, `Transform`, `SetCanonical`,
//!   `Trim`, `EvaluatePoint` of the surface, `DropPoint` and the end evaluations of the
//!   two-singularity recovery, the `NormalizedEvaluate` of the seam test, the
//!   `GetOrCreateUVTrimCurve` and `Evaluate1stDerivatives`/`ComputeParamDirection` of
//!   `EvaluateBinormal`/`NormalizedEvaluate` are ignored; [`SolidError::Unported`]
//!   still propagates.
//! - **Upstream defects.** The two-singular-end-points recovery builds the joined UV
//!   curve into the temporary second-half array, which is destroyed unused, so it never
//!   produces a curve (`plan/upstream-issues.md` WR-158); the port builds and discards
//!   it as upstream does. `NormalizedEvaluate` returns `SM_SUCCESS` for a point it
//!   dropped to the surface even where upstream's trim curve was missing.
//! - **Drop curves.** `SmSurface::DropCurve` is wired (`intersect.surface_drop_lib_p3`;
//!   a plane runs its own override): the UV curves keep their kind (`Vec<Curve>`),
//!   and only the two-singular-end-points stitch takes their `SmBSplineCurve` parts.
//! - **Unported callees.** The paths of
//!   `SmSurface::DropPoint` / `LocalPointSolve` that reach `LocalPointSolve_1`
//!   (`surface.surface_point_solve_p3`) return [`SolidError::Unported`];
//!   `GlobalPointSolve` of a non-plane surface is `surface.surface_point_solve_p3`
//!   inside.
//! - **Recursion.** None: `EvaluateBinormal` calls `CreateUVTrimCurve` and
//!   `CreateUVTrimCurve` calls `EvaluateBinormal` only with `bUse3DOnly = TRUE`, so
//!   the depth is bounded by one level (as upstream comments).

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p1::vec_lin_comb_two_vectors;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::object::ObjectRef;
use crate::geom::solution_array::ObjectKind;
use crate::geom::tol::{approx_tol_3d, are_same, xsect_tol_3d_from_zones, SmallTopoType};
use crate::geom::vector2d::Vec2;
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::crv_fit::p1::ParamMethod;
use crate::surface::surface::p2::{CreateUvTrimCurvePathType, DropCurveFail, DropCurveFailArgs};
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::Surface;
use crate::topology::edgeuse::p1::{
    are_seam_curves, brep_handle, calc_dist_and_dev, delete_uv_trim_curve, drop_curve_fail, edge,
    faceuse, faceuse_face, max_edge_face_gap, orientation, primary_edgeuse, set_uv_trim_curve,
    tolerance, uv_trim_curve, uv_trim_curve_pointer, UvTrimCurveArg,
};
use crate::topology::edgeuse::p2::BinormalEval;
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, ObjectHandle};

const OP: &str = "topology::edgeuse::p3";

/// `SM_SUCCESS` (`SmMessages.h`).
const SM_SUCCESS: i32 = crate::intersect::surface_drop_lib::p1::SM_SUCCESS;

/// `SM_ERR_UNKNOWN`.
const SM_ERR_UNKNOWN: i32 = crate::surface::surface::p2::SM_ERR_UNKNOWN;

/// `UNSURE` of `SmBoolean`.
const UNSURE: i32 = crate::surface::surface::p2::UNSURE;

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// The status of `SmVector3d::AngleBetween` of a zero-length vector, which upstream
/// checks with `SER`: `SM_ERR_INVALID_INPUT` (1007), a failing status, not a null
/// dereference.
fn vec_status<T>(r: Result<T>) -> Result<T> {
    r.map_err(|e| match e {
        SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. } => {
            SolidError::Upstream {
                operation: OP,
                code: 1007, // SM_ERR_INVALID_INPUT
                entity: None,
            }
        }
        e => e,
    })
}

// ---- unported callees -------------------------------------------------------------------

/// What `SmSurface::DropCurve` produced.
#[derive(Debug)]
struct DropCurveOut {
    /// `rdMaxDropToSurf`.
    max_dist_to_surf: f64,
    /// `rdMaxApproxDev`.
    max_approx_dev: f64,
    /// `rUVCurves`: upstream's `SmBSplineCurve*` array, whose elements may be derived
    /// curves (a plane drops a line to an `SmLine`), so they keep their kind.
    uv_curves: Vec<Curve>,
}

/// `SmSurface::DropCurve(ctx, domain, curve, interval, tol, rdMaxDropToSurf,
/// rdMaxApproxDev, rUVCurves, bKeepAllDropCurves, pDropCurveFail)` (virtual, with the
/// default `bCreatingUVTrimCurve = TRUE`): the UV curves the curve over `interval`
/// drops to ([`Surface::drop_curve`]: a plane has its own override, which does not use
/// `pOptDropCurveFail` and passes `NULL` on to the general method; every other kind runs
/// the general one, `intersect.surface_drop_lib_p3`).
fn surface_drop_curve(
    k: &Kernel,
    surface: &Surface,
    domain: &crate::geom::extent2d::Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    approx_tol_3d: f64,
    keep_all_drop_curves: bool,
    record: Option<crate::intersect::surface_drop_lib::p1::DropFailRecord<'_, ObjectHandle>>,
) -> Result<DropCurveOut> {
    let d = surface.drop_curve::<ObjectHandle>(
        k,
        domain,
        curve,
        interval,
        approx_tol_3d,
        keep_all_drop_curves,
        record,
        true,
    )?;
    Ok(DropCurveOut {
        max_dist_to_surf: d.max_drop_to_surf,
        max_approx_dev: d.max_approx_dev,
        uv_curves: d.uv_curves,
    })
}

/// The `SmBSplineCurve` an element of a drop's curve array is (a B-spline, or the
/// B-spline part of a line).
fn b_spline_of(curve: Curve) -> Result<BSplineCurve> {
    match curve.into_kind() {
        CurveKind::BSpline(b) => Ok(b),
        CurveKind::Line(l) => Ok(l.base_with_nurb()?.into_owned()),
        _ => Err(SolidError::invalid_input(
            OP,
            "drop curve is not an SmBSplineCurve",
        )),
    }
}

/// `pSurface->GetSingularities() != SM_SS_NONE` (the virtual, without pole outputs; it
/// returns the singular sides as a bit set, there is no status to test).
fn surface_has_singularities(k: &Kernel, surface: &Surface) -> Result<bool> {
    Ok(surface.get_singularities(k, false, false)?.singular != 0)
}

/// The outputs of `SmSurface::DropPoint`.
#[derive(Debug, Clone, Copy)]
struct DropPointOut {
    /// `rbFoundAnswer`.
    found: bool,
    /// `rUV`.
    uv: Point2,
}

/// `pSurface->DropPoint(point, domain, NULL, bOk, uv, gap, bIsMulti)`
/// (`SM_SO_MINIMIZE`, no guess).
fn surface_drop_point(
    k: &Kernel,
    surface: &Surface,
    point: Point3,
    domain: &crate::geom::extent2d::Extent2d,
) -> Result<DropPointOut> {
    let r = surface.drop_point::<NoObject>(
        k,
        point,
        domain,
        &crate::surface::surface::p5::DropPointOptions::default(),
    )?;
    Ok(DropPointOut {
        found: r.success,
        uv: r.result_uv,
    })
}

/// `pSurface->IsSingularity(uv, eDir)` (the virtual: default tolerance `SM_EFF_ZERO`,
/// surface and point tests).
fn surface_is_singularity(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
) -> Result<(bool, SurfParamType)> {
    let r = surface.is_singularity(k, uv, EFF_ZERO, false)?;
    Ok((r.singular, r.direction))
}

/// `pSurface->LocalPointSolve(domain, SM_SO_MINIMIZE, point, guess, bFound, sSol)`:
/// whether an answer was found, its UV point and its solution value.
fn surface_local_point_solve(
    k: &Kernel,
    surface: &Surface,
    domain: &crate::geom::extent2d::Extent2d,
    point: Point3,
    guess: Point2,
) -> Result<(bool, Point2, f64)> {
    let r = surface.local_point_solve::<NoObject>(
        k,
        domain,
        SolverOperation::Minimize,
        point,
        guess,
    )?;
    Ok((
        r.found,
        Point2::new(r.solution.start.param(0)?, r.solution.start.param(1)?),
        r.solution.start.solution_value,
    ))
}

/// The status of a callee whose failure upstream tests (`lResult != SM_SUCCESS`):
/// `None` for any failing status, soft [`SolidError::InvalidInput`] and
/// [`SolidError::NotFound`] included (`DropCurve`, `DropAndTrimCurve` and
/// `CreateByJoining` report their bad arguments as statuses);
/// [`SolidError::Unported`] propagates.
fn status<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        Err(_) => Ok(None),
    }
}

/// `smsurf_GetSurfaceCache(pSurface)` of the surface of face `f` (`NER`), as
/// `EvaluateBinormal` calls it before clearing the surface's owner: the face's trimmed
/// surface cache, kept on the surface, whose construction (when the surface has none
/// yet) attaches a UV trim curve to every edge use of the face
/// (`SmFace::CreateUVTrimCurves`, `SmTrimSrfCache.cpp:209`), built over the B-rep
/// ([`crate::query::face::p2::face_surface_cache`]). The Rust surface solve that
/// follows builds its own cache; only the face's and the surface's state is kept.
fn face_surface_cache(k: &Kernel, b: &mut Brep, f: FaceId) -> Result<()> {
    crate::query::face::p2::note_face_surface_cache(k, b, f)
}

/// `pEdge->SetTolerance(tolerance)` (default arguments).
fn edge_set_tolerance(b: &mut Brep, e: EdgeId, tolerance: f64) -> Result<()> {
    crate::topology::edge::set_tolerance(b, e, tolerance, true, true)
}

// ---- CreateUVTrimCurve ------------------------------------------------------------------

/// The outputs of [`create_uv_trim_curve`].
#[derive(Debug, Clone)]
pub struct CreatedUvTrimCurve {
    /// `rpNewUVCurve`: the edge use's new UV trim curve (not yet attached).
    pub curve: Curve,
    /// `rdMaxDistToSurf`: `MaxDist(DropPt, 3dCurvePt)`, found by sampling; the whole
    /// error (`sqrt(dist^2 + dev^2)`) without `want_deviation`.
    pub max_dist_to_surf: f64,
    /// `*pdMaxDeviation`: `MaxDist(DropPt->SrfNormalLine, 3dCurvePt)` (`Some` with
    /// `want_deviation`).
    pub max_deviation: Option<f64>,
}

/// An `SmObject*` handle (never dereferenced) naming a face's surface or an edge's
/// curve in a drop-curve record.
fn handle(kind: ObjectKind, index: u32) -> ObjectHandle {
    ObjectHandle {
        kind,
        key: crate::geom::attribute::ObjectKey(u64::from(index) + 1),
    }
}

/// The length of the UV curve mapped through the surface (`SmCrvOnSurf::ApproximateLength`
/// over the curve's natural interval with 5 samples).
fn crv_on_surf_length(k: &Kernel, surface: &Surface, uv_curve: &Curve) -> Result<f64> {
    let cos = CrvOnSurf::new(
        k,
        uv_curve.clone_for_evaluation()?,
        surface.clone_for_evaluation()?,
        None,
        0,
    )?;
    let ivl = uv_curve.natural_interval();
    Ok(Curve::from(cos).approximate_length(k, &ivl, 5, false)?.0)
}

/// Builds the other seam's UV curves for a UV curve along a seam: a copy moved by
/// `delta` (`Transform` of an axis at `delta`) and a plain copy, both made 2D.
fn seam_curves(k: &Kernel, uv_curve: &Curve, delta: Vec3) -> Result<Vec<Curve>> {
    let mut other = uv_curve.clone();
    let mut axis = Axis2Placement::default();
    let _ = ignored(axis.set_canonical(delta, Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)))?;
    let _ = ignored(other.transform(k, &axis, None))?;
    let _ = ignored(other.convert_to_2d(k))?;

    let mut copy = uv_curve.clone();
    let _ = ignored(copy.convert_to_2d(k))?;
    Ok(vec![other, copy])
}

/// `CreateUVTrimCurve`: builds the UV trim curve of an edge use by dropping the edge's
/// curve onto the face's surface. Tries, in order: the UV curve of an edge curve that
/// is a curve on this surface (a copy; two copies for a seam); `DropCurve` and
/// `DropAndTrimCurve` at the edge tolerance and again at a larger one (the larger of
/// twice the tolerance and twice the largest edge/face gap, raising the edge
/// tolerance to the error found), with the singularity recoveries; then 101 points
/// dropped to the surface and interpolated. Two UV curves are reduced to one (seams:
/// by the binormal; crossings: by the 1% / 10% rules), the edge or the UV curve is
/// trimmed when their parameter ranges differ by less than the vertex intersection
/// tolerance. `want_deviation` is `pdMaxDeviation != NULL`; `opt_drop_curve_fail` is
/// `pOptDropCurveFail`.
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null; `SM_ERR` when every attempt fails (no sample point
/// drops); the cascading errors of the evaluators; [`SolidError::Unported`] (module
/// docs).
#[allow(clippy::too_many_lines)]
pub fn create_uv_trim_curve(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    want_deviation: bool,
    opt_drop_curve_fail: Option<&mut DropCurveFail<ObjectHandle>>,
) -> Result<CreatedUvTrimCurve> {
    // always collect DropCurveData
    let mut local_fail = DropCurveFail::<ObjectHandle>::default();
    let fail: &mut DropCurveFail<ObjectHandle> = match opt_drop_curve_fail {
        Some(f) => f,
        None => &mut local_fail,
    };

    // init output
    fail.un_init();
    fail.brep = brep_handle(b, eu)?;

    // locals
    let e = edge(b, eu)?.ok_or_else(|| null("CreateUVTrimCurve: edge use without edge"))?;
    let fu = faceuse(b, eu)?.ok_or_else(|| SolidError::null_pointer(OP))?;
    let f = faceuse_face(b, fu)?.ok_or_else(|| null("CreateUVTrimCurve: face use without face"))?;
    let face_rec = b.face(f)?;
    let face_surface = face_rec
        .surface
        .as_ref()
        .ok_or_else(|| null("CreateUVTrimCurve: face without surface"))?
        .clone_for_evaluation()?;
    let face_domain = face_rec.uv_domain;
    let edge_rec = b.edge(e)?;
    let mut curve = edge_rec
        .curve
        .as_ref()
        .ok_or_else(|| null("CreateUVTrimCurve: edge without curve"))?
        .clone_for_evaluation()?;
    // the drops build the edge curve's cache at the edge's tolerance
    // (SmTol::GetApproxTol3d(&m_crCurve) reads pEdge->GetTolerance() through the owner)
    curve.set_owner_zone_tol_3d(Some(edge_rec.tolerance));
    let mut edge_domain = edge_rec.interval;
    let uv_trim_at_start = uv_trim_curve(b, eu)?.0.is_some();
    let surface_handle = handle(ObjectKind::Surface, f.index());
    let curve_handle = handle(ObjectKind::Curve, e.index());

    // DropCurve output arguments.
    // If we have a CrvOnSurf for a seam, we will populate uv_curves directly and skip
    // DropCurve
    let mut uv_curves: Vec<Curve> = Vec::new();
    let mut max_dist_to_surf = 0.0_f64;
    let mut max_drop_approx_dev = 0.0_f64;
    let mut max_deviation: Option<f64> = None;
    let mut have_curves = false;

    if let CurveKind::CrvOnSurf(cos) = curve.kind() {
        let base = cos.surface();
        // `pSurface == base || *pSurface == *base` (identity is lost in the port: value
        // equality only) && same orientation as the primary edge use
        if face_surface.approx_eq(k, base)?
            && orientation(b, eu)?
                == orientation(
                    b,
                    primary_edgeuse(b, e)?
                        .ok_or_else(|| null("CreateUVTrimCurve: edge without primary edge use"))?,
                )?
        {
            let uv_curve = cos.uv_curve();
            let surface_uv = face_surface.natural_uv_domain_with_nurb(k)?;
            let tol = tolerance(b, eu)?;

            let (start_pt, _end_pt) = match ignored(uv_curve.ends(k))? {
                Some(ends) => (ends.start, ends.end),
                None => (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0)),
            };

            let uv_box = ignored(uv_curve.calculate_bounding_box(
                k,
                &uv_curve.natural_interval(),
                BoundingBoxRequest {
                    normal: true,
                    pseudo: false,
                    polar: false,
                },
                true,
            ))?
            .and_then(|r| r.normal)
            .unwrap_or_default();

            // Checking here to ensure that if this is a seam, we make sure to pick the
            // correct UV line. If this is not a line, then we're good. Then check to
            // see if we are along an edge of the domain for a periodic surface, aka
            // this edgeuse represents a seam.
            let u_length = uv_box.u_interval().length();
            let v_length = uv_box.v_interval().length();

            if u_length < tol
                && ((start_pt.x - surface_uv.u_min()).abs() < tol
                    || (start_pt.x - surface_uv.u_max()).abs() < tol)
                && face_surface.is_periodic(k, &surface_uv, SurfParamType::U)?
            {
                // Must be a seam with constant U, so build the other possible UV curve.
                let other_x = if (start_pt.x - surface_uv.u_min()).abs() < tol {
                    surface_uv.u_max()
                } else {
                    surface_uv.u_min()
                };
                let other_point = Vec3::new(other_x, start_pt.y, 0.0);
                uv_curves = seam_curves(k, uv_curve, other_point - start_pt)?;
                have_curves = true;
                max_dist_to_surf = 0.0;
                if want_deviation {
                    max_deviation = Some(0.0);
                }
            } else if v_length < tol
                && ((start_pt.y - surface_uv.v_min()).abs() < tol
                    || (start_pt.y - surface_uv.v_max()).abs() < tol)
                && face_surface.is_periodic(k, &surface_uv, SurfParamType::V)?
            {
                // Must be a seam with constant V, so build the other possible UV curve.
                let other_y = if (start_pt.y - surface_uv.v_min()).abs() < tol {
                    surface_uv.v_max()
                } else {
                    surface_uv.v_min()
                };
                let other_point = Vec3::new(start_pt.x, other_y, 0.0);
                uv_curves = seam_curves(k, uv_curve, other_point - start_pt)?;
                have_curves = true;
                max_dist_to_surf = 0.0;
                if want_deviation {
                    max_deviation = Some(0.0);
                }
            } else {
                // Not a seam, so just copy the curve
                let mut copy = uv_curve.clone();
                let _ = ignored(copy.convert_to_2d(k))?;
                // SM_SET_CREATE_UVTRIMCURVE_FAIL(SM_SUCCESS, SM_CTC_FROM_CRVONSURF)
                set_create_fail(
                    k,
                    fail,
                    CreateUvTrimCurvePathType::FromCrvOnSurf,
                    &face_surface,
                    &curve,
                    surface_handle,
                    curve_handle,
                );
                return Ok(CreatedUvTrimCurve {
                    curve: copy,
                    max_dist_to_surf: 0.0,
                    max_deviation: want_deviation.then_some(0.0),
                });
            }
        }
    }

    // Do not trim to Face boundaries: upstream clears the surface's owner for the rest
    // of the call (`SmTemporaryChangeValue`). The port works on a copy, and clears the
    // owner of the face's surface in the B-rep as well (recorded with
    // `begin_surface_owner_cleared`), so that the B-rep reads below
    // (`EvaluateBinormal`'s `smsurf_GetSurfaceCache`, the edge/face gap) see no face,
    // as upstream; it is restored on every return.
    let rescue = crate::topology::face::p2::RescueSurfaceOwner::new(
        b.face(f)?
            .surface
            .as_ref()
            .ok_or_else(|| null("CreateUVTrimCurve: face without surface"))?,
    );
    if let Some(s) = b.face_mut(f)?.surface.as_mut() {
        s.set_owner(None);
    }
    b.begin_surface_owner_cleared(f);
    let result = (|| -> Result<CreatedUvTrimCurve> {
        let mut surface = face_surface;
        let _ = surface.set_owner(None::<ObjectRef>);

        let mut zone_tol = b.edge(e)?.tolerance;

        // iter 0 = DropCurve(), 1 = DropAndTrimCurve() only if DropCurve() fails
        // iters 2 and 3 = same with bigger tolerance.
        // DropAndTrimCurve() will find curves that are just slightly too long,
        // extending just a bit across the surface boundary; DropCurve() returns
        // nothing in that case.
        for ii in 0..4_u32 {
            // drop curve to surface
            let mut drop_curve_called = false;
            if !have_curves {
                if ii == 2 {
                    // No success at original tolerance. Try a different tolerance based on
                    // gaps.
                    let gap = max_edge_face_gap(k, b, eu, false)?
                        .ok_or_else(|| null("CreateUVTrimCurve: edge use without gap"))?;

                    // if the gap size says to increase tolerances, try that
                    // else double the tolerance
                    if gap.gap_3d * 2.0 > zone_tol {
                        zone_tol = gap.gap_3d * 2.0;
                    } else {
                        zone_tol *= 2.0;
                    }
                }

                // drop curve to surface
                let mut l_ok;
                if ii == 0 || ii == 2 {
                    drop_curve_called = true;
                    // The status is tested below (`lResult`)
                    let r = status(surface_drop_curve(
                        k,
                        &surface,
                        &face_domain,
                        &curve,
                        &edge_domain,
                        zone_tol,
                        false,
                        Some(crate::intersect::surface_drop_lib::p1::DropFailRecord {
                            fail: &mut *fail,
                            surface: Some(surface_handle),
                            curve: Some(curve_handle),
                        }),
                    ))?;
                    l_ok = r.is_some();
                    if let Some(out) = r {
                        max_dist_to_surf = out.max_dist_to_surf;
                        max_drop_approx_dev = out.max_approx_dev;
                        uv_curves = out.uv_curves;
                    }
                } else {
                    // (ii==1 || ii==3) - Try DropAndTrimCurve branch
                    drop_curve_called = false;
                    let r = status(surface.drop_and_trim_curve::<ObjectHandle>(
                        k,
                        &face_domain,
                        &curve,
                        &edge_domain,
                        zone_tol,
                        false,
                        true,
                        Some(crate::intersect::surface_drop_lib::p1::DropFailRecord {
                            fail: &mut *fail,
                            surface: Some(surface_handle),
                            curve: Some(curve_handle),
                        }),
                    ))?;
                    l_ok = r.is_some();
                    if let Some(out) = r {
                        max_dist_to_surf = out.max_drop_to_surf;
                        max_drop_approx_dev = out.max_approx_dev;
                        uv_curves = out.uv_curves;
                    }
                    fail.brep = brep_handle(b, eu)?;
                }

                // when dropCurve fails and Surface has singularities - it's likely
                // DropCurve started at a surface singularity
                if (!l_ok || uv_curves.is_empty()) && surface_has_singularities(k, &surface)? {
                    // create a temp curve copy with reverse parameterizaton
                    let mut rev_curve = curve.clone();

                    // Have to trim this, otherwise the reversals mess things up. [081031]
                    // (may snap sIvl by tol to existing knots; status ignored)
                    let _ = ignored(rev_curve.trim(k, &mut edge_domain, TrimMode::Notify))?;
                    let new_ivl = rev_curve.reverse_parameterization(k, &edge_domain)?;

                    // drop reverse curve to surface (DropCurve's default keep-all TRUE)
                    let r = status(surface_drop_curve(
                        k,
                        &surface,
                        &face_domain,
                        &rev_curve,
                        &new_ivl,
                        zone_tol,
                        true,
                        None,
                    ))?;
                    l_ok = r.is_some();
                    if let Some(out) = r {
                        max_dist_to_surf = out.max_dist_to_surf;
                        max_drop_approx_dev = out.max_approx_dev;
                        uv_curves = out.uv_curves;
                    }

                    // when reverse curve dropping works
                    if l_ok && !uv_curves.is_empty() {
                        // reverse array of sUVCurves
                        let mut cur_ivl = new_ivl;
                        for c in &mut uv_curves {
                            let old = cur_ivl;
                            cur_ivl = c.reverse_parameterization(k, &old)?;
                        }
                        fail.create_path_type = CreateUvTrimCurvePathType::ReverseDropRecovery;
                        fail.brep = brep_handle(b, eu)?;
                    }

                    // when dropReverseCurve fails - it might be that both end points sit on
                    // singularities
                    if !l_ok {
                        l_ok = two_singular_end_points(
                            k,
                            b,
                            eu,
                            &surface,
                            &face_domain,
                            &curve,
                            &edge_domain,
                            zone_tol,
                            fail,
                            &mut max_dist_to_surf,
                            &mut max_drop_approx_dev,
                        )?;
                    }
                    let _ = l_ok;
                }
            }

            // If no UVTrimCurves are made, try next iteration.
            if uv_curves.is_empty() {
                continue;
            }

            // Check for the DropCurve methods returning two curves.
            // The only legitimate ways for this to happen are (1) the curve runs along a
            // seam, or (2) it runs across a seam, but only by less than tolerance. The
            // second case can happen (hopefully only) during read-in: other systems create
            // Faces that span seams on closed surfaces; we split such Faces at the seams
            // when reading in.
            let on_seam = {
                let refs: Vec<&Curve> = uv_curves.iter().collect();
                are_seam_curves(&refs, &edge_domain, drop_curve_called)
            };

            // First check for the second case:
            // If either curve is shorter than tolerance, remove it.
            if uv_curves.len() == 2 && !on_seam {
                let edge_tol = b.edge(e)?.tolerance;
                let start_v = crate::topology::edge::start_vertex(b, e)?
                    .ok_or_else(|| null("CreateUVTrimCurve: edge without start vertex"))?;
                let vtx1_tol = b.vertex(start_v)?.tolerance;
                let other_v = crate::topology::edge::other_vertex(b, e, Some(start_v))?;
                let vtx2_tol = match other_v {
                    Some(v) => b.vertex(v)?.tolerance,
                    None => vtx1_tol,
                };

                // Remove only if one is much smaller than the other. [B513]
                // Have a case where they're seam curves, but Face's tol is bigger than the
                // surface.
                let len1 = crv_on_surf_length(
                    k,
                    &surface,
                    uv_curves.first().ok_or_else(|| null("curves"))?,
                )?;
                let len2 = crv_on_surf_length(
                    k,
                    &surface,
                    uv_curves.get(1).ok_or_else(|| null("curves"))?,
                )?;

                if len1 < len2 / 10.0 && len1 < edge_tol + vtx1_tol {
                    uv_curves.remove(0);
                    fail.create_path_type = CreateUvTrimCurvePathType::ShortPieceSeamCrossRecovery;
                    fail.brep = brep_handle(b, eu)?;
                } else if len2 < len1 / 10.0 && len2 < edge_tol + vtx2_tol {
                    uv_curves.remove(1);
                    fail.create_path_type = CreateUvTrimCurvePathType::ShortPieceSeamCrossRecovery;
                    fail.brep = brep_handle(b, eu)?;
                }
            } // end first 2-curve check: short seam crossing

            // If still two curves, do the seam check: have to decide which to keep.
            if uv_curves.len() == 2 && on_seam {
                // Take care of seam curves by choosing the correct projected curve. This
                // should work for both continuous and discontinuous seam curves.
                // (upstream's "failed to choose one seam curve" test after each removal,
                // `size > 1`, is dead code: a removal leaves one curve. When the tangent
                // matches neither U nor V within two degrees both curves stay and fall
                // through to the 10% rule below, which keeps curve 0.)
                pick_seam_curve(
                    k,
                    b,
                    eu,
                    &surface,
                    &face_domain,
                    &edge_domain,
                    &mut uv_curves,
                    fail,
                )?;
            } // end 2 UVCurves check -- looking for seam curves

            // no curves with DropCurve() (possibly curve crossing seam) - try again with
            // DropAndTrimCurve() which splits curves across seams
            if uv_curves.is_empty() {
                continue;
            }

            // If we have more than one then we may have split them because they cross the
            // boundary. Try to find one that is within 10% of the size of the original one
            // and use it.
            //   GWC: this is not a good idea - this creates gaps between the Edge and the
            //   UVTrimCurve. But it's not the worst idea either, it does not create gaps
            //   between the Edge and its vertices - leave this healing in this method for
            //   now.
            if uv_curves.len() > 1 && k.fixes.uv_trim_joins_drop_pieces {
                // Not upstream (WR-543, source issue #1383; the fork's
                // source fork@e36feb1): keep the longest run of pieces that meet
                // end to end, joined, on the edge parameters it was dropped from:
                // stretching a piece onto the whole edge interval would put every trim
                // point off its edge point. A run shorter than the edge is then kept only
                // as any shorter trim below: when its ends are within tolerance of the
                // edge's vertices.
                longest_drop_run(k, &surface, zone_tol, &mut uv_curves)?;
                fail.create_path_type = CreateUvTrimCurvePathType::TenPercentSeamCrossRecovery;
                fail.brep = brep_handle(b, eu)?;
            } else if uv_curves.len() > 1 {
                let param_size = edge_domain.length();

                // for every found UVTrimCurve
                let mut chosen = None;
                for (kkk, c) in uv_curves.iter().enumerate() {
                    let uv_param_size = c.natural_interval().length();

                    // if this UVTrimCurve param range is at least 90% of orig param range
                    if (uv_param_size - param_size).abs() < 0.1 * param_size {
                        chosen = Some(kkk);
                        break;
                    }
                }
                if let Some(kkk) = chosen {
                    // remove all the other curves
                    fail.create_path_type = CreateUvTrimCurvePathType::TenPercentSeamCrossRecovery;
                    fail.brep = brep_handle(b, eu)?;
                    let mut keep = uv_curves.swap_remove(kkk);
                    uv_curves.clear();

                    // set remaining UVcurve's param range to equal original curve
                    keep.edit_parameterization(k, &edge_domain)?;

                    // set sUVCurves = this UVTrimCurve
                    uv_curves.push(keep);
                }

                // if we don't have 1 UVTrimCurve - try again with a different tolerance
                if uv_curves.len() != 1 {
                    uv_curves.clear();
                    continue;
                }
            } // end more than 1 UVTrimCurve due to splitting (not seams) check

            // State: sUVCurves.GetSize() == 1
            let uv_ivl = uv_curves
                .first()
                .ok_or_else(|| null("CreateUVTrimCurve: no UV curve"))?
                .natural_interval();

            // when single UVTrimCurve has same param range as edge - return success
            if are_same(uv_ivl.min(), edge_domain.min())
                && are_same(uv_ivl.max(), edge_domain.max())
            {
                let new_curve = uv_curves
                    .pop()
                    .ok_or_else(|| null("CreateUVTrimCurve: no UV curve"))?;
                if fail.create_path_type == CreateUvTrimCurvePathType::Unknown {
                    fail.create_path_type = CreateUvTrimCurvePathType::Okay;
                    fail.brep = brep_handle(b, eu)?;
                }

                // clean up Surface Cache side-effect of attaching 2nd UVTrimCurve
                if !uv_trim_at_start && uv_trim_curve(b, eu)?.0.is_some() {
                    delete_uv_trim_curve(b, eu)?; // TRUE = delete existing UVTrimCurve
                }

                // successfully constructed and selected a UVTrimCurve for this Edgeuse -
                // set output
                finish(
                    b,
                    e,
                    ii,
                    want_deviation,
                    &mut max_dist_to_surf,
                    &mut max_deviation,
                    max_drop_approx_dev,
                )?;
                return Ok(CreatedUvTrimCurve {
                    curve: new_curve,
                    max_dist_to_surf,
                    max_deviation,
                });
            }

            // single UVTrimCurve with shorter param range than edge

            // Don't edit the Edge unless we are keeping the UVCurve.
            let mut edit_uv_min = false;
            let mut edit_uv_max = false;

            // Get the 2D end points of the Drop Curve
            let (uv_start, uv_end) = match ignored(
                uv_curves
                    .first()
                    .ok_or_else(|| null("CreateUVTrimCurve: no UV curve"))?
                    .ends(k),
            )? {
                Some(ends) => (ends.start, ends.end),
                None => (Vec3::UNDEF, Vec3::UNDEF),
            };
            let uv_start_2d = Point2::new(uv_start.x, uv_start.y);
            let uv_end_2d = Point2::new(uv_end.x, uv_end.y);

            // Get the 3D end points of the Drop Curve
            let drop_curve_start_3d =
                ignored(surface.evaluate_point(k, uv_start_2d))?.unwrap_or(Vec3::UNDEF);
            let drop_curve_end_3d =
                ignored(surface.evaluate_point(k, uv_end_2d))?.unwrap_or(Vec3::UNDEF);

            // Get the local XSect Tolerances
            // (`SmTol::GetXSectTol3d(pEdge, pVertex)` takes the system zone tolerance for a
            // missing vertex; a missing vertex is dereferenced only where that end's
            // parameter differs)
            let (start_vertex, end_vertex) = crate::topology::edge::start_end_vertices(b, e)?;
            let edge_tol = b.edge(e)?.tolerance;
            let vertex_zone_tol = |v: Option<crate::topology::VertexId>| -> Result<f64> {
                match v {
                    Some(v) => Ok(b.vertex(v)?.tolerance),
                    None => Ok(crate::geom::tol::zone_tol_3d(k, SmallTopoType::NormalSize)),
                }
            };
            let start_xsect_tol =
                xsect_tol_3d_from_zones(k, edge_tol, vertex_zone_tol(start_vertex)?);
            let end_xsect_tol = xsect_tol_3d_from_zones(k, edge_tol, vertex_zone_tol(end_vertex)?);

            // when Edge/UVCurve min params differ
            if !are_same(uv_ivl.min(), edge_domain.min()) {
                let start_vertex = start_vertex
                    .ok_or_else(|| null("CreateUVTrimCurve: edge without start vertex"))?;
                if b.vertex(start_vertex)?.point.distance(drop_curve_start_3d) < start_xsect_tol {
                    edit_uv_min = true;
                } else {
                    // Edge StartParam differs by more than the intersection tolerance:
                    // curve may have stepped off the surface. Do not accept this uv curve.
                    uv_curves.clear();
                    continue;
                }
            }

            // when Edge/UVCurve max params differ
            if !are_same(uv_ivl.max(), edge_domain.max()) {
                let end_vertex =
                    end_vertex.ok_or_else(|| null("CreateUVTrimCurve: edge without end vertex"))?;
                if b.vertex(end_vertex)?.point.distance(drop_curve_end_3d) < end_xsect_tol {
                    edit_uv_max = true;
                } else {
                    // Curve may have stepped off the surface - try again with larger
                    // tolerance.
                    uv_curves.clear();
                    continue;
                }
            }

            // We are keeping the curve, edit as necessary
            if edit_uv_min {
                // Sometimes the uv curve can actually come back longer, due to noise. In
                // that case, trim the uv curve.
                if uv_ivl.min() < edge_domain.min() {
                    let mut new_ivl = Extent1d::from_min_max(edge_domain.min(), uv_ivl.max());
                    if let Some(c) = uv_curves.first_mut() {
                        // may snap sIvl by tol to existing knots
                        let _ = ignored(c.trim(k, &mut new_ivl, TrimMode::Notify))?;
                    }
                    fail.create_path_type =
                        CreateUvTrimCurvePathType::OnePercentUvTrimCurveTrimRecovery1;
                    fail.brep = brep_handle(b, eu)?;
                } else {
                    // change Edge parameter to match UVTrimCurve parameter
                    let mut new_ivl = Extent1d::from_min_max(uv_ivl.min(), edge_domain.max());
                    crate::topology::edge::set_interval(b, e, &new_ivl)?;
                    edge_domain = b.edge(e)?.interval;
                    fail.create_path_type = CreateUvTrimCurvePathType::OnePercentEdgeTrimRecovery1;
                    fail.brep = brep_handle(b, eu)?;

                    // Also have to update any other Edgeuses on this Edge.
                    trim_other_uv_curves(k, b, e, eu, &mut new_ivl)?;
                }
            }
            if edit_uv_max {
                // Sometimes the uv curve can actually come back longer, due to noise. In
                // that case, trim the uv curve.
                if uv_ivl.max() > edge_domain.max() {
                    let mut new_ivl = Extent1d::from_min_max(uv_ivl.min(), edge_domain.max());
                    if let Some(c) = uv_curves.first_mut() {
                        // may snap sIvl by tol to existing knots
                        let _ = ignored(c.trim(k, &mut new_ivl, TrimMode::Notify))?;
                    }
                    fail.create_path_type =
                        CreateUvTrimCurvePathType::OnePercentUvTrimCurveTrimRecovery2;
                    fail.brep = brep_handle(b, eu)?;
                } else {
                    // Change Edge parameter to match UVTrimCurve parameter.
                    let mut new_ivl = Extent1d::from_min_max(edge_domain.min(), uv_ivl.max());
                    crate::topology::edge::set_interval(b, e, &new_ivl)?;
                    // (sEdgeDomain = pEdge->GetInterval() is not read again upstream)
                    fail.create_path_type = CreateUvTrimCurvePathType::OnePercentEdgeTrimRecovery2;
                    fail.brep = brep_handle(b, eu)?;

                    // Also have to update any other Edgeuses on this Edge.
                    trim_other_uv_curves(k, b, e, eu, &mut new_ivl)?;
                }
            }

            // State: we have accepted the single UV Trim curve, and probably adjusted the
            // Edge's param range.
            let new_curve = uv_curves
                .pop()
                .ok_or_else(|| null("CreateUVTrimCurve: no UV curve"))?;

            // clean up Surface Cache side-effect of attaching 2nd UVTrimCurve
            if !uv_trim_at_start && uv_trim_curve(b, eu)?.0.is_some() {
                delete_uv_trim_curve(b, eu)?; // TRUE = delete existing UVTrimCurve
            }

            // successfully constructed and set a UVTrimCurve for this Edgeuse
            finish(
                b,
                e,
                ii,
                want_deviation,
                &mut max_dist_to_surf,
                &mut max_deviation,
                max_drop_approx_dev,
            )?;
            return Ok(CreatedUvTrimCurve {
                curve: new_curve,
                max_dist_to_surf,
                max_deviation,
            });
        } // end iters trying DropCurve and DropAndTrimCurve

        // If we make it to here all other attempts have failed to create a curve our last
        // attempt is to tessellate the curve and drop points.
        //
        // It works better to quit here, without doing the brute-force point-dropping
        // algorithm. It's often because the 3d curve runs across a seam, in an incomplete
        // model. In this case, this will create a uv curve that jumps wildly from one end
        // of the domain to the other, rendering it misleading and mostly useless. In such a
        // case, the caller can deal with the lack of a uv trim curve appropriately.
        // [B513 B556 B557] However, there are cases where this is necessary, so we have to
        // leave it in for now.
        brute_force(
            k,
            b,
            eu,
            e,
            &surface,
            &face_domain,
            &curve,
            &edge_domain,
            want_deviation,
            uv_trim_at_start,
            fail,
        )
    })();
    // the temporary change's destructor restores the owner on every return
    b.end_surface_owner_cleared(f);
    if let Some(s) = b.face_mut(f)?.surface.as_mut() {
        rescue.restore(s);
    }
    // the face's tolerance may have changed while the surface had no owner
    b.sync_face_surface_tolerance(f)?;
    result
}

/// `SM_SET_CREATE_UVTRIMCURVE_FAIL(SM_SUCCESS, path)`.
fn set_create_fail(
    k: &Kernel,
    fail: &mut DropCurveFail<ObjectHandle>,
    path: CreateUvTrimCurvePathType,
    surface: &Surface,
    curve: &Curve,
    surface_handle: ObjectHandle,
    curve_handle: ObjectHandle,
) {
    fail.set(&DropCurveFailArgs {
        fail_status: SM_SUCCESS,
        from_call: 3,
        create_path_type: path,
        has_context: true,
        // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
        uv_domain: surface.natural_uv_domain_if_built(),
        surface: Some(surface_handle),
        curve: Some(curve_handle),
        interval: curve.natural_interval(),
        approx_tol_3d: approx_tol_3d(k, SmallTopoType::NormalSize),
        max_drop_to_surf_arg: 0.0,
        max_approx_dev: 0.0,
        uv_curves_arg: None,
        keep_all_drop_curves: UNSURE,
        degen_curve: UNSURE,
        degen_surf_u: UNSURE,
        degen_surf_v: UNSURE,
        drop_domain_is_contained: UNSURE,
        max_drop_to_surf: 0.0,
        ..DropCurveFailArgs::default()
    });
}

/// The end of a successful `CreateUVTrimCurve`: sets the outputs and, when a larger
/// tolerance was needed (`ii > 1`), the edge tolerance from the actual gaps.
fn finish(
    b: &mut Brep,
    e: EdgeId,
    ii: u32,
    want_deviation: bool,
    max_dist_to_surf: &mut f64,
    max_deviation: &mut Option<f64>,
    max_drop_approx_dev: f64,
) -> Result<()> {
    if want_deviation {
        *max_deviation = Some(max_drop_approx_dev);
    } else {
        // Put the entire error into the one output.
        *max_dist_to_surf = crate::geom::vector3d::sqrt(
            *max_dist_to_surf * *max_dist_to_surf + max_drop_approx_dev * max_drop_approx_dev,
        );
    }

    // If we had to use a bigger tolerance, update the Edge's tolerance according to the
    // actual gaps.
    if ii > 1 {
        let error = if want_deviation {
            crate::geom::vector3d::sqrt(
                *max_dist_to_surf * *max_dist_to_surf + max_drop_approx_dev * max_drop_approx_dev,
            )
        } else {
            *max_dist_to_surf
        };
        edge_set_tolerance(b, e, error * 1.01)?;
    }
    Ok(())
}

/// Trims the UV trim curves of the edge's other edge uses to `new_ivl` (their statuses
/// are ignored; the snapped interval carries over from one to the next).
///
/// Upstream `SmBSplineCurve::Trim` reads `SmTol::GetZoneTol3d(this)`, which walks an
/// attached UV trim curve's `SmEdgeuse` owner to `e` and returns `pEdge->GetTolerance()`
/// (`SM_USE_OLDTOL`), rather than the system zone tolerance that owner-less
/// [`Curve::trim`] reads from `k` (as in `sm_TrimRail`, `blend::fillet_solver::p1`).
fn trim_other_uv_curves(
    k: &Kernel,
    b: &mut Brep,
    e: EdgeId,
    this: EdgeuseId,
    new_ivl: &mut Extent1d,
) -> Result<()> {
    let mut tk = Kernel::new(&crate::kernel::KernelOptions {
        tolerances: crate::kernel::Tolerances {
            zone_tol_3d: crate::geom::tol::zone_tol3d(k, crate::topology::edge::tolerance(b, e)?),
            ..k.tolerances
        },
        ..crate::kernel::KernelOptions::default()
    });
    tk.rng = k.rng.clone();
    tk.fixes.bspline_trim_preserves_uv_spans = k.fixes.bspline_trim_preserves_uv_spans;
    for x in crate::topology::edge::edgeuses(b, e, None, None)? {
        if x == this {
            continue;
        }
        if uv_trim_curve_pointer(b, x)?.is_none() {
            continue;
        }
        if let Some(c) = b.edgeuse_mut(x)?.uv_trim_curve.as_mut() {
            // may snap sIvl by tol to existing knots
            let _ = ignored(c.trim(&tk, new_ivl, TrimMode::Notify))?;
        }
    }
    Ok(())
}

/// The recovery for a curve whose both ends drop to singularities: drops the two halves
/// separately and joins them. Upstream leaves the joined curve in the array of the
/// second half, which is destroyed unused (module docs; `plan/upstream-issues.md`
/// WR-158): the result is always "no curves", with `lResult` success when the join worked. Returns `lResult`.
#[allow(clippy::too_many_arguments)]
fn two_singular_end_points(
    k: &Kernel,
    b: &Brep,
    eu: EdgeuseId,
    surface: &Surface,
    face_domain: &crate::geom::extent2d::Extent2d,
    curve: &Curve,
    edge_domain: &Extent1d,
    zone_tol: f64,
    fail: &mut DropCurveFail<ObjectHandle>,
    max_dist_to_surf: &mut f64,
    max_drop_approx_dev: &mut f64,
) -> Result<bool> {
    // see if both curve ends drop to singularities
    let mut start_singularity = false;
    let mut end_singularity = false;
    let surface_uv_domain = surface.natural_uv_domain_with_nurb(k)?;

    // Curve EndPoints
    let start_point = ignored(curve.evaluate_point(k, edge_domain.min()))?.unwrap_or(Vec3::UNDEF);
    let end_point = ignored(curve.evaluate_point(k, edge_domain.max()))?.unwrap_or(Vec3::UNDEF);

    // drop curve endpoints onto surface
    let start_drop = ignored(surface_drop_point(
        k,
        surface,
        start_point,
        &surface_uv_domain,
    ))?;
    if let Some(d) = start_drop.filter(|d| d.found) {
        start_singularity = surface_is_singularity(k, surface, d.uv)?.0;
        if start_singularity {
            let end_drop = ignored(surface_drop_point(
                k,
                surface,
                end_point,
                &surface_uv_domain,
            ))?;
            if let Some(d) = end_drop.filter(|d| d.found) {
                end_singularity = surface_is_singularity(k, surface, d.uv)?.0;
            }
        }
    }

    // if both ends map to singulartities
    if !(start_singularity && end_singularity) {
        return Ok(false);
    }

    // get the edge half domains
    let mut first_half = Extent1d::from_min_max(edge_domain.min(), edge_domain.evaluate(0.5));
    let mut second_half = Extent1d::from_min_max(edge_domain.evaluate(0.5), edge_domain.max());

    // create a pair of temp curve copies for each half of the target curve
    let mut first_curve = curve.clone();
    let mut second_curve = curve.clone();

    // trim curves to their half of the original curve (may snap sIvl by tol to
    // existing knots)
    let _ = ignored(first_curve.trim(k, &mut first_half, TrimMode::Notify))?;
    let _ = ignored(second_curve.trim(k, &mut second_half, TrimMode::Notify))?;

    // reverse the parameterization of the 1st half
    let new_interval = first_curve.reverse_parameterization(k, &first_half)?;

    // drop the 1st half curve to surface
    let first = status(surface_drop_curve(
        k,
        surface,
        face_domain,
        &first_curve,
        &new_interval,
        zone_tol,
        true,
        None,
    ))?;
    let mut l_result = false;
    // when 1st half dropped successfully - carry on
    if let Some(mut first_out) = first {
        *max_dist_to_surf = first_out.max_dist_to_surf;
        *max_drop_approx_dev = first_out.max_approx_dev;
        if first_out.uv_curves.len() == 1 {
            // drop the 2nd half curve to the surface
            let second = status(surface_drop_curve(
                k,
                surface,
                face_domain,
                &second_curve,
                &second_half,
                zone_tol,
                true,
                None,
            ))?;
            // when 2nd half dropped successfully - carry on
            if let Some(mut second_out) = second {
                *max_dist_to_surf = second_out.max_dist_to_surf;
                *max_drop_approx_dev = second_out.max_approx_dev;
                if second_out.uv_curves.len() == 1 {
                    // restore parameterization of 1stHalf drop curve
                    if let Some(c) = first_out.uv_curves.first_mut() {
                        let old = new_interval;
                        c.reverse_parameterization(k, &old)?;
                    }

                    // stitch the two halves together
                    let second_curve_out = second_out
                        .uv_curves
                        .pop()
                        .ok_or_else(|| null("two singular end points: no second curve"))?;
                    first_out.uv_curves.push(second_curve_out);
                    let mut halves = first_out
                        .uv_curves
                        .into_iter()
                        .map(b_spline_of)
                        .collect::<Result<Vec<BSplineCurve>>>()?;
                    let joined = status(BSplineCurve::create_by_joining(&mut halves, None))?;

                    // when stitching worked (the joined curve is dropped with the
                    // second half's array: upstream defect WR-158, module docs)
                    if joined.is_some() {
                        l_result = true;
                        fail.create_path_type =
                            CreateUvTrimCurvePathType::TwoSingularEndPtsRecovery;
                        fail.brep = brep_handle(b, eu)?;
                    }
                }
            }
        }
    }
    Ok(l_result)
}

/// The seam choice of `CreateUVTrimCurve` for two UV curves along either side of a
/// seam: keeps the one whose side the binormal points to. Where the edge tangent lines
/// up with neither the U nor the V derivative within two degrees both curves remain
/// (the caller falls through to the 10% rule).
#[allow(clippy::too_many_arguments)]
fn pick_seam_curve(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    surface: &Surface,
    face_domain: &crate::geom::extent2d::Extent2d,
    edge_domain: &Extent1d,
    uv_curves: &mut Vec<Curve>,
    fail: &mut DropCurveFail<ObjectHandle>,
) -> Result<()> {
    // Get this edgeuse's Position (3d) near edge's midPoint (status ignored).
    let mid_param = if orientation(b, eu)? == OrientType::Same {
        0.48765
    } else {
        1.0 - 0.48765
    };
    let _ = ignored(normalized_evaluate(k, b, eu, mid_param, false, false, true))?;

    // evaluate NearMid 1st UVCurve Point clamped to face domain (for seams: coincident
    // to sMidPoint, for crossing curves: not). Use the uv curve's domain, not the
    // Edge's. On a seam, it's the same as the Edge's, but if crossing, we need the
    // curve's own midpoint; Edge midpoint might not even be within its own domain.
    // [091030]
    let (uv_pnt1, uv_pnt2) = {
        let c1 = uv_curves.first().ok_or_else(|| null("seam: no curve"))?;
        let c2 = uv_curves
            .get(1)
            .ok_or_else(|| null("seam: no second curve"))?;
        (
            c1.evaluate_point(k, c1.natural_interval().evaluate(0.48765))?,
            c2.evaluate_point(k, c2.natural_interval().evaluate(0.48765))?,
        )
    };
    let uv1 = face_domain.clamp_point(Point2::new(uv_pnt1.x, uv_pnt1.y));
    let uv2 = face_domain.clamp_point(Point2::new(uv_pnt2.x, uv_pnt2.y));

    // project NearMid UVCurve Points to 3Space through Edgeuse->Face->Surface
    let (_p1, du1, dv1) = surface.evaluate_1st_derivatives(k, uv1, true, true)?;
    let (_p2, du2, dv2) = surface.evaluate_1st_derivatives(k, uv2, true, true)?;

    // When there are still 2 UVTrimCurves expect to have 2 UVCurves running down either
    // side of a seam curve. Find the right one for this Edgeuse.
    // bWorkIn3d = TRUE: proj Edge pts to Surface, FALSE: use pUVTrimCurve to calc
    // Surface points. We can't use UVTrimCurves here, because that's what we're
    // creating.
    let mid = evaluate_binormal(
        k,
        b,
        eu,
        edge_domain.evaluate(0.48765),
        true,
        &BinormalOptions::default(),
    )?;
    let (binormal, tangent) = (mid.binormal, mid.tangent);

    // Get angles between Binormal and the facePoints' U and V derivs.
    let ang_bin_u1 = vec_status(du1.angle_between(binormal))?;
    let ang_bin_u2 = vec_status(du2.angle_between(binormal))?;
    let (max_ang_bin_u, min_ang_bin_u) = if ang_bin_u1 > ang_bin_u2 {
        (ang_bin_u1, ang_bin_u2)
    } else {
        (ang_bin_u2, ang_bin_u1)
    };

    let ang_bin_v1 = vec_status(dv1.angle_between(binormal))?;
    let ang_bin_v2 = vec_status(dv2.angle_between(binormal))?;
    let (max_ang_bin_v, min_ang_bin_v) = if ang_bin_v1 > ang_bin_v2 {
        (ang_bin_v1, ang_bin_v2)
    } else {
        (ang_bin_v2, ang_bin_v1)
    };

    // get angles between tangent and the facePoints' U and V tangents
    let mut ang_tan_u1 = vec_status(du1.angle_between(tangent))?;
    let mut ang_tan_v1 = vec_status(dv1.angle_between(tangent))?;
    let mut ang_tan_u2 = vec_status(du2.angle_between(tangent))?;
    let mut ang_tan_v2 = vec_status(dv2.angle_between(tangent))?;

    // The angles with the edge-curve tangent are used only for checking which
    // derivative lines up with the edge. For that, we don't care parallal or
    // anti-parallel, so mirror them about 90 degrees so that 180 degrees maps to 0.
    for a in [
        &mut ang_tan_u1,
        &mut ang_tan_v1,
        &mut ang_tan_u2,
        &mut ang_tan_v2,
    ] {
        if *a > PI / 2.0 {
            *a = PI - *a;
        }
    }

    // The cross-boundary derivative points into the face at the low end of the domain,
    // and out of the face at the high end. The binormal always points into the face. So
    // check whether the CBD and the binormal are in the same or opposite directions.
    // The CBD is the one that does not line up with the curve tangent. We check that
    // here, within two degrees.
    let two_deg = 2.0 * PI / 180.0;

    if ang_tan_v1 < two_deg || ang_tan_v2 < two_deg {
        // V lines up, so U is the cross-boundary deriv (if curve is a seam).
        let keep_first = (PI - max_ang_bin_u < min_ang_bin_u && uv_pnt1.x > uv_pnt2.x) // keep max curve and max curve is 1st
            || (PI - max_ang_bin_u > min_ang_bin_u && uv_pnt1.x < uv_pnt2.x); // keep min curve and min curve is 1st
        remove_seam_curve(b, eu, uv_curves, fail, keep_first)?;
    } else if ang_tan_u1 < two_deg || ang_tan_u2 < two_deg {
        // U lines up, so V is the cross-boundary deriv (if curve is a seam).
        let keep_first = (PI - max_ang_bin_v < min_ang_bin_v && uv_pnt1.y > uv_pnt2.y) // keep max curve and max curve is 1st
            || (PI - max_ang_bin_v > min_ang_bin_v && uv_pnt1.y < uv_pnt2.y); // keep min curve and min curve is 1st
        remove_seam_curve(b, eu, uv_curves, fail, keep_first)?;
    }
    Ok(())
}

/// Removes the 2nd (`keep_first`) or the 1st UV curve of a seam pair.
fn remove_seam_curve(
    b: &Brep,
    eu: EdgeuseId,
    uv_curves: &mut Vec<Curve>,
    fail: &mut DropCurveFail<ObjectHandle>,
    keep_first: bool,
) -> Result<()> {
    if keep_first {
        // remove 2nd UVTrimCurve
        uv_curves.remove(1);
    } else {
        // remove 1st UVTrimCurve
        uv_curves.remove(0);
    }
    fail.create_path_type = CreateUvTrimCurvePathType::PickOneSeamRecovery;
    fail.brep = brep_handle(b, eu)?;
    Ok(())
}

/// The last attempt of `CreateUVTrimCurve`: drops 101 points of the edge curve to the
/// surface and interpolates the sample points that are far enough apart.
#[allow(clippy::too_many_arguments)]
fn brute_force(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    e: EdgeId,
    surface: &Surface,
    face_domain: &crate::geom::extent2d::Extent2d,
    curve: &Curve,
    edge_domain: &Extent1d,
    want_deviation: bool,
    uv_trim_at_start: bool,
    fail: &mut DropCurveFail<ObjectHandle>,
) -> Result<CreatedUvTrimCurve> {
    let mut max_dist = 0.0_f64;
    let mut max_dev = 0.0_f64;

    // locals for GlobalPointSolve
    let mut points: Vec<Point3> = Vec::new();
    let mut params: Vec<f64> = Vec::new();
    let uv_tol = EFF_ZERO_SQRT * (face_domain.size().length() + 1.0);
    let edge_tol = b.edge(e)?.tolerance;

    // for 101 points
    for ii in 0..=100_u32 {
        // evaluate the curve at evenly spaced parameter points
        let param = edge_domain.evaluate(f64::from(ii) / 100.0);
        let pv = curve.evaluate(k, param, 1, true, true)?;
        let p0 = *pv.first().ok_or_else(|| null("brute force: no point"))?;

        // find closest point on surface to sample point
        let solutions = surface.global_point_solve::<NoObject>(
            k,
            Some(NoObject),
            face_domain,
            SolverOperation::Minimize,
            p0,
            edge_tol,
            None,
            SolutionRequested::All,
        )?;

        // when a closest point was found
        if let Some(sol) = solutions.as_slice().first() {
            // get the surface UV Point from the solution array
            let pnt = Point3::new(sol.start.param(0)?, sol.start.param(1)?, 0.0);

            // save the first sample Point and subsequent ones that are far enough apart
            if points.last().is_none_or(|l| pnt.distance(*l) > uv_tol) {
                points.push(pnt);
                params.push(param);

                // (the solution's own first object, as `sm_CalcDistAndDev` reads it)
                let (dist, dev) = calc_dist_and_dev(k, p0, sol, |_| Some(surface), want_deviation)?;
                max_dist = max_dist.max(dist);
                // (Note, dMaxDev is 0 if ptr is Null.)
                max_dev = max_dev.max(dev.unwrap_or(0.0));
            }
        }
    }

    let set_outputs = |max_dist: f64, max_dev: f64| -> (f64, Option<f64>) {
        // Set these anyway, could be useful to the caller.
        if want_deviation {
            (max_dist, Some(max_dev))
        } else {
            (
                crate::geom::vector3d::sqrt(max_dist * max_dist + max_dev * max_dev),
                None,
            )
        }
    };

    // when no sample points projected to the surface
    if params.is_empty() {
        return Err(SolidError::sm_err(OP));
    }

    // when just 1 sample point projected to the surface
    if params.len() == 1 {
        // create and use a degenerate curve as the UVTrimCurve
        let first = *points
            .first()
            .ok_or_else(|| null("brute force: no point"))?;
        let degen = BSplineCurve::create_degenerate_curve(2, first)?;
        fail.create_path_type = CreateUvTrimCurvePathType::PointDropDegenCurveRecovery;
        fail.brep = brep_handle(b, eu)?;

        // clean up Surface Cache side-effect of attaching 2nd UVTrimCurve
        if !uv_trim_at_start && uv_trim_curve(b, eu)?.0.is_some() {
            delete_uv_trim_curve(b, eu)?;
        }

        // successfully constructed a UVTrimCurve
        let (d, dv) = set_outputs(max_dist, max_dev);
        return Ok(CreatedUvTrimCurve {
            curve: Curve::from(degen),
            max_dist_to_surf: d,
            max_deviation: dv,
        });
    }

    // else have several sample points - construct UVTrimCurve to interpolate sample
    // points
    fail.create_path_type = CreateUvTrimCurvePathType::PointDropDegenCurveRecovery;
    fail.brep = brep_handle(b, eu)?;

    let mut new_curve = BSplineCurve::interpolate_points(
        &points,
        Some(&params),
        3, // in : degree
        None,
        None,
        false,
        ParamMethod::ChordLength,
    )?;
    let _ = ignored(new_curve.convert_to_2d())?;

    // clean up Surface Cache side-effect of attaching 2nd UVTrimCurve
    if !uv_trim_at_start && uv_trim_curve(b, eu)?.0.is_some() {
        delete_uv_trim_curve(b, eu)?;
    }

    // successfully constructed a UVTrimCurve
    let (d, dv) = set_outputs(max_dist, max_dev);
    Ok(CreatedUvTrimCurve {
        curve: Curve::from(new_curve),
        max_dist_to_surf: d,
        max_deviation: dv,
    })
}

// ---- GetOrCreateUVTrimCurve -------------------------------------------------------------

/// The outputs of [`get_or_create_uv_trim_curve`].
#[derive(Debug, Clone)]
pub struct UvTrimCurveFetch {
    /// The returned status: `Ok` when the curve is newly built or a well made existing
    /// one; the failing status when `CreateUVTrimCurve` failed (no curve attached) or
    /// the existing curve's record holds one (the curve is attached).
    pub status: std::result::Result<(), SolidError>,
    /// `*pdOptDistToSurf`: the new curve's `MaxDist(DropPt, 3dCurvePt)`; for an old
    /// curve the largest edge/face gap when it is cached, else `-1`.
    pub dist_to_surf: f64,
    /// `*pdOptDeviation`: the new curve's deviation; `-1` for an old curve.
    pub deviation: f64,
}

/// `GetOrCreateUVTrimCurve`: the UV trim curve of the edge use (or its mate), built and
/// attached first when there is none (and `uv_space_ok`). Afterwards the curve is
/// [`uv_trim_curve`]. With `want_dist` / `want_deviation` the outputs are set as
/// upstream's `pdOptDistToSurf` / `pdOptDeviation` (`-1` when not built).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null; [`SolidError::Unported`] from `CreateUVTrimCurve`. Other
/// failures are in [`UvTrimCurveFetch::status`].
pub fn get_or_create_uv_trim_curve(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    want_dist: bool,
    want_deviation: bool,
    uv_space_ok: bool,
) -> Result<UvTrimCurveFetch> {
    // init outputs
    let mut out = UvTrimCurveFetch {
        status: Ok(()),
        dist_to_surf: -1.0,
        deviation: -1.0,
    };

    // get cached UVTrimCurve (if any)
    let present = uv_trim_curve(b, eu)?.0.is_some();

    // no work - no UVTrimCurve and not allowed to build one for fear of DropCurve
    // failure during healing - return NULL
    if !present && !uv_space_ok {
        return Ok(out);
    }

    // low work - UVTrimCurve already exists
    if present {
        // prevent infinite loop - only ask for FaceEdge Gap if it already exists
        if want_dist && crate::topology::edgeuse::p1::is_max_edge_face_gap_init(b, eu)? {
            let gap = max_edge_face_gap(k, b, eu, false)?;
            out.dist_to_surf = gap.map_or(0.0, |g| g.gap_3d);
        }
        // `m_pDropCurveFail` itself, not `GetDropCurveFail()`: the record of this edge
        // use only (`SetUVTrimCurve` nulls the non-owner's)
        if let Some(f) = b.edgeuse(eu)?.drop_curve_fail.as_deref() {
            if f.fail_status != SM_SUCCESS {
                out.status = Err(SolidError::Upstream {
                    operation: OP,
                    code: f.fail_status,
                    entity: None,
                });
            }
        }
        return Ok(out);
    }

    // arrive here to build UVTrimCurve: create a new UVTrimCurve: err return value =
    // CreateUVTrimCurve err return value
    let created = match create_uv_trim_curve(k, b, eu, want_deviation, None) {
        Ok(c) => c,
        Err(
            e @ (SolidError::Unported { .. }
            | SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }
            | SolidError::NotFound { .. }),
        ) => return Err(e),
        Err(e) => {
            // the failing status
            out.status = Err(e);
            return Ok(out);
        }
    };

    // Save max edge to projected TrimCurve distances. If one is Null and the other is
    // non-Null, return the whole distance in the non-Null one. (If pdOptDistToSurf is
    // non-Null and pdOptDeviation is Null, CreateUVTrimCurve takes care of that.)
    if want_dist {
        out.dist_to_surf = created.max_dist_to_surf;
        if let Some(d) = created.max_deviation {
            out.deviation = d;
        }
    } else if let Some(d) = created.max_deviation {
        out.deviation = crate::geom::vector3d::sqrt(
            created.max_dist_to_surf * created.max_dist_to_surf + d * d,
        );
    }

    // Fix duplicate UVTrimCurve Problem: in the process of creating a UV trim curve a
    // trimmed surface cache will be retrieved or created for the face's surface to which
    // this Edgeuse is attached. Constructing this trimmmed surface cache may create and
    // attach another UVTrimCurve to this edgeuse rendering our new one redundant.
    //
    // A call to SetUVTrimCurve will delete the duplicate UVTrimCurve (pass a copy of
    // DropCurveFail when it exists)
    let mut target_fail = drop_curve_fail(b, eu)?.0.map(|old| {
        let mut copy = DropCurveFail::default();
        copy.assign(old);
        copy
    });
    set_uv_trim_curve(
        b,
        eu,
        UvTrimCurveArg::New(Box::new(created.curve)),
        created.max_dist_to_surf,
        true, // TRUE = delete existing UVTrimCurve
        target_fail.as_mut(),
    )?;
    Ok(out)
}

// ---- EvaluateBinormal -------------------------------------------------------------------

/// Two solutions that both lie on a seam: the first one's parameters (`sols.len() == 2 &&
/// IsOnSeam(first) && IsOnSeam(second)`); `None` otherwise.
fn seam_pair_uv(
    k: &Kernel,
    surface: &Surface,
    sols: &[crate::geom::solution_array::Solution<NoObject>],
) -> Result<Option<Point2>> {
    let [first, second] = sols else {
        return Ok(None);
    };
    let uv0 = Point2::new(first.start.param(0)?, first.start.param(1)?);
    if !surface.is_on_seam(k, uv0, None)?.on_seam {
        return Ok(None);
    }
    let uv1 = Point2::new(second.start.param(0)?, second.start.param(1)?);
    if !surface.is_on_seam(k, uv1, None)?.on_seam {
        return Ok(None);
    }
    Ok(Some(uv0))
}

/// The position and the first two derivatives of a curve evaluation (zero-filled where
/// the evaluation returned fewer).
fn pv3(v: &[Vec3]) -> [Vec3; 3] {
    let at = |i: usize| v.get(i).copied().unwrap_or(Vec3::new(0.0, 0.0, 0.0));
    [at(0), at(1), at(2)]
}

/// The optional outputs of [`evaluate_binormal`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BinormalOptions {
    /// `pOptUVVector`: also compute the surface parameter direction of the binormal
    /// ([`BinormalUv::uv_vector`]).
    pub want_uv_vector: bool,
}

/// [`evaluate_binormal`]'s result with the optional parameter direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinormalUv {
    /// The binormal point, binormal, tangent, face use normal and surface parameters.
    pub eval: BinormalEval,
    /// `pOptUVVector`: the (not unitised) parameter direction of the binormal
    /// (`ComputeParamDirection`), when asked.
    pub uv_vector: Option<Vec2>,
}

/// The binormal point and vector at `parameter` on an edge use: a 3D point on the edge
/// and a unit vector perpendicular to the face use normal and the edge use tangent,
/// pointing into the face. With `use_3d_only` the edge point is dropped onto the surface
/// unless the edge use has a UV trim curve; without it a missing UV trim curve is
/// built first (`GetOrCreateUVTrimCurve`). A parameter outside the edge's interval is
/// snapped to it. Where the tangent or the binormal vanishes the tangent is taken from
/// the second derivative or from points stepped along the curve or the surface.
///
/// # Errors
///
/// `SM_ERR` (`SolidError::Upstream`) for an edge use not in a loop, for no usable
/// tangent or binormal and when the edge point does not drop to the surface;
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null; the evaluators' errors; [`SolidError::Unported`]
/// (module docs).
#[allow(clippy::too_many_lines)]
pub fn evaluate_binormal(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    parameter: f64,
    use_3d_only: bool,
    options: &BinormalOptions,
) -> Result<BinormalEval> {
    Ok(evaluate_binormal_uv(k, b, eu, parameter, use_3d_only, options)?.eval)
}

/// [`evaluate_binormal`] with the optional parameter direction.
///
/// # Errors
///
/// As [`evaluate_binormal`].
#[allow(clippy::too_many_lines)]
pub fn evaluate_binormal_uv(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    parameter: f64,
    use_3d_only: bool,
    options: &BinormalOptions,
) -> Result<BinormalUv> {
    let mut parameter = parameter;
    let mut use_3d_only = use_3d_only;

    // check state - only Edgeuses in loops have a binormal vector
    if !crate::topology::edgeuse::p1::is_loop_edgeuse(b, eu)? {
        return Err(SolidError::Upstream {
            operation: OP,
            code: 1007, // SM_ERR_INVALID_INPUT
            entity: None,
        });
    }

    let e = edge(b, eu)?.ok_or_else(|| null("EvaluateBinormal: edge use without edge"))?;

    // check state - dParameter not within Edge->Ivl
    if !b.edge(e)?.interval.contains_value(parameter, EFF_ZERO) {
        // inform the public - this is a bug and should be examined. A lot of these
        // calls seem to be coming from CurveOnClassify where Edges are found that stop
        // near but not on the Curve being classified. snap Param value to Edge->Ivl
        // endPoint
        parameter = b.edge(e)?.interval.clamp_value(parameter);
    }

    // locals
    let edge_ivl = b.edge(e)?.interval;
    let curve_3d = b
        .edge(e)?
        .curve
        .clone()
        .ok_or_else(|| SolidError::null_pointer(OP))?; // NER(p3DCurve)
    let eu_orientation = orientation(b, eu)?;

    // evaluate the unoriented curve position and tangent values
    let mut curve_pv = pv3(&curve_3d.evaluate(k, parameter, 1, true, true)?);

    // while tangent length is zero - step along curve looking for a nonDegenerate
    // tangent direction
    let mut steps: u64 = 0;
    while curve_pv
        .get(1)
        .copied()
        .unwrap_or(Vec3::UNDEF)
        .length_squared()
        < EFF_ZERO_SQ
        && steps < 10
    {
        steps += 1;

        // First: check second derivative. If it's not zero, then its direction is what
        // the first deriv's direction will be as soon as we leave the singularity.
        // [bd, 13 Sep 05]

        // Try eval from right
        curve_pv = pv3(&curve_3d.evaluate(k, parameter, 2, true, true)?);
        if curve_pv[2].length_squared() >= EFF_ZERO_SQ {
            // assume 2nd deriv points pos at edge beg where parameterization is
            // expected to be increasing and neg at edge ends where parameterization is
            // expected to be decreasing
            curve_pv[1] = if edge_ivl.mid() > parameter {
                curve_pv[2]
            } else {
                -curve_pv[2]
            };
            break;
        }

        // Else try eval from left
        curve_pv = pv3(&curve_3d.evaluate(k, parameter, 2, false, true)?);
        if curve_pv[2].length_squared() >= EFF_ZERO_SQ {
            curve_pv[1] = if edge_ivl.mid() > parameter {
                curve_pv[2]
            } else {
                -curve_pv[2]
            };
            break;
        }

        // pick sequence of new param values progressively farther away from orig param
        let next_step = if steps % 2 == 0 {
            steps as f64 * EFF_ZERO_SQRT
        } else {
            steps as f64 * -EFF_ZERO_SQRT
        };
        let step_param = if edge_ivl.contains_value(parameter + next_step, 0.0) {
            parameter + next_step
        } else if edge_ivl.contains_value(parameter - next_step, 0.0) {
            parameter - next_step
        } else {
            return Err(SolidError::sm_err(OP));
        };

        // evaluate unoriented curve at modified param value
        let pv2 = curve_3d.evaluate(k, step_param, 1, true, true)?;
        curve_pv[1] = pv2
            .get(1)
            .copied()
            .ok_or_else(|| null("curve evaluation"))?;
    } // end while tang vector length == 0.0 - seek nonDegenerate Tangent evals

    // Reverse orientation of curve tangent to conform to edgeuse tangent.
    if eu_orientation == OrientType::Opposite {
        curve_pv[1] = -curve_pv[1];
    }

    // face and surface locals
    let f = crate::topology::edgeuse::p1::face(b, eu)?
        .ok_or_else(|| null("EvaluateBinormal: edge use without face"))?;
    let face_rec = b.face(f)?;
    let face_surface = {
        // `SmEdgeuse::EvaluateBinormal` holds `pFace->GetSurface()` directly
        // (`SmEdgeuse.cpp:2875`), so the evaluation snapshot must retain the live
        // surface's owner, exact NURBS and shared cache slot instead of running
        // `Surface`'s copy constructor.
        face_rec
            .surface
            .as_ref()
            .ok_or_else(|| null("EvaluateBinormal: face without surface"))?
            .clone_for_evaluation()?
    };
    let domain = face_rec.uv_domain;
    let face_uv_orientation = {
        let fu = faceuse(b, eu)?.ok_or_else(|| null("EvaluateBinormal: no face use"))?;
        b.faceuse(fu)?.orientation
    };

    // If we already have a uv trim curve, then we can assume it's valid. [B556]
    let has_trim = uv_trim_curve(b, eu)?.0.is_some();
    let fail_status = drop_curve_fail(b, eu)?.0.map(|f| f.fail_status);

    // Work in UV Space when good pUVTrimCurve is available
    if has_trim && fail_status.is_none_or(|s| s != SM_SUCCESS && s != SM_ERR_UNKNOWN) {
        use_3d_only = false;
    }

    // when trying to work in UVSpace - make sure to get a UVTrimCurve
    if !has_trim && !use_3d_only && fail_status.is_none_or(|s| s != SM_ERR_UNKNOWN) {
        // Get our uv trim curve. Create it if necessary. [B438] Note: a bit of a
        // hack. (Besides the non-const cast.) CreateUVTrimCurve() can call this method
        // (EvaluateBinormal), which would seem to invite infinite recursion. However,
        // that method calls this one only with bUse3DOnly set to TRUE, so this will
        // never happen. (status ignored)
        let _ = get_or_create_uv_trim_curve(k, b, eu, false, false, true)?;
    }
    let uv_trim: Option<Curve> = uv_trim_curve(b, eu)?.0.cloned();

    // the face's surface; its owner is cleared (a copy, `solve_surface`) only around
    // `GlobalPointSolve` and `IsOnSeam` below, as upstream's `SmTemporaryChangeValue`
    // scope does
    let surface = face_surface;

    let mut uv = Point2::UNDEF;
    let mut got_uv = false;
    let edge_tol = b.edge(e)?.tolerance;

    // eval pUVTrimCurve on clamped param values and clamp resulting uv point
    if let Some(trim) = uv_trim.as_ref() {
        // clamp the parameter value
        parameter = trim.natural_interval().clamp_value(parameter);

        // compute UVTrimCurve Point and clamp that to SurfaceDomain
        let pnt = trim.evaluate_point(k, parameter)?;
        uv = domain.clamp_point(Point2::new(pnt.x, pnt.y));
        got_uv = true;

        // Double check the validity of this uv point.
        let tol = edge_tol * 2.0;
        if tol > 0.0 {
            // Allow for the 3d curve point being off the surface. (It happens.) Check
            // the dist from the 3d curve point to the surface normal.
            // (status ignored)
            let (this_srf_pt, this_su, this_sv) = ignored(
                surface.evaluate_1st_derivatives(k, uv, false, false),
            )?
            .unwrap_or((Vec3::UNDEF, Vec3::UNDEF, Vec3::UNDEF));

            // try a cheap eval for surface normal
            let mut this_norm = this_su.cross(this_sv);
            let len = this_norm.length();
            if len > crate::surface::surface::p2::normal_limit() {
                this_norm /= len;
            } else {
                // when cheap is degenerate - try expensive normal eval (status ignored;
                // sThisNorm is returned unitized)
                if let Some(n) = ignored(surface.evaluate_normal(
                    k,
                    uv,
                    crate::nurbs::knots::Side::Right,
                    crate::nurbs::knots::Side::Right,
                ))? {
                    this_norm = n;
                }
            }

            // When minDist(surfacePt/surfaceNorm line, EdgePoint) < dTol - use the UV
            // point
            let gap = curve_pv[0] - this_srf_pt;
            let dot = gap.dot(this_norm);
            let norm_pt = this_srf_pt + dot * this_norm;

            got_uv = norm_pt.closer_than(tol, curve_pv[0]);

            // When edge point is too far from surface normal line, try a LocalSolve
            // using sUV as a guess. (Much cheaper than GlobalPointSolve, next.)
            if !got_uv {
                let (found, sol_uv, sol_value) =
                    surface_local_point_solve(k, &surface, &domain, curve_pv[0], uv)?;
                got_uv = found;
                if got_uv && sol_value > tol {
                    got_uv = false;
                }
                if got_uv {
                    uv = sol_uv;
                }
            }
        } // end if double-checking result of pUVTrimCurve (dTol > 0)
    } // end if have pUVTrimCurve

    // arrive here with or without a sUV point marking the face point for the given edge
    // point. May have a sUV point if(bUse3DOnly == TRUE) and UVTrimPoint projected to
    // surface was close to edge point.

    // when the edgePoint isn't matched to a surface sUV point
    if !got_uv {
        // Drop edge point to surface to get uv point.
        let mut tol = edge_tol;

        // get and lock the surface cache (of the face's surface: its owner is cleared
        // only below)
        face_surface_cache(k, b, f)?;

        // Turn off point testing so GlobalPointSolve() will keep all point solutions
        // without classifying the solution point against the trim boundaries
        // (`global_point_solve_point_test_off`; the boundary-curve flag is not
        // modelled, module docs). Use surface's domain instead of Face's domain.
        let srf_domain = surface.natural_uv_domain_with_nurb(k)?;

        // This 'turns off point testing' when GPS does not use the cache (a copy of
        // the surface without owner stands for upstream's temporary change)
        let mut solve_surface = surface.clone();
        let _ = solve_surface.set_owner(None::<ObjectRef>);

        // Find Surface sUV point for EdgePoint
        let mut solutions = solve_surface.global_point_solve_point_test_off::<NoObject>(
            k,
            Some(NoObject),
            &srf_domain,
            SolverOperation::Minimize,
            curve_pv[0],
            tol,
            None,
            SolutionRequested::All,
        )?;
        if solutions.is_empty() {
            // try again with a bigger tolerance
            tol *= 1000.0;
            solutions = solve_surface.global_point_solve_point_test_off::<NoObject>(
                k,
                Some(NoObject),
                &srf_domain,
                SolverOperation::Minimize,
                curve_pv[0],
                tol,
                None,
                SolutionRequested::All,
            )?;
        } // end no solutions check

        let sols = solutions.as_slice();
        if sols.is_empty() {
            // We should now have a UV curve, so use it unless indicated otherwise.
            if use_3d_only {
                // no hope - can't get edge point to project to surface
                return Err(SolidError::sm_err(OP));
            }
            // get sUVTrimCurve
            let trim = match uv_trim_curve(b, eu)?.0.cloned() {
                Some(c) => c,
                None => {
                    // (See Hack warning, above.) SER, then NER
                    create_uv_trim_curve(k, b, eu, false, None)?.curve
                }
            };

            // set sUV based on sUVTrimCurve eval clamped to surface domain
            parameter = trim.natural_interval().clamp_value(parameter);
            let pnt = trim.evaluate_point(k, parameter)?;
            uv = domain.clamp_point(Point2::new(pnt.x, pnt.y));
        } else if let [sol] = sols {
            // the solution is the sUV point
            uv = Point2::new(sol.start.param(0)?, sol.start.param(1)?);
        } else if let Some(seam_uv) = seam_pair_uv(k, &solve_surface, sols)? {
            // If on a seam, doesn't matter which we use, the normal vector will be the
            // same. either solution can be the sUVPoint and we have no way of telling
            // which to choose
            uv = seam_uv;
        } else {
            // Multiple solutions or not seam: let sUV be the solution closest to the
            // EdgePoint
            let mut min_dist = BIG_DOUBLE;
            for sol in sols {
                let uv2 = Point2::new(sol.start.param(0)?, sol.start.param(1)?);
                let dist = sol.start.solution_value;
                if dist < min_dist {
                    min_dist = dist;
                    uv = uv2;
                }
            }
        }
    } // end no pUVTrimCurve to use to get a surface uv point branch

    // State: we have the surface parameter in sUV.

    // get oriented Faceuse Normal vector and Binormal, which is Normal <cross> edge
    // tangent
    let mut normal = Vec3::new(0.0, 0.0, 0.0);
    let mut binormal = Vec3::new(0.0, 0.0, 0.0);

    // Deal with potential discontinuities in the curve or surface. If we get a zero
    // binormal, try small step-offs. [230227_crease]
    let crv_step = edge_ivl.length() / 1000.0;
    let srf_step = domain.size() / 1000.0;
    let mut step_uv = uv;
    let tol_uv = srf_step.x.min(srf_step.y) / 2.0;

    // Don't re-evaluate the curve step-offs on each surface iteration.
    let mut crv_eval_lo: Option<Vec3> = None;
    let mut crv_eval_hi: Option<Vec3> = None;

    // Loop over four step-off directions on the surface. In each iteration, try two
    // curve step-offs.
    for ii in 0..5_u32 {
        match ii {
            1 => step_uv = Point2::new(uv.x + srf_step.x, uv.y),
            2 => step_uv = Point2::new(uv.x - srf_step.x, uv.y),
            3 => step_uv = Point2::new(uv.x, uv.y + srf_step.y),
            4 => step_uv = Point2::new(uv.x, uv.y - srf_step.y),
            _ => {}
        }

        // If the step puts us outside the basic domain, don't use it. (But if the input
        // uv is out, skipping it could skip everything.)
        if ii > 0 && !domain.contains_point(step_uv, tol_uv) {
            continue;
        }

        normal = surface.evaluate_normal(
            k,
            step_uv,
            crate::nurbs::knots::Side::Left,
            crate::nurbs::knots::Side::Left,
        )?;

        if face_uv_orientation == OrientType::Opposite {
            normal = -normal;
        }

        // let binormal = crossProduct(Faceuse->Normal, Edgeuse->Normal). A person
        // walking along the Edgeuse from Start to End will have the interior of the
        // face to his lefthand side.
        binormal = normal.cross(curve_pv[1]);

        if binormal.length_squared() > EFF_ZERO_SQ {
            break; // we're good.
        }

        // when Binormal is zero length - try curve eval just below given param point
        if edge_ivl.min() < parameter - crv_step {
            if crv_eval_lo.is_none() {
                let pv = curve_3d.evaluate(k, parameter - crv_step, 1, true, true)?;
                crv_eval_lo = pv.get(1).copied();
            }
            curve_pv[1] = crv_eval_lo.ok_or_else(|| null("curve evaluation"))?;

            // Reverse orientation of curve tangent to conform to edgeuse tangent.
            if eu_orientation == OrientType::Opposite {
                curve_pv[1] = -curve_pv[1];
            }

            binormal = normal.cross(curve_pv[1]);

            if binormal.length_squared() > EFF_ZERO_SQ {
                break; // we're good.
            }
        }

        // when binormal is still zero length - try curve eval just above given param
        // point
        if edge_ivl.max() > parameter + crv_step {
            if crv_eval_hi.is_none() {
                let pv = curve_3d.evaluate(k, parameter + crv_step, 1, true, true)?;
                crv_eval_hi = pv.get(1).copied();
            }
            curve_pv[1] = crv_eval_hi.ok_or_else(|| null("curve evaluation"))?;

            // Reverse orientation of curve tangent to conform to edgeuse tangent.
            if eu_orientation == OrientType::Opposite {
                curve_pv[1] = -curve_pv[1];
            }

            binormal = normal.cross(curve_pv[1]);

            if binormal.length_squared() > EFF_ZERO_SQ {
                break; // we're good.
            }
        }
    } // end loop on surface-evaluation step-offs.

    // give up if binormal is still zero
    if binormal.length_squared() < EFF_ZERO_SQ {
        return Err(SolidError::sm_err(OP));
    }

    // normalize the return vector
    binormal = binormal.unitize_or_z();

    // store optional output values (ComputeParamDirection: status ignored)
    let uv_vector = if options.want_uv_vector {
        ignored(surface.compute_param_direction(k, uv, binormal))?
    } else {
        None
    };

    Ok(BinormalUv {
        eval: BinormalEval {
            point: curve_pv[0],
            binormal,
            tangent: curve_pv[1],
            face_normal: normal,
            uv,
        },
        uv_vector,
    })
}

// ---- NormalizedEvaluate -----------------------------------------------------------------

/// The outputs of [`normalized_evaluate`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizedEval {
    /// `rPoint`: the UV point (`z = 0`) or the XYZ point along the edge use.
    pub point: Point3,
    /// `*pOptFirstDeriv`: the first derivative along the edge use (with
    /// `want_first_deriv`).
    pub first_deriv: Option<Vec3>,
}

/// The evaluation of `curve` over `ivl` at the oriented normalised parameter.
fn eval_on_curve(
    k: &Kernel,
    curve: &Curve,
    ivl: &Extent1d,
    orient: OrientType,
    normalized_parameter: f64,
    want_first_deriv: bool,
) -> Result<NormalizedEval> {
    // invert parameter for opposite running Edgeuses
    let mut oriented_param = normalized_parameter;
    if orient == OrientType::Opposite {
        oriented_param = 1.0 - oriented_param;
    }

    // get requested evaluation
    if want_first_deriv {
        let pv = curve.evaluate(k, ivl.evaluate(oriented_param), 1, true, true)?;
        let p = *pv
            .first()
            .ok_or_else(|| null("NormalizedEvaluate: no point"))?;
        let mut d = *pv
            .get(1)
            .ok_or_else(|| null("NormalizedEvaluate: no tangent"))?;

        // invert tangents for opposite running Edgeuses
        if orient == OrientType::Opposite {
            d = -d;
        }
        Ok(NormalizedEval {
            point: p,
            first_deriv: Some(d),
        })
    } else {
        Ok(NormalizedEval {
            point: curve.evaluate_point(k, ivl.evaluate(oriented_param))?,
            first_deriv: None,
        })
    }
}

/// `NormalizedEvaluate`: the edge use evaluated at the normalised parameter `0..1`
/// (orientation accounted for): in UV space (`parameter_space`, evaluating the UV trim
/// curve, built first when missing and `ok_to_make_uv_trim_curve`; without one the 3D
/// point is evaluated and dropped to the surface) or in 3D (the edge curve).
///
/// # Errors
///
/// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
/// upstream dereferences null; `SM_ERR` for a point that does not drop to the surface;
/// the evaluators' errors; [`SolidError::Unported`] (module docs).
#[allow(clippy::too_many_arguments)]
pub fn normalized_evaluate(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    normalized_parameter: f64,
    parameter_space: bool,
    want_first_deriv: bool,
    ok_to_make_uv_trim_curve: bool,
) -> Result<NormalizedEval> {
    let orient = orientation(b, eu)?;

    // when looking for parameter space evaluations let p3dOrUVCurve = UVTrimCurve
    if parameter_space {
        // fetch or create UVTrimCurve (status ignored; the outputs are for the
        // suspicious-result debug check)
        let _ = get_or_create_uv_trim_curve(k, b, eu, true, true, ok_to_make_uv_trim_curve)?;

        if let Some(trim) = uv_trim_curve(b, eu)?.0 {
            let ivl = trim.natural_interval();
            return eval_on_curve(
                k,
                trim,
                &ivl,
                orient,
                normalized_parameter,
                want_first_deriv,
            );
        }
        {
            {
                // Could do: If a uv trim curve cannot be created, then evalute in 3d
                // and drop to the surface. And don't forget pOptFirstDeriv.

                // Evaluate in 3d, then drop to surface.
                let three_d = normalized_evaluate_3d(
                    k,
                    b,
                    eu,
                    orient,
                    normalized_parameter,
                    want_first_deriv,
                )?;
                let f = crate::topology::edgeuse::p1::face(b, eu)?
                    .ok_or_else(|| null("NormalizedEvaluate: edge use without face"))?;
                let surf = b
                    .face(f)?
                    .surface
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(OP))?; // NER(pSurf)
                let domain = surf.natural_uv_domain_with_nurb(k)?;
                let drop = surface_drop_point(k, surf, three_d.point, &domain)?;
                if !drop.found {
                    return Err(SolidError::sm_err(OP));
                }
                let uv = drop.uv;

                let mut first_deriv = None;
                if let Some(d3) = three_d.first_deriv {
                    // (status ignored)
                    let (s_su, s_sv) = ignored(surf.evaluate_1st_derivatives(k, uv, false, false))?
                        .map_or((Vec3::UNDEF, Vec3::UNDEF), |(_, su, sv)| (su, sv));
                    first_deriv = Some(match vec_lin_comb_two_vectors(s_su, s_sv, d3) {
                        Ok(d) => Vec3::new(d.x, d.y, 0.0),
                        Err(e) if e.is_unported() => return Err(e),
                        Err(_) => {
                            // Probably a zero vector. Set that one to zero, the other to
                            // the dot product.
                            if s_su.length() < s_sv.length() {
                                Vec3::new(0.0, d3.dot(s_sv), 0.0)
                            } else {
                                Vec3::new(d3.dot(s_su), 0.0, 0.0)
                            }
                        }
                    });
                }

                // All done.
                return Ok(NormalizedEval {
                    point: Vec3::new(uv.x, uv.y, 0.0),
                    first_deriv,
                });
            }
        }
    }

    // xyzSpace - let p3dOrUVCurve = 3DCurve and sIvl = Edge domain.
    normalized_evaluate_3d(k, b, eu, orient, normalized_parameter, want_first_deriv)
}

/// `NormalizedEvaluate(t, FALSE, ..)`: the edge curve evaluated at the oriented
/// normalised parameter.
fn normalized_evaluate_3d(
    k: &Kernel,
    b: &Brep,
    eu: EdgeuseId,
    orient: OrientType,
    normalized_parameter: f64,
    want_first_deriv: bool,
) -> Result<NormalizedEval> {
    let e = edge(b, eu)?.ok_or_else(|| SolidError::null_pointer(OP))?; // NER(pE)
    let rec = b.edge(e)?;
    let curve = rec
        .curve
        .as_ref()
        .ok_or_else(|| SolidError::null_pointer(OP))?; // NER(p3dOrUVCurve)
    eval_on_curve(
        k,
        curve,
        &rec.interval,
        orient,
        normalized_parameter,
        want_first_deriv,
    )
}

/// Not upstream (WR-543; the fork's source fork@e36feb1, `sm_LongestDropRun`):
/// of a drop that came back in pieces (`DropAndTrimCurve` splits it where the curve
/// leaves the face's domain or crosses a seam), the longest run (in edge parameter) of
/// pieces that meet end to end, as one UV trim curve: the second starts at the edge
/// parameter where the first ends (`SM_ARE_SAME`) and at the same `(u, v)`, to within
/// the drop's UV tolerances (`tol_3d` over one plus the surface's derivative lengths).
/// A run of several pieces is joined exactly (`CreateByJoining`); no piece is
/// reparameterized. `uv_curves` keeps the one run.
///
/// # Errors
///
/// Those of the derivative lengths, the end evaluations and the join.
fn longest_drop_run(
    k: &Kernel,
    surface: &Surface,
    tol_3d: f64,
    uv_curves: &mut Vec<Curve>,
) -> Result<()> {
    if uv_curves.len() < 2 {
        return Ok(());
    }

    // UV tolerances proportional to the surface's 3d/UV mapping, as DropCurve's
    let speeds = crate::surface::surface::p7::approx_derivative_lengths(
        k,
        surface,
        &surface.natural_uv_domain_with_nurb(k)?,
    )?;
    let u_tol = tol_3d / (speeds.x + 1.0);
    let v_tol = tol_3d / (speeds.y + 1.0);

    // by the start of the edge interval each piece was dropped from
    uv_curves.sort_by(|a, b| {
        a.natural_interval()
            .min()
            .total_cmp(&b.natural_interval().min())
    });

    let pieces = core::mem::take(uv_curves);
    let mut ends = Vec::with_capacity(pieces.len());
    for piece in &pieces {
        let e = piece.ends(k)?;
        ends.push((piece.natural_interval(), e.start, e.end));
    }
    let mut best: Option<Curve> = None;
    let mut best_len = -1.0;
    let mut first = 0;
    while first < pieces.len() {
        // extend the run from `first` through every piece that meets the previous one
        let mut last = first;
        while let (Some(prev), Some(next)) = (ends.get(last), ends.get(last + 1)) {
            if !are_same(prev.0.max(), next.0.min())
                || (prev.2.x - next.1.x).abs() > u_tol
                || (prev.2.y - next.1.y).abs() > v_tol
            {
                break;
            }
            last += 1;
        }

        // the run as one curve
        let run_pieces = pieces.get(first..=last).unwrap_or_default();
        let run = if let [one] = run_pieces {
            one.clone()
        } else {
            let mut segments = Vec::with_capacity(run_pieces.len());
            for piece in run_pieces {
                segments.push(b_spline_part(piece)?);
            }
            Curve::from(BSplineCurve::create_by_joining(&mut segments, None)?)
        };

        // keep the longest run
        let len = run.natural_interval().length();
        if len > best_len {
            best = Some(run);
            best_len = len;
        }
        first = last + 1;
    }
    uv_curves.extend(best);
    Ok(())
}

/// The B-spline data of a UV trim piece (an `SmBSplineCurve` kind: a B-spline, line or
/// conic; a line's NURB is made when it has none), as `CreateByJoining` reads it.
fn b_spline_part(c: &Curve) -> Result<BSplineCurve> {
    Ok(match c.kind() {
        CurveKind::BSpline(b) => b.clone(),
        CurveKind::Line(l) => l.base_with_nurb()?.into_owned(),
        CurveKind::Ellipse(e) => e.base().clone(),
        CurveKind::Hyperbola(h) => h.base().clone(),
        CurveKind::Parabola(p) => p.base().clone(),
        _ => return Err(null("CreateUVTrimCurve: a dropped piece is not a B-spline")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::Edgeuse;

    fn lone() -> (Brep, EdgeuseId) {
        let mut b = Brep::new();
        let eu = b.add_edgeuse_record(Edgeuse::default()).unwrap();
        (b, eu)
    }

    #[test]
    fn no_edge_is_an_error() {
        let (mut b, eu) = lone();
        let k = Kernel::default();
        assert!(create_uv_trim_curve(&k, &mut b, eu, false, None).is_err());
        assert!(evaluate_binormal(&k, &mut b, eu, 0.0, true, &BinormalOptions::default()).is_err());
        assert!(normalized_evaluate(&k, &mut b, eu, 0.5, false, false, true).is_err());
    }

    #[test]
    fn missing_curve_is_not_built_when_not_allowed() {
        let (mut b, eu) = lone();
        let k = Kernel::default();
        let r = get_or_create_uv_trim_curve(&k, &mut b, eu, false, false, false);
        // the mate lookup of an unlinked edge use fails like upstream's null dereference
        // or finds no curve; it never builds one
        if let Ok(r) = r {
            assert!(r.status.is_ok());
            assert_eq!(r.dist_to_surf, -1.0);
        }
    }

    #[test]
    fn uv_trim_fix_survives_edge_zone_kernel() {
        use crate::nurbs::b_spline_curve::p1::NurbCircleParam;
        use crate::nurbs::b_spline_curve::BSplineCurve;

        for enabled in [false, true] {
            let mut k = Kernel::default();
            k.fixes.bspline_trim_preserves_uv_spans = enabled;
            let uv = BSplineCurve::create_circle_segment(
                2,
                &Axis2Placement::default(),
                0.14,
                0.0,
                180.0,
                NurbCircleParam::Quadratic,
            )
            .unwrap();
            let domain = uv.natural_interval();
            let knot = uv.knots(None).unwrap().0[1];
            let original = Curve::from(uv);
            let mut b = Brep::new();
            let v0 = b.add_vertex(Vec3::ZERO).unwrap();
            let v1 = b.add_vertex(Vec3::X).unwrap();
            let edge = b.add_edge(v0, v1).unwrap();
            let this = b
                .add_edgeuse_record(Edgeuse {
                    edge: Some(edge),
                    ..Edgeuse::default()
                })
                .unwrap();
            let other = b
                .add_edgeuse_record(Edgeuse {
                    edge: Some(edge),
                    uv_trim_curve: Some(original.clone()),
                    ..Edgeuse::default()
                })
                .unwrap();
            b.edge_mut(edge).unwrap().edgeuses = vec![this, other];
            let mut ivl =
                Extent1d::from_min_max(knot - 3.5e-6 * domain.length(), domain.evaluate(0.9));
            trim_other_uv_curves(&k, &mut b, edge, this, &mut ivl).unwrap();
            let trimmed = b.edgeuse(other).unwrap().uv_trim_curve.as_ref().unwrap();
            let gap = (0..=64)
                .map(|i| {
                    let t = ivl.evaluate(f64::from(i) / 64.0);
                    let gap = (38.0
                        * (trimmed.evaluate_point(&k, t).unwrap()
                            - original.evaluate_point(&k, t).unwrap()))
                    .length();
                    assert!(gap.is_finite());
                    gap
                })
                .fold(0.0, f64::max);
            if enabled {
                assert!(gap < 1e-12, "enabled fix was lost: gap {gap}");
            } else {
                assert!(gap > k.tolerances.zone_tol_3d, "default gap {gap}");
            }
        }
    }

    /// WR-543 (source issue #1383; the fork's source fork@e36feb1):
    /// of three drop pieces, two that meet end to end (edge parameters [0, 0.4] and
    /// [0.4, 1], the same (u, v) where they meet) are joined on their own parameters and
    /// kept; a shorter piece apart from them is dropped.
    #[test]
    fn longest_drop_run_joins_the_pieces_that_meet() {
        use crate::math::Vec3;
        use crate::nurbs::crv_basic::Curve as NlCurve;
        use crate::nurbs::math::{CPoint, NOW};

        let piece = |t0: f64, t1: f64, a: [f64; 2], b: [f64; 2]| {
            let pts = vec![
                CPoint::new(a[0], a[1], 0.0, NOW),
                CPoint::new(b[0], b[1], 0.0, NOW),
            ];
            let nl = NlCurve::new(pts, 1, vec![t0, t0, t1, t1]).unwrap();
            Curve::from(BSplineCurve::from_nurb(nl, 2, false))
        };
        let k = Kernel::default();
        let plane = Surface::from(
            crate::primitive::api_primitives::create_plane_surface(
                Vec3::new(0.0, 0.0, 0.0),
                1.0,
                1.0,
            )
            .unwrap(),
        );
        let mut pieces = vec![
            piece(0.4, 1.0, [0.4, 0.0], [1.0, 0.0]),
            piece(-0.5, -0.3, [0.5, 0.5], [0.6, 0.5]),
            piece(0.0, 0.4, [0.0, 0.0], [0.4, 0.0]),
        ];
        longest_drop_run(&k, &plane, 1e-6, &mut pieces).unwrap();
        assert_eq!(pieces.len(), 1);
        let run = &pieces[0];
        let ivl = run.natural_interval();
        assert!(
            are_same(ivl.min(), 0.0) && are_same(ivl.max(), 1.0),
            "{ivl:?}"
        );
        let p = run.evaluate_point(&k, 0.4).unwrap();
        assert!((p.x - 0.4).abs() < 1e-12 && p.y.abs() < 1e-12, "{p:?}");
    }
}
