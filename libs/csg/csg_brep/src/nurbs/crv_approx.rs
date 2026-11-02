//! Curve approximation: the number of control points a non-rational circle needs,
//! the non-rational fit of a circular arc, and the global non-rational approximation
//! of a NURBS curve within a tolerance.
//!
//! Upstream: `source/SMLib/src/NL_CrvApprox.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvAdv.h`).
//!
//! | Rust                               | Upstream                         |
//! |------------------------------------|----------------------------------|
//! | [`calc_num_cpts_to_approx_arc`]    | `N_CalcNumCPtsToApproxArc`       |
//! | [`arc_cpts_function`]              | the table fit of `N_CalcNumCPtsToApproxArc` (`sfn` NULL on input) |
//! | [`approx_circ_arc_with_crv_data`], [`CircArcData`] | `N_ApproxCircArcWithCrvData` |
//! | [`approx_nurbs_with_non_rat_crv`]  | `N_ApproxNurbsWithNonRatCrv`     |
//!
//! Deviations:
//! - `N_CalcNumCPtsToApproxArc` takes an optional surface function in and out
//!   (`sfn`, kept on the caller's stack when `flg == NL_YES`). Here the function is
//!   an optional borrowed argument; a caller that wants to keep it builds it once
//!   with [`arc_cpts_function`] and passes it in. The `NL_INDEX` result is `i64`
//!   (`(NL_INDEX)ceil(dd)`; a value outside `i64` saturates).
//! - Out-params become return values (the fitted curves). The `NL_STACKS` arguments
//!   have no counterpart; a work array that cannot be reserved returns `NL_MEM_ERR`
//!   as upstream's failed `N_Malloc` does. Errors are [`SolidError::Nlib`] values
//!   with the upstream `NL_*_ERR` number. An `nc` so large that `nc + p + 1` or
//!   `4 nc` overflows `NL_INDEX` (undefined behaviour upstream) returns
//!   [`SolidError::InvalidInput`].
//! - `N_ApproxCircArcWithCrvData`, degree 2: with fewer than four control points it
//!   intersects the end tangent lines and, when they are parallel, returns success
//!   with the middle control point uninitialised; the port fails with
//!   [`SolidError::InvalidInput`] there. With four or more it projects the second and
//!   second-to-last control points onto the end tangent lines, ignores the first
//!   projection's error and returns the second's, and uses the (then uninitialised)
//!   projection whatever the outcome; the port returns any projection error, and
//!   [`SolidError::InvalidInput`] when a projection does not hit the line (which
//!   cannot happen for an unbounded line with a non-zero direction).
//! - `N_ApproxNurbsWithNonRatCrv` takes the parametrisation flag `par`
//!   (`NL_UNIFORM`, `NL_CHORDLENGTH`, `NL_CENTRIPETAL`, `NL_INHERITED`) as a
//!   [`ParamSource`]. Every array access is bounds checked; where upstream would
//!   index outside its arrays the port returns [`SolidError::InvalidInput`]. More
//!   than [`MAX_APPROX_SAMPLES`] sampling points (from the second derivatives of the
//!   Bezier segments) are [`SolidError::InvalidInput`] too: upstream interpolates
//!   them all and its knot removal then takes time quadratic in their number (a
//!   weight of `-1` asked for ten million and ran for minutes, fuzz `nlib.crv_approx`
//!   fuzz05147; upstream fixtures and the committed difftest cases need at most
//!   11,104).

use crate::error::{NlibCode, Result, SolidError};
use crate::math::{max2, Vec3};
use crate::nurbs::crv_adv::p2::ConstrainedEnds;
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::crv_fit::p1::{
    interp_given_params, interp_knot_vector, interp_knots_and_derivs, knot_vector_end_derivs,
    param_values, ParamMethod,
};
use crate::nurbs::crv_fit::p2::{approx_lst_sq, weighted_lst_sq_knots, WeightedData};
use crate::nurbs::crv_fit::p3::{interp_end_derivs, ParamSource};
use crate::nurbs::crv_geom::ToleranceKind;
use crate::nurbs::fit_arrays::{add, alloc, at, idx, invalid, mul, out_of_range, put, sg};
use crate::nurbs::funcs_basic::SurfaceFunction;
use crate::nurbs::geom::{
    isect_line_line, normalize, project_pt_line, Interval, LinePlaneIsect, LineSeg, PI,
};
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{combine2_pts, float_op_is_bad, translate_sum2_pts, CPoint, FloatOp, NOW};
use crate::nurbs::srf_fit::func_interp;

