//! Surface knot insertion and refinement, splitting, Bezier decomposition, iso-curve
//! and sub-patch extraction.
//!
//! Upstream: `source/SMLib/src/NL_SrfTool.cpp`, `source/SMLib/inc/NL_SrfTool.h`.
//!
//! | Rust                    | Upstream               |
//! |-------------------------|------------------------|
//! | [`insert_knot`]         | `N_SrfInsertKnot`      |
//! | [`split`]               | `N_SrfSplit`           |
//! | [`decompose_to_bez`]    | `N_SrfDecomposeToBez`  |
//! | [`extract_iso_crv`]     | `N_SrfExtractIsoCrv`   |
//! | [`insert_knots`]        | `N_SrfInsertKnots`     |
//! | [`extract_patch`]       | `N_SrfExtractPatch`    |
//!
//! Deviations:
//! - Out-params become return values: every routine returns new surfaces (or a new
//!   curve) instead of filling caller-provided ones. Upstream's in-place mode
//!   (`surP == surQ`) computes the same result into fresh arrays, so a caller
//!   replaces its surface with the returned one. The `NL_STACKS` arguments have no
//!   counterpart and allocation cannot fail.
//! - Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number
//!   instead of a flag plus the global `NL_ERROR` record.
//! - [`extract_patch`] takes the requested patch as a [`Rectangle`] instead of four
//!   corner parameters.
//! - `mt` of [`insert_knot`] is a `usize`, so upstream's `mt < 0` test cannot fail.
//! - Degrees above `32767` (upstream's `NL_DEGREE` is a `short`, so they cannot be
//!   expressed there) are `InvalidInput` in every `nurbs::srf_tool` routine; scratch
//!   and output arrays that cannot be allocated are `NL_MEM_ERR` (upstream's failed
//!   `N_Malloc`), and index arithmetic that would overflow (`2 * max(p, q)`) is
//!   `InvalidInput`.
//! - Every array access is bounds checked. Where upstream would index outside its
//!   arrays (invalid knot vectors, degrees above `NL_MAXDEG` for the fixed-size scratch
//!   arrays of `N_SrfSplit` / `N_SrfExtractIsoCrv`), the Rust functions return
//!   [`SolidError::InvalidInput`]. For valid clamped surfaces the results agree bit
//!   for bit (same operations in the same order).
//! - `N_SrfSplit` returns success without writing its outputs when the split knot's
//!   multiplicity exceeds the degree (a bad knot vector); [`split`] returns
//!   `InvalidInput` instead.
//! - `N_SrfExtractIsoCrv` returns its error flag without setting an error number
//!   when the knot multiplicity exceeds the degree (a bad knot vector);
//!   [`extract_iso_crv`] returns `InvalidInput` instead.

use crate::error::{NlibCode, Result, SolidError};
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::geom::Rectangle;
use crate::nurbs::knots::{KnotVector, Side, MAX_DEGREE};
use crate::nurbs::math::CPoint;
use crate::nurbs::srf_basic::{Direction, Surface};

/// Snap tolerance of [`extract_patch`] (hard coded upstream).
const PATCH_SNAP_TOL: f64 = 1.0e-08;

/// A control net, `Pw[i][j]`.
pub(super) type Net = Vec<Vec<CPoint>>;

pub(super) fn nlib(operation: &'static str, code: NlibCode) -> SolidError {
    SolidError::Nlib { operation, code }
}

pub(super) fn oob(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: "index out of range (invalid surface)",
    }
}

/// Upstream's signed index as `usize`.
pub(super) fn ix(i: isize, op: &'static str) -> Result<usize> {
    usize::try_from(i).map_err(|_| oob(op))
}

/// `usize` as upstream's signed index.
pub(super) fn sx(i: usize, op: &'static str) -> Result<isize> {
    isize::try_from(i).map_err(|_| oob(op))
}

/// `v[i]`.
pub(super) fn at<T: Copy>(v: &[T], i: isize, op: &'static str) -> Result<T> {
    v.get(ix(i, op)?).copied().ok_or_else(|| oob(op))
}

/// `v[i] = x`.
pub(super) fn put<T>(v: &mut [T], i: isize, x: T, op: &'static str) -> Result<()> {
    let slot = v.get_mut(ix(i, op)?).ok_or_else(|| oob(op))?;
    *slot = x;
    Ok(())
}

/// `w[i][j]`.
pub(super) fn at2<T: Copy>(w: &[Vec<T>], i: isize, j: isize, op: &'static str) -> Result<T> {
    let row = w.get(ix(i, op)?).ok_or_else(|| oob(op))?;
    at(row, j, op)
}

/// `w[i][j] = x`.
pub(super) fn put2<T>(w: &mut [Vec<T>], i: isize, j: isize, x: T, op: &'static str) -> Result<()> {
    let row = w.get_mut(ix(i, op)?).ok_or_else(|| oob(op))?;
    put(row, j, x, op)
}

/// `len` copies of `x`, or `NL_MEM_ERR` where the storage cannot be allocated (as
/// upstream's failed `N_Malloc`).
fn filled<T: Clone>(len: usize, x: T, op: &'static str) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len)
        .map_err(|_| nlib(op, NlibCode::Memory))?;
    v.resize(len, x);
    Ok(v)
}

/// A zero-filled `(n+1) x (m+1)` array (upstream allocates uninitialised storage).
/// `NL_MEM_ERR` where it cannot be allocated.
pub(super) fn zeros2<T: Clone + Default>(
    n: isize,
    m: isize,
    op: &'static str,
) -> Result<Vec<Vec<T>>> {
    let rows = ix(n.checked_add(1).ok_or_else(|| oob(op))?, op)?;
    let cols = ix(m.checked_add(1).ok_or_else(|| oob(op))?, op)?;
    let mut w = Vec::new();
    w.try_reserve_exact(rows)
        .map_err(|_| nlib(op, NlibCode::Memory))?;
    for _ in 0..rows {
        w.push(filled(cols, T::default(), op)?);
    }
    Ok(w)
}

/// A zero-filled array of `k + 1` entries; `NL_MEM_ERR` where it cannot be
/// allocated.
pub(super) fn zeros1<T: Clone + Default>(k: isize, op: &'static str) -> Result<Vec<T>> {
    let len = ix(k.checked_add(1).ok_or_else(|| oob(op))?, op)?;
    filled(len, T::default(), op)
}

/// `i + 1` on a signed index; `InvalidInput` where it overflows.
pub(super) fn succ(i: isize, op: &'static str) -> Result<isize> {
    i.checked_add(1).ok_or_else(|| oob(op))
}

/// `2 * a` on a signed index (scratch sizes `2 * max(p, q)`); `InvalidInput` where it
/// overflows (upstream's signed overflow is undefined).
pub(super) fn twice(a: isize, op: &'static str) -> Result<isize> {
    a.checked_mul(2).ok_or_else(|| oob(op))
}

/// Largest degree upstream can express (`NL_DEGREE` is a `short`).
pub(super) const DEGREE_RANGE_MAX: usize = 32767;

/// A degree as upstream's signed index; `InvalidInput` above
/// [`DEGREE_RANGE_MAX`] (not expressible upstream; bounds the index arithmetic).
pub(super) fn degree(p: usize, op: &'static str) -> Result<isize> {
    if p > DEGREE_RANGE_MAX {
        return Err(SolidError::InvalidInput {
            operation: op,
            reason: "degree above upstream's NL_DEGREE (short) range",
        });
    }
    sx(p, op)
}

