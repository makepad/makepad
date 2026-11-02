//! Surface extension by a distance, maximal second derivatives of an offset surface,
//! evaluation outside the parameter domain (by extension or periodic wrap), skinning
//! along a spine curve, and cross-boundary derivative strips from a vector field.
//!
//! Upstream: `source/SMLib/src/NL_SrfAdv.cpp`, `source/SMLib/inc/NL_SrfAdv.h`.
//!
//! | Rust                                                  | Upstream                           |
//! |-------------------------------------------------------|------------------------------------|
//! | [`extend_by_dist`]                                    | `N_SrfExtendByDist`                |
//! | [`offset_get_max_deriv`], [`MaxSecondDerivs`]         | `N_SrfOffsetGetMaxDeriv`           |
//! | [`eval_pt_derivs_unbounded`], [`UnboundedExtension`]  | `N_SrfEvalPtDerivsUnbounded`       |
//! | [`create_skin_spine`], [`SkinSpineOptions`], [`SpineAlignment`], [`SkinDerivs`] | `N_CreateSkinSpine` |
//! | [`cross_boundary_derivs_vector_field`]                | `N_CrossBoundaryDerivsVectorField` |
//! | [`G1_ANGLE_TOL`]                                      | `g1tol` (local of `N_SrfEvalPtDerivsUnbounded`) |
//! | [`MAX_PERIOD_SHIFTS`]                                 | no counterpart (loop bound)        |
//!
//! Deviations:
//! - Out-params become return values: a new surface, a [`MaxSecondDerivs`] instead of
//!   `Muu`, `Muv`, `Mvv`, and the derivative array `D[i][j]` of
//!   [`eval_pt_derivs_unbounded`] (the point alone, `D[0][0]`, when `ndr == 0`
//!   instead of `P`). The `NL_STACKS` arguments have no counterpart.
//! - `N_SrfEvalPtDerivsUnbounded`'s in/out pair `esur` / `gflg` (an extension that
//!   is "NULL" until first needed, and the G1-closed directions computed with it) is
//!   one `Option<UnboundedExtension>`. Upstream copies the surface into `esur` before
//!   `N_SrfIsClosedSmooth`. When that test returns the error of a failed derivative
//!   angle (after storing the closed directions in `gflg`) the option holds the copy
//!   and the directions, as upstream, so that a retry extends it; when it fails
//!   earlier (no flag stored upstream) the Rust function leaves the option `None`. Its bare error flag for `|u|` or `|v|` above `1e12` (no `NL_ERROR`
//!   number) is `InvalidInput` with reason [`BARE_FLAG_REASON`].
//! - The periodic parameter wrap of `N_SrfEvalPtDerivsUnbounded` loops while the
//!   parameter is outside the domain; where upstream would loop forever (a shift that
//!   no longer changes the parameter) or for more than [`MAX_PERIOD_SHIFTS`] shifts,
//!   the Rust function returns [`SolidError::NotConverged`].
//! - `N_SrfOffsetGetMaxDeriv` clears and skips any error of `N_SrfEvalPtNormalDeriv`;
//!   the port skips [`SolidError::Nlib`] errors and passes on the others (which mark
//!   inputs upstream would read outside its arrays with).
//! - `N_CreateSkinSpine`: the flags become types (`eDerivFlag` a [`SkinDerivs`], the
//!   `bAlignProfiles` / `sZ0` / `iSubDivLevel` triple an `Option<SpineAlignment>`, a
//!   subdivision level of `0` meaning upstream's `<= 0`, raised to `1`), gathered in
//!   [`SkinSpineOptions`]. As upstream, the profiles are modified in place (made
//!   compatible, aligned, and `profiles[0]` / `profiles[k]` made compatible with each
//!   other by the closedness test). Upstream builds the surface before it reports
//!   invalid weights (`NL_WEI_ERR`); the port returns only the error then. The u and
//!   v branches of upstream are one loop over the cross-section control points; each
//!   branch's own parametrisation and its own net layout are kept. Upstream ignores
//!   the error flags of `N_FitCalcKnotsDerivs` / `N_FitCalcKnotVectorEndDerivs`
//!   (they cannot fail for the degrees it accepts); the port passes them on.
//! - Every array access is bounds checked; where upstream would read outside its
//!   arrays (fewer spine parameters than profiles, a single profile with end
//!   derivatives, a vector field with fewer control points than the curve,
//!   incompatible profiles without `make_compatible`) the functions return
//!   [`SolidError::InvalidInput`].
//! - `N_CreateSkinSpine`: a subdivision level whose frame count `k * (subdiv + 1)`
//!   overflows is `InvalidInput`; alignment parameters that cannot be allocated are
//!   `NL_MEM_ERR` (upstream's failed `N_AllocReal1dArray`).

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::crv_adv::p1::{CurveEnd, BARE_FLAG_REASON};
use crate::nurbs::crv_basic::{curves_are_coincident, Boundary, Curve};
use crate::nurbs::crv_fit::p1::{
    interp_first_derivs_and_knots, interp_knots_and_derivs, knot_vector_end_derivs, ParamMethod,
};
use crate::nurbs::crv_fit::p2::{func_interp_given_params, knots_derivs};
use crate::nurbs::crv_tool::p2::make_compatible;
use crate::nurbs::funcs_basic::CurveFunction;
use crate::nurbs::geom::{dist_cpt_cpt, dist_pt_pt, transform_matrix_from_axes, Frame};
use crate::nurbs::index::{at, at2, oob, put};
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{CPoint, MODEL_TOL};
use crate::nurbs::srf_adv::p2::{eval_pt_normal_deriv, extend_by_dist_at_depth, SrfContinuity};
use crate::nurbs::srf_adv::p3::{create_skin_srf, extend_by_param_dist, ExtendContinuity};
use crate::nurbs::srf_adv::p4::create_b_vectors;
use crate::nurbs::srf_basic::{Direction, Surface};
use crate::nurbs::srf_fit::param_values;
use crate::nurbs::srf_tool::p1::decompose_to_bez;

