//! Surface extension by a distance (the recursive core of `N_SrfExtendByDist`) and
//! the first and second derivatives of the unit surface normal.
//!
//! Upstream: `source/SMLib/src/NL_SrfAdv.cpp`, `source/SMLib/inc/NL_SrfAdv.h`.
//!
//! | Rust                                         | Upstream                 |
//! |----------------------------------------------|--------------------------|
//! | [`extend_by_dist_at_depth`]                  | `ST_surxsdr`             |
//! | [`SrfContinuity`]                            | `NL_G1`, `NL_G2`, `NL_CMAX`, `NL_G1R` |
//! | [`MAX_EXTEND_DEPTH`]                         | recursion limit of `ST_surxsdr` |
//! | [`extend_by_dist_in_place`]                  | `ST_surxsdr` in place (`surP == surQ`) |
//! | [`KNOT_TOL`] (defined in `p3`)               | `KN_TOL` (file-private)  |
//! | [`eval_pt_normal_deriv`], [`NormalDerivs`]   | `N_SrfEvalPtNormalDeriv` |
//!
//! Deviations:
//! - Out-params become return values: [`extend_by_dist_at_depth`] returns the
//!   extended surface instead of filling `surQ`. Upstream's in-place mode
//!   (`surP == surQ`, used by its own second recursive step and by every upstream
//!   caller) builds the result into fresh arrays from a copy of the input, so a caller
//!   replaces its surface with the returned one; [`extend_by_dist_in_place`] does that
//!   for the in-place callers. When the closing `N_SrfRemoveKnotConditional` fails (it
//!   returns the error of its last failed basis-maximum search after finishing the
//!   removal), upstream returns the error with the extended surface already in `surQ`,
//!   so its callers retry from that surface; [`extend_by_dist_in_place`] leaves the
//!   same surface in its argument (also when the second recursive step fails before
//!   writing, which leaves the first step's surface). Other errors after
//!   `N_AllocSrfArrays` gave `surQ` new arrays (a `Cmax` weight error, a failed knot
//!   insertion or solve) leave them partly written, and upstream's retry reads the
//!   unwritten entries (UB-107); the port leaves the input unchanged then. The
//!   `NL_STACKS` arguments have no counterpart. [`eval_pt_normal_deriv`] returns the
//!   five derivatives in a struct instead of `ND[k][l]`.
//! - Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number
//!   instead of a flag plus the global `NL_ERROR` record. Past [`MAX_EXTEND_DEPTH`]
//!   recursion levels upstream returns its bare error flag, leaving in `NL_ERROR`
//!   whatever an earlier recovered error (an ignored iso-curve extension, a failed
//!   reflection-plane normalisation, ...) or a callee's `N_ErrClear` left there; the
//!   port returns [`SolidError::NotConverged`].
//! - The direction flag: `ST_surxsdr` treats every value other than `NL_UDIR` as
//!   `NL_VDIR`, so [`Direction::UV`] extends in v, as upstream. The `NL_START` /
//!   `NL_END` and continuity flags are the enums [`CurveEnd`] and [`SrfContinuity`].
//! - The u- and v-direction branches of `ST_surxsdr` are mirror images; the port runs
//!   one implementation over the net viewed as rows of the non-extending direction
//!   (`P[other][ext]`, which is upstream's own layout of its scratch net `Rw`). Only
//!   copies differ; every arithmetic operation and its order are upstream's, and the
//!   called routines get the real direction.
//! - Upstream's dead branch guarded by the never-set local `static int cbi = 0`
//!   (`N_SrfExtendByParamDist` instead of the extension below) and its debug printing
//!   (`siDebug`) are not ported.
//! - `ST_surxsdr` ignores errors of the sampled `N_CrvExtendByDist` calls (only
//!   successful ones are averaged); the port does the same, except that an
//!   [`SolidError::Unported`] is passed on.
//! - Every array access is bounds checked; where upstream would index outside its
//!   arrays (invalid knot vectors, a degree-1 reflection whose plane needs the third
//!   control point beyond the scratch row) the function returns
//!   [`SolidError::InvalidInput`].

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::bezier::PASCAL_TRI;
use crate::nurbs::crv_adv::p1::{Continuity, CurveEnd};
use crate::nurbs::crv_geom::ToleranceKind;
use crate::nurbs::geom::{cpt_reflect, normalize, pt_along_vector, Plane};
use crate::nurbs::index::{get, oob, set, storage, sx, ux};
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{float_op_is_bad, CPoint, FloatOp, MatrixType, RealMatrix, MODEL_TOL};
use crate::nurbs::srf_adv::p3::KNOT_TOL;
use crate::nurbs::srf_basic::{Direction, Surface, WEIGHT_MAX, WEIGHT_MIN};
use crate::nurbs::srf_tool::p1::{extract_iso_crv, insert_knot};
use crate::nurbs::srf_tool::p2::remove_knot_conditional_with_status;

/// Recursion limit of [`extend_by_dist_at_depth`] (upstream: `depth > 10` fails).
pub const MAX_EXTEND_DEPTH: usize = 10;

const OP_XSDR: &str = "nurbs::srf_adv::p2::extend_by_dist_at_depth";
const OP_NDER: &str = "nurbs::srf_adv::p2::eval_pt_normal_deriv";

/// Continuity of a surface extension at the old boundary (`NL_G1 = 2`, `NL_G2 = 3`,
/// `NL_CMAX = 4`, `NL_G1R = 6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SrfContinuity {
    /// `NL_G1`: tangent plane continuity (by reflection, Shetty & White).
    G1,
    /// `NL_G2`: curvature continuity (by reflection, Shetty & White).
    G2,
    /// `NL_CMAX`: the polynomial continuation of the boundary span (no knot left at
    /// the old boundary when possible).
    Cmax,
    /// `NL_G1R`: a ruled extension joining with G1 continuity.
    G1R,
}

