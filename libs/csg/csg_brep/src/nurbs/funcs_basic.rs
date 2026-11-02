//! Scalar B-spline curve and surface functions: structure and accessors, storage
//! sizing and compaction, and point (and curve derivative) evaluation.
//!
//! Upstream: `source/SMLib/src/NL_FuncsBasic.cpp` (declarations in
//! `source/SMLib/inc/NL_FuncsBasic.h`, `NL_CFUN` / `NL_CVALUE` / `NL_SFUN` /
//! `NL_SVALUE` in `source/SMLib/inc/NL_Nurbsdef.h`).
//!
//! | Rust                                              | Upstream                         |
//! |---------------------------------------------------|----------------------------------|
//! | [`CurveFunction`]                                 | `NL_CFUN` (with its `NL_CVALUE` and `NL_KNOTVECTOR`) |
//! | [`CurveFunction::new`], [`CurveFunction::with_indices`] | `N_CFuncFromKnotVector` + `N_CValueFromArray` |
//! | [`CurveFunction::is_sized`]                       | `N_CrvIsFuncSized`               |
//! | `None` / `Option::is_none` on `Option<CurveFunction>` | `N_CFuncInitArrays` / `N_CrvAreFuncArraysNULL` |
//! | [`CurveFunction::set_size_indices`]               | `N_CFuncSetSizeIndices`          |
//! | [`CurveFunction::size_arrays`]                    | `N_CFuncSizeArrays`              |
//! | [`CurveFunction::compact`]                        | `N_CrvFuncCompact`               |
//! | [`CurveFunction::set_arrays`]                     | `N_CrvFuncSetPtrs`               |
//! | [`CurveFunction::array_sizes`], [`CurveFunction::n`], [`CurveFunction::m`] | `N_CFuncGetArraySizes` |
//! | [`CurveFunction::degree`]                         | `N_CFuncGetDegree`               |
//! | [`CurveFunction::knot_vector`]                    | `N_CFuncGetKnotVector`           |
//! | [`CurveFunction::values`], [`CurveFunction::knots`], [`CurveFunction::values_mut`], [`CurveFunction::knots_mut`] | `N_CrvFuncCntrlValKnots` |
//! | the accessors above                               | `N_CFuncGetArrayAndKnotVector`, `N_CrvFuncCntrlValKnotVector`, `N_CFuncGetData` |
//! | [`CurveFunction::eval`]                           | `N_CFuncEval`                    |
//! | [`CurveFunction::derivs`]                         | `N_CFuncDerivs`                  |
//! | [`SurfaceFunction`]                               | `NL_SFUN` (with its `NL_SVALUE` and `NL_KNOTVECTOR`s) |
//! | [`SurfaceFunction::new`], [`SurfaceFunction::with_indices`] | `N_SFuncFromKnotVectors` + `N_SValueFromArray` |
//! | [`SurfaceFunction::is_sized`]                     | `N_SrfIsFuncSized`               |
//! | `None` / `Option::is_none` on `Option<SurfaceFunction>` | `N_SFuncInitArrays` / `N_SrfAreFuncArraysNULL` |
//! | [`SurfaceFunction::set_size_indices`]             | `N_SFuncSetSizeIndices`          |
//! | [`SurfaceFunction::size_arrays`]                  | `N_SFuncSizeArrays`              |
//! | [`SurfaceFunction::array_sizes`]                  | `N_SFuncGetArraySizes`           |
//! | [`SurfaceFunction::degrees`]                      | `N_SFuncGetDegrees`              |
//! | [`SurfaceFunction::knot_vectors`]                 | `N_SFuncGetKnotVectors`          |
//! | [`SurfaceFunction::rows`], [`SurfaceFunction::value`] | `N_SrfFuncCntrlVal`          |
//! | [`SurfaceFunction::u_knots`], [`SurfaceFunction::v_knots`] | `N_SFuncGetKnots`       |
//! | the accessors above                               | `N_SFuncGetArrayAndKnotVectors`  |
//! | [`SurfaceFunction::eval`]                         | `N_SrfFuncEvalPt`                |
//!
//! Deviations:
//! - The function objects own their value and knot arrays (upstream points at
//!   caller- or stack-owned `NL_CVALUE` / `NL_SVALUE` / `NL_KNOTVECTOR` objects). As
//!   upstream, the highest indices in use are kept apart from the storage, which may
//!   be larger; every index is checked against the storage, so the out-of-bounds
//!   accesses upstream allows (`N_CFuncSetSizeIndices` past the arrays, value arrays
//!   too short for the knot vector in the evaluators) are `InvalidInput`.
//! - The NULL function of `N_CFuncInitArrays` / `N_SFuncInitArrays` (null pointers,
//!   degrees `-1`) is `None` in an `Option`; partly NULL functions cannot be
//!   expressed. Storage that upstream allocates uninitialised (`malloc`) is
//!   zero-filled.
//! - The `NL_STACKS` arguments have no counterpart. New storage is zero-filled via
//!   fallible reservations; unrepresentable lengths/grid sizes and failed reservations
//!   return `NL_MEM_ERR` without changing the slot. [`CurveFunction::compact`] has no
//!   memory error path; `N_CrvFuncSetPtrs` takes the new arrays by value and checks
//!   that they hold the indices in use.
//! - Out-params become return values; errors are [`SolidError::Nlib`] values with the
//!   upstream `NL_*_ERR` number instead of a flag plus the global `NL_ERROR` record.
//!   The routine-name argument (`rname`) is dropped.
//! - On error the evaluators return only the error (upstream leaves `f` / `fd` / `F`
//!   unwritten).

