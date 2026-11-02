//! Knot refinement helpers (merging knot vectors with multiplicities, inserting a
//! knot vector into another, raising every knot's multiplicity) and all basis
//! functions with their derivatives at an array of parameters.
//!
//! Upstream: `source/SMLib/src/NL_BasisAdv.cpp` (declarations in
//! `source/SMLib/inc/NL_BasisAdv.h`).
//!
//! | Rust                                         | Upstream                        |
//! |----------------------------------------------|---------------------------------|
//! | [`compatible_knot_array_mult`]               | `N_GetCompatibleKnotArrayMult`  |
//! | [`insert_knots`]                             | `N_BasisInsertKnots`            |
//! | [`increase_knot_mult`]                       | `N_BasisIncreaseKnotMult`       |
//! | [`basis_derivs_array`], [`BasisDerivsArray`] | `N_BasisDerivsArray`            |
//!
//! Deviations:
//! - Out-params become return values. Knot vectors that upstream rescales in place are
//!   `&mut [f64]` (`U[0..=m]`, `m = len - 1`); the output knot vectors that upstream
//!   expects the caller to allocate are returned as `Vec`s sized by the callee.
//! - Errors are [`SolidError::Nlib`] values carrying the upstream `NL_*_ERR` number. On
//!   error only the error is returned (upstream may leave outputs partly written; the
//!   in-place rescale of [`compatible_knot_array_mult`] is kept, as upstream).
//! - Every array access is bounds checked. Where upstream would read or write outside
//!   its arrays (knot vectors too short for the degree, an empty insert vector,
//!   unclamped knots or degree 0 in `N_GetCompatibleKnotArrayMult`, parameters past the
//!   domain end in `N_BasisDerivsArray`), the functions return
//!   [`SolidError::InvalidInput`] instead.
//! - `N_BasisInsertKnots` compares `R[i] < X[j]` once more after inserting the last new
//!   knot, reading `X[mx+1]`. Whatever that slot holds, the output is the same (the next
//!   iteration copies `R[i]` anyway), so Rust skips the comparison when `j > mx`.
//! - Where upstream would loop forever (NaN knots in `N_GetCompatibleKnotArrayMult` or
//!   `N_BasisInsertKnots`: no index advances), the functions return
//!   [`SolidError::InvalidInput`]; the loop state is unchanged, so upstream would repeat
//!   it forever.
//! - `N_BasisDerivsArray` scales the derivatives by `p!/(p-k)!` in an `int`; where that
//!   product overflows (signed overflow is undefined upstream, and the product is formed
//!   one step past the last row it scales) Rust returns [`SolidError::InvalidInput`].
//! - The multiplicity increment `t` of `N_BasisIncreaseKnotMult` is a `usize` (a
//!   negative `t` inserts nothing upstream).
//! - Degrees whose initial `p + 1` index cannot fit an `i64` return
//!   [`SolidError::InvalidInput`]. Multiplicity sums are checked before comparison;
//!   a sum that overflows is above `p` and returns [`NlibCode::Knot`].
//! - [`basis_derivs_array`] checks its output and temporary storage, including
//!   nested-vector metadata, against [`MAX_DERIVS_ARRAY_BYTES`] before allocating.
//!   This prevents overcommitting allocators from accepting hostile dimensions.
//!   Actual buffers are reserved fallibly; allocation failure is [`NlibCode::Memory`].

use crate::error::{NlibCode, Result, SolidError};
use crate::nurbs::basis_basic::{reparam, PTOL};
use crate::nurbs::geom::Interval;
use crate::nurbs::knots::{KnotVector, Side};

const OP_COMPAT_MULT: &str = "nurbs::basis_adv::compatible_knot_array_mult";
const OP_INSERT: &str = "nurbs::basis_adv::insert_knots";
const OP_INC_MULT: &str = "nurbs::basis_adv::increase_knot_mult";
const OP_DERIVS_ARRAY: &str = "nurbs::basis_adv::basis_derivs_array";

