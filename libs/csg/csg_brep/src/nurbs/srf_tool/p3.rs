//! Surface knot removal of all removable knots, compatibility of a set of surfaces
//! and joining of two surfaces.
//!
//! Upstream: `source/SMLib/src/NL_SrfTool.cpp`, `source/SMLib/inc/NL_SrfTool.h`.
//!
//! | Rust                        | Upstream                                                             |
//! |-----------------------------|----------------------------------------------------------------------|
//! | [`remove_all_knots`]        | `N_SrfRemoveAllKnots`                                                |
//! | [`make_compatible`]         | `N_MakeSrfsCompatible`                                               |
//! | [`join`]                    | `N_SrfJoin`                                                          |
//! | `NO_REMOVAL` (private)      | `NOREM` (file-private)                                               |
//! | `remove_knots_kept` (crate) | shared body of `N_SrfRemoveAllKnots` / `N_SrfRemoveKnotsConstraints` |
//!
//! Deviations:
//! - Out-params become return values: [`remove_all_knots`] and [`join`] return a
//!   new surface instead of filling a caller-provided one (upstream's in-place modes
//!   compute the same result); [`make_compatible`] edits the slice of surfaces in
//!   place, as upstream edits its array. The `NL_STACKS` arguments have no
//!   counterpart.
//! - The u- and v-direction branches of `N_SrfRemoveAllKnots` are one implementation
//!   parameterised by the removal direction (control points, the error array and the
//!   knots are addressed through the direction); the arithmetic and the iteration
//!   order are upstream's in both directions.
//! - `N_SrfRemoveAllKnots` returns its error flag after finishing when the last
//!   `N_BasisFindAllSpanMaxima` it called failed and nothing reset the flag (it then
//!   assumes bounds of `0` and `1`); [`remove_all_knots`] returns only that error then,
//!   as `nurbs::srf_tool::p2::remove_knot_conditional` does. A non-NLib error of that
//!   search (where upstream would index outside its arrays or loop forever) is
//!   returned at once.
//! - `N_SrfJoin` ignores the error of its final `N_SrfRemoveKnotConditional`; [`join`]
//!   does the same for NLib errors (keeping the surface upstream leaves behind) but
//!   returns non-NLib errors, which mark inputs where upstream indexes outside its
//!   arrays.
//! - `N_MakeSrfsCompatible` ignores the status of `N_SrfReparam` /
//!   `N_SrfReparamToInterval`; their only Rust error is `InvalidInput` for a knot
//!   vector shorter than the degree (where upstream writes outside the array), which
//!   [`make_compatible`] returns. `k` (the highest surface index) is the slice length
//!   minus one; an empty slice returns `InvalidInput`.
//! - Every array access is bounds checked; where upstream would index outside its
//!   arrays (invalid surfaces) the functions return [`SolidError::InvalidInput`].
//! - `N_SrfRemoveAllKnots` loops forever when a removal step leaves its state
//!   unchanged (the chosen knot was already unremovable and stays so; e.g. a NaN
//!   error bound in u with no removable v knot sends every `NL_UVDIR` step to v);
//!   [`remove_all_knots`] detects exactly that and returns
//!   [`SolidError::NotConverged`].

use crate::error::{NlibCode, Result, SolidError};
use crate::math::max2;
use crate::nurbs::basis_basic::{compatible_knot_array, find_all_span_maxima};
use crate::nurbs::geom::Rectangle;
use crate::nurbs::knots::KnotVector;
use crate::nurbs::math::{CPoint, BIGD, NOW};
use crate::nurbs::srf_basic::{make_rat_compatible, Direction, Surface, WEIGHT_MAX, WEIGHT_MIN};

use super::p1::{
    at, at2, degree, insert_knots, ix, oob, put, put2, sx, twice, zeros1, zeros2, Local,
};
use super::p2::{
    elevate_degree, remove_knot_conditional_with_status, remove_one_knot, REMOVAL_PARAM_TOL,
};

/// Upstream `NOREM`: the error bound marking a knot that cannot be removed.
const NO_REMOVAL: f64 = 1.0e+25;

/// `0` for the u direction, `1` for v.
fn slot(d: Direction) -> usize {
    usize::from(d == Direction::V)
}

/// The other parametric direction.
fn other(d: Direction) -> Direction {
    if d == Direction::U {
        Direction::V
    } else {
        Direction::U
    }
}

/// `(i, j)` (u index, v index) of the entry `a` along `d` and `b` across it.
fn uv(d: Direction, a: isize, b: isize) -> (isize, isize) {
    if d == Direction::U {
        (a, b)
    } else {
        (b, a)
    }
}

/// Degree, highest net index and highest knot index of `sur` along `d`.
fn dims(sur: &Surface, d: Direction, op: &'static str) -> Result<(isize, isize, isize)> {
    let x = sur.indices();
    if d == Direction::U {
        Ok((degree(x.p, op)?, sx(x.n, op)?, sx(x.r, op)?))
    } else {
        Ok((degree(x.q, op)?, sx(x.m, op)?, sx(x.s, op)?))
    }
}

/// The knots of `sur` along `d`.
fn knots(sur: &Surface, d: Direction) -> &[f64] {
    if d == Direction::U {
        sur.u_knots()
    } else {
        sur.v_knots()
    }
}

/// Control point `line` across, `idx` along `d`.
fn qget(sur: &Surface, d: Direction, line: isize, idx: isize, op: &'static str) -> Result<CPoint> {
    let (i, j) = uv(d, idx, line);
    sur.cpt(ix(i, op)?, ix(j, op)?)
}

