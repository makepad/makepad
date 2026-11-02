//! Common NURBS surfaces: bilinear patch from corner points, extruded (generalized
//! cylinder), cylinder / cone, ruled and revolved surfaces.
//!
//! Upstream: `source/SMLib/src/NL_SrfCommon.cpp`, declarations in
//! `source/SMLib/inc/NL_SrfCommon.h`.
//!
//! | Rust                       | Upstream                 |
//! |----------------------------|--------------------------|
//! | [`corner_pts`]             | `N_CreateSrfCornerPts`   |
//! | [`extrude_crv`]            | `N_CreateSrfExtrudeCrv`  |
//! | [`cyl_cone`], [`CylCone`]  | `N_CreateCylCone`        |
//! | [`ruled_srf`]              | `N_CreateRuledSrf`       |
//! | [`revolved_srf`]           | `N_CreateRevolvedSrf`    |
//!
//! Deviations:
//! - The output surface is returned as a new [`Surface`] (upstream fills the caller's
//!   `NL_SURFACE`, allocating it when NULL, or reuses pre-allocated storage that is
//!   large enough). The `NL_STACKS` arguments have no counterpart.
//! - Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number
//!   instead of a flag plus the global `NL_ERROR` record. The `dir` flag is a
//!   [`Direction`]; [`Direction::UV`] takes upstream's `default:` branch
//!   (`NL_CAL_ERR`). The conic type is a [`ConicType`], so an unknown flag cannot be
//!   passed.
//! - The input curves are modified in place exactly as upstream does, so they are
//!   `&mut`: [`extrude_crv`] makes a 2D curve 3D, [`ruled_srf`] makes both curves
//!   compatible (also keeping the changes made before a failure), [`revolved_srf`]
//!   makes the profile rational and 3D.
//! - `N_CreateRevolvedSrf` stores the circle weights in `wi[9]`, which a quartic
//!   revolution of 241 degrees or more (13 weights) overflows (see
//!   `plan/findings.md`); [`revolved_srf`] keeps all weights.
//! - `NL_MTOL` is [`MODEL_TOL`].

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::crv_basic::{conic_data, ConicType, Curve};
use crate::nurbs::crv_common::{circ_arc, circ_weights, Arc};
use crate::nurbs::crv_tool::p2::make_compatible;
use crate::nurbs::geom::{normalize, project_pt_line, pt_along_vector, LinePlaneIsect, LineSeg};
use crate::nurbs::math::{CPoint, MODEL_TOL};
use crate::nurbs::srf_basic::{Direction, Surface};

const OP_CORNER: &str = "nurbs::srf_common::corner_pts";
const OP_EXTRUDE: &str = "nurbs::srf_common::extrude_crv";
const OP_CYLCONE: &str = "nurbs::srf_common::cyl_cone";
const OP_RULED: &str = "nurbs::srf_common::ruled_srf";
const OP_REVOLVED: &str = "nurbs::srf_common::revolved_srf";

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

fn get<T: Copy>(v: &[T], i: usize, op: &'static str) -> Result<T> {
    v.get(i)
        .copied()
        .ok_or_else(|| invalid(op, "index outside the curve arrays"))
}

/// Knots `0, 0, 1, 1` of a linear direction.
fn linear_knots() -> Vec<f64> {
    vec![0.0, 0.0, 1.0, 1.0]
}

/// A `2 x (k+1)` (`dir == U`) or `(k+1) x 2` (`dir == V`) net from the two boundary
/// rows `a` (`S(0, .)` / `S(., 0)`) and `b` (`S(1, .)` / `S(., 1)`).
fn two_row_net(a: Vec<CPoint>, b: Vec<CPoint>, linear_in_u: bool) -> Vec<Vec<CPoint>> {
    if linear_in_u {
        vec![a, b]
    } else {
        a.into_iter().zip(b).map(|(x, y)| vec![x, y]).collect()
    }
}

