//! Curve fitting, part 4: least-squares curve approximation with end derivative
//! constraints and its knot vector.
//!
//! Upstream: `source/SMLib/src/NL_CrvFit.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvFit.h`).
//!
//! | Rust                         | Upstream                      |
//! |------------------------------|-------------------------------|
//! | [`approx_end_derivs`]        | `N_FitCrvApproxDerivs`        |
//! | [`knots_end_derivs`]         | `N_FitCalcKnotsEndDerivs`     |
//!
//! Deviations:
//! - As in [`super::p2`]: the points are a slice (`m` is its highest index); the start
//!   and end derivatives `Ds[1..=k]` / `De[1..=l]` are the slices `ds` / `de` (`k` and
//!   `l` are their lengths); the curve and the knot vector are return values (so
//!   `N_CrvSizeArrays` cannot fail); work arrays and banded matrices are row-major
//!   blocks indexed as upstream, with accesses outside a whole block returning
//!   [`SolidError::InvalidInput`]; errors are [`SolidError::Nlib`] values; the
//!   `NL_STACKS` arguments, `N_InitNurbs` / `N_EndNurbs` and the routine-name strings
//!   have no counterpart.
//! - `ROUND(x)` is `(NL_INDEX)(x + 0.5)`, a truncation toward zero, as upstream.

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_basic::basis_derivs;
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::crv_fit::p1::{param_values, ParamMethod};
use crate::nurbs::fit_arrays::{
    alloc, at, at_mut, banded, basis, curve, idx, mget, mset, out_of_range, put, sg, Block,
};
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{blend_pt, float_op_is_bad, CPoint, FloatOp};

const OP_APPROX: &str = "nurbs::crv_fit::p4::approx_end_derivs";
const OP_KNOTS: &str = "nurbs::crv_fit::p4::knots_end_derivs";

/// `ROUND(x)`: `(NL_INDEX)(x + 0.5)`.
fn round(x: f64) -> i64 {
    (x + 0.5) as i64
}

/// `ND[i][j]` of a basis derivative table.
fn nd_at(nd: &[Vec<f64>], i: i64, j: i64, op: &'static str) -> Result<f64> {
    nd.get(idx(i, op)?)
        .and_then(|r| r.get(idx(j, op).ok()?))
        .copied()
        .ok_or_else(|| out_of_range(op))
}

/// `N_ScalePt(alpha, P, &P)`.
fn scale_pt(alpha: f64, p: Vec3) -> Vec3 {
    Vec3::new(alpha * p.x, alpha * p.y, alpha * p.z)
}

