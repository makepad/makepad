//! Offset of a NURBS surface: exact for planes, offset generator curves for surfaces
//! of revolution and planar extrusions, point sampling, interpolation and knot removal
//! for free-form surfaces.
//!
//! Upstream: `source/SMLib/src/NL_SrfOffset.cpp` (only `N_SrfOffset`; the rest of the
//! file is compiled out by `NLIB_UNUSED`).
//!
//! | Rust                 | Upstream                        |
//! |----------------------|---------------------------------|
//! | [`Surface::offset`], [`Surface::offset_with`] | `N_SrfOffset` |
//! | `SLM`, `TOTAL_MAX`   | `slm`, `totalmax` (file statics) |
//! | `UNIT_SQUARE`        | `NL_UNITSQUARE`                 |
//!
//! Deviations:
//! - The output surface `surQ` and the memory stacks become the return value; errors
//!   are [`SolidError::Nlib`] values with the upstream `NL_*_ERR` number instead of a
//!   flag plus the global `NL_ERROR` record. The `par` flag is
//!   [`OffsetParam`] (shared with [`Curve::offset_pt_sampling`]).
//! - Upstream ignores the error of two `N_SrfEvalPtPtDerivNormalFast` calls (the
//!   input's normal on the extrusion path, the result's normal on the revolution and
//!   extrusion paths) and reads the normal those calls left behind. The port
//!   reproduces that normal (`normal_left_by_failed_eval`) and, as upstream, returns
//!   the result's evaluation error unless the `N_SwapUV` that follows clears it.
//! - Upstream reads an unset tangent when `N_CrvEvalTangent` of the generator curve
//!   fails (its error is not checked); the port returns [`SolidError::InvalidInput`].
//!   The same holds when a revolution or extrusion comes back without a generator
//!   curve, and when a per-patch sample count `ceil(...)` is not finite or does not
//!   fit `NL_INDEX` (upstream casts it unchecked). Every other array access is bounds
//!   checked.
//! - `N_SrfReparamToInterval` errors are ignored, as upstream. `N_SrfCompress` only
//!   trims storage and is kept for fidelity. The unreachable `mu < 0 || mv < 0`
//!   check has no counterpart.
//! - Issue #471: [`OffsetOptions`] explicitly opts into a unit-direction extrusion
//!   test (and inherited-domain restoration on that path), plus independent u/v
//!   sampling. The latter scales the minimum density by relative span width, uses
//!   directional second derivatives with `|d|`, keeps one step on flat slivers even
//!   when capping the grid, and reserves part of `tol` for knot removal. Defaults
//!   retain the unmodified C++ algorithm and differential-test tolerances.
//!   With inherited parameters, fits are checked on cell edges and centers and
//!   refined for at most 24 iterations, still within the 10000-point grid budget;
//!   exhausting either bound returns an error instead of an unchecked surface.

use crate::error::{NlibCode, Result, SolidError};
use crate::math::{max2, Vec3};
use crate::nurbs::crv_basic::{ConicType, Curve};
use crate::nurbs::crv_fit::p1::interp_knot_vector;
use crate::nurbs::crv_offset::OffsetParam;
use crate::nurbs::geom::{normalize, pt_along_vector, Rectangle, LINE_TOL};
use crate::nurbs::index::{get, set, storage, sx, ux};
use crate::nurbs::knots::{KnotVector, Side};
use crate::nurbs::math::{float_op_is_bad, CPoint, FloatOp, MODEL_TOL};
use crate::nurbs::srf_adv::p1::offset_get_max_deriv;
use crate::nurbs::srf_adv::p3::grid_pts_normals;
use crate::nurbs::srf_basic::{Direction, Surface};
use crate::nurbs::srf_common::{corner_pts, extrude_crv, revolved_srf};
use crate::nurbs::srf_fit::{interp_given_knots, lst_sq_approx, param_values};
use crate::nurbs::srf_nurb::SurfaceKind;
use crate::nurbs::srf_tool::p1::decompose_to_bez;
use crate::nurbs::srf_tool::p3::remove_all_knots;

const OP: &str = "nurbs::srf_offset::Surface::offset";

/// Upstream `slm`: sample count above which a patch's count is bounded by its area.
const SLM: f64 = 1000.0;
/// Upstream `totalmax`: largest number of sampled grid points.
const TOTAL_MAX: isize = 10000;

/// Upstream `NL_UNITSQUARE`.
const UNIT_SQUARE: Rectangle = Rectangle::new(0.0, 1.0, 0.0, 1.0);

/// Optional corrections to `N_SrfOffset` (issue #471). The default preserves SMLib.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OffsetOptions {
    /// Compare unit extrusion and profile-normal directions, independent of extrusion
    /// length. Preserve the input domain when the inherited extrusion shortcut runs.
    pub normalize_extrusion_direction: bool,
    /// Choose independent u/v sample counts from each patch's directional curvature
    /// and relative span width, instead of a shared minimum for every patch.
    /// Inherited fits also refine cells whose sampled offset error exceeds `tol/2`.
    pub condition_sampling: bool,
}

const fn invalid(reason: &'static str) -> SolidError {
    SolidError::InvalidInput {
        operation: OP,
        reason,
    }
}

