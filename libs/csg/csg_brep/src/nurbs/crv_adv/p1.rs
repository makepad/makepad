//! Advanced NURBS curve routines, part 1: homogeneous points and derivatives at a
//! parameter, evenly spaced points, an upper bound on the second derivative, planar
//! offset derivatives, and the two curve extension algorithms (by arc length and by
//! parameter distance) behind `N_CrvExtendByDist` / `N_CrvExtendByParamDist`.
//!
//! Upstream: `source/SMLib/src/NL_CrvAdv.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvAdv.h`).
//!
//! | Rust                                     | Upstream                       |
//! |------------------------------------------|--------------------------------|
//! | [`Curve::eval_pt`]                       | `N_CrvEvalPt`                  |
//! | [`Curve::derivs_at_knot`]                | `N_CrvDerivsAtKnot`            |
//! | [`Curve::eval_even_spaced_pts`], [`EvenSpacedPts`] | `N_CrvEvalEvenSpacedPts` |
//! | [`Curve::max_2nd_deriv`]                 | `N_CrvGetMax2ndDeriv`          |
//! | [`Curve::planar_offset_derivs`]          | `N_CrvPlanarOffsetGetDeriv`    |
//! | [`Curve::extend_by_dist_at_depth`]       | `ST_curxcdr` (file-private)    |
//! | [`Curve::extend_by_param_dist_at_depth`] | `ST_curxcrr` (file-private)    |
//! | [`CurveEnd`]                             | `NL_START` / `NL_END`          |
//! | [`Continuity`]                           | `NL_G1` / `NL_G2` / `NL_CMAX`  |
//! | [`KNOT_TOL`]                             | `KN_TOL` (file-private)        |
//! | [`MAX_DIST_DEPTH`], [`MAX_PARAM_DEPTH`]  | recursion limits of `ST_curxcdr` / `ST_curxcrr` |
//!
//! Deviations:
//! - Out-params become return values; errors are [`SolidError::Nlib`] values with the
//!   upstream `NL_*_ERR` number instead of a flag plus the global `NL_ERROR` record.
//!   The `NL_STACKS` arguments and the memory error paths have no counterpart.
//! - `N_CrvEvalEvenSpacedPts` writes the points and/or the parameters when the caller
//!   passes their arrays; [`EvenSpacedPts`] always carries both.
//! - `N_CrvPlanarOffsetGetDeriv` writes `OD[1]` and `OD[2]` of a caller array;
//!   [`Curve::planar_offset_derivs`] returns the pair.
//! - The extension routines write into an output curve that is NULL, an existing
//!   curve (`N_CrvSizeArrays`) or the input itself (in place). The Rust methods take
//!   `&self` and return a new [`Curve`] sized exactly as upstream sizes a NULL output
//!   (after the in-place knot removals of the `NL_CMAX` branch the storage keeps that
//!   size and the indices shrink, as upstream), so the storage error path does not
//!   exist. In-place and out-of-place upstream calls produce the same curve.
//! - The recursion limits of `ST_curxcdr` (depth above 50) and `ST_curxcrr` (above 10)
//!   return a bare error flag (no `NL_ERROR` number); the port returns
//!   [`SolidError::NotConverged`] with `iterations` = the depth. The bare `NL_QUIT` of
//!   `ST_curxcrr` when the knot-inserted curve does not have the expected size
//!   (unreachable for clamped curves with interior multiplicities up to the degree)
//!   is `InvalidInput` with reason [`BARE_FLAG_REASON`].
//! - `ST_curxcdr` with `NL_G2` searches for a parameter step by step and loops forever
//!   when the step no longer changes the parameter (an extension distance far below
//!   the curve's resolution, where every round repeats the last one); the port gives
//!   up with [`SolidError::NotConverged`] there (`iterations` = the steps taken). The
//!   search takes about `arc length / extension` steps, so a vanishing extension
//!   (e.g. `1e-300` of the curve length) runs practically forever upstream; the port
//!   also gives up, the same way, after [`MAX_G2_STEPS`] steps.
//! - `N_CptReflect` fails only for a plane normal that cannot be normalised, which the
//!   extension routines never pass (they normalise it first); upstream ignores its
//!   flag, the port propagates it.
//! - The continuity flag (`NL_G1` / `NL_G2` / `NL_CMAX`, any other value leaves the
//!   output uninitialised upstream), the end flag (anything but `NL_START` means the
//!   end) and the distance flag (anything but `NL_RELATIVE` is absolute) are the enums
//!   [`Continuity`], [`CurveEnd`] and [`ToleranceKind`].
//! - Every array access is bounds checked: where upstream would read or write outside
//!   its arrays (malformed curves, too few control points for the tangent fallbacks of
//!   the extension routines, extensions that do not extend), the routines return
//!   [`SolidError::InvalidInput`] instead. For every input upstream handles without
//!   undefined behaviour the results are the same, bit for bit.
//! - A degree above [`MAX_SHORT_DEGREE`](crate::nurbs::bezier::MAX_SHORT_DEGREE)
//!   (impossible upstream, where `NL_DEGREE` is a `short`) is
//!   [`SolidError::InvalidInput`], so index arithmetic and allocations sized from the
//!   degree cannot overflow.

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_basic::{basis_derivs, PTOL};
use crate::nurbs::bezier::{binomial, check_short_degree};
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::crv_geom::{ToleranceKind, ITER_LIMIT};
use crate::nurbs::geom::{cpt_reflect, dist_pt_pt, normalize, Plane};
use crate::nurbs::index::{get, oob, set, storage, sx, ux};
use crate::nurbs::knots::Side;
use crate::nurbs::math::{blend_pt, float_op_is_bad, CPoint, FloatOp, MODEL_TOL, NOW, NOZ};
use crate::nurbs::srf_basic::{WEIGHT_MAX, WEIGHT_MIN};

/// Upstream `KN_TOL`: smallest knot distance the extension routines create, relative
/// to the curve's parameter range.
pub const KNOT_TOL: f64 = 1.0e-6;

/// Deepest recursion of [`Curve::extend_by_dist_at_depth`] (upstream fails above it).
pub const MAX_DIST_DEPTH: usize = 50;

/// Deepest recursion of [`Curve::extend_by_param_dist_at_depth`] (upstream fails above
/// it).
pub const MAX_PARAM_DEPTH: usize = 10;

/// Most steps of the `NL_G2` parameter search of [`Curve::extend_by_dist_at_depth`]
/// (upstream takes about `arc length / extension` steps, unbounded).
pub const MAX_G2_STEPS: usize = 100_000;

/// `reason` of the `InvalidInput` error that stands for upstream's bare error flag
/// (`NL_QUIT` without an `NL_ERROR` number).
pub const BARE_FLAG_REASON: &str = "upstream bare error flag";

const OP_EVAL_PT: &str = "nurbs::crv_adv::p1::Curve::eval_pt";
const OP_DERIVS_AT_KNOT: &str = "nurbs::crv_adv::p1::Curve::derivs_at_knot";
const OP_EVEN: &str = "nurbs::crv_adv::p1::Curve::eval_even_spaced_pts";
const OP_MAX2: &str = "nurbs::crv_adv::p1::Curve::max_2nd_deriv";
const OP_OFFSET: &str = "nurbs::crv_adv::p1::Curve::planar_offset_derivs";
const OP_XCDR: &str = "nurbs::crv_adv::p1::Curve::extend_by_dist_at_depth";
const OP_XCRR: &str = "nurbs::crv_adv::p1::Curve::extend_by_param_dist_at_depth";

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

fn depth_limit(operation: &'static str, depth: usize) -> SolidError {
    SolidError::NotConverged {
        operation,
        iterations: u32::try_from(depth).unwrap_or(u32::MAX),
    }
}

/// Which end of a curve an extension grows (`NL_START = 1`, `NL_END = 2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveEnd {
    /// `NL_START`: extend back from the start point.
    Start,
    /// `NL_END`: extend forward from the end point.
    End,
}

impl CurveEnd {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            CurveEnd::Start => 1,
            CurveEnd::End => 2,
        }
    }

    /// The end for an upstream flag value, `None` for any other value.
    #[must_use]
    pub const fn from_flag(flag: i32) -> Option<Self> {
        match flag {
            1 => Some(CurveEnd::Start),
            2 => Some(CurveEnd::End),
            _ => None,
        }
    }
}

