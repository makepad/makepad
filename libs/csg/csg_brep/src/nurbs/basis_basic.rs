//! B-spline basis extrema, all basis derivatives at a parameter, knot-vector
//! reparametrisation and knot merging (making knot vectors compatible).
//!
//! Upstream: `source/SMLib/src/NL_BasisBasic.cpp` (declarations in
//! `source/SMLib/inc/NL_BasisBasic.h`).
//!
//! | Rust                                  | Upstream                           |
//! |---------------------------------------|------------------------------------|
//! | [`find_global_max`]                   | `N_BasisFindGlobalMax`             |
//! | [`find_all_span_maxima`], [`SpanMaxima`] | `N_BasisFindAllSpanMaxima`      |
//! | [`reparam`]                           | `N_BasisReparam`                   |
//! | [`compatible_knot_array`]             | `N_GetCompatibleKnotArray`         |
//! | [`compatible_knot_vectors_to_tol`]    | `N_GetCompatibleKnotVectorToTol`   |
//! | `not_done` (private)                  | `ST_notdone`                       |
//! | [`basis_derivs`], [`BasisDerivs`]     | `N_BasisDerivs`                    |
//! | [`make_knots_compatible`]             | `N_MakeKnotsCompatible`            |
//! | [`PTOL`]                              | `NL_PTOL` (never reassigned upstream) |
//!
//! Deviations:
//! - Out-params become return values. Knot vectors that upstream rescales or adjusts
//!   in place are `&mut [f64]` (`U[0..=m]`, `m = len - 1`); the new-knot vectors that
//!   upstream allocates (or expects the caller to allocate) are returned as `Vec`s.
//! - Errors are [`SolidError::Nlib`] values carrying the upstream `NL_*_ERR` number. On
//!   error only the error is returned (upstream may leave outputs partly written; the
//!   in-place knot edits made before an error are kept, as upstream).
//! - Every array access is bounds checked. Where upstream would read or write outside
//!   its arrays (knot vectors too short for the degree, knots that run past the clamped
//!   ends in `N_MakeKnotsCompatible`, more new knots than `N_GetCompatibleKnotVectorToTol`
//!   allocated) the functions return [`SolidError::InvalidInput`] instead.
//! - Where upstream would loop forever, the functions return
//!   [`SolidError::InvalidInput`] instead: `N_BasisFindGlobalMax` when its bracketing
//!   step `du` no longer advances the parameter (huge knot values with a tiny span);
//!   `N_GetCompatibleKnotVectorToTol` when an iteration matches no knot (`tol <= 0` or
//!   NaN knots); `N_MakeKnotsCompatible` when an iteration advances neither index (NaN
//!   knots). In each case the upstream loop state is unchanged, so it would repeat
//!   forever.
//! - `N_BasisDerivs` scales the derivatives by `p!/(p-k)!` in an `int`; where that
//!   product overflows (signed overflow is undefined upstream) Rust returns
//!   [`SolidError::InvalidInput`].
//! - `basis_derivs` limits combined output/workspace storage (including row headers)
//!   to 8 MiB before allocating, returning `NL_MEM_ERR` above that practical bound.
//!   This keeps a single basis evaluation small even on wasm32; fallible reservation
//!   alone cannot prevent memory-overcommit exhaustion when zero-filling huge orders.

use std::collections::TryReserveError;

use crate::error::{NlibCode, Result, SolidError};
use crate::nurbs::geom::Interval;
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{float_op_is_bad, FloatOp};

/// Upstream `NL_PTOL`: parameter-space tolerance (`1.0e-12`).
pub const PTOL: f64 = 1.0e-12;

/// Knot tolerance used by [`compatible_knot_array`] (upstream literal `1.0e-08`).
pub const COMPATIBLE_KNOT_TOL: f64 = 1.0e-08;

/// Error for an array access that upstream would make out of bounds.
fn out_of_range(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "index out of range (knot vector too short or malformed)",
    }
}

/// Error where upstream would loop forever.
fn endless(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "upstream would loop forever on this input",
    }
}

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

/// Combined heap-storage ceiling for a single basis evaluation, not a degree limit.
const BASIS_DERIVS_MAX_BYTES: usize = 8 * 1024 * 1024;

/// Account for `nd`, square `ndu`, two rows of `a`, and `left`/`right` before
/// any allocation. Native-sized row headers also bound metadata-heavy zero rows.
fn check_derivs_allocation(rows: usize, w: usize, op: &'static str) -> Result<()> {
    let bytes = w.checked_mul(size_of::<f64>()).and_then(|row_values| {
        let row_bytes = row_values.checked_add(size_of::<Vec<f64>>())?;
        let all_rows = rows.checked_add(w)?.checked_add(2)?;
        all_rows
            .checked_mul(row_bytes)?
            .checked_add(row_values.checked_mul(2)?)
    });
    match bytes {
        Some(bytes) if bytes <= BASIS_DERIVS_MAX_BYTES => Ok(()),
        _ => Err(nlib(op, NlibCode::Memory)),
    }
}

/// A zero `rows x w` array (upstream `N_AllocReal2dArray(rows - 1, w - 1)`).
///
/// Upstream allocates the row pointers and the values as two blocks and fails softly
/// with `NL_MEM_ERR` when either does not fit. Here the row list and every row are
/// reserved fallibly and filled within that capacity, so any failed allocation
/// returns that error instead of aborting.
fn zeros2(rows: usize, w: usize, op: &'static str) -> Result<Vec<Vec<f64>>> {
    zeros2_with(rows, w, op, Vec::try_reserve_exact)
}

/// [`zeros2`] with the row reservation passed in, so a test can make it fail midway.
fn zeros2_with(
    rows: usize,
    w: usize,
    op: &'static str,
    mut reserve: impl FnMut(&mut Vec<f64>, usize) -> std::result::Result<(), TryReserveError>,
) -> Result<Vec<Vec<f64>>> {
    let mem = |_| nlib(op, NlibCode::Memory);
    let mut a: Vec<Vec<f64>> = Vec::new();
    a.try_reserve_exact(rows).map_err(mem)?;
    for _ in 0..rows {
        let mut r = Vec::new();
        reserve(&mut r, w).map_err(mem)?;
        r.resize(w, 0.0);
        a.push(r);
    }
    Ok(a)
}