/// Local notation of a surface (`N_SrfGetCPtsDegreesAndKnots`), indices signed.
pub(super) struct Local<'a> {
    pub(super) n: isize,
    pub(super) m: isize,
    pub(super) p: isize,
    pub(super) q: isize,
    pub(super) r: isize,
    pub(super) s: isize,
    pub(super) pw: Net,
    pub(super) u: &'a [f64],
    pub(super) v: &'a [f64],
}

impl<'a> Local<'a> {
    pub(super) fn of(sur: &'a Surface, op: &'static str) -> Result<Self> {
        let idx = sur.indices();
        Ok(Self {
            n: sx(idx.n, op)?,
            m: sx(idx.m, op)?,
            p: degree(idx.p, op)?,
            q: degree(idx.q, op)?,
            r: sx(idx.r, op)?,
            s: sx(idx.s, op)?,
            pw: sur.cpts(),
            u: sur.u_knots(),
            v: sur.v_knots(),
        })
    }
}

/// A surface from a net, signed degrees and knots.
pub(super) fn surface(
    net: Net,
    p: isize,
    q: isize,
    u: Vec<f64>,
    v: Vec<f64>,
    op: &'static str,
) -> Result<Surface> {
    Surface::new(net, ix(p, op)?, ix(q, op)?, u, v)
}

/// `(span, multiplicity)` of `t` for degree `p` (`N_BasisFindSpanAndMult`, `NL_LEFT`).
pub(super) fn span_mult(
    knots: &[f64],
    p: isize,
    t: f64,
    op: &'static str,
) -> Result<(isize, isize)> {
    let kv = KnotVector::from_slice(knots)?;
    let (sp, ml) = kv.find_span_and_mult(ix(p, op)?, t, Side::Left)?;
    Ok((sx(sp, op)?, sx(ml, op)?))
}

/// Span of `t` for degree `p` (`N_BasisFindSpan`, `NL_LEFT`).
fn span(knots: &[f64], p: isize, t: f64, op: &'static str) -> Result<isize> {
    let kv = KnotVector::from_slice(knots)?;
    sx(kv.find_span(ix(p, op)?, t, Side::Left)?, op)
}

/// `N_KnotVectorIsEndParam` as an error: `NL_PAR_ERR` when `t <= U[0]` or `t >= U[m]`.
pub(super) fn check_interior(knots: &[f64], t: f64, op: &'static str) -> Result<()> {
    let kv = KnotVector::from_slice(knots)?;
    if kv.is_end_param(t) {
        return Err(nlib(op, NlibCode::Parameter));
    }
    Ok(())
}

/// Inserts the knot `t` `mt` times into the u or v knot vector of `sur`
/// (`N_SrfInsertKnot`). `t` must be an interior parameter and the sum of its old and
/// new multiplicities must not exceed the degree in that direction.
///
/// Errors: [`NlibCode::Parameter`] for an end or outside parameter, or a total
/// multiplicity above the degree; [`NlibCode::Call`] for [`Direction::UV`].
pub fn insert_knot(sur: &Surface, t: f64, mt: usize, dir: Direction) -> Result<Surface> {
    const OP: &str = "nurbs::srf_tool::p1::insert_knot";
    let Local {
        n,
        m,
        p,
        q,
        r,
        s,
        pw,
        u: up,
        v: vp,
    } = Local::of(sur, OP)?;
    let mt = sx(mt, OP)?;

    // Check parameters and compute highest indexes.
    let (mut spu, mut mlu, mut spv, mut mlv) = (0, 0, 0, 0);
    let (ni, mi, ri, si) = match dir {
        Direction::U => {
            check_interior(up, t, OP)?;
            (spu, mlu) = span_mult(up, p, t, OP)?;
            // An overflowing sum exceeds the degree as well.
            if mlu.checked_add(mt).is_none_or(|x| x > p) {
                return Err(nlib(OP, NlibCode::Parameter));
            }
            (n + mt, m, r + mt, s)
        }
        Direction::V => {
            check_interior(vp, t, OP)?;
            (spv, mlv) = span_mult(vp, q, t, OP)?;
            // An overflowing sum exceeds the degree as well.
            if mlv.checked_add(mt).is_none_or(|x| x > q) {
                return Err(nlib(OP, NlibCode::Parameter));
            }
            (n, m + mt, r, s + mt)
        }
        Direction::UV => return Err(nlib(OP, NlibCode::Call)),
    };

    let mut qw: Net = zeros2(ni, mi, OP)?;
    let mut uq: Vec<f64> = zeros1(ri, OP)?;
    let mut vq: Vec<f64> = zeros1(si, OP)?;

    // Local memory.
    let a = p.max(q);
    let mut alf: Vec<Vec<f64>> = zeros2(a, a, OP)?;
    let mut oma: Vec<Vec<f64>> = zeros2(a, a, OP)?;
    let mut rw: Vec<CPoint> = zeros1(a, OP)?;

    if dir == Direction::U {
        // Save the alphas.
        for i in 1..=mt {
            let a = spu - p + i;
            for j in 0..=p - i - mlu {
                let x = (t - at(up, a + j, OP)?) / (at(up, spu + j + 1, OP)? - at(up, a + j, OP)?);
                put2(&mut alf, i, j, x, OP)?;
                put2(&mut oma, i, j, 1.0 - x, OP)?;
            }
        }

        // For each row do.
        for l in 0..=m {
            let mut a = spu - p;

            // Load auxiliary control points.
            for i in 0..=p - mlu {
                put(&mut rw, i, at2(&pw, a + i, l, OP)?, OP)?;
            }

            // Save unaltered control points.
            for i in 0..=a {
                put2(&mut qw, i, l, at2(&pw, i, l, OP)?, OP)?;
            }
            for i in spu - mlu..=n {
                put2(&mut qw, i + mt, l, at2(&pw, i, l, OP)?, OP)?;
            }

            // Now insert the knot.
            for i in 1..=mt {
                a = spu - p + i;
                for j in 0..=p - i - mlu {
                    let c = CPoint::combine2(
                        at2(&alf, i, j, OP)?,
                        at(&rw, j + 1, OP)?,
                        at2(&oma, i, j, OP)?,
                        at(&rw, j, OP)?,
                    );
                    put(&mut rw, j, c, OP)?;
                }
                put2(&mut qw, a, l, at(&rw, 0, OP)?, OP)?;
                put2(
                    &mut qw,
                    spu + mt - i - mlu,
                    l,
                    at(&rw, p - i - mlu, OP)?,
                    OP,
                )?;
            }

            // Load the remaining control points.
            for i in a + 1..spu - mlu {
                put2(&mut qw, i, l, at(&rw, i - a, OP)?, OP)?;
            }
        }

        // Load knot vectors.
        for i in 0..=spu {
            put(&mut uq, i, at(up, i, OP)?, OP)?;
        }
        for i in 1..=mt {
            put(&mut uq, i + spu, t, OP)?;
        }
        for i in spu + 1..=r {
            put(&mut uq, i + mt, at(up, i, OP)?, OP)?;
        }
        for i in 0..=s {
            put(&mut vq, i, at(vp, i, OP)?, OP)?;
        }
    }

    if dir == Direction::V {
        // Save the alphas.
        for i in 1..=mt {
            let a = spv - q + i;
            for j in 0..=q - i - mlv {
                let x = (t - at(vp, a + j, OP)?) / (at(vp, spv + j + 1, OP)? - at(vp, a + j, OP)?);
                put2(&mut alf, i, j, x, OP)?;
                put2(&mut oma, i, j, 1.0 - x, OP)?;
            }
        }

        // For each column do.
        for k in 0..=n {
            let mut a = spv - q;

            // Load auxiliary control points.
            for i in 0..=q - mlv {
                put(&mut rw, i, at2(&pw, k, a + i, OP)?, OP)?;
            }

            // Save unaltered control points.
            for i in 0..=a {
                put2(&mut qw, k, i, at2(&pw, k, i, OP)?, OP)?;
            }
            for i in spv - mlv..=m {
                put2(&mut qw, k, i + mt, at2(&pw, k, i, OP)?, OP)?;
            }

            // Now insert the knot.
            for i in 1..=mt {
                a = spv - q + i;
                for j in 0..=q - i - mlv {
                    let c = CPoint::combine2(
                        at2(&alf, i, j, OP)?,
                        at(&rw, j + 1, OP)?,
                        at2(&oma, i, j, OP)?,
                        at(&rw, j, OP)?,
                    );
                    put(&mut rw, j, c, OP)?;
                }
                put2(&mut qw, k, a, at(&rw, 0, OP)?, OP)?;
                put2(
                    &mut qw,
                    k,
                    spv + mt - i - mlv,
                    at(&rw, q - i - mlv, OP)?,
                    OP,
                )?;
            }

            // Load the remaining control points.
            for i in a + 1..spv - mlv {
                put2(&mut qw, k, i, at(&rw, i - a, OP)?, OP)?;
            }
        }

        // Load knot vectors.
        for i in 0..=spv {
            put(&mut vq, i, at(vp, i, OP)?, OP)?;
        }
        for i in 1..=mt {
            put(&mut vq, i + spv, t, OP)?;
        }
        for i in spv + 1..=s {
            put(&mut vq, i + mt, at(vp, i, OP)?, OP)?;
        }
        for i in 0..=r {
            put(&mut uq, i, at(up, i, OP)?, OP)?;
        }
    }

    surface(qw, p, q, uq, vq, OP)
}

