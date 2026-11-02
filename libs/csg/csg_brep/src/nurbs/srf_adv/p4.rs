//! Coons surfaces (bilinear and bicubic), cross-boundary derivative construction and
//! approximation, derivative strip adjustment, sweep frame helpers and knot vector
//! merging.
//!
//! Upstream: `source/SMLib/src/NL_SrfAdv.cpp`, `source/SMLib/inc/NL_SrfAdv.h`.
//!
//! | Rust                                   | Upstream                         |
//! |----------------------------------------|----------------------------------|
//! | [`create_b_vectors`]                   | `N_CreateBVectors`               |
//! | [`create_coons_srf`]                   | `N_CreateCoonsSrf`               |
//! | [`create_skin_srf_boundary_derivs`]    | `N_CreateSkinSrfBoundaryDerivs`  |
//! | [`merge_knot_vectors`]                 | `N_MergeKnotVectors`             |
//! | [`rotation_rate_of_change`], [`ZSource`] | `ST_GetRotationRateOfChange`   |
//! | [`sweep_cross_tangent_crv`]            | `N_SweepCrossTangentCrv`         |
//! | [`adjust_deriv_srf`]                   | `N_AdjustDerivSrf`               |
//! | [`adjust_weight_scale`]                | `N_AdjustWeightScale`            |
//! | [`approx_cross_boundary_derivs`]       | `N_ApproxCrossBoundaryDerivs`    |
//! | [`create_coons_boundary_crvs`], [`CoonsBoundary`] | `N_CreateCoonsBoundaryCrvs` |
//! | [`create_cross_boundary_deriv_crv`]    | `N_CreateCrossBoundaryDerivCrv`  |
//! | [`Mat3`]                               | `NL_RMATRIX` (3x3)               |
//!
//! Deviations:
//! - Out-params become return values (new curves, surfaces, vectors and matrices);
//!   the `NL_STACKS` arguments have no counterpart. Inputs that upstream modifies in
//!   place (made rational, reparametrised or compatible, a 2D curve made 3D, a knot
//!   inserted) are `&mut` and modified the same way. Upstream's in-place modes
//!   (`surP == surQ`, `derP == derQ`) compute the same values as the copying modes,
//!   so the caller replaces its input with the returned value.
//! - Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number. A bare
//!   upstream error flag (`NL_QUIT` without an error number: the sanity exit of
//!   `N_MergeKnotVectors`, the missing-input exit of `ST_GetRotationRateOfChange`) is
//!   `InvalidInput` with reason [`BARE_FLAG_REASON`].
//! - `ST_GetRotationRateOfChange`'s optional pointers become a [`ZSource`]: a curve, or
//!   a vector with its optional neighbours (both missing is the bare error). The rate
//!   matrix `RM[i][j]` is a `[[f64; 3]; 3]` (rows `i`).
//! - `N_SweepCrossTangentCrv` ignores failures of `N_CrvEval` on the cross section
//!   (the accumulated sample parameter can round past the last knot) and keeps the
//!   previous sample, as does the Rust function; only a failure on the very first
//!   sample (upstream reads an uninitialised point) is returned. A cross section
//!   without a knot span longer than `NL_PTOL` (upstream then fits uninitialised data)
//!   is `InvalidInput`.
//! - `N_ApproxCrossBoundaryDerivs` ignores failures of `N_VectorsAngle` (using an
//!   uninitialised angle) and, when one end angle equals `eps` exactly while the
//!   other exceeds it, refines with an uninitialised knot; the Rust function returns
//!   the angle error, and `InvalidInput` in the second case.
//! - `N_MergeKnotVectors` loops forever on knots that compare unordered (NaN); the
//!   Rust function returns `InvalidInput` when an iteration makes no progress.
//! - `N_SrfReparam`'s status is ignored upstream; [`Surface::reparam`] fails only
//!   where upstream would write outside its arrays, and the error is returned.
//! - `N_AdjustDerivSrf` with a direction other than u or v passes `NL_NO` on to
//!   `N_BezSrfExtend4D`; the Rust function returns `InvalidInput` where an extension
//!   in that direction is needed.
//! - Every array access is bounds checked; where upstream would index outside its
//!   arrays (curves with fewer control points than read, empty parameter lists) the
//!   functions return [`SolidError::InvalidInput`].
//! - `N_SweepCrossTangentCrv`: a cross section degree above `32767` (upstream's
//!   `NL_DEGREE` is a `short`) is `InvalidInput`; sample arrays that cannot be
//!   allocated are `NL_MEM_ERR`.

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_basic::PTOL;
use crate::nurbs::bezier::{srf_extend_4d, ExtendAt};
use crate::nurbs::crv_adv::p1::BARE_FLAG_REASON;
use crate::nurbs::crv_adv::p2::SumDiff;
use crate::nurbs::crv_basic::{curves_are_compatible, Boundary, Curve};
use crate::nurbs::crv_fit::p1::approx_knots;
use crate::nurbs::crv_tool::p2::make_compatible;
use crate::nurbs::funcs_basic::CurveFunction;
use crate::nurbs::geom::{
    dist_pt_pt, isect_line_line, normalize, pt_along_vector, vectors_angle, LineSeg, PI,
};
use crate::nurbs::index::{at, at2, oob, put, put2};
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{combine2_pts, float_op_is_bad, CPoint, FloatOp, MODEL_TOL};
use crate::nurbs::srf_basic::{Direction, Surface, WEIGHT_TOL};
use crate::nurbs::srf_common::{corner_pts, extrude_crv, ruled_srf};
use crate::nurbs::srf_tool::p4::make_compatible_uv;

const OP_BVEC: &str = "nurbs::srf_adv::p4::create_b_vectors";
const OP_COONS: &str = "nurbs::srf_adv::p4::create_coons_srf";
const OP_MERGE: &str = "nurbs::srf_adv::p4::merge_knot_vectors";
const OP_ROT: &str = "nurbs::srf_adv::p4::rotation_rate_of_change";
const OP_SWEEP_CT: &str = "nurbs::srf_adv::p4::sweep_cross_tangent_crv";
const OP_ADJ_DERIV: &str = "nurbs::srf_adv::p4::adjust_deriv_srf";
const OP_ADJ_WEIGHT: &str = "nurbs::srf_adv::p4::adjust_weight_scale";
const OP_APPROX: &str = "nurbs::srf_adv::p4::approx_cross_boundary_derivs";
const OP_COONS_BND: &str = "nurbs::srf_adv::p4::create_coons_boundary_crvs";
const OP_CROSS: &str = "nurbs::srf_adv::p4::create_cross_boundary_deriv_crv";

/// A 3x3 matrix `RM[i][j]` (upstream `NL_RMATRIX` of size 3x3), rows first.
pub type Mat3 = [[f64; 3]; 3];

fn bare(operation: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation,
        reason: BARE_FLAG_REASON,
    }
}

/// `CD[1]` of `cur` at `u` (`N_CrvDerivs` with `der = 1`), with `CD[0]`.
fn first_derivs(cur: &Curve, u: f64, op: &'static str) -> Result<(Vec3, Vec3)> {
    let cd = cur.derivs(u, Side::Left, 1)?;
    Ok((at(&cd, 0, op)?, at(&cd, 1, op)?))
}

/// The first and last knot in use.
fn ends(k: &[f64], op: &'static str) -> Result<(f64, f64)> {
    match (k.first(), k.last()) {
        (Some(&a), Some(&b)) => Ok((a, b)),
        _ => Err(oob(op)),
    }
}

// ---- N_CreateBVectors ---------------------------------------------------------------

