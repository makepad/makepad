//! NURBS circles, circular arcs, ellipses and elliptical arcs (quadratic, quartic and
//! quintic conic constructions) and circle weights.
//!
//! Upstream: `source/SMLib/src/NL_CrvCommon.cpp`, declarations in
//! `source/SMLib/inc/NL_CrvCommon.h`.
//!
//! | Rust                               | Upstream                          |
//! |------------------------------------|-----------------------------------|
//! | [`Arc`]                            | the `C, X, Y, r, as, ae` arguments |
//! | [`EllipseArc`]                     | the `C, X, Y, r1, r2, as, ae` arguments |
//! | [`quadratic_arc_tol`]              | `N_CreateQuadraticArcTol`         |
//! | [`quartic_arc_tol`]                | `N_CreateQuarticArcTol`           |
//! | [`quintic_arc_tol`]                | `N_CreateQuinticArcTol`           |
//! | [`circ_arc`], [`DEFAULT_ANGLE_TOL`] | `N_CreateCircArc` (`at = 0.01`)  |
//! | [`circ_arc_tol`]                   | `N_CreateCircArcTol`              |
//! | [`circ_weights`]                   | `N_CalcCircWeights`               |
//! | [`elliptical_arc_tol`]             | `N_CreateEllipticalArcTol`        |
//! | [`elliptical_arc_ang_type_tol`], [`AngleType`] | `N_CreateEllipticalArcAngTypeTol` |
//!
//! Deviations:
//! - The output curve is returned as a new [`Curve`] (upstream fills the caller's
//!   `NL_CURVE`, allocating it when NULL, or checks that a pre-allocated one is large
//!   enough). The `NL_STACKS` arguments have no counterpart.
//! - Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number
//!   instead of a flag plus the global `NL_ERROR` record.
//! - `N_CreateQuadraticArcTol` reads the uninitialised intersection flag when
//!   `N_IsectLineLine` fails (zero tangent or bad division); [`quadratic_arc_tol`]
//!   fails with `NL_INP_ERR` there, which is what upstream does when that flag
//!   happens to be non-zero.
//! - The conic type flag `ctp` is a [`ConicType`], so the `NL_CAL_ERR` branch of
//!   `N_CreateCircArcTol` for an unknown flag cannot be reached. The angle type `atp`
//!   is an [`AngleType`] (upstream: `0` circle angle, anything else ellipse angle).
//! - `N_CreateEllipticalArcAngTypeTol` loops forever for a non-finite end angle (and
//!   `N_CrvGetConicData` for non-finite or huge angles); the port returns
//!   `InvalidInput` instead (after [`CONIC_MAX_TURNS`] turns for the end angle
//!   normalisation, as [`conic_data`]).
//! - `N_CrvClosestPt` and the closure tests read the global `NL_MTOL`; the port uses
//!   [`MODEL_TOL`].

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_basic::PTOL;
use crate::nurbs::crv_basic::{conic_data, ConicType, Curve, CONIC_MAX_TURNS};
use crate::nurbs::geom::{
    dist_pt_pt, isect_line_line, normalize, project_pt_line, LinePlaneIsect, LineSeg, PI,
};
use crate::nurbs::math::{combine2_pts, translate_sum2_pts, CPoint, MODEL_TOL};

/// Angle tolerance in degrees used by [`circ_arc`] and [`circ_weights`] (upstream
/// `at = 0.01` in `N_CreateCircArc`), and by [`elliptical_arc_tol`] for a
/// non-positive tolerance.
pub const DEFAULT_ANGLE_TOL: f64 = 0.01;

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

fn slot<'a, T>(v: &'a mut [T], i: usize, op: &'static str) -> Result<&'a mut T> {
    v.get_mut(i)
        .ok_or_else(|| invalid(op, "index outside the curve arrays"))
}

