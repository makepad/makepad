//! Curve fitting, part 2: global function and curve interpolation (knots from the
//! parameters), least-squares approximation with a given number of control points,
//! weighted and constrained least squares with a given knot vector, knot vectors for
//! interpolation with all first derivatives or with higher end derivatives, the
//! LU-decomposed interpolation matrix, and the control point solves that reuse it.
//!
//! Upstream: `source/SMLib/src/NL_CrvFit.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvFit.h`; flags in `source/SMLib/inc/NL_DataStruct.h`).
//!
//! | Rust                              | Upstream                         |
//! |-----------------------------------|----------------------------------|
//! | [`func_interp_given_params`]      | `N_FitFuncInterpGivenParams`     |
//! | [`interp`]                        | `N_FitCrvInterp`                 |
//! | [`approx_lst_sq`]                 | `N_FitCrvApproxLstSq`            |
//! | [`cpts_with_matrix`]              | `N_FitCrvMatrix`                 |
//! | [`deriv_matrix`]                  | `N_FitCrvDerivMatrix`            |
//! | [`weighted_lst_sq_knots`], [`WeightedData`] | `N_FitCrvWeightedLstSqKnots` |
//! | [`knots_derivs`]                  | `N_FitCalcKnotsDerivs`           |
//! | [`knots_high_end_derivs`]         | `N_FitCalcKnotsHighEndDerivs`    |
//! | [`calc_matrix`], [`MatrixEnds`]   | `N_FitCalcMatrix` (`itp` flag)   |
//! | [`LstSqPoint`]                    | `NL_EPOINT` / `NL_HPOINT` branches using `N_RealMatrixMultiplyPtArray` / `N_RealMatrixMultiplyCPtArray`, `N_Diff2Pts` / `N_Diff2CPts` |
//!
//! Deviations:
//! - As in [`super::p1`], `NL_VOID *` data plus `NL_EPOINT` / `NL_HPOINT` flags become
//!   slices of a [`FitPoint`] type; the `ptp` / `whr` / `itp` values upstream rejects
//!   with `NL_CAL_ERR` / `NL_INP_ERR` cannot be expressed. Highest indices come from
//!   slice lengths (`n` of the data, `k` of the parameters, `s` of the derivative
//!   weights); counts that are not array lengths stay arguments.
//! - Out-params become return values: curves and curve functions are new values
//!   (so `N_CrvSizeArrays` / `N_CFuncSizeArrays` cannot fail), knot vectors are
//!   `Vec<f64>` of exactly the length upstream sets with `N_SetKnotIndex`, control
//!   point arrays are `Vec<CPoint>` (`cm.n() + 1` entries for the solves; `k + 1`
//!   for the degree-1 copy of [`cpts_with_matrix`]), and [`calc_matrix`] returns the
//!   decomposed matrix. Knots upstream leaves unwritten in the caller's buffer
//!   ([`knots_derivs`] for a degree other than 2 or 3) are `0.0`.
//! - Upstream's 2D work arrays and banded matrices are single row-major blocks and are
//!   indexed the same way (`A[i][-1]` is the last entry of row `i - 1`, as in
//!   `N_FitCrvInterp` when a span gives `l == -1`). Accesses outside a whole block or
//!   outside the given slices return [`SolidError::InvalidInput`] instead of
//!   undefined behaviour.
//! - Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number;
//!   outputs upstream leaves partly written on error are not returned. The
//!   `NL_STACKS` arguments and the routine-name strings have no counterpart.
//! - Work arrays that cannot be reserved return [`NlibCode::Memory`] (upstream's
//!   `NL_MEM_ERR` from a failed `N_Malloc`) instead of aborting. Index arithmetic on
//!   caller counts (`n + p + 1`, `p + 1`, `2 * p`, ...) that would overflow `NL_INDEX`
//!   (undefined behaviour upstream) returns [`SolidError::InvalidInput`]; where the
//!   sum is only compared against the knot count, an overflowing sum counts as a
//!   mismatch (`NL_INP_ERR`, as upstream's wrapped sum), and sums whose degree check
//!   comes later are formed after that check (the order is not observable).

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_basic::basis_derivs;
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::crv_fit::p1::{
    interp_knot_vector, knot_vector_approx, param_values, DerivEnd, FitPoint, ParamMethod,
};
use crate::nurbs::fit_arrays::{
    add, alloc, at, at_mut, banded, basis, curve, full, head, hi, idx, mget, mset, mul,
    out_of_range, put, sg, Block,
};
use crate::nurbs::funcs_basic::CurveFunction;
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{blend_pt, translate_sum2_pts, CPoint, RealMatrix};

mod sealed {
    pub trait Sealed {}
    impl Sealed for crate::math::Vec3 {}
    impl Sealed for crate::nurbs::math::CPoint {}
}

/// Data accepted by [`weighted_lst_sq_knots`]: the [`FitPoint`] operations plus
/// the matrix products and differences of its constrained solve.
pub trait LstSqPoint: FitPoint + sealed::Sealed {
    /// `cm * a` (`N_RealMatrixMultiplyPtArray` / `N_RealMatrixMultiplyCPtArray`).
    ///
    /// # Errors
    ///
    /// Those of [`RealMatrix::multiply_points`] / [`RealMatrix::multiply_cpoints`].
    fn mat_mul(cm: &RealMatrix, a: &[Self]) -> Result<Vec<Self>>;
    /// `a - b` (`N_Diff2Pts` / `N_Diff2CPts`).
    fn diff(a: Self, b: Self) -> Self;
}