/// Angular tolerance of the G1-closedness test of [`eval_pt_derivs_unbounded`]
/// (upstream `g1tol`).
pub const G1_ANGLE_TOL: f64 = 0.1;

/// Largest number of periodic shifts [`eval_pt_derivs_unbounded`] applies to one
/// parameter (upstream loops without a bound).
pub const MAX_PERIOD_SHIFTS: usize = 1 << 20;

const OP_MAXD: &str = "nurbs::srf_adv::p1::offset_get_max_deriv";
const OP_UNB: &str = "nurbs::srf_adv::p1::eval_pt_derivs_unbounded";
const OP_SPINE: &str = "nurbs::srf_adv::p1::create_skin_spine";
const OP_VFIELD: &str = "nurbs::srf_adv::p1::cross_boundary_derivs_vector_field";

// ---- N_SrfExtendByDist ----------------------------------------------------------------

/// Extends `sur` across the boundary `end` of direction `dir` by approximately the
/// distance `dist` (upstream `N_SrfExtendByDist`), with the continuity `cont` at the
/// old boundary. This is the entry point of the recursive
/// [`extend_by_dist_at_depth`] at depth `0`.
///
/// # Errors
///
/// The errors of [`extend_by_dist_at_depth`].
pub fn extend_by_dist(
    sur: &Surface,
    dist: f64,
    dir: Direction,
    end: CurveEnd,
    cont: SrfContinuity,
) -> Result<Surface> {
    // Pass the call along to the recursive function.
    extend_by_dist_at_depth(sur, dist, dir, end, cont, 0)
}

// ---- N_SrfOffsetGetMaxDeriv -----------------------------------------------------------

/// Bounds of the second derivatives of the offset of a surface (upstream `Muu`,
/// `Muv`, `Mvv`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MaxSecondDerivs {
    /// Largest `|S_uu| + d |N_uu|`.
    pub muu: f64,
    /// Largest `|S_uv| + d |N_uv|`.
    pub muv: f64,
    /// Largest `|S_vv| + d |N_vv|`.
    pub mvv: f64,
}

/// Estimates the largest second derivatives of the offset of `sur` by `d` (upstream
/// `N_SrfOffsetGetMaxDeriv`): the surface is split into Bezier patches, each sampled
/// at `(p + 1) x (q + 1)` uniformly spaced parameters (shared patch boundaries once),
/// and `|S_xy| + d |N_xy|` (`N` the unit normal) is maximised over the samples.
/// Samples where the normal derivatives cannot be computed are skipped; all values
/// start at `0`.
///
/// # Errors
///
/// The errors of the Bezier decomposition and of [`Surface::derivs`].
pub fn offset_get_max_deriv(sur: &Surface, d: f64) -> Result<MaxSecondDerivs> {
    const OP: &str = OP_MAXD;

    // Get locals and decompose into Bezier patches.
    let (p, q) = sur.degrees();
    let bez = decompose_to_bez(sur)?;

    // Compute maximum derivative.
    let mut u = vec![0.0; p + 1];
    let mut v = vec![0.0; q + 1];

    let (mut luu, mut luv, mut lvv) = (0.0, 0.0, 0.0);

    for (i, row) in bez.iter().enumerate() {
        let al = usize::from(i != 0);

        for (j, patch) in row.iter().enumerate() {
            let bl = usize::from(j != 0);

            let uk = patch.u_knots();
            let vk = patch.v_knots();
            let (u0, ur) = (at(uk, 0, OP)?, at(uk, uk.len().wrapping_sub(1), OP)?);
            let (v0, vs) = (at(vk, 0, OP)?, at(vk, vk.len().wrapping_sub(1), OP)?);

            let ui = (ur - u0) / p as f64;
            let vi = (vs - v0) / q as f64;

            put(&mut u, 0, u0, OP)?;
            put(&mut u, p, ur, OP)?;
            put(&mut v, 0, v0, OP)?;
            put(&mut v, q, vs, OP)?;

            for a in 1..p {
                put(&mut u, a, u0 + a as f64 * ui, OP)?;
            }

            for b in 1..q {
                put(&mut v, b, v0 + b as f64 * vi, OP)?;
            }

            for a in al..=p {
                for b in bl..=q {
                    let (ua, vb) = (at(&u, a, OP)?, at(&v, b, OP)?);
                    let sd = patch.derivs(ua, vb, Side::Left, Side::Left, true, 2, 2)?;

                    let nd = match eval_pt_normal_deriv(patch, ua, vb, Side::Left, Side::Left) {
                        Ok(nd) => nd,
                        Err(SolidError::Nlib { .. }) => continue,
                        Err(e) => return Err(e),
                    };

                    let muu = at2(&sd, 2, 0, OP)?.length();
                    let muv = at2(&sd, 1, 1, OP)?.length();
                    let mvv = at2(&sd, 0, 2, OP)?.length();

                    let nuu = nd.nuu.length();
                    let nuv = nd.nuv.length();
                    let nvv = nd.nvv.length();

                    let duu = muu + d * nuu;
                    let duv = muv + d * nuv;
                    let dvv = mvv + d * nvv;

                    if duu > luu {
                        luu = duu;
                    }

                    if duv > luv {
                        luv = duv;
                    }

                    if dvv > lvv {
                        lvv = dvv;
                    }
                }
            }
        }
    }

    Ok(MaxSecondDerivs {
        muu: luu,
        muv: luv,
        mvv: lvv,
    })
}

