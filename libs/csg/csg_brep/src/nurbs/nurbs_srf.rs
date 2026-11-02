//! SMLib's local NURBS surface routines: chord heights and turning angles of a
//! control net or of surface samples, surface allocation and copying, Bezier
//! decomposition, splitting and the degenerate-surface test.
//!
//! Upstream: `source/SMLib/src/SmNurbsSrf.cpp`, `source/SMLib/inc/SmNurbsSrf.h`.
//!
//! | Rust                                  | Upstream                                    |
//! |---------------------------------------|---------------------------------------------|
//! | [`compute_net_constants`]             | `sm_ComputeNetConstants`                    |
//! | [`compute_surf_constants`]            | `sm_ComputeSurfConstants`                   |
//! | [`ConstantsRequest`], [`NetConstants`] | the optional `double*` outputs of both     |
//! | [`allocate_nurb_surface`]             | `sm_AllocateNurbSurface`, `sm_InitNurbSurfaceMemory` |
//! | [`copy_nurb_surface`]                 | `sm_AllocateAndCopyNurbSurface`             |
//! | (none: Rust owns memory)              | `sm_ComputeNurbSurfaceSize(n, m, r, s)`     |
//! | [`decompose_srf`]                     | `sm_DecomposeSrf`                           |
//! | [`split_srf`]                         | `sm_SplitSrf`                               |
//! | [`is_nsrf_degenerate`]                | `sm_IsNSrfDegenerate`                       |
//!
//! Deviations:
//! - Out-params become return values. The optional `double*` outputs of
//!   `sm_ComputeNetConstants` / `sm_ComputeSurfConstants` (NULL = not wanted) are a
//!   [`ConstantsRequest`] in and [`NetConstants`] (`Some` for each requested value)
//!   out; which outputs are requested still steers the control flow exactly as the
//!   NULL tests do upstream.
//! - Angle walks also report a non-finite segment length before normalization in
//!   [`NetConstants::nonfinite_angle_length`]. This diagnostic does not change the
//!   evaluations, arithmetic, branches or numeric outputs; approximate surface caches
//!   use it to refuse further refinement driven by an invalid angle measurement.
//! - The `const SmSurface*` of both routines is an evaluator closure standing for
//!   `SmSurface::EvaluatePoint` (the virtual call, unit `surface.surface_p2`), so the
//!   routines do not depend on the surface class hierarchy. Its errors propagate as
//!   upstream's `SER`. `sm_ComputeNetConstants` takes it, with its domain, as an
//!   `Option` (upstream: `pSurface` may be NULL); `sm_ComputeSurfConstants`'s NULL
//!   check (`NER`) has no counterpart.
//! - `sm_ComputeNetConstants` ignores the flags of its `N_SrfEvalPt` calls (the
//!   output point then keeps its previous value), and so does the port: its points are
//!   function-scope as upstream's, and a failed evaluation leaves them unchanged. This
//!   only happens for surfaces whose knot vector the constructor's fixup left
//!   non-monotone (a Bezier patch with a reversed domain, PR #147 round 4); returning
//!   the error there, as earlier, failed the whole surface-cache build where upstream
//!   builds it.
//! - The memory-layout helpers (`sm_ComputeNurbSurfaceSize`,
//!   `sm_InitNurbSurfaceMemory`) have no counterpart: [`Surface`] owns its storage.
//!   [`allocate_nurb_surface`] returns a zero-filled surface (upstream
//!   `smos_Calloc`), allocated fallibly: `InvalidInput` where the size overflows or
//!   does not fit in memory (upstream: `NULL`). A logical surface storage — the
//!   control net's row headers and points plus both knot arrays — above
//!   [`limits::MAX_ALLOC_BYTES`] is refused before allocating; that bounds one
//!   surface's arrays, not process memory or allocator overhead.
//!   [`copy_nurb_surface`] returns a compact copy (upstream returns
//!   NULL for a surface without a net, which a [`Surface`] cannot be).
//! - `sm_DecomposeSrf` fills a caller-allocated array of `nsu * nsv` patches (degrees
//!   `p`, `q`, highest knot indices `2p+1`, `2q+1`, as its caller
//!   `sm_CreateBezierPatchBlock` allocates them); [`decompose_srf`] allocates them
//!   (zero-filled) and returns them as rows `[iu][iv]`. Upstream's error flag (no
//!   non-zero span, or more u breakpoints than spans: `NL_QUIT` without an error
//!   number) is `InvalidInput`; the partly filled patches are not returned.
//! - `sm_SplitSrf` allocates the two halves when the caller passes NULL; the port
//!   always returns new halves ([`crate::nurbs::srf_tool::p1::split`] after the same
//!   parameter checks). Its `default:` direction branch (returns the flag without an
//!   error number) is `InvalidInput` for [`Direction::UV`]. `N_SrfSplit` returns
//!   success without writing its outputs for a knot multiplicity above the degree
//!   (a bad knot vector), leaving them zero-filled; the port returns
//!   `InvalidInput` instead.
//! - Every array access is bounds checked. Where upstream would index outside its
//!   arrays (a net with a single row or column whose end points coincide, malformed
//!   surfaces) the port returns [`SolidError::InvalidInput`]; for every input upstream
//!   handles inside its arrays the results are the same, bit for bit.
//! - `sm_IsNSrfDegenerate` takes the tolerance with default `SM_EFF_ZERO`
//!   ([`EFF_ZERO`]); its `SM_ASSERT` on the box has no counterpart. It returns the
//!   errors of [`Surface::bbox`] (upstream ignores `N_SrfGetBBox`'s flag).

// `v = v / d` is upstream `operator/`; `Vec3 /= d` ports `operator/=`, which skips
// tiny divisors, so the suggested rewrite would change results.
#![allow(clippy::assign_op_pattern)]

use crate::error::{NlibCode, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::vector3d::{acos, sqrt};
use crate::limits;
use crate::math::{max2, Point2, Point3, Vec3, EFF_ZERO, EFF_ZERO_SQ, PI};
use crate::nurbs::index::oob;
use crate::nurbs::knots::Side;
use crate::nurbs::math::CPoint;
use crate::nurbs::nurbs_crv::to_euclid;
use crate::nurbs::srf_basic::{Direction, Surface};
use crate::nurbs::srf_tool::p1 as srf_tool;

const OP_NET: &str = "nurbs::nurbs_srf::compute_net_constants";
const OP_ALLOC: &str = "nurbs::nurbs_srf::allocate_nurb_surface";
const OP_DECOMPOSE: &str = "nurbs::nurbs_srf::decompose_srf";
const OP_SPLIT: &str = "nurbs::nurbs_srf::split_srf";

/// `dLimitAngRad` of `sm_ComputeNetConstants`: quit integrating a turning angle once
/// it exceeds this (more than one and a half times around).
const LIMIT_ANG_RAD: f64 = 10.0;

/// An evaluator `uv -> point` (upstream `SmSurface::EvaluatePoint`).
pub type PointEvaluator<'a> = dyn FnMut(Point2) -> Result<Point3> + 'a;

/// Which outputs of [`compute_net_constants`] / [`compute_surf_constants`] are
/// wanted (upstream: the non-NULL `double*` arguments).
// Five independent switches, as upstream's five optional output pointers.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstantsRequest {
    /// `pdUChordHeight`.
    pub u_chord_height: bool,
    /// `pdVChordHeight`.
    pub v_chord_height: bool,
    /// `pdUAngleDeg`.
    pub u_angle_deg: bool,
    /// `pdVAngleDeg`.
    pub v_angle_deg: bool,
    /// `pdUVChordHeight`.
    pub uv_chord_height: bool,
}

