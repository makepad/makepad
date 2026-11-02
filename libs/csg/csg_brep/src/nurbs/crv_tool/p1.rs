//! NURBS curve tools, part 1: knot insertion, knot refinement, knot removal and its
//! error bound, splitting, segment extraction, Bezier decomposition and degree
//! elevation of an NLib curve.
//!
//! Upstream: `source/SMLib/src/NL_CrvTool.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvTool.h`).
//!
//! | Rust                              | Upstream                 |
//! |-----------------------------------|--------------------------|
//! | [`Curve::insert_knot`]            | `N_CrvInsertKnot`        |
//! | [`Curve::split`]                  | `N_CrvSplit`             |
//! | [`Curve::decompose_bez`]          | `N_CrvDecomposeBez`      |
//! | [`Curve::refine`]                 | `N_CrvRefine`            |
//! | [`Curve::extract_seg`]            | `N_CrvExtractCrvSeg`     |
//! | [`Curve::remove_knot`], [`KnotRemoval`] | `N_CrvRemoveKnot`  |
//! | [`Curve::remove_knot_max_err`]    | `N_CrvRemoveKnotMaxErr`  |
//! | [`Curve::elevate_degree`]         | `N_CrvElevateDegree`     |
//! | [`REMOVAL_PARAM_TOL`]             | `cto` (file-private)     |
//!
//! Deviations:
//! - Upstream writes into an output curve that is either NULL (allocated), an
//!   existing curve (checked by `N_CrvSizeArrays`, `NL_STO_ERR` when too small) or the
//!   input curve itself (in-place operation). The Rust methods take `&self` and return
//!   a new [`Curve`] sized exactly as upstream sizes a NULL output, so the storage
//!   error path does not exist. In-place and out-of-place upstream calls produce the
//!   same curve.
//! - Out-params become return values; errors are [`SolidError::Nlib`] values with the
//!   upstream `NL_*_ERR` number instead of a flag plus the global `NL_ERROR` record.
//!   The `NL_STACKS` arguments and the memory error paths have no counterpart.
//! - Upstream signed counts (`r` of `N_CrvInsertKnot`, `t` of `N_CrvElevateDegree`,
//!   `nu` of `N_CrvRemoveKnot`) are `usize`, so their negative-value error paths
//!   cannot occur.
//! - Every array access is bounds checked: where upstream would read or write outside
//!   its arrays (malformed curves: knot vectors that do not match the control points or
//!   are not clamped, multiplicities above the degree, out-of-domain parameters of
//!   [`Curve::extract_seg`], wrong indices given to [`Curve::remove_knot_max_err`],
//!   and [`Curve::extract_seg`] over a span that `N_BasisFindSpan` skips as too short:
//!   for `U = [0,0,0,0,1e-300,..]`, `ul = 0` lies in span 4, `spr - mlr < spl`, and
//!   upstream writes one knot past the `m + 1` it allocated for `UQ`),
//!   the methods return [`SolidError::InvalidInput`] instead. For every input upstream
//!   handles without undefined behaviour the results are the same, bit for bit.
//! - `N_CrvDecomposeBez` returns a C array of curves and its highest index; the Rust
//!   method returns a `Vec`. A knot vector without a non-zero-length span (where
//!   upstream allocates an array of `-1` curves) is `InvalidInput`.
//! - A degree above [`MAX_SHORT_DEGREE`](crate::nurbs::bezier::MAX_SHORT_DEGREE)
//!   (impossible upstream, where `NL_DEGREE` is a `short`) is
//!   [`SolidError::InvalidInput`], so index arithmetic and allocations sized from the
//!   degree cannot overflow.

// Index arithmetic follows upstream's signed `NL_INDEX` (`isize` here); every access
// goes through `get`/`set`, which reject negative or too-large indices.

use crate::error::{NlibCode, Result, SolidError};
use crate::nurbs::basis_basic::find_global_max;
use crate::nurbs::bezier::{
    check_short_degree, elevate_degree as bez_elevate_degree, elevation_matrix,
};
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::geom::dist_cpt_cpt_homo;
use crate::nurbs::knots::{KnotVector, Side, MAX_DEGREE};
use crate::nurbs::math::CPoint;
use crate::nurbs::srf_basic::{WEIGHT_MAX, WEIGHT_MIN};

/// Upstream `cto`: relative parameter tolerance of the basis maxima searched by
/// [`Curve::remove_knot`] (scaled by the parameter range).
pub const REMOVAL_PARAM_TOL: f64 = 1.0e-05;

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

fn oob(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "index out of range (malformed curve or arguments)",
    }
}

/// `usize` -> signed index.
fn sx(u: usize, op: &'static str) -> Result<isize> {
    isize::try_from(u).map_err(|_| oob(op))
}

/// Signed index -> `usize`, rejecting negative values.
fn ux(i: isize, op: &'static str) -> Result<usize> {
    usize::try_from(i).map_err(|_| oob(op))
}

/// `a[i]`.
#[inline]
fn get<T: Copy>(a: &[T], i: isize, op: &'static str) -> Result<T> {
    usize::try_from(i)
        .ok()
        .and_then(|i| a.get(i))
        .copied()
        .ok_or_else(|| oob(op))
}

/// `a[i] = v`.
#[inline]
fn set<T>(a: &mut [T], i: isize, v: T, op: &'static str) -> Result<()> {
    let slot = usize::try_from(i)
        .ok()
        .and_then(|i| a.get_mut(i))
        .ok_or_else(|| oob(op))?;
    *slot = v;
    Ok(())
}

/// Zero-filled storage for `hi + 1` entries.
fn storage<T: Clone + Default>(hi: isize, op: &'static str) -> Result<Vec<T>> {
    let len = ux(hi, op)?.checked_add(1).ok_or_else(|| oob(op))?;
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| oob(op))?;
    v.resize(len, T::default());
    Ok(v)
}

/// A curve over whole arrays `qw[0..=n]`, `uq[0..=m]`.
fn curve(qw: Vec<CPoint>, p: usize, uq: Vec<f64>, op: &'static str) -> Result<Curve> {
    Curve::new(qw, p, uq).map_err(|_| oob(op))
}

