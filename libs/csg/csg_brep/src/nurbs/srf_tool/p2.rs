//! Surface knot removal (conditional, and the one-step error bound) and degree
//! elevation.
//!
//! Upstream: `source/SMLib/src/NL_SrfTool.cpp`, `source/SMLib/inc/NL_SrfTool.h`.
//!
//! | Rust                                         | Upstream                     |
//! |----------------------------------------------|------------------------------|
//! | [`remove_knot_conditional`], [`KnotRemoval`] | `N_SrfRemoveKnotConditional` |
//! | [`remove_one_knot`]                          | `N_SrfRemoveOneKnot`         |
//! | [`elevate_degree`]                           | `N_SrfElevateDegree`         |
//! | [`REMOVAL_PARAM_TOL`]                        | `sto` (file-private)         |
//!
//! Deviations:
//! - Out-params become return values: [`remove_knot_conditional`] and
//!   [`elevate_degree`] return a new surface (plus the removal count) instead of
//!   filling a caller-provided one; [`remove_one_knot`] takes the running maximum
//!   error `mr` by value and returns the updated one. Upstream's in-place mode
//!   (`surP == surQ`) computes the same result, so a caller replaces its surface with
//!   the returned one. The `NL_STACKS` arguments have no counterpart.
//! - The u- and v-direction branches of each routine are one implementation over the
//!   rows of the net in that direction (the net is transposed for the u direction and
//!   back). The arithmetic and the order in which rows are processed are upstream's,
//!   so results agree bit for bit.
//! - `N_SrfRemoveKnotConditional` returns its error flag after finishing when the last
//!   `N_BasisFindGlobalMax` it called failed (it then assumes a maximum of `1`); the
//!   Rust function returns only that error then, not the surface (as
//!   `nurbs::crv_tool::p1` does for curves). A non-NLib error of that search (where
//!   upstream would loop forever) is returned at once.
//! - `N_SrfElevateDegree` indexes its `rbet`/`romb` scratch rows with the non-positive
//!   `j - last` (reaching into the previous row of the same block, consistently for
//!   writes and reads); the Rust rows are indexed with `last - j`, which stores the
//!   same values under the same keys.
//! - `t` of [`elevate_degree`] and `nt` of [`remove_knot_conditional`] are `usize`, so
//!   the negative-count paths cannot occur.
//! - Every array access is bounds checked. Where upstream would index outside its
//!   arrays (invalid knot vectors, multiplicities above the degree, wrong index /
//!   multiplicity / row arguments of [`remove_one_knot`]) the functions return
//!   [`SolidError::InvalidInput`]. `N_SrfRemoveOneKnot`'s guard `off > last + 1`
//!   (an error flag without an error number) also returns `InvalidInput`.

use crate::error::{NlibCode, Result, SolidError};
use crate::math::max2;
use crate::nurbs::basis_basic::find_global_max;
use crate::nurbs::bezier::{elevation_matrix, srf_elevate_degree};
use crate::nurbs::geom::dist_cpt_cpt_homo;
use crate::nurbs::knots::{KnotVector, MAX_DEGREE};
use crate::nurbs::math::CPoint;
use crate::nurbs::srf_basic::{Direction, Surface, WEIGHT_MAX, WEIGHT_MIN};

use super::p1::{
    at, at2, check_interior, ix, oob, put, put2, span_mult, surface, sx, twice, zeros1, zeros2,
    Local, Net,
};

/// Upstream `sto`: relative parameter tolerance of the basis maxima searched by
/// [`remove_knot_conditional`] (scaled by the parameter range).
pub const REMOVAL_PARAM_TOL: f64 = 1.0e-05;

/// Result of [`remove_knot_conditional`].
#[derive(Debug, Clone, PartialEq)]
pub struct KnotRemoval {
    /// The surface after removal (a copy of the input when nothing was removed).
    pub surface: Surface,
    /// How many times the knot was removed (upstream `rt`).
    pub removed: usize,
}

/// `w[j][i]` from `w[i][j]` (rows of the u direction).
fn transpose(w: &Net) -> Net {
    let cols = w.first().map_or(0, Vec::len);
    (0..cols)
        .map(|j| w.iter().filter_map(|row| row.get(j).copied()).collect())
        .collect()
}

/// `n + 1` rows of `w`, each cut to `m + 1` entries.
fn cut(mut w: Net, n: isize, m: isize, op: &'static str) -> Result<Net> {
    w.truncate(ix(n + 1, op)?);
    let len = ix(m + 1, op)?;
    for row in &mut w {
        if row.len() < len {
            return Err(oob(op));
        }
        row.truncate(len);
    }
    Ok(w)
}

