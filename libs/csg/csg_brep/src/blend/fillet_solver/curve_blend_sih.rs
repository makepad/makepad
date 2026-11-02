//! The curve-blending self-intersection handler: joins the previous section of a
//! fillet geometry to the next traced section across a stepped-back blend.
//!
//! Upstream: `source/SMLib/src/SmFilletSolver.cpp` (8093-8669),
//! `source/SMLib/inc/SmFilletSolver.h` (596-635).
//!
//! | Rust                                           | Upstream                                      |
//! |------------------------------------------------|-----------------------------------------------|
//! | [`MakeCurveBlendSih`]                          | `SmMakeCurveBlendSIH`                         |
//! | [`MakeCurveBlendSih::algorithm_type`]          | `SmMakeCurveBlendSIH::GetAlgorithmType`       |
//! | [`MakeCurveBlendSih::smooth_out_intersection`] | `SmMakeCurveBlendSIH::SmoothOutIntersection`  |
//! | [`SmoothingSolver`]                            | the `SmFilletSolver` members it reaches (`pFS`) |
//! | `compute_fillet_radius` (private)              | `sm_ComputeFilletRadius`                      |
//!
//! Deviations:
//! - **Handles.** `pFilletGeom` is a [`FilletGeomId`] of the pseudo B-rep; its rails,
//!   their mate edge uses' UV trim curves and the centre line are read from and
//!   written back to that B-rep. `pFilletGeom->GetFilletSolver()` is checked (`NER`)
//!   and the solver running the trace (`m_rFilletSolver`, the only solver whose
//!   `FlushCurve` fills the geometry) stands for it. `pFace->GetSurface()` of the
//!   rails' original faces is read from the target B-rep; the natural UV domains
//!   upstream computes from them are unused and not computed.
//! - **In-place objects.** Upstream trims and re-NURBs the geometry's own curve
//!   objects (`Trim`, `SetFromGwNurb`) and returns those same objects in the output
//!   arrays. Here each change is written back to the pseudo B-rep when upstream makes
//!   it, and the outputs are copies of the final curves. Owner notifications those
//!   in-place edits raise upstream are not replicated (a Rust curve has no owner).
//!   The `SetSurfacesToZeroOffset` / `ReloadSurfaceOffsets` pair is not restored on
//!   an early error return, as upstream.
//! - **Undefined behaviour upstream.** A `NULL` rail, primary edge use, mate, UV trim
//!   curve, rail curve or centre line (upstream dereferences it without `NER`), and
//!   an empty `GlobalCurveSolve` result (`sSolutions[0]`), return
//!   [`SolidError::InvalidInput`] here. The `NULL` centre line is what every traced
//!   section's first `FlushCurve` meets, so upstream always crashes there once the
//!   rail faces pass their `NER` checks (plan/upstream-issues/
//!   `UB-260--smmakecurveblendsih-smoothoutintersection-null-centerline.md`).
//! - Status-ignored calls upstream (`InsertKnots`, `DampenKnotSpacing`,
//!   `SetFromGwNurb`, `SetMinMax`) ignore their result here too.

use crate::blend::fillet_geom::p1::{FilletBrep, FilletGeomId};
use crate::blend::fillet_intersector::{FilletSolverOps as IntersectorOps, HandlingAlgorithmType};
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::p9::CurveSolveOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::tol::is_zero;
use crate::geom::vector2d::Point2;
use crate::intersect::surface_intersector::p1::TsectPnt;
use crate::kernel::Kernel;
use crate::math::{Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::crv_fit::p1::ParamMethod;
use crate::surface::surface::Surface;
use crate::topology::edgeuse::p1 as eu;
use crate::topology::{EdgeId, EdgeuseId, FaceId};

const OP: &str =
    "blend::fillet_solver::curve_blend_sih::MakeCurveBlendSih::smooth_out_intersection";
const OP_RADIUS: &str = "blend::fillet_solver::curve_blend_sih::compute_fillet_radius";

/// The fillet surface self-intersection handler that steps back a little and blends
/// the curves across a self intersection (`SmMakeCurveBlendSIH`). The blended
/// section of the surface does not have the solver's cross-section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakeCurveBlendSih {
    /// `m_dStepBackDistFactor`: factor of the fillet radius to step back from an
    /// intersection before making blends between the curves.
    pub step_back_dist_factor: f64,
}

/// The `SmFilletSolver` members `SmoothOutIntersection` reaches through
/// `pFilletGeom->GetFilletSolver()`, beyond the intersector's
/// [`FilletSolverOps`](IntersectorOps).
pub trait SmoothingSolver: IntersectorOps<FilletGeom = FilletGeomId> {
    /// `pFace->GetSurface() != NULL` for an original face of the target B-rep.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without a target B-rep; the face lookup's.
    fn original_face_has_surface(&self, face: FaceId) -> Result<bool>;