/// Sets control point `line` across, `idx` along `d`.
fn qset(
    sur: &mut Surface,
    d: Direction,
    line: isize,
    idx: isize,
    c: CPoint,
    op: &'static str,
) -> Result<()> {
    let (i, j) = uv(d, idx, line);
    *sur.cpt_mut(ix(i, op)?, ix(j, op)?)? = c;
    Ok(())
}

/// `w[i][j]` with `(i, j) = uv(d, a, b)`.
fn eget(w: &[Vec<f64>], d: Direction, a: isize, b: isize, op: &'static str) -> Result<f64> {
    let (i, j) = uv(d, a, b);
    at2(w, i, j, op)
}

/// `w[i][j] = x` with `(i, j) = uv(d, a, b)`.
fn eput(
    w: &mut [Vec<f64>],
    d: Direction,
    a: isize,
    b: isize,
    x: f64,
    op: &'static str,
) -> Result<()> {
    let (i, j) = uv(d, a, b);
    put2(w, i, j, x, op)
}

/// `N_BasisFindAllSpanMaxima` on the current knots of `sur` along `d`, with
/// upstream's fallback bounds (`0` and `1`) when it fails. Records the status in
/// `error` (upstream's running `error` flag).
fn span_maxima(
    sur: &Surface,
    d: Direction,
    i: isize,
    deg: isize,
    tol: f64,
    error: &mut Option<SolidError>,
    op: &'static str,
) -> Result<(Vec<f64>, Vec<f64>)> {
    let (ku, kv) = sur.knot_vectors()?;
    let kn: KnotVector<'_> = if d == Direction::U { ku } else { kv };
    let degu = ix(deg, op)?;
    match find_all_span_maxima(&kn, ix(i, op)?, degu, tol) {
        Ok(sm) => {
            *error = None;
            Ok((sm.min, sm.max))
        }
        Err(e @ SolidError::Nlib { .. }) => {
            *error = Some(e);
            Ok((vec![0.0; degu + 1], vec![1.0; degu + 1]))
        }
        Err(e) => Err(e),
    }
}

/// State of `N_SrfRemoveAllKnots` shared by both removal directions.
struct RemoveAll {
    /// The working surface (upstream `surQ`).
    sq: Surface,
    /// Requested direction(s).
    dir: Direction,
    /// Removal tolerance (scaled for rational surfaces).
    tol: f64,
    /// `N_IsSrfRat(surP)`.
    rational: bool,
    /// Parameter tolerances of the span maxima (`stu`, `stv`).
    st: [f64; 2],
    /// Accumulated errors `er[i][j]` and trial errors `te[i][j]` per span pair.
    er: Vec<Vec<f64>>,
    te: Vec<Vec<f64>>,
    /// Removal error bounds (`bru`, `brv`) and multiplicities (`sru`, `srv`).
    bru: Vec<f64>,
    brv: Vec<f64>,
    sru: Vec<isize>,
    srv: Vec<isize>,
    /// Scratch arrays.
    alf: Vec<f64>,
    oma: Vec<f64>,
    bet: Vec<f64>,
    omb: Vec<f64>,
    max: Vec<f64>,
    rw: Vec<Vec<CPoint>>,
    lam: f64,
    oml: f64,
    /// Upstream's running `error` flag.
    error: Option<SolidError>,
}

/// Removes every knot of `sur` (in u, v or both) whose removal keeps the surface
/// within `tol` (`N_SrfRemoveAllKnots`): knots are removed one at a time, always the
/// one with the smallest error bound, accumulating the bounds per span pair; weights
/// must stay within `[WEIGHT_MIN, WEIGHT_MAX]`. The tolerance is scaled for rational
/// surfaces by `wmin / (1 + pmax)`.
///
/// Errors: the error of the last basis-maximum search when it failed and nothing was
/// removed (see the module deviations); [`NlibCode::Call`] from the one-step error
/// bound; `InvalidInput` for invalid surfaces.
pub fn remove_all_knots(sur: &Surface, tol: f64, dir: Direction) -> Result<Surface> {
    remove_knots_kept(
        sur,
        [None, None],
        tol,
        dir,
        "nurbs::srf_tool::p3::remove_all_knots",
    )
}