/// Bilinear non-rational surface through four corner points
/// (`N_CreateSrfCornerPts`): `S(0,0) = P00`, `S(1,0) = P10`, `S(0,1) = P01`,
/// `S(1,1) = P11`, knots `{0,0,1,1}` in both directions. Planar when the points are.
///
/// # Errors
///
/// None in practice (the net and knots are never empty).
pub fn corner_pts(p00: Vec3, p10: Vec3, p01: Vec3, p11: Vec3) -> Result<Surface> {
    // Compute control points.
    let net = vec![
        vec![CPoint::from_point(p00), CPoint::from_point(p01)],
        vec![CPoint::from_point(p10), CPoint::from_point(p11)],
    ];
    Surface::new(net, 1, 1, linear_knots(), linear_knots())
        .map_err(|_| invalid(OP_CORNER, "bad net"))
}

/// Generalized cylinder: `cur` extruded by distance `d` along the direction `w`
/// (`N_CreateSrfExtrudeCrv`). With [`Direction::U`] the surface is linear in u
/// (`S(0, v) = C(v)`), with [`Direction::V`] linear in v (`S(u, 0) = C(u)`); the
/// curve's knots become the other direction's knots.
///
/// A 2D `cur` is made 3D in place (`z = 0`) first, as upstream.
///
/// # Errors
///
/// [`NlibCode::Call`] for [`Direction::UV`] (before touching `cur`),
/// [`NlibCode::Geometry`] when `w` cannot be normalised.
pub fn extrude_crv(cur: &mut Curve, w: Vec3, d: f64, dir: Direction) -> Result<Surface> {
    // Get highest indexes.
    let linear_in_u = match dir {
        Direction::U => true,
        Direction::V => false,
        Direction::UV => return Err(nlib(OP_EXTRUDE, NlibCode::Call)),
    };

    // Prepare for extrusion.
    if !cur.is_3d() {
        cur.to_3d();
    }
    let (w, _) = normalize(w)?;

    // Compute control points.
    let rat = cur.is_rational();
    let base: Vec<CPoint> = cur.cpts().to_vec();
    let top: Vec<CPoint> = base
        .iter()
        .map(|&pw| {
            let (a, wt) = pw.to_euclid_and_w();
            let a = pt_along_vector(a, d, w);
            if rat {
                CPoint::weighted(a, wt)
            } else {
                CPoint::from_point(a)
            }
        })
        .collect();

    // Get the knots.
    let knots = cur.knots().to_vec();
    let p = cur.degree();
    let net = two_row_net(base, top, linear_in_u);
    let res = if linear_in_u {
        Surface::new(net, 1, p, linear_knots(), knots)
    } else {
        Surface::new(net, p, 1, knots, linear_knots())
    };
    res.map_err(|_| invalid(OP_EXTRUDE, "empty curve"))
}

/// A cylinder, cone or patch of one (the arguments of `N_CreateCylCone`): the base
/// circle has centre `center` in the plane of `x`, `y`, the top circle lies at height
/// `h` along `x × y`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CylCone {
    /// Centre `C` of the bottom circle.
    pub center: Vec3,
    /// X axis of the bottom plane (angle 0).
    pub x: Vec3,
    /// Y axis of the bottom plane (angle 90).
    pub y: Vec3,
    /// Bottom radius `rb` (at most [`MODEL_TOL`]: apex at the bottom).
    pub rb: f64,
    /// Top radius `rt` (at most [`MODEL_TOL`]: apex at the top).
    pub rt: f64,
    /// Start sweep angle `as` in degrees.
    pub start: f64,
    /// End sweep angle `ae` in degrees.
    pub end: f64,
    /// Height `h`.
    pub h: f64,
}

