//! The side walls of a draft extrusion that keep their curvature at the profile's
//! corners, or continue as their tangent planes there (source change).
//!
//! Upstream: none at the pin. Ported from the source fork of SMLib
//! (source fork 54148db "draft corners that keep the curves' curvature" and
//! e8f23e0 "the tangent draft corner continues lines' and arcs' walls as their
//! tangent planes"), `source/SMLib/src/SmPrimitiveCreation.cpp`.
//!
//! | Rust                          | Fork                                  |
//! |-------------------------------|---------------------------------------|
//! | [`TaperCornerRefusal`]        | `SmTaperCornerRefusal`                |
//! | [`CornerRefusal`]             | `pOptRefusal`, `pOptRefusedCorner`    |
//! | `Wall` (private)              | `sm_DraftWall`                        |
//! | `Seam` (private)              | `sm_DraftSeam`                        |
//! | `section` (private)           | `sm_DraftSection`                     |
//! | `crossing` (private)          | `sm_DraftCrossing`                    |
//! | `off_section` (private)       | `sm_DraftOffSection`                  |
//! | `wall_normal` (private)       | `sm_DraftNormal`                      |
//! | `angle` (private)             | `sm_DraftAngle`                       |
//! | `param` (private)             | `sm_DraftParam`                       |
//! | `seam_point` (private)        | `sm_DraftSeamPoint`                   |
//! | `wall_curve` (private)        | `sm_DraftWallCurve`                   |
//! | `quartic` (private)           | `sm_Quartic`                          |
//! | `seam_pcurve` (private)       | `sm_DraftSeamPcurve`                  |
//! | `iso_pcurve` (private)        | `sm_DraftIsoPcurve`                   |
//! | `seam_curve` (private)        | `sm_DraftSeamCurve`                   |
//! | `has_corner`                  | `sm_DraftHasCorner`                   |
//! | `draft_input` (private)       | `sm_DraftInput`                       |
//! | `lines_and_arcs`              | `sm_DraftLinesAndArcs`                |
//! | `curvature_keeping_walls`     | `sm_CreateCurvatureKeepingWalls`      |
//!
//! The method (from the fork): each profile curve's wall is the surface its draft
//! sweeps, a line's plane or an arc's cone, ruled from the curve to its offset at the
//! far end. At a sharp corner the two walls continue as themselves until they meet,
//! and the seam is their intersection, a conic: the rational quadratic through the
//! corner and the far crossing with the walls' common tangents there, through the
//! crossing at half height. On a plane wall its pcurve is its affine image; on a cone
//! wall a rational quartic. Every seam and pcurve is checked. With the tangent rule a
//! corner that opens a gap continues an arc's end along its tangent line (a flat
//! piece with no edge on the profile); a line's wall continues as itself.
//!
//! Deviations: out-params become return values (a refusal is
//! `Ok(Err(CornerRefusal))`, the fork's `SM_ERR_INVALID_INPUT` with its refusal set);
//! a profile curve is matched to its input by index into the caller's list. No
//! recursion; every loop is bounded by the profile's curve count or a fixed sample
//! count (the arc span count is at most the arc's angle over a quarter turn).

#![allow(clippy::neg_cmp_op_on_partial_ord)] // the fork's NaN-safe tests

use super::PrimitiveCreationAccess;
use crate::curve::composite_curve::{CompositeCurve, OffsetDirection};
use crate::curve::curve::p2::{CIRCLE_TYPE, LINE_TYPE};
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p1::line_line_closest_point;
use crate::kernel::Kernel;
use crate::math::{max2, min2, Point2, Point3, Vec3, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::p1::{BSplineCurve, BSplineCurveForm, CanonicalData, KnotType};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::p1::{BSplineSurface, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::Surface;
use crate::topology::brep::p7::FaceCurvesInput;
use crate::topology::BrepId;

const OP: &str = "primitive::primitive_creation::p2::taper_walls";

/// Why a draft corner could not be closed (`SmTaperCornerRefusal`, source change).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaperCornerRefusal {
    /// `SM_TCR_APART`: the continued curves do not meet.
    Apart,
    /// `SM_TCR_NOT_EXACT`: a curve there is neither a line nor an arc, so its wall has
    /// no exact continuation (or the seam is not exact on its walls).
    NotExact,
    /// `SM_TCR_WALL_CLOSES`: a wall closes before the far end: its neighbours meet
    /// across it, or its arc shrinks to its centre (with `fixes.taper_wall_closes_at_apex`,
    /// WR-571: a wall closes at the far end, the whole section closes, or an arc shrinks
    /// to its centre).
    WallCloses,
}

/// A refused draft corner: why, and the corner's point on the profile
/// (`pOptRefusal`, `pOptRefusedCorner`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CornerRefusal {
    /// Why the corner could not be closed.
    pub reason: TaperCornerRefusal,
    /// The corner's point on the profile curves.
    pub corner: Point3,
}

/// One profile curve, as its wall reads it (`sm_DraftWall`).
#[derive(Debug, Clone, Default)]
struct Wall {
    /// An arc of a circle, else a line.
    arc: bool,
    /// A line of no length on the profile: an arc's tangent plane at a corner.
    point: bool,
    /// Where the profile runs along it from.
    start: Point3,
    /// And to.
    end: Point3,
    // line
    /// Unit direction of travel.
    dir: Vec3,
    /// The offset at the far end, in the profile's plane.
    offset: Vec3,
    // arc
    center: Point3,
    /// The profile runs counterclockwise about it.
    axis: Vec3,
    x: Vec3,
    /// `axis x x`.
    y: Vec3,
    radius: f64,
    /// The radius at a fraction f of the height is `radius - f * shrink`.
    shrink: f64,
    /// Angles about `axis` from `x`, `end_ang > start_ang`.
    start_ang: f64,
    end_ang: f64,
    // the wall
    /// The parameter range its surface covers.
    lo: f64,
    hi: f64,
    /// Arc: the span ends, `lo` to `hi`.
    breaks: Vec<f64>,
}

/// A corner (or smooth joint) between the curve before and the curve after it
/// (`sm_DraftSeam`); with `fixes.taper_wall_closes_at_apex` (WR-571), a seam between
/// two walls next to each other over a stretch of the height: a corner of the profile,
/// or one where two walls meet once a wall between them has closed.
#[derive(Debug, Clone, Copy, Default)]
struct Seam {
    /// The seam's control points: the corner (its low end), its shoulder, the far end
    /// (its high end).
    p: Point3,
    s: Point3,
    x: Point3,
    /// The shoulder's weight.
    weight: f64,
    /// WR-571: the fractions of the height at `p` and at `x`.
    f0: f64,
    f1: f64,
}

/// Element `i`, or `SM_ERR` (upstream indexes unchecked).
fn at<T: Copy>(v: &[T], i: usize) -> Result<T> {
    v.get(i).copied().ok_or_else(|| SolidError::sm_err(OP))
}

/// Element `i` by reference, or `SM_ERR`.
fn at_ref<T>(v: &[T], i: usize) -> Result<&T> {
    v.get(i).ok_or_else(|| SolidError::sm_err(OP))
}

/// The section of `w` at fraction `f` of the height, in the profile's plane: a line
/// (point, unit direction) or a circle (centre, radius) (`sm_DraftSection`).
fn section(w: &Wall, f: f64) -> (Point3, Vec3, f64) {
    if w.arc {
        (w.center, w.axis, w.radius - f * w.shrink)
    } else {
        (w.start + f * w.offset, w.dir, 0.0)
    }
}

/// Where the sections of `a` and `b` at fraction `f` cross, nearest `near` (all in the
/// profile's plane, whose unit normal is `n`); `None` when they do not cross
/// (`sm_DraftCrossing`).
fn crossing(a: &Wall, b: &Wall, f: f64, n: Vec3, near: Point3) -> Option<Point3> {
    let (pa, da, ra) = section(a, f);
    let (pb, db, rb) = section(b, f);
    let mut cand: Vec<Point3> = Vec::with_capacity(2);
    if !a.arc && !b.arc {
        let den = da.cross(db).dot(n);
        if den.abs() < EFF_ZERO {
            return None;
        }
        let s = (pb - pa).cross(db).dot(n) / den;
        cand.push(pa + s * da);
    } else if a.arc != b.arc {
        let l = if a.arc { pb } else { pa };
        let d = if a.arc { db } else { da };
        let c = if a.arc { pa } else { pb };
        let r = if a.arc { ra } else { rb };
        if r <= 0.0 {
            return None;
        }
        let w = l - c;
        let bb = d.dot(w);
        let disc = bb * bb - (w.dot(w) - r * r);
        if disc < 0.0 {
            return None;
        }
        let root = disc.sqrt();
        cand.push(l + (-bb - root) * d);
        cand.push(l + (-bb + root) * d);
    } else {
        if ra <= 0.0 || rb <= 0.0 {
            return None;
        }
        let mut e = pb - pa;
        let dd = e.length();
        if dd < EFF_ZERO {
            return None;
        }
        e /= dd;
        let along = (ra * ra - rb * rb + dd * dd) / (2.0 * dd);
        let h2 = ra * ra - along * along;
        if h2 < 0.0 {
            return None;
        }
        let h = h2.sqrt();
        let q = n.cross(e);
        cand.push(pa + along * e + h * q);
        cand.push(pa + along * e - h * q);
    }
    let mut it = cand.into_iter();
    let mut x = it.next()?;
    for c in it {
        if c.distance_squared(near) < x.distance_squared(near) {
            x = c;
        }
    }
    Some(x)
}