impl ConstantsRequest {
    /// Every output.
    pub const ALL: Self = Self {
        u_chord_height: true,
        v_chord_height: true,
        u_angle_deg: true,
        v_angle_deg: true,
        uv_chord_height: true,
    };
}

impl Default for ConstantsRequest {
    fn default() -> Self {
        Self::ALL
    }
}

/// Chord heights and turning angles (`Some` for each requested output).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NetConstants {
    /// Largest distance of a u-direction polygon point from its base segment.
    pub u_chord_height: Option<f64>,
    /// Largest distance of a v-direction polygon point from its base segment.
    pub v_chord_height: Option<f64>,
    /// Largest turning angle (degrees) along a u-direction polygon.
    pub u_angle_deg: Option<f64>,
    /// Largest turning angle (degrees) along a v-direction polygon.
    pub v_angle_deg: Option<f64>,
    /// Largest distance of a diagonal sample from its base segment.
    pub uv_chord_height: Option<f64>,
    /// A requested angle walk encountered an infinite or NaN segment length before
    /// normalization. Numeric outputs retain upstream's IEEE arithmetic, which can
    /// hide this failure (e.g. division by infinity produces a zero direction).
    pub nonfinite_angle_length: bool,
}

/// Accumulated (squared) outputs before the final conversion.
#[derive(Debug, Clone, Copy, Default)]
struct Acc {
    u_ch: f64,
    v_ch: f64,
    uv_ch: f64,
    max_u_ang_rad: f64,
    max_v_ang_rad: f64,
    nonfinite_angle_length: bool,
}

impl Acc {
    /// The output conversion shared by both routines (`SM_RAD2DEG`, `smos_Sqrt`).
    fn finish(self, req: ConstantsRequest) -> NetConstants {
        let deg = |a: f64| a * (180.0 / PI);
        NetConstants {
            u_angle_deg: req.u_angle_deg.then(|| deg(self.max_u_ang_rad)),
            v_angle_deg: req.v_angle_deg.then(|| deg(self.max_v_ang_rad)),
            u_chord_height: req.u_chord_height.then(|| sqrt(self.u_ch)),
            v_chord_height: req.v_chord_height.then(|| sqrt(self.v_ch)),
            uv_chord_height: req.uv_chord_height.then(|| sqrt(self.uv_ch)),
            nonfinite_angle_length: self.nonfinite_angle_length,
        }
    }
}

/// Euclidean control point `Pw[i][j]`.
fn pt(sur: &Surface, i: usize, j: usize) -> Result<Point3> {
    Ok(to_euclid(sur.cpt(i, j)?))
}

/// Squared distance of `next` from the line `line_pnt + t * line_vec`
/// (`smgu_LineClosestPoint` and the chord vector).
fn chord_sq(line_pnt: Point3, line_vec: Vec3, next: Point3) -> f64 {
    let line_param = line_closest_point(line_pnt, line_vec, next);
    let pnt_on_line = line_pnt + (line_vec * line_param);
    let ch_vec = next - pnt_on_line;
    ch_vec.length_squared()
}

/// Turning angle (radians) between the unit vectors `curr` and `last`, clamped.
fn turn(curr: Vec3, last: Vec3) -> f64 {
    // Same as upstream's two ifs, NaN included.
    acos(curr.dot(last).clamp(-1.0, 1.0))
}