/// The knot vector `U[0..=n+p+1]` for approximating the parameters `u[0..=m]` by a
/// curve of degree `p` with control points `0..=n` and `k` start and `l` end derivatives
/// (upstream `N_FitCalcKnotsEndDerivs`): clamped ends, interior knots averaged from
/// `n - k - l + 1` cluster representatives of the parameters.
///
/// # Errors
///
/// [`NlibCode::Input`] when `n > m` (also for empty `u`) or `n <= k + l`;
/// `InvalidInput` for an access outside upstream's arrays.
pub fn knots_end_derivs(u: &[f64], n: usize, p: usize, k: usize, l: usize) -> Result<Vec<f64>> {
    const OP: &str = OP_KNOTS;
    // highest index (-1 without parameters: the `n > m` check rejects it, as upstream)
    let m = sg(u.len(), OP)? - 1;
    let (n, p, k, l) = (sg(n, OP)?, sg(p, OP)?, sg(k, OP)?, sg(l, OP)?);

    // Initialize
    if n > m {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if n <= k + l {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let nc = n - k - l;

    let mut uu = alloc(n + p + 1, 0.0, OP)?;
    for i in 0..=p {
        put(&mut uu, i, at(u, 0, OP)?, OP)?;
        put(&mut uu, n + i + 1, at(u, m, OP)?, OP)?;
    }

    // Compute representatives of clusters of parameters
    let mut uk = alloc(nc, 0.0, OP)?;
    let d = (m as f64 + 1.0) / (nc as f64 + 1.0);
    let mut il: i64 = 0;
    let mut le = -1.0;

    for i in 0..=nc {
        le += d;
        let ih = round(le);

        if il == ih {
            put(&mut uk, i, at(u, il, OP)?, OP)?;
        } else {
            let mut sum = 0.0;
            for j in il..=ih {
                sum += at(u, j, OP)?;
            }
            put(&mut uk, i, sum / (ih as f64 - il as f64 + 1.0), OP)?;
        }

        il = ih + 1;
    }

    // Now compute the knot vector
    let is = 1 - k;
    let ie = nc - p + l;
    let mut r = p;

    for i in is..=ie {
        let js = 0.max(i);
        let je = nc.min(i + p - 1);

        let mut sum = 0.0;
        for j in js..=je {
            sum += at(&uk, j, OP)?;
        }
        r += 1;
        put(&mut uu, r, sum / (je as f64 - js as f64 + 1.0), OP)?;
    }

    Ok(uu)
}

/// Least-squares approximation of the points `a[0..=m]` by a curve of degree `p` with
/// control points `0..=n`, through the first and last point and with the start
/// derivatives `ds` (`Ds[1..=k]`) and end derivatives `de` (`De[1..=l]`), parameters by
/// `par` ([`param_values`]) and the knot vector of [`knots_end_derivs`] (upstream
/// `N_FitCrvApproxDerivs`). The end control points come from the derivatives; the
/// others solve the banded normal equations.
///
/// # Errors
///
/// [`NlibCode::Input`] when `p > n`, `k >= p`, `l >= p`, `m < n` or `n <= k + l`;
/// [`NlibCode::Numerical`] for a zero leading derivative basis value; errors of
/// [`param_values`], [`knots_end_derivs`], [`basis_derivs`],
/// [`KnotVector::basis_i_eval`], [`KnotVector::basis_eval`], of the LU decomposition
/// and of the solve (no points: `m = -1 < n`, [`NlibCode::Input`]); `InvalidInput` for
/// an access outside upstream's arrays.
#[allow(clippy::too_many_lines)]
pub fn approx_end_derivs(
    a: &[Vec3],
    ds: &[Vec3],
    de: &[Vec3],
    n: usize,
    p: usize,
    par: ParamMethod,
) -> Result<Curve> {
    const OP: &str = OP_APPROX;
    // highest index (-1 without points: the `m < n` check rejects it, as upstream)
    let m = sg(a.len(), OP)? - 1;
    let k = sg(ds.len(), OP)?;
    let l = sg(de.len(), OP)?;
    let n = sg(n, OP)?;
    let p = sg(p, OP)?;
    // Ds[i] / De[i] for i >= 1
    let dsi = |i: i64| at(ds, i - 1, OP);
    let dei = |i: i64| at(de, i - 1, OP);

    // Check for error and curve memory
    if p > n {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if k >= p || l >= p {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if k < 0 || l < 0 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if m < n || n <= k + l {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let mut pw = alloc(n, CPoint::default(), OP)?;

    // Get parameters and the knot vector
    let u = param_values(a, par)?;
    let uk = knots_end_derivs(&u, idx(n, OP)?, idx(p, OP)?, idx(k, OP)?, idx(l, OP)?)?;
    let knt = KnotVector::from_slice(&uk)?;

    // Compute end control points based on end derivatives
    let b = basis_derivs(&knt, idx(p, OP)?, at(&u, 0, OP)?, Side::Left, idx(k, OP)?)?;
    let nd = b.derivs;

    put(&mut pw, 0, CPoint::from_point(at(a, 0, OP)?), OP)?;

    for i in 1..=k {
        let mut sw = CPoint::ZERO;
        for j in 0..i {
            sw.blend(nd_at(&nd, i, j, OP)?, at(&pw, j, OP)?);
        }
        let sk = sw.to_euclid();
        let mut d = dsi(i)? - sk;

        let ndii = nd_at(&nd, i, i, OP)?;
        if float_op_is_bad(1.0, ndii, FloatOp::Division) {
            return Err(SolidError::nlib(OP, NlibCode::Numerical));
        }
        let fac = 1.0 / ndii;

        d = scale_pt(fac, d);
        put(&mut pw, i, CPoint::from_point(d), OP)?;
    }

    let b = basis_derivs(&knt, idx(p, OP)?, at(&u, m, OP)?, Side::Left, idx(l, OP)?)?;
    let nd = b.derivs;

    put(&mut pw, n, CPoint::from_point(at(a, m, OP)?), OP)?;

    for i in 1..=l {
        let mut sw = CPoint::ZERO;
        for j in 0..i {
            sw.blend(
                nd_at(&nd, i, p - i + j + 1, OP)?,
                at(&pw, n - i + j + 1, OP)?,
            );
        }
        let sl = sw.to_euclid();
        let mut d = dei(i)? - sl;

        let ndpi = nd_at(&nd, i, p - i, OP)?;
        if float_op_is_bad(1.0, ndpi, FloatOp::Division) {
            return Err(SolidError::nlib(OP, NlibCode::Numerical));
        }
        let fac = 1.0 / ndpi;

        d = scale_pt(fac, d);
        put(&mut pw, n - i, CPoint::from_point(d), OP)?;
    }

    if n == k + l + 1 {
        return curve(pw, p, uk, OP);
    }

    // Get the Rk's
    let mut rk = alloc(m - 2, Vec3::ZERO, OP)?;

    for i in 1..m {
        let ui = at(&u, i, OP)?;
        let mut sk = Vec3::ZERO;
        for j in 0..=k {
            let nk = knt.basis_i_eval(idx(j, OP)?, idx(p, OP)?, ui, Side::Left)?;
            let d = at(&pw, j, OP)?.to_euclid();
            blend_pt(&mut sk, nk, d);
        }

        let mut sl = Vec3::ZERO;
        for j in 0..=l {
            let nl = knt.basis_i_eval(idx(n - j, OP)?, idx(p, OP)?, ui, Side::Left)?;
            let d = at(&pw, n - j, OP)?.to_euclid();
            blend_pt(&mut sl, nl, d);
        }

        let d = at(a, i, OP)? - sk;
        put(&mut rk, i - 1, d - sl, OP)?;
    }

    // Compute coefficient matrix
    let bw = 2 * p + 1;
    let sbw = p;
    let mut rj = p;
    let mut sj = p - k - 1;
    let mut ej = -2 - k;
    let ndn = n - k - l - 2;

    let mut am = Block::new(m - 2, p, OP)?;
    let mut index = alloc(m - 2, 0_i64, OP)?;
    let mut start = alloc(ndn, 0_i64, OP)?;
    let mut end = alloc(ndn, 0_i64, OP)?;
    let mut cm = banded(ndn, bw, OP)?;

    // NTN and A start zero (zero-filled storage)
    for i in 0..=ndn {
        put(&mut start, i, 0, OP)?;
        put(&mut end, i, m - 2, OP)?;
    }

    for i in 1..m {
        let (j, nb) = basis(&knt, p, at(&u, i, OP)?, OP)?;

        let ll = 0.max(k + 1 - j + p);
        let hk = p.min(n - l - 1 - j + p) - ll;

        for kk in 0..=hk {
            am.set(i - 1, kk, at(&nb, ll + kk, OP)?, OP)?;
        }

        put(&mut index, i - 1, 0.max(j - p - k - 1), OP)?;

        if j > rj {
            for _ in 1..=j - rj {
                sj += 1;
                ej += 1;

                if sj <= ndn {
                    put(&mut start, sj, i - 1, OP)?;
                }
                if ej >= 0 {
                    put(&mut end, ej, i - 2, OP)?;
                }
            }
            rj = j;
        }
    }

    for i in 0..=ndn {
        let lj = 0.max(i - p);
        let hj = ndn.min(i + p);

        for j in lj..=hj {
            let lk = at(&start, i, OP)?.max(at(&start, j, OP)?);
            let hk = at(&end, i, OP)?.min(at(&end, j, OP)?);

            for kk in lk..=hk {
                let ik = at(&index, kk, OP)?;
                let v = mget(&cm, i, j - i + sbw, OP)?
                    + am.get(kk, i - ik, OP)? * am.get(kk, j - ik, OP)?;
                mset(&mut cm, i, j - i + sbw, v, OP)?;
            }
        }
    }

    // Get right hand side of equation
    let mut r = alloc(ndn, Vec3::ZERO, OP)?;

    for j in 0..=ndn {
        let rj = at_mut(&mut r, j, OP)?;
        *rj = Vec3::ZERO;

        let lk = at(&start, j, OP)?;
        let hk = at(&end, j, OP)?;

        for kk in lk..=hk {
            let ik = at(&index, kk, OP)?;
            blend_pt(rj, am.get(kk, j - ik, OP)?, at(&rk, kk, OP)?);
        }
    }

    // LU decompose matrix
    cm.lu_decompose()?;

    // Solve system of linear equations
    let sol = cm.for_back_points(&r)?;
    for (i, s) in (k + 1..).zip(sol) {
        put(&mut pw, i, s, OP)?;
    }

    curve(pw, p, uk, OP)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc(n: usize) -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let t = i as f64 / (n - 1) as f64 * 1.5;
                Vec3::new(t.cos(), t.sin(), 0.1 * t)
            })
            .collect()
    }

    #[test]
    fn knots_are_clamped_and_increasing() {
        let u: Vec<f64> = (0..=10).map(|i| f64::from(i) / 10.0).collect();
        let k = knots_end_derivs(&u, 6, 3, 1, 1).unwrap();
        assert_eq!(k.len(), 6 + 3 + 2);
        assert!(k.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(k[0], 0.0);
        assert_eq!(k[10], 1.0);
    }

    #[test]
    fn approximation_passes_through_the_ends() {
        let a = arc(20);
        let c = approx_end_derivs(&a, &[], &[], 7, 3, ParamMethod::Uniform).unwrap();
        let p0 = c.cpts()[0].to_euclid();
        let pn = c.cpts()[7].to_euclid();
        assert!((p0 - a[0]).length() < 1e-12);
        assert!((pn - a[19]).length() < 1e-12);
    }

    #[test]
    fn no_points_is_an_input_error() {
        let e = approx_end_derivs(&[], &[], &[], 3, 3, ParamMethod::Uniform).unwrap_err();
        assert!(matches!(
            e,
            SolidError::Nlib {
                code: NlibCode::Input,
                ..
            }
        ));
        let e = knots_end_derivs(&[], 0, 3, 0, 0).unwrap_err();
        assert!(matches!(
            e,
            SolidError::Nlib {
                code: NlibCode::Input,
                ..
            }
        ));
    }

    #[test]
    fn input_checks() {
        let a = arc(5);
        assert!(approx_end_derivs(&a, &[], &[], 2, 3, ParamMethod::Uniform).is_err());
        assert!(approx_end_derivs(&a, &[], &[], 6, 3, ParamMethod::Uniform).is_err());
        let d = [Vec3::new(1.0, 0.0, 0.0)];
        assert!(approx_end_derivs(&a, &d, &d, 2, 2, ParamMethod::Uniform).is_err());
    }
}