use crate::error::{NlibCode, Result, SolidError};
use crate::nurbs::basis_basic::basis_derivs;
use crate::nurbs::knots::{KnotVector, Side};

const OP_CRV_NEW: &str = "nurbs::funcs_basic::CurveFunction::new";
const OP_CRV_WITH: &str = "nurbs::funcs_basic::CurveFunction::with_indices";
const OP_CRV_SIZED: &str = "nurbs::funcs_basic::CurveFunction::is_sized";
const OP_CRV_SIZE_ARRAYS: &str = "nurbs::funcs_basic::CurveFunction::size_arrays";
const OP_CRV_SET_SIZE: &str = "nurbs::funcs_basic::CurveFunction::set_size_indices";
const OP_CRV_SET_ARRAYS: &str = "nurbs::funcs_basic::CurveFunction::set_arrays";
const OP_CRV_EVAL: &str = "nurbs::funcs_basic::CurveFunction::eval";
const OP_CRV_DERIVS: &str = "nurbs::funcs_basic::CurveFunction::derivs";
const OP_SRF_NEW: &str = "nurbs::funcs_basic::SurfaceFunction::new";
const OP_SRF_WITH: &str = "nurbs::funcs_basic::SurfaceFunction::with_indices";
const OP_SRF_SIZED: &str = "nurbs::funcs_basic::SurfaceFunction::is_sized";
const OP_SRF_SIZE_ARRAYS: &str = "nurbs::funcs_basic::SurfaceFunction::size_arrays";
const OP_SRF_SET_SIZE: &str = "nurbs::funcs_basic::SurfaceFunction::set_size_indices";
const OP_SRF_VALUE: &str = "nurbs::funcs_basic::SurfaceFunction::value";
const OP_SRF_EVAL: &str = "nurbs::funcs_basic::SurfaceFunction::eval";

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

fn out_of_range(operation: &'static str) -> SolidError {
    invalid(
        operation,
        "index out of range (value array too short for the knot vector?)",
    )
}

/// `a[i]`, or `InvalidInput` when out of range.
#[inline]
fn rd(a: &[f64], i: usize, op: &'static str) -> Result<f64> {
    a.get(i).copied().ok_or_else(|| out_of_range(op))
}

/// `a - b` on indices, or `InvalidInput` where upstream's signed index would go negative.
#[inline]
fn sub(a: usize, b: usize, op: &'static str) -> Result<usize> {
    a.checked_sub(b).ok_or_else(|| out_of_range(op))
}

/// Highest index of a non-empty array.
fn last_index(a: &[f64], op: &'static str, reason: &'static str) -> Result<usize> {
    a.len().checked_sub(1).ok_or_else(|| invalid(op, reason))
}

/// `true` when `rows` holds rows `0..=n` of at least `m + 1` values.
fn rows_hold(rows: &[Vec<f64>], n: usize, m: usize) -> bool {
    match n.checked_add(1).and_then(|k| rows.get(..k)) {
        Some(rows) => rows.iter().all(|row| row.len() > m),
        None => false,
    }
}

/// Checks the byte layout before any allocation from caller-supplied dimensions.
fn array_layout<T>(count: usize, op: &'static str) -> Result<()> {
    std::alloc::Layout::array::<T>(count)
        .map(|_| ())
        .map_err(|_| nlib(op, NlibCode::Memory))
}

/// Converts a highest index to an array length without saturation or overflow.
fn array_len<T>(last: usize, op: &'static str) -> Result<usize> {
    let count = last
        .checked_add(1)
        .ok_or_else(|| nlib(op, NlibCode::Memory))?;
    array_layout::<T>(count, op)?;
    Ok(count)
}

/// Reserves before zero-filling so allocation failures return through the kernel API.
fn zeros(count: usize, op: &'static str) -> Result<Vec<f64>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| nlib(op, NlibCode::Memory))?;
    values.resize(count, 0.0);
    Ok(values)
}