/// `a[i]`, or `InvalidInput` when out of range.
#[inline]
fn rd(a: &[f64], i: usize, op: &'static str) -> Result<f64> {
    a.get(i).copied().ok_or_else(|| out_of_range(op))
}

/// `a[i]` for a signed index, or `InvalidInput` when out of range.
#[inline]
fn rdi(a: &[f64], i: i64, op: &'static str) -> Result<f64> {
    let i = usize::try_from(i).map_err(|_| out_of_range(op))?;
    rd(a, i, op)
}

/// `a[i] = v`, or `InvalidInput` when out of range.
#[inline]
fn wr(a: &mut [f64], i: usize, v: f64, op: &'static str) -> Result<()> {
    let slot = a.get_mut(i).ok_or_else(|| out_of_range(op))?;
    *slot = v;
    Ok(())
}

/// `a[i][j]` of a 2-D array, or `InvalidInput`.
#[inline]
fn rd2(a: &[Vec<f64>], i: i64, j: i64, op: &'static str) -> Result<f64> {
    let i = usize::try_from(i).map_err(|_| out_of_range(op))?;
    rdi(a.get(i).ok_or_else(|| out_of_range(op))?, j, op)
}

/// `a[i][j] = v` of a 2-D array, or `InvalidInput`.
#[inline]
fn wr2(a: &mut [Vec<f64>], i: i64, j: i64, v: f64, op: &'static str) -> Result<()> {
    let i = usize::try_from(i).map_err(|_| out_of_range(op))?;
    let j = usize::try_from(j).map_err(|_| out_of_range(op))?;
    wr(a.get_mut(i).ok_or_else(|| out_of_range(op))?, j, v, op)
}

/// Index as `i64` (indices here are bounded by slice lengths).
#[inline]
fn sgn(v: usize, op: &'static str) -> Result<i64> {
    i64::try_from(v).map_err(|_| out_of_range(op))
}

/// Highest knot index `m` of a non-empty knot slice.
fn last_index(knots: &[f64], op: &'static str) -> Result<usize> {
    knots.len().checked_sub(1).ok_or(SolidError::InvalidInput {
        operation: op,
        reason: "empty knot vector",
    })
}

/// `NL_IND_ERR` unless `i <= m - p - 1`.
fn check_index(m: usize, i: usize, p: usize, op: &'static str) -> Result<()> {
    match p.checked_add(1).and_then(|p1| m.checked_sub(p1)) {
        Some(n) if i <= n => Ok(()),
        _ => Err(nlib(op, NlibCode::Index)),
    }
}

/// `N_BasisIDerivs(knt, i, p, u, NL_LEFT, der, ND)`: writes `ND[0..=der]`, leaving the
/// rest of `nd` as it was (upstream reuses one `ND[3]` for `der = 1` and `der = 2`).
fn i_derivs(
    knt: &KnotVector<'_>,
    i: usize,
    p: usize,
    u: f64,
    der: usize,
    nd: &mut [f64; 3],
) -> Result<()> {
    let d = knt.basis_i_derivs(i, p, u, Side::Left, der)?;
    for (slot, v) in nd.iter_mut().zip(d) {
        *slot = v;
    }
    Ok(())
}

/// The global maximum of the basis function `N[i, p]` (upstream
/// `N_BasisFindGlobalMax`): returns `(max, u)` with `N[i, p](u) = max`.
///
/// The maximum is bracketed by stepping `[U[i], U[i+p+1]]` in `10p` steps until the
/// first derivative turns negative, then refined by Newton iteration (converged when
/// `|N'| < tol` and `N > tol`), re-bracketing up to 20 times. Degree 1 and functions
/// with a `p`-fold inner knot peak at `U[i+1]` with value 1.
///
/// # Errors
///
/// [`NlibCode::Index`] when `i > m - p - 1`; [`NlibCode::Numerical`] when the
/// Newton step would overflow; [`NlibCode::Convergence`] after 20 re-brackets; errors
/// of [`KnotVector::basis_i_derivs`]; `InvalidInput` where upstream would index out of
/// bounds or loop forever.
pub fn find_global_max(knt: &KnotVector<'_>, i: usize, p: usize, tol: f64) -> Result<(f64, f64)> {
    const OP: &str = "nurbs::basis_basic::find_global_max";
    const ITL: usize = 20;
    const NOK: usize = 10;

    let uk = knt.knots();
    let m = knt.last_index();
    check_index(m, i, p, OP)?;

    // Consider linear as special case.
    if p == 1 {
        return Ok((1.0, rd(uk, i + 1, OP)?));
    }

    // Check if a p-fold inner multiple knot exists.
    let mut s = 1;
    for k in (i + 1)..(i + p) {
        if rd(uk, k, OP)? == rd(uk, k + 1, OP)? {
            s += 1;
        }
    }
    if s == p {
        return Ok((1.0, rd(uk, i + 1, OP)?));
    }

    // Get guess parameter.
    let ui = rd(uk, i, OP)?;
    let ue = rd(uk, i + p + 1, OP)?;
    let nos = p * NOK;
    let mut du = (ue - ui) / nos as f64;
    let mut ur = ui;
    let mut ul = ui;
    let mut nd = [0.0f64; 3];

    while ur < ue {
        ul = ur;
        ur += du;
        if ur > ue {
            ur = ue;
        }
        i_derivs(knt, i, p, ur, 1, &mut nd)?;
        if nd[1] < 0.0 {
            break;
        }
        if ur == ul {
            // The step no longer advances: upstream repeats this iteration forever.
            return Err(endless(OP));
        }
    }

    // Perform Newton iterations till convergence reached.
    let mut conv = false;
    let mut u0 = 0.0;
    let mut it = 0;
    while it < ITL {
        u0 = 0.5 * (ul + ur);

        // Do Newton with guess parameter.
        let mut k = 0;
        while k < ITL {
            i_derivs(knt, i, p, u0, 2, &mut nd)?;
            if nd[1].abs() < tol && nd[0] > tol {
                conv = true;
                break;
            }
            if float_op_is_bad(nd[1], nd[2], FloatOp::Division) {
                return Err(nlib(OP, NlibCode::Numerical));
            }
            u0 -= nd[1] / nd[2];
            if u0 <= ui || u0 >= ue {
                break;
            }
            k += 1;
        }
        if conv {
            break;
        }

        // No convergence -> refine [ul,ur] and get a better guess.
        du = (ur - ul) / NOK as f64;
        ur = ul;
        for _ in 1..=NOK {
            ul = ur;
            ur += du;
            i_derivs(knt, i, p, ur, 1, &mut nd)?;
            if nd[1] < 0.0 {
                break;
            }
        }
        it += 1;
    }

    // If no convergence, quit.
    if it >= ITL {
        return Err(nlib(OP, NlibCode::Convergence));
    }

    // Convergence reached -> get maximum.
    Ok((nd[0], u0))
}

