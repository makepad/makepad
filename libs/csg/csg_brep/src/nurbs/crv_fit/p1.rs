//! Curve fitting, part 1: parameter values for data points, knot vectors for global
//! interpolation (plain, with end derivatives) and approximation, global
//! interpolation with given parameters and knots (plain, with one or two end
//! derivatives, with first derivatives at every point), least-squares approximation
//! with given knots, and the control point solves that reuse an LU-decomposed
//! interpolation matrix.
//!
//! Upstream: `source/SMLib/src/NL_CrvFit.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvFit.h`; flags in `source/SMLib/inc/NL_DataStruct.h`).
//!
//! | Rust                                  | Upstream                          |
//! |---------------------------------------|-----------------------------------|
//! | [`FitPoint`] (`Vec3` / [`CPoint`])    | `NL_EPOINT` / `NL_HPOINT` data behind `NL_VOID *` |
//! | [`ParamMethod`]                       | `NL_UNIFORM` / `NL_CHORDLENGTH` / `NL_CENTRIPETAL` |
//! | [`DerivEnd`]                          | `NL_START` / `NL_END`             |
//! | [`DataDir`]                           | `NL_UDIR` / `NL_VDIR`             |
//! | [`param_values`]                      | `N_FitCalcCrvParamValues`         |
//! | [`interp_knot_vector`]                | `N_FitCrvCalcKnotVector`          |
//! | [`knot_vector_end_derivs`]            | `N_FitCalcKnotVectorEndDerivs`    |
//! | [`knot_vector_deriv`]                 | `N_FitCalcKnotVectorDeriv`        |
//! | [`knot_vector_approx`]                | `N_FitCalcKnotVectorCrvApprox`    |
//! | [`interp_given_params`]               | `N_FitCrvInterpGivenParams`       |
//! | [`interp_knots_and_derivs`]           | `N_FitCrvKnotsAndDerivs`          |
//! | [`interp_knots_and_deriv`]            | `N_FitCrvKnotsAndDeriv`           |
//! | [`interp_first_derivs_and_knots`]     | `N_FitCrvFirstDerivAndKnots`      |
//! | [`approx_knots`]                      | `N_FitCrvApproxKnots`             |
//! | [`cpts_from_srf_data`]                | `N_FitCrvCPtsFromSrfData`         |
//! | [`derivs_matrix`]                     | `N_FitCrvDerivsMatrix`            |
//!
//! Deviations:
//! - The `NL_VOID *` data plus `NL_EPOINT` / `NL_HPOINT` flag pairs become slices of a
//!   [`FitPoint`] type (`Vec3` or [`CPoint`]); derivative vectors have the same type as
//!   the points. The flag enums ([`ParamMethod`], [`DerivEnd`], [`DataDir`]) cannot
//!   hold the other values upstream rejects with `NL_CAL_ERR` / `NL_INP_ERR`.
//! - Highest indices come from slice lengths (`n = a.len() - 1` for the data, `k` /
//!   `kk` likewise); the other counts (`n` of [`approx_knots`] and
//!   [`knot_vector_approx`]) stay arguments.
//! - Out-params become return values: knot vectors are returned as `Vec<f64>` of
//!   exactly the length upstream sets with `N_SetKnotIndex`, curves as new
//!   [`Curve`]s (so `N_CrvSizeArrays`' `NL_STO_ERR` for a too small caller curve
//!   cannot occur), control points as `Vec<CPoint>`. Entries upstream leaves
//!   unwritten in a caller buffer (interior parameters when the total chord length
//!   is at most [`PTOL`], interior knots of [`knot_vector_deriv`] for `p == 0`) are
//!   `0.0`. Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR`
//!   number; outputs upstream leaves partly written on error are not returned.
//! - Upstream's 2D work arrays and banded matrices are single row-major blocks, and
//!   the port indexes them the same way: `N_FitCrvFirstDerivAndKnots` writes the
//!   first derivative entry of each point to `M[2i+1][-1]`, which is the last entry
//!   of row `2i` of the block; the port writes exactly there. Accesses outside a whole
//!   block, outside the data / parameter / knot slices, or to entries upstream never
//!   initialised (a [`cpts_from_srf_data`] matrix larger than the extracted row or
//!   column) return [`SolidError::InvalidInput`] instead of undefined behaviour.
//! - The `NL_STACKS` arguments and the routine-name strings have no counterpart.
//! - Work arrays that cannot be reserved return [`NlibCode::Memory`] (upstream's
//!   `NL_MEM_ERR` from a failed `N_Malloc`) instead of aborting. Index arithmetic on
//!   caller counts (`n + p + 1`, `p + 1`, `2 * p`, ...) that would overflow `NL_INDEX`
//!   (undefined behaviour upstream) returns [`SolidError::InvalidInput`]; where the
//!   sum is only compared against the knot count, an overflowing sum counts as a
//!   mismatch (`NL_INP_ERR`, as upstream's wrapped sum), and sums whose degree check
//!   comes later are formed after that check (the order is not observable).

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_basic::{basis_derivs, PTOL};
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::fit_arrays::{
    add, alloc, at, at_mut, banded, basis, copy_knots, curve, hi, idx, mget, mset, out_of_range,
    put, sg, Block,
};
use crate::nurbs::geom::{dist_cpt_cpt_homo, dist_pt_pt};
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{blend_pt, translate_sum2_pts, CPoint, RealMatrix};

mod sealed {
    pub trait Sealed {}
    impl Sealed for crate::math::Vec3 {}
    impl Sealed for crate::nurbs::math::CPoint {}
}

