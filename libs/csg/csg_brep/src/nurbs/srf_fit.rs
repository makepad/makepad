//! NURBS surface fitting to regular grids of points: parameter values for the grid
//! rows and columns, global interpolation with given parameters and knot vectors,
//! least-squares approximation with chosen control net size, and interpolation of a
//! grid of scalar values by a B-spline surface function.
//!
//! Upstream: `source/SMLib/src/NL_SrfFit.cpp` (declarations in
//! `source/SMLib/inc/NL_SrfFit.h`). Only the routines of unit `nlib.srf_fit` are
//! ported here.
//!
//! | Rust                       | Upstream                    |
//! |----------------------------|-----------------------------|
//! | [`param_values`]           | `N_FitCalcSrfParamValues`   |
//! | [`interp_params`]          | `N_FitSrfInterpParams`      |
//! | [`interp_given_knots`]     | `N_FitSrfToPtsKnots`        |
//! | [`lst_sq_approx`]          | `N_FitSrfLstSqApprox`       |
//! | [`func_interp`]            | `N_FitSrfFuncInterp`        |
//!
//! Deviations:
//! - The `NL_VOID **` grids plus `NL_EPOINT` / `NL_HPOINT` flags become grids
//!   (`&[Vec<P>]`, row `i` = `A[i][*]`) of a [`FitPoint`] type (`Vec3` or
//!   [`CPoint`]); the parametrisation and direction flags are the enums of
//!   [`crate::nurbs::crv_fit::p1`], so the `NL_CAL_ERR` paths for bad flags cannot
//!   occur. Highest indexes come from the grid: `n = a.len() - 1` and
//!   `m = a[0].len() - 1`; rows shorter than the first, or parameter slices shorter
//!   than upstream reads, give `InvalidInput`.
//! - Out-params become return values: parameters as `Vec<f64>`, surfaces as new
//!   [`Surface`]s and functions as new [`SurfaceFunction`]s (so the `NL_STO_ERR` of
//!   `N_SrfSizeArrays` / `N_SFuncSizeArrays` for a too small caller object cannot
//!   occur). Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR`
//!   number; partly written outputs are not returned.
//! - `N_FitSrfInterpParams` always returns `NL_NO`: when every row (column) has a
//!   total chord length of at most `NL_MTOL` it only records `NL_INP_ERR` in the
//!   global error state and leaves the interior parameters `0`. [`interp_params`]
//!   returns those parameters (`[0, 0, .., 0, 1]`) as `Ok`.
//! - `N_FitSrfLstSqApprox` overwrites row `1` (column `1`) of the caller's grid with
//!   row `r` (column `s`) when `n == 1` (`m == 1`); [`lst_sq_approx`] does that on a
//!   copy and leaves the input alone.
//! - Degree `p == 0` makes `N_FitSrfToPtsKnots` / `N_FitSrfFuncInterp` request a
//!   banded matrix of bandwidth `-1` (a negative-sized allocation);
//!   [`interp_given_knots`] / [`func_interp`] return `InvalidInput` there.
//! - Upstream's 2D work arrays and banded matrices are single row-major blocks, and
//!   the port indexes them the same way (a column index may leave its row). Accesses
//!   outside a whole block return `InvalidInput` instead of undefined behaviour.
//!   The coefficient array `A` of `N_FitSrfLstSqApprox` is one block shared by the
//!   u and v passes, as upstream; its uninitialised (`malloc`) entries are `0.0`.
//! - The `NL_STACKS` arguments and the routine-name strings have no counterpart.

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::crv_fit::p1::{
    cpts_from_srf_data, interp_knot_vector, knot_vector_approx, DataDir, FitPoint, ParamMethod,
};
use crate::nurbs::fit_arrays::{
    alloc, alloc2, at, at2, at_mut, banded, basis, copy_knots, head, hi, idx, invalid, mget, mset,
    put, put2, sg, Block,
};
use crate::nurbs::funcs_basic::SurfaceFunction;
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{CPoint, RealMatrix, MODEL_TOL};
use crate::nurbs::srf_basic::Surface;

/// Highest row and column indexes `(n, m)` of a grid (the first row's length).
fn grid_his<T>(a: &[Vec<T>], op: &'static str) -> Result<(i64, i64)> {
    let n = hi(a, op, "no grid rows")?;
    let m = hi(
        a.first().map_or(&[][..], Vec::as_slice),
        op,
        "empty grid row",
    )?;
    Ok((n, m))
}

