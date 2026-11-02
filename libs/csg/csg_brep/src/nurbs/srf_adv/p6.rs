//! Sweep surfaces with scaling (trajectory decomposed into linear and curved pieces,
//! cross sections placed along it and skinned with cross-boundary derivatives) and
//! the filling of an n-sided hole with bicubically blended Coons patches.
//!
//! Upstream: `source/SMLib/src/NL_SrfAdv.cpp`, `source/SMLib/inc/NL_SrfAdv.h`.
//!
//! | Rust                                         | Upstream               |
//! |----------------------------------------------|------------------------|
//! | [`create_sweep_scale`], [`SweepScaleOptions`] | `N_CreateSweepScale`   |
//! | [`create_sweep_scale_with_placements`]       | `N_CreateSweepScale` with the fork's `placed` |
//! | [`fill_n_sided_hole`], [`NSidedFill`]        | `N_FillNSidedHole`     |
//!
//! Deviations:
//! - Out-params become return values (the swept surface; the patches with the center
//!   point and normal); the `NL_STACKS` arguments have no counterpart. The optional
//!   arguments of `N_CreateSweepScale` (`curZ`, `curS`, `tflg`, `tc`, `deg`, `dir`,
//!   `E`) are the fields of [`SweepScaleOptions`]; `N_FillNSidedHole`'s `CI`, `NI`,
//!   `flg` become an `Option<(center, normal)>`.
//! - `N_FillNSidedHole` modifies its inputs in place (2D boundary curves made 3D,
//!   boundary curves and given derivatives made compatible); `cur` and `der` are
//!   `&mut` and modified the same way. `der = NULL` is `None`, `der[i] = NULL` a
//!   `None` entry.
//! - Errors are [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number.
//! - `N_CreateSweepScale` ignores the status of `ST_GetRotationRateOfChange`; it can
//!   only fail where upstream's inputs make it unreachable (a `curZ` that cannot be
//!   differentiated where it was just evaluated, or a single frame sample), and the
//!   Rust function returns that error. `N_FillNSidedHole` ignores the status of
//!   `N_CrvRefine` on the computed derivative curve, as does the Rust function (the
//!   curve stays unrefined; the refinement fails before modifying it).
//! - Every array access is bounds checked; where upstream would index outside its
//!   arrays (`k` beyond the given derivative curves, a frame parameter past the
//!   sampled ones, a polyline without segments) the functions return
//!   [`SolidError::InvalidInput`].
//! - `N_CreateSweepScale`: a trajectory, cross section or sweep degree above `32767`
//!   (upstream's `NL_DEGREE` is a `short`, so it cannot be expressed there) is
//!   `InvalidInput`.

use crate::error::{NlibCode, Result, SolidError};
use crate::math::Vec3;
use crate::nurbs::basis_basic::{compatible_knot_array, PTOL};
use crate::nurbs::crv_basic::{curves_are_compatible, Boundary, Curve};
use crate::nurbs::crv_geom::ToleranceKind;
use crate::nurbs::crv_nurb::LinearSeg;
use crate::nurbs::crv_tool::p2::make_compatible_adj_knots;
use crate::nurbs::funcs_basic::CurveFunction;
use crate::nurbs::geom::{
    normalize, project_pt_plane, pt_along_vector, transform_matrix_from_axes, Frame, Plane,
};
use crate::nurbs::index::{at, oob, put};
use crate::nurbs::knots::Side;
use crate::nurbs::math::{combine2_pts, solve_quadratic, CPoint, QuadraticRoots, MODEL_TOL};
use crate::nurbs::srf_basic::{Direction, Surface};
use crate::nurbs::srf_tool::p1::insert_knots;
use crate::nurbs::srf_tool::p2::elevate_degree as srf_elevate_degree;
use crate::nurbs::srf_tool::p3::join;

use super::p1::cross_boundary_derivs_vector_field;
use super::p4::{
    approx_cross_boundary_derivs, create_b_vectors, create_coons_boundary_crvs,
    create_cross_boundary_deriv_crv, rotation_rate_of_change, sweep_cross_tangent_crv,
    CoonsBoundary, Mat3, ZSource,
};
use super::p5::{create_skin_srf_boundary_continuity, BoundaryContinuity, SkinBoundaryOptions};

const OP_SWEEP: &str = "nurbs::srf_adv::p6::create_sweep_scale";
const OP_FILL: &str = "nurbs::srf_adv::p6::fill_n_sided_hole";

/// Upstream `MAXPTS` of `N_CreateSweepScale`: the most polyline segments per
/// trajectory piece.
const MAXPTS: usize = 1000;
/// Upstream `lto` of `N_FillNSidedHole`: the relative arc length tolerance.
const LTO: f64 = 0.05;

/// `&v[i]`.
fn rf<'a, T>(v: &'a [T], i: usize, op: &'static str) -> Result<&'a T> {
    v.get(i).ok_or_else(|| oob(op))
}

/// `&mut v[i]`.
fn mt<'a, T>(v: &'a mut [T], i: usize, op: &'static str) -> Result<&'a mut T> {
    v.get_mut(i).ok_or_else(|| oob(op))
}

/// The last knot of `c` (`U[m]`).
fn last_knot(c: &Curve, op: &'static str) -> Result<f64> {
    c.knots().last().copied().ok_or_else(|| oob(op))
}

/// The first knot of `c` (`U[0]`).
fn first_knot(c: &Curve, op: &'static str) -> Result<f64> {
    c.knots().first().copied().ok_or_else(|| oob(op))
}

/// `C'(u)` (`N_CrvDerivs(.., 1, CD)`, `CD[1]`).
fn deriv1(c: &Curve, u: f64, op: &'static str) -> Result<Vec3> {
    let cd = c.derivs(u, Side::Left, 1)?;
    at(&cd, 1, op)
}

// ---- N_CreateSweepScale ------------------------------------------------------------

/// The optional arguments of [`create_sweep_scale`].
#[derive(Debug, Clone, Copy)]
pub struct SweepScaleOptions<'a> {
    /// Curve giving the local z axis at every trajectory parameter (upstream `curZ`);
    /// its parameter range must be the trajectory's. `None`: the z axes come from the
    /// start axis `z0` by the projection normal method.
    pub cur_z: Option<&'a Curve>,
    /// Scaling curve (upstream `curS`): `curS(t)` scales the section in x, y and z;
    /// its parameter range must be the trajectory's. `None`: no scaling.
    pub cur_s: Option<&'a Curve>,
    /// Interpolate the trajectory (upstream `tflg`): when the cross section passes
    /// through the origin at `tc`, the trajectory becomes an isocurve.
    pub tflg: bool,
    /// Parameter of the cross section at the origin (ignored without `tflg`).
    pub tc: f64,
    /// Degree in the sweep direction (at least the trajectory's with `tflg`).
    pub deg: usize,
    /// [`Direction::U`]: sweep in u (the sections are v-curves); anything else sweeps
    /// in v (the sections are u-curves).
    pub dir: Direction,
    /// Sweeping tolerance (upstream `E > 0`).
    pub e: f64,
}

impl Default for SweepScaleOptions<'_> {
    fn default() -> Self {
        Self {
            cur_z: None,
            cur_s: None,
            tflg: false,
            tc: 0.0,
            deg: 3,
            dir: Direction::V,
            e: 1e-3,
        }
    }
}

/// The sub-sampling of a trajectory piece for the frame vectors (upstream `sub`).
fn frame_subsampling(linear: bool, n_pars: usize) -> usize {
    if linear || n_pars > MAXPTS / 2 {
        0
    } else if n_pars > MAXPTS / 5 {
        1
    } else if n_pars > MAXPTS / 10 {
        2
    } else if n_pars > MAXPTS / 20 {
        3
    } else {
        4
    }
}