/// Data accepted by the fitting routines: Euclidean points (`Vec3`, upstream
/// `NL_EPOINT`) or homogeneous control points ([`CPoint`], `NL_HPOINT`).
pub trait FitPoint: Copy + sealed::Sealed {
    /// `NL_ZERO` / `NL_CZERO`.
    const ZERO: Self;
    /// Distance used for parametrisation (`N_DistPtPt` / `N_DistCptCptHomo`).
    fn dist(self, other: Self) -> f64;
    /// Control point holding this datum (`N_PtToCPt` / `N_CopyCPt`).
    fn to_cpoint(self) -> CPoint;
    /// `alpha * self` (`N_ScalePt` / `N_ScaleCPt`).
    #[must_use]
    fn scaled(self, alpha: f64) -> Self;
    /// `c + alpha*a + beta*b` (`N_TranslateSum2Pts` / `N_TranslateSum2CPts`).
    fn translate_sum2(c: Self, alpha: f64, a: Self, beta: f64, b: Self) -> Self;
    /// `self += alpha * a` (`N_VectorBlendPt` / `N_VectorBlendCPt`).
    fn blend(&mut self, alpha: f64, a: Self);
    /// Solves `LU * x = rhs` with an LU-decomposed matrix (`N_RealMatrixForBack`).
    ///
    /// # Errors
    ///
    /// Those of [`RealMatrix::for_back_points`] / [`RealMatrix::for_back_cpoints`].
    fn for_back(cm: &RealMatrix, rhs: &[Self]) -> Result<Vec<CPoint>>;
}

impl FitPoint for Vec3 {
    const ZERO: Self = Vec3::ZERO;
    fn dist(self, other: Self) -> f64 {
        dist_pt_pt(self, other)
    }
    fn to_cpoint(self) -> CPoint {
        CPoint::from_point(self)
    }
    fn scaled(self, alpha: f64) -> Self {
        Vec3::new(alpha * self.x, alpha * self.y, alpha * self.z)
    }
    fn translate_sum2(c: Self, alpha: f64, a: Self, beta: f64, b: Self) -> Self {
        translate_sum2_pts(c, alpha, a, beta, b)
    }
    fn blend(&mut self, alpha: f64, a: Self) {
        blend_pt(self, alpha, a);
    }
    fn for_back(cm: &RealMatrix, rhs: &[Self]) -> Result<Vec<CPoint>> {
        cm.for_back_points(rhs)
    }
}

impl FitPoint for CPoint {
    const ZERO: Self = CPoint::ZERO;
    fn dist(self, other: Self) -> f64 {
        dist_cpt_cpt_homo(self, other)
    }
    fn to_cpoint(self) -> CPoint {
        self
    }
    fn scaled(self, alpha: f64) -> Self {
        self.scale(alpha)
    }
    fn translate_sum2(c: Self, alpha: f64, a: Self, beta: f64, b: Self) -> Self {
        CPoint::translate_sum2(c, alpha, a, beta, b)
    }
    fn blend(&mut self, alpha: f64, a: Self) {
        CPoint::blend(self, alpha, a);
    }
    fn for_back(cm: &RealMatrix, rhs: &[Self]) -> Result<Vec<CPoint>> {
        cm.for_back_cpoints(rhs)
    }
}

/// Parametrisation of data points for global interpolation (upstream `par` flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMethod {
    /// `NL_UNIFORM` (1): equally spaced parameters.
    Uniform,
    /// `NL_CHORDLENGTH` (2): parameters proportional to chord lengths.
    ChordLength,
    /// `NL_CENTRIPETAL` (3): parameters proportional to square roots of chord lengths.
    Centripetal,
}

impl ParamMethod {
    /// The upstream flag value.
    #[must_use]
    pub fn flag(self) -> i32 {
        match self {
            ParamMethod::Uniform => 1,
            ParamMethod::ChordLength => 2,
            ParamMethod::Centripetal => 3,
        }
    }
}

/// End of the curve at which a single derivative is given (upstream `whr` flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivEnd {
    /// `NL_START` (1).
    Start,
    /// `NL_END` (2).
    End,
}

impl DerivEnd {
    /// The upstream flag value.
    #[must_use]
    pub fn flag(self) -> i32 {
        match self {
            DerivEnd::Start => 1,
            DerivEnd::End => 2,
        }
    }
}

/// Direction of a row / column of surface data (upstream `dir` flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDir {
    /// `NL_UDIR` (1): the points `A[i][k]`, `i = 0..=n` (a column of the grid).
    U,
    /// `NL_VDIR` (2): the points `A[k][j]`, `j = 0..=m` (a row of the grid).
    V,
}

impl DataDir {
    /// The upstream flag value.
    #[must_use]
    pub fn flag(self) -> i32 {
        match self {
            DataDir::U => 1,
            DataDir::V => 2,
        }
    }
}

// ---- parameters and knot vectors --------------------------------------------------

/// Parameters `u[0..=n]` for global interpolation of the points `a[0..=n]`
/// (upstream `N_FitCalcCrvParamValues`): uniform, chord length or centripetal, with
/// `u[0] = 0` and `u[n] = 1`.
///
/// When the total (square-rooted) chord length is at most [`PTOL`] the interior
/// parameters are `0.0` (upstream leaves the caller's array unwritten). For a single
/// point the result is `[1.0]` (upstream writes `u[0] = 0` and then `u[n] = 1`).
///
/// # Errors
///
/// `InvalidInput` for empty `a`.
pub fn param_values<P: FitPoint>(a: &[P], par: ParamMethod) -> Result<Vec<f64>> {
    const OP: &str = "nurbs::crv_fit::p1::param_values";
    let n = hi(a, OP, "no data points")?;
    let mut u = alloc(n, 0.0, OP)?;

    // Compute uniform parametrization.
    put(&mut u, 0, 0.0, OP)?;
    put(&mut u, n, 1.0, OP)?;
    if par == ParamMethod::Uniform {
        let fact = 1.0 / n as f64;
        for i in 1..n {
            put(&mut u, i, i as f64 * fact, OP)?;
        }
        return Ok(u);
    }

    // Compute chord length or centripetal parametrization.
    let mut dist = alloc(n, 0.0, OP)?;
    let mut sum = 0.0;
    for i in 1..=n {
        let mut d = at(a, i - 1, OP)?.dist(at(a, i, OP)?);
        if par == ParamMethod::Centripetal {
            d = d.sqrt();
        }
        put(&mut dist, i, d, OP)?;
        sum += d;
    }
    if sum > PTOL {
        for i in 1..n {
            let v = at(&u, i - 1, OP)? + at(&dist, i, OP)? / sum;
            put(&mut u, i, v, OP)?;
        }
    }
    Ok(u)
}