/// Approximate chord heights and turning angles of the control net of `sur` in the u,
/// v and diagonal directions (`sm_ComputeNetConstants`).
///
/// With `surface` (an evaluator and its uv domain) the values come from surface
/// evaluations instead ([`compute_surf_constants`]). Control points closer than
/// `zone_tol3d` are skipped by the angle walks. The diagonal chord height also uses
/// `N_SrfEvalPt` samples of `sur` along both domain diagonals.
///
/// Errors: `InvalidInput` for nets upstream would index outside (see the module
/// docs); the errors of [`Surface::eval`] and of the evaluator.
pub fn compute_net_constants(
    surface: Option<(&mut PointEvaluator<'_>, &Extent2d)>,
    sur: &Surface,
    zone_tol3d: f64,
    req: ConstantsRequest,
) -> Result<NetConstants> {
    // When given a surface with a sub-domain calculate values directly from surface
    // evaluations.
    if let Some((eval, uv_domain)) = surface {
        return compute_surf_constants(eval, uv_domain, req);
    }

    let mut acc = Acc::default();
    let (n, m) = (sur.n(), sur.m());

    // Upstream declares these points once for the whole function, and the surface
    // evaluations of the UV section below ignore their status (`N_SrfEvalPt` leaves its
    // output untouched on failure), so a failed evaluation reuses the value the earlier
    // sections left: they are function-scope here too.
    let mut line_pnt = Vec3::ZERO;
    let mut last_point = Vec3::ZERO;
    let mut try_point = Vec3::ZERO;
    let mut next = Vec3::ZERO;
    let mut line_vec = Vec3::ZERO;

    // Save time - quit when the turning angle exceeds a huge limit.
    let mut over_limit = false;

    // VDir chord height and VDir angle from the control polygon: for all u rows.
    for jj in 0..=n {
        // For the chord height, get a base segment from the first to the last control
        // point on this net row.
        if req.v_chord_height {
            line_pnt = pt(sur, jj, 0)?;
            last_point = pt(sur, jj, m)?;
            line_vec = last_point - line_pnt;
            let mut try_index = m.wrapping_sub(1);
            while line_vec.length_squared() < EFF_ZERO_SQ {
                // If first and last point coincide try a middle point.
                try_point = pt(sur, jj, try_index)?;
                line_vec = try_point - line_pnt;
                if try_index == 0 {
                    // Singular point: just continue on.
                    break;
                }
                try_index -= 1;
            }
            if line_vec.length_squared() < EFF_ZERO_SQ {
                continue; // singular point
            }
        }

        let mut total_ang_rad = 0.0;
        let mut last_vec = Vec3::ZERO;

        // Hold u constant and loop across the v points.
        let mut curr = pt(sur, jj, 0)?;
        for ii in 1..=m {
            next = pt(sur, jj, ii)?;

            // V chord height.
            if req.v_chord_height {
                let ch_sq = chord_sq(line_pnt, line_vec, next);
                if ch_sq > acc.v_ch {
                    acc.v_ch = ch_sq;
                }
            }

            // V angle.
            if req.v_angle_deg {
                // Check for the angle getting really big: sometimes the control net
                // is not suitable for these calculations.
                if total_ang_rad > LIMIT_ANG_RAD {
                    if !over_limit {
                        over_limit = true;
                        // Rough approximation of the whole turning angle (integer
                        // division upstream).
                        total_ang_rad *= (m / ii) as f64;
                        // Quit, unless doing the chord height as well.
                        if !req.v_chord_height {
                            break;
                        }
                    }
                    continue;
                }

                let mut curr_vec = next - curr;
                let dist = curr_vec.length();
                acc.nonfinite_angle_length |= !dist.is_finite();

                // Skip control points within the zone tolerance.
                if dist < zone_tol3d {
                    continue;
                }
                curr_vec = curr_vec / dist;
                if last_vec.length_squared() > EFF_ZERO_SQ {
                    total_ang_rad += turn(curr_vec, last_vec);
                }
                last_vec = curr_vec;
            }
            curr = next;
        }

        if total_ang_rad > acc.max_v_ang_rad {
            acc.max_v_ang_rad = total_ang_rad;
        }

        if over_limit && !req.v_chord_height {
            break;
        }
    }

    over_limit = false;

    // UDir chord height and UDir angle from the control polygon: for all v columns.
    for jj in 0..=m {
        if req.u_chord_height {
            line_pnt = pt(sur, 0, jj)?;
            last_point = pt(sur, n, jj)?;
            line_vec = last_point - line_pnt;
            let mut try_index = n.wrapping_sub(1);
            while line_vec.length_squared() < EFF_ZERO_SQ {
                // If first and last point coincide try a middle point.
                try_point = pt(sur, try_index, jj)?;
                line_vec = try_point - line_pnt;
                if try_index == 0 {
                    // Singular point: just continue on.
                    break;
                }
                try_index -= 1;
            }
            if line_vec.length_squared() < EFF_ZERO_SQ {
                continue; // singular point
            }
        }

        let mut total_angle = 0.0;
        let mut last_vec = Vec3::ZERO;

        let mut curr = pt(sur, 0, jj)?;
        for ii in 1..=n {
            next = pt(sur, ii, jj)?;

            // U chord height.
            if req.u_chord_height {
                let ch_sq = chord_sq(line_pnt, line_vec, next);
                if ch_sq > acc.u_ch {
                    acc.u_ch = ch_sq;
                }
            }

            // U angle.
            if req.u_angle_deg {
                if total_angle > LIMIT_ANG_RAD {
                    if !over_limit {
                        over_limit = true;
                        total_angle *= (n / ii) as f64; // rough approximation
                        if !req.u_chord_height {
                            break;
                        }
                    }
                    continue;
                }

                let mut curr_vec = next - curr;
                let dist = curr_vec.length();
                acc.nonfinite_angle_length |= !dist.is_finite();

                // Skip control points within the zone tolerance (coincident points).
                if dist < zone_tol3d {
                    continue;
                }
                curr_vec = curr_vec / dist;
                if last_vec.length_squared() > EFF_ZERO_SQ {
                    total_angle += turn(curr_vec, last_vec);
                }
                last_vec = curr_vec;
            }
            curr = next;
        }
        if total_angle > acc.max_u_ang_rad {
            acc.max_u_ang_rad = total_angle;
        }

        if over_limit && !req.u_chord_height {
            break;
        }
    }

    if req.uv_chord_height {
        // UVDir chord height from sample surface evaluations.
        let bounds = sur.parameter_bounds();
        let u_domain = Extent1d::from_min_max(bounds.ul, bounds.ur);
        let v_domain = Extent1d::from_min_max(bounds.vb, bounds.vt);
        let eval = |du: f64, dv: f64| -> Result<Point3> {
            sur.eval(
                u_domain.evaluate(du),
                v_domain.evaluate(dv),
                Side::Left,
                Side::Left,
            )
        };

        // Test the two diagonals from [0, 0] to [1, 1] and from [0, 1] to [1, 0].
        for jj in 0..2_i32 {
            let ds = if jj == 0 { -0.25 } else { 0.25 };
            let jjf = f64::from(jj);
            let mut dv = jjf;

            // Get a base segment from the first to the last corner of this diagonal.
            if let Ok(pnt) = eval(0.0, dv) {
                line_pnt = pnt;
            }
            if let Ok(pnt) = eval(1.0, 1.0 - dv) {
                last_point = pnt;
            }

            line_vec = last_point - line_pnt;
            let mut itr = 0_i32;
            while itr <= 3 && line_vec.length_squared() < EFF_ZERO_SQ {
                // If first and last point coincide try a middle point.
                let du = 1.0 - 0.25 * f64::from(itr);
                dv = f64::from(1 - jj) + ds * f64::from(itr);
                if let Ok(pnt) = eval(du, dv) {
                    try_point = pnt;
                }
                line_vec = try_point - line_pnt;
                itr += 1;
            }

            // Skip singular curves.
            if itr == 4 {
                continue;
            }

            // Loop across the diagonal points.
            for itr in 1..=3_i32 {
                let du = 0.0 + 0.25 * f64::from(itr);
                let dv = jjf - ds * f64::from(itr);
                if let Ok(pnt) = eval(du, dv) {
                    next = pnt;
                }
                let ch_sq = chord_sq(line_pnt, line_vec, next);
                if ch_sq > acc.uv_ch {
                    acc.uv_ch = ch_sq;
                }
            }
        }

        // UVDir chord height from the control polygon.
        let cnt = m.min(n);

        // Test the two diagonals from [0, 0] to [cnt, cnt] and from [0, m] to
        // [cnt, m - cnt].
        for jj in 0..2 {
            let iii = 0;
            let (jjj, je, dj): (usize, usize, isize) = if jj == 0 {
                (0, cnt, 1)
            } else {
                (m, m - cnt, -1)
            };
            // `je - dj*itr` / `jjj + dj*itr` (unsigned arithmetic upstream).
            let col = |base: usize, sign: isize, itr: usize| -> Result<usize> {
                let off = isize::try_from(itr).map_err(|_| oob(OP_NET))? * sign;
                base.checked_add_signed(off).ok_or_else(|| oob(OP_NET))
            };

            // Get a base segment from the first to the last control point of this
            // diagonal.
            let line_pnt = pt(sur, iii, jjj)?;
            let last_point = pt(sur, cnt, je)?;
            let mut line_vec = Vec3::new(
                last_point.x - line_pnt.x,
                last_point.y - line_pnt.y,
                last_point.z - line_pnt.z,
            );
            let mut itr = 1;
            while itr < cnt && line_vec.length_squared() < EFF_ZERO_SQ {
                // If first and last point coincide try a middle point.
                let try_point = pt(sur, cnt - itr, col(je, -dj, itr)?)?;
                line_vec = Vec3::new(
                    try_point.x - line_pnt.x,
                    try_point.y - line_pnt.y,
                    try_point.z - line_pnt.z,
                );
                itr += 1;
            }
            // Skip singular diagonal curves.
            if itr == cnt {
                continue;
            }

            // Loop across the diagonal points.
            for itr in 1..=cnt {
                let next = pt(sur, itr, col(jjj, dj, itr)?)?;
                let ch_sq = chord_sq(line_pnt, line_vec, next);
                if ch_sq > acc.uv_ch {
                    acc.uv_ch = ch_sq;
                }
            }
        }
    }

    Ok(acc.finish(req))
}

/// Approximate chord heights and turning angles of a surface in the u, v and
/// diagonal directions from a 5 x 5 grid of evaluations over `uv_domain`
/// (`sm_ComputeSurfConstants`); `eval` is the surface's point evaluator.
///
/// Errors: the evaluator's errors.
pub fn compute_surf_constants(
    eval: &mut PointEvaluator<'_>,
    uv_domain: &Extent2d,
    req: ConstantsRequest,
) -> Result<NetConstants> {
    let mut acc = Acc::default();
    let mut at = |du: f64, dv: f64| eval(uv_domain.evaluate(du, dv));

    // VDir chord height and VDir angle from sample surface evaluations: for all u
    // rows, hold u constant and loop across v.
    for jj in 0..=4_i32 {
        let du = f64::from(jj) / 4.0;
        let mut line_pnt = Vec3::ZERO;
        let mut line_vec = Vec3::ZERO;

        // Chord height.
        if req.v_chord_height {
            line_pnt = at(du, 0.0)?;
            let last_point = at(du, 1.0)?;
            line_vec = last_point - line_pnt;
            let mut try_index: u32 = 3;
            while line_vec.length_squared() < EFF_ZERO_SQ {
                // If first and last point coincide try a middle point.
                let dv = f64::from(try_index) / 4.0;
                let try_point = at(du, dv)?;
                line_vec = try_point - line_pnt;
                if try_index == 0 {
                    // Singular point: just continue on.
                    break;
                }
                try_index -= 1;
            }
            if line_vec.length_squared() < EFF_ZERO_SQ {
                continue; // singular point
            }
        }

        // Angle.
        let mut total_ang_rad = 0.0;
        let mut last_vec = Vec3::ZERO;
        let mut curr = at(du, 0.0)?;
        for ii in 1..=4_i32 {
            let dv = f64::from(ii) / 4.0;
            let next = at(du, dv)?;

            if req.v_chord_height {
                let ch_sq = chord_sq(line_pnt, line_vec, next);
                if ch_sq > acc.v_ch {
                    acc.v_ch = ch_sq;
                }
            }

            if req.v_angle_deg {
                let mut curr_vec = next - curr;
                let dist = curr_vec.length();
                acc.nonfinite_angle_length |= !dist.is_finite();
                if dist < EFF_ZERO {
                    break; // avoid a divide by zero
                }
                curr_vec = curr_vec / dist;
                if last_vec.length_squared() > EFF_ZERO_SQ {
                    total_ang_rad += turn(curr_vec, last_vec);
                }
                last_vec = curr_vec;
            }
            curr = next;
        }
        if total_ang_rad > acc.max_v_ang_rad {
            acc.max_v_ang_rad = total_ang_rad;
        }
    }

    // UDir chord height and UDir angle: for all v columns.
    for jj in 0..=4_i32 {
        let dv = f64::from(jj) / 4.0;
        let mut line_pnt = Vec3::ZERO;
        let mut line_vec = Vec3::ZERO;

        // Chord height.
        if req.u_chord_height {
            line_pnt = at(0.0, dv)?;
            let last_point = at(1.0, dv)?;
            line_vec = last_point - line_pnt;
            let mut try_index: u32 = 3;
            while line_vec.length_squared() < EFF_ZERO_SQ {
                // If first and last point coincide try a middle point.
                let du = f64::from(try_index) / 4.0;
                let try_point = at(du, dv)?;
                line_vec = try_point - line_pnt;
                if try_index == 0 {
                    // Singular point: just continue on.
                    break;
                }
                try_index -= 1;
            }
            if line_vec.length_squared() < EFF_ZERO_SQ {
                continue; // singular point
            }
        }

        // Angle.
        let mut total_ang_rad = 0.0;
        let mut last_vec = Vec3::ZERO;
        let mut curr = at(0.0, dv)?;
        for ii in 1..=4_i32 {
            let du = f64::from(ii) / 4.0;
            let next = at(du, dv)?;

            if req.u_chord_height {
                let ch_sq = chord_sq(line_pnt, line_vec, next);
                if ch_sq > acc.u_ch {
                    acc.u_ch = ch_sq;
                }
            }

            if req.u_angle_deg {
                let mut curr_vec = next - curr;
                let dist = curr_vec.length();
                acc.nonfinite_angle_length |= !dist.is_finite();
                if dist < EFF_ZERO {
                    break;
                }
                curr_vec = curr_vec / dist;
                if ii > 1 {
                    total_ang_rad += turn(curr_vec, last_vec);
                }
                last_vec = curr_vec;
            }
            curr = next;
        }
        if total_ang_rad > acc.max_u_ang_rad {
            acc.max_u_ang_rad = total_ang_rad;
        }
    }

    // Diagonal chord height.
    if req.uv_chord_height {
        // Test the two diagonals from [0, 0] to [1, 1] and from [0, 1] to [1, 0].
        for jj in 0..2_i32 {
            let ds = if jj == 0 { -0.25 } else { 0.25 };
            let jjf = f64::from(jj);
            let mut dv = jjf;

            // Get a base segment from the first to the last corner of this diagonal.
            let line_pnt = at(0.0, dv)?;
            let last_point = at(1.0, 1.0 - dv)?;
            let mut line_vec = last_point - line_pnt;
            let mut itr = 0_i32;
            while itr <= 3 && line_vec.length_squared() < EFF_ZERO_SQ {
                // If first and last point coincide try a middle point.
                let du = 1.0 - 0.25 * f64::from(itr);
                dv = f64::from(1 - jj) + ds * f64::from(itr);
                let try_point = at(du, dv)?;
                line_vec = try_point - line_pnt;
                itr += 1;
            }

            // Skip singular curves.
            if itr == 4 {
                continue;
            }

            // Loop across the diagonal (the first point is evaluated, unused,
            // upstream too).
            at(0.0, jjf)?;
            for itr in 1..=3_i32 {
                let du = 0.0 + 0.25 * f64::from(itr);
                let dv = jjf - ds * f64::from(itr);
                let next = at(du, dv)?;
                let ch_sq = chord_sq(line_pnt, line_vec, next);
                if ch_sq > acc.uv_ch {
                    acc.uv_ch = ch_sq;
                }
            }
        }
    }

    Ok(acc.finish(req))
}

/// Bytes one surface's logical storage takes: `rows` row headers of the Rust control
/// net, its `rows * cols` control points and the `u_len + v_len` knot values; `None`
/// when an intermediate count overflows `usize`.
fn storage_bytes(rows: usize, cols: usize, u_len: usize, v_len: usize) -> Option<usize> {
    rows.checked_mul(size_of::<Vec<CPoint>>())
        .and_then(|b| b.checked_add(rows.checked_mul(cols)?.checked_mul(size_of::<CPoint>())?))
        .and_then(|b| b.checked_add(u_len.checked_add(v_len)?.checked_mul(size_of::<f64>())?))
}

/// A zero-filled surface with highest control point indices `n`, `m`, degrees `p`,
/// `q` and highest knot indices `r`, `s` (`sm_AllocateNurbSurface`, memory from
/// `smos_Calloc`).
///
/// Errors with `InvalidInput` when an index is `usize::MAX` (no storage size), the
/// logical storage (net row headers and points, both knot arrays) exceeds
/// [`limits::MAX_ALLOC_BYTES`], or the storage does not fit in memory (upstream:
/// `smos_Calloc` returns `NULL`).
pub fn allocate_nurb_surface(
    n: usize,
    m: usize,
    p: usize,
    q: usize,
    r: usize,
    s: usize,
) -> Result<Surface> {
    let too_big = || SolidError::InvalidInput {
        operation: OP_ALLOC,
        reason: "index too large",
    };
    let rows = n.checked_add(1).ok_or_else(too_big)?;
    let cols = m.checked_add(1).ok_or_else(too_big)?;
    let u_len = r.checked_add(1).ok_or_else(too_big)?;
    let v_len = s.checked_add(1).ok_or_else(too_big)?;
    let no_memory = || SolidError::InvalidInput {
        operation: OP_ALLOC,
        reason: "storage too large for memory",
    };
    // upstream allocates the whole surface as one smos_Calloc block and returns NULL
    // (NERN) when that fails: bound the logical storage (row headers, points and both
    // knot arrays), then reserve (and release) a block of that size first, so an
    // absurd size fails before any row is allocated
    let total = storage_bytes(rows, cols, u_len, v_len).ok_or_else(too_big)?;
    if !limits::fits::<u8>(total) {
        return Err(no_memory());
    }
    Vec::<u8>::new()
        .try_reserve_exact(total)
        .map_err(|_| no_memory())?;
    let filled = |len: usize, value: f64| -> Result<Vec<f64>> {
        let mut a = Vec::new();
        a.try_reserve_exact(len).map_err(|_| no_memory())?;
        a.resize(len, value);
        Ok(a)
    };
    let mut net: Vec<Vec<CPoint>> = Vec::new();
    net.try_reserve_exact(rows).map_err(|_| no_memory())?;
    for _ in 0..rows {
        let mut row = Vec::new();
        row.try_reserve_exact(cols).map_err(|_| no_memory())?;
        row.resize(cols, CPoint::default());
        net.push(row);
    }
    let u = filled(u_len, 0.0)?;
    let v = filled(v_len, 0.0)?;
    Surface::new(net, p, q, u, v)
}

/// A compact copy of the control net and knots in use
/// (`sm_AllocateAndCopyNurbSurface`).
#[must_use]
pub fn copy_nurb_surface(from: &Surface) -> Surface {
    from.compressed_copy()
}

// ---- Bezier decomposition ---------------------------------------------------------

type Net = Vec<Vec<CPoint>>;

/// One Bezier patch under construction.
#[derive(Clone)]
struct Patch {
    net: Net,
    u: Vec<f64>,
    v: Vec<f64>,
}

fn at1(a: &[f64], i: usize) -> Result<f64> {
    a.get(i).copied().ok_or_else(|| oob(OP_DECOMPOSE))
}

fn put1(a: &mut [f64], i: usize, x: f64) -> Result<()> {
    *a.get_mut(i).ok_or_else(|| oob(OP_DECOMPOSE))? = x;
    Ok(())
}

fn at2(a: &Net, i: usize, j: usize) -> Result<CPoint> {
    a.get(i)
        .and_then(|row| row.get(j))
        .copied()
        .ok_or_else(|| oob(OP_DECOMPOSE))
}

fn put2(a: &mut Net, i: usize, j: usize, x: CPoint) -> Result<()> {
    *a.get_mut(i)
        .and_then(|row| row.get_mut(j))
        .ok_or_else(|| oob(OP_DECOMPOSE))? = x;
    Ok(())
}

fn zeros(rows: usize, cols: usize) -> Net {
    vec![vec![CPoint::default(); cols]; rows]
}

/// Where `NQw` points: the scratch array upstream allocates up front, or a patch.
#[derive(Clone, Copy)]
enum NextNet {
    Scratch,
    Patch(usize),
}

fn patch_mut(patches: &mut [Patch], k: usize) -> Result<&mut Patch> {
    patches.get_mut(k).ok_or_else(|| oob(OP_DECOMPOSE))
}

fn next_net<'a>(
    patches: &'a mut [Patch],
    scratch: &'a mut Net,
    at: NextNet,
) -> Result<&'a mut Net> {
    match at {
        NextNet::Scratch => Ok(scratch),
        NextNet::Patch(k) => Ok(&mut patch_mut(patches, k)?.net),
    }
}

