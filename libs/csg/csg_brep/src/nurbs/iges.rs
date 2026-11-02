//! IGES NURBS import: builds clamped NLib curves and surfaces from IGES entity 126
//! (rational B-spline curve) and 128 (rational B-spline surface) data, clamping them
//! to their parameter bounds and reducing knot multiplicities.
//!
//! Upstream: `source/SMLib/src/NL_Iges.cpp`; declarations in
//! `source/SMLib/inc/NL_Iges.h`.
//!
//! | Rust                       | Upstream                 |
//! |----------------------------|--------------------------|
//! | [`Entity126`]              | the arguments of `N_Iges126Crv` |
//! | [`iges_126_curve`]         | `N_Iges126Crv`           |
//! | [`clamp_curve`]            | `N_IgesClampCrv`         |
//! | [`correct_curve_knots`]    | `N_IgesCorrectCrvKnots`  |
//! | [`validate`]               | `N_IgesValidate`         |
//! | [`Entity128`]              | the arguments of `N_Iges128Srf` |
//! | [`iges_128_surface`]       | `N_Iges128Srf`           |
//! | [`clamp_surface`]          | `N_IgesClampSrf`         |
//! | [`correct_surface_knots`]  | `N_IgesCorrectSrfKnots`  |
//! | `clamp_dir` (private)      | the clamping loops shared by `N_IgesClampCrv` / `N_IgesClampSrf` |
//! | `correct_dir` (private)    | the knot loops shared by `N_IgesCorrectCrvKnots` / `N_IgesCorrectSrfKnots` |
//!
//! Deviations:
//! - `N_Iges126Crv` / `N_Iges128Srf` fill a caller-supplied `NL_CURVE` /
//!   `NL_SURFACE` (always NULL-initialised by the SMLib callers); the ports return a
//!   new [`Curve`] / [`Surface`], and take the entity data as one [`Entity126`] /
//!   [`Entity128`] instead of six / ten positional arguments. The `NL_STACKS` arguments have no counterpart and
//!   allocation cannot fail.
//! - Out-params become return values; errors are [`SolidError::Nlib`] values with the
//!   upstream `NL_*_ERR` number instead of a flag plus the global `NL_ERROR` record.
//!   The clamp routines' "no control points left" failure (`n < 0`), which upstream
//!   flags without an error number, is [`SolidError::Degenerate`].
//! - Input arrays shorter than the counts and degrees say, and inputs that would make
//!   upstream index outside its arrays (possible only when the routines' documented
//!   preconditions are broken, e.g. clamping an unvalidated curve), are
//!   `InvalidInput` errors instead of out-of-bounds accesses.
//! - The u and v passes over a surface net run the same code as the curve passes on
//!   rows (u) or on the transposed net (v); every control point goes through the same
//!   arithmetic in the same order as upstream.
//! - A degree above [`MAX_SHORT_DEGREE`](crate::nurbs::bezier::MAX_SHORT_DEGREE)
//!   (impossible upstream, where `NL_DEGREE` is a `short`) is
//!   [`SolidError::InvalidInput`], so the clamp and knot-correction index arithmetic
//!   cannot overflow.

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::bezier::check_short_degree;
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::knots::MAX_DEGREE;
use crate::nurbs::math::{CPoint, NOW, NOZ};
use crate::nurbs::srf_basic::{Surface, WEIGHT_MAX, WEIGHT_MIN};

const OP_126: &str = "nurbs::iges::iges_126_curve";
const OP_128: &str = "nurbs::iges::iges_128_surface";
const OP_VALIDATE: &str = "nurbs::iges::validate";
const OP_CLAMP: &str = "nurbs::iges::clamp";
const OP_CORRECT: &str = "nurbs::iges::correct_knots";

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

fn oob(operation: &'static str) -> SolidError {
    invalid(operation, "index outside the arrays")
}

/// `a[i]` for a signed index.
fn at<T: Clone>(a: &[T], i: isize, op: &'static str) -> Result<T> {
    usize::try_from(i)
        .ok()
        .and_then(|i| a.get(i))
        .cloned()
        .ok_or_else(|| oob(op))
}

/// `&mut a[i]` for a signed index.
fn at_mut<'a, T>(a: &'a mut [T], i: isize, op: &'static str) -> Result<&'a mut T> {
    usize::try_from(i)
        .ok()
        .and_then(|i| a.get_mut(i))
        .ok_or_else(|| oob(op))
}

fn signed(i: usize, op: &'static str) -> Result<isize> {
    isize::try_from(i).map_err(|_| invalid(op, "index too large"))
}

fn unsigned(i: isize, op: &'static str) -> Result<usize> {
    usize::try_from(i).map_err(|_| oob(op))
}