/// NURBS cylinder / cone (patch) by extruding a circular arc of type `ctp`
/// (`N_CreateCylCone`). With [`Direction::U`] the circle is a u-curve (`S(u, 0)`
/// bottom, `S(u, 1)` top), with [`Direction::V`] a v-curve. A radius at most
/// [`MODEL_TOL`] collapses that circle to its centre, keeping the other circle's
/// weights.
///
/// # Errors
///
/// [`NlibCode::Call`] when both radii are at most [`MODEL_TOL`] or for
/// [`Direction::UV`], [`NlibCode::Geometry`] when `x × y` cannot be normalised, and
/// the errors of [`circ_arc`].
pub fn cyl_cone(cc: &CylCone, ctp: ConicType, dir: Direction) -> Result<Surface> {
    let CylCone {
        center: c,
        x,
        y,
        rb,
        rt,
        start,
        end,
        h,
    } = *cc;

    // Get bottom and top circles.
    if rb <= MODEL_TOL && rt <= MODEL_TOL {
        return Err(nlib(OP_CYLCONE, NlibCode::Call));
    }
    let (z, _) = normalize(x.cross(y))?;
    let a = pt_along_vector(c, h, z);

    let arc = |center: Vec3, r: f64| Arc {
        center,
        x,
        y,
        r,
        start,
        end,
    };
    let cur_b = if rb > MODEL_TOL {
        Some(circ_arc(&arc(c, rb), ctp)?)
    } else {
        None
    };
    let cur_t = if rt > MODEL_TOL {
        Some(circ_arc(&arc(a, rt), ctp)?)
    } else {
        None
    };

    // Get local notations (the top circle's when there is one, as upstream).
    let Some(cur) = cur_t.as_ref().or(cur_b.as_ref()) else {
        return Err(nlib(OP_CYLCONE, NlibCode::Call));
    };
    let (n, p) = (cur.n(), cur.degree());
    let knots = cur.knots().to_vec();

    // Get highest indexes.
    let circle_in_u = match dir {
        Direction::U => true,
        Direction::V => false,
        Direction::UV => return Err(nlib(OP_CYLCONE, NlibCode::Call)),
    };

    let bw = cur_b.as_ref().map(Curve::cpts);
    let tw = cur_t.as_ref().map(Curve::cpts);
    let mut bottom = Vec::with_capacity(n + 1);
    let mut top = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let qb = match (bw, tw) {
            (Some(bw), _) => get(bw, i, OP_CYLCONE)?,
            (None, Some(tw)) => CPoint::weighted(c, get(tw, i, OP_CYLCONE)?.w),
            (None, None) => return Err(nlib(OP_CYLCONE, NlibCode::Call)),
        };
        let qt = match (tw, bw) {
            (Some(tw), _) => get(tw, i, OP_CYLCONE)?,
            (None, Some(bw)) => CPoint::weighted(a, get(bw, i, OP_CYLCONE)?.w),
            (None, None) => return Err(nlib(OP_CYLCONE, NlibCode::Call)),
        };
        bottom.push(qb);
        top.push(qt);
    }

    // Extrude the circle as a u- or v-curve; get the knots.
    let net = two_row_net(bottom, top, !circle_in_u);
    let res = if circle_in_u {
        Surface::new(net, p, 1, knots, linear_knots())
    } else {
        Surface::new(net, 1, p, linear_knots(), knots)
    };
    res.map_err(|_| invalid(OP_CYLCONE, "empty circle"))
}