fn get<T: Copy>(v: &[T], i: usize, op: &'static str) -> Result<T> {
    v.get(i)
        .copied()
        .ok_or_else(|| invalid(op, "index outside the curve arrays"))
}

/// A circle or circular arc: centre, axes (normalised by the constructors), radius
/// and start / end angles in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arc {
    /// Centre `C`.
    pub center: Vec3,
    /// X axis (angle 0).
    pub x: Vec3,
    /// Y axis (angle 90); the circle's normal is `X x Y`.
    pub y: Vec3,
    /// Radius.
    pub r: f64,
    /// Start angle in degrees.
    pub start: f64,
    /// End angle in degrees.
    pub end: f64,
}

/// An ellipse or elliptical arc: centre, axes, major / minor radii and start / end
/// angles in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EllipseArc {
    /// Centre `C`.
    pub center: Vec3,
    /// Major axis direction.
    pub x: Vec3,
    /// Minor axis direction.
    pub y: Vec3,
    /// Major radius (along `x`).
    pub r1: f64,
    /// Minor radius (along `y`).
    pub r2: f64,
    /// Start angle in degrees.
    pub start: f64,
    /// End angle in degrees.
    pub end: f64,
}

/// How the angles of an [`EllipseArc`] are measured (upstream `atp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AngleType {
    /// `atp = 0`: the angle of the point on the circle of radius `r1`, projected to
    /// the ellipse.
    #[default]
    Circle,
    /// `atp = 1`: the polar angle of the point on the ellipse.
    Ellipse,
}

/// Point `C + r cos(a) X + r sin(a) Y` and unit tangent `-sin(a) X + cos(a) Y`.
fn circle_pt(c: Vec3, x: Vec3, y: Vec3, r: f64, angle: f64) -> (Vec3, Vec3) {
    let cosa = angle.cos();
    let sina = angle.sin();
    let alf = r * cosa;
    let bet = r * sina;
    (
        translate_sum2_pts(c, alf, x, bet, y),
        combine2_pts(-sina, x, cosa, y),
    )
}

/// Quadratic (degree 2) NURBS circle or circular arc with double interior knots
/// (`N_CreateQuadraticArcTol`). The ends are snapped together when the sweep exceeds
/// `360 - at` degrees.
///
/// # Errors
///
/// `InvalidInput` for non-finite or huge angles; `NL_GEO_ERR` for a zero axis;
/// `NL_INP_ERR` when a segment's tangent lines cannot be intersected.
pub fn quadratic_arc_tol(arc: &Arc, at: f64) -> Result<Curve> {
    const OP: &str = "nurbs::crv_common::quadratic_arc_tol";
    let c = arc.center;
    let (r, as_deg) = (arc.r, arc.start);
    let angle_tol_deg = at;

    // Get number of arcs.
    let cd = conic_data(ConicType::Quadratic, as_deg, arc.end)?;
    let (ae, n, m) = (cd.ae, cd.n, cd.m);

    let narcs = n / 2;
    let theta = ae - as_deg;
    let dtheta = theta / narcs as f64;

    let mut pw = vec![CPoint::default(); n + 1];
    let mut u = vec![0.0; m + 1];

    // Get start point.
    let (x, _) = normalize(arc.x)?;
    let (y, _) = normalize(arc.y)?;

    let mut angle = (PI * dtheta) / 360.0;
    let w1 = angle.cos();

    angle = (PI * as_deg) / 180.0;
    let (mut p0, mut t0) = circle_pt(c, x, y, r, angle);
    *slot(&mut pw, 0, OP)? = CPoint::weighted(p0, 1.0);

    // Compute segments.
    let dangle = (PI * dtheta) / 180.0;
    let mut j = 0usize;

    for i in 1..=narcs {
        angle += dangle;
        let (p2, t2) = circle_pt(c, x, y, r, angle);

        let l0 = LineSeg::new(p0, t0, false);
        let l2 = LineSeg::new(p2, t2, false);

        let p1 = match isect_line_line(l0, l2) {
            Ok(Some(hit)) => hit.p,
            // Lines fail to intersect: this segment is very flat, use the midpoint
            // (upstream computes and ignores the distance of the ends here).
            Ok(None) => {
                let _dist = dist_pt_pt(p0, p2);
                combine2_pts(0.5, p0, 0.5, p2)
            }
            Err(_) => return Err(nlib(OP, NlibCode::Input)),
        };

        *slot(&mut pw, j + 1, OP)? = CPoint::weighted(p1, w1);
        *slot(&mut pw, j + 2, OP)? = CPoint::weighted(p2, 1.0);

        if i < narcs {
            p0 = p2;
            t0 = t2;
        }
        j += 2;
    }

    // Ensure closure if theta = 360 degrees.
    if theta > 360.0 - angle_tol_deg {
        *slot(&mut pw, n, OP)? = get(&pw, 0, OP)?;
    }

    // Get knot vector.
    for i in 0..=2 {
        *slot(&mut u, i, OP)? = 0.0;
    }
    let inner: &[f64] = match narcs {
        2 => &[0.5, 0.5],
        3 => &[1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0],
        4 => &[0.25, 0.25, 0.50, 0.50, 0.75, 0.75],
        _ => &[],
    };
    for (k, &v) in inner.iter().enumerate() {
        *slot(&mut u, 3 + k, OP)? = v;
    }
    for i in 0..=2 {
        *slot(&mut u, n + i + 1, OP)? = 1.0;
    }

    Curve::new(pw, 2, u)
}