/// The distance from `q` (in the profile's plane) to the section of `w` at fraction
/// `f` (`sm_DraftOffSection`).
fn off_section(w: &Wall, f: f64, q: Point3) -> f64 {
    let (p, d, r) = section(w, f);
    let v = q - p;
    if w.arc {
        return (v.length() - r).abs();
    }
    (v - v.dot(d) * d).length()
}

/// The wall's normal (unnormalised) at `q`, a point of its section (in the profile's
/// plane), for the height `h`: the curve's tangent across the ruling there
/// (`sm_DraftNormal`).
fn wall_normal(w: &Wall, q: Point3, h: Vec3) -> Result<Vec3> {
    if !w.arc {
        return Ok(w.dir.cross(w.offset + h));
    }
    let r = (q - w.center).unitize()?;
    let t = w.axis.cross(r);
    Ok(t.cross(h - w.shrink * r))
}

/// The angle of `q` about the arc's axis, unwrapped to within half a turn of `near`
/// (`sm_DraftAngle`).
fn angle(w: &Wall, q: Point3, near: f64) -> f64 {
    let r = q - w.center;
    let mut a = r.dot(w.y).atan2(r.dot(w.x));
    while a < near - PI {
        a += 2.0 * PI;
    }
    while a > near + PI {
        a -= 2.0 * PI;
    }
    a
}

/// The wall's parameter of `q` (in the profile's plane, on its section at fraction
/// `f`) (`sm_DraftParam`).
fn param(w: &Wall, q: Point3, f: f64, near: f64) -> Result<f64> {
    if !w.arc {
        return Ok((q - w.start - f * w.offset).dot(w.dir));
    }
    // the span holding the angle, then its parameter: tan((a - mid)/2) = tan(half/2) (2 tau - 1)
    let a = angle(w, q, near);
    let n = w.breaks.len();
    let mut j = 0;
    while j + 2 < n && a > at(&w.breaks, j + 1)? {
        j += 1;
    }
    let t0 = at(&w.breaks, j)?;
    let t1 = at(&w.breaks, j + 1)?;
    let half = 0.5 * (t1 - t0);
    let tau = 0.5 * ((0.5 * (a - 0.5 * (t0 + t1))).tan() / (0.5 * half).tan() + 1.0);
    Ok(t0 + (t1 - t0) * tau)
}

/// The point of the seam at `s` in `[0, 1]` (`sm_DraftSeamPoint`).
fn seam_point(seam: &Seam, s: f64) -> Point3 {
    let b0 = (1.0 - s) * (1.0 - s);
    let b1 = 2.0 * s * (1.0 - s) * seam.weight;
    let b2 = s * s;
    (b0 * seam.p + b1 * seam.s + b2 * seam.x) / (b0 + b1 + b2)
}

/// `SmBSplineCurve::CreateCanonical` with piecewise Bezier knots and no trim.
fn canonical(
    dim: u64,
    degree: usize,
    points: &[Point3],
    form: BSplineCurveForm,
    mults: &[usize],
    knots: &[f64],
    weights: Option<&[f64]>,
) -> Result<BSplineCurve> {
    BSplineCurve::create_canonical(&CanonicalData {
        dim,
        degree,
        control_points: points,
        form,
        knot_multiplicities: mults,
        knots,
        knot_type: KnotType::PiecewiseBezierKnots,
        weights,
        trim: None,
    })
}

/// The wall's curve at fraction `f` (0 the profile's curve, 1 the far end's), over
/// `lo..hi` (`sm_DraftWallCurve`).
fn wall_curve(w: &Wall, f: f64, h: Vec3) -> Result<BSplineCurve> {
    let mut points = Vec::new();
    let mut weights = Vec::new();
    let mut mults = Vec::new();
    let mut knots = Vec::new();
    if !w.arc {
        let base = w.start + f * (w.offset + h);
        points.push(base + w.lo * w.dir);
        points.push(base + w.hi * w.dir);
        mults.push(2);
        mults.push(2);
        knots.push(w.lo);
        knots.push(w.hi);
        return canonical(
            3,
            1,
            &points,
            BSplineCurveForm::PolylineForm,
            &mults,
            &knots,
            None,
        );
    }
    let r = w.radius - f * w.shrink;
    let c = w.center + f * h;
    let n = w.breaks.len();
    let mut j = 0;
    while j + 1 < n {
        let t0 = at(&w.breaks, j)?;
        let t1 = at(&w.breaks, j + 1)?;
        let half = 0.5 * (t1 - t0);
        let mid = 0.5 * (t0 + t1);
        if j == 0 {
            points.push(c + r * (t0.cos() * w.x + t0.sin() * w.y));
            weights.push(1.0);
        }
        points.push(c + (r / half.cos()) * (mid.cos() * w.x + mid.sin() * w.y));
        weights.push(half.cos());
        points.push(c + r * (t1.cos() * w.x + t1.sin() * w.y));
        weights.push(1.0);
        knots.push(t0);
        mults.push(if j == 0 { 3 } else { 2 });
        j += 1;
    }
    knots.push(at(&w.breaks, n.wrapping_sub(1))?);
    mults.push(3);
    canonical(
        3,
        2,
        &points,
        BSplineCurveForm::CircularArc,
        &mults,
        &knots,
        Some(&weights),
    )
}

/// Bernstein coefficients of the product of two quadratics (`sm_Quartic`).
fn quartic(a: [f64; 3], b: [f64; 3]) -> [f64; 5] {
    const BINOM2: [f64; 3] = [1.0, 2.0, 1.0];
    const BINOM4: [f64; 5] = [1.0, 4.0, 6.0, 4.0, 1.0];
    let mut c = [0.0; 5];
    for (k, (ck, bk4)) in c.iter_mut().zip(BINOM4).enumerate() {
        *ck = 0.0;
        for (i, (ai, bi2)) in a.iter().zip(BINOM2).enumerate() {
            let Some(j) = k.checked_sub(i) else {
                continue;
            };
            let (Some(bj), Some(bj2)) = (b.get(j), BINOM2.get(j)) else {
                continue;
            };
            *ck += bi2 * bj2 * ai * bj;
        }
        *ck /= bk4;
    }
    c
}

/// The seam's pcurve on wall `w`, whose surface parameters are
/// `(u0 + f (u1 - u0), v0 + (t - t0) * v_scale)` for the wall's fraction `f` and curve
/// parameter `t` (`sm_DraftSeamPcurve`). `SM_ERR` when the seam leaves one span of an
/// arc wall or a weight is not positive.
#[allow(clippy::too_many_arguments)] // the fork's arguments, and the WR-571 mode
fn seam_pcurve(
    w: &Wall,
    seam: &Seam,
    n: Vec3,
    h: Vec3,
    [u0, u1]: [f64; 2],
    t0: f64,
    v0: f64,
    v_scale: f64,
    apex: bool,
) -> Result<BSplineCurve> {
    let cp = [seam.p, seam.s, seam.x];
    let sw = [1.0, seam.weight, 1.0];
    let height = h.dot(n);
    // f of each control point (the height fraction is affine in space; WR-571: from the
    // seam's low end)
    let f0 = if apex { seam.f0 } else { 0.0 };
    let sf = cp.map(|c| f0 + (c - seam.p).dot(n) / height);
    let mut points = Vec::new();
    let mut weights = Vec::new();
    let knots = [0.0, 1.0];
    if !w.arc {
        // a plane: (f, t) affine in space
        for ((c, f), wk) in cp.iter().zip(sf).zip(sw) {
            let q = *c - f * h;
            let t = (q - w.start - f * w.offset).dot(w.dir);
            points.push(Point3::new(
                u0 + f * (u1 - u0),
                v0 + (t - t0) * v_scale,
                0.0,
            ));
            weights.push(wk);
        }
        return canonical(
            2,
            2,
            &points,
            BSplineCurveForm::Unspecified,
            &[3, 3],
            &knots,
            Some(&weights),
        );
    }
    // a cone: the span holding the seam (its angles at both ends and halfway lie in it)
    let (a0, am, a1) = if apex {
        // WR-571: each point at its own height
        let half = seam_point(seam, 0.5);
        let a0 = angle(w, seam.p - seam.f0 * h, 0.5 * (w.lo + w.hi));
        let am = angle(
            w,
            half - (seam.f0 + (half - seam.p).dot(n) / height) * h,
            a0,
        );
        let a1 = angle(w, seam.x - seam.f1 * h, a0);
        (a0, am, a1)
    } else {
        let a0 = angle(w, seam.p, 0.5 * (w.lo + w.hi));
        let am = angle(w, seam_point(seam, 0.5) - 0.5 * h, a0);
        let a1 = angle(w, seam.x - h, a0);
        (a0, am, a1)
    };
    let nb = w.breaks.len();
    let mut j = 0;
    let lo = min2(a0, min2(am, a1));
    let hi = max2(a0, max2(am, a1));
    while j + 2 < nb && lo >= at(&w.breaks, j + 1)? {
        j += 1;
    }
    let s0 = at(&w.breaks, j)?;
    let s1 = at(&w.breaks, j + 1)?;
    if lo < s0 - EFF_ZERO_SQRT || hi > s1 + EFF_ZERO_SQRT {
        return Err(SolidError::sm_err(OP));
    }
    let mid = 0.5 * (s0 + s1);
    let tan_q = (0.25 * (s1 - s0)).tan();
    let xm = mid.cos() * w.x + mid.sin() * w.y;
    let ym = w.axis.cross(xm);
    // tau = (dy + tanq (rho + dx)) / (2 tanq (rho + dx)), each term affine in space
    let mut num = [0.0; 3];
    let mut den = [0.0; 3];
    let mut su = [0.0; 3];
    let mut wt = [0.0; 3];
    for (k, ((c, f), wk)) in cp.iter().zip(sf).zip(sw).enumerate() {
        let r = *c - f * h - w.center;
        let rho = w.radius - f * w.shrink;
        let dx = r.dot(xm);
        let dy = r.dot(ym);
        if let (Some(nk), Some(dk), Some(uk), Some(tk)) =
            (num.get_mut(k), den.get_mut(k), su.get_mut(k), wt.get_mut(k))
        {
            *nk = wk * (dy + tan_q * (rho + dx));
            *dk = wk * 2.0 * tan_q * (rho + dx);
            *uk = wk * (u0 + f * (u1 - u0));
            *tk = wk;
        }
    }
    let d4 = quartic(wt, den);
    let u4 = quartic(su, den);
    let t4 = quartic(num, wt);
    for ((dk, uk), tk) in d4.iter().zip(u4).zip(t4) {
        if !(*dk > 0.0) {
            return Err(SolidError::sm_err(OP));
        }
        let tau = tk / dk;
        let t = s0 + (s1 - s0) * tau;
        points.push(Point3::new(uk / dk, v0 + (t - t0) * v_scale, 0.0));
        weights.push(*dk);
    }
    canonical(
        2,
        4,
        &points,
        BSplineCurveForm::Unspecified,
        &[5, 5],
        &knots,
        Some(&weights),
    )
}