// ---- N_SrfEvalPtDerivsUnbounded -------------------------------------------------------

/// The extension of a surface kept between calls of [`eval_pt_derivs_unbounded`]
/// (upstream `esur` and `gflg`).
#[derive(Debug, Clone, PartialEq)]
pub struct UnboundedExtension {
    /// The extended surface (upstream `esur`).
    pub surface: Surface,
    /// The directions in which the original surface is G1-closed (upstream `gflg`;
    /// `None` is `NL_NO`). Parameters outside a closed direction are wrapped instead
    /// of extended.
    pub closed: Option<Direction>,
}

/// Wraps `t` into `[a, b]` by whole periods (upstream's two `while` loops).
fn wrap(mut t: f64, a: f64, b: f64) -> Result<f64> {
    const OP: &str = OP_UNB;
    let stuck = || SolidError::NotConverged {
        operation: OP,
        iterations: u32::try_from(MAX_PERIOD_SHIFTS).unwrap_or(u32::MAX),
    };
    let mut shifts = 0usize;
    while t < a {
        let nt = b - a + t; // adjust parm
        shifts += 1;
        if nt == t || shifts > MAX_PERIOD_SHIFTS {
            return Err(stuck());
        }
        t = nt;
    }
    while t > b {
        let nt = a - b + t;
        shifts += 1;
        if nt == t || shifts > MAX_PERIOD_SHIFTS {
            return Err(stuck());
        }
        t = nt;
    }
    Ok(t)
}

/// Extends `sur` in `dir` until `t` is inside `[lo, hi]` (the domain in `dir`), in
/// steps of at most `0.9` of the current domain length.
fn extend_to(
    sur: &mut Surface,
    t: f64,
    lo: f64,
    hi: f64,
    dir: Direction,
    cflg: ExtendContinuity,
) -> Result<()> {
    let (mut lo, mut hi) = (lo, hi);
    if t < lo {
        loop {
            let tt = if lo - t <= 0.9 * (hi - lo) {
                t
            } else {
                lo - (0.9 * (hi - lo))
            };

            *sur = extend_by_param_dist(sur, tt, dir, CurveEnd::Start, cflg)?;

            lo = tt;
            if t >= lo {
                break;
            }
        }
    } else {
        loop {
            let tt = if t - hi <= 0.9 * (hi - lo) {
                t
            } else {
                hi + (0.9 * (hi - lo))
            };

            *sur = extend_by_param_dist(sur, tt, dir, CurveEnd::End, cflg)?;

            hi = tt;
            if t <= hi {
                break;
            }
        }
    }
    Ok(())
}

