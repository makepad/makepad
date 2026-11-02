//! NURBS curve geometry processing: point projection / inversion by Newton iteration,
//! arc length of a parameter interval, and polyline approximation to a chord-height
//! tolerance.
//!
//! Upstream: `source/SMLib/src/NL_CrvGeom.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvGeom.h`).
//!
//! | Rust                                         | Upstream                          |
//! |----------------------------------------------|-----------------------------------|
//! | [`Curve::closest_pt`]                        | `N_CrvClosestPt`                  |
//! | [`Curve::closest_pt_search`], [`ClosestPtSearch`] | `N_CrvClosestPt` (best values on failure, explicit `NL_ITLIM`) |
//! | [`Curve::arc_length`]                        | `N_CrvArcLength`                  |
//! | [`Curve::approx_with_polyline`], [`Polyline`] | `N_ApproxCrvWithPolyline`        |
//! | [`ToleranceKind`]                            | `NL_ABSOLUTE` / `NL_RELATIVE`     |
//! | [`ITER_LIMIT`]                               | `NL_ITLIM` (default value)        |
//! | `ARC_TYPE_TOL` (private)                     | `ctl` of `N_CrvArcLength`         |
//!
//! Deviations:
//! - Out-params become return values; errors are [`SolidError::Nlib`] values with the
//!   upstream `NL_*_ERR` number (or the error of the called routine) instead of a flag
//!   plus the global `NL_ERROR` record. The `NL_STACKS` arguments and the memory error
//!   paths have no counterpart.
//! - `N_CrvClosestPt` returns the best parameter and point found even when it fails.
//!   [`Curve::closest_pt_search`] returns both (the best values, `None` when upstream
//!   never writes them, and the error); [`Curve::closest_pt`] is the plain form. The
//!   global iteration limit `NL_ITLIM` (which `N_CrvApprox*` lowers temporarily) is an
//!   argument of [`Curve::closest_pt_search`]; [`Curve::closest_pt`] uses its default
//!   [`ITER_LIMIT`].
//! - The tolerance flag of `N_CrvArcLength` (anything but `NL_ABSOLUTE` /
//!   `NL_RELATIVE` fails with `NL_CAL_ERR`) and of `N_ApproxCrvWithPolyline` (anything
//!   but `NL_RELATIVE` is absolute) is a [`ToleranceKind`], which has no other values.
//! - `N_CrvArcLength` fails with its bare error flag (no `NL_ERROR` number) when a
//!   decomposition step finds an interior knot of multiplicity above the degree; the
//!   port returns `InvalidInput` there (unreachable for well-formed curves).
//! - `N_ApproxCrvWithPolyline` returns the points, the parameters or both (`rflg`);
//!   both are computed the same way either way, so [`Polyline`] always carries both.
//!   Upstream subdivides forever when a segment never meets the tolerance (a
//!   tolerance below the floating-point resolution of the curve, e.g. `0`); the port
//!   gives up with [`SolidError::NotConverged`] after [`POLYLINE_MAX_SPLITS`] splits.

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_basic::PTOL;
use crate::nurbs::bezier::calc_length;
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::crv_nurb::CurveType;
use crate::nurbs::geom::{dist_pt_pt, dist_sq_pt_pt, normalize, pt_along_vector, PI};
use crate::nurbs::index::{get, oob, set, sx};
use crate::nurbs::knots::Side;
use crate::nurbs::math::{float_op_is_bad, CPoint, FloatOp};

/// Upstream `NL_ITLIM` default (`50`): iteration limit of [`Curve::closest_pt`].
pub const ITER_LIMIT: usize = 50;

/// Most segment splits [`Curve::approx_with_polyline`] performs before giving up
/// (upstream has no limit and loops forever).
pub const POLYLINE_MAX_SPLITS: usize = 1 << 22;

/// Upstream `ctl` of `N_CrvArcLength`: relative tolerance of the line / circle test.
const ARC_TYPE_TOL: f64 = 1.0e-09;

/// How a tolerance argument is interpreted (`NL_ABSOLUTE = 1`, `NL_RELATIVE = 2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToleranceKind {
    /// `NL_ABSOLUTE`: the tolerance is used as is.
    Absolute,
    /// `NL_RELATIVE`: the tolerance is scaled by a size of the curve.
    Relative,
}

