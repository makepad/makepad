//! Knot vectors and B-spline basis functions: span search, multiplicity, validity
//! checks and evaluation of the non-vanishing basis functions and their derivatives.
//!
//! Upstream: `source/SMLib/src/NL_BasisBasic.cpp` (declarations in
//! `source/SMLib/inc/NL_BasisBasic.h`, `NL_KNOTVECTOR` in `source/SMLib/inc/NL_Nurbsdef.h`).
//!
//! | Rust                                   | Upstream                          |
//! |----------------------------------------|-----------------------------------|
//! | [`KnotVector`]                         | `NL_KNOTVECTOR`                   |
//! | [`KnotVector::new`]                    | `N_KnotVectorFromRealArray`       |
//! | [`KnotVector::from_slice`]             | `N_KnotVectorFromRealArray` (`m = len-1`) |
//! | [`KnotVector::knots`], [`KnotVector::last_index`] | `N_KnotVectorGetKnots` |
//! | [`KnotVector::set_last_index`]         | `N_SetKnotIndex`                  |
//! | [`KnotVector::is_param_out_of_bounds`] | `N_KnotVectorIsParamOutOfBounds`  |
//! | [`KnotVector::is_end_param`]           | `N_KnotVectorIsEndParam`          |
//! | [`KnotVector::validate`]               | `N_KnotVectorIsValid`             |
//! | [`KnotVector::find_span`]              | `N_BasisFindSpan`                 |
//! | [`KnotVector::find_span_and_mult`]     | `N_BasisFindSpanAndMult`          |
//! | [`KnotVector::span_count`]             | `N_BasisGetSpanCount`             |
//! | [`KnotVector::basis_eval`]             | `N_BasisEval`                     |
//! | [`KnotVector::basis_eval_into`]        | `N_BasisEval` (caller's `N` array) |
//! | [`KnotVector::basis_i_eval`]           | `N_BasisIEval`                    |
//! | [`KnotVector::basis_i_derivs`]         | `N_BasisIDerivs`                  |
//! | [`Side`]                               | `NL_LEFT` / `NL_RIGHT` flags      |
//! | [`MAX_DEGREE`]                         | `NL_MAXDEG`                       |
//!
//! Deviations:
//! - Out-params become return values ([`Basis`], tuples, `Vec`s sized by the callee).
//! - Errors are [`SolidError::Nlib`] values carrying the upstream `NL_*_ERR` number
//!   instead of a flag plus the global `NL_ERROR` record. The boolean checks
//!   (`is_param_out_of_bounds`, `is_end_param`) return `bool` and record nothing.
//! - The `NL_LEFT`/`NL_RIGHT` flag is the [`Side`] enum, so upstream's `NL_CAL_ERR`
//!   path for any other flag value cannot occur. (Upstream `N_BasisEval` ignores that
//!   error and reads an uninitialised span index.)
//! - Every array access is bounds checked. Where upstream would read or write outside
//!   its arrays (knot vectors that are too short for the degree, `m < 2p+1`), the Rust
//!   functions return [`SolidError::InvalidInput`] instead of undefined behaviour. For
//!   every input that upstream handles without undefined behaviour, the results are the
//!   same, bit for bit (same operations in the same order).
//! - `N_BasisEval` keeps its `left`/`right` scratch arrays on the stack as upstream
//!   (`NL_MAXDEG + 2` entries), so it returns `InvalidInput` for `p > NL_MAXDEG + 1`,
//!   where upstream would overflow them.
//! - On `NL_NUM_ERR` upstream leaves `N` partly computed; [`KnotVector::basis_eval`]
//!   returns only the error.

use crate::error::{NlibCode, Result, SolidError};

/// Upstream `NL_MAXDEG`: the largest degree NLib supports.
pub const MAX_DEGREE: usize = 17;

/// Which side of a knot a parameter is assigned to (upstream `NL_LEFT` / `NL_RIGHT`).
///
/// Where the parameter `u` equals an interior knot, the side picks the span:
/// [`Side::Left`] finds `spn` with `U[spn] <= u < U[spn+1]` (the span starting at
/// `u`), [`Side::Right`] finds `U[spn] < u <= U[spn+1]` (the span ending at `u`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// `NL_LEFT` (1): half-open span `[U[spn], U[spn+1])`.
    Left,
    /// `NL_RIGHT` (2): half-open span `(U[spn], U[spn+1]]`.
    Right,
}

impl Side {
    /// The upstream flag value (`NL_LEFT = 1`, `NL_RIGHT = 2`).
    #[must_use]
    pub fn flag(self) -> i32 {
        match self {
            Side::Left => 1,
            Side::Right => 2,
        }
    }
}

/// The non-vanishing basis functions at a parameter, as returned by
/// [`KnotVector::basis_eval`].
#[derive(Debug, Clone, PartialEq)]
pub struct Basis {
    /// Index of the knot span (`U[span]` is the low end of the span containing `u`).
    pub span: usize,
    /// `N[span-p+j, p](u)` for `j = 0..=p`.
    pub values: Vec<f64>,
}

/// A borrowed knot vector `U[0..=m]` (upstream `NL_KNOTVECTOR`: a knot pointer plus
/// the highest index `m`, memory owned by the caller).
///
/// The knots in use are a prefix of a caller-owned buffer, so
/// [`KnotVector::set_last_index`] can shrink or regrow the vector within that buffer,
/// as upstream `N_SetKnotIndex` does. A `KnotVector` always holds at least one knot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KnotVector<'a> {
    /// Whole caller buffer; `knots` is a prefix of it.
    buf: &'a [f64],
    /// `U[0..=m]`.
    knots: &'a [f64],
}