/// Splits `sur` at the interior parameter `t` in u or v (`N_SrfSplit`). Returns the
/// left part over `[U[0], t]` (or `[V[0], t]`) and the right part over `[t, U[r]]`
/// (or `[t, V[s]]`).
///
/// Errors: [`NlibCode::Parameter`] for an end or outside parameter;
/// [`NlibCode::Call`] for [`Direction::UV`]; `InvalidInput` for a degree above
/// [`MAX_DEGREE`] or a knot multiplicity above the degree.
pub fn split(sur: &Surface, t: f64, dir: Direction) -> Result<(Surface, Surface)> {
    const OP: &str = "nurbs::srf_tool::p1::split";
    let Local {
        n,
        m,
        p,
        q,
        r,
        s,
        pw,
        u: up,
        v: vp,
    } = Local::of(sur, OP)?;

    // Fixed-size scratch arrays upstream (NL_MAXDEG + 1).
    let a = p.max(q);
    if ix(a, OP)? > MAX_DEGREE {
        return Err(oob(OP));
    }
    let mut alf: Vec<Vec<f64>> = zeros2(a, a, OP)?;
    let mut oma: Vec<Vec<f64>> = zeros2(a, a, OP)?;
    let mut sw: Vec<CPoint> = zeros1(a, OP)?;

    // Check parameters and compute highest indexes.
    let (mut spu, mut mlu, mut spv, mut mlv) = (0, 0, 0, 0);
    let (nl, ml, rl, sl, nr, mr, rr, sr) = match dir {
        Direction::U => {
            check_interior(up, t, OP)?;
            (spu, mlu) = span_mult(up, p, t, OP)?;
            if mlu > p {
                return Err(SolidError::InvalidInput {
                    operation: OP,
                    reason: "knot multiplicity above the degree",
                });
            }
            let nl = spu - mlu;
            let nr = n + p - spu;
            (nl, m, nl + p + 1, s, nr, m, nr + p + 1, s)
        }
        Direction::V => {
            check_interior(vp, t, OP)?;
            (spv, mlv) = span_mult(vp, q, t, OP)?;
            if mlv > q {
                return Err(SolidError::InvalidInput {
                    operation: OP,
                    reason: "knot multiplicity above the degree",
                });
            }
            let ml = spv - mlv;
            let mr = m + q - spv;
            (n, ml, r, ml + q + 1, n, mr, r, mr + q + 1)
        }
        Direction::UV => return Err(nlib(OP, NlibCode::Call)),
    };

    let mut lw: Net = zeros2(nl, ml, OP)?;
    let mut ul: Vec<f64> = zeros1(rl, OP)?;
    let mut vl: Vec<f64> = zeros1(sl, OP)?;
    let mut rw: Net = zeros2(nr, mr, OP)?;
    let mut ur: Vec<f64> = zeros1(rr, OP)?;
    let mut vr: Vec<f64> = zeros1(sr, OP)?;

    if dir == Direction::U {
        // Save the alphas.
        for i in 1..=p - mlu {
            let a = spu - p + i;
            for j in 0..=p - i - mlu {
                let x = (t - at(up, a + j, OP)?) / (at(up, spu + j + 1, OP)? - at(up, a + j, OP)?);
                put2(&mut alf, i, j, x, OP)?;
                put2(&mut oma, i, j, 1.0 - x, OP)?;
            }
        }

        // For each row do.
        let a = spu - p;
        for l in 0..=m {
            // Load auxiliary control points.
            for i in 0..=p - mlu {
                put(&mut sw, i, at2(&pw, a + i, l, OP)?, OP)?;
            }

            // Save unaltered control points.
            for i in 0..=a {
                put2(&mut lw, i, l, at2(&pw, i, l, OP)?, OP)?;
            }
            for i in spu - mlu..=n {
                put2(&mut rw, i - a, l, at2(&pw, i, l, OP)?, OP)?;
            }

            // Now split the surface.
            for i in 1..=p - mlu {
                let b = a + i;
                let c = p - mlu - i;
                for j in 0..=p - i - mlu {
                    let x = CPoint::combine2(
                        at2(&alf, i, j, OP)?,
                        at(&sw, j + 1, OP)?,
                        at2(&oma, i, j, OP)?,
                        at(&sw, j, OP)?,
                    );
                    put(&mut sw, j, x, OP)?;
                }
                put2(&mut lw, b, l, at(&sw, 0, OP)?, OP)?;
                put2(&mut rw, c, l, at(&sw, c, OP)?, OP)?;
            }
        }

        // Load knot vectors.
        let a = spu - mlu;
        for i in 0..=a {
            put(&mut ul, i, at(up, i, OP)?, OP)?;
        }
        for i in 0..=p {
            put(&mut ul, a + i + 1, t, OP)?;
        }
        let a = spu + 1;
        for i in 0..=p {
            put(&mut ur, i, t, OP)?;
        }
        for i in a..=r {
            put(&mut ur, i - a + p + 1, at(up, i, OP)?, OP)?;
        }
        for i in 0..=s {
            let x = at(vp, i, OP)?;
            put(&mut vr, i, x, OP)?;
            put(&mut vl, i, x, OP)?;
        }
    }

    if dir == Direction::V {
        // Save the alphas.
        for i in 1..=q - mlv {
            let a = spv - q + i;
            for j in 0..=q - i - mlv {
                let x = (t - at(vp, a + j, OP)?) / (at(vp, spv + j + 1, OP)? - at(vp, a + j, OP)?);
                put2(&mut alf, i, j, x, OP)?;
                put2(&mut oma, i, j, 1.0 - x, OP)?;
            }
        }

        // For each column do.
        let a = spv - q;
        for k in 0..=n {
            // Load auxiliary control points.
            for i in 0..=q - mlv {
                put(&mut sw, i, at2(&pw, k, a + i, OP)?, OP)?;
            }

            // Save unaltered control points.
            for i in 0..=a {
                put2(&mut lw, k, i, at2(&pw, k, i, OP)?, OP)?;
            }
            for i in spv - mlv..=m {
                put2(&mut rw, k, i - a, at2(&pw, k, i, OP)?, OP)?;
            }

            // Now split the surface.
            for i in 1..=q - mlv {
                let b = a + i;
                let c = q - mlv - i;
                for j in 0..=q - i - mlv {
                    let x = CPoint::combine2(
                        at2(&alf, i, j, OP)?,
                        at(&sw, j + 1, OP)?,
                        at2(&oma, i, j, OP)?,
                        at(&sw, j, OP)?,
                    );
                    put(&mut sw, j, x, OP)?;
                }
                put2(&mut lw, k, b, at(&sw, 0, OP)?, OP)?;
                put2(&mut rw, k, c, at(&sw, c, OP)?, OP)?;
            }
        }

        // Load knot vectors.
        let a = spv - mlv;
        for i in 0..=a {
            put(&mut vl, i, at(vp, i, OP)?, OP)?;
        }
        for i in 0..=q {
            put(&mut vl, a + i + 1, t, OP)?;
        }
        let a = spv + 1;
        for i in 0..=q {
            put(&mut vr, i, t, OP)?;
        }
        for i in a..=s {
            put(&mut vr, i - a + q + 1, at(vp, i, OP)?, OP)?;
        }
        for i in 0..=r {
            let x = at(up, i, OP)?;
            put(&mut ur, i, x, OP)?;
            put(&mut ul, i, x, OP)?;
        }
    }

    Ok((
        surface(lw, p, q, ul, vl, OP)?,
        surface(rw, p, q, ur, vr, OP)?,
    ))
}