/// Result of [`Curve::remove_knot`].
#[derive(Debug, Clone, PartialEq)]
pub struct KnotRemoval {
    /// The curve after removal (the input curve's copy when nothing was removed). Its
    /// storage keeps the input's size; the indices in use shrink by [`Self::removed`].
    pub curve: Curve,
    /// How many times the knot was removed (upstream `ru`).
    pub removed: usize,
}

impl Curve {
    /// Inserts the interior knot `u` `r` times (`N_CrvInsertKnot`).
    ///
    /// Errors with [`NlibCode::Parameter`] when `u` is on or outside the ends of the
    /// knot vector or when the multiplicity of `u` plus `r` exceeds the degree.
    pub fn insert_knot(&self, u: f64, r: usize) -> Result<Curve> {
        const OP: &str = "nurbs::crv_tool::p1::Curve::insert_knot";
        let pw = self.cpts();
        let up = self.knots();
        let pu = self.degree();
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let knt = self.knot_vector()?;

        // Check parameter and get knot span.
        if knt.is_end_param(u) {
            return Err(nlib(OP, NlibCode::Parameter));
        }
        let (spn, mlt) = knt.find_span_and_mult(pu, u, Side::Left)?;
        if mlt.checked_add(r).is_none_or(|s| s > pu) {
            return Err(nlib(OP, NlibCode::Parameter));
        }
        let (spn, mlt, r) = (sx(spn, OP)?, sx(mlt, OP)?, sx(r, OP)?);
        let mut k = spn - p;

        let mut qw: Vec<CPoint> = storage(n + r, OP)?;
        let mut uq: Vec<f64> = storage(m + r, OP)?;

        // Get auxiliary control points.
        let mut rw: Vec<CPoint> = storage(p, OP)?;
        for i in 0..=p - mlt {
            set(&mut rw, i, get(pw, k + i, OP)?, OP)?;
        }

        // Save unaltered control points.
        for i in 0..=k {
            set(&mut qw, i, get(pw, i, OP)?, OP)?;
        }
        for i in spn - mlt..=n {
            set(&mut qw, i + r, get(pw, i, OP)?, OP)?;
        }

        // Now insert the knot.
        for i in 1..=r {
            k = spn - p + i;
            for j in 0..=p - i - mlt {
                let alf =
                    (u - get(up, k + j, OP)?) / (get(up, spn + j + 1, OP)? - get(up, k + j, OP)?);
                let oma = 1.0 - alf;
                let c = CPoint::combine2(alf, get(&rw, j + 1, OP)?, oma, get(&rw, j, OP)?);
                set(&mut rw, j, c, OP)?;
            }
            set(&mut qw, k, get(&rw, 0, OP)?, OP)?;
            set(&mut qw, spn + r - i - mlt, get(&rw, p - i - mlt, OP)?, OP)?;
        }

        // Load the remaining control points.
        for i in k + 1..spn - mlt {
            set(&mut qw, i, get(&rw, i - k, OP)?, OP)?;
        }

        // Load the knot vector.
        for i in 0..=spn {
            set(&mut uq, i, get(up, i, OP)?, OP)?;
        }
        for i in 1..=r {
            set(&mut uq, i + spn, u, OP)?;
        }
        for i in spn + 1..=m {
            set(&mut uq, i + r, get(up, i, OP)?, OP)?;
        }

        curve(qw, pu, uq, OP)
    }

    /// Splits the curve at the interior parameter `u` into the halves over
    /// `[U[0], u]` and `[u, U[m]]` (`N_CrvSplit`). A `u` within
    /// `1e-6 * (U[m-p] - U[p])` of a knot `U[p..=m-p]` is moved onto that knot first.
    ///
    /// Errors with [`NlibCode::Parameter`] when (the adjusted) `u` is on or outside the
    /// ends of the knot vector.
    pub fn split(&self, u: f64) -> Result<(Curve, Curve)> {
        const OP: &str = "nurbs::crv_tool::p1::Curve::split";
        let pw = self.cpts();
        let uk = self.knots();
        let pu = self.degree();
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let mut u = u;

        // If u is within PTol of a knot, set u to the knot value.
        let ptol = (get(uk, m - p, OP)? - get(uk, p, OP)?) * 1.0e-6;
        let mut i = p;
        while i <= m - p {
            let ui = get(uk, i, OP)?;
            if (ui - u).abs() < ptol {
                u = ui;
                break;
            }
            i += 1;
        }

        // Check parameter for domain.
        let knt = self.knot_vector()?;
        if knt.is_end_param(u) {
            return Err(nlib(OP, NlibCode::Parameter));
        }

        // Get span and knot multiplicity.
        let (spn, mlt) = knt.find_span_and_mult(pu, u, Side::Left)?;
        let (spn, mlt) = (sx(spn, OP)?, sx(mlt, OP)?);

        let mut lw: Vec<CPoint> = storage(spn - mlt, OP)?;
        let mut ul: Vec<f64> = storage(spn - mlt + p + 1, OP)?;
        let mut rw: Vec<CPoint> = storage(n + p - spn, OP)?;
        let mut ur: Vec<f64> = storage(n - spn + 2 * p + 1, OP)?;

        // Get auxiliary control points.
        let mut sw: Vec<CPoint> = storage(p, OP)?;
        for i in 0..=p - mlt {
            set(&mut sw, i, get(pw, spn - p + i, OP)?, OP)?;
        }

        // Save unaltered control points.
        for i in 0..=spn - p {
            set(&mut lw, i, get(pw, i, OP)?, OP)?;
        }
        for i in spn - mlt..=n {
            set(&mut rw, i + p - spn, get(pw, i, OP)?, OP)?;
        }

        // Now split the curve.
        for i in 1..=p - mlt {
            let k = spn - p + i;
            for j in 0..=p - i - mlt {
                let alf =
                    (u - get(uk, k + j, OP)?) / (get(uk, spn + j + 1, OP)? - get(uk, k + j, OP)?);
                let oma = 1.0 - alf;
                let c = CPoint::combine2(alf, get(&sw, j + 1, OP)?, oma, get(&sw, j, OP)?);
                set(&mut sw, j, c, OP)?;
            }
            set(&mut lw, k, get(&sw, 0, OP)?, OP)?;
            set(&mut rw, p - i - mlt, get(&sw, p - i - mlt, OP)?, OP)?;
        }

        // Load knot vectors.
        let k = spn - mlt;
        for i in 0..=k {
            set(&mut ul, i, get(uk, i, OP)?, OP)?;
        }
        for i in 0..=p {
            set(&mut ul, k + i + 1, u, OP)?;
        }
        let k = spn + 1;
        for i in 0..=p {
            set(&mut ur, i, u, OP)?;
        }
        for i in k..=m {
            set(&mut ur, i - k + p + 1, get(uk, i, OP)?, OP)?;
        }

        Ok((curve(lw, pu, ul, OP)?, curve(rw, pu, ur, OP)?))
    }