const OP_NUM: &str = "nurbs::crv_approx::calc_num_cpts_to_approx_arc";
const OP_TABLE: &str = "nurbs::crv_approx::arc_cpts_function";
const OP_ARC: &str = "nurbs::crv_approx::approx_circ_arc_with_crv_data";
const OP_NONRAT: &str = "nurbs::crv_approx::approx_nurbs_with_non_rat_crv";

/// Most sampling points [`approx_nurbs_with_non_rat_crv`] interpolates (not upstream;
/// see the module deviations): `2^17`, 11 times the most any upstream fixture or
/// committed difftest case needs (11,104).
pub const MAX_APPROX_SAMPLES: i64 = 1 << 17;

// ---- number of control points for a circle --------------------------------------------

/// Sweep angles (degrees) of the interpolation table.
const TABLE_ANGLES: [f64; 9] = [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0, 360.0];

/// Relative tolerances of the interpolation table.
const TABLE_TOLS: [f64; 7] = [0.000001, 0.00001, 0.0001, 0.001, 0.01, 0.1, 1.0];

/// High control point indices by sweep angle (rows) and tolerance (columns), degree 2.
const TABLE_DEG2: [[f64; 7]; 9] = [
    [8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 2.0],
    [38.0, 30.0, 20.0, 13.0, 8.0, 5.0, 4.0],
    [68.0, 56.0, 34.0, 21.0, 13.0, 8.0, 5.0],
    [98.0, 81.0, 48.0, 29.0, 17.0, 10.0, 7.0],
    [128.0, 109.0, 62.0, 37.0, 22.0, 13.0, 9.0],
    [158.0, 135.0, 76.0, 45.0, 26.0, 15.0, 10.0],
    [188.0, 161.0, 90.0, 53.0, 31.0, 18.0, 12.0],
    [218.0, 185.0, 105.0, 61.0, 35.0, 20.0, 13.0],
    [248.0, 211.0, 120.0, 69.0, 39.0, 23.0, 15.0],
];

/// As [`TABLE_DEG2`], degree 3.
const TABLE_DEG3: [[f64; 7]; 9] = [
    [4.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0],
    [21.0, 12.0, 8.0, 5.0, 4.0, 3.0, 3.0],
    [39.0, 22.0, 13.0, 8.0, 6.0, 4.0, 4.0],
    [56.0, 31.0, 18.0, 11.0, 7.0, 5.0, 4.0],
    [74.0, 40.0, 23.0, 14.0, 9.0, 6.0, 5.0],
    [92.0, 49.0, 29.0, 17.0, 11.0, 7.0, 5.0],
    [108.0, 59.0, 34.0, 20.0, 12.0, 8.0, 6.0],
    [124.0, 67.0, 38.0, 23.0, 14.0, 9.0, 6.0],
    [140.0, 74.0, 43.0, 26.0, 16.0, 10.0, 7.0],
];

/// As [`TABLE_DEG2`], degree 4.
const TABLE_DEG4: [[f64; 7]; 9] = [
    [4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0],
    [8.0, 6.0, 5.0, 4.0, 4.0, 4.0, 4.0],
    [12.0, 9.0, 7.0, 6.0, 5.0, 4.0, 4.0],
    [16.0, 12.0, 9.0, 8.0, 6.0, 5.0, 4.0],
    [20.0, 15.0, 11.0, 9.0, 7.0, 6.0, 5.0],
    [24.0, 18.0, 13.0, 10.0, 8.0, 7.0, 5.0],
    [28.0, 20.0, 15.0, 11.0, 9.0, 7.0, 6.0],
    [32.0, 23.0, 17.0, 13.0, 10.0, 8.0, 6.0],
    [36.0, 26.0, 19.0, 14.0, 11.0, 9.0, 7.0],
];