/// Scratch storage of one Bezier patch of [`decompose_to_bez`].
#[derive(Clone)]
struct Patch {
    net: Net,
    u: Vec<f64>,
    v: Vec<f64>,
}

/// `patches[iq][jq]`.
fn patch_mut<'a>(
    patches: &'a mut [Vec<Patch>],
    iq: isize,
    jq: isize,
    op: &'static str,
) -> Result<&'a mut Patch> {
    patches
        .get_mut(ix(iq, op)?)
        .and_then(|row| row.get_mut(ix(jq, op).ok()?))
        .ok_or_else(|| oob(op))
}

/// `patches[iq][jq].net[k][j]`.
fn pcpt(
    patches: &[Vec<Patch>],
    ij: (isize, isize),
    k: isize,
    j: isize,
    op: &'static str,
) -> Result<CPoint> {
    let pt = patches
        .get(ix(ij.0, op)?)
        .and_then(|row| row.get(ix(ij.1, op).ok()?))
        .ok_or_else(|| oob(op))?;
    at2(&pt.net, k, j, op)
}

/// `patches[iq][jq].net[k][j] = x`.
fn pput(
    patches: &mut [Vec<Patch>],
    ij: (isize, isize),
    k: isize,
    j: isize,
    x: CPoint,
    op: &'static str,
) -> Result<()> {
    let pt = patch_mut(patches, ij.0, ij.1, op)?;
    put2(&mut pt.net, k, j, x, op)
}

/// Decomposes `sur` into its Bezier patches without knot refinement
/// (`N_SrfDecomposeToBez`). Returns the patches as NURBS surfaces of degrees
/// `(p, q)`, indexed `[i][j]` for the `i`-th u span and `j`-th v span
/// (upstream's highest indexes `kk`, `ll` are the lengths minus one).
///
/// Errors with `InvalidInput` for knot vectors upstream cannot decompose (no
/// non-zero span, or more breakpoints than spans).
pub fn decompose_to_bez(sur: &Surface) -> Result<Vec<Vec<Surface>>> {
    const OP: &str = "nurbs::srf_tool::p1::decompose_to_bez";
    let Local {
        m,
        p,
        q,
        r,
        s,
        pw,
        u: up,
        v: vp,
        ..
    } = Local::of(sur, OP)?;

    // Allocate memory.
    let (knu, knv) = sur.knot_vectors()?;
    let nsu = sx(knu.span_count(ix(p, OP)?), OP)?;
    let nsv = sx(knv.span_count(ix(q, OP)?), OP)?;
    if nsu < 1 || nsv < 1 {
        return Err(oob(OP));
    }
    let blank = Patch {
        net: zeros2(p, q, OP)?,
        u: zeros1(succ(twice(p, OP)?, OP)?, OP)?,
        v: zeros1(succ(twice(q, OP)?, OP)?, OP)?,
    };
    let mut sur_a: Vec<Vec<Patch>> = vec![vec![blank; ix(nsv, OP)?]; ix(nsu, OP)?];

    let mut bw: Net = zeros2(p, m, OP)?;
    let mut nbw: Net = zeros2(p, m, OP)?;
    let mut uals: Vec<f64> = zeros1(p, OP)?;
    let mut omus: Vec<f64> = zeros1(p, OP)?;
    let mut vals: Vec<f64> = zeros1(q, OP)?;
    let mut omvs: Vec<f64> = zeros1(q, OP)?;

    // Initialize for u-directional decomposition.
    let mut isu = p;
    let mut ieu = p + 1;
    let mut iq: isize = -1;
    // NQw: the patch receiving the next patch's control points; upstream keeps the
    // last assigned pointer (NULL before the first assignment).
    let mut nqw: Option<(isize, isize)> = None;

    for i in 0..=p {
        for l in 0..=m {
            put2(&mut bw, i, l, at2(&pw, i, l, OP)?, OP)?;
        }
    }

    // Decompose in u-direction into Bezier strips.
    while ieu < r {
        iq += 1;

        // Get knot multiplicity.
        let i0 = ieu;
        while ieu < r && at(up, ieu, OP)? == at(up, ieu + 1, OP)? {
            ieu += 1;
        }
        let mlu = ieu - i0 + 1;
        let ru = p - mlu;

        // Insert the knot.
        if mlu < p {
            let num = at(up, ieu, OP)? - at(up, isu, OP)?;
            let mut i = p;
            while i > mlu {
                let x = num / (at(up, isu + i, OP)? - at(up, isu, OP)?);
                put(&mut uals, i - mlu - 1, x, OP)?;
                put(&mut omus, i - mlu - 1, 1.0 - x, OP)?;
                i -= 1;
            }

            for i in 1..=ru {
                let su = mlu + i;
                let save = ru - i;
                let mut j = p;
                while j >= su {
                    for l in 0..=m {
                        let c = CPoint::combine2(
                            at(&uals, j - su, OP)?,
                            at2(&bw, j, l, OP)?,
                            at(&omus, j - su, OP)?,
                            at2(&bw, j - 1, l, OP)?,
                        );
                        put2(&mut bw, j, l, c, OP)?;
                    }
                    j -= 1;
                }
                if ieu < r {
                    for l in 0..=m {
                        put2(&mut nbw, save, l, at2(&bw, p, l, OP)?, OP)?;
                    }
                }
            }
        }

        // Bezier strip completed. Initialize for v-directional decomposition.
        let mut isv = q;
        let mut iev = q + 1;
        let mut jq: isize = -1;

        for j in 0..=q {
            for k in 0..=p {
                pput(&mut sur_a, (iq, 0), k, j, at2(&bw, k, j, OP)?, OP)?;
            }
        }

        // Decompose in v-direction into Bezier patches.
        while iev < s {
            jq += 1;
            let qw = (iq, jq);
            if jq < nsv - 1 {
                nqw = Some((iq, jq + 1));
            }

            // Get knot multiplicity.
            let i0 = iev;
            while iev < s && at(vp, iev, OP)? == at(vp, iev + 1, OP)? {
                iev += 1;
            }
            let mlv = iev - i0 + 1;
            let rv = q - mlv;

            // Insert the knot.
            if mlv < q {
                let num = at(vp, iev, OP)? - at(vp, isv, OP)?;
                let mut i = q;
                while i > mlv {
                    let x = num / (at(vp, isv + i, OP)? - at(vp, isv, OP)?);
                    put(&mut vals, i - mlv - 1, x, OP)?;
                    put(&mut omvs, i - mlv - 1, 1.0 - x, OP)?;
                    i -= 1;
                }

                for i in 1..=rv {
                    let sv = mlv + i;
                    let save = rv - i;
                    let mut j = q;
                    while j >= sv {
                        for k in 0..=p {
                            let c = CPoint::combine2(
                                at(&vals, j - sv, OP)?,
                                pcpt(&sur_a, qw, k, j, OP)?,
                                at(&omvs, j - sv, OP)?,
                                pcpt(&sur_a, qw, k, j - 1, OP)?,
                            );
                            pput(&mut sur_a, qw, k, j, c, OP)?;
                        }
                        j -= 1;
                    }
                    if iev < s {
                        let nq = nqw.ok_or_else(|| oob(OP))?;
                        for k in 0..=p {
                            let c = pcpt(&sur_a, qw, k, q, OP)?;
                            pput(&mut sur_a, nq, k, save, c, OP)?;
                        }
                    }
                }
            }

            // Get knot vectors.
            let (u_s, u_e) = (at(up, isu, OP)?, at(up, ieu, OP)?);
            let (v_s, v_e) = (at(vp, isv, OP)?, at(vp, iev, OP)?);
            let pt = patch_mut(&mut sur_a, qw.0, qw.1, OP)?;
            for i in 0..=p {
                put(&mut pt.u, i, u_s, OP)?;
                put(&mut pt.u, i + p + 1, u_e, OP)?;
            }
            for j in 0..=q {
                put(&mut pt.v, j, v_s, OP)?;
                put(&mut pt.v, j + q + 1, v_e, OP)?;
            }

            // Patch completed - prepare for next Bezier patch.
            if iev < s {
                let nq = nqw.ok_or_else(|| oob(OP))?;
                for i in rv..=q {
                    for k in 0..=p {
                        pput(&mut sur_a, nq, k, i, at2(&bw, k, iev - q + i, OP)?, OP)?;
                    }
                }
            }

            isv = iev;
            iev += 1;
        }

        // Bezier strip decomposed - prepare for next strip.
        if ieu < r {
            for i in ru..=p {
                for l in 0..=m {
                    put2(&mut nbw, i, l, at2(&pw, ieu - p + i, l, OP)?, OP)?;
                }
            }
        }

        isu = ieu;
        ieu += 1;
        core::mem::swap(&mut bw, &mut nbw);
    }

    sur_a
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|pt| surface(pt.net, p, q, pt.u, pt.v, OP))
                .collect()
        })
        .collect()
}