/// Evaluates `sur` and its derivatives of total order up to `ndr` at `(u, v)`, which
/// may lie outside the parameter domain (upstream `N_SrfEvalPtDerivsUnbounded`).
///
/// Outside the domain the surface is copied into `ext` (first time only, together
/// with its G1-closed directions, [`Surface::is_closed_smooth`] with tolerances
/// [`MODEL_TOL`] and [`G1_ANGLE_TOL`]); in a closed direction the parameter is
/// wrapped by whole periods, otherwise the copy is extended with continuity `cflg`
/// ([`extend_by_param_dist`], in steps of at most `0.9` of its domain length) until
/// it contains the parameter. Once `ext` holds an extension it is evaluated (and
/// extended further) instead of `sur`; the caller must keep it while evaluating
/// the same surface.
///
/// Returns `D[i][j]` (`D[0][0]` the point) from [`Surface::derivs`] with the upper
/// triangle only, or `[[P]]` for `ndr == 0`. Evaluation uses the left side of knots.
///
/// # Errors
///
/// `InvalidInput` with reason [`BARE_FLAG_REASON`] when `|u|` or `|v|` exceeds
/// `1e12` (upstream's bare error flag); [`SolidError::NotConverged`] for a
/// periodic wrap that does not terminate; the errors of the closedness test, the
/// extension and the evaluation.
pub fn eval_pt_derivs_unbounded(
    sur: &Surface,
    u: f64,
    v: f64,
    ndr: usize,
    cflg: ExtendContinuity,
    ext: &mut Option<UnboundedExtension>,
) -> Result<Vec<Vec<Vec3>>> {
    const OP: &str = OP_UNB;
    let (mut u, mut v) = (u, v);

    // Assign surface to be evaluated and get u,v - bounds.
    let mut g1_flag = ext.as_ref().and_then(|e| e.closed);
    let b = match ext.as_ref() {
        Some(e) => e.surface.parameter_bounds(),
        None => sur.parameter_bounds(),
    };
    let (us, ue, vs, ve) = (b.ul, b.ur, b.vb, b.vt);

    // Check if (u,v) is in bounds.
    if u.abs() > 1.0e12 || v.abs() > 1.0e12 {
        return Err(SolidError::InvalidInput {
            operation: OP,
            reason: BARE_FLAG_REASON,
        });
    }

    if u < us || u > ue || v < vs || v > ve {
        // Must extend or adjust periodic parameter.
        if ext.is_none() {
            let tols = [MODEL_TOL, G1_ANGLE_TOL];
            // (upstream copies the surface into `esur` first; a failed derivative
            // angle still stores the closed directions in `*gflg` before it is
            // returned, so a later call extends that copy)
            let (closed, failed_angle) = sur.is_closed_smooth_flag(1, &tols, Direction::UV)?;
            *ext = Some(UnboundedExtension {
                surface: sur.clone(),
                closed,
            });
            if let Some(e) = failed_angle {
                return Err(e);
            }
            g1_flag = closed;
        }
        let e = ext.as_mut().ok_or_else(|| oob(OP))?;

        // Now check each direction; either extend or adjust parm.
        if u < us || u > ue {
            if matches!(g1_flag, Some(Direction::U | Direction::UV)) {
                u = wrap(u, us, ue)?;
            } else {
                extend_to(&mut e.surface, u, us, ue, Direction::U, cflg)?;
            }
        }

        if v < vs || v > ve {
            if matches!(g1_flag, Some(Direction::V | Direction::UV)) {
                v = wrap(v, vs, ve)?;
            } else {
                extend_to(&mut e.surface, v, vs, ve, Direction::V, cflg)?;
            }
        }
    }

    // Now evaluate.
    let s = ext.as_ref().map_or(sur, |e| &e.surface);
    if ndr == 0 {
        Ok(vec![vec![s.eval(u, v, Side::Left, Side::Left)?]])
    } else {
        s.derivs(u, v, Side::Left, Side::Left, true, ndr, ndr)
    }
}

// ---- N_CreateSkinSpine ----------------------------------------------------------------

/// Which derivatives [`create_skin_spine`] imposes in the skinning direction
/// (`NL_NODER = 1`, `NL_ENDDER = 2`, `NL_ALLDER = 3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkinDerivs {
    /// `NL_NODER`: plain skinning; the spine only positions the profiles.
    None,
    /// `NL_ENDDER`: the spine tangents at the first and last profile.
    Ends,
    /// `NL_ALLDER`: the spine tangent at every profile.
    All,
}

impl SkinDerivs {
    /// The upstream flag value.
    #[must_use]
    pub const fn flag(self) -> i32 {
        match self {
            SkinDerivs::None => 1,
            SkinDerivs::Ends => 2,
            SkinDerivs::All => 3,
        }
    }

    /// The value for an upstream flag, `None` for any other flag.
    #[must_use]
    pub const fn from_flag(flag: i32) -> Option<Self> {
        match flag {
            1 => Some(SkinDerivs::None),
            2 => Some(SkinDerivs::Ends),
            3 => Some(SkinDerivs::All),
            _ => None,
        }
    }
}

/// Alignment of the profiles to the spine's moving frame (upstream
/// `bAlignProfiles == NL_YES` with `sZ0` and `iSubDivLevel`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpineAlignment {
    /// Local z axis of the spine at its first parameter (`sZ0`).
    pub z0: Vec3,
    /// Extra frames computed between consecutive profiles (`iSubDivLevel`; `0` is
    /// raised to `1`, as upstream does for values `<= 0`).
    pub subdiv: usize,
}

/// Options of [`create_skin_spine`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinSpineOptions {
    /// Make the profiles compatible first (`bMakeCompatible`).
    pub make_compatible: bool,
    /// Map the profiles from the global frame to the spine frames
    /// (`bAlignProfiles`), or leave them as given (`None`).
    pub align: Option<SpineAlignment>,
    /// Derivatives imposed in the skinning direction (`eDerivFlag`).
    pub derivs: SkinDerivs,
    /// Degree in the skinning direction (`iDegree`).
    pub degree: usize,
    /// Skinning direction (`eUVDir`): [`Direction::U`] when the profiles are
    /// v-curves, [`Direction::V`] when they are u-curves.
    pub dir: Direction,
}

/// Spine data of [`create_skin_spine`].
enum SpineData {
    /// `NL_ALLDER`: distances between consecutive profile positions (`0..=k+1`,
    /// padded at both ends) and the spine tangents at the profiles.
    All {
        dists: Vec<f64>,
        tangents: Vec<Vec3>,
    },
    /// `NL_ENDDER`: first and last distances and the end tangents.
    Ends {
        dist1: f64,
        dist_k: f64,
        der0: Vec3,
        der_k: Vec3,
    },
}