// ---- curve functions ----------------------------------------------------------------

/// A scalar B-spline curve function `f(u) = sum_i N[i,p](u) * fu[i]` (upstream
/// `NL_CFUN`): values `fu[0..=n]`, degree `p`, knots `U[0..=m]`.
///
/// The storage of the value and knot arrays may be larger than the indices in use
/// (`n`, `m`), as upstream; the accessors return only the parts in use.
#[derive(Debug, Clone, PartialEq)]
pub struct CurveFunction {
    /// Value storage, at least `n + 1` entries.
    fu: Vec<f64>,
    n: usize,
    p: usize,
    /// Knot storage, at least `m + 1` entries.
    knots: Vec<f64>,
    m: usize,
}

impl CurveFunction {
    /// Curve function over whole arrays (`n = fu.len() - 1`, `m = knots.len() - 1`).
    ///
    /// Errors with `InvalidInput` for an empty value or knot array. Nothing else is
    /// checked.
    pub fn new(fu: Vec<f64>, p: usize, knots: Vec<f64>) -> Result<Self> {
        let n = last_index(&fu, OP_CRV_NEW, "empty value array")?;
        let m = last_index(&knots, OP_CRV_NEW, "empty knot vector")?;
        Ok(Self { fu, n, p, knots, m })
    }

    /// Curve function using `fu[0..=n]` and `knots[0..=m]` of possibly larger storage.
    ///
    /// Errors with `InvalidInput` when the storage is smaller than the indices.
    pub fn with_indices(
        fu: Vec<f64>,
        n: usize,
        p: usize,
        knots: Vec<f64>,
        m: usize,
    ) -> Result<Self> {
        if fu.len() <= n || knots.len() <= m {
            return Err(invalid(OP_CRV_WITH, "storage smaller than (n, m)"));
        }
        Ok(Self { fu, n, p, knots, m })
    }

    /// Highest index of the values in use.
    #[must_use]
    pub fn n(&self) -> usize {
        self.n
    }

    /// Highest index of the knots in use.
    #[must_use]
    pub fn m(&self) -> usize {
        self.m
    }

    /// `(n, m)` (`N_CFuncGetArraySizes`).
    #[must_use]
    pub fn array_sizes(&self) -> (usize, usize) {
        (self.n, self.m)
    }

    /// The degree `p` (`N_CFuncGetDegree`).
    #[must_use]
    pub fn degree(&self) -> usize {
        self.p
    }

    /// The values in use, `fu[0..=n]`.
    #[must_use]
    pub fn values(&self) -> &[f64] {
        self.fu.get(..=self.n).unwrap_or(&self.fu)
    }

    /// The knots in use, `U[0..=m]`.
    #[must_use]
    pub fn knots(&self) -> &[f64] {
        self.knots.get(..=self.m).unwrap_or(&self.knots)
    }

    /// Mutable values in use, `fu[0..=n]` (written through `N_CrvFuncCntrlValKnots`
    /// upstream).
    pub fn values_mut(&mut self) -> &mut [f64] {
        let k = self.fu.len().min(self.n.saturating_add(1));
        self.fu.get_mut(..k).unwrap_or_default()
    }

    /// Mutable knots in use, `U[0..=m]` (written through `N_CrvFuncCntrlValKnots`
    /// upstream).
    pub fn knots_mut(&mut self) -> &mut [f64] {
        let k = self.knots.len().min(self.m.saturating_add(1));
        self.knots.get_mut(..k).unwrap_or_default()
    }

    /// Size of the value and knot storage (highest storable indices plus one).
    #[must_use]
    pub fn storage(&self) -> (usize, usize) {
        (self.fu.len(), self.knots.len())
    }

