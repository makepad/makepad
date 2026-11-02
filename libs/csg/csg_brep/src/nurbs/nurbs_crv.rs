//! SMLib's local copies of NLib curve routines: unique knots and multiplicities, knot
//! spreading, Bezier extraction, split and trim, basis functions and their
//! derivatives, point and derivative evaluation, knot removal, basis function maxima
//! and the degenerate-curve test.
//!
//! Upstream: `source/SMLib/src/SmNurbsCrv.cpp`, `source/SMLib/inc/SmNurbsCrv.h`.
//!
//! | Rust                               | Upstream                                    |
//! |------------------------------------|---------------------------------------------|
//! | [`extract_frame`]                  | `sm_ExtractFrame`                           |
//! | [`get_knots`]                      | `sm_GetKnots`                               |
//! | [`copy_nurb_curve`]                | `sm_CopyNurbCurve`, `sm_AllocateAndCopyNurbCurve` |
//! | [`allocate_nurb_curve`]            | `sm_AllocateNurbCurve`, `sm_InitNurbCurveMemory` |
//! | `Drop` of [`Curve`]                | `sm_FreeNurbCurve`, `SmStackHandler::~SmStackHandler` |
//! | (none: Rust owns memory)           | `SmStackHandler::SmStackHandler`, `sm_ComputeNurbCurveSize` (both overloads) |
//! | [`fixup_knot_vector`]              | `sm_FixupKnotVector`                        |
//! | [`create_bez_segments`]            | `sm_CreateBezSegments`                      |
//! | [`crv_split`]                      | `sm_CrvSplit`                               |
//! | [`crv_trim`]                       | `sm_CrvTrim`                                |
//! | [`crv_get_epolygon`]               | `sm_CrvGetEPolygon`                         |
//! | [`basis_derivs`], [`BasisDerivs`]  | `sm_BasisDerivs`                            |
//! | [`crv_rational_basis_derivs`]      | `sm_CrvRationalBasisDerivs`                 |
//! | [`crv_derivs`], [`crv_derivs_into`] | `sm_CrvDerivs`                            |
//! | [`basis_eval`]                     | `sm_BasisEval`                              |
//! | [`crv_eval`]                       | `sm_CrvEval`                                |
//! | `to_euclid` (crate)                | `TO_EUCLID` (`SmNurbsCrv.h`)                |
//! | [`crv_remove_knots`]               | `sm_CrvRemoveKnots`                         |
//! | [`basis_find_all_span_maxima`]     | `sm_BasisFindAllSpanMaxima`                 |
//! | [`basis_i_eval`]                   | `sm_BasisIEval`                             |
//! | [`crv_remove_knot_max_err`]        | `sm_CrvRemoveKnotMaxErr`                    |
//! | [`basis_i_derivs`]                 | `sm_BasisIDerivs`                           |
//! | [`basis_find_global_max`]          | `sm_BasisFindGlobalMax`                     |
//! | [`is_ncrv_degenerate`]             | `sm_IsNCrvDegenerate`                       |
//! | [`MAX_DEGREE`], [`MAX_DERIV`]      | `GW_MAX_DEGREE`, `GW_MAX_DERIV`             |
//!
//! Deviations:
//! - Out-params become return values. Routines that fill caller-allocated curves
//!   (`sm_CreateBezSegments`, `sm_CrvSplit`, `sm_CrvTrim` with `pExisting*`) allocate
//!   their outputs with exactly the sizes upstream checks for, so the size-mismatch
//!   `return 1` paths do not exist. `sm_CreateBezSegments` allocates one segment per
//!   non-zero span (`N_BasisGetSpanCount`), as its only caller does.
//! - The memory-layout helpers (`sm_ComputeNurbCurveSize`, `sm_InitNurbCurveMemory`,
//!   `sm_FreeNurbCurve`, `SmStackHandler`) have no counterpart: [`Curve`] owns its
//!   storage. [`allocate_nurb_curve`] returns a zero-filled curve (upstream
//!   `smos_Calloc`); [`copy_nurb_curve`] returns a compact copy.
//! - Errors: where upstream returns its error flag after `N_ErrSet` or
//!   `smos_ErrorMessage(NL_*_ERR, ..)` the port returns [`SolidError::Nlib`] with that
//!   number. Where it returns the flag without a number (near-zero basis
//!   denominators, an empty trim interval) the port returns
//!   [`SolidError::Degenerate`]; for `sm_BasisFindGlobalMax`'s exhausted Newton
//!   restarts, [`SolidError::NotConverged`]. `sm_CrvTrim` returns the flag value
//!   `NL_INP_ERR` (14) for `ur <= ul`: [`NlibCode::Input`]. `sm_BasisEval` returns
//!   its flag at the domain ends after filling `N` (so `sm_CrvEval` fails there):
//!   [`SolidError::Unsupported`].
//! - Arrays sized by a caller count (the rows of [`basis_derivs`],
//!   [`crv_rational_basis_derivs`] and [`basis_i_derivs`] for `der`, the storage of
//!   [`allocate_nurb_curve`]) are allocated fallibly: `InvalidInput` when they do not
//!   fit in memory or take more than [`crate::limits::MAX_ALLOC_BYTES`] (upstream: the
//!   caller's arrays, or `smos_Calloc` returning `NULL`).
//! - `sm_CrvDerivs` accepts `der == GW_MAX_DERIV`, then writes past its row arrays;
//!   [`crv_derivs`] rejects `der >= MAX_DERIV` with [`NlibCode::MaxDerivative`]
//!   (upstream's own `der > GW_MAX_DERIV` check returns the plain flag).
//! - Every array access is bounds checked. Where upstream would index outside its
//!   arrays (degrees of `GW_MAX_DEGREE` or more, malformed curves, indices the
//!   routines do not check) the port returns [`SolidError::InvalidInput`]; for every
//!   input upstream handles inside its arrays the results are the same, bit for bit.
//!   The scratch arrays of [`crv_derivs`] and the rational basis derivatives are
//!   `SMALL_DEGREE` (8) wide for degrees below 8 and `GW_MAX_DEGREE` otherwise, and
//!   `sm_BasisDerivs`' `a` keeps only the two rows (`s1`, `s2`) it uses (same values
//!   and errors; this only avoids zeroing large stack arrays).
//! - `sm_CrvRemoveKnots` writes into an output curve that may be the input; the port
//!   works in place ([`crv_remove_knots`]; clone first for a separate output). Its
//!   return flag can be left over from a failed (and recovered) basis-maximum search;
//!   the port returns that flag as `Ok(bool)`. When an internal step fails the curve
//!   is left unchanged (upstream leaves it partly edited).
//! - `sm_BasisFindGlobalMax` loops forever when its bracketing step no longer
//!   advances; the port returns `InvalidInput` instead (as
//!   [`crate::nurbs::basis_basic::find_global_max`]).
//! - `sm_IsNCrvDegenerate` takes the tolerance with default `SM_EFF_ZERO`
//!   ([`crate::math::EFF_ZERO`]); its `SM_ASSERT` on the box has no counterpart.
//! - `sm_GetKnots` always returns the multiplicities (upstream: when asked).

// Branches keep upstream's `if( x NEQ NL_NOZ ) ... else ...` order for traceability.
#![allow(clippy::if_not_else)]

use crate::error::{NlibCode, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::tol::{in_tol, scaled_zero, scaled_zero2};
use crate::limits;
use crate::math::{max2, Vec3, EFF_ZERO, EFF_ZERO_SQ};
use crate::nurbs::basis_basic::SpanMaxima;
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::geom::dist_cpt_cpt_homo;
use crate::nurbs::knots::{Basis, KnotVector, Side, MAX_DEGREE as KNOT_MAX_DEGREE};
use crate::nurbs::math::{pascal_entry, pascal_triangle, CPoint, BIGD, NOW, NOZ};

/// `GW_MAX_DEGREE`: size of the fixed scratch arrays; degrees must stay below it.
pub const MAX_DEGREE: usize = 32;
/// `GW_MAX_DERIV`: number of derivative rows of the fixed scratch arrays.
pub const MAX_DERIV: usize = 5;
/// Scratch width used by the hot evaluators for degrees below it (performance only:
/// every index those routines touch is at most the degree, so a width of
/// `SMALL_DEGREE` gives the same values and errors as [`MAX_DEGREE`] while zeroing
/// far less stack per call).
const SMALL_DEGREE: usize = 8;

/// `WMIN`: smallest weight knot removal may create.
const WMIN: f64 = 1.0e-03;
/// `WMAX`: largest weight knot removal may create.
const WMAX: f64 = 100.0;
/// `cto`: relative tolerance of the basis-maximum searches of knot removal.
const CTO: f64 = 1.0e-05;
/// `NL_SMAD` (`DBL_MIN`).
const SMAD: f64 = f64::MIN_POSITIVE;
/// `itl`: Newton iteration / restart limit of [`basis_find_global_max`].
const ITL: usize = 20;
/// `nok`: bracketing steps per degree of [`basis_find_global_max`].
const NOK: usize = 10;

const OP_BEZ: &str = "nurbs::nurbs_crv::create_bez_segments";
const OP_SPLIT: &str = "nurbs::nurbs_crv::crv_split";
const OP_TRIM: &str = "nurbs::nurbs_crv::crv_trim";
const OP_BDERIVS: &str = "nurbs::nurbs_crv::basis_derivs";
const OP_RBDERIVS: &str = "nurbs::nurbs_crv::crv_rational_basis_derivs";
const OP_CDERIVS: &str = "nurbs::nurbs_crv::crv_derivs";
const OP_BEVAL: &str = "nurbs::nurbs_crv::basis_eval";
const OP_CEVAL: &str = "nurbs::nurbs_crv::crv_eval";
const OP_REMOVE: &str = "nurbs::nurbs_crv::crv_remove_knots";
const OP_MAXIMA: &str = "nurbs::nurbs_crv::basis_find_all_span_maxima";
const OP_IEVAL: &str = "nurbs::nurbs_crv::basis_i_eval";
const OP_MAXERR: &str = "nurbs::nurbs_crv::crv_remove_knot_max_err";
const OP_IDERIVS: &str = "nurbs::nurbs_crv::basis_i_derivs";
const OP_GMAX: &str = "nurbs::nurbs_crv::basis_find_global_max";
const OP_ALLOC: &str = "nurbs::nurbs_crv::allocate_nurb_curve";

fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

fn degenerate(operation: &'static str) -> SolidError {
    SolidError::Degenerate {
        operation,
        entity: None,
    }
}

/// The error flag `sm_BasisEval` returns at the ends of the domain.
fn at_domain_end(operation: &'static str) -> SolidError {
    SolidError::Unsupported {
        operation,
        reason: "parameter at a domain end (upstream fills N but returns its error flag)",
    }
}

fn oob(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "index out of range (malformed curve, degree or arguments)",
    }
}

/// `usize` -> signed index.
fn sx(u: usize, op: &'static str) -> Result<isize> {
    isize::try_from(u).map_err(|_| oob(op))
}

/// Signed index -> `usize` (count or size).
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

/// `a[i][j]`.
#[inline]
fn get2<const N: usize>(a: &[[f64; N]], i: isize, j: isize, op: &'static str) -> Result<f64> {
    let row = usize::try_from(i)
        .ok()
        .and_then(|i| a.get(i))
        .ok_or_else(|| oob(op))?;
    get(row, j, op)
}

