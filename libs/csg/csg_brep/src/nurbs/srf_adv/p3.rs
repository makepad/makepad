//! Grid evaluation of surface points and unit normals, surface extension by a
//! parameter distance (G1, ruled G1, G2 and C-max continuations), and the swung,
//! skinned and translational sweep surface constructions.
//!
//! Upstream: `source/SMLib/src/NL_SrfAdv.cpp`, `source/SMLib/inc/NL_SrfAdv.h`.
//!
//! | Rust                                          | Upstream                  |
//! |-----------------------------------------------|---------------------------|
//! | [`grid_pts_normals`], [`GridPtsNormals`]      | `N_SrfGetGridPtsNormals`  |
//! | [`extend_by_param_dist`], [`ExtendContinuity`] | `N_SrfExtendByParamDist` |
//! | [`create_swung_srf`]                          | `N_CreateSwungSrf`        |
//! | [`create_skin_srf`]                           | `N_CreateSkinSrf`         |
//! | [`create_trans_sweep_srf`]                    | `N_CreateTransSweepSrf`   |
//! | [`KNOT_TOL`]                                  | `KN_TOL` (file-private)   |
//! | [`MAX_EXTEND_DEPTH`]                          | no counterpart (recursion bound) |
//!
//! Deviations:
//! - Out-params become return values: every routine returns a new surface (or the
//!   point and normal grids) instead of filling caller-provided ones. Upstream's
//!   in-place mode of `N_SrfExtendByParamDist` (`surP == surQ`) computes the same
//!   result into fresh arrays, so a caller replaces its surface with the returned one.
//!   The `NL_STACKS` arguments have no counterpart; Rust owns the allocations.
//! - Grid points and normals have a local aggregate requested-storage limit of
//!   [`crate::limits::MAX_ALLOC_BYTES`], including both grids' row-vector metadata.
//!   Overflow, excess and failed output reservations return `NL_MEM_ERR`. This is
//!   stronger than upstream's allocation policy and excludes allocator overhead,
//!   basis arrays, control-point copies and other workspace; those existing
//!   allocations are not all fallible.
//! - Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number
//!   instead of a flag plus the global `NL_ERROR` record. `N_CreateSkinSrf` builds the
//!   surface before it reports invalid weights (`NL_WEI_ERR`); [`create_skin_srf`]
//!   returns only the error then.
//! - The u- and v-direction branches of `N_SrfExtendByParamDist` are one
//!   implementation over the net transposed for the v direction ([`Surface::swap_uv`],
//!   an exact copy). The knot insertion and removals it calls run on the surface in its
//!   real orientation, as upstream; the arithmetic of the extension itself is the
//!   same expression for expression, so results agree bit for bit. The `dflg` flag is
//!   a [`Direction`] (anything but [`Direction::U`] is the v direction, as upstream's
//!   `else` branch), `end` a [`CurveEnd`] and `cont` an [`ExtendContinuity`] (the
//!   values upstream handles; other `NL_*` flags fall through to its G1 code).
//! - `N_SrfExtendByParamDist` splits extensions longer than `0.9313` of the knot range
//!   (G1, G2) by recursion without a bound; the Rust function stops after
//!   [`MAX_EXTEND_DEPTH`] nested calls with [`SolidError::NotConverged`] instead of
//!   exhausting the stack.
//! - `N_CptReflect` failures (a reflection plane the normal line misses) are ignored
//!   upstream, which then keeps whatever its uninitialised scratch array holds; the
//!   Rust function returns the error.
//! - `N_CreateSkinSrf` with `k == 0` and `cld == NL_YES` divides by zero (`l % k`);
//!   [`create_skin_srf`] returns `InvalidInput`. `N_CrvsAreCoincident(cur[0], cur[k])`
//!   modifies both curves in place (it makes them compatible), and so does the Rust
//!   function; for `k == 0` it is called on two copies of the curve and the first is
//!   kept.
//! - Every array access is bounds checked. Where upstream would index outside its
//!   arrays (invalid knot vectors, incompatible cross sections with `cmp == NL_NO`,
//!   extension parameters on the wrong side of the domain, ...) the functions return
//!   [`SolidError::InvalidInput`].
//! - Scratch and output nets that cannot be allocated are `NL_MEM_ERR` (upstream's
//!   failed `N_Alloc*`); index arithmetic on a degree that overflows (`p + 1`,
//!   `ns + p + 1` for a degree near `isize::MAX`, where upstream's signed overflow is
//!   undefined) is `InvalidInput`.

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_adv::basis_derivs_array;
use crate::nurbs::bezier::PASCAL_TRI;
use crate::nurbs::crv_adv::p1::CurveEnd;
use crate::nurbs::crv_basic::{curves_are_coincident, Curve};
use crate::nurbs::crv_fit::p1::{
    interp_given_params, interp_knot_vector, interp_knots_and_derivs, knot_vector_end_derivs,
    ParamMethod,
};
use crate::nurbs::crv_tool::p2::make_compatible_knot_tol;
use crate::nurbs::geom::{cpt_reflect, normalize, pt_along_vector, Plane};
use crate::nurbs::index::{get, oob, set, storage, sx, ux};
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{blend_pt, CPoint, MatrixType, RealMatrix, MODEL_TOL, NOW};
use crate::nurbs::srf_basic::{Direction, Surface, WEIGHT_MAX, WEIGHT_MIN};
use crate::nurbs::srf_fit::param_values;
use crate::nurbs::srf_tool::p1::insert_knot;
use crate::nurbs::srf_tool::p2::remove_knot_conditional;

/// Upstream `KN_TOL`: an extension's mirror parameter is snapped to an interior knot
/// closer than this, relative to the knot range.
pub const KNOT_TOL: f64 = 1.0e-6;

/// Largest nesting of the extension splits of [`extend_by_param_dist`] (upstream
/// recurses without a bound).
pub const MAX_EXTEND_DEPTH: usize = 256;

const OP_GRID: &str = "nurbs::srf_adv::p3::grid_pts_normals";
const OP_EXTEND: &str = "nurbs::srf_adv::p3::extend_by_param_dist";
const OP_SWUNG: &str = "nurbs::srf_adv::p3::create_swung_srf";
const OP_SKIN: &str = "nurbs::srf_adv::p3::create_skin_srf";
const OP_SWEEP: &str = "nurbs::srf_adv::p3::create_trans_sweep_srf";

/// A control net, `Pw[i][j]`.
type Net = Vec<Vec<CPoint>>;

/// `w[i][j]`.
fn g2<T: Copy>(w: &[Vec<T>], i: isize, j: isize, op: &'static str) -> Result<T> {
    let row = w.get(ux(i, op)?).ok_or_else(|| oob(op))?;
    get(row, j, op)
}

/// `w[i][j] = x`.
fn s2<T>(w: &mut [Vec<T>], i: isize, j: isize, x: T, op: &'static str) -> Result<()> {
    let row = w.get_mut(ux(i, op)?).ok_or_else(|| oob(op))?;
    set(row, j, x, op)
}

/// A zero-filled `(n+1) x (m+1)` array (upstream allocates uninitialised storage).
/// `NL_MEM_ERR` where it cannot be allocated.
fn net<T: Clone + Default>(n: isize, m: isize, op: &'static str) -> Result<Vec<Vec<T>>> {
    let cols = ux(m, op)?.checked_add(1).ok_or_else(|| oob(op))?;
    let rows = ux(n, op)?.checked_add(1).ok_or_else(|| oob(op))?;
    super::grid(rows, cols, T::default(), op)
}

/// `v[i]` for a `usize` index.
fn at<T: Copy>(v: &[T], i: usize, op: &'static str) -> Result<T> {
    v.get(i).copied().ok_or_else(|| oob(op))
}

// ---- N_SrfGetGridPtsNormals ---------------------------------------------------------

/// Requested storage for both grids, including their row-vector metadata only.
fn grid_output_bytes(rows: usize, cols: usize) -> Option<usize> {
    let payload = rows.checked_mul(cols)?.checked_mul(size_of::<Vec3>())?;
    let metadata = rows.checked_mul(size_of::<Vec<Vec3>>())?;
    payload.checked_add(metadata)?.checked_mul(2)
}

/// The local aggregate output policy admits equality, but not overflow.
fn admit_grid_output(bytes: Option<usize>) -> Result<()> {
    match bytes {
        Some(bytes) if bytes <= crate::limits::MAX_ALLOC_BYTES => Ok(()),
        _ => Err(SolidError::nlib(OP_GRID, NlibCode::Memory)),
    }
}