/// Maximum requested heap bytes for one [`basis_derivs_array`] call (64 MiB),
/// including output values, nested-vector metadata, spans and temporary workspace.
pub const MAX_DERIVS_ARRAY_BYTES: usize = 64 * 1024 * 1024;

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

/// `a[i]` for a signed index, or `InvalidInput` when out of range.
#[inline]
fn rdi(a: &[f64], i: i64, op: &'static str) -> Result<f64> {
    let i = usize::try_from(i).map_err(|_| out_of_range(op))?;
    a.get(i).copied().ok_or_else(|| out_of_range(op))
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
    let slot = a
        .get_mut(i)
        .and_then(|row| row.get_mut(j))
        .ok_or_else(|| out_of_range(op))?;
    *slot = v;
    Ok(())
}

/// Index as `i64` (indices here are bounded by slice lengths).
#[inline]
fn sgn(v: usize, op: &'static str) -> Result<i64> {
    i64::try_from(v).map_err(|_| out_of_range(op))
}

/// Highest index `m` of a non-empty slice, as `i64`.
fn last_index(a: &[f64], op: &'static str) -> Result<i64> {
    let m = a.len().checked_sub(1).ok_or(SolidError::InvalidInput {
        operation: op,
        reason: "empty knot vector",
    })?;
    sgn(m, op)
}

/// Computes the knots that merge two knot vectors of the same degree `p` and the same
/// end multiplicities, keeping multiplicities (upstream `N_GetCompatibleKnotArrayMult`):
/// returns `(xr, xs)`, the knots to insert into `r` and `s`. After the insertions both
/// vectors have the same distinct inner knots, each with the higher of its two
/// multiplicities.
///
/// When the ends of `r` and `s` differ by more than [`PTOL`], the vector with the
/// shorter parameter range is first rescaled **in place** ([`reparam`]) to the range of
/// the other (`s` when `r`'s range is longer, else `r`). Inner knots closer than
/// [`PTOL`] count as aligned.
///
/// # Errors
///
/// `InvalidInput` where upstream would read outside the knot vectors (vectors too
/// short for `p`, degree 0, unclamped vectors or inner knots beyond the ends) or loop
/// forever (NaN knots), and the errors of [`reparam`].
pub fn compatible_knot_array_mult(
    r: &mut [f64],
    s: &mut [f64],
    p: usize,
) -> Result<(Vec<f64>, Vec<f64>)> {
    const OP: &str = OP_COMPAT_MULT;
    let mr = last_index(r, OP)?;
    let ms = last_index(s, OP)?;

    // Scale.
    let (r0, rm) = (rdi(r, 0, OP)?, rdi(r, mr, OP)?);
    let (s0, sm) = (rdi(s, 0, OP)?, rdi(s, ms, OP)?);
    if (r0 - s0).abs() > PTOL || (rm - sm).abs() > PTOL {
        if (rm - r0) > (sm - s0) {
            reparam(s, p, Interval::new(r0, rm))?;
        } else {
            reparam(r, p, Interval::new(s0, sm))?;
        }
    }

    let ps = sgn(p, OP)?;
    let er = mr - ps;
    let es = ms - ps;
    let mut i = ps.checked_add(1).ok_or_else(|| out_of_range(OP))?;
    let mut j = i;
    let mut xr = Vec::new();
    let mut xs = Vec::new();
    let (r, s) = (&*r, &*s);

    // Find knots to be inserted.
    while i < er || j < es {
        let (i_start, j_start) = (i, j);
        if (rdi(r, i, OP)? - rdi(s, j, OP)?).abs() < PTOL {
            let ir = i;
            while i < er && rdi(r, i, OP)? == rdi(r, i + 1, OP)? {
                i += 1;
            }
            let mlr = i - ir + 1;

            let js = j;
            while j < es && rdi(s, j, OP)? == rdi(s, j + 1, OP)? {
                j += 1;
            }
            let mls = j - js + 1;

            for _ in (mlr + 1)..=mls {
                xr.push(rdi(s, j, OP)?);
            }
            for _ in (mls + 1)..=mlr {
                xs.push(rdi(r, i, OP)?);
            }
            i += 1;
            j += 1;
        }

        if rdi(r, i, OP)? < rdi(s, j, OP)? {
            let ir = i;
            while i < er && rdi(r, i, OP)? == rdi(r, i + 1, OP)? {
                i += 1;
            }
            let mlr = i - ir + 1;
            for _ in 1..=mlr {
                xs.push(rdi(r, i, OP)?);
            }
            i += 1;
        }

        if rdi(r, i, OP)? > rdi(s, j, OP)? {
            let js = j;
            while j < es && rdi(s, j, OP)? == rdi(s, j + 1, OP)? {
                j += 1;
            }
            let mls = j - js + 1;
            for _ in 1..=mls {
                xr.push(rdi(s, j, OP)?);
            }
            j += 1;
        }

        if i == i_start && j == j_start {
            // Neither index moved: upstream repeats this iteration forever.
            return Err(endless(OP));
        }
    }

    Ok((xr, xs))
}