    /// The knot vector `U[0..=m]` (`N_CFuncGetKnotVector`). It borrows the whole
    /// knot storage, so it can be regrown up to it.
    pub fn knot_vector(&self) -> Result<KnotVector<'_>> {
        KnotVector::new(&self.knots, self.m)
    }

    /// Checks that the indices in use are at least `nf` and `mk`
    /// (`N_CrvIsFuncSized`).
    ///
    /// Errors with [`NlibCode::Storage`] (`NL_STO_ERR`) otherwise.
    pub fn is_sized(&self, nf: usize, mk: usize) -> Result<()> {
        if self.n < nf || self.m < mk {
            return Err(nlib(OP_CRV_SIZED, NlibCode::Storage));
        }
        Ok(())
    }

    /// Sets the indices in use and the degree (`N_CFuncSetSizeIndices`).
    ///
    /// Errors with `InvalidInput` (changing nothing) when the storage cannot hold
    /// the indices (upstream would then index past its arrays).
    pub fn set_size_indices(&mut self, n: usize, p: usize, m: usize) -> Result<()> {
        if self.fu.len() <= n || self.knots.len() <= m {
            return Err(invalid(OP_CRV_SET_SIZE, "storage smaller than the indices"));
        }
        self.n = n;
        self.p = p;
        self.m = m;
        Ok(())
    }

    /// Makes `slot` hold a curve function with indices `(n, m)` and degree `p`
    /// (`N_CFuncSizeArrays`).
    ///
    /// A NULL (`None`) slot gets new zero-filled storage. An existing function must
    /// have indices in use of at least `n` and `m` ([`CurveFunction::is_sized`]); its
    /// storage is kept and the indices and degree are reset to the requested values.
    ///
    /// Errors with [`NlibCode::Storage`] when the existing function is too small
    /// or [`NlibCode::Memory`] when new storage cannot be represented or reserved.
    /// The slot is left unchanged on error.
    pub fn size_arrays(slot: &mut Option<Self>, n: usize, p: usize, m: usize) -> Result<&mut Self> {
        if let Some(cfn) = slot {
            cfn.is_sized(n, m)?;
            cfn.set_size_indices(n, p, m)?;
            Ok(cfn)
        } else {
            let values_len = array_len::<f64>(n, OP_CRV_SIZE_ARRAYS)?;
            let knots_len = array_len::<f64>(m, OP_CRV_SIZE_ARRAYS)?;
            Ok(slot.insert(Self {
                fu: zeros(values_len, OP_CRV_SIZE_ARRAYS)?,
                n,
                p,
                knots: zeros(knots_len, OP_CRV_SIZE_ARRAYS)?,
                m,
            }))
        }
    }

    /// Shrinks the storage to exactly the indices in use (`N_CrvFuncCompact`).
    pub fn compact(&mut self) {
        self.fu.truncate(self.n.saturating_add(1));
        self.fu.shrink_to_fit();
        self.knots.truncate(self.m.saturating_add(1));
        self.knots.shrink_to_fit();
    }

    /// Replaces the value and knot storage, keeping the indices in use
    /// (`N_CrvFuncSetPtrs`).
    ///
    /// Errors with `InvalidInput` (changing nothing) when the new arrays cannot hold
    /// the indices in use.
    pub fn set_arrays(&mut self, fu: Vec<f64>, knots: Vec<f64>) -> Result<()> {
        if fu.len() <= self.n || knots.len() <= self.m {
            return Err(invalid(OP_CRV_SET_ARRAYS, "arrays smaller than (n, m)"));
        }
        self.fu = fu;
        self.knots = knots;
        Ok(())
    }

    /// The function value at `u` (`N_CFuncEval`): `sum_i N[spn-p+i,p](u) * fu[spn-p+i]`
    /// with the basis of [`KnotVector::basis_eval`].
    ///
    /// Errors: [`NlibCode::Parameter`] when `u` is outside `[U[0], U[m]]`; the errors
    /// of [`KnotVector::basis_eval`]; `InvalidInput` where upstream would read past
    /// the values (`n < m - p - 1`).
    pub fn eval(&self, u: f64, side: Side) -> Result<f64> {
        const OP: &str = OP_CRV_EVAL;
        // Upstream reads the value storage directly.
        let fu = &self.fu;
        let p = self.p;
        let knt = self.knot_vector()?;

        // Check parameter.
        if knt.is_param_out_of_bounds(u) {
            return Err(nlib(OP, NlibCode::Parameter));
        }

        // Compute non-vanishing B-splines.
        let basis = knt.basis_eval(p, u, side)?;
        let first = sub(basis.span, p, OP)?;

        // Compute the function value.
        let mut f = 0.0;
        for (i, &nv) in basis.values.iter().enumerate() {
            f += rd(fu, first + i, OP)? * nv;
        }
        Ok(f)
    }

    /// The function and its derivatives up to order `der` at `u` (`N_CFuncDerivs`):
    /// element `k` of the result is the `k`-th derivative (zero for `k > p`).
    ///
    /// [`Side::Left`] gives right derivatives at a knot, [`Side::Right`] left ones.
    ///
    /// Errors: [`NlibCode::Parameter`] when `u` is outside `[U[0], U[m]]`; the errors
    /// of [`basis_derivs`]; `InvalidInput` where upstream would read past the values
    /// (`n < m - p - 1`).
    pub fn derivs(&self, u: f64, side: Side, der: usize) -> Result<Vec<f64>> {
        const OP: &str = OP_CRV_DERIVS;
        // Upstream reads the value storage directly.
        let fu = &self.fu;
        let p = self.p;
        let knt = self.knot_vector()?;

        // Check parameter.
        if knt.is_param_out_of_bounds(u) {
            return Err(nlib(OP, NlibCode::Parameter));
        }

        // Compute derivatives.
        let nd = basis_derivs(&knt, p, u, side, der)?;
        let spn = nd.span;
        let first = sub(spn, p, OP)?;
        let mut fd = Vec::with_capacity(nd.derivs.len());
        for row in &nd.derivs {
            let mut acc = 0.0;
            for (r, &nv) in row.iter().enumerate() {
                acc += rd(fu, first + r, OP)? * nv;
            }
            fd.push(acc);
        }
        Ok(fd)
    }
}