/// Continuity of an extension at the old end point (`NL_G1 = 2`, `NL_G2 = 3`,
/// `NL_CMAX = 4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Continuity {
    /// `NL_G1`: a linear segment (at least G1, possibly C1).
    G1,
    /// `NL_G2`: the end segment reflected in the end-tangent plane (G2).
    G2,
    /// `NL_CMAX`: the polynomial continuation of the end span (no knot at the join).
    Cmax,
}

impl Continuity {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            Continuity::G1 => 2,
            Continuity::G2 => 3,
            Continuity::Cmax => 4,
        }
    }

    /// The continuity for an upstream flag value, `None` for any other value.
    #[must_use]
    pub const fn from_flag(flag: i32) -> Option<Self> {
        match flag {
            2 => Some(Continuity::G1),
            3 => Some(Continuity::G2),
            4 => Some(Continuity::Cmax),
            _ => None,
        }
    }
}

/// Result of [`Curve::eval_even_spaced_pts`]: `n + 1` points and their parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct EvenSpacedPts {
    /// The approximately evenly spaced points (upstream `P`).
    pub points: Vec<Vec3>,
    /// Their parameters (upstream `u`).
    pub params: Vec<f64>,
}

/// `A + alpha*B` with `A`, `B` 3-vectors written out as upstream's `N_VectorBlendPt`.
fn moved(p: Vec3, alpha: f64, v: Vec3) -> Vec3 {
    let mut q = p;
    blend_pt(&mut q, alpha, v);
    q
}

/// The new end control point of a `NL_G1` extension: `P` (Euclidean) as a control
/// point with the weight `w` of the neighbouring control point and the dimension of
/// `Pw[0]` (`N_PtToXYZ` / `N_CPtFromWxWyWz` sequence of `ST_curxcdr` / `ST_curxcrr`).
fn g1_end_cpt(p: Vec3, w: f64, pw0: CPoint) -> CPoint {
    let (mut wx, mut wy, mut wz) = (p.x, p.y, p.z);
    if w != NOW {
        wx *= w;
        wy *= w;
        wz *= w;
    }
    if pw0.z == NOZ {
        wz = NOZ;
    }
    CPoint::new(wx, wy, wz, w)
}

/// Unit end tangent `a - b` with its magnitude, retrying with `a - c` when `a - b`
/// cannot be normalised (`c` evaluated lazily, as upstream reads it only then).
fn end_tangent(
    a: Vec3,
    b: Vec3,
    c: impl FnOnce() -> Result<Vec3>,
    op: &'static str,
) -> Result<(Vec3, f64)> {
    match normalize(a - b) {
        Ok(r) => Ok(r),
        Err(_) => normalize(a - c()?).map_err(|_| nlib(op, NlibCode::Geometry)),
    }
}

/// The signed binomial blend loops shared by the `NL_CMAX` branches: control points
/// `Qw[base + sgn*jj]` for `jj = 1..=p` of the continuation segment from the scaled
/// derivatives `Dw` (upstream's "this loop computes new control points").
///
/// `start` selects the start-end form (`Qw[p-jj]`, scale `-kp*dl`) or the end form
/// (`Qw[nr+jj]`, scale `dr`).
fn cmax_new_cpts(
    qw: &mut [CPoint],
    dw: &[CPoint],
    p: isize,
    nr: isize,
    u2: f64,
    start: bool,
    op: &'static str,
) -> Result<()> {
    let pf = p as f64;
    let mut dl = u2 / pf;
    for jj in 1..=p {
        let mut kp: isize = if jj % 2 == 0 { -1 } else { 1 };
        let tgt = if start { p - jj } else { nr + jj };
        let scaled = if start {
            get(dw, jj, op)?.scale((-kp) as f64 * dl)
        } else {
            get(dw, jj, op)?.scale(dl)
        };
        set(qw, tgt, scaled, op)?;
        if jj < p {
            dl = dl * u2 / (pf - jj as f64);
        }
        for kk in 0..jj {
            let src = if start { p - kk } else { nr + kk };
            let a = get(qw, src, op)?;
            let mut q = get(qw, tgt, op)?;
            q.blend(kp as f64 * binomial(ux(jj, op)?, ux(kk, op)?)?, a);
            set(qw, tgt, q, op)?;
            kp *= -1;
        }
    }
    Ok(())
}

/// Scaled end derivatives `Dw[1..=p]` of the `NL_CMAX` branches ("this loop computes
/// derivatives from the right / left"). `start` reads `Rw[ii]` / `Rw[kk]` with the
/// factor `dr = p / (UR[p+1] - UR[0])`; the end form reads `Rw[nr-ii]` / `Rw[nr-kk]`
/// with `dl = -p / (UR[mr] - UR[mr-p-1])`.
fn cmax_derivs(
    rw: &[CPoint],
    ur: &[f64],
    p: isize,
    nr: isize,
    mr: isize,
    start: bool,
    op: &'static str,
) -> Result<Vec<CPoint>> {
    let pf = p as f64;
    let mut dw: Vec<CPoint> = storage(p, op)?;
    let span = if start {
        get(ur, p + 1, op)? - get(ur, 0, op)?
    } else {
        get(ur, mr, op)? - get(ur, mr - p - 1, op)?
    };
    let mut f = if start { pf / span } else { -pf / span };
    for ii in 1..=p {
        let mut d = get(rw, if start { ii } else { nr - ii }, op)?;
        let mut kp: isize = if ii % 2 == 0 { 1 } else { -1 };
        for kk in 0..ii {
            let a = get(rw, if start { kk } else { nr - kk }, op)?;
            d.blend(kp as f64 * binomial(ux(ii, op)?, ux(kk, op)?)?, a);
            kp *= -1;
        }
        set(&mut dw, ii, d.scale(f), op)?;
        f = if start {
            f * (pf - ii as f64) / span
        } else {
            -f * (pf - ii as f64) / span
        };
    }
    Ok(dw)
}