/// Reserve every output allocation before filling it; never clone populated rows.
fn grid_output(rows: usize, cols: usize) -> Result<Vec<Vec<Vec3>>> {
    let memory = || SolidError::nlib(OP_GRID, NlibCode::Memory);
    let mut grid = Vec::new();
    grid.try_reserve_exact(rows).map_err(|_| memory())?;
    for _ in 0..rows {
        let mut row = Vec::new();
        row.try_reserve_exact(cols).map_err(|_| memory())?;
        row.resize(cols, Vec3::ZERO);
        grid.push(row);
    }
    Ok(grid)
}

/// Result of [`grid_pts_normals`]: `points[a][b]` and unit `normals[a][b]` at
/// `(u[a], v[b])`.
#[derive(Debug, Clone, PartialEq)]
pub struct GridPtsNormals {
    /// Surface points (upstream `P`).
    pub points: Vec<Vec<Vec3>>,
    /// Unit normals (upstream `N`).
    pub normals: Vec<Vec<Vec3>>,
}

/// Points and unit normals of `sur` on the grid of increasing parameters `u` x `v`
/// (upstream `N_SrfGetGridPtsNormals`). `ufl` / `vfl` pick the span at a knot. Where
/// the cross product of the partial derivatives is shorter than `0.001 * NL_MTOL`
/// (or cannot be normalised) the point and normal come from
/// [`Surface::eval_pt_normal_at_pole`].
///
/// # Errors
///
/// [`NlibCode::Index`] for an empty `u` or `v`; [`NlibCode::Parameter`] when
/// `u[0] < U[0]`, `u[last] > U[r]` (or the same in v); errors of
/// [`basis_derivs_array`] and [`Surface::eval_pt_normal_at_pole`].
/// [`NlibCode::Memory`] if the two output grids' aggregate requested storage,
/// including row-vector metadata, overflows or exceeds
/// [`crate::limits::MAX_ALLOC_BYTES`], or an output reservation fails. This local
/// budget excludes allocator overhead and other workspace and is checked after
/// basis evaluation and control-point preparation.
pub fn grid_pts_normals(
    sur: &Surface,
    u: &[f64],
    v: &[f64],
    ufl: Side,
    vfl: Side,
) -> Result<GridPtsNormals> {
    const OP: &str = OP_GRID;
    let idx = sur.indices();
    let (p, q) = (sx(idx.p, OP)?, sx(idx.q, OP)?);
    let uk = sur.u_knots();
    let vk = sur.v_knots();

    // Check parameters.
    let (Some(mu), Some(mv)) = (u.len().checked_sub(1), v.len().checked_sub(1)) else {
        return Err(SolidError::nlib(OP, NlibCode::Index));
    };
    if at(u, 0, OP)? < get(uk, 0, OP)? || at(u, mu, OP)? > get(uk, sx(idx.r, OP)?, OP)? {
        return Err(SolidError::nlib(OP, NlibCode::Parameter));
    }
    if at(v, 0, OP)? < get(vk, 0, OP)? || at(v, mv, OP)? > get(vk, sx(idx.s, OP)?, OP)? {
        return Err(SolidError::nlib(OP, NlibCode::Parameter));
    }

    let cross_tol = 0.001 * MODEL_TOL;

    // Compute non-vanishing B-splines and map cp's to 3-D.
    let (knu, knv) = sur.knot_vectors()?;
    let bu = basis_derivs_array(&knu, idx.p, u, ufl, 1)?;
    let bv = basis_derivs_array(&knv, idx.q, v, vfl, 1)?;
    let nu = &bu.derivs;
    let nv = &bv.derivs;
    let pw = sur.cpts();
    let qp: Vec<Vec<Vec3>> = pw
        .iter()
        .map(|row| row.iter().map(|c| c.drop_w()).collect())
        .collect();
    let rational = sur.is_rational();
    let w: Vec<Vec<f64>> = if rational {
        pw.iter()
            .map(|row| row.iter().map(|c| c.w).collect())
            .collect()
    } else {
        Vec::new()
    };

    admit_grid_output(grid_output_bytes(u.len(), v.len()))?;
    let mut points = grid_output(u.len(), v.len())?;
    let mut normals = grid_output(u.len(), v.len())?;

    // Compute grid of points and normals.
    for a in 0..=mu {
        let usp = sx(at(&bu.spans, a, OP)?, OP)?;
        for b in 0..=mv {
            let vsp = sx(at(&bv.spans, b, OP)?, OP)?;
            let bi = sx(b, OP)?;
            let ai = sx(a, OP)?;
            // SD[k][l] (AD[k][l] and wd[k][l] when rational): derivative k in u,
            // l in v of the (homogeneous) surface at (u[a], v[b]).
            let comp = |k: usize, l: usize| -> Result<(Vec3, f64)> {
                let nuk = nu.get(k).ok_or_else(|| oob(OP))?;
                let nvl = nv.get(l).ok_or_else(|| oob(OP))?;
                let mut acc = Vec3::ZERO;
                let mut wacc = 0.0;
                for i in 0..=p {
                    let mut t3 = Vec3::ZERO;
                    let mut t = 0.0;
                    let c = usp - p + i;
                    for j in 0..=q {
                        let d = vsp - q + j;
                        let nvj = g2(nvl, j, bi, OP)?;
                        if rational {
                            t += g2(&w, c, d, OP)? * nvj;
                        }
                        blend_pt(&mut t3, nvj, g2(&qp, c, d, OP)?);
                    }
                    let nui = g2(nuk, i, ai, OP)?;
                    blend_pt(&mut acc, nui, t3);
                    if rational {
                        wacc += t * nui;
                    }
                }
                Ok((acc, wacc))
            };
            let (s00, s10, s01) = if rational {
                let (a00, w00) = comp(0, 0)?;
                let (a01, w01) = comp(0, 1)?;
                let (a10, w10) = comp(1, 0)?;
                let fac = 1.0 / w00;
                let s00 = a00 * fac;
                let s10 = (a10 - s00 * w10) * fac;
                let s01 = (a01 - s00 * w01) * fac;
                (s00, s10, s01)
            } else {
                (comp(0, 0)?.0, comp(1, 0)?.0, comp(0, 1)?.0)
            };

            let cross = s10.cross(s01);
            let unit = if let Ok((nn, fac)) = normalize(cross) {
                if fac <= cross_tol {
                    None
                } else {
                    Some(nn)
                }
            } else {
                None
            };
            let (pt, nn) = if let Some(nn) = unit {
                (s00, nn)
            } else {
                let e = sur.eval_pt_normal_at_pole(at(u, a, OP)?, at(v, b, OP)?, ufl, vfl)?;
                (e.p, e.n)
            };
            s2(&mut points, ai, bi, pt, OP)?;
            s2(&mut normals, ai, bi, nn, OP)?;
        }
    }
    Ok(GridPtsNormals { points, normals })
}

// ---- N_SrfExtendByParamDist ---------------------------------------------------------

/// Continuity of a surface extension at the old boundary (`NL_G1 = 2`, `NL_G2 = 3`,
/// `NL_CMAX = 4`, `NL_G1R = 6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtendContinuity {
    /// `NL_G1`: reflection extension with tangent-plane continuity.
    G1,
    /// `NL_G2`: reflection extension with curvature continuity.
    G2,
    /// `NL_CMAX`: the polynomial continuation of the boundary span (no knot there).
    Cmax,
    /// `NL_G1R`: a ruled extension joining with G1 continuity.
    G1R,
}

impl ExtendContinuity {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            ExtendContinuity::G1 => 2,
            ExtendContinuity::G2 => 3,
            ExtendContinuity::Cmax => 4,
            ExtendContinuity::G1R => 6,
        }
    }

    /// The continuity for an upstream flag value, `None` for any other value.
    #[must_use]
    pub const fn from_flag(flag: i32) -> Option<Self> {
        match flag {
            2 => Some(ExtendContinuity::G1),
            3 => Some(ExtendContinuity::G2),
            4 => Some(ExtendContinuity::Cmax),
            6 => Some(ExtendContinuity::G1R),
            _ => None,
        }
    }
}

/// The surface with u and v exchanged when `udir` is false (an involution).
fn reorient(mut s: Surface, udir: bool) -> Surface {
    if !udir {
        s.swap_uv();
    }
    s
}

