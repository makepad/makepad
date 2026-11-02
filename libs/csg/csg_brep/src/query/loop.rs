//! Classification of a parameter-space point against one boundary loop of a face
//! (`SmLoop::ContainsUVPoint`): a winding-number walk along the loop's UV trim curves
//! with on-boundary detection, plus the loop corner / surface side test it uses for
//! poles.
//!
//! Upstream: `source/SMLib/src/SmLoop.cpp`, `source/SMLib/inc/SmLoop.h`.
//!
//! | Rust                         | Upstream                                       |
//! |------------------------------|------------------------------------------------|
//! | [`contains_uv_point`]        | `SmLoop::ContainsUVPoint`                      |
//! | [`UvContainment`]            | its return value and out-params                |
//! | `calc_quad_num` (private)    | `sm_CalcQuadNum` (file static, unit `topology.loopuse`) |
//!
//! Deviations:
//! - **Topology.** The loop is a [`LoopId`] of a [`Brep`], taken mutably: upstream's
//!   `SmEdgeuse::GetOrCreateUVTrimCurve` (`topology.edgeuse_p3`) attaches the missing
//!   UV trim curves to the B-rep while the walk goes on, and edits the edge (interval,
//!   tolerance) it dropped the curve for. Every edge value is read from the B-rep where
//!   upstream reads it, after the creation. `SmLoop::GetUVCornerBoundaries` is
//!   [`crate::topology::loopuse::uv_corner_boundaries`]. `surface` is the caller's copy
//!   of the face's surface (`SmFace::PointClassify` evaluates with out-of-bounds
//!   evaluation enabled on it), else the face's own; the zone tolerance is the face's
//!   (`SmTol::GetZoneTol3d(pSurface)` of the face's own surface, whose owner is the
//!   face; the arena surfaces carry no owner). A face without surface gives
//!   [`PointContainment::Unknown`], as upstream.
//!   The same walk accepts prepared, read-only topology internally; a missing trim
//!   reports [`SolidError::Unported`] only when the walk needs it, without eager work.
//!   Temporary surface copies retain exact evaluation data and owner tolerances.
//! - **UV trim curves.** A failing status of `GetOrCreateUVTrimCurve` (a recorded drop
//!   failure of the existing curve, or a failed creation) or a missing curve gives
//!   `SM_POC_UNKNOWN`, as upstream.
//! - **Poles.** `SmSurface::GetSingularities` (unit `surface.surface_p3`) is only
//!   reached when the point lies on a side of the surface's natural domain and an edge
//!   use end of the loop does too.
//! - **Ignored statuses.** Upstream ignores the status of `EvaluatePoint` /
//!   `Evaluate` of the surface and the trim curves and then reads the outputs they did
//!   not write (`SM_UNDEF_DOUBLE` defaults, or the previous step's derivatives); the
//!   port returns their error instead (no valid input reaches it).
//!   `DropPoint` and `LocalPointSolve` failures are "no answer" (both reset their
//!   success flag first upstream). [`SolidError::Unported`] from any callee is
//!   returned.
//! - `dTol3d` is unused upstream and is kept as `_tol_3d`.
//! - **Loops.** Both walk loops keep upstream's bounds (1000 steps per edge use and
//!   1000 re-evaluations per step, then `SM_POC_UNKNOWN`); the `goto Got_dThisParam`
//!   re-evaluations are an inner loop. No recursion.

use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p1::line_point_distance;
use crate::geom::global_solver::SolverOperation;
use crate::geom::point_set::PointContainment;
use crate::geom::solution_array::Solution;
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec3, EFF_ZERO};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::p1::map_to_2d;
use crate::surface::surface::Surface;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::loopuse::{uv_corner_boundaries_with_topology, QueryTopology};
use crate::topology::{Brep, EdgeId, EdgeuseId, LoopId};

/// Upper bound of the steps along one edge use and of the re-evaluations of one step
/// (upstream's infinite-loop guards).
const MAX_LOOP_COUNT: u32 = 1000;

/// The result of [`contains_uv_point`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvContainment {
    /// `SM_POC_INSIDE`, `SM_POC_OUTSIDE`, `SM_POC_ON_BOUNDARY`, or `SM_POC_UNKNOWN`
    /// on failure.
    pub containment: PointContainment,
    /// `rpOnEdge`: the edge the point lies on (only for `OnBoundary`).
    pub on_edge: Option<EdgeId>,
    /// `rdEdgeParam`: its edge parameter (only for `OnBoundary`; else `0`).
    pub edge_param: f64,
    /// `eOrientation`: [`OrientType::Same`] for a counter-clockwise loop around an
    /// inside point, [`OrientType::Opposite`] for a clockwise one (only for `Inside`;
    /// else [`OrientType::Unknown`]).
    pub orientation: OrientType,
}