/// `(NL_INDEX)del` of a sample count (`del >= 0`, already rounded up): errors where
/// upstream's cast to the 64-bit `NL_INDEX` is undefined (not finite, negative or
/// `>= 2^63`). A count above `isize::MAX` (32-bit targets) saturates; every caller
/// only compares such a count with [`SLM`].
fn count(del: f64) -> Result<isize> {
    if !(0.0..9.223_372_036_854_775_808e18).contains(&del) {
        return Err(invalid("sample count not representable"));
    }
    Ok(del as isize)
}

/// The normal `N_SrfEvalPtPtDerivNormalFast` leaves in its `N` output when it fails
/// (upstream reads it without checking the error): zero when the derivatives fail or
/// a partial derivative cannot be normalised, the unnormalised cross product of the
/// unit partials when only its normalisation fails, else whatever the pole evaluator
/// `N_SrfEvalPtNormalAtPole` left there.
fn normal_left_by_failed_eval(sur: &Surface, u: f64, v: f64, ufl: Side, vfl: Side) -> Vec3 {
    let zero = Vec3::new(0.0, 0.0, 0.0);
    let Ok(sd) = sur.derivs(u, v, ufl, vfl, true, 1, 1) else {
        return zero;
    };
    let at = |k: usize, l: usize| sd.get(k).and_then(|r| r.get(l)).copied();
    let (Some(su), Some(sv)) = (at(1, 0), at(0, 1)) else {
        return zero;
    };
    let n = if su.length() <= 0.001 * MODEL_TOL {
        zero
    } else {
        let Ok((su, _)) = normalize(su) else {
            return zero;
        };
        if sv.length() <= 0.001 * MODEL_TOL {
            zero
        } else {
            let Ok((sv, _)) = normalize(sv) else {
                return zero;
            };
            let n = su.cross(sv);
            if n.length() > LINE_TOL {
                // Only the final normalisation can have failed.
                return normalize(n).map_or(n, |(x, _)| x);
            }
            n
        }
    };
    pole_normal(sur, u, v, ufl, vfl, n)
}

/// `N` after `N_SrfEvalPtNormalAtPole` with `n` in it on entry.
fn pole_normal(sur: &Surface, u: f64, v: f64, ufl: Side, vfl: Side, n: Vec3) -> Vec3 {
    let Ok(sd) = sur.derivs(u, v, ufl, vfl, false, 1, 1) else {
        return n;
    };
    let at = |k: usize, l: usize| sd.get(k).and_then(|r| r.get(l)).copied();
    let (Some(s10), Some(s01), Some(s11)) = (at(1, 0), at(0, 1), at(1, 1)) else {
        return n;
    };
    let mut pole = 0;
    let mut su = s10;
    let mut sv = s01;
    let mag = su.length();
    match normalize(su) {
        Ok((x, _)) if mag > 0.001 * MODEL_TOL => su = x,
        _ => {
            pole = 1;
            su = Vec3::new(0.0, 0.0, 0.0);
        }
    }
    let mag = sv.length();
    match normalize(sv) {
        Ok((x, _)) if mag > 0.001 * MODEL_TOL => sv = x,
        _ => {
            if pole == 1 {
                return n;
            }
            pole = 2;
        }
    }
    let n = match pole {
        1 => {
            let b = sur.parameter_bounds();
            let mut s01 = s01;
            if v == b.vt {
                s01 *= -1.0;
            }
            s11.cross(s01)
        }
        2 => {
            let b = sur.parameter_bounds();
            let mut s10 = s10;
            if u == b.ur {
                s10 *= -1.0;
            }
            s10.cross(s11)
        }
        _ => su.cross(sv),
    };
    if n.length() < 0.001 * MODEL_TOL {
        return n;
    }
    normalize(n).map_or(n, |(x, _)| x)
}

/// `N_SrfEvalPtPtDerivNormalFast` at `(u, v)` with `NL_RIGHT` flags whose error
/// upstream does not check: the error and the normal left in `N`.
fn unchecked_normal(sur: &Surface, u: f64, v: f64) -> (Result<()>, Vec3) {
    match sur.eval_pt_deriv_normal_fast(u, v, Side::Right, Side::Right) {
        Ok(e) => (Ok(()), e.n),
        Err(e) => (
            Err(e),
            normal_left_by_failed_eval(sur, u, v, Side::Right, Side::Right),
        ),
    }
}

/// Unit tangent of `cur` at its start (`N_CrvEvalTangent` with `NL_RIGHT`), whose
/// error upstream does not check.
fn start_tangent(cur: &Curve) -> Result<Vec3> {
    let (u0, _) = cur.param_bounds();
    cur.eval_tangent(u0, Side::Right)
        .map(|(_, t)| t)
        .map_err(|_| invalid("generator tangent undefined (upstream reads an unset vector)"))
}

/// The common tail of the revolution and extrusion paths: swap u and v of `sur_q`
/// when its normal at the domain corner opposes `np`. Returns the result's
/// evaluation error unless the swap clears it, as upstream.
fn orient(mut sur_q: Surface, np: Vec3) -> Result<Surface> {
    let b = sur_q.parameter_bounds();
    let (error, nq) = unchecked_normal(&sur_q, b.ul, b.vb);
    let dot = np.dot(nq);
    if dot < 0.0 {
        sur_q.swap_uv();
        return Ok(sur_q);
    }
    error.map(|()| sur_q)
}