/// Transposes a rectangular net (`net[i][j]` -> `out[j][i]`).
fn transpose(net: &[Vec<CPoint>]) -> Vec<Vec<CPoint>> {
    let cols = net.first().map_or(0, Vec::len);
    (0..cols)
        .map(|j| {
            net.iter()
                .map(|row| row.get(j).copied().unwrap_or_default())
                .collect()
        })
        .collect()
}

/// `alf*A + oma*B` element by element (`N_Combine2CPts` per net entry).
fn combine_rows(alf: f64, a: &[CPoint], oma: f64, b: &[CPoint]) -> Vec<CPoint> {
    a.iter()
        .zip(b)
        .map(|(a, b)| CPoint::combine2(alf, *a, oma, *b))
        .collect()
}

/// `A + B` element by element (`N_Sum2CPts` per net entry).
fn sum_rows(a: &[CPoint], b: &[CPoint]) -> Vec<CPoint> {
    a.iter().zip(b).map(|(a, b)| CPoint::sum(*a, *b)).collect()
}

/// `d * A` element by element (`N_ScaleCPt` per net entry).
fn scale_rows(d: f64, a: &[CPoint]) -> Vec<CPoint> {
    a.iter().map(|a| a.scale(d)).collect()
}

// ---- validation ---------------------------------------------------------------------

/// Validates IGES knots `u[0..=m]` of degree `p` against the bounds `v`
/// (`N_IgesValidate`): `U[p] <= v[0] < v[1] <= U[m-p]` and the knots are
/// non-decreasing.
///
/// Errors with [`NlibCode::Knot`] (`NL_KNT_ERR`) when either check fails, and with
/// `InvalidInput` when `p > m` or `u` holds fewer than `m + 1` knots.
pub fn validate(u: &[f64], m: usize, p: usize, v: [f64; 2]) -> Result<()> {
    let knots = u
        .get(..=m)
        .ok_or_else(|| invalid(OP_VALIDATE, "fewer than m + 1 knots"))?;
    let mp = m
        .checked_sub(p)
        .ok_or_else(|| invalid(OP_VALIDATE, "degree above the highest knot index"))?;
    let up = knots.get(p).copied().ok_or_else(|| oob(OP_VALIDATE))?;
    let ump = knots.get(mp).copied().ok_or_else(|| oob(OP_VALIDATE))?;
    if up > v[0] || v[0] >= v[1] || ump < v[1] {
        return Err(nlib(OP_VALIDATE, NlibCode::Knot));
    }
    for pair in knots.windows(2) {
        if let [a, b] = pair {
            if a > b {
                return Err(nlib(OP_VALIDATE, NlibCode::Knot));
            }
        }
    }
    Ok(())
}

// ---- clamping -----------------------------------------------------------------------

/// Which "already clamped" test a clamp pass uses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClampRule {
    /// `N_IgesClampCrv`: also requires `span1 >= p`.
    Curve,
    /// `N_IgesClampSrf`.
    Surface,
}