    /// `SetupOffsetValues(rTsectPnt, &dRadius)`: the fillet radius at the point.
    ///
    /// # Errors
    ///
    /// The solver's failure.
    fn setup_offset_radius(&mut self, tsect: &TsectPnt) -> Result<f64>;

    /// `GetSurface(rail)->GetBaseSurface()->GetNaturalUVDomain()`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without the offset surface.
    fn base_natural_uv_domain(&self, rail: usize) -> Result<Extent2d>;

    /// `GetSurface(rail)`, at its current offset distance.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] without the offset surface.
    fn rail_surface(&self, rail: usize) -> Result<Surface>;

    /// `GetThisApproxTol3d()`.
    fn this_approx_tol_3d(&self) -> f64;
}

/// The handle of a curve in this handler's global solves (upstream `this` and the
/// other curve): a curve with its natural interval.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SolveCurve(Extent1d);

impl SolutionObject for SolveCurve {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

/// Upstream dereferences `NULL` here (see the module deviations).
fn undefined(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// `SM_CAST_PTR(SmBSplineCurve, pCurve)`: the B-spline of a curve, `None` for any
/// other kind.
fn as_bspline(curve: &Curve) -> Option<&BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(b) => Some(b),
        _ => None,
    }
}

/// `pFilletGeom->GetRail(ii)` (the rail must exist).
fn rail(pseudo: &FilletBrep, geom: FilletGeomId, ii: usize) -> Result<EdgeId> {
    pseudo
        .geom(geom)?
        .rails
        .get(ii)
        .copied()
        .flatten()
        .ok_or_else(|| undefined("fillet geometry without the rail"))
}

/// `(SmFilletEdgeuse*)pPrevRailEdge->GetPrimaryEdgeuse()->GetMate()`.
fn rail_mate(pseudo: &FilletBrep, rail_edge: EdgeId) -> Result<EdgeuseId> {
    let primary = eu::primary_edgeuse(pseudo.brep(), rail_edge)?
        .ok_or_else(|| undefined("rail without a primary edge use"))?;
    eu::mate(pseudo.brep(), primary)?.ok_or_else(|| undefined("rail edge use without a mate"))
}

/// `pPrevMateEU->GetUVTrimCurve()` as the `SmBSplineCurve` it is upstream (`None`
/// for `NULL`).
fn mate_uv_curve(pseudo: &FilletBrep, mate: EdgeuseId) -> Result<Option<BSplineCurve>> {
    Ok(pseudo
        .brep()
        .edgeuse(mate)?
        .uv_trim_curve
        .as_ref()
        .and_then(as_bspline)
        .cloned())
}

/// `pPrevRailEdge->GetCurve()` (the curve must exist).
fn rail_curve(pseudo: &FilletBrep, rail_edge: EdgeId) -> Result<Curve> {
    pseudo
        .brep()
        .edge(rail_edge)?
        .curve
        .clone()
        .ok_or_else(|| undefined("rail without a curve"))
}

/// `pCurve->Evaluate(dT, 1, TRUE, sPV)`: point and first derivative.
fn pv(values: &[Vec3]) -> Result<[Vec3; 2]> {
    match values {
        [p, d, ..] => Ok([*p, *d]),
        _ => Err(SolidError::sm_err(OP)),
    }
}

/// `sm_ComputeFilletRadius(pFS, pUVCrv1, pUVCrv2, dT, sPV, &dRadius)`: the fillet
/// radius needed between `uv_crv1(t)` and `uv_crv2(t)`, from the fillet point the
/// solver finds on the plane through `pv[0]` normal to `pv[1]`.
fn compute_fillet_radius<S: SmoothingSolver + ?Sized>(
    fs: &mut S,
    k: &Kernel,
    uv_crv1: &BSplineCurve,
    uv_crv2: &BSplineCurve,
    t: f64,
    pv: &[Vec3; 2],
) -> Result<f64> {
    // fillet surface natural (not extended) UVDomains
    let domain1 = fs.base_natural_uv_domain(0)?;
    let domain2 = fs.base_natural_uv_domain(1)?;

    // surface0 UVPoint
    let pnt = uv_crv1.evaluate_point(t)?;
    let uv1 = Point2::new(pnt.x, pnt.y);

    // surface1 UVPoint
    let pnt = uv_crv2.evaluate_point(t)?;
    let uv2 = Point2::new(pnt.x, pnt.y);

    // find filletPoint on given plane satisfying geometry requirements
    // implemented in derived SmFilletSolver class
    let mut tsect_pnt = TsectPnt::default();
    let found_solution = fs.point_on_plane_solve(
        k,
        pv[0],
        pv[1],
        &domain1,
        &domain2,
        uv1,
        uv2,
        &mut tsect_pnt,
    )?;
    if !found_solution {
        return Err(SolidError::sm_err(OP_RADIUS));
    }

    // extract radius at this fillet point for the
    fs.setup_offset_radius(&tsect_pnt)
}