impl ToleranceKind {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            ToleranceKind::Absolute => 1,
            ToleranceKind::Relative => 2,
        }
    }

    /// The kind for an upstream flag value, `None` for any other value.
    #[must_use]
    pub const fn from_flag(flag: i32) -> Option<Self> {
        match flag {
            1 => Some(ToleranceKind::Absolute),
            2 => Some(ToleranceKind::Relative),
            _ => None,
        }
    }
}

/// Outcome of [`Curve::closest_pt_search`]: upstream's `u`, `Q` and return flag.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosestPtSearch {
    /// Best parameter and curve point found (`None` when upstream never writes its
    /// outputs, i.e. it failed before evaluating any finite distance).
    pub best: Option<(f64, Vec3)>,
    /// The error upstream returns (`NL_CON_ERR` on non-convergence, or the error of
    /// `N_CrvDerivs`), `None` on success.
    pub error: Option<SolidError>,
}

/// Output of [`Curve::approx_with_polyline`]: points on the curve and their
/// parameters (upstream `pts` and `pars`, `n + 1` entries each).
#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    /// Polyline vertices (on the curve).
    pub points: Vec<Vec3>,
    /// Curve parameters of [`Polyline::points`].
    pub params: Vec<f64>,
}

/// One entry of the subdivision stack of [`Curve::approx_with_polyline`]: Bezier
/// control points and their parameter interval.
struct StackSeg {
    cpts: Vec<CPoint>,
    range: [f64; 2],
}

impl Curve {
    /// Point `Q = C(u)` closest to `p` by Newton iteration from `u0`, with point
    /// coincidence tolerance `top` and zero-cosine tolerance `toc` (`N_CrvClosestPt`).
    ///
    /// Returns `(u, Q)`. Errors with `NL_CON_ERR` when the iteration does not converge
    /// within [`ITER_LIMIT`] steps and with the errors of [`Curve::derivs`]; use
    /// [`Curve::closest_pt_search`] for the best values found in that case.
    pub fn closest_pt(&self, p: Vec3, u0: f64, top: f64, toc: f64) -> Result<(f64, Vec3)> {
        let s = self.closest_pt_search(p, u0, top, toc, ITER_LIMIT)?;
        if let Some(e) = s.error {
            return Err(e);
        }
        s.best.ok_or_else(|| {
            SolidError::nlib("nurbs::crv_geom::Curve::closest_pt", NlibCode::Convergence)
        })
    }

    /// [`Curve::closest_pt`] with the iteration limit `itlim` (upstream `NL_ITLIM`),
    /// returning upstream's best values together with its error.
    ///
    /// The outer `Err` is only for malformed curves (array access out of range).
    pub fn closest_pt_search(
        &self,
        p: Vec3,
        u0: f64,
        top: f64,
        toc: f64,
        itlim: usize,
    ) -> Result<ClosestPtSearch> {
        const OP: &str = "nurbs::crv_geom::Curve::closest_pt_search";
        let uk = self.knots();
        let m = sx(self.m(), OP)?;
        let u_first = get(uk, 0, OP)?;
        let u_last = get(uk, m, OP)?;

        let closed = self.is_closed();

        let mut best: Option<(f64, Vec3)> = None;
        let fail = |best: Option<(f64, Vec3)>, e: SolidError| {
            Ok(ClosestPtSearch {
                best,
                error: Some(e),
            })
        };

        let mut k = 0usize;
        let mut unew = u0;
        let mut best_dis = f64::MAX;
        let mut ucount = 0usize;

        while k < itlim {
            // Get derivatives.
            let d = match self.derivs(unew, Side::Left, 2) {
                Ok(d) => d,
                Err(e) => return fail(best, e),
            };
            let d0 = get(&d, 0, OP)?;
            let d1 = get(&d, 1, OP)?;
            let d2 = get(&d, 2, OP)?;

            // Prepare for convergence test.
            let v = d0 - p;
            let num = d1.dot(v);
            let der = d1.length();

            // Check #1: point coincidence.
            let dis = v.length();

            if dis < best_dis {
                best = Some((unew, d0));
                best_dis = dis;
            }

            if dis <= top {
                break;
            }

            // Check #2: zero cosine.
            if float_op_is_bad(num, der * dis, FloatOp::Division) {
                return fail(best, SolidError::nlib(OP, NlibCode::Convergence));
            }

            let zco = num.abs() / (der * dis);

            if zco <= toc && dis == best_dis {
                break;
            }

            // No convergence, compute new parameter.
            let dot = d2.dot(v);
            let den = dot + der * der;

            if float_op_is_bad(num, den, FloatOp::Division) {
                return fail(best, SolidError::nlib(OP, NlibCode::Convergence));
            }

            let uold = unew;
            unew = uold - num / den;

            // Check #3: parameter range.
            if closed {
                if unew < u_first {
                    if (u_first - unew) < (100.0 * (u_last - u_first)) {
                        while unew < u_first {
                            unew += u_last - u_first;
                        }
                        ucount = 0;
                    } else {
                        if ucount >= 2 {
                            return fail(best, SolidError::nlib(OP, NlibCode::Convergence));
                        }
                        unew = uold;
                        ucount += 1;
                    }
                } else if unew > u_last {
                    if (unew - u_last) < (100.0 * (u_last - u_first)) {
                        while unew > u_last {
                            unew += u_first - u_last;
                        }
                        ucount = 0;
                    } else {
                        if ucount >= 2 {
                            return fail(best, SolidError::nlib(OP, NlibCode::Convergence));
                        }
                        unew = uold;
                        ucount += 1;
                    }
                } else {
                    ucount = 0;
                }
            } else {
                if unew < u_first {
                    unew = u_first;
                }

                if unew > u_last {
                    unew = u_last;
                }
            }

            // Check #4: parameter change.
            if ((unew - uold) * der).abs() <= top && dis == best_dis {
                break;
            }

            k += 1;
        }

        // If no convergence, error out.
        if k >= itlim {
            return fail(best, SolidError::nlib(OP, NlibCode::Convergence));
        }

        Ok(ClosestPtSearch { best, error: None })
    }