    /// Decomposes the curve into its Bezier segments, one per non-zero-length knot
    /// span, each a curve of degree `p` with knots `[a; p+1] ++ [b; p+1]`
    /// (`N_CrvDecomposeBez`).
    ///
    /// Errors with `InvalidInput` for a curve without a non-zero-length span or with
    /// a malformed (e.g. unclamped) knot vector.
    pub fn decompose_bez(&self) -> Result<Vec<Curve>> {
        const OP: &str = "nurbs::crv_tool::p1::Curve::decompose_bez";
        let pw = self.cpts();
        let up = self.knots();
        let pu = self.degree();
        let m = sx(self.m(), OP)?;
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let knt = self.knot_vector()?;

        // Get number of segments and allocate memory.
        let nsp = sx(knt.span_count(pu), OP)?;
        if nsp < 1 {
            return Err(oob(OP));
        }
        let mut segs: Vec<(Vec<CPoint>, Vec<f64>)> = Vec::new();
        for _ in 0..nsp {
            segs.push((storage(p, OP)?, storage(2 * p + 1, OP)?));
        }
        let mut alfs: Vec<f64> = storage(p, OP)?;
        let mut omas: Vec<f64> = storage(p, OP)?;

        // Initialize.
        let mut is = p;
        let mut ie = p + 1;
        let mut iq: isize = -1;

        // Skip over first knot over-multiplicity (bad data correction).
        while ie < m && get(up, ie - 1, OP)? == get(up, ie, OP)? {
            is += 1;
            ie += 1;
        }

        {
            let qw = &mut segs.first_mut().ok_or_else(|| oob(OP))?.0;
            for i in 0..=p {
                set(qw, i, get(pw, i + is - p, OP)?, OP)?;
            }
        }

        // Index of the curve `NQw` points at; upstream keeps the previous one for the
        // last segment.
        let mut next = 0usize;

        // Loop through the knot vector and extract each segment.
        while ie < m {
            iq += 1;
            if iq >= nsp {
                // Upstream NL_QUIT without an error number.
                return Err(oob(OP));
            }
            let iqu = ux(iq, OP)?;
            if iq < nsp - 1 {
                next = iqu + 1;
            }

            // Get knot multiplicity.
            let i0 = ie;
            while ie < m && get(up, ie, OP)? == get(up, ie + 1, OP)? {
                ie += 1;
            }
            let mlt = ie - i0 + 1;
            let mut r = p - mlt;

            // Insert the knot on the rhs only.
            if mlt < p {
                let num = get(up, ie, OP)? - get(up, is, OP)?;
                let mut i = p;
                while i > mlt {
                    let a = num / (get(up, is + i, OP)? - get(up, is, OP)?);
                    set(&mut alfs, i - mlt - 1, a, OP)?;
                    set(&mut omas, i - mlt - 1, 1.0 - a, OP)?;
                    i -= 1;
                }

                for i in 1..=r {
                    let s = mlt + i;
                    let save = r - i;
                    let qw = &mut segs.get_mut(iqu).ok_or_else(|| oob(OP))?.0;
                    let mut j = p;
                    while j >= s {
                        let c = CPoint::combine2(
                            get(&alfs, j - s, OP)?,
                            get(qw, j, OP)?,
                            get(&omas, j - s, OP)?,
                            get(qw, j - 1, OP)?,
                        );
                        set(qw, j, c, OP)?;
                        j -= 1;
                    }

                    // Copy duplicated control points from this curve to the next.
                    if ie < m {
                        let c = get(qw, p, OP)?;
                        let nqw = &mut segs.get_mut(next).ok_or_else(|| oob(OP))?.0;
                        set(nqw, save, c, OP)?;
                    }
                }
            }

            // Build the Bezier segment's knot vector.
            {
                let (a, b) = (get(up, is, OP)?, get(up, ie, OP)?);
                let uq = &mut segs.get_mut(iqu).ok_or_else(|| oob(OP))?.1;
                for i in 0..=p {
                    set(uq, i, a, OP)?;
                    set(uq, i + p + 1, b, OP)?;
                }
            }

            // Copy the remaining control points of the next segment.
            if ie < m {
                if r < 0 {
                    r = 0;
                }
                for i in r..=p {
                    let c = get(pw, ie - p + i, OP)?;
                    let nqw = &mut segs.get_mut(next).ok_or_else(|| oob(OP))?.0;
                    set(nqw, i, c, OP)?;
                }
            }
            is = ie;
            ie += 1;
        }

        segs.into_iter()
            .map(|(qw, uq)| curve(qw, pu, uq, OP))
            .collect()
    }