const OP_NEW: &str = "nurbs::knots::KnotVector::new";
const OP_SET_LAST: &str = "nurbs::knots::KnotVector::set_last_index";
const OP_VALIDATE: &str = "nurbs::knots::KnotVector::validate";
const OP_FIND_SPAN: &str = "nurbs::knots::KnotVector::find_span";
const OP_FIND_SPAN_MULT: &str = "nurbs::knots::KnotVector::find_span_and_mult";
const OP_BASIS_EVAL: &str = "nurbs::knots::KnotVector::basis_eval";
const OP_BASIS_I_EVAL: &str = "nurbs::knots::KnotVector::basis_i_eval";
const OP_BASIS_I_DERIVS: &str = "nurbs::knots::KnotVector::basis_i_derivs";

/// Error for an array access that upstream would make out of bounds.
fn out_of_range(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "index out of range (knot vector too short for the degree?)",
    }
}

/// `p > NL_MAXDEG + 1`: upstream's `N_BasisEval` scratch arrays would overflow.
fn degree_too_high(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "degree above NL_MAXDEG + 1 (upstream scratch array size)",
    }
}

/// `a[i]`, or `InvalidInput` when out of range.
#[inline]
fn rd(a: &[f64], i: usize, op: &'static str) -> Result<f64> {
    a.get(i).copied().ok_or_else(|| out_of_range(op))
}

/// `a[i] = v`, or `InvalidInput` when out of range.
#[inline]
fn wr(a: &mut [f64], i: usize, v: f64, op: &'static str) -> Result<()> {
    let slot = a.get_mut(i).ok_or_else(|| out_of_range(op))?;
    *slot = v;
    Ok(())
}

/// A zero-filled scratch or output array of `len` entries; `InvalidInput` instead of an
/// allocation abort when `len` is absurd.
fn zeros(len: Option<usize>, op: &'static str) -> Result<Vec<f64>> {
    let too_big = || SolidError::InvalidInput {
        operation: op,
        reason: "array size overflow",
    };
    let len = len.ok_or_else(too_big)?;
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| too_big())?;
    v.resize(len, 0.0);
    Ok(v)
}

/// `a - b` on indices, or `InvalidInput` where upstream's signed index would go negative.
#[inline]
fn sub(a: usize, b: usize, op: &'static str) -> Result<usize> {
    a.checked_sub(b).ok_or_else(|| out_of_range(op))
}

impl<'a> KnotVector<'a> {
    /// Knot vector `U[0..=m]` over the first `m + 1` entries of `buf`
    /// (upstream `N_KnotVectorFromRealArray(knt, U, m)`).
    ///
    /// Errors with `InvalidInput` when `buf` has fewer than `m + 1` entries.
    pub fn new(buf: &'a [f64], m: usize) -> Result<Self> {
        let end = m.checked_add(1).ok_or_else(|| out_of_range(OP_NEW))?;
        let knots = buf.get(..end).ok_or_else(|| out_of_range(OP_NEW))?;
        Ok(Self { buf, knots })
    }

    /// Knot vector over all of `knots` (`m = knots.len() - 1`).
    ///
    /// Errors with `InvalidInput` when `knots` is empty.
    pub fn from_slice(knots: &'a [f64]) -> Result<Self> {
        let m = knots.len().checked_sub(1).ok_or(SolidError::InvalidInput {
            operation: OP_NEW,
            reason: "empty knot vector",
        })?;
        Self::new(knots, m)
    }