/// Extends `sur` in direction `dir` at `end` so that the new domain boundary is the
/// parameter `par` (upstream `N_SrfExtendByParamDist`).
///
/// - [`ExtendContinuity::G1R`] appends one ruled span (degree-elevated) along the
///   boundary tangents and tries to remove one occurrence of the old boundary knot.
/// - [`ExtendContinuity::G1`] / [`ExtendContinuity::G2`] mirror the knots between the
///   old boundary and the parameter as far inside as `par` is outside, reflect the
///   control points in the boundary tangent planes (Shetty & White's corrections for
///   rational surfaces and G2) and remove the knots made redundant; extensions longer
///   than `0.9313` of the knot range are done in two steps.
/// - [`ExtendContinuity::Cmax`] continues the polynomial of the boundary span.
///
/// # Errors
///
/// [`NlibCode::Geometry`] for a degenerate reflection plane; [`NlibCode::Weight`] when
/// a C-max extension creates a weight outside `[WEIGHT_MIN, WEIGHT_MAX]`; errors of the
/// span search, knot insertion and removal, least-squares solve and reflection;
/// [`SolidError::NotConverged`] after [`MAX_EXTEND_DEPTH`] nested splits.
pub fn extend_by_param_dist(
    sur: &Surface,
    par: f64,
    dir: Direction,
    end: CurveEnd,
    cont: ExtendContinuity,
) -> Result<Surface> {
    extend_rec(
        sur,
        par,
        dir == Direction::U,
        end == CurveEnd::Start,
        cont,
        0,
    )
}