/// Extracts the iso-curve of `sur` at the parameter `t` (`N_SrfExtractIsoCrv`):
/// [`Direction::U`] gives the u-curve at `v = t`, [`Direction::V`] the v-curve at
/// `u = t`. Parameters outside the domain extrapolate the end spans.
///
/// Errors: [`NlibCode::Call`] for [`Direction::UV`]; `InvalidInput` for a degree
/// above [`MAX_DEGREE`] or a knot multiplicity above the degree.
pub fn extract_iso_crv(sur: &Surface, t: f64, dir: Direction) -> Result<Curve> {
    const OP: &str = "nurbs::srf_tool::p1::extract_iso_crv";
    let Local {
        n,
        m,
        p,
        q,
        r,
        s,
        pw,
        u: up,
        v: vp,
    } = Local::of(sur, OP)?;

    // Fixed-size scratch arrays upstream (NL_MAXDEG + 1).
    let a = p.max(q);
    if ix(a, OP)? > MAX_DEGREE {
        return Err(oob(OP));
    }
    let mut alf: Vec<Vec<f64>> = zeros2(a, a, OP)?;
    let mut oma: Vec<Vec<f64>> = zeros2(a, a, OP)?;
    let mut rw: Vec<CPoint> = zeros1(a, OP)?;

    // Check parameters and compute highest indexes.
    let (mut spu, mut mlu, mut spv, mut mlv) = (0, 0, 0, 0);
    let (ne, pe, me) = match dir {
        Direction::U => {
            (spv, mlv) = span_mult(vp, q, t, OP)?;
            (n, p, r)
        }
        Direction::V => {
            (spu, mlu) = span_mult(up, p, t, OP)?;
            (m, q, s)
        }
        Direction::UV => return Err(nlib(OP, NlibCode::Call)),
    };

    let mut cw: Vec<CPoint> = zeros1(ne, OP)?;
    let mut uc: Vec<f64> = zeros1(me, OP)?;
    let done = |cw: Vec<CPoint>, uc: Vec<f64>| Curve::new(cw, ix(pe, OP)?, uc);

    // See if a boundary curve is required.
    if dir == Direction::U && (t == at(vp, q, OP)? || t == at(vp, s - q, OP)?) {
        let col = if t == at(vp, q, OP)? { 0 } else { m };
        for i in 0..=n {
            put(&mut cw, i, at2(&pw, i, col, OP)?, OP)?;
        }
        for i in 0..=r {
            put(&mut uc, i, at(up, i, OP)?, OP)?;
        }
        return done(cw, uc);
    }

    if dir == Direction::V && (t == at(up, p, OP)? || t == at(up, r - p, OP)?) {
        let row = if t == at(up, p, OP)? { 0 } else { n };
        for j in 0..=m {
            put(&mut cw, j, at2(&pw, row, j, OP)?, OP)?;
        }
        for j in 0..=s {
            put(&mut uc, j, at(vp, j, OP)?, OP)?;
        }
        return done(cw, uc);
    }

    let bad_mult = SolidError::InvalidInput {
        operation: OP,
        reason: "knot multiplicity above the degree",
    };

    // Extract the u-curve.
    if dir == Direction::U {
        if mlv > q {
            return Err(bad_mult);
        }

        // Save the v-alphas.
        for i in 1..=q - mlv {
            let a = spv - q + i;
            for j in 0..=q - i - mlv {
                let x = (t - at(vp, a + j, OP)?) / (at(vp, spv + j + 1, OP)? - at(vp, a + j, OP)?);
                put2(&mut alf, i, j, x, OP)?;
                put2(&mut oma, i, j, 1.0 - x, OP)?;
            }
        }

        // For each column compute the curve control point.
        let a = spv - q;
        for k in 0..=n {
            for i in 0..=q - mlv {
                put(&mut rw, i, at2(&pw, k, a + i, OP)?, OP)?;
            }
            for i in 1..=q - mlv {
                for j in 0..=q - i - mlv {
                    let c = CPoint::combine2(
                        at2(&alf, i, j, OP)?,
                        at(&rw, j + 1, OP)?,
                        at2(&oma, i, j, OP)?,
                        at(&rw, j, OP)?,
                    );
                    put(&mut rw, j, c, OP)?;
                }
            }
            put(&mut cw, k, at(&rw, 0, OP)?, OP)?;
        }

        // Load the curve knot vector.
        for i in 0..=r {
            put(&mut uc, i, at(up, i, OP)?, OP)?;
        }
    }

    // Extract the v-curve.
    if dir == Direction::V {
        if mlu > p {
            return Err(bad_mult);
        }

        // Save the u-alphas.
        for i in 1..=p - mlu {
            let a = spu - p + i;
            for j in 0..=p - i - mlu {
                let x = (t - at(up, a + j, OP)?) / (at(up, spu + j + 1, OP)? - at(up, a + j, OP)?);
                put2(&mut alf, i, j, x, OP)?;
                put2(&mut oma, i, j, 1.0 - x, OP)?;
            }
        }

        // For each row compute the curve control point.
        let a = spu - p;
        for l in 0..=m {
            for i in 0..=p - mlu {
                put(&mut rw, i, at2(&pw, a + i, l, OP)?, OP)?;
            }
            for i in 1..=p - mlu {
                for j in 0..=p - i - mlu {
                    let c = CPoint::combine2(
                        at2(&alf, i, j, OP)?,
                        at(&rw, j + 1, OP)?,
                        at2(&oma, i, j, OP)?,
                        at(&rw, j, OP)?,
                    );
                    put(&mut rw, j, c, OP)?;
                }
            }
            put(&mut cw, l, at(&rw, 0, OP)?, OP)?;
        }

        // Load the curve knot vector.
        for j in 0..=s {
            put(&mut uc, j, at(vp, j, OP)?, OP)?;
        }
    }

    done(cw, uc)
}