impl Surface {
    /// Offset at distance `d` of this G1 continuous surface along its unit normal
    /// (`N_SrfOffset`).
    ///
    /// The type is recognised with [`Surface::surface_type`] and tolerance `eps`. A
    /// plane is offset exactly (translated, or with `par` other than
    /// [`OffsetParam::Inherited`] rebuilt as a bilinear patch on `[0,1]^2`). A sphere,
    /// torus, cone, cylinder or surface of revolution with a profile plane offsets
    /// its profile ([`Curve::offset_pt_sampling`]) and revolves it again; an
    /// extrusion of a planar curve along the plane normal offsets and extrudes the
    /// curve. Either result has u and v swapped when its normal at the domain corner
    /// opposes the input's. Any other surface is sampled on every Bezier patch with a
    /// density from [`offset_get_max_deriv`] and `tol` (at most 10000 points), the
    /// offset points are interpolated with degrees `(pp, qq)` (or approximated by a
    /// bicubic least-squares fit for a computed parametrisation), knots are removed
    /// within `tol`, and the result is reparametrised to the input's domain.
    /// Self-intersections and other offset anomalies are not checked.
    ///
    /// # Errors
    ///
    /// The errors of [`Surface::surface_type`], [`normalize`] (a degenerate plane),
    /// [`Surface::eval_pt_deriv_normal_fast`], [`Curve::offset_pt_sampling`], the
    /// surface constructors, [`decompose_to_bez`], [`offset_get_max_deriv`],
    /// [`grid_pts_normals`], the fitting routines and [`remove_all_knots`];
    /// [`NlibCode::Numerical`] when `1 / tol` is not computable;
    /// [`SolidError::InvalidInput`] where upstream's behaviour is undefined (see the
    /// module docs).
    pub fn offset(
        &self,
        d: f64,
        pp: usize,
        qq: usize,
        par: OffsetParam,
        eps: f64,
        tol: f64,
    ) -> Result<Surface> {
        self.offset_with(d, pp, qq, par, eps, tol, OffsetOptions::default())
    }

    /// [`Surface::offset`] with explicit corrections to upstream's extrusion test
    /// and sliver-span sampling. See [`OffsetOptions`].
    pub fn offset_with(
        &self,
        d: f64,
        pp: usize,
        qq: usize,
        par: OffsetParam,
        eps: f64,
        tol: f64,
        options: OffsetOptions,
    ) -> Result<Surface> {
        // Get surface type.
        let st = self.surface_type(eps)?;
        let stp = st.kind;
        let dir = st.dir;
        let (aa, bb, mut cc, dd) = (st.aa, st.bb, st.cc, st.dd);

        let flt = cc != Vec3::new(0.0, 0.0, 0.0);

        // Offset planar surface.
        if stp == SurfaceKind::Plane {
            let v1 = bb - aa;
            let v2 = dd - aa;
            let mut nn = normalize(v1.cross(v2));
            if nn.is_err() {
                // Assumed error is due to coincident BB,AA or DD,AA.
                let v3 = cc - aa;
                nn = normalize(v3.cross(v2));
                if nn.is_err() {
                    nn = normalize(v1.cross(v3));
                }
            }
            let (nn, _) = nn?;

            if par == OffsetParam::Inherited {
                let nn = Vec3::new(nn.x * d, nn.y * d, nn.z * d);
                let mut sur_q = self.clone();
                sur_q.translate(nn);
                return Ok(sur_q);
            }
            // Build new surface param on 0-1.
            let a = pt_along_vector(aa, d, nn);
            let b = pt_along_vector(bb, d, nn);
            let c = pt_along_vector(cc, d, nn);
            let e = pt_along_vector(dd, d, nn);
            return corner_pts(a, b, e, c);
        }

        // Offset surface of revolution.
        let rev = matches!(
            stp,
            SurfaceKind::Sphere
                | SurfaceKind::Torus
                | SurfaceKind::Cone
                | SurfaceKind::Cylinder
                | SurfaceKind::Revolution
        );

        if rev && flt {
            let pq = if matches!(stp, SurfaceKind::Cone | SurfaceKind::Cylinder) {
                1
            } else if dir == Some(Direction::U) {
                qq
            } else {
                pp
            };

            let b = self.parameter_bounds();
            let np = self
                .eval_pt_deriv_normal_fast(b.ul, b.vb, Side::Right, Side::Right)?
                .n;

            let cur_a = st
                .cur_a
                .as_ref()
                .ok_or_else(|| invalid("no generator curve"))?;
            let ta = start_tangent(cur_a)?;

            let cross = cc.cross(ta);
            let dot = np.dot(cross);
            if dot < 0.0 {
                cc = -cc;
            }

            let mut cur_p = cur_a.offset_pt_sampling(cc, d, pq, false, par, tol)?;
            let sur_q = revolved_srf(&mut cur_p, aa, bb, st.al, ConicType::Quadratic)?;
            return orient(sur_q, np);
        }

        // Offset extruded surface.
        if stp == SurfaceKind::Extrusion {
            let cur_a = st
                .cur_a
                .as_ref()
                .ok_or_else(|| invalid("no generator curve"))?;
            let planarity = cur_a.is_planar(eps)?;
            if planarity.flat {
                let (_, nn) = planarity
                    .plane
                    .ok_or_else(|| invalid("flat curve without a plane"))?;
                let axis = if options.normalize_extrusion_direction {
                    normalize(aa)?.0
                } else {
                    aa
                };
                let f1 = axis.cross(nn).length();
                if f1 < MODEL_TOL {
                    let pq = if dir == Some(Direction::U) { qq } else { pp };

                    let b = self.parameter_bounds();
                    let (_, np) = unchecked_normal(self, b.ul, b.vb);
                    let ta = start_tangent(cur_a)?;
                    let cross = nn.cross(ta);
                    let dot = np.dot(cross);
                    let nn = if dot < 0.0 { -nn } else { nn };

                    let mut cur_p = cur_a.offset_pt_sampling(nn, d, pq, false, par, tol)?;
                    let dir = dir.ok_or(SolidError::nlib(OP, NlibCode::Call))?;
                    let sur_q = extrude_crv(&mut cur_p, aa, st.r1, dir)?;
                    let mut sur_q = orient(sur_q, np)?;
                    if options.normalize_extrusion_direction && par == OffsetParam::Inherited {
                        sur_q.reparam_to_interval(self.parameter_bounds(), Direction::UV)?;
                    }
                    return Ok(sur_q);
                }
            }
        }

        self.offset_general(d, pp, qq, par, tol, options.condition_sampling)
    }

