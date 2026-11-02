//! Skinned surface through cross-sectional curves with cross-boundary continuity
//! constraints and optional rail curves.
//!
//! Upstream: `source/SMLib/src/NL_SrfAdv.cpp`, `source/SMLib/inc/NL_SrfAdv.h`.
//!
//! | Rust                                      | Upstream                             |
//! |-------------------------------------------|--------------------------------------|
//! | [`create_skin_srf_boundary_continuity`]   | `N_CreateSkinSrfBoundaryContinuity`  |
//! | [`SkinBoundaryOptions`]                   | its `der cmp cnt bnd rail deg ES dir ts kns` arguments |
//! | [`BoundaryContinuity`]                    | `NL_C1`, `NL_G1`                     |
//!
//! Deviations:
//! - Out-params become return values: the skinned surface is returned instead of
//!   filling `sur`; the `NL_STACKS` arguments have no counterpart. The inputs upstream
//!   modifies in place (cross sections made 3D, pruned or made rational, rescaled,
//!   translated, made compatible and degree elevated; derivative surfaces adjusted to
//!   the boundary they constrain, pruned, made rational and translated; rails made 3D,
//!   pruned, made rational and translated) are `&mut` and modified the same way.
//! - The optional arguments are the fields of [`SkinBoundaryOptions`]: `der` and
//!   `rail` are `Option<[Option<_>; 2]>` (upstream's `NULL` array and `NULL`
//!   entries), `ts` and `kns` are `Option<Vec<f64>>` (`kns` holds all knots of the
//!   skinning direction).
//! - Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number. When
//!   the surface's weights are invalid upstream returns `NL_WEI_ERR` with the surface
//!   built; the Rust function returns only the error.
//! - With derivative surfaces and [`Direction::UV`] upstream uses an uninitialised
//!   derivative curve; the Rust function returns `InvalidInput` at that point.
//!   Without derivative surfaces `NL_UVDIR` skins in u, as upstream.
//! - Upstream builds the derivative curves of the rational case with rails over the
//!   derivative surfaces' own knot vectors (`N_CrvFromCPolygonAndKnotVector` shares
//!   the array), so the in-place knot writes of `N_CrvsMakeCompatibleKnotTol`
//!   (reparametrisation, knot merging) and of `N_CrvFuncMultiplyCrv4d` (rescaling)
//!   reach the surfaces until the curve gets new storage. The Rust function
//!   reproduces these writes on `der` (the shared array's contents are tracked by
//!   `make_compatible_knot_tol_traced`).
//! - A derivative strip whose adjustment fails after its in-place u/v swap (the
//!   extension of a strip that is not Bezier in the swapped direction) is left
//!   swapped, as upstream's in-place `N_AdjustDerivSrf`; `SmBSplineSurface::
//!   CreateSkinnedSurface` retries on it.
//! - Every array access is bounds checked; where upstream would index outside its
//!   arrays (cross sections or derivative strips that are not compatible although
//!   `cmp` says so, rails that are not compatible with each other, too few `ts`
//!   values) the function returns [`SolidError::InvalidInput`].

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_adv::insert_knots;
use crate::nurbs::crv_basic::{cross_bound_deriv_crv_non_rat_srf, Boundary, Curve};
use crate::nurbs::crv_fit::p1::{
    derivs_matrix, interp_knot_vector, knot_vector_deriv, knot_vector_end_derivs, DataDir,
    DerivEnd, ParamMethod,
};
use crate::nurbs::crv_fit::p2::{calc_matrix, cpts_with_matrix, deriv_matrix, MatrixEnds};
use crate::nurbs::crv_tool::p2::make_compatible_knot_tol_traced;
use crate::nurbs::funcs_basic::CurveFunction;
use crate::nurbs::geom::{cpts_are_colinear, dist_cpolygon, dist_pt_pt, ColinearDim};
use crate::nurbs::index;
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{CPoint, MODEL_TOL};
use crate::nurbs::srf_basic::{Direction, Surface, WEIGHT_TOL};
use crate::nurbs::srf_fit::interp_params;
use crate::nurbs::srf_tool::p4::remove_knots_constrained;

use super::p4::{adjust_deriv_srf, adjust_weight_scale, merge_knot_vectors};

const OP: &str = "nurbs::srf_adv::p5::create_skin_srf_boundary_continuity";

/// Upstream `dper`: relative agreement of an end magnitude with the derivative data
/// below which the derivative curve is rescaled.
const DPER: f64 = 0.92;
/// Upstream `lper`: relative agreement of the two end magnitudes from which the
/// derivative curve is rescaled by a constant.
const LPER: f64 = 0.88;

/// Continuity across a constrained end cross section (upstream `cnt[i]`: `NL_C1 = 1`,
/// `NL_G1 = 2`; upstream only tests for `NL_C1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BoundaryContinuity {
    /// `NL_C1`: the derivative surface gives the cross-boundary derivative itself.
    C1,
    /// `NL_G1`: only the direction counts; magnitudes are adjusted to the skin.
    #[default]
    G1,
}

impl BoundaryContinuity {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            BoundaryContinuity::C1 => 1,
            BoundaryContinuity::G1 => 2,
        }
    }

    /// The continuity for an upstream flag value: `NL_C1` is [`Self::C1`], every other
    /// value behaves as [`Self::G1`] (upstream compares with `NL_C1` only).
    #[must_use]
    pub const fn from_flag(flag: i32) -> Self {
        if flag == 1 {
            BoundaryContinuity::C1
        } else {
            BoundaryContinuity::G1
        }
    }
}