/// Upstream's `NL_QUIT` in [`decompose_srf`] (error flag without a number).
fn quit() -> SolidError {
    SolidError::InvalidInput {
        operation: OP_DECOMPOSE,
        reason: "knot vector has no non-zero span or more breakpoints than spans",
    }
}

/// Decomposes `sur` into its Bezier patches without knot refinement
/// (`sm_DecomposeSrf`). Returns the patches as NURBS surfaces of degrees `(p, q)`
/// with highest knot indices `2p+1`, `2q+1`, indexed `[iu][iv]` over the non-zero
/// u and v spans (upstream's flat order `[patch00, patch01, ..]` row by row).
///
/// Errors: `InvalidInput` when upstream quits (no non-zero span, more u breakpoints
/// than spans) or would index outside its arrays.
pub fn decompose_srf(sur: &Surface) -> Result<Vec<Vec<Surface>>> {
    let (_, m, r, s) = sur.array_sizes();
    let (p, q) = sur.degrees();
    let up = sur.u_knots();
    let vp = sur.v_knots();
    let (knu, knv) = sur.knot_vectors()?;

    // Allocate memory.
    let nsu = knu.span_count(p);
    let nsv = knv.span_count(q);
    if nsu < 1 || nsv < 1 {
        return Err(quit());
    }
    let blank = Patch {
        net: zeros(p + 1, q + 1),
        u: vec![0.0; 2 * p + 2],
        v: vec![0.0; 2 * q + 2],
    };
    let mut patches: Vec<Patch> = vec![blank; nsu * nsv];
    let mut scratch: Net = zeros(p + 1, m + 1);
    let mut nqw = NextNet::Scratch;
    let mut bw: Net = zeros(p + 1, m + 1);
    let mut nbw: Net = zeros(p + 1, m + 1);
    let mut uals = vec![0.0; p + 1];
    let mut omus = vec![0.0; p + 1];
    let mut vals = vec![0.0; q + 1];
    let mut omvs = vec![0.0; q + 1];

    // Initialize for u-directional decomposition.
    let mut isu = p;
    let mut ieu = p + 1;
    let mut iq: isize = -1;

    for i in 0..=p {
        for l in 0..=m {
            put2(&mut bw, i, l, sur.cpt(i, l)?)?;
        }
    }

    // Decompose in the u-direction into Bezier strips.
    while ieu < r {
        iq += 1;

        // Get the knot multiplicity.
        let i0 = ieu;
        while ieu < r && at1(up, ieu)? == at1(up, ieu + 1)? {
            ieu += 1;
        }
        let mlu = ieu - i0 + 1;
        // `ru = p - mlu`, clamped at 0 (upstream fix).
        let ru = p.saturating_sub(mlu);

        // Insert the knot.
        if mlu < p {
            let num = at1(up, ieu)? - at1(up, isu)?;
            let mut i = p;
            while i > mlu {
                let x = num / (at1(up, isu + i)? - at1(up, isu)?);
                put1(&mut uals, i - mlu - 1, x)?;
                put1(&mut omus, i - mlu - 1, 1.0 - x)?;
                i -= 1;
            }

            for i in 1..=ru {
                let su = mlu + i;
                let save = ru - i;
                let mut j = p;
                while j >= su {
                    for l in 0..=m {
                        let c = CPoint::combine2(
                            at1(&uals, j - su)?,
                            at2(&bw, j, l)?,
                            at1(&omus, j - su)?,
                            at2(&bw, j - 1, l)?,
                        );
                        put2(&mut bw, j, l, c)?;
                    }
                    j -= 1;
                }
                if ieu < r {
                    for l in 0..=m {
                        put2(&mut nbw, save, l, at2(&bw, p, l)?)?;
                    }
                }
            }
        }

        // Bezier strip completed. Initialize for v-directional decomposition.
        // This avoids a crash when the knot multiplicity is above the degree.
        let iqu = usize::try_from(iq).map_err(|_| quit())?;
        if iqu >= nsu {
            return Err(quit());
        }

        let mut isv = q;
        let mut iev = q + 1;
        let mut jq: isize = -1;

        {
            let qw = &mut patch_mut(&mut patches, iqu * nsv)?.net;
            for j in 0..=q {
                for k in 0..=p {
                    put2(qw, k, j, at2(&bw, k, j)?)?;
                }
            }
        }

        // Decompose in the v-direction into Bezier patches.
        while iev < s {
            jq += 1;
            let jqu = usize::try_from(jq).map_err(|_| oob(OP_DECOMPOSE))?;
            let cur = iqu * nsv + jqu;
            if jqu + 1 < nsv {
                nqw = NextNet::Patch(cur + 1);
            }

            // Get the knot multiplicity.
            let i0 = iev;
            while iev < s && at1(vp, iev)? == at1(vp, iev + 1)? {
                iev += 1;
            }
            let mlv = iev - i0 + 1;
            let rv = q.saturating_sub(mlv);

            // Insert the knot.
            if mlv < q {
                let num = at1(vp, iev)? - at1(vp, isv)?;
                let mut i = q;
                while i > mlv {
                    let x = num / (at1(vp, isv + i)? - at1(vp, isv)?);
                    put1(&mut vals, i - mlv - 1, x)?;
                    put1(&mut omvs, i - mlv - 1, 1.0 - x)?;
                    i -= 1;
                }

                for i in 1..=rv {
                    let sv = mlv + i;
                    let save = rv - i;
                    {
                        let qw = &mut patch_mut(&mut patches, cur)?.net;
                        let mut j = q;
                        while j >= sv {
                            for k in 0..=p {
                                let c = CPoint::combine2(
                                    at1(&vals, j - sv)?,
                                    at2(qw, k, j)?,
                                    at1(&omvs, j - sv)?,
                                    at2(qw, k, j - 1)?,
                                );
                                put2(qw, k, j, c)?;
                            }
                            j -= 1;
                        }
                    }
                    if iev < s {
                        for k in 0..=p {
                            let x = at2(&patch_mut(&mut patches, cur)?.net, k, q)?;
                            put2(next_net(&mut patches, &mut scratch, nqw)?, k, save, x)?;
                        }
                    }
                }
            }

            // Get the knot vectors.
            {
                let pt = patch_mut(&mut patches, cur)?;
                for i in 0..=p {
                    put1(&mut pt.u, i, at1(up, isu)?)?;
                    put1(&mut pt.u, i + p + 1, at1(up, ieu)?)?;
                }
                for j in 0..=q {
                    put1(&mut pt.v, j, at1(vp, isv)?)?;
                    put1(&mut pt.v, j + q + 1, at1(vp, iev)?)?;
                }
            }

            // Patch completed - prepare for the next Bezier patch.
            if iev < s {
                for i in rv..=q {
                    for k in 0..=p {
                        let col = (iev + i).checked_sub(q).ok_or_else(|| oob(OP_DECOMPOSE))?;
                        let x = at2(&bw, k, col)?;
                        put2(next_net(&mut patches, &mut scratch, nqw)?, k, i, x)?;
                    }
                }
            }
            isv = iev;
            iev += 1;
        }

        // Bezier strip decomposed - prepare for the next strip.
        if ieu < r {
            for i in ru..=p {
                for l in 0..=m {
                    let row = (ieu + i).checked_sub(p).ok_or_else(|| oob(OP_DECOMPOSE))?;
                    put2(&mut nbw, i, l, sur.cpt(row, l)?)?;
                }
            }
        }
        isu = ieu;
        ieu += 1;
        std::mem::swap(&mut bw, &mut nbw);
    }

    let mut rows: Vec<Vec<Surface>> = Vec::with_capacity(nsu);
    let mut it = patches.into_iter();
    for _ in 0..nsu {
        let mut row = Vec::with_capacity(nsv);
        for _ in 0..nsv {
            let pt = it.next().ok_or_else(|| oob(OP_DECOMPOSE))?;
            row.push(Surface::new(pt.net, p, q, pt.u, pt.v)?);
        }
        rows.push(row);
    }
    Ok(rows)
}

