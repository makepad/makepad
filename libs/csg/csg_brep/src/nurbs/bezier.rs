//! NLib Bezier routines: degree elevation and product matrices, degree elevation of
//! curves and surface rows, products of Bezier functions and curves, single Bernstein
//! polynomials, degree reduction, de Casteljau splitting, arc length by subdivision
//! and smooth extension of curves and surface strips.
//!
//! Upstream: `source/SMLib/src/NL_Bezier.cpp` (declarations in
//! `source/SMLib/inc/NL_Bezier.h`; `NL_PascalTri` and `NL_RECURSELIM` in
//! `src/NL_Globals.cpp`). Only the routines owned by unit `nlib.bezier` are here; the
//! rest of the file belongs to other units.
//!
//! | Rust                                   | Upstream                              |
//! |----------------------------------------|---------------------------------------|
//! | [`elevation_matrix_into`], [`elevation_matrix`] | `N_BezGetDegreeElevationMatrix` |
//! | [`product_matrix_into`], [`product_matrix`] | `N_BezFuncMultiplyBezCrvMatrix`  |
//! | [`elevate_degree`]                     | `N_BezElevateDegree`                  |
//! | [`func_multiply_func`]                 | `N_BezFuncMultiplyBezFunc`            |
//! | [`func_multiply_crv`]                  | `N_BezFuncMultiplyBezCrv`             |
//! | [`func_multiply_crv_4d`]               | `N_BezFuncMultiplyBezCrv4D`           |
//! | [`srf_elevate_degree`]                 | `N_BezSrfElevateDegree`               |
//! | [`eval_one_basis`]                     | `N_BezEvalOneBasis`                   |
//! | [`degree_reduce_coefs`], [`ReduceCoefs`] | `N_BezDegreeReduceCoefs`            |
//! | [`reduce_degree`], [`Reduced`]         | `N_BezReduceDegree`                   |
//! | [`split`]                              | `N_BezSplit`                          |
//! | [`calc_arc_length`], [`ArcLength`]     | `N_BezCalcArcLength`                  |
//! | `arc_length_rec` (private)             | `ST_BezCalcArcLength`                 |
//! | `band_matrix_into` (private)           | the shared loops of `N_BezGetDegreeElevationMatrix` / `N_BezFuncMultiplyBezCrvMatrix` |
//! | [`calc_length`]                        | `N_BezCalcLength`                     |
//! | [`crv_extend`], [`ExtendAt`]           | `N_BezCrvExtend` (`NL_START` / `NL_END`) |
//! | [`srf_extend_4d`]                      | `N_BezSrfExtend4D`                    |
//! | [`PASCAL_TRI`], [`binomial`]           | `NL_PascalTri`                        |
//! | [`RECURSE_LIMIT`]                      | `NL_RECURSELIM`                       |
//!
//! Deviations:
//! - Out-params become return values where upstream fills a whole array
//!   ([`split`], [`reduce_degree`], [`crv_extend`], [`srf_extend_4d`],
//!   [`eval_one_basis`], [`calc_length`]). The routines that compute only an index range
//!   `f..=l` / `s..=e` of a caller array keep writing into a `&mut` slice.
//! - Upstream computes a missing (NULL) elevation / product matrix into the caller's
//!   matrix on a local memory stack that it frees before returning, leaving the caller
//!   with dangling storage. The Rust routines take the matrix by reference and build a
//!   local one when it [`is_null`](RealMatrix::is_null); the caller's matrix is never
//!   changed.
//! - `NL_PascalTri` is an `18 x 18` table; upstream reads past it (undefined
//!   behaviour) for degrees above 17. Those inputs, and every other index outside the
//!   arrays upstream is given (short control point arrays, `f..=l` past the output,
//!   coefficient arrays computed for a lower degree, [`reduce_degree`] with `p < 2`,
//!   which reads the unset `alf[0]` and `Qw[-1]`), return [`SolidError::InvalidInput`].
//! - Upstream's signed `NL_INDEX` arguments are `usize`, so the `s < 0` / `i < 0`
//!   checks cannot fail; the remaining checks keep their `NL_*_ERR` numbers.
//! - The `NL_START` / `NL_END` and `NL_YES` / `NL_NO` flags of `N_BezCrvExtend` are
//!   [`ExtendAt`] and `bool`, so its `NL_CAL_ERR` path for other values cannot occur.
//!   `N_BezSrfExtend4D` with a direction other than `NL_UDIR` / `NL_VDIR` returns a
//!   surface whose control points upstream never writes; [`srf_extend_4d`] returns
//!   `InvalidInput` for [`Direction::UV`] instead. [`srf_extend_4d`] always builds a
//!   new surface (upstream may reuse `surQ`'s storage or extend in place).
//! - `ST_BezCalcArcLength` gives up beyond [`RECURSE_LIMIT`] levels by returning its
//!   error flag without an `NL_ERROR` number; the Rust error is
//!   [`SolidError::NotConverged`] with `iterations = RECURSE_LIMIT + 1`. On any error
//!   the partly accumulated length is not returned (upstream leaves it in `*len`).
//! - The `NL_STACKS` arguments have no counterpart (Rust owns memory).
//! - Upstream's `NL_DEGREE` is a C `short`. [`eval_one_basis`] and
//!   [`degree_reduce_coefs`], which allocate (and for [`eval_one_basis`] loop
//!   `O(p^2)`) from the degree alone, return [`SolidError::InvalidInput`] for a degree
//!   above [`MAX_SHORT_DEGREE`] instead of attempting a huge allocation;
//!   [`degree_reduce_coefs`] therefore returns a `Result`. [`reduce_degree`] checks
//!   that `Pw[0]` and `Pw[p]` exist before allocating its `p` output points.

use crate::error::{NlibCode, Result, SolidError};
use crate::nurbs::geom::{dist_cpolygon, dist_cpt_cpt, dist_cpt_cpt_homo};
use crate::nurbs::math::{CPoint, MatrixType, RealMatrix};
use crate::nurbs::srf_basic::{Direction, Surface, SurfaceIndices};