/// Quartic (degree 4) NURBS circle or circular arc with quadruple interior knots and
/// a better parameterisation than [`quadratic_arc_tol`] (`N_CreateQuarticArcTol`).
///
/// # Errors
///
/// `InvalidInput` for non-finite or huge angles; `NL_GEO_ERR` for a zero axis;
/// `NL_NUM_ERR` when the reparameterisation cannot be solved; the errors of the
/// tangent intersection, or `NL_INP_ERR` when the tangents are parallel.
pub fn quartic_arc_tol(arc: &Arc, at: f64) -> Result<Curve> {
    const OP: &str = "nurbs::crv_common::quartic_arc_tol";
    let c = arc.center;
    let (r, as_deg) = (arc.r, arc.start);
    let angle_tol_deg = at;

    // Get number of arcs.
    let cd = conic_data(ConicType::Quartic, as_deg, arc.end)?;
    let (ae, n, m) = (cd.ae, cd.n, cd.m);

    let narcs = n / 4;
    let theta = ae - as_deg;

    let mut pw = vec![CPoint::default(); n + 1];
    let mut u = vec![0.0; m + 1];

    // Get start point.
    let (x, _) = normalize(arc.x)?;
    let (y, _) = normalize(arc.y)?;

    let dtheta = theta / narcs as f64;
    let mut angle = (PI * dtheta) / 180.0;
    let x2 = angle.cos();
    angle *= 0.5;
    let w1 = angle.cos();

    angle = (PI * as_deg) / 180.0;
    let (mut p0, mut t0) = circle_pt(c, x, y, r, angle);
    *slot(&mut pw, 0, OP)? = CPoint::weighted(p0, 1.0);

    // Compute quadratic segments and reparameterise them to quartics. First the two
    // parameters of the Bezier quadratic circle (u in [0,1]) which map to the points
    // at 1/4 and 3/4 of the sweep dtheta.
    let mut u1 = 0.0;
    let mut u2 = 0.0;
    for i in 0..2 {
        let ang = if i == 0 {
            (0.25 * PI * dtheta) / 180.0
        } else {
            (0.75 * PI * dtheta) / 180.0
        };
        let cs = ang.cos();

        let a = 1.0 - 2.0 * w1 + x2 - 2.0 * cs * (1.0 - w1); // quadratic equation
        let b = 2.0 * (w1 - 1.0) + 2.0 * cs * (1.0 - w1);
        let cc = 1.0 - cs;

        let disc = b * b - 4.0 * a * cc;
        if disc < 0.0 {
            return Err(nlib(OP, NlibCode::Numerical));
        }
        let disc = disc.sqrt(); // solve quadratic equation
        let v1 = (-b + disc) / (2.0 * a);
        let v2 = (-b - disc) / (2.0 * a);

        let root = if (0.0..=1.0).contains(&v1) {
            v1
        } else if (0.0..=1.0).contains(&v2) {
            v2
        } else {
            return Err(nlib(OP, NlibCode::Numerical));
        };
        if i == 0 {
            u1 = root;
        } else {
            u2 = root;
        }
    }

    // Use u1 and u2 to get the rational quadratic Bezier reparameterisation function:
    // a 2x2 linear system gives the middle control point (1D) and middle weight.
    let a = u1 - u2;
    let wf = -(8.0 + 10.0 * a) / (6.0 * a);
    let fw = (u1 * (10.0 + 6.0 * wf) - 1.0) / 6.0;

    let d = (2.0 * fw * fw) / 3.0;
    let cc = (1.0 - 6.0 * fw + 4.0 * wf * fw - 4.0 * fw * fw) / 3.0;
    let b = (-6.0 * wf + 2.0 * wf * wf - 4.0 * wf * fw + 6.0 * fw + 2.0 * fw * fw) / 3.0;
    let a = wf - fw;

    // Compute all the quadratic segments and reparameterise them: the three middle
    // control points of each quartic.
    let dangle = (PI * dtheta) / 180.0;
    let mut j = 0usize;

    for i in 1..=narcs {
        angle += dangle;
        let (p2, t2) = circle_pt(c, x, y, r, angle);

        let l0 = LineSeg::new(p0, t0, false);
        let l2 = LineSeg::new(p2, t2, false);

        let Some(hit) = isect_line_line(l0, l2)? else {
            return Err(nlib(OP, NlibCode::Input));
        };

        let qw = CPoint::weighted(hit.p, w1);
        let pj4 = CPoint::weighted(p2, 1.0);
        *slot(&mut pw, j + 4, OP)? = pj4;

        // The quadratic segment is Pw[j], Qw, Pw[j+4].
        let pj = get(&pw, j, OP)?;
        let pj1 = CPoint::combine2(a, pj, fw, qw);
        *slot(&mut pw, j + 1, OP)? = pj1;
        *slot(&mut pw, j + 2, OP)? = CPoint::combine4(b, pj, cc, qw, d, pj4, 2.0, pj1);
        *slot(&mut pw, j + 3, OP)? = CPoint::combine2(a, qw, fw, pj4);

        if i < narcs {
            p0 = p2;
            t0 = t2;
        }
        j += 4;
    }

    // Ensure closure if theta = 360 degrees.
    if theta > 360.0 - angle_tol_deg {
        *slot(&mut pw, n, OP)? = get(&pw, 0, OP)?;
    }

    // Get knot vector.
    for i in 0..=4 {
        *slot(&mut u, i, OP)? = 0.0;
    }
    match narcs {
        2 => {
            for i in 5..=8 {
                *slot(&mut u, i, OP)? = 0.5;
            }
        }
        3 => {
            for i in 5..=8 {
                *slot(&mut u, i, OP)? = 1.0 / 3.0;
            }
            for i in 9..=12 {
                *slot(&mut u, i, OP)? = 2.0 / 3.0;
            }
        }
        _ => {}
    }
    for i in 0..=4 {
        *slot(&mut u, n + i + 1, OP)? = 1.0;
    }

    Curve::new(pw, 4, u)
}