/// The body shared by `N_SrfRemoveAllKnots` and `N_SrfRemoveKnotsConstraints`
/// (`nurbs::srf_tool::p4::remove_knots_constrained`), which differ only in the
/// initial error bounds: a knot listed in `keep[0]` (u) / `keep[1]` (v) starts as not
/// removable instead of getting its one-step bound. Each list is sorted and is
/// walked alongside the distinct knots exactly as upstream walks `UK` / `VK`; a list
/// whose entries all equal the first knot (upstream reads past it) is
/// `InvalidInput`.
pub(super) fn remove_knots_kept(
    sur: &Surface,
    keep: [Option<&[f64]>; 2],
    tol: f64,
    dir: Direction,
    op: &'static str,
) -> Result<Surface> {
    let Local {
        n,
        m,
        p,
        q,
        r,
        s,
        u: up,
        v: vp,
        ..
    } = Local::of(sur, op)?;
    let (ns, ms) = (n, m);

    // Adjust removal tolerance in case of rational surfaces.
    let mut tol = tol;
    let rational = sur.is_rational();
    if rational {
        let mm = sur.min_max_weight_pos_vectors();
        tol = (tol * mm.wmin) / (1.0 + mm.pmax);
    }

    let stu = REMOVAL_PARAM_TOL * (at(up, r, op)? - at(up, 0, op)?).abs();
    let stv = REMOVAL_PARAM_TOL * (at(vp, s, op)? - at(vp, 0, op)?).abs();

    // Allocate local memory.
    let a = p.max(q);
    let b = n.max(m);
    let mut w = RemoveAll {
        sq: sur.clone(),
        dir,
        tol,
        rational,
        st: [stu, stv],
        er: zeros2(r, s, op)?,
        te: zeros2(r, s, op)?,
        bru: vec![BIGD; ix(r + 1, op)?],
        brv: vec![BIGD; ix(s + 1, op)?],
        sru: vec![0; ix(r + 1, op)?],
        srv: vec![0; ix(s + 1, op)?],
        alf: zeros1(twice(a, op)?, op)?,
        oma: zeros1(twice(a, op)?, op)?,
        bet: zeros1(twice(a, op)?, op)?,
        omb: zeros1(twice(a, op)?, op)?,
        max: zeros1(a + 1, op)?,
        rw: zeros2(b, twice(a, op)?, op)?,
        lam: 0.0,
        oml: 0.0,
        error: None,
    };

    let do_u = matches!(dir, Direction::U | Direction::UV);
    let do_v = matches!(dir, Direction::V | Direction::UV);

    // Compute the maximum of knot removal errors for each distinct knot.
    for (d, on) in [(Direction::U, do_u), (Direction::V, do_v)] {
        if !on {
            continue;
        }
        let (deg, hi, _) = dims(sur, d, op)?;
        let (_, ohi, _) = dims(sur, other(d), op)?;
        let kn = knots(sur, d);
        let kept = keep.get(slot(d)).copied().flatten();
        let mut kc: isize = 0;
        if let Some(kk) = kept {
            let k0 = at(kn, 0, op)?;
            while at(kk, kc, op)? == k0 {
                kc += 1;
            }
        }
        let mut rd = deg + 1;
        while rd <= hi {
            let i = rd;
            while rd <= hi && at(kn, rd, op)? == at(kn, rd + 1, op)? {
                rd += 1;
            }
            let sd = rd - i + 1;
            put(w.sr_mut(d), rd, sd, op)?;
            let mut krb = true;
            if let Some(kk) = kept {
                let mc = sx(kk.len(), op)? - 1;
                while kc < mc && at(kk, kc, op)? == at(kk, kc + 1, op)? {
                    kc += 1;
                }
                if kc <= mc && at(kk, kc, op)? == at(kn, rd, op)? {
                    krb = false;
                    kc += 1;
                }
            }
            if krb {
                let mr = at(w.br(d), rd, op)?;
                let mr = remove_one_knot(sur, ix(rd, op)?, ix(sd, op)?, 0, ix(ohi, op)?, d, mr)?;
                put(w.br_mut(d), rd, mr, op)?;
            } else {
                put(w.br_mut(d), rd, NO_REMOVAL, op)?;
            }
            rd += 1;
        }
    }

    // Try to remove each knot.
    let (mut bu, mut su, mut ru) = (0.0, 0, 0);
    let (mut bv, mut sv, mut rv) = (0.0, 0, 0);
    let mut steps: u32 = 0;
    loop {
        // Find knot with smallest error.
        let x = w.sq.indices();
        if do_u {
            let (pp, rr) = (degree(x.p, op)?, sx(x.r, op)?);
            bu = at(&w.bru, pp + 1, op)?;
            su = at(&w.sru, pp + 1, op)?;
            ru = pp + 1;
            for i in (pp + 2)..(rr - pp) {
                let bi = at(&w.bru, i, op)?;
                if bi < bu {
                    bu = bi;
                    su = at(&w.sru, i, op)?;
                    ru = i;
                }
            }
        }
        if do_v {
            let (qq, ss) = (degree(x.q, op)?, sx(x.s, op)?);
            bv = at(&w.brv, qq + 1, op)?;
            sv = at(&w.srv, qq + 1, op)?;
            rv = qq + 1;
            for j in (qq + 2)..(ss - qq) {
                let bj = at(&w.brv, j, op)?;
                if bj < bv {
                    bv = bj;
                    sv = at(&w.srv, j, op)?;
                    rv = j;
                }
            }
        }

        // If no more removable knot -> finished.
        let done = |b: f64| b == BIGD || b == NO_REMOVAL;
        let finished = match dir {
            Direction::U => done(bu),
            Direction::V => done(bv),
            Direction::UV => done(bu) && done(bv),
        };
        if finished {
            break;
        }

        let krm = if dir == Direction::UV {
            if bu < bv {
                Direction::U
            } else {
                Direction::V
            }
        } else {
            dir
        };

        // Remove in the chosen direction.
        let (rk, sk, bk) = if krm == Direction::U {
            (ru, su, bu)
        } else {
            (rv, sv, bv)
        };
        let was_kept = at(w.br(krm), rk, op)? == NO_REMOVAL;
        w.step(krm, rk, sk, bk, op)?;
        steps = steps.saturating_add(1);

        // A step that removes nothing from a knot already marked unremovable leaves
        // every input of the selection unchanged: upstream picks it again forever
        // (e.g. a NaN error bound in u makes `bu < bv` false while v is done).
        if was_kept && w.sq.indices() == x && at(w.br(krm), rk, op)? == NO_REMOVAL {
            return Err(SolidError::NotConverged {
                operation: op,
                iterations: steps,
            });
        }
    }

    // Compact surface.
    let x = w.sq.indices();
    if sx(x.n, op)? < ns || sx(x.m, op)? < ms {
        w.sq.compress();
        w.error = None;
    }

    match w.error {
        Some(e) => Err(e),
        None => Ok(w.sq),
    }
}

impl RemoveAll {
    /// `bru` / `brv`.
    fn br(&self, d: Direction) -> &[f64] {
        if d == Direction::U {
            &self.bru
        } else {
            &self.brv
        }
    }

    fn br_mut(&mut self, d: Direction) -> &mut [f64] {
        if d == Direction::U {
            &mut self.bru
        } else {
            &mut self.brv
        }
    }