/// Refines the knot vector `r` (degree `p`) with the knots `x` (upstream
/// `N_BasisInsertKnots`): returns the merged knot vector of `r.len() + x.len()` knots.
/// When `r` has fewer than `2p + 2` knots (only an unsorted `r` passes the range check)
/// it returns `2p + 2 + x.len()` knots, the ones upstream writes past its documented
/// output size (upstream-issues UB-196).
///
/// The new knots must fit into the old ones, `U[p] < X[0] <= ... <= X[mx] < U[m-p]`;
/// a new knot equal to an old one is placed before it.
///
/// # Errors
///
/// [`NlibCode::Knot`] when `X[0] <= U[p]` or `X[mx] >= U[m-p]`; `InvalidInput` when `x`
/// is empty or `r` has fewer than `p + 1` knots (upstream would read out of bounds), or
/// where upstream would loop forever (NaN knots).
pub fn insert_knots(r: &[f64], p: usize, x: &[f64]) -> Result<Vec<f64>> {
    const OP: &str = OP_INSERT;
    let mr = last_index(r, OP)?;
    let mx = last_index(x, OP)?;
    let ps = sgn(p, OP)?;
    let er = mr - ps;

    if rdi(x, 0, OP)? <= rdi(r, ps, OP)? || rdi(x, mx, OP)? >= rdi(r, er, OP)? {
        return Err(nlib(OP, NlibCode::Knot));
    }

    // Refine knot vector.
    let mut s = Vec::with_capacity(r.len() + x.len());
    for i in 0..=ps {
        s.push(rdi(r, i, OP)?);
    }

    let mut i = ps + 1;
    let mut j: i64 = 0;
    while i < er || j <= mx {
        if i >= er {
            s.push(rdi(x, j, OP)?);
            j += 1;
        } else if j > mx {
            s.push(rdi(r, i, OP)?);
            i += 1;
        } else {
            let (i_start, j_start) = (i, j);
            if rdi(r, i, OP)? >= rdi(x, j, OP)? {
                s.push(rdi(x, j, OP)?);
                j += 1;
            }
            // Upstream reads X[mx+1] here once j passed mx; the result never depends
            // on it (see the module docs).
            if j <= mx && rdi(r, i, OP)? < rdi(x, j, OP)? {
                s.push(rdi(r, i, OP)?);
                i += 1;
            }
            if i == i_start && j == j_start {
                // NaN knots: upstream repeats this iteration forever.
                return Err(endless(OP));
            }
        }
    }

    for i in er..=mr {
        s.push(rdi(r, i, OP)?);
    }
    Ok(s)
}