/// Quintic (degree 5) NURBS circle or circular arc without interior knots, a
/// rational quintic Bezier curve (`N_CreateQuinticArcTol`): a full quintic circle
/// whose segment between the start and end points is extracted.
///
/// # Errors
///
/// `InvalidInput` for non-finite or huge angles; `NL_GEO_ERR` for a zero axis; the
/// errors of [`Curve::closest_pt`] and [`Curve::extract_seg`].
pub fn quintic_arc_tol(arc: &Arc, at: f64) -> Result<Curve> {
    const OP: &str = "nurbs::crv_common::quintic_arc_tol";
    let c = arc.center;
    let (r, as_deg) = (arc.r, arc.start);
    let angle_tol_deg = at;

    // Get number of arcs.
    let cd = conic_data(ConicType::Quintic, as_deg, arc.end)?;
    let (ae, n, p, m) = (cd.ae, cd.n, cd.p, cd.m);

    let mut pw = vec![CPoint::default(); n + 1];
    let mut u = vec![0.0; m + 1];

    // Get start, end and mid points.
    let mut am = 0.5 * (as_deg + ae);
    am += 180.0; // ??

    let (x, _) = normalize(arc.x)?;
    let (y, _) = normalize(arc.y)?;

    let mut angle = (PI * as_deg) / 180.0;
    let ps = translate_sum2_pts(c, r * angle.cos(), x, r * angle.sin(), y);

    angle = (PI * ae) / 180.0;
    let pe = translate_sum2_pts(c, r * angle.cos(), x, r * angle.sin(), y);

    angle = (PI * am) / 180.0;
    let cosa = angle.cos();
    let sina = angle.sin();
    let alf = r * cosa;
    let bet = r * sina;

    let pm = translate_sum2_pts(c, alf, x, bet, y);
    let t = combine2_pts(-sina, x, cosa, y);
    let s = combine2_pts(-cosa, x, -sina, y);

    let (t, _) = normalize(t)?;
    let (s, _) = normalize(s)?;

    // Get quintic full circle.
    *slot(&mut pw, 0, OP)? = CPoint::weighted(pm, 5.0);
    *slot(&mut pw, 5, OP)? = CPoint::weighted(pm, 5.0);

    let a = combine2_pts(1.0, pm, 4.0 * r, t);
    *slot(&mut pw, 1, OP)? = CPoint::weighted(a, 1.0);

    let a = combine2_pts(1.0, pm, -4.0 * r, t);
    *slot(&mut pw, 4, OP)? = CPoint::weighted(a, 1.0);

    let a = translate_sum2_pts(c, 3.0 * r, s, 2.0 * r, t);
    *slot(&mut pw, 2, OP)? = CPoint::weighted(a, 1.0);

    let a = translate_sum2_pts(c, 3.0 * r, s, -2.0 * r, t);
    *slot(&mut pw, 3, OP)? = CPoint::weighted(a, 1.0);

    for i in 0..=5 {
        *slot(&mut u, i, OP)? = 0.0;
        *slot(&mut u, i + 6, OP)? = 1.0;
    }

    // Extract curve segment.
    if (ae - as_deg) > 360.0 - angle_tol_deg {
        *slot(&mut pw, 5, OP)? = get(&pw, 0, OP)?;
        return Curve::new(pw, p, u);
    }
    let cur = Curve::new(pw, p, u)?;

    let mut dangle = as_deg - am;
    dangle += 360.0;
    let u0 = dangle / 360.0;
    let (us, _) = cur.closest_pt(ps, u0, MODEL_TOL, MODEL_TOL)?;

    dangle = ae - am;
    if dangle < 0.0 {
        dangle += 360.0;
    }
    let u0 = dangle / 360.0;
    let (ue, _) = cur.closest_pt(pe, u0, MODEL_TOL, MODEL_TOL)?;

    cur.extract_seg(us, ue)
}