/// The bilinear surface function interpolating the table of high control point
/// indices over sweep angle (degrees, `u`) and relative tolerance (`v`) for a
/// non-rational circle of degree `deg` (2, 3 or 4), as `N_CalcNumCPtsToApproxArc`
/// builds it when its `sfn` is NULL.
///
/// # Errors
///
/// [`NlibCode::Input`] for a degree other than 2, 3 or 4; errors of
/// [`func_interp`].
pub fn arc_cpts_function(deg: usize) -> Result<SurfaceFunction> {
    let table = match deg {
        2 => &TABLE_DEG2,
        3 => &TABLE_DEG3,
        4 => &TABLE_DEG4,
        _ => return Err(SolidError::nlib(OP_TABLE, NlibCode::Input)),
    };
    let fv: Vec<Vec<f64>> = table.iter().map(|row| row.to_vec()).collect();
    func_interp(&fv, 1, 1, &TABLE_ANGLES, &TABLE_TOLS)
}

/// High index of the control points [`approx_circ_arc_with_crv_data`] needs to fit
/// a circular arc of sweep angle `ang` (degrees, `0 < ang <= 360`) with a
/// non-rational curve of degree `deg` (2, 3 or 4) within the relative radial error
/// `tol` (`N_CalcNumCPtsToApproxArc`). `tol` is clamped to `[1e-6, 1]` (`[1e-5, 1]`
/// for degree 2). The count is `ceil` of `sfn` at `(ang, tol)`; `None` uses
/// [`arc_cpts_function`] of `deg`.
///
/// # Errors
///
/// [`NlibCode::Input`] for a degree other than 2, 3 or 4 or an angle outside
/// `(0, 360]`; errors of [`arc_cpts_function`] and of [`SurfaceFunction::eval`].
pub fn calc_num_cpts_to_approx_arc(
    tol: f64,
    ang: f64,
    deg: usize,
    sfn: Option<&SurfaceFunction>,
) -> Result<i64> {
    let mut tol = tol;

    // Check for errors.
    if !(2..=4).contains(&deg) {
        return Err(SolidError::nlib(OP_NUM, NlibCode::Input));
    }
    if ang <= 0.0 || ang > 360.0 {
        return Err(SolidError::nlib(OP_NUM, NlibCode::Input));
    }
    // `if tol < 1e-6 { tol = 1e-6 }` then `if tol > 1 { tol = 1 }`.
    tol = tol.clamp(1.0e-6, 1.0);
    if tol < 1.0e-5 && deg == 2 {
        tol = 1.0e-5;
    }

    // Create the surface function, if necessary.
    let local;
    let sfn = if let Some(f) = sfn {
        f
    } else {
        local = arc_cpts_function(deg)?;
        &local
    };

    // Evaluate the function to get the number of control points.
    let dd = sfn.eval(ang, tol, Side::Left, Side::Left)?;
    let dd = dd.ceil();
    Ok(dd as i64)
}

// ---- non-rational fit of a circular arc ----------------------------------------------

/// A circular arc and its end points (the circle arguments of
/// `N_ApproxCircArcWithCrvData`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircArcData {
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
    /// End angle `ae` in degrees (`ae <= as` adds 360).
    pub end_angle: f64,
    /// Start point `Ps` (interpolated exactly, for continuity with neighbours).
    pub start: Vec3,
    /// End point `Pe`.
    pub end: Vec3,
}

/// `N_VectorNormalizeRef` whose failure upstream reports as `NL_INP_ERR`.
fn unit_axis(v: Vec3) -> Result<Vec3> {
    normalize(v)
        .map(|(u, _)| u)
        .map_err(|_| SolidError::nlib(OP_ARC, NlibCode::Input))
}