impl LstSqPoint for Vec3 {
    fn mat_mul(cm: &RealMatrix, a: &[Self]) -> Result<Vec<Self>> {
        cm.multiply_points(a)
    }
    fn diff(a: Self, b: Self) -> Self {
        a - b
    }
}

impl LstSqPoint for CPoint {
    fn mat_mul(cm: &RealMatrix, a: &[Self]) -> Result<Vec<Self>> {
        cm.multiply_cpoints(a)
    }
    fn diff(a: Self, b: Self) -> Self {
        CPoint::diff(a, b)
    }
}

/// Which end derivatives the interpolation matrix of [`calc_matrix`] carries
/// (upstream `itp` flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixEnds {
    /// `NL_NO` (0): points only.
    None,
    /// `NL_START` (1): derivative at the start.
    Start,
    /// `NL_END` (2): derivative at the end.
    End,
    /// `NL_BOTH` (3): derivatives at both ends.
    Both,
}

impl MatrixEnds {
    /// The upstream flag value.
    #[must_use]
    pub fn flag(self) -> i32 {
        match self {
            MatrixEnds::None => 0,
            MatrixEnds::Start => 1,
            MatrixEnds::End => 2,
            MatrixEnds::Both => 3,
        }
    }
}

/// The interpolation matrix of `N_FitFuncInterpGivenParams` / `N_FitCrvInterp` for
/// parameters `u[0..=n]` and degree `p >= 2`, LU-decomposed.
fn interp_matrix(
    knt: &KnotVector<'_>,
    p: i64,
    u: &[f64],
    n: i64,
    op: &'static str,
) -> Result<RealMatrix> {
    // Compute coefficient matrix.
    let bw = 2 * p - 1;
    let sbw = p - 1;
    let mut cm = banded(n, bw, op)?;
    mset(&mut cm, 0, sbw, 1.0, op)?;
    mset(&mut cm, n, sbw, 1.0, op)?;
    for i in 1..n {
        let (j, nb) = basis(knt, p, at(u, i, op)?, op)?;
        let l = j - i - 1;
        for k in 0..=p {
            mset(&mut cm, i, l + k, at(&nb, k, op)?, op)?;
        }
    }

    // LU decompose matrix.
    cm.lu_decompose()?;
    Ok(cm)
}

// ---- interpolation ----------------------------------------------------------------