/// NURBS circle or circular arc of type `ctp` with the default angle tolerance
/// [`DEFAULT_ANGLE_TOL`] (`N_CreateCircArc`).
///
/// # Errors
///
/// As [`circ_arc_tol`].
pub fn circ_arc(arc: &Arc, ctp: ConicType) -> Result<Curve> {
    circ_arc_tol(arc, DEFAULT_ANGLE_TOL, ctp)
}

/// NURBS circle or circular arc of type `ctp` (`N_CreateCircArcTol`): degree 2 with
/// double interior knots, degree 4 with quadruple interior knots, or a degree 5
/// Bezier curve. `at` is the angle tolerance in degrees for closing full circles.
///
/// # Errors
///
/// The errors of [`quadratic_arc_tol`], [`quartic_arc_tol`] or [`quintic_arc_tol`].
pub fn circ_arc_tol(arc: &Arc, at: f64, ctp: ConicType) -> Result<Curve> {
    match ctp {
        ConicType::Quadratic => quadratic_arc_tol(arc, at),
        ConicType::Quartic => quartic_arc_tol(arc, at),
        ConicType::Quintic => quintic_arc_tol(arc, at),
    }
}

/// Weights `w[0..=k]` of the unit circle arc from `as_deg` to `ae_deg` degrees built
/// by [`circ_arc`] with type `ctp` (`N_CalcCircWeights`); the highest index `k` is
/// `len - 1`.
///
/// # Errors
///
/// As [`circ_arc`].
pub fn circ_weights(as_deg: f64, ae_deg: f64, ctp: ConicType) -> Result<Vec<f64>> {
    // Create unit circle.
    let arc = Arc {
        center: Vec3::new(0.0, 0.0, 0.0),
        x: Vec3::new(1.0, 0.0, 0.0),
        y: Vec3::new(0.0, 1.0, 0.0),
        r: 1.0,
        start: as_deg,
        end: ae_deg,
    };
    let cur = circ_arc(&arc, ctp)?;

    // Extract weights.
    Ok(cur.cpts().iter().map(|pw| pw.w).collect())
}