/// `a[i][j] = v`.
#[inline]
fn set2<const N: usize>(
    a: &mut [[f64; N]],
    i: isize,
    j: isize,
    v: f64,
    op: &'static str,
) -> Result<()> {
    let row = usize::try_from(i)
        .ok()
        .and_then(|i| a.get_mut(i))
        .ok_or_else(|| oob(op))?;
    set(row, j, v, op)
}

/// Rejects degrees at or above [`MAX_DEGREE`] (upstream's scratch arrays).
fn check_degree(p: usize, op: &'static str) -> Result<()> {
    if p >= MAX_DEGREE {
        return Err(SolidError::InvalidInput {
            operation: op,
            reason: "degree at or above GW_MAX_DEGREE (upstream scratch array size)",
        });
    }
    Ok(())
}

// ---- frames, knots and allocation ---------------------------------------------------

/// The origin, X axis and Y axis of a reference frame (`sm_ExtractFrame`).
#[must_use]
pub fn extract_frame(frame: &Axis2Placement) -> (Vec3, Vec3, Vec3) {
    (frame.origin(), frame.x_axis(), frame.y_axis())
}

/// The distinct knots of `knt` and their multiplicities, in order (`sm_GetKnots`).
///
/// With `ivl`, only knots `ivl` contains (`ContainsValue` with tolerance 0) are
/// listed; the interval ends appear only when they are knots. A knot within the
/// scaled zero (`SM_IS_ZERO`) of the previous listed knot counts as a repeat.
#[must_use]
pub fn get_knots(knt: &KnotVector<'_>, ivl: Option<&Extent1d>) -> (Vec<f64>, Vec<usize>) {
    let u = knt.knots();
    let mut knots: Vec<f64> = Vec::new();
    let mut mults: Vec<usize> = Vec::new();
    let contains = |v: f64| ivl.is_none_or(|e| e.contains_value(v, 0.0));

    // Copy unique knots and compute multiplicities.
    let Some((&u0, rest)) = u.split_first() else {
        return (knots, mults);
    };
    if contains(u0) {
        knots.push(u0);
        mults.push(1);
    }

    // For every input knot but the first.
    for &uj in rest {
        if contains(uj) {
            match (knots.last(), mults.last_mut()) {
                (Some(&last), Some(mult)) if in_tol(last - uj, scaled_zero()) => *mult += 1,
                _ => {
                    knots.push(uj);
                    mults.push(1);
                }
            }
        }
    }
    (knots, mults)
}

/// `len` copies of `value`, allocated fallibly: `InvalidInput` instead of an
/// allocation abort when a caller-given count (derivative order, array size) makes
/// the array too large for memory or larger than [`limits::MAX_ALLOC_BYTES`].
fn filled<T: Clone>(len: usize, value: T, op: &'static str) -> Result<Vec<T>> {
    limits::try_filled(len, value).ok_or(SolidError::InvalidInput {
        operation: op,
        reason: "array too large for memory",
    })
}

/// A zero-filled curve with highest control point index `n`, degree `p` and highest
/// knot index `m` (`sm_AllocateNurbCurve`, memory from `smos_Calloc`).
///
/// Errors with `InvalidInput` when `n` or `m` is `usize::MAX` (no storage size).
pub fn allocate_nurb_curve(n: usize, p: usize, m: usize) -> Result<Curve> {
    let too_big = || SolidError::InvalidInput {
        operation: OP_ALLOC,
        reason: "index too large",
    };
    // smos_Calloc failing makes upstream return NULL (NERN): allocate fallibly
    let pw = filled(
        n.checked_add(1).ok_or_else(too_big)?,
        CPoint::default(),
        OP_ALLOC,
    )?;
    let u = filled(m.checked_add(1).ok_or_else(too_big)?, 0.0, OP_ALLOC)?;
    Curve::new(pw, p, u)
}

/// A compact copy of the control points and knots in use (`sm_CopyNurbCurve`,
/// `sm_AllocateAndCopyNurbCurve`).
#[must_use]
pub fn copy_nurb_curve(from: &Curve) -> Curve {
    let mut c = from.clone();
    c.compress();
    c
}

/// Makes knots closer than the scaled zero of the end knots equal and spreads
/// distinct knots closer than `tol2d` to `1.01 * tol2d` apart, about their midpoint
/// (the end knots stay put) (`sm_FixupKnotVector`; upstream default
/// `tol2d = SM_EFF_ZERO_PARAM`).
pub fn fixup_knot_vector(knots: &mut [f64], tol2d: f64) {
    let tol2d = tol2d.abs();
    let n_kts = knots.len();
    let (Some(&u0), Some(&u1)) = (knots.first(), knots.last()) else {
        return;
    };
    let scaled_zero = scaled_zero2(&u0, &u1);

    // Spread knots out by a little more than the given tolerance.
    let increment = tol2d * 1.01;

    // Collect unique knots, making knots closer than the scaled zero equal.
    let mut kts: Vec<f64> = Vec::new();
    let mut mults: Vec<usize> = Vec::new();
    let mut mult = 1;
    for i in 0..n_kts.saturating_sub(1) {
        let (Some(&a), Some(&b)) = (knots.get(i), knots.get(i + 1)) else {
            break;
        };
        let delta = b - a; // no fabs(): non-decreasing
        if delta < scaled_zero {
            // Same (multiple) knot.
            mult += 1;
            if let Some(slot) = knots.get_mut(i + 1) {
                *slot = a;
            }
        } else {
            mults.push(mult);
            kts.push(a);
            mult = 1;
        }
    }
    mults.push(mult);
    kts.push(knots.last().copied().unwrap_or(u1));

    // Run the algorithm on the distinct knots.
    let num_distinct = kts.len();
    for i in 0..num_distinct.saturating_sub(1) {
        let (Some(&a), Some(&b)) = (kts.get(i), kts.get(i + 1)) else {
            break;
        };
        let delta = b - a;
        if delta < tol2d {
            // Not meant to be the same, but too close: spread away from the midpoint,
            // keeping the start and end knots.
            if i < 1 {
                if let Some(slot) = kts.get_mut(i + 1) {
                    *slot = a + increment;
                }
            } else if i >= num_distinct - 2 {
                if let Some(slot) = kts.get_mut(i) {
                    *slot = b - increment;
                }
            } else {
                let mid = (a + b) / 2.0;
                if let Some(slot) = kts.get_mut(i) {
                    *slot = mid - increment / 2.0;
                }
                if let Some(slot) = kts.get_mut(i + 1) {
                    *slot = mid + increment / 2.0;
                }
            }
        }
    }

    // Dump back into the knot vector.
    let mut slots = knots.iter_mut();
    for (&k, &mult) in kts.iter().zip(&mults) {
        for _ in 0..mult {
            if let Some(slot) = slots.next() {
                *slot = k;
            }
        }
    }
}

// ---- Bezier segments, split and trim ------------------------------------------------

/// Control point `j` of curve `c` in `segs`.
fn seg_get(segs: &[Curve], c: usize, j: isize, op: &'static str) -> Result<CPoint> {
    get(segs.get(c).ok_or_else(|| oob(op))?.cpts(), j, op)
}

/// Sets control point `j` of curve `c` in `segs`.
fn seg_set(segs: &mut [Curve], c: usize, j: isize, v: CPoint, op: &'static str) -> Result<()> {
    set(segs.get_mut(c).ok_or_else(|| oob(op))?.cpts_mut(), j, v, op)
}

/// The Bezier segments of a curve, one per non-zero knot span (`sm_CreateBezSegments`):
/// each segment has `p + 1` control points and the knots `U[a]` and `U[b]` of its span,
/// each `p + 1` times.
///
/// Errors with `InvalidInput` for a curve without a non-zero span (upstream reads
/// segment 0 of an empty array) and where upstream would index out of bounds.
pub fn create_bez_segments(cur: &Curve) -> Result<Vec<Curve>> {
    const OP: &str = OP_BEZ;
    let pu = cur.degree();
    check_degree(pu, OP)?;
    let p = sx(pu, OP)?;
    let m = sx(cur.m(), OP)?;
    let pw = cur.cpts();
    let up = cur.knots();
    let knt = cur.knot_vector()?;

    // Get number of non-zero segments.
    let nsp = knt.span_count(pu);
    if nsp == 0 {
        return Err(oob(OP));
    }
    let mut segs: Vec<Curve> = (0..nsp)
        .map(|_| allocate_nurb_curve(pu, pu, 2 * pu + 1))
        .collect::<Result<_>>()?;
    let nsp = sx(nsp, OP)?;

    let mut alfs = [0.0_f64; MAX_DEGREE];
    let mut omas = [0.0_f64; MAX_DEGREE];

    // Initialize.
    let mut is = p;
    let mut ie = p + 1;
    let mut iq: isize = -1;

    // Skip over first knot over-multiplicity (bad data).
    while ie < m && get(up, ie - 1, OP)? == get(up, ie, OP)? {
        is += 1;
        ie += 1;
    }

    // If we had bad starting knots move the control point start on.
    for i in 0..=p {
        seg_set(&mut segs, 0, i, get(pw, i + is - p, OP)?, OP)?;
    }

    // Loop through the knot vector and extract each segment.
    let mut nqw: Option<usize> = None;
    while ie < m {
        iq += 1;

        // Safety check: segments [0, nsp).
        if iq >= nsp {
            return Ok(segs);
        }
        let q = ux(iq, OP)?;
        if iq < nsp - 1 {
            nqw = Some(q + 1);
        }

        // Get knot multiplicity.
        let i0 = ie;
        while ie < m && get(up, ie, OP)? == get(up, ie + 1, OP)? {
            ie += 1;
        }
        let mlt = ie - i0 + 1;
        let mut r = p - mlt;

        // If multiplicity < degree then insert the knot.
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
                let mut j = p;
                while j >= s {
                    let v = CPoint::combine2(
                        get(&alfs, j - s, OP)?,
                        seg_get(&segs, q, j, OP)?,
                        get(&omas, j - s, OP)?,
                        seg_get(&segs, q, j - 1, OP)?,
                    );
                    seg_set(&mut segs, q, j, v, OP)?;
                    j -= 1;
                }
                if ie < m {
                    let v = seg_get(&segs, q, p, OP)?;
                    seg_set(&mut segs, nqw.ok_or_else(|| oob(OP))?, save, v, OP)?;
                }
            }
        }

        // Get knot vector.
        let (ua, ub) = (get(up, is, OP)?, get(up, ie, OP)?);
        let uq = segs.get_mut(q).ok_or_else(|| oob(OP))?.knots_mut();
        for i in 0..=p {
            set(uq, i, ua, OP)?;
            set(uq, i + p + 1, ub, OP)?;
        }

        // Segment completed: prepare for next piece.
        if ie < m {
            if r < 0 {
                r = 0;
            }
            let nq = nqw.ok_or_else(|| oob(OP))?;
            for i in r..=p {
                seg_set(&mut segs, nq, i, get(pw, ie - p + i, OP)?, OP)?;
            }
        }
        is = ie;
        ie += 1;
    }
    Ok(segs)
}