    /// Arc length of the curve between the parameters `ul` and `ur` to within `tol`
    /// (`N_CrvArcLength`); a [`ToleranceKind::Relative`] tolerance is scaled by the
    /// bounding-box diagonal of the segment.
    ///
    /// Lines and circular arcs (recognised by [`Curve::get_type`]) are measured
    /// exactly; other curves are decomposed into Bezier segments on the fly, each
    /// measured by [`calc_length`] with an equal share of the tolerance.
    ///
    /// Errors with the errors of [`Curve::extract_seg`], [`Curve::bbox_max_diag_dist`],
    /// [`Curve::get_type`] and [`calc_length`].
    pub fn arc_length(&self, ul: f64, ur: f64, tol: f64, kind: ToleranceKind) -> Result<f64> {
        const OP: &str = "nurbs::crv_geom::Curve::arc_length";

        // Take care of parameter degeneracy.
        if (ur - ul).abs() < PTOL {
            return Ok(0.0);
        }

        // Extract segment.
        let cur_a = self.extract_seg(ul, ur)?;

        let d = cur_a.bbox_max_diag_dist()?;

        let lctl = d * ARC_TYPE_TOL;

        // Get line and circle.
        match cur_a.get_type(lctl)? {
            CurveType::Line { start, dir } => {
                let end = start + dir;
                return Ok(dist_pt_pt(start, end));
            }
            CurveType::Circle { radius, angle, .. } => {
                return Ok((angle / 180.0) * PI * radius);
            }
            CurveType::FreeForm => {}
        }

        // Adjust tolerance.
        let ltol = match kind {
            ToleranceKind::Relative => d * tol,
            ToleranceKind::Absolute => tol,
        };

        // Get local notation and prepare for decomposition.
        let pw = cur_a.cpts();
        let uk = cur_a.knots();
        let pu = cur_a.degree();
        let p = sx(pu, OP)?;
        let m = sx(cur_a.m(), OP)?;
        let nsp = cur_a.knot_vector()?.span_count(pu);

        let eps = ltol / nsp as f64;
        let mut len = 0.0;

        // Get local memory.
        let size = pu.checked_add(1).ok_or_else(|| oob(OP))?;
        let mut bw = vec![CPoint::ZERO; size];
        let mut nw = vec![CPoint::ZERO; size];
        let mut alfs = vec![0.0; size];
        let mut omas = vec![0.0; size];

        // Initialize.
        let mut a = p;
        let mut b = p + 1;

        for i in 0..=p {
            set(&mut bw, i, get(pw, i, OP)?, OP)?;
        }

        // Decompose curve and compute length of each piece.
        while b < m {
            let i0 = b;

            while b < m && get(uk, b, OP)? == get(uk, b + 1, OP)? {
                b += 1;
            }

            let mlt = b - i0 + 1;
            let r = p - mlt;

            if r > 0 {
                let num = get(uk, b, OP)? - get(uk, a, OP)?;

                let mut k = p;
                while k > mlt {
                    let alf = num / (get(uk, a + k, OP)? - get(uk, a, OP)?);
                    set(&mut alfs, k - mlt - 1, alf, OP)?;
                    set(&mut omas, k - mlt - 1, 1.0 - alf, OP)?;
                    k -= 1;
                }

                for j in 1..=r {
                    let save = r - j;
                    let s = mlt + j;

                    let mut k = p;
                    while k >= s {
                        let c = CPoint::combine2(
                            get(&alfs, k - s, OP)?,
                            get(&bw, k, OP)?,
                            get(&omas, k - s, OP)?,
                            get(&bw, k - 1, OP)?,
                        );
                        set(&mut bw, k, c, OP)?;
                        k -= 1;
                    }
                    set(&mut nw, save, get(&bw, p, OP)?, OP)?;
                }
            }

            // Compute length of Bezier segment and update total length.
            let lbz = calc_length(&bw, pu, eps)?;

            len += lbz;

            // Initialize for next pass through.
            if b < m {
                if r < 0 {
                    // Upstream sets its error flag without an NL_ERROR number.
                    return Err(SolidError::InvalidInput {
                        operation: OP,
                        reason: "interior knot multiplicity above the degree",
                    });
                }
                for i in 0..r {
                    set(&mut bw, i, get(&nw, i, OP)?, OP)?;
                }

                for i in r..=p {
                    set(&mut bw, i, get(pw, b - p + i, OP)?, OP)?;
                }

                a = b;
                b += 1;
            }
        }

        Ok(len)
    }

