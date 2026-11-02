//! Curve fitting, part 3: global interpolation with two end derivatives (general
//! degree and the tridiagonal cubic spline), with first derivatives at every point,
//! with higher end derivatives, of closed data with end conditions, and with
//! optional end tangents and knots selected from a suggested knot vector.
//!
//! Upstream: `source/SMLib/src/NL_CrvFit.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvFit.h`; flags in `source/SMLib/inc/NL_DataStruct.h`).
//!
//! | Rust                                  | Upstream                          |
//! |---------------------------------------|-----------------------------------|
//! | [`interp_end_derivs`], [`ParamSource`] | `N_FitCrvDerivs` (`par`, `NL_INHERITED`) |
//! | [`cubic_spline_interp`]               | `N_FitCubicSplineInterp`          |
//! | [`interp_first_derivs`], [`VectorKind`] | `N_FitCrvFirstDeriv` (`vec` / `der`: `NL_TANGENT` / `NL_DERIVATIVE`) |
//! | [`interp_knots_and_tangents`], [`EndTangents`] | `N_FitCrvKnotsAndTangents` |
//! | [`approx_closed_conditions`]          | `N_FitCrvApproxClosedConditions`  |
//! | [`interp_high_derivs`]                | `N_FitCrvHighDerivs`              |
//!
//! Deviations:
//! - Highest indices come from slice lengths (`kk`, `k`, `n`, `nn` of the points).
//!   Upstream's 1-based derivative arrays `Ds[1..=kk]`, `De[1..=ll]` and `D[1..=k]`
//!   are 0-based slices holding just those vectors (`ds[i - 1]` is `Ds[i]`), so
//!   `kk = ds.len()` and `ll = de.len()`.
//! - Out-params become return values (new [`Curve`]s), except where upstream reads
//!   the caller's data back: [`interp_end_derivs`] fills a caller curve slot sized
//!   with [`size_arrays`] (its `NL_INHERITED` mode reads the knots already there; a
//!   NULL slot gets zero knots where upstream reads unset memory), and in/out
//!   tangents, derivatives and knot vectors are `&mut` (rescaled or replaced in place
//!   as upstream does, including the writes made before a later error). On error,
//!   [`interp_end_derivs`] leaves the sized slot's arrays unwritten.
//! - Upstream ignores the errors of the cubic Bezier fit and its derivative that
//!   estimate a missing end derivative in [`interp_end_derivs`] and
//!   [`cubic_spline_interp`] (it then reads unset memory); the port returns them.
//!   Estimating needs four points (upstream reads `P[0..=3]` and `P[k-3..=k]`
//!   whatever `k` is); fewer give `InvalidInput`.
//! - In [`interp_knots_and_tangents`], upstream ignores the error of
//!   `N_FitCalcKnotVectorDeriv` (`p > k + 1`) and the following
//!   `N_FitCrvKnotsAndDeriv` then fails with `NL_INP_ERR` before reading the unset
//!   knots; the port returns that `NL_INP_ERR` directly. A caller passing the same
//!   vector as both `Ts` and `Te` is [`EndTangents::Same`].
//! - Upstream's banded matrices are single row-major blocks and are indexed the same
//!   way (`A[i][-1]` is the last entry of row `i - 1`). Accesses outside a whole
//!   block or outside the given slices return [`SolidError::InvalidInput`] instead of
//!   undefined behaviour.
//! - [`interp_knots_and_tangents`] rejects a degree above `NL_MAXDEG` with
//!   `InvalidInput` (upstream overruns its basis scratch arrays; the port would
//!   otherwise size its knot arrays from the degree).
//! - Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number.
//!   The `NL_STACKS` arguments and the routine-name strings have no counterpart.
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
use crate::nurbs::crv_basic::{size_arrays, Curve};
use crate::nurbs::crv_fit::p1::{
    interp_given_params, interp_knot_vector, interp_knots_and_deriv, interp_knots_and_derivs,
    knot_vector_deriv, knot_vector_end_derivs, param_values, DerivEnd, ParamMethod,
};
use crate::nurbs::crv_fit::p2::{interp, knots_derivs, knots_high_end_derivs};
use crate::nurbs::fit_arrays::{
    add, alloc, at, at2, at_mut, banded, basis, curve, hi, idx, invalid, mset, out_of_range, put,
    sg,
};
use crate::nurbs::geom::{dist_polygon, normalize};
use crate::nurbs::knots::{KnotVector, Side, MAX_DEGREE};
use crate::nurbs::math::{combine2_pts, CPoint};

const OP_DERIVS: &str = "nurbs::crv_fit::p3::interp_end_derivs";
const OP_CUBIC: &str = "nurbs::crv_fit::p3::cubic_spline_interp";
const OP_FIRST: &str = "nurbs::crv_fit::p3::interp_first_derivs";
const OP_TANGENTS: &str = "nurbs::crv_fit::p3::interp_knots_and_tangents";
const OP_CLOSED: &str = "nurbs::crv_fit::p3::approx_closed_conditions";
const OP_HIGH: &str = "nurbs::crv_fit::p3::interp_high_derivs";

/// Where [`interp_end_derivs`] takes the data point parameters from (upstream `par`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamSource {
    /// Computed by [`param_values`] with this method; the knot vector is then
    /// [`knot_vector_end_derivs`] of them.
    Method(ParamMethod),
    /// `NL_INHERITED` (4): `u[i] = U[i + p]` of the knots already in the output
    /// curve, which keeps those knots.
    Inherited,
}

impl ParamSource {
    /// The upstream flag value.
    #[must_use]
    pub fn flag(self) -> i32 {
        match self {
            ParamSource::Method(m) => m.flag(),
            ParamSource::Inherited => 4,
        }
    }
}

/// What given vectors are (upstream `vec` / `der` flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorKind {
    /// `NL_TANGENT` (1): directions only; they are normalised and rescaled
    /// internally (in place).
    Tangent,
    /// `NL_DERIVATIVE` (2): derivatives, used as given.
    Derivative,
}

impl VectorKind {
    /// The upstream flag value.
    #[must_use]
    pub fn flag(self) -> i32 {
        match self {
            VectorKind::Tangent => 1,
            VectorKind::Derivative => 2,
        }
    }
}