/// Splits a curve at `u` into the pieces over `[U[0], u]` and `[u, U[m]]`
/// (`sm_CrvSplit`).
///
/// Errors with [`NlibCode::Parameter`] when `u <= U[0]` or `u >= U[m]`.
pub fn crv_split(cur: &Curve, u: f64) -> Result<(Curve, Curve)> {
    const OP: &str = OP_SPLIT;
    let pu = cur.degree();
    check_degree(pu, OP)?;
    let p = sx(pu, OP)?;
    let n = sx(cur.n(), OP)?;
    let m = sx(cur.m(), OP)?;
    let pw = cur.cpts();
    let uk = cur.knots();
    let knt = cur.knot_vector()?;

    // Check parameter.
    if knt.is_end_param(u) {
        return Err(nlib(OP, NlibCode::Parameter));
    }

    // Get span and knot multiplicity.
    let (spn, mlt) = knt.find_span_and_mult(pu, u, Side::Left)?;
    let spn = sx(spn, OP)?;
    let mlt = sx(mlt, OP)?;

    // Allocate the curves.
    let mut cur_l = allocate_nurb_curve(ux(spn - mlt, OP)?, pu, ux(spn - mlt + p + 1, OP)?)?;
    let mut cur_r = allocate_nurb_curve(ux(n + p - spn, OP)?, pu, ux(n - spn + 2 * p + 1, OP)?)?;
    let mut sw = [CPoint::default(); MAX_DEGREE];

    {
        let lw = cur_l.cpts_mut();
        for i in 0..=p - mlt {
            set(&mut sw, i, get(pw, spn - p + i, OP)?, OP)?;
        }

        // Save unaltered control points.
        for i in 0..=spn - p {
            set(lw, i, get(pw, i, OP)?, OP)?;
        }
        let rw = cur_r.cpts_mut();
        for i in spn - mlt..=n {
            set(rw, i + p - spn, get(pw, i, OP)?, OP)?;
        }

        // Now split the curve.
        for i in 1..=p - mlt {
            let k = spn - p + i;
            for j in 0..=p - i - mlt {
                let alf =
                    (u - get(uk, k + j, OP)?) / (get(uk, spn + j + 1, OP)? - get(uk, k + j, OP)?);
                let oma = 1.0 - alf;
                let v = CPoint::combine2(alf, get(&sw, j + 1, OP)?, oma, get(&sw, j, OP)?);
                set(&mut sw, j, v, OP)?;
            }
            set(lw, k, get(&sw, 0, OP)?, OP)?;
            set(rw, p - i - mlt, get(&sw, p - i - mlt, OP)?, OP)?;
        }
    }

    // Load knot vectors.
    let ul = cur_l.knots_mut();
    let k = spn - mlt;
    for i in 0..=k {
        set(ul, i, get(uk, i, OP)?, OP)?;
    }
    for i in 0..=p {
        set(ul, k + i + 1, u, OP)?;
    }
    let ur = cur_r.knots_mut();
    let k = spn + 1;
    for i in 0..=p {
        set(ur, i, u, OP)?;
    }
    for i in k..=m {
        set(ur, i - k + p + 1, get(uk, i, OP)?, OP)?;
    }
    Ok((cur_l, cur_r))
}

/// The piece of a curve over `[ul, ur]` (`sm_CrvTrim`): the control points of the
/// spans covering the interval, with `ul` and `ur` inserted `p` times.
///
/// Errors with [`NlibCode::Input`] when `ur <= ul` (upstream returns the flag
/// `NL_INP_ERR`), with [`SolidError::Degenerate`] when the interval covers no
/// control point, and with the errors of
/// [`KnotVector::find_span_and_mult`](crate::nurbs::knots::KnotVector::find_span_and_mult).
pub fn crv_trim(cur: &Curve, ul: f64, ur: f64) -> Result<Curve> {
    const OP: &str = OP_TRIM;
    let pu = cur.degree();
    let p = sx(pu, OP)?;
    let m = sx(cur.m(), OP)?;
    let pw = cur.cpts();
    let up = cur.knots();
    let knt = cur.knot_vector()?;

    // Check parameters.
    if ur <= ul {
        return Err(nlib(OP, NlibCode::Input));
    }

    // Find knot spans and set new indexes (exact checks, no tolerances).
    let (spl, mll) = knt.find_span_and_mult(pu, ul, Side::Left)?;
    let (spr, mlr) = knt.find_span_and_mult(pu, ur, Side::Left)?;
    let (spl, mll) = (sx(spl, OP)?, sx(mll, OP)?);
    let (mut spr, mut mlr) = (sx(spr, OP)?, sx(mlr, OP)?);

    let is = spl - p;
    if ur == get(up, m - p, OP)? {
        spr = m;
        mlr = p + 1;
    }
    let ie = spr - mlr;

    let n = ie - is;
    let mq = spr - spl - mlr + 2 * p + 1;

    // Degenerate trim interval: no control points.
    if n < 0 {
        return Err(degenerate(OP));
    }

    let mut cur_q = allocate_nurb_curve(ux(n, OP)?, pu, ux(mq, OP)?)?;
    let qw = cur_q.cpts_mut();

    // Copy control point values that cover the span interval.
    for i in is..=ie {
        set(qw, i - is, get(pw, i, OP)?, OP)?;
    }

    // Insert the left knot.
    let ll = spl - p;
    for i in 1..=p - mll {
        for j in 0..=p - i - mll {
            let left = get(up, ll + i + j, OP)?;
            let alf = (ul - left) / (get(up, spl + j + 1, OP)? - left);
            let oma = 1.0 - alf;
            let v = CPoint::combine2(alf, get(qw, j + 1, OP)?, oma, get(qw, j, OP)?);
            set(qw, j, v, OP)?;
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
            let v = CPoint::combine2(alf, get(qw, k, OP)?, oma, get(qw, k - 1, OP)?);
            set(qw, k, v, OP)?;
            j -= 1;
        }
    }

    // Load the knot vector.
    let uq = cur_q.knots_mut();
    let mut j: isize = -1;
    for _ in 0..=p {
        j += 1;
        set(uq, j, ul, OP)?;
    }
    for i in spl + 1..=spr - mlr {
        j += 1;
        set(uq, j, get(up, i, OP)?, OP)?;
    }
    for _ in 0..=p {
        j += 1;
        set(uq, j, ur, OP)?;
    }
    Ok(cur_q)
}

/// The Euclidean images of control points `k..=l` (`sm_CrvGetEPolygon`).
///
/// Errors with [`NlibCode::Index`] when `k > l` or `l > n`.
pub fn crv_get_epolygon(cur: &Curve, k: usize, l: usize) -> Result<Vec<Vec3>> {
    cur.epolygon(k, l)
}

// ---- basis functions and evaluation -------------------------------------------------

/// Non-vanishing basis functions and derivatives at a parameter, as returned by
/// [`basis_derivs`] and [`crv_rational_basis_derivs`].
#[derive(Debug, Clone, PartialEq)]
pub struct BasisDerivs {
    /// Knot span index: `values[k][j]` belongs to control point `span - p + j`.
    pub span: usize,
    /// `values[k][j]`: `k`-th derivative of basis function `span - p + j`,
    /// `k = 0..=der`, `j = 0..=p`.
    pub values: Vec<Vec<f64>>,
}

/// Fills the `der + 1` rows of `nd` (at least `p + 1` columns each) with the
/// non-vanishing basis functions and derivatives at `u` and returns the span
/// (`sm_BasisDerivs`).
///
/// `N` is the scratch width (`GW_MAX_DEGREE` upstream): [`MAX_DEGREE`], or
/// [`SMALL_DEGREE`] for degrees below it. Only rows `s1`, `s2` (0 and 1) of
/// upstream's `a[GW_MAX_DEGREE][GW_MAX_DEGREE]` are ever used, so `a` has two rows.
#[allow(clippy::range_plus_one)] // exclusive ranges: `RangeInclusive` is measurably slower here
fn basis_derivs_into<const N: usize>(
    knt: &KnotVector<'_>,
    pu: usize,
    u: f64,
    side: Side,
    der: usize,
    nd: &mut [[f64; N]],
) -> Result<usize> {
    const OP: &str = OP_BDERIVS;
    check_degree(pu, OP)?;
    if pu >= N {
        return Err(oob(OP));
    }
    let knot_tol = EFF_ZERO_SQ;
    let p = sx(pu, OP)?;
    let der = sx(der, OP)?;
    let uk = knt.knots();

    let mut left = [0.0_f64; N];
    let mut right = [0.0_f64; N];
    let mut ndu = [[0.0_f64; N]; N];
    let mut a = [[0.0_f64; N]; 2];

    // Find the knot span u is in.
    let i = sx(knt.find_span(pu, u, side)?, OP)?;
    let spn = ux(i, OP)?;

    // Get maximum derivative index and set zero derivatives.
    let mder = p.min(der);
    for k in p + 1..der + 1 {
        for j in 0..p + 1 {
            set2(nd, k, j, 0.0, OP)?;
        }
    }

    // Compute the basis functions.
    set2(&mut ndu, 0, 0, 1.0, OP)?;
    for j in 1..p + 1 {
        set(&mut left, j, u - get(uk, i + 1 - j, OP)?, OP)?;
        set(&mut right, j, get(uk, i + j, OP)? - u, OP)?;
        let mut saved = 0.0;
        for r in 0..j {
            let rr = get(&right, r + 1, OP)?;
            let ll = get(&left, j - r, OP)?;
            let den = rr + ll;
            set2(&mut ndu, j, r, den, OP)?;

            // The denominator is the sum of the two numerators, so only an exactly
            // (near) zero delta can fail.
            if den.abs() <= knot_tol {
                return Err(degenerate(OP));
            }
            let prev = get2(&ndu, r, j - 1, OP)?;
            set2(&mut ndu, r, j, saved + prev * (rr / den), OP)?;
            saved = prev * (ll / den);
        }
        set2(&mut ndu, j, j, saved, OP)?;
    }

    // Load the basis functions.
    for j in 0..p + 1 {
        set2(nd, 0, j, get2(&ndu, j, p, OP)?, OP)?;
    }

    // Compute derivatives.
    for r in 0..p + 1 {
        let mut s1: isize = 0;
        let mut s2: isize = 1;
        set2(&mut a, 0, 0, 1.0, OP)?;
        for k in 1..mder + 1 {
            let mut d = 0.0;
            let rk = r - k;
            let pk = p - k;
            if r >= k {
                let den = get2(&ndu, pk + 1, rk, OP)?;
                if den.abs() <= knot_tol {
                    return Err(degenerate(OP));
                }
                let v = get2(&a, s1, 0, OP)? / den;
                set2(&mut a, s2, 0, v, OP)?;
                d = v * get2(&ndu, rk, pk, OP)?;
            }
            let j1 = if rk >= -1 { 1 } else { -rk };
            let j2 = if r - 1 <= pk { k - 1 } else { p - r };
            for j in j1..j2 + 1 {
                let den = get2(&ndu, pk + 1, rk + j, OP)?;
                if den.abs() <= knot_tol {
                    return Err(degenerate(OP));
                }
                let v = (get2(&a, s1, j, OP)? - get2(&a, s1, j - 1, OP)?) / den;
                set2(&mut a, s2, j, v, OP)?;
                d += v * get2(&ndu, rk + j, pk, OP)?;
            }
            if r <= pk {
                let den = get2(&ndu, pk + 1, r, OP)?;
                if den.abs() <= knot_tol {
                    return Err(degenerate(OP));
                }
                let v = -get2(&a, s1, k - 1, OP)? / den;
                set2(&mut a, s2, k, v, OP)?;
                d += v * get2(&ndu, r, pk, OP)?;
            }
            set2(nd, k, r, d, OP)?;
            core::mem::swap(&mut s1, &mut s2);
        }
    }

    // Multiply through by the correct factors.
    let mut r = p;
    for k in 1..mder + 1 {
        for j in 0..p + 1 {
            let v = get2(nd, k, j, OP)? * r as f64;
            set2(nd, k, j, v, OP)?;
        }
        r *= p - k;
    }
    Ok(spn)
}