/// The knots to insert into `u` (degree `p`) to raise the multiplicity of every
/// distinct inner knot by `t` (upstream `N_BasisIncreaseKnotMult`): each inner knot is
/// repeated `t` times, in order.
///
/// # Errors
///
/// [`NlibCode::Knot`] when an inner knot would reach a multiplicity above `p`.
pub fn increase_knot_mult(u: &[f64], p: usize, t: usize) -> Result<Vec<f64>> {
    const OP: &str = OP_INC_MULT;
    let mu = last_index(u, OP)?;
    let ps = sgn(p, OP)?;
    let ts = sgn(t, OP)?;
    let eu = mu - ps;

    let mut x = Vec::new();
    let mut i = ps.checked_add(1).ok_or_else(|| out_of_range(OP))?;

    // Find knots to be inserted.
    while i < eu {
        let k = i;
        while i < eu && rdi(u, i, OP)? == rdi(u, i + 1, OP)? {
            i += 1;
        }
        // All terms are nonnegative: an overflowing sum necessarily exceeds p.
        if (i - k)
            .checked_add(ts)
            .and_then(|mult| mult.checked_add(1))
            .is_none_or(|mult| mult > ps)
        {
            return Err(nlib(OP, NlibCode::Knot));
        }
        let v = rdi(u, i, OP)?;
        for _ in 1..=t {
            x.push(v);
        }
        i += 1;
    }
    Ok(x)
}

/// All non-vanishing basis functions and their derivatives at an array of parameters,
/// as returned by [`basis_derivs_array`].
#[derive(Debug, Clone, PartialEq)]
pub struct BasisDerivsArray {
    /// `spans[j]`: knot span index `l` of parameter `u[j]`.
    pub spans: Vec<usize>,
    /// `derivs[k][i][j]`: `k`-th derivative of `N[l-p+i, p]` at `u[j]` (`l = spans[j]`),
    /// `k = 0..=der`, `i = 0..=p`, `j = 0..u.len()` (rows `k > p` are zero).
    pub derivs: Vec<Vec<Vec<f64>>>,
}

/// Check every output and workspace dimension before allocating, not by probing
/// the allocator: virtual-memory reservations can succeed without enough RAM.
fn derivs_array_dimensions(p: usize, der: usize, cnt: usize) -> Result<(usize, usize)> {
    let too_big = || SolidError::InvalidInput {
        operation: OP_DERIVS_ARRAY,
        reason: "basis derivative storage exceeds MAX_DERIVS_ARRAY_BYTES",
    };
    let rows = der.checked_add(1).ok_or_else(too_big)?;
    let w = p.checked_add(1).ok_or_else(too_big)?;
    let columns = rows.checked_mul(w).ok_or_else(too_big)?;
    let values = columns.checked_mul(cnt).ok_or_else(too_big)?;
    // ndu[w][w], left[w], right[w], and a[2][w].
    let workspace = w
        .checked_mul(w)
        .and_then(|square| w.checked_mul(4).and_then(|v| square.checked_add(v)))
        .ok_or_else(too_big)?;
    let workspace_rows = w.checked_add(2).ok_or_else(too_big)?;
    let mut bytes = 0usize;
    for (count, element_size) in [
        (values, size_of::<f64>()),
        (columns, size_of::<Vec<f64>>()),
        (rows, size_of::<Vec<Vec<f64>>>()),
        (workspace, size_of::<f64>()),
        (workspace_rows, size_of::<Vec<f64>>()),
        (cnt, size_of::<usize>()),
        (p.min(der), size_of::<i32>()),
    ] {
        bytes = count
            .checked_mul(element_size)
            .and_then(|size| bytes.checked_add(size))
            .ok_or_else(too_big)?;
    }
    if bytes > MAX_DERIVS_ARRAY_BYTES {
        return Err(too_big());
    }
    Ok((rows, w))
}

/// Reserve the buffer actually used, rather than a temporary allocation probe.
fn filled<T: Clone>(len: usize, value: T, op: &'static str) -> Result<Vec<T>> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(len)
        .map_err(|_| nlib(op, NlibCode::Memory))?;
    result.resize(len, value);
    Ok(result)
}

fn zeros2(rows: usize, cols: usize, op: &'static str) -> Result<Vec<Vec<f64>>> {
    let mut result = filled(rows, Vec::new(), op)?;
    for row in &mut result {
        *row = filled(cols, 0.0, op)?;
    }
    Ok(result)
}