/// The optional arguments of [`create_skin_srf_boundary_continuity`].
#[derive(Debug, Clone, PartialEq)]
pub struct SkinBoundaryOptions {
    /// Homogeneous derivative surfaces (upstream `der`): `[0]` across the first cross
    /// section, `[1]` across the last; `None` for no constraint. Modified in place.
    pub der: Option<[Option<Surface>; 2]>,
    /// The cross sections (and derivative data) are already compatible (upstream
    /// `cmp == NL_YES`); otherwise they are made compatible in place.
    pub cmp: bool,
    /// Continuity for `der[0]` and `der[1]` (upstream `cnt`).
    pub cnt: [BoundaryContinuity; 2],
    /// The boundary each derivative surface was built for (upstream `bnd`).
    pub bnd: [Boundary; 2],
    /// Rail curves (upstream `rail`): `[0]` for the start of the cross sections, `[1]`
    /// for their end. Both present must be compatible. Modified in place.
    pub rail: Option<[Option<Curve>; 2]>,
    /// Degree in the skinning direction (ignored when a rail is given: the rail's).
    pub deg: usize,
    /// Skinning tolerance (upstream `ES`): `> 0` removes knots of the skinning
    /// direction within this tolerance.
    pub es: f64,
    /// [`Direction::U`]: the cross sections are v-curves (skinning in u);
    /// [`Direction::V`]: they are u-curves.
    pub dir: Direction,
    /// Skinning parameters of the cross sections (where they meet the rails); required
    /// with rails, computed (chord length) when `None` otherwise.
    pub ts: Option<Vec<f64>>,
    /// Knot vector of the skinning direction (upstream `kns`); computed when `None`.
    pub kns: Option<Vec<f64>>,
}

impl Default for SkinBoundaryOptions {
    fn default() -> Self {
        Self {
            der: None,
            cmp: false,
            cnt: [BoundaryContinuity::G1; 2],
            bnd: [Boundary::Right, Boundary::Left],
            rail: None,
            deg: 3,
            es: 0.0,
            dir: Direction::U,
            ts: None,
            kns: None,
        }
    }
}

/// Which ends carry a constraint (upstream `rfl` / `dfl`: `NL_NO`, `NL_LEFT`,
/// `NL_RIGHT`, `NL_BOTH`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ends {
    No,
    Left,
    Right,
    Both,
}

impl Ends {
    fn of(a: bool, b: bool) -> Self {
        match (a, b) {
            (true, false) => Ends::Left,
            (false, true) => Ends::Right,
            (true, true) => Ends::Both,
            (false, false) => Ends::No,
        }
    }
}

type Net = Vec<Vec<CPoint>>;

fn oob() -> SolidError {
    index::oob(OP)
}

fn bad_input() -> SolidError {
    SolidError::nlib(OP, NlibCode::Input)
}

/// `v[i]`.
fn at<T: Copy>(v: &[T], i: usize) -> Result<T> {
    v.get(i).copied().ok_or_else(oob)
}

/// `v[i] = x`.
fn put<T>(v: &mut [T], i: usize, x: T) -> Result<()> {
    *v.get_mut(i).ok_or_else(oob)? = x;
    Ok(())
}

/// `w[i][j]`.
fn at2<T: Copy>(w: &[Vec<T>], i: usize, j: usize) -> Result<T> {
    at(w.get(i).ok_or_else(oob)?, j)
}

/// `w[i][j] = x`.
fn put2<T>(w: &mut [Vec<T>], i: usize, j: usize, x: T) -> Result<()> {
    put(w.get_mut(i).ok_or_else(oob)?, j, x)
}

/// `a - 1`.
fn dec1(a: usize) -> Result<usize> {
    a.checked_sub(1).ok_or_else(oob)
}

/// A zero-filled `(n+1) x (m+1)` net.
fn net(n: usize, m: usize) -> Net {
    vec![vec![CPoint::default(); m.saturating_add(1)]; n.saturating_add(1)]
}

fn crv(cur: &[Curve], i: usize) -> Result<&Curve> {
    cur.get(i).ok_or_else(oob)
}

/// `(n, m, Pw)` of a surface (`N_SrfGetCPts`).
fn srf_cpts(s: &Surface) -> (usize, usize, Net) {
    (s.n(), s.m(), s.cpts())
}

/// Upstream's weight adjustment of a derivative row or column (`N_AdjustWeightScale`
/// of `pw, qw, rw`, then `to[x] = (1 - f) * from[x] + f * to[x]` and the rescaling
/// that gives `to[pick]` the new weight).
fn adjust_line(
    pw: CPoint,
    qw: CPoint,
    rw: CPoint,
    from: &[CPoint],
    to: &mut [CPoint],
    pick: usize,
) -> Result<()> {
    let (wa, fact) = adjust_weight_scale(pw, qw, rw)?;
    for (t, f) in to.iter_mut().zip(from) {
        *t = CPoint::combine2(1.0 - fact, *f, fact, *t);
    }
    let wb = at(to, pick)?.w;
    let fact = wa / wb;
    for t in to.iter_mut() {
        *t = t.scale(fact);
    }
    Ok(())
}

/// Column `j` of a net.
fn column(w: &[Vec<CPoint>], j: usize) -> Result<Vec<CPoint>> {
    w.iter().map(|row| at(row, j)).collect()
}

/// Writes column `j` of a net.
fn set_column(w: &mut [Vec<CPoint>], j: usize, c: &[CPoint]) -> Result<()> {
    for (row, x) in w.iter_mut().zip(c) {
        put(row, j, *x)?;
    }
    Ok(())
}

/// Rows `from` and `to` of a two-row net, adjusted by [`adjust_line`].
fn adjust_rows(
    w: &mut [Vec<CPoint>],
    p: CPoint,
    q: CPoint,
    r: CPoint,
    to: usize,
    pick: usize,
) -> Result<()> {
    let from = w.get(1 - to.min(1)).ok_or_else(oob)?.clone();
    let row = w.get_mut(to).ok_or_else(oob)?;
    adjust_line(p, q, r, &from, row, pick)
}

/// Columns `from` and `to` of a two-column net, adjusted by [`adjust_line`].
fn adjust_cols(
    w: &mut [Vec<CPoint>],
    p: CPoint,
    q: CPoint,
    r: CPoint,
    to: usize,
    pick: usize,
) -> Result<()> {
    let from = column(w, 1 - to.min(1))?;
    let mut col = column(w, to)?;
    adjust_line(p, q, r, &from, &mut col, pick)?;
    set_column(w, to, &col)
}