/// Skins `profiles[0..=k]` placed along `spine` at `spine_params[0..=k]` (upstream
/// `N_CreateSkinSpine`).
///
/// With [`SkinDerivs::None`] this is [`create_skin_srf`] of the (optionally
/// compatible and aligned) profiles. Otherwise the control points of the profiles
/// are interpolated in the skinning direction (chord-length parameters of the
/// homogeneous net) with derivatives from the spine tangents, scaled by the ratio of
/// profile control point distances to spine distances: at the two ends only
/// ([`SkinDerivs::Ends`], `k + 3` points) or at every profile ([`SkinDerivs::All`],
/// `2k + 2` points). Rational profiles get the derivatives of their weights from an
/// interpolating weight function. When the first and last profiles coincide the
/// end distances wrap around.
///
/// The profiles are modified in place: made compatible (`make_compatible`),
/// transformed to the spine frames (`align`), and `profiles[0]` / `profiles[k]` made
/// compatible with each other.
///
/// # Errors
///
/// [`NlibCode::Input`] for a degree the mode does not allow (`None`: above `k`;
/// `Ends`: outside `2..=k + 2`; `All`: outside `2..=3`);
/// [`NlibCode::Call`] for [`Direction::UV`]; [`NlibCode::Weight`] when a weight of
/// the surface is outside `[WEIGHT_MIN, WEIGHT_MAX]`; the errors of compatibility,
/// alignment, evaluation and interpolation; `InvalidInput` for no profiles, fewer
/// spine parameters than profiles, or profiles of different sizes.
///
/// [`WEIGHT_MIN`]: crate::nurbs::srf_basic::WEIGHT_MIN
/// [`WEIGHT_MAX`]: crate::nurbs::srf_basic::WEIGHT_MAX
pub fn create_skin_spine(
    profiles: &mut [Curve],
    spine: &Curve,
    spine_params: &[f64],
    opts: &SkinSpineOptions,
) -> Result<Surface> {
    const OP: &str = OP_SPINE;
    let k = profiles.len().checked_sub(1).ok_or_else(|| oob(OP))?;
    let deg = opts.degree;

    // Check some parameters.
    match opts.derivs {
        SkinDerivs::None => {
            if deg > k {
                return Err(SolidError::nlib(OP, NlibCode::Input));
            }
        }
        SkinDerivs::Ends => {
            if deg < 2 || deg > k + 2 {
                return Err(SolidError::nlib(OP, NlibCode::Input));
            }
        }
        SkinDerivs::All => {
            if !(2..=3).contains(&deg) {
                return Err(SolidError::nlib(OP, NlibCode::Input));
            }
        }
    }

    // Make curves compatible.
    if opts.make_compatible {
        make_compatible(profiles)?;
    }

    // See if alignment is needed.
    if let Some(al) = opts.align {
        let sub = al.subdiv.max(1);

        // Set up the alignment parameters: spine_params with `sub` interpolations.
        let sub1 = sub.checked_add(1).ok_or_else(|| oob(OP))?;
        let ll = k.checked_mul(sub1).ok_or_else(|| oob(OP))?;
        let mut align_params = super::filled(ll.checked_add(1).ok_or_else(|| oob(OP))?, 0.0, OP)?;
        for ii in 0..k {
            let (t0, t1) = (at(spine_params, ii, OP)?, at(spine_params, ii + 1, OP)?);
            let incr = (t1 - t0) / (sub as f64 + 1.0);
            let base = ii * sub1;

            for jj in 0..=sub {
                put(&mut align_params, base + jj, t0 + jj as f64 * incr, OP)?;
            }
        }
        put(&mut align_params, ll, at(spine_params, k, OP)?, OP)?;

        let bvecs = create_b_vectors(spine, al.z0, &align_params)?;

        let world = Frame::new(Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z);
        for ii in 0..=k {
            let (origin, x_axis) = spine.eval_tangent(at(spine_params, ii, OP)?, Side::Left)?;

            let z_axis = at(&bvecs, ii * sub1, OP)?;
            let y_axis = z_axis.cross(x_axis);

            let mat =
                transform_matrix_from_axes(world, Frame::new(origin, x_axis, y_axis, z_axis))?;

            profiles
                .get_mut(ii)
                .ok_or_else(|| oob(OP))?
                .transform(&mat)?;
        }
    }

    // Do simple skinning if no derivative is specified.
    if opts.derivs == SkinDerivs::None {
        return create_skin_srf(profiles, false, false, deg, opts.dir, 0.0);
    }

    // Check if curve set is closed or rational.
    let closed = if k == 0 {
        let first = profiles.first_mut().ok_or_else(|| oob(OP))?;
        let mut a = first.clone();
        let mut b = first.clone();
        let res = curves_are_coincident(&mut a, &mut b, MODEL_TOL)?;
        *first = a;
        res
    } else {
        let (head, tail) = profiles.split_at_mut(k);
        let c0 = head.first_mut().ok_or_else(|| oob(OP))?;
        let ck = tail.first_mut().ok_or_else(|| oob(OP))?;
        curves_are_coincident(c0, ck, MODEL_TOL)?
    };

    let first = profiles.first().ok_or_else(|| oob(OP))?;
    let rational = first.is_rational();

    // Get indexes and control point data ("c" is the cross-section direction).
    let udir = match opts.dir {
        Direction::U => true,
        Direction::V => false,
        Direction::UV => return Err(SolidError::nlib(OP, NlibCode::Call)),
    };
    let nc = first.n();
    let deg_c = first.degree();
    let knots_c = first.knots().to_vec();
    let ns = if opts.derivs == SkinDerivs::Ends {
        k + 2
    } else {
        2 * k + 1
    };

    // All profile control points, in the layout of the surface net.
    let mut all = Vec::with_capacity(k + 1);
    for prof in profiles.iter() {
        let pw = prof.cpts();
        if pw.len() != nc + 1 {
            return Err(oob(OP));
        }
        all.push(pw.to_vec());
    }
    let grid = if udir { all } else { transpose(&all) };

    // Get parameters.
    let (pu, pv) = param_values(&grid, ParamMethod::ChordLength)?;
    let params = if udir { pu } else { pv };

    // Compute distances and derivatives along the spine.
    let spine_data = if opts.derivs == SkinDerivs::All {
        let mut pts = vec![Vec3::ZERO; k + 1];
        let mut dists = vec![0.0; k + 2];
        put(
            &mut pts,
            0,
            spine.eval(at(spine_params, 0, OP)?, Side::Left)?,
            OP,
        )?;
        for ii in 1..=k {
            let pt = spine.eval(at(spine_params, ii, OP)?, Side::Left)?;
            put(&mut pts, ii, pt, OP)?;
            put(&mut dists, ii, dist_pt_pt(pt, at(&pts, ii - 1, OP)?), OP)?;
        }

        if closed {
            let wrap0 = at(&dists, k, OP)?;
            put(&mut dists, 0, wrap0, OP)?;
            let wrap1 = at(&dists, 1, OP)?;
            put(&mut dists, k + 1, wrap1, OP)?;
        } else {
            put(&mut dists, 0, 0.0, OP)?;
            put(&mut dists, k + 1, 0.0, OP)?;
        }

        // Spine tangents at the profiles.
        let mut tangents = vec![Vec3::ZERO; k + 1];
        for ii in 0..=k {
            let t = spine.derivs(at(spine_params, ii, OP)?, Side::Left, 1)?;
            put(&mut tangents, ii, at(&t, 1, OP)?, OP)?;
        }
        SpineData::All { dists, tangents }
    } else {
        let km1 = k.checked_sub(1).ok_or_else(|| oob(OP))?;
        let p0 = spine.eval(at(spine_params, 0, OP)?, Side::Left)?;
        let p1 = spine.eval(at(spine_params, 1, OP)?, Side::Left)?;
        let pkm1 = spine.eval(at(spine_params, km1, OP)?, Side::Left)?;
        let pk = spine.eval(at(spine_params, k, OP)?, Side::Left)?;

        let dist1 = dist_pt_pt(p1, p0);
        let dist_k = dist_pt_pt(pk, pkm1);

        let t = spine.derivs(at(spine_params, 0, OP)?, Side::Left, 1)?;
        let der0 = at(&t, 1, OP)?;

        let t = spine.derivs(at(spine_params, k, OP)?, Side::Left, 1)?;
        let der_k = at(&t, 1, OP)?;
        SpineData::Ends {
            dist1,
            dist_k,
            der0,
            der_k,
        }
    };

    // Compute surface control points.
    let knots_s = match spine_data {
        SpineData::All { .. } => knots_derivs(&params, deg)?,
        SpineData::Ends { .. } => knot_vector_end_derivs(&params, deg)?,
    };
    let knt = KnotVector::from_slice(&knots_s)?;

    // cols[c]: control points of the skinning curve through cross-section point c.
    let mut cols: Vec<Vec<CPoint>> = Vec::with_capacity(nc + 1);
    let mut pc = vec![CPoint::ZERO; k + 1];
    let mut weights = vec![0.0; k + 1];
    for c in 0..=nc {
        // Extract control points for interpolation.
        for (ii, prof) in profiles.iter().enumerate() {
            let pw = at(prof.cpts(), c, OP)?;
            put(&mut pc, ii, pw, OP)?;

            if rational {
                put(&mut weights, ii, pw.w, OP)?;
            }
        }

        // Get weight function.
        let wfun = if rational {
            let deg_func = k.min(deg);
            Some(func_interp_given_params(&weights, deg_func, &params)?)
        } else {
            None
        };
        let wderiv = |t: f64| -> Result<f64> {
            let f = wfun.as_ref().ok_or_else(|| oob(OP))?;
            at(&CurveFunction::derivs(f, t, Side::Left, 1)?, 1, OP)
        };

        // Get derivatives and interpolate.
        let skin = match &spine_data {
            SpineData::All { dists, tangents } => {
                let mut pd = vec![0.0; k + 2];
                for ii in 1..=k {
                    put(
                        &mut pd,
                        ii,
                        dist_cpt_cpt(at(&pc, ii, OP)?, at(&pc, ii - 1, OP)?),
                        OP,
                    )?;
                }

                if closed {
                    let wrap0 = at(&pd, k, OP)?;
                    put(&mut pd, 0, wrap0, OP)?;
                    let wrap1 = at(&pd, 1, OP)?;
                    put(&mut pd, k + 1, wrap1, OP)?;
                } else {
                    put(&mut pd, 0, 0.0, OP)?;
                    put(&mut pd, k + 1, 0.0, OP)?;
                }

                let mut rd = vec![CPoint::ZERO; k + 1];
                for ii in 0..=k {
                    let factor = (at(&pd, ii + 1, OP)? + at(&pd, ii, OP)?)
                        / (at(dists, ii + 1, OP)? + at(dists, ii, OP)?);
                    // The spine tangent at this profile, scaled.
                    let scaled = at(tangents, ii, OP)? * factor;

                    let r = if rational {
                        let wd = wderiv(at(&params, ii, OP)?)?;
                        CPoint::rat_deriv(at(&pc, ii, OP)?, scaled, wd)
                    } else {
                        CPoint::from_point(scaled)
                    };
                    put(&mut rd, ii, r, OP)?;
                }

                interp_first_derivs_and_knots(&pc, &rd, &params, &knt, deg)?
            }
            SpineData::Ends {
                dist1,
                dist_k,
                der0,
                der_k,
            } => {
                let km1 = k.checked_sub(1).ok_or_else(|| oob(OP))?;
                let pd1 = dist_cpt_cpt(at(&pc, 1, OP)?, at(&pc, 0, OP)?);
                let pdk = dist_cpt_cpt(at(&pc, k, OP)?, at(&pc, km1, OP)?);

                let (scaled0, scaled_k) = if closed {
                    let factor = (pd1 + pdk) / (dist1 + dist_k);
                    (*der0 * factor, *der_k * factor)
                } else {
                    let factor0 = pd1 / dist1;
                    let factor_k = pdk / dist_k;
                    (*der0 * factor0, *der_k * factor_k)
                };

                let (rd0, rdk) = if rational {
                    let wd = wderiv(at(&params, 0, OP)?)?;
                    let rd0 = CPoint::rat_deriv(at(&pc, 0, OP)?, scaled0, wd);

                    let wd = wderiv(at(&params, k, OP)?)?;
                    let rdk = CPoint::rat_deriv(at(&pc, k, OP)?, scaled_k, wd);
                    (rd0, rdk)
                } else {
                    (CPoint::from_point(scaled0), CPoint::from_point(scaled_k))
                };

                interp_knots_and_derivs(&pc, &params, &knt, deg, rd0, rdk)?
            }
        };

        let cp = skin.cpts();
        if cp.len() != ns + 1 {
            return Err(oob(OP));
        }
        cols.push(cp.to_vec());
    }

    // Assemble the net: (skin index, cross index) for u skinning, transposed for v.
    let (net, p, q, u, v) = if udir {
        (transpose(&cols), deg, deg_c, knots_s, knots_c)
    } else {
        (cols, deg_c, deg, knots_c, knots_s)
    };
    let sur = Surface::new(net, p, q, u, v)?;

    // Check the weights.
    sur.weights_are_valid()?;

    Ok(sur)
}