    /// `sru` / `srv`.
    fn sr(&self, d: Direction) -> &[isize] {
        if d == Direction::U {
            &self.sru
        } else {
            &self.srv
        }
    }

    fn sr_mut(&mut self, d: Direction) -> &mut [isize] {
        if d == Direction::U {
            &mut self.sru
        } else {
            &mut self.srv
        }
    }

    /// One pass of the removal loop in direction `d` (one `case` of upstream's
    /// `switch`): try to remove the knot `rd` (multiplicity `sd`, error bound `bd`).
    fn step(
        &mut self,
        d: Direction,
        rd: isize,
        sd: isize,
        bd: f64,
        op: &'static str,
    ) -> Result<()> {
        let o = other(d);
        let kd = slot(d);
        let (deg, hi, rk) = dims(&self.sq, d, op)?;
        let (odeg, ohi, ork) = dims(&self.sq, o, op)?;
        let st = self.st.get(kd).copied().ok_or_else(|| oob(op))?;

        let mut rmf = true;
        let odd = (deg + sd) % 2 != 0;
        let (k, l);
        if odd {
            // Compute maximums of basis functions over each span.
            k = (deg + sd + 1) / 2;
            l = rd - k + deg + 1;
            let kn = knots(&self.sq, d);
            let krd = at(kn, rd, op)?;
            let al =
                (krd - at(kn, rd - k, op)?) / (at(kn, rd - k + deg + 1, op)? - at(kn, rd - k, op)?);
            let be = (krd - at(kn, rd - k + 1, op)?)
                / (at(kn, rd - k + deg + 2, op)? - at(kn, rd - k + 1, op)?);
            let ob = 1.0 - be;
            self.lam = al / (al + be);
            self.oml = 1.0 - self.lam;
            let (lam, oml) = (self.lam, self.oml);

            let (mut minl, mut maxl) =
                span_maxima(&self.sq, d, rd - k, deg, st, &mut self.error, op)?;
            let (mut minr, mut maxr) =
                span_maxima(&self.sq, d, rd - k + 1, deg, st, &mut self.error, op)?;

            put(&mut self.max, 0, lam * al * at(&maxl, 0, op)?, op)?;
            for i in 1..=deg {
                let x = at(&minl, i, op)? * (lam * al);
                put(&mut minl, i, x, op)?;
                let x = at(&minr, i - 1, op)? * (oml * ob);
                put(&mut minr, i - 1, x, op)?;
                let x = at(&maxl, i, op)? * (lam * al);
                put(&mut maxl, i, x, op)?;
                let x = at(&maxr, i - 1, op)? * (oml * ob);
                put(&mut maxr, i - 1, x, op)?;
                let x = max2(
                    (at(&maxl, i, op)? - at(&minr, i - 1, op)?).abs(),
                    (at(&maxr, i - 1, op)? - at(&minl, i, op)?).abs(),
                );
                put(&mut self.max, i, x, op)?;
            }
            put(&mut self.max, deg + 1, oml * ob * at(&maxr, deg, op)?, op)?;
        } else {
            // Compute maximum of basis function.
            k = (deg + sd) / 2;
            l = rd - k + deg;
            let (_, mx) = span_maxima(&self.sq, d, rd - k, deg, st, &mut self.error, op)?;
            for i in 0..=deg {
                put(&mut self.max, i, at(&mx, i, op)?, op)?;
            }
        }

        // Check the error.
        {
            let kn = knots(&self.sq, d);
            let kno = knots(&self.sq, o);
            'rows: for i in (rd - k)..=l {
                if at(kn, i, op)? != at(kn, i + 1, op)? {
                    let tmp = at(&self.max, i - rd + k, op)? * bd;
                    for j in odeg..(ork - odeg) {
                        if at(kno, j, op)? != at(kno, j + 1, op)? {
                            let t = eget(&self.er, d, i, j, op)? + tmp;
                            eput(&mut self.te, d, i, j, t, op)?;
                            if t > self.tol {
                                rmf = false;
                                break 'rows;
                            }
                        }
                    }
                }
            }
        }

        if !rmf {
            // Knot is not removable.
            put(self.br_mut(d), rd, NO_REMOVAL, op)?;
            return Ok(());
        }

        // Error test passed -> update error vector and remove knot.
        {
            let kn = knots(&self.sq, d);
            let kno = knots(&self.sq, o);
            for i in (rd - k)..=l {
                for j in odeg..(ork - odeg) {
                    if at(kn, i, op)? != at(kn, i + 1, op)?
                        && at(kno, j, op)? != at(kno, j + 1, op)?
                    {
                        let t = eget(&self.te, d, i, j, op)?;
                        eput(&mut self.er, d, i, j, t, op)?;
                    }
                }
            }
        }

        let fout = (2 * rd - sd - deg) / 2;
        let first = rd - deg;
        let last = rd - sd;
        let off = first - 1;

        // Save some parameters.
        {
            let kn = knots(&self.sq, d);
            let krd = at(kn, rd, op)?;
            let mut i = first;
            let mut j = last;
            while j - i > 0 {
                let ki = at(kn, i, op)?;
                let al = (at(kn, i + deg + 1, op)? - ki) / (krd - ki);
                put(&mut self.alf, i - first, al, op)?;
                put(&mut self.oma, i - first, 1.0 - al, op)?;
                let kj = at(kn, j + deg + 1, op)?;
                let be = (kj - at(kn, j, op)?) / (kj - krd);
                put(&mut self.bet, j - first, be, op)?;
                put(&mut self.omb, j - first, 1.0 - be, op)?;
                i += 1;
                j -= 1;
            }
        }