/// Skinned surface through the cross sections `cur[0..=k]` with optional
/// cross-boundary derivative constraints across `cur[0]` and `cur[k]` and optional
/// rail curves along the two ends of the cross sections (upstream
/// `N_CreateSkinSrfBoundaryContinuity`).
///
/// The derivative surfaces are first re-expressed for the boundary they constrain
/// ([`adjust_deriv_srf`]); rational input makes every input rational, with the cross
/// sections reparametrised to match the rails' weights and the derivative strips
/// adjusted to be G1 with a single rail in homogeneous space. G1 derivative data is
/// rescaled to the chordal lengths along the ends (by a constant or a linear
/// function, degree elevating the cross sections in the latter case). The corresponding
/// control points of the cross sections are interpolated (with the end derivatives)
/// at the skinning parameters, the rails' control points become the boundary rows,
/// knots are removed within `es`, and for [`Direction::V`] the parametrisation is
/// swapped.
///
/// # Errors
///
/// [`NlibCode::Input`] for rails without `ts`, a cross section missing a rail by more
/// than `NL_MTOL`, derivative data not G1 (C1) with the rails, too few cross sections
/// for the degree, or a `kns` of the wrong size; [`NlibCode::Weight`] for weights out of
/// range in the input or the result; errors of the called routines; `InvalidInput`
/// for an empty `cur` and the cases listed in the module notes.
pub fn create_skin_srf_boundary_continuity(
    cur: &mut [Curve],
    opt: &mut SkinBoundaryOptions,
) -> Result<Surface> {
    use Boundary::{Bottom, Left, Right, Top};
    let k = dec1(cur.len())?;
    let dir = opt.dir;
    let cmp = opt.cmp;
    let cnt = opt.cnt;
    let bnd = opt.bnd;
    let deg = opt.deg;
    let es = opt.es;
    let has_der = opt.der.is_some();
    let has_rail = opt.rail.is_some();
    let c1 = |i: usize| cnt.get(i) == Some(&BoundaryContinuity::C1);
    let ts = opt.ts.clone();
    let tsv = |i: usize| -> Result<f64> { at(ts.as_deref().ok_or_else(bad_input)?, i) };

    // Check boundary of derivative surfaces.
    if let Some(der) = opt.der.as_mut() {
        let in_bnd = |b: Boundary| {
            if matches!(b, Bottom | Top) {
                Direction::V
            } else {
                Direction::U
            }
        };
        let targets = match dir {
            Direction::U => Some([Right, Left]),
            Direction::V => Some([Top, Bottom]),
            Direction::UV => None,
        };
        if let Some(targets) = targets {
            for (slot, (b, t)) in der.iter_mut().zip(bnd.iter().zip(targets)) {
                if let Some(d) = slot.as_mut() {
                    if *b != t {
                        match adjust_deriv_srf(d, in_bnd(*b), *b, t) {
                            Ok(adjusted) => *d = adjusted,
                            Err(e) => {
                                // Upstream adjusts in place (`surP == surQ`): where it swaps
                                // u and v before the extension that fails, the strip stays
                                // swapped (a caller retrying the skinning sees it so).
                                if matches!(
                                    (*b, t),
                                    (Left, Top) | (Bottom, Right) | (Right, Bottom) | (Top, Left)
                                ) {
                                    d.swap_uv();
                                }
                                return Err(e);
                            }
                        }
                    }
                }
            }
        }
    }

    // Prune rational input.
    if has_rail && ts.is_none() {
        return Err(bad_input());
    }
    if let Some(der) = opt.der.as_mut() {
        for d in der.iter_mut().flatten() {
            if d.is_rational() {
                d.prune_rat();
            }
        }
    }
    if let Some(rail) = opt.rail.as_mut() {
        for r in rail.iter_mut().flatten() {
            if !r.is_3d() {
                r.to_3d();
            }
            if r.is_rational() {
                r.prune_rat(false);
            }
        }
    }
    for c in cur.iter_mut() {
        if !c.is_3d() {
            c.to_3d();
        }
        if c.is_rational() {
            c.prune_rat(false);
        }
    }

    // Check weights on input.
    let mut rat = false;
    if let Some(der) = opt.der.as_ref() {
        for d in der.iter().flatten() {
            if d.is_rational() {
                d.weights_are_valid()?;
                rat = true;
            }
        }
    }
    if let Some(rail) = opt.rail.as_ref() {
        for r in rail.iter().flatten() {
            if r.is_rational() {
                r.weights_are_valid()?;
                rat = true;
            }
        }
    }
    for c in cur.iter() {
        if c.is_rational() {
            c.weights_are_valid()?;
            rat = true;
        }
    }

    let mut trl = false;
    let mut t = Vec3::ZERO;
    let mut sul: Net = Vec::new();
    let mut sur_r: Net = Vec::new();
    let mut svb: Net = Vec::new();
    let mut svt: Net = Vec::new();

    if rat {
        // Make non-rationals rational.
        if let Some(der) = opt.der.as_mut() {
            for d in der.iter_mut().flatten() {
                if !d.is_rational() {
                    d.non_rat_to_rat();
                }
            }
        }
        if let Some(rail) = opt.rail.as_mut() {
            for r in rail.iter_mut().flatten() {
                if !r.is_rational() {
                    r.non_rat_to_rat();
                }
            }
        }
        for c in cur.iter_mut() {
            if !c.is_rational() {
                c.non_rat_to_rat();
            }
        }

        if let Some(rail) = opt.rail.as_mut() {
            // Check intersections with rails in 4-D.
            for i in 0..=k {
                let mut reset = false;
                let c = crv(cur, i)?;
                let cw = c.cpts();
                let nc = c.n();
                let (ca, mut wa) = at(cw, 0)?.to_euclid_and_w();
                let (cb, mut wb) = at(cw, nc)?.to_euclid_and_w();
                if let Some(r0) = rail.first().and_then(Option::as_ref) {
                    let aw = r0.eval_pt(tsv(i)?, Side::Left)?;
                    let (a, ra) = aw.to_euclid_and_w();
                    let dist = dist_pt_pt(ca, a);
                    if dist > MODEL_TOL {
                        return Err(bad_input());
                    }
                    if (wa - ra).abs() > WEIGHT_TOL {
                        wa = ra;
                        reset = true;
                    }
                }
                if let Some(r1) = rail.get(1).and_then(Option::as_ref) {
                    let bw = r1.eval_pt(tsv(i)?, Side::Left)?;
                    let (b, rb) = bw.to_euclid_and_w();
                    let dist = dist_pt_pt(cb, b);
                    if dist > MODEL_TOL {
                        return Err(bad_input());
                    }
                    if (wb - rb).abs() > WEIGHT_TOL {
                        wb = rb;
                        reset = true;
                    }
                }
                if reset {
                    cur.get_mut(i)
                        .ok_or_else(oob)?
                        .reparam_end_weights(wa, wb)?;
                }
            }

            // Translate if rails' end tangents pass thru the global origin.
            let origin = CPoint::new(0.0, 0.0, 0.0, 1.0);
            for r in rail.iter().flatten() {
                let rw = r.cpts();
                let nr = r.n();
                if cpts_are_colinear(origin, at(rw, 0)?, at(rw, 1)?, ColinearDim::Three) {
                    trl = true;
                }
                if cpts_are_colinear(at(rw, dec1(nr)?)?, at(rw, nr)?, origin, ColinearDim::Three) {
                    trl = true;
                }
            }

            if trl {
                // Seek a translation vector: the average of the skin's corners.
                let c0 = crv(cur, 0)?;
                let cw = c0.cpts();
                let nc = c0.n();
                t = at(cw, 0)?.to_euclid();
                let mut a = at(cw, nc)?.to_euclid();
                t += a;
                let ck = crv(cur, k)?;
                let cw = ck.cpts();
                let nc = ck.n();
                a = at(cw, 0)?.to_euclid();
                t += a;
                a = at(cw, nc)?.to_euclid();
                t += a;
                t *= 0.25;
                let dist = dist_pt_pt(Vec3::ZERO, t);
                if dist <= MODEL_TOL {
                    if let Some(r0) = rail.first().and_then(Option::as_ref) {
                        t = r0.max_pos_vector().0;
                    } else if let Some(r1) = rail.get(1).and_then(Option::as_ref) {
                        t = r1.max_pos_vector().0;
                    }
                }

                for r in rail.iter_mut().flatten() {
                    r.translate(t);
                }
                if let Some(der) = opt.der.as_mut() {
                    for d in der.iter_mut().flatten() {
                        d.translate(t);
                    }
                }
                for c in cur.iter_mut() {
                    c.translate(t);
                }
            }

            // Lift rows or columns of der surface control points.
            if let Some(der) = opt.der.as_ref() {
                let d0 = der.first().and_then(Option::as_ref);
                let d1 = der.get(1).and_then(Option::as_ref);
                if dir == Direction::U {
                    if let Some(d) = d0 {
                        // The last two rows: Du(u = umax, v).
                        let (ns, ms, sw) = srf_cpts(d);
                        sul = net(1, ms);
                        for (l, i) in (dec1(ns)?..=ns).enumerate() {
                            for j in 0..=ms {
                                put2(&mut sul, l, j, at2(&sw, i, j)?)?;
                            }
                        }
                    }
                    if let Some(d) = d1 {
                        // The first two rows: Du(u = umin, v).
                        let (_, ms, sw) = srf_cpts(d);
                        sur_r = net(1, ms);
                        for i in 0..=1 {
                            for j in 0..=ms {
                                put2(&mut sur_r, i, j, at2(&sw, i, j)?)?;
                            }
                        }
                    }
                }
                if dir == Direction::V {
                    if let Some(d) = d0 {
                        // The last two columns: Dv(u, v = vmax).
                        let (ns, ms, sw) = srf_cpts(d);
                        svb = net(ns, 1);
                        for (l, j) in (dec1(ms)?..=ms).enumerate() {
                            for i in 0..=ns {
                                put2(&mut svb, i, l, at2(&sw, i, j)?)?;
                            }
                        }
                    }
                    if let Some(d) = d1 {
                        // The first two columns: Dv(u, v = vmin).
                        let (ns, _, sw) = srf_cpts(d);
                        svt = net(ns, 1);
                        for j in 0..=1 {
                            for i in 0..=ns {
                                put2(&mut svt, i, j, at2(&sw, i, j)?)?;
                            }
                        }
                    }
                }

                // Check G1 continuity of rails and der surface.
                let r0 = rail.first().and_then(Option::as_ref);
                let r1 = rail.get(1).and_then(Option::as_ref);
                let four = ColinearDim::Four;
                let three = ColinearDim::Three;
                if let (Some(r0), Some(r1)) = (r0, r1) {
                    // Two rails: must be G1 in 4-D.
                    let r0w = r0.cpts();
                    let r1w = r1.cpts();
                    let nr = r1.n();
                    let r0l = at(r0w, dec1(nr)?)?;
                    let r1l = at(r1w, dec1(nr)?)?;
                    if dir == Direction::U {
                        if let Some(d) = d0 {
                            let ms = d.m();
                            if !cpts_are_colinear(
                                at2(&sul, 0, 0)?,
                                at2(&sul, 1, 0)?,
                                at(r0w, 1)?,
                                four,
                            ) {
                                return Err(bad_input());
                            }
                            if !cpts_are_colinear(
                                at2(&sul, 0, ms)?,
                                at2(&sul, 1, ms)?,
                                at(r1w, 1)?,
                                four,
                            ) {
                                return Err(bad_input());
                            }
                        }
                        if let Some(d) = d1 {
                            let ms = d.m();
                            if !cpts_are_colinear(r0l, at2(&sur_r, 0, 0)?, at2(&sur_r, 1, 0)?, four)
                            {
                                return Err(bad_input());
                            }
                            if !cpts_are_colinear(
                                r1l,
                                at2(&sur_r, 0, ms)?,
                                at2(&sur_r, 1, ms)?,
                                four,
                            ) {
                                return Err(bad_input());
                            }
                        }
                    }
                    if dir == Direction::V {
                        if let Some(d) = d0 {
                            let ns = d.n();
                            if !cpts_are_colinear(
                                at2(&svb, 0, 0)?,
                                at2(&svb, 0, 1)?,
                                at(r0w, 1)?,
                                four,
                            ) {
                                return Err(bad_input());
                            }
                            if !cpts_are_colinear(
                                at2(&svb, ns, 0)?,
                                at2(&svb, ns, 1)?,
                                at(r1w, 1)?,
                                four,
                            ) {
                                return Err(bad_input());
                            }
                        }
                        if let Some(d) = d1 {
                            let ns = d.n();
                            if !cpts_are_colinear(r0l, at2(&svt, 0, 0)?, at2(&svt, 0, 1)?, four) {
                                return Err(bad_input());
                            }
                            if !cpts_are_colinear(r1l, at2(&svt, ns, 0)?, at2(&svt, ns, 1)?, four) {
                                return Err(bad_input());
                            }
                        }
                    }
                } else if let Some(r) = r0.or(r1) {
                    // One rail only: if not G1 in 4-D, adjust the second row of der.
                    let first = r0.is_some();
                    let rw = r.cpts();
                    let nr = r.n();
                    let r_start = at(rw, 1)?;
                    let r_end = at(rw, dec1(nr)?)?;
                    if dir == Direction::U {
                        if let Some(d) = d0.filter(|_| !c1(0)) {
                            let e = if first { 0 } else { d.m() };
                            let (a, b) = (at2(&sul, 1, e)?, at2(&sul, 0, e)?);
                            if !cpts_are_colinear(r_start, a, b, three) {
                                return Err(bad_input());
                            }
                            if !cpts_are_colinear(r_start, a, b, four) {
                                adjust_rows(&mut sul, r_start, a, b, 0, e)?;
                            }
                        }
                        if let Some(d) = d1.filter(|_| !c1(1)) {
                            let e = if first { 0 } else { d.m() };
                            let (a, b) = (at2(&sur_r, 0, e)?, at2(&sur_r, 1, e)?);
                            if !cpts_are_colinear(r_end, a, b, three) {
                                return Err(bad_input());
                            }
                            if !cpts_are_colinear(r_end, a, b, four) {
                                adjust_rows(&mut sur_r, r_end, a, b, 1, e)?;
                            }
                        }
                    }
                    if dir == Direction::V {
                        if let Some(d) = d0.filter(|_| !c1(0)) {
                            let e = if first { 0 } else { d.n() };
                            let (a, b) = (at2(&svb, e, 1)?, at2(&svb, e, 0)?);
                            if !cpts_are_colinear(r_start, a, b, three) {
                                return Err(bad_input());
                            }
                            if !cpts_are_colinear(r_start, a, b, four) {
                                adjust_cols(&mut svb, r_start, a, b, 0, e)?;
                            }
                        }
                        if let Some(d) = d1.filter(|_| !c1(1)) {
                            let e = if first { 0 } else { d.n() };
                            let (a, b) = (at2(&svt, e, 0)?, at2(&svt, e, 1)?);
                            if !cpts_are_colinear(r_end, a, b, three) {
                                return Err(bad_input());
                            }
                            if !cpts_are_colinear(r_end, a, b, four) {
                                adjust_cols(&mut svt, r_end, a, b, 1, e)?;
                            }
                        }
                    }
                }
            }
        }
    }

    // Set some flags and get locals.
    let mut pr = 0usize;
    let mut mr = 0usize;
    let mut ur: Vec<f64> = Vec::new();
    let mut rfl = Ends::No;
    if let Some(rail) = opt.rail.as_ref() {
        let r0 = rail.first().and_then(Option::as_ref);
        let r1 = rail.get(1).and_then(Option::as_ref);
        if let Some(r) = r0.or(r1) {
            pr = r.degree();
            mr = r.m();
            ur = r.knots().to_vec();
        }
        rfl = Ends::of(r0.is_some(), r1.is_some());
    }
    let mut dfl = Ends::No;
    if let Some(der) = opt.der.as_ref() {
        dfl = Ends::of(
            der.first().is_some_and(Option::is_some),
            der.get(1).is_some_and(Option::is_some),
        );
    }
    let rail0 = || opt_rail(opt.rail.as_ref(), 0);
    let rail1 = || opt_rail(opt.rail.as_ref(), 1);

    // Get cross-boundary derivatives.
    let mut cur_a: Option<Curve> = None;
    let mut cur_b: Option<Curve> = None;
    // Upstream builds the derivative curves of the rational case with rails over the
    // derivative surfaces' own knot vectors (shared storage): later in-place knot
    // writes on the curves reach the surfaces until a curve gets new storage.
    let mut alias = [false; 2];
    if let Some(der) = opt.der.as_ref() {
        let d0 = der.first().and_then(Option::as_ref);
        let d1 = der.get(1).and_then(Option::as_ref);
        let no_dir = || SolidError::InvalidInput {
            operation: OP,
            reason: "derivative data with the NL_UVDIR skinning direction",
        };
        if !rat || !has_rail {
            if let Some(d) = d0 {
                let side = match dir {
                    Direction::U => Right,
                    Direction::V => Top,
                    Direction::UV => return Err(no_dir()),
                };
                cross_bound_deriv_crv_non_rat_srf(d, side, &mut cur_a)?;
            }
            if let Some(d) = d1 {
                let side = match dir {
                    Direction::U => Left,
                    Direction::V => Bottom,
                    Direction::UV => return Err(no_dir()),
                };
                cross_bound_deriv_crv_non_rat_srf(d, side, &mut cur_b)?;
            }
        } else {
            for (idx, d) in [(0usize, d0), (1, d1)] {
                let Some(d) = d else { continue };
                let (ns, ms, _) = srf_cpts(d);
                let (ps, qs) = d.degrees();
                let us = d.u_knots();
                let vs = d.v_knots();
                let mut built: Option<Curve> = None;
                if dir == Direction::U {
                    let (strip, den) = if idx == 0 {
                        (&sul, at(us, ns + 1)? - at(us, ns)?)
                    } else {
                        (&sur_r, at(us, ps + 1)? - at(us, ps)?)
                    };
                    let fact = ps as f64 / den;
                    let mut pw = vec![CPoint::default(); ms + 1];
                    for j in 0..=ms {
                        let c = CPoint::diff(at2(strip, 1, j)?, at2(strip, 0, j)?);
                        put(&mut pw, j, c.scale(fact))?;
                    }
                    built = Some(Curve::new(pw, qs, vs.to_vec())?);
                }
                if dir == Direction::V {
                    let (strip, den) = if idx == 0 {
                        (&svb, at(vs, ms + 1)? - at(vs, ms)?)
                    } else {
                        (&svt, at(vs, qs + 1)? - at(vs, qs)?)
                    };
                    let fact = qs as f64 / den;
                    let mut pw = vec![CPoint::default(); ns + 1];
                    for i in 0..=ns {
                        let c = CPoint::diff(at2(strip, i, 1)?, at2(strip, i, 0)?);
                        put(&mut pw, i, c.scale(fact))?;
                    }
                    built = Some(Curve::new(pw, ps, us.to_vec())?);
                }
                if built.is_none() {
                    return Err(no_dir());
                }
                if let Some(a) = alias.get_mut(idx) {
                    *a = true;
                }
                if idx == 0 {
                    cur_a = built;
                } else {
                    cur_b = built;
                }
            }
        }
    }
    let der0 = opt
        .der
        .as_ref()
        .is_some_and(|d| d.first().is_some_and(Option::is_some));
    let der1 = opt
        .der
        .as_ref()
        .is_some_and(|d| d.get(1).is_some_and(Option::is_some));
    let need = |c: &Option<Curve>| -> Result<Curve> { c.clone().ok_or_else(oob) };

    // Check C1 continuity.
    if has_der && has_rail {
        for (ri, r) in [(0usize, rail0()), (1, rail1())] {
            let Some(r) = r else { continue };
            for (di, present, cd) in [(0usize, der0, &cur_a), (1, der1, &cur_b)] {
                if !(present && c1(di)) {
                    continue;
                }
                let u = if di == 0 { at(&ur, 0)? } else { at(&ur, mr)? };
                let rdw = r.derivs_at_knot(u, Side::Left, 1)?;
                let c = need(cd)?;
                let dw = c.cpts();
                let e = if ri == 0 { 0 } else { c.n() };
                if !at(dw, e)?.approx_eq(at(&rdw, 1)?, MODEL_TOL) {
                    return Err(bad_input());
                }
            }
        }
    }

    // Make all curves compatible.
    if !cmp {
        let mut all: Vec<Curve> = cur.to_vec();
        let mut extra = Vec::new();
        if has_der {
            if der0 {
                all.push(need(&cur_a)?);
                extra.push(0usize);
            }
            if der1 {
                all.push(need(&cur_b)?);
                extra.push(1);
            }
        }
        let res = make_compatible_knot_tol_traced(&mut all, 0.0);
        let mut it = all.into_iter();
        for c in cur.iter_mut() {
            if let Some(x) = it.next() {
                *c = x;
            }
        }
        for e in &extra {
            let x = it.next();
            if *e == 0 {
                cur_a = x;
            } else {
                cur_b = x;
            }
        }
        let trace = res?;
        for (t, e) in trace.iter().skip(k + 1).zip(&extra) {
            if alias.get(*e) == Some(&true) {
                write_alias(&mut opt.der, *e, dir, &t.knots)?;
                if !t.live {
                    put(&mut alias, *e, false)?;
                }
            }
        }
    }

    let c0 = crv(cur, 0)?;
    let mut nc = c0.n();
    let mut pc = c0.degree();
    let mut mc = c0.m();
    let mut uc: Vec<f64> = c0.knots().to_vec();

    // Get derivative data.
    let mut qw = vec![CPoint::default(); k + 1];
    let mut cur_da: Option<Curve> = None;
    let mut cur_db: Option<Curve> = None;
    let mut mll = false;
    let mut mlr = false;
    let mut dec = false;
    if has_der {
        let mut sdr = (der0 && !c1(0)) || (der1 && !c1(1));
        if sdr {
            // Compute chordal lengths along the ends.
            let (mut l00, mut l01, mut l10, mut l11);
            let mut l0 = 0.0;
            let mut l1 = 0.0;
            if matches!(rfl, Ends::Left | Ends::Both) {
                let r = rail0().ok_or_else(oob)?;
                l00 = at(&r.derivs_at_knot(at(&ur, 0)?, Side::Left, 1)?, 1)?.magnitude();
                l01 = at(&r.derivs_at_knot(at(&ur, mr)?, Side::Left, 1)?, 1)?.magnitude();
                if rfl == Ends::Left {
                    for i in 0..=k {
                        put(&mut qw, i, r.eval_pt(tsv(i)?, Side::Left)?)?;
                    }
                    l0 = dist_cpolygon(&qw);
                }
            } else {
                for i in 0..=k {
                    let c = crv(cur, i)?;
                    nc = c.n();
                    put(&mut qw, i, at(c.cpts(), 0)?)?;
                }
                l00 = dist_cpolygon(&qw);
                l01 = l00;
            }
            if matches!(rfl, Ends::Right | Ends::Both) {
                let r = rail1().ok_or_else(oob)?;
                l10 = at(&r.derivs_at_knot(at(&ur, 0)?, Side::Left, 1)?, 1)?.magnitude();
                l11 = at(&r.derivs_at_knot(at(&ur, mr)?, Side::Left, 1)?, 1)?.magnitude();
                if rfl == Ends::Right {
                    for i in 0..=k {
                        put(&mut qw, i, r.eval_pt(tsv(i)?, Side::Left)?)?;
                    }
                    l1 = dist_cpolygon(&qw);
                }
            } else {
                for i in 0..=k {
                    let c = crv(cur, i)?;
                    nc = c.n();
                    put(&mut qw, i, at(c.cpts(), nc)?)?;
                }
                l10 = dist_cpolygon(&qw);
                l11 = l10;
            }

            // Set magnitudes for one rail.
            if rfl == Ends::Left {
                l10 = (l10 / l0) * l00;
                l11 = (l11 / l0) * l01;
            } else if rfl == Ends::Right {
                l00 = (l00 / l1) * l10;
                l01 = (l01 / l1) * l11;
            }

            // Adjust magnitudes across cur[0] (with l00, l10) and cur[k] (l01, l11).
            for side in 0..2usize {
                if !(if side == 0 { der0 } else { der1 }) {
                    continue;
                }
                let (la, lb) = if side == 0 { (l00, l10) } else { (l01, l11) };
                let src = if side == 0 { &mut cur_a } else { &mut cur_b };
                let mut mg0 = 0.0;
                let mut mgn = 0.0;
                let mut lsm = false;
                let mut opr = false;
                if !c1(side) {
                    // See if end magnitudes match up with der.
                    sdr = true;
                    let c = src.as_ref().ok_or_else(oob)?;
                    let dw = c.cpts();
                    let nd = c.n();
                    mg0 = at(dw, 0)?.magnitude();
                    let fac0 = if mg0 < la { mg0 / la } else { la / mg0 };
                    if fac0 < DPER {
                        sdr = false;
                    }
                    mgn = at(dw, nd)?.magnitude();
                    let fac1 = if mgn < lb { mgn / lb } else { lb / mgn };
                    if fac1 < DPER {
                        sdr = false;
                    }

                    // See if end magnitudes are the same.
                    if rfl == Ends::No {
                        let fact = if la < lb { la / lb } else { lb / la };
                        if fact >= LPER {
                            lsm = true;
                        }
                    }

                    // See if magnitude opposite to rail matches up with der.
                    if (rfl == Ends::Left && fac1 >= DPER) || (rfl == Ends::Right && fac0 >= DPER) {
                        opr = true;
                    }
                }

                // Get derivative curves.
                let out;
                if c1(side) || (rfl == Ends::No && sdr) {
                    // No adjustment needed.
                    out = need(src)?;
                } else if lsm || opr {
                    // Derivative curve is scaled by a constant.
                    let mut alf = 0.0;
                    if lsm {
                        let d1 = la / mg0;
                        let d2 = lb / mgn;
                        alf = 0.5 * (d1 + d2);
                    } else if rfl == Ends::Left {
                        alf = la / mg0;
                    } else if rfl == Ends::Right {
                        alf = lb / mgn;
                    }
                    let mut pa = need(src)?;
                    pa.constant_multiply_4d(alf);
                    out = pa;
                } else {
                    // Derivative curve is scaled by a function.
                    let mut cfn =
                        CurveFunction::new(vec![la / mg0, lb / mgn], 1, vec![0.0, 0.0, 1.0, 1.0])?;
                    let a = src.as_mut().ok_or_else(oob)?;
                    out = cfn.multiply_crv_4d(a)?;
                    if alias.get(side) == Some(&true) {
                        write_alias(&mut opt.der, side, dir, a.knots())?;
                    }
                    if !a.is_4d_degen() {
                        dec = true;
                    }
                    if side == 0 {
                        mll = true;
                    } else {
                        mlr = true;
                    }
                }
                if side == 0 {
                    cur_da = Some(out);
                } else {
                    cur_db = Some(out);
                }
            }
        } else {
            // No need to compute magnitudes.
            if der0 {
                cur_da = Some(need(&cur_a)?);
            }
            if der1 {
                cur_db = Some(need(&cur_b)?);
            }
        }
    }

    // Make derivative curves compatible.
    if dfl == Ends::Both && (mll || mlr) {
        if !mll {
            cur_da = Some(need(&cur_da)?.elevate_degree(1)?);
        }
        if !mlr {
            cur_db = Some(need(&cur_db)?.elevate_degree(1)?);
        }
    }

    // Get derivative vectors.
    let dws: Vec<CPoint> = match (der0, &cur_da) {
        (true, Some(c)) => c.cpts().to_vec(),
        _ => Vec::new(),
    };
    let dwe: Vec<CPoint> = match (der1, &cur_db) {
        (true, Some(c)) => c.cpts().to_vec(),
        _ => Vec::new(),
    };

    // Degree elevate if necessary.
    if dec {
        for c in cur.iter_mut() {
            *c = c.elevate_degree(1)?;
        }
        let c0 = crv(cur, 0)?;
        nc = c0.n();
        pc = c0.degree();
        mc = c0.m();
        uc = c0.knots().to_vec();
    }

    // Get surface indices.
    let ps = if rfl == Ends::No { deg } else { pr };
    let qs = pc;
    let mut ns = match dfl {
        Ends::No => k,
        Ends::Left | Ends::Right => k + 1,
        Ends::Both => k + 2,
    };
    if ns < ps {
        return Err(bad_input());
    }
    let ms = nc;
    let mut rs = ns + ps + 1;
    let ss = mc;

    // Get parameters.
    let mut tl = vec![0.0; k + 1];
    if !has_rail && ts.is_none() {
        let mut rw: Net = net(k, nc);
        for i in 0..=k {
            let c = crv(cur, i)?;
            nc = c.n();
            let cw = c.cpts();
            for j in 0..=nc {
                put2(&mut rw, i, j, at(cw, j)?)?;
            }
        }
        tl = interp_params(&rw, ParamMethod::ChordLength, DataDir::U)?;
    } else {
        for (i, t) in tl.iter_mut().enumerate() {
            *t = tsv(i)?;
        }
    }

    // Get knot vector for interpolation.
    let knt: Vec<f64> = if let Some(kns) = opt.kns.as_ref() {
        if dec1(kns.len())? != rs {
            return Err(bad_input());
        }
        kns.clone()
    } else {
        match dfl {
            Ends::No => interp_knot_vector(&tl, ps)?,
            Ends::Left => knot_vector_deriv(&tl, ps, DerivEnd::Start)?,
            Ends::Right => knot_vector_deriv(&tl, ps, DerivEnd::End)?,
            Ends::Both => knot_vector_end_derivs(&tl, ps)?,
        }
    };
    if knt.len() != rs + 1 {
        return Err(oob());
    }
    let knt_kv = KnotVector::from_slice(&knt)?;

    // See if refinement is needed.
    let mut xr: Vec<f64> = Vec::new();
    let mut xt: Vec<f64> = Vec::new();
    if has_rail {
        let knr = KnotVector::from_slice(&ur)?;
        (xt, xr) = merge_knot_vectors(&knt_kv, &knr, ps)?;
    }

    // Update surface indexes and allocate the net.
    if !xt.is_empty() {
        ns += xt.len();
        rs += xt.len();
    }
    let mut sw: Net = net(ns, ms);

    // Refine rail(s) and get surface boundaries.
    if has_rail {
        for (r, col) in [(rail0(), 0usize), (rail1(), ms)] {
            let Some(r) = r else { continue };
            let pw: Vec<CPoint> = if xr.is_empty() {
                r.cpts().to_vec()
            } else {
                r.refine_to_knot_vector(&xr)?
            };
            let last = if xr.is_empty() { r.n() } else { ns };
            for i in 0..=last {
                put2(&mut sw, i, col, at(&pw, i)?)?;
            }
        }
    }

    // Now interpolate cross-sectional curves.
    let ends = match dfl {
        Ends::No => MatrixEnds::None,
        Ends::Left => MatrixEnds::Start,
        Ends::Right => MatrixEnds::End,
        Ends::Both => MatrixEnds::Both,
    };
    let cm = calc_matrix(&knt_kv, ps, &tl, ends)?;
    let js = usize::from(matches!(rfl, Ends::Left | Ends::Both));
    let je = if matches!(rfl, Ends::Right | Ends::Both) {
        dec1(nc)?
    } else {
        nc
    };
    for j in js..=je {
        for i in 0..=k {
            put(&mut qw, i, at(crv(cur, i)?.cpts(), j)?)?;
        }
        let dw = match dfl {
            Ends::No => cpts_with_matrix(&qw, ps, &cm)?,
            Ends::Left => deriv_matrix(&qw, ps, &knt_kv, at(&dws, j)?, DerivEnd::Start, &cm)?,
            Ends::Right => deriv_matrix(&qw, ps, &knt_kv, at(&dwe, j)?, DerivEnd::End, &cm)?,
            Ends::Both => derivs_matrix(&qw, ps, &knt_kv, at(&dws, j)?, at(&dwe, j)?, &cm)?,
        };
        if xt.is_empty() {
            for i in 0..=ns {
                put2(&mut sw, i, j, at(&dw, i)?)?;
            }
        } else {
            let cur_i = Curve::new(dw, ps, knt.clone())?;
            let pw = cur_i.refine_to_knot_vector(&xt)?;
            for i in 0..=ns {
                put2(&mut sw, i, j, at(&pw, i)?)?;
            }
        }
    }

    // Get surface knot vectors.
    let ut = if xt.is_empty() {
        knt
    } else {
        insert_knots(&knt, ps, &xt)?
    };
    let mut us = vec![0.0; rs + 1];
    for i in 0..=dec1(ut.len())? {
        put(&mut us, i, at(&ut, i)?)?;
    }
    let mut vs = vec![0.0; ss + 1];
    for j in 0..=ss {
        put(&mut vs, j, at(&uc, j)?)?;
    }
    let mut sur = Surface::new(sw, ps, qs, us, vs)?;

    // Remove knots if required.
    if es > 0.0 {
        let keep = if has_rail {
            Some(ur.get(..=mr).ok_or_else(oob)?)
        } else {
            None
        };
        sur = remove_knots_constrained(&sur, keep, None, es, Direction::U)?;
    }

    // If v-skinning is needed, swap parametrization.
    if dir == Direction::V {
        sur.swap_uv();
    }

    // Translate back if necessary.
    if trl {
        t *= -1.0;
        sur.translate(t);
    }

    // Check weights.
    sur.weights_are_valid()?;
    Ok(sur)
}

