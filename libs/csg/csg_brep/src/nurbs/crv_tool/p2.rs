//! NURBS curve tools, part 2: removal of all removable knots, degree reduction,
//! compatibility of curve sets (common degree and knot vector) and removal of
//! degenerate segments of an NLib curve.
//!
//! Upstream: `source/SMLib/src/NL_CrvTool.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvTool.h`).
//!
//! | Rust                                         | Upstream                        |
//! |----------------------------------------------|---------------------------------|
//! | [`Curve::remove_knots`]                      | `N_CrvRemoveKnots`              |
//! | [`Curve::reduce_degree_once`], [`DegreeReduction`] | `N_CrvReduceDegreeOnce`   |
//! | [`Curve::reduce_degree`]                     | `N_CrvReduceDegree`             |
//! | [`make_compatible_knot_tol`]                 | `N_CrvsMakeCompatibleKnotTol`   |
//! | [`make_compatible`]                          | `N_CrvsMakeCompatible`          |
//! | [`Curve::remove_degen_segs`], [`DegenSegRemoval`] | `N_CrvRemoveDegenSegs`     |
//! | [`make_compatible_adj_knots`]                | `N_CrvsMakeCompatibleAdjKnots`  |
//! | [`NOT_REMOVABLE`]                            | `NOREM` (file-private)          |
//!
//! Deviations:
//! - Upstream writes into an output curve that is either NULL (allocated), an
//!   existing curve or the input curve itself (in-place operation). The Rust methods
//!   take `&self` and return a new [`Curve`]; in-place and out-of-place upstream calls
//!   produce the same curve. The curve-set routines work in place on a `&mut [Curve]`
//!   (upstream: an array of curve pointers and its highest index `k`), so an error
//!   leaves the curves modified as far as upstream got.
//! - Out-params become return values; errors are [`SolidError::Nlib`] values with the
//!   upstream `NL_*_ERR` number instead of a flag plus the global `NL_ERROR` record.
//!   The `NL_STACKS` arguments and the memory error paths have no counterpart.
//! - `N_CrvRemoveKnots` and `N_CrvReduceDegreeOnce` keep going when
//!   `N_BasisFindAllSpanMaxima` fails (they substitute bounds of `0` and `1`), but return
//!   the error flag of the last call that set it. The Rust methods then return only
//!   that error, not the curve.
//! - `N_CrvReduceDegreeOnce` leaves `mtol` unwritten for degree `p <= 1`;
//!   [`DegreeReduction::max_err`] is `0.0` then.
//! - `N_CrvsMakeCompatible*` with an empty set (`k = -1`) read `cur[0]`; the Rust
//!   functions return `InvalidInput`.
//! - Every array access is bounds checked: where upstream would read or write outside
//!   its arrays (malformed curves), the routines return [`SolidError::InvalidInput`]
//!   instead. `InvalidInput` from a callee is never taken for an upstream error flag.
//!   For every input upstream handles without undefined behaviour the results are the
//!   same, bit for bit.
//! - `N_CrvRemoveDegenSegs` ignores the error flags of its `N_BasisFindSpan*` calls
//!   (they fail only where upstream would index outside the knots); the port
//!   propagates them.
//! - A degree above [`MAX_SHORT_DEGREE`](crate::nurbs::bezier::MAX_SHORT_DEGREE)
//!   (impossible upstream, where `NL_DEGREE` is a `short`) is
//!   [`SolidError::InvalidInput`], so index arithmetic and allocations sized from the
//!   degree cannot overflow.

// Index arithmetic follows upstream's signed `NL_INDEX` (`isize` here); every access
// goes through `get`/`set`, which reject negative or too-large indices.

use super::p1::REMOVAL_PARAM_TOL;
use crate::error::{NlibCode, Result, SolidError};
use crate::math::max2;
use crate::nurbs::basis_basic::{
    compatible_knot_array, compatible_knot_vectors_to_tol, find_all_span_maxima, SpanMaxima,
    COMPATIBLE_KNOT_TOL,
};
use crate::nurbs::bezier::{
    check_short_degree, degree_reduce_coefs, reduce_degree as bez_reduce_degree,
};
use crate::nurbs::crv_basic::{make_rat_compatible, Curve};
use crate::nurbs::geom::{dist_cpt_cpt_homo, Interval};
use crate::nurbs::index::{get, oob, set, storage, sx, ux};
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{CPoint, BIGD};
use crate::nurbs::srf_basic::{WEIGHT_MAX, WEIGHT_MIN};

/// Upstream `NOREM`: removal error bound marking a knot that cannot be removed.
pub const NOT_REMOVABLE: f64 = 1.0e+25;

/// Upstream `NL_SMAD` (`DBL_MIN`).
const SMAD: f64 = f64::MIN_POSITIVE;

/// Upstream `NL_UNITSPAN`.
const UNIT_SPAN: Interval = Interval::new(0.0, 1.0);

/// Resizes `v` to hold `hi + 1` entries (upstream `N_Realloc1d*Array`; new entries
/// are zero).
fn realloc<T: Clone + Default>(v: &mut Vec<T>, hi: isize, op: &'static str) -> Result<()> {
    let len = ux(hi, op)?.checked_add(1).ok_or_else(|| oob(op))?;
    v.resize(len, T::default());
    Ok(())
}

/// A curve over whole arrays `qw[0..=n]`, `uq[0..=m]`.
fn curve(qw: Vec<CPoint>, p: usize, uq: Vec<f64>, op: &'static str) -> Result<Curve> {
    Curve::new(qw, p, uq).map_err(|_| oob(op))
}

/// `N_BasisFindAllSpanMaxima(knt, i, p, tol, ...)`: `Ok(Ok(extrema))`, or `Ok(Err(e))`
/// when upstream's error flag would be set (the caller substitutes its defaults and
/// records `e`). Index errors where upstream would read outside its arrays propagate.
fn span_maxima(
    knt: &KnotVector<'_>,
    i: isize,
    p: usize,
    tol: f64,
    op: &'static str,
) -> Result<std::result::Result<SpanMaxima, SolidError>> {
    match find_all_span_maxima(knt, ux(i, op)?, p, tol) {
        Ok(sm) => Ok(Ok(sm)),
        Err(e @ SolidError::Nlib { .. }) => Ok(Err(e)),
        Err(e) => Err(e),
    }
}

/// `(minimum, maximum)` arrays (`p + 1` entries) of [`span_maxima`], or the defaults
/// `0` / `1` when it failed; records the error flag in `error`.
fn min_max_or_default(
    knt: &KnotVector<'_>,
    i: isize,
    p: usize,
    tol: f64,
    error: &mut Option<SolidError>,
    op: &'static str,
) -> Result<(Vec<f64>, Vec<f64>)> {
    Ok(match span_maxima(knt, i, p, tol, op)? {
        Ok(sm) => {
            *error = None;
            (sm.min, sm.max)
        }
        Err(e) => {
            *error = Some(e);
            (vec![0.0; p + 1], vec![1.0; p + 1])
        }
    })
}