/// Ruled surface between two curves (`N_CreateRuledSrf`): with [`Direction::U`] the
/// surface is linear in u (`S(0, v) = A(v)`, `S(1, v) = B(v)`), with
/// [`Direction::V`] linear in v. The curves are first made compatible in place
/// ([`make_compatible`]); a 2D curve's row gets `z = 0`.
///
/// # Errors
///
/// The errors of [`make_compatible`] (the curves keep the changes made before),
/// then [`NlibCode::Call`] for [`Direction::UV`].
pub fn ruled_srf(cur_a: &mut Curve, cur_b: &mut Curve, dir: Direction) -> Result<Surface> {
    // Make boundary curves compatible.
    let mut cur = [cur_a.clone(), cur_b.clone()];
    let res = make_compatible(&mut cur);
    let [ca, cb] = cur;
    *cur_a = ca;
    *cur_b = cb;
    res?;

    let aw = cur_a.cpts();
    let (p, knots) = (cur_a.degree(), cur_a.knots().to_vec());
    let bw = cur_b.cpts();
    let n = cur_b.n();

    // Get highest indexes.
    let linear_in_u = match dir {
        Direction::U => true,
        Direction::V => false,
        Direction::UV => return Err(nlib(OP_RULED, NlibCode::Call)),
    };

    // Get dimensionality of curves (surface should be 3D).
    let a3d = cur_a.is_3d();
    let b3d = cur_b.is_3d();

    // Compute control points.
    let mut rows_a = Vec::with_capacity(n + 1);
    let mut rows_b = Vec::with_capacity(n + 1);
    for j in 0..=n {
        let mut qa = get(aw, j, OP_RULED)?;
        let mut qb = get(bw, j, OP_RULED)?;
        if !a3d {
            qa.z = 0.0;
        }
        if !b3d {
            qb.z = 0.0;
        }
        rows_a.push(qa);
        rows_b.push(qb);
    }

    // Get the knots.
    let net = two_row_net(rows_a, rows_b, linear_in_u);
    let res = if linear_in_u {
        Surface::new(net, 1, p, linear_knots(), knots)
    } else {
        Surface::new(net, p, 1, knots, linear_knots())
    };
    res.map_err(|_| invalid(OP_RULED, "empty curve"))
}