/// Rows `0..=der` of `nd`, `p + 1` values each.
fn rows(nd: &[[f64; MAX_DEGREE]], der: usize, p: usize, op: &'static str) -> Result<Vec<Vec<f64>>> {
    nd.get(..=der)
        .ok_or_else(|| oob(op))?
        .iter()
        .map(|row| row.get(..=p).map(<[f64]>::to_vec).ok_or_else(|| oob(op)))
        .collect()
}

/// All non-vanishing basis functions of degree `p` and their derivatives up to `der`
/// at `u` (`sm_BasisDerivs`, algorithm A2.3). Derivatives above the degree are zero.
///
/// Errors with [`SolidError::Degenerate`] when a knot difference is within
/// `SM_EFF_ZERO_SQ` of zero, and with `InvalidInput` for `p >= MAX_DEGREE` or where
/// upstream would index out of bounds.
pub fn basis_derivs(
    knt: &KnotVector<'_>,
    p: usize,
    u: f64,
    side: Side,
    der: usize,
) -> Result<BasisDerivs> {
    let mut nd = filled(der.saturating_add(1), [0.0_f64; MAX_DEGREE], OP_BDERIVS)?;
    let span = basis_derivs_into(knt, p, u, side, der, &mut nd)?;
    Ok(BasisDerivs {
        span,
        values: rows(&nd, der, p, OP_BDERIVS)?,
    })
}

/// Rational basis derivatives into `rd` (rows `0..=der`, already sized); `N` as in
/// [`basis_derivs_into`].
fn rational_basis_derivs_into<const N: usize>(
    cur: &Curve,
    u: f64,
    side: Side,
    der: usize,
    rd: &mut [[f64; N]],
) -> Result<usize> {
    const OP: &str = OP_RBDERIVS;
    let pw = cur.cpts();
    let pu = cur.degree();
    let p = sx(pu, OP)?;
    let knt = cur.knot_vector()?;
    // `der + 1` rows: on the stack for the usual `der < MAX_DERIV`.
    let mut nd_stack = [[0.0_f64; N]; MAX_DERIV];
    let mut nd_heap: Vec<[f64; N]>;
    let nd: &mut [[f64; N]] = if let Some(rows) = nd_stack.get_mut(..=der) {
        rows
    } else {
        nd_heap = filled(der.saturating_add(1), [0.0_f64; N], OP)?;
        &mut nd_heap
    };
    let mut d = [0.0_f64; MAX_DEGREE];

    // Get derivatives of all basis functions.
    let nsp = sx(basis_derivs_into(&knt, pu, u, side, der, nd)?, OP)?;
    let der_u = der;
    let der = sx(der, OP)?;

    // Get derivatives of the denominator.
    for k in 0..=der {
        set(&mut d, k, 0.0, OP)?;
        for j in nsp - p..=nsp {
            let w = get(pw, j, OP)?.w;
            let v = get(&d, k, OP)? + w * get2(nd, k, j - nsp + p, OP)?;
            set(&mut d, k, v, OP)?;
        }
    }

    // Compute derivatives of rational basis function. Rows up to `KNOT_MAX_DEGREE`
    // come from the constant table (no allocation); larger `der` builds them as before.
    let tri = if der_u > KNOT_MAX_DEGREE {
        Some(pascal_triangle(der_u)?)
    } else {
        None
    };
    let tri_at = |k: isize, j: isize| -> Result<f64> {
        let (k, j) = (ux(k, OP)?, ux(j, OP)?);
        match &tri {
            Some(t) => t
                .get(k)
                .and_then(|row| row.get(j))
                .map(|&b| b as f64)
                .ok_or_else(|| oob(OP)),
            None => pascal_entry(k, j).ok_or_else(|| oob(OP)),
        }
    };
    for i in 0..=p {
        let w = get(pw, nsp - p + i, OP)?.w;
        for k in 0..=der {
            let mut v = w * get2(nd, k, i, OP)?;
            for j in 1..=k {
                let t = tri_at(k, j)?;
                v -= t * get(&d, j, OP)? * get2(rd, k - j, i, OP)?;
            }
            set2(rd, k, i, v / get(&d, 0, OP)?, OP)?;
        }
    }
    ux(nsp, OP)
}

/// The non-vanishing rational basis functions `R[k][j]` of a curve and their
/// derivatives up to `der` at `u` (`sm_CrvRationalBasisDerivs`).
///
/// Errors with the errors of [`basis_derivs`].
pub fn crv_rational_basis_derivs(
    cur: &Curve,
    u: f64,
    side: Side,
    der: usize,
) -> Result<BasisDerivs> {
    let mut rd = filled(der.saturating_add(1), [0.0_f64; MAX_DEGREE], OP_RBDERIVS)?;
    let span = rational_basis_derivs_into(cur, u, side, der, &mut rd)?;
    Ok(BasisDerivs {
        span,
        values: rows(&rd, der, cur.degree(), OP_RBDERIVS)?,
    })
}

/// The point and derivatives `C^(k)(u)`, `k = 0..=der`, of a curve
/// (`sm_CrvDerivs`): the (rational) basis derivatives times the Euclidean control
/// points.
///
/// Errors with [`NlibCode::MaxDerivative`] when `der >= MAX_DERIV`, with the errors of
/// [`basis_derivs`] and with [`NlibCode::Index`] when the span's control points are
/// missing.
pub fn crv_derivs(cur: &Curve, u: f64, side: Side, der: usize) -> Result<Vec<Vec3>> {
    if der >= MAX_DERIV {
        return Err(nlib(OP_CDERIVS, NlibCode::MaxDerivative));
    }
    let mut cd = vec![Vec3::ZERO; der + 1];
    crv_derivs_into(cur, u, side, &mut cd)?;
    Ok(cd)
}

/// [`crv_derivs`] into the caller's slice: `out[k] = C^(k)(u)` for `k < out.len()`
/// (`der = out.len() - 1`), without a heap allocation.
///
/// Errors as [`crv_derivs`]; an empty `out` is [`NlibCode::MaxDerivative`] as well.
pub fn crv_derivs_into(cur: &Curve, u: f64, side: Side, out: &mut [Vec3]) -> Result<()> {
    const OP: &str = OP_CDERIVS;
    let der = match out.len().checked_sub(1) {
        Some(der) if der < MAX_DERIV => der,
        _ => return Err(nlib(OP, NlibCode::MaxDerivative)),
    };
    if cur.degree() < SMALL_DEGREE {
        crv_derivs_n::<SMALL_DEGREE>(cur, u, side, der, out)
    } else {
        crv_derivs_n::<MAX_DEGREE>(cur, u, side, der, out)
    }
}

/// [`crv_derivs_into`] with scratch width `N` (see [`basis_derivs_into`]);
/// `der < MAX_DERIV`, `out.len() == der + 1`.
fn crv_derivs_n<const N: usize>(
    cur: &Curve,
    u: f64,
    side: Side,
    der: usize,
    out: &mut [Vec3],
) -> Result<()> {
    const OP: &str = OP_CDERIVS;
    let pu = cur.degree();
    let mut bd = [[0.0_f64; N]; MAX_DERIV];
    let rows_bd = bd.get_mut(..=der).ok_or_else(|| oob(OP))?;

    // Compute basis function derivatives.
    let spn = if cur.is_rational() {
        rational_basis_derivs_into(cur, u, side, der, rows_bd)?
    } else {
        basis_derivs_into(&cur.knot_vector()?, pu, u, side, der, rows_bd)?
    };

    let k0 = spn
        .checked_sub(pu)
        .ok_or_else(|| nlib(OP, NlibCode::Index))?;
    // `crv_get_epolygon` into a stack array (`pu < N`, so `pu + 1` points fit).
    let mut pts = [Vec3::ZERO; N];
    let mut npts = 0;
    for (slot, q) in pts.iter_mut().zip(cur.epolygon_iter(k0, spn)?) {
        *slot = q;
        npts += 1;
    }
    let pts = pts.get(..npts).ok_or_else(|| oob(OP))?;

    // C^(k) = Sum_i(BasisVal[k][i] * P[i]).
    for (row, slot) in bd.get(..=der).ok_or_else(|| oob(OP))?.iter().zip(out) {
        let mut c = Vec3::new(0.0, 0.0, 0.0);
        for (i, q) in pts.iter().enumerate().take(pu + 1) {
            let b = row.get(i).copied().ok_or_else(|| oob(OP))?;
            c.x += b * q.x;
            c.y += b * q.y;
            c.z += b * q.z;
        }
        *slot = c;
    }
    Ok(())
}

/// All non-vanishing basis functions of degree `p` at `u` (`sm_BasisEval`):
/// `values[j]` belongs to control point `span - p + j`.
///
/// Errors with [`SolidError::Unsupported`] at `u == U[p]` and `u == U[m-p]`
/// (upstream fills `N` with `[1, 0, .., 0]` / `[0, .., 0, 1]` there but returns its
/// error flag) and with [`SolidError::Degenerate`] when a knot difference is below
/// `SM_EFF_ZERO_SQ`.
pub fn basis_eval(knt: &KnotVector<'_>, p: usize, u: f64, side: Side) -> Result<Basis> {
    check_degree(p, OP_BEVAL)?;
    let mut n = vec![0.0_f64; p + 1];
    let span = basis_eval_into(knt, p, u, side, &mut n)?;
    Ok(Basis { span, values: n })
}

/// [`basis_eval`] into the caller's buffer `n` (at least `p + 1` long; the evaluation
/// hot paths pass a stack array of [`MAX_DEGREE`]): fills `n[..=p]` and returns the
/// knot span. The same operations in the same order, so the values are bit-identical.
///
/// # Errors
///
/// Those of [`basis_eval`]; an index error when `n` is shorter than `p + 1`.
#[allow(clippy::range_plus_one)] // exclusive ranges: `RangeInclusive` is measurably slower here
pub fn basis_eval_into(
    knt: &KnotVector<'_>,
    p: usize,
    u: f64,
    side: Side,
    n: &mut [f64],
) -> Result<usize> {
    const OP: &str = OP_BEVAL;
    check_degree(p, OP)?;
    let knot_tol = EFF_ZERO_SQ;
    let uk = knt.knots();
    let pi = sx(p, OP)?;
    let m = sx(knt.last_index(), OP)?;

    // Special cases for end values: upstream fills N there but returns its error flag
    // (initialised to NL_YES for early exits).
    if u == get(uk, pi, OP)? || u == get(uk, m - pi, OP)? {
        return Err(at_domain_end(OP));
    }

    // Find the knot span u is in.
    let span = knt.find_span(p, u, side)?;
    let i = sx(span, OP)?;

    // Compute the non-vanishing B-splines.
    let mut left = [0.0_f64; MAX_DEGREE];
    let mut right = [0.0_f64; MAX_DEGREE];
    set(n, 0, 1.0, OP)?;
    for k in 1..pi + 1 {
        set(&mut left, k, u - get(uk, i + 1 - k, OP)?, OP)?;
        set(&mut right, k, get(uk, i + k, OP)? - u, OP)?;
        let mut saved = 0.0;
        for l in 0..k {
            let rr = get(&right, l + 1, OP)?;
            let ll = get(&left, k - l, OP)?;
            let delta = rr + ll;
            if delta < knot_tol {
                return Err(degenerate(OP));
            }
            let temp = get(n, l, OP)?;
            set(n, l, saved + temp * (rr / delta), OP)?;
            saved = temp * (ll / delta);
        }
        set(n, k, saved, OP)?;
    }
    Ok(span)
}