#[allow(clippy::too_many_lines)]
fn extend_rec(
    sur: &Surface,
    par: f64,
    udir: bool,
    start: bool,
    cont: ExtendContinuity,
    depth: usize,
) -> Result<Surface> {
    const OP: &str = OP_EXTEND;
    use ExtendContinuity::{Cmax, G1, G1R, G2};
    if depth > MAX_EXTEND_DEPTH {
        return Err(SolidError::NotConverged {
            operation: OP,
            iterations: u32::try_from(depth).unwrap_or(u32::MAX),
        });
    }
    let real_dir = if udir { Direction::U } else { Direction::V };

    // Local notation, with the extension direction as u.
    let a = reorient(sur.clone(), udir);
    let idx = a.indices();
    let (n, m, p, r, s) = (
        sx(idx.n, OP)?,
        sx(idx.m, OP)?,
        sx(idx.p, OP)?,
        sx(idx.r, OP)?,
        sx(idx.s, OP)?,
    );
    let pw = a.cpts();
    let up = a.u_knots().to_vec();
    let vp = a.v_knots().to_vec();
    let rat = a.is_rational();
    let u0 = get(&up, 0, OP)?;
    let ur = get(&up, r, OP)?;
    let pf = p as f64;

    // Check if need to recurse because par is too big.
    if cont == G1 || cont == G2 {
        let big = if start {
            u0 - par > 0.9313 * (ur - u0)
        } else {
            par - ur > 0.9313 * (ur - u0)
        };
        if big {
            let d2 = if start {
                par + 0.4671 * (ur - u0)
            } else {
                par - 0.4671 * (ur - u0)
            };
            let first = extend_rec(sur, d2, udir, start, cont, depth + 1)?;
            return extend_rec(&first, par, udir, start, cont, depth + 1);
        }
    }

    // Let uu = point in range same distance from end as given par value.
    let mut uu = if start {
        2.0 * u0 - par
    } else {
        2.0 * ur - par
    };
    let mut d1 = f64::MAX;
    let mut jj: isize = 0;
    for ii in p.checked_add(1).ok_or_else(|| oob(OP))?..(r - p) {
        let x = get(&up, ii, OP)?;
        if (uu - x).abs() < d1 {
            d1 = (uu - x).abs();
            jj = ii;
        }
    }
    if d1 < KNOT_TOL * (ur - u0) {
        uu = get(&up, jj, OP)?;
    }

    // Determine nq, mq, rq, sq and allocate working memory.
    let knu = KnotVector::from_slice(&up)?;
    let mut span: isize = 0;
    let mut mult: isize = 0;
    let mut kk: isize = 0;
    let nq: isize;
    if start {
        match cont {
            Cmax => {
                mult = 1;
                while p + mult < r {
                    if get(&up, p + mult + 1, OP)? == get(&up, p + 1, OP)? {
                        mult += 1;
                    } else {
                        break;
                    }
                }
                if mult > p {
                    mult = p;
                }
                nq = n + 2 * p - mult;
                kk = 2 * p;
            }
            G1R => {
                nq = n + p;
                kk = 0;
            }
            G1 | G2 => {
                let (sp, ml) = knu.find_span_and_mult(idx.p, uu, Side::Right)?;
                span = sx(sp, OP)?;
                mult = sx(ml, OP)?;
                nq = n + span + p - mult;
                kk = 2 * span;
            }
        }
    } else {
        match cont {
            Cmax => {
                mult = 1;
                while r - p - mult > 0 {
                    if get(&up, r - p - mult - 1, OP)? == get(&up, r - p - 1, OP)? {
                        mult += 1;
                    } else {
                        break;
                    }
                }
                if mult > p {
                    mult = p;
                }
                nq = n + 2 * p - mult;
                kk = 2 * p;
            }
            G1R => nq = n + p,
            G1 | G2 => {
                let (sp, ml) = knu.find_span_and_mult(idx.p, uu, Side::Left)?;
                span = sx(sp, OP)?;
                mult = sx(ml, OP)?;
                nq = n + r - span - 1 + p - mult;
                kk = 2 * (r - span - 1);
            }
        }
    }
    let mq = m;
    let sq = s;
    let rq = nq + p + 1;
    let mut rw: Net = if cont == G1R {
        Vec::new()
    } else {
        net(mq, kk, OP)?
    };
    let mut qw: Net = net(nq, mq, OP)?;
    let mut uq: Vec<f64> = storage(rq, OP)?;
    let mut vq: Vec<f64> = storage(sq, OP)?;

    // Ruled (G1R) extension.
    if cont == G1R {
        for ii in 0..=s {
            set(&mut vq, ii, get(&vp, ii, OP)?, OP)?;
        }
        let d2 = pf;
        uu = par;
        if start {
            for ii in 0..=p {
                set(&mut uq, ii, uu, OP)?;
            }
            for ii in 1..=r {
                set(&mut uq, ii + p, get(&up, ii, OP)?, OP)?;
            }
            for ii in 0..=n {
                for jj in 0..=m {
                    s2(&mut qw, ii + p, jj, g2(&pw, ii, jj, OP)?, OP)?;
                }
            }
            // Compute new Qw[0][jj], i.e. boundary control points.
            let du = -d2 * ((u0 - uu) / (get(&up, p + 1, OP)? - u0));
            uu = u0;
            for jj in 0..=m {
                let p1 = g2(&pw, 0, jj, OP)?.to_euclid();
                let p2 = g2(&pw, 1, jj, OP)?.to_euclid();
                let v = p2 - p1;
                let p3 = pt_along_vector(p1, du, v);
                let w = g2(&pw, 1, jj, OP)?.w;
                s2(&mut qw, 0, jj, CPoint::weighted(p3, w), OP)?;
            }
            // Compute Qw[1][jj],...,Qw[p-1][jj] (degree elevation).
            for jj in 0..=m {
                for ii in 1..p {
                    let d1 = ii as f64 / d2;
                    let c =
                        CPoint::combine2(1.0 - d1, g2(&qw, 0, jj, OP)?, d1, g2(&pw, 0, jj, OP)?);
                    s2(&mut qw, ii, jj, c, OP)?;
                }
            }
        } else {
            for ii in 0..=p {
                set(&mut uq, ii + r, uu, OP)?;
            }
            for ii in 0..r {
                set(&mut uq, ii, get(&up, ii, OP)?, OP)?;
            }
            for ii in 0..=n {
                for jj in 0..=m {
                    s2(&mut qw, ii, jj, g2(&pw, ii, jj, OP)?, OP)?;
                }
            }
            // Compute new Qw[nq][jj], i.e. boundary control points.
            let du = d2 * ((uu - ur) / (ur - get(&up, r - p - 1, OP)?));
            uu = ur;
            for jj in 0..=m {
                let p1 = g2(&pw, n - 1, jj, OP)?.to_euclid();
                let p2 = g2(&pw, n, jj, OP)?.to_euclid();
                let v = p2 - p1;
                let p3 = pt_along_vector(p2, du, v);
                let w = g2(&pw, n - 1, jj, OP)?.w;
                s2(&mut qw, nq, jj, CPoint::weighted(p3, w), OP)?;
            }
            // Compute Qw[n+1][jj],...,Qw[nq-1][jj] (degree elevation).
            for jj in 0..=m {
                for ii in 1..p {
                    let d1 = ii as f64 / d2;
                    let c =
                        CPoint::combine2(1.0 - d1, g2(&qw, n, jj, OP)?, d1, g2(&qw, nq, jj, OP)?);
                    s2(&mut qw, n + ii, jj, c, OP)?;
                }
            }
        }
        let qs = reorient(Surface::new(qw, idx.p, idx.q, uq, vq)?, udir);
        // Try to remove one knot from the old end.
        return Ok(remove_knot_conditional(&qs, uu, 1, MODEL_TOL, real_dir)?.surface);
    }

    // Insert knot into surB, load UQ and VQ, partially load Qw, and load Rw with the
    // required control points from surB.
    let mut rem: isize = 0;
    let uuu = if cont != Cmax {
        uu
    } else if start {
        get(&up, p + 1, OP)?
    } else {
        get(&up, r - p - 1, OP)?
    };
    let mut surb = a.clone();
    if mult < p {
        // Make it a fully multiple knot using knot insertion (in the real orientation).
        rem = p - mult;
        let real = reorient(surb, udir);
        surb = reorient(insert_knot(&real, uuu, ux(rem, OP)?, real_dir)?, udir);
    }
    for ii in 0..=s {
        set(&mut vq, ii, get(&vp, ii, OP)?, OP)?;
    }
    let bidx = surb.indices();
    let (nb, mb, rb) = (sx(bidx.n, OP)?, sx(bidx.m, OP)?, sx(bidx.r, OP)?);
    let bw = surb.cpts();
    let ub = surb.u_knots().to_vec();

    let kby: isize;
    let kkt: isize;
    if start {
        // kby = number of knots inserted to make uuu fully multiple.
        kby = nq - nb;
        for ii in 0..=mb {
            for jj in 0..=nb {
                s2(&mut qw, kby + jj, ii, g2(&bw, jj, ii, OP)?, OP)?;
            }
            for jj in 0..=kby {
                s2(&mut rw, ii, jj + kby, g2(&bw, jj, ii, OP)?, OP)?;
            }
        }
        if cont == Cmax {
            kkt = p + 1;
            let x = 2.0 * get(&ub, 0, OP)? - uu;
            for ii in 0..=p {
                set(&mut uq, ii, x, OP)?;
            }
            for ii in 1..=rb {
                set(&mut uq, p + ii, get(&ub, ii, OP)?, OP)?;
            }
        } else {
            kkt = span + 1;
            let mut jj = kkt - 1;
            for ii in 1..=rb {
                set(&mut uq, span + ii, get(&ub, ii, OP)?, OP)?;
            }
            // Mirror UB knots between old end and uuu across old end boundary.
            for ii in (p + 1)..=span {
                let x = get(&uq, jj + 1, OP)? - (get(&ub, ii, OP)? - get(&ub, ii - 1, OP)?);
                set(&mut uq, jj, x, OP)?;
                jj -= 1;
            }
            // Clamp UQ end to par value (d1 should equal par).
            let d1 = get(&uq, jj + 1, OP)? - (get(&ub, kkt, OP)? - get(&ub, span, OP)?);
            for ii in 0..=p {
                set(&mut uq, ii, d1, OP)?;
            }
        }
    } else {
        kby = nq - nb;
        kk = nb - kby;
        for ii in 0..=mb {
            for jj in 0..=nb {
                s2(&mut qw, jj, ii, g2(&bw, jj, ii, OP)?, OP)?;
            }
            for jj in 0..=kby {
                s2(&mut rw, ii, jj, g2(&bw, jj + kk, ii, OP)?, OP)?;
            }
        }
        kkt = rb - 1;
        for ii in 0..rb {
            set(&mut uq, ii, get(&ub, ii, OP)?, OP)?;
        }
        if cont == Cmax {
            let x = 2.0 * get(&ub, rb, OP)? - uu;
            for ii in 0..=p {
                set(&mut uq, rb + ii, x, OP)?;
            }
        } else {
            let mut jj = rb;
            let mut ii = r - p - 1;
            while ii > span {
                let x = get(&uq, jj - 1, OP)? + (get(&up, ii + 1, OP)? - get(&up, ii, OP)?);
                set(&mut uq, jj, x, OP)?;
                jj += 1;
                ii -= 1;
            }
            let d1 = get(&uq, jj - 1, OP)? + (get(&up, span + 1, OP)? - uu);
            for ii in 0..=p {
                set(&mut uq, jj + ii, d1, OP)?;
            }
        }
    }

    // Compute the new control points defining the extension (Shetty & White, CAD,
    // Sept. 1991 for G1 and G2; derivative expressions for C-max).
    let nn = mq;
    let pq = p;
    let pqf = pq as f64;

    if cont == Cmax {
        cmax_points(&mut rw, &ub, uu, start, nn, kby, p, rb, rat)?;
    } else if start {
        for ii in 0..=nn {
            // Get reflection plane.
            let p1 = g2(&rw, ii, kby, OP)?.to_euclid();
            let p2 = g2(&rw, ii, kby + 1, OP)?.to_euclid();
            let (v, _) =
                normalize(p1 - p2).map_err(|_| SolidError::nlib(OP, NlibCode::Geometry))?;
            let pln = Plane::new(p1, v);
            let mut k = kby - 1;
            for jj in 1..=kby {
                let c = cpt_reflect(g2(&rw, ii, kby + jj, OP)?, pln)?;
                s2(&mut rw, ii, k, c, OP)?;
                k -= 1;
            }
        }

        if !rat && cont == G2 && pq > 1 {
            // Recompute second row using C2 condition.
            let d1 = 2.0
                + (get(&ub, p + 2, OP)? - get(&ub, p + 1, OP)?)
                    / (get(&ub, p + 1, OP)? - get(&ub, 0, OP)?);
            for ii in 0..=nn {
                let c = CPoint::translate_sum2(
                    g2(&rw, ii, kby + 2, OP)?,
                    d1,
                    g2(&rw, ii, kby - 1, OP)?,
                    -d1,
                    g2(&rw, ii, kby + 1, OP)?,
                );
                s2(&mut rw, ii, kby - 2, c, OP)?;
            }
        }

        if rat {
            let d1 = 2.0 * pf / (get(&ub, p + 1, OP)? - get(&ub, 0, OP)?);
            let (tal, ktal) = max_weight_ratio(&rw, nn, kby, kby + 1, d1)?;
            let g2c = if cont == G2 && pq > 1 {
                let d3 = get(&ub, p + 1, OP)? - get(&ub, 0, OP)?;
                let d4 = get(&ub, p + 2, OP)? - get(&ub, p + 1, OP)?;
                Some(shetty_white(&rw, ktal, kby, pf, d3, d4, tal)?)
            } else {
                None
            };
            let d1 = 2.0 + tal * (get(&uq, kkt, OP)? - get(&uq, kkt - 1, OP)?) / pqf;
            for ii in 0..=nn {
                // Recompute first row for G1.
                let c =
                    CPoint::combine2(d1, g2(&rw, ii, kby, OP)?, -1.0, g2(&rw, ii, kby + 1, OP)?);
                s2(&mut rw, ii, kby - 1, c, OP)?;
            }
            if let Some(sw) = g2c {
                // Recompute second row for G2.
                let d2 = 1.0 / sw.d2;
                let d1 = d1 * (sw.sol0 + 2.0 * tal);
                for ii in 0..=nn {
                    let h1 = g2(&rw, ii, kby - 1, OP)?.to_array();
                    let h2 = g2(&rw, ii, kby, OP)?.to_array();
                    let h3 = g2(&rw, ii, kby + 1, OP)?.to_array();
                    let h4 = g2(&rw, ii, kby + 2, OP)?.to_array();
                    let h0 = map4(h1, h2, h3, h4, |a1, a2, a3, a4| {
                        a4 + sw.d4 * (a1 - a3) + d2 * (d1 * (a2 - a3) + sw.sol1 * a2)
                    });
                    s2(&mut rw, ii, kby - 2, h0, OP)?;
                }
            }
        }
    } else {
        for ii in 0..=nn {
            // Get reflection plane.
            let p1 = g2(&rw, ii, kby, OP)?.to_euclid();
            let p2 = g2(&rw, ii, kby - 1, OP)?.to_euclid();
            let (v, _) =
                normalize(p1 - p2).map_err(|_| SolidError::nlib(OP, NlibCode::Geometry))?;
            let pln = Plane::new(p1, v);
            let mut k = 2 * kby;
            for jj in 0..kby {
                let c = cpt_reflect(g2(&rw, ii, jj, OP)?, pln)?;
                s2(&mut rw, ii, k, c, OP)?;
                k -= 1;
            }
        }

        if !rat && cont == G2 && pq > 1 {
            // Recompute second row using C2 condition.
            let d1 = 2.0
                + (get(&ub, rb - p - 1, OP)? - get(&ub, rb - p - 2, OP)?)
                    / (get(&ub, rb, OP)? - get(&ub, rb - p - 1, OP)?);
            for ii in 0..=nn {
                let c = CPoint::translate_sum2(
                    g2(&rw, ii, kby - 2, OP)?,
                    d1,
                    g2(&rw, ii, kby + 1, OP)?,
                    -d1,
                    g2(&rw, ii, kby - 1, OP)?,
                );
                s2(&mut rw, ii, kby + 2, c, OP)?;
            }
        }

        if rat {
            let d1 = 2.0 * pf / (get(&ub, rb, OP)? - get(&ub, rb - p - 1, OP)?);
            let (tal, ktal) = max_weight_ratio(&rw, nn, kby, kby - 1, d1)?;
            let g2c = if cont == G2 && pq > 1 {
                let d3 = get(&ub, rb, OP)? - get(&ub, rb - p - 1, OP)?;
                let d4 = get(&ub, rb - p - 1, OP)? - get(&ub, rb - p - 2, OP)?;
                Some(shetty_white(&rw, ktal, kby, pf, d3, d4, tal)?)
            } else {
                None
            };
            let d1 = 2.0 + tal * (get(&uq, kkt + 1, OP)? - get(&uq, kkt, OP)?) / pqf;
            for ii in 0..=nn {
                // Recompute first row for G1.
                let c =
                    CPoint::combine2(d1, g2(&rw, ii, kby, OP)?, -1.0, g2(&rw, ii, kby - 1, OP)?);
                s2(&mut rw, ii, kby + 1, c, OP)?;
            }
            if let Some(sw) = g2c {
                // Recompute second row for G2.
                let d2 = 1.0 / sw.d2;
                let d1 = d1 * (sw.sol0 + 2.0 * tal);
                for ii in 0..=nn {
                    let h0 = g2(&rw, ii, kby - 2, OP)?.to_array();
                    let h1 = g2(&rw, ii, kby - 1, OP)?.to_array();
                    let h2 = g2(&rw, ii, kby, OP)?.to_array();
                    let h3 = g2(&rw, ii, kby + 1, OP)?.to_array();
                    let h4 = map4(h0, h1, h2, h3, |a0, a1, a2, a3| {
                        a0 - sw.d4 * (a1 - a3) + d2 * (d1 * (a2 - a3) + sw.sol1 * a2)
                    });
                    s2(&mut rw, ii, kby + 2, h4, OP)?;
                }
            }
        }
    }

    // Now load the control points from Rw into Qw.
    if start {
        for ii in 0..=nn {
            for jj in 0..kby {
                s2(&mut qw, jj, ii, g2(&rw, ii, jj, OP)?, OP)?;
            }
        }
    } else {
        for ii in 0..=nn {
            for jj in 1..=kby {
                s2(&mut qw, nb + jj, ii, g2(&rw, ii, kby + jj, OP)?, OP)?;
            }
        }
    }

    // Remove unnecessary knots (in the real orientation).
    let mut res = reorient(Surface::new(qw, idx.p, idx.q, uq, vq)?, udir);
    if rem > 0 {
        res =
            remove_knot_conditional(&res, uuu, ux(rem, OP)?, 100.0 * MODEL_TOL, real_dir)?.surface;
    }
    let t_end = if start { u0 } else { ur };
    let rem = match cont {
        G1 => 1,
        G2 => 2,
        _ => p,
    };
    Ok(remove_knot_conditional(&res, t_end, ux(rem, OP)?, MODEL_TOL, real_dir)?.surface)
}