/// Surface of revolution: the profile `cur` rotated by `al` degrees about the axis
/// through `s` with direction `t` (`N_CreateRevolvedSrf`). The rotation iso-curves are
/// circles of type `ctp` in u; the profile is the v direction. A profile point within
/// [`MODEL_TOL`] of the axis gives a row of coincident points with the circle weights.
///
/// The profile is made rational (`w = 1`) and 3D in place first, as upstream.
///
/// # Errors
///
/// `InvalidInput` for an angle [`conic_data`] rejects, [`NlibCode::Geometry`] when
/// `t` cannot be normalised, [`NlibCode::Input`] when a point cannot be projected onto
/// the axis, [`NlibCode::Call`] when every profile point lies on the axis, and the
/// errors of [`circ_weights`] / [`circ_arc`].
pub fn revolved_srf(cur: &mut Curve, s: Vec3, t: Vec3, al: f64, ctp: ConicType) -> Result<Surface> {
    // Get number of arcs.
    let cd = conic_data(ctp, 0.0, al)?;
    let (al, n, p) = (cd.ae, cd.n, cd.p);

    // Make profile curve 3-D rational.
    if !cur.is_rational() {
        cur.non_rat_to_rat();
    }
    if !cur.is_3d() {
        cur.to_3d();
    }
    let pw = cur.cpts();
    let m = cur.n();

    // Extract circle weights.
    let wi = circ_weights(0.0, al, ctp)?;

    // Compute surface control points.
    let lst = LineSeg::new(s, t, false);
    let mut allrzero = true;
    let mut net = vec![Vec::with_capacity(m + 1); n + 1];
    let mut circle_knots: Option<Vec<f64>> = None;
    for j in 0..=m {
        let pj = get(pw, j, OP_REVOLVED)?;
        let a = pj.to_euclid();
        let wp = pj.w;

        let LinePlaneIsect::Hit { p: c, .. } = project_pt_line(lst, a)? else {
            return Err(nlib(OP_REVOLVED, NlibCode::Input));
        };
        let x = a - c;
        let rad = x.length();

        if rad < MODEL_TOL {
            for (i, row) in net.iter_mut().enumerate() {
                row.push(CPoint::weighted(c, get(&wi, i, OP_REVOLVED)? * wp));
            }
            continue;
        }

        allrzero = false;
        let y = t.cross(x);
        let cur_a = circ_arc(
            &Arc {
                center: c,
                x,
                y,
                r: rad,
                start: 0.0,
                end: al,
            },
            ctp,
        )?;
        let aw = cur_a.cpts();
        for (i, row) in net.iter_mut().enumerate() {
            row.push(get(aw, i, OP_REVOLVED)?.scale(wp));
        }
        circle_knots = Some(cur_a.knots().to_vec());
    }

    if allrzero {
        return Err(nlib(OP_REVOLVED, NlibCode::Call));
    }

    // Get the knots.
    let u = circle_knots.ok_or_else(|| nlib(OP_REVOLVED, NlibCode::Call))?;
    let v = cur.knots().to_vec();
    let q = cur.degree();
    Surface::new(net, p, q, u, v).map_err(|_| invalid(OP_REVOLVED, "empty profile"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::{NOW, NOZ};

    fn line2d() -> Curve {
        Curve::new(
            vec![
                CPoint::new(1.0, 0.0, NOZ, NOW),
                CPoint::new(2.0, 1.0, NOZ, NOW),
            ],
            1,
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap()
    }

    #[test]
    fn corner_net_layout() {
        let s = corner_pts(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, 12.0, 0.0),
            Vec3::new(10.0, 12.0, 0.0),
        )
        .unwrap();
        assert_eq!(s.cpt(1, 0).unwrap(), CPoint::new(10.0, 0.0, 0.0, NOW));
        assert_eq!(s.cpt(0, 1).unwrap(), CPoint::new(0.0, 12.0, 0.0, NOW));
        assert_eq!(s.u_knots(), &[0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn extrude_makes_curve_3d() {
        let mut c = line2d();
        let s = extrude_crv(&mut c, Vec3::new(0.0, 0.0, 2.0), 3.0, Direction::V).unwrap();
        assert!(c.is_3d());
        assert_eq!(s.cpt(1, 1).unwrap(), CPoint::new(2.0, 1.0, 3.0, NOW));
        assert_eq!(s.degrees(), (1, 1));
        let mut c = line2d();
        assert!(extrude_crv(&mut c, Vec3::new(0.0, 0.0, 0.0), 1.0, Direction::U).is_err());
        let mut c = line2d();
        assert!(extrude_crv(&mut c, Vec3::new(0.0, 0.0, 1.0), 1.0, Direction::UV).is_err());
        assert!(!c.is_3d());
    }

    #[test]
    fn cylinder_and_cone() {
        let mut cc = CylCone {
            center: Vec3::new(0.0, 0.0, 0.0),
            x: Vec3::new(1.0, 0.0, 0.0),
            y: Vec3::new(0.0, 1.0, 0.0),
            rb: 1.0,
            rt: 1.0,
            start: 0.0,
            end: 360.0,
            h: 1.0,
        };
        let s = cyl_cone(&cc, ConicType::Quintic, Direction::U).unwrap();
        assert_eq!((s.n(), s.m()), (5, 1));
        cc.rt = 0.0;
        let s = cyl_cone(&cc, ConicType::Quadratic, Direction::V).unwrap();
        let top = s.cpt(1, 0).unwrap();
        assert_eq!((top.x, top.y), (0.0, 0.0));
        assert_eq!(top.z, top.w);
        cc.rb = 0.0;
        assert!(cyl_cone(&cc, ConicType::Quadratic, Direction::U).is_err());
    }

    #[test]
    fn ruled_and_revolved() {
        let mut a = line2d();
        let mut b = line2d();
        let s = ruled_srf(&mut a, &mut b, Direction::U).unwrap();
        assert_eq!(s.cpt(1, 1).unwrap().z, 0.0);

        let mut c = line2d();
        let s = revolved_srf(
            &mut c,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            90.0,
            ConicType::Quadratic,
        )
        .unwrap();
        assert!(c.is_rational() && c.is_3d());
        assert_eq!((s.n(), s.m(), s.p(), s.q()), (2, 1, 2, 1));

        let mut on_axis = Curve::new(
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(0.0, 1.0, 0.0, NOW),
            ],
            1,
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        let e = revolved_srf(
            &mut on_axis,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            90.0,
            ConicType::Quadratic,
        );
        assert!(matches!(
            e,
            Err(SolidError::Nlib {
                code: NlibCode::Call,
                ..
            })
        ));
    }
}