/// Bounds of the knot removal error for an odd `p + s` (shared by
/// `N_CrvRemoveKnots` and `N_CrvReduceDegreeOnce`): `max[0..=p+1]` from the scaled
/// extrema of the two neighbouring basis functions. `abs` applies `fabs` to the end
/// values (only `N_CrvReduceDegreeOnce` does).
fn combine_bounds(
    max: &mut [f64],
    (mut minl, mut maxl): (Vec<f64>, Vec<f64>),
    (mut minr, mut maxr): (Vec<f64>, Vec<f64>),
    lam: f64,
    alf: f64,
    oml: f64,
    omb: f64,
    p: isize,
    abs: bool,
    op: &'static str,
) -> Result<()> {
    let first = lam * alf * get(&maxl, 0, op)?;
    set(max, 0, if abs { first.abs() } else { first }, op)?;
    for i in 1..=p {
        let v = get(&minl, i, op)? * (lam * alf);
        set(&mut minl, i, v, op)?;
        let v = get(&minr, i - 1, op)? * (oml * omb);
        set(&mut minr, i - 1, v, op)?;
        let v = get(&maxl, i, op)? * (lam * alf);
        set(&mut maxl, i, v, op)?;
        let v = get(&maxr, i - 1, op)? * (oml * omb);
        set(&mut maxr, i - 1, v, op)?;

        let a = (get(&maxl, i, op)? - get(&minr, i - 1, op)?).abs();
        let b = (get(&maxr, i - 1, op)? - get(&minl, i, op)?).abs();
        set(max, i, max2(a, b), op)?;
    }
    let last = oml * omb * get(&maxr, p, op)?;
    set(max, p + 1, if abs { last.abs() } else { last }, op)
}

/// Result of [`Curve::reduce_degree_once`].
#[derive(Debug, Clone, PartialEq)]
pub struct DegreeReduction {
    /// The curve of degree `p - 1` when the reduction succeeded within the tolerance
    /// (upstream `rfl = NL_YES`), else `None` (upstream frees the output curve, or
    /// restores the input of an in-place call).
    pub curve: Option<Curve>,
    /// Largest accumulated error bound over the knot spans processed (upstream
    /// `mtol`); `0.0` for degree `p <= 1`.
    pub max_err: f64,
}

/// Result of [`Curve::remove_degen_segs`].
#[derive(Debug, Clone, PartialEq)]
pub struct DegenSegRemoval {
    /// The curve without the degenerate segments.
    pub curve: Curve,
    /// Number of degenerate segments found (upstream `nr`), at least `1`.
    pub removed: usize,
}

impl Curve {
    /// Removes every removable knot, smallest error bound first, while the
    /// accumulated deviation stays within `tol` (`N_CrvRemoveKnots`, with error bounds
    /// from the extrema of the basis functions). The storage of the result
    /// is compressed when a knot was removed.
    ///
    /// Errors with the error of `N_BasisFindAllSpanMaxima` when its last failing call
    /// was the last one to set upstream's error flag (see the module deviations).
    pub fn remove_knots(&self, tol: f64) -> Result<Curve> {
        const OP: &str = "nurbs::crv_tool::p2::Curve::remove_knots";
        self.remove_knots_constrained(tol, |_, _| false, OP)
    }