/// NURBS ellipse or elliptical arc with circle angles ([`AngleType::Circle`])
/// (`N_CreateEllipticalArcTol`); a non-positive `at` becomes
/// [`DEFAULT_ANGLE_TOL`].
///
/// # Errors
///
/// As [`elliptical_arc_ang_type_tol`].
pub fn elliptical_arc_tol(e: &EllipseArc, at: f64, ctp: ConicType) -> Result<Curve> {
    let a_tol = if at <= 0.0 { DEFAULT_ANGLE_TOL } else { at };
    elliptical_arc_ang_type_tol(e, a_tol, AngleType::Circle, ctp)
}

/// NURBS ellipse or elliptical arc (`N_CreateEllipticalArcAngTypeTol`): the circle
/// of radius `r1` over the corresponding angles ([`circ_arc_tol`] with type `ctp`),
/// scaled by `r2 / r1` towards the major axis. `atp` says whether the start and end
/// angles are circle angles or polar angles on the ellipse.
///
/// # Errors
///
/// `InvalidInput` for a non-finite or huge end angle; `NL_GEO_ERR` for a zero axis;
/// the errors of [`circ_arc_tol`]; `NL_INP_ERR` when a control point cannot be
/// projected to the major axis.
pub fn elliptical_arc_ang_type_tol(
    e: &EllipseArc,
    at: f64,
    atp: AngleType,
    ctp: ConicType,
) -> Result<Curve> {
    const OP: &str = "nurbs::crv_common::elliptical_arc_ang_type_tol";
    let c = e.center;
    let (r1, r2, as_deg) = (e.r1, e.r2, e.start);
    let mut ae = e.end;

    // Normalize vectors.
    let (x, _) = normalize(e.x)?;
    let (y, _) = normalize(e.y)?;

    let lx = LineSeg::new(c, x, false);

    // Get start and end points.
    if !ae.is_finite() {
        return Err(invalid(OP, "non-finite end angle"));
    }
    let mut turns = 0usize;
    while ae <= as_deg {
        ae += 360.0;
        turns += 1;
        if turns > CONIC_MAX_TURNS {
            return Err(invalid(OP, "end angle too far from the start angle"));
        }
    }
    while (ae - as_deg) >= 360.1 {
        ae -= 360.0;
        turns += 1;
        if turns > CONIC_MAX_TURNS {
            return Err(invalid(OP, "end angle too far from the start angle"));
        }
    }
    if (ae - as_deg) > 360.0 {
        ae = as_deg + 360.0;
    }

    let mut apnt = [0.0; 2];
    for (ap_out, ang) in apnt.iter_mut().zip([as_deg, ae]) {
        let angle = (PI * ang) / 180.0;

        let (alf, bet) = match atp {
            AngleType::Circle => {
                // Point on circle at angle, projected to the ellipse.
                (r1 * angle.cos(), r2 * angle.sin())
            }
            AngleType::Ellipse => {
                // Point on the ellipse at polar angle: with e = tan(angle),
                // x = a*b/sqrt(a*a*e*e + b*b) and y = b/a * sqrt(a*a - x*x).
                let e = angle.tan();
                let mut alf = r1 * r2 / (r2 * r2 + r1 * r1 * e * e).sqrt();
                if angle.cos() < 0.0 {
                    alf = -alf;
                }
                let mut bet = r1 * r1 - alf * alf;
                if bet < 1.0e-12 {
                    bet = 0.0;
                } else {
                    bet = (r2 / r1) * bet.sqrt();
                }
                if angle.sin() < 0.0 {
                    bet = -bet;
                }
                (alf, bet)
            }
        };

        // Polar angle of the circle point.
        let tmp_angle = (bet / r2).atan2(alf / r1);
        let mut ap = (180.0 * tmp_angle) / PI;

        // Angle snapping up to NL_PTOL degrees.
        if ap.abs() <= PTOL {
            ap = 0.0;
        }
        if ap < 0.0 {
            ap += 360.0;
        }
        *ap_out = ap;
    }
    let [ap0, ap1] = apnt;

    // Compute circle/circular arc.
    let arc = Arc {
        center: c,
        x,
        y,
        r: r1,
        start: ap0,
        end: ap1,
    };
    let mut cur = circ_arc_tol(&arc, at, ctp)?;

    // Apply affine transformation to get ellipse control points.
    let fac = r2 / r1;
    let omf = 1.0 - fac;

    for pw in cur.cpts_mut() {
        let (p, w) = pw.to_euclid_and_w();
        let q = match project_pt_line(lx, p)? {
            LinePlaneIsect::Hit { p: q, .. } => q,
            LinePlaneIsect::Parallel | LinePlaneIsect::OutOfBounds { .. } => {
                return Err(nlib(OP, NlibCode::Input));
            }
        };
        let p = combine2_pts(fac, p, omf, q);
        *pw = CPoint::weighted(p, w);
    }

    Ok(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::knots::Side;

    fn unit_arc(start: f64, end: f64) -> Arc {
        Arc {
            center: Vec3::new(1.0, 2.0, 3.0),
            x: Vec3::new(2.0, 0.0, 0.0),
            y: Vec3::new(0.0, 3.0, 0.0),
            r: 2.0,
            start,
            end,
        }
    }

    fn point(cur: &Curve, t: f64) -> Vec3 {
        cur.eval(t, Side::Left).unwrap()
    }

    fn assert_on_circle(cur: &Curve, arc: &Arc) {
        let (u0, u1) = (cur.knots()[0], *cur.knots().last().unwrap());
        for k in 0..=20 {
            let t = u0 + (u1 - u0) * f64::from(k) / 20.0;
            let p = point(cur, t);
            let d = p - arc.center;
            assert!(
                (d.length() - arc.r).abs() < 1e-9,
                "t={t} |d|={}",
                d.length()
            );
        }
    }

    #[test]
    fn quadratic_quarter_circle() {
        let arc = unit_arc(0.0, 90.0);
        let cur = quadratic_arc_tol(&arc, 0.01).unwrap();
        assert_eq!((cur.n(), cur.degree(), cur.m()), (2, 2, 5));
        assert_eq!(cur.knots(), &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        let w = cur.cpts()[1].w;
        assert!((w - core::f64::consts::FRAC_1_SQRT_2).abs() < 1e-15);
        assert_on_circle(&cur, &arc);
    }

    #[test]
    fn full_circles_close() {
        for ctp in [ConicType::Quadratic, ConicType::Quartic, ConicType::Quintic] {
            let arc = unit_arc(30.0, 30.0);
            let cur = circ_arc(&arc, ctp).unwrap();
            let pw = cur.cpts();
            assert_eq!(pw[0], pw[cur.n()]);
            assert_on_circle(&cur, &arc);
        }
    }

    #[test]
    fn partial_arcs_lie_on_circle() {
        for ctp in [ConicType::Quadratic, ConicType::Quartic, ConicType::Quintic] {
            for (s, e) in [(0.0, 45.0), (10.0, 200.0), (-30.0, 250.0), (100.0, 50.0)] {
                let arc = unit_arc(s, e);
                let cur = circ_arc(&arc, ctp).unwrap();
                assert_on_circle(&cur, &arc);
            }
        }
    }

    #[test]
    fn weights_of_half_circle() {
        let w = circ_weights(0.0, 180.0, ConicType::Quadratic).unwrap();
        assert_eq!(w.len(), 5);
        assert_eq!(w[0], 1.0);
        assert_eq!(w[2], 1.0);
        assert!((w[1] - core::f64::consts::FRAC_1_SQRT_2).abs() < 1e-15);
    }

    #[test]
    fn zero_axis_fails() {
        let mut arc = unit_arc(0.0, 90.0);
        arc.x = Vec3::new(0.0, 0.0, 0.0);
        let e = quadratic_arc_tol(&arc, 0.01).unwrap_err();
        assert!(matches!(
            e,
            SolidError::Nlib {
                code: NlibCode::Geometry,
                ..
            }
        ));
    }

    #[test]
    fn ellipse_points_satisfy_equation() {
        let e = EllipseArc {
            center: Vec3::new(0.0, 0.0, 0.0),
            x: Vec3::new(1.0, 0.0, 0.0),
            y: Vec3::new(0.0, 1.0, 0.0),
            r1: 3.0,
            r2: 1.5,
            start: 20.0,
            end: 300.0,
        };
        for atp in [AngleType::Circle, AngleType::Ellipse] {
            for ctp in [ConicType::Quadratic, ConicType::Quartic, ConicType::Quintic] {
                let cur = elliptical_arc_ang_type_tol(&e, 0.01, atp, ctp).unwrap();
                let (u0, u1) = (cur.knots()[0], *cur.knots().last().unwrap());
                for k in 0..=10 {
                    let p = point(&cur, u0 + (u1 - u0) * f64::from(k) / 10.0);
                    let v = (p.x / 3.0).powi(2) + (p.y / 1.5).powi(2);
                    assert!((v - 1.0).abs() < 1e-9, "{atp:?} {ctp:?} {v}");
                }
            }
        }
    }

    #[test]
    fn non_finite_end_angle_is_rejected() {
        let e = EllipseArc {
            center: Vec3::new(0.0, 0.0, 0.0),
            x: Vec3::new(1.0, 0.0, 0.0),
            y: Vec3::new(0.0, 1.0, 0.0),
            r1: 3.0,
            r2: 1.5,
            start: 0.0,
            end: f64::INFINITY,
        };
        assert!(elliptical_arc_tol(&e, 0.0, ConicType::Quadratic).is_err());
    }
}