/// A 2D line from `(ua, va)` to `(ub, vb)` over `[t0, t1]` (`sm_DraftIsoPcurve`).
fn iso_pcurve(ua: f64, va: f64, ub: f64, vb: f64, t0: f64, t1: f64) -> Result<BSplineCurve> {
    canonical(
        2,
        1,
        &[Point3::new(ua, va, 0.0), Point3::new(ub, vb, 0.0)],
        BSplineCurveForm::PolylineForm,
        &[2, 2],
        &[t0, t1],
        None,
    )
}

/// The seam as a curve: the rational quadratic on `[0, 1]` (`sm_DraftSeamCurve`).
fn seam_curve(seam: &Seam) -> Result<BSplineCurve> {
    canonical(
        3,
        2,
        &[seam.p, seam.s, seam.x],
        BSplineCurveForm::Unspecified,
        &[3, 3],
        &[0.0, 1.0],
        Some(&[1.0, seam.weight, 1.0]),
    )
}

/// The tangent at `t` (`Evaluate(t, 1, TRUE, v)`), `None` when it fails.
fn tangent(k: &Kernel, c: &Curve, t: f64) -> Option<Vec3> {
    c.evaluate(k, t, 1, true, true)
        .ok()
        .and_then(|v| v.get(1).copied())
}

/// Whether the profile, its segments in `composite`, has a sharp corner
/// (`sm_DraftHasCorner`).
pub(super) fn has_corner(k: &Kernel, composite: &CompositeCurve, n: Vec3) -> bool {
    let canon = composite.canonical();
    let count = canon.curves.len();
    for i in 0..count {
        let prev = (i + count - 1) % count;
        let (Some(a), Some(b), Some(&sa), Some(&sb)) = (
            canon.curves.get(prev),
            canon.curves.get(i),
            canon.senses.get(prev),
            canon.senses.get(i),
        ) else {
            return false;
        };
        let (ia, ib) = (a.natural_interval(), b.natural_interval());
        let Some(mut va) = tangent(k, a, if sa { ia.max() } else { ia.min() }) else {
            return false;
        };
        let Some(mut vb) = tangent(k, b, if sb { ib.min() } else { ib.max() }) else {
            return false;
        };
        if !sa {
            va = -va;
        }
        if !sb {
            vb = -vb;
        }
        let Ok(turn) = n.ccw_angle_between(va, vb) else {
            return false;
        };
        if turn.abs() > EFF_ZERO_SQRT {
            return true;
        }
    }
    false
}

/// The line or circle among `inputs` that the profile's segment `c` is (its copy,
/// split at tangent breaks, is a plain B-spline; a line or a circle has none, so it is
/// the one with the same ends and middle), or `None` (`sm_DraftInput`).
fn draft_input<'a>(
    k: &Kernel,
    c: &Curve,
    inputs: &'a [Curve],
    tol: f64,
) -> Result<Option<&'a Curve>> {
    let ivl = c.natural_interval();
    let start = c.evaluate_point(k, ivl.min())?;
    let end = c.evaluate_point(k, ivl.max())?;
    let mid = c.evaluate_point(k, ivl.mid())?;
    for input in inputs {
        if !(input.is_kind_of(CIRCLE_TYPE) || input.is_kind_of(LINE_TYPE)) {
            continue;
        }
        let j = input.natural_interval();
        // (GetEnds' status is ignored)
        let (a, b) = input
            .ends(k)
            .map_or((Point3::ZERO, Point3::ZERO), |e| (e.start, e.end));
        let m = input.evaluate_point(k, j.mid())?;
        let ends = (a.distance(start) <= tol && b.distance(end) <= tol)
            || (b.distance(start) <= tol && a.distance(end) <= tol);
        if ends && m.distance(mid) <= tol {
            return Ok(Some(input));
        }
    }
    Ok(None)
}