    /// The body of `N_CrvRemoveKnots`, shared with `N_CrvRemoveKnotsDerivConstraints`
    /// (`nurbs::crv_adv::p2`), which is the same routine plus a constraint check:
    /// `blocked(r, n)` is called with the index `r` of the knot with the smallest
    /// error bound and the current highest control point index `n`; when it returns
    /// `true` the knot is marked not removable before its error is checked.
    pub(crate) fn remove_knots_constrained(
        &self,
        tol: f64,
        blocked: impl Fn(isize, isize) -> bool,
        op: &'static str,
    ) -> Result<Curve> {
        let pu = self.degree();
        check_short_degree(pu, op)?;
        let p = sx(pu, op)?;
        let mut n = sx(self.n(), op)?;
        let mut m = sx(self.m(), op)?;
        let ns = n;
        let up = self.knots();
        let rational = self.is_rational();

        // Adjust removal tolerance in case of rational curves.
        let tol = if rational {
            let mm = self.min_max_weights_and_pts();
            (tol * mm.wmin) / (1.0 + mm.pmax)
        } else {
            tol
        };

        let lto = REMOVAL_PARAM_TOL * (get(up, m, op)? - get(up, 0, op)?).abs();

        // Work on a copy of the curve.
        let mut qw = self.cpts().to_vec();
        let mut uq = self.knots().to_vec();

        // Get local memory.
        let mut rw: Vec<CPoint> = storage(2 * p, op)?;
        let mut br: Vec<f64> = storage(m, op)?;
        let mut sr: Vec<isize> = storage(m, op)?;
        let mut er: Vec<f64> = storage(m, op)?;
        let mut te: Vec<f64> = storage(m, op)?;
        let mut max: Vec<f64> = storage(p + 1, op)?;

        // Initialize.
        br.fill(BIGD);

        // The error flag of the last call that set it.
        let mut error: Option<SolidError> = None;

        // The removal error bound of `U[r]` (multiplicity `s`) on the current curve.
        // (`Curve::with_indices` on copies of `qw`, `uq`, then `remove_knot_max_err`; the
        // same size check and the same `Pw[0..=n]`, `U[0..=m]`, without the copies)
        let max_err = |qw: &[CPoint], uq: &[f64], n: isize, m: isize, r: isize, s: isize| {
            let (n, m) = (ux(n, op)?, ux(m, op)?);
            let (Some(pw), Some(uk)) = (qw.get(..=n), uq.get(..=m)) else {
                return Err(SolidError::InvalidInput {
                    operation: "nurbs::crv_basic::Curve::with_indices",
                    reason: "storage smaller than (n, m)",
                });
            };
            Curve::remove_knot_max_err_of(pw, uk, pu, ux(r, op)?, ux(s, op)?)
        };

        // Compute knot removal errors for each distinct knot.
        let mut r = p + 1;
        while r <= n {
            let i = r;
            while r <= n && get(&uq, r, op)? == get(&uq, r + 1, op)? {
                r += 1;
            }
            set(&mut sr, r, r - i + 1, op)?;
            let b = self.remove_knot_max_err(ux(r, op)?, ux(r - i + 1, op)?)?;
            set(&mut br, r, b, op)?;
            error = None;
            r += 1;
        }

        let mut lam = 0.0;
        let mut oml = 0.0;

        // Try to remove each knot.
        loop {
            // Find knot with smallest error.
            let mut b = get(&br, p + 1, op)?;
            let mut s = get(&sr, p + 1, op)?;
            r = p + 1;
            for i in (p + 2)..(m - p) {
                let bi = get(&br, i, op)?;
                if bi < b {
                    b = bi;
                    s = get(&sr, i, op)?;
                    r = i;
                }
            }

            // If no more removable knot -> finished.
            if b == BIGD || b == NOT_REMOVABLE {
                break;
            }

            // Check constraints.
            if blocked(r, n) {
                set(&mut br, r, NOT_REMOVABLE, op)?;
                continue;
            }

            // Check error of removal.
            let mut rmf = true;
            let k;
            let l;
            if (p + s) % 2 != 0 {
                // Compute maximums of basis functions over each span.
                k = (p + s + 1) / 2;
                l = r - k + p + 1;
                let alf = (get(&uq, r, op)? - get(&uq, r - k, op)?)
                    / (get(&uq, r - k + p + 1, op)? - get(&uq, r - k, op)?);
                let bet = (get(&uq, r, op)? - get(&uq, r - k + 1, op)?)
                    / (get(&uq, r - k + p + 2, op)? - get(&uq, r - k + 1, op)?);
                let omb = 1.0 - bet;
                lam = alf / (alf + bet);
                oml = 1.0 - lam;

                let knt = KnotVector::new(&uq, ux(m, op)?)?;
                let left = min_max_or_default(&knt, r - k, pu, lto, &mut error, op)?;
                let right = min_max_or_default(&knt, r - k + 1, pu, lto, &mut error, op)?;
                combine_bounds(&mut max, left, right, lam, alf, oml, omb, p, false, op)?;
            } else {
                // Compute maximum of basis function.
                k = (p + s) / 2;
                l = r - k + p;

                let knt = KnotVector::new(&uq, ux(m, op)?)?;
                let (_, mx) = min_max_or_default(&knt, r - k, pu, lto, &mut error, op)?;
                for i in 0..=p {
                    set(&mut max, i, get(&mx, i, op)?, op)?;
                }
            }

            // Check the error.
            for i in r - k..=l {
                if get(&uq, i, op)? != get(&uq, i + 1, op)? {
                    let t = get(&er, i, op)? + get(&max, i - r + k, op)? * b;
                    set(&mut te, i, t, op)?;
                    if t > tol {
                        rmf = false;
                        break;
                    }
                }
            }

            // If error test passed -> update error vector and remove knot.
            if !rmf {
                // Knot is not removable.
                set(&mut br, r, NOT_REMOVABLE, op)?;
                continue;
            }

            for i in r - k..=l {
                if get(&uq, i, op)? != get(&uq, i + 1, op)? {
                    set(&mut er, i, get(&te, i, op)?, op)?;
                }
            }

            let fout = (2 * r - s - p) / 2;
            let first = r - p;
            let last = r - s;
            let off = first - 1;
            let mut i = first;
            let mut j = last;
            let mut ii: isize = 1;
            let mut jj = last - off;

            set(&mut rw, 0, get(&qw, off, op)?, op)?;
            set(&mut rw, last + 1 - off, get(&qw, last + 1, op)?, op)?;

            // Get new control points for one removal step.
            let ur = get(&uq, r, op)?;
            while j - i > 0 {
                let alf = (get(&uq, i + p + 1, op)? - get(&uq, i, op)?) / (ur - get(&uq, i, op)?);
                let oma = 1.0 - alf;
                let bet =
                    (get(&uq, j + p + 1, op)? - get(&uq, j, op)?) / (get(&uq, j + p + 1, op)? - ur);
                let omb = 1.0 - bet;
                let c = CPoint::combine2(alf, get(&qw, i, op)?, oma, get(&rw, ii - 1, op)?);
                set(&mut rw, ii, c, op)?;
                let c = CPoint::combine2(bet, get(&qw, j, op)?, omb, get(&rw, jj + 1, op)?);
                set(&mut rw, jj, c, op)?;
                i += 1;
                j -= 1;
                ii += 1;
                jj -= 1;
            }

            // Check for disallowed weights.
            if rational {
                i = first;
                j = last;
                let mut wmin = BIGD;
                let mut wmax = SMAD;
                while j - i > 0 {
                    let wi = get(&rw, i - off, op)?.w;
                    let wj = get(&rw, j - off, op)?.w;
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
                    set(&mut br, r, NOT_REMOVABLE, op)?;
                    continue;
                }
            }

            // Save new control points.
            if (p + s) % 2 != 0 {
                let c = CPoint::combine2(lam, get(&rw, jj + 1, op)?, oml, get(&rw, ii - 1, op)?);
                set(&mut rw, jj + 1, c, op)?;
            }

            i = first;
            j = last;
            while j - i > 0 {
                set(&mut qw, i, get(&rw, i - off, op)?, op)?;
                set(&mut qw, j, get(&rw, j - off, op)?, op)?;
                i += 1;
                j -= 1;
            }

            // Shift down some parameters.
            if s == 1 {
                let v = max2(get(&er, r - 1, op)?, get(&er, r, op)?);
                set(&mut er, r - 1, v, op)?;
            }
            if s > 1 {
                let v = get(&sr, r, op)? - 1;
                set(&mut sr, r - 1, v, op)?;
            }
            for i in r + 1..=m {
                let v = get(&br, i, op)?;
                set(&mut br, i - 1, v, op)?;
                let v = get(&sr, i, op)?;
                set(&mut sr, i - 1, v, op)?;
                let v = get(&er, i, op)?;
                set(&mut er, i - 1, v, op)?;
            }

            // Shift down knots and control points.
            for i in r + 1..=m {
                let v = get(&uq, i, op)?;
                set(&mut uq, i - 1, v, op)?;
            }
            for i in fout + 1..=n {
                let v = get(&qw, i, op)?;
                set(&mut qw, i - 1, v, op)?;
            }

            n -= 1;
            m -= 1;

            // If no more internal knots -> finished.
            if n == p {
                break;
            }

            // Update error bounds.
            let k = (r - p).max(p + 1);
            let l = n.min(r + p - s);
            for i in k..=l {
                if get(&uq, i, op)? != get(&uq, i + 1, op)? && get(&br, i, op)? != NOT_REMOVABLE {
                    let e = max_err(&qw, &uq, n, m, i, get(&sr, i, op)?)?;
                    set(&mut br, i, e, op)?;
                    error = None;
                }
            }
        }

        let mut out = Curve::with_indices(qw, ux(n, op)?, pu, uq, ux(m, op)?)?;

        // Compact output curve.
        if n < ns {
            out.compress();
            error = None;
        }

        match error {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }

    /// Reduces the degree by one when the reduction error stays within `tol`
    /// (`N_CrvReduceDegreeOnce`, algorithm A5.11 of The NURBS Book): Bezier segments
    /// are extracted, reduced and the knots between them removed again, accumulating
    /// error bounds per knot span.
    ///
    /// Errors with the errors of the Bezier reduction, and with the error of
    /// `N_BasisFindAllSpanMaxima` when its failing call was the last one to set
    /// upstream's error flag (see the module deviations).
    pub fn reduce_degree_once(&self, tol: f64) -> Result<DegreeReduction> {
        const OP: &str = "nurbs::crv_tool::p2::Curve::reduce_degree_once";
        let pw = self.cpts();
        let up = self.knots();
        let pu = self.degree();
        let np = sx(self.n(), OP)?;
        let mp = sx(self.m(), OP)?;
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let knt = self.knot_vector()?;

        if pu <= 1 {
            return Ok(DegreeReduction {
                curve: None,
                max_err: 0.0,
            });
        }

        // Adjust tolerance in case of rational curves.
        let rational = self.is_rational();
        let tol = if rational {
            let mm = self.min_max_weights_and_pts();
            (tol * mm.wmin) / (1.0 + mm.pmax)
        } else {
            tol
        };

        let lto = REMOVAL_PARAM_TOL * (get(up, mp, OP)? - get(up, 0, OP)?).abs();

        // Sizes of the reduced curve.
        let s = sx(knt.span_count(pu), OP)?;
        let nq = np - s;
        let qu = pu - 1;
        let q = p - 1;
        let mq = mp - s - 1;

        let mut qw: Vec<CPoint> = storage(nq, OP)?;
        let mut uq: Vec<f64> = storage(mq, OP)?;

        // Get local memory.
        let mut bw: Vec<CPoint> = storage(p, OP)?;
        let mut nw: Vec<CPoint> = storage(p, OP)?;
        let mut alfs: Vec<f64> = storage(p, OP)?;
        let mut omas: Vec<f64> = storage(p, OP)?;
        let mut x: Vec<f64> = storage(3 * p + 2, OP)?;
        let mut e: Vec<f64> = storage(mp, OP)?;
        let mut max: Vec<f64> = storage(p + 1, OP)?;

        // Get degree reduction coefficients.
        let coefs = degree_reduce_coefs(pu)?;

        // Initialize.
        let mut a = p;
        let mut b = p + 1;
        let mut r: isize = -1;
        let mut spn = p + 1;
        let mut cind: isize = 1;
        let mut kind = q + 1;
        let mut mlt = p + 1;
        let mut rfl = true;
        let mut mtol = 0.0;
        let mut lam = 0.0;
        let mut oml = 0.0;
        let mut error: Option<SolidError> = None;

        for i in 0..=q {
            set(&mut uq, i, get(up, a, OP)?, OP)?;
        }
        set(&mut qw, 0, get(pw, 0, OP)?, OP)?;
        for i in 0..=p {
            set(&mut bw, i, get(pw, i, OP)?, OP)?;
        }

        // Loop through the knot vector: extract the i-th Bezier segment, degree
        // reduce it, remove the knot between the i-th and the (i-1)-th segment.
        while b < mp {
            // Get multiplicity of the knot.
            let i0 = spn;
            while spn < mp && get(up, spn, OP)? == get(up, spn + 1, OP)? {
                spn += 1;
            }
            let oldmlt = mlt;
            mlt = spn - i0 + 1;

            // Insert knot to get Bezier segment.
            b = b + mlt - 1;
            let oldr = r;
            r = p - mlt;

            let lbz = if oldr > 0 { (oldr + 2) / 2 } else { 1 };

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

            // Now degree reduce the Bezier segment.
            let red = bez_reduce_degree(&bw, pu, &coefs)?;
            error = None;
            let mut dw = red.qw;
            let de = red.err;

            let ea = get(&e, a, OP)? + de;
            set(&mut e, a, ea, OP)?;
            if ea > mtol {
                mtol = ea;
            }
            if ea > tol {
                rfl = false;
                break;
            }

            // Remove the knot UP[a].
            if oldr > 0 {
                let mut first = kind;
                let den = get(up, b, OP)? - get(up, a, OP)?;
                let ua = get(up, a, OP)?;
                let ub = get(up, b, OP)?;
                let mut i: isize = first;

                for (k, last) in (0..oldr).zip(kind..) {
                    i = first;
                    let mut j = last;
                    let mut l = j - kind;

                    while j - i > k {
                        let alf = (ub - get(&uq, i - 1, OP)?) / (ua - get(&uq, i - 1, OP)?);
                        let oma = 1.0 - alf;
                        let bet = (ub - get(&uq, j - k - 1, OP)?) / den;
                        let omb = 1.0 - bet;
                        let c =
                            CPoint::combine2(alf, get(&qw, i - 1, OP)?, oma, get(&qw, i - 2, OP)?);
                        set(&mut qw, i - 1, c, OP)?;
                        let c = CPoint::combine2(bet, get(&dw, l, OP)?, omb, get(&dw, l + 1, OP)?);
                        set(&mut dw, l, c, OP)?;

                        if rational {
                            let wi = get(&qw, i - 1, OP)?.w;
                            let wl = get(&dw, l, OP)?.w;
                            if wi < WEIGHT_MIN
                                || wl < WEIGHT_MIN
                                || wi > WEIGHT_MAX
                                || wl > WEIGHT_MAX
                            {
                                rfl = false;
                                break;
                            }
                        }
                        i += 1;
                        j -= 1;
                        l -= 1;
                    }

                    if !rfl {
                        break;
                    }

                    // Load the knot vector to compute precise error.
                    let kk = a + oldr - k;
                    let ll = kk - (2 * p - k + 1) / 2;
                    let jj = k % 2;
                    let mut c: isize = -1;

                    let ull = get(up, ll, OP)?;
                    for _ in 0..=p {
                        c += 1;
                        set(&mut x, c, ull, OP)?;
                    }
                    for ii in ll + 1..=a - oldmlt {
                        c += 1;
                        set(&mut x, c, get(up, ii, OP)?, OP)?;
                    }
                    for _ in 1..=p - k {
                        c += 1;
                        set(&mut x, c, ua, OP)?;
                    }
                    for _ in kk - ll - jj..=2 * p {
                        c += 1;
                        set(&mut x, c, ub, OP)?;
                    }

                    let knx = KnotVector::new(&x, ux(c, OP)?)?;

                    // Compute the error.
                    let dwv;
                    if j - i < k {
                        let alf = (ua - get(&uq, i - 2, OP)?) / (ub - get(&uq, i - 2, OP)?);
                        let bet = (ua - get(&uq, i - 1, OP)?) / (ub - get(&uq, i - 1, OP)?);
                        let omb = 1.0 - bet;
                        lam = alf / (alf + bet);
                        oml = 1.0 - lam;

                        dwv = dist_cpt_cpt_homo(get(&qw, i - 2, OP)?, get(&dw, l + 1, OP)?);

                        let left = min_max_or_default(&knx, p, pu, lto, &mut error, OP)?;
                        let right = min_max_or_default(&knx, p + 1, pu, lto, &mut error, OP)?;
                        combine_bounds(&mut max, left, right, lam, alf, oml, omb, p, true, OP)?;
                    } else {
                        let alf = (ua - get(&uq, i - 1, OP)?) / (ub - get(&uq, i - 1, OP)?);
                        let oma = 1.0 - alf;
                        let av =
                            CPoint::combine2(alf, get(&dw, l + 1, OP)?, oma, get(&qw, i - 2, OP)?);
                        dwv = dist_cpt_cpt_homo(get(&qw, i - 1, OP)?, av);

                        let (_, mx) = min_max_or_default(&knx, p, pu, lto, &mut error, OP)?;
                        for ii in 0..=p {
                            set(&mut max, ii, get(&mx, ii, OP)?, OP)?;
                        }
                    }

                    for ii in ll..=a {
                        if get(up, ii, OP)? != get(up, ii + 1, OP)? {
                            let v = get(&e, ii, OP)? + dwv * get(&max, ii - ll, OP)?;
                            set(&mut e, ii, v, OP)?;
                            if v > mtol {
                                mtol = v;
                            }
                            if v > tol {
                                rfl = false;
                                break;
                            }
                        }
                    }

                    if !rfl {
                        break;
                    }

                    // Average control points.
                    if j - i < k {
                        let c =
                            CPoint::combine2(lam, get(&dw, l + 1, OP)?, oml, get(&qw, i - 2, OP)?);
                        set(&mut qw, i - 2, c, OP)?;
                    }
                    first -= 1;
                }

                if !rfl {
                    break;
                }
                cind = i - 1;
            }

            // Load knot vector and control points.
            if a != p {
                for _ in 0..q - oldr {
                    set(&mut uq, kind, get(up, a, OP)?, OP)?;
                    kind += 1;
                }
            }
            for i in lbz..=q {
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
                spn += 1;
            } else {
                for i in 0..=q {
                    set(&mut uq, kind + i, get(up, b, OP)?, OP)?;
                }
            }
        }

        if let Some(e) = error {
            return Err(e);
        }
        let curve = if rfl {
            Some(curve(qw, qu, uq, OP)?)
        } else {
            None
        };
        Ok(DegreeReduction {
            curve,
            max_err: mtol,
        })
    }

    /// Reduces the degree as far as possible: repeats
    /// [`Curve::reduce_degree_once`] with the remaining tolerance until a reduction
    /// fails or the accumulated error reaches `tol` (`N_CrvReduceDegree`). Returns the
    /// last successfully reduced curve (a copy of `self` when none succeeded).
    pub fn reduce_degree(&self, tol: f64) -> Result<Curve> {
        // Reduce the degree to as low as possible.
        let mut ctol = 0.0;
        let mut rfl = true;
        let mut cur = self.clone();

        while rfl && ctol < tol {
            let red = cur.reduce_degree_once(tol - ctol)?;
            rfl = red.curve.is_some();
            if let Some(c) = red.curve {
                cur = c;
            }
            ctol += red.max_err;
        }
        Ok(cur)
    }

    /// Removes the degenerate segments, i.e. parameter intervals whose image has arc
    /// length below `tol` (`N_CrvRemoveDegenSegs`): the segments are made Bezier
    /// segments, reparameterised to short parameter lengths and cut out; the
    /// parameterisation changes, the geometry moves by at most `tol`. When the
    /// degenerate segments cover the whole curve only its removable knots are removed
    /// ([`Curve::remove_knots`]).
    ///
    /// Returns `None` (upstream `nr = 0`) for a single-segment curve, a curve without
    /// degenerate segments, or one that is a single degenerate segment.
    ///
    /// Errors with the errors of [`Curve::degen_segs`], knot insertion and knot
    /// removal.
    pub fn remove_degen_segs(&self, tol: f64) -> Result<Option<DegenSegRemoval>> {
        const OP: &str = "nurbs::crv_tool::p2::Curve::remove_degen_segs";
        let up = self.knots();
        let pu = self.degree();
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let np = sx(self.n(), OP)?;
        let mp = sx(self.m(), OP)?;

        // Only check curves with at least two segments.
        if np <= p {
            return Ok(None);
        }

        // Find degeneracies.
        let mut segs: Vec<[f64; 2]> = self
            .degen_segs(tol)?
            .into_iter()
            .map(|(a, b)| [a, b])
            .collect();
        let ns = sx(segs.len(), OP)? - 1;
        if ns < 0 {
            return Ok(None);
        }

        let up0 = get(up, 0, OP)?;
        let upm = get(up, mp, OP)?;
        let sg = |segs: &[[f64; 2]], i: isize, k: usize| -> Result<f64> {
            get(segs, i, OP)?.get(k).copied().ok_or_else(|| oob(OP))
        };
        let sgs = |segs: &mut [[f64; 2]], i: isize, k: usize, v: f64| -> Result<()> {
            let slot = usize::try_from(i)
                .ok()
                .and_then(|i| segs.get_mut(i))
                .and_then(|s| s.get_mut(k))
                .ok_or_else(|| oob(OP))?;
            *slot = v;
            Ok(())
        };
        // Contiguous degenerate segments from `ii` on: index of the last one.
        let run_end = |segs: &[[f64; 2]], ii: isize| -> Result<isize> {
            let mut jj = ii;
            while jj < ns {
                if sg(segs, jj, 1)? != sg(segs, jj + 1, 0)? {
                    break;
                }
                jj += 1;
            }
            Ok(jj)
        };

        if ns == 0 && sg(&segs, 0, 0)? == up0 && sg(&segs, 0, 1)? == upm {
            return Ok(None);
        }

        // There are degenerate segments to be removed.
        let dlen = 0.00005 * (upm - up0);
        let removed = ux(ns + 1, OP)?;

        // Work on a copy of the curve.
        let mut cur = self.clone();

        // Make segments to be removed into Beziers and reparameterize so that their
        // parameter lengths are small.
        let mut ii: isize = 0;
        while ii <= ns {
            let jj = run_end(&segs, ii)?;

            // Special case out contiguous segments equal entire curve.
            if sg(&segs, ii, 0)? == up0 && sg(&segs, jj, 1)? == upm {
                // Just remove internal knots.
                let curve = cur.remove_knots(tol)?;
                return Ok(Some(DegenSegRemoval { curve, removed }));
            }

            // Make sure the segment ends are p-multiplicity knots.
            let a0 = sg(&segs, ii, 0)?;
            let (_, mult1) = cur.knot_vector()?.find_span_and_mult(pu, a0, Side::Left)?;
            if mult1 < pu {
                cur = cur.insert_knot(a0, pu - mult1)?;
            }
            let b1 = sg(&segs, jj, 1)?;
            let (_, mult2) = cur.knot_vector()?.find_span_and_mult(pu, b1, Side::Left)?;
            if mult2 < pu {
                cur = cur.insert_knot(b1, pu - mult2)?;
            }

            let mq = sx(cur.m(), OP)?;
            let du = sg(&segs, jj, 1)? - sg(&segs, ii, 0)?;

            if du > dlen {
                // Reparameterize.
                let uq0 = get(cur.knots(), 0, OP)?;
                if sg(&segs, ii, 0)? == uq0 {
                    // Segment at start.
                    let span2 = sx(
                        cur.knot_vector()?
                            .find_span(pu, sg(&segs, jj, 1)?, Side::Right)?,
                        OP,
                    )? + 1;
                    let uq = cur.knots_mut();

                    let u2 = get(uq, 0, OP)? + dlen;
                    let d2 = dlen / du;

                    for kk in ii..jj {
                        let u0 = get(uq, 0, OP)?;
                        let v = d2 * (sg(&segs, kk, 1)? - u0) + u0;
                        sgs(&mut segs, kk, 1, v)?;
                        let v = d2 * (sg(&segs, kk + 1, 0)? - u0) + u0;
                        sgs(&mut segs, kk + 1, 0, v)?;
                    }

                    for kk in p + 1..span2 {
                        let u0 = get(uq, 0, OP)?;
                        set(uq, kk, d2 * (get(uq, kk, OP)? - u0) + u0, OP)?;
                    }

                    let d3 = (get(uq, mq, OP)? - u2) / (get(uq, mq, OP)? - sg(&segs, jj, 1)?);

                    for kk in jj + 1..=ns {
                        let sj = sg(&segs, jj, 1)?;
                        let v = d3 * (sg(&segs, kk, 0)? - sj) + u2;
                        sgs(&mut segs, kk, 0, v)?;
                        if sg(&segs, kk, 1)? != get(uq, mq, OP)? {
                            let v = d3 * (sg(&segs, kk, 1)? - sj) + u2;
                            sgs(&mut segs, kk, 1, v)?;
                        }
                    }

                    for kk in span2 + p..mq - p {
                        let v = d3 * (get(uq, kk, OP)? - get(uq, span2, OP)?) + u2;
                        set(uq, kk, v, OP)?;
                    }

                    sgs(&mut segs, jj, 1, u2)?;

                    for kk in 0..p {
                        set(uq, span2 + kk, u2, OP)?;
                    }
                } else if sg(&segs, jj, 1)? == get(cur.knots(), mq, OP)? {
                    // Segment at end.
                    let span1 = sx(
                        cur.knot_vector()?
                            .find_span(pu, sg(&segs, ii, 0)?, Side::Left)?,
                        OP,
                    )?;
                    let uq = cur.knots_mut();

                    let u2 = get(uq, mq, OP)? - dlen;
                    let d2 = dlen / du;

                    for kk in ii..jj {
                        let us = get(uq, span1, OP)?;
                        let v = d2 * (sg(&segs, kk, 1)? - us) + u2;
                        sgs(&mut segs, kk, 1, v)?;
                        let v = d2 * (sg(&segs, kk + 1, 0)? - us) + u2;
                        sgs(&mut segs, kk + 1, 0, v)?;
                    }

                    for kk in span1 + 1..mq - p {
                        let v = d2 * (get(uq, kk, OP)? - get(uq, span1, OP)?) + u2;
                        set(uq, kk, v, OP)?;
                    }

                    let d3 = (u2 - get(uq, 0, OP)?) / (get(uq, span1, OP)? - get(uq, 0, OP)?);

                    for kk in 0..ii {
                        let u0 = get(uq, 0, OP)?;
                        let v = d3 * (sg(&segs, kk, 1)? - u0) + u0;
                        sgs(&mut segs, kk, 1, v)?;
                        if sg(&segs, kk, 0)? != u0 {
                            let v = d3 * (sg(&segs, kk, 0)? - u0) + u0;
                            sgs(&mut segs, kk, 0, v)?;
                        }
                    }

                    for kk in p + 1..=span1 - p {
                        let u0 = get(uq, 0, OP)?;
                        set(uq, kk, d3 * (get(uq, kk, OP)? - u0) + u0, OP)?;
                    }

                    sgs(&mut segs, ii, 0, u2)?;

                    for kk in 0..p {
                        set(uq, span1 - kk, u2, OP)?;
                    }
                } else {
                    // Internal segment.
                    let knt = cur.knot_vector()?;
                    let span1 = sx(knt.find_span(pu, sg(&segs, ii, 0)?, Side::Left)?, OP)?;
                    let span2 = sx(knt.find_span(pu, sg(&segs, jj, 1)?, Side::Right)?, OP)? + 1;
                    let uq = cur.knots_mut();

                    let d1 = 0.5 * (sg(&segs, ii, 0)? + sg(&segs, jj, 1)?);
                    let u1 = d1 - 0.499 * dlen;
                    let u2 = d1 + 0.499 * dlen;
                    let d2 = (u2 - u1) / du;

                    for kk in ii..jj {
                        let us = get(uq, span1, OP)?;
                        let v = d2 * (sg(&segs, kk, 1)? - us) + u1;
                        sgs(&mut segs, kk, 1, v)?;
                        let v = d2 * (sg(&segs, kk + 1, 0)? - us) + u1;
                        sgs(&mut segs, kk + 1, 0, v)?;
                    }

                    for kk in span1 + 1..span2 {
                        let v = d2 * (get(uq, kk, OP)? - get(uq, span1, OP)?) + u1;
                        set(uq, kk, v, OP)?;
                    }

                    let d3 = (u1 - get(uq, 0, OP)?) / (get(uq, span1, OP)? - get(uq, 0, OP)?);

                    for kk in 0..ii {
                        let u0 = get(uq, 0, OP)?;
                        let v = d3 * (sg(&segs, kk, 1)? - u0) + u0;
                        sgs(&mut segs, kk, 1, v)?;
                        if sg(&segs, kk, 0)? != u0 {
                            let v = d3 * (sg(&segs, kk, 0)? - u0) + u0;
                            sgs(&mut segs, kk, 0, v)?;
                        }
                    }

                    for kk in p + 1..=span1 - p {
                        let u0 = get(uq, 0, OP)?;
                        set(uq, kk, d3 * (get(uq, kk, OP)? - u0) + u0, OP)?;
                    }

                    sgs(&mut segs, ii, 0, u1)?;

                    for kk in 0..p {
                        set(uq, span1 - kk, u1, OP)?;
                    }

                    let d3 = (get(uq, mq, OP)? - u2) / (get(uq, mq, OP)? - sg(&segs, jj, 1)?);

                    for kk in jj + 1..=ns {
                        let sj = sg(&segs, jj, 1)?;
                        let v = d3 * (sg(&segs, kk, 0)? - sj) + u2;
                        sgs(&mut segs, kk, 0, v)?;
                        if sg(&segs, kk, 1)? != get(uq, mq, OP)? {
                            let v = d3 * (sg(&segs, kk, 1)? - sj) + u2;
                            sgs(&mut segs, kk, 1, v)?;
                        }
                    }

                    for kk in span2 + p..mq - p {
                        let v = d3 * (get(uq, kk, OP)? - get(uq, span2, OP)?) + u2;
                        set(uq, kk, v, OP)?;
                    }

                    sgs(&mut segs, jj, 1, u2)?;

                    for kk in 0..p {
                        set(uq, span2 + kk, u2, OP)?;
                    }
                }
            }

            ii = jj + 1;
        }

        // Now remove the degenerate segments. There are two cases: interior segments
        // vs. end segments.
        ii = 0;
        while ii <= ns {
            let jj = run_end(&segs, ii)?;

            // Get local notation.
            let mut nq = sx(cur.n(), OP)?;
            let mut mq = sx(cur.m(), OP)?;

            // Now remove the segment.
            if sg(&segs, ii, 0)? == up0 {
                // Start segment.
                let span2 = sx(
                    cur.knot_vector()?
                        .find_span(pu, sg(&segs, jj, 1)?, Side::Right)?,
                    OP,
                )?;
                let mut uq = cur.knots().to_vec();
                let mut qw = cur.cpts().to_vec();

                let mut k1 = p + 1;
                for kk in span2 + p + 1..=mq {
                    let v = get(&uq, kk, OP)?;
                    set(&mut uq, k1, v, OP)?;
                    k1 += 1;
                }
                realloc(&mut uq, k1 - 1, OP)?;

                k1 = 1;
                for kk in span2 + 1..=nq {
                    let v = get(&qw, kk, OP)?;
                    set(&mut qw, k1, v, OP)?;
                    k1 += 1;
                }
                realloc(&mut qw, k1 - 1, OP)?;
                cur = curve(qw, pu, uq, OP)?;
            } else if sg(&segs, jj, 1)? == upm {
                // End segment.
                let span1 = sx(
                    cur.knot_vector()?
                        .find_span(pu, sg(&segs, ii, 0)?, Side::Left)?,
                    OP,
                )? + 1;
                let mut uq = cur.knots().to_vec();
                let mut qw = cur.cpts().to_vec();

                let um = get(&uq, mq, OP)?;
                for k1 in span1 - p..=span1 {
                    set(&mut uq, k1, um, OP)?;
                }
                realloc(&mut uq, span1, OP)?;

                let v = get(&qw, nq, OP)?;
                set(&mut qw, nq - mq + span1, v, OP)?;
                realloc(&mut qw, nq - mq + span1, OP)?;
                cur = curve(qw, pu, uq, OP)?;
            } else {
                // Interior segment.
                let u2 = 0.5 * (sg(&segs, ii, 0)? + sg(&segs, jj, 1)?);
                let (_, mult2) = cur.knot_vector()?.find_span_and_mult(pu, u2, Side::Left)?;
                if mult2 < pu {
                    cur = cur.insert_knot(u2, pu - mult2)?;
                    nq = sx(cur.n(), OP)?;
                    mq = sx(cur.m(), OP)?;
                }

                let knt = cur.knot_vector()?;
                let span1 = sx(knt.find_span(pu, sg(&segs, ii, 0)?, Side::Left)?, OP)?;
                let span2 = sx(knt.find_span(pu, u2, Side::Left)?, OP)?;
                let span3 = sx(knt.find_span(pu, sg(&segs, jj, 1)?, Side::Left)?, OP)?;
                let mut uq = cur.knots().to_vec();
                let mut qw = cur.cpts().to_vec();

                let mut k1 = span1 - p + 1;
                for _ in 0..p {
                    set(&mut uq, k1, u2, OP)?;
                    k1 += 1;
                }
                for kk in span3 + 1..=mq {
                    let v = get(&uq, kk, OP)?;
                    set(&mut uq, k1, v, OP)?;
                    k1 += 1;
                }
                realloc(&mut uq, k1 - 1, OP)?;

                k1 = span1 - p;
                let v = get(&qw, span2 - p, OP)?;
                set(&mut qw, k1, v, OP)?;
                k1 += 1;
                for kk in span3 - p + 1..=nq {
                    let v = get(&qw, kk, OP)?;
                    set(&mut qw, k1, v, OP)?;
                    k1 += 1;
                }
                realloc(&mut qw, k1 - 1, OP)?;
                cur = curve(qw, pu, uq, OP)?;
            }

            ii = jj + 1;
        }

        Ok(Some(DegenSegRemoval {
            curve: cur,
            removed,
        }))
    }
}

/// Elevates every curve to the highest degree of the set (upstream loop shared by
/// the `N_CrvsMakeCompatible*` routines).
fn elevate_to_common_degree(curves: &mut [Curve]) -> Result<usize> {
    // Get highest degree.
    let mut ph = curves.first().map_or(0, Curve::degree);
    for c in curves.iter().skip(1) {
        let p = c.degree();
        if p > ph {
            ph = p;
        }
    }

    // Elevate the degrees.
    for c in curves.iter_mut() {
        let t = ph - c.degree();
        if t > 0 {
            *c = c.elevate_degree(t)?;
        }
    }
    Ok(ph)
}

/// Merges the knot vectors with `merge` (which may move knots in place, as upstream
/// does through the curves' own knot vectors) and refines each curve with its new
/// knots.
fn merge_and_refine(
    curves: &mut [Curve],
    merge: impl FnOnce(&mut [Vec<f64>]) -> Result<Vec<Vec<f64>>>,
    mut trace: Option<&mut [EntryKnots]>,
) -> Result<()> {
    // Merge knot vectors.
    let mut knots: Vec<Vec<f64>> = curves.iter().map(|c| c.knots().to_vec()).collect();
    let merged = merge(&mut knots);
    for (c, u) in curves.iter_mut().zip(&knots) {
        for (dst, src) in c.knots_mut().iter_mut().zip(u) {
            *dst = *src;
        }
    }
    if let Some(t) = trace.as_deref_mut() {
        EntryKnots::observe(t, curves);
    }
    let knx = merged?;

    // Refine curves.
    for (i, (c, x)) in curves.iter_mut().zip(&knx).enumerate() {
        if !x.is_empty() {
            *c = c.refine(x)?;
            if let Some(e) = trace.as_deref_mut().and_then(|t| t.get_mut(i)) {
                e.live = false;
            }
        }
    }
    Ok(())
}

/// The knot storage a curve had on entry to [`make_compatible_knot_tol_traced`], as
/// upstream's in-place writes leave it: `N_CrvReparamToInterval` and the knot merging
/// write into the curve's current knot array, while a degree elevation or a
/// refinement gives the curve a new one (the entry array then keeps its contents).
/// Callers whose curve shares its knot array with another object (upstream
/// `N_CrvFromCPolygonAndKnotVector` over a surface's knot vector) see these writes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EntryKnots {
    /// Contents of the entry storage.
    pub(crate) knots: Vec<f64>,
    /// The curve still uses its entry storage.
    pub(crate) live: bool,
}