/// `k[0..=r]`.
fn cut1(k: &[f64], r: isize, op: &'static str) -> Result<Vec<f64>> {
    k.get(..ix(r + 1, op)?)
        .map(<[f64]>::to_vec)
        .ok_or_else(|| oob(op))
}

/// Conditionally removes the interior knot `t` up to `nt` times from the u or v knot
/// vector of `sur` (`N_SrfRemoveKnotConditional`): each removal step is kept only if
/// every row of the net changes by at most `tol` (scaled for rational surfaces by
/// `wmin / (1 + pmax)`) and no new weight leaves `[WEIGHT_MIN, WEIGHT_MAX]`. `nt` is
/// limited to the multiplicity of `t`.
///
/// Errors: [`NlibCode::Parameter`] for an end or outside parameter;
/// [`NlibCode::Call`] for [`Direction::UV`]; errors of the span search; the error of
/// the last basis-maximum search when it failed (see the module deviations).
pub fn remove_knot_conditional(
    sur: &Surface,
    t: f64,
    nt: usize,
    tol: f64,
    dir: Direction,
) -> Result<KnotRemoval> {
    match remove_knot_conditional_with_status(sur, t, nt, tol, dir)? {
        (_, Some(e)) => Err(e),
        (out, None) => Ok(out),
    }
}

/// [`remove_knot_conditional`] that also returns the surface when upstream finishes
/// with its error flag set by the last failed basis-maximum search: `Ok((removal,
/// Some(error)))` then (upstream's output surface plus the error it returns). `Err`
/// is returned where upstream exits before writing its output.
pub(crate) fn remove_knot_conditional_with_status(
    sur: &Surface,
    t: f64,
    nt: usize,
    tol: f64,
    dir: Direction,
) -> Result<(KnotRemoval, Option<SolidError>)> {
    const OP: &str = "nurbs::srf_tool::p2::remove_knot_conditional";
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
    let rational = sur.is_rational();

    // Adjust removal tolerance in case of rational surfaces.
    let mut tol = tol;
    if rational {
        let mm = sur.min_max_weight_pos_vectors();
        tol = (tol * mm.wmin) / (1.0 + mm.pmax);
    }

    // Check parameters: find span and multiplicity of t.
    let (spn, mlt) = match dir {
        Direction::U => {
            check_interior(up, t, OP)?;
            span_mult(up, p, t, OP)?
        }
        Direction::V => {
            check_interior(vp, t, OP)?;
            span_mult(vp, q, t, OP)?
        }
        Direction::UV => return Err(SolidError::nlib(OP, NlibCode::Call)),
    };

    // Limit removal count to multiplicity of knot.
    let mut nt = sx(nt, OP)?;
    if nt > mlt {
        nt = mlt;
    }

    // Rows in the removal direction, their highest index, degree and knots.
    let (mut lines, hi, deg, mut kq, rk) = match dir {
        Direction::U => (transpose(&pw), n, p, up.to_vec(), r),
        _ => (pw, m, q, vp.to_vec(), s),
    };
    let a = p.max(q);
    let (removed, error) = remove_conditional_rows(
        &mut lines,
        RemovalArgs {
            hi,
            deg,
            rk,
            spn,
            mlt,
            nt,
            t,
            tol,
            a,
            rational,
        },
        &mut kq,
        OP,
    )?;

    // Complete output.
    let hi_new = hi - removed;
    let kq = cut1(&kq, rk - removed, OP)?;
    let out = match dir {
        Direction::U => {
            let lines = cut(lines, m, hi_new, OP)?;
            surface(
                cut(transpose(&lines), hi_new, m, OP)?,
                p,
                q,
                kq,
                vp.to_vec(),
                OP,
            )?
        }
        _ => surface(cut(lines, n, hi_new, OP)?, p, q, up.to_vec(), kq, OP)?,
    };
    Ok((
        KnotRemoval {
            surface: out,
            removed: ix(removed, OP)?,
        },
        error,
    ))
}

/// Scalars of one conditional removal along the rows of a net.
#[derive(Clone, Copy)]
struct RemovalArgs {
    /// Highest control point index along a row.
    hi: isize,
    /// Degree along a row.
    deg: isize,
    /// Highest knot index.
    rk: isize,
    /// Span and multiplicity of `t`.
    spn: isize,
    mlt: isize,
    /// Requested removal count (at most `mlt`).
    nt: isize,
    /// The knot to remove.
    t: f64,
    /// Removal tolerance (already scaled).
    tol: f64,
    /// `max(p, q)`: the scratch arrays hold `2a + 1` entries.
    a: isize,
    /// `N_IsSrfRat(surP)`.
    rational: bool,
}