/// `NL_PascalTri`: binomial coefficients `C(i, j)` for `i <= 17` (zero for `j > i`).
#[rustfmt::skip]
pub const PASCAL_TRI: [[i64; 18]; 18] = [
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 3, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 4, 6, 4, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 5, 10, 10, 5, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 6, 15, 20, 15, 6, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 7, 21, 35, 35, 21, 7, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 8, 28, 56, 70, 56, 28, 8, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 9, 36, 84, 126, 126, 84, 36, 9, 1, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 10, 45, 120, 210, 252, 210, 120, 45, 10, 1, 0, 0, 0, 0, 0, 0, 0],
    [1, 11, 55, 165, 330, 462, 462, 330, 165, 55, 11, 1, 0, 0, 0, 0, 0, 0],
    [1, 12, 66, 220, 495, 792, 924, 792, 495, 220, 66, 12, 1, 0, 0, 0, 0, 0],
    [1, 13, 78, 286, 715, 1287, 1716, 1716, 1287, 715, 286, 78, 13, 1, 0, 0, 0, 0],
    [1, 14, 91, 364, 1001, 2002, 3003, 3432, 3003, 2002, 1001, 364, 91, 14, 1, 0, 0, 0],
    [1, 15, 105, 455, 1365, 3003, 5005, 6435, 6435, 5005, 3003, 1365, 455, 105, 15, 1, 0, 0],
    [1, 16, 120, 560, 1820, 4368, 8008, 11440, 12870, 11440, 8008, 4368, 1820, 560, 120, 16, 1, 0],
    [1, 17, 136, 680, 2380, 6188, 12376, 19448, 24310, 24310, 19448, 12376, 6188, 2380, 680, 136, 17, 1],
];

/// Largest degree upstream's `NL_DEGREE` (a C `short`) can hold.
pub const MAX_SHORT_DEGREE: usize = 32_767;

/// `NL_RECURSELIM`: maximum recursion depth of [`calc_arc_length`].
pub const RECURSE_LIMIT: usize = 20;

const OP_PASCAL: &str = "nurbs::bezier::binomial";
const OP_ELEV_MATRIX: &str = "nurbs::bezier::elevation_matrix";
const OP_PROD_MATRIX: &str = "nurbs::bezier::product_matrix";
const OP_ELEVATE: &str = "nurbs::bezier::elevate_degree";
const OP_FUNC_FUNC: &str = "nurbs::bezier::func_multiply_func";
const OP_FUNC_CRV: &str = "nurbs::bezier::func_multiply_crv";
const OP_FUNC_CRV_4D: &str = "nurbs::bezier::func_multiply_crv_4d";
const OP_SRF_ELEVATE: &str = "nurbs::bezier::srf_elevate_degree";
const OP_ONE_BASIS: &str = "nurbs::bezier::eval_one_basis";
const OP_REDUCE: &str = "nurbs::bezier::reduce_degree";
const OP_REDUCE_COEFS: &str = "nurbs::bezier::degree_reduce_coefs";
const OP_SPLIT: &str = "nurbs::bezier::split";
const OP_ARC_LENGTH: &str = "nurbs::bezier::calc_arc_length";
const OP_CRV_EXTEND: &str = "nurbs::bezier::crv_extend";
const OP_SRF_EXTEND: &str = "nurbs::bezier::srf_extend_4d";

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

/// `InvalidInput` for a degree upstream's `short` `NL_DEGREE` cannot hold
/// (above [`MAX_SHORT_DEGREE`]). Routines that do index arithmetic with a caller's
/// degree call this first, so that arithmetic cannot overflow.
pub(crate) fn check_short_degree(p: usize, operation: &'static str) -> Result<()> {
    if p > MAX_SHORT_DEGREE {
        return Err(SolidError::InvalidInput {
            operation,
            reason: "degree exceeds upstream's NL_DEGREE (short) range",
        });
    }
    Ok(())
}

fn out_of_range(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "index out of range (upstream would access outside its arrays)",
    }
}

/// `a[i]` (copied), or `InvalidInput`.
#[inline]
fn at<T: Copy>(a: &[T], i: usize, op: &'static str) -> Result<T> {
    a.get(i).copied().ok_or_else(|| out_of_range(op))
}

/// `&mut a[i]`, or `InvalidInput`.
#[inline]
fn at_mut<'a, T>(a: &'a mut [T], i: usize, op: &'static str) -> Result<&'a mut T> {
    a.get_mut(i).ok_or_else(|| out_of_range(op))
}

/// `a[i][j]` (copied), or `InvalidInput`.
#[inline]
fn at2<T: Copy>(a: &[Vec<T>], i: usize, j: usize, op: &'static str) -> Result<T> {
    a.get(i)
        .and_then(|row| row.get(j))
        .copied()
        .ok_or_else(|| out_of_range(op))
}

/// `&mut a[i][j]`, or `InvalidInput`.
#[inline]
fn at2_mut<'a, T>(a: &'a mut [Vec<T>], i: usize, j: usize, op: &'static str) -> Result<&'a mut T> {
    a.get_mut(i)
        .and_then(|row| row.get_mut(j))
        .ok_or_else(|| out_of_range(op))
}

/// Matrix element `[i][j]` with the error naming `op`.
#[inline]
fn mat(a: &RealMatrix, i: usize, j: usize, op: &'static str) -> Result<f64> {
    a.get(i, j).map_err(|_| out_of_range(op))
}

/// `NL_PascalTri[i][j]` as a double, or `InvalidInput` outside the `18 x 18` table.
pub fn binomial(i: usize, j: usize) -> Result<f64> {
    PASCAL_TRI
        .get(i)
        .and_then(|row| row.get(j))
        .map(|&c| c as f64)
        .ok_or_else(|| out_of_range(OP_PASCAL))
}