impl EntryKnots {
    /// Copies the current knots of every curve still using its entry storage.
    fn observe(trace: &mut [EntryKnots], curves: &[Curve]) {
        for (e, c) in trace.iter_mut().zip(curves) {
            if e.live {
                e.knots = c.knots().to_vec();
            }
        }
    }
}

/// Makes a set of curves compatible in place: the same rationality and dimension,
/// the same (highest) degree and the same knot vector (`N_CrvsMakeCompatibleKnotTol`).
///
/// When the parameter ranges differ (by more than `1e-12`) all curves are first
/// reparameterised to `[0, 1]`. Knots closer than `knot_tol` count as one;
/// `knot_tol = 0` means `0.001` after rescaling, else a thousandth of the first
/// curve's range, and any tolerance up to `1e-8` merges with
/// [`compatible_knot_array`].
///
/// Errors with `InvalidInput` for an empty set, with the errors of degree
/// elevation, knot merging and refinement (the curves keep the changes made before).
pub fn make_compatible_knot_tol(curves: &mut [Curve], knot_tol: f64) -> Result<()> {
    make_compatible_knot_tol_traced(curves, knot_tol).map(|_| ())
}

/// [`make_compatible_knot_tol`], also returning what upstream's in-place writes leave
/// in each curve's entry knot storage ([`EntryKnots`]).
pub(crate) fn make_compatible_knot_tol_traced(
    curves: &mut [Curve],
    knot_tol: f64,
) -> Result<Vec<EntryKnots>> {
    const OP: &str = "nurbs::crv_tool::p2::make_compatible_knot_tol";
    let mut trace: Vec<EntryKnots> = curves
        .iter()
        .map(|c| EntryKnots {
            knots: c.knots().to_vec(),
            live: true,
        })
        .collect();
    let mut knot_tol = knot_tol;
    if curves.is_empty() {
        return Err(oob(OP));
    }

    // Make curve definitions compatible.
    make_rat_compatible(curves);

    // If necessary, scale knot vectors to the unit interval.
    let (us, ue) = curves.first().ok_or_else(|| oob(OP))?.param_bounds();

    // Remember if any curve's parameter bounds are not equal.
    let differ = curves.iter().skip(1).any(|c| {
        let (u1, u2) = c.param_bounds();
        (u1 - us).abs() > 1.0e-12 || (u2 - ue).abs() > 1.0e-12
    });

    // When any curve's parameter bounds are not equal, scale all curves to the unit
    // interval.
    if differ {
        for c in curves.iter_mut() {
            c.reparam_to_interval(UNIT_SPAN)?;
        }
        if knot_tol == 0.0 {
            knot_tol = 0.001;
        }
        EntryKnots::observe(&mut trace, curves);
    }

    if knot_tol == 0.0 {
        // Set the knot tolerance to 0.001 of the curve domain.
        knot_tol = (ue - us) / 1000.0;
    }

    let degrees: Vec<usize> = curves.iter().map(Curve::degree).collect();
    elevate_to_common_degree(curves)?;
    for ((e, c), p) in trace.iter_mut().zip(curves.iter()).zip(&degrees) {
        if c.degree() != *p {
            e.live = false;
        }
    }

    if knot_tol <= COMPATIBLE_KNOT_TOL {
        merge_and_refine(curves, compatible_knot_array, Some(&mut trace))?;
    } else {
        merge_and_refine(
            curves,
            |k| compatible_knot_vectors_to_tol(k, knot_tol),
            Some(&mut trace),
        )?;
    }
    Ok(trace)
}