/// `TO_EUCLID`: homogeneous to Euclidean by dividing by `w` (no reciprocal, unlike
/// [`CPoint::to_euclid`]); a missing `z` becomes `0`.
#[must_use]
pub(crate) fn to_euclid(cw: CPoint) -> Vec3 {
    if cw.w != NOW {
        let z = if cw.z != NOZ { cw.z / cw.w } else { 0.0 };
        Vec3::new(cw.x / cw.w, cw.y / cw.w, z)
    } else {
        let z = if cw.z != NOZ { cw.z } else { 0.0 };
        Vec3::new(cw.x, cw.y, z)
    }
}

/// The point at `u` of a curve (`sm_CrvEval`): `Sum_i(N[i] * Pw[i])` mapped to
/// Euclidean space (a missing `z` becomes `0`).
///
/// Errors with the errors of [`basis_eval`] (so at the domain ends, as upstream,
/// whose caller evaluates the end control points itself there).
pub fn crv_eval(cur: &Curve, u: f64, side: Side) -> Result<Vec3> {
    const OP: &str = OP_CEVAL;
    let pw = cur.cpts();
    let p = cur.degree();
    let knt = cur.knot_vector()?;

    // Non-zero basis values of the span containing u (into a stack array: `p` is
    // below MAX_DEGREE, which basis_eval_into checks first).
    let mut values = [0.0_f64; MAX_DEGREE];
    let span = basis_eval_into(&knt, p, u, side, &mut values)?;
    let values = values.get(..=p).ok_or_else(|| oob(OP))?;

    // Compute the point on the curve.
    let mut cw = CPoint::new(0.0, 0.0, 0.0, 0.0);
    let spn_p = span.checked_sub(p).ok_or_else(|| oob(OP))?;
    for (i, &alpha) in values.iter().enumerate().take(p + 1) {
        let aw = pw.get(spn_p + i).copied().ok_or_else(|| oob(OP))?;
        // MAC_A_updcpt: skip zero weights, add without multiplying for one.
        if alpha == 1.0 {
            cw.x += aw.x;
            cw.y += aw.y;
            if aw.z != NOZ {
                cw.z += aw.z;
            } else {
                cw.z = NOZ;
            }
            if aw.w != NOW {
                cw.w += aw.w;
            } else {
                cw.w = NOW;
            }
        } else if alpha != 0.0 {
            cw.x += alpha * aw.x;
            cw.y += alpha * aw.y;
            if aw.z != NOZ {
                cw.z += alpha * aw.z;
            } else {
                cw.z = NOZ;
            }
            if aw.w != NOW {
                cw.w += alpha * aw.w;
            } else {
                cw.w = NOW;
            }
        }
    }

    Ok(to_euclid(cw))
}

// ---- knot removal -------------------------------------------------------------------

/// [`crv_remove_knot_max_err`] on raw arrays (control points `pw`, knots `u`).
fn remove_knot_max_err(pw: &[CPoint], p: isize, u: &[f64], r: isize, s: isize) -> Result<f64> {
    const OP: &str = OP_MAXERR;
    let mut rw = [CPoint::default(); MAX_DEGREE * 2];

    // Compute removal error.
    let mut i = r - p;
    let mut j = r - s;
    let mut ii: isize = 1;
    let mut jj = p - s + 1;

    set(&mut rw, 0, get(pw, r - p - 1, OP)?, OP)?;
    set(&mut rw, p - s + 2, get(pw, r - s + 1, OP)?, OP)?;

    // Get new control points for one removal step.
    while j - i > 0 {
        let ui = get(u, i, OP)?;
        let ur = get(u, r, OP)?;
        let alf = (get(u, i + p + 1, OP)? - ui) / (ur - ui);
        let oma = 1.0 - alf;
        let uj1 = get(u, j + p + 1, OP)?;
        let bet = (uj1 - get(u, j, OP)?) / (uj1 - ur);
        let omb = 1.0 - bet;
        let v = CPoint::combine2(alf, get(pw, i, OP)?, oma, get(&rw, ii - 1, OP)?);
        set(&mut rw, ii, v, OP)?;
        let v = CPoint::combine2(bet, get(pw, j, OP)?, omb, get(&rw, jj + 1, OP)?);
        set(&mut rw, jj, v, OP)?;
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
        let ui = get(u, i, OP)?;
        let alf = (get(u, r, OP)? - ui) / (get(u, i + p + 1, OP)? - ui);
        let oma = 1.0 - alf;
        let a = CPoint::combine2(alf, get(&rw, jj + 1, OP)?, oma, get(&rw, ii - 1, OP)?);
        Ok(dist_cpt_cpt_homo(get(pw, i, OP)?, a))
    }
}

/// The error bound of removing knot `U[r]` (multiplicity `s`, `U[r] != U[r+1]`,
/// interior) once (`sm_CrvRemoveKnotMaxErr`). Upstream does not check `r` and `s`.
///
/// Errors with `InvalidInput` where upstream would index out of bounds.
pub fn crv_remove_knot_max_err(cur: &Curve, r: usize, s: usize) -> Result<f64> {
    const OP: &str = OP_MAXERR;
    let p = cur.degree();
    check_degree(p, OP)?;
    remove_knot_max_err(cur.cpts(), sx(p, OP)?, cur.knots(), sx(r, OP)?, sx(s, OP)?)
}