/// Per-span extrema of one basis function, as returned by [`find_all_span_maxima`].
#[derive(Debug, Clone, PartialEq)]
pub struct SpanMaxima {
    /// `min[k]`: minimum of `N[i, p]` over `[U[i+k], U[i+k+1]]`, `k = 0..=p`.
    pub min: Vec<f64>,
    /// `max[k]`: maximum of `N[i, p]` over `[U[i+k], U[i+k+1]]`, `k = 0..=p`.
    pub max: Vec<f64>,
    /// Parameter of the global maximum.
    pub u: f64,
}

/// The minimum and maximum of the basis function `N[i, p]` over each of its `p + 1`
/// knot spans, plus the parameter of its global maximum (upstream
/// `N_BasisFindAllSpanMaxima`).
///
/// # Errors
///
/// [`NlibCode::Index`] when `i > m - p - 1`; [`NlibCode::Convergence`] when
/// the global maximum lies in no span of the function's support; errors of
/// [`find_global_max`] and [`KnotVector::basis_i_eval`].
pub fn find_all_span_maxima(
    knt: &KnotVector<'_>,
    i: usize,
    p: usize,
    tol: f64,
) -> Result<SpanMaxima> {
    const OP: &str = "nurbs::basis_basic::find_all_span_maxima";
    let uk = knt.knots();
    let m = knt.last_index();
    check_index(m, i, p, OP)?;

    let mut min = vec![0.0; p + 1];
    let mut max = vec![0.0; p + 1];

    // Consider linear as special case.
    if p == 1 {
        min = vec![0.0, 0.0];
        max = vec![1.0, 1.0];
        let u = rd(uk, i + 1, OP)?;
        return Ok(SpanMaxima { min, max, u });
    }

    // Check if a p-fold inner multiple knot exists.
    let mut s = 1;
    for k in (i + 1)..(i + p) {
        if rd(uk, k, OP)? == rd(uk, k + 1, OP)? {
            s += 1;
        }
    }
    if s == p {
        max.fill(1.0);
        for k in 1..p {
            wr(&mut min, k, 1.0, OP)?;
        }
        let v0 = if rd(uk, i, OP)? == rd(uk, i + 1, OP)? {
            1.0
        } else {
            0.0
        };
        wr(&mut min, 0, v0, OP)?;
        let vp = if rd(uk, i + p, OP)? == rd(uk, i + p + 1, OP)? {
            1.0
        } else {
            0.0
        };
        wr(&mut min, p, vp, OP)?;
        let u = rd(uk, i + 1, OP)?;
        return Ok(SpanMaxima { min, max, u });
    }

    // Get global maximum.
    let (nm, u) = find_global_max(knt, i, p, tol)?;

    // Get span index of global maximum.
    let mut spn = None;
    for k in i..=(i + p) {
        if u >= rd(uk, k, OP)? && u < rd(uk, k + 1, OP)? {
            spn = Some(k);
            break;
        }
    }
    let spn = spn.ok_or_else(|| nlib(OP, NlibCode::Convergence))?;
    let off = spn - i;

    let n_at = |x: f64| knt.basis_i_eval(i, p, x, Side::Left);

    // Compute local extrema.
    for k in 0..off {
        wr(&mut min, k, n_at(rd(uk, i + k, OP)?)?, OP)?;
        wr(&mut max, k, n_at(rd(uk, i + k + 1, OP)?)?, OP)?;
    }
    wr(&mut max, off, nm, OP)?;
    let nl = n_at(rd(uk, spn, OP)?)?;
    let nr = n_at(rd(uk, spn + 1, OP)?)?;
    wr(&mut min, off, if nl < nr { nl } else { nr }, OP)?;
    for k in (off + 1)..=p {
        wr(&mut min, k, n_at(rd(uk, i + k + 1, OP)?)?, OP)?;
        wr(&mut max, k, n_at(rd(uk, i + k, OP)?)?, OP)?;
    }

    Ok(SpanMaxima { min, max, u })
}

/// Rescales the knot vector `U[0..=m]` in place to `interval` (upstream
/// `N_BasisReparam`): the first `p + 1` knots become `interval.ul`, the last `p + 1`
/// become `interval.ur`, and the inner knots are mapped linearly from
/// `[U[0], U[m]]`. Nothing changes when the ends already match.
///
/// # Errors
///
/// `InvalidInput` when `knots` is empty or has fewer than `p + 1` knots
/// (upstream would write out of bounds).
pub fn reparam(knots: &mut [f64], p: usize, interval: Interval) -> Result<()> {
    const OP: &str = "nurbs::basis_basic::reparam";
    let m = last_index(knots, OP)?;
    if p > m {
        return Err(out_of_range(OP));
    }
    let Interval { ul, ur } = interval;
    let first = rd(knots, 0, OP)?;
    let last = rd(knots, m, OP)?;

    // Compute new knots.
    if first != ul || last != ur {
        let u0 = first;
        let fac = (ur - ul) / (last - first);
        for i in 0..=p {
            wr(knots, i, ul, OP)?;
        }
        for i in (p + 1)..(m - p) {
            let v = rd(knots, i, OP)?;
            wr(knots, i, ul + fac * (v - u0), OP)?;
        }
        for i in (m - p)..=m {
            wr(knots, i, ur, OP)?;
        }
    }
    Ok(())
}