// ---- surface functions --------------------------------------------------------------

/// A scalar B-spline surface function
/// `F(u,v) = sum_i sum_j N[i,p](u) N[j,q](v) fuv[i][j]` (upstream `NL_SFUN`): values
/// `fuv[0..=n][0..=m]`, degrees `p`, `q`, knots `U[0..=r]`, `V[0..=s]`.
///
/// The storage may be larger than the indices in use, as upstream; the accessors
/// return only the parts in use.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceFunction {
    /// Value storage; rows `0..=n` hold at least `m + 1` values.
    fuv: Vec<Vec<f64>>,
    n: usize,
    m: usize,
    p: usize,
    q: usize,
    /// U knot storage, at least `r + 1` entries.
    u: Vec<f64>,
    r: usize,
    /// V knot storage, at least `s + 1` entries.
    v: Vec<f64>,
    s: usize,
}

impl SurfaceFunction {
    /// Surface function over a whole rectangular value array and whole knot arrays.
    ///
    /// Errors with `InvalidInput` for an empty value array, rows of different
    /// lengths, or an empty knot array. Nothing else is checked.
    pub fn new(fuv: Vec<Vec<f64>>, p: usize, q: usize, u: Vec<f64>, v: Vec<f64>) -> Result<Self> {
        let n = fuv
            .len()
            .checked_sub(1)
            .ok_or_else(|| invalid(OP_SRF_NEW, "empty value array"))?;
        let cols = fuv.first().map_or(0, Vec::len);
        let m = cols
            .checked_sub(1)
            .ok_or_else(|| invalid(OP_SRF_NEW, "empty value row"))?;
        if fuv.iter().any(|row| row.len() != cols) {
            return Err(invalid(OP_SRF_NEW, "value rows differ in length"));
        }
        let r = last_index(&u, OP_SRF_NEW, "empty u knot vector")?;
        let s = last_index(&v, OP_SRF_NEW, "empty v knot vector")?;
        Ok(Self {
            fuv,
            n,
            m,
            p,
            q,
            u,
            r,
            v,
            s,
        })
    }

    /// Surface function using `fuv[0..=n][0..=m]`, `u[0..=r]` and `v[0..=s]` of
    /// possibly larger storage.
    ///
    /// Errors with `InvalidInput` when the storage is smaller than the indices.
    pub fn with_indices(
        fuv: Vec<Vec<f64>>,
        n: usize,
        m: usize,
        p: usize,
        q: usize,
        u: Vec<f64>,
        r: usize,
        v: Vec<f64>,
        s: usize,
    ) -> Result<Self> {
        if !rows_hold(&fuv, n, m) {
            return Err(invalid(OP_SRF_WITH, "value array smaller than (n, m)"));
        }
        if u.len() <= r || v.len() <= s {
            return Err(invalid(OP_SRF_WITH, "knot array smaller than (r, s)"));
        }
        Ok(Self {
            fuv,
            n,
            m,
            p,
            q,
            u,
            r,
            v,
            s,
        })
    }

    /// `(n, m, r, s)` (`N_SFuncGetArraySizes`).
    #[must_use]
    pub fn array_sizes(&self) -> (usize, usize, usize, usize) {
        (self.n, self.m, self.r, self.s)
    }

    /// `(p, q)` (`N_SFuncGetDegrees`).
    #[must_use]
    pub fn degrees(&self) -> (usize, usize) {
        (self.p, self.q)
    }