/// Projection of `q` onto the unbounded line `lsg` (`N_ProjectPtLine`).
fn project(lsg: LineSeg, q: Vec3) -> Result<Vec3> {
    match project_pt_line(lsg, q)? {
        LinePlaneIsect::Hit { p, .. } => Ok(p),
        LinePlaneIsect::Parallel | LinePlaneIsect::OutOfBounds { .. } => Err(invalid(
            OP_ARC,
            "end tangent projection missed (upstream uses an uninitialised point)",
        )),
    }
}

/// Least-squares data of the degree 3 and 4 fits: knots, parameters, weights and
/// points (upstream `U`, `up`, `wp`, `P`).
struct LstSqData {
    knots: Vec<f64>,
    up: Vec<f64>,
    wp: Vec<f64>,
    pts: Vec<Vec3>,
}

/// The clamped knot vector and sampled arc of the weighted least-squares fit with
/// high index `nc` and degree `p` (3 or 4): `nk = nc + p + 1`, `np = 4 nc`.
fn lst_sq_data(
    arc: &CircArcData,
    x: Vec3,
    y: Vec3,
    as_: f64,
    theta: f64,
    nc: i64,
    p: i64,
) -> Result<LstSqData> {
    let op = OP_ARC;
    let nk = add(add(nc, p, op)?, 1, op)?;
    let np = mul(4, nc, op)?;

    let mut uk = alloc(nk, 0.0, op)?;
    for ii in 0..=p {
        put(&mut uk, ii, 0.0, op)?;
        put(&mut uk, nk - ii, 1.0, op)?;
    }
    let kk = nk - 2 * (p + 1);
    if kk >= 0 {
        let du = 1.0 / (kk as f64 + 2.0);
        for ii in 0..=kk {
            put(&mut uk, ii + p + 1, (ii as f64 + 1.0) * du, op)?;
        }
    }

    let mut up = alloc(np, 0.0, op)?;
    let mut wp = alloc(np, 0.0, op)?;
    let mut pts = alloc(np, Vec3::default(), op)?;

    let du = 1.0 / np as f64;
    put(&mut up, 0, 0.0, op)?;
    put(&mut up, np, 1.0, op)?;
    put(&mut wp, 0, -1.0, op)?;
    put(&mut wp, np, -1.0, op)?;
    put(&mut pts, 0, arc.start, op)?;
    put(&mut pts, np, arc.end, op)?;

    let (cen, rad) = (arc.center, arc.radius);
    for ii in 1..np {
        let upi = ii as f64 * du;
        put(&mut up, ii, upi, op)?;
        put(&mut wp, ii, 1.0, op)?;
        let u = as_ + upi * theta;
        put(
            &mut pts,
            ii,
            translate_sum2_pts(cen, rad * u.cos(), x, rad * u.sin(), y),
            op,
        )?;
    }
    Ok(LstSqData {
        knots: uk,
        up,
        wp,
        pts,
    })
}