/// Shared body of [`elevation_matrix_into`] and [`product_matrix_into`] (upstream
/// spells the same loops twice): the `(p+q+1) x (p+1)` matrix with
/// `[i][j] = C(p, j) C(q, i-j) / C(p+q, i)` on the band
/// `max(0, i-q) <= j <= min(i, p)`, the lower half mirrored from the upper.
fn band_matrix_into(p: usize, q: usize, a: &mut RealMatrix, op: &'static str) -> Result<()> {
    let n = p.checked_add(q).ok_or_else(|| out_of_range(op))?;
    a.ensure(n, p, MatrixType::Full, p).map_err(|e| match e {
        SolidError::Nlib { code, .. } => nlib(op, code),
        other => other,
    })?;

    let r = n / 2;
    a.set(0, 0, 1.0).map_err(|_| out_of_range(op))?;
    a.set(n, p, 1.0).map_err(|_| out_of_range(op))?;

    for i in 1..=r {
        let inv = 1.0 / binomial(n, i)?;
        let jl = i.saturating_sub(q);
        let jh = i.min(p);
        for j in jl..=jh {
            let v = inv * binomial(p, j)? * binomial(q, i - j)?;
            a.set(i, j, v).map_err(|_| out_of_range(op))?;
        }
    }

    for i in r + 1..n {
        let jl = i.saturating_sub(q);
        let jh = i.min(p);
        for j in jl..=jh {
            let v = mat(a, n - i, p - j, op)?;
            a.set(i, j, v).map_err(|_| out_of_range(op))?;
        }
    }
    Ok(())
}

/// Writes the matrix that elevates a degree-`p` Bezier entity by `t` into `dm`
/// (`N_BezGetDegreeElevationMatrix`): `dm` is allocated `(p+t+1) x (p+1)` when null,
/// else must be at least that large (`NL_STO_ERR`). Only the band
/// `max(0, i-t) <= j <= min(i, p)` of each row `i` is written.
pub fn elevation_matrix_into(p: usize, t: usize, dm: &mut RealMatrix) -> Result<()> {
    band_matrix_into(p, t, dm, OP_ELEV_MATRIX)
}

/// The degree elevation matrix of [`elevation_matrix_into`] in a new matrix.
pub fn elevation_matrix(p: usize, t: usize) -> Result<RealMatrix> {
    let mut dm = RealMatrix::null();
    elevation_matrix_into(p, t, &mut dm)?;
    Ok(dm)
}

/// Writes the matrix for the product of a degree-`p` and a degree-`q` Bezier entity
/// into `pm` (`N_BezFuncMultiplyBezCrvMatrix`): `(p+q+1) x (p+1)`, allocated when
/// null, else checked (`NL_STO_ERR`). Only the band `max(0, i-q) <= j <= min(p, i)`
/// of each row `i` is written.
pub fn product_matrix_into(p: usize, q: usize, pm: &mut RealMatrix) -> Result<()> {
    band_matrix_into(p, q, pm, OP_PROD_MATRIX)
}

/// The product matrix of [`product_matrix_into`] in a new matrix.
pub fn product_matrix(p: usize, q: usize) -> Result<RealMatrix> {
    let mut pm = RealMatrix::null();
    product_matrix_into(p, q, &mut pm)?;
    Ok(pm)
}

/// `given`, or when it is null a matrix from `build` stored in `local`.
fn given_or_local<'a>(
    given: &'a RealMatrix,
    local: &'a mut Option<RealMatrix>,
    build: impl FnOnce() -> Result<RealMatrix>,
) -> Result<&'a RealMatrix> {
    if given.is_null() {
        Ok(local.insert(build()?))
    } else {
        Ok(given)
    }
}

/// Control points `Qw[f..=l]` of the degree-`p` Bezier curve `Pw[0..=p]` elevated by
/// `t` (`N_BezElevateDegree`). `dm` is a precomputed [`elevation_matrix`] or null
/// (then computed here).
pub fn elevate_degree(
    pw: &[CPoint],
    p: usize,
    t: usize,
    dm: &RealMatrix,
    f: usize,
    l: usize,
    qw: &mut [CPoint],
) -> Result<()> {
    let op = OP_ELEVATE;
    let mut local = None;
    let rm = given_or_local(dm, &mut local, || elevation_matrix(p, t))?;

    for i in f..=l {
        let a = i.saturating_sub(t);
        let b = i.min(p);
        let mut c = CPoint::ZERO;
        for j in a..=b {
            c.blend(mat(rm, i, j, op)?, at(pw, j, op)?);
        }
        *at_mut(qw, i, op)? = c;
    }
    Ok(())
}

/// `true` when `s..=e` is a valid product index range (`0 <= s <= e <= p+q`).
fn product_range_ok(p: usize, q: usize, s: usize, e: usize) -> bool {
    p.checked_add(q).is_some_and(|n| e <= n) && s <= e
}

/// Control values `fg[s..=e]` of the product of the Bezier functions `f` (degree
/// `p`) and `g` (degree `q`) (`N_BezFuncMultiplyBezFunc`). `pm` is a precomputed
/// [`product_matrix`] or null. `NL_INP_ERR` unless `s <= e <= p+q`.
pub fn func_multiply_func(
    f: &[f64],
    p: usize,
    g: &[f64],
    q: usize,
    pm: &RealMatrix,
    s: usize,
    e: usize,
    fg: &mut [f64],
) -> Result<()> {
    let op = OP_FUNC_FUNC;
    if !product_range_ok(p, q, s, e) {
        return Err(nlib(op, NlibCode::Input));
    }
    let mut local = None;
    let a = given_or_local(pm, &mut local, || product_matrix(p, q))?;

    for i in s..=e {
        let mut v = 0.0;
        let jl = i.saturating_sub(q);
        let jh = p.min(i);
        for j in jl..=jh {
            v += mat(a, i, j, op)? * at(f, j, op)? * at(g, i - j, op)?;
        }
        *at_mut(fg, i, op)? = v;
    }
    Ok(())
}

/// Shared body of [`func_multiply_crv`] and [`func_multiply_crv_4d`].
fn func_times_crv(
    f: &[f64],
    p: usize,
    pw: &[CPoint],
    q: usize,
    pm: &RealMatrix,
    s: usize,
    e: usize,
    qw: &mut [CPoint],
    scale: fn(CPoint, f64) -> CPoint,
    op: &'static str,
) -> Result<()> {
    if !product_range_ok(p, q, s, e) {
        return Err(nlib(op, NlibCode::Input));
    }
    let mut local = None;
    let a = given_or_local(pm, &mut local, || product_matrix(p, q))?;

    for i in s..=e {
        let mut c = CPoint::ZERO;
        let jl = i.saturating_sub(q);
        let jh = p.min(i);
        for j in jl..=jh {
            let tw = scale(at(pw, i - j, op)?, at(f, j, op)?);
            c.blend(mat(a, i, j, op)?, tw);
        }
        *at_mut(qw, i, op)? = c;
    }
    Ok(())
}