/// One clamping pass (the body of `N_IgesClampCrv`, and of each direction of
/// `N_IgesClampSrf`) over knots `up[0..=m]` (`m = up.len() - 1`) and control points
/// `pw` of degree `p`, to the bounds `b`.
///
/// Junk end knots are overwritten in `up` in place, as upstream. Returns `None` when
/// the knots are then already clamped, else the new control points and knots.
fn clamp_dir<T: Clone>(
    up: &mut [f64],
    pw: &[T],
    p: usize,
    b: [f64; 2],
    rule: ClampRule,
    comb: impl Fn(f64, &T, f64, &T) -> T,
) -> Result<Option<(Vec<T>, Vec<f64>)>> {
    const OP: &str = OP_CLAMP;
    let m = up
        .len()
        .checked_sub(1)
        .ok_or_else(|| invalid(OP, "empty knot vector"))?;
    let m = signed(m, OP)?;
    check_short_degree(p, OP)?;
    let p = signed(p, OP)?;

    // Find knot spans and set multiplicities of bounds (as knots).
    let (mut span1, mut span2) = (0_isize, 0_isize);
    let mut mult1 = 0_isize;
    for i in 0..=m {
        let ui = at(up, i, OP)?;
        if ui == b[0] {
            mult1 += 1;
        }
        if ui > b[0] {
            span1 = i - 1;
            break;
        }
    }
    let mut mult2 = 0_isize;
    for i in (0..=m).rev() {
        let ui = at(up, i, OP)?;
        if ui == b[1] {
            mult2 += 1;
        }
        if ui < b[1] {
            span2 = i + mult2;
            break;
        }
    }

    // Handle the case that junk is in the first and last knots.
    if span1 >= p && mult1 == span1 {
        *at_mut(up, 0, OP)? = b[0];
        mult1 += 1;
    }
    if span2 == m - 1 && mult2 >= p {
        *at_mut(up, m, OP)? = b[1];
        mult2 += 1;
        span2 += 1;
    }

    // Check if it is already properly clamped (at the ends).
    let clamped = match rule {
        ClampRule::Curve => span1 >= p && mult1 == span1 + 1 && span2 == m && mult2 > p,
        ClampRule::Surface => mult1 == span1 + 1 && span2 == m && mult2 > p,
    };
    if clamped {
        return Ok(None);
    }

    // New indices.
    let is = span1 - p;
    let ie = span2 - mult2;
    let n = ie - is;
    let mq = span2 - span1 - mult2 + 2 * p + 1;
    if n < 0 {
        return Err(SolidError::Degenerate {
            operation: OP,
            entity: None,
        });
    }
    let len_q = unsigned(n, OP)? + 1;
    let len_uq = unsigned(mq, OP)?.checked_add(1).ok_or_else(|| oob(OP))?;

    // Initial control points.
    let mut qw = Vec::with_capacity(len_q);
    for i in is..=ie {
        qw.push(at(pw, i, OP)?);
    }

    // Insert the left knot.
    let ll = span1 - p;
    for i in 1..=(p - mult1) {
        for j in 0..=(p - i - mult1) {
            let left = at(up, ll + i + j, OP)?;
            let alf = (b[0] - left) / (at(up, span1 + j + 1, OP)? - left);
            let oma = 1.0 - alf;
            let a = at(&qw, j + 1, OP)?;
            let slot = at_mut(&mut qw, j, OP)?;
            *slot = comb(alf, &a, oma, slot);
        }
    }

    // Insert the right knot.
    let lr = span2 - p;
    let lk = n - p + mult2;
    for i in 1..=(p - mult2) {
        for j in (0..=(p - i - mult2)).rev() {
            let k = lk + i + j;
            let mut left = at(up, lr + i + j, OP)?;
            if left < b[0] {
                left = b[0];
            }
            let alf = (b[1] - left) / (at(up, span2 + j + 1, OP)? - left);
            let oma = 1.0 - alf;
            let prev = at(&qw, k - 1, OP)?;
            let slot = at_mut(&mut qw, k, OP)?;
            *slot = comb(alf, slot, oma, &prev);
        }
    }

    // Load the knot vector.
    let mut uq = Vec::with_capacity(len_uq);
    uq.extend(core::iter::repeat_n(b[0], unsigned(p, OP)? + 1));
    for i in (span1 + 1)..=(span2 - mult2) {
        uq.push(at(up, i, OP)?);
    }
    uq.extend(core::iter::repeat_n(b[1], unsigned(p, OP)? + 1));
    if uq.len() != len_uq {
        return Err(oob(OP));
    }
    Ok(Some((qw, uq)))
}

/// Clamps an IGES curve in place to its bounds `v` (`N_IgesClampCrv`). Assumes
/// `m = n + p + 1`, non-decreasing knots and `U[p] <= v[0] < v[1] <= U[m-p]` (see
/// [`validate`]).
///
/// Junk end knots (`U[0]` below a `p`-fold `v[0]`, `U[m]` above a `p`-fold `v[1]`)
/// are replaced by the bounds; a curve that is then clamped is left as is, otherwise
/// the bounds are inserted as `(p+1)`-fold end knots and the curve gets new, exactly
/// sized storage.
///
/// Errors with [`SolidError::Degenerate`] when no control point would remain, and
/// with `InvalidInput` when the preconditions are broken so far that upstream would
/// index outside its arrays.
pub fn clamp_curve(cur: &mut Curve, v: [f64; 2]) -> Result<()> {
    let p = cur.degree();
    let pw = cur.cpts().to_vec();
    let comb = |alf: f64, a: &CPoint, oma: f64, b: &CPoint| CPoint::combine2(alf, *a, oma, *b);
    if let Some((qw, uq)) = clamp_dir(cur.knots_mut(), &pw, p, v, ClampRule::Curve, comb)? {
        *cur = Curve::new(qw, p, uq)?;
    }
    Ok(())
}

/// Clamps an IGES surface in place to its bounds `ub` (u) and `vb` (v)
/// (`N_IgesClampSrf`): first in u, then in v, each pass as in [`clamp_curve`] (the
/// "already clamped" test does not require `span1 >= p` here, as upstream).
///
/// Errors as [`clamp_curve`].
pub fn clamp_surface(sur: &mut Surface, ub: [f64; 2], vb: [f64; 2]) -> Result<()> {
    let (p, q) = sur.degrees();
    let comb = |alf: f64, a: &Vec<CPoint>, oma: f64, b: &Vec<CPoint>| combine_rows(alf, a, oma, b);

    let rows = sur.cpts();
    if let Some((qrows, uq)) = clamp_dir(sur.u_knots_mut(), &rows, p, ub, ClampRule::Surface, comb)?
    {
        let v = sur.v_knots().to_vec();
        *sur = Surface::new(qrows, p, q, uq, v)?;
    }

    let cols = transpose(&sur.cpts());
    if let Some((qcols, vq)) = clamp_dir(sur.v_knots_mut(), &cols, q, vb, ClampRule::Surface, comb)?
    {
        let u = sur.u_knots().to_vec();
        *sur = Surface::new(transpose(&qcols), p, q, u, vq)?;
    }
    Ok(())
}