/// The step from `prev_t` back along the previous rail and from `next_t` forward
/// along the next rail (`dStepDistance / |tangent|`), each halved towards the
/// interval end when it overshoots it; the intervals are then cut to the steps.
#[allow(clippy::too_many_arguments)]
fn step_back(
    step_distance: f64,
    prev_t: f64,
    prev_tangent: Vec3,
    next_t: f64,
    next_tangent: Vec3,
    prev_trim_ivl: &mut Extent1d,
    next_trim_ivl: &mut Extent1d,
) -> Result<()> {
    // step back param distance
    let mut tan_leng = prev_tangent.length();
    if is_zero(tan_leng) {
        return Err(SolidError::sm_err(OP));
    }
    let mut prev_step = prev_t - step_distance / tan_leng;

    // If the step goes too far then step back 1/2 of distance to start of curve.
    if prev_step <= prev_trim_ivl.min() {
        prev_step = (prev_t + prev_trim_ivl.min()) / 2.0;
    }

    // step forward param distance
    tan_leng = next_tangent.length();
    if is_zero(tan_leng) {
        return Err(SolidError::sm_err(OP));
    }
    let mut next_step = next_t + step_distance / tan_leng;
    if next_step >= next_trim_ivl.max() {
        next_step = (next_t + next_trim_ivl.max()) / 2.0;
    }

    // adjust the Prev and Next curve intervals to the step back/forward points
    // (SetMinMax's status is ignored upstream)
    if prev_step < prev_trim_ivl.max() {
        let min = prev_trim_ivl.min();
        prev_trim_ivl.set_min_max(min, prev_step).ok();
    }
    if next_step > next_trim_ivl.min() {
        let max = next_trim_ivl.max();
        next_trim_ivl.set_min_max(next_step, max).ok();
    }
    Ok(())
}

/// `sHalfInterval.SetMinMax(sHalfInterval.Evaluate(0.5), sHalfInterval.GetMax())`.
fn half_interval(ivl: Extent1d) -> Extent1d {
    let mut half = ivl;
    half.set_min_max(half.evaluate(0.5), half.max()).ok();
    half
}

/// The blend from the end of `prev` (at `prev_t`) to the start of `next` (at
/// `next_t`): `FairBlendCurveDerivatives` on the two ends, then
/// `CreateInterpolatingCurve(SM_CP_UNIFORM, dim, 3, .., TRUE)` and `InsertKnots`
/// of the nine interior knots `0.1 .. 0.9`.
fn blend_curve(
    dim: u64,
    prev: &BSplineCurve,
    prev_t: f64,
    next: &BSplineCurve,
    next_t: f64,
    new_knots: &[f64],
) -> Result<BSplineCurve> {
    let mut points: Vec<Point3> = Vec::new();
    let mut vectors: Vec<Vec3> = Vec::new();

    // add Prev trimmed end point and 1stderiv
    let [p, d] = pv(&prev.evaluate(prev_t, 1, true, false)?)?;
    points.push(p);
    vectors.push(d);

    // add Next trimmed start point and 1stDeriv
    let [p, d] = pv(&next.evaluate(next_t, 1, true, false)?)?;
    points.push(p);
    vectors.push(d);

    // adjust 1stDeriv mags to optimize curvature distribution of upcoming curve
    BSplineCurve::fair_blend_curve_derivatives(&points, &mut vectors, None, false)?;

    // create blend piece as a BSpline Curve from PrevEndPoint to NextStartPoint
    let mut blend = BSplineCurve::create_interpolating_curve(
        ParamMethod::Uniform,
        dim,
        3,
        &points,
        &vectors,
        None,
        true,
    )?;
    // (status ignored upstream)
    blend.insert_knots(new_knots).ok();
    Ok(blend)
}

/// `GetSurface(rail)->LiftCurve(.., GetNaturalUVDomain(), uv, uv->GetNaturalInterval(),
/// GetThisApproxTol3d() * 10.0, dMaxDist, p3DLift)`, as the `SmBSplineCurve` it is
/// upstream.
fn lift<S: SmoothingSolver + ?Sized>(
    fs: &S,
    k: &Kernel,
    rail: usize,
    uv: &BSplineCurve,
) -> Result<BSplineCurve> {
    let surface = fs.rail_surface(rail)?;
    let uv_curve = Curve::from(uv.clone());
    let lifted = surface.lift_curve(
        k,
        &surface.natural_uv_domain_with_nurb(k)?,
        &uv_curve,
        &uv.natural_interval(),
        fs.this_approx_tol_3d() * 10.0,
        false,
    )?;
    as_bspline(&lifted.curve)
        .cloned()
        .ok_or_else(|| undefined("lifted curve is not a B-spline"))
}

