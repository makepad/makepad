//! `SmSurface`, part 4: the lazy evaluator's normal and cross-section evaluations, the
//! point-on-normal convergence test, the seam curve and seam test, the global point solve,
//! the classification of curve/surface intersection points, and the Newton equation sets of
//! the curve/surface and surface/surface solves (intersection, normalize, minimize,
//! maximize, signed directed minimize, surface/surface/plane intersection).
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`, `source/SMLib/inc/SmSurface.h`.
//! The file is split over several plan units (`surface.surface_p1`, `_p2`, ...); each lives
//! in its own submodule and adds methods to [`Surface`] (or free functions).
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`SurfaceEval::normal`]                           | `SmSurfaceEval::Normal`                          |
//! | [`SurfaceEval::eval_normal_section`]              | `SmSurfaceEval::EvalNormalSection`               |
//! | [`pass_sp_convergence_test`]                      | `sm_PassSPConvergenceTest` (static)              |
//! | [`Surface::create_seam_curve`]                    | `SmSurface::CreateSeamCurve`                     |
//! | [`Surface::is_on_seam`], [`SeamInfo`]             | `SmSurface::IsOnSeam`                            |
//! | [`Surface::global_point_solve`], [`Surface::global_point_solve_point_test_off`], [`Surface::global_point_solve_face_tested`], [`FaceTest`], [`global_point_solve_starts`], [`NoObject`] | `SmSurface::GlobalPointSolve` |
//! | [`classify_curve_surface_xsect_point`], [`XsectClassification`] | `sm_ClassifyCurveSurfaceXSectPoint` (static) |
//! | [`CsIntersectEnfo`] (`EvalNFunctions`)           | `SmCSIntersectENFO::Evaluate` (struct: `surface.surface_p1`) |
//! | [`CsNormalizeEnfo`] (`EvalNFunctions`)            | `SmCSNormalizeENFO::Evaluate` (struct: `surface.surface_p1`) |
//! | [`SspIntersectEnfo`] (`EvalNFunctions`)           | `SmSSPIntersectENFO::Evaluate` (struct: `surface.surface_p1`) |
//! | [`SsiNormalizeEnfo`] (`EvalNFunctions`)           | `SmSSINormalizeENFO::Evaluate` (struct: `surface.surface_p1`) |
//! | [`SsNormalizeEnfo`] (`EvalNFunctions`)            | `SmSSNormalizeENFO::Evaluate` (struct: `surface.surface_p1`) |
//! | [`is_point_on_normal`], [`PointOnNormal`], [`NormalTestReason`] (co-ported) | `smsurf_IsPointOnNormal` (unit `surface.surface_p3`) |
//! | [`SurfaceEval::eval_normal`] (co-ported)          | `SmSurfaceEval::EvalNormal` (unit `surface.surface_p3`) |
//!
//! Deviations:
//! - **Maximize without a handle.** `GlobalPointSolve(.., maximize)` names `this` in its
//!   solutions; a caller without a handle (`Surface::drop_point`) gets the same solutions
//!   with null objects.
//! - **Equation sets.** Each `Sm*ENFO` class is a struct of unit `surface.surface_p1`
//!   (`surface::surface::p1::*Enfo`, with its constructor); this unit implements
//!   [`EvalNFunctions`] for them. `SmCSIntersectENFO` keeps its call count, last gap and
//!   minimize flag in the struct, as upstream. The finite-difference Jacobian `SmSSINormalizeENFO` computes after
//!   its analytic one is the only one that reaches the caller, so the analytic one is not
//!   built. In the `sN2U` fallback `SmSSINormalizeENFO` evaluates the
//!   second point's normal on the *first* surface, as upstream does (kept, since the difftest
//!   compares it); the small-gap branch uses the other surface.
//! - **Seam curves of planes.** `SmPlane` is an `SmBSplineSurface` upstream, so with
//!   `known_closed` the C++ builds a seam curve for it; [`Surface::create_seam_curve`]
//!   returns `InvalidInput` for a plane (its base is not a stored B-spline surface here),
//!   and `Unported` (`curve.crv_on_surf` dispatch gap) for an `SmSurfOfRevolution`,
//!   `SmSphere` or `SmTorus`.
//! - **Owner checks.** Only an owner of type `SmFace` (surface) or `SmEdge` (curve) takes
//!   the topology path, as upstream's `SM_CAST_PTR`.
//! - **Co-ported callees.** `smsurf_IsPointOnNormal` and `SmSurfaceEval::EvalNormal` are
//!   small callees owned by unit `surface.surface_p3` that this unit's functions need to
//!   run at all; they are ported here (recorded as `CO-PORTED` in the plan notes).
//!   `smsurf_EvaluateNormalSection` is `surface::surface::p1::evaluate_normal_section`.
//!   `Normal` fails with the status of a failed evaluation where upstream asserts and returns the stale matrix entry.
//! - **Statuses.** `smsurf_IsPointOnNormal` returns its `SmStatus` reasons as
//!   [`NormalTestReason`] and always succeeds, as upstream; its deviation output is `0` when
//!   the surface could not be evaluated (upstream leaves it unset).
//! - **`CreateSeamCurve`** returns `None` for a surface that is not closed and builds
//!   its [`CrvOnSurf`] on the concrete [`Surface`], retaining its virtual evaluation
//!   and native owning-copy behavior.
//! - **Classification** (`sm_ClassifyCurveSurfaceXSectPoint`) returns the outputs as
//!   [`XsectClassification`]; an out-of-domain guess keeps its unclamped values, as
//!   upstream. `SmCurve::EvaluateGeometric` failing with `SM_ERR` marks the point touching
//!   (other failures propagate; upstream would read uninitialised vectors). The shape
//!   variables only read on paths where they were set start at zero.
//! - **Kernel.** `is_on_seam` and `global_point_solve` (and `global_point_solve_starts`)
//!   take the [`Kernel`] that the tolerance, drop and cache callees now need.
//! - **Unported callees** (return [`SolidError::Unported`]): the zone tolerance of a face's
//!   surface whose B-rep has not cached it on the surface ([`Surface::owner_zone_tol_3d`];
//!   `SmFace::GetTolerance`, `topology.face_p2`), `GlobalPointSolve_1`
//!   (`surface.surface_point_solve_p3`), the trim containment of a face's surface
//!   (`SmFace::PointClassify`, `query.face_p2`) and the edge and face tolerances of owned
//!   curves and surfaces (`topology.edge`, `topology.face_p2`).

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p2::evaluate_geometric as curve_evaluate_geometric;
use crate::curve::curve::Curve;
use crate::curve::line::Line;
use crate::error::{ignore_status, ignored, Result, SolidError, SM_ERR};
use crate::geom::assert_array::labels::{EDGE_TYPE, FACE_TYPE};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::{
    circle_coplanar_circle_intersect, line_coplanar_circle_intersect, line_point_distance,
    vec_lin_comb_two_vectors, CircleCircle,
};
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::local_solve_nd::{compute_jacobian, EvalNFunctions};
use crate::geom::matrix::Matrix;
use crate::geom::solution_array::{
    ObjectKind, OperandObject, Solution, SolutionArray, SolutionEnd, SolutionObject, MAX_TREES,
};
use crate::geom::vector2d::Vec2;
use crate::geom::vector3d::{asin, sqrt as sm_sqrt};
use crate::kernel::Kernel;
use crate::math::{
    Point2, Point3, Vec3, EFF_ZERO, EFF_ZERO_RAD, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE,
};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::surface::plane::Plane;

use super::p1::get_surface_cache;
use super::p1::{
    evaluate_1st_derivatives, evaluate_2nd_derivatives, evaluate_directional_derivs,
    evaluate_normal_section, map_to_2d_dir, zone_tol_3d, CsIntersectEnfo, CsNormalizeEnfo,
    SsNormalizeEnfo, SsiNormalizeEnfo, SspIntersectEnfo,
};
use super::p2::{
    convert_to_parameter_delta, evaluate_geometric, LocalSurfaceType, SurfaceEval, TsectCurveType,
};
use super::Surface;

const OP_CS_INTERSECT: &str = "surface::surface::p4::CsIntersectEnfo::evaluate";
const OP_CS_NORMALIZE: &str = "surface::surface::p4::CsNormalizeEnfo::evaluate";
const OP_SSP_INTERSECT: &str = "surface::surface::p4::SspIntersectEnfo::evaluate";
const OP_SSI_NORMALIZE: &str = "surface::surface::p4::SsiNormalizeEnfo::evaluate";
const OP_SS_NORMALIZE: &str = "surface::surface::p4::SsNormalizeEnfo::evaluate";
const OP_EVAL_NORMAL: &str = "surface::surface::p4::SurfaceEval::eval_normal";
const OP_CLASSIFY: &str = "surface::surface::p4::classify_curve_surface_xsect_point";

/// The trim boundary test a caller holding a face gives
/// [`Surface::global_point_solve_face_tested`]: whether the solution at `uv` (classified
/// with the tolerance given) lies in or on the face; `None` for no test.
pub type FaceTest<'a> = Option<&'a mut dyn FnMut(&Kernel, Point2, f64) -> Result<bool>>;

/// Entry `i` of an evaluation result.
fn nth(v: &[Vec3], i: usize, op: &'static str) -> Result<Vec3> {
    v.get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(op, "evaluation result too short"))
}

/// The `x` and `f` slices must hold `n` entries.
fn check_len(x: &[f64], f: &[f64], n: usize, op: &'static str) -> Result<()> {
    if x.len() == n && f.len() == n {
        Ok(())
    } else {
        Err(SolidError::invalid_input(
            op,
            "parameter or function array has the wrong length",
        ))
    }
}

/// Writes `values` into the leading entries of `f`.
fn set_all(f: &mut [f64], values: &[f64]) {
    for (slot, value) in f.iter_mut().zip(values) {
        *slot = *value;
    }
}

impl EvalNFunctions for CsIntersectEnfo<'_> {
    /// `Evaluate`: `x = [t u v]`; never reports an answer found.
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        mut jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let k = self.kernel;
        check_len(x, f, 3, OP_CS_INTERSECT)?;
        let (Some(&t), Some(&u), Some(&v)) = (x.first(), x.get(1), x.get(2)) else {
            return Err(SolidError::sm_err(OP_CS_INTERSECT));
        };

        // count calls
        self.count += 1;

        // Evaluate CurvePoint
        let crv = self.curve.evaluate(k, t, 1, true, true)?;
        let c = nth(&crv, 0, OP_CS_INTERSECT)?;
        let ct = nth(&crv, 1, OP_CS_INTERSECT)?;

        // Evaluate SurfacePoint
        let uv = Point2::new(u, v);
        let (mut s, mut du, mut dv) =
            evaluate_1st_derivatives(k, self.surface, uv, Side::Left, Side::Left)?;

        // Fail at singular Curve and Surface Points - zero tangent lengths
        if du.length_squared() < EFF_ZERO_SQ
            || dv.length_squared() < EFF_ZERO_SQ
            || ct.length_squared() < EFF_ZERO_SQ
        {
            return Err(SolidError::sm_err(OP_CS_INTERSECT));
        }

        // get Gap Vector = S - C
        let mut gap: Vec3 = s - c;

        if let Some(jac) = jacobian.as_deref_mut() {
            jac.set_at(0, 0, -ct.x)?;
            jac.set_at(0, 1, du.x)?;
            jac.set_at(0, 2, dv.x)?;

            jac.set_at(1, 0, -ct.y)?;
            jac.set_at(1, 1, du.y)?;
            jac.set_at(1, 2, dv.y)?;

            jac.set_at(2, 0, -ct.z)?;
            jac.set_at(2, 1, du.z)?;
            jac.set_at(2, 2, dv.z)?;
        }

        // Compute function values
        set_all(f, &[gap.x, gap.y, gap.z]);

        // Check for solver not converging: too many iterations near a tangency.
        let iter_lim: u64 = 10;
        let tangent_tol_deg = 0.5;
        let scaled_zero = EFF_ZERO * (1.0 + s.max_dimension());

        if self.do_minimize || (self.count > iter_lim && gap.length_squared() > self.last_sum) {
            // if we are near a tangent point
            let crv2 = self.curve.evaluate(k, t, 2, true, true)?;
            let norm = self
                .surface
                .evaluate_normal(k, uv, Side::Left, Side::Left)?;
            let c0 = nth(&crv2, 0, OP_CS_INTERSECT)?;
            let c1 = nth(&crv2, 1, OP_CS_INTERSECT)?;
            if norm.is_perpendicular_to(c1, tangent_tol_deg) {
                // Slow convergence problem.  Switch to a normalize operation.
                self.do_minimize = true;
                let d2 = evaluate_2nd_derivatives(k, self.surface, uv, Side::Left, Side::Left)?;
                s = d2.point;
                du = d2.du;
                dv = d2.dv;
                let (duv, duu, dvv) = (d2.duv, d2.duu, d2.dvv);
                let c2 = nth(&crv2, 2, OP_CS_INTERSECT)?;

                gap = s - c0;

                // translate the surface point by its normal when the gap is tiny
                if gap.length_squared() < 1000.0 * scaled_zero {
                    if norm.dot(gap) >= 0.0 {
                        gap = (s + norm) - c0;
                    } else {
                        gap = (s - norm) - c0;
                    }
                }

                if let Some(jac) = jacobian {
                    jac.set_at(0, 0, c2.dot(gap) - c1.dot(c1))?;
                    jac.set_at(0, 1, c1.dot(du))?;
                    jac.set_at(0, 2, c1.dot(dv))?;

                    jac.set_at(1, 0, -du.dot(c1))?;
                    jac.set_at(1, 1, duu.dot(gap) + du.dot(du))?;
                    let j12 = duv.dot(gap) + du.dot(dv);
                    jac.set_at(1, 2, j12)?;

                    jac.set_at(2, 0, -dv.dot(c1))?;
                    jac.set_at(2, 1, j12)?;
                    jac.set_at(2, 2, dvv.dot(gap) + dv.dot(dv))?;
                }

                set_all(f, &[c1.dot(gap), du.dot(gap), dv.dot(gap)]);
            }
        }

        if self.count > 9 {
            self.last_sum = gap.length_squared();
        }

        Ok(false)
    }
}

impl EvalNFunctions for CsNormalizeEnfo<'_> {
    /// `Evaluate`: `x = [t u v]` (and `w` for the signed directed minimize); never
    /// reports an answer found.
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        mut jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let k = self.kernel;
        let directed = self.solver_operation == SolverOperation::Signed3dDirectedMinimize;
        let count = if directed { 4 } else { 3 };
        check_len(x, f, count, OP_CS_NORMALIZE)?;
        let (Some(&t), Some(&u), Some(&v)) = (x.first(), x.get(1), x.get(2)) else {
            return Err(SolidError::sm_err(OP_CS_NORMALIZE));
        };

        // evaluate Curve Point and derivs
        let crv = self.curve.evaluate(k, t, 2, true, true)?;
        let c = nth(&crv, 0, OP_CS_NORMALIZE)?;
        let ct = nth(&crv, 1, OP_CS_NORMALIZE)?;
        let ctt = nth(&crv, 2, OP_CS_NORMALIZE)?;

        // evaluate Surface Point and derivs
        let uv = Point2::new(u, v);
        let d2 = evaluate_2nd_derivatives(k, self.surface, uv, Side::Left, Side::Left)?;
        let (s, su, sv, suu, suv, svv) = (d2.point, d2.du, d2.dv, d2.duu, d2.duv, d2.dvv);

        // Fail at singular curve or surface points - zero length tangents
        if ct.length_squared() < EFF_ZERO_SQ
            || su.length_squared() < EFF_ZERO_SQ
            || sv.length_squared() < EFF_ZERO_SQ
        {
            return Err(SolidError::sm_err(OP_CS_NORMALIZE));
        }

        // get gap vector
        let mut gap: Vec3 = s - c;

        let scaled_zero = EFF_ZERO * 1000.0 * (1.0 + s.max_dimension());

        // Translate the surface point by its normal when the gap gets very small.
        if gap.length_squared() < scaled_zero {
            let norm = if directed {
                nth(self.opt_vectors.unwrap_or(&[]), 0, OP_CS_NORMALIZE)?
            } else {
                self.surface
                    .evaluate_normal(k, uv, Side::Left, Side::Left)?
            };
            if norm.dot(gap) >= 0.0 {
                gap = (s + norm) - c;
            } else {
                gap = (s - norm) - c;
            }
        }

        // Unitize Ct and get its derivative.
        let ct_unit = ct.unitize_or_z();
        let ct_unit_t = ct.unitized_derivative(ctt);

        if directed {
            if self.opt_vectors.unwrap_or(&[]).is_empty() {
                return Err(SolidError::null_pointer(OP_CS_NORMALIZE));
            }
            let dir = nth(self.opt_vectors.unwrap_or(&[]), 0, OP_CS_NORMALIZE)?;

            // Surface Normal, unit, and derivatives
            let cross = su.cross(sv);
            let cross_u = su.derivative_of_cross_product(sv, suu, suv);
            let cross_v = su.derivative_of_cross_product(sv, suv, svv);

            let n = cross.unitize_or_z();
            let n_u = cross.unitized_derivative(cross_u);
            let n_v = cross.unitized_derivative(cross_v);

            if let Some(jac) = jacobian.as_deref_mut() {
                jac.set_at(0, 0, ct_unit_t.dot(n))?;
                jac.set_at(0, 1, ct_unit.dot(n_u))?;
                jac.set_at(0, 2, ct_unit.dot(n_v))?;
                jac.set_at(0, 3, 0.0)?;

                jac.set_at(1, 0, -ct.x)?;
                jac.set_at(1, 1, su.x)?;
                jac.set_at(1, 2, sv.x)?;
                jac.set_at(1, 3, -dir.x)?;

                jac.set_at(2, 0, -ct.y)?;
                jac.set_at(2, 1, su.y)?;
                jac.set_at(2, 2, sv.y)?;
                jac.set_at(2, 3, -dir.y)?;

                jac.set_at(3, 0, -ct.z)?;
                jac.set_at(3, 1, su.z)?;
                jac.set_at(3, 2, sv.z)?;
                jac.set_at(3, 3, -dir.z)?;
            }

            // Compute function values
            let w = x.get(3).copied().unwrap_or(0.0);
            let vec: Vec3 = (s - c) - w * dir;

            set_all(f, &[ct_unit.dot(n), vec.x, vec.y, vec.z]);
        } else {
            // Normalization, Minimization, Maximization: unitized Ct, Su and Sv.
            let su_unit = su.unitize_or_z();
            let su_unit_u = su.unitized_derivative(suu);
            let su_unit_v = su.unitized_derivative(suv);
            let sv_unit = sv.unitize_or_z();
            let sv_unit_u = sv.unitized_derivative(suv);
            let sv_unit_v = sv.unitized_derivative(svv);

            if let Some(jac) = jacobian {
                jac.set_at(0, 0, ct_unit_t.dot(gap) - ct_unit.dot(ct))?;
                jac.set_at(0, 1, ct_unit.dot(su))?;
                jac.set_at(0, 2, ct_unit.dot(sv))?;

                jac.set_at(1, 0, -su_unit.dot(ct))?;
                jac.set_at(1, 1, su_unit_u.dot(gap) + su_unit.dot(su))?;
                jac.set_at(1, 2, su_unit_v.dot(gap) + su_unit.dot(sv))?;

                jac.set_at(2, 0, -sv_unit.dot(ct))?;
                jac.set_at(2, 1, sv_unit_u.dot(gap) + sv_unit.dot(su))?;
                jac.set_at(2, 2, sv_unit_v.dot(gap) + sv_unit.dot(sv))?;
            }

            set_all(f, &[ct_unit.dot(gap), su_unit.dot(gap), sv_unit.dot(gap)]);
        }

        Ok(false)
    }
}

impl EvalNFunctions for SspIntersectEnfo<'_> {
    /// `Evaluate`: true when both surface points lie on the plane and coincide.
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let k = self.kernel;
        check_len(x, f, 4, OP_SSP_INTERSECT)?;
        let (Some(&u1), Some(&v1), Some(&u2), Some(&v2)) =
            (x.first(), x.get(1), x.get(2), x.get(3))
        else {
            return Err(SolidError::sm_err(OP_SSP_INTERSECT));
        };

        // get surface 3d points - with 1st derivatives
        let uv = Point2::new(u1, v1);
        let uv2 = Point2::new(u2, v2);
        let (s_f, duf, dvf) =
            evaluate_1st_derivatives(k, self.surface, uv, Side::Left, Side::Left)?;
        let (s_g, dug, dvg) =
            evaluate_1st_derivatives(k, self.other_surface, uv2, Side::Left, Side::Left)?;

        // pick the plane of this evaluation
        let temp_plane;
        let plane: &Plane = if self.steepest_descent {
            // plane = plane(surf1Point, surf2Point)
            let through = (s_f + s_g) / 2.0;
            let mut norm1 = duf.cross(dvf);
            let mut norm2 = dug.cross(dvg);
            if norm1.length_squared() < EFF_ZERO {
                norm1 = self
                    .surface
                    .evaluate_normal(k, uv, Side::Left, Side::Left)?;
            }
            if norm2.length_squared() < EFF_ZERO {
                norm2 = self
                    .other_surface
                    .evaluate_normal(k, uv2, Side::Left, Side::Left)?;
            }
            let plane_norm = norm1.cross(norm2);
            if plane_norm.length_squared() < EFF_ZERO_SQ {
                return Err(SolidError::sm_err(OP_SSP_INTERSECT));
            }
            temp_plane = Plane::from_normal(through, plane_norm.unitize_or_z());
            &temp_plane
        } else {
            self.plane
                .ok_or_else(|| SolidError::null_pointer(OP_SSP_INTERSECT))?
        };

        // evaluate Plane derivatives
        let pe = plane.evaluate(Point2::new(0.0, 0.0), 1, 1, true)?;
        let h = nth(
            pe.first().map_or(&[][..], Vec::as_slice),
            0,
            OP_SSP_INTERSECT,
        )?;
        let duh = nth(
            pe.get(1).map_or(&[][..], Vec::as_slice),
            0,
            OP_SSP_INTERSECT,
        )?;
        let dvh = nth(
            pe.first().map_or(&[][..], Vec::as_slice),
            1,
            OP_SSP_INTERSECT,
        )?;

        // Utilize plane equation aX + bY + cZ + d = 0 in solver
        let abc = duh.cross(dvh).unitize()?;
        let d = -abc.dot(h);

        // if we have a zero length tangent then we must fail
        if duf.length_squared() < EFF_ZERO_SQ
            || dvf.length_squared() < EFF_ZERO_SQ
            || dug.length_squared() < EFF_ZERO_SQ
            || dvg.length_squared() < EFF_ZERO_SQ
        {
            return Err(SolidError::sm_err(OP_SSP_INTERSECT));
        }

        // get gap vector
        let diff: Vec3 = s_f - s_g;

        // evaluate F[3]
        let f3 = abc.dot(s_f) + d + abc.dot(s_g) + d;
        if let Some(slot) = f.get_mut(3) {
            *slot = f3;
        }

        // exit criteria - Surface points on plane and close to each other
        if diff.length_squared() < EFF_ZERO_SQ && f3 < EFF_ZERO {
            return Ok(true);
        }

        if let Some(jac) = jacobian {
            jac.set_at(0, 0, duf.x)?;
            jac.set_at(0, 1, dvf.x)?;
            jac.set_at(0, 2, -dug.x)?;
            jac.set_at(0, 3, -dvg.x)?;

            jac.set_at(1, 0, duf.y)?;
            jac.set_at(1, 1, dvf.y)?;
            jac.set_at(1, 2, -dug.y)?;
            jac.set_at(1, 3, -dvg.y)?;

            jac.set_at(2, 0, duf.z)?;
            jac.set_at(2, 1, dvf.z)?;
            jac.set_at(2, 2, -dug.z)?;
            jac.set_at(2, 3, -dvg.z)?;

            jac.set_at(3, 0, abc.dot(duf))?;
            jac.set_at(3, 1, abc.dot(dvf))?;
            jac.set_at(3, 2, abc.dot(dug))?;
            jac.set_at(3, 3, abc.dot(dvg))?;
        }

        // Compute function values (F[3] above)
        for (slot, value) in f.iter_mut().zip([diff.x, diff.y, diff.z]) {
            *slot = value;
        }

        Ok(false)
    }
}

impl EvalNFunctions for SsiNormalizeEnfo<'_> {
    /// `Evaluate`: never reports an answer found; the Jacobian is the finite
    /// difference one (upstream replaces its analytic one).
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let k = self.kernel;
        check_len(x, f, 4, OP_SSI_NORMALIZE)?;
        let (Some(&u1), Some(&v1), Some(&u2), Some(&v2)) =
            (x.first(), x.get(1), x.get(2), x.get(3))
        else {
            return Err(SolidError::sm_err(OP_SSI_NORMALIZE));
        };

        let uv = Point2::new(u1, v1);
        let uv2 = Point2::new(u2, v2);
        let plane_norm = self.plane_normal;

        // get surface 3d points - with 2nd derivatives
        let d1 = evaluate_2nd_derivatives(k, self.surface, uv, Side::Left, Side::Left)?;
        let d2 = evaluate_2nd_derivatives(k, self.other_surface, uv2, Side::Left, Side::Left)?;
        let mut srf_point = d1.point;
        let srf_point2 = d2.point;
        let (du, dv) = (d1.du, d1.dv);
        let (du2, dv2) = (d2.du, d2.dv);

        // if we have a zero length tangent then we must fail
        if du.length_squared() < EFF_ZERO_SQ
            || dv.length_squared() < EFF_ZERO_SQ
            || du2.length_squared() < EFF_ZERO_SQ
            || dv2.length_squared() < EFF_ZERO_SQ
        {
            return Err(SolidError::sm_err(OP_SSI_NORMALIZE));
        }

        // when the gap is small
        let mut gap: Vec3 = srf_point2 - srf_point;
        let tol = EFF_ZERO * 1000.0 * (1.0 + srf_point.max_dimension());
        if gap.length_squared() < tol {
            // translate one surface point by its normal and solve a little better
            let norm = self
                .surface
                .evaluate_normal(k, uv, Side::Left, Side::Left)?;
            let norm2 = self
                .other_surface
                .evaluate_normal(k, uv2, Side::Left, Side::Left)?;
            let ave_norm = if norm.dot(norm2) < 0.0 {
                (norm - norm2) / 2.0
            } else {
                (norm + norm2) / 2.0
            };
            // project AveNorm to Plane
            let move_in_plane = plane_norm.cross(ave_norm).cross(plane_norm);
            if norm.dot(gap) > 0.0 {
                srf_point -= move_in_plane;
                gap = srf_point2 - srf_point;
            } else {
                srf_point += move_in_plane;
                gap = srf_point2 - srf_point;
            }
        }

        // get surface normals
        let n = du.cross(dv);
        let n2 = du2.cross(dv2);

        // unitize surf1 Normal
        let mut nu = n;
        if nu.length_squared() < EFF_ZERO_SQ {
            nu = self
                .surface
                .evaluate_normal(k, uv, Side::Left, Side::Left)?;
            if nu.length_squared() < EFF_ZERO_SQ {
                return Err(SolidError::sm_err(OP_SSI_NORMALIZE));
            }
        }
        let nu = nu.unitize_or_z();

        // unitize surf2 Normal (upstream evaluates it on the first surface)
        let mut n2u = n2;
        if n2u.length_squared() < EFF_ZERO_SQ {
            n2u = self
                .surface
                .evaluate_normal(k, uv2, Side::Left, Side::Left)?;
            if n2u.length_squared() < EFF_ZERO_SQ {
                return Err(SolidError::sm_err(OP_SSI_NORMALIZE));
            }
        }
        let n2u = n2u.unitize_or_z();

        // Get two plane vectors equal to surf/plane intersection directions
        let v_1 = nu.cross(plane_norm);
        let v_2 = n2u.cross(plane_norm);

        // Compute function values
        set_all(
            f,
            &[
                v_1.dot(gap),
                v_2.dot(gap),
                srf_point.dot(plane_norm) + self.plane_d,
                srf_point2.dot(plane_norm) + self.plane_d,
            ],
        );

        if let Some(jac) = jacobian {
            // The analytic Jacobian upstream builds is overwritten by the finite
            // difference one over the natural domains of both surfaces.
            let dom = self.surface.natural_uv_domain_with_nurb(k)?;
            let dom2 = self.other_surface.natural_uv_domain_with_nurb(k)?;
            let domains = [
                dom.u_interval(),
                dom.v_interval(),
                dom2.u_interval(),
                dom2.v_interval(),
            ];
            let mut tmp_x = x.to_vec();
            let f_now = f.to_vec();
            compute_jacobian(self, &mut tmp_x, &f_now, jac, Some(&domains))?;
        }

        Ok(false)
    }
}

impl EvalNFunctions for SsNormalizeEnfo<'_> {
    /// `Evaluate`: never reports an answer found.
    fn evaluate(
        &mut self,
        x: &[f64],
        f: &mut [f64],
        mut jacobian: Option<&mut Matrix>,
    ) -> Result<bool> {
        let k = self.kernel;
        let directed = self.solver_operation == SolverOperation::Signed3dDirectedMinimize;
        let count = if directed { 5 } else { 4 };
        check_len(x, f, count, OP_SS_NORMALIZE)?;
        let (Some(&u1), Some(&v1), Some(&u2), Some(&v2)) =
            (x.first(), x.get(1), x.get(2), x.get(3))
        else {
            return Err(SolidError::sm_err(OP_SS_NORMALIZE));
        };

        // Surf1Point and Surf2Point current solution values
        let uv1 = Point2::new(u1, v1);
        let uv2 = Point2::new(u2, v2);
        let d1 = evaluate_2nd_derivatives(k, self.surface, uv1, Side::Left, Side::Left)?;
        let d2 = evaluate_2nd_derivatives(k, self.other_surface, uv2, Side::Left, Side::Left)?;
        let (p1, s1u, s1v, s1uu, s1uv, s1vv) = (d1.point, d1.du, d1.dv, d1.duu, d1.duv, d1.dvv);
        let (p2, s2u, s2v, s2uu, s2uv, s2vv) = (d2.point, d2.du, d2.dv, d2.duu, d2.duv, d2.dvv);

        // if we have a zero length tangent then we must fail
        if s1u.length_squared() < EFF_ZERO_SQ
            || s1v.length_squared() < EFF_ZERO_SQ
            || s2u.length_squared() < EFF_ZERO_SQ
            || s2v.length_squared() < EFF_ZERO_SQ
        {
            return Err(SolidError::sm_err(OP_SS_NORMALIZE));
        }

        // when the gap is small
        let mut gap: Vec3 = p2 - p1;
        let tol = EFF_ZERO * 1000.0 * (1.0 + p1.max_dimension());
        if gap.length_squared() < tol {
            // translate one surface point by its normal and solve a little better
            let norm = if directed {
                nth(self.opt_vectors.unwrap_or(&[]), 0, OP_SS_NORMALIZE)?
            } else {
                self.surface
                    .evaluate_normal(k, uv1, Side::Left, Side::Left)?
            };
            gap = if norm.dot(gap) > 0.0 {
                p2 - (p1 - norm)
            } else {
                p2 - (p1 + norm)
            };
        }

        // Unitize S1u and S1v and get their derivatives.
        let s1u_unit = s1u.unitize_or_z();
        let s1u_unit_u = s1u.unitized_derivative(s1uu);
        let s1u_unit_v = s1u.unitized_derivative(s1uv);
        let s1v_unit = s1v.unitize_or_z();
        let s1v_unit_u = s1v.unitized_derivative(s1uv); // not the same as s1u_unit_v
        let s1v_unit_v = s1v.unitized_derivative(s1vv);

        if directed {
            if self.opt_vectors.unwrap_or(&[]).is_empty() {
                return Err(SolidError::null_pointer(OP_SS_NORMALIZE));
            }
            let dir = nth(self.opt_vectors.unwrap_or(&[]), 0, OP_SS_NORMALIZE)?;

            // Surf2 unit Normal and Normal derivatives
            let cross2 = s2u.cross(s2v);
            let cross2_u = s2u.derivative_of_cross_product(s2v, s2uu, s2uv);
            let cross2_v = s2u.derivative_of_cross_product(s2v, s2uv, s2vv);

            let n2 = cross2.unitize_or_z();
            let n2_u = cross2.unitized_derivative(cross2_u);
            let n2_v = cross2.unitized_derivative(cross2_v);

            if let Some(jac) = jacobian.as_deref_mut() {
                jac.set_at(0, 0, s1u_unit_u.dot(n2))?;
                jac.set_at(0, 1, s1u_unit_v.dot(n2))?;
                jac.set_at(0, 2, s1u_unit.dot(n2_u))?;
                jac.set_at(0, 3, s1u_unit.dot(n2_v))?;
                jac.set_at(0, 4, 0.0)?;

                jac.set_at(1, 0, s1v_unit_u.dot(n2))?;
                jac.set_at(1, 1, s1v_unit_v.dot(n2))?;
                jac.set_at(1, 2, s1v_unit.dot(n2_u))?;
                jac.set_at(1, 3, s1v_unit.dot(n2_v))?;
                jac.set_at(1, 4, 0.0)?;

                jac.set_at(2, 0, s1u.x)?;
                jac.set_at(2, 1, s1v.x)?;
                jac.set_at(2, 2, -s2u.x)?;
                jac.set_at(2, 3, -s2v.x)?;
                jac.set_at(2, 4, -dir.x)?;

                jac.set_at(3, 0, s1u.y)?;
                jac.set_at(3, 1, s1v.y)?;
                jac.set_at(3, 2, -s2u.y)?;
                jac.set_at(3, 3, -s2v.y)?;
                jac.set_at(3, 4, -dir.y)?;

                jac.set_at(4, 0, s1u.z)?;
                jac.set_at(4, 1, s1v.z)?;
                jac.set_at(4, 2, -s2u.z)?;
                jac.set_at(4, 3, -s2v.z)?;
                jac.set_at(4, 4, -dir.z)?;
            }

            // Compute function values
            let w = x.get(4).copied().unwrap_or(0.0);
            let vec: Vec3 = (p1 - p2) - w * dir;
            set_all(
                f,
                &[s1u_unit.dot(n2), s1v_unit.dot(n2), vec.x, vec.y, vec.z],
            );
        } else {
            // Unitize S2u and S2v and get their derivatives.
            let s2u_unit = s2u.unitize_or_z();
            let s2u_unit_u = s2u.unitized_derivative(s2uu);
            let s2u_unit_v = s2u.unitized_derivative(s2uv);
            let s2v_unit = s2v.unitize_or_z();
            let s2v_unit_u = s2v.unitized_derivative(s2uv); // not the same as s2u_unit_v
            let s2v_unit_v = s2v.unitized_derivative(s2vv);

            if let Some(jac) = jacobian {
                jac.set_at(0, 0, s1u_unit_u.dot(gap) - s1u_unit.dot(s1u))?;
                jac.set_at(0, 1, s1u_unit_v.dot(gap) - s1u_unit.dot(s1v))?;
                jac.set_at(0, 2, s1u_unit.dot(s2u))?;
                jac.set_at(0, 3, s1u_unit.dot(s2v))?;

                jac.set_at(1, 0, s1v_unit_u.dot(gap) - s1v_unit.dot(s1u))?;
                jac.set_at(1, 1, s1v_unit_v.dot(gap) - s1v_unit.dot(s1v))?;
                jac.set_at(1, 2, s1v_unit.dot(s2u))?;
                jac.set_at(1, 3, s1v_unit.dot(s2v))?;

                jac.set_at(2, 0, -s2u_unit.dot(s1u))?;
                jac.set_at(2, 1, -s2u_unit.dot(s1v))?;
                jac.set_at(2, 2, s2u_unit_u.dot(gap) + s2u_unit.dot(s2u))?;
                jac.set_at(2, 3, s2u_unit_v.dot(gap) + s2u_unit.dot(s2v))?;

                jac.set_at(3, 0, -s2v_unit.dot(s1u))?;
                jac.set_at(3, 1, -s2v_unit.dot(s1v))?;
                jac.set_at(3, 2, s2v_unit_u.dot(gap) + s2v_unit.dot(s2v))?;
                jac.set_at(3, 3, s2v_unit_v.dot(gap) + s2v_unit.dot(s2v))?;
            }

            // Compute function values
            set_all(
                f,
                &[
                    s1u_unit.dot(gap),
                    s1v_unit.dot(gap),
                    s2u_unit.dot(gap),
                    s2v_unit.dot(gap),
                ],
            );
        }

        Ok(false)
    }
}

// ---- co-ported callees (unit surface.surface_p3) ------------------------------------------

impl SurfaceEval<'_> {
    /// Evaluates the unit normal into the matrix entry `[2][2]` unless done already
    /// (`EvalNormal`, co-ported from unit `surface.surface_p3`): `Du x Dv`, at a
    /// singular point the mixed partial's cross product, then `EvaluateNormal`.
    ///
    /// A failed evaluation does not stop it: as upstream, which ignores `Eval`'s
    /// status, the normal is made from the matrix the failed evaluation wrote (the
    /// derivatives before a failed zero-tangent correction).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from [`SurfaceEval::eval`]; those of `EvaluateNormal`.
    pub fn eval_normal(&mut self, k: &Kernel) -> Result<()> {
        if *self.state_mut().normal_set {
            return Ok(());
        }
        // upstream ignores the status (`Eval();`) and reads the matrix as it stands
        ignore_status(self.eval())?;

        let uv = self.uv();
        let (u_left, v_left) = self.sides();
        let surface = self.surface();
        let st = self.state_mut();
        let du = st.pvv[1][0];
        let dv = st.pvv[0][1];
        let duv = st.pvv[1][1];

        let normal = du.cross(dv);
        st.pvv[2][2] = normal;
        if normal.length_squared() >= EFF_ZERO_SQ {
            st.pvv[2][2] = normal.unitize_or_z();
            *st.normal_set = true;
            return Ok(());
        }

        // Some kind of singularity.  Assume it's a zero derivative (as opposed to
        // parallel derivatives).  The direction of the mixed partial describes how the
        // degenerate derivative is changing.
        let sing_u = dv.length_squared() >= du.length_squared();

        // On the high side of the 'good' domain, this must be flipped (because it's
        // decreasing).
        let surface = surface.ok_or(SolidError::null_pointer(OP_EVAL_NORMAL))?;
        let domain = surface.natural_uv_domain_with_nurb(k)?;
        if sing_u {
            let mut n = duv.cross(dv);
            if uv.x > domain.u_interval().mid() {
                n = -n;
            }
            st.pvv[2][2] = n;
        } else {
            let mut n = du.cross(duv);
            if uv.y > domain.v_interval().mid() {
                n = -n;
            }
            st.pvv[2][2] = n;
        }
        if st.pvv[2][2].length_squared() >= EFF_ZERO_SQ {
            st.pvv[2][2] = st.pvv[2][2].unitize_or_z();
            *st.normal_set = true;
            return Ok(());
        }

        // Try EvaluateNormal().
        let side = |left: bool| if left { Side::Left } else { Side::Right };
        let normal = surface.evaluate_normal(k, uv, side(u_left), side(v_left))?;
        st.pvv[2][2] = normal;
        *st.normal_set = true;
        Ok(())
    }

    /// The unit surface normal at the evaluator's point (`Normal`).
    ///
    /// # Errors
    ///
    /// Those of [`SurfaceEval::eval_normal`] (upstream's assertion on a failed
    /// evaluation, then the stale value).
    pub fn normal(&mut self, k: &Kernel) -> Result<Vec3> {
        self.eval_normal(k)?;
        Ok(self.state_mut().pvv[2][2])
    }

    /// Evaluates the surface tangent and curvature in the evaluator's section
    /// direction unless done already (`EvalNormalSection`); they are readable with
    /// [`SurfaceEval::stored_section`].
    ///
    /// # Errors
    ///
    /// Those of [`evaluate_normal_section`] (`surface.surface_p1`); [`SolidError::Unported`] from the
    /// evaluations.
    pub fn eval_normal_section(&mut self, k: &Kernel) -> Result<()> {
        if *self.state_mut().sect_dir_set {
            return Ok(());
        }

        // upstream ignores the statuses of both evaluations
        ignore_status(self.eval())?;
        ignore_status(self.eval_normal(k))?;

        let dir = self.sect_dir();
        let st = self.state_mut();
        match evaluate_normal_section(st.pvv, dir) {
            Ok((tangent, curvature)) => {
                *st.sect_tangent = tangent;
                *st.sect_curvature = curvature;
                *st.sect_dir_set = true;
                Ok(())
            }
            Err(e) => {
                // the outputs are zeroed before the evaluation can fail
                *st.sect_tangent = Vec3::ZERO;
                *st.sect_curvature = 0.0;
                Err(e)
            }
        }
    }
}

/// Why a test point is not on a surface normal ([`PointOnNormal::reason`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalTestReason {
    /// `SM_ERR_OUTSIDE_OF_DOMAIN`: the UV point is outside the domain, or on its
    /// boundary with the drop vector heading out of it.
    OutsideOfDomain,
    /// `SM_ERR_BAD_SURFACE_POINT`: the surface evaluation failed.
    BadSurfacePoint,
    /// `SM_ERR_NOT_WITHIN_TOLERANCE`: the test ran and the point is not on the normal.
    NotWithinTolerance,
    /// `SM_ERR_UNKNOWN`: the test ran and the point is on the normal.
    Unknown,
}

/// The result of [`is_point_on_normal`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointOnNormal {
    /// `rbFromLeftU`: the surface was evaluated from the left in `u`.
    pub from_left_u: bool,
    /// `rbFromLeftV`: the surface was evaluated from the left in `v`.
    pub from_left_v: bool,
    /// `rbGoodPoint`: the test point is within tolerance of the surface normal line.
    pub good_point: bool,
    /// `reReasonIsBad`.
    pub reason: NormalTestReason,
    /// `rdDropToSurf`: the distance to the surface for a good point, else `0`.
    pub drop_to_surf: f64,
    /// `rdApproxDev`: the deviation of the point from the normal line (`0` when the
    /// surface could not be evaluated, where upstream leaves it unset).
    pub approx_dev: f64,
}

/// Whether `point_to_test` is within `approx_tol` of the normal line of the surface at
/// `uv_to_test` (`smsurf_IsPointOnNormal`, co-ported from unit `surface.surface_p3`).
/// `uv_domain` clamps the UV point; `first_deriv` picks the side of a boundary.
///
/// # Errors
///
/// [`SolidError::Unported`] from the surface evaluations (upstream returns success for
/// every other failure, reporting it in [`PointOnNormal::reason`]).
pub fn is_point_on_normal(
    k: &Kernel,
    surface: &Surface,
    uv_domain: Option<&Extent2d>,
    uv_to_test: Point2,
    point_to_test: Point3,
    first_deriv: Option<Vec3>,
    approx_tol: f64,
) -> Result<PointOnNormal> {
    let side = |left: bool| if left { Side::Left } else { Side::Right };
    let mut out = PointOnNormal {
        from_left_u: true,
        from_left_v: true,
        good_point: false,
        reason: NormalTestReason::Unknown,
        drop_to_surf: 0.0,
        approx_dev: 0.0,
    };

    let natural = surface.natural_uv_domain_with_nurb(k)?;
    let uv_domain_used = uv_domain.copied().unwrap_or(natural);

    // Clamp input uv if indicated.
    let clamped = uv_domain.map_or(uv_to_test, |d| d.clamp_point(uv_to_test));

    // evaluate surface point
    let mut eval = ignored(evaluate_1st_derivatives(
        k,
        surface,
        clamped,
        Side::Left,
        Side::Left,
    ))?;
    let (_, du, dv) = eval.unwrap_or((Vec3::UNDEF, Vec3::UNDEF, Vec3::UNDEF));
    out.from_left_u = first_deriv.is_none_or(|d| du.dot(d) >= 0.0);
    out.from_left_v = first_deriv.is_none_or(|d| dv.dot(d) >= 0.0);
    if !out.from_left_u || !out.from_left_v {
        eval = ignored(evaluate_1st_derivatives(
            k,
            surface,
            clamped,
            side(out.from_left_u),
            side(out.from_left_v),
        ))?;
    }
    let Some((surf_pnt, du, dv)) = eval else {
        out.reason = NormalTestReason::BadSurfacePoint; // most likely reason
        return Ok(out);
    };

    // try quick surfNorm calc, if not call full SurfNorm evaluator
    let mut norm = du.cross(dv);
    if norm.length_squared() < EFF_ZERO_SQ {
        let Some(n) = ignored(surface.evaluate_normal(
            k,
            clamped,
            side(out.from_left_u),
            side(out.from_left_v),
        ))?
        else {
            out.reason = NormalTestReason::BadSurfacePoint;
            return Ok(out);
        };
        norm = n;
    } else {
        norm = norm.unitize_or_z();
    }

    // TestPoint to SurfNorm Line distance; line_t is the distance to the surface since
    // the normal is unit
    let (dev, line_t) = line_point_distance(surf_pnt, norm, point_to_test);
    out.approx_dev = dev;
    let line_pnt = surf_pnt + line_t * norm;
    let diff: Vec3 = line_pnt - point_to_test;

    // any point within an approximately sized cone/cylinder envelope is good
    let mut dist_to_surf = line_t.abs();
    if dist_to_surf < 1.0 {
        dist_to_surf = 1.0;
    }

    if diff.length() < approx_tol * dist_to_surf {
        out.good_point = true;
        out.drop_to_surf = line_t.abs();
    } else {
        // point is too far from surfNorm line
        out.good_point = false;
        out.reason = NormalTestReason::NotWithinTolerance;
        out.drop_to_surf = 0.0;

        // remember when UVTestPt is outside the given UVDomain
        if let Some(d) = uv_domain {
            if !d.contains_point(uv_to_test, 0.0) {
                out.reason = NormalTestReason::OutsideOfDomain;
            }
        } else {
            // remember when 3DTestPt is dropping outside the domain of the surface
            let (on_boundary, bi_norm) = uv_domain_used.is_point_on_boundary(uv_to_test, EFF_ZERO);
            if on_boundary {
                // Get Surface 3d outside pointing vector
                let outward = -bi_norm;
                let srf_pd = ignored(evaluate_directional_derivs(
                    k,
                    surface,
                    uv_to_test,
                    outward,
                    1,
                    Side::Left,
                    Side::Left,
                ))?;
                if let Some(pd) = srf_pd {
                    if let (Some(&p0), Some(&p1)) = (pd.first(), pd.get(1)) {
                        // the drop vector from SrfPoint3d to 3dTestPoint
                        let drop_vec: Vec3 = point_to_test - p0;
                        // when UVPoint is on boundary and the drop vector is heading
                        // beyond the surface
                        if drop_vec.dot(p1) > EFF_ZERO {
                            out.reason = NormalTestReason::OutsideOfDomain;
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

// ---- callees in units not ported yet -------------------------------------------------------

// ---- the unit's functions ------------------------------------------------------------------

/// Whether `solved_point` is on the normal line of the surface at `uv`
/// (`sm_PassSPConvergenceTest`).
///
/// # Errors
///
/// [`SolidError::Unported`] from the surface evaluations.
pub fn pass_sp_convergence_test(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    solved_point: Point3,
    tol_3d: f64,
) -> Result<bool> {
    // test crSolvedPoint/SurfaceNormal(crUV) dist against d3dTol
    let r = is_point_on_normal(k, surface, None, uv, solved_point, None, tol_3d)?;
    Ok(r.good_point)
}

/// The result of [`Surface::is_on_seam`].
#[derive(Debug, Clone, PartialEq)]
pub struct SeamInfo {
    /// The return value: the point is on a seam.
    pub on_seam: bool,
    /// `eOptSeamDir`: the direction of the seam picked by the point (`U`, `V`, `Both`
    /// or `Neither`).
    pub seam_dir: SurfParamType,
    /// `pOptCrossSeamUVs`: the UV points that map to the same 3d point.
    pub cross_seam_uvs: Vec<Point2>,
}

impl Surface {
    /// A 3d curve on the surface along its seam in direction `param` (`CreateSeamCurve`):
    /// `u` constant for [`SurfParamType::U`], else `v` constant; `None` when the surface
    /// is not closed in that direction. `known_closed` skips the closure test.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::is_closed`], the surface's [`Surface::copy`] and the curve
    /// construction, including its documented geometry-nesting bound.
    pub fn create_seam_curve(
        &self,
        kernel: &Kernel,
        param: SurfParamType,
        known_closed: bool,
    ) -> Result<Option<CrvOnSurf>> {
        // locals
        let uv_domain = self.natural_uv_domain_with_nurb(kernel)?;
        let uv_min = uv_domain.min();
        let uv_max = uv_domain.max();
        let is_closed = known_closed || self.is_closed(kernel, &uv_domain, param, None, false)?.0;

        // When Surface closed
        if !is_closed {
            return Ok(None);
        }
        let (start, end) = if param == SurfParamType::U {
            // Build SeamConstU curve
            (
                Point3::new(uv_min.x, uv_min.y, 0.0),
                Point3::new(uv_min.x, uv_max.y, 0.0),
            )
        } else {
            // Build SeamConstV curve
            (
                Point3::new(uv_min.x, uv_min.y, 0.0),
                Point3::new(uv_max.x, uv_min.y, 0.0),
            )
        };
        let line = Line::from_points(start, end, 2); // Line Ivl:[0, 1]
        let seam = CrvOnSurf::new(
            kernel,
            Curve::from(line),
            self.clone_for_evaluation()?,
            Some(uv_domain),
            2,
        )?;
        Ok(Some(seam))
    }

    /// Whether `uv` is on a seam of the surface (`IsOnSeam`): on a domain boundary
    /// across which the surface is closed, with the seam direction and the UV points
    /// across the seam that map to the same 3d point. `opt_tol_3d` defaults to the
    /// surface's zone tolerance.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: the zone tolerance of a face's surface; those of
    /// [`Surface::is_on_boundary`], the tolerances and [`Surface::is_closed`].
    pub fn is_on_seam(&self, k: &Kernel, uv: Point2, opt_tol_3d: Option<f64>) -> Result<SeamInfo> {
        // locals
        let domain = self.natural_uv_domain_with_nurb(k)?;

        // init output
        let mut info = SeamInfo {
            on_seam: false,
            seam_dir: SurfParamType::Neither,
            cross_seam_uvs: Vec::new(),
        };

        // when point is not on boundary
        let (on_boundary, point_param, other_u, other_v) =
            self.is_on_boundary(k, &domain, uv, opt_tol_3d)?;
        let (other_u, other_v) = (
            other_u.unwrap_or(UNDEF_DOUBLE),
            other_v.unwrap_or(UNDEF_DOUBLE),
        );
        if !on_boundary {
            // point can't be on a seam
            return Ok(info);
        }

        // pick a tolerance
        let tol_3d = match opt_tol_3d {
            Some(t) => t,
            None => zone_tol_3d(k, self)?,
        };

        let tol_2d_u = map_to_2d_dir(k, tol_3d, uv, Vec2::new(1.0, 0.0), self)?;
        let tol_2d_v = map_to_2d_dir(k, tol_3d, uv, Vec2::new(0.0, 1.0), self)?;

        // see if surface is closed across point's boundary
        let mut closed_u = false;
        let mut closed_v = false;
        if point_param == SurfParamType::Both {
            closed_u = self
                .is_closed(k, &domain, SurfParamType::U, Some(tol_2d_u), false)?
                .0;
            closed_v = self
                .is_closed(k, &domain, SurfParamType::V, Some(tol_2d_v), false)?
                .0;
        } else if point_param == SurfParamType::U {
            closed_u = self
                .is_closed(k, &domain, SurfParamType::U, Some(tol_2d_u), false)?
                .0;
        } else {
            closed_v = self
                .is_closed(k, &domain, SurfParamType::V, Some(tol_2d_v), false)?
                .0;
        }

        // optional output
        info.seam_dir = if closed_u && closed_v {
            SurfParamType::Both
        } else if closed_u {
            SurfParamType::U
        } else if closed_v {
            SurfParamType::V
        } else {
            SurfParamType::Neither
        };
        let u_side = point_param == SurfParamType::Both || point_param == SurfParamType::U;
        let v_side = point_param == SurfParamType::Both || point_param == SurfParamType::V;
        if u_side && closed_u {
            info.cross_seam_uvs.push(Point2::new(other_u, uv.y));
        }
        if v_side && closed_v {
            info.cross_seam_uvs.push(Point2::new(uv.x, other_v));
        }
        if point_param == SurfParamType::Both && closed_u && closed_v {
            info.cross_seam_uvs.push(Point2::new(other_u, other_v));
        }

        info.on_seam = if point_param == SurfParamType::Both {
            closed_u || closed_v
        } else if point_param == SurfParamType::U {
            closed_u
        } else {
            closed_v
        };
        Ok(info)
    }

    /// The extrema of the distance from `test_point` to the surface over `uv_domain`, as
    /// UV parameter values (`GlobalPointSolve`): a fast drop for planes (intersect and
    /// minimize), else the global solvers. `surface` is this surface's handle, which the
    /// original solver (`GlobalPointSolve_0`, maximize) names in its solutions and
    /// needs for its cache; `None` is for callers without one: maximize then solves
    /// with a stand-in handle and the solutions' objects are null (upstream: `this`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: the trim containment of a face's surface,
    /// `GlobalPointSolve_1` (`surface.surface_point_solve_p3`); those of
    /// [`Surface::global_point_solve_0`] for maximize.
    pub fn global_point_solve<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        uv_domain: &Extent2d,
        op: SolverOperation,
        test_point: Point3,
        src_zone_tol_3d: f64,
        opt_target_distance: Option<f64>,
        requested: SolutionRequested,
    ) -> Result<SolutionArray<O>> {
        let mut solutions = SolutionArray::new();
        self.global_point_solve_into(
            k,
            surface,
            uv_domain,
            op,
            test_point,
            src_zone_tol_3d,
            opt_target_distance,
            requested,
            &mut solutions,
        )?;
        Ok(solutions)
    }

    /// [`Surface::global_point_solve`] by a caller that holds the surface's cache with
    /// its point testing switched off (`SmTemporaryChangeValue` on
    /// `m_bPointTestEnabled` around the solve, as `SmFace::Point3DClassify` does): the
    /// trim boundary check of a face's surface keeps every solution, so the cache is
    /// not fetched for it.
    ///
    /// # Errors
    ///
    /// As [`Surface::global_point_solve`].
    pub fn global_point_solve_point_test_off<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        uv_domain: &Extent2d,
        op: SolverOperation,
        test_point: Point3,
        src_zone_tol_3d: f64,
        opt_target_distance: Option<f64>,
        requested: SolutionRequested,
    ) -> Result<SolutionArray<O>> {
        let mut solutions = SolutionArray::new();
        self.global_point_solve_in(
            k,
            surface,
            None,
            None,
            uv_domain,
            op,
            test_point,
            src_zone_tol_3d,
            opt_target_distance,
            requested,
            &mut solutions,
            None,
            true,
        )?;
        Ok(solutions)
    }

    /// [`Surface::global_point_solve_point_test_off`] with upstream's out-parameter, as
    /// [`Surface::global_point_solve_into`]: `solutions` keeps what was found when the
    /// call fails (for callers that ignore the status).
    ///
    /// # Errors
    ///
    /// As [`Surface::global_point_solve`].
    pub(crate) fn global_point_solve_point_test_off_into<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        uv_domain: &Extent2d,
        op: SolverOperation,
        test_point: Point3,
        src_zone_tol_3d: f64,
        opt_target_distance: Option<f64>,
        requested: SolutionRequested,
        solutions: &mut SolutionArray<O>,
    ) -> Result<()> {
        self.global_point_solve_in(
            k,
            surface,
            None,
            None,
            uv_domain,
            op,
            test_point,
            src_zone_tol_3d,
            opt_target_distance,
            requested,
            solutions,
            None,
            true,
        )
    }

    /// [`Surface::global_point_solve`] with upstream's out-parameter: `solutions` keeps
    /// what was found when the call fails (for callers that ignore the status), except
    /// what the fast drop produced before it failed itself.
    ///
    /// # Errors
    ///
    /// As [`Surface::global_point_solve`].
    pub fn global_point_solve_into<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        uv_domain: &Extent2d,
        op: SolverOperation,
        test_point: Point3,
        src_zone_tol_3d: f64,
        opt_target_distance: Option<f64>,
        requested: SolutionRequested,
        solutions: &mut SolutionArray<O>,
    ) -> Result<()> {
        self.global_point_solve_in(
            k,
            surface,
            None,
            None,
            uv_domain,
            op,
            test_point,
            src_zone_tol_3d,
            opt_target_distance,
            requested,
            solutions,
            None,
            false,
        )
    }

    /// [`Surface::global_point_solve_into`] with lazy owning-face topology for
    /// the fast-drop trim test. The context is consulted at the original cache
    /// lookup, not before the analytic drop or UV-domain rejection.
    ///
    /// # Errors
    /// Those of the solve, cache preparation and face classification.
    #[allow(clippy::too_many_arguments)]
    pub fn global_point_solve_into_with_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        cache: Option<&crate::surface::surface_cache::SurfaceCache<O>>,
        context: &mut dyn super::p1::SurfaceSolveContext<O>,
        uv_domain: &Extent2d,
        op: SolverOperation,
        test_point: Point3,
        src_zone_tol_3d: f64,
        opt_target_distance: Option<f64>,
        requested: SolutionRequested,
        solutions: &mut SolutionArray<O>,
    ) -> Result<()> {
        self.global_point_solve_in(
            k,
            surface,
            cache,
            Some(context),
            uv_domain,
            op,
            test_point,
            src_zone_tol_3d,
            opt_target_distance,
            requested,
            solutions,
            None,
            false,
        )
    }

    /// [`Surface::global_point_solve_into_with_context`] with the surface cache's point
    /// testing off for the call (`m_bPointTestEnabled` FALSE, as `SmSurface::DropCurve`
    /// sets it for its whole call): the fast drop keeps every solution, the face's trim
    /// boundaries are not tested.
    ///
    /// # Errors
    /// As [`Surface::global_point_solve_into_with_context`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn global_point_solve_into_with_context_point_test_off<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        cache: Option<&crate::surface::surface_cache::SurfaceCache<O>>,
        context: &mut dyn super::p1::SurfaceSolveContext<O>,
        uv_domain: &Extent2d,
        op: SolverOperation,
        test_point: Point3,
        src_zone_tol_3d: f64,
        opt_target_distance: Option<f64>,
        requested: SolutionRequested,
        solutions: &mut SolutionArray<O>,
    ) -> Result<()> {
        self.global_point_solve_in(
            k,
            surface,
            cache,
            Some(context),
            uv_domain,
            op,
            test_point,
            src_zone_tol_3d,
            opt_target_distance,
            requested,
            solutions,
            None,
            true,
        )
    }

    /// [`Surface::global_point_solve`] of a face's surface by a caller that holds the
    /// face and the surface's cache: the trim boundary test of the fast drop
    /// (`pFace->PointClassify(uv, tol, TRUE, FALSE, ..)` of each solution, keeping
    /// face, edge and vertex points) is `classify` (`uv`, `src_zone_tol_3d`), or none
    /// when `classify` is `None` (the cache is not a trimmed one, has point testing off
    /// or its face was modified).
    ///
    /// # Errors
    ///
    /// As [`Surface::global_point_solve`], and those of `classify`.
    #[allow(clippy::too_many_arguments)]
    pub fn global_point_solve_face_tested<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        uv_domain: &Extent2d,
        op: SolverOperation,
        test_point: Point3,
        src_zone_tol_3d: f64,
        opt_target_distance: Option<f64>,
        requested: SolutionRequested,
        classify: FaceTest<'_>,
    ) -> Result<SolutionArray<O>> {
        let mut solutions = SolutionArray::new();
        self.global_point_solve_in(
            k,
            surface,
            None,
            None,
            uv_domain,
            op,
            test_point,
            src_zone_tol_3d,
            opt_target_distance,
            requested,
            &mut solutions,
            Some(classify),
            false,
        )?;
        Ok(solutions)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn global_point_solve_in<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        cache: Option<&crate::surface::surface_cache::SurfaceCache<O>>,
        mut context: Option<&mut dyn super::p1::SurfaceSolveContext<O>>,
        uv_domain: &Extent2d,
        op: SolverOperation,
        test_point: Point3,
        src_zone_tol_3d: f64,
        opt_target_distance: Option<f64>,
        requested: SolutionRequested,
        solutions: &mut SolutionArray<O>,
        face_test: Option<FaceTest<'_>>,
        point_test_off: bool,
    ) -> Result<()> {
        // init output
        *solutions = SolutionArray::new();

        // Try for cheap drop fast solutions for SmPlane, SmSurfOfExtrusion, and
        // SmSurfOfRevolution cases when appropriate
        if op == SolverOperation::Intersect || op == SolverOperation::Minimize {
            // upstream ignores a failing status and tries the solvers
            let fast = ignored(self.drop_point_fast::<O>(
                k,
                uv_domain,
                op,
                test_point,
                None,
                src_zone_tol_3d,
                opt_target_distance,
                requested,
            ))?;

            // when dropPointFast works
            if let Some(fast) = fast.filter(|s| !s.is_empty()) {
                *solutions = fast;

                // Reject results that are not within the given domain.  DropPointFast
                // might not do that.
                let mut i = 0;
                while i < solutions.len() {
                    let uv = solutions.get(i).map_or(Point2::UNDEF, |s| {
                        Point2::new(
                            s.start.parameters.first().copied().unwrap_or(0.0),
                            s.start.parameters.get(1).copied().unwrap_or(0.0),
                        )
                    });
                    if self.is_in_domain(k, uv_domain, uv, Some(src_zone_tol_3d))? {
                        i += 1;
                    } else {
                        solutions.remove_at(i, 1);
                    }
                }

                // If this surface belongs to a face, then check whether the returned UV
                // values are within the trim boundaries of the face, unless flags in its
                // surface cache indicate otherwise.
                if !point_test_off && self.owner().is_some_and(|o| o.type_code == FACE_TYPE) {
                    if let Some(test) = face_test {
                        // the caller holds the face and its surface's cache: it tests
                        // only when that cache is trimmed, point testing is on and its
                        // face was not modified (`None`: no test)
                        if let Some(classify) = test {
                            let mut i = 0;
                            while i < solutions.len() {
                                let point = solutions.get_at(i)?;
                                let uv = Point2::new(point.start.param(0)?, point.start.param(1)?);
                                if classify(k, uv, src_zone_tol_3d)? {
                                    i += 1;
                                } else {
                                    solutions.remove_at(i, 1);
                                }
                            }
                        }
                    } else if let Some(context) = context.as_deref_mut() {
                        let built;
                        let cache = if let Some(cache) = cache {
                            cache
                        } else {
                            built = context.surface_cache_state(k, self, surface)?;
                            &built
                        };
                        if cache.is_trimmed()
                            && cache.point_test_enabled
                            && !cache.face_was_modified
                        {
                            let face = cache.face()?.ok_or(SolidError::null_pointer(
                                "surface::surface::p4::global_point_solve",
                            ))?;
                            let queries = context.face_queries().ok_or_else(|| {
                                SolidError::unported(
                                    "prepared face classifier for fast drop",
                                    "query.face_p2",
                                )
                            })?;
                            let mut i = 0;
                            while i < solutions.len() {
                                let point = solutions.get_at(i)?;
                                let uv = Point2::new(point.start.param(0)?, point.start.param(1)?);
                                // Here upstream removes a candidate on classification
                                // failure (unlike sm_IsPointValid's keep-on-error).
                                let good = super::p1::ignore_native_classification(
                                    queries.face_point_classify_fast_drop(
                                        k,
                                        face,
                                        uv,
                                        src_zone_tol_3d,
                                    ),
                                )?
                                .is_some_and(|(class, _)| matches!(class, 102..=104));
                                if good {
                                    i += 1;
                                } else {
                                    solutions.remove_at(i, 1);
                                }
                            }
                        }
                    } else {
                        // Without a context the surface's own cache is made here. For a
                        // face-owned surface its trimmed cache needs the face's
                        // topology, so `get_surface_cache` already returns `Unported`
                        // when it would build one; a plain cache it falls back to is not
                        // an SmTrimSrfCache (SM_CAST_PTR gives NULL): no test, as
                        // upstream.
                        let cache = get_surface_cache(k, self, None::<NoObject>)?;
                        if cache.is_trimmed()
                            && cache.point_test_enabled
                            && !cache.face_was_modified
                        {
                            // SmFace::PointClassify of a bare face-owned surface: the
                            // classifier is the context's FaceQueries
                            return Err(SolidError::unported(
                                "SmFace::PointClassify",
                                "query.face_p2",
                            ));
                        }
                    }
                }

                if !solutions.is_empty() {
                    return Ok(()); // all done
                }
            }
        }

        // arrive here when DropPointFast wasn't appropriate or didn't work.
        // m_bUseNewPointSolvers is always TRUE; the new solvers are not used for
        // MAXIMIZE, but always for DIRECTED.
        let use_new =
            (op != SolverOperation::Maximize) || op == SolverOperation::Signed3dDirectedMaximize;
        *solutions = if use_new {
            // upstream's seam solutions name this surface, whose natural UV domain
            // IsEqual (AddSortedSolution) reads: solve naming a stand-in for it, and
            // return the solutions without objects, as before
            self.global_point_solve_1(
                k,
                uv_domain,
                op,
                test_point,
                src_zone_tol_3d,
                opt_target_distance,
                requested,
                Some(OperandObject::Surface(0, self.stand_in_key_domain(k)?)),
            )?
            .without_objects()
        } else if let Some(surface) = surface {
            self.global_point_solve_0_in(
                k,
                surface,
                cache,
                context,
                point_test_off,
                uv_domain,
                op,
                test_point,
                src_zone_tol_3d,
                opt_target_distance,
                requested,
            )?
        } else {
            // no handle for the solutions' objects (upstream passes `this`): solve with
            // a stand-in and leave the objects null
            let found = self.global_point_solve_0_in(
                k,
                NoObject,
                None,
                None,
                point_test_off,
                uv_domain,
                op,
                test_point,
                src_zone_tol_3d,
                opt_target_distance,
                requested,
            )?;
            let mut out = SolutionArray::new();
            for s in found.as_slice() {
                out.add(&Solution {
                    solution_type: s.solution_type,
                    num_variables: s.num_variables,
                    start: s.start,
                    end: s.end,
                    num_objects: s.num_objects,
                    objects: [None; MAX_TREES],
                    nodes: s.nodes,
                });
            }
            out
        };
        Ok(())
    }
}