/// Function of degree `p` interpolating the values `f[0..=n]` at the parameters
/// `u[0..=n]`, with the knot vector of [`interp_knot_vector`] (upstream
/// `N_FitFuncInterpGivenParams`). For `p == 1` the control values are the data.
///
/// # Errors
///
/// [`NlibCode::Input`] when `p > n`; errors of [`KnotVector::basis_eval`], of the LU
/// decomposition and of the solve (`NL_NUM_ERR` for a singular system);
/// `InvalidInput` for empty `f`, fewer than `n + 1` parameters, or `p == 0` (upstream
/// asks `N_SetRealMatrix` for bandwidth `-1`).
pub fn func_interp_given_params(f: &[f64], p: usize, u: &[f64]) -> Result<CurveFunction> {
    const OP: &str = "nurbs::crv_fit::p2::func_interp_given_params";
    let n = hi(f, OP, "no values")?;
    let p = sg(p, OP)?;

    // Check for function memory.
    if p > n {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    // Compute knot vector.
    let u = head(u, n, OP)?;
    let uk = interp_knot_vector(u, idx(p, OP)?)?;

    // Linear splines.
    if p == 1 {
        return CurveFunction::new(f.to_vec(), 1, uk);
    }

    // Compute coefficient matrix, LU decompose it and solve.
    let knt = KnotVector::from_slice(&uk)?;
    let cm = interp_matrix(&knt, p, u, n, OP)?;
    let fu = cm.right_for_back(f)?;
    CurveFunction::new(fu, idx(p, OP)?, uk)
}

/// Curve of degree `p` interpolating the points `a[0..=n]`, with parameters by `par`
/// ([`param_values`]) and the knot vector of [`interp_knot_vector`] (upstream
/// `N_FitCrvInterp`). For `p == 1` the control points are the data points.
///
/// # Errors
///
/// [`NlibCode::Input`] when `p > n`; errors of [`KnotVector::basis_eval`], of the LU
/// decomposition and of the solve; `InvalidInput` for empty `a` or `p == 0`.
pub fn interp(a: &[Vec3], p: usize, par: ParamMethod) -> Result<Curve> {
    const OP: &str = "nurbs::crv_fit::p2::interp";
    let n = hi(a, OP, "no data points")?;
    let p = sg(p, OP)?;

    // Check for curve memory.
    if p > n {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    // Get parameters and compute knot vector.
    let u = param_values(a, par)?;
    let uk = interp_knot_vector(&u, idx(p, OP)?)?;

    // Control points are the data points in case of linear curves.
    if p == 1 {
        let pw = a.iter().map(|q| CPoint::from_point(*q)).collect();
        return curve(pw, p, uk, OP);
    }

    // Compute coefficient matrix, LU decompose it and solve.
    let knt = KnotVector::from_slice(&uk)?;
    let cm = interp_matrix(&knt, p, &u, n, OP)?;
    let pw = cm.for_back_points(a)?;
    curve(pw, p, uk, OP)
}

// ---- approximation ----------------------------------------------------------------

/// Least-squares approximation of the points `a[0..=m]` by a curve of degree `p`
/// with control points `0..=n` through the first and last point, with parameters by
/// `par` ([`param_values`]) and the knot vector of [`knot_vector_approx`] (upstream
/// `N_FitCrvApproxLstSq`).
///
/// # Errors
///
/// [`NlibCode::Input`] when `n < 2`, `p > n`, `m < n` or some interior basis function
/// gets no sample; errors of [`KnotVector::basis_i_eval`],
/// [`KnotVector::basis_eval`], of the LU decomposition and of the solve;
/// `InvalidInput` for empty `a` or `p == 0` where upstream would index outside its
/// arrays.
pub fn approx_lst_sq(a: &[Vec3], n: usize, p: usize, par: ParamMethod) -> Result<Curve> {
    const OP: &str = "nurbs::crv_fit::p2::approx_lst_sq";
    let m = hi(a, OP, "no data points")?;
    let n = sg(n, OP)?;
    let p = sg(p, OP)?;

    // Check for curve memory.
    if n < 2 || p > n || m < n {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let mut pw = alloc(n, CPoint::default(), OP)?;
    let (q0, qm) = (at(a, 0, OP)?, at(a, m, OP)?);
    put(&mut pw, 0, CPoint::from_point(q0), OP)?;
    put(&mut pw, n, CPoint::from_point(qm), OP)?;

    // Get parameters and the knot vector.
    let u = param_values(a, par)?;
    let uk = knot_vector_approx(&u, idx(n, OP)?, idx(p, OP)?)?;
    let knt = KnotVector::from_slice(&uk)?;

    // Get the Rk's.
    let pi = idx(p, OP)?;
    let ni = idx(n, OP)?;
    let mut rk = alloc(m - 2, Vec3::ZERO, OP)?;
    for i in 1..m {
        let ui = at(&u, i, OP)?;
        let n0 = knt.basis_i_eval(0, pi, ui, Side::Left)?;
        let np = knt.basis_i_eval(ni, pi, ui, Side::Left)?;
        put(
            &mut rk,
            i - 1,
            translate_sum2_pts(at(a, i, OP)?, -n0, q0, -np, qm),
            OP,
        )?;
    }

    // Compute coefficient matrix.
    let bw = 2 * p + 1;
    let sbw = p;
    let mut rj = p;
    let mut sj = p - 1;
    let mut ej: i64 = -2;
    let mut b = Block::new(m - 2, p, OP)?;
    let mut index = alloc(m - 2, 0_i64, OP)?;
    let mut start = alloc(n - 2, 0_i64, OP)?;
    let mut end = alloc(n - 2, 0_i64, OP)?;
    let mut cm = banded(n - 2, bw, OP)?;

    // NTN and A[i][p] start zero (zero-filled storage).
    for i in 0..=(p - 1).min(n - 2) {
        put(&mut start, i, 0, OP)?;
    }
    put(&mut end, 0, -2, OP)?;

    for i in 1..m {
        let (j, nb) = basis(&knt, p, at(&u, i, OP)?, OP)?;
        let l = i64::from(j == p);
        let hk = if j == p || j == n { p - 1 } else { p };
        for k in 0..=hk {
            b.set(i - 1, k, at(&nb, l + k, OP)?, OP)?;
        }
        put(&mut index, i - 1, 0.max(j - p - 1), OP)?;
        if j > rj {
            for _ in 1..=j - rj {
                sj += 1;
                ej += 1;
                if sj <= n - 2 {
                    put(&mut start, sj, i - 1, OP)?;
                }
                if ej >= 0 {
                    put(&mut end, ej, i - 2, OP)?;
                }
            }
            rj = j;
        }
    }

    if sj < n - 2 || at(&end, 0, OP)? == -1 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    for i in 0.max(ej + 1)..=n - 2 {
        put(&mut end, i, m - 2, OP)?;
    }

    for i in 0..=n - 2 {
        let lj = 0.max(i - p);
        let hj = (n - 2).min(i + p);
        for j in lj..=hj {
            let lk = at(&start, i, OP)?.max(at(&start, j, OP)?);
            let hk = at(&end, i, OP)?.min(at(&end, j, OP)?);
            for k in lk..=hk {
                let ik = at(&index, k, OP)?;
                let v = mget(&cm, i, j - i + sbw, OP)?
                    + b.get(k, i - ik, OP)? * b.get(k, j - ik, OP)?;
                mset(&mut cm, i, j - i + sbw, v, OP)?;
            }
        }
    }

    // Get right hand side of equation.
    let mut r = alloc(n - 2, Vec3::ZERO, OP)?;
    for i in 1..n {
        let ri = at_mut(&mut r, i - 1, OP)?;
        *ri = Vec3::ZERO;
        let lk = at(&start, i - 1, OP)?;
        let hk = at(&end, i - 1, OP)?;
        for k in lk..=hk {
            let ik = at(&index, k, OP)?;
            blend_pt(ri, b.get(k, i - ik - 1, OP)?, at(&rk, k, OP)?);
        }
    }

    // LU decompose matrix and solve the system of linear equations.
    cm.lu_decompose()?;
    let sol = cm.for_back_points(&r)?;
    for (i, s) in (1..).zip(sol) {
        put(&mut pw, i, s, OP)?;
    }
    curve(pw, p, uk, OP)
}

/// The data of [`weighted_lst_sq_knots`] (upstream `E`, `wp`, `F`, `wd`, `I`).
///
/// A positive weight makes a datum an approximated (unconstrained) one; zero or a
/// negative weight makes it a constraint the curve meets exactly.
#[derive(Debug, Clone, Copy)]
pub struct WeightedData<'a, P> {
    /// Points `E[0..=r]`.
    pub points: &'a [P],
    /// Point weights `wp[0..=r]`.
    pub point_weights: &'a [f64],
    /// First derivatives `F[0..=s]` (vectors of the point type).
    pub derivs: &'a [P],
    /// Derivative weights `wd[0..=s]`; their count sets `s`.
    pub deriv_weights: &'a [f64],
    /// `I[0..=s]`: the derivative `F[j]` is at the point `E[I[j]]`.
    pub deriv_index: &'a [usize],
}

/// Weighted and constrained least-squares approximation of point and first
/// derivative data at the parameters `u` by a curve with control points `0..=n`,
/// degree `p` and knot vector `knt` (upstream `N_FitCrvWeightedLstSqKnots`). Without
/// constraints the weighted normal equations are solved; with constraints the
/// Lagrange multiplier system is solved through matrix inverses.
///
/// # Errors
///
/// [`NlibCode::Input`] when the constraint count `mc + 1` is at least `n`, `mc + n`
/// exceeds the unconstrained count `mu`, `n < 1`, `p > n`, `knt` does not have
/// `n + p + 2` knots, or some basis function gets no unconstrained datum; errors of
/// [`basis_derivs`], of the LU decompositions, inverses, products and solves;
/// `InvalidInput` for empty points, too few weights, derivatives, indices or
/// parameters, or other accesses outside upstream's arrays.
pub fn weighted_lst_sq_knots<P: LstSqPoint>(
    data: &WeightedData<'_, P>,
    u: &[f64],
    knt: &KnotVector<'_>,
    n: usize,
    p: usize,
) -> Result<Curve> {
    const OP: &str = "nurbs::crv_fit::p2::weighted_lst_sq_knots";
    let e = data.points;
    let wp = data.point_weights;
    let f = data.derivs;
    let wd = data.deriv_weights;
    let ii = data.deriv_index;
    let r = hi(e, OP, "no data points")?;
    let s = sg(wd.len(), OP)? - 1;
    let n = sg(n, OP)?;
    let p = sg(p, OP)?;

    // Initialize some parameters.
    let mut rj = p;
    let mut sj = p;
    let mut ej: i64 = -1;
    let mut ru: i64 = -1;
    let mut su: i64 = -1;
    let mut rc: i64 = -1;
    let mut sc: i64 = -1;

    // Get constrained and unconstrained indexes and check error.
    for i in 0..=r {
        if at(wp, i, OP)? > 0.0 {
            ru += 1;
        } else {
            rc += 1;
        }
    }
    for j in 0..=s {
        if at(wd, j, OP)? > 0.0 {
            su += 1;
        } else {
            sc += 1;
        }
    }

    let mknt = sg(knt.last_index(), OP)?;
    let uk = knt.knots();
    let mu = ru + su + 1;
    let mc = rc + sc + 1;

    // An overflowing `mc + n` exceeds `mu + 1`.
    if mc >= n || mc.checked_add(n).is_none_or(|t| t > mu) {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if n < 1 || p > n {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    // An overflowing `n + p + 1` cannot equal `mknt`.
    let mcur = n.checked_add(p).and_then(|t| t.checked_add(1));
    if Some(mknt) != mcur {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    // Check for curve memory.
    let uc = head(uk, mknt, OP)?.to_vec();

    // Allocate memory (zero-filled, as upstream's initialisation loops leave it).
    let mut index = alloc(mu, 0_i64, OP)?;
    let mut start = alloc(n, 0_i64, OP)?;
    let mut end = alloc(n, mu, OP)?;
    let mut w = alloc(mu, 0.0, OP)?;
    let mut nm = Block::new(mu, p, OP)?;
    let mut wn = Block::new(mu, p, OP)?;
    let mut ntwn = full(n, n, n, OP)?;
    let mut mm = if mc >= 0 {
        Some(full(mc, n, n, OP)?)
    } else {
        None
    };

    // Compute N, M and W.
    let mut mu2: i64 = 0;
    let mut mc2: i64 = 0;
    let mut jd: i64 = 0;
    {
        // Records one row of N (unconstrained datum of weight `wt`) or of M.
        let mut row = |vals: &[f64], j: i64, wt: Option<f64>| -> Result<()> {
            if let Some(wt) = wt {
                put(&mut w, mu2, wt, OP)?;
                for k in 0..=p {
                    nm.set(mu2, k, at(vals, k, OP)?, OP)?;
                }
                put(&mut index, mu2, j - p, OP)?;
                if j > rj {
                    for _ in 1..=j - rj {
                        sj += 1;
                        ej += 1;
                        put(&mut start, sj, mu2, OP)?;
                        put(&mut end, ej, mu2 - 1, OP)?;
                    }
                    rj = j;
                }
                mu2 += 1;
            } else {
                let m = mm.as_mut().ok_or_else(|| out_of_range(OP))?;
                for k in 0..=p {
                    mset(m, mc2, j - p + k, at(vals, k, OP)?, OP)?;
                }
                mc2 += 1;
            }
            Ok(())
        };

        for i in 0..=r {
            let nd = basis_derivs(knt, idx(p, OP)?, at(u, i, OP)?, Side::Left, 1)?;
            let j = sg(nd.span, OP)?;
            let nd0 = nd.derivs.first().ok_or_else(|| out_of_range(OP))?;
            let nd1 = nd.derivs.get(1).ok_or_else(|| out_of_range(OP))?;

            let der = jd <= s && i == sg(at(ii, jd, OP)?, OP)?;

            // Unconstrained or constrained point data.
            let wpi = at(wp, i, OP)?;
            row(nd0, j, (wpi > 0.0).then_some(wpi))?;

            if der {
                // Unconstrained or constrained derivative data.
                let wdj = at(wd, jd, OP)?;
                row(nd1, j, (wdj > 0.0).then_some(wdj))?;
                jd += 1;
            }
        }
    }

    if sj < n || at(&end, 0, OP)? == -1 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    // Compute WN, NTWN and LU-decompose NTWN.
    for j in 0..=n {
        let lk = at(&start, j, OP)?;
        let hk = at(&end, j, OP)?;
        for k in lk..=hk {
            let c = j - at(&index, k, OP)?;
            wn.set(k, c, at(&w, k, OP)? * nm.get(k, c, OP)?, OP)?;
        }
    }
    for i in 0..=n {
        for j in 0..=n {
            let lk = at(&start, i, OP)?.max(at(&start, j, OP)?);
            let hk = at(&end, i, OP)?.min(at(&end, j, OP)?);
            for k in lk..=hk {
                let ik = at(&index, k, OP)?;
                let v = mget(&ntwn, i, j, OP)? + nm.get(k, i - ik, OP)? * wn.get(k, j - ik, OP)?;
                mset(&mut ntwn, i, j, v, OP)?;
            }
        }
    }
    ntwn.lu_decompose()?;

    // Compute input dependent data.
    let mut sv = alloc(mu, P::ZERO, OP)?;
    let mut t = alloc(mc.max(0), P::ZERO, OP)?;
    let mut mu2: i64 = 0;
    let mut mc2: i64 = 0;
    let mut jd: i64 = 0;
    for i in 0..=r {
        let der = jd <= s && i == sg(at(ii, jd, OP)?, OP)?;

        if at(wp, i, OP)? > 0.0 {
            // Unconstrained point data.
            put(&mut sv, mu2, at(e, i, OP)?.scaled(at(&w, mu2, OP)?), OP)?;
            mu2 += 1;
        } else {
            // Constrained point data.
            put(&mut t, mc2, at(e, i, OP)?, OP)?;
            mc2 += 1;
        }

        if der {
            if at(wd, jd, OP)? > 0.0 {
                // Unconstrained derivative data.
                put(&mut sv, mu2, at(f, jd, OP)?.scaled(at(&w, mu2, OP)?), OP)?;
                mu2 += 1;
            } else {
                // Constrained derivative data.
                put(&mut t, mc2, at(f, jd, OP)?, OP)?;
                mc2 += 1;
            }
            jd += 1;
        }
    }

    let mut ntws = alloc(n, P::ZERO, OP)?;
    for i in 0..=n {
        let lk = at(&start, i, OP)?;
        let hk = at(&end, i, OP)?;
        let acc = at_mut(&mut ntws, i, OP)?;
        *acc = P::ZERO;
        for k in lk..=hk {
            let ik = at(&index, k, OP)?;
            acc.blend(nm.get(k, i - ik, OP)?, at(&sv, k, OP)?);
        }
    }

    // In no constraints, do weighted approximation.
    let Some(m) = mm else {
        let pw = P::for_back(&ntwn, &ntws)?;
        return curve(pw, p, uc, OP);
    };

    // Get matrices to compute Lagrange multipliers.
    let intwn = ntwn.inverse(true)?;
    let mt = m.transpose()?;
    let mit = intwn.multiply(&mt)?;
    let mut mi = m.multiply(&mit)?;
    let imit = mi.inverse(false)?;

    // Compute control points.
    let intws = P::mat_mul(&intwn, &ntws)?;
    let mut v = P::mat_mul(&m, &intws)?;
    for i in 0..=mc {
        let vi = at_mut(&mut v, i, OP)?;
        *vi = P::diff(*vi, at(&t, i, OP)?);
    }
    let am = P::mat_mul(&imit, &v)?;
    let v = P::mat_mul(&mit, &am)?;
    let mut pw = alloc(n, CPoint::default(), OP)?;
    for i in 0..=n {
        put(
            &mut pw,
            i,
            P::diff(at(&intws, i, OP)?, at(&v, i, OP)?).to_cpoint(),
            OP,
        )?;
    }
    if at(wp, 0, OP)? < 0.0 {
        put(&mut pw, 0, at(e, 0, OP)?.to_cpoint(), OP)?;
    }
    if at(wp, r, OP)? < 0.0 {
        put(&mut pw, n, at(e, r, OP)?.to_cpoint(), OP)?;
    }
    curve(pw, p, uc, OP)
}

// ---- solves with a given matrix ---------------------------------------------------

/// Control points of the curve of degree `p` interpolating the points `a[0..=k]`,
/// with the LU-decomposed interpolation matrix `cm` (upstream `N_FitCrvMatrix`).
/// For `p == 1` the control points are the `k + 1` data points; otherwise
/// `cm.n() + 1` control points.
///
/// # Errors
///
/// Errors of the solve; `InvalidInput` when `a` has fewer than `cm.n() + 1` points.
pub fn cpts_with_matrix<P: FitPoint>(a: &[P], p: usize, cm: &RealMatrix) -> Result<Vec<CPoint>> {
    // Control points are the data points in case of linear curves.
    if p == 1 {
        return Ok(a.iter().map(|q| q.to_cpoint()).collect());
    }

    // Solve system of linear equations.
    if a.len() <= cm.n() {
        return Err(out_of_range("nurbs::crv_fit::p2::cpts_with_matrix"));
    }
    P::for_back(cm, a)
}

/// Control points of the curve of degree `p` with knot vector `knt` interpolating
/// the points `a[0..=k]` and the derivative `t` at the start or end (`whr`), with the
/// LU-decomposed matrix `cm` (upstream `N_FitCrvDerivMatrix`). Returns `cm.n() + 1`
/// control points (`k + 2` for the matching matrix).
///
/// # Errors
///
/// Errors of the solve; `InvalidInput` for empty `a`, too few knots, or a matrix
/// larger than `k + 2` rows.
pub fn deriv_matrix<P: FitPoint>(
    a: &[P],
    p: usize,
    knt: &KnotVector<'_>,
    t: P,
    whr: DerivEnd,
    cm: &RealMatrix,
) -> Result<Vec<CPoint>> {
    const OP: &str = "nurbs::crv_fit::p2::deriv_matrix";
    let k = hi(a, OP, "no data points")?;
    let p = sg(p, OP)?;
    let uk = knt.knots();
    let n = k + 1;

    // Solve system of linear equations.
    let mut q = alloc(n, P::ZERO, OP)?;
    match whr {
        DerivEnd::Start => {
            let fact = (at(uk, add(p, 1, OP)?, OP)? - at(uk, 1, OP)?) / p as f64;
            put(&mut q, 0, at(a, 0, OP)?, OP)?;
            put(&mut q, 1, t.scaled(fact), OP)?;
            for i in 2..=n {
                put(&mut q, i, at(a, i - 1, OP)?, OP)?;
            }
        }
        DerivEnd::End => {
            let fact = (at(uk, add(n, p, OP)?, OP)? - at(uk, n, OP)?) / p as f64;
            put(&mut q, n - 1, t.scaled(fact), OP)?;
            put(&mut q, n, at(a, k, OP)?, OP)?;
            for i in 0..=n - 2 {
                put(&mut q, i, at(a, i, OP)?, OP)?;
            }
        }
    }
    if q.len() <= cm.n() {
        return Err(out_of_range(OP));
    }
    P::for_back(cm, &q)
}

// ---- knot vectors and matrices ----------------------------------------------------

/// Knot vector for global interpolation of points at parameters `u[0..=k]` with all
/// first derivatives, degree `p` (2 or 3) (upstream `N_FitCalcKnotsDerivs`):
/// `2k + p + 3` knots. For other degrees only the `p + 1` end knots are set (the
/// interior is `0.0`, upstream leaves it unwritten).
///
/// # Errors
///
/// `InvalidInput` for empty `u`, or `p == 2` with a single parameter (upstream reads
/// `u[1]`).
pub fn knots_derivs(u: &[f64], p: usize) -> Result<Vec<f64>> {
    const OP: &str = "nurbs::crv_fit::p2::knots_derivs";
    let k = hi(u, OP, "no parameters")?;
    let p = sg(p, OP)?;

    // Compute knot vector.
    let n = 2 * k + 1;
    let mut uk = alloc(add(add(n, p, OP)?, 1, OP)?, 0.0, OP)?;
    let (u0, uk_end) = (at(u, 0, OP)?, at(u, k, OP)?);
    for i in 0..=p {
        put(&mut uk, i, u0, OP)?;
        put(&mut uk, n + i + 1, uk_end, OP)?;
    }

    let mut j = p + 1;
    match p {
        2 => {
            put(&mut uk, p + 1, 0.5 * (at(u, 1, OP)? + at(u, 0, OP)?), OP)?;
            for i in 1..k {
                j += 1;
                put(&mut uk, j, at(u, i, OP)?, OP)?;
                j += 1;
                put(&mut uk, j, 0.5 * (at(u, i, OP)? + at(u, i + 1, OP)?), OP)?;
            }
        }
        3 => {
            if k > 1 {
                put(&mut uk, p + 1, 0.5 * (at(u, 1, OP)? + at(u, 0, OP)?), OP)?;
                put(&mut uk, n, 0.5 * (at(u, k, OP)? + at(u, k - 1, OP)?), OP)?;
            }
            let alf = 1.0 / 3.0;
            let bet = 2.0 / 3.0;
            for i in 1..k - 1 {
                j += 1;
                put(
                    &mut uk,
                    j,
                    bet * at(u, i, OP)? + alf * at(u, i + 1, OP)?,
                    OP,
                )?;
                j += 1;
                put(
                    &mut uk,
                    j,
                    alf * at(u, i, OP)? + bet * at(u, i + 1, OP)?,
                    OP,
                )?;
            }
        }
        _ => {}
    }
    Ok(uk)
}

/// Knot vector for global interpolation of points at parameters `u[0..=n]` with the
/// first through `k`-th derivatives at the start and the first through `l`-th at the
/// end, degree `p` (`k, l < p`) (upstream `N_FitCalcKnotsHighEndDerivs`):
/// `n + k + l + p + 2` knots; each interior knot averages up to `p` consecutive
/// parameters.
///
/// # Errors
///
/// `InvalidInput` for empty `u`.
pub fn knots_high_end_derivs(u: &[f64], p: usize, k: usize, l: usize) -> Result<Vec<f64>> {
    const OP: &str = "nurbs::crv_fit::p2::knots_high_end_derivs";
    let n = hi(u, OP, "no parameters")?;
    let p = sg(p, OP)?;
    let k = sg(k, OP)?;
    let l = sg(l, OP)?;

    // Compute knot vector.
    let m = add(add(n, k, OP)?, l, OP)?;
    let mut uk = alloc(add(add(m, p, OP)?, 1, OP)?, 0.0, OP)?;
    let (u0, un) = (at(u, 0, OP)?, at(u, n, OP)?);
    for i in 0..=p {
        put(&mut uk, i, u0, OP)?;
        put(&mut uk, m + i + 1, un, OP)?;
    }

    let is = 1 - k;
    let ie = n - p + l;
    let mut r = p;
    for i in is..=ie {
        let js = 0.max(i);
        let je = n.min(i + p - 1);
        let mut sum = 0.0;
        for j in js..=je {
            sum += at(u, j, OP)?;
        }
        r += 1;
        put(&mut uk, r, sum / (je as f64 - js as f64 + 1.0), OP)?;
    }
    Ok(uk)
}

/// The LU-decomposed matrix for global interpolation at the parameters `t[0..=n]`
/// with degree `p` and knot vector `knt`, with or without end derivative rows
/// (`itp`) (upstream `N_FitCalcMatrix`). The matrix is banded with highest index
/// `n`, `n + 1` or `n + 2`.
///
/// # Errors
///
/// [`NlibCode::Equations`] when a basis span falls outside the matrix band; errors of
/// [`KnotVector::basis_eval`] and of the LU decomposition; `InvalidInput` for empty
/// `t` or a matrix row upstream would write outside its storage.
pub fn calc_matrix(
    knt: &KnotVector<'_>,
    p: usize,
    t: &[f64],
    itp: MatrixEnds,
) -> Result<RealMatrix> {
    const OP: &str = "nurbs::crv_fit::p2::calc_matrix";
    let n = hi(t, OP, "no parameters")?;
    let p = sg(p, OP)?;

    // Allocate memory.
    let k = match itp {
        MatrixEnds::None => n,
        MatrixEnds::Start | MatrixEnds::End => n + 1,
        MatrixEnds::Both => n + 2,
    };
    let (bw, sbw) = if mul(2, p, OP)? <= k {
        (2 * p + 1, p)
    } else {
        (2 * p - 1, p - 1)
    };
    let sp = sbw - p;
    let mut cm = banded(k, bw, OP)?;

    // Compute matrix entries.
    let band_row = |cm: &mut RealMatrix, i: i64, ti: f64| -> Result<()> {
        let (j, nb) = basis(knt, p, ti, OP)?;
        let l = sp - i + j;
        if l < 0 || l + p >= bw {
            return Err(SolidError::nlib(OP, NlibCode::Equations));
        }
        for kk in 0..=p {
            mset(cm, i, l + kk, at(&nb, kk, OP)?, OP)?;
        }
        Ok(())
    };
    match itp {
        MatrixEnds::None => {
            mset(&mut cm, 0, sbw, 1.0, OP)?;
            mset(&mut cm, n, sbw, 1.0, OP)?;
            for i in 1..n {
                band_row(&mut cm, i, at(t, i, OP)?)?;
            }
        }
        MatrixEnds::Start => {
            mset(&mut cm, 0, sbw, 1.0, OP)?;
            mset(&mut cm, 1, sbw - 1, -1.0, OP)?;
            mset(&mut cm, 1, sbw, 1.0, OP)?;
            mset(&mut cm, n + 1, sbw, 1.0, OP)?;
            for i in 2..=n {
                band_row(&mut cm, i, at(t, i - 1, OP)?)?;
            }
        }
        MatrixEnds::End => {
            mset(&mut cm, 0, sbw, 1.0, OP)?;
            mset(&mut cm, n, sbw, -1.0, OP)?;
            mset(&mut cm, n, sbw + 1, 1.0, OP)?;
            mset(&mut cm, n + 1, sbw, 1.0, OP)?;
            for i in 1..n {
                band_row(&mut cm, i, at(t, i, OP)?)?;
            }
        }
        MatrixEnds::Both => {
            mset(&mut cm, 0, sbw, 1.0, OP)?;
            mset(&mut cm, 1, sbw - 1, -1.0, OP)?;
            mset(&mut cm, 1, sbw, 1.0, OP)?;
            mset(&mut cm, n + 1, sbw, -1.0, OP)?;
            mset(&mut cm, n + 1, sbw + 1, 1.0, OP)?;
            mset(&mut cm, n + 2, sbw, 1.0, OP)?;
            for i in 2..=n {
                band_row(&mut cm, i, at(t, i - 1, OP)?)?;
            }
        }
    }

    // LU decompose matrix.
    cm.lu_decompose()?;
    Ok(cm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::crv_fit::p1;

    fn pts(c: &[[f64; 3]]) -> Vec<Vec3> {
        c.iter().map(|p| Vec3::new(p[0], p[1], p[2])).collect()
    }

    fn eval(c: &Curve, t: f64) -> Vec3 {
        let knt = c.knot_vector().unwrap();
        let b = knt.basis_eval(c.degree(), t, Side::Left).unwrap();
        let mut s = Vec3::ZERO;
        for (r, v) in b.values.iter().enumerate() {
            let pw = c.cpts()[b.span - c.degree() + r];
            s += Vec3::new(pw.x, pw.y, pw.z) * *v;
        }
        s
    }

    fn data() -> Vec<Vec3> {
        pts(&[
            [0.0, 0.0, 0.0],
            [1.0, 2.0, 0.0],
            [3.0, 3.0, 1.0],
            [4.0, 1.0, 0.0],
            [6.0, 0.0, 2.0],
            [7.0, 2.0, 1.0],
        ])
    }

    #[test]
    fn interp_hits_points() {
        let a = data();
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        for p in 1..=4 {
            let c = interp(&a, p, ParamMethod::ChordLength).unwrap();
            assert_eq!(c.n(), 5);
            for (q, t) in a.iter().zip(&u) {
                assert!((eval(&c, *t) - *q).length() < 1e-9, "p={p}");
            }
        }
        assert!(matches!(
            interp(&a, 6, ParamMethod::Uniform),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn func_interp_matches_curve_interp() {
        let a = data();
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        let f: Vec<f64> = a.iter().map(|q| q.y).collect();
        let cf = func_interp_given_params(&f, 3, &u).unwrap();
        let c = interp(&a, 3, ParamMethod::ChordLength).unwrap();
        for (v, pw) in cf.values().iter().zip(c.cpts()) {
            assert!((v - pw.y).abs() < 1e-12);
        }
        let lin = func_interp_given_params(&f, 1, &u).unwrap();
        assert_eq!(lin.values(), &f[..]);
    }

    #[test]
    fn lst_sq_reproduces_line() {
        let a: Vec<Vec3> = (0..=10)
            .map(|i| Vec3::new(f64::from(i), 2.0, 0.0))
            .collect();
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        let c = approx_lst_sq(&a, 5, 3, ParamMethod::ChordLength).unwrap();
        for (q, t) in a.iter().zip(&u) {
            assert!((eval(&c, *t) - *q).length() < 1e-9);
        }
        assert!(approx_lst_sq(&a, 1, 1, ParamMethod::ChordLength).is_err());
    }

    #[test]
    fn weighted_without_constraints_is_lst_sq() {
        let a: Vec<Vec3> = (0..=8)
            .map(|i| Vec3::new(f64::from(i), f64::from(i * i) * 0.1, 0.0))
            .collect();
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        let kv = knot_vector_approx(&u, 4, 2).unwrap();
        let knt = KnotVector::from_slice(&kv).unwrap();
        let wp = vec![1.0; a.len()];
        let data = WeightedData {
            points: &a,
            point_weights: &wp,
            derivs: &[],
            deriv_weights: &[],
            deriv_index: &[],
        };
        let c = weighted_lst_sq_knots(&data, &u, &knt, 4, 2).unwrap();
        assert_eq!(c.n(), 4);
        // Constrained ends: the curve passes through them.
        let mut wp2 = wp.clone();
        wp2[0] = -1.0;
        wp2[8] = -1.0;
        let data = WeightedData {
            point_weights: &wp2,
            ..data
        };
        let c = weighted_lst_sq_knots(&data, &u, &knt, 4, 2).unwrap();
        assert!((eval(&c, 0.0) - a[0]).length() < 1e-9);
        assert!((eval(&c, 1.0) - a[8]).length() < 1e-9);
    }

    #[test]
    fn knot_vectors() {
        let u = [0.0, 0.2, 0.5, 0.7, 1.0];
        let k = knots_derivs(&u, 2).unwrap();
        assert_eq!(k.len(), 13);
        assert_eq!(k[3], 0.1);
        assert_eq!(k[4], 0.2);
        let k = knots_derivs(&u, 3).unwrap();
        assert_eq!(k.len(), 14);
        assert!(knots_derivs(&u[..1], 2).is_err());
        let k = knots_high_end_derivs(&u, 3, 1, 2).unwrap();
        assert_eq!(k.len(), 4 + 1 + 2 + 3 + 2);
        assert!(k.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn matrix_solves() {
        let a = data();
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        let kv = interp_knot_vector(&u, 3).unwrap();
        let knt = KnotVector::from_slice(&kv).unwrap();
        let cm = calc_matrix(&knt, 3, &u, MatrixEnds::None).unwrap();
        let w = cpts_with_matrix(&a, 3, &cm).unwrap();
        let c = interp(&a, 3, ParamMethod::ChordLength).unwrap();
        for (x, y) in w.iter().zip(c.cpts()) {
            assert!((x.x - y.x).abs() < 1e-12 && (x.y - y.y).abs() < 1e-12);
        }
        let kv = p1::knot_vector_deriv(&u, 3, DerivEnd::Start).unwrap();
        let knt = KnotVector::from_slice(&kv).unwrap();
        let cm = calc_matrix(&knt, 3, &u, MatrixEnds::Start).unwrap();
        let t = Vec3::new(1.0, 0.0, 0.0);
        let w = deriv_matrix(&a, 3, &knt, t, DerivEnd::Start, &cm).unwrap();
        assert_eq!(w.len(), 7);
    }

    /// Fuzz regressions: absurd degrees / counts return errors.
    #[test]
    fn absurd_degree_is_an_error() {
        let u = [0.0, 0.5, 1.0];
        let big = i64::MAX as usize;
        assert!(knots_derivs(&u, big).is_err());
        assert!(knots_high_end_derivs(&u, 3, big, big).is_err());
        assert!(knots_high_end_derivs(&u, big, 0, 0).is_err());
        let kv = [0.0, 0.0, 1.0, 1.0];
        let knt = KnotVector::from_slice(&kv).unwrap();
        assert!(calc_matrix(&knt, big, &u, MatrixEnds::None).is_err());
    }
}