/// `pTarget->SetFromGwNurb(0, pSource->GetOrCreateGwNurbPointer())` (status ignored
/// upstream).
fn set_from_nurb_of(target: &mut BSplineCurve, source: &mut BSplineCurve) -> Result<()> {
    let nurb = source.get_or_create_gw_nurb()?.clone();
    target.set_from_gw_nurb(Some(&nurb)).ok();
    Ok(())
}

impl MakeCurveBlendSih {
    /// `SmMakeCurveBlendSIH(dStepBackDistFactor)`.
    #[must_use]
    pub const fn new(step_back_dist_factor: f64) -> Self {
        Self {
            step_back_dist_factor,
        }
    }

    /// `GetAlgorithmType`: `SM_HA_SMOOTHING`.
    #[must_use]
    pub const fn algorithm_type(&self) -> HandlingAlgorithmType {
        HandlingAlgorithmType::Smoothing
    }

    /// `SmoothOutIntersection(pFilletGeom, r3DCurves, rUVCurves1, rUVCurves2)`:
    /// joins the previous section of fillet geometry `geom` (its centre line, rails
    /// and rail UV curves in `pseudo`) to the next section (`curves_3d` = next centre
    /// line, rail 0, rail 1; `uv_curves1[0]`, `uv_curves2[0]` = next rail UV curves).
    /// Both sections are trimmed back about `step_back_dist_factor` fillet radii from
    /// where the rail UV curves intersect (else from the closest points of the
    /// rails), a blend curve bridges each gap, and the joined curves replace the
    /// geometry's and are returned in the arrays.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without the geometry's solver, the rails' original
    /// faces or their surfaces, or a B-spline rail curve or UV curve in the joining
    /// pass; `SM_ERR` when the rails are more than three step distances apart, a
    /// tangent is zero or the radius solve finds no point; [`SolidError::InvalidInput`]
    /// where upstream dereferences `NULL` (module deviations); those of the curve
    /// operations and the solver.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub fn smooth_out_intersection<S: SmoothingSolver + ?Sized>(
        &self,
        fs: &mut S,
        k: &Kernel,
        pseudo: &mut FilletBrep,
        geom: FilletGeomId,
        curves_3d: &mut Vec<BSplineCurve>,
        uv_curves1: &mut Vec<BSplineCurve>,
        uv_curves2: &mut Vec<BSplineCurve>,
    ) -> Result<()> {
        // check input
        pseudo.geom(geom)?;

        // Get next piece of fillet geometry curves from the input arrays
        // (an empty array is an out-of-range read upstream)
        let next = |v: &[BSplineCurve], i: usize| {
            v.get(i)
                .cloned()
                .ok_or_else(|| undefined("missing next curve piece"))
        };
        let mut next_center_curve = next(curves_3d, 0)?; // next centerline piece
        let mut next_rail_curves = [next(curves_3d, 1)?, next(curves_3d, 2)?]; // next rail pieces
        let mut next_uv_curves = [next(uv_curves1, 0)?, next(uv_curves2, 0)?]; // next railUV pieces

        // previous piece of fillet geometry is in pFilletGeom object
        if pseudo.geom(geom)?.solver.is_none() {
            return Err(SolidError::null_pointer(OP));
        }

        let prev_rail = [rail(pseudo, geom, 0)?, rail(pseudo, geom, 1)?];
        for &rail_edge in &prev_rail {
            let face = pseudo
                .fillet_edge(rail_edge)?
                .orig_face
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            if !fs.original_face_has_surface(face)? {
                return Err(SolidError::null_pointer(OP));
            }
        }
        // (upstream checks pFace1, pFace2, then pSurf1, pSurf2; any NULL is the same
        // SM_ERR_NULL_POINTER)

        // This method assumes that parameter values are increasing from the
        // existing curve to the new curves.

        let mut prev_trim_ivl = pseudo
            .geom(geom)?
            .center_line_curve
            .as_ref()
            .ok_or_else(|| undefined("fillet geometry without a centre line"))?
            .natural_interval();
        let mut next_trim_ivl = next_center_curve.natural_interval();

        let mut found_intersection = false;

        // Find intersection between corresponding UV rail curves
        let mut ii = 0;
        while ii < 2 && !found_intersection {
            // prev and next UVTrimCurve pieces
            let prev_rail_edge = rail(pseudo, geom, ii)?;
            let prev_mate_eu = rail_mate(pseudo, prev_rail_edge)?;
            let prev_uv_curve = mate_uv_curve(pseudo, prev_mate_eu)?
                .ok_or_else(|| undefined("rail mate without a UV trim curve"))?;
            let next_uv_curve = next_uv_curves
                .get(ii)
                .ok_or_else(|| undefined("rail index"))?;

            // Use this little half interval trick to eliminate the possibility of
            // finding closed intersections.  It may be a little dangerous.
            let half = half_interval(prev_uv_curve.natural_interval());

            // intersect prev UVTrimCurve piece with next piece
            let prev_uv = Curve::from(prev_uv_curve.clone());
            let next_uv = Curve::from(next_uv_curve.clone());
            let solutions = prev_uv.global_curve_intersect(
                k,
                &half,
                &next_uv,
                &next_uv.natural_interval(),
                EFF_ZERO_SQRT,
                &CurveIntersectOptions::<SolveCurve>::default(),
            )?;

            // for every prev/next piece UVTrimCurve intersection
            for sol in solutions.as_slice() {
                let prev_t = sol.start.parameters[0];
                let next_t = sol.start.parameters[1];

                // prev piece UVPoint
                let prev_pv =
                    pv(&rail_curve(pseudo, prev_rail_edge)?.evaluate(k, prev_t, 1, true, true)?)?;

                // Compute stepback distance using derivative of curve
                // This is just an approximation.
                let [uv0, uv1] = &next_uv_curves;
                let radius = compute_fillet_radius(fs, k, uv0, uv1, next_t, &prev_pv)?;

                // step distance
                let step_distance = self.step_back_dist_factor * radius;

                // step forward param distance (evaluated after the step back upstream;
                // the step back's zero-tangent return comes first in step_back)
                let next_rail = next_rail_curves
                    .get(ii)
                    .ok_or_else(|| undefined("rail index"))?;
                let prev_tangent = prev_pv[1];
                if is_zero(prev_tangent.length()) {
                    return Err(SolidError::sm_err(OP));
                }
                let next_pv = pv(&next_rail.evaluate(next_t, 1, true, false)?)?;
                step_back(
                    step_distance,
                    prev_t,
                    prev_tangent,
                    next_t,
                    next_pv[1],
                    &mut prev_trim_ivl,
                    &mut next_trim_ivl,
                )?;
                found_intersection = true;
            } // end iter (jj) every next/prev UVtrimCurve intersection solution
            ii += 1;
        } // end iter (ii) both rail curves

        // when Prev and Next UVTrim curves did not intersect
        if !found_intersection {
            let mut d_min = BIG_DOUBLE;
            let mut l_min = 0usize;
            let mut prev_t = prev_trim_ivl.min();
            let mut next_t = next_trim_ivl.max();

            // for both rail curves -
            for kkk in 0..2 {
                let prev_rail_curve = rail_curve(pseudo, rail(pseudo, geom, kkk)?)?;

                // Use this little half interval trick to eliminate the possibility of
                // finding closed intersections.  It may be a little dangerous.
                let half = half_interval(prev_rail_curve.natural_interval());

                // find closest point between prev and Next rail curve pieces
                let next_rail = Curve::from(
                    next_rail_curves
                        .get(kkk)
                        .ok_or_else(|| undefined("rail index"))?
                        .clone(),
                );
                let solutions = prev_rail_curve.global_curve_solve(
                    k,
                    &half,
                    &next_rail,
                    &next_rail.natural_interval(),
                    SolverOperation::Minimize,
                    EFF_ZERO_SQRT,
                    &CurveSolveOptions::<SolveCurve>::default(),
                    SolutionRequested::Single,
                )?;

                // save the closest points on prev/next rail curve pieces
                let first = solutions
                    .get(0)
                    .ok_or_else(|| undefined("no closest point between the rails"))?;
                if first.start.solution_value < d_min {
                    prev_t = first.start.parameters[0];
                    next_t = first.start.parameters[1];
                    d_min = first.start.solution_value;
                    l_min = kkk;
                }
            } // end iter both rail curves

            // prev railCurve nearest 3d point
            let prev_rail_edge = rail(pseudo, geom, l_min)?;
            let prev_pv =
                pv(&rail_curve(pseudo, prev_rail_edge)?.evaluate(k, prev_t, 1, true, true)?)?;

            // Compute stepback distance using derivative of curve
            // This is just an approximation.
            let [uv0, uv1] = &next_uv_curves;
            let radius = compute_fillet_radius(fs, k, uv0, uv1, next_t, &prev_pv)?;

            // step distance scaled by fillet radius
            let step_distance = self.step_back_dist_factor * radius;

            if d_min > 3.0 * step_distance {
                return Err(SolidError::sm_err(OP));
            }

            // stap back param distance (its zero-tangent test precedes the next
            // rail's evaluation, as upstream)
            let prev_tangent = prev_pv[1];
            if is_zero(prev_tangent.length()) {
                return Err(SolidError::sm_err(OP));
            }
            let next_rail = next_rail_curves
                .get(l_min)
                .ok_or_else(|| undefined("rail index"))?;
            let next_pv = pv(&next_rail.evaluate(next_t, 1, true, false)?)?;

            // Now see if we are actually close enough to do this
            step_back(
                step_distance,
                prev_t,
                prev_tangent,
                next_t,
                next_pv[1],
                &mut prev_trim_ivl,
                &mut next_trim_ivl,
            )?;
        } // end Prev and Next UVTrim curves did not intersect check

        // Now Trim Back next and prev centerline curves with new intervals
        next_center_curve.trim(k, &mut next_trim_ivl, TrimMode::Notify)?; // may snap sIvl by tol to existing knots
        let mut prev_center_line = {
            let center = pseudo
                .geom_mut(geom)?
                .center_line_curve
                .as_mut()
                .ok_or_else(|| undefined("fillet geometry without a centre line"))?;
            center.trim(k, &mut prev_trim_ivl, TrimMode::Notify)?; // may snap sIvl by tol to existing knots
            as_bspline(center)
                .cloned()
                .ok_or_else(|| undefined("centre line is not a B-spline"))?
        };

        // Create these blend curves with 10 knots along the U direction
        let new_knots = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9];

        // create blend Centerline piece from PrevEndPoint to NextStartPoint, with the
        // internal knots
        let mut center_blend = blend_curve(
            3,
            &prev_center_line,
            prev_trim_ivl.max(),
            &next_center_curve,
            next_trim_ivl.min(),
            &new_knots,
        )?;

        let mut blend_arc_leng = 0.0;

        // sets the surface offsets back to zero making them equivalent to the base surfaces.
        fs.set_surfaces_to_zero_offset()?;

        // for both rail curves
        let mut uv_blends: Vec<BSplineCurve> = Vec::new();
        for ii in 0..2 {
            let prev_rail_edge = rail(pseudo, geom, ii)?;
            let prev_mate_eu = rail_mate(pseudo, prev_rail_edge)?;
            let mut prev_uv_curve =
                mate_uv_curve(pseudo, prev_mate_eu)?.ok_or_else(|| SolidError::null_pointer(OP))?;
            let mut prev_rail_curve = as_bspline(&rail_curve(pseudo, prev_rail_edge)?)
                .cloned()
                .ok_or_else(|| SolidError::null_pointer(OP))?;

            // Trim PrevRailEdge(ii), UVCurve, and RailCurve intervals
            crate::topology::edge::set_interval(pseudo.brep_mut(), prev_rail_edge, &prev_trim_ivl)?;
            let trimmed = prev_uv_curve.trim(k, &mut prev_trim_ivl, TrimMode::Notify); // may snap sIvl by tol to existing knots
            store_mate_uv(pseudo, prev_mate_eu, &prev_uv_curve)?;
            trimmed?;
            let trimmed = prev_rail_curve.trim(k, &mut prev_trim_ivl, TrimMode::Notify); // may snap sIvl by tol to existing knots
            store_rail_curve(pseudo, prev_rail_edge, &prev_rail_curve)?;
            trimmed?;

            // Trim NextRail(ii) UVCurve and RailCurve intervals
            let [nuv0, nuv1] = &mut next_uv_curves;
            let [nrail0, nrail1] = &mut next_rail_curves;
            let (next_uv, next_rail) = if ii == 0 {
                (nuv0, nrail0)
            } else {
                (nuv1, nrail1)
            };
            next_uv.trim(k, &mut next_trim_ivl, TrimMode::Notify)?; // may snap sIvl by tol to existing knots
            next_rail.trim(k, &mut next_trim_ivl, TrimMode::Notify)?; // may snap sIvl by tol to existing knots

            // Create UV Blend curves and their 3D appproximations
            let uv_blend = blend_curve(
                2,
                &prev_uv_curve,
                prev_trim_ivl.max(),
                next_uv,
                next_trim_ivl.min(),
                &new_knots,
            )?;

            // lift UVCurve to 3D curve and accumulate its arc length
            let lift_3d = lift(fs, k, ii, &uv_blend)?;
            let (arc_leng, _) = Curve::from(lift_3d.clone()).approximate_length(
                k,
                &lift_3d.natural_interval(),
                20,
                false,
            )?;
            blend_arc_leng += arc_leng;

            // add rail(ii) blend UVCurve to array
            uv_blends.push(uv_blend);
        } // end iter (ii) both rail curves

        // parameterize blend on arclenght
        let arc_leng_ivl = Extent1d::from_min_max(0.0, blend_arc_leng / 2.0);
        center_blend.edit_parameterization(&arc_leng_ivl)?;

        // join 3 curves and place output in pCenterBlend
        let mut segments = vec![prev_center_line.clone(), center_blend, next_center_curve];
        let mut joined = BSplineCurve::create_by_joining(&mut segments, None)?;

        // replace Prev CenterLine piece Nurb with joint Nurb curve
        set_from_nurb_of(&mut prev_center_line, &mut joined)?;
        pseudo.geom_mut(geom)?.center_line_curve = Some(Curve::from(prev_center_line.clone()));

        // set output - joined centerline (the geometry's own object upstream)
        let mut out_3d = vec![prev_center_line.clone()];
        let mut new_curves = vec![prev_center_line];
        let mut out_uv: [Option<BSplineCurve>; 2] = [None, None];
        // (sNewCurves order: centerline, then per rail its UV curve and its 3D curve)
        let mut new_curve_slots: Vec<(usize, Slot)> = vec![(0, Slot::Center)];

        // for both rail curves
        for (iii, mut uv_blend) in uv_blends.into_iter().enumerate() {
            let prev_rail_edge = rail(pseudo, geom, iii)?;
            let prev_mate_eu = rail_mate(pseudo, prev_rail_edge)?;
            let mut prev_uv_curve =
                mate_uv_curve(pseudo, prev_mate_eu)?.ok_or_else(|| SolidError::null_pointer(OP))?;
            let mut prev_rail_curve = as_bspline(&rail_curve(pseudo, prev_rail_edge)?)
                .cloned()
                .ok_or_else(|| SolidError::null_pointer(OP))?;

            // adjust UVblend parameterization to arc length
            uv_blend.edit_parameterization(&arc_leng_ivl)?;

            // (the lifted blend's own reparameterization has no use upstream)

            // load sCurves array with Prev, Blend, and Next UVCurves
            let next_uv = next_uv_curves
                .get(iii)
                .cloned()
                .ok_or_else(|| undefined("rail index"))?;
            let mut segments = vec![prev_uv_curve.clone(), uv_blend, next_uv];

            // join the 3 curves into pUVBlend
            let mut joined_uv = BSplineCurve::create_by_joining(&mut segments, None)?;

            // replace PrevUVCurve with pUVBlend NURB definition
            set_from_nurb_of(&mut prev_uv_curve, &mut joined_uv)?;
            store_mate_uv(pseudo, prev_mate_eu, &prev_uv_curve)?;

            // add to sNewCurves array
            new_curve_slots.push((new_curves.len(), Slot::Uv(iii, prev_mate_eu)));
            new_curves.push(prev_uv_curve.clone());

            // set output
            if let Some(slot) = out_uv.get_mut(iii) {
                *slot = Some(prev_uv_curve.clone());
            }

            // lift railUVCurve and add to to output
            let mut lift_3d = lift(fs, k, iii, &prev_uv_curve)?;
            set_from_nurb_of(&mut prev_rail_curve, &mut lift_3d)?;
            store_rail_curve(pseudo, prev_rail_edge, &prev_rail_curve)?;

            out_3d.push(prev_rail_curve.clone());
            new_curve_slots.push((new_curves.len(), Slot::Rail(iii, prev_rail_edge)));
            new_curves.push(prev_rail_curve);
        } // end iter both rail curves

        // the output arrays hold the geometry's (joined) curves
        *curves_3d = out_3d;
        let [uv_out0, uv_out1] = out_uv;
        *uv_curves1 = uv_out0.into_iter().collect();
        *uv_curves2 = uv_out1.into_iter().collect();

        // Make sure all curves are syncronized
        BSplineCurve::synchronize_knots_of_curves(&mut new_curves, EFF_ZERO * 100.0)?;
        for curve in &mut new_curves {
            // (status ignored upstream)
            curve.dampen_knot_spacing(2.0, 3).ok();
        }
        store_new_curves(
            pseudo,
            geom,
            &new_curves,
            &new_curve_slots,
            curves_3d,
            uv_curves1,
            uv_curves2,
        )?;

        // restore FilletSolver offsets
        fs.reload_surface_offsets()?;

        // all done
        Ok(())
    }
}