/// The result of [`classify_curve_surface_xsect_point`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XsectClassification {
    /// `rbFoundGuess`: a nearby second intersection was found (only for
    /// [`TsectCurveType::NearTangent`], inside one of the domains).
    pub found_guess: bool,
    /// `rNewUVGuess`: the surface guess point (meaningful with `found_guess`).
    pub new_uv_guess: Point2,
    /// `rdNewTGuess`: the curve guess point (meaningful with `found_guess`).
    pub new_t_guess: f64,
    /// `rePointType`: touching, crossing, tangent, coincident or near tangent.
    pub point_type: TsectCurveType,
}

/// `smos_Sgn`: `+1` for positive or zero values, else `-1`.
fn sgn(a: f64) -> f64 {
    if a >= 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// The line parameter of the intersection farther from the line point, from the one or
/// two line/circle intersection parameters.
fn farther_line_param(params: &[f64]) -> f64 {
    let a = params.first().copied().unwrap_or(0.0);
    let b = params.get(1).copied().unwrap_or(0.0);
    if b.abs() > a.abs() {
        b
    } else {
        a
    }
}

/// Classifies a curve/surface intersection point as touching, crossing, tangent,
/// coincident or near tangent, and for a near tangent one looks for the guess of a nearby
/// second intersection (`sm_ClassifyCurveSurfaceXSectPoint`): the surface's
/// normal-section osculating circle (or the Dupin indicatrix asymptotes when the curve
/// plane is tangent to the surface) is intersected with the curve's osculating circle,
/// taking Meusnier's theorem into account.
///
/// # Errors
///
/// [`SolidError::Unported`]: an edge or face tolerance of an owned curve or surface
/// (`topology.edge`, `topology.face_p2`); those of the evaluations and of the
/// line/circle intersections.
pub fn classify_curve_surface_xsect_point(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    current_uv: Point2,
    current_t: f64,
) -> Result<XsectClassification> {
    classify_curve_surface_xsect_point_with(
        k, surface, uv_domain, curve, interval, current_uv, current_t, None,
    )
}

/// [`classify_curve_surface_xsect_point`] by a caller that resolves the tolerance of
/// the curve's owning edge (`pCurveOwner->GetTolerance()`): `edge_tolerance`, `None`
/// when it cannot.
///
/// # Errors
///
/// As [`classify_curve_surface_xsect_point`]; [`SolidError::Unported`] for an
/// edge's curve only when `edge_tolerance` is `None`.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub fn classify_curve_surface_xsect_point_with(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    current_uv: Point2,
    current_t: f64,
    edge_tolerance: Option<f64>,
) -> Result<XsectClassification> {
    let mut out = XsectClassification {
        found_guess: false,
        new_uv_guess: Point2::UNDEF,
        new_t_guess: UNDEF_DOUBLE,
        point_type: TsectCurveType::Crossing,
    };

    // Get SurfacePoint Evaluation and SurfaceNormal: matrix of evaluation values
    // organized [D Du Duu] [Dv Duv 0] [Dvv 0 0]
    let flags = EvaluateFlags {
        u_side: Side::Left,
        v_side: Side::Left,
        only_upper_half: true,
        non_zero_tangents: false, // exact tangent values
    };
    let m = surface.evaluate(k, current_uv, 2, 2, flags)?;
    let mut pvv = [[Vec3::UNDEF; 3]; 3];
    for (i, row) in pvv.iter_mut().enumerate() {
        for (j, slot) in row.iter_mut().enumerate() {
            if i + j <= 2 {
                *slot =
                    m.get(i).and_then(|r| r.get(j)).copied().ok_or_else(|| {
                        SolidError::invalid_input(OP_CLASSIFY, "short evaluation")
                    })?;
            }
        }
    }

    // get surface Normal
    let mut surface_normal = pvv[1][0].cross(pvv[0][1]);
    if surface_normal.length_squared() < EFF_ZERO_SQ {
        surface_normal = surface.evaluate_normal(k, current_uv, Side::Left, Side::Left)?;
    } else {
        surface_normal = surface_normal.unitize()?;
    }
    pvv[2][2] = surface_normal; // Must be here for smsurf_EvaluateGeometric().

    // Get the geometric properties of the curve at the current intersection:
    // a[0] position, a[1] first derivative, a[2] curvature vector
    let geom = match curve_evaluate_geometric(k, curve, current_t, 2, true) {
        Ok(v) => v,
        Err(SolidError::Upstream { code: SM_ERR, .. }) => {
            // no extra guesses or possible coincidence signalled for degenerate curves
            out.point_type = TsectCurveType::Touching;
            return Ok(out);
        }
        Err(e) => return Err(e),
    };
    let curve_pos = nth(&geom, 0, OP_CLASSIFY)?;
    let curve_tan = nth(&geom, 1, OP_CLASSIFY)?;
    let curve_curv_vec = nth(&geom, 2, OP_CLASSIFY)?;

    // no work - curve is not within 20 degrees of tangency to surface
    if !surface_normal.is_perpendicular_to(curve_tan, 20.0) {
        out.point_type = TsectCurveType::Crossing;
        return Ok(out);
    }

    // Evaluate Surface Position and NormalSection in direction of CurveTangent.  This
    // should give us two circles or a line and a circle to intersect.
    let surface_point = surface.evaluate_point(k, current_uv)?;
    let (surface_section_tangent, surface_section_curvature) =
        evaluate_normal_section(&mut pvv, curve_tan)?;
    let props = evaluate_geometric(&mut pvv)?;
    let (gaussian, normal_curv, k1, k2) = (
        props.gaussian_curvature,
        props.normal_curvature,
        props.principle_curvature1,
        props.principle_curvature2,
    );
    let (pk_vec1, pk_vec2) = (
        props.principle_curvature_vector1,
        props.principle_curvature_vector2,
    );
    let surface_type = if normal_curv.abs() < EFF_ZERO_SQRT
        && k1.abs() < EFF_ZERO_SQRT
        && k2.abs() < EFF_ZERO_SQRT
    {
        LocalSurfaceType::Planar
    } else if gaussian > EFF_ZERO_SQRT {
        LocalSurfaceType::Elliptical
    } else if gaussian < -EFF_ZERO_SQRT {
        LocalSurfaceType::Hyperbolic
    } else {
        LocalSurfaceType::Cylindrical
    };

    // Get Curve Curvature, Normal vector, and Binormal vectors
    let mut curve_normal = curve_curv_vec;
    let curve_curvature = curve_normal.length();
    if curve_curvature.abs() > EFF_ZERO_SQRT {
        curve_normal /= curve_curvature;
    } else {
        // curve is close to being a straight line - let CurveNormal = sSurfaceNormal
        curve_normal = surface_normal;
    }
    let mut curve_binormal = curve_tan.cross(curve_normal);

    // degenerate curve check
    if curve_binormal.length_squared() < EFF_ZERO_SQ {
        out.point_type = TsectCurveType::Touching;
        return Ok(out);
    }
    curve_binormal = curve_binormal.unitize()?;

    // get the cosine of the angle between the surfaceSectionNormal plane and the plane
    // of the curve's osculating circle (needed for Meusnier's Theorem)
    let ang_rad = curve_binormal.angle_between(surface_normal).unwrap_or(0.0);
    let cos_phi = (ang_rad - PI / 2.0).cos().abs();

    // characterize the curve shape as a circle or a line
    let mut curve_line: bool;
    let mut curve_radius = 0.0;
    let mut curve_center = Vec3::ZERO;
    let mut curve_circle_normal = Vec3::ZERO;
    if curve_curvature.abs() > EFF_ZERO_SQRT {
        // let Curve = curve osculating circle
        curve_line = false;
        curve_radius = (1.0 / curve_curvature).abs();
        curve_center = curve_pos + curve_normal * curve_radius;
        curve_circle_normal = curve_binormal;
    } else {
        // curve is a line
        curve_line = true;
    }

    // always load CurvePoint and CurveVec because when circle is large the curve is
    // treated as a line
    let curve_point = curve_pos;
    let curve_vec = curve_tan;

    // characterize the surface section curve or the surface dupin indicatrix shape (when
    // curve plane is tangent to surface) as an osculating circle in the CurvePlane, a
    // line, or two lines
    let mut surface_line: bool;
    let surface_two_lines: bool;
    let mut surface_radius = 0.0;
    let mut surface_center = Vec3::ZERO;
    let mut surface_circle_normal = Vec3::ZERO;
    let mut surface_vec: Vec3;
    let mut surface_vec2 = Vec3::ZERO;

    if cos_phi.abs() < EFF_ZERO_RAD {
        // the curve plane is tangent to the surface plane: it is either coincident,
        // tangent or tangent and needs to be intersected with the asymptotes of the
        // dupin's indicatrix curves.
        match surface_type {
            LocalSurfaceType::Planar => {
                out.point_type = TsectCurveType::Coincident;
                return Ok(out);
            }
            LocalSurfaceType::Elliptical => {
                out.point_type = TsectCurveType::Tangent;
                return Ok(out);
            }
            LocalSurfaceType::Hyperbolic => {
                out.point_type = TsectCurveType::Tangent;
                surface_line = true;
                surface_two_lines = true;
                // Evaluate the hyperbola K1*x^2 + K2*y^2 = 1 at a large Y value and
                // compute the X.  This gives a vector in terms of the principal
                // curvature vectors which defines the asymptotic directions.
                let d_y = 10000.0;
                let mut tmp = (1.0 - k1 * d_y * d_y) / k2;
                if tmp < 0.0 {
                    tmp = 0.0; // caught as elliptical or cylindrical cases, but just in case
                }
                let d_x = sm_sqrt(tmp);
                surface_vec = d_y * pk_vec1 + d_x * pk_vec2;
                surface_vec2 = d_y * pk_vec1 - d_x * pk_vec2;
                surface_vec = surface_vec.unitize_or_z();
                surface_vec2 = surface_vec2.unitize_or_z();
            }
            LocalSurfaceType::Cylindrical => {
                out.point_type = TsectCurveType::Tangent;
                surface_line = true;
                surface_two_lines = false;
                surface_vec = if k1.abs() < EFF_ZERO_SQRT {
                    pk_vec1
                } else {
                    pk_vec2
                };
            }
        }
    } else if surface_section_curvature.abs() < EFF_ZERO_SQRT {
        // curvePlane is not tangent to surface and the surfaceSection is flat
        surface_line = true;
        surface_two_lines = false;
        surface_vec = surface_section_tangent;
    } else {
        // curvePlane is not tangent to surface and surfaceSection is curved
        surface_line = false;
        surface_two_lines = false;
        surface_vec = surface_section_tangent;

        // get surface osculating circle in the curve's plane; Meusnier's theorem picks
        // the correct SurfaceCircleRadius
        surface_radius = (1.0 / surface_section_curvature).abs() * cos_phi;

        // rotate the surface normal into the curve's plane about the tangent vector - by
        // projecting the surface normal onto the curve plane and unitizing.
        let mut circle_center_vec = curve_binormal
            .cross(sgn(surface_section_curvature) * surface_normal)
            .cross(curve_binormal);
        circle_center_vec = circle_center_vec.unitize_or_z();

        // the center lies in direction of the circle center vector, not the surface
        // normal
        surface_center = surface_point + circle_center_vec * surface_radius;
        surface_circle_normal = curve_binormal;
    }

    // We need a tolerance for line and circle intersections.  They're purely geometric,
    // but could well have tolerance issues.  Look to topology first, otherwise use
    // heuristics.
    let mut curve_tol = EFF_ZERO_SQRT; // default values, if nothing else to use.
    let mut surface_tol = EFF_ZERO_SQRT;
    if curve.owner().is_some_and(|o| o.type_code == EDGE_TYPE) {
        // pCurveOwner->GetTolerance(), as the caller resolves it
        curve_tol = edge_tolerance.ok_or(
            // UNPORTED(topology.edge): SmEdge::GetTolerance (a caller without the edge)
            // Proof (unreachable): the plain wrapper passes None, the library callers
            // (surface p9, p10) use classify_curve_surface_xsect_point_with with the
            // caller's edge tolerance; 0 hits in the green difftest sweep.
            SolidError::unported("SmEdge::GetTolerance", "topology.edge"),
        )?;
    } else if !curve_line {
        curve_tol = curve_radius / 1000.0;
    }
    if surface.owner().is_some_and(|o| o.type_code == FACE_TYPE) {
        // pSurfaceOwner->GetTolerance(): the direct owner is the face, so the face the
        // surface reaches; its tolerance as cached by the B-rep
        surface_tol = surface.owner_zone_tol_3d().ok_or(
            // UNPORTED(topology.face_p2): SmFace::GetTolerance (a bare face handle)
            // Proof (unreachable): a B-rep face's surface carries the face tolerance
            // (Brep::sync_face_surface_tolerance invariant, see Surface::face_zone_tol_3d);
            // 0 hits in the green difftest sweep.
            SolidError::unported("SmFace::GetTolerance", "topology.face_p2"),
        )?;
    } else if !surface_line {
        surface_tol = surface_radius / 1000.0;
    }
    let tol_3d = curve_tol + surface_tol;

    // when both curve and surface shapes are lines
    if curve_line && surface_line {
        // classify the point as crossing or coincident
        out.point_type = if (surface_two_lines && curve_vec.is_parallel_to(surface_vec2, 2.0))
            || (surface_line && curve_vec.is_parallel_to(surface_vec, 2.0))
        {
            TsectCurveType::Coincident
        } else {
            TsectCurveType::Crossing
        };
        return Ok(out);
    }

    // when both shapes are curved - see if one should be treated as a line
    if !curve_line && !surface_line {
        // when osculating circle radii vastly differ - treat one shape as a line
        // (Note, 2.0 is not enough. [B30])
        if curve_radius > 1000.0 * surface_radius {
            curve_line = true;
        } else if surface_radius > 1000.0 * curve_radius {
            surface_line = true;
        }
    }

    // look for second intersections
    let mut found_vector = Vec3::ZERO;

    // when curve is linear or close enough
    if curve_line {
        // intersect CurveLine with SurfaceOsculating Circle
        let params = line_coplanar_circle_intersect(
            curve_point,
            curve_vec,
            surface_center,
            surface_circle_normal,
            surface_radius,
            tol_3d,
        )?;

        // If only have one or zero intersections we are done without picking up extra
        // guess or finding coincidence
        match params.len() {
            // If no intersection, it's probably a tolerance thing, and probably tangent.
            0 | 1 => {
                out.point_type = TsectCurveType::Tangent;
                return Ok(out);
            }
            2 => out.point_type = TsectCurveType::NearTangent,
            _ => {}
        }

        // Find the other intersection -- i.e. the one we don't have
        let line_t = farther_line_param(&params);
        found_vector = line_t * curve_vec;
    }

    // when surface is linear
    if surface_line {
        // intersect SurfaceLine with CurveOsculating Circle
        let mut found_vector2 = Vec3::ZERO;
        let mut local_type = TsectCurveType::Unknown;
        let mut local_type2 = TsectCurveType::Unknown;

        let params = line_coplanar_circle_intersect(
            surface_point,
            surface_vec,
            curve_center,
            curve_circle_normal,
            curve_radius,
            tol_3d,
        )?;

        // use number of intersections to classify the point against SurfaceVec
        match params.len() {
            0 | 1 => local_type = TsectCurveType::Tangent,
            2 => {
                local_type = TsectCurveType::NearTangent;
                // Find the other intersection -- i.e. the one we don't have
                let line_t = farther_line_param(&params);
                found_vector = line_t * surface_vec;
            }
            _ => {}
        }

        // when surface is represented by two lines
        if surface_two_lines {
            // intersect SurfaceLine2 with CurveOsculating Circle
            let params2 = line_coplanar_circle_intersect(
                surface_point,
                surface_vec2,
                curve_center,
                curve_circle_normal,
                curve_radius,
                tol_3d,
            )?;

            // use number of intersections to classify the point against SurfaceVec2
            match params2.len() {
                0 | 1 => local_type2 = TsectCurveType::Tangent,
                2 => {
                    local_type2 = TsectCurveType::NearTangent;
                    // Find the other intersection -- i.e. the one we don't have
                    let line_t = farther_line_param(&params2);
                    // (upstream scales SurfaceVec, not SurfaceVec2, here)
                    found_vector2 = line_t * surface_vec;
                }
                _ => {}
            }
        }

        // When there are no near-tangent cases - classify the intersection and exit
        if local_type != TsectCurveType::NearTangent && local_type2 != TsectCurveType::NearTangent {
            out.point_type = if surface_two_lines {
                TsectCurveType::Tangent
            } else {
                local_type
            };
            return Ok(out);
        }

        // arrive here when we have a near-tangent case
        out.point_type = TsectCurveType::NearTangent;

        // when there are two near-tangent cases
        if local_type == TsectCurveType::NearTangent && local_type2 == TsectCurveType::NearTangent {
            // pick the closest 2nd intersection point (maybe we should use both)
            if found_vector2.length_squared() < found_vector.length_squared() {
                found_vector = found_vector2;
            }
        } else if local_type2 == TsectCurveType::NearTangent {
            // when only the second intersection was a near tangent case
            found_vector = found_vector2;
        }
        // else first intersection is a tangent case and sFoundVector is already set
    }

    // when curve and surface are both curved
    if !curve_line && !surface_line {
        // intersect osculating curve and surface circles: BiggerCircle/SmallerCircle
        let hit = if curve_radius > surface_radius {
            circle_coplanar_circle_intersect(
                curve_circle_normal,
                curve_center,
                curve_radius,
                surface_center,
                surface_radius,
                tol_3d,
            )?
        } else {
            circle_coplanar_circle_intersect(
                surface_circle_normal,
                surface_center,
                surface_radius,
                curve_center,
                curve_radius,
                tol_3d,
            )?
        };

        // If only have one or zero intersections we are done without picking up extra
        // guess
        let (p0, p1) = match hit {
            CircleCircle::None | CircleCircle::Tangent(_) => {
                out.point_type = TsectCurveType::Tangent;
                return Ok(out);
            }
            CircleCircle::Coincident => {
                out.point_type = TsectCurveType::Coincident;
                return Ok(out);
            }
            CircleCircle::Crossing(a, b) => (a, b),
        };
        out.point_type = TsectCurveType::NearTangent;

        // else save new intersection guess point
        found_vector = p0 - surface_point;
        found_vector = if found_vector.length() < p1.distance(surface_point) {
            p1 - surface_point
        } else {
            p0 - surface_point
        };
    }

    // Use the found vector to compute an approximate step in the curve and surface
    // parameters.
    let du = pvv[1][0];
    let dv = pvv[0][1];
    out.found_guess = true;

    // account for circular arc distance in curves.
    let arc = if curve_line {
        found_vector.length()
    } else {
        2.0 * curve_radius * asin(found_vector.length() / 2.0 / curve_radius)
    };
    let mut new_t = current_t + sgn(curve_tan.dot(found_vector)) * arc / curve_tan.length();

    // Use the combined 2d shift, instead of two 1-d shifts. [B30]
    let new_uv = match vec_lin_comb_two_vectors(du, dv, found_vector) {
        Ok(delta) => current_uv + delta,
        Err(_) => {
            // Well, ok, if the 2d shift fails, then try two 1-d shifts.
            Point2::new(
                current_uv.x + convert_to_parameter_delta(du, found_vector),
                current_uv.y + convert_to_parameter_delta(dv, found_vector),
            )
        }
    };

    // skip guesses outside of both given domains - keep guesses in at least one domain
    if !interval.contains_value(new_t, 0.0) && !uv_domain.contains_point(new_uv, 0.0) {
        // If the guess is outside of the domain we'll be able to pick it up using
        // another method.
        out.found_guess = false;
        out.new_t_guess = new_t;
        out.new_uv_guess = new_uv;
        return Ok(out);
    }

    // set output
    new_t = interval.clamp_value(new_t);
    out.new_t_guess = new_t;
    out.new_uv_guess = uv_domain.clamp_point(new_uv);
    Ok(out)
}