/// The largest `d1 * (w1 - w0) / w0` over the rows (`w0`, `w1`: weights of
/// `Rw[ii][c0]`, `Rw[ii][c1]`) and its row: `(tal, ktal)`; `tal` starts at `-1e20`,
/// `ktal` at `0`.
fn max_weight_ratio(
    rw: &[Vec<CPoint>],
    nn: isize,
    c0: isize,
    c1: isize,
    d1: f64,
) -> Result<(f64, isize)> {
    const OP: &str = OP_EXTEND;
    let mut tal = -1.0e+20;
    let mut ktal: isize = 0;
    for ii in 0..=nn {
        let w0 = g2(rw, ii, c0, OP)?.w;
        let w1 = g2(rw, ii, c1, OP)?.w;
        let d2 = d1 * (w1 - w0) / w0;
        if d2 > tal {
            tal = d2;
            ktal = ii;
        }
    }
    Ok((tal, ktal))
}

/// `CPoint` whose coordinates are `f` of the corresponding coordinates of `a..d`.
fn map4(
    a: [f64; 4],
    b: [f64; 4],
    c: [f64; 4],
    d: [f64; 4],
    f: impl Fn(f64, f64, f64, f64) -> f64,
) -> CPoint {
    let [a0, a1, a2, a3] = a;
    let [b0, b1, b2, b3] = b;
    let [c0, c1, c2, c3] = c;
    let [d0, d1, d2, d3] = d;
    CPoint::new(
        f(a0, b0, c0, d0),
        f(a1, b1, c1, d1),
        f(a2, b2, c2, d2),
        f(a3, b3, c3, d3),
    )
}

/// Shetty & White's `theta`, `e0` for the rational G2 extension: the least-squares
/// solution and the derived constants upstream keeps in `d2`, `d4`.
struct ShettyWhite {
    d2: f64,
    d4: f64,
    sol0: f64,
    sol1: f64,
}

/// Sets up and solves the 4x2 system of the rational G2 extension (row `ktal`,
/// columns `kby-2..=kby+2`); `d3`, `d4` are upstream's span lengths.
fn shetty_white(
    rw: &[Vec<CPoint>],
    ktal: isize,
    kby: isize,
    pf: f64,
    d3: f64,
    d4: f64,
    tal: f64,
) -> Result<ShettyWhite> {
    const OP: &str = OP_EXTEND;
    let mut rma = RealMatrix::new(3, 1, MatrixType::Full, 3)?;
    let d1 = pf / d3;
    let d2 = (pf * (pf - 1.0)) / (d3 * (d3 + d4));
    let d4 = (2.0 * d3 + d4) / d3;
    let d3 = -2.0 * tal * d1;
    let row = |c: isize| -> Result<[f64; 4]> { Ok(g2(rw, ktal, kby - 2 + c, OP)?.to_array()) };
    let (h0, h1, h2, h3, h4) = (row(0)?, row(1)?, row(2)?, row(3)?, row(4)?);
    let mut rhs = Vec::with_capacity(4);
    let cols = h0.iter().zip(h1).zip(h2).zip(h3).zip(h4);
    for (ii, ((((&a0, a1), a2), a3), a4)) in cols.enumerate() {
        rma.set(ii, 0, d1 * (a2 - a3))?;
        rma.set(ii, 1, a2)?;
        rhs.push(d2 * (d4 * (a3 - a1) + (a0 - a4)) + d3 * (a2 - a3));
    }
    let sol = rma.lst_sq_solve(&rhs)?;
    Ok(ShettyWhite {
        d2,
        d4,
        sol0: at(&sol, 0, OP)?,
        sol1: at(&sol, 1, OP)?,
    })
}