/// [`compatible_knot_vectors_to_tol`] with the knot tolerance
/// [`COMPATIBLE_KNOT_TOL`] (upstream `N_GetCompatibleKnotArray`).
///
/// # Errors
///
/// As [`compatible_knot_vectors_to_tol`].
pub fn compatible_knot_array(knots: &mut [Vec<f64>]) -> Result<Vec<Vec<f64>>> {
    // Pass the call along.
    compatible_knot_vectors_to_tol(knots, COMPATIBLE_KNOT_TOL)
}

/// `true` while some knot index is below its vector's highest index (upstream
/// `ST_notdone`).
fn not_done(knot_indices: &[i64], knots: &[Vec<f64>]) -> bool {
    knot_indices
        .iter()
        .zip(knots)
        .any(|(&ki, u)| ki < u.len() as i64 - 1)
}

/// Merges a set of knot vectors (upstream `N_GetCompatibleKnotVectorToTol`): returns,
/// for each input vector, the knots to insert into it so that all vectors have the
/// same distinct knots, each with the highest multiplicity it has in any vector.
///
/// Knots closer than `tol` (reduced to `0.99999` times the next gap where that gap is
/// smaller) count as one knot: they are moved, **in place in `knots`**, to their
/// average, and the inserted copies take that average value.
///
/// # Errors
///
/// [`NlibCode::Input`] when a knot index runs past a vector's end (malformed
/// input); `InvalidInput` for an empty set or an empty vector, where upstream would
/// write more new knots than it allocated, or would loop forever (an iteration that
/// matches no knot: `tol <= 0` or NaN knots).
pub fn compatible_knot_vectors_to_tol(knots: &mut [Vec<f64>], tol: f64) -> Result<Vec<Vec<f64>>> {
    const OP: &str = "nurbs::basis_basic::compatible_knot_vectors_to_tol";
    if knots.is_empty() {
        return Err(SolidError::InvalidInput {
            operation: OP,
            reason: "no knot vectors",
        });
    }
    let mut ms = Vec::with_capacity(knots.len());
    for u in knots.iter() {
        ms.push(sgn(last_index(u, OP)?, OP)?);
    }

    // Each new knot vector has room for the total number of knots (upstream
    // allocates s + 1 entries, s = sum of the m's).
    let s_total: i64 = ms.iter().sum();
    let cap = usize::try_from(s_total + 1).map_err(|_| out_of_range(OP))?;

    let nvec = knots.len();
    let mut knot_indices = vec![0i64; nvec];
    let mut mlt = vec![0i64; nvec];
    let mut kna: Vec<Vec<f64>> = vec![Vec::new(); nvec];

    // Each iteration finds the next lowest knot in any of the vectors and records its
    // multiplicity in each of them.
    while not_done(&knot_indices, knots) {
        // Smallest next knot value in any vector, and the gap after it in that vector.
        let mut us = 0.0;
        let mut gap_size = -1.0;
        for (r, (u, (&ki, &m))) in knots
            .iter()
            .zip(knot_indices.iter().zip(ms.iter()))
            .enumerate()
        {
            if ki > m {
                return Err(nlib(OP, NlibCode::Input));
            }
            let uki = rdi(u, ki, OP)?;
            if r == 0 || uki < us {
                us = uki;
                // Use <= not ==: allow for slightly out-of-order knot values. [B168]
                let mut knot_index = ki + 1;
                while knot_index <= m && rdi(u, knot_index, OP)? <= uki {
                    knot_index += 1;
                }
                gap_size = if knot_index <= m {
                    rdi(u, knot_index, OP)? - uki
                } else {
                    -1.0
                };
            }
        }

        // Watch out for gaps smaller than tol: reduce tol below the next gap size.
        let u_tol = if gap_size < tol && gap_size != -1.0 {
            0.99999 * gap_size
        } else {
            tol
        };

        // Compute multiplicities.
        for (u, ((ki, ml), &m)) in knots
            .iter()
            .zip(knot_indices.iter_mut().zip(mlt.iter_mut()).zip(ms.iter()))
        {
            if (rdi(u, *ki, OP)? - us).abs() < u_tol {
                let s = *ki;
                // Use <= not ==: allow for slightly out-of-order knot values. [B168]
                while *ki < m && rdi(u, *ki + 1, OP)? <= rdi(u, *ki, OP)? {
                    *ki += 1;
                }
                *ml = *ki - s + 1;
            } else {
                *ml = 0;
                *ki -= 1;
            }
        }
        if mlt.iter().all(|&ml| ml == 0) {
            // Nothing matched: the next iteration would start from the same state.
            return Err(endless(OP));
        }

        // Adjust knots: average the next knots within u_tol of the smallest one.
        let mut num_ave = 0i64;
        let mut u_ave = 0.0;
        for (u, (&ki, &ml)) in knots.iter().zip(knot_indices.iter().zip(mlt.iter())) {
            if ml > 0 {
                let v = rdi(u, ki, OP)?;
                if (v - us).abs() < u_tol {
                    u_ave += v;
                    num_ave += 1;
                }
            }
        }
        us = u_ave / num_ave as f64;

        // Move all next knots within u_tol of the average to the average. (Upstream
        // also tests vectors with no match, at index ki = -1 or the previous knot; it
        // then moves nothing.)
        for (u, (&ki, &ml)) in knots.iter_mut().zip(knot_indices.iter().zip(mlt.iter())) {
            if ml > 0 && (rdi(u, ki, OP)? - us).abs() < u_tol {
                for s in 1..=ml {
                    let at = usize::try_from(ki - ml + s).map_err(|_| out_of_range(OP))?;
                    wr(u, at, us, OP)?;
                }
            }
        }

        // Get the highest multiplicity.
        let mh = mlt.iter().copied().max().unwrap_or(0);

        // Load needed new knots into the output vectors.
        for (x, (ki, &ml)) in kna.iter_mut().zip(knot_indices.iter_mut().zip(mlt.iter())) {
            for _ in (ml + 1)..=mh {
                if x.len() >= cap {
                    return Err(out_of_range(OP));
                }
                x.push(us);
            }
            *ki += 1;
        }
    }

    Ok(kna)
}