/// The removal loop of `N_SrfRemoveKnotConditional` for one direction, on
/// `lines[row][i]` and the knots `kq` (both edited in place, keeping their storage
/// size). Returns the number of removals `k` and the error of the last
/// basis-maximum search if it failed.
fn remove_conditional_rows(
    lines: &mut Net,
    args: RemovalArgs,
    kq: &mut [f64],
    op: &'static str,
) -> Result<(isize, Option<SolidError>)> {
    let RemovalArgs {
        hi,
        deg,
        rk,
        spn,
        mlt,
        nt,
        t,
        tol,
        a,
        rational,
    } = args;
    let degu = ix(deg, op)?;
    let rows = sx(lines.len(), op)? - 1;

    let mut alf: Vec<f64> = zeros1(twice(a, op)?, op)?;
    let mut oma: Vec<f64> = zeros1(twice(a, op)?, op)?;
    let mut bet: Vec<f64> = zeros1(twice(a, op)?, op)?;
    let mut omb: Vec<f64> = zeros1(twice(a, op)?, op)?;
    let mut rw: Vec<CPoint> = zeros1(twice(a, op)?, op)?;

    let fout = (2 * spn - mlt - deg) / 2;
    let mut rmf = true;
    let mut first = spn - deg;
    let mut last = spn - mlt;
    let (mut lam, mut oml) = (0.0, 0.0);

    // Upstream `error` after the latest `N_BasisFindGlobalMax`.
    let mut error: Option<SolidError> = None;
    let lto = REMOVAL_PARAM_TOL * (at(kq, rk, op)? - at(kq, 0, op)?).abs();
    let global_max = |kq: &[f64], i: isize, error: &mut Option<SolidError>| -> Result<f64> {
        let kv = KnotVector::from_slice(kq)?;
        match find_global_max(&kv, ix(i, op)?, degu, lto) {
            Ok((max, _)) => {
                *error = None;
                Ok(max)
            }
            Err(e @ SolidError::Nlib { .. }) => {
                *error = Some(e);
                Ok(1.0)
            }
            Err(e) => Err(e),
        }
    };

    // Try to remove the knot nt times.
    let mut k: isize = 0;
    while k < nt {
        let off = first - 1;

        // Save some parameters.
        let mut i = first;
        let mut j = last;
        while j - i > k {
            let ki = at(kq, i, op)?;
            let al = (at(kq, i + deg + 1, op)? - ki) / (t - ki);
            put(&mut alf, i - first, al, op)?;
            put(&mut oma, i - first, 1.0 - al, op)?;
            let kj = at(kq, j + deg - k + 1, op)?;
            let be = (kj - at(kq, j - k, op)?) / (kj - t);
            put(&mut bet, j - first, be, op)?;
            put(&mut omb, j - first, 1.0 - be, op)?;
            i += 1;
            j -= 1;
        }

        let ki = at(kq, i, op)?;
        let del = (t - ki) / (at(kq, i + deg + 1, op)? - ki);
        let omd = 1.0 - del;

        let max = if j - i < k {
            if i - first > 0 && j - first + 1 >= 0 {
                let al = at(&alf, i - first - 1, op)?;
                lam = al / (al + at(&bet, j - first + 1, op)?);
                oml = 1.0 - lam;
                let maxl = global_max(kq, i - 1, &mut error)?;
                let maxr = global_max(kq, i, &mut error)?;
                max2(lam * al * maxl, oml * omd * maxr)
            } else {
                lam = 0.5;
                oml = 0.5;
                1.0
            }
        } else {
            global_max(kq, i, &mut error)?
        };

        // Try to remove one knot along each row.
        for line in lines.iter_mut() {
            let mut i = first;
            let mut j = last;
            let mut ii: isize = 1;
            let mut jj = last - off;

            put(&mut rw, 0, at(line, off, op)?, op)?;
            put(&mut rw, last + 1 - off, at(line, last + 1, op)?, op)?;

            // Get new control points for one removal step.
            while j - i > k {
                let c = CPoint::combine2(
                    at(&alf, i - first, op)?,
                    at(line, i, op)?,
                    at(&oma, i - first, op)?,
                    at(&rw, ii - 1, op)?,
                );
                put(&mut rw, ii, c, op)?;
                let c = CPoint::combine2(
                    at(&bet, j - first, op)?,
                    at(line, j, op)?,
                    at(&omb, j - first, op)?,
                    at(&rw, jj + 1, op)?,
                );
                put(&mut rw, jj, c, op)?;
                i += 1;
                j -= 1;
                ii += 1;
                jj -= 1;
            }

            // Check if knot is removable.
            if j - i < k {
                let dw = dist_cpt_cpt_homo(at(&rw, ii - 1, op)?, at(&rw, jj + 1, op)?);
                let maxdw = max * dw;
                if maxdw > tol {
                    rmf = false;
                    break;
                }
                let c = CPoint::combine2(lam, at(&rw, jj + 1, op)?, oml, at(&rw, ii - 1, op)?);
                put(&mut rw, jj + 1, c, op)?;
            } else {
                let c = CPoint::combine2(del, at(&rw, jj + 1, op)?, omd, at(&rw, ii - 1, op)?);
                let dw = dist_cpt_cpt_homo(at(line, i, op)?, c);
                let maxdw = max * dw;
                if maxdw > tol {
                    rmf = false;
                    break;
                }
            }

            // Check for disallowed weights.
            if rational {
                let mut i = first;
                let mut j = last;
                let mut wmin = f64::MAX;
                let mut wmax = f64::MIN_POSITIVE;
                while j - i > k {
                    let wi = at(&rw, i - off, op)?.w;
                    let wj = at(&rw, j - off, op)?.w;
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
                    rmf = false;
                    break;
                }
            }

            // Save new control points.
            let mut i = first;
            let mut j = last;
            while j - i > k {
                put(line, i, at(&rw, i - off, op)?, op)?;
                put(line, j, at(&rw, j - off, op)?, op)?;
                i += 1;
                j -= 1;
            }
        }

        // See if knot was removable for each row.
        if !rmf {
            break;
        }

        // Successful removal -> shift down knots.
        for l in (spn - k)..(rk - k) {
            let x = at(kq, l + 1, op)?;
            put(kq, l, x, op)?;
        }

        first -= 1;
        last += 1;
        k += 1;
    }

    // If no knot was removed --> out.
    if k > 0 {
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
        for row in 0..=rows {
            for (a, l) in (j..).zip((i + 1)..=hi) {
                let c = at2(lines, row, l, op)?;
                put2(lines, row, a, c, op)?;
            }
        }
    }

    Ok((k, error))
}