/// Chord-length / centripetal accumulation over one direction (the shared loop of
/// `N_FitCalcSrfParamValues` and `N_FitSrfInterpParams`): `t[1..len]` gets the
/// averaged normalised cumulative distances of the `rows` sequences of `len + 1`
/// points `pt(k, i)`. Returns `false` when no sequence has a total above `NL_MTOL`
/// (upstream's `NL_INP_ERR`; `t` then keeps its zeros).
fn chord_dir<P: FitPoint>(
    t: &mut [f64],
    dist: &mut [f64],
    len: i64,
    rows: i64,
    par: ParamMethod,
    pt: impl Fn(i64, i64) -> Result<P>,
    op: &'static str,
) -> Result<bool> {
    for i in 1..len {
        put(t, i, 0.0, op)?;
    }
    let mut num = rows + 1;
    for k in 0..=rows {
        let mut sum = 0.0;
        for i in 1..=len {
            let mut d = pt(k, i - 1)?.dist(pt(k, i)?);
            if par == ParamMethod::Centripetal {
                d = d.sqrt();
            }
            put(dist, i, d, op)?;
            sum += d;
        }
        if sum > MODEL_TOL {
            let mut d = 0.0;
            for i in 1..len {
                d += at(dist, i, op)?;
                let ti = at_mut(t, i, op)?;
                *ti += d / sum;
            }
        } else {
            num -= 1;
        }
    }
    if num == 0 {
        return Ok(false);
    }
    for i in 1..len {
        let ti = at_mut(t, i, op)?;
        *ti /= num as f64;
    }
    Ok(true)
}

// ---- parameters -------------------------------------------------------------------