    /// Refines the curve with the non-decreasing interior knots `x`
    /// (`N_CrvRefine`, algorithm A5.4).
    ///
    /// Errors with [`NlibCode::Input`] for an empty `x` and with
    /// [`NlibCode::Parameter`] when `x[0]` or its last knot is on or outside the ends
    /// of the knot vector.
    pub fn refine(&self, x: &[f64]) -> Result<Curve> {
        const OP: &str = "nurbs::crv_tool::p1::Curve::refine";
        let pw = self.cpts();
        let up = self.knots();
        let pu = self.degree();
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let knt = self.knot_vector()?;
        let r = sx(x.len(), OP)? - 1;

        // Check input parameters.
        if r < 0 {
            return Err(nlib(OP, NlibCode::Input));
        }
        let x0 = get(x, 0, OP)?;
        let xr = get(x, r, OP)?;
        if knt.is_end_param(x0) || knt.is_end_param(xr) {
            return Err(nlib(OP, NlibCode::Parameter));
        }

        let mut qw: Vec<CPoint> = storage(n + r + 1, OP)?;
        let mut uq: Vec<f64> = storage(m + r + 1, OP)?;

        // Find knot spans.
        let a = sx(knt.find_span(pu, x0, Side::Left)?, OP)?;
        let b = sx(knt.find_span(pu, xr, Side::Left)?, OP)? + 1;

        // Initialize output knot vector.
        for j in 0..=a {
            set(&mut uq, j, get(up, j, OP)?, OP)?;
        }
        for j in b + p..=m {
            set(&mut uq, j + r + 1, get(up, j, OP)?, OP)?;
        }

        // Save unaltered control points.
        for j in 0..=a - p {
            set(&mut qw, j, get(pw, j, OP)?, OP)?;
        }
        for j in b - 1..=n {
            set(&mut qw, j + r + 1, get(pw, j, OP)?, OP)?;
        }

        // Now refine the knot vector.
        let mut i = b + p - 1;
        let mut k = b + p + r;
        let mut j = r;
        while j >= 0 {
            let xj = get(x, j, OP)?;
            while xj <= get(up, i, OP)? && i > a {
                set(&mut qw, k - p - 1, get(pw, i - p - 1, OP)?, OP)?;
                set(&mut uq, k, get(up, i, OP)?, OP)?;
                k -= 1;
                i -= 1;
            }

            let v = get(&qw, k - p, OP)?;

            set(&mut qw, k - p - 1, v, OP)?;

            for l in 1..=p {
                let t = k - p + l;
                let mut alf = get(&uq, k + l, OP)? - xj;
                if alf.abs() <= 0.0 {
                    let v = get(&qw, t, OP)?;
                    set(&mut qw, t - 1, v, OP)?;
                } else {
                    alf /= get(&uq, k + l, OP)? - get(up, i - p + l, OP)?;
                    let oma = 1.0 - alf;
                    let c = CPoint::combine2(alf, get(&qw, t - 1, OP)?, oma, get(&qw, t, OP)?);
                    set(&mut qw, t - 1, c, OP)?;
                }
            }
            set(&mut uq, k, xj, OP)?;
            k -= 1;
            j -= 1;
        }

        curve(qw, pu, uq, OP)
    }

    /// Extracts the segment over `[ul, ur]` by knot insertion
    /// (`N_CrvExtractCrvSeg`); `U[0] <= ul < ur <= U[m]` must hold.
    ///
    /// Errors with [`NlibCode::Input`] when `ur <= ul`; `InvalidInput` for parameters
    /// outside the knot range where upstream would index outside its arrays.
    pub fn extract_seg(&self, ul: f64, ur: f64) -> Result<Curve> {
        const OP: &str = "nurbs::crv_tool::p1::Curve::extract_seg";
        let pw = self.cpts();
        let up = self.knots();
        let pu = self.degree();
        let m0 = sx(self.m(), OP)?;
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let knt = self.knot_vector()?;

        // Check parameters.
        if ur <= ul {
            return Err(nlib(OP, NlibCode::Input));
        }

        // Find knot spans and set new indexes.
        let (spl, mll) = knt.find_span_and_mult(pu, ul, Side::Left)?;
        let (spr, mlr) = knt.find_span_and_mult(pu, ur, Side::Left)?;
        let (spl, mll) = (sx(spl, OP)?, sx(mll, OP)?);
        let (mut spr, mut mlr) = (sx(spr, OP)?, sx(mlr, OP)?);

        let is = spl - p;
        if ur == get(up, m0 - p, OP)? {
            spr = m0;
            mlr = p + 1;
        }
        let ie = spr - mlr;

        let n = ie - is;
        let m = spr - spl - mlr + 2 * p + 1;
        if n < 0 {
            return Err(nlib(OP, NlibCode::Input));
        }

        let mut qw: Vec<CPoint> = storage(n, OP)?;
        let mut uq: Vec<f64> = storage(m, OP)?;

        // Get initial control points.
        for i in is..=ie {
            set(&mut qw, i - is, get(pw, i, OP)?, OP)?;
        }

        // Insert the left knot.
        let ll = spl - p;
        for i in 1..=p - mll {
            for j in 0..=p - i - mll {
                let left = get(up, ll + i + j, OP)?;
                let alf = (ul - left) / (get(up, spl + j + 1, OP)? - left);
                let oma = 1.0 - alf;
                let c = CPoint::combine2(alf, get(&qw, j + 1, OP)?, oma, get(&qw, j, OP)?);
                set(&mut qw, j, c, OP)?;
            }
        }

        // Insert the right knot.
        let lr = spr - p;
        let lk = n - p + mlr;
        for i in 1..=p - mlr {
            let mut j = p - i - mlr;
            while j >= 0 {
                let k = lk + i + j;
                let mut left = get(up, lr + i + j, OP)?;
                if left < ul {
                    left = ul;
                }
                let alf = (ur - left) / (get(up, spr + j + 1, OP)? - left);
                let oma = 1.0 - alf;
                let c = CPoint::combine2(alf, get(&qw, k, OP)?, oma, get(&qw, k - 1, OP)?);
                set(&mut qw, k, c, OP)?;
                j -= 1;
            }
        }

        // Load the knot vector.
        let mut j: isize = -1;
        for _ in 0..=p {
            j += 1;
            set(&mut uq, j, ul, OP)?;
        }
        for i in spl + 1..=spr - mlr {
            j += 1;
            set(&mut uq, j, get(up, i, OP)?, OP)?;
        }
        for _ in 0..=p {
            j += 1;
            set(&mut uq, j, ur, OP)?;
        }

        curve(qw, pu, uq, OP)
    }