// ---- knot correction ----------------------------------------------------------------

/// One knot-correction pass (the body of `N_IgesCorrectCrvKnots`, and of each
/// direction of `N_IgesCorrectSrfKnots`) over knots `up[0..=m]` and control points
/// `pw[0..=n]` of degree `p`, in place. Returns the new `(n, m)`.
fn correct_dir<T: Clone>(
    up: &mut [f64],
    m: usize,
    pw: &mut [T],
    n: usize,
    p: usize,
    sum: impl Fn(&T, &T) -> T,
    scale: impl Fn(f64, &T) -> T,
) -> Result<(usize, usize)> {
    const OP: &str = OP_CORRECT;
    let mut nn = signed(n, OP)?;
    let mut mm = signed(m, OP)?;
    check_short_degree(p, OP)?;
    let p = signed(p, OP)?;

    // Eliminate internal multiplicities greater than p.
    let us = at(up, 0, OP)?;
    let ue = at(up, mm, OP)?;
    let mut ii = p + 1;
    while at(up, ii, OP)? == at(up, ii - 1, OP)? {
        ii += 1;
    }
    while at(up, ii, OP)? < ue {
        // Multiplicity of UP[ii].
        let mut mult = 1;
        while at(up, ii, OP)? == at(up, ii + mult, OP)? {
            mult += 1;
        }
        if mult > p {
            // Reduce to multiplicity p.
            let mut k1 = ii - 1; // first control point to replace
            let k2 = k1 + mult - p; // last control point to replace
            let k3 = k2 - k1;
            for jj in (k1 + 1)..=k2 {
                let a = at(pw, k1, OP)?;
                let b = at(pw, jj, OP)?;
                *at_mut(pw, k1, OP)? = sum(&a, &b);
            }
            let d = 1.0 / (k3 as f64 + 1.0);
            let a = at(pw, k1, OP)?;
            *at_mut(pw, k1, OP)? = scale(d, &a);
            k1 += 1;
            for jj in (k2 + 1)..=nn {
                let a = at(pw, jj, OP)?;
                *at_mut(pw, k1, OP)? = a;
                k1 += 1;
            }
            nn -= k3;
            k1 = ii + p;
            for jj in (ii + mult)..=mm {
                let a = at(up, jj, OP)?;
                *at_mut(up, k1, OP)? = a;
                k1 += 1;
            }
            mm -= k3;
            mult = p;
        }
        ii += mult;
    }

    // Validate that the multiplicity of UP[0] is not greater than p+1.
    let mut mult = 0;
    while us == at(up, p + mult + 1, OP)? {
        mult += 1;
    }
    if mult > 0 {
        let mut k1 = p + 1;
        for jj in (p + mult + 1)..=mm {
            let a = at(up, jj, OP)?;
            *at_mut(up, k1, OP)? = a;
            k1 += 1;
        }
        mm -= mult;
        k1 = 1;
        for jj in (mult + 1)..=nn {
            let a = at(pw, jj, OP)?;
            *at_mut(pw, k1, OP)? = a;
            k1 += 1;
        }
        nn -= mult;
    }

    // Validate that the multiplicity of UP[mm] is not greater than p+1.
    let mut mult = 0;
    while ue == at(up, mm - p - mult - 1, OP)? {
        mult += 1;
    }
    if mult > 0 {
        mm -= mult;
        let a = at(pw, nn, OP)?;
        *at_mut(pw, nn - mult, OP)? = a;
        nn -= mult;
    }

    Ok((unsigned(nn, OP)?, unsigned(mm, OP)?))
}

/// Reduces the knot multiplicities of a clamped IGES curve in place
/// (`N_IgesCorrectCrvKnots`): interior knots of multiplicity above `p` are reduced to
/// `p` (the replaced control points are averaged), end knots of multiplicity above
/// `p + 1` to `p + 1`. A curve that loses control points gets new, exactly sized
/// storage.
///
/// Errors with `InvalidInput` when the curve is so malformed that upstream would
/// index outside its arrays.
pub fn correct_curve_knots(cur: &mut Curve) -> Result<()> {
    let (n, m) = cur.array_sizes();
    let p = cur.degree();
    let mut up = cur.knots().to_vec();
    let mut pw = cur.cpts().to_vec();
    let (nn, mm) = correct_dir(
        &mut up,
        m,
        &mut pw,
        n,
        p,
        |a, b| CPoint::sum(*a, *b),
        |d, a| a.scale(d),
    )?;
    if nn < n {
        let mut out = Curve::with_indices(pw, nn, p, up, mm)?;
        out.compress();
        *cur = out;
    }
    Ok(())
}