/// A solution handle naming nothing: the solutions of a global point solve only use
/// their parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoObject;

impl SolutionObject for NoObject {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Surface
    }
}

/// The start ends of the solutions of `GlobalPointSolve(domain, op, point, tol, NULL,
/// requested, ..)`.
///
/// # Errors
///
/// Those of [`Surface::global_point_solve`].
pub fn global_point_solve_starts(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    op: SolverOperation,
    point: Point3,
    zone_tol_3d: f64,
    requested: SolutionRequested,
) -> Result<Vec<SolutionEnd>> {
    let solutions = surface.global_point_solve::<NoObject>(
        k,
        Some(NoObject),
        uv_domain,
        op,
        point,
        zone_tol_3d,
        None,
        requested,
    )?;
    Ok(solutions.as_slice().iter().map(|s| s.start).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::b_spline_surface::LocationType;

    fn plane() -> Surface {
        Surface::from(Plane::from_normal(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ))
    }

    #[test]
    fn plane_step_inversion_uses_the_plane_overrides() {
        // GlobalPointSolve finds the point, then the plane's own (virtual) IsSingularity
        // and IsClosed answer FALSE instead of reaching the unported base bodies
        let s = plane();
        let k = Kernel::default();
        let dom = Extent2d::from_points(Point2::new(-2.0, -2.0), Point2::new(2.0, 2.0));
        let p = s.evaluate_point(&k, Point2::new(0.25, 0.5)).unwrap();
        let (uv, loc) = s.step_inversion(&k, &dom, p, 1e-6, None).unwrap();
        assert!((uv.x - 0.25).abs() < 1e-9 && (uv.y - 0.5).abs() < 1e-9);
        assert_eq!(loc, LocationType::Interior);
    }

    #[test]
    fn plane_normal_through_the_evaluator() {
        let k = &Kernel::default();
        let s = plane();
        let mut e = SurfaceEval::with_uv(k, Some(&s), 0.3, 0.2);
        let n = e.normal(k).unwrap();
        assert!((n.z.abs() - 1.0).abs() < 1e-12 && n.x.abs() < 1e-12);
    }

    #[test]
    fn point_on_the_normal_of_a_plane() {
        let k = &Kernel::default();
        let s = plane();
        let uv = Point2::new(0.3, 0.2);
        let on = s.evaluate_point(k, uv).unwrap();
        let n = s.evaluate_normal(k, uv, Side::Left, Side::Left).unwrap();
        let r = is_point_on_normal(k, &s, None, uv, on + n * 5.0, None, 1e-6).unwrap();
        assert!(r.good_point);
        assert!((r.drop_to_surf - 5.0).abs() < 1e-12);
        let off = is_point_on_normal(
            k,
            &s,
            None,
            uv,
            on + n * 5.0 + Vec3::new(1.0, 0.0, 0.0),
            None,
            1e-6,
        )
        .unwrap();
        assert!(!off.good_point);
        assert_eq!(off.reason, NormalTestReason::NotWithinTolerance);
        assert!(pass_sp_convergence_test(k, &s, uv, on + n, 1e-6).unwrap());
    }

    #[test]
    fn a_line_through_a_plane_crosses_it() {
        let k = &Kernel::default();
        let s = plane();
        let c = Curve::from(Line::from_points(
            Vec3::new(0.1, 0.2, -1.0),
            Vec3::new(0.1, 0.2, 1.0),
            3,
        ));
        let uv = Point2::new(0.1, 0.2);
        let dom = s.natural_uv_domain_if_built();
        let r = classify_curve_surface_xsect_point(
            k,
            &s,
            &dom,
            &c,
            &Extent1d::from_min_max(0.0, 1.0),
            uv,
            0.5,
        )
        .unwrap();
        assert_eq!(r.point_type, TsectCurveType::Crossing);
        assert!(!r.found_guess);
    }

    #[test]
    fn a_plane_has_no_seam() {
        let k = Kernel::new(&crate::kernel::KernelOptions::default());
        assert!(plane()
            .create_seam_curve(&k, SurfParamType::U, false)
            .unwrap()
            .is_none());
    }

    #[test]
    fn normal_section_of_a_plane_is_flat() {
        let k = &Kernel::default();
        let s = plane();
        let flags = EvaluateFlags {
            u_side: Side::Left,
            v_side: Side::Left,
            only_upper_half: true,
            non_zero_tangents: true,
        };
        let m = s.evaluate(k, Point2::new(0.1, 0.1), 2, 2, flags).unwrap();
        let mut pvv = [[Vec3::UNDEF; 3]; 3];
        for (i, row) in pvv.iter_mut().enumerate() {
            for (j, slot) in row.iter_mut().enumerate() {
                if i + j <= 2 {
                    *slot = m[i][j];
                }
            }
        }
        pvv[2][2] = Vec3::new(0.0, 0.0, 1.0);
        let (tangent, curvature) =
            evaluate_normal_section(&mut pvv, Vec3::new(1.0, 1.0, 0.0)).unwrap();
        assert!(curvature.abs() < 1e-12);
        assert!((tangent.length() - 1.0).abs() < 1e-12);
    }
}