/// Removes, in place, every interior knot of a curve whose removal keeps the curve
/// within `tol` (`sm_CrvRemoveKnots`, curve in = curve out). For rational curves the
/// tolerance is scaled by `wmin / (1 + pmax)`. The storage is compacted when a knot
/// was removed.
///
/// Returns upstream's error flag, which is set (`true`) only when no knot was removed
/// and the last basis-maximum search failed (upstream then uses fallback bounds).
///
/// Errors with `InvalidInput` (leaving the curve unchanged) where upstream would
/// index out of bounds.
pub fn crv_remove_knots(cur: &mut Curve, tol: f64) -> Result<bool> {
    const OP: &str = OP_REMOVE;
    let pu = cur.degree();
    check_degree(pu, OP)?;
    let p = sx(pu, OP)?;
    let mut n = sx(cur.n(), OP)?;
    let mut m = sx(cur.m(), OP)?;
    let ns = n;
    let mut tol = tol;
    let mut rat = false;

    // Adjust removal tolerance in case of rational curves.
    if cur.is_rational() {
        let mm = cur.min_max_weights_and_pts();
        tol = (tol * mm.wmin) / (1.0 + mm.pmax);
        rat = true;
    }

    let lto = {
        let uk = cur.knots();
        CTO * (get(uk, m, OP)? - get(uk, 0, OP)?).abs()
    };

    // Working copies (the output curve).
    let pw0 = cur.cpts().to_vec();
    let mut qw = pw0.clone();
    let mut uq = cur.knots().to_vec();

    // Local memory.
    let mlen = ux(m + 1, OP)?;
    let mut rw = vec![CPoint::default(); 2 * pu + 1];
    let mut br = vec![BIGD; mlen];
    let mut sr = vec![0_isize; mlen];
    let mut er = vec![0.0_f64; mlen];
    let mut te = vec![0.0_f64; mlen];
    let mut minl = vec![0.0_f64; pu + 1];
    let mut maxl = vec![0.0_f64; pu + 1];
    let mut minr = vec![0.0_f64; pu + 1];
    let mut maxr = vec![0.0_f64; pu + 1];
    let mut max = vec![0.0_f64; pu + 2];
    let mut error = false;

    // Compute knot removal errors for each distinct knot.
    let mut r = p + 1;
    while r <= n {
        let i = r;
        while r <= n && get(&uq, r, OP)? == get(&uq, r + 1, OP)? {
            r += 1;
        }
        set(&mut sr, r, r - i + 1, OP)?;
        if get(&sr, r, OP)? > p {
            return Ok(error);
        }
        let b = remove_knot_max_err(&pw0, p, cur.knots(), r, get(&sr, r, OP)?)?;
        set(&mut br, r, b, OP)?;
        error = false;
        r += 1;
    }

    // Try to remove each knot.
    loop {
        // Find knot with smallest error.
        let mut b = get(&br, p + 1, OP)?;
        let mut s = get(&sr, p + 1, OP)?;
        r = p + 1;
        for i in p + 2..m - p {
            if get(&br, i, OP)? < b {
                b = get(&br, i, OP)?;
                s = get(&sr, i, OP)?;
                r = i;
            }
        }

        // If no more removable knot -> finished.
        if b == BIGD {
            break;
        }

        // Check error of removal.
        let mut rmf = true;
        let k;
        let mut lam = 0.0;
        let mut oml = 0.0;
        if (p + s) % 2 != 0 {
            // Compute maximums of basis functions over each span.
            k = (p + s + 1) / 2;
            let alf = (get(&uq, r, OP)? - get(&uq, r - k, OP)?)
                / (get(&uq, r - k + p + 1, OP)? - get(&uq, r - k, OP)?);
            let bet = (get(&uq, r, OP)? - get(&uq, r - k + 1, OP)?)
                / (get(&uq, r - k + p + 2, OP)? - get(&uq, r - k + 1, OP)?);
            let omb = 1.0 - bet;
            lam = alf / (alf + bet);
            oml = 1.0 - lam;

            let knt = KnotVector::new(&uq, ux(m, OP)?)?;
            // (The flag of this search is overwritten by the next one.)
            if let Some(sm) =
                span_maxima_result(basis_find_all_span_maxima(&knt, ux(r - k, OP)?, pu, lto))?
            {
                copy_into(&mut minl, &sm.min, OP)?;
                copy_into(&mut maxl, &sm.max, OP)?;
            } else {
                minl.fill(0.0);
                maxl.fill(1.0);
            }
            if let Some(sm) = span_maxima_result(basis_find_all_span_maxima(
                &knt,
                ux(r - k + 1, OP)?,
                pu,
                lto,
            ))? {
                copy_into(&mut minr, &sm.min, OP)?;
                copy_into(&mut maxr, &sm.max, OP)?;
                error = false;
            } else {
                error = true;
                minr.fill(0.0);
                maxr.fill(1.0);
            }

            set(&mut max, 0, lam * alf * get(&maxl, 0, OP)?, OP)?;
            for i in 1..=p {
                let v = get(&minl, i, OP)? * (lam * alf);
                set(&mut minl, i, v, OP)?;
                let v = get(&minr, i - 1, OP)? * (oml * omb);
                set(&mut minr, i - 1, v, OP)?;
                let v = get(&maxl, i, OP)? * (lam * alf);
                set(&mut maxl, i, v, OP)?;
                let v = get(&maxr, i - 1, OP)? * (oml * omb);
                set(&mut maxr, i - 1, v, OP)?;

                let d1 = (get(&maxl, i, OP)? - get(&minr, i - 1, OP)?).abs();
                let d2 = (get(&maxr, i - 1, OP)? - get(&minl, i, OP)?).abs();
                set(&mut max, i, if d1 > d2 { d1 } else { d2 }, OP)?;
            }
            set(&mut max, p + 1, oml * omb * get(&maxr, p, OP)?, OP)?;

            // Check the error.
            for i in r - k..=r - k + p + 1 {
                if get(&uq, i, OP)? != get(&uq, i + 1, OP)? {
                    let t = get(&er, i, OP)? + get(&max, i - r + k, OP)? * b;
                    set(&mut te, i, t, OP)?;
                    if t > tol {
                        rmf = false;
                        break;
                    }
                }
            }
        } else {
            // Compute maximum of basis function.
            k = (p + s) / 2;
            let knt = KnotVector::new(&uq, ux(m, OP)?)?;
            if let Some(sm) =
                span_maxima_result(basis_find_all_span_maxima(&knt, ux(r - k, OP)?, pu, lto))?
            {
                copy_into(&mut minl, &sm.min, OP)?;
                copy_into(&mut max, &sm.max, OP)?;
                error = false;
            } else {
                error = true;
                for i in 0..=p {
                    set(&mut max, i, 1.0, OP)?;
                }
            }

            // Check the error.
            for i in r - k..=r - k + p {
                if get(&uq, i, OP)? != get(&uq, i + 1, OP)? {
                    let t = get(&er, i, OP)? + get(&max, i - r + k, OP)? * b;
                    set(&mut te, i, t, OP)?;
                    if t > tol {
                        rmf = false;
                        break;
                    }
                }
            }
        }

        if !rmf {
            // Knot is not removable.
            set(&mut br, r, BIGD, OP)?;
            continue;
        }

        // Error test passed -> update error vector.
        let l = if (p + s) % 2 != 0 {
            r - k + p + 1
        } else {
            r - k + p
        };
        for i in r - k..=l {
            if get(&uq, i, OP)? != get(&uq, i + 1, OP)? {
                set(&mut er, i, get(&te, i, OP)?, OP)?;
            }
        }

        // Remove the knot one time.
        let fout = (2 * r - s - p) / 2;
        let first = r - p;
        let last = r - s;
        let off = first - 1;
        let mut i = first;
        let mut j = last;
        let mut ii: isize = 1;
        let mut jj = last - off;

        set(&mut rw, 0, get(&qw, off, OP)?, OP)?;
        set(&mut rw, last + 1 - off, get(&qw, last + 1, OP)?, OP)?;

        // Get new control points for one removal step.
        while j - i > 0 {
            let ui = get(&uq, i, OP)?;
            let ur = get(&uq, r, OP)?;
            let alf = (get(&uq, i + p + 1, OP)? - ui) / (ur - ui);
            let oma = 1.0 - alf;
            let uj1 = get(&uq, j + p + 1, OP)?;
            let bet = (uj1 - get(&uq, j, OP)?) / (uj1 - ur);
            let omb = 1.0 - bet;
            let v = CPoint::combine2(alf, get(&qw, i, OP)?, oma, get(&rw, ii - 1, OP)?);
            set(&mut rw, ii, v, OP)?;
            let v = CPoint::combine2(bet, get(&qw, j, OP)?, omb, get(&rw, jj + 1, OP)?);
            set(&mut rw, jj, v, OP)?;
            i += 1;
            j -= 1;
            ii += 1;
            jj -= 1;
        }

        // Check for disallowed weights.
        if rat {
            let mut i = first;
            let mut j = last;
            let mut wmin = BIGD;
            let mut wmax = SMAD;
            while j - i > 0 {
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
            if wmin < WMIN || wmax > WMAX {
                set(&mut br, r, BIGD, OP)?;
                continue;
            }
        }

        // Save new control points.
        if (p + s) % 2 != 0 {
            let v = CPoint::combine2(lam, get(&rw, jj + 1, OP)?, oml, get(&rw, ii - 1, OP)?);
            set(&mut rw, ii - 1, v, OP)?;
        }
        let mut i = first;
        let mut j = last;
        while j - i > 0 {
            set(&mut qw, i, get(&rw, i - off, OP)?, OP)?;
            set(&mut qw, j, get(&rw, j - off, OP)?, OP)?;
            i += 1;
            j -= 1;
        }

        // Shift down some parameters.
        if s == 1 {
            let (a, c) = (get(&er, r - 1, OP)?, get(&er, r, OP)?);
            set(&mut er, r - 1, if a > c { a } else { c }, OP)?;
        }
        if s > 1 {
            let v = get(&sr, r, OP)? - 1;
            set(&mut sr, r - 1, v, OP)?;
        }
        for i in r + 1..=m {
            let v = get(&br, i, OP)?;
            set(&mut br, i - 1, v, OP)?;
            let v = get(&sr, i, OP)?;
            set(&mut sr, i - 1, v, OP)?;
            let v = get(&er, i, OP)?;
            set(&mut er, i - 1, v, OP)?;
        }

        // Shift down knots and control points.
        for i in r + 1..=m {
            let v = get(&uq, i, OP)?;
            set(&mut uq, i - 1, v, OP)?;
        }
        m -= 1;
        for i in fout + 1..=n {
            let v = get(&qw, i, OP)?;
            set(&mut qw, i - 1, v, OP)?;
        }
        n -= 1;

        // If no more internal knots -> finished.
        if n == p {
            break;
        }

        // Update error bounds.
        let k = if r - p > p + 1 { r - p } else { p + 1 };
        let l = if n < r + p - s { n } else { r + p - s };
        for i in k..=l {
            if get(&uq, i, OP)? != get(&uq, i + 1, OP)? {
                let b = remove_knot_max_err(&qw, p, &uq, i, get(&sr, i, OP)?)?;
                set(&mut br, i, b, OP)?;
                error = false;
            }
        }
    }

    // Compact output curve.
    if n < ns {
        qw.truncate(ux(n + 1, OP)?);
        uq.truncate(ux(m + 1, OP)?);
        *cur = Curve::new(qw, pu, uq)?;
        error = false;
    }
    Ok(error)
}

/// `Ok(Some(..))` on success, `Ok(None)` for an upstream error flag (recovered by the
/// caller with fallback bounds), `Err` for a port-only bounds error.
fn span_maxima_result(r: Result<SpanMaxima>) -> Result<Option<SpanMaxima>> {
    match r {
        Ok(sm) => Ok(Some(sm)),
        Err(
            e @ (SolidError::InvalidInput { .. }
            | SolidError::PortInvariant { .. }
            | SolidError::Unported { .. }),
        ) => Err(e),
        Err(_) => Ok(None),
    }
}

/// Copies `src` over the start of `dst`.
fn copy_into(dst: &mut [f64], src: &[f64], op: &'static str) -> Result<()> {
    dst.get_mut(..src.len())
        .ok_or_else(|| oob(op))?
        .copy_from_slice(src);
    Ok(())
}

// ---- basis function maxima ----------------------------------------------------------

/// Index check of the basis-maximum routines: `0 <= i <= m - p - 1`.
fn check_basis_index(m: usize, i: usize, p: usize, op: &'static str) -> Result<()> {
    if m.checked_sub(p + 1).is_none_or(|top| i > top) {
        return Err(nlib(op, NlibCode::Index));
    }
    Ok(())
}

/// The minimum and maximum of basis function `N[i, p]` over each of its `p + 1` knot
/// spans and the parameter of its global maximum (`sm_BasisFindAllSpanMaxima`).
///
/// Errors with [`NlibCode::Index`] when `i > m - p - 1`, with
/// [`NlibCode::Convergence`] when the global maximum lies in no span of the support,
/// and with the errors of [`basis_find_global_max`] and [`basis_i_eval`].
pub fn basis_find_all_span_maxima(
    knt: &KnotVector<'_>,
    i: usize,
    p: usize,
    tol: f64,
) -> Result<SpanMaxima> {
    const OP: &str = OP_MAXIMA;
    let uk = knt.knots();
    check_basis_index(knt.last_index(), i, p, OP)?;
    let ii = sx(i, OP)?;
    let pi = sx(p, OP)?;
    let mut min = vec![0.0_f64; p + 1];
    let mut max = vec![0.0_f64; p + 1];

    // Consider linear as special case.
    if p == 1 {
        min = vec![0.0, 0.0];
        max = vec![1.0, 1.0];
        let u = get(uk, ii + 1, OP)?;
        return Ok(SpanMaxima { min, max, u });
    }

    // Check if a p-fold inner multiple knot exists.
    let mut s = 1;
    for k in ii + 1..ii + pi {
        if get(uk, k, OP)? == get(uk, k + 1, OP)? {
            s += 1;
        }
    }
    if s == pi {
        max.fill(1.0);
        for k in 1..pi {
            set(&mut min, k, 1.0, OP)?;
        }
        let v0 = if get(uk, ii, OP)? == get(uk, ii + 1, OP)? {
            1.0
        } else {
            0.0
        };
        set(&mut min, 0, v0, OP)?;
        let vp = if get(uk, ii + pi, OP)? == get(uk, ii + pi + 1, OP)? {
            1.0
        } else {
            0.0
        };
        set(&mut min, pi, vp, OP)?;
        let u = get(uk, ii + 1, OP)?;
        return Ok(SpanMaxima { min, max, u });
    }

    // Get global maximum.
    let (nm, u) = basis_find_global_max(knt, i, p, tol)?;

    // Get span index of global maximum.
    let mut spn: isize = -1;
    for k in ii..=ii + pi {
        if u >= get(uk, k, OP)? && u < get(uk, k + 1, OP)? {
            spn = k;
            break;
        }
    }
    if spn == -1 {
        return Err(nlib(OP, NlibCode::Convergence));
    }

    let n_at = |x: f64| basis_i_eval(knt, i, p, x, Side::Left);

    // Compute local extrema.
    for k in 0..spn - ii {
        set(&mut min, k, n_at(get(uk, ii + k, OP)?)?, OP)?;
        set(&mut max, k, n_at(get(uk, ii + k + 1, OP)?)?, OP)?;
    }
    set(&mut max, spn - ii, nm, OP)?;
    let nl = n_at(get(uk, spn, OP)?)?;
    let nr = n_at(get(uk, spn + 1, OP)?)?;
    set(&mut min, spn - ii, if nl < nr { nl } else { nr }, OP)?;
    for k in spn - ii + 1..=pi {
        set(&mut min, k, n_at(get(uk, ii + k + 1, OP)?)?, OP)?;
        set(&mut max, k, n_at(get(uk, ii + k, OP)?)?, OP)?;
    }
    Ok(SpanMaxima { min, max, u })
}

/// Basis function `N[i, p]` at `u` (`sm_BasisIEval`; clamped knot vectors).
///
/// Errors with [`NlibCode::Index`] when `i > m - p - 1`.
pub fn basis_i_eval(knt: &KnotVector<'_>, i: usize, p: usize, u: f64, side: Side) -> Result<f64> {
    const OP: &str = OP_IEVAL;
    check_degree(p, OP)?;
    let uk = knt.knots();
    let m = sx(knt.last_index(), OP)?;
    let pi = sx(p, OP)?;
    let n = m - pi - 1;
    let ii = sx(i, OP)?;

    // Check index.
    if ii > n {
        return Err(nlib(OP, NlibCode::Index));
    }

    // Special cases for end values.
    if u == get(uk, pi, OP)? {
        return Ok(if ii == 0 { 1.0 } else { 0.0 });
    }
    if u == get(uk, m - pi, OP)? {
        return Ok(if ii == n { 1.0 } else { 0.0 });
    }

    let mut na = [0.0_f64; MAX_DEGREE];

    // Compute degree zero B-splines.
    match side {
        Side::Left => {
            if u < get(uk, ii, OP)? || u >= get(uk, ii + pi + 1, OP)? {
                return Ok(0.0);
            }
            for j in 0..=pi {
                let v = if u >= get(uk, ii + j, OP)? && u < get(uk, ii + j + 1, OP)? {
                    1.0
                } else {
                    0.0
                };
                set(&mut na, j, v, OP)?;
            }
        }
        Side::Right => {
            if u <= get(uk, ii, OP)? || u > get(uk, ii + pi + 1, OP)? {
                return Ok(0.0);
            }
            for j in 0..=pi {
                let v = if u > get(uk, ii + j, OP)? && u <= get(uk, ii + j + 1, OP)? {
                    1.0
                } else {
                    0.0
                };
                set(&mut na, j, v, OP)?;
            }
        }
    }

    // Compute the i-th B-spline using a triangular array.
    let ui = get(uk, ii, OP)?;
    for k in 1..=pi {
        let mut saved = if get(&na, 0, OP)? == 0.0 {
            0.0
        } else {
            ((u - ui) * get(&na, 0, OP)?) / (get(uk, ii + k, OP)? - ui)
        };
        for j in 0..=pi - k {
            let ur = get(uk, ii + j + k + 1, OP)?;
            let ul = get(uk, ii + j + 1, OP)?;
            if get(&na, j + 1, OP)? == 0.0 {
                set(&mut na, j, saved, OP)?;
                saved = 0.0;
            } else {
                let temp = get(&na, j + 1, OP)? / (ur - ul);
                set(&mut na, j, saved + (ur - u) * temp, OP)?;
                saved = (u - ul) * temp;
            }
        }
    }
    get(&na, 0, OP)
}

/// Basis function `N[i, p]` and its derivatives up to `der` at `u`
/// (`sm_BasisIDerivs`; clamped knot vectors). [`Side::Left`] gives right
/// derivatives at knots, [`Side::Right`] left derivatives.
///
/// Errors with [`NlibCode::Index`] when `i > m - p - 1`.
pub fn basis_i_derivs(
    knt: &KnotVector<'_>,
    i: usize,
    p: usize,
    u: f64,
    side: Side,
    der: usize,
) -> Result<Vec<f64>> {
    const OP: &str = OP_IDERIVS;
    const NT: usize = MAX_DEGREE + 1;
    check_degree(p, OP)?;
    let uk = knt.knots();
    let m = sx(knt.last_index(), OP)?;
    let pi = sx(p, OP)?;
    let n = m - pi - 1;
    let ii = sx(i, OP)?;
    let deri = sx(der, OP)?;
    let mut nd_out = filled(der.saturating_add(1), 0.0_f64, OP)?;

    // Check index.
    if ii > n {
        return Err(nlib(OP, NlibCode::Index));
    }

    let mut nt = [[0.0_f64; NT]; NT];
    let mut nd = [0.0_f64; NT];

    // Compute the degree zero B-splines.
    let inside_left = |u: f64, j: isize| -> Result<bool> {
        Ok(u >= get(uk, ii + j, OP)? && u < get(uk, ii + j + 1, OP)?)
    };
    let inside_right = |u: f64, j: isize| -> Result<bool> {
        Ok(u > get(uk, ii + j, OP)? && u <= get(uk, ii + j + 1, OP)?)
    };
    match side {
        Side::Left => {
            if u == get(uk, m - pi, OP)? && ii >= n - pi {
                for j in 0..=pi {
                    let v = if inside_right(u, j)? { 1.0 } else { 0.0 };
                    set2(&mut nt, j, 0, v, OP)?;
                }
            } else {
                if u < get(uk, ii, OP)? || u >= get(uk, ii + pi + 1, OP)? {
                    return Ok(nd_out);
                }
                for j in 0..=pi {
                    let v = if inside_left(u, j)? { 1.0 } else { 0.0 };
                    set2(&mut nt, j, 0, v, OP)?;
                }
            }
        }
        Side::Right => {
            if u == get(uk, pi, OP)? && ii <= pi {
                for j in 0..=pi {
                    let v = if inside_left(u, j)? { 1.0 } else { 0.0 };
                    set2(&mut nt, j, 0, v, OP)?;
                }
            } else {
                if u <= get(uk, ii, OP)? || u > get(uk, ii + pi + 1, OP)? {
                    return Ok(nd_out);
                }
                for j in 0..=pi {
                    let v = if inside_right(u, j)? { 1.0 } else { 0.0 };
                    set2(&mut nt, j, 0, v, OP)?;
                }
            }
        }
    }

    // Compute the full triangular array.
    let ui = get(uk, ii, OP)?;
    for k in 1..=pi {
        let mut saved = if get2(&nt, 0, k - 1, OP)? == 0.0 {
            0.0
        } else {
            ((u - ui) * get2(&nt, 0, k - 1, OP)?) / (get(uk, ii + k, OP)? - ui)
        };
        for j in 0..=pi - k {
            let ur = get(uk, ii + j + k + 1, OP)?;
            let ul = get(uk, ii + j + 1, OP)?;
            if get2(&nt, j + 1, k - 1, OP)? == 0.0 {
                set2(&mut nt, j, k, saved, OP)?;
                saved = 0.0;
            } else {
                let temp = get2(&nt, j + 1, k - 1, OP)? / (ur - ul);
                set2(&mut nt, j, k, saved + (ur - u) * temp, OP)?;
                saved = (u - ul) * temp;
            }
        }
    }

    // Compute derivatives.
    set(&mut nd_out, 0, get2(&nt, 0, pi, OP)?, OP)?;
    let mder = pi.min(deri);
    for k in pi + 1..=deri {
        set(&mut nd_out, k, 0.0, OP)?;
    }
    for k in 1..=mder {
        // Load the appropriate column into the derivative array.
        for j in 0..=k {
            set(&mut nd, j, get2(&nt, j, pi - k, OP)?, OP)?;
        }

        // Compute the triangular table of width k.
        for l in 1..=k {
            let mut saved = if get(&nd, 0, OP)? == 0.0 {
                0.0
            } else {
                get(&nd, 0, OP)? / (get(uk, ii + pi - k + l, OP)? - ui)
            };
            let f = (pi - k + l) as f64;
            for j in 0..=k - l {
                let ur = get(uk, pi - k + l + ii + j + 1, OP)?;
                let ul = get(uk, ii + j + 1, OP)?;
                if get(&nd, j + 1, OP)? == 0.0 {
                    set(&mut nd, j, f * saved, OP)?;
                    saved = 0.0;
                } else {
                    let temp = get(&nd, j + 1, OP)? / (ur - ul);
                    set(&mut nd, j, f * (saved - temp), OP)?;
                    saved = temp;
                }
            }
        }
        set(&mut nd_out, k, get(&nd, 0, OP)?, OP)?;
    }
    Ok(nd_out)
}

/// The global maximum of basis function `N[i, p]`: returns `(max, u)`
/// (`sm_BasisFindGlobalMax`). The maximum is bracketed by `10p` steps over the
/// support, then refined by Newton iteration (converged when `|N'| < tol` and
/// `N > tol`) with up to 20 re-brackets. Degree 1 and functions with a `p`-fold inner
/// knot peak at `U[i+1]` with value 1.
///
/// Errors with [`NlibCode::Index`] when `i > m - p - 1`, with [`NlibCode::Numerical`]
/// when `N'' == 0` in a Newton step, with [`SolidError::NotConverged`] after 20
/// re-brackets, and with `InvalidInput` where upstream would loop forever.
pub fn basis_find_global_max(
    knt: &KnotVector<'_>,
    i: usize,
    p: usize,
    tol: f64,
) -> Result<(f64, f64)> {
    const OP: &str = OP_GMAX;
    let uk = knt.knots();
    check_basis_index(knt.last_index(), i, p, OP)?;
    let ii = sx(i, OP)?;
    let pi = sx(p, OP)?;

    // Consider linear as special case.
    if p == 1 {
        return Ok((1.0, get(uk, ii + 1, OP)?));
    }

    // Check if a p-fold inner multiple knot exists.
    let mut s = 1;
    for k in ii + 1..ii + pi {
        if get(uk, k, OP)? == get(uk, k + 1, OP)? {
            s += 1;
        }
    }
    if s == pi {
        return Ok((1.0, get(uk, ii + 1, OP)?));
    }

    let mut nd = [0.0_f64; 3];
    let derivs = |u: f64, der: usize, nd: &mut [f64; 3]| -> Result<()> {
        let d = basis_i_derivs(knt, i, p, u, Side::Left, der)?;
        for (slot, v) in nd.iter_mut().zip(d) {
            *slot = v;
        }
        Ok(())
    };

    // Get guess parameter.
    let ui = get(uk, ii, OP)?;
    let ue = get(uk, ii + pi + 1, OP)?;
    let nos = p * NOK;
    let mut du = (ue - ui) / nos as f64;
    let mut ur = ui;
    let mut ul = ui;
    while ur < ue {
        ul = ur;
        ur += du;
        if ur > ue {
            ur = ue;
        }
        derivs(ur, 1, &mut nd)?;
        if nd[1] < 0.0 {
            break;
        }
        if ur == ul {
            // The step no longer advances: upstream repeats this iteration forever.
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "bracketing step does not advance (upstream loops forever)",
            });
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
            derivs(u0, 2, &mut nd)?;
            if nd[1].abs() < tol && nd[0] > tol {
                conv = true;
                break;
            }
            if nd[2] == 0.0 {
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
            derivs(ur, 1, &mut nd)?;
            if nd[1] < 0.0 {
                break;
            }
        }
        it += 1;
    }

    // If no convergence, quit.
    if it >= ITL {
        return Err(SolidError::NotConverged {
            operation: OP,
            iterations: ITL as u32,
        });
    }

    // Convergence reached -> get maximum.
    Ok((nd[0], u0))
}