/// Knot vector for global interpolation of points at parameters `u[0..=n]` with
/// degree `p` (upstream `N_FitCrvCalcKnotVector`): `p+1` knots `u[0]`, the averages
/// of `p` consecutive interior parameters, `p+1` knots `u[n]` (`n + p + 2` knots).
///
/// # Errors
///
/// `InvalidInput` for empty `u`.
pub fn interp_knot_vector(u: &[f64], p: usize) -> Result<Vec<f64>> {
    const OP: &str = "nurbs::crv_fit::p1::interp_knot_vector";
    let n = hi(u, OP, "no parameters")?;
    let p = sg(p, OP)?;
    let mut uk = alloc(add(add(n, p, OP)?, 1, OP)?, 0.0, OP)?;

    let (u0, un) = (at(u, 0, OP)?, at(u, n, OP)?);
    for i in 0..=p {
        put(&mut uk, i, u0, OP)?;
        put(&mut uk, n + i + 1, un, OP)?;
    }
    for i in 1..=n - p {
        let mut sum = 0.0;
        for j in i..i + p {
            sum += at(u, j, OP)?;
        }
        put(&mut uk, i + p, sum / p as f64, OP)?;
    }
    Ok(uk)
}

/// Knot vector for global interpolation of points at parameters `u[0..=k]` plus both
/// end derivatives, degree `p` (upstream `N_FitCalcKnotVectorEndDerivs`): `k + p + 4`
/// knots.
///
/// # Errors
///
/// `InvalidInput` for empty `u`, or `p == 1` with a single parameter (upstream reads
/// `u[1]`).
pub fn knot_vector_end_derivs(u: &[f64], p: usize) -> Result<Vec<f64>> {
    const OP: &str = "nurbs::crv_fit::p1::knot_vector_end_derivs";
    let k = hi(u, OP, "no parameters")?;
    let p = sg(p, OP)?;
    let n = k + 2;
    let mut uk = alloc(add(add(n, p, OP)?, 1, OP)?, 0.0, OP)?;

    let (u0, uk_end) = (at(u, 0, OP)?, at(u, k, OP)?);
    for i in 0..=p {
        put(&mut uk, i, u0, OP)?;
        put(&mut uk, n + i + 1, uk_end, OP)?;
    }
    if p == 1 {
        if k > 1 {
            put(&mut uk, 2, 0.5 * (at(u, 0, OP)? + at(u, 1, OP)?), OP)?;
            put(&mut uk, n, 0.5 * (at(u, k, OP)? + at(u, k - 1, OP)?), OP)?;
        } else {
            put(&mut uk, 2, 0.75 * at(u, 0, OP)? + 0.25 * at(u, 1, OP)?, OP)?;
            put(&mut uk, 3, 0.25 * at(u, 0, OP)? + 0.75 * at(u, 1, OP)?, OP)?;
        }
        for i in 1..k {
            put(&mut uk, i + 2, at(u, i, OP)?, OP)?;
        }
    } else {
        for i in 0..=k - p + 1 {
            let mut sum = 0.0;
            for j in i..i + p {
                sum += at(u, j, OP)?;
            }
            put(&mut uk, i + p + 1, sum / p as f64, OP)?;
        }
    }
    Ok(uk)
}

/// Knot vector for global interpolation of points at parameters `u[0..=k]` plus one
/// end derivative at `whr`, degree `p` (upstream `N_FitCalcKnotVectorDeriv`):
/// `k + p + 3` knots. Interior knots upstream leaves unwritten (`p == 0`) are `0.0`.
///
/// # Errors
///
/// [`NlibCode::Input`] when `p > k + 1`; `InvalidInput` for empty `u`, or `p == 1`
/// with a single parameter (upstream reads `u[1]` or `u[-1]`).
pub fn knot_vector_deriv(u: &[f64], p: usize, whr: DerivEnd) -> Result<Vec<f64>> {
    const OP: &str = "nurbs::crv_fit::p1::knot_vector_deriv";
    let k = hi(u, OP, "no parameters")?;
    let p = sg(p, OP)?;

    // Check error.
    if p > k + 1 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    // Compute knot vector.
    let n = k + 1;
    let mut uk = alloc(add(add(n, p, OP)?, 1, OP)?, 0.0, OP)?;
    let (u0, uk_end) = (at(u, 0, OP)?, at(u, k, OP)?);
    for i in 0..=p {
        put(&mut uk, i, u0, OP)?;
        put(&mut uk, n + i + 1, uk_end, OP)?;
    }

    if p == 1 {
        // Special case out.
        if k == 1 {
            put(&mut uk, 2, 0.5 * (at(u, 0, OP)? + at(u, 1, OP)?), OP)?;
        } else {
            if whr == DerivEnd::Start {
                put(&mut uk, 2, 0.5 * (at(u, 0, OP)? + at(u, 1, OP)?), OP)?;
                for i in 1..k {
                    put(&mut uk, i + 2, at(u, i, OP)?, OP)?;
                }
            }
            if whr == DerivEnd::End {
                put(&mut uk, n, 0.5 * (at(u, k, OP)? + at(u, k - 1, OP)?), OP)?;
                for i in 1..k {
                    put(&mut uk, i + 1, at(u, i, OP)?, OP)?;
                }
            }
        }
    }

    if p > 1 && p < n {
        if whr == DerivEnd::Start {
            for i in 1..=k - p {
                let mut sum = 0.0;
                for j in i..i + p {
                    sum += at(u, j, OP)?;
                }
                put(&mut uk, i + p + 1, sum / p as f64, OP)?;
            }
            let mut sum = 0.0;
            for j in 0..p {
                sum += at(u, j, OP)?;
            }
            put(&mut uk, p + 1, sum / p as f64, OP)?;
        }
        if whr == DerivEnd::End {
            for i in 1..=k - p {
                let mut sum = 0.0;
                for j in i..i + p {
                    sum += at(u, j, OP)?;
                }
                put(&mut uk, i + p, sum / p as f64, OP)?;
            }
            let mut sum = 0.0;
            for j in k - p + 1..=k {
                sum += at(u, j, OP)?;
            }
            put(&mut uk, n, sum / p as f64, OP)?;
        }
    }
    Ok(uk)
}