/// Writes `knots` into the knot vector of `der[i]` shared with its derivative curve
/// (v for [`Direction::U`], u otherwise).
fn write_alias(
    der: &mut Option<[Option<Surface>; 2]>,
    i: usize,
    dir: Direction,
    knots: &[f64],
) -> Result<()> {
    let d = der
        .as_mut()
        .and_then(|d| d.get_mut(i))
        .and_then(Option::as_mut)
        .ok_or_else(oob)?;
    let dst = if dir == Direction::U {
        d.v_knots_mut()
    } else {
        d.u_knots_mut()
    };
    for (a, b) in dst.iter_mut().zip(knots) {
        *a = *b;
    }
    Ok(())
}

/// `rail[i]` of an optional rail pair.
fn opt_rail(rail: Option<&[Option<Curve>; 2]>, i: usize) -> Option<&Curve> {
    rail.and_then(|r| r.get(i)).and_then(Option::as_ref)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;

    fn section(z: f64, r: f64) -> Curve {
        let pts = (0..4)
            .map(|j| CPoint::new(f64::from(j), r * f64::from(j % 2), z, NOW))
            .collect();
        Curve::new(pts, 2, vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0]).unwrap()
    }

    fn sections() -> Vec<Curve> {
        (0..4)
            .map(|i| section(2.0 * f64::from(i), 0.5 + 0.1 * f64::from(i)))
            .collect()
    }

    #[test]
    fn interpolates_cross_sections() {
        for dir in [Direction::U, Direction::V] {
            let mut cs = sections();
            let mut opt = SkinBoundaryOptions {
                deg: 3,
                dir,
                cmp: true,
                ..SkinBoundaryOptions::default()
            };
            let s = create_skin_srf_boundary_continuity(&mut cs, &mut opt).unwrap();
            let (u, v) = if dir == Direction::U {
                (1.0, 0.25)
            } else {
                (0.25, 1.0)
            };
            let p = s.eval(u, v, Side::Left, Side::Left).unwrap();
            let q = cs[3].eval(0.25, Side::Left).unwrap();
            assert!((p - q).length() < 1e-9, "{dir:?}");
        }
    }

    #[test]
    fn derivative_strips_add_rows() {
        let mut cs = sections();
        let strip = |c: &Curve, sgn: f64| {
            let row: Vec<CPoint> = c.cpts().to_vec();
            let off: Vec<CPoint> = row
                .iter()
                .map(|p| CPoint::new(p.x, p.y, p.z + sgn, NOW))
                .collect();
            let net = if sgn < 0.0 {
                vec![off, row]
            } else {
                vec![row, off]
            };
            Surface::new(net, 1, 2, vec![0.0, 0.0, 1.0, 1.0], c.knots().to_vec()).unwrap()
        };
        let d0 = strip(&cs[0], -1.0);
        let d1 = strip(&cs[3], 1.0);
        for cnt in [BoundaryContinuity::C1, BoundaryContinuity::G1] {
            let mut opt = SkinBoundaryOptions {
                der: Some([Some(d0.clone()), Some(d1.clone())]),
                cnt: [cnt; 2],
                deg: 3,
                ..SkinBoundaryOptions::default()
            };
            let s = create_skin_srf_boundary_continuity(&mut cs, &mut opt).unwrap();
            assert_eq!(s.n(), 5);
        }
    }

    #[test]
    fn rejects_bad_input() {
        let mut cs = sections();
        let rail = Curve::new(
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(0.0, 0.0, 6.0, NOW),
            ],
            1,
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap();
        let mut opt = SkinBoundaryOptions {
            rail: Some([Some(rail), None]),
            deg: 1,
            ..SkinBoundaryOptions::default()
        };
        let e = create_skin_srf_boundary_continuity(&mut cs, &mut opt).unwrap_err();
        assert!(matches!(
            e,
            SolidError::Nlib {
                code: NlibCode::Input,
                ..
            }
        ));
        let mut opt = SkinBoundaryOptions {
            deg: 5,
            ..SkinBoundaryOptions::default()
        };
        assert!(create_skin_srf_boundary_continuity(&mut cs, &mut opt).is_err());
        assert!(create_skin_srf_boundary_continuity(&mut [], &mut opt).is_err());
        assert_eq!(BoundaryContinuity::from_flag(7), BoundaryContinuity::G1);
        assert_eq!(BoundaryContinuity::C1.flag(), 1);
    }
}