    /// The free-form path of [`Surface::offset`].
    fn offset_general(
        &self,
        d: f64,
        pp: usize,
        qq: usize,
        par: OffsetParam,
        tol: f64,
        condition_sampling: bool,
    ) -> Result<Surface> {
        // Offset general surface.
        let up = self.u_knots();
        let vp = self.v_knots();
        let (_, _, r, s) = self.array_sizes();
        let rp = sx(r, OP)?;
        let sp = sx(s, OP)?;
        let (pu, qu) = self.degrees();
        let p = sx(pu, OP)?;
        let q = sx(qu, OP)?;

        // Set output surface control point size (for the least-squares fit only).
        let mut nnn = rp + 1;
        let mut mmm = sp + 1;

        let exp = if pp == 1 || qq == 1 { 0.5 } else { 0.34 };

        if float_op_is_bad(1.0, tol, FloatOp::Division) {
            return Err(SolidError::nlib(OP, NlibCode::Numerical));
        }

        let mut f1 = 1.0 / tol;
        f1 = f1.powf(exp);
        let pq = pp.max(qq);
        let f2 = 2.0 * pq as f64;

        let du = 0.00005 * (get(up, rp, OP)? - get(up, 0, OP)?);
        let dv = 0.00005 * (get(vp, sp, OP)? - get(vp, 0, OP)?);

        // Get Bezier patches.
        let mut bez = decompose_to_bez(self)?;
        let ku = bez.len();
        let lv = bez.first().map_or(0, Vec::len);
        if ku == 0 || lv == 0 || bez.iter().any(|row| row.len() != lv) {
            return Err(invalid("empty Bezier decomposition"));
        }

        // Get number of sampling points.
        let mut np = vec![vec![(0_isize, 0_isize); lv]; ku];
        for (bez_row, np_row) in bez.iter_mut().zip(np.iter_mut()) {
            for (patch, npij) in bez_row.iter_mut().zip(np_row.iter_mut()) {
                let b = patch.parameter_bounds();
                if !condition_sampling && (b.ur - b.ul <= du || b.vt - b.vb <= dv) {
                    *npij = (1, 1);
                    continue;
                }

                // Upstream ignores the error.
                let _ = patch.reparam_to_interval(UNIT_SQUARE, Direction::UV);

                let md = offset_get_max_deriv(patch, if condition_sampling { d.abs() } else { d })?;

                if condition_sampling {
                    let bounds = self.parameter_bounds();
                    let fu = (b.ur - b.ul) / (bounds.ur - bounds.ul);
                    let fv = (b.vt - b.vb) / (bounds.vt - bounds.vb);
                    let samples = |fraction: f64, second: f64, mixed: f64| -> Result<isize> {
                        // Only curvature needs the tolerance-dependent density.
                        // Multiplying the degree floor by f1 oversamples even a
                        // straight ruling and consumes the refinement budget.
                        let curvature = f1 * (0.125 * second + 0.125 * mixed).sqrt();
                        let n = max2(f2 * fraction, curvature).ceil();
                        if !n.is_finite() {
                            return Err(invalid("non-finite sample count"));
                        }
                        count(n.clamp(1.0, SLM))
                    };
                    *npij = (
                        samples(fu, md.muu, md.muv * fu / fv)?,
                        samples(fv, md.mvv, md.muv * fv / fu)?,
                    );
                    continue;
                }

                let num = 0.125 * (md.muu + md.mvv) + 0.25 * md.muv;
                let gro = max2(f2, num.sqrt());
                let mut del = f1 * gro;
                del = del.ceil();

                if del >= SLM {
                    *npij = (SLM as isize, SLM as isize);
                    patch.offset_patch_net(d, pu, qu)?;

                    let al = patch.average_len();
                    let mut del = (al.area / tol).sqrt();
                    del = del.ceil();
                    let jj = count(del)?;
                    if jj < npij.0 {
                        *npij = (jj, jj);
                    }
                } else {
                    let n = count(del)?;
                    *npij = (n, n);
                }
            }
        }

        // Get maximum number of points to be sampled.
        let mut nu: Vec<isize> = Vec::with_capacity(ku);
        let mut mu: isize = 0;
        for row in &np {
            let mut x = row.first().map_or(0, |n| n.0);
            for &(y, _) in row.iter().skip(1) {
                if y > x {
                    x = y;
                }
            }
            nu.push(x);
            mu = mu
                .checked_add(x)
                .ok_or_else(|| invalid("too many samples"))?;
        }
        let mut nv: Vec<isize> = Vec::with_capacity(lv);
        let mut mv: isize = 0;
        for j in 0..lv {
            let col = |i: usize| np.get(i).and_then(|r| r.get(j)).map_or(0, |n| n.1);
            let mut x = col(0);
            for i in 1..ku {
                if col(i) > x {
                    x = col(i);
                }
            }
            nv.push(x);
            mv = mv
                .checked_add(x)
                .ok_or_else(|| invalid("too many samples"))?;
        }

        let total = (mu + 1).checked_mul(mv + 1);
        if total.is_none_or(|t| t > TOTAL_MAX) {
            let ddd = (mu as f64 + 1.0) * (mv as f64 + 1.0);
            let mut per = TOTAL_MAX as f64 / ddd;
            per = per.sqrt();

            mu = 0;
            for x in &mut nu {
                let ddd = *x as f64;
                *x = (ddd * per) as isize;
                let minimum = if condition_sampling { 1 } else { p };
                if *x < minimum {
                    *x = minimum;
                }
                mu += *x;
            }

            mv = 0;
            for x in &mut nv {
                let ddd = *x as f64;
                *x = (ddd * per) as isize;
                let minimum = if condition_sampling { 1 } else { q };
                if *x < minimum {
                    *x = minimum;
                }
                mv += *x;
            }
        }

        // Adjust output surface control point size (least-squares fit only).
        if condition_sampling
            && ((mu + 1).checked_mul(mv + 1).is_none_or(|n| n > TOTAL_MAX)
                || mu < sx(pp, OP)?
                || mv < sx(qq, OP)?)
        {
            return Err(invalid(
                "sample budget cannot accommodate spans and degrees",
            ));
        }
        if nnn > mu {
            nnn = mu;
        }
        if mmm > mv {
            mmm = mv;
        }

        // Compute parameters.
        let mut u = sample_params(up, p, rp, &nu, mu)?;
        let mut v = sample_params(vp, q, sp, &nv, mv)?;

        if condition_sampling && par == OffsetParam::Inherited {
            return self.offset_refined(d, pp, qq, u, v, tol);
        }

        // Sample surface.
        let g = grid_pts_normals(self, &u, &v, Side::Left, Side::Left)?;
        let pts: Vec<Vec<Vec3>> = g
            .points
            .iter()
            .zip(&g.normals)
            .map(|(pr, nr)| {
                pr.iter()
                    .zip(nr)
                    .map(|(&pt, &n)| pt_along_vector(pt, d, n))
                    .collect()
            })
            .collect();

        // Get knot vectors.
        let fit = match par {
            OffsetParam::Fit(m) => {
                (u, v) = param_values(&pts, m)?;
                Some(m)
            }
            OffsetParam::Inherited => None,
        };

        // Interpolate points.
        let mut sur_q = match fit {
            None => {
                let knu = interp_knot_vector(&u, pp)?;
                let knv = interp_knot_vector(&v, qq)?;
                let knu = KnotVector::from_slice(&knu)?;
                let knv = KnotVector::from_slice(&knv)?;
                interp_given_knots(&pts, &u, &v, &knu, &knv, pp, qq)?
            }
            Some(m) => lst_sq_approx(&pts, ux(nnn, OP)?, ux(mmm, OP)?, 3, 3, m)?,
        };

        // Remove knots.
        let removal_tol = if condition_sampling { 0.5 * tol } else { tol };
        sur_q = remove_all_knots(&sur_q, removal_tol, Direction::UV)?;

        // Compact output surface.
        let (n, m, _, _) = sur_q.array_sizes();
        if sx(n, OP)? < mu || sx(m, OP)? < mv {
            sur_q.compress();
        }

        // Rescale knot vector if necessary.
        if fit.is_some() {
            let uq = sur_q.u_knots();
            let vq = sur_q.v_knots();
            let (up0, upr) = (get(up, 0, OP)?, get(up, rp, OP)?);
            let (vp0, vps) = (get(vp, 0, OP)?, get(vp, sp, OP)?);
            let (uq0, uqr) = (
                uq.first().copied().ok_or_else(|| invalid("empty knots"))?,
                uq.last().copied().ok_or_else(|| invalid("empty knots"))?,
            );
            let (vq0, vqs) = (
                vq.first().copied().ok_or_else(|| invalid("empty knots"))?,
                vq.last().copied().ok_or_else(|| invalid("empty knots"))?,
            );

            if up0 != uq0 || upr != uqr || vp0 != vq0 || vps != vqs {
                let rect = Rectangle::new(up0, upr, vp0, vps);
                let dir = if up0 == uq0 && upr == uqr {
                    Direction::V
                } else if vp0 == vq0 && vps == vqs {
                    Direction::U
                } else {
                    Direction::UV
                };
                // Upstream ignores the error.
                let _ = sur_q.reparam_to_interval(rect, dir);
            }
        }

        Ok(sur_q)
    }