/// Knot vector for global least-squares approximation of samples at parameters
/// `u[0..=m]` (ascending) by a curve with control points `0..=n` and degree `p`
/// (upstream `N_FitCalcKnotVectorCrvApprox`): the samples are split into `n + 1`
/// clusters, and each interior knot is the average of `p` consecutive cluster means
/// (`n + p + 2` knots).
///
/// # Errors
///
/// [`NlibCode::Input`] when `n > m`; `InvalidInput` for empty `u`.
pub fn knot_vector_approx(u: &[f64], n: usize, p: usize) -> Result<Vec<f64>> {
    const OP: &str = "nurbs::crv_fit::p1::knot_vector_approx";
    let m = hi(u, OP, "no parameters")?;
    let n = sg(n, OP)?;
    let p = sg(p, OP)?;

    // Check state: as many or more sample points than control points.
    if n > m {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    // Set first and last p+1 knots to the first and last sample parameters.
    let mut uk = alloc(add(add(n, p, OP)?, 1, OP)?, 0.0, OP)?;
    let (u0, um) = (at(u, 0, OP)?, at(u, m, OP)?);
    for i in 0..=p {
        put(&mut uk, i, u0, OP)?;
        put(&mut uk, n + i + 1, um, OP)?;
    }

    // Representatives of the clusters of parameters, one per control point.
    let mut ukk = alloc(n, 0.0, OP)?;
    // Number of samples per control point (at least 1).
    let d = (m as f64 + 1.0) / (n as f64 + 1.0);
    // First index of the current cluster.
    let mut il: i64 = 0;
    let mut l = -1.0;
    for i in 0..=n {
        // Last sample of the cluster: upstream ROUND(x) = (NL_INDEX)(x + 0.5).
        l += d;
        let ih = (l + 0.5) as i64;
        if il == ih {
            put(&mut ukk, i, at(u, il, OP)?, OP)?;
        } else {
            let mut sum = 0.0;
            for j in il..=ih {
                sum += at(u, j, OP)?;
            }
            put(&mut ukk, i, sum / (ih as f64 - il as f64 + 1.0), OP)?;
        }
        il = ih + 1;
    }

    // Interior knots: averages of p consecutive cluster representatives.
    for i in 1..=n - p {
        let mut sum = 0.0;
        for j in i..i + p {
            sum += at(&ukk, j, OP)?;
        }
        put(&mut uk, i + p, sum / p as f64, OP)?;
    }
    Ok(uk)
}

// ---- interpolation ----------------------------------------------------------------

/// Curve of degree `p` with knot vector `knt` interpolating the points `a[0..=n]`
/// at the parameters `u` (upstream `N_FitCrvInterpGivenParams`). For `p == 1` the
/// control points are the data points.
///
/// # Errors
///
/// [`NlibCode::Input`] when `knt` does not have `n + p + 2` knots or a basis span
/// falls outside the matrix band; errors of [`KnotVector::basis_eval`] and of the
/// LU decomposition and solve (`NL_NUM_ERR` for a singular system); `InvalidInput`
/// for empty `a` or too few parameters.
pub fn interp_given_params<P: FitPoint>(
    a: &[P],
    u: &[f64],
    knt: &KnotVector<'_>,
    p: usize,
) -> Result<Curve> {
    const OP: &str = "nurbs::crv_fit::p1::interp_given_params";
    let n = hi(a, OP, "no data points")?;
    let p = sg(p, OP)?;

    // Check for curve memory.
    let m = sg(knt.last_index(), OP)?;
    // An overflowing `n + p + 1` cannot equal `m` (upstream's wrapped sum does not).
    let mc = n.checked_add(p).and_then(|x| x.checked_add(1));
    if Some(m) != mc {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let uc = copy_knots(knt, m, OP)?;
    let mut pw = alloc(n, CPoint::default(), OP)?;

    // Control points are the data points in case of linear curves.
    if p == 1 {
        for i in 0..=n {
            put(&mut pw, i, at(a, i, OP)?.to_cpoint(), OP)?;
        }
        return curve(pw, p, uc, OP);
    }

    // Compute coefficient matrix.
    let (bw, sbw) = if 2 * p <= n {
        (2 * p + 1, p)
    } else {
        (2 * p - 1, p - 1)
    };
    // This must be non-positive.
    let sp = sbw - p;
    let mut cm = banded(n, bw, OP)?;
    mset(&mut cm, 0, sbw, 1.0, OP)?;
    mset(&mut cm, n, sbw, 1.0, OP)?;
    for i in 1..n {
        let (j, nb) = basis(knt, p, at(u, i, OP)?, OP)?;
        let l = sp - i + j;
        if l < 0 || l + p >= bw {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        for k in 0..=p {
            mset(&mut cm, i, l + k, at(&nb, k, OP)?, OP)?;
        }
    }

    // LU decompose matrix and solve the system of linear equations.
    cm.lu_decompose()?;
    let sol = P::for_back(&cm, a)?;
    for i in 0..=n {
        put(&mut pw, i, at(&sol, i, OP)?, OP)?;
    }
    curve(pw, p, uc, OP)
}

/// Curve of degree `p >= 2` with knot vector `knt` interpolating the points
/// `a[0..=kk]` at the parameters `u` and the start and end derivatives `ds`, `de`
/// (upstream `N_FitCrvKnotsAndDerivs`); it has `kk + 3` control points.
///
/// # Errors
///
/// [`NlibCode::Input`] when `p < 2`, `p > kk + 2`, `knt` does not have `kk + p + 4`
/// knots or a basis span falls outside the matrix band; errors of
/// [`KnotVector::basis_eval`] and of the LU decomposition and solve; `InvalidInput`
/// for empty `a` or too few parameters.
pub fn interp_knots_and_derivs<P: FitPoint>(
    a: &[P],
    u: &[f64],
    knt: &KnotVector<'_>,
    p: usize,
    ds: P,
    de: P,
) -> Result<Curve> {
    const OP: &str = "nurbs::crv_fit::p1::interp_knots_and_derivs";
    let kk = hi(a, OP, "no data points")?;
    let p = sg(p, OP)?;

    // Check for errors.
    let m = sg(knt.last_index(), OP)?;
    let uk = knt.knots();
    let n = kk + 2;
    if p < 2 || p > kk + 2 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let mc = n + p + 1;
    if m != mc {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let uc = copy_knots(knt, m, OP)?;

    // Compute coefficient matrix.
    let (bw, sbw) = if 2 * p <= n {
        (2 * p + 1, p)
    } else {
        (2 * p - 1, p - 1)
    };
    // This must be non-positive.
    let sp = sbw - p;
    let mut cm = banded(n, bw, OP)?;
    mset(&mut cm, 0, sbw, 1.0, OP)?;
    mset(&mut cm, 1, sbw - 1, -1.0, OP)?;
    mset(&mut cm, 1, sbw, 1.0, OP)?;
    mset(&mut cm, n - 1, sbw, -1.0, OP)?;
    mset(&mut cm, n - 1, sbw + 1, 1.0, OP)?;
    mset(&mut cm, n, sbw, 1.0, OP)?;
    for i in 2..=kk {
        let (j, nb) = basis(knt, p, at(u, i - 1, OP)?, OP)?;
        let l = sp - i + j;
        if l < 0 || l + p >= bw {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        for k in 0..=p {
            mset(&mut cm, i, l + k, at(&nb, k, OP)?, OP)?;
        }
    }

    // LU decompose matrix.
    cm.lu_decompose()?;

    // Solve system of linear equations.
    let mut q = alloc(n, P::ZERO, OP)?;
    let fact = (at(uk, add(p, 1, OP)?, OP)? - at(uk, 1, OP)?) / p as f64;
    put(&mut q, 0, at(a, 0, OP)?, OP)?;
    put(&mut q, 1, ds.scaled(fact), OP)?;
    for i in 2..=kk {
        put(&mut q, i, at(a, i - 1, OP)?, OP)?;
    }
    let fact = (at(uk, add(n, p, OP)?, OP)? - at(uk, n, OP)?) / p as f64;
    put(&mut q, n - 1, de.scaled(fact), OP)?;
    put(&mut q, n, at(a, kk, OP)?, OP)?;
    let pw = P::for_back(&cm, &q)?;
    curve(pw, p, uc, OP)
}

/// Curve of degree `p >= 2` with knot vector `knt` interpolating the points
/// `a[0..=kk]` at the parameters `u` and the derivative `t` at the start or end
/// (`whr`) (upstream `N_FitCrvKnotsAndDeriv`); it has `kk + 2` control points.
///
/// # Errors
///
/// [`NlibCode::Input`] when `p < 2`, `p > kk + 1`, `knt` does not have `kk + p + 3`
/// knots or a basis span falls outside the matrix band; errors of
/// [`KnotVector::basis_eval`] and of the LU decomposition and solve; `InvalidInput`
/// for empty `a` or too few parameters.
pub fn interp_knots_and_deriv<P: FitPoint>(
    a: &[P],
    u: &[f64],
    knt: &KnotVector<'_>,
    p: usize,
    t: P,
    whr: DerivEnd,
) -> Result<Curve> {
    const OP: &str = "nurbs::crv_fit::p1::interp_knots_and_deriv";
    let kk = hi(a, OP, "no data points")?;
    let p = sg(p, OP)?;

    // Check for errors.
    let m = sg(knt.last_index(), OP)?;
    let uk = knt.knots();
    let n = kk + 1;
    if p < 2 || p > n {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let mc = n + p + 1;
    if m != mc {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let uc = copy_knots(knt, m, OP)?;

    // Compute coefficient matrix.
    let bw = 2 * p - 1;
    let sbw = p - 1;
    let mut cm = banded(n, bw, OP)?;
    let band_row = |cm: &mut RealMatrix, i: i64, ui: f64| -> Result<()> {
        let (j, nb) = basis(knt, p, ui, OP)?;
        let l = if p == 2 { 0 } else { j - i - 1 };
        if l < 0 || l + p >= bw {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        for k in 0..=p {
            mset(cm, i, l + k, at(&nb, k, OP)?, OP)?;
        }
        Ok(())
    };
    match whr {
        DerivEnd::Start => {
            mset(&mut cm, 0, sbw, 1.0, OP)?;
            mset(&mut cm, 1, sbw - 1, -1.0, OP)?;
            mset(&mut cm, 1, sbw, 1.0, OP)?;
            mset(&mut cm, n, sbw, 1.0, OP)?;
            for i in 2..=kk {
                band_row(&mut cm, i, at(u, i - 1, OP)?)?;
            }
        }
        DerivEnd::End => {
            mset(&mut cm, 0, sbw, 1.0, OP)?;
            mset(&mut cm, n - 1, sbw, -1.0, OP)?;
            mset(&mut cm, n - 1, sbw + 1, 1.0, OP)?;
            mset(&mut cm, n, sbw, 1.0, OP)?;
            for i in 1..kk {
                band_row(&mut cm, i, at(u, i, OP)?)?;
            }
        }
    }

    // LU decompose matrix.
    cm.lu_decompose()?;

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
            put(&mut q, n, at(a, kk, OP)?, OP)?;
            for i in 0..=n - 2 {
                put(&mut q, i, at(a, i, OP)?, OP)?;
            }
        }
    }
    let pw = P::for_back(&cm, &q)?;
    curve(pw, p, uc, OP)
}

/// Curve of degree 2 or 3 with knot vector `knt` interpolating the points
/// `a[0..=kk]` and the first derivatives `d[0..=kk]` at the parameters `u`
/// (upstream `N_FitCrvFirstDerivAndKnots`); it has `2*kk + 2` control points.
///
/// # Errors
///
/// [`NlibCode::Degree`] when `p` is not 2 or 3; [`NlibCode::Input`] when `knt` does
/// not have `2*kk + p + 3` knots or a basis span falls outside the matrix band;
/// errors of [`basis_derivs`] and of the LU decomposition and solve; `InvalidInput`
/// for empty `a`, too few derivatives or parameters.
pub fn interp_first_derivs_and_knots<P: FitPoint>(
    a: &[P],
    d: &[P],
    u: &[f64],
    knt: &KnotVector<'_>,
    p: usize,
) -> Result<Curve> {
    const OP: &str = "nurbs::crv_fit::p1::interp_first_derivs_and_knots";
    let kk = hi(a, OP, "no data points")?;
    let p = sg(p, OP)?;

    // Check error and curve memory.
    let m = sg(knt.last_index(), OP)?;
    let uk = knt.knots();
    let n = 2 * kk + 1;
    if !(2..=3).contains(&p) {
        return Err(SolidError::nlib(OP, NlibCode::Degree));
    }
    let mc = n + p + 1;
    if m != mc {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let uc = copy_knots(knt, m, OP)?;

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
        let nd = basis_derivs(knt, idx(p, OP)?, at(u, i, OP)?, Side::Left, 1)?;
        let j = sg(nd.span, OP)?;
        let nd0 = nd.derivs.first().ok_or_else(|| out_of_range(OP))?;
        let nd1 = nd.derivs.get(1).ok_or_else(|| out_of_range(OP))?;
        let ii = 2 * i;
        let l = j - ii - 1;
        if l < 0 || l + hp >= bw {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        for k in 0..=hp {
            mset(&mut cm, ii, l + k, at(nd0, k, OP)?, OP)?;
            // For l + k == 0 this is M[ii+1][-1], the last entry of row ii.
            mset(&mut cm, ii + 1, l + k - 1, at(nd1, k, OP)?, OP)?;
        }
    }

    // LU decompose matrix.
    cm.lu_decompose()?;

    // Solve system of linear equations.
    let mut q = alloc(n, P::ZERO, OP)?;
    let fact = (at(uk, add(p, 1, OP)?, OP)? - at(uk, 1, OP)?) / p as f64;
    put(&mut q, 0, at(a, 0, OP)?, OP)?;
    put(&mut q, 1, at(d, 0, OP)?.scaled(fact), OP)?;
    for i in 1..kk {
        let ii = 2 * i;
        put(&mut q, ii, at(a, i, OP)?, OP)?;
        put(&mut q, ii + 1, at(d, i, OP)?, OP)?;
    }
    let fact = (at(uk, add(n, p, OP)?, OP)? - at(uk, n, OP)?) / p as f64;
    put(&mut q, n - 1, at(d, kk, OP)?.scaled(fact), OP)?;
    put(&mut q, n, at(a, kk, OP)?, OP)?;
    let pw = P::for_back(&cm, &q)?;
    curve(pw, p, uc, OP)
}

// ---- approximation ----------------------------------------------------------------

/// Least-squares approximation of the points `a[0..=m]` at the parameters `u` by a
/// curve with control points `0..=n`, degree `p` and knot vector `knt`, through the
/// first and last point (upstream `N_FitCrvApproxKnots`). For `n < 2` the result is
/// the line between `a[0]` and `a[1]`.
///
/// # Errors
///
/// [`NlibCode::Input`] when `n < p`, `knt` does not have `n + p + 2` knots,
/// `m < n`, or some interior basis function gets no sample; errors of
/// [`KnotVector::basis_eval`], [`KnotVector::basis_i_eval`] and of the LU
/// decomposition and solve; `InvalidInput` for empty `a`, too few knots (upstream
/// copies `n + p + 2` knots first), `n == 0`, or too few parameters.
pub fn approx_knots<P: FitPoint>(
    a: &[P],
    u: &[f64],
    knt: &KnotVector<'_>,
    n: usize,
    p: usize,
) -> Result<Curve> {
    const OP: &str = "nurbs::crv_fit::p1::approx_knots";
    let m = hi(a, OP, "no data points")?;
    let n = sg(n, OP)?;
    let p = sg(p, OP)?;

    if n < p {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let mk = sg(knt.last_index(), OP)?;

    // Check curve memory.
    let mc = add(add(n, p, OP)?, 1, OP)?;
    let uc = copy_knots(knt, mc, OP)?;
    let mut pw = alloc(n, CPoint::default(), OP)?;

    if n < 2 {
        // Just a line between 2 points.
        put(&mut pw, 0, at(a, 0, OP)?.to_cpoint(), OP)?;
        put(&mut pw, 1, at(a, 1, OP)?.to_cpoint(), OP)?;
        return curve(pw, p, uc, OP);
    }

    // Check error.
    if mk != mc {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if n < 2 || p > n || m < n {
        return Err(SolidError::nlib(OP, NlibCode::Input));
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

    // NTN and B[i][p] start zero (zero-filled storage).
    for i in 0..=(p - 1).min(n - 2) {
        put(&mut start, i, 0, OP)?;
    }
    put(&mut end, 0, -2, OP)?;

    for i in 1..m {
        let (j, nb) = basis(knt, p, at(u, i, OP)?, OP)?;
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

    cm.lu_decompose()?;

    // Setup and solve system of equations.
    put(&mut pw, 0, at(a, 0, OP)?.to_cpoint(), OP)?;
    put(&mut pw, n, at(a, m, OP)?.to_cpoint(), OP)?;
    let (q0, qm) = (at(a, 0, OP)?, at(a, m, OP)?);
    let pi = idx(p, OP)?;
    let ni = idx(n, OP)?;
    let mut rk = alloc(m - 2, P::ZERO, OP)?;
    for i in 1..m {
        let ui = at(u, i, OP)?;
        let n0 = knt.basis_i_eval(0, pi, ui, Side::Left)?;
        let np = knt.basis_i_eval(ni, pi, ui, Side::Left)?;
        put(
            &mut rk,
            i - 1,
            P::translate_sum2(at(a, i, OP)?, -n0, q0, -np, qm),
            OP,
        )?;
    }
    let mut r = alloc(n - 2, P::ZERO, OP)?;
    for i in 1..n {
        let ri = at_mut(&mut r, i - 1, OP)?;
        *ri = P::ZERO;
        let lk = at(&start, i - 1, OP)?;
        let hk = at(&end, i - 1, OP)?;
        for k in lk..=hk {
            let ik = at(&index, k, OP)?;
            ri.blend(b.get(k, i - ik - 1, OP)?, at(&rk, k, OP)?);
        }
    }
    let sol = P::for_back(&cm, &r)?;
    for (i, s) in (1..).zip(sol) {
        put(&mut pw, i, s, OP)?;
    }
    curve(pw, p, uc, OP)
}

// ---- solves with a given matrix ---------------------------------------------------

/// Control points of the curve interpolating one column (`DataDir::U`: `a[i][k]` for
/// every row `i`) or one row (`DataDir::V`: all of `a[k]`) of surface data, with the
/// LU-decomposed interpolation matrix `cm` (upstream `N_FitCrvCPtsFromSrfData`).
/// Returns `cm.n() + 1` control points.
///
/// # Errors
///
/// Errors of the solve; `InvalidInput` when the row or column is missing or shorter
/// than `cm.n() + 1` (upstream reads uninitialised or foreign memory).
pub fn cpts_from_srf_data<P: FitPoint>(
    a: &[Vec<P>],
    k: usize,
    dir: DataDir,
    cm: &RealMatrix,
) -> Result<Vec<CPoint>> {
    const OP: &str = "nurbs::crv_fit::p1::cpts_from_srf_data";
    let k = sg(k, OP)?;
    let r: Vec<P> = match dir {
        DataDir::U => a.iter().map(|row| at(row, k, OP)).collect::<Result<_>>()?,
        DataDir::V => a.get(idx(k, OP)?).ok_or_else(|| out_of_range(OP))?.clone(),
    };
    if r.len() <= cm.n() {
        return Err(out_of_range(OP));
    }
    P::for_back(cm, &r)
}

/// Control points of the curve of degree `p` with knot vector `knt` interpolating
/// the points `a[0..=k]` and the start and end derivatives `ds`, `de`, with the
/// LU-decomposed interpolation matrix `cm` (upstream `N_FitCrvDerivsMatrix`).
/// Returns `cm.n() + 1` control points (`k + 3` for the matching matrix).
///
/// # Errors
///
/// Errors of the solve; `InvalidInput` for empty `a`, too few knots, or a matrix
/// larger than `k + 3` rows.
pub fn derivs_matrix<P: FitPoint>(
    a: &[P],
    p: usize,
    knt: &KnotVector<'_>,
    ds: P,
    de: P,
    cm: &RealMatrix,
) -> Result<Vec<CPoint>> {
    const OP: &str = "nurbs::crv_fit::p1::derivs_matrix";
    let k = hi(a, OP, "no data points")?;
    let p = sg(p, OP)?;
    let uk = knt.knots();
    let n = k + 2;

    // Solve system of linear equations.
    let mut q = alloc(n, P::ZERO, OP)?;
    let fact = (at(uk, add(p, 1, OP)?, OP)? - at(uk, 1, OP)?) / p as f64;
    put(&mut q, 0, at(a, 0, OP)?, OP)?;
    put(&mut q, 1, ds.scaled(fact), OP)?;
    for i in 2..=k {
        put(&mut q, i, at(a, i - 1, OP)?, OP)?;
    }
    let fact = (at(uk, add(n, p, OP)?, OP)? - at(uk, n, OP)?) / p as f64;
    put(&mut q, n - 1, de.scaled(fact), OP)?;
    put(&mut q, n, at(a, k, OP)?, OP)?;
    P::for_back(cm, &q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::MatrixType;

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

    #[test]
    fn param_values_methods() {
        let a = pts(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 3.0, 0.0]]);
        let u = param_values(&a, ParamMethod::Uniform).unwrap();
        assert_eq!(u, vec![0.0, 0.5, 1.0]);
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        assert_eq!(u, vec![0.0, 0.25, 1.0]);
        let u = param_values(&a, ParamMethod::Centripetal).unwrap();
        let s = 1.0 + 3.0_f64.sqrt();
        assert_eq!(u, vec![0.0, 1.0 / s, 1.0]);
        // Coincident points: interior parameters stay 0.
        let z = pts(&[[1.0, 1.0, 1.0]; 3]);
        assert_eq!(
            param_values(&z, ParamMethod::ChordLength).unwrap(),
            vec![0.0, 0.0, 1.0]
        );
        assert_eq!(
            param_values(&z[..1], ParamMethod::Uniform).unwrap(),
            vec![1.0]
        );
        assert!(param_values::<Vec3>(&[], ParamMethod::Uniform).is_err());
    }

    #[test]
    fn knot_vectors() {
        let u = [0.0, 0.2, 0.5, 0.7, 1.0];
        let k = interp_knot_vector(&u, 3).unwrap();
        assert_eq!(k.len(), 9);
        assert_eq!(&k[..4], &[0.0; 4]);
        assert_eq!(k[4], (0.2 + 0.5 + 0.7) / 3.0);
        assert_eq!(&k[5..], &[1.0; 4]);

        let k = knot_vector_end_derivs(&u, 3).unwrap();
        assert_eq!(k.len(), 11);
        assert_eq!(k[4], (0.0 + 0.2 + 0.5) / 3.0);
        let k = knot_vector_deriv(&u, 3, DerivEnd::Start).unwrap();
        assert_eq!(k.len(), 10);
        assert!(matches!(
            knot_vector_deriv(&u, 6, DerivEnd::End),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
        let k = knot_vector_approx(&u, 3, 2).unwrap();
        assert_eq!(k.len(), 7);
        assert!(knot_vector_approx(&u, 5, 2).is_err());
    }

    #[test]
    fn interpolation_hits_points() {
        let a = pts(&[
            [0.0, 0.0, 0.0],
            [1.0, 2.0, 0.0],
            [3.0, 3.0, 1.0],
            [4.0, 1.0, 0.0],
            [6.0, 0.0, 2.0],
        ]);
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        let kv = interp_knot_vector(&u, 3).unwrap();
        let knt = KnotVector::from_slice(&kv).unwrap();
        let c = interp_given_params(&a, &u, &knt, 3).unwrap();
        for (q, t) in a.iter().zip(&u) {
            let e = eval(&c, *t);
            assert!((e - *q).length() < 1e-9, "{e:?} vs {q:?}");
        }
        let lin =
            interp_given_params(&a, &u, &KnotVector::from_slice(&[0.0; 7]).unwrap(), 1).unwrap();
        assert_eq!(lin.cpts()[2], CPoint::from_point(a[2]));
    }

    #[test]
    fn interpolation_with_derivs() {
        let a = pts(&[
            [0.0, 0.0, 0.0],
            [1.0, 2.0, 0.0],
            [3.0, 3.0, 1.0],
            [4.0, 1.0, 0.0],
        ]);
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        let kv = knot_vector_end_derivs(&u, 3).unwrap();
        let knt = KnotVector::from_slice(&kv).unwrap();
        let ds = Vec3::new(1.0, 0.0, 0.0);
        let de = Vec3::new(0.0, -1.0, 0.0);
        let c = interp_knots_and_derivs(&a, &u, &knt, 3, ds, de).unwrap();
        assert_eq!(c.n(), 5);
        for (q, t) in a.iter().zip(&u) {
            assert!((eval(&c, *t) - *q).length() < 1e-9);
        }
        let kv = knot_vector_deriv(&u, 3, DerivEnd::End).unwrap();
        let knt = KnotVector::from_slice(&kv).unwrap();
        let c = interp_knots_and_deriv(&a, &u, &knt, 3, de, DerivEnd::End).unwrap();
        assert_eq!(c.n(), 4);
        for (q, t) in a.iter().zip(&u) {
            assert!((eval(&c, *t) - *q).length() < 1e-9);
        }
    }

    #[test]
    fn first_derivs_cubic() {
        let a = pts(&[[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 0.0, 0.0]]);
        let d = pts(&[[1.0, 1.0, 0.0], [1.0, 0.0, 0.0], [1.0, -1.0, 0.0]]);
        let u = [0.0, 0.5, 1.0];
        let kv = [0.0, 0.0, 0.0, 0.0, 0.25, 0.75, 1.0, 1.0, 1.0, 1.0];
        let knt = KnotVector::from_slice(&kv).unwrap();
        let c = interp_first_derivs_and_knots(&a, &d, &u, &knt, 3).unwrap();
        assert_eq!(c.n(), 5);
        assert!((eval(&c, 0.5) - a[1]).length() < 1e-9);
        assert!(matches!(
            interp_first_derivs_and_knots(&a, &d, &u, &knt, 4),
            Err(SolidError::Nlib {
                code: NlibCode::Degree,
                ..
            })
        ));
    }

    #[test]
    fn approximation_reproduces_line_data() {
        let a: Vec<Vec3> = (0..=10)
            .map(|i| Vec3::new(f64::from(i), 2.0, 0.0))
            .collect();
        let u = param_values(&a, ParamMethod::ChordLength).unwrap();
        let kv = knot_vector_approx(&u, 5, 3).unwrap();
        let knt = KnotVector::from_slice(&kv).unwrap();
        let c = approx_knots(&a, &u, &knt, 5, 3).unwrap();
        for (q, t) in a.iter().zip(&u) {
            assert!((eval(&c, *t) - *q).length() < 1e-9);
        }
        let line = approx_knots(&a, &u, &KnotVector::from_slice(&[0.0; 4]).unwrap(), 1, 1).unwrap();
        assert_eq!(line.cpts()[1], CPoint::from_point(a[1]));
    }

    #[test]
    fn matrix_solves() {
        let a = pts(&[[0.0, 0.0, 0.0], [1.0, 2.0, 0.0], [3.0, 3.0, 1.0]]);
        let mut cm = RealMatrix::new(2, 2, MatrixType::Banded, 3).unwrap();
        for i in 0..=2 {
            cm.set(i, 1, 2.0).unwrap();
        }
        cm.set(1, 0, 0.5).unwrap();
        cm.lu_decompose().unwrap();
        let grid = vec![a.clone(), a.clone()];
        let r = cpts_from_srf_data(&grid, 1, DataDir::V, &cm).unwrap();
        assert_eq!(r.len(), 3);
        assert!(cpts_from_srf_data(&grid, 1, DataDir::U, &cm).is_err());
        let kv = [0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0];
        let knt = KnotVector::from_slice(&kv).unwrap();
        let w = derivs_matrix(&a[..1], 3, &knt, a[1], a[2], &cm).unwrap();
        assert_eq!(w.len(), 3);
    }

    /// Fuzz regressions: absurd degrees / counts return errors instead of
    /// overflowing `NL_INDEX` arithmetic or aborting on allocation.
    #[test]
    fn absurd_degree_is_an_error() {
        let u = [0.0, 0.5, 1.0];
        let big = i64::MAX as usize;
        assert!(interp_knot_vector(&u, big).is_err());
        assert!(knot_vector_end_derivs(&u, big).is_err());
        assert!(knot_vector_deriv(&u, big, DerivEnd::Start).is_err());
        assert!(knot_vector_approx(&u, 2, big).is_err());
        assert!(matches!(
            interp_knot_vector(&u, 1 << 61),
            Err(SolidError::Nlib {
                code: NlibCode::Memory,
                ..
            })
        ));
        let a = pts(&[[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 0.0, 0.0]]);
        let kv = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let knt = KnotVector::from_slice(&kv).unwrap();
        // An overflowing n + p + 1 never matches the knot count: NL_INP_ERR.
        assert!(matches!(
            interp_given_params(&a, &u, &knt, big),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
        assert!(matches!(
            interp_knots_and_derivs(&a, &u, &knt, big, a[0], a[1]),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
        assert!(approx_knots(&a, &u, &knt, big, big).is_err());
        let cm = RealMatrix::new(4, 4, MatrixType::Full, 0).unwrap();
        assert!(derivs_matrix(&a, big, &knt, a[0], a[1], &cm).is_err());
    }
}