/// Writes the synchronised and dampened `new_curves` back to the objects they are
/// upstream (the geometry's centre line, its rails' mate UV curves and rail
/// curves, which the output arrays also hold).
#[allow(clippy::too_many_arguments)]
fn store_new_curves(
    pseudo: &mut FilletBrep,
    geom: FilletGeomId,
    new_curves: &[BSplineCurve],
    slots: &[(usize, Slot)],
    curves_3d: &mut [BSplineCurve],
    uv_curves1: &mut [BSplineCurve],
    uv_curves2: &mut [BSplineCurve],
) -> Result<()> {
    for &(index, slot) in slots {
        let curve = new_curves
            .get(index)
            .ok_or_else(|| undefined("curve index"))?;
        match slot {
            Slot::Center => {
                pseudo.geom_mut(geom)?.center_line_curve = Some(Curve::from(curve.clone()));
                if let Some(out) = curves_3d.get_mut(0) {
                    out.clone_from(curve);
                }
            }
            Slot::Uv(rail, mate) => {
                store_mate_uv(pseudo, mate, curve)?;
                let out = if rail == 0 {
                    &mut *uv_curves1
                } else {
                    &mut *uv_curves2
                };
                if let Some(out) = out.get_mut(0) {
                    out.clone_from(curve);
                }
            }
            Slot::Rail(rail, edge) => {
                store_rail_curve(pseudo, edge, curve)?;
                if let Some(out) = curves_3d.get_mut(rail + 1) {
                    out.clone_from(curve);
                }
            }
        }
    }
    Ok(())
}