// ---- splitting ----------------------------------------------------------------------

/// Splits `sur` at the interior parameter `t` in u ([`Direction::U`]) or v
/// ([`Direction::V`]) (`sm_SplitSrf`). Returns the left part over `[min, t]` and the
/// right part over `[t, max]`.
///
/// Errors: [`NlibCode::Parameter`] for an end or outside parameter;
/// the errors of `N_BasisFindSpanAndMult` and of
/// [`crate::nurbs::srf_tool::p1::split`]; `InvalidInput` for [`Direction::UV`].
pub fn split_srf(sur: &Surface, t: f64, dir: Direction) -> Result<(Surface, Surface)> {
    let (knu, knv) = sur.knot_vectors()?;
    let (p, q) = sur.degrees();

    // Check parameters (the highest indexes upstream computes here are the sizes
    // of the halves `split` returns).
    let (kn, deg) = match dir {
        Direction::U => (knu, p),
        Direction::V => (knv, q),
        Direction::UV => {
            return Err(SolidError::InvalidInput {
                operation: OP_SPLIT,
                reason: "split direction must be U or V",
            })
        }
    };
    if kn.is_end_param(t) {
        return Err(SolidError::nlib(OP_SPLIT, NlibCode::Parameter));
    }
    kn.find_span_and_mult(deg, t, Side::Left)?;

    srf_tool::split(sur, t, dir)
}