/// The C-max extension control points: derivatives of the boundary span at the old
/// end, re-expanded at the new end (`NL_PascalTri` binomials).
#[allow(clippy::too_many_arguments)]
fn cmax_points(
    rw: &mut [Vec<CPoint>],
    ub: &[f64],
    uu: f64,
    start: bool,
    nn: isize,
    kby: isize,
    p: isize,
    rb: isize,
    rat: bool,
) -> Result<()> {
    const OP: &str = OP_EXTEND;
    let pq = p;
    let pqf = pq as f64;
    let mut dw: Vec<CPoint> = storage(pq, OP)?;
    let pascal = |j: isize, k: isize| -> Result<f64> {
        let row = PASCAL_TRI.get(ux(j, OP)?).ok_or_else(|| oob(OP))?;
        Ok(get(row, k, OP)? as f64)
    };

    if start {
        let d2 = uu - get(ub, 0, OP)?;
        let d3 = get(ub, p + 1, OP)? - get(ub, 0, OP)?;
        let d4 = 1.0 / d3;
        for ii in 0..=nn {
            let mut dr = pqf * d4;
            for jj in 1..=pq {
                // Compute the jj-th derivative from the right.
                let mut d = g2(rw, ii, kby + jj, OP)?;
                let mut kp: i64 = if jj % 2 == 0 { 1 } else { -1 };
                for kk in 0..jj {
                    d.blend(kp as f64 * pascal(jj, kk)?, g2(rw, ii, kby + kk, OP)?);
                    kp *= -1;
                }
                set(&mut dw, jj, d.scale(dr), OP)?;
                dr = dr * (pqf - jj as f64) * d4;
            }
            let mut dl = d2 / pqf;
            for jj in 1..=pq {
                // Compute new control points.
                let mut kp: i64 = if jj % 2 == 0 { -1 } else { 1 };
                let mut c = get(&dw, jj, OP)?.scale(-kp as f64 * dl);
                if jj < pq {
                    dl = dl * d2 / (pqf - jj as f64);
                }
                for kk in 0..jj {
                    c.blend(kp as f64 * pascal(jj, kk)?, g2(rw, ii, pq - kk, OP)?);
                    kp *= -1;
                }
                s2(rw, ii, pq - jj, c, OP)?;
            }
            if rat {
                // Don't create unacceptable weights.
                for jj in 0..pq {
                    let w = g2(rw, ii, jj, OP)?.w;
                    if w < WEIGHT_MIN || w > WEIGHT_MAX {
                        return Err(SolidError::nlib(OP, NlibCode::Weight));
                    }
                }
            }
        }
    } else {
        let d2 = get(ub, rb, OP)? - uu;
        let d3 = get(ub, rb, OP)? - get(ub, rb - p - 1, OP)?;
        let d4 = 1.0 / d3;
        for ii in 0..=nn {
            let mut dl = -pqf * d4;
            for jj in 1..=pq {
                // Compute the jj-th derivative from the left.
                let mut d = g2(rw, ii, kby - jj, OP)?;
                let mut kp: i64 = if jj % 2 == 0 { 1 } else { -1 };
                for kk in 0..jj {
                    d.blend(kp as f64 * pascal(jj, kk)?, g2(rw, ii, kby - kk, OP)?);
                    kp *= -1;
                }
                set(&mut dw, jj, d.scale(dl), OP)?;
                dl = -dl * (pqf - jj as f64) * d4;
            }
            let mut dr = d2 / pqf;
            for jj in 1..=pq {
                // Compute new control points.
                let mut kp: i64 = if jj % 2 == 0 { -1 } else { 1 };
                let mut c = get(&dw, jj, OP)?.scale(dr);
                if jj < pq {
                    dr = dr * d2 / (pqf - jj as f64);
                }
                for kk in 0..jj {
                    c.blend(kp as f64 * pascal(jj, kk)?, g2(rw, ii, kby + kk, OP)?);
                    kp *= -1;
                }
                s2(rw, ii, kby + jj, c, OP)?;
            }
            if rat {
                // Don't create unacceptable weights.
                for jj in 1..=pq {
                    let w = g2(rw, ii, kby + jj, OP)?.w;
                    if w < WEIGHT_MIN || w > WEIGHT_MAX {
                        return Err(SolidError::nlib(OP, NlibCode::Weight));
                    }
                }
            }
        }
    }
    Ok(())
}

// ---- swung and translational sweep surfaces -----------------------------------------

/// `Qw` from a Euclidean point and the two weights of the generating control points
/// (`N_PtToCPt` / `N_Weight` by which weights are `NL_NOW`).
fn combined_weight(q: Vec3, wa: f64, wb: f64) -> CPoint {
    if wa == NOW && wb == NOW {
        CPoint::from_point(q)
    } else if wa == NOW && wb != NOW {
        CPoint::weighted(q, wb)
    } else if wa != NOW && wb == NOW {
        CPoint::weighted(q, wa)
    } else {
        CPoint::weighted(q, wa * wb)
    }
}

/// Swung surface of a profile curve in the `[x, z]` plane and a trajectory curve in
/// the `[x, y]` plane, scaled by `alf` (upstream `N_CreateSwungSrf`):
/// `Q[i][j] = (alf * xp * xt, alf * xp * yt, zp)`, weighted by the product of the
/// weights. The u direction is the profile, the v direction the trajectory.
///
/// # Errors
///
/// `InvalidInput` for curves without control points or knots.
pub fn create_swung_srf(cur_p: &Curve, cur_t: &Curve, alf: f64) -> Result<Surface> {
    const OP: &str = OP_SWUNG;
    let pw = cur_p.cpts();
    let tw = cur_t.cpts();
    let mut qw: Net = Vec::with_capacity(pw.len());
    for pc in pw {
        let pp = pc.to_euclid();
        let wp = pc.w;
        let mut row = Vec::with_capacity(tw.len());
        for tc in tw {
            let t = tc.to_euclid();
            let wt = tc.w;
            let xq = alf * pp.x * t.x;
            let yq = alf * pp.x * t.y;
            let zq = pp.z;
            row.push(combined_weight(Vec3::new(xq, yq, zq), wp, wt));
        }
        qw.push(row);
    }
    if qw.is_empty() || tw.is_empty() {
        return Err(oob(OP));
    }
    Surface::new(
        qw,
        cur_p.degree(),
        cur_t.degree(),
        cur_p.knots().to_vec(),
        cur_t.knots().to_vec(),
    )
}

/// Translational sweep surface of a section curve (u direction) along a trajectory
/// curve (v direction) (upstream `N_CreateTransSweepSrf`):
/// `P[i][j] = S[i] + T[j]`, weighted by the product of the weights.
///
/// # Errors
///
/// `InvalidInput` for curves without control points or knots.
pub fn create_trans_sweep_srf(cur_t: &Curve, cur_s: &Curve) -> Result<Surface> {
    const OP: &str = OP_SWEEP;
    let sw = cur_s.cpts();
    let tw = cur_t.cpts();
    let mut pw: Net = Vec::with_capacity(sw.len());
    for sc in sw {
        let s = sc.to_euclid();
        let ws = sc.w;
        let mut row = Vec::with_capacity(tw.len());
        for tc in tw {
            let t = tc.to_euclid();
            let wt = tc.w;
            row.push(combined_weight(s + t, ws, wt));
        }
        pw.push(row);
    }
    if pw.is_empty() || tw.is_empty() {
        return Err(oob(OP));
    }
    Surface::new(
        pw,
        cur_s.degree(),
        cur_t.degree(),
        cur_s.knots().to_vec(),
        cur_t.knots().to_vec(),
    )
}

// ---- N_CreateSkinSrf ----------------------------------------------------------------