    /// Fit inherited parameters and refine only directions with measured error.
    /// This is a bounded sampling check, not a proof against offset singularities.
    fn offset_refined(
        &self,
        d: f64,
        p: usize,
        q: usize,
        mut u: Vec<f64>,
        mut v: Vec<f64>,
        tol: f64,
    ) -> Result<Surface> {
        const MAX_REFINEMENTS: u32 = 24;
        for _ in 0..MAX_REFINEMENTS {
            let grid = grid_pts_normals(self, &u, &v, Side::Left, Side::Left)?;
            let pts: Vec<Vec<Vec3>> = grid
                .points
                .iter()
                .zip(&grid.normals)
                .map(|(pr, nr)| {
                    pr.iter()
                        .zip(nr)
                        .map(|(&pt, &n)| pt_along_vector(pt, d, n))
                        .collect()
                })
                .collect();
            let ku = interp_knot_vector(&u, p)?;
            let kv = interp_knot_vector(&v, q)?;
            let ku = KnotVector::from_slice(&ku)?;
            let kv = KnotVector::from_slice(&kv)?;
            let fit = interp_given_knots(&pts, &u, &v, &ku, &kv, p, q)?;
            let fit = remove_all_knots(&fit, 0.25 * tol, Direction::UV)?;
            let inaccurate = |u: f64, v: f64| -> Result<bool> {
                let e = self.eval_pt_deriv_normal_fast(u, v, Side::Left, Side::Left)?;
                let target = pt_along_vector(e.p, d, e.n);
                let actual = fit.eval(u, v, Side::Left, Side::Left)?;
                let error = (actual - target).length();
                if !error.is_finite() {
                    return Err(SolidError::nlib(OP, NlibCode::Numerical));
                }
                Ok(error > 0.5 * tol)
            };
            // A bad interpolant at its own samples cannot be repaired by adding
            // more samples to the same ill-conditioned system.
            for &uu in &u {
                for &vv in &v {
                    if inaccurate(uu, vv)? {
                        return Err(SolidError::nlib(OP, NlibCode::Numerical));
                    }
                }
            }
            let mut refine_u = vec![false; u.len().saturating_sub(1)];
            let mut refine_v = vec![false; v.len().saturating_sub(1)];
            for (span_u, ru) in u.windows(2).zip(&mut refine_u) {
                let &[u0, u1] = span_u else {
                    return Err(invalid("missing sample interval"));
                };
                let um = u0 + 0.5 * (u1 - u0);
                for (span_v, rv) in v.windows(2).zip(&mut refine_v) {
                    let &[v0, v1] = span_v else {
                        return Err(invalid("missing sample interval"));
                    };
                    let vm = v0 + 0.5 * (v1 - v0);
                    let bad_u = inaccurate(um, v0)? || inaccurate(um, v1)?;
                    let bad_v = inaccurate(u0, vm)? || inaccurate(u1, vm)?;
                    let interior = !bad_u && !bad_v && inaccurate(um, vm)?;
                    *ru |= bad_u || interior;
                    *rv |= bad_v || interior;
                }
            }
            if !refine_u.contains(&true) && !refine_v.contains(&true) {
                return Ok(fit);
            }
            let nu = u.len() + refine_u.iter().filter(|&&b| b).count();
            let nv = v.len() + refine_v.iter().filter(|&&b| b).count();
            if nu.checked_mul(nv).is_none_or(|n| n > TOTAL_MAX as usize) {
                return Err(invalid("offset refinement exceeds sample budget"));
            }
            u = refine_parameters(&u, &refine_u)?;
            v = refine_parameters(&v, &refine_v)?;
        }
        Err(SolidError::NotConverged {
            operation: OP,
            iterations: MAX_REFINEMENTS,
        })
    }