impl UvContainment {
    const fn of(containment: PointContainment) -> Self {
        Self {
            containment,
            on_edge: None,
            edge_param: 0.0,
            orientation: OrientType::Unknown,
        }
    }

    const fn on(edge: EdgeId, edge_param: f64) -> Self {
        Self {
            containment: PointContainment::OnBoundary,
            on_edge: Some(edge),
            edge_param,
            orientation: OrientType::Unknown,
        }
    }
}

/// `sm_CalcQuadNum` (unit `topology.loopuse`): the quadrant of `test` in a frame
/// centred on `origin` (`1 | 0` above, `2 | 3` below).
fn calc_quad_num(origin: Point3, test: Point3) -> i32 {
    crate::topology::loopuse::calc_quad_num(origin, test) as i32
}

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input("query::loop::contains_uv_point", reason)
}

/// The edge of an edge use (`GetEdge`, dereferenced by `ContainsUVPoint`).
fn edge_of(b: &Brep, eu: EdgeuseId) -> Result<EdgeId> {
    eu1::edge(b, eu)?.ok_or_else(|| null("ContainsUVPoint: edge use without edge"))
}

/// `SmSurface::GetSingularities`: the sides of the natural domain that are poles.
fn surface_singularities(k: &Kernel, surface: &Surface) -> Result<u32> {
    Ok(surface.get_singularities(k, false, false)?.singular)
}

/// `DropPoint` of the test point onto an edge curve (`SM_SO_INTERSECT`, no snapping
/// vector, no guess): the parameter when it lies within `tol`. A failing drop is no
/// drop (its status is ignored upstream).
fn drop_on_edge(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    point: Point3,
    tol: f64,
) -> Result<Option<f64>> {
    match curve.drop_point(
        k,
        interval,
        point,
        None,
        tol,
        None,
        SolverOperation::Intersect,
    ) {
        Ok(d) => Ok(d.success.then_some(d.parameter)),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        Err(_) => Ok(None),
    }
}

/// `LocalPointSolve(SM_SO_MINIMIZE)` of the test point on a UV trim curve: whether it
/// converged and the solution. A failing solve has not converged (its status is
/// ignored upstream and the flag is reset first).
fn local_minimize(
    k: &Kernel,
    trim: &Curve,
    interval: &Extent1d,
    point: Point3,
    tol_uv: f64,
    guess: f64,
) -> Result<Option<Solution<()>>> {
    let opts = PointSolveOptions {
        distance_tolerance: Some(tol_uv),
        ..PointSolveOptions::default()
    };
    match trim.local_point_solve::<()>(k, interval, SolverOperation::Minimize, point, &opts, guess)
    {
        Ok((true, sol)) => Ok(Some(sol)),
        Err(e @ SolidError::Unported { .. }) => Err(e),
        Ok((false, _)) | Err(_) => Ok(None),
    }
}

/// The point and unit tangent (in the edge use's direction) of a trim curve at `t`,
/// with the tangent's length (`1` when below `SM_EFF_ZERO`).
fn eval_with_tangent(
    k: &Kernel,
    trim: &Curve,
    t: f64,
    orient: OrientType,
) -> Result<(Point3, Vec3, f64)> {
    let der = trim.evaluate(k, t, 1, false, true)?;
    let pt = der.first().copied().unwrap_or(Vec3::UNDEF);
    let mut tan = der.get(1).copied().unwrap_or(Vec3::UNDEF);
    if orient == OrientType::Opposite {
        tan = -tan;
    }
    let mut len = tan.length();
    if len < EFF_ZERO {
        len = 1.0;
    }
    tan /= len;
    Ok((pt, tan, len))
}

/// The unit vector from `origin` to `p` (unscaled when shorter than `SM_EFF_ZERO`)
/// and the distance.
fn unit_from(origin: Point3, p: Point3) -> (Vec3, f64) {
    let mut v = p - origin;
    let dist = v.length();
    if dist > EFF_ZERO {
        v /= dist;
    }
    (v, dist)
}