    /// Polyline through points on the curve whose chords deviate from the curve by at
    /// most `tol`, with a vertex at every distinct knot (`N_ApproxCrvWithPolyline`);
    /// a [`ToleranceKind::Relative`] tolerance is scaled by the arc length of the
    /// curve.
    ///
    /// Degree-1 curves return their control points and the knots `U[1..=n+1]`.
    /// Otherwise the Bezier segments of the curve are halved until their control
    /// points lie within `tol` of the chord; vertices closer than `tol / 100` to the
    /// previous one are dropped (the last one replaces its predecessor).
    ///
    /// Errors with the errors of [`Curve::decompose_bez`] and [`calc_length`], and
    /// with [`SolidError::NotConverged`] after [`POLYLINE_MAX_SPLITS`] splits.
    pub fn approx_with_polyline(&self, tol: f64, kind: ToleranceKind) -> Result<Polyline> {
        const OP: &str = "nurbs::crv_geom::Curve::approx_with_polyline";
        let pu = self.degree();

        // Handle p = 1 as special case.
        if pu == 1 {
            let pw = self.cpts();
            let uk = self.knots();
            let nn = sx(self.n(), OP)?;
            let mut points = Vec::new();
            let mut params = Vec::new();
            for ii in 0..=nn {
                points.push(get(pw, ii, OP)?.to_euclid());
                params.push(get(uk, ii + 1, OP)?);
            }
            return Ok(Polyline { points, params });
        }

        // Decompose curve into Bezier segments.
        let bezs = self.decompose_bez()?;
        let mut tol = tol;

        // Adjust tolerance if necessary.
        if kind == ToleranceKind::Relative {
            let mut length = 0.0;

            for bez in &bezs {
                let dd = calc_length(bez.cpts(), bez.degree(), 0.01)?;
                length += dd;
            }

            tol *= length;
        }

        let p = sx(pu, OP)?;

        // The stack of Bezier segments (upstream's parallel `cps` / `pms` stacks);
        // the last entry is the top.
        let mut stack: Vec<StackSeg> = Vec::new();

        for bez in bezs.iter().rev() {
            let uk = bez.knots();
            let kk = sx(bez.m(), OP)?;
            let range = [get(uk, 0, OP)?, get(uk, kk, OP)?];

            let pw = bez.cpts();
            let mut cpts = Vec::new();
            for jj in 0..=p {
                cpts.push(get(pw, jj, OP)?);
            }
            stack.push(StackSeg { cpts, range });
        }

        // Initialize the points and parameters.
        let first_seg = stack.last().ok_or_else(|| oob(OP))?;
        let first = get(&first_seg.cpts, 0, OP)?.to_euclid();
        let mut points = vec![first];
        let mut params = vec![first_seg.range[0]];
        let mut last_pt = first;

        // Now subdivide the segments until the stack is empty.
        let tolsq = tol * tol;
        let mut splits = 0usize;

        loop {
            // Upstream `top EQ 0`.
            let top_is_bottom = stack.len() == 1;
            let Some(seg) = stack.last_mut() else {
                break;
            };
            let row = &mut seg.cpts;

            // Check if this segment has converged.
            let p1 = get(row, 0, OP)?.to_euclid();
            let p2 = get(row, p, OP)?.to_euclid();
            let v12 = p2 - p1;

            let converged = if let Ok((v12, _)) = normalize(v12) {
                let mut ok = true;
                for ii in 1..p {
                    let pp = get(row, ii, OP)?.to_euclid();
                    let v1p = pp - p1;
                    let dot = v12.dot(v1p);
                    let qq = pt_along_vector(p1, dot, v12);
                    let dd = dist_sq_pt_pt(pp, qq);

                    if dd > tolsq {
                        ok = false;
                        break;
                    }
                }
                ok
            } else {
                // Segment is either closed or degenerate.
                let pp = get(row, (p + 1) / 2, OP)?.to_euclid();
                let dd = dist_sq_pt_pt(pp, p1);
                dd < tolsq
            };

            if converged {
                // Load point and parameter.
                let mut dd = dist_pt_pt(p2, last_pt);

                if top_is_bottom {
                    if points.len() == 1 {
                        dd = tol;
                    } else if dd <= 0.01 * tol {
                        points.pop();
                        params.pop();
                        dd = tol;
                    }
                }

                let pm = seg.range;
                stack.pop();

                if dd > 0.01 * tol {
                    points.push(p2);
                    params.push(pm[1]);
                    last_pt = p2;
                }
            } else {
                // Must split segment.
                splits += 1;
                if splits > POLYLINE_MAX_SPLITS {
                    return Err(SolidError::NotConverged {
                        operation: OP,
                        iterations: u32::try_from(POLYLINE_MAX_SPLITS).unwrap_or(u32::MAX),
                    });
                }

                let mut left = vec![CPoint::ZERO; row.len()];
                set(&mut left, 0, get(row, 0, OP)?, OP)?;

                for kk in 1..=p {
                    for ii in 0..=p - kk {
                        let c =
                            CPoint::combine2(0.5, get(row, ii, OP)?, 0.5, get(row, ii + 1, OP)?);
                        set(row, ii, c, OP)?;
                    }
                    set(&mut left, kk, get(row, 0, OP)?, OP)?;
                }

                let pm = &mut seg.range;
                let lo = pm[0];
                pm[0] = 0.5 * (pm[0] + pm[1]);
                let range = [lo, pm[0]];

                stack.push(StackSeg { cpts: left, range });
            }
        }

        Ok(Polyline { points, params })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;

    fn cp(x: f64, y: f64, z: f64, w: f64) -> CPoint {
        CPoint::new(x, y, z, w)
    }

    fn quarter() -> Curve {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        Curve::new(
            vec![
                cp(1.0, 0.0, 0.0, 1.0),
                cp(s, s, 0.0, s),
                cp(0.0, 1.0, 0.0, 1.0),
            ],
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .unwrap()
    }

    fn cubic() -> Curve {
        Curve::new(
            vec![
                cp(0.0, 0.0, 0.0, NOW),
                cp(1.0, 2.0, 0.0, NOW),
                cp(2.0, -1.0, 1.0, NOW),
                cp(3.0, 1.0, 0.0, NOW),
                cp(4.0, 0.0, 2.0, NOW),
            ],
            3,
            vec![0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0],
        )
        .unwrap()
    }

    #[test]
    fn tolerance_kind_flags_round_trip() {
        for k in [ToleranceKind::Absolute, ToleranceKind::Relative] {
            assert_eq!(ToleranceKind::from_flag(k.flag()), Some(k));
        }
        assert_eq!(ToleranceKind::from_flag(3), None);
    }

    #[test]
    fn closest_point_on_quarter_circle() {
        let c = quarter();
        let p = Vec3::new(2.0, 2.0, 0.0);
        let (u, q) = c.closest_pt(p, 0.2, 1e-10, 1e-10).unwrap();
        let s = std::f64::consts::FRAC_1_SQRT_2;
        assert!((q.x - s).abs() < 1e-9 && (q.y - s).abs() < 1e-9);
        assert!((u - 0.5).abs() < 1e-9);
    }

    #[test]
    fn closest_point_clamps_and_reports_best() {
        let c = cubic();
        let s = c
            .closest_pt_search(Vec3::new(-5.0, 0.0, 0.0), 0.5, 1e-12, 1e-12, ITER_LIMIT)
            .unwrap();
        let (u, _) = s.best.unwrap();
        assert_eq!(u, 0.0);
        let s = c
            .closest_pt_search(Vec3::new(2.0, 5.0, 0.0), 0.5, 1e-14, 1e-14, 0)
            .unwrap();
        assert!(s.best.is_none());
        assert!(matches!(
            s.error,
            Some(SolidError::Nlib {
                code: NlibCode::Convergence,
                ..
            })
        ));
        assert!(c
            .closest_pt(Vec3::ZERO, 2.0, 1e-9, 1e-9)
            .is_err_and(|e| matches!(
                e,
                SolidError::Nlib {
                    code: NlibCode::Parameter,
                    ..
                }
            )));
    }

    #[test]
    fn arc_length_of_circle_line_and_freeform() {
        let c = quarter();
        let l = c
            .arc_length(0.0, 1.0, 1e-9, ToleranceKind::Absolute)
            .unwrap();
        assert!((l - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        assert_eq!(
            c.arc_length(0.3, 0.3, 1e-9, ToleranceKind::Absolute),
            Ok(0.0)
        );
        let line = Curve::new(
            vec![cp(0.0, 0.0, 0.0, NOW), cp(3.0, 4.0, 0.0, NOW)],
            1,
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        assert_eq!(
            line.arc_length(0.0, 1.0, 1e-6, ToleranceKind::Relative),
            Ok(5.0)
        );
        let k = cubic();
        let a = k
            .arc_length(0.0, 1.0, 1e-8, ToleranceKind::Relative)
            .unwrap();
        let b = k
            .arc_length(0.0, 1.0, 1e-10, ToleranceKind::Absolute)
            .unwrap();
        assert!((a - b).abs() < 1e-6 && a > 4.0);
        assert!(k
            .arc_length(0.6, 0.2, 1e-8, ToleranceKind::Absolute)
            .is_err());
    }

    #[test]
    fn polyline_vertices_lie_on_curve() {
        let k = cubic();
        let pl = k
            .approx_with_polyline(1e-3, ToleranceKind::Absolute)
            .unwrap();
        assert_eq!(pl.points.len(), pl.params.len());
        assert!(pl.points.len() > 4);
        assert_eq!(pl.params.first(), Some(&0.0));
        assert_eq!(pl.params.last(), Some(&1.0));
        assert!(pl.params.contains(&0.5));
        for (p, &u) in pl.points.iter().zip(&pl.params) {
            let q = k.eval(u, Side::Left).unwrap();
            assert!(dist_pt_pt(*p, q) < 1e-12);
        }
        let rel = k
            .approx_with_polyline(1e-4, ToleranceKind::Relative)
            .unwrap();
        assert!(rel.points.len() >= pl.points.len());
    }

    #[test]
    fn polyline_of_degree_one_is_its_polygon() {
        let c = Curve::new(
            vec![
                cp(0.0, 0.0, 0.0, NOW),
                cp(1.0, 0.0, 0.0, NOW),
                cp(1.0, 1.0, 0.0, NOW),
            ],
            1,
            vec![0.0, 0.0, 0.5, 1.0, 1.0],
        )
        .unwrap();
        let pl = c
            .approx_with_polyline(0.1, ToleranceKind::Absolute)
            .unwrap();
        assert_eq!(pl.params, vec![0.0, 0.5, 1.0]);
        assert_eq!(pl.points.len(), 3);
    }

    #[test]
    fn polyline_zero_tolerance_gives_up() {
        let k = quarter();
        assert!(matches!(
            k.approx_with_polyline(0.0, ToleranceKind::Absolute),
            Err(SolidError::NotConverged { .. })
        ));
    }
}