/// All non-vanishing basis functions and their derivatives at one parameter, as
/// returned by [`basis_derivs`].
#[derive(Debug, Clone, PartialEq)]
pub struct BasisDerivs {
    /// Knot span index `j` of `u` (see [`KnotVector::find_span`]).
    pub span: usize,
    /// `derivs[k][r]`: `k`-th derivative of `N[j-p+r, p]` at `u`, `k = 0..=der`,
    /// `r = 0..=p` (rows `k > p` are zero).
    pub derivs: Vec<Vec<f64>>,
}

/// All non-vanishing basis functions of degree `p` and their derivatives up to order
/// `der` at `u` (upstream `N_BasisDerivs`, The NURBS Book A2.3).
///
/// # Errors
///
/// [`NlibCode::Numerical`] when a knot difference is below `1e-24` (more than
/// `p + 1` equal knots); [`NlibCode::Memory`] when the `(der + 1) x (p + 1)` or
/// `(p + 1) x (p + 1)` arrays cannot be allocated, their sizes overflow, or combined
/// output/workspace storage (values and row headers) exceeds the practical 8 MiB
/// budget, even if the allocator could reserve it; errors of
/// [`KnotVector::find_span`]; `InvalidInput` where upstream would index out of bounds
/// or overflow its `int` derivative factor.
#[allow(clippy::too_many_lines)] // one upstream routine, kept in its original shape
pub fn basis_derivs(
    knt: &KnotVector<'_>,
    p: usize,
    u: f64,
    side: Side,
    der: usize,
) -> Result<BasisDerivs> {
    const OP: &str = "nurbs::basis_basic::basis_derivs";
    // Guard against an exactly zero (or tiny) denominator.
    let d_zero_tol = 1e-24;

    let uk = knt.knots();

    // Find the knot span u is in.
    let i = knt.find_span(p, u, side)?;
    let is = sgn(i, OP)?;
    let ps = sgn(p, OP)?;

    // Get maximum derivative index; rows above p stay zero.
    let mder = sgn(p.min(der), OP)?;
    let rows = der
        .checked_add(1)
        .ok_or_else(|| nlib(OP, NlibCode::Memory))?;
    let w = p.checked_add(1).ok_or_else(|| out_of_range(OP))?;
    check_derivs_allocation(rows, w, OP)?;
    let mut nd = zeros2(rows, w, OP)?;

    let mut left = crate::limits::try_filled(w, 0.0).ok_or_else(|| nlib(OP, NlibCode::Memory))?;
    let mut right = crate::limits::try_filled(w, 0.0).ok_or_else(|| nlib(OP, NlibCode::Memory))?;
    let mut ndu = zeros2(w, w, OP)?;
    let mut a = zeros2(2, w, OP)?;

    // Compute the basis functions.
    wr2(&mut ndu, 0, 0, 1.0, OP)?;
    for j in 1..=ps {
        let lj = u - rdi(uk, is + 1 - j, OP)?;
        let rj = rdi(uk, is + j, OP)? - u;
        let ju = usize::try_from(j).map_err(|_| out_of_range(OP))?;
        wr(&mut left, ju, lj, OP)?;
        wr(&mut right, ju, rj, OP)?;
        let mut saved = 0.0;
        for r in 0..j {
            let rr = rdi(&right, r + 1, OP)?;
            let ll = rdi(&left, j - r, OP)?;
            let delta = rr + ll;
            wr2(&mut ndu, j, r, delta, OP)?;
            if delta < d_zero_tol {
                return Err(nlib(OP, NlibCode::Numerical));
            }
            // Keep the divisions inside the parentheses: dividing ndu[][] by the knot
            // difference can be huge.
            let prev = rd2(&ndu, r, j - 1, OP)?;
            wr2(&mut ndu, r, j, saved + prev * (rr / delta), OP)?;
            saved = prev * (ll / delta);
        }
        wr2(&mut ndu, j, j, saved, OP)?;
    }

    // Load the basis functions.
    for j in 0..=ps {
        let v = rd2(&ndu, j, ps, OP)?;
        wr2(&mut nd, 0, j, v, OP)?;
    }

    // Compute derivatives.
    for r in 0..=ps {
        let mut s1: i64 = 0;
        let mut s2: i64 = 1;
        wr2(&mut a, 0, 0, 1.0, OP)?;

        for k in 1..=mder {
            let mut d = 0.0;
            let rk = r - k;
            let pk = ps - k;

            if r >= k {
                let v = rd2(&a, s1, 0, OP)? / rd2(&ndu, pk + 1, rk, OP)?;
                wr2(&mut a, s2, 0, v, OP)?;
                d = v * rd2(&ndu, rk, pk, OP)?;
            }

            let j1 = if rk >= -1 { 1 } else { -rk };
            let j2 = if r - 1 <= pk { k - 1 } else { ps - r };

            for j in j1..=j2 {
                let v = (rd2(&a, s1, j, OP)? - rd2(&a, s1, j - 1, OP)?)
                    / rd2(&ndu, pk + 1, rk + j, OP)?;
                wr2(&mut a, s2, j, v, OP)?;
                d += v * rd2(&ndu, rk + j, pk, OP)?;
            }

            if r <= pk {
                let v = -rd2(&a, s1, k - 1, OP)? / rd2(&ndu, pk + 1, r, OP)?;
                wr2(&mut a, s2, k, v, OP)?;
                d += v * rd2(&ndu, r, pk, OP)?;
            }
            wr2(&mut nd, k, r, d, OP)?;
            std::mem::swap(&mut s1, &mut s2);
        }
    }

    // Multiply through by the correct factors (an `int` upstream).
    let overflow = || SolidError::InvalidInput {
        operation: OP,
        reason: "derivative factor overflows upstream int",
    };
    let mut r = i32::try_from(p).map_err(|_| overflow())?;
    for k in 1..=mder {
        let row = usize::try_from(k).map_err(|_| out_of_range(OP))?;
        let f = f64::from(r);
        for v in nd.get_mut(row).ok_or_else(|| out_of_range(OP))? {
            *v *= f;
        }
        let pk = i32::try_from(ps - k).map_err(|_| overflow())?;
        r = r.checked_mul(pk).ok_or_else(overflow)?;
    }

    Ok(BasisDerivs {
        span: i,
        derivs: nd,
    })
}

