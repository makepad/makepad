//! Bounds-checked work arrays and matrix access with upstream's signed `NL_INDEX`
//! arithmetic, shared by the curve and surface fitting ports (`nurbs::crv_fit::p1`,
//! `nurbs::crv_fit::p2`, `nurbs::srf_fit`).
//!
//! Upstream: no counterpart. `NL_CrvFit.cpp` and `NL_SrfFit.cpp` index raw C arrays
//! with signed `NL_INDEX` values, allocate them with `N_AllocReal1dArray(hi)` /
//! `N_AllocReal2dArray(n, m)` (one row-major block behind row pointers), and write
//! `NL_REALMATRIX` storage as `RM[i][j]`, where a column index may leave its row. The
//! ports keep that arithmetic in `i64`; every access goes through these helpers,
//! which return [`SolidError::InvalidInput`] where upstream would read or write
//! outside a whole block.
//!
//! Deviations: `alloc` reserves fallibly and returns [`NlibCode::Memory`] (upstream's
//! `NL_MEM_ERR` for a failed `N_Malloc`) instead of aborting on a size taken from
//! caller input, also above [`crate::limits::MAX_ALLOC_BYTES`] (so a degree of `2^31`
//! fails on every machine); `add` / `mul` return `InvalidInput` where upstream's signed `NL_INDEX`
//! sum or product would overflow (undefined behaviour).
//!
//! | Rust                         | Upstream                                       |
//! |------------------------------|------------------------------------------------|
//! | `at`, `at_mut`, `put`        | `a[i]`, `&a[i]`, `a[i] = v`                    |
//! | `at2`, `put2`                | `a[i][j]`, `a[i][j] = v` of a point grid       |
//! | `alloc`, `alloc2`            | `N_Alloc*1dArray(hi)`, `N_Alloc*2dArray(n, m)` |
//! | `Block`                      | `N_AllocReal2dArray` (one row-major block)     |
//! | `mget`, `mset`               | `RM[i][j]` of an `NL_REALMATRIX`               |
//! | `banded`, `full`             | `N_SetRealMatrix(.., NL_MT_BANDED / NL_MT_FULL, bw)` |
//! | `copy_knots`                 | the knot-by-knot copy into an output curve     |
//! | `basis`                      | `N_BasisEval(.., NL_LEFT, ..)`                 |
//! | `curve`                      | filling an output `NL_CURVE`                   |
//! | `sg`, `idx`, `hi`, `head`    | `NL_INDEX` <-> slice length conversions        |
//! | `add`, `mul`                 | `NL_INDEX` `a + b`, `a * b`                    |

use crate::error::{NlibCode, Result, SolidError};
use crate::limits;
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{CPoint, MatrixType, RealMatrix};

/// [`SolidError::InvalidInput`] with a fixed reason.
pub(crate) const fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// The error of an access outside the arrays upstream would use.
pub(crate) const fn out_of_range(op: &'static str) -> SolidError {
    invalid(op, "index outside the arrays upstream would access")
}

/// `usize` -> signed index (upstream `NL_INDEX` arithmetic).
pub(crate) fn sg(x: usize, op: &'static str) -> Result<i64> {
    i64::try_from(x).map_err(|_| invalid(op, "size overflow"))
}

/// Signed index -> `usize`, rejecting negative values.
pub(crate) fn idx(i: i64, op: &'static str) -> Result<usize> {
    usize::try_from(i).map_err(|_| out_of_range(op))
}

/// Highest index of a slice (`len - 1`), `InvalidInput` (`what`) when empty.
pub(crate) fn hi<T>(a: &[T], op: &'static str, what: &'static str) -> Result<i64> {
    match a.len().checked_sub(1) {
        Some(n) => sg(n, op),
        None => Err(invalid(op, what)),
    }
}

/// The first `n + 1` entries of a slice (upstream passes the slice with highest
/// index `n`).
pub(crate) fn head<'a, T>(a: &'a [T], n: i64, op: &'static str) -> Result<&'a [T]> {
    let len = idx(n, op)?.checked_add(1).ok_or_else(|| out_of_range(op))?;
    a.get(..len).ok_or_else(|| out_of_range(op))
}

/// `a[i]`.
pub(crate) fn at<T: Copy>(a: &[T], i: i64, op: &'static str) -> Result<T> {
    usize::try_from(i)
        .ok()
        .and_then(|i| a.get(i).copied())
        .ok_or_else(|| out_of_range(op))
}

/// `&mut a[i]`.
pub(crate) fn at_mut<'a, T>(a: &'a mut [T], i: i64, op: &'static str) -> Result<&'a mut T> {
    usize::try_from(i)
        .ok()
        .and_then(|i| a.get_mut(i))
        .ok_or_else(|| out_of_range(op))
}

/// `a[i] = v`.
pub(crate) fn put<T>(a: &mut [T], i: i64, v: T, op: &'static str) -> Result<()> {
    *at_mut(a, i, op)? = v;
    Ok(())
}