/// Reduces the knot multiplicities of a clamped IGES surface in place
/// (`N_IgesCorrectSrfKnots`): as [`correct_curve_knots`], first in u, then in v.
///
/// Errors as [`correct_curve_knots`].
pub fn correct_surface_knots(sur: &mut Surface) -> Result<()> {
    let idx = sur.indices();
    let mut up = sur.u_knots().to_vec();
    let mut vp = sur.v_knots().to_vec();
    let mut rows = sur.cpts();
    let (nn, rr) = correct_dir(
        &mut up,
        idx.r,
        &mut rows,
        idx.n,
        idx.p,
        |a, b| sum_rows(a, b),
        |d, a| scale_rows(d, a),
    )?;
    rows.truncate(nn + 1);
    let mut cols = transpose(&rows);
    let (mm, ss) = correct_dir(
        &mut vp,
        idx.s,
        &mut cols,
        idx.m,
        idx.q,
        |a, b| sum_rows(a, b),
        |d, a| scale_rows(d, a),
    )?;
    if nn < idx.n || mm < idx.m {
        cols.truncate(mm + 1);
        up.truncate(rr + 1);
        vp.truncate(ss + 1);
        *sur = Surface::new(transpose(&cols), idx.p, idx.q, up, vp)?;
    }
    Ok(())
}

// ---- entities 126 and 128 -----------------------------------------------------------

/// IGES entity 126 (rational B-spline curve) data, the arguments of `N_Iges126Crv`.
#[derive(Debug, Clone, Copy)]
pub struct Entity126<'a> {
    /// Upper index of the control points (`K`; `k + 1` points).
    pub k: usize,
    /// Degree (`M`).
    pub m: usize,
    /// Knots `t[0..=k+m+1]`.
    pub t: &'a [f64],
    /// Weights `w[0..=k]`.
    pub w: &'a [f64],
    /// Euclidean control points `pts[0..=k]`; `z == `[`NOZ`] marks a 2D point.
    pub pts: &'a [Vec3],
    /// Parameter bounds `[V(0), V(1)]`.
    pub v: [f64; 2],
}

/// IGES entity 128 (rational B-spline surface) data, the arguments of
/// `N_Iges128Srf`.
#[derive(Debug, Clone, Copy)]
pub struct Entity128<'a> {
    /// Upper index of the control points in u (`K1`).
    pub k1: usize,
    /// Upper index of the control points in v (`K2`).
    pub k2: usize,
    /// Degree in u (`M1`).
    pub m1: usize,
    /// Degree in v (`M2`).
    pub m2: usize,
    /// u knots `s[0..=k1+m1+1]`.
    pub s: &'a [f64],
    /// v knots `t[0..=k2+m2+1]`.
    pub t: &'a [f64],
    /// Weights `w[i][j]`, `0 <= i <= k1`, `0 <= j <= k2`.
    pub w: &'a [Vec<f64>],
    /// Euclidean control points `pts[i][j]`, indexed as `w`.
    pub pts: &'a [Vec<Vec3>],
    /// u parameter bounds `[U(0), U(1)]`.
    pub u: [f64; 2],
    /// v parameter bounds `[V(0), V(1)]`.
    pub v: [f64; 2],
}

/// Builds a clamped NLib curve from IGES entity 126 data (`N_Iges126Crv`).
///
/// The curve is rational unless all weights equal `w[0]`; it is clamped to `v` with
/// [`clamp_curve`] and its knots are corrected with [`correct_curve_knots`].
///
/// Errors with [`NlibCode::Degree`] for `m` outside `1..=MAX_DEGREE`,
/// [`NlibCode::Weight`] for a weight outside `[WEIGHT_MIN, WEIGHT_MAX]`,
/// [`NlibCode::Knot`] from [`validate`], and as [`clamp_curve`] /
/// [`correct_curve_knots`]; with `InvalidInput` when an array is shorter than `k` and
/// `m` say.
pub fn iges_126_curve(entity: &Entity126<'_>) -> Result<Curve> {
    let Entity126 { k, m, t, w, pts, v } = *entity;
    let pp = m;
    let nn = k;
    let mm = nn
        .checked_add(pp)
        .and_then(|x| x.checked_add(1))
        .ok_or_else(|| invalid(OP_126, "size overflow"))?;

    if !(1..=MAX_DEGREE).contains(&pp) {
        return Err(nlib(OP_126, NlibCode::Degree));
    }
    let w = w
        .get(..=nn)
        .ok_or_else(|| invalid(OP_126, "fewer than k + 1 weights"))?;
    let pts = pts
        .get(..=nn)
        .ok_or_else(|| invalid(OP_126, "fewer than k + 1 control points"))?;
    let t = t
        .get(..=mm)
        .ok_or_else(|| invalid(OP_126, "fewer than k + m + 2 knots"))?;

    let w0 = w.first().copied().unwrap_or(0.0);
    let mut rat = false;
    for &wi in w {
        if wi < WEIGHT_MIN || wi > WEIGHT_MAX {
            return Err(nlib(OP_126, NlibCode::Weight));
        }
        if wi != w0 {
            rat = true;
        }
    }
    validate(t, mm, pp, v)?;

    let pw = pts
        .iter()
        .zip(w)
        .map(|(pt, &ww)| {
            let (mut xx, mut yy, mut zz) = (pt.x, pt.y, pt.z);
            let ww = if rat {
                xx *= ww;
                yy *= ww;
                zz = if zz == NOZ { zz } else { ww * zz };
                ww
            } else {
                NOW
            };
            CPoint::new(xx, yy, zz, ww)
        })
        .collect();
    let mut cur = Curve::new(pw, pp, t.to_vec())?;
    clamp_curve(&mut cur, v)?;
    correct_curve_knots(&mut cur)?;
    Ok(cur)
}