/// Control points `Qw[s..=e]` of the product of the Bezier function `f` (degree `p`)
/// and the curve `Pw` (degree `q`), scaling only `x, y, z` of each control point
/// (`N_BezFuncMultiplyBezCrv`, `N_ScaleCPtXYZ`). `NL_INP_ERR` unless
/// `s <= e <= p+q`.
pub fn func_multiply_crv(
    f: &[f64],
    p: usize,
    pw: &[CPoint],
    q: usize,
    pm: &RealMatrix,
    s: usize,
    e: usize,
    qw: &mut [CPoint],
) -> Result<()> {
    func_times_crv(f, p, pw, q, pm, s, e, qw, CPoint::scale_xyz, OP_FUNC_CRV)
}

/// As [`func_multiply_crv`], scaling all four homogeneous coordinates
/// (`N_BezFuncMultiplyBezCrv4D`, `N_ScaleCPt`).
pub fn func_multiply_crv_4d(
    f: &[f64],
    p: usize,
    pw: &[CPoint],
    q: usize,
    pm: &RealMatrix,
    s: usize,
    e: usize,
    qw: &mut [CPoint],
) -> Result<()> {
    func_times_crv(f, p, pw, q, pm, s, e, qw, CPoint::scale, OP_FUNC_CRV_4D)
}

/// Elevates one row or column of a Bezier surface net by `t` (`N_BezSrfElevateDegree`):
/// for [`Direction::U`], `Qw[f..=l][roc]` from `Pw[0..=r][roc]`; for
/// [`Direction::V`], `Qw[roc][f..=l]` from `Pw[roc][0..=r]`. `dm` is a precomputed
/// [`elevation_matrix`] or null. [`Direction::UV`] fails with `NL_CAL_ERR` (after the
/// matrix is built, as upstream).
pub fn srf_elevate_degree(
    pw: &[Vec<CPoint>],
    r: usize,
    t: usize,
    dm: &RealMatrix,
    dir: Direction,
    f: usize,
    l: usize,
    roc: usize,
    qw: &mut [Vec<CPoint>],
) -> Result<()> {
    let op = OP_SRF_ELEVATE;
    let mut local = None;
    let rm = given_or_local(dm, &mut local, || elevation_matrix(r, t))?;

    match dir {
        Direction::U => {
            for i in f..=l {
                let a = i.saturating_sub(t);
                let b = i.min(r);
                let mut c = CPoint::ZERO;
                for k in a..=b {
                    c.blend(mat(rm, i, k, op)?, at2(pw, k, roc, op)?);
                }
                *at2_mut(qw, i, roc, op)? = c;
            }
        }
        Direction::V => {
            for j in f..=l {
                let a = j.saturating_sub(t);
                let b = j.min(r);
                let mut c = CPoint::ZERO;
                for k in a..=b {
                    c.blend(mat(rm, j, k, op)?, at2(pw, roc, k, op)?);
                }
                *at2_mut(qw, roc, j, op)? = c;
            }
        }
        Direction::UV => return Err(nlib(op, NlibCode::Call)),
    }
    Ok(())
}

/// The Bernstein polynomial `B[i,p](u)` by its recursive definition
/// (`N_BezEvalOneBasis`). `NL_IND_ERR` for `i > p`, `NL_PAR_ERR` for `u` outside
/// `[0, 1]` (a NaN `u` passes both checks, as upstream). `InvalidInput` for
/// `p > MAX_SHORT_DEGREE`.
pub fn eval_one_basis(p: usize, i: usize, u: f64) -> Result<f64> {
    let op = OP_ONE_BASIS;
    check_short_degree(p, op)?;
    if i > p {
        return Err(nlib(op, NlibCode::Index));
    }
    if u < 0.0 || u > 1.0 {
        return Err(nlib(op, NlibCode::Parameter));
    }

    if u == 0.0 {
        return Ok(if i == 0 { 1.0 } else { 0.0 });
    }
    if u == 1.0 {
        return Ok(if i == p { 1.0 } else { 0.0 });
    }

    let mut tmp = vec![0.0; p + 1];
    *at_mut(&mut tmp, p - i, op)? = 1.0;

    let omu = 1.0 - u;
    for k in 1..=p {
        for j in (k..=p).rev() {
            let v = omu * at(&tmp, j, op)? + u * at(&tmp, j - 1, op)?;
            *at_mut(&mut tmp, j, op)? = v;
        }
    }
    at(&tmp, p, op)
}

/// Coefficients for reducing the degree of a Bezier curve by one
/// ([`degree_reduce_coefs`]). Each array has `max(p-1, r+1)` entries,
/// `r = (p-1)/2`; `alf`/`oma` are set at `1..=r`, `bet`/`omb` at `r..=p-2`, the rest
/// are zero (upstream leaves them unset).
#[derive(Debug, Clone, PartialEq)]
pub struct ReduceCoefs {
    /// Left reduction coefficients `p / (p - i)`.
    pub alf: Vec<f64>,
    /// `1 - alf[i]`.
    pub oma: Vec<f64>,
    /// Right reduction coefficients `p / (i + 1)`.
    pub bet: Vec<f64>,
    /// `1 - bet[i]`.
    pub omb: Vec<f64>,
}

/// `r = (p - 1) / 2` with C integer division (`0` for `p = 0`).
fn reduce_mid(p: usize) -> usize {
    p.saturating_sub(1) / 2
}

/// Degree reduction coefficients for degree `p` (`N_BezDegreeReduceCoefs`).
///
/// # Errors
///
/// `InvalidInput` for `p > MAX_SHORT_DEGREE`.
pub fn degree_reduce_coefs(p: usize) -> Result<ReduceCoefs> {
    check_short_degree(p, OP_REDUCE_COEFS)?;
    let r = reduce_mid(p);
    let len = p.saturating_sub(1).max(r + 1);
    let mut c = ReduceCoefs {
        alf: vec![0.0; len],
        oma: vec![0.0; len],
        bet: vec![0.0; len],
        omb: vec![0.0; len],
    };
    let pf = p as f64;
    for i in 1..=r {
        let a = pf / (pf - i as f64);
        if let (Some(alf), Some(oma)) = (c.alf.get_mut(i), c.oma.get_mut(i)) {
            *alf = a;
            *oma = 1.0 - a;
        }
    }
    if p >= 2 {
        for i in (r..=p - 2).rev() {
            let b = pf / (i as f64 + 1.0);
            if let (Some(bet), Some(omb)) = (c.bet.get_mut(i), c.omb.get_mut(i)) {
                *bet = b;
                *omb = 1.0 - b;
            }
        }
    }
    Ok(c)
}