/// The rotation matrix `[XL | YL | ZL]` (columns) of a sweep frame.
fn frame_matrix(xl: Vec3, yl: Vec3, zl: Vec3) -> Mat3 {
    [[xl.x, yl.x, zl.x], [xl.y, yl.y, zl.y], [xl.z, yl.z, zl.z]]
}

/// Multiplies the homogeneous coordinates of every control point of `c` by `wt`
/// (weight `1` first unless `rat_c`), as upstream adjusts the cross sections to a
/// rational trajectory.
fn weight_section(c: &mut Curve, wt: f64, rat_c: bool) {
    for pw in c.cpts_mut() {
        let [wx, wy, wz, w0] = pw.to_array();
        let w = if rat_c { w0 } else { 1.0 };
        *pw = CPoint::new(wt * wx, wt * wy, wt * wz, wt * w);
    }
}

/// Sweep surface of the cross section `cur_c` along the trajectory `cur_t` with
/// optional scaling (upstream `N_CreateSweepScale`): `S(u, v) = T(v) + M(v) C(u)`.
///
/// The trajectory is split into its linear and curved pieces (a degree-1 trajectory
/// must be a line); each piece (degree elevated to `deg` when interpolating it) is
/// sampled by a polyline within `E / 2` (at most `MAXPTS` segments, the tolerance
/// growing tenfold until then), refined to enough samples for the degree or the
/// scaling curve. At every sample the (scaled) cross section is placed in the frame
/// `x = T'/|T'|`, `z` from `curZ` or the projection normal method from `z0`, `y = z x
/// x`; with `tflg` and a rational trajectory the sections are weighted by its
/// denominator. The sections of each piece are skinned in v with G1 cross-boundary
/// derivative strips from the sweep's exact cross tangents (the trajectory piece as a
/// rail with `tflg`, where a cross section crossing the origin inside its range is
/// split there and the two skins are joined along the trajectory); the pieces are
/// joined in v after matching their u degrees. For [`Direction::U`] the
/// parametrisation is swapped at the end.
///
/// # Errors
///
/// [`NlibCode::Input`] for a degree-1 trajectory that is not a line; errors of the
/// called routines; `InvalidInput` for the cases listed in the module notes.
pub fn create_sweep_scale(
    cur_t: &Curve,
    cur_c: &Curve,
    z0: Vec3,
    opt: &SweepScaleOptions<'_>,
) -> Result<Surface> {
    sweep_scale(cur_t, cur_c, z0, opt, None)
}

/// The local systems a sweep placed its cross section on (the source fork's
/// `placed` output of `N_CreateSweepScale`, source fork 5d3939e): the frames
/// `(OL, XL, YL, ZL)` of the first, the middle (of all placed, in order: index
/// `n / 2` of `n`) and the last section.
pub type SweepPlacements = [Frame; 3];

/// [`create_sweep_scale`] that also reports where it placed the cross section
/// ([`SweepPlacements`]; the source fork's `placed` output, source fork
/// 5d3939e, #1450). The surface is the same as [`create_sweep_scale`]'s.
///
/// # Errors
///
/// Those of [`create_sweep_scale`].
pub fn create_sweep_scale_with_placements(
    cur_t: &Curve,
    cur_c: &Curve,
    z0: Vec3,
    opt: &SweepScaleOptions<'_>,
) -> Result<(Surface, SweepPlacements)> {
    let mut placed = [Frame::default(); 3];
    let surface = sweep_scale(cur_t, cur_c, z0, opt, Some(&mut placed))?;
    Ok((surface, placed))
}