        // Remove the knot for each row (u) / column (v).
        let mut wfl = true;
        for line in 0..=ohi {
            let rw = self.rw.get_mut(ix(line, op)?).ok_or_else(|| oob(op))?;
            let mut i = first;
            let mut j = last;
            let mut ii: isize = 1;
            let mut jj = last - off;

            put(rw, 0, qget(&self.sq, d, line, off, op)?, op)?;
            put(
                rw,
                last + 1 - off,
                qget(&self.sq, d, line, last + 1, op)?,
                op,
            )?;

            // Get new control points for one removal step.
            while j - i > 0 {
                let c = CPoint::combine2(
                    at(&self.alf, i - first, op)?,
                    qget(&self.sq, d, line, i, op)?,
                    at(&self.oma, i - first, op)?,
                    at(rw, ii - 1, op)?,
                );
                put(rw, ii, c, op)?;
                let c = CPoint::combine2(
                    at(&self.bet, j - first, op)?,
                    qget(&self.sq, d, line, j, op)?,
                    at(&self.omb, j - first, op)?,
                    at(rw, jj + 1, op)?,
                );
                put(rw, jj, c, op)?;
                i += 1;
                j -= 1;
                ii += 1;
                jj -= 1;
            }

            // Check for disallowed weights.
            if self.rational {
                let mut i = first;
                let mut j = last;
                let mut wmin = f64::MAX;
                let mut wmax = f64::MIN_POSITIVE;
                while j - i > 0 {
                    let wi = at(rw, i - off, op)?.w;
                    let wj = at(rw, j - off, op)?.w;
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
                    wfl = false;
                    break;
                }
            }

            if odd {
                let c =
                    CPoint::combine2(self.lam, at(rw, jj + 1, op)?, self.oml, at(rw, ii - 1, op)?);
                put(rw, jj + 1, c, op)?;
            }
        }

        // See if weights are in the allowable range.
        if !wfl {
            put(self.br_mut(d), rd, NO_REMOVAL, op)?;
            return Ok(());
        }

        // Save control points.
        for line in 0..=ohi {
            let rw = self.rw.get(ix(line, op)?).ok_or_else(|| oob(op))?;
            let mut i = first;
            let mut j = last;
            while j - i > 0 {
                qset(&mut self.sq, d, line, i, at(rw, i - off, op)?, op)?;
                qset(&mut self.sq, d, line, j, at(rw, j - off, op)?, op)?;
                i += 1;
                j -= 1;
            }
        }

        // Successful removal -> shift down some entities.
        if sd == 1 {
            let kno = knots(&self.sq, o);
            for j in odeg..(ork - odeg) {
                if at(kno, j, op)? != at(kno, j + 1, op)? {
                    let x = max2(
                        eget(&self.er, d, rd - 1, j, op)?,
                        eget(&self.er, d, rd, j, op)?,
                    );
                    eput(&mut self.er, d, rd - 1, j, x, op)?;
                }
            }
        }

        if sd > 1 {
            let x = at(self.sr(d), rd, op)? - 1;
            put(self.sr_mut(d), rd - 1, x, op)?;
        }

        for i in (rd + 1)..=rk {
            let x = at(self.br(d), i, op)?;
            put(self.br_mut(d), i - 1, x, op)?;
            let x = at(self.sr(d), i, op)?;
            put(self.sr_mut(d), i - 1, x, op)?;
            let kn: &mut [f64] = if d == Direction::U {
                self.sq.u_knots_mut()
            } else {
                self.sq.v_knots_mut()
            };
            let x = at(kn, i, op)?;
            put(kn, i - 1, x, op)?;
            for j in odeg..=ohi {
                let x = eget(&self.er, d, i, j, op)?;
                eput(&mut self.er, d, i - 1, j, x, op)?;
            }
        }

        for line in 0..=ohi {
            for i in (fout + 1)..=hi {
                let c = qget(&self.sq, d, line, i, op)?;
                qset(&mut self.sq, d, line, i - 1, c, op)?;
            }
        }

        let mut x = self.sq.indices();
        if d == Direction::U {
            x.n -= 1;
            x.r -= 1;
        } else {
            x.m -= 1;
            x.s -= 1;
        }
        self.sq.set_size_indices(x)?;
        let hi = hi - 1;

        // If no more internal knots -> finished (upstream leaves the switch).
        if self.dir == d {
            if hi == deg {
                return Ok(());
            }
        } else if self.dir == Direction::UV && x.n == x.p && x.m == x.q {
            return Ok(());
        }

        // Update error bounds.
        let k = (rd - deg).max(deg + 1);
        let l = hi.min(rd + deg - sd);
        for i in k..=l {
            let kn = knots(&self.sq, d);
            if at(kn, i, op)? != at(kn, i + 1, op)? && at(self.br(d), i, op)? != NO_REMOVAL {
                let mr = remove_one_knot(
                    &self.sq,
                    ix(i, op)?,
                    ix(at(self.sr(d), i, op)?, op)?,
                    0,
                    ix(ohi, op)?,
                    d,
                    at(self.br(d), i, op)?,
                )?;
                put(self.br_mut(d), i, mr, op)?;
                self.error = None;
            }
        }

        if self.dir == Direction::UV {
            for j in (odeg + 1)..(ork - odeg) {
                let kno = knots(&self.sq, o);
                if at(kno, j, op)? != at(kno, j + 1, op)? && at(self.br(o), j, op)? != NO_REMOVAL {
                    let mr = remove_one_knot(
                        &self.sq,
                        ix(j, op)?,
                        ix(at(self.sr(o), j, op)?, op)?,
                        ix(first, op)?,
                        ix(last, op)?,
                        o,
                        at(self.br(o), j, op)?,
                    )?;
                    put(self.br_mut(o), j, mr, op)?;
                    self.error = None;
                }
            }
        }
        Ok(())
    }
}