impl SrfContinuity {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            SrfContinuity::G1 => 2,
            SrfContinuity::G2 => 3,
            SrfContinuity::Cmax => 4,
            SrfContinuity::G1R => 6,
        }
    }

    /// The continuity for an upstream flag value, `None` for any other value.
    #[must_use]
    pub const fn from_flag(flag: i32) -> Option<Self> {
        match flag {
            2 => Some(SrfContinuity::G1),
            3 => Some(SrfContinuity::G2),
            4 => Some(SrfContinuity::Cmax),
            6 => Some(SrfContinuity::G1R),
            _ => None,
        }
    }
}

/// Derivatives of the unit normal `N(u, v)` (upstream `ND[k][l]`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NormalDerivs {
    /// `N_u` (`ND[1][0]`).
    pub nu: Vec3,
    /// `N_v` (`ND[0][1]`).
    pub nv: Vec3,
    /// `N_uu` (`ND[2][0]`).
    pub nuu: Vec3,
    /// `N_uv` (`ND[1][1]`).
    pub nuv: Vec3,
    /// `N_vv` (`ND[0][2]`).
    pub nvv: Vec3,
}

/// A control net viewed as `net[other][ext]`.
type Net = Vec<Vec<CPoint>>;

/// `w[i][j]`.
fn g2(w: &[Vec<CPoint>], i: isize, j: isize, op: &'static str) -> Result<CPoint> {
    let row = usize::try_from(i)
        .ok()
        .and_then(|i| w.get(i))
        .ok_or_else(|| oob(op))?;
    get(row, j, op)
}

/// `NL_PascalTri[i][j]` as a real.
fn pascal(i: isize, j: isize, op: &'static str) -> Result<f64> {
    let row = usize::try_from(i)
        .ok()
        .and_then(|i| PASCAL_TRI.get(i))
        .ok_or_else(|| oob(op))?;
    Ok(get(row, j, op)? as f64)
}

/// `w[j][i]` from `w[i][j]`.
fn transpose(w: &[Vec<CPoint>]) -> Net {
    let cols = w.first().map_or(0, Vec::len);
    (0..cols)
        .map(|j| w.iter().filter_map(|row| row.get(j).copied()).collect())
        .collect()
}

/// The net of `sur` as `net[other][ext]`.
fn ext_net(sur: &Surface, udir: bool) -> Net {
    let pw = sur.cpts();
    if udir {
        transpose(&pw)
    } else {
        pw
    }
}

/// `(highest ext net index, ext degree, highest ext knot index, ext knots)`.
fn ext_dims(sur: &Surface, udir: bool) -> (usize, usize, usize, &[f64]) {
    let idx = sur.indices();
    if udir {
        (idx.n, idx.p, idx.r, sur.u_knots())
    } else {
        (idx.m, idx.q, idx.s, sur.v_knots())
    }
}

/// Surface from `net[other][ext]`, the ext knots and the other knots of `like`.
fn build(like: &Surface, udir: bool, q: Net, ext_knots: Vec<f64>) -> Result<Surface> {
    let (p, qd) = like.degrees();
    if udir {
        Surface::new(transpose(&q), p, qd, ext_knots, like.v_knots().to_vec())
    } else {
        Surface::new(q, p, qd, like.u_knots().to_vec(), ext_knots)
    }
}

/// Extends `sur` across one boundary by about the distance `dist` (the recursive
/// core `ST_surxsdr` of `N_SrfExtendByDist`; `depth` is the recursion level, `0` for
/// a top-level call). The parameter domain grows with the surface: the old domain
/// still maps to the old surface.
///
/// The extension parameter is the average end parameter of five iso-curves (at
/// `0.07, 0.19, 0.45, 0.73, 0.92` of the other direction) extended by `dist` with
/// `N_CrvExtendByDist`; a reflection extension (`G1`, `G2`) longer than `0.9313`
/// times the parameter range is done in two recursive steps. Then:
/// - [`SrfContinuity::G1R`]: a ruled extension of degree-elevated linear segments,
///   after which one copy of the old boundary knot is removed if possible;
/// - [`SrfContinuity::G1`] / [`SrfContinuity::G2`]: the boundary rows are mirrored
///   in the plane through the boundary control point normal to the last leg (Shetty &
///   White), with the second (and, for rational surfaces, the first) mirrored row
///   recomputed for G2 / G1;
/// - [`SrfContinuity::Cmax`]: the boundary span continued as a polynomial;
///
/// and finally the inserted knots and the old boundary knot are removed where the
/// surface allows (`N_SrfRemoveKnotConditional`).
///
/// `dir` selects the u ([`Direction::U`]) or v direction (anything else), `end` the
/// boundary.
///
/// # Errors
///
/// [`SolidError::NotConverged`] past [`MAX_EXTEND_DEPTH`] levels; [`NlibCode::Numerical`]
/// when no sampled iso-curve could be extended; [`NlibCode::Weight`] for a `Cmax`
/// extension of a rational surface producing weights outside
/// `[WEIGHT_MIN, WEIGHT_MAX]`; the errors of the iso-curve extraction, the span
/// search, knot insertion and removal and the least-squares solve.
pub fn extend_by_dist_at_depth(
    sur: &Surface,
    dist: f64,
    dir: Direction,
    end: CurveEnd,
    cont: SrfContinuity,
    depth: usize,
) -> Result<Surface> {
    xsdr(sur, dist, dir, end, cont, depth, &mut InPlace::Unwritten)
}