    /// The `del >= slm` branch of `N_SrfOffset` on this unit-square Bezier patch:
    /// moves every control point along the unit normal at its uniform parameter.
    fn offset_patch_net(&mut self, d: f64, p: usize, q: usize) -> Result<()> {
        let uinc = 1.0 / p as f64;
        let vinc = 1.0 / q as f64;

        let mut nrm = vec![vec![Vec3::new(0.0, 0.0, 0.0); q + 1]; p + 1];
        for (ii, row) in nrm.iter_mut().enumerate() {
            let mut ul = ii as f64 * uinc;
            if ul > 1.0 {
                ul = 1.0;
            }
            for (jj, n) in row.iter_mut().enumerate() {
                let mut vl = jj as f64 * vinc;
                if vl > 1.0 {
                    vl = 1.0;
                }
                *n = match self.eval_pt_deriv_normal_fast(ul, vl, Side::Left, Side::Left) {
                    Ok(e) => e.n,
                    Err(_) => {
                        self.eval_pt_normal_at_pole(ul, vl, Side::Left, Side::Left)?
                            .n
                    }
                };
            }
        }

        for (ii, row) in nrm.iter().enumerate() {
            for (jj, &n) in row.iter().enumerate() {
                let pw = self.cpt_mut(ii, jj)?;
                let (a, h) = pw.to_euclid_and_w();
                let a = pt_along_vector(a, d, n);
                *pw = CPoint::weighted(a, h);
            }
        }
        Ok(())
    }
}