/// Result of [`reduce_degree`].
#[derive(Debug, Clone, PartialEq)]
pub struct Reduced {
    /// Control points `Qw[0..=p-1]` of the degree `p-1` curve.
    pub qw: Vec<CPoint>,
    /// Bound on the reduction error.
    pub err: f64,
}

/// Reduces the degree-`p` Bezier curve `Pw[0..=p]` by one and bounds the error
/// (`N_BezReduceDegree`), with coefficients from [`degree_reduce_coefs`].
/// `InvalidInput` for `p < 2` (upstream reads unset coefficients and `Qw[-1]`).
pub fn reduce_degree(pw: &[CPoint], p: usize, c: &ReduceCoefs) -> Result<Reduced> {
    let op = OP_REDUCE;
    if p < 2 {
        return Err(SolidError::InvalidInput {
            operation: op,
            reason: "degree must be at least 2",
        });
    }
    let (first, last) = (at(pw, 0, op)?, at(pw, p, op)?);
    let mut qw = vec![CPoint::ZERO; p];
    *at_mut(&mut qw, 0, op)? = first;
    *at_mut(&mut qw, p - 1, op)? = last;

    let r = reduce_mid(p);
    let odd = p % 2 == 1;

    // From the left (up to the middle point for odd degrees, which is set below),
    // then from the right.
    let left_end = if odd { r } else { r + 1 };
    for i in 1..left_end {
        let v = CPoint::combine2(
            at(&c.alf, i, op)?,
            at(pw, i, op)?,
            at(&c.oma, i, op)?,
            at(&qw, i - 1, op)?,
        );
        *at_mut(&mut qw, i, op)? = v;
    }
    for i in (r + 1..=p - 2).rev() {
        let v = CPoint::combine2(
            at(&c.bet, i, op)?,
            at(pw, i + 1, op)?,
            at(&c.omb, i, op)?,
            at(&qw, i + 1, op)?,
        );
        *at_mut(&mut qw, i, op)? = v;
    }

    let err = if odd {
        // The middle point averages the left and right formulas.
        let pl = CPoint::combine2(
            at(&c.alf, r, op)?,
            at(pw, r, op)?,
            at(&c.oma, r, op)?,
            at(&qw, r - 1, op)?,
        );
        let pr = CPoint::combine2(
            at(&c.bet, r, op)?,
            at(pw, r + 1, op)?,
            at(&c.omb, r, op)?,
            at(&qw, r + 1, op)?,
        );
        *at_mut(&mut qw, r, op)? = CPoint::combine2(0.5, pl, 0.5, pr);

        let pf = p as f64;
        let u = 0.5 * (1.0 - (1.0 / pf).sqrt());
        let bb = eval_one_basis(p, r, u)?;
        let bb1 = eval_one_basis(p, r + 1, u)?;
        let dw = dist_cpt_cpt_homo(pl, pr);
        let a = 0.5 * (pf - r as f64) / pf;
        let b = (bb - bb1).abs();
        a * b * dw
    } else {
        let u = (r as f64 + 1.0) / p as f64;
        let bb1 = eval_one_basis(p, r + 1, u)?;
        let pl = CPoint::combine2(0.5, at(&qw, r, op)?, 0.5, at(&qw, r + 1, op)?);
        let dw = dist_cpt_cpt_homo(at(pw, r + 1, op)?, pl);
        bb1 * dw
    };
    Ok(Reduced { qw, err })
}

/// Splits the degree-`p` Bezier curve `Pw[0..=p]` at `u` by de Casteljau
/// (`N_BezSplit`): the left and right segments, `p + 1` points each. `NL_PAR_ERR` for
/// `u` outside `[0, 1]`.
pub fn split(pw: &[CPoint], p: usize, u: f64) -> Result<(Vec<CPoint>, Vec<CPoint>)> {
    let op = OP_SPLIT;
    if u < 0.0 || u > 1.0 {
        return Err(nlib(op, NlibCode::Parameter));
    }
    let n = p.checked_add(1).ok_or_else(|| out_of_range(op))?;
    let aw = pw.get(..n).ok_or_else(|| out_of_range(op))?;

    if u == 0.0 {
        let first = at(aw, 0, op)?;
        return Ok((vec![first; n], aw.to_vec()));
    }
    if u == 1.0 {
        let last = at(aw, p, op)?;
        return Ok((aw.to_vec(), vec![last; n]));
    }

    let mut aw = aw.to_vec();
    let mut qw = vec![CPoint::ZERO; n];
    let mut rw = vec![CPoint::ZERO; n];
    *at_mut(&mut qw, 0, op)? = at(&aw, 0, op)?;
    *at_mut(&mut rw, p, op)? = at(&aw, p, op)?;

    for k in 1..=p {
        for i in 0..=p - k {
            let v = CPoint::combine2(1.0 - u, at(&aw, i, op)?, u, at(&aw, i + 1, op)?);
            *at_mut(&mut aw, i, op)? = v;
        }
        *at_mut(&mut qw, k, op)? = at(&aw, 0, op)?;
        *at_mut(&mut rw, p - k, op)? = at(&aw, p - k, op)?;
    }
    Ok((qw, rw))
}

/// Result of [`calc_arc_length`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcLength {
    /// Tolerance actually reached (sum of the leaf bound gaps).
    pub eps: f64,
    /// The input length plus the sum of `upper + lower` bounds of every leaf (twice
    /// the arc length when the input length is zero).
    pub len: f64,
}