/// [`extend_by_dist_at_depth`] at depth `0` in upstream's in-place mode (`surP ==
/// surQ`, how every upstream caller extends): on success `esur` becomes the extended
/// surface. On a failing knot removal at the end of the extension (or of its second
/// recursive step), upstream has already written the extended surface, as that
/// removal left it, into `surQ`; `esur` becomes that surface and the error is
/// returned. On the other errors `esur` is unchanged.
///
/// # Errors
///
/// Those of [`extend_by_dist_at_depth`].
pub fn extend_by_dist_in_place(
    esur: &mut Surface,
    dist: f64,
    dir: Direction,
    end: CurveEnd,
    cont: SrfContinuity,
) -> Result<()> {
    let mut state = InPlace::Unwritten;
    match xsdr(esur, dist, dir, end, cont, 0, &mut state) {
        Ok(s) => {
            *esur = s;
            Ok(())
        }
        Err(e) => {
            if let InPlace::Written(s) = state {
                *esur = s;
            }
            Err(e)
        }
    }
}

/// What upstream's in-place `ST_surxsdr` (`surP == surQ`) leaves in `surQ` when it
/// fails.
enum InPlace {
    /// It failed before giving `surQ` new arrays: `surQ` is the input.
    Unwritten,
    /// It failed after `N_AllocSrfArrays` gave `surQ` new arrays and before they were
    /// complete: upstream's retry then reads unwritten entries (UB-107); the port
    /// leaves the input unchanged.
    HalfWritten,
    /// It failed with `surQ` complete: this surface.
    Written(Surface),
}

/// `N_SrfRemoveKnotConditional(surQ, .., surQ)` inside `ST_surxsdr`: the surface after
/// the removal, or its error. Upstream finishes the removal before it returns the error
/// of its last failed basis-maximum search, so that surface is what `surQ` holds then:
/// it goes to `state`.
fn remove_conditional(
    s: &Surface,
    t: f64,
    nt: usize,
    tol: f64,
    dir: Direction,
    state: &mut InPlace,
) -> Result<Surface> {
    match remove_knot_conditional_with_status(s, t, nt, tol, dir)? {
        (removal, None) => Ok(removal.surface),
        (removal, Some(e)) => {
            *state = InPlace::Written(removal.surface);
            Err(e)
        }
    }
}