/// `SmLoop::ContainsUVPoint`: classifies the parameter-space point `uv` against loop
/// `l` of the B-rep only (other loops of the face are not looked at) by counting the
/// quadrant changes of the loop's UV trim curves around it: `±4` is inside
/// (counter-clockwise [`OrientType::Same`], clockwise [`OrientType::Opposite`]), `0`
/// outside, with one quadrant of noise allowed. The point is on the boundary when its
/// surface point lies on an edge curve within the edge's zone tolerance, when a trim
/// curve passes within the face's zone tolerance mapped to UV space, or when it lies
/// on a gap between consecutive edge uses (a pole); a point on a pole side shared by
/// the loop is on the boundary too. A vertex loop contains nothing. A UV trim curve
/// the edge use does not have is made (`GetOrCreateUVTrimCurve`) and attached to the
/// B-rep. `_tol_3d` is unused, as upstream.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] where upstream dereferences null (a loop without loop
/// use, a loop use without face use, a face use without face, an edge use without
/// edge, an edge without curve); [`SolidError::Unported`] from the callees (module
/// docs); the errors of the surface and trim curve evaluations (see the module
/// Deviations).
pub fn contains_uv_point(
    k: &Kernel,
    b: &mut Brep,
    l: LoopId,
    surface: Option<&Surface>,
    uv: Point2,
    tol_3d: f64,
) -> Result<UvContainment> {
    contains_uv_point_with_topology(k, &mut QueryTopology::Mutable(b), l, surface, uv, tol_3d)
}