/// Makes the surfaces `sur[0..=k]` compatible in u, v or both
/// (`N_MakeSrfsCompatible`): rationality is unified, the parameter ranges in the
/// requested direction(s) are mapped to `[0, 1]` when they differ (the other
/// direction then takes the bounds of `sur[0]`), degrees are raised to the highest
/// one and the knot vectors are merged by knot insertion. Knots of different
/// surfaces within the knot-merge tolerance are moved to their average first.
///
/// Errors: `InvalidInput` for an empty slice or invalid surfaces; errors of the
/// degree elevation, the knot merge and the knot insertion (the surfaces are left as
/// far as upstream got).
pub fn make_compatible(sur: &mut [Surface], dir: Direction) -> Result<()> {
    const OP: &str = "nurbs::srf_tool::p3::make_compatible";
    if sur.is_empty() {
        return Err(SolidError::InvalidInput {
            operation: OP,
            reason: "no surfaces",
        });
    }

    // Make surface definitions compatible.
    make_rat_compatible(sur);

    // Scale surface knot vectors if necessary.
    let b0 = sur.first().ok_or_else(|| oob(OP))?.parameter_bounds();
    let (us, ue, vs, ve) = (b0.ul, b0.ur, b0.vb, b0.vt);
    let (mut iu, mut iv) = (false, false);
    for s in sur.iter().skip(1) {
        let b = s.parameter_bounds();
        if us != b.ul || ue != b.ur {
            iu = true;
        }
        if vs != b.vb || ve != b.vt {
            iv = true;
        }
    }
    if iu && dir == Direction::V {
        iu = false;
    }
    if iv && dir == Direction::U {
        iv = false;
    }

    match (iu, iv) {
        (true, false) => {
            for s in sur.iter_mut() {
                s.reparam(0.0, 1.0, vs, ve)?;
            }
        }
        (false, true) => {
            for s in sur.iter_mut() {
                s.reparam(us, ue, 0.0, 1.0)?;
            }
        }
        (true, true) => {
            for s in sur.iter_mut() {
                s.reparam_to_interval(Rectangle::new(0.0, 1.0, 0.0, 1.0), Direction::UV)?;
            }
        }
        (false, false) => {}
    }

    // Get highest degrees.
    let (mut ph, mut qh) = (0, 0);
    for (i, s) in sur.iter().enumerate() {
        let (p, q) = s.degrees();
        if i == 0 || p > ph {
            ph = p;
        }
        if i == 0 || q > qh {
            qh = q;
        }
    }

    // Elevate the degrees.
    for s in sur.iter_mut() {
        let (p, q) = s.degrees();
        let tu = ph - p;
        let tv = qh - q;
        if tu > 0 {
            *s = elevate_degree(s, tu, Direction::U)?;
        }
        if tv > 0 {
            *s = elevate_degree(s, tv, Direction::V)?;
        }
    }

    // Merge knot vectors (the merge moves nearly equal knots in place).
    let kux = if dir == Direction::V {
        None
    } else {
        Some(merge_knots(sur, Direction::U)?)
    };
    let kvx = if dir == Direction::U {
        None
    } else {
        Some(merge_knots(sur, Direction::V)?)
    };

    // Refine surfaces.
    for (i, s) in sur.iter_mut().enumerate() {
        if let Some(x) = kux
            .as_ref()
            .and_then(|k| k.get(i))
            .filter(|x| !x.is_empty())
        {
            *s = insert_knots(s, x, Direction::U)?;
        }
        if let Some(x) = kvx
            .as_ref()
            .and_then(|k| k.get(i))
            .filter(|x| !x.is_empty())
        {
            *s = insert_knots(s, x, Direction::V)?;
        }
    }
    Ok(())
}

/// `N_GetCompatibleKnotArray` over the knot vectors of `sur` in `d`, writing the
/// knots it moves back into the surfaces (upstream passes the surfaces' own knot
/// vectors). Returns the knots to insert into each surface.
fn merge_knots(sur: &mut [Surface], d: Direction) -> Result<Vec<Vec<f64>>> {
    let mut kn: Vec<Vec<f64>> = sur.iter().map(|s| knots(s, d).to_vec()).collect();
    let res = compatible_knot_array(&mut kn);
    for (s, k) in sur.iter_mut().zip(&kn) {
        let dst = if d == Direction::U {
            s.u_knots_mut()
        } else {
            s.v_knots_mut()
        };
        for (a, b) in dst.iter_mut().zip(k) {
            *a = *b;
        }
    }
    res
}