/// Insert each requested midpoint, keeping the existing parameters and their order.
fn refine_parameters(parameters: &[f64], refine: &[bool]) -> Result<Vec<f64>> {
    let mut result = Vec::new();
    for (span, &split) in parameters.windows(2).zip(refine) {
        let &[lo, hi] = span else {
            return Err(invalid("missing sample interval"));
        };
        result.push(lo);
        if split {
            let mid = lo + 0.5 * (hi - lo);
            if !(lo < mid && mid < hi) {
                return Err(invalid(
                    "offset samples cannot be refined at this precision",
                ));
            }
            result.push(mid);
        }
    }
    result.push(
        *parameters
            .last()
            .ok_or_else(|| invalid("missing sample interval"))?,
    );
    Ok(result)
}

/// Sample parameters of one direction: `ns[j]` equal steps on the `j`-th non-zero
/// knot span of `kn` (degree `p`, highest knot index `r`), `hi + 1` values.
fn sample_params(kn: &[f64], p: isize, r: isize, ns: &[isize], hi: isize) -> Result<Vec<f64>> {
    let mut u: Vec<f64> = storage(hi, OP)?;
    let mut ul = get(kn, p, OP)?;
    let mut i = p + 1;
    let mut j: isize = 0;
    set(&mut u, 0, ul, OP)?;
    let mut l: isize = 0;

    while i < r {
        while i < r && get(kn, i, OP)? == get(kn, i + 1, OP)? {
            i += 1;
        }
        let ur = get(kn, i, OP)?;
        let nj = get(ns, j, OP)?;
        let uinc = (ur - ul) / nj as f64;
        for k in 1..nj {
            l += 1;
            set(&mut u, l, ul + k as f64 * uinc, OP)?;
        }
        l += 1;
        set(&mut u, l, ur, OP)?;
        ul = ur;
        i += 1;
        j += 1;
    }
    Ok(u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::crv_fit::p1::ParamMethod;
    use crate::nurbs::math::NOW;
    use crate::nurbs::srf_tool::p1::insert_knot;

    fn check_offset(s: &Surface, q: &Surface, d: f64, tol: f64) {
        assert_eq!(s.parameter_bounds(), q.parameter_bounds());
        let b = s.parameter_bounds();
        for i in 0..=40 {
            for j in 0..=20 {
                let u = b.ul + (b.ur - b.ul) * (f64::from(i) / 40.0);
                let v = b.vb + (b.vt - b.vb) * (f64::from(j) / 20.0);
                let e = s
                    .eval_pt_deriv_normal_fast(u, v, Side::Left, Side::Left)
                    .unwrap();
                let p = q.eval(u, v, Side::Left, Side::Left).unwrap();
                let error = (p - pt_along_vector(e.p, d, e.n)).length();
                assert!(error <= tol, "u={u} v={v} error={error} tol={tol}");
            }
        }
    }

    #[test]
    fn conditioned_sampling_handles_slivers_in_either_parameter_direction() {
        for width in [2.1e-4, 1e-6] {
            let s = grid(3, 3, |x, y| 0.2 * x * x - 0.1 * y * y + 0.05 * x * y);
            let s = insert_knot(&s, 0.5, 3, Direction::U).unwrap();
            let mut s = insert_knot(&s, 0.5 + width, 3, Direction::U).unwrap();
            s.reparam_to_interval(Rectangle::new(20.0, 60.0, -2.0, 3.0), Direction::UV)
                .unwrap();
            for _ in 0..2 {
                for d in [-0.25, 0.25] {
                    let q = s
                        .offset_with(
                            d,
                            3,
                            3,
                            OffsetParam::Inherited,
                            1e-6,
                            1e-4,
                            OffsetOptions {
                                condition_sampling: true,
                                ..OffsetOptions::default()
                            },
                        )
                        .unwrap();
                    check_offset(&s, &q, d, 1e-4);
                    assert!(q
                        .cpts()
                        .iter()
                        .flatten()
                        .all(|p| p.to_euclid().length() < 10.0));
                }
                s.swap_uv();
            }
        }
    }

    #[test]
    fn extrusion_test_is_independent_of_length_and_rejects_oblique_axes() {
        let options = OffsetOptions {
            normalize_extrusion_direction: true,
            ..OffsetOptions::default()
        };
        for length in [0.1, 100.0] {
            for tilt in [1e-8, 1e-3] {
                for dir in [Direction::U, Direction::V] {
                    let mut curve = Curve::new(
                        vec![
                            CPoint::from_point(Vec3::new(0.0, 0.0, 0.0)),
                            CPoint::from_point(Vec3::new(1.0, 2.0, 0.0)),
                            CPoint::from_point(Vec3::new(3.0, 1.0, 0.0)),
                            CPoint::from_point(Vec3::new(4.0, 0.0, 0.0)),
                        ],
                        3,
                        vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
                    )
                    .unwrap();
                    let mut s =
                        extrude_crv(&mut curve, Vec3::new(tilt, 0.0, 1.0), length, dir).unwrap();
                    s.reparam_to_interval(Rectangle::new(20.0, 60.0, -2.0, 3.0), Direction::UV)
                        .unwrap();
                    let q = s
                        .offset_with(0.1, 3, 3, OffsetParam::Inherited, 1e-6, 1e-4, options)
                        .unwrap();
                    let expected = if tilt == 1e-3 {
                        (3, 3)
                    } else if dir == Direction::U {
                        (1, 3)
                    } else {
                        (3, 1)
                    };
                    assert_eq!(q.degrees(), expected);
                    check_offset(&s, &q, 0.1, 1e-4);
                }
            }
        }
    }

    fn grid(n: usize, m: usize, f: impl Fn(f64, f64) -> f64) -> Surface {
        let net: Vec<Vec<CPoint>> = (0..=n)
            .map(|i| {
                (0..=m)
                    .map(|j| {
                        let (x, y) = (i as f64, j as f64);
                        CPoint::new(x, y, f(x, y), NOW)
                    })
                    .collect()
            })
            .collect();
        let kn = |n: usize| {
            let mut k = vec![0.0; 4];
            for i in 1..n - 2 {
                k.push(i as f64 / (n - 2) as f64);
            }
            k.extend([1.0; 4]);
            k
        };
        Surface::new(net, 3, 3, kn(n), kn(m)).unwrap()
    }

    #[test]
    fn plane_offset_translates() {
        let s = grid(3, 3, |_, _| 0.0);
        let q = s
            .offset(0.5, 3, 3, OffsetParam::Inherited, 1e-3, 1e-4)
            .unwrap();
        for row in q.cpts() {
            for pw in row {
                assert!((pw.to_euclid().z.abs() - 0.5).abs() < 1e-12);
            }
        }
        let q = s
            .offset(
                0.5,
                3,
                3,
                OffsetParam::Fit(ParamMethod::ChordLength),
                1e-3,
                1e-4,
            )
            .unwrap();
        assert_eq!(q.degrees(), (1, 1));
    }

    #[test]
    fn freeform_offset_is_at_distance() {
        let s = grid(4, 4, |x, y| 0.2 * x * x - 0.1 * y * y + 0.05 * x * y);
        let d = 0.25;
        let q = s
            .offset(d, 3, 3, OffsetParam::Inherited, 1e-6, 1e-4)
            .unwrap();
        let e = s
            .eval_pt_deriv_normal_fast(0.0, 0.0, Side::Left, Side::Left)
            .unwrap();
        let o = pt_along_vector(e.p, d, e.n);
        let a = q.cpt(0, 0).unwrap().to_euclid();
        assert!((a - o).length() < 1e-6);
    }

    #[test]
    fn large_patches_take_area_bound_and_total_cap() {
        let s = grid(5, 5, |x, y| 300.0 * (x * x - y * y) + 100.0 * x * y);
        let tol: f64 = 1e-4;
        // The sample count of some patch exceeds `slm`.
        let f1 = (1.0 / tol).powf(0.34);
        let big = decompose_to_bez(&s).unwrap().iter().flatten().any(|b| {
            let mut b = b.clone();
            b.reparam_to_interval(UNIT_SQUARE, Direction::UV).unwrap();
            let md = offset_get_max_deriv(&b, 0.5).unwrap();
            let num = 0.125 * (md.muu + md.mvv) + 0.25 * md.muv;
            (f1 * max2(6.0, num.sqrt())).ceil() >= SLM
        });
        assert!(big);
        let q = s
            .offset(0.5, 3, 3, OffsetParam::Inherited, 1e-2, tol)
            .unwrap();
        assert_eq!(q.degrees(), (3, 3));
    }

    #[test]
    fn bad_tolerance() {
        let s = grid(4, 4, |x, y| 0.2 * x * x - 0.1 * y * y);
        let e = s.offset(0.1, 3, 3, OffsetParam::Inherited, 1e-6, 0.0);
        assert!(matches!(
            e,
            Err(SolidError::Nlib {
                code: NlibCode::Numerical,
                ..
            })
        ));
    }

    #[test]
    fn sample_counts_follow_the_nl_index_cast() {
        assert_eq!(count(3.0).unwrap(), 3);
        // Finite counts beyond 1e15 cast fine upstream (and lose to `slm`).
        assert!(count(1.0e16).unwrap() >= 1000);
        assert!(count(f64::INFINITY).is_err());
        assert!(count(f64::NAN).is_err());
        assert!(count(9.3e18).is_err());
        assert!(count(-1.0).is_err());
    }
}