/// A zero-filled array whose dimensions have passed [`derivs_array_dimensions`].
fn zeros3(rows: usize, cols: usize, cnt: usize, op: &'static str) -> Result<Vec<Vec<Vec<f64>>>> {
    let mut result = filled(rows, Vec::new(), op)?;
    for row in &mut result {
        *row = zeros2(cols, cnt, op)?;
    }
    Ok(result)
}

/// All non-vanishing basis functions of degree `p` and their derivatives up to order
/// `der` at the parameters `u[0..=n]` (upstream `N_BasisDerivsArray`, The NURBS Book
/// A2.3 applied to a parameter array).
///
/// The spans of `u[0]` and `u[n]` come from [`KnotVector::find_span`]; the spans of
/// `u[1..n]` are found by walking forward from the span of `u[0]` (`u` should be
/// ascending). With [`Side::Left`] the walk stops at the first `U[l+1] > u[j]`, with
/// [`Side::Right`] at the first `U[l+1] >= u[j]`.
///
/// # Errors
///
/// Errors of [`KnotVector::find_span`]; `InvalidInput` when `u` is empty, where
/// upstream would index out of bounds (an inner parameter at or past the domain end
/// with [`Side::Left`], or past it with [`Side::Right`]), or overflow its `int`
/// derivative factor, or the output and workspace exceed [`MAX_DERIVS_ARRAY_BYTES`].
/// [`NlibCode::Memory`] if a buffer cannot be reserved.
pub fn basis_derivs_array(
    knt: &KnotVector<'_>,
    p: usize,
    u: &[f64],
    side: Side,
    der: usize,
) -> Result<BasisDerivsArray> {
    const OP: &str = OP_DERIVS_ARRAY;
    let uk = knt.knots();
    let n = last_index(u, OP)?;
    let cnt = u.len();
    let (rows, w) = derivs_array_dimensions(p, der, cnt)?;
    let ps = sgn(p, OP)?;

    // Compute span array.
    let mut spn = filled(cnt, 0usize, OP)?;
    let un = rdi(u, n, OP)?;
    let last = knt.find_span(p, un, side)?;
    let first = knt.find_span(p, rdi(u, 0, OP)?, side)?;
    let mut k = sgn(first, OP)?;
    if let Some(slot) = spn.last_mut() {
        *slot = last;
    }
    if let Some(slot) = spn.first_mut() {
        *slot = first;
    }
    for (i, slot) in spn
        .iter_mut()
        .enumerate()
        .take(cnt.saturating_sub(1))
        .skip(1)
    {
        let ui = rdi(u, sgn(i, OP)?, OP)?;
        // Upstream's NL_LEFT case falls through into the NL_RIGHT loop.
        if side == Side::Left {
            while ui >= rdi(uk, k + 1, OP)? {
                k += 1;
            }
        }
        while ui > rdi(uk, k + 1, OP)? {
            k += 1;
        }
        *slot = usize::try_from(k).map_err(|_| out_of_range(OP))?;
    }

    // Get maximum derivative index; rows above p stay zero.
    let mder = sgn(p.min(der), OP)?;
    let mut nd = zeros3(rows, w, cnt, OP)?;

    // Derivative factors p!/(p-k)! (an `int` upstream, formed one step past the last
    // row it scales).
    let overflow = || SolidError::InvalidInput {
        operation: OP,
        reason: "derivative factor overflows upstream int",
    };
    let mut factors = Vec::new();
    factors
        .try_reserve_exact(p.min(der))
        .map_err(|_| nlib(OP, NlibCode::Memory))?;
    let mut fac = i32::try_from(p).map_err(|_| overflow())?;
    for kk in 1..=mder {
        factors.push(fac);
        let pk = i32::try_from(ps - kk).map_err(|_| overflow())?;
        fac = fac.checked_mul(pk).ok_or_else(overflow)?;
    }

    let mut left = filled(w, 0.0, OP)?;
    let mut right = filled(w, 0.0, OP)?;
    let mut ndu = zeros2(w, w, OP)?;
    let mut a = zeros2(2, w, OP)?;

    // Compute the basis functions.
    for (j, (&uj, &l)) in u.iter().zip(&spn).enumerate() {
        let l = sgn(l, OP)?;
        wr2(&mut ndu, 0, 0, 1.0, OP)?;

        for i in 1..=ps {
            let iu = usize::try_from(i).map_err(|_| out_of_range(OP))?;
            let li = uj - rdi(uk, l + 1 - i, OP)?;
            let ri = rdi(uk, l + i, OP)? - uj;
            *left.get_mut(iu).ok_or_else(|| out_of_range(OP))? = li;
            *right.get_mut(iu).ok_or_else(|| out_of_range(OP))? = ri;
            let mut saved = 0.0;

            for r in 0..i {
                let rr = rdi(&right, r + 1, OP)?;
                let ll = rdi(&left, i - r, OP)?;
                wr2(&mut ndu, i, r, rr + ll, OP)?;
                let temp = rd2(&ndu, r, i - 1, OP)? / rd2(&ndu, i, r, OP)?;
                wr2(&mut ndu, r, i, saved + rr * temp, OP)?;
                saved = ll * temp;
            }
            wr2(&mut ndu, i, i, saved, OP)?;
        }

        // Load basis functions.
        let row0 = nd.first_mut().ok_or_else(|| out_of_range(OP))?;
        for (i, col) in row0.iter_mut().enumerate() {
            let v = rd2(&ndu, sgn(i, OP)?, ps, OP)?;
            *col.get_mut(j).ok_or_else(|| out_of_range(OP))? = v;
        }

        // Compute derivatives.
        for r in 0..=ps {
            let mut s1: i64 = 0;
            let mut s2: i64 = 1;
            wr2(&mut a, 0, 0, 1.0, OP)?;

            for kk in 1..=mder {
                let mut d = 0.0;
                let rk = r - kk;
                let pk = ps - kk;

                if r >= kk {
                    let v = rd2(&a, s1, 0, OP)? / rd2(&ndu, pk + 1, rk, OP)?;
                    wr2(&mut a, s2, 0, v, OP)?;
                    d = v * rd2(&ndu, rk, pk, OP)?;
                }

                let i1 = if rk >= -1 { 1 } else { -rk };
                let i2 = if r - 1 <= pk { kk - 1 } else { ps - r };

                for i in i1..=i2 {
                    let v = (rd2(&a, s1, i, OP)? - rd2(&a, s1, i - 1, OP)?)
                        / rd2(&ndu, pk + 1, rk + i, OP)?;
                    wr2(&mut a, s2, i, v, OP)?;
                    d += v * rd2(&ndu, rk + i, pk, OP)?;
                }

                if r <= pk {
                    let v = -rd2(&a, s1, kk - 1, OP)? / rd2(&ndu, pk + 1, r, OP)?;
                    wr2(&mut a, s2, kk, v, OP)?;
                    d += v * rd2(&ndu, r, pk, OP)?;
                }

                let slot = usize::try_from(kk)
                    .ok()
                    .and_then(|kk| nd.get_mut(kk))
                    .and_then(|row| usize::try_from(r).ok().and_then(|r| row.get_mut(r)))
                    .and_then(|col| col.get_mut(j))
                    .ok_or_else(|| out_of_range(OP))?;
                *slot = d;
                std::mem::swap(&mut s1, &mut s2);
            }
        }

        // Multiply through by the correct factors.
        for (row, &f) in nd.iter_mut().skip(1).zip(&factors) {
            let f = f64::from(f);
            for col in row.iter_mut() {
                *col.get_mut(j).ok_or_else(|| out_of_range(OP))? *= f;
            }
        }
    }

    Ok(BasisDerivsArray {
        spans: spn,
        derivs: nd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::basis_basic::basis_derivs;

    fn kv(k: &[f64]) -> KnotVector<'_> {
        KnotVector::from_slice(k).unwrap()
    }

    #[test]
    fn mult_merge_keeps_multiplicities() {
        let mut r = vec![0., 0., 0., 0.3, 0.5, 0.5, 1., 1., 1.];
        let mut s = vec![0., 0., 0., 0.2, 0.5, 0.7, 0.7, 1., 1., 1.];
        let (xr, xs) = compatible_knot_array_mult(&mut r, &mut s, 2).unwrap();
        assert_eq!(xr, vec![0.2, 0.7, 0.7]);
        assert_eq!(xs, vec![0.3, 0.5]);
    }

    #[test]
    fn mult_merge_rescales_shorter_range() {
        let mut r = vec![0., 0., 1., 2., 2.];
        let mut s = vec![0., 0., 0.25, 1., 1.];
        let (xr, xs) = compatible_knot_array_mult(&mut r, &mut s, 1).unwrap();
        assert_eq!(s, vec![0., 0., 0.5, 2., 2.]);
        assert_eq!(xr, vec![0.5]);
        assert_eq!(xs, vec![1.0]);
        let mut a = vec![0., 0., f64::NAN, 1., 1.];
        let mut b = vec![0., 0., 0.5, 1., 1.];
        assert!(matches!(
            compatible_knot_array_mult(&mut a, &mut b, 1),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn insert_merges_and_checks_range() {
        let r = [0., 0., 0., 0.5, 1., 1., 1.];
        let s = insert_knots(&r, 2, &[0.25, 0.5, 0.75]).unwrap();
        assert_eq!(s, vec![0., 0., 0., 0.25, 0.5, 0.5, 0.75, 1., 1., 1.]);
        assert!(matches!(
            insert_knots(&r, 2, &[0.0, 0.5]),
            Err(SolidError::Nlib {
                code: NlibCode::Knot,
                ..
            })
        ));
        assert!(matches!(
            insert_knots(&r, 2, &[0.5, 1.0]),
            Err(SolidError::Nlib {
                code: NlibCode::Knot,
                ..
            })
        ));
        assert!(matches!(
            insert_knots(&r, 2, &[]),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(insert_knots(&[0.0, 1.0], 3, &[0.5]).is_err());
    }

    #[test]
    fn increase_mult_repeats_inner_knots() {
        let u = [0., 0., 0., 0., 0.3, 0.5, 0.5, 1., 1., 1., 1.];
        assert_eq!(increase_knot_mult(&u, 3, 1).unwrap(), vec![0.3, 0.5]);
        assert!(matches!(
            increase_knot_mult(&u, 3, 2),
            Err(SolidError::Nlib {
                code: NlibCode::Knot,
                ..
            })
        ));
        assert_eq!(
            increase_knot_mult(&[0., 0., 1., 1.], 1, 5).unwrap(),
            Vec::<f64>::new()
        );
    }

    #[test]
    fn compatible_mult_rejects_degree_that_overflows_signed_index() {
        let Ok(p) = usize::try_from(i64::MAX) else {
            return;
        };
        let mut r = [0.0, 0.0, 1.0, 1.0];
        let mut s = r;
        assert!(matches!(
            compatible_knot_array_mult(&mut r, &mut s, p),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn increase_mult_rejects_degree_that_overflows_signed_index() {
        let Ok(p) = usize::try_from(i64::MAX) else {
            return;
        };
        assert!(matches!(
            increase_knot_mult(&[0.0, 0.0, 1.0, 1.0], p, 1),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn increase_mult_rejects_increment_that_overflows_signed_sum() {
        let Ok(t) = usize::try_from(i64::MAX) else {
            return;
        };
        for (u, p) in [
            (&[0.0, 0.0, 0.5, 1.0, 1.0][..], 1),
            (&[0.0, 0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0][..], 2),
        ] {
            // A single inner knot overflows the final +1; a repeated knot
            // overflows the preceding addition of t as well.
            assert!(matches!(
                increase_knot_mult(u, p, t),
                Err(SolidError::Nlib {
                    code: NlibCode::Knot,
                    ..
                })
            ));
        }
    }

    #[test]
    fn derivs_array_matches_single_derivs() {
        let k = [0.0, 0.0, 0.0, 0.0, 0.3, 0.5, 0.5, 0.7, 1.0, 1.0, 1.0, 1.0];
        let knt = kv(&k);
        let u = [0.0, 0.1, 0.3, 0.42, 0.5, 0.6, 0.99, 1.0];
        for side in [Side::Left, Side::Right] {
            let arr = basis_derivs_array(&knt, 3, &u, side, 4).unwrap();
            assert_eq!(arr.derivs.len(), 5);
            for (j, &uj) in u.iter().enumerate() {
                let one = basis_derivs(&knt, 3, uj, side, 4).unwrap();
                assert_eq!(one.span, arr.spans[j], "u = {uj}");
                for kd in 0..=4 {
                    for i in 0..=3 {
                        let (a, b) = (arr.derivs[kd][i][j], one.derivs[kd][i]);
                        assert!((a - b).abs() <= 1e-9 * a.abs().max(1.0), "{a} {b}");
                    }
                }
            }
        }
    }

    #[test]
    fn derivs_array_refuses_runaway_derivative_storage() {
        // The saved fuzz input reached a successful virtual reservation, then
        // tried to materialize billions of nested vectors on macOS.
        let knots = [
            -0.6825879003318405,
            1.7695994926457606,
            4.848088756954498,
            6.2545297135587035,
            18.924315562186447,
            19.636574334775027,
            22.704326541087685,
            26.36179241269924,
        ];
        let u = [
            4.848088756954498,
            11.33749281771211,
            17.706121534281504,
            18.584008024367456,
            18.924315562186447,
            19.63657433477503,
        ];
        for der in [1usize << 31, usize::MAX] {
            assert!(matches!(
                basis_derivs_array(&kv(&knots), 3, &u, Side::Right, der),
                Err(SolidError::InvalidInput {
                    operation: OP_DERIVS_ARRAY,
                    reason: "basis derivative storage exceeds MAX_DERIVS_ARRAY_BYTES",
                })
            ));
        }
    }

    #[test]
    fn derivs_array_budget_includes_vector_metadata_and_square_workspace() {
        // Values alone would fit, but the two levels of vector headers do not.
        let rows = MAX_DERIVS_ARRAY_BYTES / size_of::<Vec<f64>>();
        assert!(rows * size_of::<f64>() < MAX_DERIVS_ARRAY_BYTES);
        assert!(matches!(
            basis_derivs_array(&kv(&[0.0, 1.0]), 0, &[0.5], Side::Left, rows - 1),
            Err(SolidError::InvalidInput { .. })
        ));

        // One output row is small; the degree-squared temporary matrix is not.
        let degree = 4096;
        let mut knots = vec![0.0; degree + 1];
        knots.extend(vec![1.0; degree + 1]);
        assert!(matches!(
            basis_derivs_array(&kv(&knots), degree, &[0.5], Side::Left, 0),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn derivs_array_storage_allocation_fails_softly() {
        assert!(matches!(
            filled(usize::MAX, 0.0f64, OP_DERIVS_ARRAY),
            Err(SolidError::Nlib {
                code: NlibCode::Memory,
                ..
            })
        ));
    }

    #[test]
    fn derivs_array_errors() {
        let k = [0.0, 0.0, 0.5, 1.0, 1.0];
        assert!(basis_derivs_array(&kv(&k), 1, &[], Side::Left, 1).is_err());
        // An inner parameter at the domain end walks past U[m] with NL_LEFT.
        assert!(matches!(
            basis_derivs_array(&kv(&k), 1, &[0.2, 1.0, 1.0], Side::Left, 1),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(basis_derivs_array(&kv(&k), 1, &[0.2, 1.0, 1.0], Side::Right, 1).is_ok());
        let mut long = vec![0.0; 14];
        long.extend(vec![1.0; 14]);
        assert!(basis_derivs_array(&kv(&long), 13, &[0.5], Side::Left, 10).is_err());
        assert!(basis_derivs_array(&kv(&long), 13, &[0.5], Side::Left, 9).is_ok());
    }
}