/// Computes the knots that make two knot vectors compatible (upstream
/// `N_MakeKnotsCompatible`): returns `(xr, xs)`, the knots to insert into `r` and `s`
/// so that both have the same distinct inner knots (multiplicities may differ).
///
/// When the ends of `r` and `s` differ by more than [`PTOL`], the vector with the
/// shorter parameter range is first rescaled **in place** ([`reparam`]) to the range
/// of the other (`s` when `r`'s range is longer, else `r`). Inner knots closer than
/// [`PTOL`] count as equal.
///
/// # Errors
///
/// `InvalidInput` where upstream would read outside the knot vectors
/// (vectors that are too short, unclamped, or with inner knots beyond the ends;
/// upstream-issues UB-13) or loop forever (NaN knots), for a degree too large for
/// the `i64` start index `p + 1` (upstream's degree is a `short`), and for the errors
/// of [`reparam`].
pub fn make_knots_compatible(
    r: &mut [f64],
    s: &mut [f64],
    p: usize,
    q: usize,
) -> Result<(Vec<f64>, Vec<f64>)> {
    const OP: &str = "nurbs::basis_basic::make_knots_compatible";
    let mr = last_index(r, OP)?;
    let ms = last_index(s, OP)?;

    // Scale.
    let (r0, rm) = (rd(r, 0, OP)?, rd(r, mr, OP)?);
    let (s0, sm) = (rd(s, 0, OP)?, rd(s, ms, OP)?);
    if (r0 - s0).abs() > PTOL || (rm - sm).abs() > PTOL {
        if (rm - r0) > (sm - s0) {
            reparam(s, q, Interval::new(r0, rm))?;
        } else {
            reparam(r, p, Interval::new(s0, sm))?;
        }
    }

    // Checked: degrees past `i64::MAX - 1` (far beyond upstream's `short`) would
    // overflow the start indices.
    let too_large = || SolidError::InvalidInput {
        operation: OP,
        reason: "degree too large",
    };
    let (ps, qs) = (sgn(p, OP)?, sgn(q, OP)?);
    let nr = sgn(mr, OP)?
        .checked_sub(ps)
        .and_then(|v| v.checked_sub(1))
        .ok_or_else(too_large)?;
    let ns = sgn(ms, OP)?
        .checked_sub(qs)
        .and_then(|v| v.checked_sub(1))
        .ok_or_else(too_large)?;
    let mut i = ps.checked_add(1).ok_or_else(too_large)?;
    let mut j = qs.checked_add(1).ok_or_else(too_large)?;
    let mut xr = Vec::new();
    let mut xs = Vec::new();
    let (r, s) = (&*r, &*s);

    // Find knots to be inserted.
    while i <= nr || j <= ns {
        let (i_start, j_start) = (i, j);
        if (rdi(r, i, OP)? - rdi(s, j, OP)?).abs() < PTOL {
            while i <= nr && rdi(r, i, OP)? == rdi(r, i + 1, OP)? {
                i += 1;
            }
            while j <= ns && rdi(s, j, OP)? == rdi(s, j + 1, OP)? {
                j += 1;
            }
            i += 1;
            j += 1;
        }

        if rdi(r, i, OP)? < rdi(s, j, OP)? {
            while i <= nr && rdi(r, i, OP)? == rdi(r, i + 1, OP)? {
                i += 1;
            }
            xs.push(rdi(r, i, OP)?);
            i += 1;
        }

        if rdi(r, i, OP)? > rdi(s, j, OP)? {
            while j <= ns && rdi(s, j, OP)? == rdi(s, j + 1, OP)? {
                j += 1;
            }
            xr.push(rdi(s, j, OP)?);
            j += 1;
        }

        if i == i_start && j == j_start {
            // Neither index moved: upstream repeats this iteration forever.
            return Err(endless(OP));
        }
    }

    Ok((xr, xs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(k: &[f64]) -> KnotVector<'_> {
        KnotVector::from_slice(k).unwrap()
    }

    #[test]
    fn derivs_partition_of_unity_and_zero_derivative_sum() {
        let k = [0.0, 0.0, 0.0, 0.0, 0.3, 0.5, 0.5, 0.7, 1.0, 1.0, 1.0, 1.0];
        let b = basis_derivs(&kv(&k), 3, 0.42, Side::Left, 5).unwrap();
        assert_eq!(b.span, 4);
        assert_eq!(b.derivs.len(), 6);
        let sum: f64 = b.derivs[0].iter().sum();
        assert!((sum - 1.0).abs() < 1e-14);
        for row in &b.derivs[1..=3] {
            let s: f64 = row.iter().sum();
            assert!(s.abs() < 1e-9, "{s}");
        }
        assert!(b.derivs[4].iter().chain(&b.derivs[5]).all(|&v| v == 0.0));
    }

    #[test]
    fn derivs_match_single_function_derivs() {
        let k = [0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 4.0, 5.0, 5.0, 5.0];
        let knt = kv(&k);
        let b = basis_derivs(&knt, 2, 2.5, Side::Left, 2).unwrap();
        for r in 0..=2 {
            let one = knt
                .basis_i_derivs(b.span - 2 + r, 2, 2.5, Side::Left, 2)
                .unwrap();
            for (d, v) in one.iter().enumerate() {
                assert!((v - b.derivs[d][r]).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn derivs_degree_zero_and_errors() {
        let k = [0.0, 1.0, 2.0];
        let b = basis_derivs(&kv(&k), 0, 0.5, Side::Left, 2).unwrap();
        assert_eq!(b.derivs, vec![vec![1.0], vec![0.0], vec![0.0]]);
        // Too short for the degree.
        assert!(matches!(
            basis_derivs(&kv(&[0.0, 1.0]), 3, 0.5, Side::Left, 1),
            Err(SolidError::InvalidInput { .. })
        ));
        // Factor overflow (13!/0! does not fit an i32) is rejected.
        let mut long = vec![0.0; 14];
        long.extend(vec![1.0; 14]);
        assert!(matches!(
            basis_derivs(&kv(&long), 13, 0.5, Side::Left, 13),
            Err(SolidError::InvalidInput { .. })
        ));
        // ... including the unused final product (13!/2! at k = 10).
        assert!(basis_derivs(&kv(&long), 13, 0.5, Side::Left, 10).is_err());
        assert!(basis_derivs(&kv(&long), 13, 0.5, Side::Left, 9).is_ok());
    }

    /// A derivative order too large to allocate is `NL_MEM_ERR`, as upstream's
    /// `N_AllocReal2dArray`, not a capacity-overflow panic or an abort (issue #206).
    #[test]
    fn derivs_huge_order_is_memory_error() {
        let k = [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        for der in [1 << 56, 1 << 60, i64::MAX as usize, usize::MAX / 2] {
            assert!(
                matches!(
                    basis_derivs(&kv(&k), 3, 0.5, Side::Left, der),
                    Err(SolidError::Nlib {
                        code: NlibCode::Memory,
                        ..
                    })
                ),
                "der = {der}"
            );
        }
    }

    #[test]
    fn derivs_representable_order_and_square_workspace_are_bounded() {
        let cubic = [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        // Previously each reservation could succeed under overcommit (>100 GiB).
        assert!(matches!(
            basis_derivs(&kv(&cubic), 3, 0.5, Side::Left, 2_147_483_648),
            Err(SolidError::Nlib {
                code: NlibCode::Memory,
                ..
            })
        ));
        assert!(matches!(
            basis_derivs(&kv(&cubic), 3, 0.5, Side::Left, usize::MAX),
            Err(SolidError::Nlib {
                code: NlibCode::Memory,
                ..
            })
        ));
        let mut square = vec![0.0; 1024];
        square.extend(vec![1.0; 1024]);
        // A tiny output does not excuse the square workspace from the budget.
        assert!(matches!(
            basis_derivs(&kv(&square), 1023, 0.5, Side::Left, 0),
            Err(SolidError::Nlib {
                code: NlibCode::Memory,
                ..
            })
        ));
    }

    #[test]
    fn derivs_budget_includes_metadata_and_all_workspace_at_boundary() {
        let row = size_of::<f64>() + size_of::<Vec<f64>>();
        let workspace = 3 * row + 2 * size_of::<f64>();
        let max_rows = (BASIS_DERIVS_MAX_BYTES - workspace) / row;
        assert!(check_derivs_allocation(max_rows, 1, "test").is_ok());
        for (rows, w) in [
            (max_rows + 1, 1),
            (BASIS_DERIVS_MAX_BYTES / size_of::<f64>(), 1),
            (1, 1024),
            (usize::MAX, 1),
            (1, usize::MAX),
            (usize::MAX / 2, 2),
        ] {
            assert!(
                matches!(
                    check_derivs_allocation(rows, w, "test"),
                    Err(SolidError::Nlib {
                        code: NlibCode::Memory,
                        ..
                    })
                ),
                "rows={rows}, w={w}"
            );
        }
    }

    #[test]
    fn derivs_bounded_high_orders_preserve_full_output_shape() {
        for (knots, p) in [
            (&[0.0, 1.0][..], 0),
            (&[0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0][..], 3),
        ] {
            let b = basis_derivs(&kv(knots), p, 0.5, Side::Left, 4096).unwrap();
            assert_eq!(b.derivs.len(), 4097);
            assert!(b.derivs.iter().all(|row| row.len() == p + 1));
            assert!(b.derivs.iter().skip(p + 1).flatten().all(|&v| v == 0.0));
            assert_eq!(b.derivs[0].iter().sum::<f64>(), 1.0);
        }
    }

    /// A row allocation that fails after earlier ones succeeded is `NL_MEM_ERR` too, not
    /// an abort (review of PR #214).
    #[test]
    fn zeros2_row_failure_midway_is_memory_error() {
        for fail_at in 1..=4 {
            let mut calls = 0;
            let got = zeros2_with(4, 4, "test", |r, n| {
                calls += 1;
                if calls == fail_at {
                    // A real reservation error, without allocating.
                    Vec::<f64>::new().try_reserve(usize::MAX)
                } else {
                    r.try_reserve_exact(n)
                }
            });
            assert!(
                matches!(
                    got,
                    Err(SolidError::Nlib {
                        code: NlibCode::Memory,
                        ..
                    })
                ),
                "fail_at = {fail_at}"
            );
            assert_eq!(calls, fail_at);
        }
        assert_eq!(zeros2(4, 3, "test").unwrap(), vec![vec![0.0; 3]; 4]);
    }

    #[test]
    fn global_max_cubic_bezier_middle() {
        // N[1,3] of the cubic Bezier basis peaks at u = 1/3 with 4/9.
        let k = [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let (max, u) = find_global_max(&kv(&k), 1, 3, 1e-12).unwrap();
        assert!((u - 1.0 / 3.0).abs() < 1e-9);
        assert!((max - 4.0 / 9.0).abs() < 1e-12);
        // Index out of range.
        assert!(matches!(
            find_global_max(&kv(&k), 4, 3, 1e-12),
            Err(SolidError::Nlib {
                code: NlibCode::Index,
                ..
            })
        ));
        // Linear and p-fold knots.
        assert_eq!(find_global_max(&kv(&k), 0, 1, 1e-9).unwrap(), (1.0, 0.0));
        assert_eq!(find_global_max(&kv(&k), 0, 3, 1e-9).unwrap(), (1.0, 0.0));
    }

    #[test]
    fn global_max_stalled_step_is_an_error() {
        let k = [
            1e17,
            1e17,
            1e17,
            1e17 + 16.0,
            1e17 + 32.0,
            1e17 + 32.0,
            1e17 + 32.0,
        ];
        assert!(matches!(
            find_global_max(&kv(&k), 1, 2, 1e-12),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn span_maxima_shapes() {
        let k = [0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 3.0, 3.0];
        let sm = find_all_span_maxima(&kv(&k), 1, 2, 1e-12).unwrap();
        assert_eq!(sm.min.len(), 3);
        assert_eq!(sm.max.len(), 3);
        assert!(sm.max.iter().all(|&v| v <= 1.0));
        let lin = find_all_span_maxima(&kv(&k), 1, 1, 1e-12).unwrap();
        assert_eq!(lin.max, vec![1.0, 1.0]);
    }

    #[test]
    fn reparam_maps_ends_and_inner() {
        let mut k = vec![2.0, 2.0, 3.0, 4.0, 6.0, 6.0];
        reparam(&mut k, 1, Interval::new(0.0, 1.0)).unwrap();
        assert_eq!(k, vec![0.0, 0.0, 0.25, 0.5, 1.0, 1.0]);
        let before = k.clone();
        reparam(&mut k, 1, Interval::new(0.0, 1.0)).unwrap();
        assert_eq!(k, before);
        assert!(reparam(&mut [0.0, 1.0], 2, Interval::new(0.0, 2.0)).is_err());
        assert!(reparam(&mut [], 0, Interval::new(0.0, 2.0)).is_err());
    }

    #[test]
    fn compatible_doc_example() {
        // Example from the upstream N_GetCompatibleKnotVector documentation.
        let mut k = vec![
            vec![0., 0., 0., 0.2, 0.5, 0.5, 1., 1., 1.],
            vec![
                0., 0., 0., 0., 0.2, 0.2, 0.4, 0.4, 0.4, 0.7, 0.7, 1., 1., 1., 1.,
            ],
            vec![0., 0., 0., 0.3, 0.3, 0.3, 0.8, 0.8, 1., 1., 1.],
        ];
        let x = compatible_knot_array(&mut k).unwrap();
        // Multiplicities are carried over (unlike the doc comment says).
        assert_eq!(
            x[0],
            vec![0.0, 0.2, 0.3, 0.3, 0.3, 0.4, 0.4, 0.4, 0.7, 0.7, 0.8, 0.8, 1.0]
        );
        assert!(x.iter().all(|v| v.windows(2).all(|w| w[0] <= w[1])));
    }

    #[test]
    fn compatible_tolerant_merge_and_endless() {
        let mut k = vec![
            vec![0., 0., 0.40200000001, 0.40200000001, 1., 1.],
            vec![0., 0., 0.402, 1., 1.],
        ];
        let x = compatible_knot_vectors_to_tol(&mut k, 1e-7).unwrap();
        assert_eq!(k[0][2], k[1][2]);
        assert_eq!(x[1], vec![k[1][2]]);
        assert!(x[0].is_empty());
        let mut k = vec![vec![0., 0., 1., 1.]];
        assert!(matches!(
            compatible_knot_vectors_to_tol(&mut k, 0.0),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(compatible_knot_vectors_to_tol(&mut [], 1e-8).is_err());
        let mut single = vec![vec![1.0]];
        assert_eq!(
            compatible_knot_vectors_to_tol(&mut single, 0.0).unwrap(),
            vec![Vec::<f64>::new()]
        );
    }

    #[test]
    fn compatible_different_ranges_is_input_error() {
        // Upstream assumes a common parameter range: the shorter vector runs out.
        let mut k = vec![vec![0., 0., 1., 1.], vec![0., 0., 0.5, 2., 2.]];
        assert!(matches!(
            compatible_knot_array(&mut k),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn make_compatible_doc_example() {
        let mut r = vec![0., 0., 0., 0., 0.3, 0.5, 0.5, 0.7, 1., 1., 1., 1.];
        let mut s = vec![0., 0., 0., 0.2, 0.2, 0.5, 0.6, 1., 1., 1.];
        let (xr, xs) = make_knots_compatible(&mut r, &mut s, 3, 2).unwrap();
        assert_eq!(xr, vec![0.2, 0.6]);
        assert_eq!(xs, vec![0.3, 0.7]);
    }

    #[test]
    fn make_compatible_rescales_shorter_range() {
        let mut r = vec![0., 0., 1., 2., 2.];
        let mut s = vec![0., 0., 0.25, 1., 1.];
        let (xr, xs) = make_knots_compatible(&mut r, &mut s, 1, 1).unwrap();
        assert_eq!(s, vec![0., 0., 0.5, 2., 2.]);
        assert_eq!(xr, vec![0.5]);
        assert_eq!(xs, vec![1.0]);
        let mut a = vec![0., f64::NAN, 1.];
        let mut b = vec![0., 0.5, 1.];
        assert!(make_knots_compatible(&mut a, &mut b, 0, 0).is_err());
    }

    #[test]
    fn make_compatible_huge_degree_is_an_error_not_a_panic() {
        let mut r = vec![1., 1., 1., 1., 2., 2., 2., 2.];
        let mut s = vec![1., 2.];
        let huge = usize::try_from(i64::MAX).unwrap();
        for (p, q) in [(3, huge), (huge, 0), (usize::MAX, 3)] {
            assert!(matches!(
                make_knots_compatible(&mut r, &mut s, p, q),
                Err(SolidError::InvalidInput { .. })
            ));
        }
        let k = [0., 0., 1., 1.];
        assert!(find_global_max(&kv(&k), 0, usize::MAX, 1e-9).is_err());
    }
}