/// The object a curve of `sNewCurves` is upstream.
#[derive(Debug, Clone, Copy)]
enum Slot {
    /// The geometry's centre line.
    Center,
    /// Rail `.0`'s mate edge use UV trim curve.
    Uv(usize, EdgeuseId),
    /// Rail `.0`'s 3D curve.
    Rail(usize, EdgeId),
}

/// The in-place edit of the mate edge use's UV trim curve object.
fn store_mate_uv(pseudo: &mut FilletBrep, mate: EdgeuseId, curve: &BSplineCurve) -> Result<()> {
    pseudo.brep_mut().edgeuse_mut(mate)?.uv_trim_curve = Some(Curve::from(curve.clone()));
    Ok(())
}

/// The in-place edit of the rail edge's curve object (no `SetCurve` notification).
fn store_rail_curve(
    pseudo: &mut FilletBrep,
    rail_edge: EdgeId,
    curve: &BSplineCurve,
) -> Result<()> {
    pseudo.brep_mut().edge_mut(rail_edge)?.curve = Some(Curve::from(curve.clone()));
    Ok(())
}

/// `pNewFS->SetFilletExecutive(m_pExecutive)` of a fillet corner's temporary solver:
/// the solver reads the executive's self-intersection handler, which every solver
/// the executive loaded mirrors (the first one with an executive in `store`).
pub(crate) fn inherit_executive_handler(
    solver: &mut super::p1::FilletSolver,
    store: &dyn crate::blend::fillet_corner::p3::CornerSolverStore,
) {
    if let Some(loaded) = (0..store.len())
        .filter_map(|i| store.get(i))
        .find(|s| s.executive.is_some())
    {
        solver.executive_self_intersection_handler = loaded.executive_self_intersection_handler;
        solver.executive_curve_blend_sih = loaded.executive_curve_blend_sih;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::fillet_corner::p3::IndexedSolvers;
    use crate::blend::fillet_solver::p1::{FilletExecutiveRef, FilletSolver};

    #[test]
    fn handler_is_smoothing() {
        let h = MakeCurveBlendSih::new(1.5);
        assert_eq!(h.algorithm_type(), HandlingAlgorithmType::Smoothing);
        assert_eq!(h.step_back_dist_factor, 1.5);
    }

    #[test]
    fn temporary_solver_reads_the_executive_handler() {
        let mut unloaded = FilletSolver::new(1e-5, 1e-3, 0.01);
        unloaded.executive_curve_blend_sih = Some(MakeCurveBlendSih::new(9.0));
        let mut loaded = FilletSolver::new(1e-5, 1e-3, 0.01);
        loaded.executive = Some(FilletExecutiveRef);
        loaded.executive_self_intersection_handler = Some(HandlingAlgorithmType::Smoothing);
        loaded.executive_curve_blend_sih = Some(MakeCurveBlendSih::new(1.0));
        let table = [unloaded, loaded];
        let mut temp = FilletSolver::new(1e-5, 1e-3, 0.01);
        inherit_executive_handler(&mut temp, &IndexedSolvers(&table));
        assert_eq!(
            temp.executive_self_intersection_handler,
            Some(HandlingAlgorithmType::Smoothing)
        );
        assert_eq!(
            temp.executive_curve_blend_sih,
            Some(MakeCurveBlendSih::new(1.0))
        );
    }

    #[test]
    fn step_back_halves_overshooting_steps() {
        let mut prev = Extent1d::from_min_max(0.0, 1.0);
        let mut next = Extent1d::from_min_max(0.0, 1.0);
        step_back(
            10.0,
            0.5,
            Vec3::new(1.0, 0.0, 0.0),
            0.5,
            Vec3::new(0.0, 2.0, 0.0),
            &mut prev,
            &mut next,
        )
        .unwrap();
        assert_eq!((prev.min(), prev.max()), (0.0, 0.25));
        assert_eq!((next.min(), next.max()), (0.75, 1.0));
        let zero = step_back(1.0, 0.5, Vec3::ZERO, 0.5, Vec3::ZERO, &mut prev, &mut next);
        assert!(zero.is_err());
    }
}