/// Updates the knot-removal error bound `mr` for removing the knot `T[r]` (of
/// multiplicity `s`) once in `dir` (`N_SrfRemoveOneKnot`), using the rows (u
/// direction: columns `f..=l`; v direction: rows `f..=l`) of the net. `mr ==
/// f64::MAX` (upstream `NL_BIGD`) is first reset to `-1`. Returns the updated bound.
///
/// Upstream assumes, without checking, that `T[r]` is an interior knot with
/// `T[r] != T[r+1]` and multiplicity `s > 0`.
///
/// Errors: [`NlibCode::Call`] for [`Direction::UV`]; `InvalidInput` for indices
/// outside the surface's arrays.
pub fn remove_one_knot(
    sur: &Surface,
    r: usize,
    s: usize,
    f: usize,
    l: usize,
    dir: Direction,
    mr: f64,
) -> Result<f64> {
    const OP: &str = "nurbs::srf_tool::p2::remove_one_knot";
    // Local notation; the net is read in place below (upstream reads `Pw` directly),
    // not copied and transposed.
    let idx = sur.indices();
    let (p, q) = (sx(idx.p, OP)?, sx(idx.q, OP)?);
    let (u, v) = (sur.u_knots(), sur.v_knots());
    let (r, s, f, l) = (sx(r, OP)?, sx(s, OP)?, sx(f, OP)?, sx(l, OP)?);

    let a = p.max(q);
    let mut alf: Vec<f64> = zeros1(twice(a, OP)?, OP)?;
    let mut oma: Vec<f64> = zeros1(twice(a, OP)?, OP)?;
    let mut bet: Vec<f64> = zeros1(twice(a, OP)?, OP)?;
    let mut omb: Vec<f64> = zeros1(twice(a, OP)?, OP)?;
    let mut rw: Vec<CPoint> = zeros1(twice(a, OP)?, OP)?;

    let mut mr = mr;
    if mr == f64::MAX {
        mr = -1.0;
    }

    let (deg, kn) = match dir {
        Direction::U => (p, u),
        Direction::V => (q, v),
        Direction::UV => return Err(SolidError::nlib(OP, NlibCode::Call)),
    };
    // Entry `i` of line `row`: `Pw[i][row]` in u (a column), `Pw[row][i]` in v.
    let line_at = |row: isize, i: isize| -> Result<CPoint> {
        let (a, b) = if dir == Direction::U {
            (i, row)
        } else {
            (row, i)
        };
        at(sur.row(ix(a, OP)?).map_err(|_| oob(OP))?, b, OP)
    };

    let first = r - deg;
    let last = r - s;
    let off = first - 1;

    // To avoid a possible crash upstream makes sure [last + 1 - off] is positive
    // (u direction only).
    if dir == Direction::U && off > last + 1 {
        return Err(oob(OP));
    }

    // Save some parameters.
    let kr = at(kn, r, OP)?;
    let mut i = first;
    let mut j = last;
    while j - i > 0 {
        let ki = at(kn, i, OP)?;
        let al = (at(kn, i + deg + 1, OP)? - ki) / (kr - ki);
        put(&mut alf, i - first, al, OP)?;
        put(&mut oma, i - first, 1.0 - al, OP)?;
        let kj = at(kn, j + deg + 1, OP)?;
        let be = (kj - at(kn, j, OP)?) / (kj - kr);
        put(&mut bet, j - first, be, OP)?;
        put(&mut omb, j - first, 1.0 - be, OP)?;
        i += 1;
        j -= 1;
    }

    let ki = at(kn, i, OP)?;
    let del = (kr - ki) / (at(kn, i + deg + 1, OP)? - ki);
    let omd = 1.0 - del;

    // Update maximum error for the requested rows.
    for row in f..=l {
        let mut i = first;
        let mut j = last;
        let mut ii: isize = 1;
        let mut jj = last - off;

        put(&mut rw, 0, line_at(row, off)?, OP)?;
        put(&mut rw, last + 1 - off, line_at(row, last + 1)?, OP)?;

        // Get new control points for one removal step.
        while j - i > 0 {
            let c = CPoint::combine2(
                at(&alf, i - first, OP)?,
                line_at(row, i)?,
                at(&oma, i - first, OP)?,
                at(&rw, ii - 1, OP)?,
            );
            put(&mut rw, ii, c, OP)?;
            let c = CPoint::combine2(
                at(&bet, j - first, OP)?,
                line_at(row, j)?,
                at(&omb, j - first, OP)?,
                at(&rw, jj + 1, OP)?,
            );
            put(&mut rw, jj, c, OP)?;
            i += 1;
            j -= 1;
            ii += 1;
            jj -= 1;
        }

        // Compute the error.
        let dw = if j - i < 0 {
            dist_cpt_cpt_homo(at(&rw, ii - 1, OP)?, at(&rw, jj + 1, OP)?)
        } else {
            let c = CPoint::combine2(del, at(&rw, jj + 1, OP)?, omd, at(&rw, ii - 1, OP)?);
            dist_cpt_cpt_homo(line_at(row, i)?, c)
        };

        if dw > mr {
            mr = dw;
        }
    }
    Ok(mr)
}