    /// The value rows in use, `fuv[i][0..=m]` for `i = 0..=n` (`N_SrfFuncCntrlVal`).
    pub fn rows(&self) -> impl Iterator<Item = &[f64]> + '_ {
        let m = self.m;
        self.fuv
            .iter()
            .take(self.n.saturating_add(1))
            .map(move |row| row.get(..=m).unwrap_or(row))
    }

    /// `fuv[i][j]`, or `InvalidInput` outside the indices in use.
    pub fn value(&self, i: usize, j: usize) -> Result<f64> {
        if i > self.n || j > self.m {
            return Err(out_of_range(OP_SRF_VALUE));
        }
        self.fuv
            .get(i)
            .and_then(|row| row.get(j))
            .copied()
            .ok_or_else(|| out_of_range(OP_SRF_VALUE))
    }

    /// The u knots in use, `U[0..=r]` (`N_SFuncGetKnots`).
    #[must_use]
    pub fn u_knots(&self) -> &[f64] {
        self.u.get(..=self.r).unwrap_or(&self.u)
    }

    /// The v knots in use, `V[0..=s]` (`N_SFuncGetKnots`).
    #[must_use]
    pub fn v_knots(&self) -> &[f64] {
        self.v.get(..=self.s).unwrap_or(&self.v)
    }

    /// The u and v knot vectors (`N_SFuncGetKnotVectors`).
    pub fn knot_vectors(&self) -> Result<(KnotVector<'_>, KnotVector<'_>)> {
        Ok((
            KnotVector::new(&self.u, self.r)?,
            KnotVector::new(&self.v, self.s)?,
        ))
    }

    /// Checks that the indices in use are at least `(nf, mf, rk, sk)`
    /// (`N_SrfIsFuncSized`).
    ///
    /// Errors with [`NlibCode::Storage`] (`NL_STO_ERR`) otherwise.
    pub fn is_sized(&self, nf: usize, mf: usize, rk: usize, sk: usize) -> Result<()> {
        if self.n < nf || self.m < mf || self.r < rk || self.s < sk {
            return Err(nlib(OP_SRF_SIZED, NlibCode::Storage));
        }
        Ok(())
    }

    /// Sets the indices in use and the degrees (`N_SFuncSetSizeIndices`).
    ///
    /// Errors with `InvalidInput` (changing nothing) when the storage cannot hold
    /// the indices.
    pub fn set_size_indices(
        &mut self,
        n: usize,
        m: usize,
        p: usize,
        q: usize,
        r: usize,
        s: usize,
    ) -> Result<()> {
        if !rows_hold(&self.fuv, n, m) || self.u.len() <= r || self.v.len() <= s {
            return Err(invalid(OP_SRF_SET_SIZE, "storage smaller than the indices"));
        }
        self.n = n;
        self.m = m;
        self.p = p;
        self.q = q;
        self.r = r;
        self.s = s;
        Ok(())
    }

    /// Makes `slot` hold a surface function with indices `(n, m, r, s)` and degrees
    /// `(p, q)` (`N_SFuncSizeArrays`).
    ///
    /// A NULL (`None`) slot gets new zero-filled storage. An existing function must
    /// have indices in use of at least the requested ones
    /// ([`SurfaceFunction::is_sized`]); its storage is kept and the indices and
    /// degrees are reset to the requested values.
    ///
    /// Errors with [`NlibCode::Storage`] when the existing function is too small
    /// or [`NlibCode::Memory`] when new storage cannot be represented or reserved.
    /// The complete grid size is checked before allocating any rows; the slot is
    /// left unchanged on error.
    pub fn size_arrays(
        slot: &mut Option<Self>,
        n: usize,
        m: usize,
        p: usize,
        q: usize,
        r: usize,
        s: usize,
    ) -> Result<&mut Self> {
        if let Some(sfn) = slot {
            sfn.is_sized(n, m, r, s)?;
            sfn.set_size_indices(n, m, p, q, r, s)?;
            Ok(sfn)
        } else {
            const OP: &str = OP_SRF_SIZE_ARRAYS;
            let rows = array_len::<Vec<f64>>(n, OP)?;
            let cols = array_len::<f64>(m, OP)?;
            let u_len = array_len::<f64>(r, OP)?;
            let v_len = array_len::<f64>(s, OP)?;
            let cells = rows
                .checked_mul(cols)
                .ok_or_else(|| nlib(OP, NlibCode::Memory))?;
            array_layout::<f64>(cells, OP)?;

            let mut fuv = Vec::new();
            fuv.try_reserve_exact(rows)
                .map_err(|_| nlib(OP, NlibCode::Memory))?;
            for _ in 0..rows {
                fuv.push(zeros(cols, OP)?);
            }
            Ok(slot.insert(Self {
                fuv,
                n,
                m,
                p,
                q,
                u: zeros(u_len, OP)?,
                r,
                v: zeros(v_len, OP)?,
                s,
            }))
        }
    }

    /// The function value at `(u, v)` (`N_SrfFuncEvalPt`), with the basis of
    /// [`KnotVector::basis_eval`] in each direction.
    ///
    /// Errors: [`NlibCode::Parameter`] when `u` or `v` is outside its knot range; the
    /// errors of [`KnotVector::basis_eval`] (u first); `InvalidInput` where upstream
    /// would read past the value storage.
    pub fn eval(&self, u: f64, v: f64, ufl: Side, vfl: Side) -> Result<f64> {
        const OP: &str = OP_SRF_EVAL;
        let (p, q) = (self.p, self.q);
        let (knu, knv) = self.knot_vectors()?;

        // Check parameters.
        if knu.is_param_out_of_bounds(u) || knv.is_param_out_of_bounds(v) {
            return Err(nlib(OP, NlibCode::Parameter));
        }

        // Compute non-vanishing B-splines.
        let nu = knu.basis_eval(p, u, ufl)?;
        let nv = knv.basis_eval(q, v, vfl)?;

        // Compute the function value.
        let ufirst = sub(nu.span, p, OP)?;
        let vfirst = sub(nv.span, q, OP)?;
        let mut tu = Vec::with_capacity(nu.values.len());
        for i in 0..nu.values.len() {
            let tmp = ufirst + i;
            let mut t = 0.0;
            for (j, &bv) in nv.values.iter().enumerate() {
                // Upstream reads the value storage directly.
                let f = self
                    .fuv
                    .get(tmp)
                    .and_then(|row| row.get(vfirst + j))
                    .copied()
                    .ok_or_else(|| out_of_range(OP))?;
                t += bv * f;
            }
            tu.push(t);
        }
        let mut f = 0.0;
        for (&bu, &t) in nu.values.iter().zip(&tu) {
            f += bu * t;
        }
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> CurveFunction {
        // f = 1 - 2u + 3u^2 ... as a degree-2 Bezier with values 1, 0, 2.
        CurveFunction::new(vec![1.0, 0.0, 2.0], 2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).unwrap()
    }

    #[test]
    fn curve_eval_matches_bernstein() {
        let c = quad();
        for &u in &[0.0, 0.25, 0.5, 1.0] {
            let b = [(1.0 - u) * (1.0 - u), 2.0 * u * (1.0 - u), u * u];
            let want = b[0] * 1.0 + b[1] * 0.0 + b[2] * 2.0;
            let got = c.eval(u, Side::Left).unwrap();
            assert!((got - want).abs() < 1e-14, "u={u}: {got} vs {want}");
        }
    }

    #[test]
    fn curve_derivs() {
        let c = quad();
        // f(u) = (1-u)^2 + 2u^2 = 1 - 2u + 3u^2; f' = -2 + 6u; f'' = 6.
        let d = c.derivs(0.5, Side::Left, 3).unwrap();
        assert_eq!(d.len(), 4);
        assert!((d[0] - 0.75).abs() < 1e-14);
        assert!((d[1] - 1.0).abs() < 1e-14);
        assert!((d[2] - 6.0).abs() < 1e-14);
        assert_eq!(d[3], 0.0);
    }

    #[test]
    fn curve_param_out_of_bounds() {
        let c = quad();
        assert!(matches!(
            c.eval(1.5, Side::Left),
            Err(SolidError::Nlib {
                code: NlibCode::Parameter,
                ..
            })
        ));
        assert!(matches!(
            c.derivs(-0.1, Side::Right, 1),
            Err(SolidError::Nlib {
                code: NlibCode::Parameter,
                ..
            })
        ));
    }

    #[test]
    fn curve_short_values_is_invalid_input() {
        let c = CurveFunction::new(vec![1.0], 1, vec![0.0, 0.0, 0.5, 1.0, 1.0]).unwrap();
        assert!(matches!(
            c.eval(0.75, Side::Left),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn curve_sizing_and_compact() {
        let mut slot = None;
        let c = CurveFunction::size_arrays(&mut slot, 3, 2, 6).unwrap();
        assert_eq!(c.array_sizes(), (3, 6));
        assert_eq!(c.degree(), 2);
        assert_eq!(c.values(), &[0.0; 4]);
        // Shrink: allowed, storage kept.
        let c = CurveFunction::size_arrays(&mut slot, 2, 1, 4).unwrap();
        assert_eq!(c.array_sizes(), (2, 4));
        assert_eq!(c.storage(), (4, 7));
        // Grow beyond the indices in use: storage error.
        assert!(matches!(
            CurveFunction::size_arrays(&mut slot, 3, 1, 4),
            Err(SolidError::Nlib {
                code: NlibCode::Storage,
                ..
            })
        ));
        let c = slot.as_mut().unwrap();
        c.compact();
        assert_eq!(c.storage(), (3, 5));
        assert!(c.set_size_indices(3, 1, 4).is_err());
        assert!(c.set_arrays(vec![1.0; 2], vec![0.0; 5]).is_err());
        c.set_arrays(vec![1.0; 3], vec![0.0; 5]).unwrap();
        assert_eq!(c.values(), &[1.0; 3]);
        assert!(c.is_sized(2, 4).is_ok());
        assert!(c.is_sized(3, 4).is_err());
    }

    #[test]
    fn surface_eval_bilinear() {
        let s = SurfaceFunction::new(
            vec![vec![0.0, 1.0], vec![2.0, 4.0]],
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        let f = s.eval(0.5, 0.25, Side::Left, Side::Right).unwrap();
        // (1-u)(1-v)*0 + (1-u)v*1 + u(1-v)*2 + uv*4
        let want = 0.5 * 0.25 * 1.0 + 0.5 * 0.75 * 2.0 + 0.5 * 0.25 * 4.0;
        assert!((f - want).abs() < 1e-14);
        assert!(matches!(
            s.eval(0.5, 2.0, Side::Left, Side::Left),
            Err(SolidError::Nlib {
                code: NlibCode::Parameter,
                ..
            })
        ));
    }

    #[test]
    fn surface_sizing() {
        let mut slot = None;
        let s = SurfaceFunction::size_arrays(&mut slot, 2, 3, 1, 2, 4, 6).unwrap();
        assert_eq!(s.array_sizes(), (2, 3, 4, 6));
        assert_eq!(s.degrees(), (1, 2));
        assert_eq!(s.rows().count(), 3);
        assert!(SurfaceFunction::size_arrays(&mut slot, 2, 4, 1, 2, 4, 6).is_err());
        let s = SurfaceFunction::size_arrays(&mut slot, 1, 1, 1, 1, 3, 3).unwrap();
        assert_eq!(s.array_sizes(), (1, 1, 3, 3));
        assert_eq!(s.u_knots().len(), 4);
        assert!(s.is_sized(1, 1, 3, 3).is_ok());
        assert!(s.is_sized(1, 1, 3, 4).is_err());
        assert!(s.set_size_indices(3, 1, 1, 1, 3, 3).is_err());
        assert!(SurfaceFunction::new(vec![vec![1.0], vec![]], 0, 0, vec![0.0], vec![0.0]).is_err());
    }

    #[test]
    fn curve_sizing_rejects_unrepresentable_allocations_without_changing_slot() {
        for last in [usize::MAX, isize::MAX as usize / size_of::<f64>()] {
            for (n, m) in [(last, 0), (0, last)] {
                let mut slot = None;
                assert!(matches!(
                    CurveFunction::size_arrays(&mut slot, n, 1, m),
                    Err(SolidError::Nlib {
                        code: NlibCode::Memory,
                        ..
                    })
                ));
                assert!(slot.is_none());
            }
        }
    }

    #[test]
    fn surface_sizing_rejects_unrepresentable_allocations_without_changing_slot() {
        for last in [usize::MAX, isize::MAX as usize / size_of::<f64>()] {
            for (n, m, r, s) in [
                (last, 0, 0, 0),
                (0, last, 0, 0),
                (0, 0, last, 0),
                (0, 0, 0, last),
            ] {
                let mut slot = None;
                assert!(matches!(
                    SurfaceFunction::size_arrays(&mut slot, n, m, 1, 1, r, s),
                    Err(SolidError::Nlib {
                        code: NlibCode::Memory,
                        ..
                    })
                ));
                assert!(slot.is_none());
            }
        }
    }

    #[test]
    fn surface_sizing_checks_the_whole_grid_before_allocating_rows() {
        // Each row and the row table fit separately, but the complete grid does not.
        let side = 1_usize << (usize::BITS / 2);
        for (n, m) in [(1, isize::MAX as usize / 16), (side - 1, side - 1)] {
            let mut slot = None;
            assert!(matches!(
                SurfaceFunction::size_arrays(&mut slot, n, m, 1, 1, 0, 0),
                Err(SolidError::Nlib {
                    operation: OP_SRF_SIZE_ARRAYS,
                    code: NlibCode::Memory,
                })
            ));
            assert!(slot.is_none());
        }
    }

    #[test]
    fn oversized_requests_preserve_existing_functions_and_storage_errors() {
        let mut curve = Some(quad());
        let original_curve = curve.clone();
        assert!(matches!(
            CurveFunction::size_arrays(&mut curve, usize::MAX, 0, 0),
            Err(SolidError::Nlib {
                code: NlibCode::Storage,
                ..
            })
        ));
        assert_eq!(curve, original_curve);

        let mut surface = None;
        SurfaceFunction::size_arrays(&mut surface, 1, 1, 1, 1, 3, 3).unwrap();
        let original_surface = surface.clone();
        assert!(matches!(
            SurfaceFunction::size_arrays(&mut surface, 0, 0, 0, 0, usize::MAX, 0),
            Err(SolidError::Nlib {
                code: NlibCode::Storage,
                ..
            })
        ));
        assert_eq!(surface, original_surface);
    }
}