    /// Removes the interior knot `u` up to `nu` times (at most its multiplicity),
    /// stopping at the first removal that would move the curve by more than `tol`
    /// (`N_CrvRemoveKnot`, algorithm A5.8 with the basis-maximum error bound).
    ///
    /// Errors with [`NlibCode::Parameter`] when `u` is on or outside the ends of the
    /// knot vector. A failing `N_BasisFindGlobalMax` is handled: its maximum is taken
    /// as `1`.
    pub fn remove_knot(&self, u: f64, nu: usize, tol: f64) -> Result<KnotRemoval> {
        const OP: &str = "nurbs::crv_tool::p1::Curve::remove_knot";
        let pw = self.cpts();
        let up = self.knots();
        let pu = self.degree();
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let knp = self.knot_vector()?;
        let rational = self.is_rational();

        // Adjust removal tolerance in case of rational curves.
        let tol = if rational {
            let mm = self.min_max_weights_and_pts();
            (tol * mm.wmin) / (1.0 + mm.pmax)
        } else {
            tol
        };

        let lto = REMOVAL_PARAM_TOL * (get(up, m, OP)? - get(up, 0, OP)?).abs();

        // Don't remove boundary knots.
        if knp.is_end_param(u) {
            return Err(nlib(OP, NlibCode::Parameter));
        }
        let (spn, mlt) = knp.find_span_and_mult(pu, u, Side::Left)?;
        let nu = sx(nu.min(mlt), OP)?;
        let (spn, mlt) = (sx(spn, OP)?, sx(mlt, OP)?);

        let mut qw = pw.to_vec();
        let mut uq = up.to_vec();
        let mut rw: Vec<CPoint> = storage(2 * p, OP)?;

        // A failing `N_BasisFindGlobalMax` is handled: fall back to the bound max == 1.
        let global_max = |uq: &[f64], i: isize| -> Result<f64> {
            let knq = KnotVector::new(uq, ux(m, OP)?)?;
            Ok(match find_global_max(&knq, ux(i, OP)?, pu, lto) {
                Ok((max, _)) => max,
                Err(_) => 1.0,
            })
        };

        // Remove the knot nu times.
        let fout = (2 * spn - mlt - p) / 2;
        let mut first = spn - p;
        let mut last = spn - mlt;

        let mut k: isize = 0;
        while k < nu {
            let off = first - 1;
            let mut i = first;
            let mut j = last;
            let mut ii: isize = 1;
            let mut jj = last - off;

            set(&mut rw, 0, get(&qw, off, OP)?, OP)?;
            set(&mut rw, last + 1 - off, get(&qw, last + 1, OP)?, OP)?;

            // Get new control points for one removal step.
            let mut alf = 0.5;
            let mut bet = 0.5;
            while j - i > k {
                alf = (get(&uq, i + p + 1, OP)? - get(&uq, i, OP)?) / (u - get(&uq, i, OP)?);
                let oma = 1.0 - alf;
                bet = (get(&uq, j + p - k + 1, OP)? - get(&uq, j - k, OP)?)
                    / (get(&uq, j + p - k + 1, OP)? - u);
                let omb = 1.0 - bet;
                let c = CPoint::combine2(alf, get(&qw, i, OP)?, oma, get(&rw, ii - 1, OP)?);
                set(&mut rw, ii, c, OP)?;
                let c = CPoint::combine2(bet, get(&qw, j, OP)?, omb, get(&rw, jj + 1, OP)?);
                set(&mut rw, jj, c, OP)?;
                i += 1;
                j -= 1;
                ii += 1;
                jj -= 1;
            }

            // Check if the knot is removable.
            let del = (u - get(&uq, i, OP)?) / (get(&uq, i + p + 1, OP)? - get(&uq, i, OP)?);
            let omd = 1.0 - del;

            if j - i < k {
                let lam = alf / (alf + bet);
                let oml = 1.0 - lam;
                let dw = dist_cpt_cpt_homo(get(&rw, ii - 1, OP)?, get(&rw, jj + 1, OP)?);
                let maxl = global_max(&uq, i - 1)?;
                let maxr = global_max(&uq, i)?;
                let max = (lam * alf * maxl).max(oml * omd * maxr);
                if max * dw > tol {
                    break;
                }
                let c = CPoint::combine2(lam, get(&rw, jj + 1, OP)?, oml, get(&rw, ii - 1, OP)?);
                set(&mut rw, jj + 1, c, OP)?;
            } else {
                let a = CPoint::combine2(del, get(&rw, jj + 1, OP)?, omd, get(&rw, ii - 1, OP)?);
                let dw = dist_cpt_cpt_homo(get(&qw, i, OP)?, a);
                let max = global_max(&uq, i)?;
                if max * dw > tol {
                    break;
                }
            }

            // Check for disallowed weights.
            if rational {
                i = first;
                j = last;
                let mut wmin = f64::MAX;
                let mut wmax = f64::MIN_POSITIVE;
                while j - i > k {
                    let wi = get(&rw, i - off, OP)?.w;
                    let wj = get(&rw, j - off, OP)?.w;
                    if wi < wmin {
                        wmin = wi;
                    }
                    if wj < wmin {
                        wmin = wj;
                    }
                    if wi > wmax {
                        wmax = wi;
                    }
                    if wj > wmax {
                        wmax = wj;
                    }
                    i += 1;
                    j -= 1;
                }
                if wmin < WEIGHT_MIN || wmax > WEIGHT_MAX {
                    break;
                }
            }

            // Save new control points.
            i = first;
            j = last;
            while j - i > k {
                set(&mut qw, i, get(&rw, i - off, OP)?, OP)?;
                set(&mut qw, j, get(&rw, j - off, OP)?, OP)?;
                i += 1;
                j -= 1;
            }

            // Fill the hole in the knot array by shifting down the higher knots.
            for l in spn - k..m - k {
                let v = get(&uq, l + 1, OP)?;
                set(&mut uq, l, v, OP)?;
            }

            first -= 1;
            last += 1;
            k += 1;
        }

        // If no knot was removed --> out.
        if k == 0 {
            return Ok(KnotRemoval {
                curve: curve(qw, pu, uq, OP)?,
                removed: 0,
            });
        }

        // Shift down control points.
        let mut j = fout;
        let mut i = j;
        for l in 1..k {
            if l % 2 != 0 {
                i += 1;
            } else {
                j -= 1;
            }
        }
        for l in i + 1..=n {
            let v = get(&qw, l, OP)?;
            set(&mut qw, j, v, OP)?;
            j += 1;
        }

        let mut out = curve(qw, pu, uq, OP)?;
        out.set_size_indices(ux(n - k, OP)?, pu, ux(m - k, OP)?)?;
        Ok(KnotRemoval {
            curve: out,
            removed: ux(k, OP)?,
        })
    }