/// Whether every segment of the profile `composite` is a line or an arc among
/// `inputs` (`sm_DraftLinesAndArcs`).
pub(super) fn lines_and_arcs(
    k: &Kernel,
    composite: &CompositeCurve,
    inputs: &[Curve],
    tol: f64,
) -> Result<bool> {
    for c in composite.canonical().curves {
        if draft_input(k, c, inputs, tol)?.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// A refusal of `reason` at `corner`.
const fn refuse(reason: TaperCornerRefusal, corner: Point3) -> Walls {
    Err(CornerRefusal { reason, corner })
}

/// The far end's curves, or the refused corner.
type Walls = core::result::Result<Vec<Curve>, CornerRefusal>;

/// The inputs of [`curvature_keeping_walls`].
pub(super) struct WallsInput<'a> {
    /// `bTangent`: gaps closed by the walls' tangent planes.
    pub tangent: bool,
    /// The profile, one closed chain.
    pub composite: &'a CompositeCurve,
    /// The curves it was made from (its lines and circles).
    pub inputs: &'a [Curve],
    /// Unit normal of the profile's plane.
    pub normal: Vec3,
    /// `+` along the normal, `-` opposite.
    pub height: f64,
    /// How far the far end's section is offset (> 0).
    pub offset: f64,
    /// The side it is offset to.
    pub side: OffsetDirection,
    /// The B-rep's tolerance.
    pub tol: f64,
}

/// The walls of the profile, read from its segments (the first loop of
/// `sm_CreateCurvatureKeepingWalls`).
fn read_walls(
    k: &Kernel,
    input: &WallsInput<'_>,
) -> Result<core::result::Result<Vec<Wall>, CornerRefusal>> {
    let WallsInput {
        composite,
        inputs,
        normal: n,
        offset,
        side,
        tol,
        ..
    } = *input;
    let side = if side == OffsetDirection::LeftHandSide {
        1.0
    } else {
        -1.0
    };
    let canon = composite.canonical();
    let mut walls = Vec::new();
    for (pc, &sense) in canon.curves.iter().zip(&canon.senses) {
        let mut w = Wall::default();
        let ivl = pc.natural_interval();
        w.start = pc.evaluate_point(k, if sense { ivl.min() } else { ivl.max() })?;
        w.end = pc.evaluate_point(k, if sense { ivl.max() } else { ivl.min() })?;
        let v = pc.evaluate(k, if sense { ivl.min() } else { ivl.max() }, 1, true, true)?;
        let v1 = at(&v, 1)?;
        let t = if sense { v1 } else { -v1 };
        let found = draft_input(k, pc, inputs, tol)?;
        match found.map(Curve::kind) {
            Some(CurveKind::Ellipse(circle))
                if found.is_some_and(|c| c.is_kind_of(CIRCLE_TYPE)) =>
            {
                let frame = circle.position();
                w.arc = true;
                w.center = frame.origin();
                w.radius = circle.radius();
                w.x = frame.x_axis().unitize()?;
                let kk = w.x.cross(frame.y_axis()).unitize()?;
                // counterclockwise about the axis it is run along
                w.axis = if kk.cross(w.start - w.center).dot(t) >= 0.0 {
                    kk
                } else {
                    -kk
                };
                w.y = w.axis.cross(w.x);
                w.shrink = side * n.dot(w.axis) * offset;
                w.start_ang = angle(&w, w.start, 0.0);
                w.end_ang = angle(&w, w.end, w.start_ang);
                if w.end_ang <= w.start_ang + EFF_ZERO_SQRT {
                    w.end_ang += 2.0 * PI;
                }
                if w.radius - w.shrink <= tol {
                    let corner = pc.evaluate_point(k, ivl.mid())?;
                    return Ok(Err(CornerRefusal {
                        reason: TaperCornerRefusal::WallCloses,
                        corner,
                    }));
                }
            }
            Some(_) => {
                w.arc = false;
                w.dir = (w.end - w.start).unitize()?;
                w.offset = side * offset * n.cross(w.dir);
            }
            None => {
                return Ok(Err(CornerRefusal {
                    reason: TaperCornerRefusal::NotExact,
                    corner: w.start,
                }));
            }
        }
        walls.push(w);
    }
    Ok(Ok(walls))
}

/// Tangent planes: where a corner opens a gap, an arc's end continues along its
/// tangent line, a wall of no length on the profile (a line's wall continues as
/// itself) (the `bTangent` step of `sm_CreateCurvatureKeepingWalls`).
fn with_tangent_planes(walls: &[Wall], n: Vec3, offset: f64, side: f64) -> Result<Vec<Wall>> {
    let count = walls.len();
    let mut out = Vec::new();
    for i in 0..count {
        let a = at_ref(walls, (i + count - 1) % count)?;
        let b = at_ref(walls, i)?;
        let ta = if a.arc {
            a.axis.cross(b.start - a.center)
        } else {
            a.dir
        };
        let tb = if b.arc {
            b.axis.cross(b.start - b.center)
        } else {
            b.dir
        };
        let ta = ta.unitize()?;
        let tb = tb.unitize()?;
        let turn = n.ccw_angle_between(ta, tb)?;
        // the offset parts where the corner turns away from the side it goes to
        let gap = turn.abs() > EFF_ZERO_SQRT && side * turn < 0.0;
        if gap {
            for (arc, dir) in [(a.arc, ta), (b.arc, tb)] {
                if !arc {
                    continue;
                }
                // after the arc ending here (side 0), before the one starting here (side 1)
                out.push(Wall {
                    arc: false,
                    point: true,
                    start: b.start,
                    end: b.start,
                    dir,
                    offset: side * offset * n.cross(dir),
                    ..Wall::default()
                });
            }
        }
        out.push(b.clone());
    }
    Ok(out)
}

// ---- WR-571: walls that close partway up (the fork's source fork@183ea65) ------

/// The unit tangent of `w`'s section at `q` (in the profile's plane), the way the
/// profile runs (`sm_DraftTangent`).
fn tangent_at(w: &Wall, q: Point3) -> Result<Vec3> {
    let t = if w.arc {
        w.axis.cross(q - w.center)
    } else {
        w.dir
    };
    t.unitize()
}

/// The seam where walls `a` and `b` cross, from `seam.p` (at the fraction `seam.f0` of
/// the height) to `seam.x` (at `seam.f1`), both on the crossing: the rational quadratic
/// through them with the walls' common tangents there, through their crossing halfway
/// up. A seam that is not that conic (checked on both walls) is refused
/// (`sm_DraftSharpSeam`).
fn sharp_seam(
    a: &Wall,
    b: &Wall,
    n: Vec3,
    h: Vec3,
    tol: f64,
    seam: &mut Seam,
) -> Result<core::result::Result<(), CornerRefusal>> {
    let height = h.dot(n);
    let p2 = seam.p - seam.f0 * h;
    let x2 = seam.x - seam.f1 * h;
    let f_mid = 0.5 * (seam.f0 + seam.f1);
    let refusal = |reason| Ok(Err(CornerRefusal { reason, corner: p2 }));
    let Some(mut mid) = crossing(a, b, f_mid, n, p2) else {
        return refusal(TaperCornerRefusal::Apart);
    };
    mid += f_mid * h;
    // the walls' normals at both ends
    let na = wall_normal(a, p2, h)?;
    let nb = wall_normal(b, p2, h)?;
    let na1 = wall_normal(a, x2, h)?;
    let nb1 = wall_normal(b, x2, h)?;
    let mut t0 = na.cross(nb);
    let mut t1 = na1.cross(nb1);
    if t0.dot(h) < 0.0 {
        t0 = -t0;
    }
    if t1.dot(h) < 0.0 {
        t1 = -t1;
    }
    let p = seam.p;
    let chord = seam.x - p;
    let scale = chord.length();
    let t0 = t0.unitize()?;
    let t1 = t1.unitize()?;
    let off_mid = (mid - p) - ((mid - p).dot(chord) / chord.dot(chord)) * chord;
    if off_mid.length() <= EFF_ZERO_SQRT * scale {
        // a straight seam (two planes)
        seam.s = 0.5 * (p + seam.x);
        seam.weight = 1.0;
    } else {
        // the shoulder where the end tangents cross, the weight from the halfway point
        let (d0, d1) = line_line_closest_point(p, t0, seam.x, t1)?;
        let s0 = p + d0 * t0;
        let s1 = seam.x + d1 * t1;
        if s0.distance(s1) > EFF_ZERO_SQRT * scale || !(d0 > 0.0) || !(d1 < 0.0) {
            return refusal(TaperCornerRefusal::NotExact);
        }
        seam.s = 0.5 * (s0 + s1);
        // M - P = b1 (S - P) + b2 (X - P)
        let e1 = seam.s - p;
        let e2 = seam.x - p;
        let m = mid - p;
        let a11 = e1.dot(e1);
        let a12 = e1.dot(e2);
        let a22 = e2.dot(e2);
        let r1 = m.dot(e1);
        let r2 = m.dot(e2);
        let det = a11 * a22 - a12 * a12;
        let b1 = (r1 * a22 - r2 * a12) / det;
        let b2 = (a11 * r2 - a12 * r1) / det;
        let b0 = 1.0 - b1 - b2;
        if !(det.abs() > 0.0) || !(b0 > 0.0) || !(b1 > 0.0) || !(b2 > 0.0) {
            return refusal(TaperCornerRefusal::NotExact);
        }
        seam.weight = b1 / (2.0 * (b0 * b2).sqrt());
    }
    // the seam must lie on both walls
    for k in 1..8 {
        let q = seam_point(seam, f64::from(k) / 8.0);
        let f = seam.f0 + (q - seam.p).dot(n) / height;
        let qp = q - f * h;
        if off_section(a, f, qp) > tol || off_section(b, f, qp) > tol {
            return refusal(TaperCornerRefusal::NotExact);
        }
    }
    Ok(Ok(()))
}

/// What a piece's seam follows (`sm_DraftPiece::eKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PieceKind {
    /// Where the two walls cross.
    Crossing,
    /// Their common ruling (a smooth joint).
    Ruling,
    /// The line where two walls facing each other meet, at one height.
    Ridge,
}

/// Two walls next to each other from one height to another, and the seam between them
/// there (`sm_DraftPiece`).
#[derive(Debug, Clone, Copy)]
struct Piece {
    /// The wall before.
    a: usize,
    /// The wall after.
    b: usize,
    kind: PieceKind,
    /// Its ends (`p`, `f0` and `x`, `f1`), then its curve.
    seam: Seam,
    /// Of no length: left out.
    none: bool,
}

/// Where the piece's seam is at fraction `f` of the height, in the profile's plane;
/// `None` when its walls do not cross there (`sm_DraftPieceAt`).
fn piece_at(walls: &[Wall], piece: &Piece, n: Vec3, h: Vec3, f: f64) -> Result<Option<Point3>> {
    let base = piece.seam.p - piece.seam.f0 * h;
    match piece.kind {
        PieceKind::Crossing => Ok(crossing(
            at_ref(walls, piece.a)?,
            at_ref(walls, piece.b)?,
            f,
            n,
            base,
        )),
        PieceKind::Ridge => Ok(Some(base)),
        PieceKind::Ruling => {
            let wb = at_ref(walls, piece.b)?;
            if !wb.arc {
                return Ok(Some(base + (f - piece.seam.f0) * wb.offset));
            }
            let Ok(r) = (base - wb.center).unitize() else {
                return Ok(None);
            };
            Ok(Some(base - (f - piece.seam.f0) * wb.shrink * r))
        }
    }
}

/// The state of the walls' sections followed up the height.
struct Sections<'a> {
    walls: &'a [Wall],
    pieces: Vec<Piece>,
    /// A live wall's open pieces: the one it starts from and the one it ends at.
    ins: Vec<usize>,
    outs: Vec<usize>,
    n: Vec3,
    h: Vec3,
}

impl Sections<'_> {
    /// Where piece `p` is at `f`, in the profile's plane.
    fn at(&self, p: usize, f: f64) -> Result<Option<Point3>> {
        piece_at(self.walls, at_ref(&self.pieces, p)?, self.n, self.h, f)
    }

    /// Piece `p` ends at `f`, at the point `q` of the profile's plane.
    fn end(&mut self, p: usize, f: f64, q: Point3) -> Result<()> {
        let h = self.h;
        let piece = self
            .pieces
            .get_mut(p)
            .ok_or_else(|| SolidError::sm_err(OP))?;
        piece.seam.x = q + f * h;
        piece.seam.f1 = f;
        Ok(())
    }

    /// The length of live wall `w`'s section at `f` (an arc's as an angle): its parameter
    /// where it crosses the wall after less where it crosses the wall before. An arc's
    /// start angle `start` is followed from the call before, its length from `hint`.
    /// `None` when a crossing is missing.
    fn length(&self, w: usize, f: f64, start: &mut f64, hint: f64) -> Result<Option<f64>> {
        let wall = at_ref(self.walls, w)?;
        let (Some(xa), Some(xb)) = (
            self.at(at(&self.ins, w)?, f)?,
            self.at(at(&self.outs, w)?, f)?,
        ) else {
            return Ok(None);
        };
        if !wall.arc {
            *start = (xa - wall.start - f * wall.offset).dot(wall.dir);
            return Ok(Some(
                (xb - wall.start - f * wall.offset).dot(wall.dir) - *start,
            ));
        }
        *start = angle(wall, xa, *start);
        Ok(Some(angle(wall, xb, *start + hint) - *start))
    }

    /// A refusal of `reason` at piece `p`'s low end, in the profile's plane.
    fn refuse(&self, reason: TaperCornerRefusal, p: usize) -> Result<Walls> {
        let seam = at_ref(&self.pieces, p)?.seam;
        Ok(refuse(reason, seam.p - seam.f0 * self.h))
    }
}