/// Recursive body of [`calc_arc_length`] (`ST_BezCalcArcLength`).
fn arc_length_rec(
    pw: &[CPoint],
    p: usize,
    eps: &mut f64,
    len: &mut f64,
    depth: usize,
) -> Result<()> {
    let op = OP_ARC_LENGTH;
    if depth > RECURSE_LIMIT {
        return Err(SolidError::NotConverged {
            operation: op,
            iterations: (RECURSE_LIMIT + 1) as u32,
        });
    }

    let poly = pw.get(..=p).ok_or_else(|| out_of_range(op))?;
    let ub = dist_cpolygon(poly);
    let lb = dist_cpt_cpt(at(pw, 0, op)?, at(pw, p, op)?);

    let del = (ub - lb).abs();

    if del <= *eps {
        *len += ub + lb;
        *eps = del;
    } else {
        let (qw, rw) = split(pw, p, 0.5)?;

        let mut epl = *eps / 2.0;
        arc_length_rec(&qw, p, &mut epl, len, depth + 1)?;

        let mut epr = *eps - epl;
        arc_length_rec(&rw, p, &mut epr, len, depth + 1)?;

        *eps = epl + epr;
    }
    Ok(())
}

/// Accumulates the doubled arc length of the degree-`p` Bezier curve `Pw[0..=p]`
/// onto `len` by subdividing until the control polygon and the chord differ by at
/// most the (halved per level) tolerance `eps` (`N_BezCalcArcLength`).
pub fn calc_arc_length(pw: &[CPoint], p: usize, eps: f64, len: f64) -> Result<ArcLength> {
    let mut eps = eps;
    let mut len = len;
    arc_length_rec(pw, p, &mut eps, &mut len, 0)?;
    Ok(ArcLength { eps, len })
}

/// Arc length of the degree-`p` Bezier curve `Pw[0..=p]` within `tol`
/// (`N_BezCalcLength`).
pub fn calc_length(pw: &[CPoint], p: usize, tol: f64) -> Result<f64> {
    let eps = 2.0 * tol;
    let total = calc_arc_length(pw, p, eps, 0.0)?.len;
    Ok(0.5 * total)
}

/// Which end of a Bezier curve [`crv_extend`] extends (`NL_START = 1`, `NL_END = 2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtendAt {
    /// `NL_START`: before the start (`u < 0`).
    Start,
    /// `NL_END`: past the end (`u > 1`).
    End,
}

impl ExtendAt {
    /// The upstream flag value.
    #[must_use]
    pub fn flag(self) -> i32 {
        match self {
            ExtendAt::Start => 1,
            ExtendAt::End => 2,
        }
    }

    /// The end for an upstream flag value, `None` for any other value.
    #[must_use]
    pub fn from_flag(flag: i32) -> Option<Self> {
        match flag {
            1 => Some(ExtendAt::Start),
            2 => Some(ExtendAt::End),
            _ => None,
        }
    }
}

/// The degree-`p` Bezier curve that continues `Pw[0..=p]` beyond `whr` with the same
/// homogeneous derivatives there (`N_BezCrvExtend`); `rev` reverses the extension's
/// parametrization. Returns `Qw[0..=p]`.
/// Returns `InvalidInput` if `p + 1` overflows or `pw` has fewer than `p + 1`
/// points; additional input points are ignored.
pub fn crv_extend(pw: &[CPoint], p: usize, whr: ExtendAt, rev: bool) -> Result<Vec<CPoint>> {
    let op = OP_CRV_EXTEND;
    let n = p.checked_add(1).ok_or_else(|| out_of_range(op))?;
    let pw = pw.get(..n).ok_or_else(|| out_of_range(op))?;
    let mut qw = vec![CPoint::ZERO; n];

    match whr {
        ExtendAt::Start => {
            let p0 = at(pw, 0, op)?;
            if rev {
                *at_mut(&mut qw, 0, op)? = p0;
            } else {
                *at_mut(&mut qw, p, op)? = p0;
            }

            let mut dgs = -1.0;
            let mut cgs = 1.0;
            let mut sgs = -1.0;

            for i in 1..=p {
                let mut dls = dgs;
                let mut cls = cgs;

                let mut dw = CPoint::ZERO;
                for j in 0..=i {
                    let fac = dls * binomial(i, j)?;
                    dw.blend(fac, at(pw, j, op)?);
                    dls = -dls;
                }

                let mut aw = CPoint::ZERO;
                for j in 0..i {
                    let fac = cls * binomial(i, j)?;
                    let k = if rev { j } else { p - j };
                    aw.blend(fac, at(&qw, k, op)?);
                    cls = -cls;
                }

                if rev {
                    dw = dw.scale(-1.0);
                    *at_mut(&mut qw, i, op)? = CPoint::sum(dw, aw);
                } else {
                    dw = dw.scale(sgs);
                    *at_mut(&mut qw, p - i, op)? = CPoint::sum(dw, aw);
                }

                dgs = -dgs;
                cgs = -cgs;
                sgs = -sgs;
            }
        }
        ExtendAt::End => {
            let pp = at(pw, p, op)?;
            if rev {
                *at_mut(&mut qw, p, op)? = pp;
            } else {
                *at_mut(&mut qw, 0, op)? = pp;
            }

            let mut cgs = 1.0;
            let mut sgs = 1.0;

            for i in 1..=p {
                let mut dls = 1.0;
                let mut cls = cgs;

                let mut dw = CPoint::ZERO;
                for j in 0..=i {
                    let fac = dls * binomial(i, j)?;
                    dw.blend(fac, at(pw, p - j, op)?);
                    dls = -dls;
                }

                let mut aw = CPoint::ZERO;
                for j in 0..i {
                    let fac = cls * binomial(i, j)?;
                    let k = if rev { p - j } else { j };
                    aw.blend(fac, at(&qw, k, op)?);
                    cls = -cls;
                }

                if rev {
                    dw = dw.scale(sgs);
                    *at_mut(&mut qw, p - i, op)? = CPoint::sum(dw, aw);
                } else {
                    *at_mut(&mut qw, i, op)? = CPoint::sum(dw, aw);
                }

                cgs = -cgs;
                sgs = -sgs;
            }
        }
    }
    Ok(qw)
}