/// Elevates the degree of `sur` in `dir` by `t` (`N_SrfElevateDegree`, the surface
/// form of algorithm A5.9): each row of the net is cut into Bezier strips, elevated
/// and the knots between them removed again.
///
/// Errors: [`NlibCode::Degree`] when the new degree exceeds `MAX_DEGREE`;
/// [`NlibCode::Knot`] for an invalid knot vector in `dir`; [`NlibCode::Call`] for
/// [`Direction::UV`]; errors of the Bezier elevation.
pub fn elevate_degree(sur: &Surface, t: usize, dir: Direction) -> Result<Surface> {
    const OP: &str = "nurbs::srf_tool::p2::elevate_degree";
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

    let (lines, hi, deg, kn, rk) = match dir {
        Direction::U => (transpose(&pw), n, p, up, r),
        Direction::V => (pw, m, q, vp, s),
        Direction::UV => return Err(SolidError::nlib(OP, NlibCode::Call)),
    };

    // Check parameters and compute highest indexes.
    let degu = ix(deg, OP)?;
    if degu.checked_add(t).is_none_or(|d| d > MAX_DEGREE) {
        return Err(SolidError::nlib(OP, NlibCode::Degree));
    }
    let kv = KnotVector::from_slice(kn)?;
    kv.validate(degu)?;
    let spans = sx(kv.span_count(degu), OP)?;
    let ts = sx(t, OP)?;
    let hq = hi + ts * spans;
    let rq = rk + ts * (spans + 1);
    let dq = deg + ts;

    let (qlines, kq) = if t == 0 {
        // No elevation required: copy.
        (lines, kn.to_vec())
    } else {
        elevate_rows(
            &lines,
            ElevateArgs {
                hi: hq,
                deg,
                rk,
                rq,
                dq,
                t,
                a: p.max(q),
            },
            kn,
            OP,
        )?
    };

    match dir {
        Direction::U => {
            let qlines = cut(qlines, m, hq, OP)?;
            surface(transpose(&qlines), dq, q, kq, vp.to_vec(), OP)
        }
        _ => surface(cut(qlines, n, hq, OP)?, p, dq, up.to_vec(), kq, OP),
    }
}