    /// The error bound of removing the knot `U[r]` (multiplicity `s`, `U[r] != U[r+1]`)
    /// once (`N_CrvRemoveKnotMaxErr`). The index and multiplicity are not checked, as
    /// upstream, except that indices outside the arrays are `InvalidInput`.
    pub fn remove_knot_max_err(&self, r: usize, s: usize) -> Result<f64> {
        Self::remove_knot_max_err_of(self.cpts(), self.knots(), self.degree(), r, s)
    }

    /// [`Curve::remove_knot_max_err`] of the curve with control points `pw`
    /// (`Pw[0..=n]`), knots `uk` (`U[0..=m]`) and degree `p`, without building it.
    pub(crate) fn remove_knot_max_err_of(
        pw: &[CPoint],
        uk: &[f64],
        p: usize,
        r: usize,
        s: usize,
    ) -> Result<f64> {
        const OP: &str = "nurbs::crv_tool::p1::Curve::remove_knot_max_err";
        check_short_degree(p, OP)?;
        let p = sx(p, OP)?;
        let r = sx(r, OP)?;
        let s = sx(s, OP)?;

        let mut rw: Vec<CPoint> = storage(2 * p, OP)?;

        // Compute removal error.
        let mut i = r - p;
        let mut j = r - s;
        let mut ii: isize = 1;
        let mut jj = p - s + 1;

        set(&mut rw, 0, get(pw, r - p - 1, OP)?, OP)?;
        set(&mut rw, p - s + 2, get(pw, r - s + 1, OP)?, OP)?;

        // Get new control points for one removal step.
        let ur = get(uk, r, OP)?;
        while j - i > 0 {
            let alf = (get(uk, i + p + 1, OP)? - get(uk, i, OP)?) / (ur - get(uk, i, OP)?);
            let oma = 1.0 - alf;
            let bet = (get(uk, j + p + 1, OP)? - get(uk, j, OP)?) / (get(uk, j + p + 1, OP)? - ur);
            let omb = 1.0 - bet;
            let c = CPoint::combine2(alf, get(pw, i, OP)?, oma, get(&rw, ii - 1, OP)?);
            set(&mut rw, ii, c, OP)?;
            let c = CPoint::combine2(bet, get(pw, j, OP)?, omb, get(&rw, jj + 1, OP)?);
            set(&mut rw, jj, c, OP)?;
            i += 1;
            j -= 1;
            ii += 1;
            jj -= 1;
        }

        // Compute error bound.
        if j - i < 0 {
            Ok(dist_cpt_cpt_homo(
                get(&rw, ii - 1, OP)?,
                get(&rw, jj + 1, OP)?,
            ))
        } else {
            let alf = (ur - get(uk, i, OP)?) / (get(uk, i + p + 1, OP)? - get(uk, i, OP)?);
            let oma = 1.0 - alf;
            let a = CPoint::combine2(alf, get(&rw, ii + 1, OP)?, oma, get(&rw, ii - 1, OP)?);
            Ok(dist_cpt_cpt_homo(get(pw, i, OP)?, a))
        }
    }