/// Joins `sur1` and `sur2` along `dir` (`N_SrfJoin`): the end of `sur1` in `dir`
/// meets the start of `sur2` (whose knots are shifted to continue `sur1`'s); the
/// boundary rows are averaged into one, the lower degree is elevated first, and the
/// joint knot is then removed as often as `tol` allows. Any `dir` other than
/// [`Direction::U`] joins in v.
///
/// Errors: [`NlibCode::Input`] when the net sizes or degrees across `dir` differ;
/// errors of the degree elevation; `InvalidInput` for invalid surfaces.
pub fn join(sur1: &Surface, sur2: &Surface, dir: Direction, tol: f64) -> Result<Surface> {
    const OP: &str = "nurbs::srf_tool::p3::join";
    let rat1 = sur1.is_rational();
    let rat2 = sur2.is_rational();

    // Work in the v direction; the u direction transposes the inputs and the result.
    let tr = |s: &Surface| {
        let mut t = s.clone();
        t.swap_uv();
        t
    };
    let (a, b) = if dir == Direction::U {
        (tr(sur1), tr(sur2))
    } else {
        (sur1.clone(), sur2.clone())
    };

    // Check for errors, compute indexes, and raise degree if required.
    let (n1, n2) = (a.n(), b.n());
    let (p1, p2) = (a.p(), b.p());
    if n1 != n2 || p1 != p2 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let (a, b) = match a.q().cmp(&b.q()) {
        core::cmp::Ordering::Equal => (a, b),
        core::cmp::Ordering::Less => {
            let t = b.q() - a.q();
            (elevate_degree(&a, t, Direction::V)?, b)
        }
        core::cmp::Ordering::Greater => {
            let t = a.q() - b.q();
            let e = elevate_degree(&b, t, Direction::V)?;
            (a, e)
        }
    };
    let Local {
        n: n1,
        m: m1,
        p: pj,
        q: qj,
        r: r1,
        s: s1,
        pw: pw1,
        u: u1,
        v: v1,
    } = Local::of(&a, OP)?;
    let Local {
        m: m2,
        s: s2,
        pw: pw2,
        v: v2,
        ..
    } = Local::of(&b, OP)?;
    let mj = m1 + m2;
    let sj = mj + qj + 1;
    let nj = n1;
    let rj = r1;

    // Load knots and control points.
    let uj: Vec<f64> = u1.get(..ix(rj + 1, OP)?).ok_or_else(|| oob(OP))?.to_vec();
    let mut vj: Vec<f64> = zeros1(sj, OP)?;
    let mut ii: isize = 0;
    while ii < s1 {
        put(&mut vj, ii, at(v1, ii, OP)?, OP)?;
        ii += 1;
    }
    let dd = at(v1, s1, OP)? - at(v2, 0, OP)?;
    for (jj, ii) in (ii..).zip((qj + 1)..=s2) {
        put(&mut vj, jj, at(v2, ii, OP)? + dd, OP)?;
    }

    let mut pwj: Vec<Vec<CPoint>> = zeros2(nj, mj, OP)?;
    for ii in 0..=nj {
        for jj in 0..m1 {
            put2(&mut pwj, ii, jj, at2(&pw1, ii, jj, OP)?, OP)?;
        }
        let a1 = at2(&pw1, ii, m1, OP)?;
        let b0 = at2(&pw2, ii, 0, OP)?;
        let mut c = CPoint::combine2(0.5, a1, 0.5, b0);
        if rat1 != rat2 {
            let w1 = if a1.w == NOW { 1.0 } else { a1.w };
            let w2 = if b0.w == NOW { 1.0 } else { b0.w };
            c.w = 0.5 * (w1 + w2);
        }
        put2(&mut pwj, ii, m1, c, OP)?;
        for (kk, jj) in ((m1 + 1)..).zip(1..=m2) {
            put2(&mut pwj, ii, kk, at2(&pw2, ii, jj, OP)?, OP)?;
        }
    }
    let mut surj = Surface::new(pwj, ix(pj, OP)?, ix(qj, OP)?, uj, vj)?;

    // Attempt knot removal (its error is ignored upstream).
    let t = at(v1, s1, OP)?;
    match remove_knot_conditional_with_status(&surj, t, ix(qj, OP)?, tol, Direction::V) {
        Ok((rm, _)) => surj = rm.surface,
        Err(SolidError::Nlib { .. }) => {}
        Err(e) => return Err(e),
    }

    // Make sure weights/rationality is consistently set.
    if rat1 != rat2 {
        for i in 0..=surj.n() {
            for j in 0..=surj.m() {
                let c = surj.cpt_mut(i, j)?;
                if c.w == NOW {
                    c.w = 1.0;
                }
            }
        }
    }

    if dir == Direction::U {
        surj.swap_uv();
    }
    Ok(surj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::knots::Side;

    fn sample() -> Surface {
        let net: Vec<Vec<CPoint>> = (0..5)
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
    fn remove_all_undoes_insertion() {
        let s = sample();
        let a = insert_knots(&s, &[0.2, 0.7, 0.7], Direction::U).unwrap();
        let a = insert_knots(&a, &[0.25, 0.8], Direction::V).unwrap();
        let rm = remove_all_knots(&a, 1e-9, Direction::UV).unwrap();
        assert_eq!(rm.n(), s.n());
        assert_eq!(rm.m(), s.m());
        assert_eq!(rm.u_knots(), s.u_knots());
        assert_eq!(rm.v_knots(), s.v_knots());
        assert!(max_dist(&s, &rm) < 1e-9);

        let ru = remove_all_knots(&a, 1e-9, Direction::U).unwrap();
        assert_eq!(ru.n(), s.n());
        assert_eq!(ru.m(), a.m());
        let rv = remove_all_knots(&a, 1e-9, Direction::V).unwrap();
        assert_eq!(rv.n(), a.n());
        assert_eq!(rv.m(), s.m());
        // Nothing removable at a zero tolerance on the original.
        assert_eq!(remove_all_knots(&s, 0.0, Direction::UV).unwrap(), s);
    }

    #[test]
    fn split_then_join_round_trips() {
        let s = sample();
        let (a, b) = super::super::p1::split(&s, 0.4, Direction::U).unwrap();
        let j = join(&a, &b, Direction::U, 1e-9).unwrap();
        assert_eq!(j.n(), s.n());
        assert!(max_dist(&s, &j) < 1e-9);
        let (a, b) = super::super::p1::split(&s, 0.3, Direction::V).unwrap();
        let j = join(&a, &b, Direction::V, 1e-9).unwrap();
        assert!(max_dist(&s, &j) < 1e-9);
        assert!(matches!(
            join(&s, &a, Direction::U, 1e-9),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn compatible_surfaces_share_knots() {
        let s = sample();
        let mut t = elevate_degree(&s, 1, Direction::V).unwrap();
        t = insert_knots(&t, &[0.6], Direction::U).unwrap();
        let mut v = vec![s.clone(), t];
        make_compatible(&mut v, Direction::UV).unwrap();
        assert_eq!(v[0].u_knots(), v[1].u_knots());
        assert_eq!(v[0].v_knots(), v[1].v_knots());
        assert_eq!(v[0].degrees(), v[1].degrees());
        assert!(max_dist(&s, &v[0]) < 1e-9);
        assert!(make_compatible(&mut [], Direction::U).is_err());
    }

    #[test]
    fn absurd_degree_is_an_error() {
        let s = sample();
        let (u, v) = (s.u_knots().to_vec(), s.v_knots().to_vec());
        let bad = Surface::new(s.cpts(), i64::MAX as usize, s.q(), u, v).unwrap();
        assert!(remove_all_knots(&bad, 0.1, Direction::UV).is_err());
    }

    #[test]
    fn stuck_removal_loop_is_not_converged() {
        // fuzz (nlib.srf_tool_p3 fuzz00246): a control point at 1e300 makes the u
        // error bounds NaN; with nothing removable in v every `NL_UVDIR` step goes to
        // v and changes nothing, so upstream loops forever.
        use crate::nurbs::math::CPoint;
        let net = vec![
            vec![
                CPoint::new(
                    1626.3087426842285,
                    -9.743918576531186,
                    5.643171203164258,
                    1.0,
                ),
                CPoint::new(
                    519.5078505845089,
                    -2.9675111313088722,
                    1.7078684560266675,
                    0.36724555114660634,
                ),
                CPoint::new(
                    0.16358720963267612,
                    0.21218302875865636,
                    -0.1386939392132162,
                    0.07033813493236826,
                ),
                CPoint::new(
                    -2.9596848534411486,
                    6.906520213597406,
                    3.9556725479410804,
                    0.8874920132658637,
                ),
                CPoint::new(
                    -6.458485261542945,
                    5.280748590770612,
                    0.1518901176891312,
                    1.0,
                ),
            ],
            vec![
                CPoint::new(
                    -1.5787813219128228,
                    0.7418135932976195,
                    -9.03132939503351,
                    1.0,
                ),
                CPoint::new(
                    -6.229192732531399,
                    1016.7293044374496,
                    -2.884348915780943,
                    1.0,
                ),
                CPoint::new(
                    -8.411305618125482,
                    1493.461188913565,
                    -8.168913180903867e-07,
                    1.0,
                ),
                CPoint::new(
                    -1.422549489599622e-06,
                    -2.1715420326530586,
                    -14.026370188646485,
                    1.5699444326596041,
                ),
                CPoint::new(
                    9.361202292046882e-07,
                    9.882733024988777,
                    -3.6075633563374776,
                    1.0,
                ),
            ],
            vec![
                CPoint::new(
                    2.9196125327924736,
                    -4.289018443510048,
                    8.210002077396046,
                    0.9431015856528775,
                ),
                CPoint::new(
                    -1.256655965527212,
                    -4.6044238256936065,
                    0.005063449909195938,
                    0.9818282734558954,
                ),
                CPoint::new(
                    -3.216286737780081,
                    -4.752421516194052,
                    -3.8449405010672955,
                    1.0,
                ),
                CPoint::new(
                    -1.4301272519182891e-06,
                    11.539207454443742,
                    -4.472860455363832,
                    1.4889698651863237,
                ),
                CPoint::new(
                    3.678236824741495,
                    2.9872992417753297,
                    5.681212789521694,
                    0.7351019376987773,
                ),
            ],
            vec![
                CPoint::new(
                    0.0,
                    -5.47642869895013,
                    -15.524988749452309,
                    1.6831404287925364,
                ),
                CPoint::new(
                    -1.9002930294812972,
                    -1.7490147291009897,
                    -4.958237472739259,
                    1.632269172959596,
                ),
                CPoint::new(-2.791967666389214, 0.0, 0.0, 1.6083988498724937),
                CPoint::new(-2.62985801022589, 2.306046547439225, 6.327673951208524, 1.0),
                CPoint::new(
                    5399.490649237752,
                    4.311559664997214,
                    2.3594855955162117,
                    1.0,
                ),
            ],
            vec![
                CPoint::new(
                    0.09516099892339171,
                    0.0,
                    -0.010259986886403916,
                    0.01968707308526039,
                ),
                CPoint::new(
                    0.030391701947088776,
                    -3.188545969307479,
                    -1293.9973169913883,
                    0.44467229738037856,
                ),
                CPoint::new(
                    0.0,
                    -4.684707627187429,
                    -1901.1749581854826,
                    0.6440881451920273,
                ),
                CPoint::new(
                    1e+300,
                    1.921649850073933,
                    -13.850288600357576,
                    1.9056967771560864,
                ),
                CPoint::new(
                    0.0,
                    0.649768547588242,
                    12.305716587561323,
                    1.5177859821925366,
                ),
            ],
        ];
        let s = Surface::new(
            net,
            1,
            1,
            vec![
                -0.42006782694164,
                -0.42006782694164,
                0.377647184343457,
                1.175362195628554,
                1.973077206913651,
                2.770792218198748,
                2.770792218198748,
            ],
            vec![
                67.2383433427168,
                67.2383433427168,
                121.29078320883525,
                146.65381351199017,
                161.5377674219393,
                172.7821035234011,
                172.7821035234011,
            ],
        )
        .unwrap();
        assert!(matches!(
            remove_all_knots(&s, 0.0, Direction::UV),
            Err(SolidError::NotConverged { .. })
        ));
    }
}