/// Refines `sur` with the knots `x` in u or v (`N_SrfInsertKnots`). The knots must
/// not decrease and must lie strictly inside the knot vector they refine.
///
/// Errors: [`NlibCode::Input`] for an empty `x`; [`NlibCode::Parameter`] when
/// `x[0]` or the last knot is an end or outside parameter; [`NlibCode::Call`] for
/// [`Direction::UV`].
pub fn insert_knots(sur: &Surface, x: &[f64], dir: Direction) -> Result<Surface> {
    const OP: &str = "nurbs::srf_tool::p1::insert_knots";
    let Local {
        n,
        m,
        p,
        q,
        r,
        s,
        pw,
        u: up,
        v: vp,
    } = Local::of(sur, OP)?;
    let xr = sx(x.len(), OP)? - 1;

    // Check parameters and compute highest indexes.
    if xr < 0 {
        return Err(nlib(OP, NlibCode::Input));
    }
    let (x0, xl) = (at(x, 0, OP)?, at(x, xr, OP)?);
    let (ni, mi, ri, si) = match dir {
        Direction::U => {
            check_interior(up, x0, OP)?;
            check_interior(up, xl, OP)?;
            (n + xr + 1, m, r + xr + 1, s)
        }
        Direction::V => {
            check_interior(vp, x0, OP)?;
            check_interior(vp, xl, OP)?;
            (n, m + xr + 1, r, s + xr + 1)
        }
        Direction::UV => return Err(nlib(OP, NlibCode::Call)),
    };

    let mut qw: Net = zeros2(ni, mi, OP)?;
    let mut uq: Vec<f64> = zeros1(ri, OP)?;
    let mut vq: Vec<f64> = zeros1(si, OP)?;

    // Refine the U knot vector.
    if dir == Direction::U {
        // Find knot span indexes.
        let a = span(up, p, x0, OP)?;
        let b = span(up, p, xl, OP)? + 1;

        // Initialize output knot vectors.
        for i in 0..=a {
            put(&mut uq, i, at(up, i, OP)?, OP)?;
        }
        for i in b + p..=r {
            put(&mut uq, i + xr + 1, at(up, i, OP)?, OP)?;
        }
        for j in 0..=s {
            put(&mut vq, j, at(vp, j, OP)?, OP)?;
        }

        // Save unaltered control points.
        for col in 0..=m {
            for j in 0..=a - p {
                put2(&mut qw, j, col, at2(&pw, j, col, OP)?, OP)?;
            }
            for j in b - 1..=n {
                put2(&mut qw, j + xr + 1, col, at2(&pw, j, col, OP)?, OP)?;
            }
        }

        // Now refine the knot vector.
        let mut i = b + p - 1;
        let mut k = b + p + xr;
        for j in (0..=xr).rev() {
            let xj = at(x, j, OP)?;
            while xj <= at(up, i, OP)? && i > a {
                for col in 0..=m {
                    put2(&mut qw, k - p - 1, col, at2(&pw, i - p - 1, col, OP)?, OP)?;
                }
                put(&mut uq, k, at(up, i, OP)?, OP)?;
                k -= 1;
                i -= 1;
            }

            for col in 0..=m {
                let c = at2(&qw, k - p, col, OP)?;
                put2(&mut qw, k - p - 1, col, c, OP)?;
            }

            for l in 1..=p {
                let t = k - p + l;
                let mut alf = at(&uq, k + l, OP)? - xj;
                if alf.abs() <= 0.0 {
                    for col in 0..=m {
                        let c = at2(&qw, t, col, OP)?;
                        put2(&mut qw, t - 1, col, c, OP)?;
                    }
                } else {
                    alf /= at(&uq, k + l, OP)? - at(up, i - p + l, OP)?;
                    let oma = 1.0 - alf;
                    for col in 0..=m {
                        let c = CPoint::combine2(
                            alf,
                            at2(&qw, t - 1, col, OP)?,
                            oma,
                            at2(&qw, t, col, OP)?,
                        );
                        put2(&mut qw, t - 1, col, c, OP)?;
                    }
                }
            }
            put(&mut uq, k, xj, OP)?;
            k -= 1;
        }
    }

    // Refine the V knot vector.
    if dir == Direction::V {
        // Find knot span indexes.
        let a = span(vp, q, x0, OP)?;
        let b = span(vp, q, xl, OP)? + 1;

        // Initialize output knot vectors.
        for j in 0..=a {
            put(&mut vq, j, at(vp, j, OP)?, OP)?;
        }
        for j in b + q..=s {
            put(&mut vq, j + xr + 1, at(vp, j, OP)?, OP)?;
        }
        for i in 0..=r {
            put(&mut uq, i, at(up, i, OP)?, OP)?;
        }

        // Save unaltered control points.
        for row in 0..=n {
            for j in 0..=a - q {
                put2(&mut qw, row, j, at2(&pw, row, j, OP)?, OP)?;
            }
            for j in b - 1..=m {
                put2(&mut qw, row, j + xr + 1, at2(&pw, row, j, OP)?, OP)?;
            }
        }

        // Now refine the knot vector.
        let mut i = b + q - 1;
        let mut k = b + q + xr;
        for j in (0..=xr).rev() {
            let xj = at(x, j, OP)?;
            while xj <= at(vp, i, OP)? && i > a {
                for row in 0..=n {
                    put2(&mut qw, row, k - q - 1, at2(&pw, row, i - q - 1, OP)?, OP)?;
                }
                put(&mut vq, k, at(vp, i, OP)?, OP)?;
                k -= 1;
                i -= 1;
            }

            for row in 0..=n {
                let c = at2(&qw, row, k - q, OP)?;
                put2(&mut qw, row, k - q - 1, c, OP)?;
            }

            for l in 1..=q {
                let t = k - q + l;
                let mut alf = at(&vq, k + l, OP)? - xj;
                if alf.abs() <= 0.0 {
                    for row in 0..=n {
                        let c = at2(&qw, row, t, OP)?;
                        put2(&mut qw, row, t - 1, c, OP)?;
                    }
                } else {
                    alf /= at(&vq, k + l, OP)? - at(vp, i - q + l, OP)?;
                    let oma = 1.0 - alf;
                    for row in 0..=n {
                        let c = CPoint::combine2(
                            alf,
                            at2(&qw, row, t - 1, OP)?,
                            oma,
                            at2(&qw, row, t, OP)?,
                        );
                        put2(&mut qw, row, t - 1, c, OP)?;
                    }
                }
            }
            put(&mut vq, k, xj, OP)?;
            k -= 1;
        }
    }

    surface(qw, p, q, uq, vq, OP)
}