// ---- degeneracy ---------------------------------------------------------------------

/// `true` when the box of the curve's control polygon is point sized within
/// `max(tol3d, SM_EFF_ZERO * (1 + max |coord|))` (`sm_IsNCrvDegenerate`; upstream
/// default `tol3d = SM_EFF_ZERO`).
pub fn is_ncrv_degenerate(cur: &Curve, tol3d: f64) -> Result<bool> {
    let b = cur.bbox()?;
    let bx = Extent3d::from_bounds(b.xl, b.yb, b.zn, b.xr, b.yt, b.zf);

    // Select a tolerance scaled to the box.
    let scaled_zero = EFF_ZERO * (1.0 + bx.max_dimension());
    let tol = max2(tol3d, scaled_zero);

    // Test the box for point size.
    Ok(bx.is_point_sized(tol))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(p: usize) -> Curve {
        // Degree p Bezier along x from 0 to p.
        let pw = (0..=p)
            .map(|i| CPoint::new(i as f64, 0.0, 0.0, NOW))
            .collect();
        let mut u = vec![0.0; p + 1];
        u.extend(vec![1.0; p + 1]);
        Curve::new(pw, p, u).unwrap()
    }

    fn cubic() -> Curve {
        let pw = vec![
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(1.0, 2.0, 0.0, NOW),
            CPoint::new(2.0, -1.0, 1.0, NOW),
            CPoint::new(3.0, 0.5, 0.0, NOW),
            CPoint::new(4.0, 1.0, 2.0, NOW),
            CPoint::new(5.0, 0.0, 0.0, NOW),
        ];
        let u = vec![0.0, 0.0, 0.0, 0.0, 0.3, 0.6, 1.0, 1.0, 1.0, 1.0];
        Curve::new(pw, 3, u).unwrap()
    }

    #[test]
    fn knots_and_multiplicities() {
        let c = cubic();
        let (k, mlt) = get_knots(&c.knot_vector().unwrap(), None);
        assert_eq!(k, vec![0.0, 0.3, 0.6, 1.0]);
        assert_eq!(mlt, vec![4, 1, 1, 4]);
        let ivl = Extent1d::from_min_max(0.2, 0.7);
        let (k, mlt) = get_knots(&c.knot_vector().unwrap(), Some(&ivl));
        assert_eq!(k, vec![0.3, 0.6]);
        assert_eq!(mlt, vec![1, 1]);
    }

    #[test]
    fn fixup_spreads_close_knots() {
        let mut k = vec![0.0, 0.0, 0.5, 0.5 + 1e-13, 0.5 + 1e-9, 1.0, 1.0];
        fixup_knot_vector(&mut k, 1e-6);
        assert_eq!(k[2], k[3]);
        assert!(k[4] - k[3] > 1e-6);
        assert_eq!(k[0], 0.0);
        assert_eq!(k[6], 1.0);
    }

    #[test]
    fn eval_matches_derivs() {
        let c = cubic();
        assert!(crv_eval(&c, 0.0, Side::Left).is_err());
        assert!(crv_eval(&c, 1.0, Side::Right).is_err());
        for &u in &[0.1, 0.3, 0.45, 0.6, 0.9] {
            let p = crv_eval(&c, u, Side::Left).unwrap();
            let d = crv_derivs(&c, u, Side::Left, 2).unwrap();
            assert!((p - d[0]).length() < 1e-12, "u = {u}");
        }
        assert!(crv_derivs(&c, 0.5, Side::Left, 5).is_err());
    }

    #[test]
    fn bezier_segments_reproduce_curve() {
        let c = cubic();
        let segs = create_bez_segments(&c).unwrap();
        assert_eq!(segs.len(), 3);
        for s in &segs {
            let (a, b) = s.param_bounds();
            let um = 0.5 * (a + b);
            let ps = crv_eval(s, um, Side::Left).unwrap();
            let pc = crv_eval(&c, um, Side::Left).unwrap();
            assert!((ps - pc).length() < 1e-12);
        }
    }

    #[test]
    fn split_and_trim_reproduce_curve() {
        let c = cubic();
        let (l, r) = crv_split(&c, 0.45).unwrap();
        let t = crv_trim(&c, 0.2, 0.8).unwrap();
        for &(cv, u) in &[(&l, 0.1), (&r, 0.7), (&t, 0.5)] {
            let a = crv_eval(cv, u, Side::Left).unwrap();
            let b = crv_eval(&c, u, Side::Left).unwrap();
            assert!((a - b).length() < 1e-12);
        }
        assert!(matches!(
            crv_split(&c, 0.0),
            Err(SolidError::Nlib {
                code: NlibCode::Parameter,
                ..
            })
        ));
        assert!(matches!(
            crv_trim(&c, 0.5, 0.5),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn remove_inserted_knot() {
        // A Bezier line with an extra knot: the knot is removable.
        let mut c = line(2);
        let (l, r) = crv_split(&c, 0.5).unwrap();
        let mut pw = l.cpts().to_vec();
        pw.extend_from_slice(&r.cpts()[1..]);
        let u = vec![0.0, 0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0];
        c = Curve::new(pw, 2, u).unwrap();
        let flag = crv_remove_knots(&mut c, 1e-6).unwrap();
        assert!(!flag);
        assert_eq!(c.m(), 5);
        assert_eq!(c.n(), 2);
    }

    /// The heap-allocating `basis_eval` / `crv_eval` as they were before
    /// `basis_eval_into` (the reference for the bit-identity test).
    fn old_basis_eval(knt: &KnotVector<'_>, p: usize, u: f64, side: Side) -> Result<Basis> {
        const OP: &str = OP_BEVAL;
        check_degree(p, OP)?;
        let knot_tol = EFF_ZERO_SQ;
        let uk = knt.knots();
        let pi = sx(p, OP)?;
        let m = sx(knt.last_index(), OP)?;
        let mut n = vec![0.0_f64; p + 1];
        if u == get(uk, pi, OP)? || u == get(uk, m - pi, OP)? {
            return Err(at_domain_end(OP));
        }
        let span = knt.find_span(p, u, side)?;
        let i = sx(span, OP)?;
        let mut left = [0.0_f64; MAX_DEGREE];
        let mut right = [0.0_f64; MAX_DEGREE];
        set(&mut n, 0, 1.0, OP)?;
        for k in 1..=pi {
            set(&mut left, k, u - get(uk, i + 1 - k, OP)?, OP)?;
            set(&mut right, k, get(uk, i + k, OP)? - u, OP)?;
            let mut saved = 0.0;
            for l in 0..k {
                let rr = get(&right, l + 1, OP)?;
                let ll = get(&left, k - l, OP)?;
                let delta = rr + ll;
                if delta < knot_tol {
                    return Err(degenerate(OP));
                }
                let temp = get(&n, l, OP)?;
                set(&mut n, l, saved + temp * (rr / delta), OP)?;
                saved = temp * (ll / delta);
            }
            set(&mut n, k, saved, OP)?;
        }
        Ok(Basis { span, values: n })
    }

    fn old_crv_eval(cur: &Curve, u: f64, side: Side) -> Result<Vec3> {
        let pw = cur.cpts();
        let p = cur.degree();
        let knt = cur.knot_vector()?;
        let basis = old_basis_eval(&knt, p, u, side)?;
        let mut cw = CPoint::new(0.0, 0.0, 0.0, 0.0);
        let spn_p = basis.span - p;
        for (i, &alpha) in basis.values.iter().enumerate().take(p + 1) {
            let aw = pw[spn_p + i];
            if alpha == 1.0 {
                cw.x += aw.x;
                cw.y += aw.y;
                if aw.z != NOZ {
                    cw.z += aw.z;
                } else {
                    cw.z = NOZ;
                }
                if aw.w != NOW {
                    cw.w += aw.w;
                } else {
                    cw.w = NOW;
                }
            } else if alpha != 0.0 {
                cw.x += alpha * aw.x;
                cw.y += alpha * aw.y;
                if aw.z != NOZ {
                    cw.z += alpha * aw.z;
                } else {
                    cw.z = NOZ;
                }
                if aw.w != NOW {
                    cw.w += alpha * aw.w;
                } else {
                    cw.w = NOW;
                }
            }
        }
        Ok(to_euclid(cw))
    }

    #[test]
    fn stack_basis_eval_is_bit_identical_to_the_heap_path() {
        // a fixed-seed LCG: deterministic control points, weights and knots
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut rnd = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut compared = 0;
        for p in 1..=7usize {
            for rational in [false, true] {
                let n_cpts = p + 1 + 4;
                let pw: Vec<CPoint> = (0..n_cpts)
                    .map(|_| {
                        let w = if rational { 0.25 + 2.0 * rnd() } else { NOW };
                        let (x, y, z) = (rnd() * 10.0 - 5.0, rnd() * 10.0 - 5.0, rnd() * 3.0);
                        if rational {
                            CPoint::new(x * w, y * w, z * w, w)
                        } else {
                            CPoint::new(x, y, z, w)
                        }
                    })
                    .collect();
                // clamped knots with non-uniform interior knots (a double knot from
                // degree 2 on)
                let mut u = vec![0.0; p + 1];
                let mut t = 0.0;
                for k in 0..n_cpts - p - 1 {
                    if k != 2 || p < 2 {
                        t += 0.1 + rnd();
                    }
                    u.push(t);
                }
                let end = t + 0.1 + rnd();
                u.extend(vec![end; p + 1]);
                let c = Curve::new(pw, p, u.clone()).unwrap();
                let knt = c.knot_vector().unwrap();
                let mut params: Vec<f64> = (1..64).map(|j| end * f64::from(j) / 64.0).collect();
                params.extend(u.iter().copied()); // knots themselves, domain ends included
                for &x in &params {
                    for side in [Side::Left, Side::Right] {
                        let old = old_basis_eval(&knt, p, x, side);
                        let new = basis_eval(&knt, p, x, side);
                        match (&old, &new) {
                            (Ok(a), Ok(b)) => {
                                assert_eq!(a.span, b.span);
                                let ab: Vec<u64> = a.values.iter().map(|v| v.to_bits()).collect();
                                let bb: Vec<u64> = b.values.iter().map(|v| v.to_bits()).collect();
                                assert_eq!(ab, bb, "p {p} u {x}");
                            }
                            (Err(a), Err(b)) => assert_eq!(format!("{a:?}"), format!("{b:?}")),
                            _ => panic!("p {p} u {x}: {old:?} vs {new:?}"),
                        }
                        let old = old_crv_eval(&c, x, side);
                        let new = crv_eval(&c, x, side);
                        match (&old, &new) {
                            (Ok(a), Ok(b)) => {
                                assert_eq!(
                                    [a.x.to_bits(), a.y.to_bits(), a.z.to_bits()],
                                    [b.x.to_bits(), b.y.to_bits(), b.z.to_bits()],
                                    "p {p} rational {rational} u {x}"
                                );
                                compared += 1;
                            }
                            (Err(a), Err(b)) => assert_eq!(format!("{a:?}"), format!("{b:?}")),
                            _ => panic!("p {p} u {x}: {old:?} vs {new:?}"),
                        }
                    }
                }
            }
        }
        assert!(compared > 1000, "{compared}");
    }

    #[test]
    fn basis_partition_of_unity() {
        let c = cubic();
        let knt = c.knot_vector().unwrap();
        for &u in &[0.05, 0.3, 0.55, 0.99] {
            let b = basis_eval(&knt, 3, u, Side::Left).unwrap();
            let s: f64 = b.values.iter().sum();
            assert!((s - 1.0).abs() < 1e-14);
            let d = basis_derivs(&knt, 3, u, Side::Left, 2).unwrap();
            assert_eq!(d.span, b.span);
            let mut t = 0.0;
            for i in 0..6 {
                t += basis_i_eval(&knt, i, 3, u, Side::Left).unwrap();
            }
            assert!((t - 1.0).abs() < 1e-14);
        }
        let (mx, u) = basis_find_global_max(&knt, 2, 3, 1e-10).unwrap();
        assert!(mx > 0.0 && mx <= 1.0 && u > 0.0 && u < 1.0);
        let sm = basis_find_all_span_maxima(&knt, 2, 3, 1e-10).unwrap();
        assert_eq!(sm.max.len(), 4);
        assert!(basis_i_eval(&knt, 6, 3, 0.5, Side::Left).is_err());
    }

    #[test]
    fn degenerate_curves() {
        let c = cubic();
        assert!(!is_ncrv_degenerate(&c, EFF_ZERO).unwrap());
        let pw = vec![CPoint::new(1.0, 1.0, 1.0, NOW); 2];
        let d = Curve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        assert!(is_ncrv_degenerate(&d, EFF_ZERO).unwrap());
    }

    #[test]
    fn absurd_derivative_orders_are_errors() {
        // fuzz regressions (nurbs.nurbs_crv fuzz00220, fuzz00094, fuzz00112): the
        // derivative rows were allocated with an aborting `vec!`
        let c = line(3);
        let knt = c.knot_vector().unwrap();
        // (sizes beyond the 47-bit address space, so no test machine can grant them)
        for der in [1usize << 44, 1 << 62, usize::MAX] {
            assert!(basis_derivs(&knt, 3, 0.5, Side::Left, der).is_err());
            assert!(crv_rational_basis_derivs(&c, 0.5, Side::Left, der).is_err());
            assert!(basis_i_derivs(&knt, 1, 3, 0.5, Side::Left, der).is_err());
        }
        assert!(allocate_nurb_curve(1 << 47, 3, 3).is_err());
        assert!(allocate_nurb_curve(3, 3, 1 << 48).is_err());
        assert!(allocate_nurb_curve(3, 3, 7).is_ok());
    }
}