/// Steps in which each live wall's section is followed up the height.
const SECTION_STEPS: u32 = 64;

/// Halvings to a wall's closing height.
const CLOSING_HALVINGS: u32 = 100;

/// Where a wall's seam pcurves go: the profile's plane normal, the height, and the
/// wall surface's `u` range, `v` origin and `v` scale.
struct PcurveFrame {
    n: Vec3,
    h: Vec3,
    u: [f64; 2],
    v0: f64,
    v_scale: f64,
}

/// A wall face's loop: its edges' curves, pcurves and orientations, and each edge's seam
/// (for a refusal's corner).
#[derive(Default)]
struct EdgeLoop {
    curves: Vec<BSplineCurve>,
    pcurves: Vec<BSplineCurve>,
    orientations: Vec<OrientType>,
    of: Vec<Seam>,
}

impl EdgeLoop {
    fn push(&mut self, curve: BSplineCurve, pcurve: BSplineCurve, orient: OrientType, of: Seam) {
        self.curves.push(curve);
        self.pcurves.push(pcurve);
        self.orientations.push(orient);
        self.of.push(of);
    }

    /// A seam's curve and pcurve on wall `w`; `Some(corner)` when the pcurve cannot be
    /// made (refused at the seam's low end, in the profile's plane).
    fn seam(
        &mut self,
        w: &Wall,
        seam: &Seam,
        frame: &PcurveFrame,
        orient: OrientType,
    ) -> Result<Option<Point3>> {
        let curve = seam_curve(seam)?;
        let pcurve = match seam_pcurve(
            w,
            seam,
            frame.n,
            frame.h,
            frame.u,
            w.lo,
            frame.v0,
            frame.v_scale,
            true,
        ) {
            Ok(pc) => pc,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(Some(seam.p - seam.f0 * frame.h)),
        };
        self.push(curve, pcurve, orient, *seam);
        Ok(None)
    }
}