/// End tangents of [`interp_knots_and_tangents`] (upstream `Ts`, `Te`, each NULL or
/// not). With [`VectorKind::Tangent`] they are rescaled in place.
#[derive(Debug)]
pub enum EndTangents<'a> {
    /// No end tangent (`Ts == Te == NULL`).
    None,
    /// Start tangent only.
    Start(&'a mut Vec3),
    /// End tangent only.
    End(&'a mut Vec3),
    /// Start and end tangents.
    Both(&'a mut Vec3, &'a mut Vec3),
    /// One vector passed as both `Ts` and `Te` (it is rescaled twice, as upstream).
    Same(&'a mut Vec3),
}

// ---- helpers ----------------------------------------------------------------------

/// The slice `a[s..=e]` (upstream passes `&a[s]` with highest index `e - s`).
fn sub<'a, T>(a: &'a [T], s: i64, e: i64, op: &'static str) -> Result<&'a [T]> {
    let s = idx(s, op)?;
    let e = idx(e, op)?.checked_add(1).ok_or_else(|| out_of_range(op))?;
    a.get(s..e).ok_or_else(|| out_of_range(op))
}

/// `ND[0..=der][0..=p]` and the span (`N_BasisDerivs`, `NL_LEFT`).
fn basis_d(
    knt: &KnotVector<'_>,
    p: i64,
    u: f64,
    der: i64,
    op: &'static str,
) -> Result<(i64, Vec<Vec<f64>>)> {
    let b = basis_derivs(knt, idx(p, op)?, u, Side::Left, idx(der, op)?)?;
    Ok((sg(b.span, op)?, b.derivs))
}

/// `N_VectorScale(v, alpha, &v)`.
fn vscale(v: Vec3, alpha: f64) -> Vec3 {
    Vec3::new(alpha * v.x, alpha * v.y, alpha * v.z)
}

/// `N_VectorNormalizeRef`: the unit vector, or the vector unchanged with the error.
fn normalize_ref(v: &mut Vec3) -> Result<()> {
    let (u, _) = normalize(*v)?;
    *v = u;
    Ok(())
}

/// Missing end derivative estimated from the cubic Bezier interpolating the four
/// points `q` at `t` (`0.0` with `NL_LEFT` or `1.0` with `NL_RIGHT`), normalised
/// (left unscaled when that fails, as upstream ignores the error) and scaled by the
/// length `len` of the whole data polygon.
fn estimate_end_deriv(q: &[Vec3], t: f64, side: Side, len: f64) -> Result<Vec3> {
    // Use cubic Bezier to get start and end tangent directions.
    let cubic = interp(q, 3, ParamMethod::ChordLength)?;
    let cd = cubic.derivs(t, side, 1)?;
    let mut d = cd.get(1).copied().ok_or_else(|| out_of_range(OP_DERIVS))?;
    // Upstream ignores the normalisation error and scales the vector as it is.
    let _ = normalize_ref(&mut d);
    Ok(vscale(d, len))
}

/// The end derivatives with zero ones estimated (upstream's two
/// "derivatives are missing" blocks of `N_FitCrvDerivs` / `N_FitCubicSplineInterp`).
fn fill_end_derivs(
    a: &[Vec3],
    kk: i64,
    ds: Vec3,
    de: Vec3,
    op: &'static str,
) -> Result<(Vec3, Vec3)> {
    let mut ds = ds;
    let mut de = de;
    if ds.x == 0.0 && ds.y == 0.0 && ds.z == 0.0 {
        let q = sub(a, 0, 3, op)?;
        let len = dist_polygon(sub(a, 0, kk, op)?);
        ds = estimate_end_deriv(q, 0.0, Side::Left, len)?;
    }
    if de.x == 0.0 && de.y == 0.0 && de.z == 0.0 {
        let q = sub(a, kk - 3, kk, op)?;
        let len = dist_polygon(sub(a, 0, kk, op)?);
        de = estimate_end_deriv(q, 1.0, Side::Right, len)?;
    }
    Ok((ds, de))
}

// ---- interpolation with two end derivatives ---------------------------------------

/// Curve of degree `p` interpolating the points `a[0..=kk]` and the start and end
/// derivatives `ds`, `de` into the curve slot `cur` (upstream `N_FitCrvDerivs`); it
/// has `kk + 3` control points. A zero `ds` / `de` is estimated from the cubic
/// through the first / last four points, scaled by the length of the data polygon.
/// The derivatives are taken as scaled to a `[0, 1]` parameter range (`Q[1] =
/// U[p+1]/p * ds`, `Q[n-1] = (1 - U[n])/p * de`).
///
/// `cur` is sized with [`size_arrays`] (`None` gets new storage); with
/// [`ParamSource::Inherited`] the parameters are `U[p..=kk+p]` of its knots, which
/// it keeps.
///
/// # Errors
///
/// [`NlibCode::Input`] when `p < 2` or `p > kk + 2`; [`NlibCode::Storage`] when an
/// existing `cur` is too small; errors of the estimate, of
/// [`KnotVector::basis_eval`], of the LU decomposition and of the solve;
/// `InvalidInput` for empty `a`, fewer than four points with a derivative to
/// estimate, or a matrix entry outside upstream's storage.
pub fn interp_end_derivs(
    a: &[Vec3],
    p: usize,
    ds: Vec3,
    de: Vec3,
    par: ParamSource,
    cur: &mut Option<Curve>,
) -> Result<()> {
    const OP: &str = OP_DERIVS;
    let kk = hi(a, OP, "no data points")?;
    let p = sg(p, OP)?;

    // Check for curve memory.
    if p < 2 || p > kk + 2 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    // If derivatives are missing we will estimate them.
    let (ds, de) = fill_end_derivs(a, kk, ds, de, OP)?;

    let n = kk + 2;
    let crv = size_arrays(cur, idx(n, OP)?, idx(p, OP)?, idx(n + p + 1, OP)?)?;

    // Get parameters and compute knot vector.
    let (u, uk) = match par {
        ParamSource::Inherited => {
            let uk = crv.knots().to_vec();
            let mut u = alloc(kk, 0.0, OP)?;
            for i in 0..=kk {
                put(&mut u, i, at(&uk, i + p, OP)?, OP)?;
            }
            (u, uk)
        }
        ParamSource::Method(m) => {
            let u = param_values(a, m)?;
            // This modifies the knots.
            let uk = knot_vector_end_derivs(&u, idx(p, OP)?)?;
            (u, uk)
        }
    };
    let knt = KnotVector::from_slice(&uk)?;

    // Compute coefficient matrix.
    let bw = 2 * p - 1;
    let sbw = p - 1;
    let mut cm = banded(n, bw, OP)?;
    mset(&mut cm, 0, sbw, 1.0, OP)?;
    mset(&mut cm, 1, sbw - 1, -1.0, OP)?;
    mset(&mut cm, 1, sbw, 1.0, OP)?;
    mset(&mut cm, n - 1, sbw, -1.0, OP)?;
    mset(&mut cm, n - 1, sbw + 1, 1.0, OP)?;
    mset(&mut cm, n, sbw, 1.0, OP)?;
    for i in 2..=kk {
        let (j, nb) = basis(&knt, p, at(&u, i - 1, OP)?, OP)?;
        let l = j - i - 1;
        for k in 0..=p {
            mset(&mut cm, i, l + k, at(&nb, k, OP)?, OP)?;
        }
    }

    // LU decompose matrix.
    cm.lu_decompose()?;

    // Solve system of linear equations.
    let mut q = alloc(n, Vec3::ZERO, OP)?;
    // This may be where the routine gives poor results ( /p ??).
    let fact = at(&uk, p + 1, OP)? / p as f64;
    put(&mut q, 0, at(a, 0, OP)?, OP)?;
    put(&mut q, 1, vscale(ds, fact), OP)?;
    for i in 2..=kk {
        put(&mut q, i, at(a, i - 1, OP)?, OP)?;
    }
    let fact = (1.0 - at(&uk, n, OP)?) / p as f64;
    put(&mut q, n - 1, vscale(de, fact), OP)?;
    put(&mut q, n, at(a, kk, OP)?, OP)?;
    let pw = cm.for_back_points(&q)?;

    // Write the result into the sized curve.
    let dst = crv.cpts_mut();
    if dst.len() != pw.len() {
        return Err(out_of_range(OP));
    }
    dst.copy_from_slice(&pw);
    let dk = crv.knots_mut();
    if dk.len() != uk.len() {
        return Err(out_of_range(OP));
    }
    dk.copy_from_slice(&uk);
    Ok(())
}

/// Cubic spline interpolating the points `a[0..=k]` and the start and end
/// derivatives `ds`, `de` with parameters by `par` (upstream
/// `N_FitCubicSplineInterp`), solving the tridiagonal system directly; the knots are
/// the parameters (`k + 7` knots, `k + 3` control points). A zero `ds` / `de` is
/// estimated as in [`interp_end_derivs`].
///
/// # Errors
///
/// [`NlibCode::Input`] when `k == 0`; errors of the estimate and of
/// [`KnotVector::basis_eval`]; `InvalidInput` for empty `a` or fewer than four
/// points with a derivative to estimate.
#[allow(clippy::many_single_char_names)] // upstream's names
pub fn cubic_spline_interp(a: &[Vec3], ds: Vec3, de: Vec3, par: ParamMethod) -> Result<Curve> {
    const OP: &str = OP_CUBIC;
    let k = hi(a, OP, "no data points")?;

    if k <= 0 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    // If derivatives are missing we will estimate them.
    let (ds, de) = fill_end_derivs(a, k, ds, de, OP)?;

    // Check for curve memory.
    let n = k + 2;
    let mut uk = alloc(n + 4, 0.0, OP)?;
    let mut pw = alloc(n, CPoint::default(), OP)?;

    // Get parameters.
    let u = param_values(a, par)?;

    // Compute knot vector.
    for i in 0..=3 {
        put(&mut uk, i, 0.0, OP)?;
        put(&mut uk, n + i + 1, 1.0, OP)?;
    }
    for i in 1..k {
        put(&mut uk, i + 3, at(&u, i, OP)?, OP)?;
    }

    // Compute first two and last two control points.
    let alf = 1.0;
    let bet = at(&uk, 4, OP)? / 3.0;
    put(&mut pw, 0, CPoint::from_point(at(a, 0, OP)?), OP)?;
    let pt = combine2_pts(alf, at(a, 0, OP)?, bet, ds);
    put(&mut pw, 1, CPoint::from_point(pt), OP)?;

    let bet = (at(&uk, n, OP)? - 1.0) / 3.0;
    put(&mut pw, n, CPoint::from_point(at(a, k, OP)?), OP)?;
    let pt = combine2_pts(alf, at(a, k, OP)?, bet, de);
    put(&mut pw, n - 1, CPoint::from_point(pt), OP)?;

    if k == 1 {
        return curve(pw, 3, uk, OP);
    }

    let knt = KnotVector::from_slice(&uk)?;

    // Handle case k equal 2 special.
    if k == 2 {
        let (_, abc) = basis(&knt, 3, at(&uk, 4, OP)?, OP)?;
        let den = at(&abc, 1, OP)?;
        let alf = 1.0 / den;
        let bet = -at(&abc, 0, OP)? / den;
        let aw = CPoint::from_point(at(a, 1, OP)?);
        let p2 = CPoint::combine2(alf, aw, bet, at(&pw, 1, OP)?);
        put(&mut pw, 2, p2, OP)?;

        let alf = 1.0;
        let bet = -at(&abc, 2, OP)? / den;
        let p2 = CPoint::combine2(alf, at(&pw, 2, OP)?, bet, at(&pw, 3, OP)?);
        put(&mut pw, 2, p2, OP)?;
        return curve(pw, 3, uk, OP);
    }

    // Solve tridiagonal system.
    let mut rw = alloc(k, CPoint::default(), OP)?;
    let mut dd = alloc(k, 0.0, OP)?;
    for i in 3..k {
        put(&mut rw, i, CPoint::from_point(at(a, i - 1, OP)?), OP)?;
    }

    let (_, mut abc) = basis(&knt, 3, at(&uk, 4, OP)?, OP)?;
    let mut den = at(&abc, 1, OP)?;
    let alf = 1.0 / den;
    let bet = -at(&abc, 0, OP)? / den;
    let aw = CPoint::from_point(at(a, 1, OP)?);
    let p2 = CPoint::combine2(alf, aw, bet, at(&pw, 1, OP)?);
    put(&mut pw, 2, p2, OP)?;

    for i in 3..k {
        put(&mut dd, i, at(&abc, 2, OP)? / den, OP)?;
        abc = basis(&knt, 3, at(&uk, i + 2, OP)?, OP)?.1;
        den = at(&abc, 1, OP)? - at(&abc, 0, OP)? * at(&dd, i, OP)?;
        let alf = 1.0 / den;
        let bet = -at(&abc, 0, OP)? / den;
        let pi = CPoint::combine2(alf, at(&rw, i, OP)?, bet, at(&pw, i - 1, OP)?);
        put(&mut pw, i, pi, OP)?;
    }

    put(&mut dd, k, at(&abc, 2, OP)? / den, OP)?;

    let (_, abc) = basis(&knt, 3, at(&uk, n, OP)?, OP)?;
    let den = at(&abc, 1, OP)? - at(&abc, 0, OP)? * at(&dd, k, OP)?;
    let alf = 1.0 / den;
    let bet = -at(&abc, 2, OP)? / den;
    let gam = -at(&abc, 0, OP)? / den;

    let aw = CPoint::from_point(at(a, k - 1, OP)?);
    let mut pk = CPoint::combine2(alf, aw, bet, at(&pw, k + 1, OP)?);
    pk.blend(gam, at(&pw, k - 1, OP)?);
    put(&mut pw, k, pk, OP)?;

    for i in (2..k).rev() {
        let next = at(&pw, i + 1, OP)?;
        let f = -at(&dd, i + 1, OP)?;
        at_mut(&mut pw, i, OP)?.blend(f, next);
    }

    curve(pw, 3, uk, OP)
}

// ---- interpolation with first derivatives -----------------------------------------

/// Curve of degree `p` (2 or 3) interpolating the points `a[0..=kk]` and a first
/// derivative `d[i]` at every point, with parameters by `par` and the knot vector of
/// [`knots_derivs`] (upstream `N_FitCrvFirstDeriv`); it has `2kk + 2` control
/// points. With [`VectorKind::Tangent`] each `d[i]` is first normalised and scaled
/// by the length of the data polygon, in place. For `p == 2` only the first two of
/// the three basis function values and derivatives enter each row (as upstream).
///
/// # Errors
///
/// [`NlibCode::Degree`] when `p` is not 2 or 3; `NL_GEO_ERR` for a tangent that
/// cannot be normalised (the earlier ones stay rescaled); errors of
/// [`basis_derivs`], of the LU decomposition and of the solve; `InvalidInput` for
/// empty `a`, fewer than `kk + 1` derivatives or a matrix entry outside upstream's
/// storage.
pub fn interp_first_derivs(
    a: &[Vec3],
    d: &mut [Vec3],
    p: usize,
    vec: VectorKind,
    par: ParamMethod,
) -> Result<Curve> {
    const OP: &str = OP_FIRST;
    let kk = hi(a, OP, "no data points")?;
    let p = sg(p, OP)?;

    // Check for curve memory.
    if !(2..=3).contains(&p) {
        return Err(SolidError::nlib(OP, NlibCode::Degree));
    }
    let n = 2 * kk + 1;

    // Get parameters.
    let u = param_values(a, par)?;

    // Compute knot vector.
    let uk = knots_derivs(&u, idx(p, OP)?)?;
    let knt = KnotVector::from_slice(&uk)?;

    // Compute coefficient matrix.
    let bw = 2 * p - 1;
    let sbw = p - 1;
    let mut cm = banded(n, bw, OP)?;
    mset(&mut cm, 0, sbw, 1.0, OP)?;
    mset(&mut cm, 1, sbw - 1, -1.0, OP)?;
    mset(&mut cm, 1, sbw, 1.0, OP)?;
    mset(&mut cm, n - 1, sbw, -1.0, OP)?;
    mset(&mut cm, n - 1, sbw + 1, 1.0, OP)?;
    mset(&mut cm, n, sbw, 1.0, OP)?;

    let hp = if p == 3 { p } else { p - 1 };

    for i in 1..kk {
        let (j, nd_) = basis_d(&knt, p, at(&u, i, OP)?, 1, OP)?;
        let ii = 2 * i;
        let l = j - ii - 1;
        for k in 0..=hp {
            mset(&mut cm, ii, l + k, at2(&nd_, 0, k, OP)?, OP)?;
            mset(&mut cm, ii + 1, l + k - 1, at2(&nd_, 1, k, OP)?, OP)?;
        }
    }

    // LU decompose matrix.
    cm.lu_decompose()?;

    // Compute magnitudes if needed.
    if vec == VectorKind::Tangent {
        let alf = dist_polygon(a);
        for i in 0..=kk {
            let di = at_mut(d, i, OP)?;
            normalize_ref(di)?;
            *di = vscale(*di, alf);
        }
    }

    // Solve system of linear equations.
    let mut q = alloc(n, Vec3::ZERO, OP)?;
    let fact = at(&uk, p + 1, OP)? / p as f64;
    put(&mut q, 0, at(a, 0, OP)?, OP)?;
    put(&mut q, 1, vscale(at(d, 0, OP)?, fact), OP)?;
    for i in 1..kk {
        let ii = 2 * i;
        put(&mut q, ii, at(a, i, OP)?, OP)?;
        put(&mut q, ii + 1, at(d, i, OP)?, OP)?;
    }
    let fact = (1.0 - at(&uk, n, OP)?) / p as f64;
    put(&mut q, n - 1, vscale(at(d, kk, OP)?, fact), OP)?;
    put(&mut q, n, at(a, kk, OP)?, OP)?;

    let pw = cm.for_back_points(&q)?;
    curve(pw, p, uk, OP)
}

// ---- interpolation with end tangents and suggested knots --------------------------

/// Which end tangents are present (upstream `tan`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tan {
    No,
    Start,
    End,
    Both,
}

/// The ideal knot vector for `tan` and degree `p`; `None` where upstream's
/// `N_FitCalcKnotVectorDeriv` refuses (`p > k + 1`) and leaves the array unset.
fn tan_knots(ul: &[f64], p: usize, tan: Tan) -> Result<Option<Vec<f64>>> {
    let r = match tan {
        Tan::No => interp_knot_vector(ul, p),
        Tan::Start => knot_vector_deriv(ul, p, DerivEnd::Start),
        Tan::End => knot_vector_deriv(ul, p, DerivEnd::End),
        Tan::Both => knot_vector_end_derivs(ul, p),
    };
    match r {
        Ok(v) => Ok(Some(v)),
        Err(SolidError::Nlib {
            code: NlibCode::Input,
            ..
        }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `N_DistPolygon`, normalise, scale for one tangent (upstream "Get tangent
/// magnitudes").
fn rescale(t: &mut Vec3, fac: f64) -> Result<()> {
    normalize_ref(t)?;
    *t = vscale(*t, fac);
    Ok(())
}

/// Curve of degree `p >= 2` interpolating the points `a[0..=k]` and optional end
/// tangents (upstream `N_FitCrvKnotsAndTangents`).
///
/// The parameters are `u` or, when `None`, chord-length ones (mapped onto
/// `[U[0], U[mt]]` of a given knot vector). Without a knot vector (`*knt == None`)
/// the knots are the ideal ones for the end conditions and are returned in `knt`.
/// With one, each interior knot is taken from it where one lies in a window around
/// the ideal knot (`per` in `[0, 1]` sets the window width: `0` takes no knot,
/// `1` the most) and is the ideal knot otherwise; the ideal knots added this way are
/// merged into `knt`. With [`VectorKind::Tangent`] the tangents are normalised and
/// scaled by the polygon length times the parameter range, in place.
///
/// # Errors
///
/// [`NlibCode::Input`] for a given knot vector whose knots are all equal, `p < 2`,
/// `per` outside `[0, 1]`, `u` not spanning a given knot vector, or `p > k + 1`
/// with one tangent; [`NlibCode::Knot`] for an invalid given knot vector;
/// `NL_GEO_ERR` for a tangent that cannot be normalised; errors of
/// [`interp_given_params`], [`interp_knots_and_deriv`] and
/// [`interp_knots_and_derivs`]; `InvalidInput` for empty `a`, too few parameters, a
/// degree above `NL_MAXDEG` or a knot search leaving the given knot vector.
#[allow(clippy::too_many_lines)] // one upstream routine, kept in its original shape
pub fn interp_knots_and_tangents(
    a: &[Vec3],
    u: Option<&[f64]>,
    mut tangents: EndTangents<'_>,
    der: VectorKind,
    knt: &mut Option<Vec<f64>>,
    per: f64,
    p: usize,
) -> Result<Curve> {
    const OP: &str = OP_TANGENTS;
    let k = hi(a, OP, "no data points")?;
    let pu = p;
    let p = sg(p, OP)?;

    // Check incoming data.
    let mut mt: i64 = 0;
    let mut pt: i64 = 0;
    if let Some(ut) = knt.as_deref() {
        mt = hi(ut, OP, "empty knot vector")?;
        while pt < mt {
            if at(ut, pt, OP)? != at(ut, pt + 1, OP)? {
                break;
            }
            pt += 1;
        }
        if pt >= mt {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        KnotVector::from_slice(ut)?.validate(idx(pt, OP)?)?;
    }

    if p < 2 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if pu > MAX_DEGREE {
        return Err(SolidError::InvalidInput {
            operation: OP,
            reason: "degree above NL_MAXDEG (upstream scratch array size)",
        });
    }
    if per < 0.0 || per > 1.0 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    if let (Some(u), Some(ut)) = (u, knt.as_deref()) {
        if at(u, 0, OP)? != at(ut, 0, OP)? || at(u, k, OP)? != at(ut, mt, OP)? {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
    }

    // Check tangency conditions.
    let (tan, n) = match tangents {
        EndTangents::None => (Tan::No, k),
        EndTangents::Start(_) => (Tan::Start, k + 1),
        EndTangents::End(_) => (Tan::End, k + 1),
        EndTangents::Both(..) | EndTangents::Same(_) => (Tan::Both, k + 2),
    };

    // Compute parameters if not passed in.
    let ul: Vec<f64> = if let Some(u) = u {
        sub(u, 0, k, OP)?.to_vec()
    } else {
        {
            let mut ul = param_values(a, ParamMethod::ChordLength)?;
            if let Some(ut) = knt.as_deref() {
                let (u0, ue) = (at(ut, 0, OP)?, at(ut, mt, OP)?);
                if at(&ul, 0, OP)? != u0 || at(&ul, k, OP)? != ue {
                    let fac = (ue - u0) / (at(&ul, k, OP)? - at(&ul, 0, OP)?);
                    let l0 = at(&ul, 0, OP)?;
                    for i in 1..k {
                        let v = u0 + fac * (at(&ul, i, OP)? - l0);
                        put(&mut ul, i, v, OP)?;
                    }
                    put(&mut ul, 0, u0, OP)?;
                    put(&mut ul, k, ue, OP)?;
                }
            }
            ul
        }
    };

    // Get knot vector if none passed in.
    let mut ui = alloc(add(add(n, p, OP)?, 1, OP)?, 0.0, OP)?;
    let mut ideal_ok = true;
    if knt.is_none() {
        match tan_knots(&ul, pu, tan)? {
            Some(v) => ui = v,
            None => ideal_ok = false,
        }
    }

    // Get tangent magnitudes.
    if tan != Tan::No && der == VectorKind::Tangent {
        let mut fac = dist_polygon(a);
        fac *= at(&ul, k, OP)? - at(&ul, 0, OP)?;
        match &mut tangents {
            EndTangents::None => {}
            EndTangents::Start(ts) => rescale(ts, fac)?,
            EndTangents::End(te) => rescale(te, fac)?,
            EndTangents::Both(ts, te) => {
                rescale(ts, fac)?;
                rescale(te, fac)?;
            }
            EndTangents::Same(t) => {
                // Ts and Te are one vector: it is rescaled twice.
                rescale(t, fac)?;
                rescale(t, fac)?;
            }
        }
    }

    // Select knots for interpolation.
    let mut us = alloc(n + p + 1, 0.0, OP)?;
    let mut ms: i64 = -1;
    if let Some(ut) = knt.as_deref() {
        let (u0, ue) = (at(&ul, 0, OP)?, at(&ul, k, OP)?);
        for i in 0..=p {
            put(&mut ui, i, u0, OP)?;
            put(&mut ui, n + i + 1, ue, OP)?;
        }

        // Get ideal knot vector for degree p and for degree p-1 (unset where
        // upstream's knot routine refuses; the loop below then never runs).
        let ua = tan_knots(&ul, pu, tan)?.unwrap_or_default();
        let up = tan_knots(&ul, pu - 1, tan)?.unwrap_or_default();

        // Select knots from knt.
        let mut j = pt + 1;
        for i in p + 1..=n {
            let uai = at(&ua, i, OP)?;
            let lo = (1.0 - per) * uai + per * at(&up, i - 1, OP)?;
            let hi_ = (1.0 - per) * uai + per * at(&up, i, OP)?;

            // See if there are knots in (lo,hi). If yes, select the one closest to
            // UA[i]. If not, add UA[i] to knt.
            while at(ut, j, OP)? <= lo {
                j += 1;
            }
            let mut fac = hi_ - lo;
            let mut jc: i64 = -1;
            loop {
                let utj = at(ut, j, OP)?;
                if !(utj > lo && utj < hi_) {
                    break;
                }
                let d = (uai - utj).abs();
                if d < fac {
                    fac = d;
                    jc = j;
                }
                j += 1;
            }
            if jc == -1 {
                put(&mut ui, i, uai, OP)?;
                ms += 1;
                put(&mut us, ms, uai, OP)?;
            } else {
                put(&mut ui, i, at(ut, jc, OP)?, OP)?;
            }
        }
    }

    // Now interpolate data points.
    if !ideal_ok {
        // N_FitCrvKnotsAndDeriv: p > k + 1 = n.
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let kni = KnotVector::from_slice(&ui)?;
    let cur = match &tangents {
        EndTangents::None => interp_given_params(a, &ul, &kni, pu)?,
        EndTangents::Start(ts) => interp_knots_and_deriv(a, &ul, &kni, pu, **ts, DerivEnd::Start)?,
        EndTangents::End(te) => interp_knots_and_deriv(a, &ul, &kni, pu, **te, DerivEnd::End)?,
        EndTangents::Both(ts, te) => interp_knots_and_derivs(a, &ul, &kni, pu, **ts, **te)?,
        EndTangents::Same(t) => interp_knots_and_derivs(a, &ul, &kni, pu, **t, **t)?,
    };

    // Update input knot vector.
    match knt.as_deref() {
        None => *knt = Some(ui),
        Some(ut) if ms > -1 => {
            let total = mt + ms + 1;
            let mut uo = alloc(total, 0.0, OP)?;
            let mut mo: i64 = -1;
            let mut i: i64 = 0;
            let mut j: i64 = 0;
            while i <= mt && j <= ms {
                let (uti, usj) = (at(ut, i, OP)?, at(&us, j, OP)?);
                mo += 1;
                if uti < usj {
                    put(&mut uo, mo, uti, OP)?;
                    i += 1;
                } else {
                    put(&mut uo, mo, usj, OP)?;
                    j += 1;
                }
            }
            while i <= mt {
                mo += 1;
                put(&mut uo, mo, at(ut, i, OP)?, OP)?;
                i += 1;
            }
            if mo != total {
                // Upstream would leave the last knots unset.
                return Err(invalid(OP, "merged knot vector not filled"));
            }
            *knt = Some(uo);
        }
        Some(_) => {}
    }

    Ok(cur)
}

// ---- interpolation with higher end derivatives ------------------------------------

/// Curve of degree `p` interpolating the closed point set `a[0..=n]` (`a[0] ==
/// a[n]`) with the derivatives `1..=k` equal at both ends, parameters by `par`
/// (upstream `N_FitCrvApproxClosedConditions`), via [`interp_high_derivs`]. The
/// derivatives are `d[0..k]` (upstream `D[1..=k]`) or, when `None`, those of a local
/// interpolant of degree `p` through the points around the start point.
///
/// # Errors
///
/// Those of [`interp_given_params`], [`Curve::derivs`] and [`interp_high_derivs`];
/// `InvalidInput` for empty `a`, fewer than `k` derivatives or local data outside
/// `a`.
pub fn approx_closed_conditions(
    a: &[Vec3],
    p: usize,
    d: Option<&[Vec3]>,
    k: usize,
    par: ParamMethod,
) -> Result<Curve> {
    const OP: &str = OP_CLOSED;
    let n = hi(a, OP, "no data points")?;
    let ki = sg(k, OP)?;
    let pi = sg(p, OP)?;

    // Allocate memory for end derivatives (upstream Ds[1..=k], De[1..=k]).
    let mut ds = alloc(ki - 1, Vec3::ZERO, OP)?;
    let mut de = alloc(ki - 1, Vec3::ZERO, OP)?;

    if let Some(d) = d {
        // Derivatives passed in.
        for i in 1..=ki {
            let v = at(d, i - 1, OP)?;
            put(&mut ds, i - 1, v, OP)?;
            put(&mut de, i - 1, v, OP)?;
        }
    } else {
        // No derivatives are passed in; compute them internally.
        let nm = n / 2;
        let is = nm.max(n - pi - 1);
        let ie = nm.min(add(pi, 1, OP)?);
        let nl = n - is + ie;

        let mut q = alloc(nl, Vec3::ZERO, OP)?;
        let mut u = alloc(nl, 0.0, OP)?;

        // Get data points for local fitting.
        let v = param_values(a, par)?;
        put(&mut q, 0, at(a, is, OP)?, OP)?;
        put(&mut u, 0, 0.0, OP)?;

        let mut i = is + 1;
        let mut j: i64 = 1;
        let mut id: i64 = 0;
        while j <= nl {
            put(&mut q, j, at(a, i, OP)?, OP)?;
            let uj = at(&u, j - 1, OP)? + (at(&v, i, OP)? - at(&v, i - 1, OP)?);
            put(&mut u, j, uj, OP)?;
            if i == n {
                i = 0;
                id = j;
            }
            i += 1;
            j += 1;
        }

        // Interpolate local data through the start point.
        let uk = interp_knot_vector(&u, p)?;
        let knt = KnotVector::from_slice(&uk)?;
        let cur_i = interp_given_params(&q, &u, &knt, p)?;

        // Get derivatives off the local interpolant.
        let dl = cur_i.derivs(at(&u, id, OP)?, Side::Left, k)?;
        for i in 1..=ki {
            let v = at(&dl, i, OP)?;
            put(&mut ds, i - 1, v, OP)?;
            put(&mut de, i - 1, v, OP)?;
        }
    }

    // Interpolate data set.
    interp_high_derivs(a, p, &ds, &de, par)
}

/// Curve of degree `p` interpolating the points `a[0..=nn]`, the start derivatives
/// `ds` (orders `1..=kk`, `kk = ds.len()`) and the end derivatives `de` (orders
/// `1..=ll`), with parameters by `par` and the knot vector of
/// [`knots_high_end_derivs`] (upstream `N_FitCrvHighDerivs`); it has
/// `nn + kk + ll + 1` control points.
///
/// # Errors
///
/// [`NlibCode::Input`] when `p < 1`, `p > nn + kk + ll`, `kk >= p` or `ll >= p`;
/// errors of [`basis_derivs`], [`KnotVector::basis_eval`], of the LU decomposition
/// and of the solve; `InvalidInput` for empty `a` or a matrix entry outside
/// upstream's storage.
pub fn interp_high_derivs(
    a: &[Vec3],
    p: usize,
    ds: &[Vec3],
    de: &[Vec3],
    par: ParamMethod,
) -> Result<Curve> {
    const OP: &str = OP_HIGH;
    let nn = hi(a, OP, "no data points")?;
    let p = sg(p, OP)?;
    let kk = sg(ds.len(), OP)?;
    let ll = sg(de.len(), OP)?;

    // Check for curve memory.
    let n = nn + kk + ll;
    if p < 1 || p > n {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if kk >= p || ll >= p {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    // Get parameters and compute knot vector.
    let u = param_values(a, par)?;
    let uk = knots_high_end_derivs(&u, idx(p, OP)?, ds.len(), de.len())?;
    let knt = KnotVector::from_slice(&uk)?;

    // Compute coefficient matrix.
    let bw = 2 * p - 1;
    let sbw = p - 1;
    let mut cm = banded(n, bw, OP)?;
    mset(&mut cm, 0, sbw, 1.0, OP)?;

    let mut row: i64 = 1;
    let (_, nds) = basis_d(&knt, p, at(&u, 0, OP)?, kk, OP)?;
    for i in 1..=kk {
        for j in 0..=i {
            mset(&mut cm, row, sbw - i + j, at2(&nds, i, j, OP)?, OP)?;
        }
        row += 1;
    }

    for i in 1..nn {
        let (j, nb) = basis(&knt, p, at(&u, i, OP)?, OP)?;
        let l = j - row - 1;
        for k in 0..=p {
            mset(&mut cm, row, l + k, at(&nb, k, OP)?, OP)?;
        }
        row += 1;
    }

    let (_, nde) = basis_d(&knt, p, at(&u, nn, OP)?, ll, OP)?;
    for i in (1..=ll).rev() {
        for j in 0..=i {
            mset(&mut cm, row, sbw + j, at2(&nde, i, p - i + j, OP)?, OP)?;
        }
        row += 1;
    }
    mset(&mut cm, row, sbw, 1.0, OP)?;

    // LU decompose matrix.
    cm.lu_decompose()?;

    // Solve system of linear equations.
    let mut q = alloc(n, Vec3::ZERO, OP)?;
    put(&mut q, 0, at(a, 0, OP)?, OP)?;
    let mut row: i64 = 1;
    for i in 1..=kk {
        put(&mut q, row, at(ds, i - 1, OP)?, OP)?;
        row += 1;
    }
    for i in 1..nn {
        put(&mut q, row, at(a, i, OP)?, OP)?;
        row += 1;
    }
    for i in (1..=ll).rev() {
        put(&mut q, row, at(de, i - 1, OP)?, OP)?;
        row += 1;
    }
    put(&mut q, row, at(a, nn, OP)?, OP)?;

    let pw = cm.for_back_points(&q)?;
    curve(pw, p, uk, OP)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts() -> Vec<Vec3> {
        [
            (-11.0, 2.0),
            (-11.0, 5.0),
            (-10.0, 7.0),
            (-6.0, 4.0),
            (-2.0, 7.0),
            (2.0, 6.0),
            (4.0, 6.0),
            (6.0, 7.0),
            (9.0, 8.0),
            (12.0, 6.0),
            (13.0, 4.0),
            (13.0, 2.0),
        ]
        .iter()
        .map(|&(x, y)| Vec3::new(x, y, 0.0))
        .collect()
    }

    fn close(a: Vec3, b: Vec3, tol: f64) -> bool {
        (a - b).length() <= tol
    }

    fn at_param(c: &Curve, u: f64, der: usize) -> Vec<Vec3> {
        c.derivs(u, Side::Left, der).unwrap()
    }

    #[test]
    fn end_derivs_interpolates_points_and_derivatives() {
        let a = pts();
        let ds = Vec3::new(3.0, 4.0, 0.0);
        let de = Vec3::new(0.0, -5.0, 0.0);
        let mut slot = None;
        interp_end_derivs(
            &a,
            3,
            ds,
            de,
            ParamSource::Method(ParamMethod::ChordLength),
            &mut slot,
        )
        .unwrap();
        let c = slot.unwrap();
        assert_eq!(c.n(), a.len() + 1);
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        for (q, t) in a.iter().zip(&u) {
            assert!(close(at_param(&c, *t, 0)[0], *q, 1e-9));
        }
        assert!(close(at_param(&c, 0.0, 1)[1], ds, 1e-9));
        assert!(close(at_param(&c, 1.0, 1)[1], de, 1e-9));
    }

    #[test]
    fn end_derivs_inherited_keeps_knots_and_checks_storage() {
        let a = pts();
        let n = a.len() + 1;
        let p = 3;
        let ni = n - p;
        let mut knots = vec![0.0; p + 1];
        knots.extend((1..=ni).map(|i| i as f64 / (ni + 1) as f64));
        knots.extend(vec![1.0; p + 1]);
        let cur = Curve::new(vec![CPoint::default(); n + 1], p, knots.clone()).unwrap();
        let mut slot = Some(cur.clone());
        let ds = Vec3::new(1.0, 0.0, 0.0);
        interp_end_derivs(&a, p, ds, ds, ParamSource::Inherited, &mut slot).unwrap();
        assert_eq!(slot.unwrap().knots(), knots.as_slice());

        let small = Curve::new(vec![CPoint::default(); n], p, knots).unwrap();
        let mut slot = Some(small);
        let r = interp_end_derivs(&a, p, ds, ds, ParamSource::Inherited, &mut slot);
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Storage,
                ..
            })
        ));
    }

    #[test]
    fn end_derivs_rejects_degree_and_short_estimates() {
        let a = pts();
        let mut slot = None;
        let v = Vec3::new(1.0, 0.0, 0.0);
        let r = interp_end_derivs(
            &a,
            1,
            v,
            v,
            ParamSource::Method(ParamMethod::Uniform),
            &mut slot,
        );
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
        let two = &a[..2];
        let r = interp_end_derivs(
            two,
            3,
            Vec3::ZERO,
            v,
            ParamSource::Method(ParamMethod::Uniform),
            &mut slot,
        );
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
    }

    #[test]
    fn cubic_spline_interpolates() {
        let a = pts();
        let ds = Vec3::new(3.0, 4.0, 0.0);
        let de = Vec3::new(0.0, -5.0, 0.0);
        let c = cubic_spline_interp(&a, ds, de, ParamMethod::ChordLength).unwrap();
        assert_eq!((c.n(), c.degree(), c.m()), (a.len() + 1, 3, a.len() + 5));
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        for (q, t) in a.iter().zip(&u) {
            assert!(close(at_param(&c, *t, 0)[0], *q, 1e-9));
        }
        assert!(close(at_param(&c, 0.0, 1)[1], ds, 1e-9));
        assert!(close(at_param(&c, 1.0, 1)[1], de, 1e-9));
        // Small cases and the estimate.
        for k in 1..4 {
            let c = cubic_spline_interp(&a[..=k], ds, de, ParamMethod::Uniform).unwrap();
            assert!(close(at_param(&c, 1.0, 0)[0], a[k], 1e-9));
        }
        assert!(cubic_spline_interp(&a, Vec3::ZERO, Vec3::ZERO, ParamMethod::Uniform).is_ok());
        let r = cubic_spline_interp(&a[..1], ds, de, ParamMethod::Uniform);
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn first_derivs_interpolates_points_and_rescales() {
        let a = pts();
        let mut d: Vec<Vec3> = (0..a.len())
            .map(|i| a[(i + 1).min(a.len() - 1)] - a[i.saturating_sub(1)])
            .collect();
        let c = interp_first_derivs(&a, &mut d, 3, VectorKind::Tangent, ParamMethod::ChordLength)
            .unwrap();
        assert_eq!(c.n(), 2 * a.len() - 1);
        let len = dist_polygon(&a);
        for v in &d {
            assert!((v.length() - len).abs() <= 1e-9 * len);
        }
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        for (q, t) in a.iter().zip(&u) {
            assert!(close(at_param(&c, *t, 0)[0], *q, 1e-9));
        }
        let r = interp_first_derivs(&a, &mut d, 4, VectorKind::Derivative, ParamMethod::Uniform);
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Degree,
                ..
            })
        ));
    }

    #[test]
    fn knots_and_tangents_returns_and_merges_knots() {
        let a = pts();
        let mut knt = None;
        let c = interp_knots_and_tangents(
            &a,
            None,
            EndTangents::None,
            VectorKind::Tangent,
            &mut knt,
            1.0,
            3,
        )
        .unwrap();
        assert_eq!(knt.as_deref(), Some(c.knots()));

        // A coarse suggested knot vector gets the missing ideal knots merged in.
        let mut knt = Some(vec![0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0]);
        let mut ts = Vec3::new(1.0, 1.0, 0.0);
        let c = interp_knots_and_tangents(
            &a,
            None,
            EndTangents::Same(&mut ts),
            VectorKind::Tangent,
            &mut knt,
            0.8,
            3,
        )
        .unwrap();
        let merged = knt.unwrap();
        assert!(merged.len() > 9);
        assert!(merged.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(c.n(), a.len() + 1);
        assert!(close(at_param(&c, 0.0, 1)[1], ts, 1e-9));

        let mut knt = Some(vec![0.5; 6]);
        let r = interp_knots_and_tangents(
            &a,
            None,
            EndTangents::None,
            VectorKind::Tangent,
            &mut knt,
            1.0,
            3,
        );
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
        let mut knt = None;
        let r = interp_knots_and_tangents(
            &a,
            None,
            EndTangents::None,
            VectorKind::Tangent,
            &mut knt,
            1.5,
            3,
        );
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
        let mut t = Vec3::new(1.0, 0.0, 0.0);
        let r = interp_knots_and_tangents(
            &a[..2],
            None,
            EndTangents::Start(&mut t),
            VectorKind::Derivative,
            &mut knt,
            1.0,
            3,
        );
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn closed_conditions_closes_smoothly() {
        let mut a = pts();
        a.push(a[0]);
        let c = approx_closed_conditions(&a, 3, None, 2, ParamMethod::ChordLength).unwrap();
        let s = at_param(&c, 0.0, 2);
        let e = c.derivs(1.0, Side::Right, 2).unwrap();
        for (x, y) in s.iter().zip(&e) {
            assert!(close(*x, *y, 1e-8 * (1.0 + x.length())));
        }
        let d = [Vec3::new(5.0, 0.0, 0.0)];
        let c = approx_closed_conditions(&a, 3, Some(&d), 1, ParamMethod::Uniform).unwrap();
        assert!(close(at_param(&c, 0.0, 1)[1], d[0], 1e-9));
        let r = approx_closed_conditions(&a, 2, None, 2, ParamMethod::Uniform);
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn high_derivs_interpolates_end_derivatives() {
        let a = pts();
        let ds = [Vec3::new(3.0, 4.0, 0.0), Vec3::new(1.0, -2.0, 0.5)];
        let de = [Vec3::new(0.0, -5.0, 0.0)];
        let c = interp_high_derivs(&a, 4, &ds, &de, ParamMethod::ChordLength).unwrap();
        assert_eq!(c.n(), a.len() + 2);
        let s = at_param(&c, 0.0, 2);
        assert!(close(s[0], a[0], 1e-9));
        assert!(close(s[1], ds[0], 1e-9));
        assert!(close(s[2], ds[1], 1e-8));
        assert!(close(
            c.derivs(1.0, Side::Right, 1).unwrap()[1],
            de[0],
            1e-9
        ));
        let r = interp_high_derivs(&a, 2, &ds, &[], ParamMethod::Uniform);
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
        let r = interp_high_derivs(&a, 0, &[], &[], ParamMethod::Uniform);
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    /// Fuzz regressions: an absurd derivative count fails with `NL_MEM_ERR` (as a
    /// failed upstream allocation) and an absurd degree with an error, no abort.
    #[test]
    fn absurd_counts_are_errors() {
        let a = pts();
        assert!(matches!(
            approx_closed_conditions(&a, 3, None, 1 << 62, ParamMethod::Uniform),
            Err(SolidError::Nlib {
                code: NlibCode::Memory,
                ..
            })
        ));
        assert!(
            approx_closed_conditions(&a, i64::MAX as usize, None, 1, ParamMethod::Uniform).is_err()
        );
    }
}