/// `a[i][j]` of a grid.
pub(crate) fn at2<T: Copy>(a: &[Vec<T>], i: i64, j: i64, op: &'static str) -> Result<T> {
    let row = usize::try_from(i)
        .ok()
        .and_then(|i| a.get(i))
        .ok_or_else(|| out_of_range(op))?;
    at(row, j, op)
}

/// `a[i][j] = v` of a grid.
pub(crate) fn put2<T>(a: &mut [Vec<T>], i: i64, j: i64, v: T, op: &'static str) -> Result<()> {
    let row = usize::try_from(i)
        .ok()
        .and_then(|i| a.get_mut(i))
        .ok_or_else(|| out_of_range(op))?;
    put(row, j, v, op)
}

/// A vector of `n + 1` copies of `v` (`n` an upstream highest index; empty for
/// `n == -1`).
///
/// [`NlibCode::Memory`] (upstream `NL_MEM_ERR` from a failed `N_Malloc`) when the
/// storage cannot be reserved or would take more than [`limits::MAX_ALLOC_BYTES`],
/// instead of an allocation abort.
pub(crate) fn alloc<T: Clone>(n: i64, v: T, op: &'static str) -> Result<Vec<T>> {
    let len = n
        .checked_add(1)
        .and_then(|l| usize::try_from(l).ok())
        .ok_or_else(|| out_of_range(op))?;
    limits::try_filled(len, v).ok_or_else(|| SolidError::nlib(op, NlibCode::Memory))
}

/// `a + b` on upstream `NL_INDEX` values, `InvalidInput` where the sum overflows
/// (signed overflow is undefined behaviour upstream).
pub(crate) fn add(a: i64, b: i64, op: &'static str) -> Result<i64> {
    a.checked_add(b)
        .ok_or_else(|| invalid(op, "index arithmetic overflow"))
}

/// `a * b` on upstream `NL_INDEX` values, `InvalidInput` where the product overflows.
pub(crate) fn mul(a: i64, b: i64, op: &'static str) -> Result<i64> {
    a.checked_mul(b)
        .ok_or_else(|| invalid(op, "index arithmetic overflow"))
}

/// An `(n+1) x (m+1)` grid of `v`; [`NlibCode::Memory`] also when the whole grid
/// would take more than [`limits::MAX_ALLOC_BYTES`] (upstream allocates it as one
/// block).
pub(crate) fn alloc2<T: Clone>(n: i64, m: i64, v: T, op: &'static str) -> Result<Vec<Vec<T>>> {
    let row = alloc(m, v, op)?;
    let rows = n
        .checked_add(1)
        .and_then(|l| usize::try_from(l).ok())
        .ok_or_else(|| out_of_range(op))?;
    if !rows.checked_mul(row.len()).is_some_and(limits::fits::<T>) {
        return Err(SolidError::nlib(op, NlibCode::Memory));
    }
    alloc(n, row, op)
}

/// Upstream's row-major block of reals (`N_AllocReal2dArray` plus its zeroing loops),
/// indexed as `B[i][j]` = element `i*cols + j` of the block.
pub(crate) struct Block {
    cols: i64,
    data: Vec<f64>,
}

impl Block {
    /// Block for highest indexes `n`, `m` (`(n+1) x (m+1)`), zero-filled.
    pub(crate) fn new(n: i64, m: i64, op: &'static str) -> Result<Self> {
        let cols = m.checked_add(1).ok_or_else(|| out_of_range(op))?;
        let len = n
            .checked_add(1)
            .and_then(|r| r.checked_mul(cols))
            .ok_or_else(|| out_of_range(op))?;
        Ok(Self {
            cols,
            data: alloc(len - 1, 0.0, op)?,
        })
    }

    fn flat(&self, i: i64, j: i64, op: &'static str) -> Result<i64> {
        i.checked_mul(self.cols)
            .and_then(|r| r.checked_add(j))
            .ok_or_else(|| out_of_range(op))
    }

    /// `B[i][j]`.
    pub(crate) fn get(&self, i: i64, j: i64, op: &'static str) -> Result<f64> {
        at(&self.data, self.flat(i, j, op)?, op)
    }

    /// `B[i][j] = v`.
    pub(crate) fn set(&mut self, i: i64, j: i64, v: f64, op: &'static str) -> Result<()> {
        let k = self.flat(i, j, op)?;
        put(&mut self.data, k, v, op)
    }
}

/// Element `(i, j)` of a matrix's storage block addressed as upstream's `RM[i][j]`
/// (row pointers into one row-major block): `j` may leave row `i`.
fn mat_slot(cm: &RealMatrix, i: i64, j: i64, op: &'static str) -> Result<(usize, usize)> {
    let (_, cols) = cm.storage_shape().ok_or_else(|| out_of_range(op))?;
    let cols = sg(cols, op)?;
    let k = i
        .checked_mul(cols)
        .and_then(|r| r.checked_add(j))
        .ok_or_else(|| out_of_range(op))?;
    if k < 0 || cols == 0 {
        return Err(out_of_range(op));
    }
    Ok((idx(k / cols, op)?, idx(k % cols, op)?))
}