/// Skinned surface through the cross-sectional curves `cur[0..=k]` (upstream
/// `N_CreateSkinSrf`), interpolating corresponding control points with degree `deg`
/// at chord-length parameters.
///
/// With `cmp` the curves are first made compatible in place
/// ([`make_compatible_knot_tol`], `knot_tol = 0` lets it choose). `cur[0]` and `cur[k]`
/// are then made compatible with each other ([`curves_are_coincident`], also in
/// place); when they coincide and `cld` is set the surface is closed with C1
/// continuity. With [`Direction::U`] the curves are v-curves (skinning in u), with
/// [`Direction::V`] u-curves.
///
/// # Errors
///
/// [`NlibCode::Call`] for [`Direction::UV`]; [`NlibCode::Weight`] when a weight of the
/// surface is outside `[WEIGHT_MIN, WEIGHT_MAX]`; errors of the compatibility,
/// parametrisation and interpolation routines; `InvalidInput` for an empty curve set,
/// a closed skin of one curve, or curves of different sizes.
#[allow(clippy::too_many_lines)]
pub fn create_skin_srf(
    cur: &mut [Curve],
    cmp: bool,
    cld: bool,
    deg: usize,
    dir: Direction,
    knot_tol: f64,
) -> Result<Surface> {
    const OP: &str = OP_SKIN;
    let k = cur.len().checked_sub(1).ok_or_else(|| oob(OP))?;
    let ki = sx(k, OP)?;

    // Make curves compatible.
    if cmp {
        make_compatible_knot_tol(cur, knot_tol)?;
    }

    // Check if curve set is closed.
    let coincident = if k == 0 {
        let first = cur.first_mut().ok_or_else(|| oob(OP))?;
        let mut a = first.clone();
        let mut b = first.clone();
        let res = curves_are_coincident(&mut a, &mut b, MODEL_TOL)?;
        *first = a;
        res
    } else {
        let (head, tail) = cur.split_at_mut(k);
        let c0 = head.first_mut().ok_or_else(|| oob(OP))?;
        let ck = tail.first_mut().ok_or_else(|| oob(OP))?;
        curves_are_coincident(c0, ck, MODEL_TOL)?
    };
    let closed = coincident && cld;

    let udir = match dir {
        Direction::U => true,
        Direction::V => false,
        Direction::UV => return Err(SolidError::nlib(OP, NlibCode::Call)),
    };

    // Get indexes and control point data (the cross-section direction is "c": its
    // highest index, degree, knot index and knots come from cur[0]).
    let c0 = cur.first().ok_or_else(|| oob(OP))?;
    let mc = sx(c0.n(), OP)?;
    let deg_c = c0.degree();
    let knots_c = c0.knots().to_vec();
    let ns = if closed { ki + 2 } else { ki };
    let ps = sx(deg, OP)?;

    // Rw[i][j]: control point j of curve i (skinning-major).
    let mut rw: Net = net(ki, mc, OP)?;
    for i in 0..=ki {
        let pw = cur.get(ux(i, OP)?).ok_or_else(|| oob(OP))?.cpts();
        for j in 0..=mc {
            s2(&mut rw, i, j, get(pw, j, OP)?, OP)?;
        }
    }
    let sw: Net = if closed {
        let mut sw = net(ki, mc, OP)?;
        let nh = ki / 2;
        for i in 0..=ki {
            let mut l = (nh + i).checked_rem(ki).ok_or(SolidError::InvalidInput {
                operation: OP,
                reason: "closed skin of a single curve (upstream divides by zero)",
            })?;
            if l == 0 {
                l = ki;
            }
            for j in 0..=mc {
                s2(&mut sw, i, j, g2(&rw, l, j, OP)?, OP)?;
            }
        }
        sw
    } else {
        Vec::new()
    };

    // Get parameters. Upstream computes them on Rw as stored: [k][m] for the u
    // direction, [n][k] for the v direction (transposed).
    let transpose = |w: &Net| -> Net {
        let cols = w.first().map_or(0, Vec::len);
        (0..cols)
            .map(|j| w.iter().filter_map(|row| row.get(j).copied()).collect())
            .collect()
    };
    let sk = if udir {
        param_values(&rw, ParamMethod::ChordLength)?.0
    } else {
        param_values(&transpose(&rw), ParamMethod::ChordLength)?.1
    };
    let skc = if closed {
        if udir {
            Some(param_values(&sw, ParamMethod::ChordLength)?.0)
        } else {
            Some(param_values(&transpose(&sw), ParamMethod::ChordLength)?.1)
        }
    } else {
        None
    };

    // Compute surface control points (rows of the skinning direction).
    let rs = ns
        .checked_add(ps)
        .and_then(|x| x.checked_add(1))
        .ok_or_else(|| oob(OP))?;
    let kn_s: Vec<f64>;
    let knt: Option<Vec<f64>>;
    if let Some(uc) = &skc {
        knt = Some(interp_knot_vector(uc, deg)?);
        kn_s = knot_vector_end_derivs(&sk, deg)?;
    } else {
        knt = None;
        kn_s = interp_knot_vector(&sk, deg)?;
    }
    if sx(kn_s.len(), OP)? - 1 != rs {
        return Err(oob(OP));
    }
    let kv_s = KnotVector::from_slice(&kn_s)?;

    // qs[j][i]: column j of the cross-section direction, point i of the skin direction.
    let mut qs: Net = net(mc, ns, OP)?;
    let mut aw: Vec<CPoint> = storage(ki, OP)?;
    for j in 0..=mc {
        for i in 0..=ki {
            let pw = cur.get(ux(i, OP)?).ok_or_else(|| oob(OP))?.cpts();
            set(&mut aw, i, get(pw, j, OP)?, OP)?;
        }
        let cur_a = match (&skc, &knt) {
            (Some(uc), Some(kt)) => {
                let mut cw: Vec<CPoint> = storage(ki, OP)?;
                for i in 0..=ki {
                    set(&mut cw, i, g2(&sw, i, j, OP)?, OP)?;
                }
                let kvt = KnotVector::from_slice(kt)?;
                let tmp = interp_given_params(&cw, uc, &kvt, deg)?;
                let l = (ki + 1) / 2;
                let dw = tmp.derivs_at_knot(get(uc, l, OP)?, Side::Left, 1)?;
                let d1 = at(&dw, 1, OP)?;
                interp_knots_and_derivs(&aw, &sk, &kv_s, deg, d1, d1)?
            }
            _ => interp_given_params(&aw, &sk, &kv_s, deg)?,
        };
        let bw = cur_a.cpts();
        for i in 0..=ns {
            s2(&mut qs, j, i, get(bw, i, OP)?, OP)?;
        }
    }

    let sur = if udir {
        Surface::new(transpose(&qs), deg, deg_c, kn_s, knots_c)?
    } else {
        Surface::new(qs, deg_c, deg, knots_c, kn_s)?
    };

    // Check the weights.
    sur.weights_are_valid()?;
    Ok(sur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::knots::Side;

    fn plane_srf(rational: bool) -> Surface {
        let w = if rational { 1.0 } else { NOW };
        let net: Net = (0..4)
            .map(|i| {
                (0..3)
                    .map(|j| {
                        let (x, y, z) = (f64::from(i), f64::from(j), 0.1 * f64::from(i * j));
                        if rational {
                            CPoint::new(x * w, y * w, z * w, w)
                        } else {
                            CPoint::new(x, y, z, NOW)
                        }
                    })
                    .collect()
            })
            .collect();
        Surface::new(
            net,
            2,
            2,
            vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .unwrap()
    }

    #[test]
    fn grid_matches_surface_eval() {
        for rational in [false, true] {
            let s = plane_srf(rational);
            let u = [0.0, 0.3, 0.5, 1.0];
            let v = [0.0, 0.25, 1.0];
            let g = grid_pts_normals(&s, &u, &v, Side::Left, Side::Left).unwrap();
            assert_eq!(g.points.len(), u.len());
            assert_eq!(g.normals.len(), u.len());
            for (a, &uu) in u.iter().enumerate() {
                assert_eq!(g.points[a].len(), v.len());
                assert_eq!(g.normals[a].len(), v.len());
                for (b, &vv) in v.iter().enumerate() {
                    let p = s.eval(uu, vv, Side::Left, Side::Left).unwrap();
                    assert!((g.points[a][b] - p).length() < 1e-12);
                    assert!((g.normals[a][b].length() - 1.0).abs() < 1e-12);
                }
            }
        }
    }

    #[test]
    fn grid_rejects_bad_params() {
        let s = plane_srf(false);
        for (u, v) in [
            (&[][..], &[0.0][..]),
            (&[0.0][..], &[][..]),
            (&[][..], &[][..]),
            (&[][..], &[1.5][..]),
            (&[-0.1][..], &[][..]),
        ] {
            assert_eq!(
                grid_pts_normals(&s, u, v, Side::Left, Side::Left).unwrap_err(),
                SolidError::nlib(OP_GRID, NlibCode::Index)
            );
        }
        for (u, v) in [
            (&[-0.1][..], &[0.0][..]),
            (&[1.5][..], &[0.0][..]),
            (&[0.0][..], &[-0.1][..]),
            (&[0.0][..], &[1.5][..]),
            (&[-0.1][..], &[1.5][..]),
            (&[-0.1][..], &[0.0, 1.0, 1.0][..]),
        ] {
            assert_eq!(
                grid_pts_normals(&s, u, v, Side::Left, Side::Left).unwrap_err(),
                SolidError::nlib(OP_GRID, NlibCode::Parameter)
            );
        }
    }

    #[test]
    fn grid_output_budget_boundaries_and_overflow() {
        let limit = crate::limits::MAX_ALLOC_BYTES;
        let point = size_of::<Vec3>();
        let row = size_of::<Vec<Vec3>>();
        assert_eq!(grid_output_bytes(4, 3), Some(2 * (4 * 3 * point + 4 * row)));
        assert_ne!(grid_output_bytes(4, 3), grid_output_bytes(3, 4));
        let cols = (limit / 2 - row) / point;
        assert!(admit_grid_output(grid_output_bytes(1, cols)).is_ok());
        let memory = SolidError::nlib(OP_GRID, NlibCode::Memory);
        assert_eq!(
            admit_grid_output(grid_output_bytes(1, cols + 1)),
            Err(memory.clone())
        );
        assert!(admit_grid_output(Some(limit)).is_ok());
        assert_eq!(admit_grid_output(Some(limit + 1)), Err(memory.clone()));
        // Cell product, payload, metadata, sum, and final doubling overflow.
        for (rows, cols) in [
            (usize::MAX, 2),
            (1, usize::MAX / point + 1),
            (usize::MAX / row + 1, 0),
            (1, usize::MAX / point),
            (1, usize::MAX / point / 2 + 1),
            (usize::MAX, usize::MAX),
        ] {
            assert_eq!(grid_output_bytes(rows, cols), None, "{rows} x {cols}");
            assert_eq!(
                admit_grid_output(grid_output_bytes(rows, cols)),
                Err(memory.clone())
            );
        }
    }

    #[test]
    fn grid_preserves_basis_error_precedence() {
        let s = plane_srf(false);
        let (ku, kv) = s.knot_vectors().unwrap();
        let bad_u = [0.0, 1.0, 1.0];
        let bad_v = [0.25, 1.0, 1.0];
        let eu = basis_derivs_array(&ku, s.indices().p, &bad_u, Side::Left, 1).unwrap_err();
        let ev = basis_derivs_array(&kv, s.indices().q, &bad_v, Side::Left, 1).unwrap_err();
        for (u, v, error) in [
            (&bad_u[..], &[0.25][..], &eu),
            (&[0.25][..], &bad_v[..], &ev),
            (&bad_u[..], &bad_v[..], &eu),
        ] {
            assert_eq!(
                grid_pts_normals(&s, u, v, Side::Left, Side::Left).unwrap_err(),
                *error
            );
        }
    }

    /// Small inputs and linear basis workspace only: the output guard must be in
    /// place before this regression is run. Never run against the old allocator.
    #[test]
    fn grid_rejects_moderate_over_budget_output_after_basis() {
        let mut dim = 1usize;
        while admit_grid_output(grid_output_bytes(dim, dim)).is_ok() {
            dim += 1;
        }
        assert!(admit_grid_output(grid_output_bytes(dim - 1, dim - 1)).is_ok());
        assert!(dim < 10_000, "keep the parameter/basis inputs bounded");
        let valid: Vec<f64> = (0..dim).map(|i| (i as f64 + 0.5) / dim as f64).collect();
        let s = plane_srf(false);
        assert_eq!(
            grid_pts_normals(&s, &valid, &valid, Side::Left, Side::Left).unwrap_err(),
            SolidError::nlib(OP_GRID, NlibCode::Memory)
        );
        // A basis-invalid inner domain endpoint wins over the output budget in
        // either direction, and the u basis still runs first when both fail.
        let mut bad = valid.clone();
        bad[dim - 2] = 1.0;
        bad[dim - 1] = 1.0;
        let (ku, kv) = s.knot_vectors().unwrap();
        let eu = basis_derivs_array(&ku, s.indices().p, &bad, Side::Left, 1).unwrap_err();
        let ev = basis_derivs_array(&kv, s.indices().q, &bad, Side::Left, 1).unwrap_err();
        for (u, v, error) in [(&bad, &valid, &eu), (&valid, &bad, &ev), (&bad, &bad, &eu)] {
            assert_eq!(
                grid_pts_normals(&s, u, v, Side::Left, Side::Left).unwrap_err(),
                *error
            );
        }
    }

    #[test]
    fn extensions_keep_old_domain() {
        for rational in [false, true] {
            let s = plane_srf(rational);
            for cont in [
                ExtendContinuity::G1,
                ExtendContinuity::G2,
                ExtendContinuity::Cmax,
                ExtendContinuity::G1R,
            ] {
                for (dir, end, par) in [
                    (Direction::U, CurveEnd::Start, -0.2),
                    (Direction::U, CurveEnd::End, 1.3),
                    (Direction::V, CurveEnd::Start, -0.4),
                    (Direction::V, CurveEnd::End, 1.1),
                ] {
                    let e = extend_by_param_dist(&s, par, dir, end, cont).unwrap();
                    let b = e.parameter_bounds();
                    let lim = match (dir, end) {
                        (Direction::U, CurveEnd::Start) => b.ul,
                        (Direction::U, _) => b.ur,
                        (_, CurveEnd::Start) => b.vb,
                        _ => b.vt,
                    };
                    assert!((lim - par).abs() < 1e-9, "{cont:?} {dir:?} {end:?} {lim}");
                    let p0 = s.eval(0.3, 0.6, Side::Left, Side::Left).unwrap();
                    let p1 = e.eval(0.3, 0.6, Side::Left, Side::Left).unwrap();
                    assert!((p0 - p1).length() < 1e-5, "{cont:?} {dir:?} {end:?}");
                }
            }
        }
    }

    #[test]
    fn large_extension_splits() {
        let s = plane_srf(false);
        let e = extend_by_param_dist(
            &s,
            -3.0,
            Direction::U,
            CurveEnd::Start,
            ExtendContinuity::G1,
        )
        .unwrap();
        assert!((e.parameter_bounds().ul + 3.0).abs() < 1e-9);
    }

    fn line(a: [f64; 3], b: [f64; 3], w: f64) -> Curve {
        let c = |p: [f64; 3]| {
            if w == NOW {
                CPoint::new(p[0], p[1], p[2], NOW)
            } else {
                CPoint::new(p[0] * w, p[1] * w, p[2] * w, w)
            }
        };
        Curve::new(vec![c(a), c(b)], 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap()
    }

    #[test]
    fn sweeps() {
        let s = line([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], NOW);
        let t = line([0.0, 0.0, 0.0], [0.0, 2.0, 1.0], 2.0);
        let sur = create_trans_sweep_srf(&t, &s).unwrap();
        let p = sur.eval(1.0, 1.0, Side::Left, Side::Left).unwrap();
        assert!((p - Vec3::new(1.0, 2.0, 1.0)).length() < 1e-12);
        let sw = create_swung_srf(&s, &t, 2.0).unwrap();
        assert_eq!(sw.indices().n, 1);
        assert_eq!(sw.cpt(1, 1).unwrap().w, 2.0);
    }

    #[test]
    fn skins() {
        let mut cs: Vec<Curve> = (0..4)
            .map(|i| {
                let z = f64::from(i);
                line([0.0, 0.0, z], [1.0, 0.1 * z, z], NOW)
            })
            .collect();
        for dir in [Direction::U, Direction::V] {
            let s = create_skin_srf(&mut cs, true, false, 2, dir, 0.0).unwrap();
            let p = s.eval(1.0, 1.0, Side::Left, Side::Left).unwrap();
            assert!((p - Vec3::new(1.0, 0.3, 3.0)).length() < 1e-9);
        }
        assert!(create_skin_srf(&mut cs, false, false, 2, Direction::UV, 0.0).is_err());
        let mut one = vec![cs[0].clone()];
        assert!(create_skin_srf(&mut one, false, true, 2, Direction::U, 0.0).is_err());
    }

    #[test]
    fn absurd_degrees_are_errors() {
        // fuzz (nlib.srf_adv_p3 fuzz00356, fuzz00379): degrees of i64::MAX overflowed
        // the extension's knot loop and the skin's knot count.
        let s = plane_srf(false);
        let (u, v) = (s.u_knots().to_vec(), s.v_knots().to_vec());
        let bad = Surface::new(s.cpts(), s.p(), i64::MAX as usize, u, v).unwrap();
        assert!(extend_by_param_dist(
            &bad,
            1.2,
            Direction::V,
            CurveEnd::End,
            ExtendContinuity::Cmax
        )
        .is_err());
        let mut cs: Vec<Curve> = (0..3)
            .map(|i| {
                let z = f64::from(i);
                line([0.0, 0.0, z], [1.0, 0.0, z], NOW)
            })
            .collect();
        assert!(
            create_skin_srf(&mut cs, false, false, i64::MAX as usize, Direction::V, 0.0).is_err()
        );
        // Byte counts past isize::MAX, so the reservation fails on every target; sizes
        // the OS merely cannot back (2^40 x 2^40) are granted lazily on macOS and the
        // zero fill then commits terabytes.
        assert!(net::<f64>(1 << 61, 0, "t").is_err());
        assert!(net::<f64>(0, 1 << 61, "t").is_err());
    }
}