/// Builds a clamped NLib surface from IGES entity 128 data (`N_Iges128Srf`).
///
/// The surface is rational unless all weights equal `w[0][0]` (as upstream, `z` is
/// weighted even when it is [`NOZ`]); it is clamped with [`clamp_surface`] and its
/// knots are corrected with [`correct_surface_knots`].
///
/// Errors with [`NlibCode::Degree`] for a degree outside `1..=MAX_DEGREE`,
/// [`NlibCode::Weight`] for a weight outside `[WEIGHT_MIN, WEIGHT_MAX]`,
/// [`NlibCode::Knot`] from [`validate`] (u first), and as [`clamp_surface`] /
/// [`correct_surface_knots`]; with `InvalidInput` when an array is shorter than the
/// indices and degrees say.
pub fn iges_128_surface(entity: &Entity128<'_>) -> Result<Surface> {
    let Entity128 {
        k1,
        k2,
        m1,
        m2,
        s,
        t,
        w,
        pts,
        u,
        v,
    } = *entity;
    let (pp, qq, nn, mm) = (m1, m2, k1, k2);
    let size = |a: usize, b: usize| {
        a.checked_add(b)
            .and_then(|x| x.checked_add(1))
            .ok_or_else(|| invalid(OP_128, "size overflow"))
    };
    let rr = size(nn, pp)?;
    let ss = size(mm, qq)?;

    if !(1..=MAX_DEGREE).contains(&pp) || !(1..=MAX_DEGREE).contains(&qq) {
        return Err(nlib(OP_128, NlibCode::Degree));
    }
    let w = w
        .get(..=nn)
        .filter(|rows| rows.iter().all(|r| r.len() > mm))
        .ok_or_else(|| invalid(OP_128, "weight net smaller than (k1, k2)"))?;
    let pts = pts
        .get(..=nn)
        .filter(|rows| rows.iter().all(|r| r.len() > mm))
        .ok_or_else(|| invalid(OP_128, "control net smaller than (k1, k2)"))?;
    let s = s
        .get(..=rr)
        .ok_or_else(|| invalid(OP_128, "fewer than k1 + m1 + 2 u knots"))?;
    let t = t
        .get(..=ss)
        .ok_or_else(|| invalid(OP_128, "fewer than k2 + m2 + 2 v knots"))?;

    let w00 = w.first().and_then(|r| r.first()).copied().unwrap_or(0.0);
    let mut rat = false;
    for row in w {
        for &wij in row.get(..=mm).unwrap_or(&[]) {
            if wij < WEIGHT_MIN || wij > WEIGHT_MAX {
                return Err(nlib(OP_128, NlibCode::Weight));
            }
            if wij != w00 {
                rat = true;
            }
        }
    }
    validate(s, rr, pp, u)?;
    validate(t, ss, qq, v)?;

    let net = pts
        .iter()
        .zip(w)
        .map(|(prow, wrow)| {
            prow.iter()
                .zip(wrow)
                .take(mm + 1)
                .map(|(pt, &ww)| {
                    let (mut xx, mut yy, mut zz) = (pt.x, pt.y, pt.z);
                    let ww = if rat {
                        xx *= ww;
                        yy *= ww;
                        zz *= ww;
                        ww
                    } else {
                        NOW
                    };
                    CPoint::new(xx, yy, zz, ww)
                })
                .collect()
        })
        .collect();
    let mut sur = Surface::new(net, pp, qq, s.to_vec(), t.to_vec())?;
    clamp_surface(&mut sur, u, v)?;
    correct_surface_knots(&mut sur)?;
    Ok(sur)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c126(k: usize, m: usize, t: &[f64], w: &[f64], pts: &[Vec3], v: [f64; 2]) -> Result<Curve> {
        iges_126_curve(&Entity126 { k, m, t, w, pts, v })
    }

    fn line_pts(k: usize) -> Vec<Vec3> {
        (0..k).map(|i| Vec3::new(i as f64, 0.0, 0.0)).collect()
    }

    #[test]
    fn validate_checks_bounds_and_order() {
        let u = [0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0];
        assert!(validate(&u, 6, 2, [0.0, 2.0]).is_ok());
        assert!(validate(&u, 6, 2, [0.5, 1.5]).is_ok());
        let knot = Err(nlib(OP_VALIDATE, NlibCode::Knot));
        assert_eq!(validate(&u, 6, 2, [1.0, 1.0]), knot);
        assert_eq!(validate(&u, 6, 2, [-0.5, 1.0]), knot);
        assert_eq!(validate(&u, 6, 2, [0.0, 2.5]), knot);
        let bad = [0.0, 0.0, 1.0, 0.5, 2.0, 2.0];
        assert_eq!(validate(&bad, 5, 1, [0.0, 2.0]), knot);
        assert!(matches!(
            validate(&u, 6, 7, [0.0, 1.0]),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            validate(&u, 9, 2, [0.0, 1.0]),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn curve_126_clean_is_unchanged() {
        let t = [0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        let c = c126(3, 2, &t, &[1.0; 4], &line_pts(4), [0.0, 1.0]).unwrap();
        assert_eq!(c.knots(), &t);
        assert!(!c.is_rational());
        assert_eq!(c.cpts()[2], CPoint::new(2.0, 0.0, 0.0, NOW));
    }

    #[test]
    fn curve_126_rational_and_2d() {
        let t = [0.0, 0.0, 1.0, 1.0];
        let pts = [Vec3::new(1.0, 2.0, NOZ), Vec3::new(3.0, 4.0, NOZ)];
        let c = c126(1, 1, &t, &[1.0, 2.0], &pts, [0.0, 1.0]).unwrap();
        assert_eq!(c.cpts()[1], CPoint::new(6.0, 8.0, NOZ, 2.0));
    }

    #[test]
    fn curve_126_errors() {
        let t = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let pts = line_pts(3);
        let e = |r: Result<Curve>| match r {
            Err(SolidError::Nlib { code, .. }) => Some(code),
            _ => None,
        };
        assert_eq!(
            e(c126(2, 0, &t, &[1.0; 3], &pts, [0.0, 1.0])),
            Some(NlibCode::Degree)
        );
        assert_eq!(
            e(c126(2, 2, &t, &[1.0, 1e-4, 1.0], &pts, [0.0, 1.0])),
            Some(NlibCode::Weight)
        );
        assert_eq!(
            e(c126(2, 2, &t, &[1.0; 3], &pts, [1.0, 0.0])),
            Some(NlibCode::Knot)
        );
        assert!(matches!(
            c126(2, 2, &t[..5], &[1.0; 3], &pts, [0.0, 1.0]),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn clamp_unclamped_curve() {
        // Uniform knots 0..=7, degree 2, 5 points: clamp to [2, 5].
        let u: Vec<f64> = (0..8).map(f64::from).collect();
        let pw: Vec<CPoint> = (0..5)
            .map(|i| CPoint::new(f64::from(i), 0.0, 0.0, NOW))
            .collect();
        let mut c = Curve::new(pw, 2, u).unwrap();
        clamp_curve(&mut c, [2.0, 5.0]).unwrap();
        assert_eq!(c.knots(), &[2.0, 2.0, 2.0, 3.0, 4.0, 5.0, 5.0, 5.0]);
        assert_eq!(c.n(), 4);
        // The clamped curve keeps the original's end points C(2) and C(5).
        assert_eq!(c.cpts()[0], CPoint::new(0.5, 0.0, 0.0, NOW));
        assert_eq!(c.cpts()[4], CPoint::new(3.5, 0.0, 0.0, NOW));
    }

    #[test]
    fn clamp_junk_ends_in_place() {
        let u = vec![-1.0, 0.0, 0.0, 1.0, 1.0, 2.0];
        let pw = vec![CPoint::new(0.0, 0.0, 0.0, NOW); 3];
        let mut c = Curve::new(pw, 2, u).unwrap();
        clamp_curve(&mut c, [0.0, 1.0]).unwrap();
        assert_eq!(c.knots(), &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn correct_reduces_multiplicities() {
        // Interior knot 1 of multiplicity 3 > p = 2, end multiplicity 4 > p + 1.
        let u = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0];
        let pw: Vec<CPoint> = (0..7)
            .map(|i| CPoint::new(f64::from(i), 0.0, 0.0, NOW))
            .collect();
        let mut c = Curve::new(pw, 2, u).unwrap();
        correct_curve_knots(&mut c).unwrap();
        assert_eq!(c.knots(), &[0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 2.0]);
        assert_eq!(c.n(), 4);
        assert_eq!(c.storage(), (5, 8));
    }

    #[test]
    fn correct_rejects_malformed_curve() {
        let u = vec![0.0; 6];
        let pw = vec![CPoint::default(); 3];
        let mut c = Curve::new(pw, 2, u).unwrap();
        assert!(matches!(
            correct_curve_knots(&mut c),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn clamp_degenerate_bounds() {
        let u: Vec<f64> = (0..8).map(f64::from).collect();
        let pw = vec![CPoint::default(); 5];
        let mut c = Curve::new(pw, 2, u).unwrap();
        // Reversed bounds (not validated) leave no control points.
        assert!(clamp_curve(&mut c, [5.0, 2.0]).is_err());
    }

    #[test]
    fn surface_128_clamps_both_directions() {
        let s: Vec<f64> = (0..6).map(f64::from).collect(); // 3 x deg 2, unclamped
        let t = [0.0, 0.0, 1.0, 1.0]; // 2 x deg 1, clamped
        let w = vec![vec![1.0; 2]; 3];
        let pts: Vec<Vec<Vec3>> = (0..3)
            .map(|i| {
                (0..2)
                    .map(|j| Vec3::new(f64::from(i), f64::from(j), 0.0))
                    .collect()
            })
            .collect();
        let sur = iges_128_surface(&Entity128 {
            k1: 2,
            k2: 1,
            m1: 2,
            m2: 1,
            s: &s,
            t: &t,
            w: &w,
            pts: &pts,
            u: [2.0, 3.0],
            v: [0.0, 1.0],
        })
        .unwrap();
        assert_eq!(sur.u_knots(), &[2.0, 2.0, 2.0, 3.0, 3.0, 3.0]);
        assert_eq!(sur.v_knots(), &t);
        assert_eq!(sur.indices().n, 2);
    }

    #[test]
    fn correct_surface_reduces_v() {
        let u = vec![0.0, 0.0, 1.0, 1.0];
        let v = vec![0.0, 0.0, 0.0, 1.0, 1.0];
        let net = vec![vec![CPoint::new(1.0, 1.0, 1.0, NOW); 3]; 2];
        let mut s = Surface::new(net, 1, 1, u, v).unwrap();
        correct_surface_knots(&mut s).unwrap();
        assert_eq!(s.v_knots(), &[0.0, 0.0, 1.0, 1.0]);
        assert_eq!(s.indices().m, 1);
    }

    /// #427 (`UB-250`): upstream `N_IgesClampSrf` writes past its new knot vector for these
    /// knots. The port's checked indexing refuses them instead. Shapes the reference does
    /// handle stay as upstream: a NaN interior knot between clamped ends takes the "already
    /// clamped" path and is kept, so the fork's broader guard (source fork@0a7e06d,
    /// which also refuses infinite and huge knots the reference accepts) is not ported.
    #[test]
    fn clamp_surface_refuses_the_427_knots() {
        // degree 2 in v: knots.len() - 3 control points
        let clamp = |v: Vec<f64>, vb: [f64; 2]| {
            let net = vec![vec![CPoint::new(1.0, 1.0, 1.0, NOW); v.len() - 3]; 2];
            let mut s = Surface::new(net, 1, 2, vec![0.0, 0.0, 1.0, 1.0], v).unwrap();
            clamp_surface(&mut s, [0.0, 1.0], vb).map(|()| s.v_knots().to_vec())
        };
        let refused = |r: Result<Vec<f64>>| matches!(r, Err(SolidError::InvalidInput { .. }));
        let issue = vec![360.0, 360.0, 360.0, f64::NAN, f64::NAN, 540.0, 540.0, 540.0];
        assert!(refused(clamp(issue, [360.0, 540.0])));
        let descending = vec![0.0, 0.0, 0.0, 0.8, 0.2, 1.0, 1.0];
        assert!(refused(clamp(descending, [0.1, 0.9])));
        let good = || vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        for vb in [[-1.0, 2.0], [1.5, 2.0], [f64::NAN, 1.0]] {
            assert!(refused(clamp(good(), vb)), "{vb:?}");
        }
        // As upstream: already clamped, nothing written, the NaN kept.
        let nan = clamp(vec![0.0, 0.0, 0.0, f64::NAN, 1.0, 1.0, 1.0], [0.0, 1.0]).unwrap();
        assert!(nan[3].is_nan());
        assert_eq!(
            clamp(good(), [0.25, 0.75]).unwrap(),
            [0.25, 0.25, 0.25, 0.5, 0.75, 0.75, 0.75]
        );
    }

    /// Fuzz regression: a huge degree overflowed the clamp / knot-correction index
    /// arithmetic (and sized a huge allocation).
    #[test]
    fn huge_degree_is_an_error() {
        let pw: Vec<CPoint> = (0..4)
            .map(|i| CPoint::new(f64::from(i), 0.0, 0.0, NOW))
            .collect();
        let u = vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        for p in [1usize << 31, i64::MAX as usize, usize::MAX] {
            let mut c = Curve::new(pw.clone(), p, u.clone()).unwrap();
            let r = clamp_curve(&mut c, [0.25, 0.75]);
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
            let r = correct_curve_knots(&mut c);
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        }
    }
}