/// `RM[i][j] = v`.
pub(crate) fn mset(cm: &mut RealMatrix, i: i64, j: i64, v: f64, op: &'static str) -> Result<()> {
    let (r, c) = mat_slot(cm, i, j, op)?;
    cm.set(r, c, v).map_err(|_| out_of_range(op))
}

/// `RM[i][j]`.
pub(crate) fn mget(cm: &RealMatrix, i: i64, j: i64, op: &'static str) -> Result<f64> {
    let (r, c) = mat_slot(cm, i, j, op)?;
    cm.get(r, c).map_err(|_| out_of_range(op))
}

/// Banded `(n+1) x (n+1)` matrix with bandwidth `bw` (`N_SetRealMatrix`); the storage
/// is zero-filled, which is what upstream's explicit zeroing loops produce.
pub(crate) fn banded(n: i64, bw: i64, op: &'static str) -> Result<RealMatrix> {
    RealMatrix::new(idx(n, op)?, idx(n, op)?, MatrixType::Banded, idx(bw, op)?)
}

/// Full `(n+1) x (m+1)` matrix (`N_SetRealMatrix(.., NL_MT_FULL, bw)`), zero-filled.
pub(crate) fn full(n: i64, m: i64, bw: i64, op: &'static str) -> Result<RealMatrix> {
    RealMatrix::new(idx(n, op)?, idx(m, op)?, MatrixType::Full, idx(bw, op)?)
}

/// Copy of `U[0..=last]` of a knot vector (upstream copies knot by knot).
pub(crate) fn copy_knots(knt: &KnotVector<'_>, last: i64, op: &'static str) -> Result<Vec<f64>> {
    let uk = knt.knots();
    let mut uc = alloc(last, 0.0, op)?;
    for i in 0..=last {
        put(&mut uc, i, at(uk, i, op)?, op)?;
    }
    Ok(uc)
}

/// The curve with control points `pw`, degree `p` and knots `uc`.
pub(crate) fn curve(pw: Vec<CPoint>, p: i64, uc: Vec<f64>, op: &'static str) -> Result<Curve> {
    Curve::new(pw, idx(p, op)?, uc)
}

/// Basis functions `N[0..=p]` at `u` and the span index (`N_BasisEval`, `NL_LEFT`).
pub(crate) fn basis(
    knt: &KnotVector<'_>,
    p: i64,
    u: f64,
    op: &'static str,
) -> Result<(i64, Vec<f64>)> {
    let b = knt.basis_eval(idx(p, op)?, u, Side::Left)?;
    Ok((sg(b.span, op)?, b.values))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_access_and_blocks() {
        let mut a = alloc(2, 0.0, "t").unwrap();
        put(&mut a, 2, 4.0, "t").unwrap();
        assert_eq!(at(&a, 2, "t").unwrap(), 4.0);
        assert!(at(&a, -1, "t").is_err());
        assert!(put(&mut a, 3, 0.0, "t").is_err());
        assert!(alloc(-1, 0.0, "t").unwrap().is_empty());
        assert_eq!(hi(&a, "t", "empty").unwrap(), 2);
        assert!(hi::<f64>(&[], "t", "empty").is_err());
        assert_eq!(head(&a, 1, "t").unwrap().len(), 2);
        let mut g = alloc2(1, 2, 0_i64, "t").unwrap();
        put2(&mut g, 1, 2, 9, "t").unwrap();
        assert_eq!(at2(&g, 1, 2, "t").unwrap(), 9);
        assert!(at2(&g, 2, 0, "t").is_err());
        // A block row index may run into the next row, as upstream's B[i][j].
        let mut b = Block::new(1, 1, "t").unwrap();
        b.set(0, 2, 3.0, "t").unwrap();
        assert_eq!(b.get(1, 0, "t").unwrap(), 3.0);
        assert!(b.get(1, 2, "t").is_err());
    }

    #[test]
    fn matrix_slots_wrap_rows() {
        let mut cm = full(1, 1, 1, "t").unwrap();
        mset(&mut cm, 0, 2, 5.0, "t").unwrap();
        assert_eq!(mget(&cm, 1, 0, "t").unwrap(), 5.0);
        assert!(mget(&cm, -1, 0, "t").is_err());
    }

    #[test]
    fn huge_sizes_fail_softly() {
        // Upstream's failed N_Malloc is NL_MEM_ERR; the port must not abort.
        assert!(matches!(
            alloc(1_i64 << 61, 0.0, "t"),
            Err(SolidError::Nlib {
                code: NlibCode::Memory,
                ..
            })
        ));
        assert!(add(i64::MAX, 1, "t").is_err());
        assert!(mul(2, i64::MAX, "t").is_err());
        assert_eq!(add(2, 3, "t").unwrap(), 5);
    }
}