/// Non-rational curve of degree `deg` (2, 3 or 4) with control points `0..=nc`
/// approximating the circular arc `arc` (`N_ApproxCircArcWithCrvData`). The end
/// points are `arc.start` / `arc.end`; with enough control points for a fit the
/// curve is a least-squares fit of points sampled on the arc, otherwise a parabolic
/// arc (degree 2) or an interpolant with the arc's end derivatives (degrees 3, 4).
/// [`calc_num_cpts_to_approx_arc`] gives the `nc` for a tolerance.
///
/// # Errors
///
/// [`NlibCode::Input`] for a degree other than 2, 3 or 4, `nc < deg`, a radius
/// `<= 0`, or axes that cannot be normalised; `InvalidInput` for parallel end
/// tangents of a degree 2 parabolic arc (see the module deviations); errors of the
/// fits ([`approx_lst_sq`], [`interp_end_derivs`], [`weighted_lst_sq_knots`]) and of
/// the tangent line intersections and projections.
pub fn approx_circ_arc_with_crv_data(arc: &CircArcData, deg: usize, nc: usize) -> Result<Curve> {
    let op = OP_ARC;

    // Check for errors.
    if !(2..=4).contains(&deg) {
        return Err(SolidError::nlib(op, NlibCode::Input));
    }
    if nc < deg {
        return Err(SolidError::nlib(op, NlibCode::Input));
    }
    if arc.radius <= 0.0 {
        return Err(SolidError::nlib(op, NlibCode::Input));
    }
    let nci = sg(nc, op)?;
    let (cen, rad, ps, pe) = (arc.center, arc.radius, arc.start, arc.end);

    // Normalize axis vectors and convert angles.
    let x = unit_axis(arc.x)?;
    let y = unit_axis(arc.y)?;

    let mut as_ = arc.start_angle;
    let mut ae = arc.end_angle;
    let mut closed = false;
    if ae <= as_ {
        ae += 360.0;
    }
    if (ae - as_) > 359.9999 {
        ae = as_ + 360.0;
        closed = true;
    }

    let theta = (ae - as_) * (PI / 180.0);
    as_ *= PI / 180.0;
    ae *= PI / 180.0;

    // Compute start/end first derivative vector.
    let mag = rad * theta;
    let d0 = combine2_pts(-mag * as_.sin(), x, mag * as_.cos(), y);
    let d1 = if closed {
        d0
    } else {
        combine2_pts(-mag * ae.sin(), x, mag * ae.cos(), y)
    };

    // Now fit a nonrational curve; the method depends on degree and number of
    // control points.
    match deg {
        2 => {
            // Make tangent lines.
            let lsg1 = LineSeg::new(ps, d0, false);
            let lsg2 = LineSeg::new(pe, d1, false);

            if nc < 3 {
                // Can't least squares fit; construct parabolic arc.
                let r = isect_line_line(lsg1, lsg2)?.ok_or_else(|| {
                    invalid(
                        op,
                        "parallel end tangents (upstream leaves the middle control point unset)",
                    )
                })?;
                let pw = vec![
                    CPoint::weighted(ps, NOW),
                    CPoint::weighted(r.p, NOW),
                    CPoint::weighted(pe, NOW),
                ];
                Curve::new(pw, 2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
            } else {
                // Least squares fit (without end tangent constraints).
                let t = 4.4 * nci as f64;
                let np = t as i64;

                let mut pts = alloc(np, Vec3::default(), op)?;
                let du = theta / np as f64;
                put(&mut pts, 0, ps, op)?;
                put(&mut pts, np, pe, op)?;
                for ii in 1..np {
                    let u = as_ + ii as f64 * du;
                    put(
                        &mut pts,
                        ii,
                        translate_sum2_pts(cen, rad * u.cos(), x, rad * u.sin(), y),
                        op,
                    )?;
                }

                let mut cur = approx_lst_sq(&pts, nc, 2, ParamMethod::Uniform)?;

                // Now adjust end tangents.
                let kk = sg(cur.n(), op)?;
                let pw = cur.cpts_mut();
                let q = at(pw, 1, op)?.to_euclid();
                let r1 = project(lsg1, q)?;
                put(pw, 1, CPoint::weighted(r1, NOW), op)?;

                let q = at(pw, kk - 1, op)?.to_euclid();
                let r2 = project(lsg2, q)?;
                put(pw, kk - 1, CPoint::weighted(r2, NOW), op)?;
                Ok(cur)
            }
        }
        3 | 4 => {
            let p = sg(deg, op)?;
            if nci < p + 1 {
                // Can't least squares fit; use interpolation.
                let pts = if p == 3 {
                    vec![ps, pe]
                } else {
                    let u = as_ + 0.5 * theta;
                    let mid = translate_sum2_pts(cen, rad * u.cos(), x, rad * u.sin(), y);
                    vec![ps, mid, pe]
                };
                let mut cur = None;
                interp_end_derivs(
                    &pts,
                    deg,
                    d0,
                    d1,
                    ParamSource::Method(ParamMethod::Uniform),
                    &mut cur,
                )?;
                cur.ok_or_else(|| out_of_range(op))
            } else {
                // Least squares fit.
                let data = lst_sq_data(arc, x, y, as_, theta, nci, p)?;
                let knt = KnotVector::from_slice(&data.knots)?;
                let np = sg(data.pts.len(), op)? - 1;
                let derivs = [d0, d1];
                let wd = [-1.0, -1.0];
                let ii = [0, idx(np, op)?];
                let wdata = WeightedData {
                    points: &data.pts,
                    point_weights: &data.wp,
                    derivs: &derivs,
                    deriv_weights: &wd,
                    deriv_index: &ii,
                };
                weighted_lst_sq_knots(&wdata, &data.up, &knt, nc, deg)
            }
        }
        _ => Err(SolidError::nlib(op, NlibCode::Input)),
    }
}

// ---- non-rational approximation of a NURBS curve -------------------------------------

/// Non-rational curve of degree `deg` approximating `cur` within `tol` everywhere
/// (`N_ApproxNurbsWithNonRatCrv`). Each Bezier segment of `cur` is sampled with a
/// count from its second derivative bound, the samples are interpolated (with the
/// end tangents of `cur` when `tan`), and every knot removable within `tol` is
/// removed (keeping the end first derivatives for `tan` with degree 2). `par` picks
/// the parametrisation of the samples: a [`ParamMethod`] (the result is then
/// rescaled to `cur`'s parameter range, and the end tangents to the arc length), or
/// [`ParamSource::Inherited`] for `cur`'s own parameters.
///
/// # Errors
///
/// [`NlibCode::Numerical`] when `1 / tol` is a bad division; errors of the Bezier
/// decomposition, the second derivative bound, evaluation, parametrisation, knot
/// vectors, interpolation, arc length, normalisation, knot removal and
/// reparametrisation; `InvalidInput` where upstream would index outside its
/// arrays and for more than [`MAX_APPROX_SAMPLES`] sampling points.
pub fn approx_nurbs_with_non_rat_crv(
    cur: &Curve,
    tol: f64,
    deg: usize,
    par: ParamSource,
    tan: bool,
) -> Result<Curve> {
    let op = OP_NONRAT;
    let pct = 0.01;

    // Get local notation.
    let up_knots = cur.knots();
    let mp = sg(cur.m(), op)?;
    let p = sg(cur.degree(), op)?;
    let twod = !cur.is_3d();

    // Initialize.
    let exp = if deg == 1 { 0.5 } else { 0.34 };
    if float_op_is_bad(1.0, tol, FloatOp::Division) {
        return Err(SolidError::nlib(op, NlibCode::Numerical));
    }
    let mut f1 = 1.0 / tol;
    f1 = f1.powf(exp);
    let f2 = deg as f64;

    // Get Bezier segments.
    let mut bez = cur.decompose_bez()?;

    // Get number of sampling points.
    let mut nu: Vec<i64> = Vec::with_capacity(bez.len());
    let mut mu: i64 = 0;
    for b in &mut bez {
        b.reparam_to_interval(Interval::new(0.0, 1.0))?;
        let muu = b.max_2nd_deriv()?;
        let num = 0.125 * muu;
        let sq = num.sqrt();
        let gro = max2(f2, sq);
        let del = f1 * gro;
        let del = del.ceil();
        let n = del as i64;
        nu.push(n);
        mu = mu
            .checked_add(n)
            .ok_or_else(|| invalid(op, "sample count overflow"))?;
    }

    // (not upstream) bound the interpolation and knot removal: see MAX_APPROX_SAMPLES
    if mu > MAX_APPROX_SAMPLES {
        return Err(invalid(op, "more than MAX_APPROX_SAMPLES sampling points"));
    }

    // Compute parameters.
    let mut u = alloc(mu, 0.0, op)?;
    let mut ul = at(up_knots, p, op)?;
    let mut i = p + 1;
    let mut j: i64 = 0;
    put(&mut u, 0, ul, op)?;
    let mut l: i64 = 0;
    while i < mp {
        while i < mp && at(up_knots, i, op)? == at(up_knots, i + 1, op)? {
            i += 1;
        }
        let ur = at(up_knots, i, op)?;
        let nuj = at(&nu, j, op)?;
        let uinc = (ur - ul) / nuj as f64;
        for k in 1..nuj {
            l += 1;
            put(&mut u, l, ul + k as f64 * uinc, op)?;
        }
        l += 1;
        put(&mut u, l, ur, op)?;
        ul = ur;
        i += 1;
        j += 1;
    }

    // Sample curve.
    let mut pts = alloc(mu, Vec3::default(), op)?;
    for i in 0..=mu {
        let p = cur.eval(at(&u, i, op)?, Side::Left)?;
        put(&mut pts, i, p, op)?;
    }

    // Get knot vector.
    if let ParamSource::Method(m) = par {
        u = param_values(&pts, m)?;
    }
    let knu = if tan {
        knot_vector_end_derivs(&u, deg)?
    } else {
        interp_knot_vector(&u, deg)?
    };
    let knt = KnotVector::from_slice(&knu)?;

    // Interpolate points.
    let u0 = at(up_knots, 0, op)?;
    let um = at(up_knots, mp, op)?;
    let mut q = if tan {
        // Get end tangents.
        let cd = cur.derivs(u0, Side::Left, 1)?;
        let mut ts = *cd.get(1).ok_or_else(|| out_of_range(op))?;
        let cd = cur.derivs(um, Side::Left, 1)?;
        let mut te = *cd.get(1).ok_or_else(|| out_of_range(op))?;

        if par != ParamSource::Inherited {
            // Adjust magnitudes.
            let del = cur.arc_length(u0, um, pct, ToleranceKind::Relative)?;
            ts = normalize(ts)?.0;
            te = normalize(te)?.0;
            ts = del * ts;
            te = del * te;
        }
        interp_knots_and_derivs(&pts, &u, &knt, deg, ts, te)?
    } else {
        interp_given_params(&pts, &u, &knt, deg)?
    };

    if twod {
        q.to_2d();
    }

    // Remove knots.
    let der = usize::from(tan && deg == 2);
    let mut q = q.remove_knots_deriv_constraints(tol, ConstrainedEnds::Both, der)?;

    // Compact output curve.
    let (n, _) = q.array_sizes();
    if sg(n, op)? < mu {
        q.compress();
    }

    // Rescale knot vector if necessary.
    if par != ParamSource::Inherited {
        let uq = q.knots();
        let mq = sg(q.m(), op)?;
        if u0 != at(uq, 0, op)? || um != at(uq, mq, op)? {
            q.reparam_to_interval(Interval::new(u0, um))?;
        }
    }
    Ok(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle_arc(as_: f64, ae: f64) -> CircArcData {
        let (a, b) = (as_.to_radians(), ae.to_radians());
        CircArcData {
            center: Vec3::new(1.0, 2.0, 0.0),
            x: Vec3::new(1.0, 0.0, 0.0),
            y: Vec3::new(0.0, 1.0, 0.0),
            radius: 2.0,
            start_angle: as_,
            end_angle: ae,
            start: Vec3::new(1.0 + 2.0 * a.cos(), 2.0 + 2.0 * a.sin(), 0.0),
            end: Vec3::new(1.0 + 2.0 * b.cos(), 2.0 + 2.0 * b.sin(), 0.0),
        }
    }

    #[test]
    fn num_cpts_table_values() {
        // At table nodes the bilinear interpolant reproduces the table.
        assert_eq!(
            calc_num_cpts_to_approx_arc(0.001, 90.0, 3, None).unwrap(),
            8
        );
        assert_eq!(
            calc_num_cpts_to_approx_arc(0.1, 360.0, 2, None).unwrap(),
            23
        );
        assert_eq!(calc_num_cpts_to_approx_arc(1.0, 45.0, 4, None).unwrap(), 4);
        // Clamped tolerance.
        assert_eq!(
            calc_num_cpts_to_approx_arc(1e-9, 180.0, 2, None).unwrap(),
            calc_num_cpts_to_approx_arc(1e-5, 180.0, 2, None).unwrap()
        );
        let f = arc_cpts_function(3).unwrap();
        assert_eq!(
            calc_num_cpts_to_approx_arc(0.003, 100.0, 3, Some(&f)).unwrap(),
            calc_num_cpts_to_approx_arc(0.003, 100.0, 3, None).unwrap()
        );
    }

    #[test]
    fn num_cpts_errors() {
        assert!(calc_num_cpts_to_approx_arc(0.1, 90.0, 5, None).is_err());
        assert!(calc_num_cpts_to_approx_arc(0.1, 0.0, 3, None).is_err());
        assert!(calc_num_cpts_to_approx_arc(0.1, 360.5, 3, None).is_err());
        assert!(arc_cpts_function(1).is_err());
    }

    #[test]
    fn arc_fits_stay_near_circle() {
        for deg in 2..=4 {
            for nc in [deg, deg + 1, 8, 15] {
                let arc = circle_arc(10.0, 120.0);
                let c = approx_circ_arc_with_crv_data(&arc, deg, nc).unwrap();
                assert_eq!(c.degree(), deg);
                let (a, b) = c.param_bounds();
                for k in 0..=20 {
                    let t = a + (b - a) * f64::from(k) / 20.0;
                    let p = c.eval(t, Side::Left).unwrap();
                    let r = ((p.x - 1.0).powi(2) + (p.y - 2.0).powi(2)).sqrt();
                    let lim = if nc == deg { 0.4 } else { 0.05 };
                    assert!((r - 2.0).abs() < lim, "deg {deg} nc {nc}: r = {r}");
                }
            }
        }
    }

    #[test]
    fn arc_fit_errors() {
        let arc = circle_arc(0.0, 90.0);
        assert!(approx_circ_arc_with_crv_data(&arc, 5, 8).is_err());
        assert!(approx_circ_arc_with_crv_data(&arc, 3, 2).is_err());
        let mut bad = arc;
        bad.radius = 0.0;
        assert!(approx_circ_arc_with_crv_data(&bad, 3, 5).is_err());
        let mut bad = arc;
        bad.x = Vec3::default();
        assert!(approx_circ_arc_with_crv_data(&bad, 3, 5).is_err());
        // Half circle, degree 2 parabola: parallel tangents.
        let half = circle_arc(0.0, 180.0);
        assert!(approx_circ_arc_with_crv_data(&half, 2, 2).is_err());
    }

    #[test]
    fn non_rat_approx_of_quarter_circle() {
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let c = Curve::new(
            vec![
                CPoint::new(1.0, 0.0, 0.0, 1.0),
                CPoint::new(w, w, 0.0, w),
                CPoint::new(0.0, 1.0, 0.0, 1.0),
            ],
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .unwrap();
        for par in [
            ParamSource::Inherited,
            ParamSource::Method(ParamMethod::ChordLength),
        ] {
            for tan in [false, true] {
                let q = approx_nurbs_with_non_rat_crv(&c, 1e-4, 3, par, tan).unwrap();
                assert!(!q.is_rational());
                let (a, b) = q.param_bounds();
                assert_eq!((a, b), (0.0, 1.0));
                for k in 0..=10 {
                    let t = a + (b - a) * f64::from(k) / 10.0;
                    let p = q.eval(t, Side::Left).unwrap();
                    let r = (p.x * p.x + p.y * p.y).sqrt();
                    assert!((r - 1.0).abs() < 1e-3, "r = {r}");
                }
            }
        }
        assert!(approx_nurbs_with_non_rat_crv(&c, 0.0, 3, ParamSource::Inherited, false).is_err());
    }

    /// Fuzz regression: a control point count whose `nc + p + 1` / `4 nc` overflows
    /// is an error, not a panic.
    #[test]
    fn absurd_cpt_count_is_an_error() {
        let arc = circle_arc(10.0, 100.0);
        assert!(approx_circ_arc_with_crv_data(&arc, 3, i64::MAX as usize).is_err());
        assert!(approx_circ_arc_with_crv_data(&arc, 4, 1 << 62).is_err());
    }
}