/// `b[j][i] = a[i][j]` of a rectangular net.
fn transpose(a: &[Vec<CPoint>]) -> Vec<Vec<CPoint>> {
    let cols = a.first().map_or(0, Vec::len);
    (0..cols)
        .map(|j| a.iter().filter_map(|row| row.get(j).copied()).collect())
        .collect()
}

// ---- N_CrossBoundaryDerivsVectorField -------------------------------------------------

/// Cross-boundary derivative strip from a boundary curve and a vector field on the
/// same knot vector (upstream `N_CrossBoundaryDerivsVectorField`): a surface linear
/// across `bndy` whose boundary row is `cur` and whose other row is `cur` translated
/// by the (Euclidean) vector field control points, forwards for [`Boundary::Left`] /
/// [`Boundary::Bottom`] and backwards (rows swapped) for [`Boundary::Right`] /
/// [`Boundary::Top`]. The linear direction has knots `[0, 0, 1, 1]`.
///
/// # Errors
///
/// [`NlibCode::Input`] when the knot vectors differ in length or in any value;
/// `InvalidInput` when `vfield` has fewer control points than `cur`.
pub fn cross_boundary_derivs_vector_field(
    cur: &Curve,
    vfield: &Curve,
    bndy: Boundary,
) -> Result<Surface> {
    const OP: &str = OP_VFIELD;

    // When cur and vfield knot counts or values vary - quit.
    let uk = cur.knots();
    let vk = vfield.knots();
    if uk.len() != vk.len() {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    if uk.iter().zip(vk).any(|(a, b)| a != b) {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    let pw = cur.cpts();
    let qw = vfield.cpts();
    let deg = cur.degree();
    let uvc = uk.to_vec();
    let lin = vec![0.0, 0.0, 1.0, 1.0];

    // Load control points: one row along the curve for each side of the strip.
    let mut base = Vec::with_capacity(pw.len());
    let mut moved = Vec::with_capacity(pw.len());
    for (ii, &p) in pw.iter().enumerate() {
        // Convert vector field control points to Euclidean space.
        let t = at(qw, ii, OP)?.to_euclid();
        match bndy {
            Boundary::Left | Boundary::Bottom => {
                base.push(p);
                moved.push(p.translate(t));
            }
            Boundary::Right | Boundary::Top => {
                base.push(p.translate(-t));
                moved.push(p);
            }
        }
    }

    match bndy {
        Boundary::Left | Boundary::Right => Surface::new(vec![base, moved], 1, deg, lin, uvc),
        Boundary::Bottom | Boundary::Top => {
            let net = base
                .into_iter()
                .zip(moved)
                .map(|(a, b)| vec![a, b])
                .collect();
            Surface::new(net, deg, 1, uvc, lin)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;

    fn line_crv(a: Vec3, b: Vec3) -> Curve {
        let pw = vec![CPoint::from_point(a), CPoint::from_point(b)];
        Curve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap()
    }

    fn plane(n: usize) -> Surface {
        let net = (0..=n)
            .map(|i| {
                (0..=n)
                    .map(|j| CPoint::new(i as f64, j as f64, 0.0, NOW))
                    .collect()
            })
            .collect();
        let mut k = vec![0.0; n + 1];
        k.extend(vec![1.0; n + 1]);
        Surface::new(net, n, n, k.clone(), k).unwrap()
    }

    #[test]
    fn vector_field_strip_left_and_top() {
        let cur = line_crv(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
        let vf = line_crv(Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0));
        let s = cross_boundary_derivs_vector_field(&cur, &vf, Boundary::Left).unwrap();
        assert_eq!(s.degrees(), (1, 1));
        assert_eq!(s.cpt(1, 1).unwrap().to_euclid(), Vec3::new(2.0, 1.0, 0.0));
        let s = cross_boundary_derivs_vector_field(&cur, &vf, Boundary::Top).unwrap();
        assert_eq!(s.cpt(0, 0).unwrap().to_euclid(), Vec3::new(-1.0, 0.0, 0.0));
        assert_eq!(s.cpt(0, 1).unwrap().to_euclid(), Vec3::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn vector_field_knot_mismatch() {
        let cur = line_crv(Vec3::ZERO, Vec3::X);
        let vf = Curve::new(
            vec![CPoint::from_point(Vec3::X), CPoint::from_point(Vec3::Y)],
            1,
            vec![0.0, 0.0, 2.0, 2.0],
        )
        .unwrap();
        let e = cross_boundary_derivs_vector_field(&cur, &vf, Boundary::Left).unwrap_err();
        assert!(matches!(
            e,
            SolidError::Nlib {
                code: NlibCode::Input,
                ..
            }
        ));
    }

    #[test]
    fn unbounded_extends_plane() {
        let s = plane(2);
        let mut ext = None;
        let d =
            eval_pt_derivs_unbounded(&s, 1.5, -0.25, 1, ExtendContinuity::G1, &mut ext).unwrap();
        assert!(ext.is_some());
        let p = d[0][0];
        assert!(
            (p.x - 3.0).abs() < 1e-9 && (p.y + 0.5).abs() < 1e-9,
            "{p:?}"
        );
        // Inside the domain the original surface is still evaluated directly.
        let mut none = None;
        let d = eval_pt_derivs_unbounded(&s, 0.5, 0.5, 0, ExtendContinuity::G1, &mut none).unwrap();
        assert!(none.is_none());
        assert_eq!(d[0][0], Vec3::new(1.0, 1.0, 0.0));
        let e = eval_pt_derivs_unbounded(&s, 2e12, 0.5, 0, ExtendContinuity::G1, &mut none);
        assert!(e.is_err());
    }

    #[test]
    fn max_deriv_of_plane_is_zero() {
        let m = offset_get_max_deriv(&plane(2), 1.0).unwrap();
        assert_eq!(m, MaxSecondDerivs::default());
    }

    #[test]
    fn skin_spine_ends_straight() {
        let mut profiles: Vec<Curve> = (0..3)
            .map(|i| {
                let z = f64::from(i);
                line_crv(Vec3::new(0.0, 0.0, z), Vec3::new(1.0, 0.0, z))
            })
            .collect();
        let spine = line_crv(Vec3::ZERO, Vec3::new(0.0, 0.0, 2.0));
        let opts = SkinSpineOptions {
            make_compatible: false,
            align: None,
            derivs: SkinDerivs::Ends,
            degree: 3,
            dir: Direction::U,
        };
        let s = create_skin_spine(&mut profiles, &spine, &[0.0, 0.5, 1.0], &opts).unwrap();
        assert_eq!(s.degrees(), (3, 1));
        assert_eq!(s.u_knots().len(), (2 + 2) + 3 + 2);
        let p = s.eval(0.5, 0.5, Side::Left, Side::Left).unwrap();
        assert!(
            (p.z - 1.0).abs() < 1e-9 && (p.x - 0.5).abs() < 1e-9,
            "{p:?}"
        );
        let opts = SkinSpineOptions {
            derivs: SkinDerivs::All,
            degree: 4,
            ..opts
        };
        let e = create_skin_spine(&mut profiles, &spine, &[0.0, 0.5, 1.0], &opts).unwrap_err();
        assert!(matches!(
            e,
            SolidError::Nlib {
                code: NlibCode::Input,
                ..
            }
        ));
    }

    #[test]
    fn absurd_subdivision_is_an_error() {
        // fuzz (nlib.srf_adv_p1 fuzz00596, fuzz01129): `k * (subdiv + 1)` overflowed
        // and sized the alignment parameters.
        let mut profiles = vec![
            line_crv(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)),
            line_crv(Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0)),
        ];
        let spine = line_crv(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        for subdiv in [usize::MAX, i64::MAX as usize] {
            let opts = SkinSpineOptions {
                make_compatible: false,
                align: Some(SpineAlignment {
                    z0: Vec3::new(1.0, 0.0, 0.0),
                    subdiv,
                }),
                derivs: SkinDerivs::None,
                degree: 1,
                dir: Direction::V,
            };
            assert!(create_skin_spine(&mut profiles, &spine, &[0.0, 1.0], &opts).is_err());
        }
    }
}