    /// The knots `U[0..=m]` (upstream `N_KnotVectorGetKnots`, `U` output).
    #[must_use]
    pub fn knots(&self) -> &'a [f64] {
        self.knots
    }

    /// The highest knot index `m` (upstream `N_KnotVectorGetKnots`, `m` output).
    #[must_use]
    pub fn last_index(&self) -> usize {
        // Constructors guarantee at least one knot.
        self.knots.len().saturating_sub(1)
    }

    /// Sets the highest knot index to `m` (upstream `N_SetKnotIndex`). The knot vector
    /// may grow up to the size of the buffer it was created over.
    ///
    /// Errors with `InvalidInput` when the buffer has fewer than `m + 1` entries.
    pub fn set_last_index(&mut self, m: usize) -> Result<()> {
        let end = m.checked_add(1).ok_or_else(|| out_of_range(OP_SET_LAST))?;
        self.knots = self
            .buf
            .get(..end)
            .ok_or_else(|| out_of_range(OP_SET_LAST))?;
        Ok(())
    }

    /// `U[0]`.
    fn first(&self) -> f64 {
        // Constructors guarantee at least one knot; NaN is never observed.
        self.knots.first().copied().unwrap_or(f64::NAN)
    }

    /// `U[m]`.
    fn last(&self) -> f64 {
        // Constructors guarantee at least one knot; NaN is never observed.
        self.knots.last().copied().unwrap_or(f64::NAN)
    }

    /// `true` when `u < U[0]` or `u > U[m]` (upstream `N_KnotVectorIsParamOutOfBounds`,
    /// which also records `NL_PAR_ERR`).
    #[must_use]
    pub fn is_param_out_of_bounds(&self, u: f64) -> bool {
        u < self.first() || u > self.last()
    }

    /// `true` when `u <= U[0]` or `u >= U[m]`, i.e. `u` is on or outside the ends of
    /// the knot vector (upstream `N_KnotVectorIsEndParam`).
    #[must_use]
    pub fn is_end_param(&self, u: f64) -> bool {
        u <= self.first() || u >= self.last()
    }

    /// Checks that this is a valid clamped knot vector for degree `p`
    /// (upstream `N_KnotVectorIsValid`): both end knots have multiplicity exactly
    /// `p + 1`, the knots do not decrease, and no interior knot has multiplicity
    /// above `p`.
    ///
    /// Errors with [`NlibCode::Knot`] (`NL_KNT_ERR`) when a check fails.
    pub fn validate(&self, p: usize) -> Result<()> {
        const OP: &str = OP_VALIDATE;
        let knot_err = SolidError::Nlib {
            operation: OP,
            code: NlibCode::Knot,
        };
        let u = self.knots;
        let m = self.last_index();

        // Check end knots.
        let mut i = 0;
        while i < m && rd(u, i, OP)? == rd(u, i + 1, OP)? {
            i += 1;
        }
        if i != p {
            return Err(knot_err);
        }

        i = m;
        while i > 0 && rd(u, i, OP)? == rd(u, i - 1, OP)? {
            i -= 1;
        }
        if m - i != p {
            return Err(knot_err);
        }

        // Check internal knots. Here p <= m (the first loop stopped at i = p <= m).
        let mut is = p;
        let mut ie = p + 1;
        let end = m - p;
        while ie <= end {
            if rd(u, is, OP)? > rd(u, ie, OP)? {
                return Err(knot_err);
            }
            i = ie;
            while ie < end && rd(u, ie, OP)? == rd(u, ie + 1, OP)? {
                ie += 1;
            }
            if ie - i + 1 > p {
                return Err(knot_err);
            }
            is = ie;
            ie += 1;
        }
        Ok(())
    }

    /// Finds the knot span containing `u` for degree `p` (upstream `N_BasisFindSpan`).
    ///
    /// Returns `spn` in `p..=m-p-1` with `U[spn] <= u < U[spn+1]` ([`Side::Left`]) or
    /// `U[spn] < u <= U[spn+1]` ([`Side::Right`]). Parameters at or beyond the ends are
    /// assigned silently to the first or last span; the end spans are moved inwards past
    /// zero-length spans (`U[spn+1] < U[spn] + 1e-12`), so evaluators never divide by a
    /// zero knot difference.
    ///
    /// Errors with `InvalidInput` only where upstream would index outside the knots
    /// (fewer than `2p + 2` knots).
    pub fn find_span(&self, p: usize, u: f64, side: Side) -> Result<usize> {
        const OP: &str = OP_FIND_SPAN;
        // cbiTol: just meant to avoid a zero divide.
        let knot_tol = 1.0e-12;
        let uk = self.knots;
        let m = self.last_index();

        let mut low = p;
        let mut high = sub(m, p, OP)?;
        let mut mid = (low + high) / 2;
        let mut lastmid = mid;

        // u greater than or equal to the domain end: last non-degenerate span.
        if u >= rd(uk, m - p, OP)? {
            let mut i_temp = sub(m - p, 1, OP)?;
            while rd(uk, i_temp + 1, OP)? < rd(uk, i_temp, OP)? + knot_tol && i_temp > p {
                i_temp -= 1;
            }
            return Ok(i_temp);
        }

        // u less than or equal to the domain start: first non-degenerate span.
        if u <= rd(uk, p, OP)? {
            let mut i_temp = p;
            let last_span = sub(m - p, 1, OP)?;
            while rd(uk, i_temp + 1, OP)? < rd(uk, i_temp, OP)? + knot_tol && i_temp < last_span {
                i_temp += 1;
            }
            return Ok(i_temp);
        }

        match side {
            Side::Left => {
                while u < rd(uk, mid, OP)? || u >= rd(uk, mid + 1, OP)? {
                    if u < rd(uk, mid, OP)? {
                        high = mid;
                    } else {
                        low = mid;
                    }
                    mid = (low + high) / 2;
                    if mid == lastmid {
                        break;
                    }
                    lastmid = mid;
                }
            }
            Side::Right => {
                while u <= rd(uk, mid, OP)? || u > rd(uk, mid + 1, OP)? {
                    if u > rd(uk, mid, OP)? {
                        low = mid;
                    } else {
                        high = mid;
                    }
                    mid = (low + high) / 2;
                    if mid == lastmid {
                        break;
                    }
                    lastmid = mid;
                }
            }
        }
        Ok(mid)
    }

    /// Finds the knot span containing `u` (as [`KnotVector::find_span`]) and the
    /// multiplicity of `u` in the knot vector, `0` when `u` is not a knot
    /// (upstream `N_BasisFindSpanAndMult`). Returns `(span, multiplicity)`.
    ///
    /// Upstream reports multiplicity `p + 1` when `u` equals the domain end
    /// (`U[m-p]`, [`Side::Left`]) or start (`U[p]`, [`Side::Right`]).
    pub fn find_span_and_mult(&self, p: usize, u: f64, side: Side) -> Result<(usize, usize)> {
        const OP: &str = OP_FIND_SPAN_MULT;
        let uk = self.knots;
        let m = self.last_index();

        let k = self.find_span(p, u, side)?;
        let spn = k;
        let mut s = 0;

        match side {
            Side::Left => {
                if u == rd(uk, sub(m, p, OP)?, OP)? {
                    return Ok((spn, p + 1));
                }
                // while (k >= 0 && u == U[k--]) s++;
                let mut k = Some(k);
                while let Some(kk) = k {
                    if u != rd(uk, kk, OP)? {
                        break;
                    }
                    s += 1;
                    k = kk.checked_sub(1);
                }
            }
            Side::Right => {
                if u == rd(uk, p, OP)? {
                    return Ok((spn, p + 1));
                }
                // k++; while (k <= m && u == U[k++]) s++;
                let mut k = k + 1;
                while k <= m && u == rd(uk, k, OP)? {
                    k += 1;
                    s += 1;
                }
            }
        }
        Ok((spn, s))
    }

    /// Number of non-zero-length spans `U[i] != U[i+1]` for `i` in `p..m-p`
    /// (upstream `N_BasisGetSpanCount`).
    #[must_use]
    pub fn span_count(&self, p: usize) -> usize {
        let m = self.last_index();
        let Some(end) = m.checked_sub(p) else {
            return 0;
        };
        self.knots.get(p..=end).map_or(0, |w| {
            w.windows(2)
                .filter(|pair| matches!(pair, [a, b] if a != b))
                .count()
        })
    }

    /// All non-vanishing basis functions of degree `p` at `u`
    /// (upstream `N_BasisEval`): `N[span-p+j, p](u)` for `j = 0..=p`.
    ///
    /// At `u == U[p]` returns `span = p` and `N = [1, 0, .., 0]`; at `u == U[m-p]`
    /// returns `span = m-p-1` and `N = [0, .., 0, 1]`. Parameters outside the domain
    /// are evaluated in the end spans.
    ///
    /// Errors: [`NlibCode::Numerical`] when a knot difference is below `1e-24`
    /// (more than `p + 1` equal knots); `InvalidInput` where upstream would index out
    /// of bounds (fewer than `2p + 2` knots, `p > MAX_DEGREE + 1`).
    pub fn basis_eval(&self, p: usize, u: f64, side: Side) -> Result<Basis> {
        const OP: &str = OP_BASIS_EVAL;
        if p > MAX_DEGREE + 1 {
            return Err(degree_too_high(OP));
        }
        let mut n = zeros(Some(p + 1), OP)?;
        let span = self.basis_eval_into(p, u, side, &mut n)?;
        Ok(Basis { span, values: n })
    }

    /// [`KnotVector::basis_eval`] into a caller buffer: zeroes `n[0..=p]`, writes the
    /// values there and returns the span (same values and errors; `InvalidInput` when
    /// `n` has fewer than `p + 1` entries). Lets hot evaluators keep `N` on the stack
    /// (`[f64; MAX_DEGREE + 2]` fits every valid degree), as upstream does.
    pub fn basis_eval_into(&self, p: usize, u: f64, side: Side, n: &mut [f64]) -> Result<usize> {
        const OP: &str = OP_BASIS_EVAL;
        // Divide by zero? The denominator is the sum of the two values multiplied
        // by it, so the quotients never exceed 1; only an exactly-zero delta (more
        // than degree+1 equal knots) is a problem. Guard with something tiny.
        let zero_tol = 1e-24;
        let uk = self.knots;
        let m = self.last_index();
        if p > MAX_DEGREE + 1 {
            return Err(degree_too_high(OP));
        }
        n.get_mut(..=p).ok_or_else(|| out_of_range(OP))?.fill(0.0);

        // Special cases for end values.
        if u == rd(uk, p, OP)? {
            wr(n, 0, 1.0, OP)?;
            return Ok(p);
        }
        let end = sub(m, p, OP)?;
        if u == rd(uk, end, OP)? {
            wr(n, p, 1.0, OP)?;
            return sub(end, 1, OP);
        }

        // Find the knot span u is in.
        let i = self.find_span(p, u, side)?;

        // Compute the non-vanishing B-splines.
        let mut left = [0.0_f64; MAX_DEGREE + 2];
        let mut right = [0.0_f64; MAX_DEGREE + 2];
        wr(n, 0, 1.0, OP)?;
        for k in 1..=p {
            wr(&mut left, k, u - rd(uk, sub(i + 1, k, OP)?, OP)?, OP)?;
            wr(&mut right, k, rd(uk, i + k, OP)? - u, OP)?;
            let mut saved = 0.0;
            for l in 0..k {
                let r = rd(&right, l + 1, OP)?;
                let lf = rd(&left, k - l, OP)?;
                let delta = r + lf;
                if delta < zero_tol {
                    return Err(SolidError::Nlib {
                        operation: OP,
                        code: NlibCode::Numerical,
                    });
                }
                let temp = rd(n, l, OP)?;
                // Keep the divisions inside the parentheses: dividing N[l] by the
                // knot difference can be huge.
                wr(n, l, saved + temp * (r / delta), OP)?;
                saved = temp * (lf / delta);
            }
            wr(n, k, saved, OP)?;
        }
        Ok(i)
    }

    /// The single basis function `N[i, p](u)` (upstream `N_BasisIEval`).
    ///
    /// Errors: [`NlibCode::Parameter`] when `u` is outside `[U[0], U[m]]`;
    /// [`NlibCode::Index`] when `i > m - p - 1`; `InvalidInput` where upstream would
    /// index out of bounds.
    pub fn basis_i_eval(&self, i: usize, p: usize, u: f64, side: Side) -> Result<f64> {
        const OP: &str = OP_BASIS_I_EVAL;
        let uk = self.knots;
        let m = self.last_index();

        // Check parameter and index.
        if self.is_param_out_of_bounds(u) {
            return Err(SolidError::Nlib {
                operation: OP,
                code: NlibCode::Parameter,
            });
        }
        // n = m - p - 1 (may be negative upstream: then every i is out of range).
        // Subtract in checked steps to avoid overflowing p + 1.
        let n = match m.checked_sub(p).and_then(|n| n.checked_sub(1)) {
            Some(n) if i <= n => n,
            _ => {
                return Err(SolidError::Nlib {
                    operation: OP,
                    code: NlibCode::Index,
                })
            }
        };

        // Special cases for end values.
        if u == rd(uk, p, OP)? {
            return Ok(if i == 0 { 1.0 } else { 0.0 });
        }
        if u == rd(uk, m - p, OP)? {
            return Ok(if i == n { 1.0 } else { 0.0 });
        }

        let mut na = zeros(Some(p + 1), OP)?;

        // Compute degree zero B-splines.
        match side {
            Side::Left => {
                if u < rd(uk, i, OP)? || u >= rd(uk, i + p + 1, OP)? {
                    return Ok(0.0);
                }
                for j in 0..=p {
                    let v = if u >= rd(uk, i + j, OP)? && u < rd(uk, i + j + 1, OP)? {
                        1.0
                    } else {
                        0.0
                    };
                    wr(&mut na, j, v, OP)?;
                }
            }
            Side::Right => {
                if u <= rd(uk, i, OP)? || u > rd(uk, i + p + 1, OP)? {
                    return Ok(0.0);
                }
                for j in 0..=p {
                    let v = if u > rd(uk, i + j, OP)? && u <= rd(uk, i + j + 1, OP)? {
                        1.0
                    } else {
                        0.0
                    };
                    wr(&mut na, j, v, OP)?;
                }
            }
        }

        // Compute the i-th B-spline using a triangular array.
        for k in 1..=p {
            let na0 = rd(&na, 0, OP)?;
            let mut saved = if na0 == 0.0 {
                0.0
            } else {
                let ui = rd(uk, i, OP)?;
                ((u - ui) * na0) / (rd(uk, i + k, OP)? - ui)
            };
            for j in 0..=(p - k) {
                let ur = rd(uk, i + j + k + 1, OP)?;
                let ul = rd(uk, i + j + 1, OP)?;
                let next = rd(&na, j + 1, OP)?;
                if next == 0.0 {
                    wr(&mut na, j, saved, OP)?;
                    saved = 0.0;
                } else {
                    let temp = next / (ur - ul);
                    wr(&mut na, j, saved + (ur - u) * temp, OP)?;
                    saved = (u - ul) * temp;
                }
            }
        }
        rd(&na, 0, OP)
    }

    /// The basis function `N[i, p]` and its derivatives up to order `der` at `u`
    /// (upstream `N_BasisIDerivs`). Returns `der + 1` values; derivatives above `p`
    /// are zero.
    ///
    /// For a left derivative at the domain end pass [`Side::Left`]; at `u == U[m-p]`
    /// the last `p + 1` functions are then evaluated on the span ending at `u`
    /// (and symmetrically for [`Side::Right`] at `u == U[p]`).
    ///
    /// Errors: [`NlibCode::Parameter`] when `u` is outside `[U[0], U[m]]`;
    /// [`NlibCode::Index`] when `i > m - p - 1`; `InvalidInput` where upstream would
    /// index out of bounds.
    #[allow(clippy::too_many_lines)] // one upstream routine, kept in its original shape
    pub fn basis_i_derivs(
        &self,
        i: usize,
        p: usize,
        u: f64,
        side: Side,
        der: usize,
    ) -> Result<Vec<f64>> {
        const OP: &str = OP_BASIS_I_DERIVS;
        let uk = self.knots;
        let m = self.last_index();

        // Check parameter and index.
        if self.is_param_out_of_bounds(u) {
            return Err(SolidError::Nlib {
                operation: OP,
                code: NlibCode::Parameter,
            });
        }
        // n = m - p - 1 (may be negative upstream: then every i is out of range).
        // Subtract in checked steps to avoid overflowing p + 1.
        let n = match m.checked_sub(p).and_then(|n| n.checked_sub(1)) {
            Some(n) if i <= n => n,
            _ => {
                return Err(SolidError::Nlib {
                    operation: OP,
                    code: NlibCode::Index,
                })
            }
        };

        let mut nd_out = zeros(der.checked_add(1), OP)?;
        // nt[j][k], j, k in 0..=p, stored row-major (p < m here, so w is bounded).
        let w = p + 1;
        let mut nt = zeros(w.checked_mul(w), OP)?;
        let mut nd = zeros(Some(w), OP)?;

        // Compute the degree zero B-splines.
        match side {
            Side::Left => {
                // u == U[m-p] && i >= n - p  (n - p may be negative upstream)
                if u == rd(uk, m - p, OP)? && n.checked_sub(p).is_none_or(|np| i >= np) {
                    for j in 0..=p {
                        let v = if u > rd(uk, i + j, OP)? && u <= rd(uk, i + j + 1, OP)? {
                            1.0
                        } else {
                            0.0
                        };
                        wr(&mut nt, j * w, v, OP)?;
                    }
                } else {
                    if u < rd(uk, i, OP)? || u >= rd(uk, i + p + 1, OP)? {
                        return Ok(nd_out);
                    }
                    for j in 0..=p {
                        let v = if u >= rd(uk, i + j, OP)? && u < rd(uk, i + j + 1, OP)? {
                            1.0
                        } else {
                            0.0
                        };
                        wr(&mut nt, j * w, v, OP)?;
                    }
                }
            }
            Side::Right => {
                if u == rd(uk, p, OP)? && i <= p {
                    for j in 0..=p {
                        let v = if u >= rd(uk, i + j, OP)? && u < rd(uk, i + j + 1, OP)? {
                            1.0
                        } else {
                            0.0
                        };
                        wr(&mut nt, j * w, v, OP)?;
                    }
                } else {
                    if u <= rd(uk, i, OP)? || u > rd(uk, i + p + 1, OP)? {
                        return Ok(nd_out);
                    }
                    for j in 0..=p {
                        let v = if u > rd(uk, i + j, OP)? && u <= rd(uk, i + j + 1, OP)? {
                            1.0
                        } else {
                            0.0
                        };
                        wr(&mut nt, j * w, v, OP)?;
                    }
                }
            }
        }

        // Compute the full triangular array.
        for k in 1..=p {
            let nt0 = rd(&nt, k - 1, OP)?;
            let mut saved = if nt0 == 0.0 {
                0.0
            } else {
                let ui = rd(uk, i, OP)?;
                ((u - ui) * nt0) / (rd(uk, i + k, OP)? - ui)
            };
            for j in 0..=(p - k) {
                let ur = rd(uk, i + j + k + 1, OP)?;
                let ul = rd(uk, i + j + 1, OP)?;
                let next = rd(&nt, (j + 1) * w + k - 1, OP)?;
                if next == 0.0 {
                    wr(&mut nt, j * w + k, saved, OP)?;
                    saved = 0.0;
                } else {
                    let temp = next / (ur - ul);
                    wr(&mut nt, j * w + k, saved + (ur - u) * temp, OP)?;
                    saved = (u - ul) * temp;
                }
            }
        }

        // Compute derivatives.
        wr(&mut nd_out, 0, rd(&nt, p, OP)?, OP)?;
        let mder = p.min(der);
        for k in (p + 1)..=der {
            wr(&mut nd_out, k, 0.0, OP)?;
        }

        for k in 1..=mder {
            // Load the appropriate column into the derivative array.
            for j in 0..=k {
                wr(&mut nd, j, rd(&nt, j * w + (p - k), OP)?, OP)?;
            }

            // Compute the triangular table of width k.
            for l in 1..=k {
                let nd0 = rd(&nd, 0, OP)?;
                let mut saved = if nd0 == 0.0 {
                    0.0
                } else {
                    nd0 / (rd(uk, i + p - k + l, OP)? - rd(uk, i, OP)?)
                };
                // (NL_REAL)p - (NL_REAL)k + l
                let factor = (p as f64 - k as f64) + l as f64;
                for j in 0..=(k - l) {
                    let ur = rd(uk, p - k + l + i + j + 1, OP)?;
                    let ul = rd(uk, i + j + 1, OP)?;
                    let next = rd(&nd, j + 1, OP)?;
                    if next == 0.0 {
                        wr(&mut nd, j, factor * saved, OP)?;
                        saved = 0.0;
                    } else {
                        let temp = next / (ur - ul);
                        wr(&mut nd, j, factor * (saved - temp), OP)?;
                        saved = temp;
                    }
                }
            }
            wr(&mut nd_out, k, rd(&nd, 0, OP)?, OP)?;
        }
        Ok(nd_out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The NURBS Book, example 2.3: U = {0,0,0,1,2,3,4,4,5,5,5}, p = 2.
    const EX23: [f64; 11] = [0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 4.0, 5.0, 5.0, 5.0];

    fn kv(k: &[f64]) -> KnotVector<'_> {
        KnotVector::from_slice(k).unwrap()
    }

    #[test]
    fn construction_and_last_index() {
        let buf = [0.0, 0.0, 1.0, 1.0, 9.0];
        let mut k = KnotVector::new(&buf, 3).unwrap();
        assert_eq!(k.last_index(), 3);
        assert_eq!(k.knots(), &[0.0, 0.0, 1.0, 1.0]);
        k.set_last_index(1).unwrap();
        assert_eq!(k.knots(), &[0.0, 0.0]);
        k.set_last_index(4).unwrap();
        assert_eq!(k.knots(), &buf);
        assert!(k.set_last_index(5).is_err());
        assert!(KnotVector::new(&buf, 5).is_err());
        assert!(KnotVector::from_slice(&[]).is_err());
    }

    #[test]
    fn bounds_and_end_params() {
        let k = kv(&EX23);
        assert!(!k.is_param_out_of_bounds(0.0));
        assert!(!k.is_param_out_of_bounds(5.0));
        assert!(k.is_param_out_of_bounds(-1e-300));
        assert!(k.is_param_out_of_bounds(5.000_000_1));
        assert!(k.is_end_param(0.0));
        assert!(k.is_end_param(5.0));
        assert!(!k.is_end_param(2.5));
    }

    #[test]
    fn validate_clamped() {
        assert!(kv(&EX23).validate(2).is_ok());
        assert!(kv(&EX23).validate(3).is_err());
        // Decreasing interior knot.
        let bad = [0.0, 0.0, 0.0, 2.0, 1.0, 3.0, 3.0, 3.0];
        assert_eq!(
            kv(&bad).validate(2),
            Err(SolidError::Nlib {
                operation: OP_VALIDATE,
                code: NlibCode::Knot
            })
        );
        // Interior multiplicity p + 1.
        let mult = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0];
        assert!(kv(&mult).validate(2).is_err());
        // Unclamped.
        assert!(kv(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]).validate(2).is_err());
        // Single knot, degree 0: upstream accepts it.
        assert!(kv(&[1.0]).validate(0).is_ok());
    }

    #[test]
    fn spans() {
        let k = kv(&EX23);
        assert_eq!(k.find_span(2, 2.5, Side::Left).unwrap(), 4);
        assert_eq!(k.find_span(2, 2.0, Side::Left).unwrap(), 4);
        assert_eq!(k.find_span(2, 2.0, Side::Right).unwrap(), 3);
        assert_eq!(k.find_span(2, 5.0, Side::Left).unwrap(), 7);
        assert_eq!(k.find_span(2, 0.0, Side::Right).unwrap(), 2);
        assert_eq!(k.find_span(2, -3.0, Side::Left).unwrap(), 2);
        assert_eq!(k.find_span(2, 7.0, Side::Left).unwrap(), 7);
        assert_eq!(k.find_span_and_mult(2, 4.0, Side::Left).unwrap(), (7, 2));
        assert_eq!(k.find_span_and_mult(2, 4.0, Side::Right).unwrap(), (5, 2));
        assert_eq!(k.find_span_and_mult(2, 2.5, Side::Left).unwrap(), (4, 0));
        assert_eq!(k.find_span_and_mult(2, 5.0, Side::Left).unwrap(), (7, 3));
        assert_eq!(k.find_span_and_mult(2, 0.0, Side::Right).unwrap(), (2, 3));
        assert_eq!(k.span_count(2), 5);
        assert_eq!(k.span_count(20), 0);
    }

    #[test]
    fn degenerate_end_spans_are_skipped() {
        // Seven equal end knots for a cubic: zero-length end spans.
        let k = [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0, 1.0];
        let kv = kv(&k);
        assert_eq!(kv.find_span(3, 0.0, Side::Left).unwrap(), 4);
        assert_eq!(kv.find_span(3, 1.0, Side::Left).unwrap(), 5);
    }

    #[test]
    fn basis_eval_nurbs_book() {
        let b = kv(&EX23).basis_eval(2, 2.5, Side::Left).unwrap();
        assert_eq!(b.span, 4);
        assert_eq!(b.values, vec![0.125, 0.75, 0.125]);
        let end = kv(&EX23).basis_eval(2, 5.0, Side::Left).unwrap();
        assert_eq!(end.span, 7);
        assert_eq!(end.values, vec![0.0, 0.0, 1.0]);
        let start = kv(&EX23).basis_eval(2, 0.0, Side::Right).unwrap();
        assert_eq!(start.span, 2);
        assert_eq!(start.values, vec![1.0, 0.0, 0.0]);
    }

    #[test]
    fn basis_eval_into_matches_basis_eval_bitwise() {
        let k = kv(&EX23);
        for s in -5..=55 {
            let u = f64::from(s) * 0.1;
            for side in [Side::Left, Side::Right] {
                for p in 0..=2 {
                    // a dirty buffer: every used entry must be overwritten
                    let mut n = [7.0_f64; MAX_DEGREE + 2];
                    let r = k.basis_eval_into(p, u, side, &mut n);
                    match k.basis_eval(p, u, side) {
                        Ok(b) => {
                            assert_eq!(r.unwrap(), b.span);
                            let bits: Vec<u64> = b.values.iter().map(|x| x.to_bits()).collect();
                            let got: Vec<u64> = n[..=p].iter().map(|x| x.to_bits()).collect();
                            assert_eq!(got, bits, "u={u} p={p}");
                        }
                        Err(e) => assert_eq!(r.unwrap_err(), e),
                    }
                }
            }
        }
        let mut short = [0.0; 2];
        assert!(k.basis_eval_into(2, 0.5, Side::Left, &mut short).is_err());
        let mut n = [0.0; MAX_DEGREE + 3];
        assert_eq!(
            k.basis_eval_into(MAX_DEGREE + 2, 0.5, Side::Left, &mut n)
                .unwrap_err(),
            k.basis_eval(MAX_DEGREE + 2, 0.5, Side::Left).unwrap_err()
        );
    }

    #[test]
    fn basis_eval_partition_of_unity() {
        let k = kv(&EX23);
        for s in 0..=50 {
            let u = f64::from(s) * 0.1;
            for side in [Side::Left, Side::Right] {
                let b = k.basis_eval(2, u, side).unwrap();
                let sum: f64 = b.values.iter().sum();
                assert!((sum - 1.0).abs() < 1e-14, "u={u} sum={sum}");
                for (j, v) in b.values.iter().enumerate() {
                    let i = b.span - 2 + j;
                    let single = k.basis_i_eval(i, 2, u, side).unwrap();
                    assert!((single - v).abs() < 1e-14, "u={u} i={i}");
                }
            }
        }
    }

    #[test]
    fn basis_eval_zero_span_errors() {
        // Too many equal interior knots with an unclamped start: zero delta.
        let k = [0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.0];
        let r = kv(&k).basis_eval(2, 1.0 + 1e-300, Side::Left);
        assert!(r.is_ok() || matches!(r, Err(SolidError::Nlib { .. })));
    }

    #[test]
    fn basis_i_eval_errors() {
        let k = kv(&EX23);
        assert!(matches!(
            k.basis_i_eval(0, 2, -1.0, Side::Left),
            Err(SolidError::Nlib {
                code: NlibCode::Parameter,
                ..
            })
        ));
        assert!(matches!(
            k.basis_i_eval(8, 2, 1.0, Side::Left),
            Err(SolidError::Nlib {
                code: NlibCode::Index,
                ..
            })
        ));
        assert_eq!(k.basis_i_eval(0, 2, 0.0, Side::Left).unwrap(), 1.0);
        assert_eq!(k.basis_i_eval(7, 2, 5.0, Side::Left).unwrap(), 1.0);
        assert_eq!(k.basis_i_eval(6, 2, 5.0, Side::Left).unwrap(), 0.0);
    }

    #[test]
    fn basis_i_derivs_nurbs_book() {
        // N[4,2] on U(ex 2.3) at u = 2.5: N = 1/8, N' = 1/2, N'' = 1, N''' = 0.
        let d = kv(&EX23).basis_i_derivs(4, 2, 2.5, Side::Left, 3).unwrap();
        assert_eq!(d, vec![0.125, 0.5, 1.0, 0.0]);
        // Outside the support: zeros.
        let z = kv(&EX23).basis_i_derivs(0, 2, 2.5, Side::Left, 2).unwrap();
        assert_eq!(z, vec![0.0, 0.0, 0.0]);
        // Left derivative at the domain end.
        let e = kv(&EX23).basis_i_derivs(7, 2, 5.0, Side::Left, 1).unwrap();
        assert_eq!(e, vec![1.0, 2.0]);
    }

    #[test]
    fn basis_i_eval_rejects_excessive_degrees() {
        let k = kv(&EX23);
        for p in [k.last_index(), k.last_index() + 1, usize::MAX] {
            for side in [Side::Left, Side::Right] {
                assert_eq!(
                    k.basis_i_eval(0, p, 2.5, side),
                    Err(SolidError::Nlib {
                        operation: OP_BASIS_I_EVAL,
                        code: NlibCode::Index,
                    })
                );
                assert_eq!(
                    k.basis_i_eval(0, p, -1.0, side),
                    Err(SolidError::Nlib {
                        operation: OP_BASIS_I_EVAL,
                        code: NlibCode::Parameter,
                    })
                );
            }
        }
    }

    #[test]
    fn basis_i_derivs_rejects_excessive_degrees() {
        let k = kv(&EX23);
        for p in [k.last_index(), k.last_index() + 1, usize::MAX] {
            for side in [Side::Left, Side::Right] {
                assert_eq!(
                    k.basis_i_derivs(0, p, 2.5, side, 3),
                    Err(SolidError::Nlib {
                        operation: OP_BASIS_I_DERIVS,
                        code: NlibCode::Index,
                    })
                );
                assert_eq!(
                    k.basis_i_derivs(0, p, -1.0, side, 3),
                    Err(SolidError::Nlib {
                        operation: OP_BASIS_I_DERIVS,
                        code: NlibCode::Parameter,
                    })
                );
            }
        }
    }

    #[test]
    fn basis_i_quadratic_endpoints_and_derivatives() {
        let k = kv(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        assert_eq!(k.basis_i_eval(0, 2, 0.0, Side::Right).unwrap(), 1.0);
        assert_eq!(k.basis_i_eval(2, 2, 1.0, Side::Left).unwrap(), 1.0);
        assert_eq!(
            k.basis_i_derivs(0, 2, 0.0, Side::Right, 3).unwrap(),
            vec![1.0, -2.0, 2.0, 0.0]
        );
        assert_eq!(
            k.basis_i_derivs(2, 2, 1.0, Side::Left, 3).unwrap(),
            vec![1.0, 2.0, 2.0, 0.0]
        );
    }

    #[test]
    fn short_knot_vectors_error_instead_of_panicking() {
        let k = kv(&[0.0, 1.0]);
        for p in 0..4 {
            for side in [Side::Left, Side::Right] {
                for u in [-1.0, 0.0, 0.5, 1.0, 2.0, f64::NAN] {
                    let _ = k.find_span(p, u, side);
                    let _ = k.find_span_and_mult(p, u, side);
                    let _ = k.basis_eval(p, u, side);
                    let _ = k.basis_i_eval(0, p, u, side);
                    let _ = k.basis_i_derivs(0, p, u, side, 3);
                    let _ = k.validate(p);
                    let _ = k.span_count(p);
                }
            }
        }
        assert!(kv(&[0.0, 1.0]).basis_eval(1, 0.5, Side::Left).is_err());
        let big = vec![0.0; 2 * (MAX_DEGREE + 2) + 2];
        assert!(kv(&big)
            .basis_eval(MAX_DEGREE + 2, 0.5, Side::Left)
            .is_err());
    }
}