/// Extends a surface strip that is Bezier in `dir` beyond `whr` (`N_BezSrfExtend4D`):
/// every row (for [`Direction::U`]) or column ([`Direction::V`]) is extended with
/// [`crv_extend`]; degrees, indices and knots are copied. `NL_INP_ERR` when the strip
/// is not Bezier in `dir` (`p != n` / `q != m`); `InvalidInput` for
/// [`Direction::UV`].
pub fn srf_extend_4d(sur: &Surface, dir: Direction, whr: ExtendAt, rev: bool) -> Result<Surface> {
    let op = OP_SRF_EXTEND;
    let (n, m, r, s) = sur.array_sizes();
    let (p, q) = sur.degrees();

    if dir == Direction::U && p != n {
        return Err(nlib(op, NlibCode::Input));
    }
    if dir == Direction::V && q != m {
        return Err(nlib(op, NlibCode::Input));
    }

    let mut net = vec![vec![CPoint::ZERO; m + 1]; n + 1];

    match dir {
        Direction::U => {
            for j in 0..=m {
                let aw = (0..=p).map(|i| sur.cpt(i, j)).collect::<Result<Vec<_>>>()?;
                let bw = crv_extend(&aw, p, whr, rev)?;
                for (i, b) in bw.into_iter().enumerate() {
                    *at2_mut(&mut net, i, j, op)? = b;
                }
            }
        }
        Direction::V => {
            for i in 0..=n {
                let aw = (0..=q).map(|j| sur.cpt(i, j)).collect::<Result<Vec<_>>>()?;
                let bw = crv_extend(&aw, q, whr, rev)?;
                for (j, b) in bw.into_iter().enumerate() {
                    *at2_mut(&mut net, i, j, op)? = b;
                }
            }
        }
        Direction::UV => {
            return Err(SolidError::InvalidInput {
                operation: op,
                reason: "direction must be U or V (upstream leaves the net unset)",
            });
        }
    }

    Surface::with_indices(
        net,
        sur.u_knots().to_vec(),
        sur.v_knots().to_vec(),
        SurfaceIndices { n, m, p, q, r, s },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;

    fn cp(x: f64, y: f64, z: f64) -> CPoint {
        CPoint::new(x, y, z, NOW)
    }

    #[test]
    fn pascal_table_matches_upstream_rows() {
        assert_eq!(PASCAL_TRI[17][8], 24310);
        assert_eq!(
            PASCAL_TRI[4],
            [1, 4, 6, 4, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert!(binomial(18, 0).is_err());
    }

    #[test]
    fn elevation_matrix_rows_sum_to_one() {
        let dm = elevation_matrix(3, 2).unwrap();
        for i in 0..=5usize {
            let mut s = 0.0;
            for j in i.saturating_sub(2)..=i.min(3) {
                s += dm.get(i, j).unwrap();
            }
            assert!((s - 1.0).abs() < 1e-15, "row {i}: {s}");
        }
        assert!(elevation_matrix(10, 8).is_err());
    }

    #[test]
    fn elevate_keeps_ends_and_shape() {
        let pw = [cp(0.0, 0.0, 0.0), cp(1.0, 2.0, 0.0), cp(3.0, 0.0, 1.0)];
        let mut qw = vec![CPoint::ZERO; 4];
        elevate_degree(&pw, 2, 1, &RealMatrix::null(), 0, 3, &mut qw).unwrap();
        assert_eq!(qw[0], pw[0]);
        assert_eq!(qw[3], pw[2]);
        // Q1 = 1/3 P0 + 2/3 P1
        assert!((qw[1].x - 2.0 / 3.0).abs() < 1e-15);
    }

    #[test]
    fn one_basis_partition_of_unity() {
        let s: f64 = (0..=5).map(|i| eval_one_basis(5, i, 0.3).unwrap()).sum();
        assert!((s - 1.0).abs() < 1e-14);
        assert!(eval_one_basis(3, 4, 0.5).is_err());
        assert!(eval_one_basis(3, 1, 1.5).is_err());
        assert!(eval_one_basis(3, 1, f64::NAN).unwrap().is_nan());
    }

    #[test]
    fn split_and_length_of_line() {
        let pw = [cp(0.0, 0.0, 0.0), cp(1.0, 0.0, 0.0), cp(3.0, 0.0, 0.0)];
        let (l, r) = split(&pw, 2, 0.5).unwrap();
        assert_eq!(l[0], pw[0]);
        assert_eq!(r[2], pw[2]);
        assert_eq!(l[2], r[0]);
        let len = calc_length(&pw, 2, 1e-9).unwrap();
        assert!((len - 3.0).abs() < 1e-12);
    }

    #[test]
    fn arc_length_recursion_limit() {
        let pw = [cp(0.0, 0.0, 0.0), cp(1.0, 1.0, 0.0), cp(2.0, 0.0, 0.0)];
        assert!(matches!(
            calc_length(&pw, 2, 0.0),
            Err(SolidError::NotConverged { .. })
        ));
    }

    #[test]
    fn reduce_elevated_curve_is_exact() {
        let pw = [
            cp(0.0, 0.0, 0.0),
            cp(1.0, 2.0, 0.0),
            cp(3.0, 0.0, 1.0),
            cp(4.0, 1.0, 2.0),
        ];
        for t in [1usize, 2] {
            let mut qw = vec![CPoint::ZERO; 4 + t];
            elevate_degree(&pw, 3, t, &RealMatrix::null(), 0, 3 + t, &mut qw).unwrap();
            let mut cur = qw;
            for p in (4..=3 + t).rev() {
                let red = reduce_degree(&cur, p, &degree_reduce_coefs(p).unwrap()).unwrap();
                assert!(red.err < 1e-12);
                cur = red.qw;
            }
            for (a, b) in cur.iter().zip(&pw) {
                assert!((a.x - b.x).abs() < 1e-12 && (a.y - b.y).abs() < 1e-12);
            }
        }
        assert!(reduce_degree(&pw, 1, &degree_reduce_coefs(1).unwrap()).is_err());
    }

    #[test]
    fn extend_line_continues_it() {
        let pw = [cp(0.0, 0.0, 0.0), cp(1.0, 0.0, 0.0)];
        let q = crv_extend(&pw, 1, ExtendAt::End, false).unwrap();
        assert_eq!(q[0], pw[1]);
        assert_eq!(q[1].x, 2.0);
        let q = crv_extend(&pw, 1, ExtendAt::Start, false).unwrap();
        assert_eq!(q[1], pw[0]);
        assert_eq!(q[0].x, -1.0);
    }

    #[test]
    fn crv_extend_huge_degree_rejects_short_input() {
        let pw = [cp(1.0, 2.0, 3.0)];
        for input in [&pw[..0], &pw[..]] {
            for p in [usize::MAX - 1, usize::MAX] {
                for whr in [ExtendAt::Start, ExtendAt::End] {
                    for rev in [false, true] {
                        assert!(matches!(
                            crv_extend(input, p, whr, rev),
                            Err(SolidError::InvalidInput { .. })
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn crv_extend_requires_complete_prefix() {
        let pw = [cp(0.0, 0.0, 0.0), cp(1.0, 2.0, 3.0)];
        for input in [&pw[..0], &pw[..1], &pw[..]] {
            for whr in [ExtendAt::Start, ExtendAt::End] {
                for rev in [false, true] {
                    assert!(matches!(
                        crv_extend(input, 2, whr, rev),
                        Err(SolidError::InvalidInput { .. })
                    ));
                }
            }
        }
    }

    #[test]
    fn crv_extend_degree_zero_preserves_point() {
        let pw = [cp(1.0, 2.0, 3.0)];
        for whr in [ExtendAt::Start, ExtendAt::End] {
            for rev in [false, true] {
                assert_eq!(crv_extend(&pw, 0, whr, rev).unwrap(), pw);
                assert!(matches!(
                    crv_extend(&[], 0, whr, rev),
                    Err(SolidError::InvalidInput { .. })
                ));
            }
        }
    }

    #[test]
    fn crv_extend_degree_seventeen_reverses_and_ignores_extra_points() {
        let mut pw: Vec<_> = (0..18)
            .map(|i| cp(f64::from(i), f64::from(i % 3), f64::from(i % 5)))
            .collect();
        pw.push(cp(999.0, -999.0, 123.0));
        for whr in [ExtendAt::Start, ExtendAt::End] {
            // NL_Bezier.cpp:5417-5558 seeds the chosen endpoint and builds
            // outward from it. Without rev, the point at distance i from the
            // seed is sum_j C(i,j) * 2^(i-j) * (-1)^j * P[j] (Start), or
            // the same sum using P[17-j] (End). Check the seed, the first
            // point that differs from rev for this fixture, and the far end.
            // These integer-valued sums are exactly representable in f64.
            let forward = crv_extend(&pw[..18], 17, whr, false).unwrap();
            assert_eq!(forward.len(), 18);
            match whr {
                ExtendAt::Start => {
                    assert_eq!(forward[17], pw[0]);
                    assert_eq!(forward[13], cp(-4.0, 17.0, -4.0));
                    assert_eq!(forward[0], cp(-17.0, -17_539_616.0, 49_838_048.0));
                }
                ExtendAt::End => {
                    assert_eq!(forward[0], pw[17]);
                    assert_eq!(forward[4], cp(21.0, -15.0, -29.0));
                    assert_eq!(forward[17], cp(34.0, 17_539_618.0, 22_894_754.0));
                }
            }
            let reversed = crv_extend(&pw[..18], 17, whr, true).unwrap();
            assert_eq!(reversed.len(), 18);
            // The rev branches instead scale Dw by -1 (Start) or by
            // alternating +1/-1 (End). Substitution in the binomial recurrence
            // gives Q[j] = 2 * endpoint - P[j], NOT forward[17-j] or -P[j].
            // Check every coordinate, including the unchanged unit weight.
            for (j, point) in reversed.iter().enumerate() {
                let j = i32::try_from(j).unwrap();
                let (x, y, z) = match whr {
                    ExtendAt::Start => (-j, -(j % 3), -(j % 5)),
                    ExtendAt::End => (34 - j, 4 - j % 3, 4 - j % 5),
                };
                assert_eq!(*point, cp(f64::from(x), f64::from(y), f64::from(z)));
            }
            // Only P[0..=17] participates: P[18] is dropped, not clamped
            // into the endpoint or included in either recurrence.
            for rev in [false, true] {
                assert_eq!(
                    crv_extend(&pw, 17, whr, rev).unwrap(),
                    crv_extend(&pw[..18], 17, whr, rev).unwrap()
                );
            }
        }
    }

    #[test]
    fn product_of_functions() {
        // (1-u+u) * (2(1-u) + 4u) = 2 + 2u -> Bezier deg 2 coefficients [2, 3, 4]
        let mut fg = [0.0; 3];
        func_multiply_func(
            &[1.0, 1.0],
            1,
            &[2.0, 4.0],
            1,
            &RealMatrix::null(),
            0,
            2,
            &mut fg,
        )
        .unwrap();
        assert_eq!(fg, [2.0, 3.0, 4.0]);
        assert!(
            func_multiply_func(&[1.0], 0, &[1.0], 0, &RealMatrix::null(), 1, 0, &mut fg).is_err()
        );
    }

    #[test]
    fn srf_extend_requires_bezier_direction() {
        let net = vec![
            vec![cp(0.0, 0.0, 0.0), cp(0.0, 1.0, 0.0)],
            vec![cp(1.0, 0.0, 0.0), cp(1.0, 1.0, 0.0)],
        ];
        let sur = Surface::new(
            net,
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        let e = srf_extend_4d(&sur, Direction::V, ExtendAt::End, false).unwrap();
        assert_eq!(e.cpt(0, 1).unwrap().y, 2.0);
        assert!(srf_extend_4d(&sur, Direction::UV, ExtendAt::End, false).is_err());
    }

    /// Fuzz regression: degrees beyond upstream's `short` `NL_DEGREE` used to size
    /// allocations (capacity overflow / out-of-memory abort).
    #[test]
    fn huge_degree_is_an_error_not_an_allocation() {
        for p in [
            MAX_SHORT_DEGREE + 1,
            1 << 31,
            1 << 32,
            i64::MAX as usize,
            usize::MAX,
        ] {
            assert!(matches!(
                eval_one_basis(p, 0, 0.5),
                Err(SolidError::InvalidInput { .. })
            ));
            assert!(matches!(
                degree_reduce_coefs(p),
                Err(SolidError::InvalidInput { .. })
            ));
            let pw = [CPoint::ZERO; 3];
            let c = degree_reduce_coefs(2).unwrap();
            assert!(matches!(
                reduce_degree(&pw, p, &c),
                Err(SolidError::InvalidInput { .. })
            ));
        }
        assert!(degree_reduce_coefs(MAX_SHORT_DEGREE).is_ok());
    }
}