/// Shared containment walk, with lazy trim creation limited to mutable topology.
pub(crate) fn contains_uv_point_with_topology(
    k: &Kernel,
    b: &mut QueryTopology<'_>,
    l: LoopId,
    surface: Option<&Surface>,
    uv: Point2,
    _tol_3d: f64,
) -> Result<UvContainment> {
    // Work from Loopuses, because they will give us an ordered list of Edgeuses.
    let lu = b
        .loop_(l)?
        .loopuse
        .ok_or_else(|| null("ContainsUVPoint: loop without loop use"))?;
    let eus = eu1::loopuse_edgeuses(b, lu)?;
    let Some(&last_eu) = eus.last() else {
        // could be a vertex loopuse
        return Ok(UvContainment::of(PointContainment::Outside));
    };

    // check state - must have a Face and surface
    let fu = b
        .loopuse(lu)?
        .faceuse
        .ok_or_else(|| null("ContainsUVPoint: loop use without face use"))?;
    let f = b
        .faceuse(fu)?
        .face
        .ok_or_else(|| null("ContainsUVPoint: face use without face"))?;
    let face_rec = b.face(f)?;
    let own_surface;
    let surface: &Surface = match surface {
        Some(s) => s,
        None => match face_rec.surface.as_ref() {
            Some(s) => {
                // Keep the exact evaluation data and owner tolerance while the
                // mutable walk may attach trim curves to the B-rep.
                own_surface = s.clone_for_evaluation()?;
                &own_surface
            }
            None => return Ok(UvContainment::of(PointContainment::Unknown)),
        },
    };
    // `SmTol::GetZoneTol3d(pSurface)`: the surface is the face's (the face is its owner
    // upstream), so the face's tolerance
    let zone_tol = face_rec.tolerance;

    let srf_uv_domain = surface.natural_uv_domain_with_nurb(k)?;
    let point_3d = surface.evaluate_point(k, uv)?;
    let uv_pt = Point3::new(uv.x, uv.y, 0.0);

    // Get a tolerance in uv space.
    // Scale the input tolerance by the UV deriv size.
    let mut tol_uv = map_to_2d(k, zone_tol, uv, surface)?;

    // minimize pole checking - if UVPnt on boundary -then-> if Loop on boundary -then->
    // if GetSingularities -then-> expensive check

    // Is crUVPoint on Surface Natural UVBoundary (where poles are allowed)
    let uv_boundaries = srf_uv_domain.point_boundaries(uv, tol_uv);

    // When crUVPoint is on Surface UVBoundary - does Loop have any edgeuse ends on
    // Surface Natural UVBoundary
    let corners = if uv_boundaries != 0 {
        uv_corner_boundaries_with_topology(k, b, l, &srf_uv_domain, tol_uv)?
    } else {
        crate::topology::loopuse::UvCorners::default()
    };
    let loop_boundaries = corners.boundaries;

    // When point and loop touch the surface UVBoundaries - does surface have poles
    let poles = if uv_boundaries & loop_boundaries != 0 {
        surface_singularities(k, surface)?
    } else {
        0
    };

    // remember when checking for points on poles is required
    let common_poles = poles & loop_boundaries & uv_boundaries;

    // when crUVPoint and Loop share a common singular edge - classify the point
    if common_poles != 0 {
        let mut out = UvContainment::of(PointContainment::OnBoundary);
        // set output with first hit on a singluar boundary
        for ((edge, bnd), param) in corners
            .on_edges
            .iter()
            .zip(&corners.on_boundaries)
            .zip(&corners.on_params)
        {
            if bnd & common_poles != 0 {
                out.on_edge = *edge;
                out.edge_param = *param;
                break;
            }
        }

        // UVPoints on Singular boundaries shared by the loop are always classified
        // ON_BOUNDARY
        return Ok(out);
    }

    // check input:
    if tol_uv < EFF_ZERO * 100.0 {
        tol_uv = EFF_ZERO * 100.0;
    }

    // Locals:
    let mut accum: i32 = 0; // This is the main result: 0, 4 or -4 when done.
    let mut this_quad_num: i32 = 0; // Quadrant numbers: 0-3 going CCW.
    let mut this_param = 0.0;
    // Uninitialised upstream; first written by the walk of an edge use longer than
    // SM_EFF_ZERO.
    let mut this_uv = Vec3::UNDEF;

    // Start with the end of the final Edgeuse.
    let mut last_end_edge = edge_of(b, last_eu)?;
    let mut last_end_param = {
        let ivl = b.edge(last_end_edge)?.interval;
        if eu1::orientation(b, last_eu)? == OrientType::Same {
            ivl.max()
        } else {
            ivl.min()
        }
    };
    if !b.ensure_trim_curve(k, last_eu)? {
        // stat != SM_SUCCESS || pThisUVCurve == NULL
        return Ok(UvContainment::of(PointContainment::Unknown));
    }
    let mut last_end_uv = {
        let last_trim = eu1::uv_trim_curve(b, last_eu)?
            .0
            .ok_or_else(|| null("ContainsUVPoint: edge use without UV trim curve"))?;
        last_trim.evaluate_point(k, last_end_param)?
    };
    let mut last_end_quad_num = calc_quad_num(uv_pt, last_end_uv);

    // Now start the outer loop, on Edgeuses.
    for &this_eu in &eus {
        let this_edge = edge_of(b, this_eu)?;

        // low work: see if TgtPoint is on this EdgeCurve
        let on_edge_curve = {
            let edge = b.edge(this_edge)?;
            // the drop builds the edge curve's cache at the edge's tolerance
            // (SmTol::GetApproxTol3d(&m_crCurve) reads pEdge->GetTolerance() through the
            // owner)
            let mut curve = edge
                .curve
                .as_ref()
                .ok_or_else(|| null("ContainsUVPoint: edge without curve"))?
                .clone_for_evaluation()?;
            curve.set_owner_zone_tol_3d(Some(edge.tolerance));
            drop_on_edge(k, &curve, &edge.interval, point_3d, edge.tolerance)?
        };
        if let Some(param) = on_edge_curve {
            // TgtPoint is on EdgeCurve - set output and return
            return Ok(UvContainment::on(this_edge, param));
        }

        if !b.ensure_trim_curve(k, this_eu)? {
            // we can't work
            return Ok(UvContainment::of(PointContainment::Unknown));
        }
        let trim = eu1::uv_trim_curve(b, this_eu)?
            .0
            .ok_or_else(|| null("ContainsUVPoint: edge use without UV trim curve"))?;

        // Grab some info about this EU and its uv curve (the edge as the creation left
        // it).
        let orient = eu1::orientation(b, this_eu)?;
        let domain = b.edge(this_edge)?.interval;
        let (mut prev_param, end_param) = if orient == OrientType::Opposite {
            (domain.max(), domain.min())
        } else {
            (domain.min(), domain.max())
        };

        // These have to do with step size:
        // (dDefaultParamStep: every step below overwrites it, so it is never used)
        let _default_param_step = domain.length() / trim.number_control_points()? as f64;
        let min_param_step = domain.length() / 1000.0;
        let max_param_step = domain.length() / 20.0;
        let curve_is_linear = trim.is_linear(k, tol_uv, false)?.linear;

        // Eval info for start point of this EU.
        let (mut prev_uv, mut prev_uv_tan, mut prev_uv_tan_len) =
            eval_with_tangent(k, trim, prev_param, orient)?;
        let (prev_vec, mut prev_dist) = unit_from(uv_pt, prev_uv);
        let mut prev_cos = prev_vec.dot(prev_uv_tan);

        // Note: if dPrevDist < tol, then this uv pt is on the edge.
        if prev_dist < tol_uv {
            // We should refine the point.
            if let Some(sol) = local_minimize(k, trim, &domain, uv_pt, tol_uv, prev_param)? {
                // since the guess was good enough - we expect the refined solution to
                // be as good or better
                if sol.start.solution_value <= tol_uv {
                    // return the refined answer.
                    return Ok(UvContainment::on(this_edge, sol.start.param(0)?));
                }
            }
            // Should not arrive here... (inconsistent classification; continue)
        }

        // Calculate the initial quadrant.
        let mut prev_quad_num = calc_quad_num(uv_pt, prev_uv);

        // Check here for non-closed loops.
        let delta = prev_quad_num - last_end_quad_num;

        // Check for and accumulate any change in quadrant.
        if delta != 0 {
            // First check 'on'.
            let (seg_dist, seg_param) =
                line_point_distance(last_end_uv, prev_uv - last_end_uv, uv_pt);
            if seg_dist <= tol_uv && seg_param > -EFF_ZERO && seg_param < 1.0 + EFF_ZERO {
                // On. Return closer end point.
                return Ok(if seg_param <= 0.5 {
                    UvContainment::on(last_end_edge, last_end_param)
                } else {
                    UvContainment::on(this_edge, prev_param)
                });
            }

            if delta == 1 || delta == -3 {
                accum += 1;
            } else if delta == -1 || delta == 3 {
                accum -= 1;
            } else {
                // +- 2: diagonal.  Here, we're just shooting from one point to the
                // next, so treat it as a straight line. See whether we're heading CW
                // or CCW around the test point: use a cross product.
                let cross = (prev_uv - last_end_uv).cross(uv_pt - last_end_uv);
                if cross.z > 0.0 {
                    accum += 2; // CCW
                } else {
                    accum -= 2; // CW
                }
            }
        } // end check for change from end of last curve

        // A flag to be used in this next loop:
        let mut did_local_solve_last_time = false;

        // Now step along this Edgeuse.
        let mut loop_count1 = 0;
        while (end_param - prev_param).abs() > EFF_ZERO {
            loop_count1 += 1;
            if loop_count1 > MAX_LOOP_COUNT {
                // Infinite Loop.
                return Ok(UvContainment::of(PointContainment::Unknown));
            }

            // Get the parameter for the next sample point.
            let param_step = if curve_is_linear {
                // For straight lines, we can shoot right to the end.
                (end_param - prev_param).abs()
            } else {
                // When we're far from the test point, we can take bigger steps.
                // Also increase step size if we're moving away from the test point:
                // a fraction of the distance to the test point, from 1/4 (directly
                // towards) through 1/2 (perpendicular) to 1 (directly away).
                // Fraction = 1/8 cos^2  +  3/8 cos  +  1/2.
                let frac = prev_cos * (prev_cos + 3.0) / 8.0 + 0.5;
                let move_dist = frac * prev_dist;
                let mut param_step = move_dist / prev_uv_tan_len;

                if param_step < min_param_step {
                    param_step = min_param_step;
                }
                if param_step > max_param_step {
                    param_step = max_param_step;
                }
                param_step
            };

            // Got param step, set dThisParam
            if orient == OrientType::Same {
                this_param = prev_param + param_step;
                if this_param > end_param {
                    this_param = end_param;
                }
            } else {
                this_param = prev_param - param_step;
                if this_param < end_param {
                    this_param = end_param;
                }
            }

            // Here we have the next candidate curve parameter, for a step. Evaluate
            // the curve and get all of the state variables we need; when a better
            // value is found, come back here (upstream `goto Got_dThisParam`).
            let mut loop_count2 = 0;
            let (this_uv_tan, this_uv_tan_len, this_dist, this_cos) = loop {
                loop_count2 += 1;
                if loop_count2 > MAX_LOOP_COUNT {
                    // Infinite Loop2.
                    return Ok(UvContainment::of(PointContainment::Unknown));
                }

                let (uv_at, this_uv_tan, this_uv_tan_len) =
                    eval_with_tangent(k, trim, this_param, orient)?;
                this_uv = uv_at;
                let (this_vec, this_dist) = unit_from(uv_pt, this_uv);
                let this_cos = this_vec.dot(this_uv_tan);

                // A check for too big a step: if the direction changed by more than,
                // say, 90 degrees. But, if the curve is linear, and it doubles back on
                // itself (it happens), we don't have to step along, can just shoot to
                // the end. [Fillet regression 2:237]
                if !curve_is_linear
                    && prev_uv_tan.dot(this_uv_tan) < 0.0
                    // Cusps happen.  In case of a turnaround in a very short interval,
                    // just continue on with the next parameter value.
                    && (this_param - prev_param).abs() > min_param_step
                {
                    this_param = (this_param + prev_param) / 2.0;
                    continue;
                }

                // Here we need to check whether this segment contains the test point.
                let mut check_on_curve = false;

                // If we do check on-curve, we will use dSegParam to find a guess
                // parameter for LocalPointSolve.
                let mut seg_param = 0.5; // just in case nothing else sets it...

                // First, if the current point equals the test point:
                if this_dist <= tol_uv {
                    check_on_curve = true;
                    seg_param = 1.0; // end of segment
                }

                // If the previous point was approaching and this point is receding:
                if !check_on_curve
                    && !did_local_solve_last_time
                    && (prev_cos < -0.001 && this_cos > 0.001)
                {
                    let (seg_dist, sp) = line_point_distance(prev_uv, this_uv - prev_uv, uv_pt);
                    seg_param = sp;

                    // Check whether we should check whether the pt is on the curve.
                    check_on_curve = seg_dist <= tol_uv;

                    // tidy up a projection exterior to the line segment
                    seg_param = seg_param.clamp(0.0, 1.0);

                    // Check whether this curve segment could possibly reach the test pt.
                    if !check_on_curve && !curve_is_linear {
                        // Extend start & end tangents to see whether they could reach
                        // as far as the test point.

                        // We know that dPrevCos < 0...
                        let chk_pt = prev_uv
                            + prev_uv_tan * prev_uv_tan_len * (this_param - prev_param).abs();
                        let (chk_pt_dist, _) =
                            line_point_distance(prev_uv, this_uv - prev_uv, chk_pt);
                        check_on_curve = chk_pt_dist >= seg_dist;

                        // ... and that dThisCos > 0.
                        if !check_on_curve {
                            // Try other end of segment.
                            let chk_pt = this_uv
                                - this_uv_tan * this_uv_tan_len * (this_param - prev_param).abs();
                            let (chk_pt_dist, _) =
                                line_point_distance(prev_uv, this_uv - prev_uv, chk_pt);
                            check_on_curve = chk_pt_dist >= seg_dist;
                        }
                    } // end more-detailed check for possible on-curve.
                } // end cosine-change check, for passing the test point.

                did_local_solve_last_time = false;

                if check_on_curve {
                    // We have a very good guess parameter for a local solve.
                    let guess_t = prev_param + seg_param * (this_param - prev_param);

                    // Interval: between prev pt and this pt.  Loosen up a bit.
                    let mut crv_ivl = Extent1d::default();
                    crv_ivl.add_value(prev_param);
                    crv_ivl.add_value(this_param);
                    crv_ivl.expand_absolute(EFF_ZERO);

                    if let Some(sol) = local_minimize(k, trim, &crv_ivl, uv_pt, tol_uv, guess_t)? {
                        if sol.start.solution_value <= tol_uv {
                            return Ok(UvContainment::on(this_edge, sol.start.param(0)?));
                        }

                        // We have a local min/max to the curve: do another point at
                        // the solution value, to avoid the case where the test point
                        // is between the segment chord and the actual curve. The
                        // solution will have a cosine of zero: use that fact to avoid
                        // getting stuck.
                        let solve_t = sol.start.param(0)?;
                        if crv_ivl.contains_value(solve_t, 0.0001) {
                            this_param = solve_t;
                            did_local_solve_last_time = true;
                            continue;
                        }
                    } // end if bFoundAnswer
                } // end LocalPointSolve check for test pt on curve

                // Got the next sample point, find its quadrant.
                this_quad_num = calc_quad_num(uv_pt, this_uv);
                let delta = this_quad_num - prev_quad_num;

                // Check for and accumulate any change in quadrant.
                if delta == 0 {
                } else if delta == 1 || delta == -3 {
                    accum += 1;
                } else if delta == -1 || delta == 3 {
                    accum -= 1;
                } else {
                    // +- 2: diagonal. Just take a smaller step; is doesn't have to be
                    // much smaller.
                    this_param = prev_param + 0.75 * (this_param - prev_param);
                    continue;
                }
                break (this_uv_tan, this_uv_tan_len, this_dist, this_cos);
            };

            // Update This/Prev data.
            prev_param = this_param;
            prev_uv = this_uv;
            prev_uv_tan = this_uv_tan;
            prev_uv_tan_len = this_uv_tan_len;
            prev_dist = this_dist;
            prev_cos = this_cos;
            prev_quad_num = this_quad_num;
        } // end while stepping along this EU

        // Update the last-end stuff, for checking non-closed loops.
        last_end_edge = this_edge;
        last_end_param = this_param;
        last_end_uv = this_uv;
        last_end_quad_num = this_quad_num;
    } // end loop on all EU's

    // Check the result: iAccum
    if accum == 0 {
        return Ok(UvContainment::of(PointContainment::Outside));
    } else if accum == 4 || accum == -4 {
        return Ok(inside(accum));
    }

    // It's common for uv points to have 'exactly' the same u or v value as the input
    // point.  In that case, a single bit of noise can change the quadrant
    // classification by one. So we'll allow a one-quadrant error here.
    if accum == 1 || accum == -1 {
        return Ok(UvContainment::of(PointContainment::Outside));
    } else if accum == 3 || accum == -3 || accum == 5 || accum == -5 {
        return Ok(inside(accum));
    }

    // This is an error.  The only bit-noise error should be at the start/end of the
    // loop, and it can't be off by two.
    Ok(UvContainment::of(PointContainment::Unknown))
}