/// [`make_compatible_knot_tol`] with the default knot tolerance `0`
/// (`N_CrvsMakeCompatible`).
pub fn make_compatible(curves: &mut [Curve]) -> Result<()> {
    make_compatible_knot_tol(curves, 0.0)
}

/// Makes a set of curves compatible in place like [`make_compatible_knot_tol`], with
/// `tol` both for the parameter range check and as the knot merging tolerance
/// (`N_CrvsMakeCompatibleAdjKnots`). Before merging, a curve whose last interior
/// knot lies within `tol` of the (smallest) end parameter has that knot removed
/// (with unlimited tolerance), so every last span is longer than `tol`.
///
/// Errors with `InvalidInput` for an empty set, [`NlibCode::Input`] when that knot
/// would have to be removed from a Bezier curve, and with the errors of degree
/// elevation, knot removal, knot merging and refinement (the curves keep the
/// changes made before).
pub fn make_compatible_adj_knots(curves: &mut [Curve], tol: f64) -> Result<()> {
    const OP: &str = "nurbs::crv_tool::p2::make_compatible_adj_knots";
    if curves.is_empty() {
        return Err(oob(OP));
    }

    // Make curve definitions compatible.
    make_rat_compatible(curves);

    // Scale knot vectors to the unit interval if necessary.
    let (ul, ur) = curves.first().ok_or_else(|| oob(OP))?.param_bounds();
    let mut dd = ur;
    let mut scale = false;
    for c in curves.iter().skip(1) {
        let (u0, um) = c.param_bounds();
        if (u0 - ul).abs() > tol || (um - ur).abs() > tol {
            scale = true;
            break;
        }
        if um < dd {
            dd = um;
        }
    }

    if scale {
        for c in curves.iter_mut() {
            c.reparam_to_interval(UNIT_SPAN)?;
        }
        dd = 1.0;
    }

    let ph = elevate_to_common_degree(curves)?;

    // Ensure that the last span in all knot vectors has length greater than tol.
    let phs = sx(ph, OP)?;
    for c in curves.iter_mut() {
        let m = sx(c.m(), OP)?;
        let v = get(c.knots(), m - phs - 1, OP)?;
        if dd - v <= tol {
            // Must remove this knot.
            if m == 2 * phs + 1 {
                return Err(SolidError::nlib(OP, NlibCode::Input));
            }
            *c = c.remove_knot(v, ph, BIGD)?.curve;
        }
    }

    merge_and_refine(curves, |k| compatible_knot_vectors_to_tol(k, tol), None)
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

    fn close(a: &Curve, b: &Curve, eps: f64) -> bool {
        a.n() == b.n()
            && a.m() == b.m()
            && a.knots()
                .iter()
                .zip(b.knots())
                .all(|(x, y)| (x - y).abs() <= eps)
            && a.cpts().iter().zip(b.cpts()).all(|(x, y)| {
                (x.x - y.x).abs() <= eps && (x.y - y.y).abs() <= eps && (x.z - y.z).abs() <= eps
            })
    }

    #[test]
    fn remove_knots_undoes_refinement() {
        let c = cubic();
        let r = c.refine(&[0.2, 0.3, 0.7]).unwrap();
        let q = r.remove_knots(1e-9).unwrap();
        assert!(close(&q, &c, 1e-9));
        assert_eq!(q.storage(), (q.n() + 1, q.m() + 1));
    }

    #[test]
    fn remove_knots_keeps_needed_knot() {
        let c = cubic();
        let q = c.remove_knots(1e-12).unwrap();
        assert_eq!(q, c);
        let q = c.remove_knots(1e6).unwrap();
        assert_eq!(q.n(), 3);
    }

    #[test]
    fn reduce_degree_once_of_elevated_curve() {
        let c = cubic();
        let e = c.elevate_degree(1).unwrap();
        let red = e.reduce_degree_once(1e-9).unwrap();
        let q = red.curve.unwrap();
        assert_eq!(q.degree(), 3);
        assert!(close(&q, &c, 1e-9));
        assert!(red.max_err < 1e-9);
    }

    #[test]
    fn reduce_degree_once_fails_beyond_tolerance() {
        let red = cubic().reduce_degree_once(1e-12).unwrap();
        assert!(red.curve.is_none());
        assert!(red.max_err > 1e-12);
        let line = Curve::new(
            vec![pt(0.0, 0.0, 0.0), pt(1.0, 1.0, 0.0)],
            1,
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        let red = line.reduce_degree_once(1.0).unwrap();
        assert_eq!(
            red,
            DegreeReduction {
                curve: None,
                max_err: 0.0
            }
        );
    }

    #[test]
    fn reduce_degree_repeats() {
        let c = cubic();
        let e = c.elevate_degree(2).unwrap();
        let q = e.reduce_degree(1e-9).unwrap();
        assert_eq!(q.degree(), 3);
        assert!(close(&q, &c, 1e-8));
        assert_eq!(c.reduce_degree(0.0).unwrap(), c);
    }

    #[test]
    fn make_compatible_common_degree_and_knots() {
        let a = cubic();
        let b = Curve::new(
            vec![pt(0.0, 0.0, 0.0), pt(1.0, 1.0, 0.0), pt(2.0, 0.0, 0.0)],
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .unwrap();
        let mut v = vec![a, b];
        make_compatible(&mut v).unwrap();
        assert_eq!(v[0].degree(), v[1].degree());
        assert_eq!(v[0].knots(), v[1].knots());
        assert_eq!(v[0].n(), v[1].n());
        assert!(make_compatible(&mut []).is_err());
    }

    #[test]
    fn make_compatible_rescales_different_ranges() {
        let a = cubic();
        let mut b = cubic();
        b.reparam_to_interval(Interval::new(2.0, 5.0)).unwrap();
        let mut v = vec![a, b];
        make_compatible_knot_tol(&mut v, 0.0).unwrap();
        assert_eq!(v[0].param_bounds(), (0.0, 1.0));
        assert_eq!(v[0].knots(), v[1].knots());
    }

    #[test]
    fn adj_knots_removes_short_last_span() {
        let pw = vec![
            pt(0.0, 0.0, 0.0),
            pt(1.0, 2.0, 0.0),
            pt(2.0, -1.0, 1.0),
            pt(3.0, 3.0, 0.5),
            pt(4.0, 0.0, 2.0),
        ];
        let a = Curve::new(pw, 3, vec![0.0, 0.0, 0.0, 0.0, 0.9999, 1.0, 1.0, 1.0, 1.0]).unwrap();
        let mut v = vec![a, cubic()];
        make_compatible_adj_knots(&mut v, 1e-3).unwrap();
        assert_eq!(v[0].knots(), v[1].knots());
        assert!(!v[0].knots().contains(&0.9999));
    }

    #[test]
    fn remove_degen_segs_single_segment_is_noop() {
        let c = Curve::new(
            vec![pt(0.0, 0.0, 0.0), pt(1.0, 1.0, 0.0), pt(2.0, 0.0, 0.0)],
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .unwrap();
        assert_eq!(c.remove_degen_segs(1e-6).unwrap(), None);
    }

    #[test]
    fn remove_degen_segs_none_found() {
        assert_eq!(cubic().remove_degen_segs(1e-9).unwrap(), None);
    }

    /// Fuzz regression: a huge degree overflowed `2 * p` in the knot removal.
    #[test]
    fn huge_degree_is_an_error() {
        let c = cubic();
        for p in [1usize << 31, i64::MAX as usize, usize::MAX] {
            let h = Curve::new(c.cpts().to_vec(), p, c.knots().to_vec()).unwrap();
            let r = h.remove_knots(1e-3);
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
            let r = h.reduce_degree_once(1e-3);
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        }
    }
}