/// Scalars of one degree elevation along the rows of a net.
#[derive(Clone, Copy)]
struct ElevateArgs {
    /// Highest control point index along a new row.
    hi: isize,
    /// Old degree.
    deg: isize,
    /// Highest old knot index.
    rk: isize,
    /// Highest new knot index.
    rq: isize,
    /// New degree.
    dq: isize,
    /// Elevation increment (`> 0`).
    t: usize,
    /// `max(p, q)`: sizes of the scratch arrays.
    a: isize,
}

/// The elevation loop of `N_SrfElevateDegree` for one direction: the new rows
/// (`hi + 1` points each) and knots from the old rows `pw[row][i]` and knots `up`.
fn elevate_rows(
    pw: &Net,
    args: ElevateArgs,
    up: &[f64],
    op: &'static str,
) -> Result<(Net, Vec<f64>)> {
    let ElevateArgs {
        hi,
        deg: pp,
        rk: rp,
        rq,
        dq: pq,
        t,
        a: asz,
    } = args;
    let rows = sx(pw.len(), op)? - 1;
    let ppu = ix(pp, op)?;
    let pqu = ix(pq, op)?;

    let mut qw: Net = zeros2(rows, hi, op)?;
    let mut uq: Vec<f64> = zeros1(rq, op)?;
    let mut bw: Net = zeros2(rows, pp, op)?;
    let mut nw: Net = zeros2(rows, pp, op)?;
    let mut dw: Net = zeros2(rows, pq, op)?;
    let mut ralf: Vec<Vec<f64>> = zeros2(asz, twice(asz, op)?, op)?;
    let mut roma: Vec<Vec<f64>> = zeros2(asz, twice(asz, op)?, op)?;
    let mut rbet: Vec<Vec<f64>> = zeros2(asz, twice(asz, op)?, op)?;
    let mut romb: Vec<Vec<f64>> = zeros2(asz, twice(asz, op)?, op)?;
    let mut alfs: Vec<f64> = zeros1(asz, op)?;
    let mut omas: Vec<f64> = zeros1(asz, op)?;

    let mut a = pp;
    let mut b = pp + 1;
    let mut r: isize = -1;
    let mut cind: isize = 1;
    let mut kind = pq + 1;
    let mut pind: isize = 0;

    let ua = at(up, a, op)?;
    for i in 0..=pq {
        put(&mut uq, i, ua, op)?;
    }

    // Initialize Bezier strip.
    for row in 0..=rows {
        put2(&mut qw, row, 0, at2(pw, row, 0, op)?, op)?;
        for i in 0..=pp {
            put2(&mut bw, row, i, at2(pw, row, i, op)?, op)?;
        }
    }

    // Get degree elevation matrix.
    let dm = elevation_matrix(ppu, t)?;

    // Loop through the knot vector: extract the i-th Bezier strip, degree elevate
    // it, remove the knot between the i-th and the (i-1)-th strip.
    while b < rp {
        // Get multiplicity of the knot.
        let i0 = b;
        while b < rp && at(up, b, op)? == at(up, b + 1, op)? {
            b += 1;
        }
        let mlt = b - i0 + 1;

        let oldr = r;
        r = pp - mlt;

        let lbz = if oldr > 0 { (oldr + 2) / 2 } else { 1 };
        let rbz = if r > 0 { pq - (r + 1) / 2 } else { pq };

        // Save some entities.
        if r > 0 {
            let num = at(up, b, op)? - at(up, a, op)?;
            let mut k = pp;
            while k > mlt {
                let al = num / (at(up, a + k, op)? - at(up, a, op)?);
                put(&mut alfs, k - mlt - 1, al, op)?;
                put(&mut omas, k - mlt - 1, 1.0 - al, op)?;
                k -= 1;
            }
        }

        let mut first = kind - 2;
        let ub = at(up, b, op)?;
        let den = ub - at(up, a, op)?;

        for (k, last) in (1..oldr).zip(kind..) {
            let mut i = first;
            let mut j = last;
            let ralf_k = ralf.get_mut(ix(k, op)?).ok_or_else(|| oob(op))?;
            let roma_k = roma.get_mut(ix(k, op)?).ok_or_else(|| oob(op))?;
            let rbet_k = rbet.get_mut(ix(k, op)?).ok_or_else(|| oob(op))?;
            let romb_k = romb.get_mut(ix(k, op)?).ok_or_else(|| oob(op))?;
            while j - i > k {
                if i < cind {
                    let uqi = at(&uq, i, op)?;
                    let al = (ub - uqi) / (at(up, a, op)? - uqi);
                    put(ralf_k, i - first, al, op)?;
                    put(roma_k, i - first, 1.0 - al, op)?;
                }
                if j >= lbz {
                    let be = (ub - at(&uq, j - k, op)?) / den;
                    put(rbet_k, last - j, be, op)?;
                    put(romb_k, last - j, 1.0 - be, op)?;
                }
                i += 1;
                j -= 1;
            }
            first -= 1;
        }

        // For each row, perform curve degree elevation.
        for row in 0..=rows {
            pind = cind;

            // Insert knot.
            if r > 0 {
                for j in 1..=r {
                    let save = r - j;
                    let s = mlt + j;
                    let mut k = pp;
                    while k >= s {
                        let c = CPoint::combine2(
                            at(&alfs, k - s, op)?,
                            at2(&bw, row, k, op)?,
                            at(&omas, k - s, op)?,
                            at2(&bw, row, k - 1, op)?,
                        );
                        put2(&mut bw, row, k, c, op)?;
                        k -= 1;
                    }
                    put2(&mut nw, row, save, at2(&bw, row, pp, op)?, op)?;
                }
            }

            // Now degree elevate the Bezier strip.
            srf_elevate_degree(
                &bw,
                ppu,
                t,
                &dm,
                Direction::V,
                ix(lbz, op)?,
                pqu,
                ix(row, op)?,
                &mut dw,
            )?;

            // Remove the knot UP[a].
            if oldr > 1 {
                let mut first = kind - 2;
                for (k, last) in (1..oldr).zip(kind..) {
                    let mut i = first;
                    let mut j = last;
                    let mut l = j - kind + 1;
                    let kk = ix(k, op)?;
                    while j - i > k {
                        if i < cind {
                            let ra = ralf.get(kk).ok_or_else(|| oob(op))?;
                            let ro = roma.get(kk).ok_or_else(|| oob(op))?;
                            let c = CPoint::combine2(
                                at(ra, i - first, op)?,
                                at2(&qw, row, i, op)?,
                                at(ro, i - first, op)?,
                                at2(&qw, row, i - 1, op)?,
                            );
                            put2(&mut qw, row, i, c, op)?;
                        }
                        if j >= lbz {
                            let rb = rbet.get(kk).ok_or_else(|| oob(op))?;
                            let rm = romb.get(kk).ok_or_else(|| oob(op))?;
                            let c = CPoint::combine2(
                                at(rb, last - j, op)?,
                                at2(&dw, row, l, op)?,
                                at(rm, last - j, op)?,
                                at2(&dw, row, l + 1, op)?,
                            );
                            put2(&mut dw, row, l, c, op)?;
                        }
                        i += 1;
                        j -= 1;
                        l -= 1;
                    }
                    first -= 1;
                }
            }

            // Load control points.
            for i in lbz..=rbz {
                put2(&mut qw, row, pind, at2(&dw, row, i, op)?, op)?;
                pind += 1;
            }

            // Initialize for next pass through.
            if b < rp {
                for i in 0..r {
                    put2(&mut bw, row, i, at2(&nw, row, i, op)?, op)?;
                }
                for i in r..=pp {
                    put2(&mut bw, row, i, at2(pw, row, b - pp + i, op)?, op)?;
                }
            }
        }

        cind = pind;

        // Load knot vector and prepare for next pass through.
        if a != pp {
            let ua = at(up, a, op)?;
            for _ in 0..(pq - oldr) {
                put(&mut uq, kind, ua, op)?;
                kind += 1;
            }
        }

        if b < rp {
            a = b;
            b += 1;
        } else {
            let ub = at(up, b, op)?;
            for i in 0..=pq {
                put(&mut uq, kind + i, ub, op)?;
            }
        }
    }

    Ok((qw, uq))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::knots::Side;
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

    fn max_dist(a: &Surface, b: &Surface) -> f64 {
        let mut d: f64 = 0.0;
        for k in 0..=10 {
            for l in 0..=10 {
                let (u, v) = (f64::from(k) / 10.0, f64::from(l) / 10.0);
                let pa = a.eval(u, v, Side::Left, Side::Left).unwrap();
                let pb = b.eval(u, v, Side::Left, Side::Left).unwrap();
                d = d.max((pa - pb).length());
            }
        }
        d
    }

    #[test]
    fn elevate_then_remove_round_trips() {
        let s = sample();
        let e = elevate_degree(&s, 1, Direction::U).unwrap();
        assert_eq!(e.p(), 4);
        assert_eq!(e.n(), 4 + 2);
        assert_eq!(
            e.u_knots(),
            &[0.0, 0.0, 0.0, 0.0, 0.0, 0.4, 0.4, 1.0, 1.0, 1.0, 1.0, 1.0]
        );
        assert!(max_dist(&s, &e) < 1e-12);

        // An inserted knot is removable again.
        let ins = super::super::p1::insert_knot(&e, 0.7, 2, Direction::U).unwrap();
        let rm = remove_knot_conditional(&ins, 0.7, 3, 1e-9, Direction::U).unwrap();
        assert_eq!(rm.removed, 2);
        assert_eq!(rm.surface.n(), e.n());
        assert!(max_dist(&s, &rm.surface) < 1e-9);

        let ev = elevate_degree(&s, 2, Direction::V).unwrap();
        assert_eq!(ev.q(), 4);
        assert_eq!(ev.m(), 3 + 2 * 2);
        assert!(max_dist(&s, &ev) < 1e-12);
    }

    #[test]
    fn elevate_errors_and_zero() {
        let s = sample();
        assert_eq!(elevate_degree(&s, 0, Direction::U).unwrap(), s);
        assert!(matches!(
            elevate_degree(&s, 15, Direction::U),
            Err(SolidError::Nlib {
                code: NlibCode::Degree,
                ..
            })
        ));
        assert!(matches!(
            elevate_degree(&s, 1, Direction::UV),
            Err(SolidError::Nlib {
                code: NlibCode::Call,
                ..
            })
        ));
    }

    #[test]
    fn remove_conditional_respects_tolerance() {
        let s = sample();
        // 0.4 is a genuine knot of the cubic: not removable at a tight tolerance.
        let rm = remove_knot_conditional(&s, 0.4, 1, 1e-12, Direction::U).unwrap();
        assert_eq!(rm.removed, 0);
        assert_eq!(rm.surface, s);
        // A non-knot parameter has multiplicity 0.
        let rm = remove_knot_conditional(&s, 0.3, 1, 1.0, Direction::V).unwrap();
        assert_eq!(rm.removed, 0);
        assert!(matches!(
            remove_knot_conditional(&s, 0.0, 1, 1.0, Direction::U),
            Err(SolidError::Nlib {
                code: NlibCode::Parameter,
                ..
            })
        ));
    }

    #[test]
    fn remove_one_knot_bounds() {
        let s = sample();
        let e = super::super::p1::insert_knot(&s, 0.7, 1, Direction::U).unwrap();
        // Knot 0.7 is U[5] of e (multiplicity 1).
        let mr = remove_one_knot(&e, 5, 1, 0, 3, Direction::U, f64::MAX).unwrap();
        assert!((0.0..1e-9).contains(&mr));
        // A genuine knot has a positive error bound.
        let mr = remove_one_knot(&s, 4, 1, 0, 3, Direction::U, f64::MAX).unwrap();
        assert!(mr > 1e-6);
        let mr2 = remove_one_knot(&s, 4, 1, 0, 3, Direction::U, 1e9).unwrap();
        assert_eq!(mr2, 1e9);
        assert!(remove_one_knot(&s, 3, 1, 0, 9, Direction::V, -1.0).is_err());
    }

    #[test]
    fn absurd_degree_is_an_error() {
        // fuzz (nlib.srf_tool_p2 fuzz00301): `2 * max(p, q)` overflowed.
        let s = sample();
        let (u, v) = (s.u_knots().to_vec(), s.v_knots().to_vec());
        let bad = Surface::new(s.cpts(), i64::MAX as usize, s.q(), u, v).unwrap();
        assert!(remove_one_knot(&bad, 4, 1, 0, 1, Direction::U, f64::MAX).is_err());
        assert!(elevate_degree(&bad, 1, Direction::V).is_err());
    }
}