/// The "B" vectors of the projection normal method along `cur` at the parameters `t`
/// (upstream `N_CreateBVectors`): `B[0]` is the unit `b0`, and each next one is
/// the previous one minus its component along the unit tangent at `t[i]`,
/// normalised. On a closed curve whose last vector misses the first by more than
/// `NL_MTOL`, the last is reset to the first and the others averaged backwards.
///
/// # Errors
///
/// [`NlibCode::Geometry`] when a vector cannot be normalised; errors of
/// [`Curve::eval_tangent`]; `InvalidInput` for an empty `t`.
pub fn create_b_vectors(cur: &Curve, b0: Vec3, t: &[f64]) -> Result<Vec<Vec3>> {
    const OP: &str = OP_BVEC;
    let k = t.len().checked_sub(1).ok_or_else(|| oob(OP))?;
    let mut b = vec![Vec3::ZERO; k + 1];
    let mut tg = vec![Vec3::ZERO; k + 1];

    // Initialize.
    let (b0, _) = normalize(b0)?;
    put(&mut b, 0, b0, OP)?;

    // Compute unit tangents and the B's in one direction.
    for i in 1..=k {
        let (_, ti) = cur.eval_tangent(at(t, i, OP)?, Side::Left)?;
        put(&mut tg, i, ti, OP)?;
        // let B[i] = B[i-1] - (B[i-1]*T[i])*T[i]
        let bp = at(&b, i - 1, OP)?;
        let dot = bp.dot(ti);
        let (bi, _) = normalize(combine2_pts(1.0, bp, -dot, ti))?;
        put(&mut b, i, bi, OP)?;
    }

    // Correct for closed curves.
    if cur.is_closed() {
        let dis = dist_pt_pt(at(&b, 0, OP)?, at(&b, k, OP)?);
        if dis > MODEL_TOL {
            let first = at(&b, 0, OP)?;
            put(&mut b, k, first, OP)?;
            for i in (2..=k).rev() {
                let bi = at(&b, i, OP)?;
                let ti = at(&tg, i - 1, OP)?;
                let dot = bi.dot(ti);
                let (a, _) = normalize(combine2_pts(1.0, bi, -dot, ti))?;
                let avg = combine2_pts(0.5, at(&b, i - 1, OP)?, 0.5, a);
                let (bm, _) = normalize(avg)?;
                put(&mut b, i - 1, bm, OP)?;
            }
        }
    }
    Ok(b)
}

// ---- N_CreateCoonsSrf ---------------------------------------------------------------

/// Bilinear Coons surface of four boundary curves (upstream `N_CreateCoonsSrf`):
/// `cur_u[0]` / `cur_u[1]` are the boundaries `v = vmin` / `v = vmax` (running in
/// u), `cur_v[0]` / `cur_v[1]` the boundaries `u = umin` / `u = umax`. The corners
/// are the midpoints of the curves' end points. If any curve is rational all four
/// are made rational and reparametrised in place so that their end weights equal
/// the mean end weight; the ruled surfaces make `cur_v` and `cur_u` compatible in
/// place as well.
///
/// # Errors
///
/// Errors of [`Curve::eval`], [`Curve::reparam_weights`], [`ruled_srf`],
/// [`Surface::reparam`] and [`make_compatible_uv`]; `InvalidInput` for curves
/// without knots.
pub fn create_coons_srf(cur_u: &mut [Curve; 2], cur_v: &mut [Curve; 2]) -> Result<Surface> {
    const OP: &str = OP_COONS;
    let side = Side::Left;

    // Get corner points.
    let corner = |a: &Curve, ua: bool, b: &Curve, vb: bool| -> Result<Vec3> {
        let (a0, a1) = ends(a.knots(), OP)?;
        let (b0, b1) = ends(b.knots(), OP)?;
        let cu = a.eval(if ua { a1 } else { a0 }, side)?;
        let cv = b.eval(if vb { b1 } else { b0 }, side)?;
        Ok(combine2_pts(0.5, cu, 0.5, cv))
    };
    let p00 = corner(&cur_u[0], false, &cur_v[0], false)?;
    let p10 = corner(&cur_u[0], true, &cur_v[1], false)?;
    let p01 = corner(&cur_u[1], false, &cur_v[0], true)?;
    let p11 = corner(&cur_u[1], true, &cur_v[1], true)?;

    // Check rationality of boundaries.
    let allnra = !cur_u.iter().chain(cur_v.iter()).any(Curve::is_rational);

    let mut w = 0.0;
    if !allnra {
        // Make all curves rational.
        for c in cur_u.iter_mut().chain(cur_v.iter_mut()) {
            if !c.is_rational() {
                c.non_rat_to_rat();
            }
        }

        // Average end weights.
        let mut sum = 0.0;
        for (cu, cv) in cur_u.iter().zip(cur_v.iter()) {
            for c in [cu, cv] {
                let aw = c.cpts();
                sum += at(aw, 0, OP)?.w;
                sum += at(aw, c.n(), OP)?.w;
            }
        }

        // Reparametrize to get end weights = w.
        w = sum / 8.0;
        for (cu, cv) in cur_u.iter_mut().zip(cur_v.iter_mut()) {
            for c in [cu, cv] {
                let (a, b) = ends(c.knots(), OP)?;
                c.reparam_weights(w, a, b)?;
            }
        }
    }

    // U-ruling.
    let [v0, v1] = &mut *cur_v;
    let mut sur_u = ruled_srf(v0, v1, Direction::U)?;
    // V-ruling.
    let [u0, u1] = &mut *cur_u;
    let mut sur_v = ruled_srf(u0, u1, Direction::V)?;
    // Bilinear surface.
    let mut sur_t = corner_pts(p00, p10, p01, p11)?;

    // Ensure surface has same parameter domain as curves.
    let (us, ue) = cur_u[0].param_bounds();
    let (vs, ve) = cur_v[0].param_bounds();
    sur_u.reparam(us, ue, vs, ve)?;
    sur_v.reparam(us, ue, vs, ve)?;
    sur_t.reparam(us, ue, vs, ve)?;

    // Scale control points of surT if necessary.
    if !allnra {
        for (i, j) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let c = sur_t.cpt_mut(i, j)?;
            c.w = 1.0;
            *c = c.scale(w);
        }
    }

    // Make surfaces compatible.
    let mut sur_a = [sur_u, sur_v, sur_t];
    make_compatible_uv(&mut sur_a)?;

    // Compute output surface.
    let [sa, sb, sc] = &sur_a;
    combine_coons(sa, sb, sc, OP)
}

/// `Uw + Vw - Tw` over the net of `sa`, with `sa`'s degrees and knots.
fn combine_coons(sa: &Surface, sb: &Surface, sc: &Surface, op: &'static str) -> Result<Surface> {
    let idx = sa.indices();
    let mut net = Vec::with_capacity(idx.n + 1);
    for i in 0..=idx.n {
        let mut row = Vec::with_capacity(idx.m + 1);
        for j in 0..=idx.m {
            row.push(CPoint::translate_sum2(
                sa.cpt(i, j)?,
                1.0,
                sb.cpt(i, j)?,
                -1.0,
                sc.cpt(i, j)?,
            ));
        }
        net.push(row);
    }
    let u = sa.u_knots().to_vec();
    let v = sa.v_knots().to_vec();
    if u.len() != idx.r + 1 || v.len() != idx.s + 1 {
        return Err(oob(op));
    }
    Surface::new(net, idx.p, idx.q, u, v)
}

// ---- N_CreateSkinSrfBoundaryDerivs --------------------------------------------------