/// `ST_surxsdr` (see [`extend_by_dist_at_depth`]); on an error, `state` tells what
/// upstream's in-place mode leaves in `surQ` (see [`extend_by_dist_in_place`]).
fn xsdr(
    sur: &Surface,
    dist: f64,
    dir: Direction,
    end: CurveEnd,
    cont: SrfContinuity,
    depth: usize,
    state: &mut InPlace,
) -> Result<Surface> {
    const OP: &str = OP_XSDR;

    // Limit recursion to 10 levels.
    if depth > MAX_EXTEND_DEPTH {
        return Err(SolidError::NotConverged {
            operation: OP,
            iterations: u32::try_from(depth).unwrap_or(u32::MAX),
        });
    }

    let udir = dir == Direction::U;
    let ext_dir = if udir { Direction::U } else { Direction::V };
    let start = end == CurveEnd::Start;

    // Get local notation (`P[other][ext]`).
    let pw = ext_net(sur, udir);
    let (ne, pe, re, ke) = ext_dims(sur, udir);
    let (ne, pe, re) = (sx(ne, OP)?, sx(pe, OP)?, sx(re, OP)?);
    let ko = if udir { sur.v_knots() } else { sur.u_knots() };
    let ro = sx(ko.len(), OP)? - 1;
    let no = sx(pw.len(), OP)? - 1;

    // Determine the u or v value that corresponds roughly to dist: average the
    // results of curve extensions.
    let uv = [0.07, 0.19, 0.45, 0.73, 0.92];
    let lcont = match cont {
        SrfContinuity::Cmax => Continuity::Cmax,
        SrfContinuity::G1R => Continuity::G1,
        _ => Continuity::G2,
    };

    let ke0 = get(ke, 0, OP)?;
    let ker = get(ke, re, OP)?;
    let ko0 = get(ko, 0, OP)?;
    let kor = get(ko, ro, OP)?;

    let mut kk: isize = 0;
    let mut ext = 0.0;
    for &t in &uv {
        let tt = (1.0 - t) * ko0 + t * kor;
        let cur_a = extract_iso_crv(sur, tt, ext_dir)?;
        match cur_a.extend_by_dist(dist, ToleranceKind::Absolute, end, lcont) {
            Ok(cur_b) => {
                kk += 1;
                let (d1, d2) = cur_b.param_bounds();
                if start {
                    ext += d1;
                } else {
                    ext += d2;
                }
            }
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            Err(_) => {}
        }
    }
    if kk == 0 {
        return Err(SolidError::nlib(OP, NlibCode::Numerical));
    }
    ext /= kk as f64;

    // Recurse when the distance is too big.
    if matches!(cont, SrfContinuity::G1 | SrfContinuity::G2)
        && ((start && ke0 - ext > 0.9313 * (ker - ke0))
            || (!start && ext - ker > 0.9313 * (ker - ke0)))
    {
        let d2 = if start {
            (dist * (ker - ke0)) / (2.0 * (ke0 - ext))
        } else {
            (dist * (ker - ke0)) / (2.0 * (ext - ker))
        };
        let q = xsdr(sur, dist - d2, dir, end, cont, depth + 1, state)?;
        // the second step extends surQ in place: until it gives surQ new arrays, surQ
        // holds the first step's surface
        *state = InPlace::Unwritten;
        let r = xsdr(&q, d2, dir, end, cont, depth + 1, state);
        if r.is_err() && matches!(state, InPlace::Unwritten) {
            *state = InPlace::Written(q);
        }
        return r;
    }

    // Set the reflection parameter from the extension parameter.
    let mut refl = if start {
        2.0 * ke0 - ext
    } else {
        2.0 * ker - ext
    };

    // Find the closest knot to the reflection parameter and snap to it if close enough.
    let mut d1 = f64::MAX;
    let mut jj = pe + 1;
    for ii in pe + 1..re - pe {
        let kv = get(ke, ii, OP)?;
        if (refl - kv).abs() < d1 {
            d1 = (refl - kv).abs();
            jj = ii;
        }
    }
    if d1 < KNOT_TOL * (ker - ke0) {
        refl = get(ke, jj, OP)?;
    }

    // Sizes of the extended surface in the extending direction.
    let mut span: isize = 0;
    let mut mult: isize = 0;
    let nqe: isize;
    let mut kk: isize = 0;
    if start {
        match cont {
            SrfContinuity::Cmax => {
                mult = 1;
                while pe + mult < re {
                    if get(ke, pe + mult + 1, OP)? == get(ke, pe + 1, OP)? {
                        mult += 1;
                    } else {
                        break;
                    }
                }
                if mult > pe {
                    mult = pe;
                }
                nqe = ne + 2 * pe - mult;
                kk = 2 * pe;
            }
            SrfContinuity::G1R => nqe = ne + pe,
            _ => {
                let (sp, ml) = KnotVector::from_slice(ke)?.find_span_and_mult(
                    ux(pe, OP)?,
                    refl,
                    Side::Right,
                )?;
                span = sx(sp, OP)?;
                mult = sx(ml, OP)?;
                nqe = ne + span + pe - mult;
                kk = 2 * span;
            }
        }
    } else {
        match cont {
            SrfContinuity::Cmax => {
                mult = 1;
                while re - pe - mult > 0 {
                    if get(ke, re - pe - mult - 1, OP)? == get(ke, re - pe - 1, OP)? {
                        mult += 1;
                    } else {
                        break;
                    }
                }
                if mult > pe {
                    mult = pe;
                }
                nqe = ne + 2 * pe - mult;
                kk = 2 * pe;
            }
            SrfContinuity::G1R => nqe = ne + pe,
            _ => {
                let (sp, ml) = KnotVector::from_slice(ke)?.find_span_and_mult(
                    ux(pe, OP)?,
                    refl,
                    Side::Left,
                )?;
                span = sx(sp, OP)?;
                mult = sx(ml, OP)?;
                nqe = ne + re - span - 1 + pe - mult;
                kk = 2 * (re - span - 1);
            }
        }
    }
    let rqe = nqe + pe + 1;

    // Output net `Q[other][ext]` and ext knots (upstream's in-place mode gives surQ
    // new arrays here: an error from now on leaves them partly written).
    *state = InPlace::HalfWritten;
    let row: Vec<CPoint> = storage(nqe, OP)?;
    let mut qw: Net = vec![row; ux(no + 1, OP)?];
    let mut uq: Vec<f64> = storage(rqe, OP)?;

    if cont == SrfContinuity::G1R {
        let d2 = pe as f64;
        let end_prm;
        if start {
            // Recompute the extension parameter in case we snapped.
            let ext = 2.0 * ke0 - refl;
            for ii in 0..=pe {
                set(&mut uq, ii, ext, OP)?;
            }
            for ii in 1..=re {
                set(&mut uq, ii + pe, get(ke, ii, OP)?, OP)?;
            }
            for (qrow, prow) in qw.iter_mut().zip(&pw) {
                for ii in 0..=ne {
                    set(qrow, ii + pe, get(prow, ii, OP)?, OP)?;
                }
            }

            // New boundary control points.
            let du = -d2 * ((ke0 - ext) / (get(ke, pe + 1, OP)? - ke0));
            end_prm = ke0;
            for (qrow, prow) in qw.iter_mut().zip(&pw) {
                let p0 = get(prow, 0, OP)?;
                let p1 = get(prow, 1, OP)?;
                let a = p0.to_euclid();
                let b = p1.to_euclid();
                let v = b - a;
                let c = pt_along_vector(a, du, v);
                set(qrow, 0, CPoint::weighted(c, p1.w), OP)?;
            }

            // Degree elevation.
            for (qrow, prow) in qw.iter_mut().zip(&pw) {
                for ii in 1..pe {
                    let d1 = ii as f64 / d2;
                    let c = CPoint::combine2(1.0 - d1, get(qrow, 0, OP)?, d1, get(prow, 0, OP)?);
                    set(qrow, ii, c, OP)?;
                }
            }
        } else {
            let ext = 2.0 * ker - refl;
            for ii in 0..=pe {
                set(&mut uq, ii + re, ext, OP)?;
            }
            for ii in 0..re {
                set(&mut uq, ii, get(ke, ii, OP)?, OP)?;
            }
            for (qrow, prow) in qw.iter_mut().zip(&pw) {
                for ii in 0..=ne {
                    set(qrow, ii, get(prow, ii, OP)?, OP)?;
                }
            }

            let du = d2 * ((ext - ker) / (ker - get(ke, re - pe - 1, OP)?));
            end_prm = ker;
            for (qrow, prow) in qw.iter_mut().zip(&pw) {
                let pa = get(prow, ne - 1, OP)?;
                let pb = get(prow, ne, OP)?;
                let a = pa.to_euclid();
                let b = pb.to_euclid();
                let v = b - a;
                let c = pt_along_vector(b, du, v);
                set(qrow, nqe, CPoint::weighted(c, pa.w), OP)?;
            }

            for qrow in &mut qw {
                for ii in 1..pe {
                    let d1 = ii as f64 / d2;
                    let c = CPoint::combine2(1.0 - d1, get(qrow, ne, OP)?, d1, get(qrow, nqe, OP)?);
                    set(qrow, ne + ii, c, OP)?;
                }
            }
        }

        // Try to remove one occurrence of the old end knot.
        let s = build(sur, udir, qw, uq)?;
        return remove_conditional(&s, end_prm, 1, MODEL_TOL, ext_dir, state);
    }

    // Reflection or last-interval extension (G1, G2, Cmax): insert the knot into a
    // copy, load the knots, partially load Q and load Rw.
    let rrow: Vec<CPoint> = storage(kk, OP)?;
    let mut rw: Net = vec![rrow; ux(no + 1, OP)?];
    let mut rem: isize = 0;

    let uuu = if cont != SrfContinuity::Cmax {
        refl
    } else if start {
        get(ke, pe + 1, OP)?
    } else {
        get(ke, re - pe - 1, OP)?
    };

    // Make the new knot full multiplicity.
    let mut sur_b = sur.clone();
    if mult < pe {
        rem = pe - mult;
        sur_b = insert_knot(&sur_b, uuu, ux(rem, OP)?, ext_dir)?;
    }

    let bw = ext_net(&sur_b, udir);
    let (nbe, _, rbe, kb) = ext_dims(&sur_b, udir);
    let (nbe, rbe) = (sx(nbe, OP)?, sx(rbe, OP)?);
    let kby: isize;
    let kkt: isize;

    if start {
        // Number of new control points of the extension.
        kby = nqe - nbe;
        for ((qrow, rrow), brow) in qw.iter_mut().zip(rw.iter_mut()).zip(&bw) {
            for jj in 0..=nbe {
                set(qrow, kby + jj, get(brow, jj, OP)?, OP)?;
            }
            for jj in 0..=kby {
                set(rrow, jj + kby, get(brow, jj, OP)?, OP)?;
            }
        }

        if cont == SrfContinuity::Cmax {
            kkt = pe + 1;
            let kb0 = get(kb, 0, OP)?;
            for ii in 0..=pe {
                set(&mut uq, ii, 2.0 * kb0 - refl, OP)?;
            }
            for ii in 1..=rbe {
                set(&mut uq, pe + ii, get(kb, ii, OP)?, OP)?;
            }
        } else {
            kkt = span + 1;
            let mut jj = kkt - 1;
            for ii in 1..=rbe {
                set(&mut uq, span + ii, get(kb, ii, OP)?, OP)?;
            }
            for ii in pe + 1..=span {
                let x = get(&uq, jj + 1, OP)? - (get(kb, ii, OP)? - get(kb, ii - 1, OP)?);
                set(&mut uq, jj, x, OP)?;
                jj -= 1;
            }
            let d1 = get(&uq, jj + 1, OP)? - (get(kb, kkt, OP)? - get(kb, span, OP)?);
            for ii in 0..=pe {
                set(&mut uq, ii, d1, OP)?;
            }
        }
    } else {
        kby = nqe - nbe;
        let kk = nbe - kby;
        for ((qrow, rrow), brow) in qw.iter_mut().zip(rw.iter_mut()).zip(&bw) {
            for jj in 0..=nbe {
                set(qrow, jj, get(brow, jj, OP)?, OP)?;
            }
            for jj in 0..=kby {
                set(rrow, jj, get(brow, jj + kk, OP)?, OP)?;
            }
        }

        kkt = rbe - 1;
        for ii in 0..rbe {
            set(&mut uq, ii, get(kb, ii, OP)?, OP)?;
        }

        if cont == SrfContinuity::Cmax {
            let kbr = get(kb, rbe, OP)?;
            for ii in 0..=pe {
                set(&mut uq, rbe + ii, 2.0 * kbr - refl, OP)?;
            }
        } else {
            let mut jj = rbe;
            let mut ii = re - pe - 1;
            while ii > span {
                let x = get(&uq, jj - 1, OP)? + (get(ke, ii + 1, OP)? - get(ke, ii, OP)?);
                set(&mut uq, jj, x, OP)?;
                jj += 1;
                ii -= 1;
            }
            let d1 = get(&uq, jj - 1, OP)? + (get(ke, span + 1, OP)? - refl);
            for ii in 0..=pe {
                set(&mut uq, jj + ii, d1, OP)?;
            }
        }
    }

    // Compute the new control points of the extension: Shetty & White for G1 / G2,
    // derivative expressions for Cmax.
    let rat = sur.is_rational();
    let nn = no;
    let pq = pe;
    let pqf = pq as f64;

    let kb_at = |i: isize| get(kb, i, OP);

    if cont == SrfContinuity::Cmax {
        let mut dw: Vec<CPoint> = storage(pq, OP)?;
        if start {
            let d2 = refl - kb_at(0)?;
            let d3 = kb_at(pe + 1)? - kb_at(0)?;
            let d4 = 1.0 / d3;
            for row in &mut rw {
                // Derivatives from the right.
                let mut dr = pqf * d4;
                for jj in 1..=pq {
                    let mut d = get(row, kby + jj, OP)?;
                    let mut kp: isize = if jj % 2 == 0 { 1 } else { -1 };
                    for kk in 0..jj {
                        d.blend(kp as f64 * pascal(jj, kk, OP)?, get(row, kby + kk, OP)?);
                        kp *= -1;
                    }
                    d = d.scale(dr);
                    set(&mut dw, jj, d, OP)?;
                    dr = dr * (pqf - jj as f64) * d4;
                }

                // New control points.
                let mut dl = d2 / pqf;
                for jj in 1..=pq {
                    let mut kp: isize = if jj % 2 == 0 { -1 } else { 1 };
                    let mut x = get(&dw, jj, OP)?.scale(-kp as f64 * dl);
                    if jj < pq {
                        dl = dl * d2 / (pqf - jj as f64);
                    }
                    for kk in 0..jj {
                        x.blend(kp as f64 * pascal(jj, kk, OP)?, get(row, pq - kk, OP)?);
                        kp *= -1;
                    }
                    set(row, pq - jj, x, OP)?;
                }

                // Don't create unacceptable weights.
                if rat {
                    for jj in 0..pq {
                        let w = get(row, jj, OP)?.w;
                        if w < WEIGHT_MIN || w > WEIGHT_MAX {
                            return Err(SolidError::nlib(OP, NlibCode::Weight));
                        }
                    }
                }
            }
        } else {
            let d2 = kb_at(rbe)? - refl;
            let d3 = kb_at(rbe)? - kb_at(rbe - pe - 1)?;
            let d4 = 1.0 / d3;
            for row in &mut rw {
                // Derivatives from the left.
                let mut dl = -pqf * d4;
                for jj in 1..=pq {
                    let mut d = get(row, kby - jj, OP)?;
                    let mut kp: isize = if jj % 2 == 0 { 1 } else { -1 };
                    for kk in 0..jj {
                        d.blend(kp as f64 * pascal(jj, kk, OP)?, get(row, kby - kk, OP)?);
                        kp *= -1;
                    }
                    d = d.scale(dl);
                    set(&mut dw, jj, d, OP)?;
                    dl = -dl * (pqf - jj as f64) * d4;
                }

                let mut dr = d2 / pqf;
                for jj in 1..=pq {
                    let mut kp: isize = if jj % 2 == 0 { -1 } else { 1 };
                    let mut x = get(&dw, jj, OP)?.scale(dr);
                    if jj < pq {
                        dr = dr * d2 / (pqf - jj as f64);
                    }
                    for kk in 0..jj {
                        x.blend(kp as f64 * pascal(jj, kk, OP)?, get(row, kby + kk, OP)?);
                        kp *= -1;
                    }
                    set(row, kby + jj, x, OP)?;
                }

                if rat {
                    for jj in 1..=pq {
                        let w = get(row, kby + jj, OP)?.w;
                        if w < WEIGHT_MIN || w > WEIGHT_MAX {
                            return Err(SolidError::nlib(OP, NlibCode::Weight));
                        }
                    }
                }
            }
        }
    } else {
        let mut pln = Plane::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        // Neighbour offsets of the mirroring: `+1` from the start, `-1` from the end.
        let sg: isize = if start { 1 } else { -1 };
        for ii in 0..=nn {
            // Reflection plane in Euclidean space.
            let p1 = g2(&rw, ii, kby, OP)?.to_euclid();
            let p2 = g2(&rw, ii, kby + sg, OP)?.to_euclid();
            let mut nv = normalize(p1 - p2);
            if nv.is_err() {
                let p2 = g2(&rw, ii, kby + 2 * sg, OP)?.to_euclid();
                nv = normalize(p1 - p2);
            }
            // With no reflection plane, just copy.
            let ok = if let Ok((v, _)) = nv {
                pln = Plane::new(p1, v);
                true
            } else {
                false
            };

            let row = rw.get_mut(ux(ii, OP)?).ok_or_else(|| oob(OP))?;
            if start {
                let mut kk = kby - 1;
                for jj in 1..=kby {
                    let src = get(row, kby + jj, OP)?;
                    let x = if ok { cpt_reflect(src, pln)? } else { src };
                    set(row, kk, x, OP)?;
                    kk -= 1;
                }
            } else {
                let mut kk = 2 * kby;
                for jj in 0..kby {
                    let src = get(row, jj, OP)?;
                    let x = if ok { cpt_reflect(src, pln)? } else { src };
                    set(row, kk, x, OP)?;
                    kk -= 1;
                }
            }
        }

        // Non-rational G2: recompute the second row with the C2 condition.
        if !rat && cont == SrfContinuity::G2 && pq > 1 {
            if start {
                let d1 = 2.0 + (kb_at(pe + 2)? - kb_at(pe + 1)?) / (kb_at(pe + 1)? - kb_at(0)?);
                for row in &mut rw {
                    let x = CPoint::translate_sum2(
                        get(row, kby + 2, OP)?,
                        d1,
                        get(row, kby - 1, OP)?,
                        -d1,
                        get(row, kby + 1, OP)?,
                    );
                    set(row, kby - 2, x, OP)?;
                }
            } else {
                let d1 = 2.0
                    + (kb_at(rbe - pe - 1)? - kb_at(rbe - pe - 2)?)
                        / (kb_at(rbe)? - kb_at(rbe - pe - 1)?);
                for row in &mut rw {
                    let x = CPoint::translate_sum2(
                        get(row, kby - 2, OP)?,
                        d1,
                        get(row, kby + 1, OP)?,
                        -d1,
                        get(row, kby - 1, OP)?,
                    );
                    set(row, kby + 2, x, OP)?;
                }
            }
        }

        // Rational: recompute the first one or two reflected rows for G1 / G2.
        if rat {
            let mut d1 = if start {
                2.0 * pqf / (kb_at(pe + 1)? - kb_at(0)?)
            } else {
                2.0 * pqf / (kb_at(rbe)? - kb_at(rbe - pe - 1)?)
            };
            let mut tal = -1.0e+20;
            let mut ktal: isize = 0;
            for ii in 0..=nn {
                let w0 = g2(&rw, ii, kby, OP)?.w;
                let w1 = g2(&rw, ii, kby + sg, OP)?.w;
                let d2 = d1 * (w1 - w0) / w0;
                if d2 > tal {
                    tal = d2;
                    ktal = ii;
                }
            }

            let g2c = cont == SrfContinuity::G2 && pq > 1;
            let mut d2 = 0.0;
            let mut d4 = 0.0;
            let mut sol = [0.0; 2];
            if g2c {
                // Compute theta and e0 (Shetty & White).
                let (d3, d4b) = if start {
                    (kb_at(pe + 1)? - kb_at(0)?, kb_at(pe + 2)? - kb_at(pe + 1)?)
                } else {
                    (
                        kb_at(rbe)? - kb_at(rbe - pe - 1)?,
                        kb_at(rbe - pe - 1)? - kb_at(rbe - pe - 2)?,
                    )
                };
                d1 = pqf / d3;
                d2 = (pqf * (pqf - 1.0)) / (d3 * (d3 + d4b));
                d4 = (2.0 * d3 + d4b) / d3;
                let d3 = -2.0 * tal * d1;

                let mut hh = [[0.0; 4]; 5];
                for (ii, h) in (0..).zip(hh.iter_mut()) {
                    *h = g2(&rw, ktal, kby - 2 + ii, OP)?.to_array();
                }
                let mut rma = RealMatrix::new(3, 1, MatrixType::Full, 3)?;
                let mut rhs = [0.0; 4];
                for (ii, r) in rhs.iter_mut().enumerate() {
                    let h = |k: usize| hh.get(k).and_then(|x| x.get(ii)).copied().unwrap_or(0.0);
                    rma.set(ii, 0, d1 * (h(2) - h(3)))?;
                    rma.set(ii, 1, h(2))?;
                    *r = d2 * (d4 * (h(3) - h(1)) + (h(0) - h(4))) + d3 * (h(2) - h(3));
                }
                let s = rma.lst_sq_solve(&rhs)?;
                sol = [get(&s, 0, OP)?, get(&s, 1, OP)?];
            }

            let dq = if start {
                get(&uq, kkt, OP)? - get(&uq, kkt - 1, OP)?
            } else {
                get(&uq, kkt + 1, OP)? - get(&uq, kkt, OP)?
            };
            d1 = 2.0 + tal * dq / pqf;

            // Recompute the first row for G1. The coefficients do not add up to one:
            // in homogeneous space this only changes the weight of the first point,
            // to that of the second.
            for row in &mut rw {
                let x = CPoint::combine2(d1, get(row, kby, OP)?, -1.0, get(row, kby + sg, OP)?);
                set(row, kby - sg, x, OP)?;
            }

            if g2c {
                // Recompute the second row for G2.
                d2 = 1.0 / d2;
                d1 *= sol[0] + 2.0 * tal;
                for row in &mut rw {
                    let mut hh = [[0.0; 4]; 5];
                    if start {
                        for jj in 1..=4 {
                            let h = hh.get_mut(ux(jj, OP)?).ok_or_else(|| oob(OP))?;
                            *h = get(row, kby - 2 + jj, OP)?.to_array();
                        }
                    } else {
                        for jj in 0..=3 {
                            let h = hh.get_mut(ux(jj, OP)?).ok_or_else(|| oob(OP))?;
                            *h = get(row, kby - 2 + jj, OP)?.to_array();
                        }
                    }
                    let mut out = [0.0; 4];
                    for (jj, o) in out.iter_mut().enumerate() {
                        let h =
                            |k: usize| hh.get(k).and_then(|x| x.get(jj)).copied().unwrap_or(0.0);
                        *o = if start {
                            h(4) + d4 * (h(1) - h(3)) + d2 * (d1 * (h(2) - h(3)) + sol[1] * h(2))
                        } else {
                            h(0) - d4 * (h(1) - h(3)) + d2 * (d1 * (h(2) - h(3)) + sol[1] * h(2))
                        };
                    }
                    let [x, y, z, w] = out;
                    set(row, kby - 2 * sg, CPoint::new(x, y, z, w), OP)?;
                }
            }
        }
    }

    // Load the new control points from Rw into Q.
    for (qrow, rrow) in qw.iter_mut().zip(&rw) {
        if start {
            for jj in 0..kby {
                set(qrow, jj, get(rrow, jj, OP)?, OP)?;
            }
        } else {
            for jj in 1..=kby {
                set(qrow, nbe + jj, get(rrow, kby + jj, OP)?, OP)?;
            }
        }
    }

    // Remove unnecessary knots: first the inserted ones (needed only temporarily),
    // then those at the old boundary.
    let mut s = build(sur, udir, qw, uq)?;
    if rem > 0 {
        s = remove_conditional(&s, uuu, ux(rem, OP)?, 100.0 * MODEL_TOL, ext_dir, state)?;
    }
    let old_end = if start { ke0 } else { ker };
    let rem = match cont {
        SrfContinuity::G1 => 1,
        SrfContinuity::G2 => 2,
        _ => ux(pe, OP)?,
    };
    remove_conditional(&s, old_end, rem, MODEL_TOL, ext_dir, state)
}