/// Snaps `x` to the first of `knots[0..k]` within [`PATCH_SNAP_TOL`]; returns whether
/// it snapped.
fn snap(x: &mut f64, knots: &[f64], k: isize) -> bool {
    for &kn in knots.iter().take(usize::try_from(k).unwrap_or(0)) {
        if *x > kn - PATCH_SNAP_TOL && *x < kn + PATCH_SNAP_TOL {
            *x = kn;
            return true;
        }
    }
    false
}

/// Extracts the patch `rect` (`[ul, ur] x [vb, vt]`) of `sur` by knot insertion
/// (`N_SrfExtractPatch`, whose four corner parameters are the fields of `rect`). Corner parameters within `1e-8` of an existing knot are
/// first snapped to it; the result is then reparametrised back to the requested
/// rectangle.
///
/// Errors with [`NlibCode::Input`] for an empty (or, after snapping, inverted)
/// rectangle.
pub fn extract_patch(sur: &Surface, rect: Rectangle) -> Result<Surface> {
    const OP: &str = "nurbs::srf_tool::p1::extract_patch";
    let Local {
        m,
        p,
        q,
        r,
        s,
        pw,
        u: up,
        v: vp,
        ..
    } = Local::of(sur, OP)?;

    // The requested box is `rect`; its corners may be snapped below.
    let (mut ul, mut ur, mut vl, mut vr) = (rect.ul, rect.ur, rect.vb, rect.vt);

    // Snap to existing knots if within tolerance.
    let mut snapped = false;
    snapped |= snap(&mut ul, up, r);
    snapped |= snap(&mut ur, up, r);
    snapped |= snap(&mut vl, vp, s);
    snapped |= snap(&mut vr, vp, s);

    // Check for degenerate parameters.
    if ur <= ul || vr <= vl {
        return Err(nlib(OP, NlibCode::Input));
    }

    // Find knot spans and set new indexes.
    let (span_ul, mul) = span_mult(up, p, ul, OP)?;
    let (mut span_ur, mut mur) = span_mult(up, p, ur, OP)?;
    let (span_vl, mvl) = span_mult(vp, q, vl, OP)?;
    let (mut span_vr, mut mvr) = span_mult(vp, q, vr, OP)?;

    let is = span_ul - p;
    if ur == at(up, r - p, OP)? {
        span_ur = r;
        mur = p + 1;
    }
    let ie = span_ur - mur;

    let js = span_vl - q;
    if vr == at(vp, s - q, OP)? {
        span_vr = s;
        mvr = q + 1;
    }
    let je = span_vr - mvr;

    let n = ie - is;
    let r = span_ur - span_ul - mur + 2 * p + 1;
    if n < 0 {
        return Err(nlib(OP, NlibCode::Input));
    }

    // Auxiliary control points.
    let mut aw: Net = zeros2(n, m, OP)?;

    // Get the surface strip; for each column: initial control points, insert the
    // left knot, insert the right knot.
    for col in 0..=m {
        for i in is..=ie {
            put2(&mut aw, i - is, col, at2(&pw, i, col, OP)?, OP)?;
        }

        // Insert the left knot.
        let ll = span_ul - p;
        for i in 1..=p - mul {
            for j in 0..=p - i - mul {
                let left = at(up, ll + i + j, OP)?;
                let alf = (ul - left) / (at(up, span_ul + j + 1, OP)? - left);
                let oma = 1.0 - alf;
                let c =
                    CPoint::combine2(alf, at2(&aw, j + 1, col, OP)?, oma, at2(&aw, j, col, OP)?);
                put2(&mut aw, j, col, c, OP)?;
            }
        }

        // Insert the right knot.
        let lr = span_ur - p;
        let lk = n - p + mur;
        for i in 1..=p - mur {
            let mut j = p - i - mur;
            while j >= 0 {
                let k = lk + i + j;
                let mut left = at(up, lr + i + j, OP)?;
                if left < ul {
                    left = ul;
                }
                let alf = (ur - left) / (at(up, span_ur + j + 1, OP)? - left);
                let oma = 1.0 - alf;
                let c =
                    CPoint::combine2(alf, at2(&aw, k, col, OP)?, oma, at2(&aw, k - 1, col, OP)?);
                put2(&mut aw, k, col, c, OP)?;
                j -= 1;
            }
        }
    }

    let m = je - js;
    let s = span_vr - span_vl - mvr + 2 * q + 1;
    if m < 0 {
        return Err(nlib(OP, NlibCode::Input));
    }

    let mut qw: Net = zeros2(n, m, OP)?;
    let mut uq: Vec<f64> = zeros1(r, OP)?;
    let mut vq: Vec<f64> = zeros1(s, OP)?;

    // Extract the surface patch; for each row: initial control points, insert the
    // left knot, insert the right knot.
    for row in 0..=n {
        for j in js..=je {
            put2(&mut qw, row, j - js, at2(&aw, row, j, OP)?, OP)?;
        }

        // Insert the left knot.
        let ll = span_vl - q;
        for i in 1..=q - mvl {
            for j in 0..=q - i - mvl {
                let left = at(vp, ll + i + j, OP)?;
                let alf = (vl - left) / (at(vp, span_vl + j + 1, OP)? - left);
                let oma = 1.0 - alf;
                let c =
                    CPoint::combine2(alf, at2(&qw, row, j + 1, OP)?, oma, at2(&qw, row, j, OP)?);
                put2(&mut qw, row, j, c, OP)?;
            }
        }

        // Insert the right knot.
        let lr = span_vr - q;
        let lk = m - q + mvr;
        for i in 1..=q - mvr {
            let mut j = q - i - mvr;
            while j >= 0 {
                let k = lk + i + j;
                let mut left = at(vp, lr + i + j, OP)?;
                if left < vl {
                    left = vl;
                }
                let alf = (vr - left) / (at(vp, span_vr + j + 1, OP)? - left);
                let oma = 1.0 - alf;
                let c =
                    CPoint::combine2(alf, at2(&qw, row, k, OP)?, oma, at2(&qw, row, k - 1, OP)?);
                put2(&mut qw, row, k, c, OP)?;
                j -= 1;
            }
        }
    }

    // Load knot vectors.
    let mut k: isize = -1;
    for _ in 0..=p {
        k += 1;
        put(&mut uq, k, ul, OP)?;
    }
    for i in span_ul + 1..=span_ur - mur {
        k += 1;
        put(&mut uq, k, at(up, i, OP)?, OP)?;
    }
    for _ in 0..=p {
        k += 1;
        put(&mut uq, k, ur, OP)?;
    }

    k = -1;
    for _ in 0..=q {
        k += 1;
        put(&mut vq, k, vl, OP)?;
    }
    for j in span_vl + 1..=span_vr - mvr {
        k += 1;
        put(&mut vq, k, at(vp, j, OP)?, OP)?;
    }
    for _ in 0..=q {
        k += 1;
        put(&mut vq, k, vr, OP)?;
    }

    let mut out = surface(qw, p, q, uq, vq, OP)?;

    // When the final interval was snapped away from what was requested, scale the
    // output back to the requested interval.
    if snapped {
        out.reparam_to_interval(rect, Direction::UV)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;

    /// Bicubic-by-quadratic surface over a 5 x 4 net with interior knots.
    fn sample() -> Surface {
        let net: Net = (0..5)
            .map(|i| {
                (0..4)
                    .map(|j| {
                        let (x, y) = (f64::from(i), f64::from(j));
                        CPoint::new(x, y, (x * 0.7 - y * 0.3).sin(), NOW)
                    })
                    .collect()
            })
            .collect();
        let u = vec![0.0, 0.0, 0.0, 0.0, 0.4, 1.0, 1.0, 1.0, 1.0];
        let v = vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        Surface::new(net, 3, 2, u, v).unwrap()
    }

    #[test]
    fn insert_knot_grows_net_and_keeps_ends() {
        let s = sample();
        let out = insert_knot(&s, 0.7, 2, Direction::U).unwrap();
        assert_eq!(out.n(), 6);
        assert_eq!(out.r(), 10);
        assert_eq!(
            out.u_knots(),
            &[0.0, 0.0, 0.0, 0.0, 0.4, 0.7, 0.7, 1.0, 1.0, 1.0, 1.0]
        );
        assert_eq!(out.cpt(0, 0).unwrap(), s.cpt(0, 0).unwrap());
        assert_eq!(out.cpt(6, 3).unwrap(), s.cpt(4, 3).unwrap());
        let v = insert_knot(&s, 0.25, 1, Direction::V).unwrap();
        assert_eq!(v.m(), 4);
        assert_eq!(v.v_knots(), &[0.0, 0.0, 0.0, 0.25, 0.5, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn insert_knot_errors() {
        let s = sample();
        let par = |r: Result<Surface>| {
            matches!(
                r,
                Err(SolidError::Nlib {
                    code: NlibCode::Parameter,
                    ..
                })
            )
        };
        assert!(par(insert_knot(&s, 0.0, 1, Direction::U)));
        assert!(par(insert_knot(&s, 1.5, 1, Direction::U)));
        assert!(par(insert_knot(&s, 0.4, 3, Direction::U)));
        assert!(matches!(
            insert_knot(&s, 0.5, 1, Direction::UV),
            Err(SolidError::Nlib {
                code: NlibCode::Call,
                ..
            })
        ));
    }

    #[test]
    fn split_halves_meet() {
        let s = sample();
        let (l, r) = split(&s, 0.6, Direction::U).unwrap();
        assert_eq!(*l.u_knots().last().unwrap(), 0.6);
        assert_eq!(r.u_knots()[0], 0.6);
        for j in 0..=s.m() {
            let a = l.cpt(l.n(), j).unwrap();
            let b = r.cpt(0, j).unwrap();
            assert!((a.x - b.x).abs() < 1e-12 && (a.z - b.z).abs() < 1e-12);
        }
        let (l, r) = split(&s, 0.5, Direction::V).unwrap();
        // 0.5 is a simple knot: one fewer shared row than at a new parameter.
        assert_eq!(l.m() + r.m(), s.m() + s.q() - 1);
    }

    #[test]
    fn decompose_counts_spans() {
        let s = sample();
        let pts = decompose_to_bez(&s).unwrap();
        assert_eq!(pts.len(), 2);
        assert!(pts.iter().all(|row| row.len() == 2));
        let b = &pts[1][1];
        assert_eq!(b.u_knots(), &[0.4, 0.4, 0.4, 0.4, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(b.v_knots(), &[0.5, 0.5, 0.5, 1.0, 1.0, 1.0]);
        assert_eq!(b.cpt(3, 2).unwrap(), s.cpt(4, 3).unwrap());
    }

    #[test]
    fn iso_curve_boundary_and_interior() {
        let s = sample();
        let c = extract_iso_crv(&s, 0.0, Direction::U).unwrap();
        assert_eq!(c.n(), s.n());
        assert_eq!(c.cpts()[2], s.cpt(2, 0).unwrap());
        let c = extract_iso_crv(&s, 0.3, Direction::V).unwrap();
        assert_eq!(c.n(), s.m());
        assert_eq!(c.knots(), s.v_knots());
    }

    #[test]
    fn insert_knots_matches_repeated_insert_knot() {
        let s = sample();
        let a = insert_knots(&s, &[0.7], Direction::U).unwrap();
        let b = insert_knot(&s, 0.7, 1, Direction::U).unwrap();
        assert_eq!(a.u_knots(), b.u_knots());
        for i in 0..=a.n() {
            for j in 0..=a.m() {
                let (x, y) = (a.cpt(i, j).unwrap(), b.cpt(i, j).unwrap());
                assert!((x.x - y.x).abs() < 1e-12 && (x.z - y.z).abs() < 1e-12);
            }
        }
        assert!(matches!(
            insert_knots(&s, &[], Direction::U),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn extract_patch_whole_and_part() {
        let s = sample();
        let whole = extract_patch(&s, Rectangle::new(0.0, 1.0, 0.0, 1.0)).unwrap();
        assert_eq!(whole.u_knots(), s.u_knots());
        assert_eq!(whole.v_knots(), s.v_knots());
        let part = extract_patch(&s, Rectangle::new(0.2, 0.8, 0.1, 0.9)).unwrap();
        assert_eq!(part.u_knots()[0], 0.2);
        assert_eq!(*part.v_knots().last().unwrap(), 0.9);
        assert!(matches!(
            extract_patch(&s, Rectangle::new(0.5, 0.5, 0.0, 1.0)),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn absurd_degrees_and_sizes_are_errors() {
        // fuzz (nlib.srf_tool_p1..p4): a degree of i64::MAX overflowed the index
        // arithmetic, and scratch arrays sized from it aborted on allocation.
        let s = sample();
        let huge = i64::MAX as usize;
        let (u, v) = (s.u_knots().to_vec(), s.v_knots().to_vec());
        let bad = Surface::new(s.cpts(), huge, s.q(), u, v).unwrap();
        assert!(insert_knot(&bad, 0.5, 1, Direction::U).is_err());
        assert!(decompose_to_bez(&bad).is_err());
        assert!(insert_knot(&s, 0.5, usize::MAX >> 1, Direction::U).is_err());
        assert!(zeros1::<f64>(isize::MAX - 1, "t").is_err());
        // Byte counts past isize::MAX, so the reservation fails on every target; sizes
        // the OS merely cannot back (2^40 x 2^40) are granted lazily on macOS and the
        // zero fill then commits terabytes.
        assert!(zeros2::<f64>(1 << 61, 0, "t").is_err());
        assert!(zeros2::<f64>(0, 1 << 61, "t").is_err());
        assert!(twice(isize::MAX, "t").is_err());
        assert!(degree(DEGREE_RANGE_MAX + 1, "t").is_err());
    }
}