/// Cross-boundary derivative strip of a curve and a constant vector (upstream
/// `N_CreateSkinSrfBoundaryDerivs`): `cur` extruded along `v` (reversed for
/// [`Boundary::Right`] and [`Boundary::Top`]) by its length, linear in u for
/// [`Boundary::Left`] / [`Boundary::Right`] and in v otherwise; for `Right` / `Top`
/// the two rows (columns) of the strip are swapped. A 2D `cur` is made 3D in place.
///
/// # Errors
///
/// Errors of [`extrude_crv`] ([`NlibCode::Geometry`] for a zero `v`).
pub fn create_skin_srf_boundary_derivs(
    cur: &mut Curve,
    v: Vec3,
    bndy: Boundary,
) -> Result<Surface> {
    // Handle four cases.
    let dir = match bndy {
        Boundary::Left | Boundary::Right => Direction::U,
        Boundary::Bottom | Boundary::Top => Direction::V,
    };
    let v2 = match bndy {
        Boundary::Left | Boundary::Bottom => v,
        Boundary::Right | Boundary::Top => -v,
    };

    // Create derivative surface: a cylinder by extruding cur in the V2 direction.
    let mag = v2.length();
    let der = extrude_crv(cur, v2, mag, dir)?;

    // Reverse direction if bndy = NL_RIGHT or NL_TOP.
    if !matches!(bndy, Boundary::Right | Boundary::Top) {
        return Ok(der);
    }
    let idx = der.indices();
    let mut net = der.cpts();
    if bndy == Boundary::Right {
        if net.len() < 2 {
            return Err(oob("nurbs::srf_adv::p4::create_skin_srf_boundary_derivs"));
        }
        net.swap(0, 1);
    } else {
        for row in &mut net {
            if row.len() < 2 {
                return Err(oob("nurbs::srf_adv::p4::create_skin_srf_boundary_derivs"));
            }
            row.swap(0, 1);
        }
    }
    Surface::with_indices(net, der.u_knots().to_vec(), der.v_knots().to_vec(), idx)
}

// ---- N_MergeKnotVectors -------------------------------------------------------------