/// [`curvature_keeping_walls`] with `fixes.taper_wall_closes_at_apex` (WR-571; the fork's
/// source fork@183ea65): the seams come from following the section up. Each live
/// wall's length between its crossings with its neighbours is followed to the first
/// height where it is gone (halved to the root); there the wall ends at its apex and its
/// neighbours meet from that point up, a new seam built the same exact way. Two lines
/// brought face to face so (a slot's sides) close together along their common line, and
/// the walls beyond them meet from its other end. Each wall's face runs over its seams in
/// order; a closed wall has no far edge. A wall closing at the far end, the whole section
/// closing, and an arc shrinking to its centre are still refused.
#[allow(clippy::too_many_lines)] // one fork routine
fn walls_closing_at_apexes(
    a: &mut impl PrimitiveCreationAccess,
    kernel_brep: BrepId,
    target: BrepId,
    input: &WallsInput<'_>,
    mut walls: Vec<Wall>,
) -> Result<Walls> {
    let n = input.normal;
    let tol = input.tol;
    let h = input.height * n;
    let count = walls.len();

    // The section at every height is the walls' sections, each from where it crosses the
    // wall before to where it crosses the wall after. Piece i starts at the profile's
    // corner where wall i-1 ends and wall i starts; each event ends pieces and starts one.
    let mut sections = Sections {
        walls: &walls,
        pieces: Vec::new(),
        ins: vec![0; count],
        outs: vec![0; count],
        n,
        h,
    };
    let mut prev = vec![0; count];
    let mut next = vec![0; count];
    let mut live = vec![true; count];
    let mut live_count = count;
    for i in 0..count {
        let before = (i + count - 1) % count;
        let wa = at_ref(&walls, before)?;
        let wb = at_ref(&walls, i)?;
        let ta = tangent_at(wa, wb.start)?;
        let tb = tangent_at(wb, wb.start)?;
        let turn = n.ccw_angle_between(ta, tb)?;
        sections.pieces.push(Piece {
            a: before,
            b: i,
            kind: if turn.abs() > EFF_ZERO_SQRT {
                PieceKind::Crossing
            } else {
                PieceKind::Ruling
            },
            seam: Seam {
                p: wb.start,
                f0: 0.0,
                ..Seam::default()
            },
            none: false,
        });
        *sections
            .ins
            .get_mut(i)
            .ok_or_else(|| SolidError::sm_err(OP))? = i;
        *sections
            .outs
            .get_mut(before)
            .ok_or_else(|| SolidError::sm_err(OP))? = i;
        *prev.get_mut(i).ok_or_else(|| SolidError::sm_err(OP))? = before;
        *next.get_mut(i).ok_or_else(|| SolidError::sm_err(OP))? = (i + 1) % count;
    }
    let set = |v: &mut Vec<usize>, i: usize, x: usize| -> Result<()> {
        *v.get_mut(i).ok_or_else(|| SolidError::sm_err(OP))? = x;
        Ok(())
    };
    let mut f_cur = 0.0;
    for _event in 0..=count {
        // the lowest height at which a live wall closes: its length followed up in steps
        // (an arc's angles stay continuous), the first step at which it is gone, then
        // halved to the root
        let mut f_event = 2.0;
        let mut w_event = 0;
        for w in 0..count {
            if !at(&live, w)? {
                continue;
            }
            let wall = at_ref(&walls, w)?;
            let mut start = if wall.arc { wall.start_ang } else { 0.0 };
            let hint = if wall.arc {
                wall.end_ang - wall.start_ang
            } else {
                0.0
            };
            let Some(mut len) = sections.length(w, f_cur, &mut start, hint)? else {
                return sections.refuse(TaperCornerRefusal::Apart, at(&sections.ins, w)?);
            };
            let mut f_lo = f_cur;
            let mut f_hi = 1.0;
            let mut closes = false;
            for k in 1..=SECTION_STEPS {
                if closes {
                    break;
                }
                let f = f_cur + (1.0 - f_cur) * f64::from(k) / f64::from(SECTION_STEPS);
                let mut s = start;
                let Some(l) = sections.length(w, f, &mut s, len)? else {
                    return sections.refuse(TaperCornerRefusal::Apart, at(&sections.ins, w)?);
                };
                if l <= 0.0 {
                    closes = true;
                    f_hi = f;
                } else {
                    f_lo = f;
                    start = s;
                    len = l;
                }
            }
            if !closes {
                // a wall reaching the far end with no length closes there: the far end
                // would pinch
                if len <= if wall.arc { tol / wall.radius } else { tol } {
                    return sections.refuse(TaperCornerRefusal::WallCloses, at(&sections.ins, w)?);
                }
                continue;
            }
            for _ in 0..CLOSING_HALVINGS {
                if !(f_lo < f_hi) {
                    break;
                }
                let f = 0.5 * (f_lo + f_hi);
                let mut s = start;
                let Some(l) = sections.length(w, f, &mut s, len)? else {
                    return sections.refuse(TaperCornerRefusal::Apart, at(&sections.ins, w)?);
                };
                if l > 0.0 {
                    if f <= f_lo {
                        break;
                    }
                    f_lo = f;
                    start = s;
                    len = l;
                } else {
                    if f >= f_hi {
                        break;
                    }
                    f_hi = f;
                }
            }
            let f = 0.5 * (f_lo + f_hi);
            if f < f_event {
                f_event = f;
                w_event = w;
            }
        }
        if f_event > 1.0 {
            break;
        }
        let f = f_event;
        let w = w_event;
        let w_in = at(&sections.ins, w)?;
        if (1.0 - f) * input.height.abs() <= tol || at_ref(&walls, w)?.point {
            return sections.refuse(TaperCornerRefusal::WallCloses, w_in);
        }
        // the wall ends at its apex
        let (Some(xa), Some(xb)) = (
            sections.at(w_in, f)?,
            sections.at(at(&sections.outs, w)?, f)?,
        ) else {
            return sections.refuse(TaperCornerRefusal::Apart, w_in);
        };
        let mut apex = 0.5 * (xa + xb);
        sections.end(w_in, f, apex)?;
        sections.end(at(&sections.outs, w)?, f, apex)?;
        let mut wa = at(&prev, w)?;
        let mut wb = at(&next, w)?;
        *live.get_mut(w).ok_or_else(|| SolidError::sm_err(OP))? = false;
        live_count -= 1;
        set(&mut next, wa, wb)?;
        set(&mut prev, wb, wa)?;
        // its neighbours meet from there up (each ridge closes two walls, so the loop ends
        // before every wall has closed)
        let mut met = false;
        for _ in 0..=count {
            if live_count < 3 || wa == wb {
                return sections.refuse(TaperCornerRefusal::WallCloses, w_in);
            }
            let ta = tangent_at(at_ref(&walls, wa)?, apex)?;
            let tb = tangent_at(at_ref(&walls, wb)?, apex)?;
            let lines = !at_ref(&walls, wa)?.arc && !at_ref(&walls, wb)?.arc;
            if lines && ta.dot(tb) < -1.0 + EFF_ZERO_SQRT {
                // face to face: both close along their common line, to where the walls
                // beyond meet them
                let (Some(ea), Some(eb)) = (
                    sections.at(at(&sections.ins, wa)?, f)?,
                    sections.at(at(&sections.outs, wb)?, f)?,
                ) else {
                    return sections.refuse(TaperCornerRefusal::Apart, at(&sections.ins, wa)?);
                };
                if ea.distance(eb) > tol {
                    return sections.refuse(TaperCornerRefusal::WallCloses, w_in);
                }
                let end = 0.5 * (ea + eb);
                sections.pieces.push(Piece {
                    a: wa,
                    b: wb,
                    kind: PieceKind::Ridge,
                    seam: Seam {
                        p: apex + f * h,
                        f0: f,
                        ..Seam::default()
                    },
                    none: false,
                });
                sections.end(sections.pieces.len() - 1, f, end)?;
                sections.end(at(&sections.ins, wa)?, f, end)?;
                sections.end(at(&sections.outs, wb)?, f, end)?;
                let c = at(&prev, wa)?;
                let d = at(&next, wb)?;
                *live.get_mut(wa).ok_or_else(|| SolidError::sm_err(OP))? = false;
                *live.get_mut(wb).ok_or_else(|| SolidError::sm_err(OP))? = false;
                live_count -= 2;
                if live_count < 3 || c == wb || d == wa {
                    return sections.refuse(TaperCornerRefusal::WallCloses, w_in);
                }
                set(&mut next, c, d)?;
                set(&mut prev, d, c)?;
                wa = c;
                wb = d;
                apex = end;
                continue;
            }
            let turn = n.ccw_angle_between(ta, tb)?;
            sections.pieces.push(Piece {
                a: wa,
                b: wb,
                kind: if turn.abs() > EFF_ZERO_SQRT {
                    PieceKind::Crossing
                } else {
                    PieceKind::Ruling
                },
                seam: Seam {
                    p: apex + f * h,
                    f0: f,
                    ..Seam::default()
                },
                none: false,
            });
            let p = sections.pieces.len() - 1;
            set(&mut sections.outs, wa, p)?;
            set(&mut sections.ins, wb, p)?;
            met = true;
            break;
        }
        if !met {
            return Err(SolidError::invalid_input(
                OP,
                "draft wall closing bound exceeded",
            ));
        }
        f_cur = f;
    }
    // the pieces still open reach the far end
    for w in 0..count {
        if !at(&live, w)? {
            continue;
        }
        let p = at(&sections.outs, w)?;
        let Some(x) = sections.at(p, 1.0)? else {
            return sections.refuse(TaperCornerRefusal::Apart, p);
        };
        sections.end(p, 1.0, x)?;
    }
    // the seams' curves; a piece of no length is left out
    let mut pieces = sections.pieces;
    for piece in &mut pieces {
        let seam = &mut piece.seam;
        if seam.p.distance(seam.x) <= tol {
            piece.none = true;
            continue;
        }
        if piece.kind == PieceKind::Crossing {
            let wa = at_ref(&walls, piece.a)?;
            let wb = at_ref(&walls, piece.b)?;
            if let Err(refusal) = sharp_seam(wa, wb, n, h, tol, seam)? {
                return Ok(Err(refusal));
            }
        } else {
            // a ruling, or the line two walls close along
            seam.s = 0.5 * (seam.p + seam.x);
            seam.weight = 1.0;
        }
    }
    // each wall's seams from its start corner up and from its end corner up, in order
    let mut starts: Vec<Vec<Seam>> = vec![Vec::new(); count];
    let mut ends: Vec<Vec<Seam>> = vec![Vec::new(); count];
    for piece in &pieces {
        if piece.none {
            continue;
        }
        ends.get_mut(piece.a)
            .ok_or_else(|| SolidError::sm_err(OP))?
            .push(piece.seam);
        starts
            .get_mut(piece.b)
            .ok_or_else(|| SolidError::sm_err(OP))?
            .push(piece.seam);
    }
    for (i, (s, e)) in starts.iter().zip(&ends).enumerate() {
        if s.is_empty() || e.is_empty() {
            return Ok(refuse(
                TaperCornerRefusal::WallCloses,
                at_ref(&walls, i)?.start,
            ));
        }
    }

    // each wall's extent: from its start seams' reach to its end seams', in spans that
    // each hold a seam
    let h2 = h.dot(n);
    for (i, w) in walls.iter_mut().enumerate() {
        let chains = [at_ref(&starts, i)?, at_ref(&ends, i)?];
        let ref0 = if w.arc { w.start_ang } else { 0.0 };
        let ref1 = if w.arc {
            w.end_ang
        } else {
            (w.end - w.start).dot(w.dir)
        };
        let mut reach = [(0.0, 0.0); 2];
        for (slot, (chain, rf)) in reach.iter_mut().zip(chains.into_iter().zip([ref0, ref1])) {
            let mut lo = rf;
            let mut hi = rf;
            for seam in chain {
                for k in 0..=8 {
                    let q = seam_point(seam, f64::from(k) / 8.0);
                    let f = seam.f0 + (q - seam.p).dot(n) / h2;
                    let qp = q - f * h;
                    let t = if w.arc {
                        angle(w, qp, rf)
                    } else {
                        (qp - w.start - f * w.offset).dot(w.dir)
                    };
                    lo = min2(lo, t);
                    hi = max2(hi, t);
                }
            }
            let pad = 0.05 * (hi - lo) + if w.arc { 1e-3 } else { 1e-3 * input.offset };
            *slot = (lo - pad, hi + pad);
        }
        let [(lo0, hi0), (lo1, hi1)] = reach;
        let first_start = at(at_ref(&starts, i)?, 0)?.p;
        let first_end = at(at_ref(&ends, i)?, 0)?.p;
        w.lo = lo0;
        w.hi = hi1;
        w.breaks.clear();
        if w.arc {
            let quarter = 0.5 * PI;
            w.breaks.push(lo0);
            if hi0 >= lo1 {
                // one span holds both ends' seams
                if hi1 - lo0 >= PI - EFF_ZERO_SQRT {
                    return Ok(refuse(TaperCornerRefusal::WallCloses, first_start));
                }
            } else {
                w.breaks.push(hi0);
                // (at most the arc's angle over a quarter turn: lo1 and hi0 lie within
                // a turn and a half of the arc's start angle)
                let spans = ((lo1 - hi0) / quarter).ceil() as u64;
                for k in 1..spans {
                    w.breaks.push(hi0 + (lo1 - hi0) * k as f64 / spans as f64);
                }
                w.breaks.push(lo1);
                if hi0 - lo0 >= PI - EFF_ZERO_SQRT || hi1 - lo1 >= PI - EFF_ZERO_SQRT {
                    let corner = if hi0 - lo0 >= hi1 - lo1 {
                        first_start
                    } else {
                        first_end
                    };
                    return Ok(refuse(TaperCornerRefusal::NotExact, corner));
                }
            }
            w.breaks.push(hi1);
        }
    }

    // the faces
    let mut far_curves = Vec::new();
    for i in 0..count {
        let w = at_ref(&walls, i)?;
        let w_starts = at_ref(&starts, i)?;
        let w_ends = at_ref(&ends, i)?;
        let face = {
            let k = a.kernel(kernel_brep)?;
            let low = wall_curve(w, 0.0, h)?;
            let high = wall_curve(w, 1.0, h)?;
            let surface = BSplineSurface::create_ruled_surface(&low, &high, SurfParamType::U)?;
            let domain = surface.natural_uv_domain();
            let u0 = domain.u_min();
            let u1 = domain.u_max();
            let v_scale = (domain.v_max() - domain.v_min()) / (w.hi - w.lo);
            let v0 = domain.v_min();
            let mid = if w.arc {
                0.5 * (w.start_ang + w.end_ang)
            } else {
                0.0
            };
            let near0 = if w.arc { w.start_ang } else { mid };
            let near1 = if w.arc { w.end_ang } else { mid };
            // the loop: the profile's curve (none for a tangent plane's piece), the end
            // seams up, the far end's curve back (none for a wall that closes), the start
            // seams down; each edge's seam, for a refusal's corner
            let frame = PcurveFrame {
                n,
                h,
                u: [u0, u1],
                v0,
                v_scale,
            };
            let mut edges = EdgeLoop::default();
            if !w.point {
                let s0 = *w_starts.first().ok_or_else(|| SolidError::sm_err(OP))?;
                let e0 = *w_ends.first().ok_or_else(|| SolidError::sm_err(OP))?;
                let tp0 = param(w, s0.p, 0.0, near0)?;
                let tp1 = param(w, e0.p, 0.0, near1)?;
                let mut bottom = low.copy();
                bottom.trim(k, &mut Extent1d::from_min_max(tp0, tp1), TrimMode::Notify)?;
                let now = bottom.natural_interval();
                let pc_bottom = iso_pcurve(
                    u0,
                    v0 + (now.min() - w.lo) * v_scale,
                    u0,
                    v0 + (now.max() - w.lo) * v_scale,
                    now.min(),
                    now.max(),
                )?;
                edges.push(bottom, pc_bottom, OrientType::Same, s0);
            }
            for seam in w_ends {
                if let Some(corner) = edges.seam(w, seam, &frame, OrientType::Same)? {
                    return Ok(refuse(TaperCornerRefusal::NotExact, corner));
                }
            }
            let mut top_curve = None;
            if at(&live, i)? {
                let last_start = *w_starts.last().ok_or_else(|| SolidError::sm_err(OP))?;
                let last_end = *w_ends.last().ok_or_else(|| SolidError::sm_err(OP))?;
                let tx0 = param(w, last_start.x - h, 1.0, near0)?;
                let tx1 = param(w, last_end.x - h, 1.0, near1)?;
                let mut top = high.copy();
                top.trim(k, &mut Extent1d::from_min_max(tx0, tx1), TrimMode::Notify)?;
                let top_now = top.natural_interval();
                let pc_top = iso_pcurve(
                    u1,
                    v0 + (top_now.min() - w.lo) * v_scale,
                    u1,
                    v0 + (top_now.max() - w.lo) * v_scale,
                    top_now.min(),
                    top_now.max(),
                )?;
                top_curve = Some(top.copy());
                edges.push(top, pc_top, OrientType::Opposite, last_end);
            }
            for seam in w_starts.iter().rev() {
                if let Some(corner) = edges.seam(w, seam, &frame, OrientType::Opposite)? {
                    return Ok(refuse(TaperCornerRefusal::NotExact, corner));
                }
            }
            let EdgeLoop {
                curves,
                pcurves,
                orientations,
                of,
            } = edges;
            // every pcurve must lift onto its curve
            for ((c, pc), seam) in curves.iter().zip(&pcurves).zip(&of) {
                let ivl = c.natural_interval();
                for step in 0..=8 {
                    let t = ivl.evaluate(f64::from(step) / 8.0);
                    let uv = pc.evaluate_point(t)?;
                    let on = surface.evaluate_point(Point2::new(uv.x, uv.y))?;
                    let at_t = c.evaluate_point(t)?;
                    if on.distance(at_t) > tol {
                        return Ok(refuse(TaperCornerRefusal::NotExact, seam.p - seam.f0 * h));
                    }
                }
            }
            // the far end's curve, for its cap
            if let Some(top) = top_curve {
                far_curves.push(Curve::from(top));
            }
            let face_curves = FaceCurvesInput {
                loops: vec![curves.len()],
                curves_3d: Some(curves.into_iter().map(|c| Some(Curve::from(c))).collect()),
                uv_curves: Some(pcurves.into_iter().map(Curve::from).collect()),
                orientations,
                loop_points: Vec::new(),
            };
            (face_curves, Surface::from(surface), domain)
        };
        // the loop runs clockwise in (u, v): opposite to the surface's normal
        let (curves, surface, domain) = face;
        a.face_with_curves(target, curves, surface, domain, OrientType::Opposite)?;
    }
    Ok(Ok(far_curves))
}