/// `SM_POC_INSIDE` with the loop orientation of the winding sign.
fn inside(accum: i32) -> UvContainment {
    UvContainment {
        containment: PointContainment::Inside,
        on_edge: None,
        edge_param: 0.0,
        orientation: if accum > 0 {
            OrientType::Same
        } else {
            OrientType::Opposite
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::Line;
    use crate::geom::extent1d::Extent1d;
    use crate::geom::extent2d::Extent2d;
    use crate::math::Vec2;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::crv_basic::Curve as NurbCurve;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::surface::plane::Plane;
    use crate::topology::{
        Edge, Edgeuse, EdgeuseOwner, Face, FaceId, Faceuse, FaceuseId, Loop, Loopuse, LoopuseId,
        LoopuseStart, Region, RegionId, Shell, ShellId, Vertex, VertexId,
    };

    fn plane() -> Surface {
        Surface::from(Plane::new(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(-20.0, -20.0, 20.0, 20.0),
        ))
    }

    fn trim(a: [f64; 2], b: [f64; 2]) -> Curve {
        let pw = vec![
            CPoint::new(a[0], a[1], 0.0, NOW),
            CPoint::new(b[0], b[1], 0.0, NOW),
        ];
        let cur = NurbCurve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        Curve::from(BSplineCurve::from_nurb(cur, 2, false))
    }

    /// A one-face B-rep on `surface` whose loop is the square `[-5, 5]^2`
    /// counter-clockwise (or clockwise) as 4 line edges, each edge use with its UV
    /// trim curve (except the ones in `without_trim`).
    fn square(surface: Surface, ccw: bool, without_trim: &[usize]) -> Brep {
        let mut v = [[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        if !ccw {
            v.reverse();
        }
        let mut b = Brep::new();
        b.add_region_record(Region {
            in_brep: true,
            ..Region::default()
        })
        .unwrap();
        b.add_shell_record(Shell {
            faces: vec![FaceId::from_index(0)],
            region: Some(RegionId::from_index(0)),
            ..Shell::default()
        })
        .unwrap();
        b.add_face_record(Face {
            loops: vec![LoopId::from_index(0)],
            surface: Some(surface),
            faceuse: Some(FaceuseId::from_index(0)),
            tolerance: 1e-6,
            ..Face::new(ShellId::from_index(0))
        })
        .unwrap();
        b.add_loop_record(Loop {
            face: FaceId::from_index(0),
            edges: Vec::new(),
            loopuse: Some(LoopuseId::from_index(0)),
        })
        .unwrap();
        b.add_faceuse_record(Faceuse {
            shell: Some(ShellId::from_index(0)),
            face: Some(FaceId::from_index(0)),
            mate: None,
            orientation: OrientType::Same,
            loopuses: vec![LoopuseId::from_index(0)],
            ..Faceuse::default()
        })
        .unwrap();
        b.add_loopuse_record(Loopuse {
            faceuse: Some(FaceuseId::from_index(0)),
            loop_: Some(LoopId::from_index(0)),
            mate: None,
            start: LoopuseStart::Edgeuse(Some(EdgeuseId::from_index(0))),
            orientation: OrientType::Same,
        })
        .unwrap();
        for i in 0..4_u32 {
            let (p, q) = (v[i as usize], v[(i as usize + 1) % 4]);
            b.add_vertex_record(Vertex {
                point: Vec3::new(p[0], p[1], 0.0),
                tolerance: 1e-6,
                ..Vertex::default()
            })
            .unwrap();
            b.add_edge_record(Edge {
                vertices: [VertexId::from_index(i), VertexId::from_index((i + 1) % 4)],
                edgeuses: vec![EdgeuseId::from_index(i)],
                interval: Extent1d::from_min_max(0.0, 1.0),
                tolerance: 1e-6,
                curve: Some(Curve::from(Line::from_points(
                    Vec3::new(p[0], p[1], 0.0),
                    Vec3::new(q[0], q[1], 0.0),
                    3,
                ))),
            })
            .unwrap();
            b.add_edgeuse_record(Edgeuse {
                edge: Some(EdgeId::from_index(i)),
                owner: EdgeuseOwner::Loopuse(Some(LoopuseId::from_index(0))),
                ccw: Some(EdgeuseId::from_index((i + 1) % 4)),
                cw: Some(EdgeuseId::from_index((i + 3) % 4)),
                orientation: OrientType::Same,
                uv_trim_curve: (!without_trim.contains(&(i as usize))).then(|| trim(p, q)),
                ..Edgeuse::default()
            })
            .unwrap();
        }
        b
    }

    fn contains(b: &mut Brep, uv: [f64; 2]) -> Result<UvContainment> {
        contains_uv_point(
            &Kernel::default(),
            b,
            LoopId::from_index(0),
            None,
            Point2::new(uv[0], uv[1]),
            0.0,
        )
    }

    #[test]
    fn square_inside_outside_on() {
        let mut b = square(plane(), true, &[]);
        let r = contains(&mut b, [0.0, 0.0]).unwrap();
        assert_eq!(r.containment, PointContainment::Inside);
        assert_eq!(r.orientation, OrientType::Same);
        let r = contains(&mut b, [8.0, 1.0]).unwrap();
        assert_eq!(r.containment, PointContainment::Outside);
        assert_eq!(r.on_edge, None);
        let r = contains(&mut b, [5.0, 0.0]).unwrap();
        assert_eq!(r.containment, PointContainment::OnBoundary);
        assert_eq!(r.on_edge, Some(EdgeId::from_index(1)));
        assert!((r.edge_param - 0.5).abs() < 1e-12);

        let mut b = square(plane(), false, &[]);
        let r = contains(&mut b, [1.0, -2.0]).unwrap();
        assert_eq!(r.containment, PointContainment::Inside);
        assert_eq!(r.orientation, OrientType::Opposite);
    }

    #[test]
    fn vertex_loop_face_and_missing_trim() {
        // a loop use without edge uses (a vertex loop) contains nothing
        let mut b = square(plane(), true, &[]);
        b.loopuse_mut(LoopuseId::from_index(0)).unwrap().start = LoopuseStart::Vertexuse(None);
        let r = contains(&mut b, [0.0, 0.0]).unwrap();
        assert_eq!(r, UvContainment::of(PointContainment::Outside));

        // a face without surface cannot classify
        let mut b = square(plane(), true, &[]);
        b.face_mut(FaceId::from_index(0)).unwrap().surface = None;
        let r = contains(&mut b, [0.0, 0.0]).unwrap();
        assert_eq!(r.containment, PointContainment::Unknown);

        // a missing trim curve is made (GetOrCreateUVTrimCurve): the lines drop onto the
        // plane or the creation defers on a callee that is not ported yet, but the
        // missing curve itself is not a stub
        let mut b = square(plane(), true, &[3]);
        let r = contains(&mut b, [0.0, 0.0]);
        assert!(!matches!(
            r,
            Err(SolidError::Unported {
                unit: "topology.edgeuse_p3",
                ..
            })
        ));
    }

    #[test]
    fn corner_boundaries_and_poles() {
        // a point on a domain side with the loop on that side reaches GetSingularities
        let dom = Extent2d::from_bounds(-5.0, -5.0, 5.0, 5.0);
        let s2 = Surface::from(Plane::new(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec2::new(1.0, 1.0),
            &dom,
        ));
        let mut b = square(s2, true, &[]);
        // a plane has no singular sides, so GetSingularities is wired and no longer defers
        let r = contains(&mut b, [-5.0, 0.0]);
        assert!(!matches!(r, Err(SolidError::Unported { .. })));
    }

    #[test]
    fn prepared_walk_keeps_lazy_trim_order() {
        let k = Kernel::default();
        // The final trim is read first. The first edge's 3D drop then exits before
        // needing its missing trim or the missing trims of later edge uses.
        let mut b = square(plane(), true, &[0, 1, 2]);
        let prepared = contains_uv_point_with_topology(
            &k,
            &mut QueryTopology::Prepared(&b),
            LoopId::from_index(0),
            None,
            Point2::new(0.0, -5.0),
            0.0,
        )
        .unwrap();
        assert_eq!(prepared.containment, PointContainment::OnBoundary);
        assert_eq!(prepared.on_edge, Some(EdgeId::from_index(0)));
        assert_eq!(prepared, contains(&mut b, [0.0, -5.0]).unwrap());
        for i in 0..3 {
            assert!(b
                .edgeuse(EdgeuseId::from_index(i))
                .unwrap()
                .uv_trim_curve
                .is_none());
        }

        // Conversely, a missing final trim is needed even for that same 3D drop.
        let b = square(plane(), true, &[3]);
        assert!(matches!(
            contains_uv_point_with_topology(
                &k,
                &mut QueryTopology::Prepared(&b),
                LoopId::from_index(0),
                None,
                Point2::new(0.0, -5.0),
                0.0,
            ),
            Err(SolidError::Unported {
                unit: "topology.edgeuse_p3",
                ..
            })
        ));
    }

    #[test]
    fn quadrants() {
        let o = Point3::ZERO;
        assert_eq!(calc_quad_num(o, Point3::new(1.0, 1.0, 0.0)), 0);
        assert_eq!(calc_quad_num(o, Point3::new(-1.0, 1.0, 0.0)), 1);
        assert_eq!(calc_quad_num(o, Point3::new(-1.0, -1.0, 0.0)), 2);
        assert_eq!(calc_quad_num(o, Point3::new(1.0, -1.0, 0.0)), 3);
        assert_eq!(calc_quad_num(o, o), 0);
    }
}