    /// Elevates the degree by `t` (`N_CrvElevateDegree`, algorithm A5.9): Bezier
    /// segments are extracted, elevated and the knots between them removed again.
    ///
    /// Errors with [`NlibCode::Degree`] when `p + t > MAX_DEGREE`, with
    /// [`NlibCode::Knot`] for an invalid knot vector ([`KnotVector::validate`]), and
    /// with the errors of the Bezier elevation.
    pub fn elevate_degree(&self, t: usize) -> Result<Curve> {
        const OP: &str = "nurbs::crv_tool::p1::Curve::elevate_degree";
        let pw = self.cpts();
        let up = self.knots();
        let pu = self.degree();
        let np = sx(self.n(), OP)?;
        let mp = sx(self.m(), OP)?;
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let knt = self.knot_vector()?;

        // Check error.
        if pu.checked_add(t).is_none_or(|q| q > MAX_DEGREE) {
            return Err(nlib(OP, NlibCode::Degree));
        }
        knt.validate(pu)?;

        let s = sx(knt.span_count(pu), OP)?;
        let ts = sx(t, OP)?;
        let nq = np + ts * s;
        let qu = pu + t;
        let q = sx(qu, OP)?;
        let mq = mp + ts * (s + 1);

        let mut qw: Vec<CPoint> = storage(nq, OP)?;
        let mut uq: Vec<f64> = storage(mq, OP)?;

        // See if elevation is required.
        if t == 0 {
            for i in 0..=np {
                set(&mut qw, i, get(pw, i, OP)?, OP)?;
            }
            for i in 0..=mp {
                set(&mut uq, i, get(up, i, OP)?, OP)?;
            }
            return curve(qw, qu, uq, OP);
        }

        let mut bw: Vec<CPoint> = storage(p, OP)?;
        let mut nw: Vec<CPoint> = storage(p, OP)?;
        let mut dw: Vec<CPoint> = storage(q, OP)?;
        let mut alfs: Vec<f64> = storage(p, OP)?;
        let mut omas: Vec<f64> = storage(p, OP)?;

        // Initialize.
        let mut a = p;
        let mut b = p + 1;
        let mut r: isize = -1;
        let mut cind: isize = 1;
        let mut kind = q + 1;

        for i in 0..=q {
            set(&mut uq, i, get(up, a, OP)?, OP)?;
        }
        set(&mut qw, 0, get(pw, 0, OP)?, OP)?;
        for i in 0..=p {
            set(&mut bw, i, get(pw, i, OP)?, OP)?;
        }

        // Get degree elevation matrix.
        let dm = elevation_matrix(pu, t)?;

        // Loop through the knot vector: extract the i-th Bezier segment, degree
        // elevate it, remove the knot between the i-th and the (i-1)-th segment.
        while b < mp {
            // Get multiplicity of the knot.
            let i0 = b;
            while b < mp && get(up, b, OP)? == get(up, b + 1, OP)? {
                b += 1;
            }
            let mlt = b - i0 + 1;

            // Insert knot to get Bezier segment.
            let oldr = r;
            r = p - mlt;
            let lbz = if oldr > 0 { (oldr + 2) / 2 } else { 1 };
            let rbz = if r > 0 { q - (r + 1) / 2 } else { q };

            if r > 0 {
                // Save the alphas.
                let num = get(up, b, OP)? - get(up, a, OP)?;
                let mut k = p;
                while k > mlt {
                    let al = num / (get(up, a + k, OP)? - get(up, a, OP)?);
                    set(&mut alfs, k - mlt - 1, al, OP)?;
                    set(&mut omas, k - mlt - 1, 1.0 - al, OP)?;
                    k -= 1;
                }

                // Now insert the knot.
                for j in 1..=r {
                    let save = r - j;
                    let s = mlt + j;
                    let mut k = p;
                    while k >= s {
                        let c = CPoint::combine2(
                            get(&alfs, k - s, OP)?,
                            get(&bw, k, OP)?,
                            get(&omas, k - s, OP)?,
                            get(&bw, k - 1, OP)?,
                        );
                        set(&mut bw, k, c, OP)?;
                        k -= 1;
                    }
                    set(&mut nw, save, get(&bw, p, OP)?, OP)?;
                }
            }

            // Now degree elevate the Bezier segment.
            bez_elevate_degree(&bw, pu, t, &dm, ux(lbz, OP)?, qu, &mut dw)?;

            // Remove the knot UP[a].
            if oldr > 1 {
                let mut first = kind - 2;
                let den = get(up, b, OP)? - get(up, a, OP)?;
                let bet = (get(up, b, OP)? - get(&uq, kind - 1, OP)?) / den;
                let omb = 1.0 - bet;

                for (k, last) in (1..oldr).zip(kind..) {
                    let mut i = first;
                    let mut j = last;
                    let mut l = j - kind + 1;
                    while j - i > k {
                        if i < cind {
                            let alf = (get(up, b, OP)? - get(&uq, i, OP)?)
                                / (get(up, a, OP)? - get(&uq, i, OP)?);
                            let oma = 1.0 - alf;
                            let c =
                                CPoint::combine2(alf, get(&qw, i, OP)?, oma, get(&qw, i - 1, OP)?);
                            set(&mut qw, i, c, OP)?;
                        }
                        if j >= lbz {
                            if j - k <= kind - q + oldr {
                                let gam = (get(up, b, OP)? - get(&uq, j - k, OP)?) / den;
                                let omg = 1.0 - gam;
                                let c = CPoint::combine2(
                                    gam,
                                    get(&dw, l, OP)?,
                                    omg,
                                    get(&dw, l + 1, OP)?,
                                );
                                set(&mut dw, l, c, OP)?;
                            } else {
                                let c = CPoint::combine2(
                                    bet,
                                    get(&dw, l, OP)?,
                                    omb,
                                    get(&dw, l + 1, OP)?,
                                );
                                set(&mut dw, l, c, OP)?;
                            }
                        }
                        i += 1;
                        j -= 1;
                        l -= 1;
                    }
                    first -= 1;
                }
            }

            // Load knot vector and control points.
            if a != p {
                for _ in 0..q - oldr {
                    set(&mut uq, kind, get(up, a, OP)?, OP)?;
                    kind += 1;
                }
            }
            for i in lbz..=rbz {
                set(&mut qw, cind, get(&dw, i, OP)?, OP)?;
                cind += 1;
            }

            // Initialize for next pass through.
            if b < mp {
                for i in 0..r {
                    set(&mut bw, i, get(&nw, i, OP)?, OP)?;
                }
                for i in r..=p {
                    set(&mut bw, i, get(pw, b - p + i, OP)?, OP)?;
                }
                a = b;
                b += 1;
            } else {
                // Get the end knot.
                for i in 0..=q {
                    set(&mut uq, kind + i, get(up, b, OP)?, OP)?;
                }
            }
        }

        curve(qw, qu, uq, OP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;

    fn pt(x: f64, y: f64, z: f64) -> CPoint {
        CPoint::new(x, y, z, NOW)
    }

    /// Cubic with one interior knot at 0.5.
    fn cubic() -> Curve {
        let pw = vec![
            pt(0.0, 0.0, 0.0),
            pt(1.0, 2.0, 0.0),
            pt(2.0, -1.0, 1.0),
            pt(3.0, 3.0, 0.5),
            pt(4.0, 0.0, 2.0),
        ];
        let u = vec![0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0];
        Curve::new(pw, 3, u).unwrap()
    }

    #[test]
    fn insert_knot_adds_points_and_knots() {
        let c = cubic();
        let q = c.insert_knot(0.25, 2).unwrap();
        assert_eq!(q.n(), 6);
        assert_eq!(q.m(), 10);
        assert_eq!(
            q.knots(),
            &[0.0, 0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 1.0, 1.0, 1.0, 1.0]
        );
        assert_eq!(q.cpts()[0], c.cpts()[0]);
        assert_eq!(q.cpts()[6], c.cpts()[4]);
    }

    #[test]
    fn insert_knot_errors() {
        let c = cubic();
        let par = |e: SolidError| {
            matches!(
                e,
                SolidError::Nlib {
                    code: NlibCode::Parameter,
                    ..
                }
            )
        };
        assert!(par(c.insert_knot(0.0, 1).unwrap_err()));
        assert!(par(c.insert_knot(1.0, 1).unwrap_err()));
        assert!(par(c.insert_knot(0.5, 3).unwrap_err()));
        assert!(par(c.insert_knot(0.3, 4).unwrap_err()));
    }

    #[test]
    fn insert_then_remove_round_trips() {
        let c = cubic();
        let q = c.insert_knot(0.25, 1).unwrap();
        let r = q.remove_knot(0.25, 1, 1e-9).unwrap();
        assert_eq!(r.removed, 1);
        assert_eq!(r.curve.n(), c.n());
        assert_eq!(r.curve.knots(), c.knots());
        for (a, b) in r.curve.cpts().iter().zip(c.cpts()) {
            assert!((a.x - b.x).abs() < 1e-12 && (a.y - b.y).abs() < 1e-12);
        }
    }

    #[test]
    fn remove_knot_respects_tolerance() {
        let c = cubic();
        let r = c.remove_knot(0.5, 1, 1e-12).unwrap();
        assert_eq!(r.removed, 0);
        assert_eq!(r.curve, c);
        let r = c.remove_knot(0.5, 1, 1e6).unwrap();
        assert_eq!(r.removed, 1);
        assert_eq!(r.curve.n(), 3);
        assert_eq!(r.curve.knots(), &[0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn remove_knot_rational_unit_weights() {
        let plain = cubic();
        let mut c = cubic();
        for p in c.cpts_mut() {
            p.w = 1.0;
        }
        // Unit weights: the tolerance is unchanged (wmin = 1, pmax = 0).
        let r = c.remove_knot(0.5, 1, 1.0).unwrap();
        assert_eq!(r.removed, plain.remove_knot(0.5, 1, 1.0).unwrap().removed);
    }

    #[test]
    fn split_and_extract_agree() {
        let c = cubic();
        let (l, r) = c.split(0.3).unwrap();
        assert_eq!(l.knots(), &[0.0, 0.0, 0.0, 0.0, 0.3, 0.3, 0.3, 0.3]);
        assert_eq!(r.knots(), &[0.3, 0.3, 0.3, 0.3, 0.5, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(l.cpts().last(), r.cpts().first());
        let e = c.extract_seg(0.0, 0.3).unwrap();
        assert_eq!(e.knots(), l.knots());
        for (a, b) in e.cpts().iter().zip(l.cpts()) {
            assert!((a.x - b.x).abs() < 1e-12 && (a.y - b.y).abs() < 1e-12);
        }
        // Snapped onto the interior knot.
        let (l, _) = c.split(0.5 + 1e-9).unwrap();
        assert_eq!(l.knots().last(), Some(&0.5));
        assert!(c.split(1.0).is_err());
        assert!(matches!(
            c.extract_seg(0.5, 0.5),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn decompose_bez_gives_one_segment_per_span() {
        let c = cubic();
        let segs = c.decompose_bez().unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].knots(), &[0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5]);
        assert_eq!(segs[1].knots(), &[0.5, 0.5, 0.5, 0.5, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(segs[0].cpts().last(), segs[1].cpts().first());
        assert_eq!(segs[1].cpts().last(), c.cpts().last());
    }

    #[test]
    fn refine_matches_repeated_insertion() {
        let c = cubic();
        let a = c.refine(&[0.2, 0.7]).unwrap();
        let b = c.insert_knot(0.2, 1).unwrap().insert_knot(0.7, 1).unwrap();
        assert_eq!(a.knots(), b.knots());
        for (x, y) in a.cpts().iter().zip(b.cpts()) {
            assert!((x.x - y.x).abs() < 1e-12 && (x.y - y.y).abs() < 1e-12);
        }
        assert!(matches!(
            c.refine(&[]),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn elevate_degree_counts() {
        let c = cubic();
        let e = c.elevate_degree(1).unwrap();
        assert_eq!(e.degree(), 4);
        assert_eq!(e.n(), 6);
        assert_eq!(
            e.knots(),
            &[0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0, 1.0, 1.0]
        );
        assert_eq!(e.cpts().first(), c.cpts().first());
        assert_eq!(e.cpts().last(), c.cpts().last());
        assert_eq!(c.elevate_degree(0).unwrap(), c);
        assert!(matches!(
            c.elevate_degree(15),
            Err(SolidError::Nlib {
                code: NlibCode::Degree,
                ..
            })
        ));
    }

    #[test]
    fn remove_knot_max_err_zero_for_inserted_knot() {
        let c = cubic().insert_knot(0.25, 1).unwrap();
        let br = c.remove_knot_max_err(4, 1).unwrap();
        assert!(br < 1e-12);
        let br = cubic().remove_knot_max_err(4, 1).unwrap();
        assert!(br > 1e-3);
    }

    /// Fuzz regression: a huge degree overflowed `2 * p` in `remove_knot_max_err`.
    #[test]
    fn huge_degree_is_an_error() {
        let c = cubic();
        for p in [1usize << 31, i64::MAX as usize, usize::MAX] {
            let h = Curve::new(c.cpts().to_vec(), p, c.knots().to_vec()).unwrap();
            let r = h.remove_knot_max_err(4, 1);
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
            let r = h.insert_knot(0.3, 1);
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        }
    }

    /// Fuzz regression (`nlib.srf_adv_p6` sweep): a `1e-300` first interior knot makes
    /// `extract_seg(0, 1e-300)` need one knot more than upstream allocates; the port
    /// reports `InvalidInput` (upstream writes past `UQ`).
    #[test]
    fn extract_seg_over_skipped_tiny_span_is_invalid_input() {
        let pw: Vec<CPoint> = (0..6)
            .map(|i| CPoint::new(1.5 * f64::from(i), 0.0, 0.0, NOW))
            .collect();
        let u = vec![0.0, 0.0, 0.0, 0.0, 1e-300, 6.0, 9.0, 9.0, 9.0, 9.0];
        let c = Curve::new(pw, 3, u).unwrap();
        let r = c.extract_seg(0.0, 1e-300);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
    }
}