/// `N_CreateSweepScale` with the fork's optional `placed` output.
fn sweep_scale(
    cur_t: &Curve,
    cur_c: &Curve,
    z0: Vec3,
    opt: &SweepScaleOptions<'_>,
    mut placed: Option<&mut SweepPlacements>,
) -> Result<Surface> {
    const OP: &str = OP_SWEEP;
    let SweepScaleOptions {
        cur_z,
        cur_s,
        tflg,
        tc,
        deg,
        dir,
        e,
    } = *opt;

    // Degrees upstream cannot express (`NL_DEGREE` is a `short`).
    super::degree(deg, OP)?;
    super::degree(cur_c.degree(), OP)?;

    // Get local notation.
    let pt = super::degree(cur_t.degree(), OP)?;
    let mut ruled = false;

    // Decompose trajectory into linear and curved pieces.
    let seg_pbnds: Vec<LinearSeg> = if pt == 1 {
        // Error check: curT better be linear.
        if !cur_t.is_line(MODEL_TOL) {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        let (start, end) = cur_t.param_bounds();
        match cur_s {
            Some(s) => {
                if s.is_line(MODEL_TOL) && deg == 1 {
                    ruled = true;
                }
            }
            None => ruled = true,
        }
        vec![LinearSeg {
            start,
            end,
            linear: true,
        }]
    } else {
        cur_t.linear_segs(MODEL_TOL)?
    };
    let n_segs = seg_pbnds.len().checked_sub(1).ok_or_else(|| oob(OP))?;

    // Place curT into the segs array.
    let mut segs: Vec<Curve> = if n_segs == 0 {
        vec![cur_t.clone()]
    } else {
        seg_pbnds
            .iter()
            .map(|s| cur_t.extract_seg(s.start, s.end))
            .collect::<Result<_>>()?
    };

    // Degree elevate the trajectory segments when interpolating it.
    if tflg && deg > pt {
        for s in &mut segs {
            *s = s.elevate_degree(deg - pt)?;
        }
    }

    // Trajectory parameters where the sections are placed.
    let tol = 0.5 * e;
    let mut seg_pars: Vec<Vec<f64>> = Vec::with_capacity(segs.len());
    for seg in &segs {
        let mut d1 = tol;
        let temp = loop {
            let pl = seg.approx_with_polyline(d1, ToleranceKind::Absolute)?;
            if pl.params.len() <= MAXPTS + 1 {
                break pl.params;
            }
            d1 *= 10.0;
        };
        let kk = temp.len().checked_sub(1).ok_or_else(|| oob(OP))?;

        let mut jj = deg;
        if let Some(s) = cur_s {
            jj = s.n() + 1;
        }

        if kk >= jj || ruled {
            seg_pars.push(temp);
        } else {
            if kk == 0 {
                return Err(SolidError::InvalidInput {
                    operation: OP,
                    reason: "polyline without segments (upstream divides by zero)",
                });
            }
            let mm = jj - kk;
            let nn = mm / kk + 1;
            let mut sp = Vec::with_capacity(kk * (nn + 1) + 1);
            sp.push(at(&temp, 0, OP)?);
            for j in 1..=kk {
                let t0 = at(&temp, j - 1, OP)?;
                let t1 = at(&temp, j, OP)?;
                let du = (t1 - t0) / (nn as f64 + 1.0);
                for k1 in 1..=nn {
                    sp.push(t0 + k1 as f64 * du);
                }
                sp.push(t1);
            }
            seg_pars.push(sp);
        }
    }

    // Frame z vectors at (sub-sampled) parameters when curZ is not given.
    let mut tt: Vec<f64> = Vec::new();
    let mut zl_vecs: Vec<Vec3> = Vec::new();
    if cur_z.is_none() {
        tt.push(at(rf(&seg_pars, 0, OP)?, 0, OP)?);
        for (ii, sp) in seg_pars.iter().enumerate() {
            let np = sp.len() - 1;
            let sub = frame_subsampling(rf(&seg_pbnds, ii, OP)?.linear, np);
            if sub == 0 {
                for jj in 1..=np {
                    tt.push(at(sp, jj, OP)?);
                }
            } else {
                for jj in 1..=np {
                    let t0 = at(sp, jj - 1, OP)?;
                    let t1 = at(sp, jj, OP)?;
                    let du = (t1 - t0) / (sub as f64 + 1.0);
                    for k1 in 1..=sub {
                        tt.push(t0 + k1 as f64 * du);
                    }
                    tt.push(t1);
                }
            }
        }
        zl_vecs = create_b_vectors(cur_t, z0, &tt)?;
    }
    let zl_count = zl_vecs.len();

    // Handle trajectory interpolation (rail) and split curC if necessary.
    let mut us = 0.0;
    let split;
    let cur_cs0: Curve;
    let mut cur_cs1: Option<Curve> = None;
    if tflg {
        let (a, b) = cur_c.param_bounds();
        us = a;
        let ue = b;
        if tc > us && tc < ue {
            let (c0, c1) = cur_c.split(tc)?;
            cur_cs0 = c0;
            cur_cs1 = Some(c1);
            split = true;
        } else {
            cur_cs0 = cur_c.clone();
            split = false;
        }
    } else {
        cur_cs0 = cur_c.clone();
        split = false;
    }

    // Handle rationality correctly if interpolating the trajectory.
    let mut rat_t = false;
    let mut rat_c = false;
    let mut wtfun: Option<CurveFunction> = None;
    if tflg {
        rat_t = cur_t.is_rational();
        if rat_t {
            if cur_t.cpts().iter().all(|pw| pw.w == 1.0) {
                rat_t = false;
            } else {
                rat_c = cur_c.is_rational();
                cur_t.denom_func(&mut wtfun)?;
            }
        }
    }

    let world = Frame::new(Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z);
    let mut k1 = 0usize;
    let mut dscale1 = [Vec3::new(1.0, 1.0, 1.0), Vec3::ZERO];
    let mut dscale2 = [Vec3::new(1.0, 1.0, 1.0), Vec3::ZERO];
    let mut rma1: Mat3 = [[0.0; 3]; 3];
    let mut rma2: Mat3 = [[0.0; 3]; 3];
    let mut drma1: Mat3 = [[0.0; 3]; 3];
    let mut drma2: Mat3 = [[0.0; 3]; 3];
    let cnt = [BoundaryContinuity::G1; 2];
    let bndy = [Boundary::Top, Boundary::Bottom];

    let mut result: Option<Surface> = None;

    // source fork (5d3939e): the sections placed so far, and in all (see `placed`).
    let mut placed_at = 0usize;
    let n_placed: usize = seg_pars.iter().map(Vec::len).sum();

    // The big loop over the trajectory segments.
    for (ii, seg) in segs.iter().enumerate() {
        let sp = rf(&seg_pars, ii, OP)?;
        let np = sp.len() - 1;
        let mut sects1: Vec<Curve> = Vec::with_capacity(np + 1);
        let mut sects2: Vec<Curve> = Vec::with_capacity(np + 1);

        // Scale and place the cross sections.
        for (jj, &par) in sp.iter().enumerate() {
            let mut s1 = cur_cs0.clone();
            let mut s2 = cur_cs1.clone();

            if let Some(cs) = cur_s {
                let scale = cs.eval(par, Side::Left)?;
                if jj == 0 {
                    let d = cs.derivs(par, Side::Left, 1)?;
                    dscale1 = [at(&d, 0, OP)?, at(&d, 1, OP)?];
                }
                if jj == np {
                    let d = cs.derivs(par, Side::Left, 1)?;
                    dscale2 = [at(&d, 0, OP)?, at(&d, 1, OP)?];
                }
                s1.scale(Vec3::ZERO, scale);
                if let Some(s2) = s2.as_mut() {
                    s2.scale(Vec3::ZERO, scale);
                }
            }

            let (ol, xl) = seg.eval_tangent(par, Side::Left)?;

            let zl = if let Some(cz) = cur_z {
                normalize(cz.eval(par, Side::Left)?)?.0
            } else {
                while at(&tt, k1, OP)? < par {
                    k1 += 1;
                }
                at(&zl_vecs, k1, OP)?
            };

            let yl = zl.cross(xl);
            let rma = transform_matrix_from_axes(world, Frame::new(ol, xl, yl, zl))?;

            // source fork (5d3939e): the local systems of the first, middle and last
            // sections.
            if let Some(placed) = placed.as_deref_mut() {
                let slots = [0, n_placed / 2, n_placed.saturating_sub(1)];
                for (slot, frame) in slots.iter().zip(placed.iter_mut()) {
                    if placed_at == *slot {
                        *frame = Frame::new(ol, xl, yl, zl);
                    }
                }
            }
            placed_at += 1;

            if jj == 0 || jj == np {
                let zsrc = match cur_z {
                    Some(cz) => ZSource::Curve(cz),
                    None => ZSource::Samples {
                        z: zl,
                        prev: if k1 > 0 {
                            Some((at(&zl_vecs, k1 - 1, OP)?, at(&tt, k1 - 1, OP)?))
                        } else {
                            None
                        },
                        next: if k1 + 1 < zl_count {
                            Some((at(&zl_vecs, k1 + 1, OP)?, at(&tt, k1 + 1, OP)?))
                        } else {
                            None
                        },
                    },
                };
                if jj == 0 {
                    rma1 = frame_matrix(xl, yl, zl);
                    drma1 = rotation_rate_of_change(seg, par, zsrc)?;
                } else {
                    rma2 = frame_matrix(xl, yl, zl);
                    drma2 = rotation_rate_of_change(seg, par, zsrc)?;
                }
            }

            s1.transform(&rma)?;
            if let Some(s2) = s2.as_mut() {
                s2.transform(&rma)?;
            }

            if tflg && rat_t {
                let wt = wtfun
                    .as_ref()
                    .ok_or_else(|| oob(OP))?
                    .eval(par, Side::Left)?;
                weight_section(&mut s1, wt, rat_c);
                if let Some(s2) = s2.as_mut() {
                    weight_section(s2, wt, rat_c);
                }
            }

            sects1.push(s1);
            if let Some(s2) = s2 {
                sects2.push(s2);
            }
        }

        // Define the boundary cross derivatives.
        let lseg = rf(&seg_pbnds, ii, OP)?;
        let derivs1 = deriv1(seg, lseg.start, OP)?;
        let derivs2 = deriv1(seg, lseg.end, OP)?;

        let skin = if split {
            let cs1 = cur_cs1.as_ref().ok_or_else(|| oob(OP))?;
            let ct10 =
                sweep_cross_tangent_crv(derivs1, dscale1[0], dscale1[1], &rma1, &drma1, &cur_cs0)?;
            let ct11 =
                sweep_cross_tangent_crv(derivs2, dscale2[0], dscale2[1], &rma2, &drma2, &cur_cs0)?;
            let ct20 =
                sweep_cross_tangent_crv(derivs1, dscale1[0], dscale1[1], &rma1, &drma1, cs1)?;
            let ct21 =
                sweep_cross_tangent_crv(derivs2, dscale2[0], dscale2[1], &rma2, &drma2, cs1)?;
            let ds0 =
                cross_boundary_derivs_vector_field(rf(&sects1, 0, OP)?, &ct10, Boundary::Top)?;
            let de0 =
                cross_boundary_derivs_vector_field(rf(&sects1, np, OP)?, &ct11, Boundary::Bottom)?;
            let ds1 =
                cross_boundary_derivs_vector_field(rf(&sects2, 0, OP)?, &ct20, Boundary::Top)?;
            let de1 =
                cross_boundary_derivs_vector_field(rf(&sects2, np, OP)?, &ct21, Boundary::Bottom)?;

            // Skin from the cross-section start to the trajectory, then from the
            // trajectory to the cross-section end, and join along the trajectory.
            let mut o1 = SkinBoundaryOptions {
                der: Some([Some(ds0), Some(de0)]),
                cmp: true,
                cnt,
                bnd: bndy,
                rail: Some([None, Some(seg.clone())]),
                deg,
                es: 0.0,
                dir: Direction::V,
                ts: Some(sp.clone()),
                kns: None,
            };
            let s2 = create_skin_srf_boundary_continuity(&mut sects1, &mut o1)?;
            let mut o2 = SkinBoundaryOptions {
                der: Some([Some(ds1), Some(de1)]),
                cmp: true,
                cnt,
                bnd: bndy,
                rail: Some([Some(seg.clone()), None]),
                deg,
                es: 0.0,
                dir: Direction::V,
                ts: Some(sp.clone()),
                kns: None,
            };
            let s3 = create_skin_srf_boundary_continuity(&mut sects2, &mut o2)?;
            join(&s2, &s3, Direction::U, MODEL_TOL)?
        } else {
            let ct10 =
                sweep_cross_tangent_crv(derivs1, dscale1[0], dscale1[1], &rma1, &drma1, &cur_cs0)?;
            let ct11 =
                sweep_cross_tangent_crv(derivs2, dscale2[0], dscale2[1], &rma2, &drma2, &cur_cs0)?;
            let ds0 =
                cross_boundary_derivs_vector_field(rf(&sects1, 0, OP)?, &ct10, Boundary::Top)?;
            let de0 =
                cross_boundary_derivs_vector_field(rf(&sects1, np, OP)?, &ct11, Boundary::Bottom)?;

            let (ts, rail) = if tflg {
                let r = if tc == us {
                    [Some(seg.clone()), None]
                } else {
                    [None, Some(seg.clone())]
                };
                (Some(sp.clone()), Some(r))
            } else {
                (None, None)
            };
            // If ruled ignore end derivatives.
            let der = if ruled {
                None
            } else {
                Some([Some(ds0), Some(de0)])
            };
            let mut o = SkinBoundaryOptions {
                der,
                cmp: true,
                cnt,
                bnd: bndy,
                rail,
                deg,
                es: e,
                dir: Direction::V,
                ts,
                kns: None,
            };
            create_skin_srf_boundary_continuity(&mut sects1, &mut o)?
        };

        // Merge surfaces.
        result = Some(match result {
            None => skin,
            Some(sur1) => {
                let (ps, _) = sur1.degrees();
                let (pt2, _) = skin.degrees();
                let (sur1, sur2) = match ps.cmp(&pt2) {
                    core::cmp::Ordering::Less => {
                        (srf_elevate_degree(&sur1, pt2 - ps, Direction::U)?, skin)
                    }
                    core::cmp::Ordering::Greater => {
                        let s = srf_elevate_degree(&skin, ps - pt2, Direction::U)?;
                        (sur1, s)
                    }
                    core::cmp::Ordering::Equal => (sur1, skin),
                };
                join(&sur1, &sur2, Direction::V, MODEL_TOL)?
            }
        });
    }

    let mut sur = result.ok_or_else(|| oob(OP))?;

    // Make sure the sweep direction is correct.
    if dir == Direction::U {
        sur.swap_uv();
    }
    Ok(sur)
}

// ---- N_FillNSidedHole --------------------------------------------------------------

/// The result of [`fill_n_sided_hole`].
#[derive(Debug, Clone, PartialEq)]
pub struct NSidedFill {
    /// The `k + 1` patches; patch `i` fills the corner between the second half of
    /// boundary `i - 1` and the first half of boundary `i`.
    pub surfaces: Vec<Surface>,
    /// The center point (upstream `CI` on output).
    pub center: Vec3,
    /// The unit center normal (upstream `NI` on output).
    pub normal: Vec3,
}

/// `(i - 1) mod (k + 1)`.
fn prev(i: usize, k: usize) -> usize {
    if i == 0 {
        k
    } else {
        i - 1
    }
}

/// `(i + 1) mod (k + 1)`.
fn next(i: usize, k: usize) -> usize {
    if i == k {
        0
    } else {
        i + 1
    }
}

/// The curves of the corner patch layout, by upstream name.
struct Net {
    cur_l: Vec<Curve>,
    cur_r: Vec<Curve>,
    der_l: Vec<Curve>,
    der_r: Vec<Curve>,
    cur_i: Vec<Curve>,
    bdr_l: Vec<Curve>,
    bdr_r: Vec<Curve>,
}

impl Net {
    /// Makes the seven curves meeting along inner curve `i` compatible unless they
    /// are (`N_CrvsAreCombatible` / `N_CrvsMakeCompatibleAdjKnots`), in place.
    fn make_compatible(&mut self, i: usize, k: usize) -> Result<()> {
        const OP: &str = OP_FILL;
        let a = prev(i, k);
        let b = next(i, k);
        let mut cc = vec![
            rf(&self.cur_r, a, OP)?.clone(),
            rf(&self.cur_i, i, OP)?.clone(),
            rf(&self.cur_l, b, OP)?.clone(),
            rf(&self.der_r, a, OP)?.clone(),
            rf(&self.bdr_l, i, OP)?.clone(),
            rf(&self.bdr_r, i, OP)?.clone(),
            rf(&self.der_l, b, OP)?.clone(),
        ];
        if curves_are_compatible(&cc) {
            return Ok(());
        }
        let res = make_compatible_adj_knots(&mut cc, PTOL);
        let mut it = cc.into_iter();
        let mut store = |v: &mut Vec<Curve>, j: usize| -> Result<()> {
            if let Some(c) = it.next() {
                put(v, j, c, OP)?;
            }
            Ok(())
        };
        store(&mut self.cur_r, a)?;
        store(&mut self.cur_i, i)?;
        store(&mut self.cur_l, b)?;
        store(&mut self.der_r, a)?;
        store(&mut self.bdr_l, i)?;
        store(&mut self.bdr_r, i)?;
        store(&mut self.der_l, b)?;
        res
    }
}

/// The end factors `|C1'| / |D(t0)|` and `|C2'| / |D(t1)|` rescaling the derivative
/// curve `d` to the end tangents of `c1` and `c2` (at the given parameters).
fn end_factors(
    d: &Curve,
    t0: f64,
    t1: f64,
    c1: (&Curve, f64),
    c2: (&Curve, f64),
) -> Result<[f64; 2]> {
    let dlv = d.eval(t0, Side::Left)?;
    let drv = d.eval(t1, Side::Left)?;
    let cd1 = deriv1(c1.0, c1.1, OP_FILL)?;
    let cd2 = deriv1(c2.0, c2.1, OP_FILL)?;
    let dl = dlv.length();
    let dr = drv.length();
    let alf = cd1.length();
    let bet = cd2.length();
    Ok([alf / dl, bet / dr])
}

/// Rescales `c` by the linear function with end values `fu` (the shared `cfn`, whose
/// knots the product may rescale in place, as upstream).
fn rescale(cfn: &mut CurveFunction, fu: [f64; 2], c: &mut Curve) -> Result<Curve> {
    let v = cfn.values_mut();
    put(v, 0, fu[0], OP_FILL)?;
    put(v, 1, fu[1], OP_FILL)?;
    cfn.multiply_crv(c)
}

/// The derivative across boundary `i` when none is given: the line between the
/// neighbours' end tangents, elevated to the boundary's degree and refined with its
/// interior knots (when it has at least two).
fn computed_derivative(
    cur: &[Curve],
    cur_l: &[Curve],
    cur_r: &[Curve],
    i: usize,
    k: usize,
) -> Result<Curve> {
    const OP: &str = OP_FILL;
    let a = prev(i, k);
    let b = next(i, k);
    let ra = rf(cur_r, a, OP)?;
    let lb = rf(cur_l, b, OP)?;
    let cd1 = deriv1(ra, first_knot(ra, OP)?, OP)?;
    let cd2 = deriv1(lb, first_knot(lb, OP)?, OP)?;
    let mut der_a = Curve::new(
        vec![CPoint::from_point(cd1), CPoint::from_point(cd2)],
        1,
        vec![0.0, 0.0, 1.0, 1.0],
    )?;
    let ci = rf(cur, i, OP)?;
    let p = ci.degree();
    let n = ci.n();
    let u = ci.knots();
    if p > 1 {
        der_a = der_a.elevate_degree(p - 1)?;
    }
    // mx = n - p - 1 > 0
    if n > p + 1 {
        let mx = n - p - 1;
        let ux = u.get(p + 1..=p + 1 + mx).ok_or_else(|| oob(OP))?;
        // Upstream ignores a failure (the curve is then unchanged).
        if let Ok(r) = der_a.refine(ux) {
            der_a = r;
        }
    }
    Ok(der_a)
}

/// WR-461 (#1170): shortens the cross-boundary derivative `top_deriv` across the
/// inner curve `top` where the bicubic blend from `bottom` would leave the tangent
/// plane of `bottom` (spanned by its tangent and `bottom_deriv`) to the side away from
/// `top`. Along `v` the blend's height above that plane is a cubic Bezier with control
/// heights `0, 0, D - dv m / 3, D` (`D` the height of `top`, `m` the derivative's
/// normal component, `dv` the parameter length); with all four on `top`'s side the
/// curve is too, so a control height on the other side is what has to be removed. At
/// every inner control point of `top` where that happens the factor that brings it to
/// zero is found, and the smallest factor scales the derivative's control points
/// `2..=n-3` together: a single factor keeps the direction, so G1 across `top`, and the
/// four end control points (the corner values and the twists that the neighbouring
/// curves share) are untouched. The condition is sufficient, not necessary: it also
/// acts, slightly, where the blend dips below the plane by less than the neighbour's
/// own curvature leaves room for (a round's surface lies below its tangent plane).
/// A quadrilateral whose curves are not compatible (one control point per index) or
/// whose parameter length is not positive is left alone. The plane is the tangent
/// plane at the Greville points, so "inside the neighbour" is exact for a planar
/// neighbour only.
fn limit_cross_deriv(
    bottom: &Curve,
    bottom_deriv: &Curve,
    top: &Curve,
    top_deriv: &mut Curve,
    dv: f64,
) -> Result<()> {
    const OP: &str = OP_FILL;
    let n = top.cpts().len();
    let p = top.degree();
    if bottom.cpts().len() != n
        || bottom_deriv.cpts().len() != n
        || top_deriv.cpts().len() != n
        || dv <= 0.0
    {
        return Ok(());
    }
    let knots = top.knots().to_vec();
    let mut scale = 1.0_f64;
    // the factor is found at every inner control point, but applied only to 2..=n-3:
    // control points 0, 1, n-2 and n-1 are the corner values and twists that the
    // neighbouring curves share
    for j in 1..n.saturating_sub(1) {
        // Greville abscissa of control point j.
        let xi = match knots.get(j + 1..j + 1 + p) {
            Some(s) if p > 0 => s.iter().sum::<f64>() / p as f64,
            _ => continue,
        };
        let b = bottom.eval(xi, Side::Left)?;
        let t = top.eval(xi, Side::Left)?;
        let normal = deriv1(bottom, xi, OP)?.cross(bottom_deriv.eval(xi, Side::Left)?);
        let l = normal.length();
        if l <= 0.0 {
            continue;
        }
        let normal = normal / l;
        let height = (t - b).dot(normal);
        let cp = at(top_deriv.cpts(), j, OP)?;
        let m = Vec3::new(cp.x, cp.y, cp.z).dot(normal);
        // `height` and `m` are nonzero here: `inner` is on the other side of zero.
        if height * (height - dv / 3.0 * m) < 0.0 {
            scale = scale.min(3.0 * height / (dv * m));
        }
    }
    if scale < 1.0 {
        for c in top_deriv
            .cpts_mut()
            .iter_mut()
            .take(n.saturating_sub(2))
            .skip(2)
        {
            c.x *= scale;
            c.y *= scale;
            c.z *= scale;
        }
    }
    Ok(())
}

/// [`limit_cross_deriv`] on both inner-curve derivatives of every quadrilateral.
fn limit_inner_cross_derivs(net: &mut Net, k: usize) -> Result<()> {
    const OP: &str = OP_FILL;
    for i in 0..=k {
        let a = prev(i, k);
        let (u0, ur) = rf(&net.cur_l, i, OP)?.param_bounds();
        let (v0, vs) = rf(&net.cur_r, a, OP)?.param_bounds();
        let (du, dv) = (ur - u0, vs - v0);
        let (bottom, bottom_deriv, top) = (
            rf(&net.cur_l, i, OP)?.clone(),
            rf(&net.der_l, i, OP)?.clone(),
            rf(&net.cur_i, a, OP)?.clone(),
        );
        limit_cross_deriv(&bottom, &bottom_deriv, &top, mt(&mut net.bdr_l, a, OP)?, dv)?;
        let (left, left_deriv, right) = (
            rf(&net.cur_r, a, OP)?.clone(),
            rf(&net.der_r, a, OP)?.clone(),
            rf(&net.cur_i, i, OP)?.clone(),
        );
        limit_cross_deriv(&left, &left_deriv, &right, mt(&mut net.bdr_r, i, OP)?, du)?;
    }
    Ok(())
}

/// Fills an n-sided hole with `k + 1` patches (upstream `N_FillNSidedHole`).
///
/// Given `k + 1 >= 3` non-rational boundary curves `cur` (the end of `cur[i]` is the
/// start of `cur[i+1]`) and optional non-rational cross-boundary derivatives `der`
/// (`None`, or an entry per curve, `None` where it is computed by blending the
/// neighbours' end tangents), the boundaries and derivatives are split at their
/// midpoints; inner cubic curves run from each midpoint to a center point where they
/// share the tangent plane with normal `NI` (both given in `center`, or computed from
/// the midpoints and the scaled middle derivatives). Cross-boundary derivatives across
/// the inner curves are built, magnitudes adjusted, twist compatibility enforced by
/// approximating the derivatives within `eps` degrees, and a bicubically blended
/// Coons patch is fitted into each quadrilateral; knots are finally merged along the
/// common boundaries.
///
/// # Errors
///
/// [`NlibCode::Input`] for fewer than three curves, rational input, or boundaries
/// that do not join (by more than `NL_MTOL`); [`NlibCode::Geometry`] when an inner
/// curve's quadratic has fewer than two roots; errors of the called routines;
/// `InvalidInput` for a `der` with fewer entries than `cur` and the cases listed in
/// the module notes.
pub fn fill_n_sided_hole(
    cur: &mut [Curve],
    der: Option<&mut [Option<Curve>]>,
    eps: f64,
    center: Option<(Vec3, Vec3)>,
) -> Result<NSidedFill> {
    fill_n_sided_hole_with(cur, der, eps, center, false)
}

/// [`fill_n_sided_hole`] with the opt-in fix of WR-461 (#1170): with `stay_inside`,
/// the cross-boundary derivatives across the inner curves are shortened where their
/// cubic blend from the opposite boundary would cross that boundary's tangent
/// plane (see [`limit_inner_cross_derivs`]).
///
/// # Errors
///
/// As [`fill_n_sided_hole`].
pub fn fill_n_sided_hole_with(
    cur: &mut [Curve],
    mut der: Option<&mut [Option<Curve>]>,
    eps: f64,
    center: Option<(Vec3, Vec3)>,
    stay_inside: bool,
) -> Result<NSidedFill> {
    const OP: &str = OP_FILL;

    // Check input.
    if cur.len() < 3 {
        return Err(SolidError::nlib(OP, NlibCode::Input));
    }
    let k = cur.len() - 1;
    if let Some(d) = der.as_deref() {
        if d.len() < cur.len() {
            return Err(oob(OP));
        }
    }
    let der_at = |der: &Option<&mut [Option<Curve>]>, i: usize| -> Option<Curve> {
        der.as_deref()
            .and_then(|d| d.get(i))
            .and_then(Option::as_ref)
            .cloned()
    };

    for i in 0..=k {
        let c = mt(cur, i, OP)?;
        if c.is_rational() {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        if !c.is_3d() {
            c.to_3d();
        }
        if let Some(d) = der_at(&der, i) {
            if d.is_rational() {
                return Err(SolidError::nlib(OP, NlibCode::Input));
            }
        }
    }

    // Check compatibility.
    for i in 0..=k {
        if let Some(d) = der_at(&der, i) {
            let mut cc = vec![rf(cur, i, OP)?.clone(), d];
            if !curves_are_compatible(&cc) {
                let res = make_compatible_adj_knots(&mut cc, PTOL);
                let mut it = cc.into_iter();
                if let Some(c) = it.next() {
                    put(cur, i, c, OP)?;
                }
                if let (Some(c), Some(ds)) = (it.next(), der.as_deref_mut()) {
                    put(ds, i, Some(c), OP)?;
                }
                res?;
            }
        }
    }

    // Check direction of parametrization.
    for i in 0..=k {
        let b = next(i, k);
        let ci = rf(cur, i, OP)?;
        let cb = rf(cur, b, OP)?;
        let ml = ci.eval(last_knot(ci, OP)?, Side::Left)?;
        let mr = cb.eval(first_knot(cb, OP)?, Side::Left)?;
        let a = ml - mr;
        if a.length() > MODEL_TOL {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
    }

    // Split boundary curves at their mid points.
    let mut cur_l: Vec<Curve> = Vec::with_capacity(k + 1);
    let mut cur_r: Vec<Curve> = Vec::with_capacity(k + 1);
    let mut mpt: Vec<Vec3> = Vec::with_capacity(k + 1);
    let mut ph = 0;
    for c in cur.iter() {
        let p = c.degree();
        if p > ph {
            ph = p;
        }
        let um = 0.5 * (first_knot(c, OP)? + last_knot(c, OP)?);
        mpt.push(c.eval(um, Side::Left)?);
        let (l, mut r) = c.split(um)?;
        r.reverse()?;
        cur_l.push(l);
        cur_r.push(r);
    }

    // Check degrees.
    ph = ph.max(3);
    for i in 0..=k {
        let l = mt(&mut cur_l, i, OP)?;
        let pl = l.degree();
        if pl < ph {
            *l = l.elevate_degree(ph - pl)?;
        }
        let r = mt(&mut cur_r, i, OP)?;
        let pr = r.degree();
        if pr < ph {
            *r = r.elevate_degree(ph - pr)?;
        }
    }

    // Split derivative curves at their mid points.
    let mut der_l: Vec<Curve> = Vec::with_capacity(k + 1);
    let mut der_r: Vec<Curve> = Vec::with_capacity(k + 1);
    let mut dmid: Vec<Vec3> = Vec::with_capacity(k + 1);
    for i in 0..=k {
        let dc = if let Some(d) = der_at(&der, i) {
            // Derivative passed in.
            d
        } else {
            // Compute derivative.
            computed_derivative(cur, &cur_l, &cur_r, i, k)?
        };
        // Get mid derivative.
        let um = 0.5 * (first_knot(&dc, OP)? + last_knot(&dc, OP)?);
        dmid.push(dc.eval(um, Side::Left)?);
        // Split derivative curve.
        let (l, mut r) = dc.split(um)?;
        r.reverse()?;
        der_l.push(l);
        der_r.push(r);
    }

    for i in 0..=k {
        // Check degrees.
        let l = mt(&mut der_l, i, OP)?;
        let pl = l.degree();
        if pl < ph {
            *l = l.elevate_degree(ph - pl)?;
        }
        let r = mt(&mut der_r, i, OP)?;
        let pr = r.degree();
        if pr < ph {
            *r = r.elevate_degree(ph - pr)?;
        }

        // Check direction of derivatives.
        let a = prev(i, k);
        let b = next(i, k);
        let ra = rf(&cur_r, a, OP)?;
        let lb = rf(&cur_l, b, OP)?;
        let cd1 = deriv1(ra, first_knot(ra, OP)?, OP)?;
        let cd2 = deriv1(lb, first_knot(lb, OP)?, OP)?;

        let dli = rf(&der_l, i, OP)?;
        let dri = rf(&der_r, i, OP)?;
        let dl_pt = dli.eval(first_knot(dli, OP)?, Side::Left)?;
        let dr_pt = dri.eval(first_knot(dri, OP)?, Side::Left)?;

        let dl = cd1.dot(dl_pt);
        let dr = cd2.dot(dr_pt);

        if dl < 0.0 {
            for pw in mt(&mut der_l, i, OP)?.cpts_mut() {
                *pw = pw.scale(-1.0);
            }
        }
        if dr < 0.0 {
            for pw in mt(&mut der_r, i, OP)?.cpts_mut() {
                *pw = pw.scale(-1.0);
            }
        }
        if dl < 0.0 && dr < 0.0 {
            let d = mt(&mut dmid, i, OP)?;
            *d = -*d;
        }
    }

    // Scale magnitude of middle derivative.
    for i in 0..=k {
        let a = prev(i, k);
        let b = next(i, k);
        let ca = rf(cur, a, OP)?;
        let cb = rf(cur, b, OP)?;
        let dl = ca.arc_length(
            first_knot(ca, OP)?,
            last_knot(ca, OP)?,
            LTO,
            ToleranceKind::Relative,
        )?;
        let dr = cb.arc_length(
            first_knot(cb, OP)?,
            last_knot(cb, OP)?,
            LTO,
            ToleranceKind::Relative,
        )?;
        let d = mt(&mut dmid, i, OP)?;
        let mag = d.length();
        let la = 0.25 * (dl + dr);
        let fac = la / mag;
        *d *= fac;
    }

    // Get center point and normal vector.
    let (cc, nn0) = match center {
        None => {
            let q: Vec<Vec3> = mpt
                .iter()
                .zip(&dmid)
                .map(|(&m, &d)| pt_along_vector(m, 0.5, d))
                .collect();
            let mut cc = Vec3::ZERO;
            for &qi in &q {
                cc += qi;
            }
            let fac = 1.0 / (k as f64 + 1.0);
            cc *= fac;

            let mut nn = Vec3::ZERO;
            for i in 0..=k {
                let b = next(i, k);
                let a = at(&mpt, i, OP)? - cc;
                let bb = at(&mpt, b, OP)? - cc;
                let f = a.cross(bb);
                nn += f;
            }
            let fac = 1.0 / (k as f64 + 1.0);
            nn *= fac;
            (cc, nn)
        }
        Some((c, n)) => (c, n),
    };
    let (nn, _) = normalize(nn0)?;
    let pln = Plane::new(cc, nn);

    // Get inner boundary curves.
    let mut cur_i: Vec<Curve> = Vec::with_capacity(k + 1);
    for i in 0..=k {
        let mi = at(&mpt, i, OP)?;
        let (t0, _) = normalize(at(&dmid, i, OP)?)?;
        let a = project_pt_plane(pln, mi)?;
        let (t3, _) = normalize(cc - a)?;

        let a = t0 + t3;
        let b = cc - mi;
        let mag = a.length();
        let del = b.length();
        let dl = a.dot(b);

        let alf = 16.0 - mag * mag;
        let bet = 12.0 * dl;
        let gam = -36.0 * del * del;

        let fac = match solve_quadratic(alf, bet, gam)? {
            QuadraticRoots::Two(_, r2) => r2,
            QuadraticRoots::None | QuadraticRoots::One(_) => {
                return Err(SolidError::nlib(OP, NlibCode::Geometry));
            }
        };
        let fac = fac / 3.0;

        let a = pt_along_vector(mi, fac, t0);
        let b = pt_along_vector(cc, -fac, t3);

        cur_i.push(Curve::new(
            vec![
                CPoint::from_point(mi),
                CPoint::from_point(a),
                CPoint::from_point(b),
                CPoint::from_point(cc),
            ],
            3,
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )?);
    }

    // Get unit normals at the mid points.
    let mut nrm: Vec<Vec3> = Vec::with_capacity(k + 1);
    for i in 0..=k {
        let li = rf(&cur_l, i, OP)?;
        let ii = rf(&cur_i, i, OP)?;
        let cd1 = deriv1(li, last_knot(li, OP)?, OP)?;
        let cd2 = deriv1(ii, first_knot(ii, OP)?, OP)?;
        let (n, _) = normalize(cd1.cross(cd2))?;
        nrm.push(n);
    }

    // Get cross-boundary derivatives across the inner curves.
    let mut bdr_l: Vec<Curve> = Vec::with_capacity(k + 1);
    let mut bdr_r: Vec<Curve> = Vec::with_capacity(k + 1);
    for i in 0..=k {
        let a = prev(i, k);
        let b = next(i, k);
        let ri = rf(&cur_r, i, OP)?;
        let ib = rf(&cur_i, b, OP)?;
        let cd1 = deriv1(ri, last_knot(ri, OP)?, OP)?;
        let cd2 = deriv1(ib, last_knot(ib, OP)?, OP)?;
        let ii = rf(&cur_i, i, OP)?;
        let ni = at(&nrm, i, OP)?;
        bdr_l.push(create_cross_boundary_deriv_crv(ii, ni, nn, cd1, cd2)?);

        let li = rf(&cur_l, i, OP)?;
        let ia = rf(&cur_i, a, OP)?;
        let cd1 = deriv1(li, last_knot(li, OP)?, OP)?;
        let cd2 = deriv1(ia, last_knot(ia, OP)?, OP)?;
        bdr_r.push(create_cross_boundary_deriv_crv(ii, ni, nn, cd1, cd2)?);
    }

    let mut net = Net {
        cur_l,
        cur_r,
        der_l,
        der_r,
        cur_i,
        bdr_l,
        bdr_r,
    };

    // Make boundary and cross-derivatives compatible.
    for i in 0..=k {
        net.make_compatible(i, k)?;
    }

    // Adjust derivative magnitudes.
    let mut cfn = CurveFunction::new(vec![0.0, 0.0], 1, vec![0.0, 0.0, 1.0, 1.0])?;
    for i in 0..=k {
        let a = prev(i, k);
        let u = rf(&net.cur_l, i, OP)?.knots().to_vec();
        let v = rf(&net.cur_r, a, OP)?.knots().to_vec();
        let (u0, ur) = (at(&u, 0, OP)?, *u.last().ok_or_else(|| oob(OP))?);
        let (v0, vs) = (at(&v, 0, OP)?, *v.last().ok_or_else(|| oob(OP))?);

        let fu = end_factors(
            rf(&net.der_r, a, OP)?,
            v0,
            vs,
            (rf(&net.cur_l, i, OP)?, u0),
            (rf(&net.cur_i, a, OP)?, u0),
        )?;
        let slot = mt(&mut net.der_r, a, OP)?;
        *slot = rescale(&mut cfn, fu, slot)?;

        let fu = end_factors(
            rf(&net.der_l, i, OP)?,
            u0,
            ur,
            (rf(&net.cur_r, a, OP)?, v0),
            (rf(&net.cur_i, i, OP)?, v0),
        )?;
        let slot = mt(&mut net.der_l, i, OP)?;
        *slot = rescale(&mut cfn, fu, slot)?;

        let fu = end_factors(
            rf(&net.bdr_r, i, OP)?,
            v0,
            vs,
            (rf(&net.cur_l, i, OP)?, ur),
            (rf(&net.cur_i, a, OP)?, ur),
        )?;
        let slot = mt(&mut net.bdr_r, i, OP)?;
        *slot = rescale(&mut cfn, fu, slot)?;

        let fu = end_factors(
            rf(&net.bdr_l, a, OP)?,
            u0,
            ur,
            (rf(&net.cur_r, a, OP)?, vs),
            (rf(&net.cur_i, i, OP)?, vs),
        )?;
        let slot = mt(&mut net.bdr_l, a, OP)?;
        *slot = rescale(&mut cfn, fu, slot)?;
    }

    // Approximate cross-boundary derivatives to ensure twist compatibility.
    let (mut le, mut ri, mut bo, mut to) = (true, true, true, true);
    for i in 0..=k {
        // Get average of corner derivatives.
        let a = prev(i, k);
        let v = rf(&net.der_r, a, OP)?.knots().to_vec();
        let u = rf(&net.der_l, i, OP)?.knots().to_vec();
        let (v0, vs) = (at(&v, 0, OP)?, *v.last().ok_or_else(|| oob(OP))?);
        let (u0, ur) = (at(&u, 0, OP)?, *u.last().ok_or_else(|| oob(OP))?);

        let corner = |c1: (&Curve, f64), c2: (&Curve, f64)| -> Result<(bool, Vec3)> {
            let cd1 = deriv1(c1.0, c1.1, OP)?;
            let cd2 = deriv1(c2.0, c2.1, OP)?;
            let a = cd1 - cd2;
            let mag = a.length();
            Ok((mag > MODEL_TOL, combine2_pts(0.5, cd1, 0.5, cd2)))
        };

        let (off, d1) = corner((rf(&net.der_r, a, OP)?, v0), (rf(&net.der_l, i, OP)?, u0))?;
        if off {
            le = false;
            bo = false;
        }
        let (off, d2) = corner((rf(&net.der_l, i, OP)?, ur), (rf(&net.bdr_r, i, OP)?, v0))?;
        if off {
            bo = false;
            ri = false;
        }
        let (off, d3) = corner((rf(&net.bdr_r, i, OP)?, vs), (rf(&net.bdr_l, a, OP)?, ur))?;
        if off {
            ri = false;
            to = false;
        }
        let (off, d4) = corner((rf(&net.bdr_l, a, OP)?, u0), (rf(&net.der_r, a, OP)?, vs))?;
        if off {
            to = false;
            le = false;
        }

        // Now approximate cross-derivatives.
        if !bo {
            let c = mt(&mut net.der_l, i, OP)?;
            *c = approx_cross_boundary_derivs(c, d1, d2, eps)?;
        }
        if !ri {
            let c = mt(&mut net.bdr_r, i, OP)?;
            *c = approx_cross_boundary_derivs(c, d2, d3, 0.5 * eps)?;
        }
        if !to {
            let c = mt(&mut net.bdr_l, a, OP)?;
            *c = approx_cross_boundary_derivs(c, d4, d3, 0.5 * eps)?;
        }
        if !le {
            let c = mt(&mut net.der_r, a, OP)?;
            *c = approx_cross_boundary_derivs(c, d1, d4, eps)?;
        }
    }

    // Make boundary and approximated cross-derivatives compatible.
    for i in 0..=k {
        net.make_compatible(i, k)?;
    }

    if stay_inside {
        limit_inner_cross_derivs(&mut net, k)?;
    }

    // Fit bicubically blended Coons patches into each rectangular hole.
    let mut sur_a: Vec<Surface> = Vec::with_capacity(k + 1);
    for i in 0..=k {
        let a = prev(i, k);
        let mut bnd = CoonsBoundary {
            left: rf(&net.cur_r, a, OP)?.clone(),
            left_deriv: rf(&net.der_r, a, OP)?.clone(),
            right: rf(&net.cur_i, i, OP)?.clone(),
            right_deriv: rf(&net.bdr_r, i, OP)?.clone(),
            bottom: rf(&net.cur_l, i, OP)?.clone(),
            bottom_deriv: rf(&net.der_l, i, OP)?.clone(),
            top: rf(&net.cur_i, a, OP)?.clone(),
            top_deriv: rf(&net.bdr_l, a, OP)?.clone(),
        };
        let res = create_coons_boundary_crvs(&mut bnd, false);
        // The inputs are modified in place upstream.
        let CoonsBoundary {
            left,
            left_deriv,
            right,
            right_deriv,
            bottom,
            bottom_deriv,
            top,
            top_deriv,
        } = bnd;
        put(&mut net.cur_r, a, left, OP)?;
        put(&mut net.der_r, a, left_deriv, OP)?;
        put(&mut net.cur_i, i, right, OP)?;
        put(&mut net.bdr_r, i, right_deriv, OP)?;
        put(&mut net.cur_l, i, bottom, OP)?;
        put(&mut net.der_l, i, bottom_deriv, OP)?;
        put(&mut net.cur_i, a, top, OP)?;
        put(&mut net.bdr_l, a, top_deriv, OP)?;
        sur_a.push(res?);
    }

    // Merge knots along common boundaries.
    for i in 0..=k {
        let b = next(i, k);
        let mut kna = vec![
            rf(&sur_a, i, OP)?.v_knots().to_vec(),
            rf(&sur_a, b, OP)?.u_knots().to_vec(),
        ];
        let res = compatible_knot_array(&mut kna);
        // Knots within tolerance are merged in place in the surfaces' knot vectors.
        if let Some(kv) = kna.first() {
            let dst = mt(&mut sur_a, i, OP)?.v_knots_mut();
            if dst.len() == kv.len() {
                dst.copy_from_slice(kv);
            }
        }
        if let Some(ku) = kna.get(1) {
            let dst = mt(&mut sur_a, b, OP)?.u_knots_mut();
            if dst.len() == ku.len() {
                dst.copy_from_slice(ku);
            }
        }
        let knm = res?;
        let xl = rf(&knm, 0, OP)?;
        let xr = rf(&knm, 1, OP)?;
        if !xl.is_empty() {
            let s = insert_knots(rf(&sur_a, i, OP)?, xl, Direction::V)?;
            put(&mut sur_a, i, s, OP)?;
        }
        if !xr.is_empty() {
            let s = insert_knots(rf(&sur_a, b, OP)?, xr, Direction::U)?;
            put(&mut sur_a, b, s, OP)?;
        }
    }

    Ok(NSidedFill {
        surfaces: sur_a,
        center: cc,
        normal: nn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::math::NOW;

    fn crv(pts: &[[f64; 3]], p: usize, u: Vec<f64>) -> Curve {
        Curve::new(
            pts.iter()
                .map(|q| CPoint::new(q[0], q[1], q[2], NOW))
                .collect(),
            p,
            u,
        )
        .unwrap()
    }

    fn arc_traj() -> Curve {
        crv(
            &[
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.5],
                [4.0, 2.0, 1.0],
                [5.0, 5.0, 1.5],
            ],
            3,
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )
    }

    fn section() -> Curve {
        crv(
            &[[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
    }

    #[test]
    fn sweep_along_curve_interpolates_the_sections() {
        let t = arc_traj();
        let c = section();
        let s = create_sweep_scale(
            &t,
            &c,
            Vec3::Z,
            &SweepScaleOptions {
                e: 1e-2,
                ..SweepScaleOptions::default()
            },
        )
        .unwrap();
        // Swept in v: the first row of the net is the placed cross section.
        let (p, _) = s.degrees();
        assert_eq!(p, 2);
        let p0 = s.cpt(0, 0).unwrap().to_euclid();
        assert!(p0.length().is_finite());
    }

    #[test]
    fn linear_trajectory_gives_ruled_surface() {
        let t = crv(
            &[[0.0, 0.0, 0.0], [0.0, 0.0, 5.0]],
            1,
            vec![0.0, 0.0, 1.0, 1.0],
        );
        let c = section();
        let s = create_sweep_scale(
            &t,
            &c,
            Vec3::X,
            &SweepScaleOptions {
                deg: 1,
                dir: Direction::U,
                ..SweepScaleOptions::default()
            },
        )
        .unwrap();
        // Swapped: u is the sweep direction, linear.
        assert_eq!(s.degrees(), (1, 2));
    }

    #[test]
    fn bent_linear_trajectory_is_rejected() {
        let t = crv(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
            1,
            vec![0.0, 0.0, 0.5, 1.0, 1.0],
        );
        let r = create_sweep_scale(&t, &section(), Vec3::Z, &SweepScaleOptions::default());
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    fn triangle() -> Vec<Curve> {
        let p = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [2.0, 3.5, 0.0]];
        (0..3)
            .map(|i| {
                let a = p[i];
                let b = p[(i + 1) % 3];
                let m = [
                    0.5 * (a[0] + b[0]),
                    0.5 * (a[1] + b[1]),
                    0.5 * (a[2] + b[2]) + 0.3,
                ];
                crv(&[a, m, b], 2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
            })
            .collect()
    }

    #[test]
    fn fills_a_three_sided_hole() {
        let mut cur = triangle();
        let f = fill_n_sided_hole(&mut cur, None, 1.0, None).unwrap();
        assert_eq!(f.surfaces.len(), 3);
        assert!((f.normal.length() - 1.0).abs() < 1e-12);
        for s in &f.surfaces {
            let (p, q) = s.degrees();
            assert!(p >= 3 && q >= 3);
        }
    }

    #[test]
    fn too_few_or_open_boundaries_are_rejected() {
        let mut two: Vec<Curve> = triangle().into_iter().take(2).collect();
        assert!(fill_n_sided_hole(&mut two, None, 1.0, None).is_err());
        let mut cur = triangle();
        cur[1] = crv(
            &[[4.0, 0.5, 0.0], [3.0, 2.0, 0.0], [2.0, 3.5, 0.0]],
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        );
        let r = fill_n_sided_hole(&mut cur, None, 1.0, None);
        assert!(matches!(
            r,
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn absurd_degrees_are_errors() {
        // fuzz (nlib.srf_adv_p6 fuzz00016, fuzz00101): degrees of i64::MAX reached the
        // curve tools and the cross tangent sampling.
        let t = arc_traj();
        let c = section();
        let opts = SweepScaleOptions {
            e: 1e-2,
            deg: i64::MAX as usize,
            ..SweepScaleOptions::default()
        };
        assert!(create_sweep_scale(&t, &c, Vec3::Z, &opts).is_err());
        let bad = Curve::new(c.cpts().to_vec(), i64::MAX as usize, c.knots().to_vec()).unwrap();
        let opts = SweepScaleOptions {
            e: 1e-2,
            ..SweepScaleOptions::default()
        };
        assert!(create_sweep_scale(&t, &bad, Vec3::Z, &opts).is_err());
        assert!(create_sweep_scale(&bad, &c, Vec3::Z, &opts).is_err());
    }
}