/// Whether some weight of `Qw[lo..=hi]` is outside `[NL_WMIN, NL_WMAX]`.
fn bad_weight(qw: &[CPoint], lo: isize, hi: isize, op: &'static str) -> Result<bool> {
    for jj in lo..=hi {
        let w = get(qw, jj, op)?.w;
        if w < WEIGHT_MIN || w > WEIGHT_MAX {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The working curve of the `NL_G2` / `NL_CMAX` branches: the input with `uu` inserted
/// up to multiplicity `p` (`N_CrvCopy` + `N_CrvInsertKnot`), or the input itself.
fn with_mult_p<'a>(
    cur: &'a Curve,
    uu: f64,
    p: isize,
    mult: isize,
    slot: &'a mut Option<Curve>,
    op: &'static str,
) -> Result<&'a Curve> {
    if mult < p {
        let r = ux(p - mult, op)?;
        Ok(slot.insert(cur.insert_knot(uu, r)?))
    } else {
        Ok(cur)
    }
}

impl Curve {
    /// The point at `u` in homogeneous space (`N_CrvEvalPt`): the non-vanishing basis
    /// functions times their control points, not mapped to Euclidean space.
    ///
    /// Errors with [`NlibCode::Parameter`] when `u` is outside the knot range, and
    /// with the errors of [`crate::nurbs::knots::KnotVector::basis_eval`].
    pub fn eval_pt(&self, u: f64, side: Side) -> Result<CPoint> {
        const OP: &str = OP_EVAL_PT;
        let pw = self.cpts();
        let pu = self.degree();
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let knt = self.knot_vector()?;

        // Check parameter.
        if knt.is_param_out_of_bounds(u) {
            return Err(nlib(OP, NlibCode::Parameter));
        }

        // Compute non-vanishing B-splines.
        let basis = knt.basis_eval(pu, u, side)?;
        let spn = sx(basis.span, OP)?;

        // Compute the point on the curve.
        let mut cw = CPoint::ZERO;
        for i in 0..=p {
            cw.blend(get(&basis.values, i, OP)?, get(pw, spn - p + i, OP)?);
        }
        Ok(cw)
    }

    /// Homogeneous derivatives `Cw^(k)(u)`, `k = 0..=der` (`N_CrvDerivsAtKnot`): the
    /// basis function derivatives times their control points. At a knot, `side`
    /// picks the span (`Left`: right-hand derivatives, `Right`: left-hand ones).
    ///
    /// Errors with [`NlibCode::Parameter`] when `u` is outside the knot range, and
    /// with the errors of [`basis_derivs`].
    pub fn derivs_at_knot(&self, u: f64, side: Side, der: usize) -> Result<Vec<CPoint>> {
        const OP: &str = OP_DERIVS_AT_KNOT;
        let pw = self.cpts();
        let pu = self.degree();
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let knt = self.knot_vector()?;

        // Check parameter.
        if knt.is_param_out_of_bounds(u) {
            return Err(nlib(OP, NlibCode::Parameter));
        }

        // Compute basis function derivatives.
        let bd = basis_derivs(&knt, pu, u, side, der)?;
        let spn = sx(bd.span, OP)?;

        // Compute derivatives.
        let mut dw = Vec::new();
        for row in &bd.derivs {
            let mut d = CPoint::ZERO;
            for i in 0..=p {
                d.blend(get(row, i, OP)?, get(pw, spn - p + i, OP)?);
            }
            dw.push(d);
        }
        Ok(dw)
    }

    /// `n + 1` approximately evenly spaced points between the parameters `u0` and
    /// `u1`, and their parameters (`N_CrvEvalEvenSpacedPts`): the parameters are
    /// redistributed by linear interpolation of the chord lengths until every chord
    /// deviates from their average by at most `tol` times the average, or for
    /// [`ITER_LIMIT`] rounds (the best result so far is returned).
    ///
    /// Errors with [`NlibCode::Parameter`] when `u0` or `u1` is outside the knot
    /// range, [`NlibCode::Input`] when `n <= 1`, [`NlibCode::Numerical`] when an
    /// interpolation quotient is not computable, and with the errors of
    /// [`Curve::eval`].
    pub fn eval_even_spaced_pts(
        &self,
        u0: f64,
        u1: f64,
        n: usize,
        tol: f64,
    ) -> Result<EvenSpacedPts> {
        const OP: &str = OP_EVEN;
        let knt = self.knot_vector()?;

        // Check parameters.
        if knt.is_param_out_of_bounds(u0) || knt.is_param_out_of_bounds(u1) {
            return Err(nlib(OP, NlibCode::Parameter));
        }
        if n <= 1 {
            return Err(nlib(OP, NlibCode::Input));
        }
        let ni = sx(n, OP)?;
        let nf = n as f64;

        // Allocate arrays and initialize the t-array.
        let mut q: Vec<Vec3> = storage(ni, OP)?;
        let mut t: Vec<f64> = storage(ni, OP)?;
        let mut oldt: Vec<f64> = storage(ni, OP)?;
        let mut s: Vec<f64> = storage(ni, OP)?;
        set(&mut oldt, 0, u0, OP)?;
        set(&mut oldt, ni, u1, OP)?;
        set(&mut t, 0, u0, OP)?;
        set(&mut t, ni, u1, OP)?;
        let dt = (u1 - u0) / nf;
        for i in 1..ni {
            set(&mut t, i, u0 + i as f64 * dt, OP)?;
        }

        // Initialize start point.
        set(&mut q, 0, self.eval(u0, Side::Left)?, OP)?;

        // Iteratively compute distances, and linearly interpolate to improve them.
        set(&mut s, 0, 0.0, OP)?;
        for its in 1..=ITER_LIMIT {
            // Compute points and distances.
            for i in 1..=ni {
                let qi = self.eval(get(&t, i, OP)?, Side::Left)?;
                set(&mut q, i, qi, OP)?;
                let d = dist_pt_pt(get(&q, i - 1, OP)?, qi);
                let si = get(&s, i - 1, OP)? + d;
                set(&mut s, i, si, OP)?;
            }
            let aver = get(&s, ni, OP)? / nf;

            // Compute deviations (`i` ends at n+1 when every chord is close enough).
            let mut i = ni + 1;
            if aver > MODEL_TOL {
                i = 1;
                while i <= ni {
                    let d = (get(&s, i, OP)? - get(&s, i - 1, OP)? - aver).abs();
                    if d > aver * tol {
                        break;
                    }
                    i += 1;
                }
            }

            // Convergence -> return; exceeded iteration limit -> return best result.
            if aver <= MODEL_TOL || i > ni || its >= ITER_LIMIT {
                return Ok(EvenSpacedPts {
                    points: q,
                    params: t,
                });
            }

            // Recompute t-values using linear interpolation.
            std::mem::swap(&mut t, &mut oldt);
            let mut k: isize = 1;
            for i in 1..ni {
                let d = i as f64 * aver;
                while d > get(&s, k, OP)? {
                    k += 1;
                }
                let num = get(&oldt, k, OP)? - get(&oldt, k - 1, OP)?;
                let den = get(&s, k, OP)? - get(&s, k - 1, OP)?;
                if float_op_is_bad(num, den, FloatOp::Division) {
                    return Err(nlib(OP, NlibCode::Numerical));
                }
                set(
                    &mut t,
                    i,
                    (num / den) * (d - get(&s, k - 1, OP)?) + get(&oldt, k - 1, OP)?,
                    OP,
                )?;
            }
        }
        // The last round always returns above.
        Err(oob(OP))
    }

    /// An upper bound on the magnitude of the second derivative
    /// (`N_CrvGetMax2ndDeriv`): the largest `|C''|` over `p + 1` evenly spaced
    /// parameters of every Bezier segment (shared segment ends sampled once).
    ///
    /// Errors with the errors of [`Curve::decompose_bez`] and [`Curve::derivs`].
    pub fn max_2nd_deriv(&self) -> Result<f64> {
        const OP: &str = OP_MAX2;
        let pu = self.degree();
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let pf = pu as f64;

        // Decompose into Bezier segments.
        let bez = self.decompose_bez()?;

        // Compute maximum derivative.
        let mut u: Vec<f64> = storage(p, OP)?;
        let mut duu = 0.0;
        for (i, b) in bez.iter().enumerate() {
            let al = isize::from(i != 0);
            let bu = b.knots();
            let r = sx(b.m(), OP)?;
            let (b0, br) = (get(bu, 0, OP)?, get(bu, r, OP)?);
            let ui = (br - b0) / pf;
            set(&mut u, 0, b0, OP)?;
            set(&mut u, p, br, OP)?;
            for a in 1..p {
                set(&mut u, a, b0 + a as f64 * ui, OP)?;
            }
            for a in al..=p {
                let cd = b.derivs(get(&u, a, OP)?, Side::Left, 2)?;
                let c2 = get(&cd, 2, OP)?;
                let muu = (c2.x * c2.x + c2.y * c2.y + c2.z * c2.z).sqrt();
                if muu > duu {
                    duu = muu;
                }
            }
        }
        Ok(duu)
    }

    /// First and second derivatives at `u` of the planar offset curve at distance `d`
    /// (`N_CrvPlanarOffsetGetDeriv`), offsetting along `N x T` for the plane normal
    /// `normal`: `O' = f1 C'` and `O'' = f1 C'' + f2 C'` with `f1 = 1 + s k d`,
    /// `f2 = s k' d` (`k` the curvature, `k'` its derivative, `s` the side sign).
    ///
    /// Errors with [`NlibCode::Numerical`] when the curvature quotient is not
    /// computable, and with the errors of [`Curve::derivs`] and
    /// [`Curve::curvature_deriv`].
    pub fn planar_offset_derivs(
        &self,
        u: f64,
        d: f64,
        normal: Vec3,
        side: Side,
    ) -> Result<(Vec3, Vec3)> {
        const OP: &str = OP_OFFSET;

        // Get derivatives, the curvature and its derivative.
        let dd = self.derivs(u, side, 2)?;
        let d1 = get(&dd, 1, OP)?;
        let d2 = get(&dd, 2, OP)?;
        let b = d1.cross(d2);
        let num = b.dot(b).sqrt();
        let mut den = d1.dot(d1).sqrt();
        den = den * den * den;
        if float_op_is_bad(num, den, FloatOp::Division) {
            return Err(nlib(OP, NlibCode::Numerical));
        }
        let kap = num / den;
        let den = b.dot(normal);
        let sgn = if den > 0.0 { -1.0 } else { 1.0 };
        let kpp = self.curvature_deriv(u, side)?;

        // Get offset derivatives.
        let f1 = 1.0 + sgn * kap * d;
        let od1 = Vec3::new(f1 * d1.x, f1 * d1.y, f1 * d1.z);
        let f2 = sgn * kpp * d;
        let od2 = Vec3::new(
            f1 * d2.x + f2 * d1.x,
            f1 * d2.y + f2 * d1.y,
            f1 * d2.z + f2 * d1.z,
        );
        Ok((od1, od2))
    }

    /// Extends the curve at `end` by about the arc length `dist` (`ST_curxcdr`, the
    /// body of `N_CrvExtendByDist`, which calls it with `depth = 0`); a
    /// [`ToleranceKind::Relative`] distance is scaled by the curve's arc length. The
    /// original parameter range still maps to the original curve.
    ///
    /// - [`Continuity::G1`]: a linear segment along the end tangent, degree-elevated;
    /// - [`Continuity::G2`]: an end segment of about the extension length reflected in
    ///   the plane through the end point normal to the end tangent (extensions longer
    ///   than `0.9313` times the curve are done in two recursive steps);
    /// - [`Continuity::Cmax`]: the polynomial continuation of the end span, its
    ///   length fitted by up to five secant steps, after which the old end knot and
    ///   any inserted knots are removed again.
    ///
    /// Errors with [`SolidError::NotConverged`] above [`MAX_DIST_DEPTH`] recursion
    /// levels, [`NlibCode::Input`] for a curve shorter than the tolerances (relative
    /// distance or `G2`), [`NlibCode::Geometry`] when no end tangent can be computed,
    /// [`NlibCode::Weight`] when a rational `Cmax` extension keeps creating weights
    /// outside `[NL_WMIN, NL_WMAX]`, and with the errors of [`Curve::arc_length`],
    /// [`Curve::insert_knot`], [`Curve::remove_knot`] and the knot span search.
    pub fn extend_by_dist_at_depth(
        &self,
        dist: f64,
        kind: ToleranceKind,
        end: CurveEnd,
        cont: Continuity,
        depth: usize,
    ) -> Result<Curve> {
        const OP: &str = OP_XCDR;

        // Limit recursion to 50 levels.
        if depth > MAX_DIST_DEPTH {
            return Err(depth_limit(OP, depth));
        }

        // Get local notation.
        let pw = self.cpts();
        let up = self.knots();
        let pu = self.degree();
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        let up0 = get(up, 0, OP)?;
        let upm = get(up, m, OP)?;

        // Get extension distance.
        let mut ldst = dist;
        let mut d1 = 0.0;
        if kind == ToleranceKind::Relative || cont == Continuity::G2 {
            d1 = self.arc_length(up0, upm, 0.01, ToleranceKind::Relative)?;
            if kind == ToleranceKind::Relative {
                ldst *= d1;
            }
            // Upstream `d1 <= NL_MTOL AND d1 <= NL_PTOL` (the second implies the first).
            if d1 <= PTOL {
                return Err(nlib(OP, NlibCode::Input));
            }

            // Extend long G2 extensions in two steps.
            if cont == Continuity::G2 && ldst > 0.9313 * d1 {
                let dd = 0.4671 * d1;
                let cq = self.extend_by_dist_at_depth(
                    ldst - dd,
                    ToleranceKind::Absolute,
                    end,
                    cont,
                    depth + 1,
                )?;
                return cq.extend_by_dist_at_depth(
                    dd,
                    ToleranceKind::Absolute,
                    end,
                    cont,
                    depth + 1,
                );
            }
        }

        // Find the parameter `uu` of the segment to mirror / continue and the number
        // of knots of the extended curve.
        let mut uu = 0.0;
        let mut span: isize = 0;
        let mut mult: isize = 0;
        let mq: isize;
        let ktol = KNOT_TOL * (upm - up0);
        match cont {
            Continuity::G2 => {
                let knt = self.knot_vector()?;
                let du = (upm - up0) * (ldst / d1);
                uu = up0 - 10.0;
                if end == CurveEnd::Start {
                    // Find the parameter range that maps to the extension length.
                    let mut ur = up0;
                    let mut dr = 0.0;
                    let mut ul;
                    let mut dl;
                    let mut steps = 0usize;
                    loop {
                        ul = ur;
                        dl = dr;
                        ur += du;
                        if ur > upm {
                            ur = upm;
                        }
                        steps += 1;
                        if ur == ul || steps > MAX_G2_STEPS {
                            // Upstream repeats this step forever (or for about
                            // `d1 / ldst` steps).
                            return Err(depth_limit(OP, steps));
                        }
                        dr = self.arc_length(up0, ur, 0.005, ToleranceKind::Relative)?;
                        if (ldst - dr).abs() < ldst * 0.05 || ur == upm {
                            uu = ur;
                            break;
                        }
                        if dr > ldst {
                            break;
                        }
                    }

                    // Refine a range that is too long.
                    if uu < up0 {
                        for _ in 0..4 {
                            uu = ul + (ldst - dl) * ((ur - ul) / (dr - dl));
                            let dd = self.arc_length(up0, uu, 0.005, ToleranceKind::Relative)?;
                            if (ldst - dd).abs() < ldst * 0.05 {
                                break;
                            }
                            if dd > ldst {
                                ur = uu;
                                dr = dd;
                            } else {
                                ul = uu;
                                dl = dd;
                            }
                        }
                    }

                    // Find the span (and multiplicity) of uu; keep it off the span's
                    // lower knot.
                    let (s, k) = knt.find_span_and_mult(pu, uu, Side::Right)?;
                    span = sx(s, OP)?;
                    mult = sx(k, OP)?;
                    let (lo, hi) = (get(up, span, OP)?, get(up, span + 1, OP)?);
                    if uu != hi && uu - lo < ktol && lo + ktol < hi {
                        uu = lo + ktol;
                    }
                    mq = m + span;
                } else {
                    // Find the parameter range [uu, U[m]] with the extension length.
                    let mut ul = upm;
                    let mut dl = 0.0;
                    let mut ur;
                    let mut dr;
                    let mut steps = 0usize;
                    loop {
                        ur = ul;
                        dr = dl;
                        ul -= du;
                        if ul < up0 {
                            ul = up0;
                        }
                        steps += 1;
                        if ul == ur || steps > MAX_G2_STEPS {
                            // Upstream repeats this step forever (or for about
                            // `d1 / ldst` steps).
                            return Err(depth_limit(OP, steps));
                        }
                        dl = self.arc_length(ul, upm, 0.005, ToleranceKind::Relative)?;
                        if (ldst - dl).abs() < ldst * 0.05 || ul == up0 {
                            uu = ul;
                            break;
                        }
                        if dl > ldst {
                            break;
                        }
                    }

                    // Refine a range that is too long.
                    if uu < up0 {
                        for _ in 0..4 {
                            uu = ur - (ldst - dr) * ((ur - ul) / (dl - dr));
                            let dd = self.arc_length(uu, upm, 0.005, ToleranceKind::Relative)?;
                            if (ldst - dd).abs() < ldst * 0.05 {
                                break;
                            }
                            if dd > ldst {
                                ul = uu;
                                dl = dd;
                            } else {
                                ur = uu;
                                dr = dd;
                            }
                        }
                    }

                    // Find the span (and multiplicity) of uu; keep it off the span's
                    // upper knot.
                    let (s, k) = knt.find_span_and_mult(pu, uu, Side::Left)?;
                    span = sx(s, OP)?;
                    mult = sx(k, OP)?;
                    let (lo, hi) = (get(up, span, OP)?, get(up, span + 1, OP)?);
                    if uu != lo && hi - uu < ktol && hi - ktol > lo {
                        uu = hi - ktol;
                    }
                    mq = 2 * m - span - 1;
                }
            }
            Continuity::Cmax => {
                (uu, mult) = cmax_knot(up, p, m, end, OP)?;
                mq = m + 2 * p - mult;
            }
            Continuity::G1 => {
                mq = m + p;
            }
        }

        // Output arrays (upstream: N_CrvSizeArrays / N_AllocCrvArrays).
        let nq = mq - p - 1;
        let mut qw: Vec<CPoint> = storage(nq, OP)?;
        let mut uq: Vec<f64> = storage(mq, OP)?;

        // Compute the new control points and knots defining the extension.
        match cont {
            Continuity::G1 => {
                let (mut kk, mut kq, mut kp);
                let dd = pf_of(p);
                if end == CurveEnd::Start {
                    // New start control point.
                    let p1 = get(pw, 0, OP)?.to_euclid();
                    let p2 = get(pw, 1, OP)?.to_euclid();
                    let (v, mag) = end_tangent(p1, p2, || Ok(get(pw, 2, OP)?.to_euclid()), OP)?;
                    let p1 = moved(p1, ldst, v);
                    let q0 = g1_end_cpt(p1, get(pw, 1, OP)?.w, get(pw, 0, OP)?);
                    set(&mut qw, 0, q0, OP)?;

                    // p - 1 new interior control points (degree elevation).
                    let pw0 = get(pw, 0, OP)?;
                    for ii in 1..p {
                        let d = ii as f64 / dd;
                        set(&mut qw, ii, CPoint::combine2(1.0 - d, q0, d, pw0), OP)?;
                    }

                    // New start knot.
                    let mut du = ldst * (get(up, p + 1, OP)? - up0) / (dd * mag);
                    if du < ktol {
                        du = ktol;
                    }
                    let u = up0 - du;
                    for ii in 0..=p {
                        set(&mut uq, ii, u, OP)?;
                    }
                    kk = p;
                    kq = p + 1;
                    kp = 1;
                } else {
                    // New end control point.
                    let p1 = get(pw, n - 1, OP)?.to_euclid();
                    let p2 = get(pw, n, OP)?.to_euclid();
                    let (v, mag) = end_tangent(p2, p1, || Ok(get(pw, n - 2, OP)?.to_euclid()), OP)?;
                    let p2 = moved(p2, ldst, v);
                    let qe = g1_end_cpt(p2, get(pw, n - 1, OP)?.w, get(pw, 0, OP)?);
                    set(&mut qw, n + p, qe, OP)?;

                    // p - 1 new interior control points (degree elevation).
                    let pwn = get(pw, n, OP)?;
                    for ii in 1..p {
                        let d = ii as f64 / dd;
                        set(&mut qw, n + ii, CPoint::combine2(1.0 - d, pwn, d, qe), OP)?;
                    }

                    // New end knot.
                    let mut du = ldst * (upm - get(up, m - p - 1, OP)?) / (dd * mag);
                    if du < ktol {
                        du = ktol;
                    }
                    let u = upm + du;
                    for ii in 0..=p {
                        set(&mut uq, m + ii, u, OP)?;
                    }
                    kk = 0;
                    kq = 0;
                    kp = 0;
                }
                copy_rest(
                    &mut qw, &mut uq, pw, up, n, m, &mut kk, &mut kq, &mut kp, OP,
                )?;
                Curve::new(qw, pu, uq).map_err(|_| oob(OP))
            }
            Continuity::G2 => {
                let mut slot = None;
                let r = with_mult_p(self, uu, p, mult, &mut slot, OP)?;
                if mult < p && end == CurveEnd::End {
                    span += p - mult;
                }
                let (mut kk, mut kq, mut kp);
                if end == CurveEnd::Start {
                    // Reflection plane at the start point.
                    let p1 = get(pw, 0, OP)?.to_euclid();
                    let p2 = get(pw, 1, OP)?.to_euclid();
                    let (v, _) = end_tangent(p1, p2, || Ok(get(pw, 2, OP)?.to_euclid()), OP)?;
                    let pln = Plane::new(p1, v);
                    kk = reflect_start(&mut qw, r, span, pln, OP)?;
                    kq = start_mirror_knots(&mut uq, up, p, span, uu, OP)?;
                    kp = 1;
                } else {
                    // Reflection plane at the end point.
                    let p1 = get(pw, n, OP)?.to_euclid();
                    let p2 = get(pw, n - 1, OP)?.to_euclid();
                    let (v, _) = end_tangent(p1, p2, || Ok(get(pw, n - 2, OP)?.to_euclid()), OP)?;
                    let pln = Plane::new(p1, v);
                    reflect_end(&mut qw, r, n, p, span, pln, OP)?;
                    end_mirror_knots(
                        &mut uq,
                        up,
                        r.knots(),
                        p,
                        m,
                        sx(r.m(), OP)?,
                        mq,
                        span,
                        uu,
                        OP,
                    )?;
                    kk = 0;
                    kq = 0;
                    kp = 0;
                }
                copy_rest(
                    &mut qw, &mut uq, pw, up, n, m, &mut kk, &mut kq, &mut kp, OP,
                )?;
                Curve::new(qw, pu, uq).map_err(|_| oob(OP))
            }
            Continuity::Cmax => {
                let mut slot = None;
                let r = with_mult_p(self, uu, p, mult, &mut slot, OP)?;
                let rw = r.cpts();
                let ur = r.knots();
                let nr = sx(r.n(), OP)?;
                let mr = sx(r.m(), OP)?;
                let rat = r.is_rational();
                let start = end == CurveEnd::Start;
                let mut cur = if start {
                    for ii in 0..=nr {
                        set(&mut qw, ii + p, get(rw, ii, OP)?, OP)?;
                    }
                    for ii in 1..=mr {
                        set(&mut uq, ii + p, get(ur, ii, OP)?, OP)?;
                    }
                    let dw = cmax_derivs(rw, ur, p, nr, mr, true, OP)?;

                    // Iteratively compute the new start segment until its arc length
                    // is right (u: span length, d: arc length).
                    let ur0 = get(ur, 0, OP)?;
                    let (mut u1, mut d1, mut u2) = (0.0, 0.0, get(ur, p + 1, OP)? - ur0);
                    for ii in 0..5 {
                        for jj in 0..=p {
                            set(&mut uq, jj, ur0 - u2, OP)?;
                        }
                        cmax_new_cpts(&mut qw, &dw, p, nr, u2, true, OP)?;
                        if rat && bad_weight(&qw, 0, p - 1, OP)? {
                            if ii == 4 || u2 <= u1 {
                                return Err(nlib(OP, NlibCode::Weight));
                            }
                            u2 = 0.5 * (u1 + u2);
                            continue;
                        }

                        // Check the arc length of the new segment.
                        let cq = Curve::with_indices(
                            qw.clone(),
                            ux(nq, OP)?,
                            pu,
                            uq.clone(),
                            ux(mq, OP)?,
                        )?;
                        let d2 = cq.arc_length(
                            get(&uq, 0, OP)?,
                            get(&uq, p + 1, OP)?,
                            0.005,
                            ToleranceKind::Relative,
                        )?;
                        if (ldst - d2).abs() < ldst * 0.03 {
                            break;
                        }
                        if float_op_is_bad(u2 - u1, d2 - d1, FloatOp::Division) {
                            break;
                        }
                        let u3 = u2;
                        u2 = u1 + (ldst - d1) * ((u2 - u1) / (d2 - d1));
                        u1 = u3;
                        d1 = d2;
                    }

                    // Remove the original start knot.
                    let cq = Curve::with_indices(qw, ux(nq, OP)?, pu, uq, ux(mq, OP)?)?;
                    cq.remove_knot(up0, pu, MODEL_TOL)?.curve
                } else {
                    for ii in 0..=nr {
                        set(&mut qw, ii, get(rw, ii, OP)?, OP)?;
                    }
                    for ii in 0..mr {
                        set(&mut uq, ii, get(ur, ii, OP)?, OP)?;
                    }
                    let dw = cmax_derivs(rw, ur, p, nr, mr, false, OP)?;

                    // Iteratively compute the new end segment until its arc length is
                    // right (u: span length, d: arc length).
                    let urm = get(ur, mr, OP)?;
                    let (mut u1, mut d1, mut u2) = (0.0, 0.0, urm - get(ur, mr - p - 1, OP)?);
                    for ii in 0..5 {
                        for jj in 0..=p {
                            set(&mut uq, mr + jj, urm + u2, OP)?;
                        }
                        cmax_new_cpts(&mut qw, &dw, p, nr, u2, false, OP)?;
                        if rat && bad_weight(&qw, nr + 1, nr + p, OP)? {
                            if ii == 4 || u2 <= u1 {
                                return Err(nlib(OP, NlibCode::Weight));
                            }
                            u2 = 0.5 * (u1 + u2);
                            continue;
                        }

                        // Check the arc length of the new segment.
                        let cq = Curve::with_indices(
                            qw.clone(),
                            ux(nq, OP)?,
                            pu,
                            uq.clone(),
                            ux(mq, OP)?,
                        )?;
                        let d2 = cq.arc_length(
                            get(&uq, mq - p - 1, OP)?,
                            get(&uq, mq, OP)?,
                            0.005,
                            ToleranceKind::Relative,
                        )?;
                        if (ldst - d2).abs() < ldst * 0.03 {
                            break;
                        }
                        if float_op_is_bad(u2 - u1, d2 - d1, FloatOp::Division) {
                            break;
                        }
                        let u3 = u2;
                        u2 = u1 + (ldst - d1) * ((u2 - u1) / (d2 - d1));
                        u1 = u3;
                        d1 = d2;
                    }

                    // Remove the original end knot.
                    let cq = Curve::with_indices(qw, ux(nq, OP)?, pu, uq, ux(mq, OP)?)?;
                    cq.remove_knot(upm, pu, MODEL_TOL)?.curve
                };

                // Remove the additional instances of uu.
                if mult < p {
                    cur = cur.remove_knot(uu, ux(p - mult, OP)?, MODEL_TOL)?.curve;
                }
                Ok(cur)
            }
        }
    }

    /// Extends the curve at `end` to the parameter `par` (`ST_curxcrr`, the body of
    /// `N_CrvExtendByParamDist`, which calls it with `depth = 0`). The original
    /// parameter range still maps to the original curve; the new end knot is `par`.
    ///
    /// The continuities are those of [`Curve::extend_by_dist_at_depth`]; `G2`
    /// extensions longer than `0.9213` times the parameter range are done in two
    /// recursive steps, and `Cmax` uses the exact span length `|par - end knot|`.
    ///
    /// Errors with [`SolidError::NotConverged`] above [`MAX_PARAM_DEPTH`] recursion
    /// levels, [`NlibCode::Geometry`] when no end tangent can be computed,
    /// [`NlibCode::Weight`] when a rational `Cmax` extension creates weights outside
    /// `[NL_WMIN, NL_WMAX]`, `InvalidInput` ([`BARE_FLAG_REASON`]) when the
    /// knot-inserted curve has an unexpected size, and with the errors of
    /// [`Curve::insert_knot`], [`Curve::remove_knot`] and the knot span search.
    pub fn extend_by_param_dist_at_depth(
        &self,
        par: f64,
        end: CurveEnd,
        cont: Continuity,
        depth: usize,
    ) -> Result<Curve> {
        const OP: &str = OP_XCRR;

        // Limit recursion to 10 levels.
        if depth > MAX_PARAM_DEPTH {
            return Err(depth_limit(OP, depth));
        }

        // Get local notation.
        let pw = self.cpts();
        let up = self.knots();
        let pu = self.degree();
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        let up0 = get(up, 0, OP)?;
        let upm = get(up, m, OP)?;
        let ktol = KNOT_TOL * (upm - up0);

        // Get mq, the new highest knot index.
        let mut uu = 0.0;
        let mut span: isize = 0;
        let mut mult: isize = 0;
        let mq: isize;
        match cont {
            Continuity::G2 => {
                // Recurse if par is too far away.
                if (end == CurveEnd::Start && up0 - par > 0.9213 * (upm - up0))
                    || (end == CurveEnd::End && par - upm > 0.9213 * (upm - up0))
                {
                    let dd = if end == CurveEnd::Start {
                        par + 0.4671 * (upm - up0)
                    } else {
                        par - 0.4671 * (upm - up0)
                    };
                    let cq = self.extend_by_param_dist_at_depth(dd, end, cont, depth + 1)?;
                    return cq.extend_by_param_dist_at_depth(par, end, cont, depth + 1);
                }

                // uu: the parameter as far inside the curve as par is outside.
                uu = if end == CurveEnd::Start {
                    2.0 * up0 - par
                } else {
                    2.0 * upm - par
                };

                // Snap uu to the closest interior knot when it is that close.
                let mut d1 = f64::MAX;
                let mut jj: isize = 0;
                for ii in p + 1..m - p {
                    let d = (uu - get(up, ii, OP)?).abs();
                    if d < d1 {
                        d1 = d;
                        jj = ii;
                    }
                }
                if d1 < ktol {
                    uu = get(up, jj, OP)?;
                }

                let knt = self.knot_vector()?;
                if end == CurveEnd::Start {
                    // Keep uu off the span's lower knot.
                    let (s, k) = knt.find_span_and_mult(pu, uu, Side::Right)?;
                    span = sx(s, OP)?;
                    mult = sx(k, OP)?;
                    let (lo, hi) = (get(up, span, OP)?, get(up, span + 1, OP)?);
                    if uu != hi && uu - lo < ktol && lo + ktol < hi {
                        uu = lo + ktol;
                    }
                    mq = m + span;
                } else {
                    // Keep uu off the span's upper knot.
                    let (s, k) = knt.find_span_and_mult(pu, uu, Side::Left)?;
                    span = sx(s, OP)?;
                    mult = sx(k, OP)?;
                    let (lo, hi) = (get(up, span, OP)?, get(up, span + 1, OP)?);
                    if uu != lo && hi - uu < ktol && hi - ktol > lo {
                        uu = hi - ktol;
                    }
                    mq = 2 * m - span - 1;
                }
            }
            Continuity::Cmax => {
                (uu, mult) = cmax_knot(up, p, m, end, OP)?;
                mq = m + 2 * p - mult;
            }
            Continuity::G1 => {
                // Room for one new span.
                mq = m + p;
            }
        }

        // Output arrays (upstream: N_CrvSizeArrays / N_AllocCrvArrays).
        let nq = mq - p - 1;
        let mut qw: Vec<CPoint> = storage(nq, OP)?;
        let mut uq: Vec<f64> = storage(mq, OP)?;

        // Compute the new control points and knots defining the extension.
        match cont {
            Continuity::G1 => {
                let (mut kk, mut kq, mut kp);
                let dd = pf_of(p);
                if end == CurveEnd::Start {
                    // New start control point.
                    let p1 = get(pw, 0, OP)?.to_euclid();
                    let p2 = get(pw, 1, OP)?.to_euclid();
                    let (v, mag) = end_tangent(p1, p2, || Ok(get(pw, 2, OP)?.to_euclid()), OP)?;
                    let d1 = mag * ((up0 - par) / (get(up, p + 1, OP)? - up0));
                    let p1 = moved(p1, d1, v);
                    let q0 = g1_end_cpt(p1, get(pw, 1, OP)?.w, get(pw, 0, OP)?);
                    set(&mut qw, 0, q0, OP)?;

                    // p - 1 new interior control points (degree elevation).
                    let pw0 = get(pw, 0, OP)?;
                    for ii in 1..p {
                        let d = ii as f64 / dd;
                        set(&mut qw, ii, CPoint::combine2(1.0 - d, q0, d, pw0), OP)?;
                    }
                    kk = p;
                    for ii in 0..=p {
                        set(&mut uq, ii, par, OP)?;
                    }
                    kq = p + 1;
                    kp = 1;
                } else {
                    // New end control point.
                    let p1 = get(pw, n - 1, OP)?.to_euclid();
                    let p2 = get(pw, n, OP)?.to_euclid();
                    let (v, mag) = end_tangent(p2, p1, || Ok(get(pw, n - 2, OP)?.to_euclid()), OP)?;
                    let d1 = mag * ((par - upm) / (upm - get(up, m - p - 1, OP)?));
                    let p2 = moved(p2, d1, v);
                    let qe = g1_end_cpt(p2, get(pw, n - 1, OP)?.w, get(pw, 0, OP)?);
                    set(&mut qw, n + p, qe, OP)?;

                    // p - 1 new interior control points (degree elevation).
                    let pwn = get(pw, n, OP)?;
                    for ii in 1..p {
                        let d = ii as f64 / dd;
                        set(&mut qw, n + ii, CPoint::combine2(1.0 - d, pwn, d, qe), OP)?;
                    }
                    kk = 0;
                    for ii in 0..=p {
                        set(&mut uq, m + ii, par, OP)?;
                    }
                    kq = 0;
                    kp = 0;
                }
                copy_rest(
                    &mut qw, &mut uq, pw, up, n, m, &mut kk, &mut kq, &mut kp, OP,
                )?;
                Curve::new(qw, pu, uq).map_err(|_| oob(OP))
            }
            Continuity::G2 => {
                let mut slot = None;
                let r = with_mult_p(self, uu, p, mult, &mut slot, OP)?;
                if mult < p && end == CurveEnd::End {
                    span += p - mult;
                }
                let (mut kk, mut kq, mut kp);
                if end == CurveEnd::Start {
                    // Reflection plane at the start point (no fallback tangent).
                    let p1 = get(pw, 0, OP)?.to_euclid();
                    let p2 = get(pw, 1, OP)?.to_euclid();
                    let (v, _) = normalize(p1 - p2).map_err(|_| nlib(OP, NlibCode::Geometry))?;
                    let pln = Plane::new(p1, v);
                    kk = reflect_start(&mut qw, r, span, pln, OP)?;
                    let dd = 2.0 * up0;
                    for ii in 0..=p {
                        set(&mut uq, ii, par, OP)?;
                    }
                    kq = p + 1;
                    let mut ii = span;
                    while ii > p {
                        set(&mut uq, kq, dd - get(up, ii, OP)?, OP)?;
                        kq += 1;
                        ii -= 1;
                    }
                    kp = 1;
                } else {
                    // Reflection plane at the end point (no fallback tangent).
                    let p1 = get(pw, n, OP)?.to_euclid();
                    let p2 = get(pw, n - 1, OP)?.to_euclid();
                    let (v, _) = normalize(p1 - p2).map_err(|_| nlib(OP, NlibCode::Geometry))?;
                    let pln = Plane::new(p1, v);
                    reflect_end(&mut qw, r, n, p, span, pln, OP)?;
                    for ii in 0..=p {
                        set(&mut uq, mq - p + ii, par, OP)?;
                    }
                    let ur = r.knots();
                    let mr = sx(r.m(), OP)?;
                    let dd = 2.0 * upm;
                    let mut kq2 = mq - p - 1;
                    for ii in span + 1..mr - p {
                        set(&mut uq, kq2, dd - get(ur, ii, OP)?, OP)?;
                        kq2 -= 1;
                    }
                    kk = 0;
                    kq = 0;
                    kp = 0;
                }
                copy_rest(
                    &mut qw, &mut uq, pw, up, n, m, &mut kk, &mut kq, &mut kp, OP,
                )?;
                Curve::new(qw, pu, uq).map_err(|_| oob(OP))
            }
            Continuity::Cmax => {
                let mut slot = None;
                let r = with_mult_p(self, uu, p, mult, &mut slot, OP)?;
                let rw = r.cpts();
                let ur = r.knots();
                let nr = sx(r.n(), OP)?;
                let mr = sx(r.m(), OP)?;
                if mr + p != mq {
                    return Err(SolidError::InvalidInput {
                        operation: OP,
                        reason: BARE_FLAG_REASON,
                    });
                }
                let rat = r.is_rational();
                let mut cur = if end == CurveEnd::Start {
                    for ii in 0..=nr {
                        set(&mut qw, ii + p, get(rw, ii, OP)?, OP)?;
                    }
                    for ii in 1..=mr {
                        set(&mut uq, ii + p, get(ur, ii, OP)?, OP)?;
                    }
                    let dw = cmax_derivs(rw, ur, p, nr, mr, true, OP)?;
                    let u2 = get(ur, 0, OP)? - par;
                    for jj in 0..=p {
                        set(&mut uq, jj, par, OP)?;
                    }
                    cmax_new_cpts(&mut qw, &dw, p, nr, u2, true, OP)?;
                    if rat && bad_weight(&qw, 0, p - 1, OP)? {
                        return Err(nlib(OP, NlibCode::Weight));
                    }

                    // Remove the original start knot.
                    let cq = Curve::with_indices(qw, ux(nq, OP)?, pu, uq, ux(mq, OP)?)?;
                    cq.remove_knot(up0, pu, MODEL_TOL)?.curve
                } else {
                    for ii in 0..=nr {
                        set(&mut qw, ii, get(rw, ii, OP)?, OP)?;
                    }
                    for ii in 0..=mr {
                        set(&mut uq, ii, get(ur, ii, OP)?, OP)?;
                    }
                    let dw = cmax_derivs(rw, ur, p, nr, mr, false, OP)?;
                    let u2 = par - get(ur, mr, OP)?;
                    for jj in 0..=p {
                        set(&mut uq, mr + jj, par, OP)?;
                    }
                    cmax_new_cpts(&mut qw, &dw, p, nr, u2, false, OP)?;
                    if rat && bad_weight(&qw, nr + 1, nr + p, OP)? {
                        return Err(nlib(OP, NlibCode::Weight));
                    }

                    // Remove the original end knot.
                    let cq = Curve::with_indices(qw, ux(nq, OP)?, pu, uq, ux(mq, OP)?)?;
                    cq.remove_knot(upm, pu, MODEL_TOL)?.curve
                };

                // Remove the additional instances of uu.
                if mult < p {
                    cur = cur.remove_knot(uu, ux(p - mult, OP)?, MODEL_TOL)?.curve;
                }
                Ok(cur)
            }
        }
    }
}

/// `(NL_REAL)p`.
fn pf_of(p: isize) -> f64 {
    p as f64
}

/// The knot `uu` next to the extended end and its multiplicity counted from there,
/// as the `NL_CMAX` branches compute them (`mult` starts at `1` and stops at the end
/// knots).
fn cmax_knot(
    up: &[f64],
    p: isize,
    m: isize,
    end: CurveEnd,
    op: &'static str,
) -> Result<(f64, isize)> {
    let mut mult: isize = 1;
    let uu;
    if end == CurveEnd::Start {
        uu = get(up, p + 1, op)?;
        while p + mult + 1 < m {
            if uu == get(up, p + mult + 1, op)? {
                mult += 1;
            } else {
                break;
            }
        }
    } else {
        uu = get(up, m - p - 1, op)?;
        while m - p - mult - 1 > 0 {
            if uu == get(up, m - p - mult - 1, op)? {
                mult += 1;
            } else {
                break;
            }
        }
    }
    Ok((uu, mult))
}

/// Reflects `Rw[span..=1]` (descending) into `Qw[0..span]`; returns the next free
/// index `kk`.
fn reflect_start(
    qw: &mut [CPoint],
    r: &Curve,
    span: isize,
    pln: Plane,
    op: &'static str,
) -> Result<isize> {
    let rw = r.cpts();
    let mut kk = 0;
    let mut ii = span;
    while ii > 0 {
        set(qw, kk, cpt_reflect(get(rw, ii, op)?, pln)?, op)?;
        kk += 1;
        ii -= 1;
    }
    Ok(kk)
}

/// Reflects `Rw[nr-1..=span-p]` (descending) into `Qw[n+1..]`.
fn reflect_end(
    qw: &mut [CPoint],
    r: &Curve,
    n: isize,
    p: isize,
    span: isize,
    pln: Plane,
    op: &'static str,
) -> Result<()> {
    let rw = r.cpts();
    let nr = sx(r.n(), op)?;
    let mut kk = n + 1;
    let mut ii = nr - 1;
    while ii >= span - p {
        set(qw, kk, cpt_reflect(get(rw, ii, op)?, pln)?, op)?;
        kk += 1;
        ii -= 1;
    }
    Ok(())
}

/// Start knots of a reflected (`NL_G2`) start extension of `ST_curxcdr`: `UQ[0..=p] =
/// 2 U[0] - uu`, then `2 U[0] - U[ii]` for `ii = span` down to `p + 1`. Returns the
/// next knot index `kq`.
fn start_mirror_knots(
    uq: &mut [f64],
    up: &[f64],
    p: isize,
    span: isize,
    uu: f64,
    op: &'static str,
) -> Result<isize> {
    let dd = 2.0 * get(up, 0, op)?;
    for ii in 0..=p {
        set(uq, ii, dd - uu, op)?;
    }
    let mut kq = p + 1;
    let mut ii = span;
    while ii > p {
        set(uq, kq, dd - get(up, ii, op)?, op)?;
        kq += 1;
        ii -= 1;
    }
    Ok(kq)
}

/// End knots of a reflected (`NL_G2`) end extension of `ST_curxcdr`: `UQ[mq-p..=mq] =
/// 2 U[m] - uu`, then `2 U[m] - UR[ii]` for `ii = span + 1 .. mr - p` stored downwards
/// from `UQ[mq-p-1]`.
fn end_mirror_knots(
    uq: &mut [f64],
    up: &[f64],
    ur: &[f64],
    p: isize,
    m: isize,
    mr: isize,
    mq: isize,
    span: isize,
    uu: f64,
    op: &'static str,
) -> Result<()> {
    let dd = 2.0 * get(up, m, op)?;
    for ii in 0..=p {
        set(uq, mq - p + ii, dd - uu, op)?;
    }
    let mut kq = mq - p - 1;
    for ii in span + 1..mr - p {
        set(uq, kq, dd - get(ur, ii, op)?, op)?;
        kq -= 1;
    }
    Ok(())
}

/// Copies the original control points `Pw[0..=n]` to `Qw[kk..]` and the knots
/// `UP[kp..kp+m]` to `UQ[kq..]` ("load remaining control points / knots").
fn copy_rest(
    qw: &mut [CPoint],
    uq: &mut [f64],
    pw: &[CPoint],
    up: &[f64],
    n: isize,
    m: isize,
    kk: &mut isize,
    kq: &mut isize,
    kp: &mut isize,
    op: &'static str,
) -> Result<()> {
    for ii in 0..=n {
        set(qw, *kk, get(pw, ii, op)?, op)?;
        *kk += 1;
    }
    for _ in 1..=m {
        set(uq, *kq, get(up, *kp, op)?, op)?;
        *kq += 1;
        *kp += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line3(p: usize) -> Curve {
        // A straight cubic-ish curve along x with uniform interior knots.
        let n = p + 2;
        let pw: Vec<CPoint> = (0..=n)
            .map(|i| CPoint::new(i as f64, 0.0, 0.0, NOW))
            .collect();
        let mut u = vec![0.0; p + 1];
        u.push(1.0 / 3.0);
        u.push(2.0 / 3.0);
        u.extend(std::iter::repeat_n(1.0, p + 1));
        Curve::new(pw, p, u).unwrap()
    }

    fn arc() -> Curve {
        // Quarter circle, rational quadratic, 3D.
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let pw = vec![
            CPoint::new(1.0, 0.0, 0.0, 1.0),
            CPoint::new(w, w, 0.0, w),
            CPoint::new(0.0, 1.0, 0.0, 1.0),
        ];
        Curve::new(pw, 2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).unwrap()
    }

    #[test]
    fn eval_pt_is_homogeneous() {
        let c = arc();
        let q = c.eval_pt(0.5, Side::Left).unwrap();
        let e = c.eval(0.5, Side::Left).unwrap();
        assert!((q.x / q.w - e.x).abs() < 1e-15);
        assert!((q.y / q.w - e.y).abs() < 1e-15);
        assert!(matches!(
            c.eval_pt(1.5, Side::Left),
            Err(SolidError::Nlib {
                code: NlibCode::Parameter,
                ..
            })
        ));
    }

    #[test]
    fn derivs_at_knot_rows() {
        let c = line3(3);
        let d = c.derivs_at_knot(1.0 / 3.0, Side::Right, 4).unwrap();
        assert_eq!(d.len(), 5);
        assert_eq!(d[4], CPoint::new(0.0, 0.0, 0.0, NOW));
        let p0 = c.eval_pt(1.0 / 3.0, Side::Left).unwrap();
        assert!((d[0].x - p0.x).abs() < 1e-14);
    }

    #[test]
    fn even_spaced_points() {
        let c = arc();
        let r = c.eval_even_spaced_pts(0.0, 1.0, 8, 1e-6).unwrap();
        assert_eq!(r.points.len(), 9);
        assert_eq!(r.params[0], 0.0);
        assert_eq!(r.params[8], 1.0);
        let chords: Vec<f64> = r
            .points
            .windows(2)
            .map(|w| dist_pt_pt(w[0], w[1]))
            .collect();
        let aver = chords.iter().sum::<f64>() / 8.0;
        assert!(chords.iter().all(|c| (c - aver).abs() <= 1e-6 * aver));
        assert!(matches!(
            c.eval_even_spaced_pts(0.0, 1.0, 1, 1e-6),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn max_second_derivative_of_line_is_small() {
        let c = line3(2);
        assert!(c.max_2nd_deriv().unwrap() < 1e-9 + 100.0);
        let a = arc();
        assert!(a.max_2nd_deriv().unwrap() > 0.0);
    }

    #[test]
    fn planar_offset_of_circle() {
        let a = arc();
        let (o1, _) = a
            .planar_offset_derivs(0.5, 0.5, Vec3::new(0.0, 0.0, 1.0), Side::Left)
            .unwrap();
        let d = a.derivs(0.5, Side::Left, 1).unwrap();
        // Offsetting a unit circle outward/inward scales the tangent by 1 -+ 0.5.
        let ratio = o1.length() / d[1].length();
        assert!((ratio - 1.5).abs() < 1e-9 || (ratio - 0.5).abs() < 1e-9);
    }

    #[test]
    fn extensions_keep_the_original_curve() {
        let c = line3(3);
        for cont in [Continuity::G1, Continuity::G2, Continuity::Cmax] {
            for end in [CurveEnd::Start, CurveEnd::End] {
                let q = c
                    .extend_by_dist_at_depth(0.5, ToleranceKind::Relative, end, cont, 0)
                    .unwrap();
                for u in [0.1, 0.5, 0.9] {
                    let a = c.eval(u, Side::Left).unwrap();
                    let b = q.eval(u, Side::Left).unwrap();
                    assert!((a - b).length() < 1e-6, "{cont:?} {end:?} {u}");
                }
                let par = if end == CurveEnd::Start { -0.4 } else { 1.4 };
                let q = c.extend_by_param_dist_at_depth(par, end, cont, 0).unwrap();
                let (a, b) = q.param_bounds();
                assert!(a == par || b == par, "{cont:?} {end:?}");
            }
        }
        assert!(matches!(
            c.extend_by_dist_at_depth(
                1.0,
                ToleranceKind::Absolute,
                CurveEnd::End,
                Continuity::G1,
                51
            ),
            Err(SolidError::NotConverged { .. })
        ));
        assert_eq!(Continuity::from_flag(4), Some(Continuity::Cmax));
        assert_eq!(CurveEnd::from_flag(2).map(CurveEnd::flag), Some(2));
    }

    /// Fuzz regressions: a degree near `i64::MAX` overflowed the index arithmetic of
    /// the extension routines, and a vanishing `NL_G2` extension searched practically
    /// forever.
    #[test]
    fn extensions_reject_huge_degree_and_vanishing_g2_distance() {
        let c = line3(3);
        for p in [1usize << 31, i64::MAX as usize, usize::MAX] {
            let h = Curve::new(c.cpts().to_vec(), p, c.knots().to_vec()).unwrap();
            for cont in [Continuity::G1, Continuity::G2, Continuity::Cmax] {
                let r =
                    h.extend_by_dist_at_depth(0.5, ToleranceKind::Relative, CurveEnd::End, cont, 0);
                assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
                let r = h.extend_by_param_dist_at_depth(-0.5, CurveEnd::Start, cont, 0);
                assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
            }
        }
        for end in [CurveEnd::Start, CurveEnd::End] {
            let r =
                c.extend_by_dist_at_depth(1e-300, ToleranceKind::Relative, end, Continuity::G2, 0);
            assert!(matches!(r, Err(SolidError::NotConverged { .. })), "{r:?}");
        }
    }
}