// ---- degeneracy ---------------------------------------------------------------------

/// `true` when the box of the surface's control net is point sized within
/// `max(tol3d, 1000 * SM_EFF_ZERO * (1 + max |coord|))` (`sm_IsNSrfDegenerate`;
/// upstream default `tol3d = SM_EFF_ZERO`).
///
/// Errors with the errors of [`Surface::bbox`].
pub fn is_nsrf_degenerate(sur: &Surface, tol3d: f64) -> Result<bool> {
    // For speed - build the bounding box directly from the NLib representation.
    let b = sur.bbox()?;
    let bx = Extent3d::from_bounds(b.xl, b.yb, b.zn, b.xr, b.yt, b.zf);

    // Select a tolerance scaled to the box.
    let scaled_zero = 1000.0 * EFF_ZERO * (1.0 + bx.max_dimension());
    let tol = max2(tol3d, scaled_zero);

    // Test the box for point size.
    Ok(bx.is_point_sized(tol))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;

    fn grid(n: usize, m: usize, f: impl Fn(usize, usize) -> [f64; 3]) -> Net {
        (0..=n)
            .map(|i| {
                (0..=m)
                    .map(|j| {
                        let [x, y, z] = f(i, j);
                        CPoint::new(x, y, z, NOW)
                    })
                    .collect()
            })
            .collect()
    }

    fn clamped(cnt: usize, p: usize, interior: &[f64]) -> Vec<f64> {
        let mut k = vec![0.0; p + 1];
        k.extend_from_slice(interior);
        k.extend(vec![1.0; p + 1]);
        assert_eq!(k.len(), cnt);
        k
    }

    fn bicubic() -> Surface {
        let net = grid(4, 3, |i, j| {
            [i as f64, j as f64, ((i * j) % 3) as f64 * 0.5]
        });
        Surface::new(net, 3, 2, clamped(9, 3, &[0.4]), clamped(7, 2, &[0.5])).unwrap()
    }

    #[test]
    fn plane_has_zero_chord_heights() {
        let net = grid(3, 3, |i, j| [i as f64, j as f64, 0.0]);
        let s = Surface::new(net, 3, 3, clamped(8, 3, &[]), clamped(8, 3, &[])).unwrap();
        let c = compute_net_constants(None, &s, 1e-9, ConstantsRequest::ALL).unwrap();
        assert_eq!(c.u_chord_height, Some(0.0));
        assert_eq!(c.v_chord_height, Some(0.0));
        assert_eq!(c.uv_chord_height, Some(0.0));
        assert_eq!(c.u_angle_deg, Some(0.0));
        assert_eq!(c.v_angle_deg, Some(0.0));
    }

    #[test]
    fn request_selects_outputs() {
        let s = bicubic();
        let req = ConstantsRequest {
            u_chord_height: false,
            v_chord_height: true,
            u_angle_deg: true,
            v_angle_deg: false,
            uv_chord_height: false,
        };
        let c = compute_net_constants(None, &s, 1e-9, req).unwrap();
        assert!(c.u_chord_height.is_none());
        assert!(c.v_chord_height.is_some());
        assert!(c.u_angle_deg.is_some());
        assert!(c.v_angle_deg.is_none());
        assert!(c.uv_chord_height.is_none());
    }

    #[test]
    fn surf_constants_of_a_plane() {
        let dom = Extent2d::from_bounds(0.0, 0.0, 2.0, 3.0);
        let mut f = |uv: Point2| -> Result<Point3> { Ok(Vec3::new(uv.x, uv.y, 0.0)) };
        let c = compute_surf_constants(&mut f, &dom, ConstantsRequest::ALL).unwrap();
        assert_eq!(c.u_chord_height, Some(0.0));
        assert_eq!(c.uv_chord_height, Some(0.0));
        assert_eq!(c.u_angle_deg, Some(0.0));
        assert!(!c.nonfinite_angle_length);
    }

    #[test]
    fn angle_length_diagnostic_uses_differences_not_positions() {
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let angles = ConstantsRequest {
            u_chord_height: false,
            v_chord_height: false,
            uv_chord_height: false,
            ..ConstantsRequest::ALL
        };
        for x in [0.0, 1e155] {
            let mut eval = |uv: Point2| Ok(Vec3::new(x, uv.x, uv.y));
            let c = compute_surf_constants(&mut eval, &dom, angles).unwrap();
            assert!(!c.nonfinite_angle_length, "translation {x}");
            assert_eq!((c.u_angle_deg, c.v_angle_deg), (Some(0.0), Some(0.0)));
        }

        let mut eval = |uv: Point2| Ok(Vec3::new(uv.x * 1e300, uv.y, 0.0));
        let c = compute_surf_constants(&mut eval, &dom, angles).unwrap();
        assert!(c.nonfinite_angle_length);
        // Preserve the original computed outputs, even though division by infinity
        // turns the u-direction into zero and reports spurious 90-degree turns.
        assert_eq!((c.u_angle_deg, c.v_angle_deg), (Some(270.0), Some(0.0)));

        let no_angles = ConstantsRequest {
            u_angle_deg: false,
            v_angle_deg: false,
            ..angles
        };
        let c = compute_surf_constants(&mut eval, &dom, no_angles).unwrap();
        assert!(
            !c.nonfinite_angle_length,
            "unrequested measurements must not run"
        );
    }

    #[test]
    fn net_constants_delegate_to_surface() {
        let s = bicubic();
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let mut f = |uv: Point2| s.eval(uv.x, uv.y, Side::Left, Side::Left);
        let a = compute_net_constants(
            Some((&mut f as &mut PointEvaluator<'_>, &dom)),
            &s,
            1e-9,
            ConstantsRequest::ALL,
        )
        .unwrap();
        let mut g = |uv: Point2| s.eval(uv.x, uv.y, Side::Left, Side::Left);
        let b = compute_surf_constants(&mut g, &dom, ConstantsRequest::ALL).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn decompose_matches_nlib() {
        let s = bicubic();
        let a = decompose_srf(&s).unwrap();
        let b = srf_tool::decompose_to_bez(&s).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a.first().unwrap().len(), 2);
        for (ra, rb) in a.iter().zip(&b) {
            for (pa, pb) in ra.iter().zip(rb) {
                assert_eq!(pa.cpts(), pb.cpts());
                assert_eq!(pa.u_knots(), pb.u_knots());
                assert_eq!(pa.v_knots(), pb.v_knots());
            }
        }
    }

    #[test]
    fn split_checks() {
        let s = bicubic();
        assert!(split_srf(&s, 0.0, Direction::U).is_err());
        assert!(split_srf(&s, 0.5, Direction::UV).is_err());
        let (l, r) = split_srf(&s, 0.3, Direction::V).unwrap();
        assert_eq!(l.v_knots().last(), Some(&0.3));
        assert_eq!(r.v_knots().first(), Some(&0.3));
    }

    #[test]
    fn alloc_copy_degenerate() {
        let z = allocate_nurb_surface(2, 3, 1, 2, 4, 6).unwrap();
        assert_eq!(z.array_sizes(), (2, 3, 4, 6));
        assert!(is_nsrf_degenerate(&z, EFF_ZERO).unwrap());
        let s = bicubic();
        assert_eq!(copy_nurb_surface(&s), s);
        assert!(!is_nsrf_degenerate(&s, EFF_ZERO).unwrap());
        assert!(is_nsrf_degenerate(&s, 100.0).unwrap());
    }

    #[test]
    fn allocate_absurd_surface_is_an_error() {
        // fuzz regressions (nurbs.nurbs_srf fuzz00748, fuzz01665): absurd sizes
        // aborted the allocation
        // (sizes beyond the 47-bit address space, so no test machine can grant them)
        assert!(allocate_nurb_surface(1 << 46, 1, 1, 1, 3, 3).is_err());
        assert!(allocate_nurb_surface(1 << 62, 1 << 62, 1, 1, 3, 3).is_err());
        assert!(allocate_nurb_surface(1, 1, 1, 1, 1 << 47, 3).is_err());
        assert!(allocate_nurb_surface(1, 1, 1, 1, 3, 3).is_ok());
    }

    #[test]
    fn alloc_zero_fills_small_surface() {
        let s = allocate_nurb_surface(1, 1, 1, 1, 3, 3).unwrap();
        assert_eq!(s.array_sizes(), (1, 1, 3, 3));
        assert!(s.cpts().iter().flatten().all(|c| *c == CPoint::ZERO));
        assert!(s.u_knots().iter().all(|k| *k == 0.0));
        assert!(s.v_knots().iter().all(|k| *k == 0.0));
    }

    #[test]
    fn storage_bytes_counts_headers_and_payload() {
        let (rows, cols, u_len, v_len) = (2, 3, 4, 5);
        let payload = rows * cols * size_of::<CPoint>() + (u_len + v_len) * size_of::<f64>();
        let expected = rows * size_of::<Vec<CPoint>>() + payload;
        assert_eq!(storage_bytes(rows, cols, u_len, v_len), Some(expected));
        // The row headers are not in the point and knot payload alone.
        assert!(expected > payload);
    }

    #[test]
    fn storage_boundary_uses_one_surface_budget() {
        let fixed = size_of::<Vec<CPoint>>() + 2 * size_of::<f64>();
        let cols = (limits::MAX_ALLOC_BYTES - fixed) / size_of::<CPoint>();
        let at_cap = storage_bytes(1, cols, 1, 1).unwrap();
        assert!(at_cap <= limits::MAX_ALLOC_BYTES);
        assert!(limits::fits::<u8>(at_cap));
        let over = storage_bytes(1, cols + 1, 1, 1).unwrap();
        assert!(over > limits::MAX_ALLOC_BYTES);
        assert!(!limits::fits::<u8>(over));
    }

    #[test]
    fn tall_narrow_net_headers_count() {
        let knots = 2 * size_of::<f64>();
        let per_row = size_of::<CPoint>() + size_of::<Vec<CPoint>>();
        let rows = (limits::MAX_ALLOC_BYTES - knots) / per_row + 1;
        let total = storage_bytes(rows, 1, 1, 1).unwrap();
        assert!(total > limits::MAX_ALLOC_BYTES);
        // Without the row headers this tall net would look affordable.
        assert!(rows * size_of::<CPoint>() + knots <= limits::MAX_ALLOC_BYTES);
    }

    #[test]
    fn storage_bytes_overflow_is_none() {
        assert_eq!(storage_bytes(usize::MAX, 1, 1, 1), None); // row headers
        assert_eq!(storage_bytes(1, usize::MAX, 1, 1), None); // point product
        assert_eq!(storage_bytes(1, 1, usize::MAX, 1), None); // knot sum
    }

    #[test]
    fn allocate_over_cap_is_refused_before_allocation() {
        let fixed = size_of::<Vec<CPoint>>() + 2 * size_of::<f64>();
        let cols = (limits::MAX_ALLOC_BYTES - fixed) / size_of::<CPoint>();
        // One column past the largest one-row surface that fits the logical budget.
        let err = allocate_nurb_surface(0, cols, 1, 1, 0, 0).unwrap_err();
        assert!(matches!(
            err,
            SolidError::InvalidInput {
                operation: OP_ALLOC,
                reason: "storage too large for memory",
            }
        ));
    }

    #[test]
    fn allocate_overflow_is_index_too_large() {
        let point_overflow = usize::MAX / 2 + 1;
        for (n, m, r, s) in [
            (usize::MAX, 1, 0, 0),
            (0, point_overflow, 0, 0),
            (0, 0, usize::MAX - 1, usize::MAX - 1),
        ] {
            let err = allocate_nurb_surface(n, m, 1, 1, r, s).unwrap_err();
            assert!(matches!(
                err,
                SolidError::InvalidInput {
                    operation: OP_ALLOC,
                    reason: "index too large",
                }
            ));
        }
    }
}