/// Knots to insert into two knot vectors with the same end knots (within `NL_PTOL`)
/// to merge them (upstream `N_MergeKnotVectors`): `(XR, XS)`, the knots missing
/// from `knr` and from `kns`, each in increasing order. Interior knots within
/// `NL_PTOL * (R[mr] - R[0])` of each other are considered equal (the value of the
/// other vector is inserted to make up a multiplicity difference).
///
/// # Errors
///
/// [`NlibCode::Input`] when the end knots differ; `InvalidInput` with reason
/// [`BARE_FLAG_REASON`] for upstream's sanity exit (an index past the knots) or
/// overflow of the starting index `p + 1`;
/// `InvalidInput` for knots that compare unordered.
pub fn merge_knot_vectors(
    knr: &KnotVector<'_>,
    kns: &KnotVector<'_>,
    p: usize,
) -> Result<(Vec<f64>, Vec<f64>)> {
    const OP: &str = OP_MERGE;
    let rk = knr.knots();
    let sk = kns.knots();
    let mr = knr.last_index();
    let ms = kns.last_index();
    let r = |i: usize| at(rk, i, OP);
    let s = |j: usize| at(sk, j, OP);

    if (r(0)? - s(0)?).abs() > PTOL || (r(mr)? - s(ms)?).abs() > PTOL {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    let tol = PTOL * (r(mr)? - r(0)?);
    let mut xr = Vec::new();
    let mut xs = Vec::new();
    let first = p.checked_add(1).ok_or_else(|| bare(OP))?;
    let mut ii = first;
    let mut jj = first;
    // `ii < mr - p` with upstream's signed arithmetic.
    let lt = |i: usize, mx: usize| i.checked_add(p).is_some_and(|ip| ip < mx);

    // Find knots to be inserted.
    while lt(ii, mr) || lt(jj, ms) {
        // Sanity check: avoid infinite loop. With bad data, these can slip past.
        if ii > mr || jj > ms {
            return Err(bare(OP));
        }

        let (ri, sj) = (r(ii)?, s(jj)?);
        if (ri - sj).abs() < tol {
            let ir = ii;
            while lt(ii, mr) && r(ii)? == r(ii + 1)? {
                ii += 1;
            }
            let mult_r = ii - ir + 1;

            let js = jj;
            while lt(jj, ms) && s(jj)? == s(jj + 1)? {
                jj += 1;
            }
            let mult_s = jj - js + 1;

            for _ in mult_r + 1..=mult_s {
                xr.push(s(jj)?);
            }
            for _ in mult_s + 1..=mult_r {
                xs.push(r(ii)?);
            }
            ii += 1;
            jj += 1;
        } else if ri < sj {
            let ir = ii;
            while lt(ii, mr) && r(ii)? == r(ii + 1)? {
                ii += 1;
            }
            let mult_r = ii - ir + 1;
            for _ in 1..=mult_r {
                xs.push(r(ii)?);
            }
            ii += 1;
        } else if ri > sj {
            let js = jj;
            while lt(jj, ms) && s(jj)? == s(jj + 1)? {
                jj += 1;
            }
            let mult_s = jj - js + 1;
            for _ in 1..=mult_s {
                xr.push(s(jj)?);
            }
            jj += 1;
        } else {
            // Unordered (NaN) knots: upstream loops forever.
            return Err(SolidError::InvalidInput {
                operation: OP,
                reason: "knots compare unordered",
            });
        }
    }
    Ok((xr, xs))
}

// ---- ST_GetRotationRateOfChange -----------------------------------------------------

/// Where [`rotation_rate_of_change`] takes the frame's z axis and its derivative
/// from (upstream's `curZ` or `Z`, `prevZ`, `nextZ` arguments).
#[derive(Debug, Clone, Copy)]
pub enum ZSource<'a> {
    /// `curZ`: the unit z axis and its derivative come from the curve at `u`.
    Curve(&'a Curve),
    /// `Z` with the discrete neighbours `(prevZ, prev_u)` and `(nextZ, next_u)` for a
    /// finite difference (central with both, one-sided with one).
    Samples {
        /// `Z`, used as is.
        z: Vec3,
        /// `prevZ` and its parameter.
        prev: Option<(Vec3, f64)>,
        /// `nextZ` and its parameter.
        next: Option<(Vec3, f64)>,
    },
}

/// Rate of change `[x' | y' | z']` of the sweep frame `M = [x | y | z]` at `u`
/// (upstream `ST_GetRotationRateOfChange`), where `x = T'/|T'|` for the trajectory
/// `T`, `z` comes from `zsrc`, and `y = z x x`. Column 0 is `x'`, column 1
/// `y' = z' x x + z x x'`, column 2 `z'`.
///
/// # Errors
///
/// Errors of [`Curve::derivs`]; `InvalidInput` with reason [`BARE_FLAG_REASON`] for
/// [`ZSource::Samples`] without neighbours (upstream's return code 1).
pub fn rotation_rate_of_change(traj: &Curve, u: f64, zsrc: ZSource<'_>) -> Result<Mat3> {
    const OP: &str = OP_ROT;
    let (m_z, dm_z) = match zsrc {
        ZSource::Curve(cur_z) => {
            // Evaluate z and z'.
            let (z0, z1) = first_derivs(cur_z, u, OP)?;
            let d12 = z0.x * z1.x + z0.y * z1.y + z0.z * z1.z;
            let mag = (z0.x * z0.x + z0.y * z0.y + z0.z * z0.z).sqrt();
            let mag3 = mag * mag * mag;
            // Save normalized Z.
            let m_z = Vec3::new(z0.x / mag, z0.y / mag, z0.z / mag);
            // With x = z/|z|, dx = z'/|z| - z*dot(z,z')/|z|**3.
            let dm_z = Vec3::new(
                z1.x / mag - z0.x * d12 / mag3,
                z1.y / mag - z0.y * d12 / mag3,
                z1.z / mag - z0.z * d12 / mag3,
            );
            (m_z, dm_z)
        }
        ZSource::Samples { z, prev, next } => {
            if prev.is_none() && next.is_none() {
                return Err(bare(OP));
            }
            // Finite differences for dZ.
            let (f1, du1) = match prev {
                Some((pz, pu)) => (pz, pu - u),
                None => (z, 0.0),
            };
            let (f2, du2) = match next {
                Some((nz, nu)) => (nz, nu - u),
                None => (z, 0.0),
            };
            let dm_z = Vec3::new(
                (f1.x - f2.x) / (du1 - du2),
                (f1.y - f2.y) / (du1 - du2),
                (f1.z - f2.z) / (du1 - du2),
            );
            (z, dm_z)
        }
    };

    // Get sweep trajectory evaluations.
    let t = traj.derivs(u, Side::Left, 2)?;
    let (t1, t2) = (at(&t, 1, OP)?, at(&t, 2, OP)?);
    let mag = (t1.x * t1.x + t1.y * t1.y + t1.z * t1.z).sqrt();
    let mag3 = mag * mag * mag;
    let d12 = t1.x * t2.x + t1.y * t2.y + t1.z * t2.z;

    // X = t'/|t'|.
    let x = Vec3::new(t1.x / mag, t1.y / mag, t1.z / mag);
    // dX = t''/|t'| - t' (t'.t'')/|t'|**3.
    let dx = Vec3::new(
        t2.x / mag - t1.x * d12 / mag3,
        t2.y / mag - t1.y * d12 / mag3,
        t2.z / mag - t1.z * d12 / mag3,
    );

    // dy = cross_product(z',x) + cross_product(z,x').
    let dy = Vec3::new(
        (dm_z.y * x.z - dm_z.z * x.y) + (m_z.y * dx.z - m_z.z * dx.y),
        (dm_z.z * x.x - dm_z.x * x.z) + (m_z.z * dx.x - m_z.x * dx.z),
        (dm_z.x * x.y - dm_z.y * x.x) + (m_z.x * dx.y - m_z.y * dx.x),
    );
    Ok([
        [dx.x, dy.x, dm_z.x],
        [dx.y, dy.y, dm_z.y],
        [dx.z, dy.z, dm_z.z],
    ])
}

// ---- N_SweepCrossTangentCrv ---------------------------------------------------------

/// Cross-tangent curve of a sweep surface `S(u, v) = T(v) + M(v) C(u)` at one
/// trajectory parameter (upstream `N_SweepCrossTangentCrv`): `Sv = T' + M' C(u)` with
/// `M' = [scale_j * drma[.][j] + dscale_j * rma[.][j]]`, sampled `p` times per
/// knot span of the cross section (`p + 1` on the last span) and fitted by least
/// squares on the cross section's knot vector and degree.
///
/// # Errors
///
/// Errors of [`Curve::eval`] and [`approx_knots`]; `InvalidInput` for a cross
/// section of degree zero or without a span longer than `NL_PTOL`.
pub fn sweep_cross_tangent_crv(
    dtrajectory: Vec3,
    scale: Vec3,
    dscale: Vec3,
    rma: &Mat3,
    drma: &Mat3,
    cross_section: &Curve,
) -> Result<Curve> {
    const OP: &str = OP_SWEEP_CT;
    let pc = super::degree(cross_section.degree(), OP)?;
    let (nc, mc) = cross_section.array_sizes();
    let knt = cross_section.knot_vector()?;
    let uk = cross_section.knots();
    if pc == 0 {
        return Err(SolidError::InvalidInput {
            operation: OP,
            reason: "cross section of degree zero",
        });
    }

    // Count cross_section segments.
    let mut segment_count = 0usize;
    for i1 in 1..=mc {
        if at(uk, i1, OP)? - at(uk, i1 - 1, OP)? > PTOL {
            segment_count += 1;
        }
    }
    if segment_count == 0 {
        return Err(SolidError::InvalidInput {
            operation: OP,
            reason: "cross section without a knot span",
        });
    }
    let sample_count = pc
        .checked_mul(segment_count)
        .and_then(|x| x.checked_add(1))
        .ok_or_else(|| oob(OP))?;
    let mut cross_tangents = super::filled(sample_count, Vec3::ZERO, OP)?;
    let mut params = super::filled(sample_count, 0.0, OP)?;

    let mut sample: Option<Vec3> = None;
    let mut i_smp = 0usize;
    let mut i_seg = 0usize;
    for i1 in 1..=mc {
        let (k0, k1) = (at(uk, i1 - 1, OP)?, at(uk, i1, OP)?);
        if k1 - k0 > PTOL {
            // Sample each segment pc times (last segment gets pc + 1).
            let cnt = pc + usize::from(i_seg == segment_count - 1);
            let du = (k1 - k0) / pc as f64;
            put(&mut params, i_smp, k0, OP)?;
            for _ in 0..cnt {
                // Sample the cross section; upstream ignores a failed evaluation (a
                // parameter rounded past the last knot) and keeps the last sample.
                match cross_section.eval(at(&params, i_smp, OP)?, Side::Left) {
                    Ok(s) => sample = Some(s),
                    Err(e) if sample.is_none() => return Err(e),
                    Err(_) => {}
                }
                let sample = sample.ok_or_else(|| oob(OP))?;
                // Sv(u,v) = Tv(v) + Mv(v)C(u).
                let [r0, r1, r2] = *rma;
                let [d0, d1, d2] = *drma;
                let ct = Vec3::new(
                    cross_tangent_row(dtrajectory.x, scale, dscale, d0, r0, sample),
                    cross_tangent_row(dtrajectory.y, scale, dscale, d1, r1, sample),
                    cross_tangent_row(dtrajectory.z, scale, dscale, d2, r2, sample),
                );
                put(&mut cross_tangents, i_smp, ct, OP)?;
                i_smp += 1;
                if i_smp < sample_count {
                    let prev = at(&params, i_smp - 1, OP)?;
                    put(&mut params, i_smp, prev + du, OP)?;
                }
            }
            i_seg += 1;
        }
    }

    // Build cross-tangent curve approximating the sampled cross-tangent values.
    approx_knots(&cross_tangents, &params, &knt, nc, pc)
}

/// One coordinate of `Tv + Mv C`: `dt + (sx*dr0 + dsx*rr0)*C.x + ... `.
fn cross_tangent_row(
    dt: f64,
    scale: Vec3,
    dscale: Vec3,
    dr: [f64; 3],
    rr: [f64; 3],
    c: Vec3,
) -> f64 {
    let [dr0, dr1, dr2] = dr;
    let [rr0, rr1, rr2] = rr;
    dt + (scale.x * dr0 + dscale.x * rr0) * c.x
        + (scale.y * dr1 + dscale.y * rr1) * c.y
        + (scale.z * dr2 + dscale.z * rr2) * c.z
}

// ---- N_AdjustDerivSrf ---------------------------------------------------------------

/// Adjusts a derivative strip that is Bezier in `dir` (upstream `N_AdjustDerivSrf`):
/// constructed along the boundary `cst`, the result is the strip for use along `usg`
/// (a copy, the u and v directions swapped, and/or the strip extended in
/// homogeneous space with [`srf_extend_4d`] to flip its Bezier direction).
///
/// # Errors
///
/// Errors of [`srf_extend_4d`] ([`NlibCode::Input`] when the strip is not Bezier in
/// the extended direction); `InvalidInput` for [`Direction::UV`] where an extension
/// is needed.
pub fn adjust_deriv_srf(
    sur_p: &Surface,
    dir: Direction,
    cst: Boundary,
    usg: Boundary,
) -> Result<Surface> {
    use Boundary::{Bottom, Left, Right, Top};
    // Prepare: sdr is the other direction (upstream NL_NO for anything else).
    let sdr = match dir {
        Direction::U => Some(Direction::V),
        Direction::V => Some(Direction::U),
        Direction::UV => None,
    };
    let no_dir = || SolidError::InvalidInput {
        operation: OP_ADJ_DERIV,
        reason: "derivative strip direction is neither u nor v",
    };
    let extend = |s: &Surface, d: Option<Direction>, whr: ExtendAt| -> Result<Surface> {
        match d {
            Some(d) => srf_extend_4d(s, d, whr, false),
            None => Err(no_dir()),
        }
    };
    let pdir = match dir {
        Direction::UV => None,
        d => Some(d),
    };
    let swapped = || {
        let mut q = sur_p.clone();
        q.swap_uv();
        q
    };
    let (start, end) = (ExtendAt::Start, ExtendAt::End);

    // Create new surface.
    match (cst, usg) {
        (Left, Left) | (Right, Right) | (Bottom, Bottom) | (Top, Top) => Ok(sur_p.clone()),
        (Left, Right) | (Bottom, Top) => extend(sur_p, pdir, start),
        (Right, Left) | (Top, Bottom) => extend(sur_p, pdir, end),
        (Left, Bottom) | (Right, Top) | (Bottom, Left) | (Top, Right) => Ok(swapped()),
        (Left, Top) | (Bottom, Right) => extend(&swapped(), sdr, start),
        (Right, Bottom) | (Top, Left) => extend(&swapped(), sdr, end),
    }
}

// ---- N_AdjustWeightScale ------------------------------------------------------------

/// New weight for `Rw` and scale factor along `Qw - Rw` that make the homogeneous
/// polygon `Pw - Qw - Rw` G1 (upstream `N_AdjustWeightScale`): `(wr, alf)`.
///
/// # Errors
///
/// [`NlibCode::Geometry`] when a line intersection fails (parallel lines) or the
/// final division is bad; errors of [`isect_line_line`].
pub fn adjust_weight_scale(pw: CPoint, qw: CPoint, rw: CPoint) -> Result<(f64, f64)> {
    const OP: &str = OP_ADJ_WEIGHT;
    let (p, wp) = pw.to_euclid_and_w();
    let (q, wq) = qw.to_euclid_and_w();
    let ps = pw.drop_w();
    let qs = qw.drop_w();
    let rs = rw.drop_w();

    let wr;
    let bs;
    if (wp - wq).abs() < WEIGHT_TOL {
        bs = combine2_pts(2.0, qs, -1.0, ps);
        wr = 0.5 * (wp + wq);
    } else {
        wr = (wq * wq) / wp;
        let a_s = Vec3::new(wr * q.x, wr * q.y, wr * q.z);
        let ls1 = LineSeg::new(a_s, q - p, false);
        let ls2 = LineSeg::new(ps, qs - ps, false);
        bs = isect_line_line(ls1, ls2)?
            .ok_or_else(|| SolidError::nlib(OP, NlibCode::Geometry))?
            .p;
    }

    let ls1 = LineSeg::new(qs, rs - qs, false);
    let ls2 = LineSeg::new(Vec3::ZERO, bs - Vec3::ZERO, false);
    let ds = isect_line_line(ls1, ls2)?
        .ok_or_else(|| SolidError::nlib(OP, NlibCode::Geometry))?
        .p;

    let d1 = dist_pt_pt(qs, ds);
    let d2 = dist_pt_pt(qs, rs);
    if float_op_is_bad(d1, d2, FloatOp::Division) {
        return Err(SolidError::nlib(OP, NlibCode::Geometry));
    }
    Ok((wr, d1 / d2))
}

// ---- N_ApproxCrossBoundaryDerivs ----------------------------------------------------

/// Approximates a non-rational cross-boundary derivative curve so that its end
/// derivatives are `ds` and `de` while its direction stays within `eps` degrees of
/// the original (upstream `N_ApproxCrossBoundaryDerivs`): the second and
/// next-to-last control points are moved to give the end derivatives, after
/// inserting a knot near each end whose angle exceeds `eps`. On a single Bezier
/// span whose two new knots would come closer than a quarter of the span, the span
/// is first split at its midpoint in `der_p` itself (as upstream) and the test is
/// repeated.
///
/// # Errors
///
/// [`NlibCode::Input`] for a rational `der_p`; [`NlibCode::Knot`] when a new knot
/// falls outside its end span; errors of [`vectors_angle`], [`Curve::derivs`],
/// [`Curve::insert_knot`] and [`Curve::refine`]; `InvalidInput` for curves with
/// fewer than two control points or an end angle equal to `eps` (see the module
/// notes).
pub fn approx_cross_boundary_derivs(
    der_p: &mut Curve,
    ds: Vec3,
    de: Vec3,
    eps: f64,
) -> Result<Curve> {
    const OP: &str = OP_APPROX;

    // Check if angular tolerance is maintained.
    if der_p.is_rational() {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    let mut repeat = false;
    let (mut ts, mut te) = (0.0, 0.0);
    let (ps, pe, als, ale);
    loop {
        let n = der_p.n();
        let p = der_p.degree();
        let m = der_p.m();
        let up = der_p.knots();
        let pw = der_p.cpts();
        let nm1 = n.checked_sub(1).ok_or_else(|| oob(OP))?;

        let dts = at(up, p + 1, OP)? - at(up, p, OP)?;
        let dte = at(up, n + 1, OP)? - at(up, n, OP)?;
        let fs = dts / p as f64;
        let fe = dte / p as f64;

        let ps_ = at(pw, 0, OP)?.to_euclid();
        let a_s = at(pw, 1, OP)?.to_euclid();
        let a_e = at(pw, nm1, OP)?.to_euclid();
        let pe_ = at(pw, n, OP)?.to_euclid();

        let bs = pt_along_vector(ps_, fs, ds);
        let be = pt_along_vector(pe_, -fe, de);

        let als_ = vectors_angle(a_s, bs)?;
        let ale_ = vectors_angle(a_e, be)?;

        // Angle check in degrees.
        if als_ <= eps && ale_ <= eps {
            let mut q = der_p.clone();
            let qw = q.cpts_mut();
            put(qw, 1, CPoint::from_point(bs), OP)?;
            put(qw, nm1, CPoint::from_point(be), OP)?;
            return Ok(q);
        }

        // Insert knots to bring derivatives in compliance.
        let rad = (eps * PI) / 180.0;
        let sne = rad.sin();
        let mut mx_: isize = -1;

        if als_ > eps {
            let (_, cd1) = first_derivs(der_p, at(up, 0, OP)?, OP)?;
            let a = ps_.length();
            let b = (ds - cd1).length();
            let c = cd1.length();
            let d = (sne * p as f64 * a) / (b + sne * c);
            ts = at(up, p, OP)? + d;
            if ts > at(up, p + 1, OP)? {
                return Err(SolidError::nlib(OP, NlibCode::Knot));
            }
            mx_ += 1;
        }

        if ale_ > eps {
            let (_, cd1) = first_derivs(der_p, at(up, m, OP)?, OP)?;
            let a = pe_.length();
            let b = (de - cd1).length();
            let c = cd1.length();
            let d = (sne * p as f64 * a) / (b + sne * c);
            te = at(up, n + 1, OP)? - d;
            if te < at(up, n, OP)? {
                return Err(SolidError::nlib(OP, NlibCode::Knot));
            }
            mx_ += 1;
        }

        // This algorithm assumes that ts < te, but on a single span (Bezier, n = p)
        // te < ts is possible, which would put the new knots out of order.
        if !repeat && n == p && mx_ == 1 {
            let deltat = at(up, p + 1, OP)? - at(up, p, OP)?;
            if (te - ts) < 0.25 * deltat {
                // Insert a mid knot and reset ts, te and mx.
                let tmid = at(up, p, OP)? + 0.5 * deltat;
                *der_p = der_p.insert_knot(tmid, 1)?;
                repeat = true;
            }
        } else {
            repeat = false;
        }
        if !repeat {
            ps = ps_;
            pe = pe_;
            als = als_;
            ale = ale_;
            break;
        }
    }

    let mut n = der_p.n();
    let p = der_p.degree();
    let ux: Vec<f64> = if als > eps && ale < eps {
        n += 1;
        vec![ts]
    } else if als < eps && ale > eps {
        n += 1;
        vec![te]
    } else if als > eps && ale > eps {
        n += 2;
        vec![ts, te]
    } else {
        return Err(SolidError::InvalidInput {
            operation: OP,
            reason: "end angle equal to the tolerance (upstream uses an uninitialised knot)",
        });
    };

    let mut q = der_p.refine(&ux)?;
    let up = q.knots();
    let dts = at(up, p + 1, OP)? - at(up, p, OP)?;
    let dte = at(up, n + 1, OP)? - at(up, n, OP)?;
    let fs = dts / p as f64;
    let fe = dte / p as f64;
    let bs = pt_along_vector(ps, fs, ds);
    let be = pt_along_vector(pe, -fe, de);
    let nm1 = n.checked_sub(1).ok_or_else(|| oob(OP))?;
    let qw = q.cpts_mut();
    put(qw, 1, CPoint::from_point(bs), OP)?;
    put(qw, nm1, CPoint::from_point(be), OP)?;
    Ok(q)
}

// ---- N_CreateCoonsBoundaryCrvs ------------------------------------------------------

/// The boundary curves and cross-boundary derivatives of a bicubic Coons surface
/// ([`create_coons_boundary_crvs`]). `left` / `right` (and their derivatives) share
/// the v domain, `bottom` / `top` the u domain.
#[derive(Debug, Clone, PartialEq)]
pub struct CoonsBoundary {
    /// `curL`: the boundary `u = umin`.
    pub left: Curve,
    /// `derL`: the cross-boundary derivative across `left`.
    pub left_deriv: Curve,
    /// `curR`: the boundary `u = umax`.
    pub right: Curve,
    /// `derR`: the cross-boundary derivative across `right`.
    pub right_deriv: Curve,
    /// `curB`: the boundary `v = vmin`.
    pub bottom: Curve,
    /// `derB`: the cross-boundary derivative across `bottom`.
    pub bottom_deriv: Curve,
    /// `curT`: the boundary `v = vmax`.
    pub top: Curve,
    /// `derT`: the cross-boundary derivative across `top`.
    pub top_deriv: Curve,
}

/// Makes `cs` compatible unless they are (`N_CrvsAreCombatible` /
/// `N_CrvsMakeCompatible`), writing them back even on error, as upstream modifies
/// them in place.
fn make_four_compatible(cs: [&mut Curve; 4]) -> Result<()> {
    let mut arr: Vec<Curve> = cs.iter().map(|c| (**c).clone()).collect();
    if curves_are_compatible(&arr) {
        return Ok(());
    }
    let res = make_compatible(&mut arr);
    for (dst, src) in cs.into_iter().zip(arr) {
        *dst = src;
    }
    res
}

/// Bicubic Coons surface of four non-rational boundary curves and their
/// cross-boundary derivatives (upstream `N_CreateCoonsBoundaryCrvs`). Opposite
/// curves and derivatives are made compatible in place when they are not. With
/// `cpf`, the derivatives must match the boundaries' end derivatives within
/// `NL_MTOL` and the implied twists agree within `100 * NL_MTOL`; otherwise the
/// surface may violate the Coons condition but still interpolates the boundaries.
///
/// # Errors
///
/// [`NlibCode::Input`] for a rational curve, differing parameter domains, or a
/// failed compatibility check; errors of [`make_compatible`], [`Curve::eval`],
/// [`Curve::derivs`] and [`make_compatible_uv`]; `InvalidInput` for curves whose
/// sizes disagree after being made compatible.
pub fn create_coons_boundary_crvs(bnd: &mut CoonsBoundary, cpf: bool) -> Result<Surface> {
    const OP: &str = OP_COONS_BND;
    let side = Side::Left;

    // Check rationality, parameter domains, and initialize constants.
    let all = [
        &bnd.left,
        &bnd.right,
        &bnd.bottom,
        &bnd.top,
        &bnd.left_deriv,
        &bnd.right_deriv,
        &bnd.bottom_deriv,
        &bnd.top_deriv,
    ];
    if all.iter().any(|c| c.is_rational()) {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }

    let one_3 = 1.0 / 3.0;
    let one_9 = 1.0 / 9.0;

    let (us, ue) = bnd.bottom.param_bounds();
    for c in [&bnd.top, &bnd.bottom_deriv, &bnd.top_deriv] {
        let (d1, d2) = c.param_bounds();
        if us != d1 || ue != d2 {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
    }
    let du = ue - us;

    let (vs, ve) = bnd.left.param_bounds();
    for c in [&bnd.right, &bnd.left_deriv, &bnd.right_deriv] {
        let (d1, d2) = c.param_bounds();
        if vs != d1 || ve != d2 {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
    }
    let dv = ve - vs;

    // Make input data compatible in the B-spline sense.
    make_four_compatible([
        &mut bnd.left,
        &mut bnd.right,
        &mut bnd.left_deriv,
        &mut bnd.right_deriv,
    ])?;
    make_four_compatible([
        &mut bnd.bottom,
        &mut bnd.top,
        &mut bnd.bottom_deriv,
        &mut bnd.top_deriv,
    ])?;
    let aa = [&bnd.left, &bnd.right, &bnd.left_deriv, &bnd.right_deriv];
    let bb = [&bnd.bottom, &bnd.top, &bnd.bottom_deriv, &bnd.top_deriv];

    // Check compatibility.
    let uk = bb[0].knots();
    let vk = aa[0].knots();
    let (u0, ur) = ends(uk, OP)?;
    let (v0, vsn) = ends(vk, OP)?;

    if cpf {
        // (derivative curve, its two parameters, boundary pair, boundary parameter)
        let checks: [(&Curve, f64, f64, [&Curve; 2], f64); 4] = [
            (aa[2], v0, vsn, [bb[0], bb[1]], u0),
            (aa[3], v0, vsn, [bb[0], bb[1]], ur),
            (bb[2], u0, ur, [aa[0], aa[1]], v0),
            (bb[3], u0, ur, [aa[0], aa[1]], vsn),
        ];
        for (der, ta, tb, [c1, c2], tc) in checks {
            let a = der.eval(ta, side)?;
            let b = der.eval(tb, side)?;
            let (_, cd1) = first_derivs(c1, tc, OP)?;
            let (_, cd2) = first_derivs(c2, tc, OP)?;
            let da = (a - cd1).length();
            let db = (b - cd2).length();
            if da > MODEL_TOL {
                return Err(SolidError::nlib(OP, NlibCode::Input));
            }
            if db > MODEL_TOL {
                return Err(SolidError::nlib(OP, NlibCode::Input));
            }
        }
    }

    let twist_tol = MODEL_TOL * 1.0e2;
    let twist = |c1: &Curve, t1: f64, c2: &Curve, t2: f64| -> Result<Vec3> {
        let (_, cd1) = first_derivs(c1, t1, OP)?;
        let (_, cd2) = first_derivs(c2, t2, OP)?;
        let mag = (cd1 - cd2).length();
        if cpf && mag > twist_tol {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        Ok(combine2_pts(0.5, cd1, 0.5, cd2))
    };
    let t00 = twist(aa[2], v0, bb[2], u0)?;
    let t10 = twist(bb[2], ur, aa[3], v0)?;
    let t11 = twist(aa[3], vsn, bb[3], ur)?;
    let t01 = twist(bb[3], u0, aa[2], vsn)?;

    // ---- Compute the various surfaces ----

    let m = aa[0].n();
    let q = aa[0].degree();
    let va = aa[0].knots().to_vec();
    let n = bb[0].n();
    let p = bb[0].degree();
    let ua = bb[0].knots().to_vec();
    let cubic = |a: f64, b: f64| vec![a, a, a, a, b, b, b, b];

    // Compute u-blend.
    let mut uw = vec![vec![CPoint::default(); m + 1]; 4];
    for j in 0..=m {
        let c0 = at(aa[0].cpts(), j, OP)?;
        let c3 = at(aa[1].cpts(), j, OP)?;
        put2(&mut uw, 0, j, c0, OP)?;
        put2(&mut uw, 3, j, c3, OP)?;
    }
    for j in 0..=m {
        let d = at(aa[2].cpts(), j, OP)?;
        let x = CPoint::combine2(1.0, at2(&uw, 0, j, OP)?, du * one_3, d);
        put2(&mut uw, 1, j, x, OP)?;
    }
    for j in 0..=m {
        let d = at(aa[3].cpts(), j, OP)?;
        let x = CPoint::combine2(1.0, at2(&uw, 3, j, OP)?, -du * one_3, d);
        put2(&mut uw, 2, j, x, OP)?;
    }

    // Compute v-blend.
    let mut vw = vec![vec![CPoint::default(); 4]; n + 1];
    for i in 0..=n {
        let c0 = at(bb[0].cpts(), i, OP)?;
        let c3 = at(bb[1].cpts(), i, OP)?;
        put2(&mut vw, i, 0, c0, OP)?;
        put2(&mut vw, i, 3, c3, OP)?;
    }
    for i in 0..=n {
        let d = at(bb[2].cpts(), i, OP)?;
        let x = CPoint::combine2(1.0, at2(&vw, i, 0, OP)?, dv * one_3, d);
        put2(&mut vw, i, 1, x, OP)?;
    }
    for i in 0..=n {
        let d = at(bb[3].cpts(), i, OP)?;
        let x = CPoint::combine2(1.0, at2(&vw, i, 3, OP)?, -dv * one_3, d);
        put2(&mut vw, i, 2, x, OP)?;
    }

    // Compute tensor product surface.
    let mut tw = vec![vec![CPoint::default(); 4]; 4];
    for i in 0..=3 {
        put2(&mut tw, i, 0, at2(&uw, i, 0, OP)?, OP)?;
        put2(&mut tw, i, 3, at2(&uw, i, m, OP)?, OP)?;
    }
    for j in 1..=2 {
        put2(&mut tw, 0, j, at2(&vw, 0, j, OP)?, OP)?;
        put2(&mut tw, 3, j, at2(&vw, n, j, OP)?, OP)?;
    }
    let t00w = CPoint::from_point(t00);
    let t10w = CPoint::from_point(t10);
    let t01w = CPoint::from_point(t01);
    let t11w = CPoint::from_point(t11);
    let d1 = du * dv * one_9;
    let g = |w: &[Vec<CPoint>], i: usize, j: usize| at2(w, i, j, OP);
    let x11 = CPoint::combine4(
        d1,
        t00w,
        1.0,
        g(&tw, 1, 0)?,
        1.0,
        g(&tw, 0, 1)?,
        -1.0,
        g(&tw, 0, 0)?,
    );
    put2(&mut tw, 1, 1, x11, OP)?;
    let x21 = CPoint::combine4(
        -d1,
        t10w,
        1.0,
        g(&tw, 3, 1)?,
        1.0,
        g(&tw, 2, 0)?,
        -1.0,
        g(&tw, 3, 0)?,
    );
    put2(&mut tw, 2, 1, x21, OP)?;
    let x12 = CPoint::combine4(
        -d1,
        t01w,
        1.0,
        g(&tw, 1, 3)?,
        1.0,
        g(&tw, 0, 2)?,
        -1.0,
        g(&tw, 0, 3)?,
    );
    put2(&mut tw, 1, 2, x12, OP)?;
    let x22 = CPoint::combine4(
        d1,
        t11w,
        1.0,
        g(&tw, 2, 3)?,
        1.0,
        g(&tw, 3, 2)?,
        -1.0,
        g(&tw, 3, 3)?,
    );
    put2(&mut tw, 2, 2, x22, OP)?;

    let sur_u = Surface::new(uw, 3, q, cubic(us, ue), va)?;
    let sur_v = Surface::new(vw, p, 3, ua, cubic(vs, ve))?;
    let sur_t = Surface::new(tw, 3, 3, cubic(us, ue), cubic(vs, ve))?;

    // Make surfaces compatible.
    let mut sur_a = [sur_u, sur_v, sur_t];
    make_compatible_uv(&mut sur_a)?;

    // Compute output surface.
    let [sa, sb, sc] = &sur_a;
    combine_coons(sa, sb, sc, OP)
}

// ---- N_CreateCrossBoundaryDerivCrv --------------------------------------------------

/// Cross-boundary derivative curve `B(v) = p(v) D(v) + q(v) T(v)` along `cur`
/// (upstream `N_CreateCrossBoundaryDerivCrv`), where `D = C'`, `T` blends linearly
/// between the unit vectors `Ns x D` at the ends, and the linear functions `p`, `q`
/// are fixed by decomposing the given end derivatives `bs`, `be` along `D` and `T`;
/// `ns`, `ne` are the unit normals at the ends. The end control points are set to
/// `bs` and `be` exactly (weighted when the result is rational).
///
/// # Errors
///
/// [`NlibCode::Geometry`] when a tangent cannot be normalised or a line
/// intersection fails; errors of [`Curve::derivs`], [`Curve::rat_first_deriv`],
/// [`CurveFunction::multiply_crv`] and [`Curve::sum_diff`].
pub fn create_cross_boundary_deriv_crv(
    cur: &Curve,
    ns: Vec3,
    ne: Vec3,
    bs: Vec3,
    be: Vec3,
) -> Result<Curve> {
    const OP: &str = OP_CROSS;

    // Get coordinates (p0, p1) and (q0, q1) so that Bs = p0*Ds + q0*Ts and
    // Be = p1*De + q1*Te, where Ds and De are the end derivatives of cur.
    let (k0, km) = ends(cur.knots(), OP)?;
    let (ps, ds) = first_derivs(cur, k0, OP)?;
    let mds = ds.length();
    let (pe, de) = first_derivs(cur, km, OP)?;
    let mde = de.length();

    let ts = ns.cross(ds);
    let te = ne.cross(de);
    let vs = ps + bs;
    let ve = pe + be;
    let (ts, _) = normalize(ts)?;
    let (te, _) = normalize(te)?;

    let isect = |p1: Vec3, v1: Vec3, p2: Vec3, v2: Vec3| -> Result<Vec3> {
        let ls1 = LineSeg::new(p1, v1, false);
        let ls2 = LineSeg::new(p2, v2, false);
        Ok(isect_line_line(ls1, ls2)?
            .ok_or_else(|| SolidError::nlib(OP, NlibCode::Geometry))?
            .p)
    };
    // Signed coordinates at one end.
    let coords = |pt: Vec3, d: Vec3, t: Vec3, v: Vec3, md: f64| -> Result<(f64, f64)> {
        let a = isect(pt, d, v, t)? - pt;
        let b = isect(pt, t, v, d)? - pt;
        let mut pc = a.length();
        let mut qc = b.length();
        pc /= md;
        if a.dot(d) < 0.0 {
            pc = -pc;
        }
        if b.dot(t) < 0.0 {
            qc = -qc;
        }
        Ok((pc, qc))
    };
    let (p0, q0) = coords(ps, ds, ts, vs, mds)?;
    let (p1, q1) = coords(pe, de, te, ve, mde)?;

    // Get linear functions.
    let lin = || vec![0.0, 0.0, 1.0, 1.0];
    let mut cfn_p = CurveFunction::new(vec![p0, p1], 1, lin())?;
    let mut cfn_q = CurveFunction::new(vec![q0, q1], 1, lin())?;

    // Get derivative curve.
    let mut cur_d = cur.rat_first_deriv()?;

    // Get tangent curve.
    let mut cur_t = Curve::new(
        vec![CPoint::from_point(ts), CPoint::from_point(te)],
        1,
        lin(),
    )?;

    // Compute cross-boundary curve B(v) = p(v)*D(v) + q(v)*T(v).
    let pd = cfn_p.multiply_crv(&mut cur_d)?;
    let qt = cfn_q.multiply_crv(&mut cur_t)?;
    let mut der = pd.sum_diff(&qt, SumDiff::Plus)?;

    let n = der.n();
    let rational = der.is_rational();
    let pw = der.cpts_mut();
    if rational {
        let w0 = at(pw, 0, OP)?.w;
        let wn = at(pw, n, OP)?.w;
        put(pw, 0, CPoint::weighted(bs, w0), OP)?;
        put(pw, n, CPoint::weighted(be, wn), OP)?;
    } else {
        put(pw, 0, CPoint::from_point(bs), OP)?;
        put(pw, n, CPoint::from_point(be), OP)?;
    }
    Ok(der)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;

    fn line(a: Vec3, b: Vec3) -> Curve {
        Curve::new(
            vec![CPoint::from_point(a), CPoint::from_point(b)],
            1,
            vec![0.0, 0.0, 1.0, 1.0],
        )
        .unwrap()
    }

    #[test]
    fn merge_knot_vectors_basic() {
        let r = [0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        let s = [0.0, 0.0, 0.0, 0.25, 0.5, 0.5, 1.0, 1.0, 1.0];
        let kr = KnotVector::from_slice(&r).unwrap();
        let ks = KnotVector::from_slice(&s).unwrap();
        let (xr, xs) = merge_knot_vectors(&kr, &ks, 2).unwrap();
        assert_eq!(xr, vec![0.25, 0.5]);
        assert!(xs.is_empty());
        let bad = [0.0, 0.0, 0.0, 2.0, 2.0, 2.0];
        let kb = KnotVector::from_slice(&bad).unwrap();
        assert!(merge_knot_vectors(&kr, &kb, 2).is_err());
    }

    #[test]
    fn merge_knot_vectors_start_index_overflow() {
        let knots = [0.0, 0.0, 1.0, 1.0];
        let kr = KnotVector::from_slice(&knots).unwrap();
        let ks = KnotVector::from_slice(&knots).unwrap();
        assert_eq!(
            merge_knot_vectors(&kr, &ks, usize::MAX),
            Err(SolidError::InvalidInput {
                operation: "nurbs::srf_adv::p4::merge_knot_vectors",
                reason: BARE_FLAG_REASON,
            })
        );
    }

    #[test]
    fn merge_knot_vectors_endpoint_mismatch_precedes_start_index_overflow() {
        let r = [0.0, 0.0, 1.0, 1.0];
        let s = [0.0, 0.0, 2.0, 2.0];
        let kr = KnotVector::from_slice(&r).unwrap();
        let ks = KnotVector::from_slice(&s).unwrap();
        assert_eq!(
            merge_knot_vectors(&kr, &ks, usize::MAX),
            Err(SolidError::nlib(
                "nurbs::srf_adv::p4::merge_knot_vectors",
                NlibCode::Input,
            ))
        );
    }

    #[test]
    fn coons_of_unit_square_is_bilinear() {
        let (a, b, c, d) = (
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
        );
        let mut cu = [line(a, b), line(c, d)];
        let mut cv = [line(a, c), line(b, d)];
        let s = create_coons_srf(&mut cu, &mut cv).unwrap();
        assert_eq!(s.cpt(1, 1).unwrap(), CPoint::new(1.0, 1.0, 1.0, NOW));
        assert_eq!(s.cpt(0, 0).unwrap(), CPoint::new(0.0, 0.0, 0.0, NOW));
    }

    #[test]
    fn adjust_weight_scale_equal_weights() {
        let pw = CPoint::new(0.0, 0.0, 0.0, 1.0);
        let qw = CPoint::new(1.0, 0.0, 0.0, 1.0);
        let rw = CPoint::new(3.0, 0.0, 0.0, 1.0);
        // Collinear lines: the second intersection is parallel.
        assert!(adjust_weight_scale(pw, qw, rw).is_err());
        let rw = CPoint::new(1.0, 2.0, 0.0, 1.0);
        let (wr, alf) = adjust_weight_scale(pw, qw, rw).unwrap();
        assert_eq!(wr, 1.0);
        assert!(alf.is_finite());
    }

    #[test]
    fn rotation_rate_requires_neighbours() {
        let t = line(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0));
        let z = ZSource::Samples {
            z: Vec3::new(0.0, 0.0, 1.0),
            prev: None,
            next: None,
        };
        assert!(rotation_rate_of_change(&t, 0.5, z).is_err());
        let z = ZSource::Samples {
            z: Vec3::new(0.0, 0.0, 1.0),
            prev: None,
            next: Some((Vec3::new(0.0, 0.0, 1.0), 0.6)),
        };
        let r = rotation_rate_of_change(&t, 0.5, z).unwrap();
        assert_eq!(r[2][2], 0.0);
    }

    #[test]
    fn b_vectors_on_a_line_stay_constant() {
        let t = line(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0));
        let b = create_b_vectors(&t, Vec3::new(0.0, 0.0, 2.0), &[0.0, 0.5, 1.0]).unwrap();
        assert_eq!(b.len(), 3);
        assert!(b.iter().all(|v| *v == Vec3::new(0.0, 0.0, 1.0)));
    }

    #[test]
    fn adjust_deriv_srf_same_boundary_copies() {
        let s = create_skin_srf_boundary_derivs(
            &mut line(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0)),
            Vec3::new(0.0, 1.0, 0.0),
            Boundary::Left,
        )
        .unwrap();
        let q = adjust_deriv_srf(&s, Direction::U, Boundary::Left, Boundary::Left).unwrap();
        assert_eq!(q, s);
        let q = adjust_deriv_srf(&s, Direction::U, Boundary::Left, Boundary::Bottom).unwrap();
        assert_eq!(q.degrees(), (s.degrees().1, s.degrees().0));
    }

    #[test]
    fn absurd_cross_section_degree_is_an_error() {
        // fuzz (nlib.srf_adv_p4 fuzz00189, fuzz00067): `p * segments + 1` overflowed or
        // sized the samples.
        let c = line(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        for p in [i64::MAX as usize, 1 << 40] {
            let bad = Curve::new(c.cpts().to_vec(), p, c.knots().to_vec()).unwrap();
            let r = sweep_cross_tangent_crv(
                Vec3::X,
                Vec3::new(1.0, 1.0, 1.0),
                Vec3::ZERO,
                &id,
                &id,
                &bad,
            );
            assert!(r.is_err());
        }
    }
}