/// Parameters `u[0..=n]` and `v[0..=m]` of the rows and columns of the grid
/// `a[0..=n][0..=m]` for global surface interpolation (upstream
/// `N_FitCalcSrfParamValues`): uniform, or the average over all columns (rows) of
/// the normalised cumulative chord lengths (square roots for centripetal); columns
/// (rows) of total length at most `NL_MTOL` are left out of the average.
///
/// For a single row (column) the parameters are `[1.0]` (upstream writes `u[0] = 0`
/// and then `u[n] = 1`).
///
/// # Errors
///
/// [`NlibCode::Input`] when every column (then the u parameters are reported first)
/// or every row has total length at most `NL_MTOL` (always for `n == 0` or `m == 0`
/// with chord-length or centripetal parameters); `InvalidInput` for an empty grid or
/// a row shorter than the first.
pub fn param_values<P: FitPoint>(a: &[Vec<P>], par: ParamMethod) -> Result<(Vec<f64>, Vec<f64>)> {
    const OP: &str = "nurbs::srf_fit::param_values";
    let (n, m) = grid_his(a, OP)?;
    let mut u = alloc(n, 0.0, OP)?;
    let mut v = alloc(m, 0.0, OP)?;

    // Compute uniform parametrization.
    put(&mut u, 0, 0.0, OP)?;
    put(&mut u, n, 1.0, OP)?;
    put(&mut v, 0, 0.0, OP)?;
    put(&mut v, m, 1.0, OP)?;
    if par == ParamMethod::Uniform {
        let d = 1.0 / n as f64;
        for i in 1..n {
            put(&mut u, i, i as f64 * d, OP)?;
        }
        let d = 1.0 / m as f64;
        for j in 1..m {
            put(&mut v, j, j as f64 * d, OP)?;
        }
        return Ok((u, v));
    }

    // Compute chord length or centripetal parametrization.
    let mut dist = alloc(n.max(m), 0.0, OP)?;

    // Get u-directional parameters (the points P[i][j], i = 0..=n, of column j).
    if !chord_dir(&mut u, &mut dist, n, m, par, |j, i| at2(a, i, j, OP), OP)? {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    // Get v-directional parameters (the points P[i][j], j = 0..=m, of row i).
    if !chord_dir(&mut v, &mut dist, m, n, par, |i, j| at2(a, i, j, OP), OP)? {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    Ok((u, v))
}

/// Parameters of the grid `a[0..=n][0..=m]` in one direction (upstream
/// `N_FitSrfInterpParams`): `t[0..=n]` for `DataDir::U`, `t[0..=m]` for
/// `DataDir::V`, computed as in [`param_values`].
///
/// When every column (row) has total length at most `NL_MTOL` the interior
/// parameters are `0.0` and the result is still `Ok` (upstream returns `NL_NO`
/// after recording `NL_INP_ERR`). For a single row (column) the result is `[1.0]`.
///
/// # Errors
///
/// `InvalidInput` for an empty grid or a row shorter than the first.
pub fn interp_params<P: FitPoint>(
    a: &[Vec<P>],
    par: ParamMethod,
    dir: DataDir,
) -> Result<Vec<f64>> {
    const OP: &str = "nurbs::srf_fit::interp_params";
    let (n, m) = grid_his(a, OP)?;
    let len = match dir {
        DataDir::U => n,
        DataDir::V => m,
    };
    let mut t = alloc(len, 0.0, OP)?;
    put(&mut t, 0, 0.0, OP)?;
    put(&mut t, len, 1.0, OP)?;

    // Compute uniform parametrization.
    if par == ParamMethod::Uniform {
        let d = 1.0 / len as f64;
        for i in 1..len {
            put(&mut t, i, i as f64 * d, OP)?;
        }
        return Ok(t);
    }

    // Compute chord length or centripetal parametrization.
    let mut dist = alloc(n.max(m), 0.0, OP)?;
    // The NL_INP_ERR outcome only reaches upstream's global error record.
    let _ = match dir {
        DataDir::U => chord_dir(&mut t, &mut dist, n, m, par, |j, i| at2(a, i, j, OP), OP)?,
        DataDir::V => chord_dir(&mut t, &mut dist, m, n, par, |i, j| at2(a, i, j, OP), OP)?,
    };
    Ok(t)
}

// ---- interpolation ----------------------------------------------------------------

/// Interpolation matrix of one direction (`N_FitSrfToPtsKnots`): banded with
/// bandwidth `2p - 1`, unit end rows, basis functions of `knt` at `t[1..n]`,
/// LU-decomposed.
fn interp_matrix(
    knt: &KnotVector<'_>,
    t: &[f64],
    n: i64,
    p: i64,
    op: &'static str,
) -> Result<RealMatrix> {
    let ub = 2 * p - 1;
    let usb = p - 1;
    let mut cm = banded(n, ub, op)?;
    mset(&mut cm, 0, usb, 1.0, op)?;
    mset(&mut cm, n, usb, 1.0, op)?;
    for i in 1..n {
        let (j, nb) = basis(knt, p, at(t, i, op)?, op)?;
        let l = j - i - 1;
        if l < 0 || l + p >= ub {
            return Err(SolidError::nlib(op, NlibCode::Input));
        }
        for k in 0..=p {
            mset(&mut cm, i, l + k, at(&nb, k, op)?, op)?;
        }
    }
    cm.lu_decompose()?;
    Ok(cm)
}

/// Surface of degrees `p`, `q` with knot vectors `knu`, `knv` interpolating the grid
/// `a[0..=n][0..=m]`, the point `a[i][j]` at the parameters `(u[i], v[j])` (upstream
/// `N_FitSrfToPtsKnots`): column-wise interpolation in u (copy for `p == 1`), then
/// row-wise interpolation in v of the result (skipped for `q <= 1`).
///
/// # Errors
///
/// [`NlibCode::Input`] when `knu` / `knv` do not have `n + p + 2` / `m + q + 2`
/// knots, `p > n`, `q > m`, or a basis span falls outside the matrix band; errors
/// of [`KnotVector::basis_eval`] and of the LU decomposition and solves;
/// `InvalidInput` for an empty grid, a short row, too few parameters, or `p == 0`.
pub fn interp_given_knots<P: FitPoint>(
    a: &[Vec<P>],
    u: &[f64],
    v: &[f64],
    knu: &KnotVector<'_>,
    knv: &KnotVector<'_>,
    p: usize,
    q: usize,
) -> Result<Surface> {
    const OP: &str = "nurbs::srf_fit::interp_given_knots";
    let (n, m) = grid_his(a, OP)?;
    let (p, q) = (sg(p, OP)?, sg(q, OP)?);

    // Check for surface memory.
    let r = sg(knu.last_index(), OP)?;
    let s = sg(knv.last_index(), OP)?;
    if r != n + p + 1 || s != m + q + 1 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if p > n || q > m {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let us = copy_knots(knu, r, OP)?;
    let vs = copy_knots(knv, s, OP)?;
    let mut pw = alloc2(n, m, CPoint::default(), OP)?;

    // Interpolate in u-direction.
    if p == 1 {
        for i in 0..=n {
            for j in 0..=m {
                put2(&mut pw, i, j, at2(a, i, j, OP)?.to_cpoint(), OP)?;
            }
        }
    } else {
        if p == 0 {
            return Err(invalid(
                OP,
                "degree 0 (upstream sizes a matrix of bandwidth -1)",
            ));
        }
        let cm = interp_matrix(knu, u, n, p, OP)?;
        for j in 0..=m {
            let rw = cpts_from_srf_data(a, idx(j, OP)?, DataDir::U, &cm)?;
            for i in 0..=n {
                put2(&mut pw, i, j, at(&rw, i, OP)?, OP)?;
            }
        }
    }

    // Interpolate in v-direction.
    if q > 1 {
        let cm = interp_matrix(knv, v, m, q, OP)?;
        for i in 0..=n {
            let rw = cpts_from_srf_data(&pw, idx(i, OP)?, DataDir::V, &cm)?;
            for j in 0..=m {
                put2(&mut pw, i, j, at(&rw, j, OP)?, OP)?;
            }
        }
    }
    Surface::new(pw, idx(p, OP)?, idx(q, OP)?, us, vs)
}

// ---- least-squares approximation --------------------------------------------------

/// Band bookkeeping of `N_FitSrfLstSqApprox`, shared by its u and v passes as
/// upstream (entries a pass does not rewrite keep the previous pass's values).
struct Band {
    /// `index[k]`: first basis function index of sample `k`'s row of `A`.
    index: Vec<i64>,
    /// `start[i]`: first sample on which basis function `i + 1` is non-zero.
    start: Vec<i64>,
    /// `end[i]`: last sample on which basis function `i + 1` is non-zero.
    end: Vec<i64>,
}

impl Band {
    /// Zeroed bookkeeping for highest sample index `samples` and highest interior
    /// basis index `basis`.
    fn new(samples: i64, basis: i64, op: &'static str) -> Result<Self> {
        Ok(Self {
            index: alloc(samples, 0, op)?,
            start: alloc(basis, 0, op)?,
            end: alloc(basis, 0, op)?,
        })
    }
}

/// Band bookkeeping and normal equations of one direction of
/// `N_FitSrfLstSqApprox`: fills `A` (basis functions per sample, shared block) and
/// `band`, and returns the LU-decomposed matrix `N^T N`.
fn normal_matrix(
    knt: &KnotVector<'_>,
    t: &[f64],
    r: i64,
    n: i64,
    p: i64,
    a: &mut Block,
    band: &mut Band,
) -> Result<RealMatrix> {
    const OP: &str = "nurbs::srf_fit::lst_sq_approx";
    let index = band.index.as_mut_slice();
    let start = band.start.as_mut_slice();
    let end = band.end.as_mut_slice();
    let ub = 2 * p + 1;
    let usb = p;
    let mut rj = p;
    let mut sj = p - 1;
    let mut ej: i64 = -2;
    // NTN (zeroed by upstream for its band) as a fresh banded matrix.
    let mut cm = banded(n - 2, ub, OP)?;

    for i in 0..=r - 2 {
        a.set(i, p, 0.0, OP)?;
    }
    for i in 0..=(p - 1).min(n - 2) {
        put(start, i, 0, OP)?;
    }
    put(end, 0, -2, OP)?;

    for i in 1..r {
        let (j, nb) = basis(knt, p, at(t, i, OP)?, OP)?;
        let l = i64::from(j == p);
        let hk = if j == p || j == n { p - 1 } else { p };
        for k in 0..=hk {
            a.set(i - 1, k, at(&nb, l + k, OP)?, OP)?;
        }
        put(index, i - 1, 0.max(j - p - 1), OP)?;
        if j > rj {
            for _ in 1..=j - rj {
                sj += 1;
                ej += 1;
                if sj <= n - 2 {
                    put(start, sj, i - 1, OP)?;
                }
                if ej >= 0 {
                    put(end, ej, i - 2, OP)?;
                }
            }
            rj = j;
        }
    }

    if sj < n - 2 || at(end, 0, OP)? == -1 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    for i in 0.max(ej + 1)..=n - 2 {
        put(end, i, r - 2, OP)?;
    }

    for i in 0..=n - 2 {
        let lj = 0.max(i - p);
        let hj = (n - 2).min(i + p);
        for j in lj..=hj {
            let lk = at(start, i, OP)?.max(at(start, j, OP)?);
            let hk = at(end, i, OP)?.min(at(end, j, OP)?);
            for k in lk..=hk {
                let ik = at(index, k, OP)?;
                let v = mget(&cm, i, j - i + usb, OP)?
                    + a.get(k, i - ik, OP)? * a.get(k, j - ik, OP)?;
                mset(&mut cm, i, j - i + usb, v, OP)?;
            }
        }
    }

    cm.lu_decompose()?;
    Ok(cm)
}

/// `N_{0,p}(t[i])` and `N_{n,p}(t[i])` for `i = 1..r` into `ts[i-1]`, `te[i-1]`.
fn end_basis(
    knt: &KnotVector<'_>,
    t: &[f64],
    r: i64,
    n: i64,
    p: i64,
    ts: &mut [f64],
    te: &mut [f64],
) -> Result<()> {
    const OP: &str = "nurbs::srf_fit::lst_sq_approx";
    let (pi, ni) = (idx(p, OP)?, idx(n, OP)?);
    for i in 1..r {
        let ti = at(t, i, OP)?;
        put(ts, i - 1, knt.basis_i_eval(0, pi, ti, Side::Left)?, OP)?;
        put(te, i - 1, knt.basis_i_eval(ni, pi, ti, Side::Left)?, OP)?;
    }
    Ok(())
}

/// Least-squares right-hand sides `R[i-1] = sum_k A[k][i-index[k]-1] * Rk[k]` for
/// `i = 1..n` (`R` keeps its length; entries past `n - 2` are not touched).
fn rhs<P: FitPoint>(a: &Block, rk: &[P], n: i64, band: &Band, out: &mut [P]) -> Result<()> {
    const OP: &str = "nurbs::srf_fit::lst_sq_approx";
    let Band { index, start, end } = band;
    for i in 1..n {
        let ri = at_mut(out, i - 1, OP)?;
        *ri = P::ZERO;
        let lk = at(start, i - 1, OP)?;
        let hk = at(end, i - 1, OP)?;
        for k in lk..=hk {
            let ik = at(index, k, OP)?;
            ri.blend(a.get(k, i - ik - 1, OP)?, at(rk, k, OP)?);
        }
    }
    Ok(())
}

/// Surface with control net `(n+1) x (m+1)` and degrees `p`, `q` approximating the
/// grid `pts[0..=r][0..=s]` in the least-squares sense, through its corner rows and
/// columns (upstream `N_FitSrfLstSqApprox`): parameters by `par`
/// ([`param_values`]), knot vectors by [`knot_vector_approx`], then column-wise
/// approximation in u and row-wise approximation in v of the result. For
/// `n == m == 1` the net is the four corner points; `n == 1` (`m == 1`) rules the
/// surface between the first and last row (column).
///
/// # Errors
///
/// [`NlibCode::Input`] when `n > r`, `n < 1`, `p > n` (likewise `m`, `s`, `q`) or
/// some interior basis function gets no sample; the errors of [`param_values`],
/// [`knot_vector_approx`], [`KnotVector::basis_eval`],
/// [`KnotVector::basis_i_eval`] and of the LU decompositions and solves;
/// `InvalidInput` for an empty grid or a short row.
pub fn lst_sq_approx(
    pts: &[Vec<Vec3>],
    n: usize,
    m: usize,
    p: usize,
    q: usize,
    par: ParamMethod,
) -> Result<Surface> {
    const OP: &str = "nurbs::srf_fit::lst_sq_approx";
    let (r, s) = grid_his(pts, OP)?;
    let (n, m, p, q) = (sg(n, OP)?, sg(m, OP)?, sg(p, OP)?, sg(q, OP)?);

    // Check for error (upstream's further checks are implied by these).
    if n > r || n < 1 || p > n {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if m > s || m < 1 || q > m {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let mut pw = alloc2(n, m, CPoint::default(), OP)?;

    // Get parameters and knot vectors.
    let (u, v) = param_values(pts, par)?;
    let uk = knot_vector_approx(&u, idx(n, OP)?, idx(p, OP)?)?;
    let vk = knot_vector_approx(&v, idx(m, OP)?, idx(q, OP)?)?;
    let (knu, knv) = (KnotVector::from_slice(&uk)?, KnotVector::from_slice(&vk)?);
    let surface = |pw| Surface::new(pw, idx(p, OP)?, idx(q, OP)?, uk.clone(), vk.clone());

    // Only corner points wanted.
    if n == 1 && m == 1 {
        put2(&mut pw, 0, 0, at2(pts, 0, 0, OP)?.to_cpoint(), OP)?;
        put2(&mut pw, 1, 0, at2(pts, r, 0, OP)?.to_cpoint(), OP)?;
        put2(&mut pw, 0, 1, at2(pts, 0, s, OP)?.to_cpoint(), OP)?;
        put2(&mut pw, 1, 1, at2(pts, r, s, OP)?.to_cpoint(), OP)?;
        return surface(pw);
    }

    // Reset data points if ruling is required (on a copy).
    let mut pts = pts.to_vec();
    if n == 1 {
        for j in 0..=s {
            let pj = at2(&pts, r, j, OP)?;
            put2(&mut pts, 1, j, pj, OP)?;
        }
    }
    if m == 1 {
        for i in 0..=r {
            let pi = at2(&pts, i, s, OP)?;
            put2(&mut pts, i, 1, pi, OP)?;
        }
    }

    // Allocate memory.
    let ii = (r - 2).max(s - 2);
    let kk = p.max(q);
    let jj = (n - 2).max(m - 2);
    let mut a = Block::new(ii, kk, OP)?;
    let mut ts = alloc(ii, 0.0, OP)?;
    let mut te = alloc(ii, 0.0, OP)?;
    let mut band = Band::new(ii, jj, OP)?;
    let mut rk = alloc(ii, Vec3::ZERO, OP)?;
    let mut rv = alloc(jj, Vec3::ZERO, OP)?;
    let mut rkw = alloc(ii, CPoint::ZERO, OP)?;
    let mut rw = alloc(jj, CPoint::ZERO, OP)?;
    let mut qw = alloc2(n, s, CPoint::ZERO, OP)?;

    // Compute coefficient matrix for u-directional approximation.
    if n > 1 {
        let cm = normal_matrix(&knu, &u, r, n, p, &mut a, &mut band)?;

        // Approximate rows in the u-direction.
        end_basis(&knu, &u, r, n, p, &mut ts, &mut te)?;
        let l = if m == 1 { 1 } else { s };
        for j in 0..=l {
            let (p0, pr) = (at2(&pts, 0, j, OP)?, at2(&pts, r, j, OP)?);
            let target = if m == 1 { &mut pw } else { &mut qw };
            put2(target, 0, j, p0.to_cpoint(), OP)?;
            put2(target, n, j, pr.to_cpoint(), OP)?;
            for i in 1..r {
                let v = Vec3::translate_sum2(
                    at2(&pts, i, j, OP)?,
                    -at(&ts, i - 1, OP)?,
                    p0,
                    -at(&te, i - 1, OP)?,
                    pr,
                );
                put(&mut rk, i - 1, v, OP)?;
            }
            rhs(&a, &rk, n, &band, &mut rv)?;
            let aw = cm.for_back_points(&rv)?;
            for i in 0..=n - 2 {
                put2(target, i + 1, j, at(&aw, i, OP)?, OP)?;
            }
        }
    }

    // Compute coefficient matrix for v-directional approximation.
    if m > 1 {
        let cm = normal_matrix(&knv, &v, s, m, q, &mut a, &mut band)?;

        // Approximate rows in the v-direction.
        end_basis(&knv, &v, s, m, q, &mut ts, &mut te)?;
        if n == 1 {
            for i in 0..=n {
                let (p0, ps) = (at2(&pts, i, 0, OP)?, at2(&pts, i, s, OP)?);
                put2(&mut pw, i, 0, p0.to_cpoint(), OP)?;
                put2(&mut pw, i, m, ps.to_cpoint(), OP)?;
                for j in 1..s {
                    let v = Vec3::translate_sum2(
                        at2(&pts, i, j, OP)?,
                        -at(&ts, j - 1, OP)?,
                        p0,
                        -at(&te, j - 1, OP)?,
                        ps,
                    );
                    put(&mut rk, j - 1, v, OP)?;
                }
                rhs(&a, &rk, m, &band, &mut rv)?;
                let aw = cm.for_back_points(&rv)?;
                for j in 0..=m - 2 {
                    put2(&mut pw, i, j + 1, at(&aw, j, OP)?, OP)?;
                }
            }
        } else {
            for i in 0..=n {
                let (q0, qs) = (at2(&qw, i, 0, OP)?, at2(&qw, i, s, OP)?);
                put2(&mut pw, i, 0, q0, OP)?;
                put2(&mut pw, i, m, qs, OP)?;
                for j in 1..s {
                    let v = CPoint::translate_sum2(
                        at2(&qw, i, j, OP)?,
                        -at(&ts, j - 1, OP)?,
                        q0,
                        -at(&te, j - 1, OP)?,
                        qs,
                    );
                    put(&mut rkw, j - 1, v, OP)?;
                }
                rhs(&a, &rkw, m, &band, &mut rw)?;
                let aw = cm.for_back_cpoints(&rw)?;
                for j in 0..=m - 2 {
                    put2(&mut pw, i, j + 1, at(&aw, j, OP)?, OP)?;
                }
            }
        }
    }
    surface(pw)
}

// ---- surface functions ------------------------------------------------------------

/// Interpolation matrix of one direction of `N_FitSrfFuncInterp`: as
/// `interp_matrix` but without the band check (upstream writes `A[i][l + k]` for
/// any span).
fn func_matrix(
    knt: &KnotVector<'_>,
    t: &[f64],
    n: i64,
    p: i64,
    op: &'static str,
) -> Result<RealMatrix> {
    let ub = 2 * p - 1;
    let usb = p - 1;
    let mut cm = banded(n, ub, op)?;
    mset(&mut cm, 0, usb, 1.0, op)?;
    mset(&mut cm, n, usb, 1.0, op)?;
    for i in 1..n {
        let (j, nb) = basis(knt, p, at(t, i, op)?, op)?;
        let l = j - i - 1;
        for k in 0..=p {
            mset(&mut cm, i, l + k, at(&nb, k, op)?, op)?;
        }
    }
    cm.lu_decompose()?;
    Ok(cm)
}

/// Surface function of degrees `p`, `q` interpolating the values `f[0..=n][0..=m]`,
/// `f[i][j]` at `(u[i], v[j])` (upstream `N_FitSrfFuncInterp`); the knot vectors are
/// built from `u[0..=n]` and `v[0..=m]` by [`interp_knot_vector`].
///
/// # Errors
///
/// [`NlibCode::Input`] when `p > n` or `q > m`; errors of
/// [`KnotVector::basis_eval`] and of the LU decompositions and solves;
/// `InvalidInput` for an empty or ragged value grid, too few parameters, `p == 0`,
/// or a basis span that puts a matrix entry outside upstream's matrix block.
pub fn func_interp(
    f: &[Vec<f64>],
    p: usize,
    q: usize,
    u: &[f64],
    v: &[f64],
) -> Result<SurfaceFunction> {
    const OP: &str = "nurbs::srf_fit::func_interp";
    let (n, m) = grid_his(f, OP)?;
    let (pp, qq) = (p, q);
    let (p, q) = (sg(p, OP)?, sg(q, OP)?);

    // Check for function memory.
    if p > n || q > m {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    // Compute knot vectors.
    let uk = interp_knot_vector(head(u, n, OP)?, pp)?;
    let vk = interp_knot_vector(head(v, m, OP)?, qq)?;
    let (knu, knv) = (KnotVector::from_slice(&uk)?, KnotVector::from_slice(&vk)?);
    let mut fuv = alloc2(n, m, 0.0, OP)?;

    // Interpolate in u-direction.
    if p == 1 {
        for i in 0..=n {
            for j in 0..=m {
                put2(&mut fuv, i, j, at2(f, i, j, OP)?, OP)?;
            }
        }
    } else {
        if p == 0 {
            return Err(invalid(
                OP,
                "degree 0 (upstream sizes a matrix of bandwidth -1)",
            ));
        }
        let cm = func_matrix(&knu, u, n, p, OP)?;
        for j in 0..=m {
            let mut g = alloc(n, 0.0, OP)?;
            for i in 0..=n {
                put(&mut g, i, at2(f, i, j, OP)?, OP)?;
            }
            let h = cm.right_for_back(&g)?;
            for i in 0..=n {
                put2(&mut fuv, i, j, at(&h, i, OP)?, OP)?;
            }
        }
    }

    // Interpolate in v-direction.
    if q > 1 {
        let cm = func_matrix(&knv, v, m, q, OP)?;
        for i in 0..=n {
            let mut g = alloc(m, 0.0, OP)?;
            for j in 0..=m {
                put(&mut g, j, at2(&fuv, i, j, OP)?, OP)?;
            }
            let h = cm.right_for_back(&g)?;
            for j in 0..=m {
                put2(&mut fuv, i, j, at(&h, j, OP)?, OP)?;
            }
        }
    }
    SurfaceFunction::new(fuv, pp, qq, uk, vk)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: usize, m: usize, f: impl Fn(f64, f64) -> [f64; 3]) -> Vec<Vec<Vec3>> {
        (0..=n)
            .map(|i| {
                (0..=m)
                    .map(|j| {
                        let c = f(i as f64, j as f64);
                        Vec3::new(c[0], c[1], c[2])
                    })
                    .collect()
            })
            .collect()
    }

    fn eval(s: &Surface, u: f64, v: f64) -> Vec3 {
        let (ku, kv) = s.knot_vectors().unwrap();
        let bu = ku.basis_eval(s.p(), u, Side::Left).unwrap();
        let bv = kv.basis_eval(s.q(), v, Side::Left).unwrap();
        let mut acc = Vec3::ZERO;
        for (a, nu) in bu.values.iter().enumerate() {
            for (b, nv) in bv.values.iter().enumerate() {
                let pw = s.cpt(bu.span - s.p() + a, bv.span - s.q() + b).unwrap();
                acc += Vec3::new(pw.x, pw.y, pw.z) * (nu * nv);
            }
        }
        acc
    }

    #[test]
    fn params_uniform_and_chord() {
        let a = grid(2, 3, |i, j| [i * i, j, 0.0]);
        let (u, v) = param_values(&a, ParamMethod::Uniform).unwrap();
        assert_eq!(u, vec![0.0, 0.5, 1.0]);
        assert_eq!(v, vec![0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]);
        let (u, v) = param_values(&a, ParamMethod::ChordLength).unwrap();
        assert_eq!(u, vec![0.0, 0.25, 1.0]);
        assert!((v[1] - 1.0 / 3.0).abs() < 1e-15);
        let t = interp_params(&a, ParamMethod::ChordLength, DataDir::U).unwrap();
        assert_eq!(t, u);
        let t = interp_params(&a, ParamMethod::ChordLength, DataDir::V).unwrap();
        assert_eq!(t, v);
        // Single row: chord parameters fail, interp_params keeps zeros.
        let one = grid(0, 2, |_, j| [j, 0.0, 0.0]);
        assert!(matches!(
            param_values(&one, ParamMethod::ChordLength),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
        let z = grid(2, 2, |_, _| [1.0, 1.0, 1.0]);
        assert_eq!(
            interp_params(&z, ParamMethod::Centripetal, DataDir::U).unwrap(),
            vec![0.0, 0.0, 1.0]
        );
        assert!(param_values::<Vec3>(&[], ParamMethod::Uniform).is_err());
    }

    #[test]
    fn interpolation_hits_points() {
        let a = grid(4, 3, |i, j| [i, j, (i * 0.7).sin() + j * j * 0.1]);
        let (u, v) = param_values(&a, ParamMethod::ChordLength).unwrap();
        let ku = interp_knot_vector(&u, 3).unwrap();
        let kv = interp_knot_vector(&v, 2).unwrap();
        let s = interp_given_knots(
            &a,
            &u,
            &v,
            &KnotVector::from_slice(&ku).unwrap(),
            &KnotVector::from_slice(&kv).unwrap(),
            3,
            2,
        )
        .unwrap();
        for (i, row) in a.iter().enumerate() {
            for (j, pt) in row.iter().enumerate() {
                assert!((eval(&s, u[i], v[j]) - *pt).length() < 1e-9);
            }
        }
        // Wrong knot count.
        assert!(matches!(
            interp_given_knots(
                &a,
                &u,
                &v,
                &KnotVector::from_slice(&kv).unwrap(),
                &KnotVector::from_slice(&kv).unwrap(),
                3,
                2,
            ),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn approximation_of_plane_is_exact() {
        let a = grid(8, 6, |i, j| [i, 2.0 * j, 0.5 * i - j]);
        let s = lst_sq_approx(&a, 5, 4, 3, 2, ParamMethod::ChordLength).unwrap();
        assert_eq!(s.array_sizes(), (5, 4, 9, 7));
        let (u, v) = param_values(&a, ParamMethod::ChordLength).unwrap();
        for (i, row) in a.iter().enumerate() {
            for (j, pt) in row.iter().enumerate() {
                assert!((eval(&s, u[i], v[j]) - *pt).length() < 1e-9);
            }
        }
        // Ruled and bilinear cases.
        let s = lst_sq_approx(&a, 1, 3, 1, 2, ParamMethod::Uniform).unwrap();
        assert_eq!(s.cpt(1, 0).unwrap(), CPoint::from_point(a[8][0]));
        let s = lst_sq_approx(&a, 4, 1, 2, 1, ParamMethod::Centripetal).unwrap();
        assert_eq!(s.cpt(0, 1).unwrap(), CPoint::from_point(a[0][6]));
        let s = lst_sq_approx(&a, 1, 1, 1, 1, ParamMethod::Uniform).unwrap();
        assert_eq!(s.cpt(1, 1).unwrap(), CPoint::from_point(a[8][6]));
        assert!(lst_sq_approx(&a, 9, 3, 2, 2, ParamMethod::Uniform).is_err());
        // The input is left alone.
        assert_eq!(a[1][0], Vec3::new(1.0, 0.0, 0.5));
    }

    #[test]
    fn function_interpolation() {
        let f: Vec<Vec<f64>> = (0..5)
            .map(|i| (0..4).map(|j| f64::from(i * i - j)).collect())
            .collect();
        let u = [0.0, 0.2, 0.5, 0.7, 1.0];
        let v = [0.0, 0.3, 0.6, 1.0];
        let sf = func_interp(&f, 3, 2, &u, &v).unwrap();
        for (i, row) in f.iter().enumerate() {
            for (j, x) in row.iter().enumerate() {
                let e = sf.eval(u[i], v[j], Side::Left, Side::Left).unwrap();
                assert!((e - x).abs() < 1e-9);
            }
        }
        assert!(func_interp(&f, 5, 2, &u, &v).is_err());
        assert!(func_interp(&f, 0, 2, &u, &v).is_err());
    }
}