/// First and second derivatives of the unit normal `N = (S_u x S_v) / |S_u x S_v|`
/// at `(u, v)` (`N_SrfEvalPtNormalDeriv`), from the surface derivatives up to order
/// three. `ufl` / `vfl` select the side of a knot (for discontinuous surfaces).
///
/// # Errors
///
/// [`NlibCode::Numerical`] when `1 / |S_u x S_v|` is out of range (degenerate
/// point); the errors of [`Surface::derivs`].
pub fn eval_pt_normal_deriv(
    sur: &Surface,
    u: f64,
    v: f64,
    ufl: Side,
    vfl: Side,
) -> Result<NormalDerivs> {
    const OP: &str = OP_NDER;

    // Get surface derivatives.
    let sd = sur.derivs(u, v, ufl, vfl, true, 3, 3)?;
    let d = |k: usize, l: usize| -> Result<Vec3> {
        sd.get(k)
            .and_then(|row| row.get(l))
            .copied()
            .ok_or_else(|| oob(OP))
    };

    // Derivatives of the cross product S_u x S_v.
    let s = d(1, 0)?.cross(d(0, 1)?);

    let a = d(2, 0)?.cross(d(0, 1)?);
    let b = d(1, 0)?.cross(d(1, 1)?);
    let su_v = a + b;

    let a = d(1, 1)?.cross(d(0, 1)?);
    let b = d(1, 0)?.cross(d(0, 2)?);
    let sv_v = a + b;

    let a = d(3, 0)?.cross(d(0, 1)?);
    let b = d(2, 0)?.cross(d(1, 1)?) * 2.0;
    let c = d(1, 0)?.cross(d(2, 1)?);
    let suu_v = (a + b) + c;

    let a = d(2, 1)?.cross(d(0, 1)?);
    let b = d(2, 0)?.cross(d(0, 2)?);
    let c = d(1, 0)?.cross(d(1, 2)?);
    let suv_v = (a + b) + c;

    let a = d(1, 2)?.cross(d(0, 1)?);
    let b = d(1, 1)?.cross(d(0, 2)?) * 2.0;
    let c = d(1, 0)?.cross(d(0, 3)?);
    let svv_v = (a + b) + c;

    // Derivatives of |S_u x S_v|.
    let sm = s.dot(s).sqrt();
    if float_op_is_bad(1.0, sm, FloatOp::Division) {
        return Err(SolidError::nlib(OP, NlibCode::Numerical));
    }
    let os = 1.0 / sm;
    let mut mlt = os;

    let su = mlt * s.dot(su_v);
    let sv = mlt * s.dot(sv_v);
    let suu = mlt * (s.dot(suu_v) + su_v.dot(su_v) - su * su);
    let suv = mlt * (s.dot(suv_v) + su_v.dot(sv_v) - su * sv);
    let svv = mlt * (s.dot(svv_v) + sv_v.dot(sv_v) - sv * sv);

    // Derivatives of the unit normal.
    mlt = os * os;
    let nu = (su_v * sm - s * su) * mlt;
    let nv = (sv_v * sm - s * sv) * mlt;

    let ss = sm * sm;
    mlt *= os;

    let fac_b = 2.0 * sm * su;
    let fac_c = 2.0 * su * su - sm * suu;
    let nuu = ((suu_v * ss - su_v * fac_b) + s * fac_c) * mlt;

    let fac_b = sm * sv;
    let fac_c = sm * su;
    let fac_d = 2.0 * su * sv - sm * suv;
    let nuv = (((suv_v * ss - su_v * fac_b) - sv_v * fac_c) + s * fac_d) * mlt;

    let fac_b = 2.0 * sm * sv;
    let fac_c = 2.0 * sv * sv - sm * svv;
    let nvv = ((svv_v * ss - sv_v * fac_b) + s * fac_c) * mlt;

    Ok(NormalDerivs {
        nu,
        nv,
        nuu,
        nuv,
        nvv,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;

    fn plane_patch(p: usize, q: usize) -> Surface {
        let n = p + 2;
        let m = q + 1;
        let net = (0..=n)
            .map(|i| {
                (0..=m)
                    .map(|j| CPoint::new(i as f64, j as f64, 0.1 * (i * j) as f64, NOW))
                    .collect()
            })
            .collect();
        let knots = |deg: usize, hi: usize| {
            let mut k = vec![0.0; deg + 1];
            for i in 1..=(hi - deg) {
                k.push(i as f64 / (hi - deg + 1) as f64);
            }
            k.extend(std::iter::repeat_n(1.0, deg + 1));
            k
        };
        Surface::new(net, p, q, knots(p, n), knots(q, m)).unwrap()
    }

    #[test]
    fn continuity_flags_round_trip() {
        for c in [
            SrfContinuity::G1,
            SrfContinuity::G2,
            SrfContinuity::Cmax,
            SrfContinuity::G1R,
        ] {
            assert_eq!(SrfContinuity::from_flag(c.flag()), Some(c));
        }
        assert_eq!(SrfContinuity::from_flag(5), None);
    }

    #[test]
    fn extension_grows_the_domain() {
        let s = plane_patch(3, 2);
        for cont in [
            SrfContinuity::G1,
            SrfContinuity::G2,
            SrfContinuity::Cmax,
            SrfContinuity::G1R,
        ] {
            for dir in [Direction::U, Direction::V] {
                for end in [CurveEnd::Start, CurveEnd::End] {
                    let q = extend_by_dist_at_depth(&s, 0.5, dir, end, cont, 0).unwrap();
                    let (k0, k1) = if dir == Direction::U {
                        (s.u_knots(), q.u_knots())
                    } else {
                        (s.v_knots(), q.v_knots())
                    };
                    if end == CurveEnd::Start {
                        assert!(k1[0] < k0[0], "{cont:?} {dir:?}");
                    } else {
                        assert!(k1.last() > k0.last(), "{cont:?} {dir:?}");
                    }
                    // The old corner stays on the surface.
                    let (a, b) = (s.u_knots()[0], s.v_knots()[0]);
                    let p0 = s.eval(a, b, Side::Left, Side::Left).unwrap();
                    let p1 = q.eval(a, b, Side::Left, Side::Left).unwrap();
                    assert!((p0 - p1).length() < 1e-6, "{cont:?} {dir:?} {end:?}");
                }
            }
        }
    }

    #[test]
    fn depth_limit() {
        let s = plane_patch(2, 2);
        let e = extend_by_dist_at_depth(
            &s,
            0.5,
            Direction::U,
            CurveEnd::Start,
            SrfContinuity::G1,
            MAX_EXTEND_DEPTH + 1,
        );
        assert!(matches!(e, Err(SolidError::NotConverged { .. })));
    }

    #[test]
    fn normal_derivs_of_a_plane_vanish() {
        let net = (0..=1)
            .map(|i| {
                (0..=1)
                    .map(|j| CPoint::new(f64::from(i), f64::from(j), 0.0, NOW))
                    .collect()
            })
            .collect();
        let s = Surface::new(
            net,
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        let nd = eval_pt_normal_deriv(&s, 0.3, 0.6, Side::Left, Side::Left).unwrap();
        for v in [nd.nu, nd.nv, nd.nuu, nd.nuv, nd.nvv] {
            assert_eq!(v.length(), 0.0);
        }
    }

    #[test]
    fn degenerate_normal_is_an_error() {
        let net = (0..=1)
            .map(|_| {
                (0..=1)
                    .map(|j| CPoint::new(0.0, f64::from(j), 0.0, NOW))
                    .collect()
            })
            .collect();
        let s = Surface::new(
            net,
            1,
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        let e = eval_pt_normal_deriv(&s, 0.3, 0.6, Side::Left, Side::Left);
        assert!(matches!(
            e,
            Err(SolidError::Nlib {
                code: NlibCode::Numerical,
                ..
            })
        ));
    }
}