/// The side faces of a draft extrusion of the closed profile `input.composite`, into
/// the B-rep `target`, and the far end's curves in order
/// (`sm_CreateCurvatureKeepingWalls`). Its walls keep their curvature at its corners;
/// with `input.tangent`, where a corner opens a gap, each continues as the plane
/// tangent to it there instead. A corner that cannot be closed is `Ok(Err(refusal))`
/// (upstream's `SM_ERR_INVALID_INPUT` with why and where). `kernel_brep` names the
/// B-rep whose kernel evaluates the curves.
#[allow(clippy::too_many_lines)] // one fork routine
pub(super) fn curvature_keeping_walls(
    a: &mut impl PrimitiveCreationAccess,
    kernel_brep: BrepId,
    target: BrepId,
    input: &WallsInput<'_>,
) -> Result<Walls> {
    let n = input.normal;
    let tol = input.tol;
    if input.composite.segments().is_empty() {
        return Err(SolidError::invalid_input(OP, "the profile has no curves"));
    }
    let h = input.height * n;
    let side = if input.side == OffsetDirection::LeftHandSide {
        1.0
    } else {
        -1.0
    };

    // the walls
    let mut walls = match read_walls(a.kernel(kernel_brep)?, input)? {
        Ok(walls) => walls,
        Err(refusal) => return Ok(Err(refusal)),
    };
    if input.tangent {
        walls = with_tangent_planes(&walls, n, input.offset, side)?;
    }
    if a.kernel(kernel_brep)?.fixes.taper_wall_closes_at_apex {
        return walls_closing_at_apexes(a, kernel_brep, target, input, walls);
    }
    let count = walls.len();

    // the seams: seam i is the corner where wall i-1 ends and wall i starts
    let mut seams = Vec::new();
    for i in 0..count {
        let wa = at_ref(&walls, (i + count - 1) % count)?;
        let wb = at_ref(&walls, i)?;
        let mut seam = Seam {
            p: wb.start,
            ..Seam::default()
        };
        let p = seam.p;
        // the walls' normals at the corner
        let na = wall_normal(wa, p, h)?;
        let nb = wall_normal(wb, p, h)?;
        let ta = if wa.arc {
            wa.axis.cross(p - wa.center)
        } else {
            wa.dir
        };
        let tb = if wb.arc {
            wb.axis.cross(p - wb.center)
        } else {
            wb.dir
        };
        let turn = n.ccw_angle_between(ta, tb)?;
        let sharp = turn.abs() > EFF_ZERO_SQRT;
        if !sharp {
            // a smooth joint: the common ruling
            if wb.arc {
                let r = (p - wb.center).unitize()?;
                seam.x = p - wb.shrink * r + h;
            } else {
                seam.x = p + wb.offset + h;
            }
            seam.s = 0.5 * (p + seam.x);
            seam.weight = 1.0;
            seams.push(seam);
            continue;
        }
        let (Some(mut mid), Some(far)) = (crossing(wa, wb, 0.5, n, p), crossing(wa, wb, 1.0, n, p))
        else {
            return Ok(refuse(TaperCornerRefusal::Apart, p));
        };
        let na1 = wall_normal(wa, far, h)?;
        let nb1 = wall_normal(wb, far, h)?;
        mid += 0.5 * h;
        seam.x = far + h;
        let mut t0 = na.cross(nb);
        let mut t1 = na1.cross(nb1);
        if t0.dot(h) < 0.0 {
            t0 = -t0;
        }
        if t1.dot(h) < 0.0 {
            t1 = -t1;
        }
        let chord = seam.x - p;
        let scale = chord.length();
        let t0 = t0.unitize()?;
        let t1 = t1.unitize()?;
        let off_mid = (mid - p) - ((mid - p).dot(chord) / chord.dot(chord)) * chord;
        if off_mid.length() <= EFF_ZERO_SQRT * scale {
            // a straight seam (two planes)
            seam.s = 0.5 * (p + seam.x);
            seam.weight = 1.0;
        } else {
            // the shoulder where the end tangents cross, the weight from the halfway point
            let (d0, d1) = line_line_closest_point(p, t0, seam.x, t1)?;
            let s0 = p + d0 * t0;
            let s1 = seam.x + d1 * t1;
            if s0.distance(s1) > EFF_ZERO_SQRT * scale || !(d0 > 0.0) || !(d1 < 0.0) {
                return Ok(refuse(TaperCornerRefusal::NotExact, p));
            }
            seam.s = 0.5 * (s0 + s1);
            // M - P = b1 (S - P) + b2 (X - P)
            let e1 = seam.s - p;
            let e2 = seam.x - p;
            let m = mid - p;
            let a11 = e1.dot(e1);
            let a12 = e1.dot(e2);
            let a22 = e2.dot(e2);
            let r1 = m.dot(e1);
            let r2 = m.dot(e2);
            let det = a11 * a22 - a12 * a12;
            let b1 = (r1 * a22 - r2 * a12) / det;
            let b2 = (a11 * r2 - a12 * r1) / det;
            let b0 = 1.0 - b1 - b2;
            if !(det.abs() > 0.0) || !(b0 > 0.0) || !(b1 > 0.0) || !(b2 > 0.0) {
                return Ok(refuse(TaperCornerRefusal::NotExact, p));
            }
            seam.weight = b1 / (2.0 * (b0 * b2).sqrt());
        }
        // the seam must lie on both walls
        for k in 1..8 {
            let q = seam_point(&seam, f64::from(k) / 8.0);
            let f = (q - p).dot(n) / h.dot(n);
            let qp = q - f * h;
            if off_section(wa, f, qp) > tol || off_section(wb, f, qp) > tol {
                return Ok(refuse(TaperCornerRefusal::NotExact, p));
            }
        }
        seams.push(seam);
    }

    // each wall's extent: from its first seam's reach to its last's, in spans that each
    // hold a seam
    for i in 0..count {
        let s0 = at(&seams, i)?;
        let s1 = at(&seams, (i + 1) % count)?;
        let w = walls.get_mut(i).ok_or_else(|| SolidError::sm_err(OP))?;
        let ref0 = if w.arc { w.start_ang } else { 0.0 };
        let ref1 = if w.arc {
            w.end_ang
        } else {
            (w.end - w.start).dot(w.dir)
        };
        let mut reach = [(0.0, 0.0); 2];
        for (slot, (seam, rf)) in reach.iter_mut().zip([(s0, ref0), (s1, ref1)]) {
            let mut lo = rf;
            let mut hi = rf;
            for k in 0..=8 {
                let q = seam_point(&seam, f64::from(k) / 8.0);
                let f = (q - seam.p).dot(n) / h.dot(n);
                let qp = q - f * h;
                let t = if w.arc {
                    angle(w, qp, rf)
                } else {
                    (qp - w.start - f * w.offset).dot(w.dir)
                };
                lo = min2(lo, t);
                hi = max2(hi, t);
            }
            let pad = 0.05 * (hi - lo) + if w.arc { 1e-3 } else { 1e-3 * input.offset };
            *slot = (lo - pad, hi + pad);
        }
        let [(lo0, hi0), (lo1, hi1)] = reach;
        // the far end must stay longer than nothing
        let q0 = s0.x - h;
        let q1 = s1.x - h;
        let far0 = if w.arc {
            angle(w, q0, ref0)
        } else {
            (q0 - w.start - w.offset).dot(w.dir)
        };
        let far1 = if w.arc {
            angle(w, q1, ref1)
        } else {
            (q1 - w.start - w.offset).dot(w.dir)
        };
        if !(far1 > far0 + tol / if w.arc { w.radius } else { 1.0 }) {
            return Ok(refuse(TaperCornerRefusal::WallCloses, s0.p));
        }
        w.lo = lo0;
        w.hi = hi1;
        w.breaks.clear();
        if w.arc {
            let quarter = 0.5 * PI;
            w.breaks.push(lo0);
            if hi0 >= lo1 {
                // one span holds both seams
                if hi1 - lo0 >= PI - EFF_ZERO_SQRT {
                    return Ok(refuse(TaperCornerRefusal::WallCloses, s0.p));
                }
            } else {
                w.breaks.push(hi0);
                // (at most the arc's angle over a quarter turn: lo1 and hi0 lie within
                // a turn and a half of the arc's start angle)
                let spans = ((lo1 - hi0) / quarter).ceil() as u64;
                for k in 1..spans {
                    w.breaks.push(hi0 + (lo1 - hi0) * k as f64 / spans as f64);
                }
                w.breaks.push(lo1);
                if hi0 - lo0 >= PI - EFF_ZERO_SQRT || hi1 - lo1 >= PI - EFF_ZERO_SQRT {
                    let corner = if hi0 - lo0 >= hi1 - lo1 { s0.p } else { s1.p };
                    return Ok(refuse(TaperCornerRefusal::NotExact, corner));
                }
            }
            w.breaks.push(hi1);
        }
    }

    // the faces
    let mut far_curves = Vec::new();
    for i in 0..count {
        let w = at_ref(&walls, i)?;
        let s0 = at(&seams, i)?;
        let s1 = at(&seams, (i + 1) % count)?;
        let face = {
            let k = a.kernel(kernel_brep)?;
            let low = wall_curve(w, 0.0, h)?;
            let high = wall_curve(w, 1.0, h)?;
            let surface = BSplineSurface::create_ruled_surface(&low, &high, SurfParamType::U)?;
            let domain = surface.natural_uv_domain();
            let u0 = domain.u_min();
            let u1 = domain.u_max();
            let v_scale = (domain.v_max() - domain.v_min()) / (w.hi - w.lo);
            let v0 = domain.v_min();
            // the corners' parameters on the wall: at the profile and at the far end
            let mid = if w.arc {
                0.5 * (w.start_ang + w.end_ang)
            } else {
                0.0
            };
            let near0 = if w.arc { w.start_ang } else { mid };
            let near1 = if w.arc { w.end_ang } else { mid };
            let tp0 = param(w, s0.p, 0.0, near0)?;
            let tp1 = param(w, s1.p, 0.0, near1)?;
            let tx0 = param(w, s0.x - h, 1.0, near0)?;
            let tx1 = param(w, s1.x - h, 1.0, near1)?;
            // the loop: the profile's curve (none for a tangent plane's piece), the end
            // seam, the far end's curve back, the start seam back
            let seam0 = seam_curve(&s0)?;
            let seam1 = seam_curve(&s1)?;
            let mut top = high.copy();
            top.trim(k, &mut Extent1d::from_min_max(tx0, tx1), TrimMode::Notify)?;
            let top_now = top.natural_interval();
            let pc_top = iso_pcurve(
                u1,
                v0 + (top_now.min() - w.lo) * v_scale,
                u1,
                v0 + (top_now.max() - w.lo) * v_scale,
                top_now.min(),
                top_now.max(),
            )?;
            let pc_seam1 = match seam_pcurve(w, &s1, n, h, [u0, u1], w.lo, v0, v_scale, false) {
                Ok(pc) => pc,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Ok(refuse(TaperCornerRefusal::NotExact, s1.p)),
            };
            let pc_seam0 = match seam_pcurve(w, &s0, n, h, [u0, u1], w.lo, v0, v_scale, false) {
                Ok(pc) => pc,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Ok(refuse(TaperCornerRefusal::NotExact, s0.p)),
            };
            let mut curves: Vec<BSplineCurve> = Vec::new();
            let mut pcurves: Vec<BSplineCurve> = Vec::new();
            let mut orientations = Vec::new();
            if !w.point {
                let mut bottom = low.copy();
                bottom.trim(k, &mut Extent1d::from_min_max(tp0, tp1), TrimMode::Notify)?;
                let now = bottom.natural_interval();
                let pc_bottom = iso_pcurve(
                    u0,
                    v0 + (now.min() - w.lo) * v_scale,
                    u0,
                    v0 + (now.max() - w.lo) * v_scale,
                    now.min(),
                    now.max(),
                )?;
                curves.push(bottom);
                pcurves.push(pc_bottom);
                orientations.push(OrientType::Same);
            }
            let seam0_index = curves.len() + 2;
            curves.push(seam1);
            pcurves.push(pc_seam1);
            orientations.push(OrientType::Same);
            curves.push(top.copy());
            pcurves.push(pc_top);
            orientations.push(OrientType::Opposite);
            curves.push(seam0);
            pcurves.push(pc_seam0);
            orientations.push(OrientType::Opposite);
            // every pcurve must lift onto its curve
            for (e, (c, pc)) in curves.iter().zip(&pcurves).enumerate() {
                let ivl = c.natural_interval();
                for step in 0..=8 {
                    let t = ivl.evaluate(f64::from(step) / 8.0);
                    let uv = pc.evaluate_point(t)?;
                    let on = surface.evaluate_point(Point2::new(uv.x, uv.y))?;
                    let at_t = c.evaluate_point(t)?;
                    if on.distance(at_t) > tol {
                        let corner = if e == seam0_index { s0.p } else { s1.p };
                        return Ok(refuse(TaperCornerRefusal::NotExact, corner));
                    }
                }
            }
            // the far end's curve, for its cap
            far_curves.push(Curve::from(top));
            let face_curves = FaceCurvesInput {
                loops: vec![curves.len()],
                curves_3d: Some(curves.into_iter().map(|c| Some(Curve::from(c))).collect()),
                uv_curves: Some(pcurves.into_iter().map(Curve::from).collect()),
                orientations,
                loop_points: Vec::new(),
            };
            (face_curves, Surface::from(surface), domain)
        };
        // the loop runs clockwise in (u, v): opposite to the surface's normal
        let (curves, surface, domain) = face;
        a.face_with_curves(target, curves, surface, domain, OrientType::Opposite)?;
    }
    Ok(Ok(far_curves))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quartic_multiplies_bernstein_quadratics() {
        // (1 - t)^2 * t^2 = t^2 - 2t^3 + t^4: Bernstein coefficients [0, 0, 1/6, 0, 0]
        let c = quartic([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert_eq!(c, [0.0, 0.0, 1.0 / 6.0, 0.0, 0.0]);
        // 1 * 1 = 1
        assert_eq!(quartic([1.0; 3], [1.0; 3]), [1.0; 5]);
    }

    #[test]
    fn seam_point_interpolates_the_conic() {
        let seam = Seam {
            p: Point3::new(0.0, 0.0, 0.0),
            s: Point3::new(1.0, 1.0, 0.0),
            x: Point3::new(2.0, 0.0, 0.0),
            weight: 1.0,
            ..Seam::default()
        };
        assert_eq!(seam_point(&seam, 0.0), seam.p);
        assert_eq!(seam_point(&seam, 1.0), seam.x);
        assert_eq!(seam_point(&seam, 0.5), Point3::new(1.0, 0.5, 0.0));
    }

    #[test]
    fn two_lines_cross_at_their_offset_corner() {
        let n = Vec3::Z;
        let a = Wall {
            start: Point3::new(0.0, 0.0, 0.0),
            dir: Vec3::X,
            offset: Vec3::new(0.0, 1.0, 0.0),
            ..Wall::default()
        };
        let b = Wall {
            start: Point3::new(10.0, 0.0, 0.0),
            dir: Vec3::Y,
            offset: Vec3::new(-1.0, 0.0, 0.0),
            ..Wall::default()
        };
        let x = crossing(&a, &b, 1.0, n, b.start);
        assert_eq!(x, Some(Point3::new(9.0, 1.0, 0.0)));
        // parallel lines do not cross
        assert_eq!(crossing(&a, &a, 1.0, n, b.start), None);
    }
}
