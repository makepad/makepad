//! `SmSurface`, part 5: singularity tests (`IsSingularity`, `EvaluateNormalAtSingularity`),
//! the iso-parametric curve of the base class, the point solves and the point drop
//! (`LocalPointSolve`, `LocalPointSolve_0`, `DropPoint`), the three-surface local
//! intersection from an image-space guess, the coincident-surface cover, the file-static
//! solution filter and surface/point local solve, and the `IdenticalSolutions` overrides
//! of the surface global solvers.
//!
//! Upstream: `source/SMLib/src/SmSurface.cpp`, `source/SMLib/inc/SmSurface.h`.
//!
//! | Rust                                                | Upstream                                      |
//! |-----------------------------------------------------|-----------------------------------------------|
//! | [`SurfaceEval::sect_curvature`]                     | `SmSurfaceEval::SectCurvature`                |
//! | [`Surface::is_singularity`], [`Surface::is_singularity_with_poles`], [`Singularity`] | `IsSingularity` (both overloads; virtual: the `SmPlane` override) |
//! | [`Surface::evaluate_normal_at_singularity`]         | `EvaluateNormalAtSingularity` (virtual: the `SmCurveBoundedSurface` override) |
//! | [`Surface::create_iso_parametric_curve`], [`IsoParametricCurve`] | `CreateIsoParametricCurve` (virtual: `SmBSplineSurface`, `SmPlane`; the `SmSurface` body for the rest) |
//! | [`Surface::local_point_solve`], [`PointSolve`]      | `LocalPointSolve`                             |
//! | [`Surface::local_point_solve_0`], [`PointSolve0`]   | `LocalPointSolve_0`                           |
//! | [`Surface::drop_point`], [`DropPointOptions`], [`DropPoint`] | `DropPoint`                          |
//! | [`Surface::local_surface_surface_intersect_xyz`]    | `LocalSurfaceSurfaceIntersect` (from an image-space guess) |
//! | [`Surface::cover_other_coincident_surface_deferring_trims`], [`CoverOtherCoincident`] | `CoverOtherCoincidentSurface` |
//! | [`Surface::cover_other_coincident_surface_deferring_trims_with_context`], [`CoverSolveContext`] | `CoverOtherCoincidentSurface` (`smsurf_GetSurfaceCache` seeing each face) |
//! | [`sp_local_solve`]                                  | `sm_SpLocalSolve` (static)                    |
//! | [`filter_solution`]                                 | `sm_FilterSolution` (static)                  |
//! | [`SpGlobalSolver`]'s and [`LsiGlobalSolver`]'s `identical_solutions` | `SmSPGlobalSolver::IdenticalSolutions`, `SmLSIGlobalSolver::IdenticalSolutions` |
//! | [`cs_identical_solutions`], [`cs_identical_solutions_in`], [`csi_identical_solutions`], [`csi_identical_solutions_in`] | `SmCSGlobalSolver::IdenticalSolutions`, `SmCSIGlobalSolver::IdenticalSolutions` |
//!
//! Deviations:
//! - **Solver classes without a Rust struct.** `SmCSGlobalSolver` and
//!   `SmCSIGlobalSolver` have no struct yet (their constructors belong to unit
//!   `surface.surface_p1`), so their `IdenticalSolutions` are free functions over the
//!   solver state they read (`SmGlobalSolver` base, surface, tolerance).
//!   `m_d3dTolerance` of the base class is [`GlobalSolver::tolerance_3d`].
//! - **File-static functions** take what their callers resolve: `sm_SpLocalSolve` gets
//!   the surface `sm_GetSurfaceToEvaluate(node, patch surface)` returns (unit
//!   `surface.surface_p1`) instead of the tree node and Bezier patch.
//! - **Iso curves** keep their original concrete surface through `Surface` dispatch
//!   (`curve.iso_curve` extended by `surface.surface_point_solve_p3`, PR #106).
//! - **`m_bUseNewPointSolvers`** has no field: its upstream default (`TRUE`) is used and
//!   the only setter (`SetUseNewPointSolvers`) has no caller. `LocalPointSolve` therefore
//!   runs `LocalPointSolve_0` only for [`SolverOperation::Maximize`].
//! - **Statuses ignored upstream:** [`SolidError::Unported`] still propagates.
//! - **`IsSingularity`** returns an `SmBoolean` and uses `SER`, so a failing evaluation
//!   returns the (non-zero) status, that is `TRUE`, with the direction left at
//!   `SM_SP_NEITHER`; the port does the same ([`Singularity::singular`]).
//! - **Debug-only work** (drawing, dumps, assertions) is not ported.
//! - **Kernel.** `IsSingularity` (through `GetSingularities`' approximation tolerance),
//!   `EvaluateNormalAtSingularity`, `CoverOtherCoincidentSurface`, `LocalPointSolve` and
//!   `DropPoint`, the `IdenticalSolutions` callbacks and the image-space
//!   `LocalSurfaceSurfaceIntersect` retain the caller's [`Kernel`] (#76).
//! - **`CoverOtherCoincidentSurface`** takes both surfaces `&mut`: the covering surface
//!   is expanded in place by `CoverCoincidentSurface`, as upstream. A failing
//!   `CoverCoincidentSurface` leaves `rdMaxDist` at `SM_BIG_DOUBLE` (its first
//!   statement). The `SmFace::RemoveUVTrimCurves` a successful `CoverCoincidentSurface`
//!   runs on a face-owned covering surface is left to the caller holding the B-rep
//!   ([`Surface::cover_other_coincident_surface_deferring_trims`] lists the faces in
//!   [`CoverOtherCoincident::remove_uv_trim_curves_of`], in call order). A caller holding the faces lends them
//!   through a [`CoverSolveContext`], under which each `CoverCoincidentSurface`'s
//!   `STEPInversion` solves see the covering surface's face.
//! - **Unported callees** (return [`SolidError::Unported`]): `SmSurface::GetFace`,
//!   `SmFace::GetTolerance` (for a surface whose B-rep has not cached its face's
//!   tolerance on it, [`Surface::owner_zone_tol_3d`]), `LocalPointSolve_1`, `sm_GetSurfaceToEvaluate`, the STEP
//!   domain conversions and the surface cache.

use crate::curve::curve::p1::ApproximateCurveOptions;
use crate::curve::curve::p2::evaluate_geometric as curve_evaluate_geometric;
use crate::curve::curve::p3::PointSolveOptions;
use crate::curve::curve::Curve;
use crate::curve::iso_curve::IsoCurve;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::{Extent1d, ExtentNd, ExtentPointType};
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::{
    GlobalSolver, PreferredSolution, SolutionPairType, SolutionRequested, SolverOperation,
};
use crate::geom::local_solve_nd::{BoundaryHandler, LocalSolveNd, TerminationReason};
use crate::geom::object::ObjectRef;
use crate::geom::solution_array::{
    OwnDomains, ParameterDomains, Solution, SolutionArray, SolutionObject, SolutionType,
};
use crate::geom::tol::{are_same, ZONE_TOL_3D};
use crate::geom::vector2d::Vec2;
use crate::kernel::Kernel;
use crate::math::{max2, Point2, Point3, Vec3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;

use super::p1::{
    evaluate_1st_derivatives, get_surface_cache, FindPsNormalEnfo, SurfaceSolveContext,
};
use super::p2::{LsiGlobalSolver, SurfaceEval, B_SPLINE_SURFACE_TYPE};
use super::p4::is_point_on_normal;
use super::p4::NoObject;
use super::p7::{good_drop_result, load_guesses, SpGlobalSolver};
use super::{Surface, SurfaceKind};

const OP_SING: &str = "surface::surface::p5::Surface::is_singularity";
const OP_LPS0: &str = "surface::surface::p5::Surface::local_point_solve_0";
const OP_DROP: &str = "surface::surface::p5::Surface::drop_point";
const OP_SP_LOCAL: &str = "surface::surface::p5::sp_local_solve";
const OP_FILTER: &str = "surface::surface::p5::filter_solution";

/// `SM_SS_NONE` (`SmTypes.h`): no singular sides.
pub const SS_NONE: u64 = 0;
/// `SM_SS_UMIN`.
pub const SS_UMIN: u64 = 1;
/// `SM_SS_VMIN`.
pub const SS_VMIN: u64 = 2;
/// `SM_SS_UMAX`.
pub const SS_UMAX: u64 = 4;
/// `SM_SS_VMAX`.
pub const SS_VMAX: u64 = 8;
/// `SM_SS_UNKNOWN`: singularities not yet tested.
pub const SS_UNKNOWN: u64 = 16;

/// `SM_XSECT_TOL_3D` (`2 * SM_ZONE_TOL_3D`).
const XSECT_TOL_3D: f64 = 2.0 * ZONE_TOL_3D;

/// The value `SmContinuityType` takes in upstream's ordered comparisons.
const fn ct(c: ContinuityType) -> i32 {
    c as i32
}

/// The `SmSurface::Evaluate` flags of a full-matrix (upper half) evaluation.
const fn upper_flags(u_side: Side, v_side: Side, non_zero_tangents: bool) -> EvaluateFlags {
    EvaluateFlags {
        u_side,
        v_side,
        only_upper_half: true,
        non_zero_tangents,
    }
}

/// `SmSurfaceEval::SectCurvature` and its companions.
impl SurfaceEval<'_> {
    /// The curvature of the cross section in the evaluator's section direction
    /// (`SectCurvature`): evaluates the section unless done already.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluations (`EvalNormalSection` returns `void`
    /// upstream, so its status is ignored).
    pub fn sect_curvature(&mut self, k: &Kernel) -> Result<f64> {
        crate::error::ignore_status(self.eval_normal_section(k))?;
        Ok(self.stored_section().1)
    }
}

/// An entry of an evaluation result.
fn at(m: &[Vec<Vec3>], i: usize, j: usize, op: &'static str) -> Result<Vec3> {
    m.get(i)
        .and_then(|r| r.get(j))
        .copied()
        .ok_or(SolidError::invalid_input(op, "evaluation result too short"))
}

// ---- IsSingularity ----------------------------------------------------------------------------

/// The result of [`Surface::is_singularity`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Singularity {
    /// The `SmBoolean` return value. Upstream returns the status of a failing evaluation
    /// as is, so a failed evaluation reads `true` with `direction`
    /// [`SurfParamType::Neither`].
    pub singular: bool,
    /// `reSingularDirection`: [`SurfParamType::U`] (a degenerate `v` iso curve),
    /// [`SurfParamType::V`], [`SurfParamType::Both`] or [`SurfParamType::Neither`].
    pub direction: SurfParamType,
}

impl Surface {
    /// Whether `uv` is a singularity (a zero or near zero first derivative, or a point
    /// within `d3d_tol` of a pole) and in which direction (`IsSingularity`; upstream's
    /// defaults are `d3dTol = SM_EFF_ZERO`, `bPtTestOnly = FALSE`). A plane has none.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: the evaluations of a curve-bounded surface and the
    /// tolerance of a face-owned surface in `GetSingularities`.
    pub fn is_singularity(
        &self,
        k: &Kernel,
        uv: Point2,
        d3d_tol: f64,
        pt_test_only: bool,
    ) -> Result<Singularity> {
        self.is_singularity_with_poles(k, uv, SS_UNKNOWN, None, d3d_tol, pt_test_only)
    }

    /// [`Surface::is_singularity`] with the surface's singular sides (`SM_SS_*` bits) and
    /// pole points `[UMinPole, VMinPole, UMaxPole, VMaxPole]` already known
    /// (`pole_points: None` asks `GetSingularities`).
    ///
    /// # Errors
    ///
    /// As [`Surface::is_singularity`]; [`SolidError::InvalidInput`] for fewer than four
    /// pole points.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn is_singularity_with_poles(
        &self,
        k: &Kernel,
        uv: Point2,
        singularities: u64,
        pole_points: Option<&[Point3]>,
        d3d_tol: f64,
        pt_test_only: bool,
    ) -> Result<Singularity> {
        // the SmPlane overrides: a plane has no singularity
        if let SurfaceKind::Plane(p) = self.kind() {
            let (singular, direction) = if pole_points.is_some() || singularities != SS_UNKNOWN {
                p.is_singularity_with_poles()
            } else {
                p.is_singularity()
            };
            return Ok(Singularity {
                singular,
                direction,
            });
        }

        // init output
        let mut singular_u = false;
        let mut singular_v = false;

        // SER in a function returning SmBoolean: a failing status reads as TRUE
        let failed = Singularity {
            singular: true,
            direction: SurfParamType::Neither,
        };
        let simple =
            match self.evaluate_simple(k, uv, 1, 1, upper_flags(Side::Left, Side::Left, false)) {
                Ok(m) => m,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Ok(failed),
            };
        let su = at(&simple, 1, 0, OP_SING)?;
        let sv = at(&simple, 0, 1, OP_SING)?;

        let mag_sq_su = su.length_squared();
        let mag_sq_sv = sv.length_squared();

        let ratio_limit = ZONE_TOL_3D; // Was SM_EFF_ZERO_SQRT, that's too small. [B163]
        let abs_limit = EFF_ZERO * 100.0;

        // when TangentU is small see if it is zero or near zero
        if mag_sq_su < EFF_ZERO_SQRT {
            // When TangentU is equivalently zero
            if mag_sq_su < EFF_ZERO_SQ {
                singular_u = true;
            } else {
                // when TangentU is nearly zero
                let mut mag_sv = crate::geom::vector3d::sqrt(mag_sq_sv);
                let mut mag_su = crate::geom::vector3d::sqrt(mag_sq_su);
                let dom_size = self.natural_uv_domain_with_nurb(k)?.size();
                mag_su *= dom_size.x;
                mag_sv *= dom_size.y;

                // when TangentU is much less than TangentV
                if mag_su < mag_sv * ratio_limit || mag_su < abs_limit {
                    // Further tests still: use the regular evaluator with the flag set
                    // to return non-zero derivatives.
                    let m3 =
                        match self.evaluate(k, uv, 2, 2, upper_flags(Side::Left, Side::Left, true))
                        {
                            Ok(m) => m,
                            Err(e) if e.is_unported() => return Err(e),
                            Err(_) => return Ok(failed),
                        };
                    let su_unit = at(&m3, 1, 0, OP_SING)?.unitize_or_z();
                    let suv = at(&m3, 1, 1, OP_SING)?;
                    let mag_su_v = su_unit.dot(suv);

                    // At the low end of the domain (the v domain), Su would be
                    // increasing, at the top it's decreasing.
                    let mid_param = self.natural_uv_domain_with_nurb(k)?.v_interval().mid();
                    if (uv.y < mid_param && mag_su_v > mag_su)
                        || (uv.y > mid_param && mag_su_v < -mag_su)
                    {
                        singular_u = true;
                    }
                }
            }
        }

        // when TangentV is small see if it is zero or near zero
        if mag_sq_sv < EFF_ZERO_SQRT {
            // When TangentV is equivalently zero
            if mag_sq_sv < EFF_ZERO_SQ {
                singular_v = true;
            } else {
                let mut mag_sv = crate::geom::vector3d::sqrt(mag_sq_sv);
                let mut mag_su = crate::geom::vector3d::sqrt(mag_sq_su);
                let dom_size = self.natural_uv_domain_with_nurb(k)?.size();
                mag_su *= dom_size.x;
                mag_sv *= dom_size.y;

                // when TangentV is much less than TangentU
                if mag_sv < mag_su * ratio_limit || mag_sv < abs_limit {
                    let m3 =
                        match self.evaluate(k, uv, 2, 2, upper_flags(Side::Left, Side::Left, true))
                        {
                            Ok(m) => m,
                            Err(e) if e.is_unported() => return Err(e),
                            Err(_) => return Ok(failed),
                        };
                    let sv_unit = at(&m3, 0, 1, OP_SING)?.unitize_or_z();
                    let suv = at(&m3, 1, 1, OP_SING)?;
                    let mag_sv_u = sv_unit.dot(suv);

                    // At the low end of the domain (the u domain), Sv would be
                    // increasing, at the top it's decreasing.
                    let mid_param = self.natural_uv_domain_with_nurb(k)?.u_interval().mid();
                    if (uv.x < mid_param && mag_sv_u > mag_sv)
                        || (uv.x > mid_param && mag_sv_u < -mag_sv)
                    {
                        singular_v = true;
                    }
                }
            }
        }

        // all done set output and return
        let mut direction = match (singular_u, singular_v) {
            (true, true) => SurfParamType::Both,
            (true, false) => SurfParamType::U,
            (false, true) => SurfParamType::V,
            (false, false) => SurfParamType::Neither,
        };

        if direction == SurfParamType::Neither && !pt_test_only {
            // when still SM_SP_NEITHER check 3d distances to poles
            let (sides, poles) = match pole_points {
                Some(p) => (singularities, p.to_vec()),
                None => {
                    // GetSingularities returns no status; a failing evaluation inside it
                    // leaves its initial outputs (SM_SS_NONE)
                    match ignored(self.get_singularities(k, true, false))? {
                        Some(r) => (
                            u64::from(r.singular),
                            r.pole_points
                                .map_or_else(|| vec![Vec3::UNDEF; 4], |p| p.to_vec()),
                        ),
                        None => (SS_NONE, vec![Vec3::UNDEF; 4]),
                    }
                }
            };

            // when the surface has poles
            if sides != SS_NONE {
                let srf_point = ignored(self.evaluate_point(k, uv))?.unwrap_or(Vec3::UNDEF);
                let pole = |i: usize| -> Result<Point3> {
                    poles.get(i).copied().ok_or(SolidError::invalid_input(
                        OP_SING,
                        "fewer than 4 pole points",
                    ))
                };

                if (sides & SS_UMIN) != 0 && srf_point.closer_than(d3d_tol, pole(0)?) {
                    direction = SurfParamType::V;
                } else if (sides & SS_VMIN) != 0 && srf_point.closer_than(d3d_tol, pole(1)?) {
                    direction = SurfParamType::U;
                } else if (sides & SS_UMAX) != 0 && srf_point.closer_than(d3d_tol, pole(2)?) {
                    direction = SurfParamType::V;
                } else if (sides & SS_VMAX) != 0 && srf_point.closer_than(d3d_tol, pole(3)?) {
                    direction = SurfParamType::U;
                }
            }
        }

        Ok(Singularity {
            singular: direction != SurfParamType::Neither,
            direction,
        })
    }

    /// The unit surface normal at a singularity, the normalised sum of 17 sample normals
    /// along the iso line next to it (`EvaluateNormalAtSingularity`; the curved-bounded
    /// override forwards to its basis surface; upstream's default `d3dTol` is
    /// `SM_EFF_ZERO`). At a point that is no singularity it is the normal of that point.
    ///
    /// # Errors
    ///
    /// Those of `EvaluateNormal`; `SM_ERR_INVALID_INPUT` when the normals sum to zero
    /// (`SER(Unitize())`); [`SolidError::Unported`] from the singularity test.
    pub fn evaluate_normal_at_singularity(
        &self,
        k: &Kernel,
        uv: Point2,
        d3d_tol: f64,
    ) -> Result<Vec3> {
        if let SurfaceKind::CurveBounded(c) = self.kind() {
            return c.basis().evaluate_normal_at_singularity(k, uv, d3d_tol);
        }

        // skip nonSingular points
        let sing = self.is_singularity(k, uv, d3d_tol, false)?;
        if !sing.singular {
            // This point is not a singularity (a warning upstream).
            let dom = self.natural_uv_domain_with_nurb(k)?;
            let low_u = uv.x < dom.u_interval().mid();
            let low_v = uv.y < dom.v_interval().mid();
            let side = |low: bool| if low { Side::Left } else { Side::Right };
            return self.evaluate_normal(k, uv, side(low_u), side(low_v));
        }

        // locals
        let uv_domain = self.natural_uv_domain_with_nurb(k)?;
        let mut norm_sum = Vec3::ZERO;

        if sing.direction == SurfParamType::V {
            let delta = (uv_domain.max().x - uv_domain.min().x) / 100.00;
            let mut stepoff_u = uv.x + delta;
            if stepoff_u > uv_domain.max().x {
                stepoff_u = uv.x - delta;
            }
            for i in 0..17_u32 {
                let norm_param = f64::from(i) / 17.0;
                let mut uv_pnt = uv_domain.evaluate(norm_param, norm_param);
                uv_pnt.x = stepoff_u;
                let norm = self.evaluate_normal(k, uv_pnt, Side::Left, Side::Left)?;
                norm_sum += norm;
            }
        }
        if sing.direction == SurfParamType::U {
            let delta = (uv_domain.max().y - uv_domain.min().y) / 100.00;
            let mut stepoff_v = uv.y + delta;
            if stepoff_v > uv_domain.max().y {
                stepoff_v = uv.y - delta;
            }
            for i in 0..17_u32 {
                let norm_param = f64::from(i) / 17.0;
                let mut uv_pnt = uv_domain.evaluate(norm_param, norm_param);
                uv_pnt.y = stepoff_v;
                let norm = self.evaluate_normal(k, uv_pnt, Side::Left, Side::Left)?;
                norm_sum += norm;
            }
        }

        // SER(sNormSum.Unitize()): a zero sum is the SM_ERR_INVALID_INPUT status
        norm_sum.unitize().map_err(|_| SolidError::Upstream {
            operation: "surface::surface::p5::evaluate_normal_at_singularity",
            code: crate::error::SM_ERR_INVALID_INPUT,
            entity: None,
        })
    }
}

// ---- CreateIsoParametricCurve -----------------------------------------------------------------

/// The result of [`Surface::create_iso_parametric_curve`].
#[derive(Debug, Clone)]
pub struct IsoParametricCurve {
    /// `rpNewIsoCurve`: the 3D iso curve (exact for planes and B-splines, else a
    /// piecewise Hermite approximation).
    pub curve: Curve,
    /// `pOptMaxGap3d`: the achieved maximum gap.
    pub max_gap_3d: f64,
    /// `pOptUVIsoCurve`: the 2D UV line, when asked for and built.
    pub uv_iso_curve: Option<Curve>,
}

/// An `SmIsoCurve` of the original concrete surface.
fn iso_curve_of(
    k: &Kernel,
    surface: &Surface,
    param: SurfParamType,
    iso: f64,
    from_left: bool,
) -> Result<IsoCurve> {
    IsoCurve::from_surface(
        k,
        surface.clone_for_evaluation()?,
        param,
        iso,
        from_left,
        false,
    )
}

impl Surface {
    /// The 3D iso-parametric curve at `iso_parameter` (`CreateIsoParametricCurve`):
    /// constant `u` for [`SurfParamType::U`] (a curve in `v`), else constant `v`. B-spline
    /// surfaces and planes build it exactly (their overrides); other surfaces run the
    /// `SmSurface` body: a point curve at a singularity, else a piecewise Hermite
    /// approximation within `approx_tol_3d` (`0` picks `1e-4 * length`-style old
    /// tolerance: `length * SM_EFF_ZERO_SQRT * 100`). `domain` trims the curve;
    /// `want_uv_iso_curve` asks for the 2D UV line.
    ///
    /// # Errors
    ///
    /// Those of the overrides and of the approximation; [`SolidError::Unported`] for a
    /// curve-bounded surface (see the module deviations).
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn create_iso_parametric_curve(
        &self,
        k: &Kernel,
        param: SurfParamType,
        iso_parameter: f64,
        approx_tol_3d: f64,
        domain: Option<&Extent2d>,
        want_uv_iso_curve: bool,
    ) -> Result<IsoParametricCurve> {
        match self.kind_with_nurb(k)? {
            // (SmSurfOfExtrusion inherits the B-spline override)
            SurfaceKind::SurfOfExtrusion(_) => {
                let opts = crate::nurbs::b_spline_surface::p2::IsoCurveOptions {
                    domain: domain.copied(),
                    uv_iso_curve: want_uv_iso_curve,
                };
                let r = self
                    .extrusion_base_with_nurb(k)?
                    .create_iso_parametric_curve(k, param, iso_parameter, opts)?;
                return Ok(IsoParametricCurve {
                    curve: Curve::from(r.curve),
                    max_gap_3d: r.max_gap_3d,
                    uv_iso_curve: r.uv_iso_curve,
                });
            }
            SurfaceKind::BSpline(b) => {
                let opts = crate::nurbs::b_spline_surface::p2::IsoCurveOptions {
                    domain: domain.copied(),
                    uv_iso_curve: want_uv_iso_curve,
                };
                let r = b.create_iso_parametric_curve(k, param, iso_parameter, opts)?;
                return Ok(IsoParametricCurve {
                    curve: Curve::from(r.curve),
                    max_gap_3d: r.max_gap_3d,
                    uv_iso_curve: r.uv_iso_curve,
                });
            }
            SurfaceKind::Plane(p) => {
                let r =
                    p.create_iso_parametric_curve(param, iso_parameter, domain, want_uv_iso_curve)?;
                return Ok(IsoParametricCurve {
                    curve: Curve::from(r.curve),
                    max_gap_3d: r.max_gap_3d,
                    uv_iso_curve: r.uv_iso_curve,
                });
            }
            SurfaceKind::Cone(s) => {
                // (the Rust override has no UV iso line option: `uv_iso_curve` is `None`)
                let r = s.create_iso_parametric_curve(
                    k,
                    param,
                    iso_parameter,
                    domain,
                    want_uv_iso_curve,
                )?;
                return Ok(IsoParametricCurve {
                    curve: r.curve,
                    max_gap_3d: r.max_gap_3d,
                    uv_iso_curve: r.uv_iso_curve,
                });
            }
            SurfaceKind::Sphere(s) => {
                let r = s.create_iso_parametric_curve(
                    k,
                    param,
                    iso_parameter,
                    domain,
                    want_uv_iso_curve,
                )?;
                return Ok(IsoParametricCurve {
                    curve: r.curve,
                    max_gap_3d: r.max_gap_3d,
                    uv_iso_curve: r.uv_iso_curve,
                });
            }
            SurfaceKind::Torus(s) => {
                let r = s.create_iso_parametric_curve(
                    k,
                    param,
                    iso_parameter,
                    domain,
                    want_uv_iso_curve,
                )?;
                return Ok(IsoParametricCurve {
                    curve: r.curve,
                    max_gap_3d: r.max_gap_3d,
                    uv_iso_curve: r.uv_iso_curve,
                });
            }
            SurfaceKind::SurfOfRevolution(s) => {
                // (the Rust override has no UV iso line option: `uv_iso_curve` is `None`)
                let r = s.create_iso_parametric_curve(k, param, iso_parameter, domain)?;
                return Ok(IsoParametricCurve {
                    curve: r.curve,
                    max_gap_3d: r.max_gap_3d,
                    uv_iso_curve: r.uv_iso_curve,
                });
            }
            SurfaceKind::OffsetSurface(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::SrfInVolume(_) => {}
        }

        // define the isoParameter Curve as an object
        let iso = iso_curve_of(k, self, param, iso_parameter, true)?;

        // when asked - build OptUVIsoCurve (status ignored)
        let uv_iso_curve = if want_uv_iso_curve {
            ignored(iso.create_uv_iso_line_with_nurb(k))?
        } else {
            None
        };

        // get isoParameter curve mid UVPoint
        let mut uv = self.natural_uv_domain_with_nurb(k)?.evaluate(0.5, 0.5);
        if param == SurfParamType::U {
            uv.x = iso_parameter;
        } else {
            uv.y = iso_parameter;
        }

        // If we have a singluarity create a degenerate curve
        if self.is_singularity(k, uv, EFF_ZERO, false)?.singular {
            let pnt = self.evaluate_point(k, uv)?;
            let mut curve = BSplineCurve::create_point_curve(pnt)?;

            // adjust param range to match the surface Natural UVDomain or a given domain
            let mut uv_domain = self.natural_uv_domain_with_nurb(k)?;
            if let Some(d) = domain {
                // upstream ignores the status: a disjoint domain leaves Init()
                uv_domain = uv_domain.intersect(d).unwrap_or_else(|_| Extent2d::new());
            }
            let ivl = if param == SurfParamType::U {
                uv_domain.v_interval()
            } else {
                uv_domain.u_interval()
            };
            if !ivl.is_degenerate(0.0) {
                crate::error::ignore_status(curve.edit_parameterization(&ivl))?;
            }
            return Ok(IsoParametricCurve {
                curve: Curve::from(curve),
                max_gap_3d: 0.0,
                uv_iso_curve,
            });
        }

        // Select a tolerance (SM_USE_OLDTOL)
        let mut approx_tol = approx_tol_3d;
        if approx_tol == 0.0 {
            let iso_curve = Curve::from(iso.clone());
            let (length, _) =
                iso_curve.approximate_length(k, &iso_curve.natural_interval(), 5, false)?;
            approx_tol = length * EFF_ZERO_SQRT * 100.0;
        }

        // get unique knots crossing the isoParameter curve
        let (mut knots, _) = if param == SurfParamType::U {
            self.knots_with_nurb(k, SurfParamType::V, None)?
        } else {
            self.knots_with_nurb(k, SurfParamType::U, None)?
        };

        // when given an optional domain
        if let Some(d) = domain {
            // get min/max values of isoParameter Line's varying parameter
            let (d_min, d_max) = if param == SurfParamType::U {
                (d.min().y, d.max().y)
            } else {
                (d.min().x, d.max().x)
            };

            // remove knots below or equal to dMin value -
            while knots.len() > 1 && knots.first().is_some_and(|&x| x <= d_min + EFF_ZERO) {
                knots.remove(0);
            }
            if knots.len() < 2 || knots.first().is_some_and(|&x| (x - d_min).abs() > EFF_ZERO) {
                knots.insert(0, d_min);
            }

            // remove knots above or equal to dMax value
            while knots.len() > 1 && knots.last().is_some_and(|&x| x >= d_max) {
                knots.pop();
            }
            if knots.len() < 2 || knots.last().is_some_and(|&x| (x - d_max).abs() > EFF_ZERO) {
                knots.push(d_max);
            }

            // when isoParameter line domain length is degenerate
            let first = knots.first().copied().unwrap_or(0.0);
            let last = knots.last().copied().unwrap_or(0.0);
            if last - first < EFF_ZERO {
                // return a degenerate curve (parameterized from 0 to 1)
                let pnt = self.evaluate_point(k, uv)?;
                let curve = BSplineCurve::create_point_curve(pnt)?;
                return Ok(IsoParametricCurve {
                    curve: Curve::from(curve),
                    max_gap_3d: 0.0,
                    uv_iso_curve,
                });
            }
        }

        // build approximation curve as a piecewise Hermite Curve approximation
        let iso_curve = Curve::from(iso);
        let opts = ApproximateCurveOptions {
            create_analytics: false,
            match_parameterization: false,
            just_copy_bsplines: false,
        };
        let approx = iso_curve.approximate_curve(k, &knots, approx_tol, &opts)?;

        Ok(IsoParametricCurve {
            curve: approx.curve,
            max_gap_3d: approx.achieved_tol.unwrap_or(0.0),
            uv_iso_curve,
        })
    }
}

// ---- LocalPointSolve --------------------------------------------------------------------------

/// The result of [`Surface::local_point_solve`].
#[derive(Debug, Clone, Copy)]
pub struct PointSolve<O> {
    /// `rbFoundAnswer`.
    pub found: bool,
    /// `rSolution`: type, value, `[u, v]` and `m_lNumVariables = 2`.
    pub solution: Solution<O>,
}

/// The result of [`Surface::local_point_solve_0`].
#[derive(Debug, Clone, Copy)]
pub struct PointSolve0<O> {
    /// `rbFoundAnswer`.
    pub found: bool,
    /// `rSolution` (`m_lNumVariables` is left at `0`).
    pub solution: Solution<O>,
    /// `eReason_0`.
    pub reason: TerminationReason,
    /// `iIterCount_0`.
    pub iterations: i32,
}

/// What the Newton solve of `SmFindPSNormalENFO` produced.
struct PsSolve {
    /// `bFoundSolution`.
    found: bool,
    /// `sSolutionVector`.
    solution: [f64; 2],
    /// `sLS.GetTerminationReason()`.
    reason: TerminationReason,
    /// `sLS.m_lCurrentIteration`.
    iterations: i32,
}

/// `SmLocalSolveNd::SolveIt` of an `SmFindPSNormalENFO` over `domain` from `guess`.
fn find_ps_normal_solve(
    k: &Kernel,
    surface: &Surface,
    test_point: Point3,
    domain: &Extent2d,
    op: SolverOperation,
    guess: Point2,
) -> Result<PsSolve> {
    let mut intervals = ExtentNd::new(2);
    if let Some(slot) = intervals.get_mut(0) {
        *slot = Extent1d::from_min_max(domain.min().x, domain.max().x);
    }
    if let Some(slot) = intervals.get_mut(1) {
        *slot = Extent1d::from_min_max(domain.min().y, domain.max().y);
    }

    // In the local solver, we don't check for periodicity.
    let periodicities = [false; 2];

    // set the guess value
    let guess_t = [guess.x, guess.y];

    // construct - the equation set callback function
    //           - the Newton-Raphson solver using the equation set
    //           - specify boundary behavior
    let mut equations = FindPsNormalEnfo::new(k, test_point, surface, EFF_ZERO * EFF_ZERO, op);
    let mut solver = LocalSolveNd::new(&mut equations, Some(&intervals), Some(&periodicities));
    solver.set_boundary_handler(BoundaryHandler::TotalBoundaryHits, 3);

    // let solver look for an answer with a loose tolerance (SER)
    let loose_tol = 1.0; // formerly SM_EFF_ZERO_SQRT*100.0
    let r = solver.solve_it(&guess_t, loose_tol)?;
    let solution = [
        r.x.first().copied().unwrap_or(f64::NAN),
        r.x.get(1).copied().unwrap_or(f64::NAN),
    ];

    Ok(PsSolve {
        found: r.found,
        solution,
        reason: solver.termination_reason(),
        iterations: i32::try_from(solver.current_iteration()).unwrap_or(i32::MAX),
    })
}

/// The tail of `LocalPointSolve_0` for minimise, maximise and normalise: reject a failed
/// solve unless it stopped at a boundary, re-test a normalised point and fill the solution.
fn finish_ps_solve<O: Copy>(
    k: &Kernel,
    surface: &Surface,
    test_point: Point3,
    domain: &Extent2d,
    op: SolverOperation,
    solve: &PsSolve,
) -> Result<PointSolve0<O>> {
    let mut out = PointSolve0 {
        found: false,
        solution: Solution::new(),
        reason: solve.reason,
        iterations: solve.iterations,
    };
    let uv = Point2::new(solve.solution[0], solve.solution[1]);

    // when SolveIt failed to converge
    if !solve.found
        && ((solve.reason != TerminationReason::OutOfBounds
            && !domain.is_point_on_boundary(uv, EFF_ZERO).0)
            || op == SolverOperation::Normalize)
    {
        // when normalizing must be within bounds
        return Ok(out);
    }

    // Since we used a loose tolerance, we don't want false positives when Normalizing.
    if op == SolverOperation::Normalize {
        // test crTestPoint/SurfaceNormal(sUV) dist against SM_EFF_ZERO * 100.0
        let on_normal =
            is_point_on_normal(k, surface, None, uv, test_point, None, EFF_ZERO * 100.0)?;
        out.found = on_normal.good_point;

        let d = ignored(evaluate_1st_derivatives(
            k,
            surface,
            uv,
            Side::Right,
            Side::Right,
        ))?
        .unwrap_or((Vec3::UNDEF, Vec3::UNDEF, Vec3::UNDEF));
        let (pnt1, du, dv) = d;
        let mut gap = pnt1 - test_point;
        let len_sq = gap.length_squared();
        if len_sq > EFF_ZERO {
            // if same point, we're ok.
            gap /= crate::geom::vector3d::sqrt(len_sq);
            let mut f_val = gap.dot(du).abs();
            f_val += gap.dot(dv).abs();
            let tighter_tol = EFF_ZERO_SQRT * 100.0;
            if f_val > tighter_tol {
                out.found = false;
                return Ok(out);
            }
        }
    }

    // else when SolveIt converged or found a boundary value solution
    let pnt1 = surface.evaluate_point(k, uv)?;

    out.found = true;
    out.solution.solution_type = SolutionType::SingleValue;
    out.solution.start.solution_value = pnt1.distance(test_point);
    *out.solution.start.param_mut(0)? = solve.solution[0];
    *out.solution.start.param_mut(1)? = solve.solution[1];
    Ok(out)
}

impl Surface {
    /// Refines the guess `guess_uv` to a solution of `op` for `test_point`
    /// (`LocalPointSolve`): `LocalPointSolve_1` (unit `surface.surface_point_solve_p3`)
    /// unless the operation is [`SolverOperation::Maximize`] (upstream's new point
    /// solvers are on), which runs [`Surface::local_point_solve_0`].
    ///
    /// # Errors
    ///
    /// Those of the selected local solver. The caller's kernel supplies tolerances.
    pub fn local_point_solve<O: Copy>(
        &self,
        k: &Kernel,
        domain: &Extent2d,
        op: SolverOperation,
        test_point: Point3,
        guess_uv: Point2,
    ) -> Result<PointSolve<O>> {
        // Don't use new solvers for MAXIMIZE, but always use them for DIRECTED
        let use_new =
            op != SolverOperation::Maximize || op == SolverOperation::Signed3dDirectedMaximize;

        if use_new {
            let r = self.local_point_solve_1(k, domain, op, test_point, guess_uv, None)?;
            let mut solution = r.solution;
            solution.num_variables = 2;
            return Ok(PointSolve {
                found: r.found,
                solution,
            });
        }

        let r = self.local_point_solve_0(k, domain, op, test_point, guess_uv)?;
        let mut solution = r.solution;
        solution.num_variables = 2;
        Ok(PointSolve {
            found: r.found,
            solution,
        })
    }

    /// The original `LocalPointSolve`. For [`SolverOperation::Intersect`] it drops the
    /// point to the tangent plane up to 100 times, succeeding when two successive surface
    /// points are within a scaled zero; the other operations run a Newton solve of
    /// `SmFindPSNormalENFO` (unit `surface.surface_p1`).
    ///
    /// # Errors
    ///
    /// The evaluation errors; [`SolidError::Unported`] for `SmFindPSNormalENFO`.
    pub fn local_point_solve_0<O: Copy>(
        &self,
        k: &Kernel,
        domain: &Extent2d,
        op: SolverOperation,
        test_point: Point3,
        guess_uv: Point2,
    ) -> Result<PointSolve0<O>> {
        // (upstream only asserts the operation in debug builds; any other one runs the
        // Newton branch below)

        // for intersections - Repeatedly drop Point to tangent plane to convergence
        if op != SolverOperation::Intersect {
            let solve = find_ps_normal_solve(k, self, test_point, domain, op, guess_uv)?;
            return finish_ps_solve(k, self, test_point, domain, op, &solve);
        }

        let mut found = false;
        let mut iterations = 0_i32;
        let mut uv = guess_uv;
        let mut boundary_hits = 0_u32;

        // get initial SurfacePoint guess
        let mut last_pnt = self.evaluate_point(k, uv)?;

        // scale zero
        let scaled_zero = EFF_ZERO * (1.0 + last_pnt.max_dimension());
        let scaled_zero_sq = scaled_zero * scaled_zero;

        // for 100 iterations
        let mut iteration = 0_u32;
        while iteration < 100 {
            // Drop 3dPoint to surface tangent plane
            let on_plane = super::p7::drop_points_to_tangent_plane(
                k,
                self,
                uv,
                Side::Left,
                Side::Left,
                &[test_point],
            )?;
            let uv_on_plane = on_plane
                .first()
                .copied()
                .ok_or(SolidError::invalid_input(OP_LPS0, "no dropped point"))?;

            // count boundary hits - Clamp outOfBounds values to domain
            if !domain.contains_point(uv_on_plane, 0.0) {
                boundary_hits += 1;
                if boundary_hits >= 4 {
                    break;
                }
            }
            uv = domain.clamp_point(uv_on_plane);

            // Get the newly dropped 3D Point
            let pnt = self.evaluate_point(k, uv)?;

            // when Drop was less than ZeroSquared
            if pnt.distance_squared(last_pnt) < scaled_zero_sq {
                if iteration < 98 {
                    iteration = 98; // Just about done take one more and quit
                }
                found = true;
            }

            // increment LastPoint
            last_pnt = pnt;
            iterations += 1;
            iteration += 1;
        }

        let mut out = PointSolve0 {
            found,
            solution: Solution::new(),
            reason: TerminationReason::FoundAnswerConverged,
            iterations,
        };

        // no convergence - all done
        if !found {
            out.reason = TerminationReason::UnableToConverge;
            return Ok(out);
        }

        // else set output
        out.solution.solution_type = SolutionType::SingleValue;
        out.solution.start.solution_value = last_pnt.distance(test_point);
        *out.solution.start.param_mut(0)? = uv.x;
        *out.solution.start.param_mut(1)? = uv.y;
        Ok(out)
    }
}

// ---- DropPoint --------------------------------------------------------------------------------

/// The optional arguments of [`Surface::drop_point`].
#[derive(Debug, Clone, Copy)]
pub struct DropPointOptions {
    /// `pOptGuessUV`: when given, only local solves are tried before the global one and
    /// the solution nearest it wins.
    pub guess_uv: Option<Point2>,
    /// `eOperationType` (default [`SolverOperation::Minimize`]).
    pub operation: SolverOperation,
    /// `pOptTgtSeamSide`: selects the solution on the seam side whose inward tangent best
    /// matches this vector.
    pub tgt_seam_side: Option<Vec3>,
    /// Whether the full solution array is wanted (`pOptSolutionArray != NULL`).
    pub want_solutions: bool,
}

impl Default for DropPointOptions {
    fn default() -> Self {
        Self {
            guess_uv: None,
            operation: SolverOperation::Minimize,
            tgt_seam_side: None,
            want_solutions: false,
        }
    }
}

/// The result of [`Surface::drop_point`].
#[derive(Debug, Clone)]
pub struct DropPoint<O> {
    /// `rbSuccess`.
    pub success: bool,
    /// `rResultUV`: the drop point (the guess, or undefined, when none was found).
    pub result_uv: Point2,
    /// `rdGap`: the drop distance (`SM_BIG_DOUBLE` when none was found).
    pub gap: f64,
    /// `rbIsMultivalued`.
    pub is_multivalued: bool,
    /// The solution array as upstream leaves it (`pOptSolutionArray`, or its local array):
    /// the solutions before a best one was picked.
    pub solutions: SolutionArray<O>,
}

impl Surface {
    /// Inverts `pt_to_drop` on the surface: the UV of the surface point closest to it
    /// (`DropPoint`). A fast drop (planes) is tried first, then a local solve from the
    /// guess, then the global solve. With several solutions the best is the one with the
    /// largest inward-tangent match to the seam side, else nearest the guess, else the
    /// smallest gap.
    ///
    /// # Errors
    ///
    /// Those of the global solve; [`SolidError::Unported`]: `GetFace` and
    /// `SmFace::GetTolerance` for an owned surface without the face tolerance cached
    /// ([`Surface::owner_zone_tol_3d`]), `LocalPointSolve_1`.
    #[allow(clippy::too_many_lines)] // one upstream routine
    pub fn drop_point<O: SolutionObject>(
        &self,
        k: &Kernel,
        pt_to_drop: Point3,
        domain: &Extent2d,
        opts: &DropPointOptions,
    ) -> Result<DropPoint<O>> {
        self.drop_point_from(k, pt_to_drop, domain, opts, None, None)
    }

    /// `DropPoint` with lazy cache preparation on the surface's owning face.
    /// The context is consulted only at the original global-solve call; fast
    /// and local drops use the same implementation and operation order.
    pub(crate) fn drop_point_with_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        pt_to_drop: Point3,
        domain: &Extent2d,
        opts: &DropPointOptions,
        handle: Option<O>,
        context: &mut dyn SurfaceSolveContext<O>,
    ) -> Result<DropPoint<O>> {
        self.drop_point_from(k, pt_to_drop, domain, opts, handle, Some(context))
    }

    fn drop_point_from<O: SolutionObject>(
        &self,
        k: &Kernel,
        pt_to_drop: Point3,
        domain: &Extent2d,
        opts: &DropPointOptions,
        handle: Option<O>,
        context: Option<&mut dyn SurfaceSolveContext<O>>,
    ) -> Result<DropPoint<O>> {
        let op = opts.operation;
        let guess = opts.guess_uv;

        // Init outputs
        let mut success = false;
        let mut is_multivalued = false;
        let mut gap = BIG_DOUBLE;
        let mut result_uv = guess.unwrap_or(Vec2::UNDEF);

        let mut solutions = SolutionArray::<O>::new();
        let mut sol = Solution::<O>::new();
        let mut answer_in_sol = false;
        let mut answer_in_array = false;

        // We need a tolerance value.
        // pFace ? pFace->GetTolerance() : SM_XSECT_TOL_3D (the face's tolerance cached
        // on the surface; UNPORTED(topology.face_p2) for a bare face handle)
        // Proof (unreachable): a B-rep face's surface carries the face tolerance
        // (Brep::sync_face_surface_tolerance invariant, see Surface::face_zone_tol_3d); 0
        // hits in the green difftest sweep.
        let dist_tol = self.face_zone_tol_3d()?.unwrap_or(XSECT_TOL_3D);

        // first try DropPointFast(), to catch special cases.
        if let Some(fast) = ignored(self.drop_point_fast::<O>(
            k,
            domain,
            op,
            pt_to_drop,
            None,
            dist_tol,
            None,
            SolutionRequested::All,
        ))? {
            solutions = fast;
            if let Some(first) = solutions.get(0) {
                // DropPointFast() can return points outside the domain.
                let uv = Point2::new(first.start.param(0)?, first.start.param(1)?);
                if domain.contains_point(uv, 0.0) {
                    success = true;
                    answer_in_array = true;
                }
            }
        }

        // Try LocalPointSolve if a guess was supplied.
        if !success {
            if let Some(g) = guess {
                match ignored(self.local_point_solve::<O>(k, domain, op, pt_to_drop, g))? {
                    Some(r) => {
                        success = r.found;
                        sol = r.solution;
                    }
                    None => success = false,
                }

                // Check whether we think that this solution is ok, i.e., was it a good
                // enough guess?  [080206]
                if success {
                    success = good_drop_result(k, self, domain, pt_to_drop, g, &sol.start)?;
                }
                if success {
                    answer_in_sol = true;
                }
            }
        }

        if !success {
            // Haven't found it yet. Call GlobalPointSolve.
            if let Some(context) = context {
                self.global_point_solve_into_with_context(
                    k,
                    handle,
                    None,
                    context,
                    domain,
                    op,
                    pt_to_drop,
                    dist_tol,
                    None,
                    SolutionRequested::All,
                    &mut solutions,
                )?;
            } else {
                solutions = self.global_point_solve::<O>(
                    k,
                    None,
                    domain,
                    op,
                    pt_to_drop,
                    dist_tol,
                    None,
                    SolutionRequested::All,
                )?;
            }
            if !solutions.is_empty() {
                success = true;
                answer_in_array = true;
            }
        }

        // Check results of local and global drops.
        if answer_in_sol {
            result_uv = Point2::new(sol.start.param(0)?, sol.start.param(1)?);
            gap = sol.start.solution_value;
        }

        if answer_in_array {
            let first_type = solutions
                .get(0)
                .map(|s| s.solution_type)
                .ok_or(SolidError::invalid_input(OP_DROP, "empty solution array"))?;
            if solutions.len() > 1 || first_type == SolutionType::RangeOfValues {
                is_multivalued = true;
            }

            if opts.want_solutions && is_multivalued {
                return Ok(DropPoint {
                    success,
                    result_uv,
                    gap,
                    is_multivalued,
                    solutions,
                });
            }

            // Search array for best solution
            let nat_domain = self.natural_uv_domain_with_nurb(k)?;
            let mut best_sol = BIG_DOUBLE;
            let mut best_dot = -BIG_DOUBLE;
            let mut best_uv_dist = BIG_DOUBLE;
            let mut i_best_sol: Option<usize> = None;
            let mut i_best_uv_dist: Option<usize> = None;
            let mut i_best_dot: Option<usize> = None;

            // for every solution - get iBestDot, iBestIdx, and iBestSol
            for (ii, s) in solutions.as_slice().iter().enumerate() {
                let sol_value = s.start.solution_value;

                // save index to best SolutionValue solution
                if sol_value < best_sol {
                    best_sol = sol_value;
                    i_best_sol = Some(ii);
                }

                // when asked to pick best solution based on UV Point location
                if opts.tgt_seam_side.is_some() || guess.is_some() {
                    let this_uv = Point2::new(s.start.param(0)?, s.start.param(1)?);

                    // When asked, save Sol whose SolPointUV is closest to OptGuessUV
                    if let Some(g) = guess {
                        let uv_dist = g.distance_squared(this_uv);
                        if uv_dist < best_uv_dist {
                            best_uv_dist = uv_dist;
                            i_best_uv_dist = Some(ii);
                        }
                    }

                    // When asked, save Sol whose inward-facing SurfTan best matches the
                    // target seam side
                    if let Some(tgt) = opts.tgt_seam_side {
                        // classify UV point against NatUVDomain boundaries
                        let (u_type, v_type) = nat_domain.classify_point(this_uv, EFF_ZERO, None);
                        let on_edge = |t: ExtentPointType| {
                            matches!(
                                t,
                                ExtentPointType::Start
                                    | ExtentPointType::End
                                    | ExtentPointType::Both
                            )
                        };

                        // when sThisUV is on a domain boundary - save the solution with
                        // the best matching SurfTangent
                        if on_edge(u_type) || on_edge(v_type) {
                            // evaluate Surf(sThisUV) - eval with NonZero tangents
                            let (_, du, dv) = ignored(evaluate_1st_derivatives(
                                k,
                                self,
                                this_uv,
                                Side::Left,
                                Side::Left,
                            ))?
                            .unwrap_or((
                                Vec3::UNDEF,
                                Vec3::UNDEF,
                                Vec3::UNDEF,
                            ));
                            let du = du.unitize_or_z();
                            let dv = dv.unitize_or_z();

                            let mut consider = |dot: f64| {
                                if dot > best_dot {
                                    best_dot = dot;
                                    i_best_dot = Some(ii);
                                }
                            };
                            // low U boundary
                            if matches!(u_type, ExtentPointType::Start | ExtentPointType::Both) {
                                consider(du.dot(tgt));
                            }
                            // high U boundary
                            if matches!(u_type, ExtentPointType::End | ExtentPointType::Both) {
                                consider(-du.dot(tgt));
                            }
                            // low V boundary
                            if matches!(v_type, ExtentPointType::Start | ExtentPointType::Both) {
                                consider(dv.dot(tgt));
                            }
                            // high V boundary
                            if matches!(v_type, ExtentPointType::End | ExtentPointType::Both) {
                                consider(-dv.dot(tgt));
                            }
                        }
                    }
                }
            }

            // pick the solution to return
            let best = i_best_dot.or(i_best_uv_dist).or(i_best_sol);

            // set output when a best sol was found
            if let Some(i) = best {
                let s = solutions.get_at(i)?;
                result_uv = Point2::new(s.start.param(0)?, s.start.param(1)?);
                gap = s.start.solution_value;
            } else {
                // should never happen.
                success = false;
            }
        }

        Ok(DropPoint {
            success,
            result_uv,
            gap,
            is_multivalued,
            solutions,
        })
    }
}

// ---- LocalSurfaceSurfaceIntersect (from an image-space guess) -----------------------------------

impl Surface {
    /// The local intersection of this surface and two others near the image-space guess
    /// `xyz_guess1` (or `opt_xyz_guess2` when the first does not project): each surface's
    /// nearest point is found with a global point solve, then the local
    /// three-surface solve (`surface.surface_p3`) refines them
    /// (`LocalSurfaceSurfaceIntersect`). Faces' tolerances are `1e-5` for surfaces no
    /// face owns.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: `GetFace`/`SmFace::GetTolerance` for an owned surface
    /// without the face tolerance cached ([`Surface::owner_zone_tol_3d`]) and the
    /// surface caches (`smsurf_GetSurfaceCache`). The status of the UV-guess
    /// `LocalSurfaceSurfaceIntersect` is ignored, as upstream (no answer).
    #[allow(clippy::too_many_arguments)] // upstream's argument list
    pub fn local_surface_surface_intersect_xyz<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain1: &Extent2d,
        surface2: &Self,
        uv_domain2: &Extent2d,
        surface3: &Self,
        uv_domain3: &Extent2d,
        xyz_guess1: Point3,
        opt_xyz_guess2: Option<Point3>,
    ) -> Result<(bool, Solution<O>)> {
        // GWC:TODO - upstream's magic numbers for the tolerance
        // GetFace() ? its tolerance (cached on the surface; UNPORTED(topology.face_p2)
        // for a bare face handle) : 1e-05
        // Proof (unreachable): a B-rep face's surface carries the face tolerance
        // (Brep::sync_face_surface_tolerance invariant, see Surface::face_zone_tol_3d); 0
        // hits in the green difftest sweep.
        let face_tol = |s: &Self| -> Result<f64> { Ok(s.face_zone_tol_3d()?.unwrap_or(1e-05)) };
        let tol1 = face_tol(self)?;
        let tol2 = face_tol(surface2)?;
        let tol3 = face_tol(surface3)?;
        let new_tol = 10.0 * (tol1 + tol2 + tol3);

        // when surfaces are trimmmed - turn off GlobalPointSolve PointTesting
        let kernel = k;
        get_surface_cache(kernel, self, None::<NoObject>)?;
        get_surface_cache(kernel, surface2, None::<NoObject>)?;
        get_surface_cache(kernel, surface3, None::<NoObject>)?;

        // nearest point of each surface (statuses ignored)
        let nearest =
            |s: &Self, domain: &Extent2d, fallback_domain: &Extent2d| -> Result<Option<Point2>> {
                let mut sols = ignored(s.global_point_solve::<O>(
                    kernel,
                    None,
                    domain,
                    SolverOperation::Minimize,
                    xyz_guess1,
                    new_tol,
                    None,
                    SolutionRequested::Single,
                ))?
                .unwrap_or_default();
                if sols.is_empty() {
                    if let Some(g2) = opt_xyz_guess2 {
                        sols = ignored(s.global_point_solve::<O>(
                            kernel,
                            None,
                            fallback_domain,
                            SolverOperation::Minimize,
                            g2,
                            new_tol,
                            None,
                            SolutionRequested::Single,
                        ))?
                        .unwrap_or_default();
                    }
                }
                sols.get(0)
                    .map(|f| Ok(Point2::new(f.start.param(0)?, f.start.param(1)?)))
                    .transpose()
            };

        // (upstream passes the first surface's domain to the second guess of surfaces
        // two and three)
        let Some(uv1) = nearest(self, uv_domain1, uv_domain1)? else {
            return Ok((false, Solution::new()));
        };
        let Some(uv2) = nearest(surface2, uv_domain2, uv_domain1)? else {
            return Ok((false, Solution::new()));
        };
        let Some(uv3) = nearest(surface3, uv_domain3, uv_domain1)? else {
            return Ok((false, Solution::new()));
        };

        // find surf/surf/surf intersection in this neighborhood (status ignored: a
        // failing solve leaves rbFoundAnswer FALSE and rSolution untouched)
        let mut solution = Solution::new();
        let Some(Some(sss)) = ignored(self.local_surface_surface_intersect(
            k, uv_domain1, surface2, uv_domain2, surface3, uv_domain3, uv1, uv2, uv3,
        ))?
        else {
            return Ok((false, solution));
        };
        solution.solution_type = SolutionType::SingleValue;
        solution.start.solution_value = sss.distance;
        *solution.start.param_mut(0)? = sss.uv1.x;
        *solution.start.param_mut(1)? = sss.uv1.y;
        *solution.start.param_mut(2)? = sss.uv2.x;
        *solution.start.param_mut(3)? = sss.uv2.y;
        *solution.start.param_mut(4)? = sss.uv3.x;
        *solution.start.param_mut(5)? = sss.uv3.y;
        Ok((true, solution))
    }
}

// ---- CoverOtherCoincidentSurface --------------------------------------------------------------

/// The result of [`Surface::cover_other_coincident_surface_deferring_trims`].
#[derive(Debug, Clone, PartialEq)]
pub struct CoverOtherCoincident {
    /// `pCoveringSurf`: which surface covers the other: `Some(false)` this one,
    /// `Some(true)` the other, `None` when neither can.
    pub covering_is_other: Option<bool>,
    /// `rdMaxDist`: as the last `CoverCoincidentSurface` call left it (`SM_BIG_DOUBLE`
    /// when none ran or none reported).
    pub max_dist: f64,
    /// `pOptMergedDomain`: the union of both retained domains in the covering surface's
    /// final NURBS parameters; only meaningful with a covering surface, and only when a
    /// merged domain was asked for.
    pub merged_domain: Option<Extent2d>,
    /// The faces whose `SmFace::RemoveUVTrimCurves` the successful
    /// `CoverCoincidentSurface` calls ran upstream (a face-owned covering surface), in
    /// call order, for the caller holding the B-rep to run
    /// (`topology::face::p1::remove_uv_trim_curves`).
    pub remove_uv_trim_curves_of: Vec<ObjectRef>,
}

/// What one `SmSurface::CoverCoincidentSurface` call reports.
struct CoverCoincident {
    /// `bSuccess`.
    success: bool,
    /// `rdMaxDist`.
    max_dist: f64,
    /// `pOptSTEPDomain`: the covered domain projected onto the covering surface, in STEP
    /// coordinates, when asked for.
    step_domain: Option<Extent2d>,
}

/// `cover->CoverCoincidentSurface(covered, tol, bSuccess, rdMaxDist, &covered_domain
/// [, &step_domain])`: whether `cover` covers `covered_domain` of `covered` and the
/// largest gap. `cover` is expanded on success. The owner face whose
/// `RemoveUVTrimCurves` the call runs upstream is appended to `trims`. With
/// `context` (the face solve context and `cover`'s handle in it) the cover's
/// `STEPInversion` solves see `cover`'s face
/// ([`Surface::cover_coincident_surface_deferring_trims_with_context`]).
#[allow(clippy::too_many_arguments)] // upstream's arguments plus the deferred trims
fn cover_coincident_surface<O: SolutionObject>(
    k: &Kernel,
    cover: &mut Surface,
    covered: &Surface,
    tol: f64,
    covered_domain: &Extent2d,
    want_step_domain: bool,
    trims: &mut Vec<ObjectRef>,
    context: Option<(&mut dyn SurfaceSolveContext<O>, Option<O>)>,
) -> Result<CoverCoincident> {
    let r = match context {
        Some(context) => cover.cover_coincident_surface_deferring_trims_with_context(
            k,
            covered,
            tol,
            Some(covered_domain),
            context,
        )?,
        None => {
            cover.cover_coincident_surface_deferring_trims(k, covered, tol, Some(covered_domain))?
        }
    };
    trims.extend(r.remove_uv_trim_curves_of);
    Ok(CoverCoincident {
        success: r.success,
        max_dist: r.max_dist,
        step_domain: if want_step_domain {
            r.covered_step_domain
        } else {
            None
        },
    })
}

/// The state `CoverOtherCoincidentSurface`'s `Cover` lambda updates.
struct CoverState {
    /// `sDomain0`: this surface's domain.
    domain0: Extent2d,
    /// `sDomain1`: the other surface's domain.
    domain1: Extent2d,
    /// `rdMaxDist`.
    max_dist: f64,
    /// `*pOptMergedDomain`.
    merged: Option<Extent2d>,
    /// The faces whose UV trim curves the caller deletes.
    trims: Vec<ObjectRef>,
}

/// The face solve context of
/// [`Surface::cover_other_coincident_surface_deferring_trims_with_context`]: one context
/// lending the faces of both surfaces, and each surface's handle in it.
pub struct CoverSolveContext<'c, O: Copy + PartialEq> {
    /// The solve context (`smsurf_GetSurfaceCache` of either surface sees its face).
    pub context: &'c mut dyn SurfaceSolveContext<O>,
    /// The handles of this surface and of the other one in `context`.
    pub handles: [Option<O>; 2],
}

impl<O: Copy + PartialEq> core::fmt::Debug for CoverSolveContext<'_, O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CoverSolveContext").finish_non_exhaustive()
    }
}

/// The context and the handle of surface `index` (`0` this, `1` the other) in it, for
/// one `CoverCoincidentSurface` call.
fn cover_context<'a, O: Copy + PartialEq>(
    context: &'a mut Option<CoverSolveContext<'_, O>>,
    index: usize,
) -> Option<(&'a mut dyn SurfaceSolveContext<O>, Option<O>)> {
    context.as_mut().map(|c| {
        let handle = c.handles.get(index).copied().flatten();
        (&mut *c.context as &mut dyn SurfaceSolveContext<O>, handle)
    })
}

/// The `Cover` lambda: `cover` covers the other surface; returns `bSuccess && eStat ==
/// SM_SUCCESS`. A failing `CoverCoincidentSurface` or conversion is no cover (a failing
/// `CoverCoincidentSurface` has already set `rdMaxDist` to `SM_BIG_DOUBLE`); only
/// [`SolidError::Unported`] propagates.
#[allow(clippy::too_many_arguments)] // the lambda's captures
fn try_cover<O: SolutionObject>(
    k: &Kernel,
    st: &mut CoverState,
    this: &mut Surface,
    other: &mut Surface,
    cover_is_other: bool,
    tol: f64,
    want_merged: bool,
    context: &mut Option<CoverSolveContext<'_, O>>,
) -> Result<bool> {
    // the covering surface's handle in the face solve context
    let cover_handle = usize::from(cover_is_other);
    let (cover, covered): (&mut Surface, &Surface) = if cover_is_other {
        (other, this)
    } else {
        (this, other)
    };
    let cover_domain = if cover_is_other {
        st.domain1
    } else {
        st.domain0
    };
    let covered_domain = if cover_is_other {
        st.domain0
    } else {
        st.domain1
    };

    if !want_merged {
        return match cover_coincident_surface(
            k,
            cover,
            covered,
            tol,
            &covered_domain,
            false,
            &mut st.trims,
            cover_context(context, cover_handle),
        ) {
            Ok(r) => {
                st.max_dist = r.max_dist;
                Ok(r.success)
            }
            Err(e) if e.is_unported() => Err(e),
            Err(_) => {
                st.max_dist = BIG_DOUBLE;
                Ok(false)
            }
        };
    }

    // STEP coordinates survive rebuilding an analytic surface's NURBS form. Save the
    // retained subset before coverage changes the surface, union it with the projected
    // subset, then map back using the final parameters.
    let this_step = match cover.convert_domain_from_nurbs_to_step(k, &cover_domain) {
        Ok(d) => d,
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => return Ok(false),
    };
    let r = match cover_coincident_surface(
        k,
        cover,
        covered,
        tol,
        &covered_domain,
        true,
        &mut st.trims,
        cover_context(context, cover_handle),
    ) {
        Ok(r) => r,
        Err(e) if e.is_unported() => return Err(e),
        Err(_) => {
            st.max_dist = BIG_DOUBLE;
            return Ok(false);
        }
    };
    st.max_dist = r.max_dist;
    if !r.success {
        return Ok(false);
    }
    // (upstream's sOtherSTEPDomain is default constructed when none is written)
    let other_step = r.step_domain.unwrap_or_else(Extent2d::new);
    let merged_step = this_step.union(&other_step);
    match cover.convert_domain_from_step_to_nurbs(k, &merged_step) {
        Ok(d) => {
            st.merged = Some(d);
            Ok(true)
        }
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(false),
    }
}

impl Surface {
    /// Whether this surface or `other` covers the other within `tol` (`CoverOtherCoincidentSurface`):
    /// planes first, then bilinear surfaces, then B-splines, then anything, each trying to
    /// cover the other with `CoverCoincidentSurface` (a failing try goes on to the next
    /// candidate). `this_domain` and `other_domain` are subsets in NURBS parameters
    /// (`None`: the natural domains). With `want_merged` the union of both retained
    /// domains in the covering surface's final NURBS parameters is returned. The
    /// covering surface may have been expanded (`CoverCoincidentSurface` changes the
    /// surface it is called on when it succeeds).
    ///
    /// The faces whose UV trim curves the successful `CoverCoincidentSurface` calls
    /// delete upstream (`SmFace::RemoveUVTrimCurves` of a face-owned covering surface)
    /// are returned in [`CoverOtherCoincident::remove_uv_trim_curves_of`], in call
    /// order, and the caller, which holds the B-rep, runs
    /// `topology::face::p1::remove_uv_trim_curves` on each right after this call.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from `CoverCoincidentSurface` (the tolerance of an owned
    /// surface) and, when a merged domain is wanted, the STEP domain conversions.
    pub fn cover_other_coincident_surface_deferring_trims(
        &mut self,
        k: &Kernel,
        other: &mut Self,
        tol: f64,
        this_domain: Option<&Extent2d>,
        other_domain: Option<&Extent2d>,
        want_merged: bool,
    ) -> Result<CoverOtherCoincident> {
        self.cover_other_coincident_surface_in::<NoObject>(
            k,
            other,
            tol,
            this_domain,
            other_domain,
            want_merged,
            None,
        )
    }

    /// [`Surface::cover_other_coincident_surface_deferring_trims`] whose
    /// `CoverCoincidentSurface` calls run their `STEPInversion` solves under `context`,
    /// so a face-owned covering surface's drops are classified against its face's trims
    /// as upstream's `GlobalPointSolve` does through `GetFace()`.
    ///
    /// # Errors
    ///
    /// As [`Surface::cover_other_coincident_surface_deferring_trims`], with those of
    /// the context.
    #[allow(clippy::too_many_arguments)] // upstream's arguments plus the context
    pub fn cover_other_coincident_surface_deferring_trims_with_context<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        other: &mut Self,
        tol: f64,
        this_domain: Option<&Extent2d>,
        other_domain: Option<&Extent2d>,
        want_merged: bool,
        context: CoverSolveContext<'_, O>,
    ) -> Result<CoverOtherCoincident> {
        self.cover_other_coincident_surface_in(
            k,
            other,
            tol,
            this_domain,
            other_domain,
            want_merged,
            Some(context),
        )
    }

    #[allow(clippy::too_many_arguments)] // upstream's arguments plus the trim deferral
    fn cover_other_coincident_surface_in<O: SolutionObject>(
        &mut self,
        k: &Kernel,
        other: &mut Self,
        tol: f64,
        this_domain: Option<&Extent2d>,
        other_domain: Option<&Extent2d>,
        want_merged: bool,
        mut context: Option<CoverSolveContext<'_, O>>,
    ) -> Result<CoverOtherCoincident> {
        let mut st = CoverState {
            domain0: match this_domain {
                Some(d) => *d,
                None => self.natural_uv_domain_with_nurb(k)?,
            },
            domain1: match other_domain {
                Some(d) => *d,
                None => other.natural_uv_domain_with_nurb(k)?,
            },
            max_dist: BIG_DOUBLE,
            merged: None,
            trims: Vec::new(),
        };
        let done = |st: &mut CoverState, covering_is_other: bool| CoverOtherCoincident {
            covering_is_other: Some(covering_is_other),
            max_dist: st.max_dist,
            merged_domain: st.merged,
            remove_uv_trim_curves_of: core::mem::take(&mut st.trims),
        };

        let is_plane = |s: &Self| matches!(s.kind(), SurfaceKind::Plane(_));
        // SM_CAST_PTR(SmBSplineSurface, ..): every kind derived from it (a plane, tried
        // before, a cone, sphere, torus or surface of revolution or extrusion)
        let is_bspline = |s: &Self| s.is_kind_of(B_SPLINE_SURFACE_TYPE);
        let mut tried0 = false;
        let mut tried1 = false;

        // Favor SmPlanes:
        if is_plane(self) {
            tried0 = true;
            if try_cover(
                k,
                &mut st,
                self,
                other,
                false,
                tol,
                want_merged,
                &mut context,
            )? {
                return Ok(done(&mut st, false));
            }
        }
        if is_plane(other) {
            tried1 = true;
            if try_cover(
                k,
                &mut st,
                self,
                other,
                true,
                tol,
                want_merged,
                &mut context,
            )? {
                return Ok(done(&mut st, true));
            }
        }

        // Next look for bilinear.
        if !tried0 && self.is_bilinear(k) {
            tried0 = true;
            if try_cover(
                k,
                &mut st,
                self,
                other,
                false,
                tol,
                want_merged,
                &mut context,
            )? {
                return Ok(done(&mut st, false));
            }
        }
        if !tried1 && other.is_bilinear(k) {
            tried1 = true;
            if try_cover(
                k,
                &mut st,
                self,
                other,
                true,
                tol,
                want_merged,
                &mut context,
            )? {
                return Ok(done(&mut st, true));
            }
        }

        // Next, favor SmBSplineSurfaces.
        if !tried0 && is_bspline(self)? {
            tried0 = true;
            if try_cover(
                k,
                &mut st,
                self,
                other,
                false,
                tol,
                want_merged,
                &mut context,
            )? {
                return Ok(done(&mut st, false));
            }
        }
        if !tried1 && is_bspline(other)? {
            tried1 = true;
            if try_cover(
                k,
                &mut st,
                self,
                other,
                true,
                tol,
                want_merged,
                &mut context,
            )? {
                return Ok(done(&mut st, true));
            }
        }

        // No special cases.
        if !tried0 {
            tried0 = true;
            if try_cover(
                k,
                &mut st,
                self,
                other,
                false,
                tol,
                want_merged,
                &mut context,
            )? {
                return Ok(done(&mut st, false));
            }
        }
        let _ = tried0;
        if !tried1 {
            tried1 = true;
            if try_cover(
                k,
                &mut st,
                self,
                other,
                true,
                tol,
                want_merged,
                &mut context,
            )? {
                return Ok(done(&mut st, true));
            }
        }
        let _ = tried1;

        Ok(CoverOtherCoincident {
            covering_is_other: None,
            max_dist: st.max_dist,
            merged_domain: st.merged,
            remove_uv_trim_curves_of: core::mem::take(&mut st.trims),
        })
    }
}

// ---- sm_SpLocalSolve ----------------------------------------------------------------------------

/// The surface/point local solve of one Bezier patch (`sm_SpLocalSolve`): the
/// closest/furthest surface point near a guess, normals, or points within `tolerance`,
/// then the same on the boundary curves of discontinuous patch edges.
///
/// `surface` is what `sm_GetSurfaceToEvaluate(node, patch surface)` returns (unit
/// `surface.surface_p1`); `u_conts` and `v_conts` are the patch's continuity with its
/// neighbours at the lower and upper ends.
///
/// # Errors
///
/// Those of `sm_LoadGuesses`, the iso curve solves and the boundary tests;
/// [`SolidError::Unported`] from the local point solve.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // upstream's list
pub fn sp_local_solve<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    u_conts: [ContinuityType; 2],
    v_conts: [ContinuityType; 2],
    point: Point3,
    tolerance: f64,
    op: SolverOperation,
    solve_for_surface: bool,
    process_boundary_curves: bool,
) -> Result<SolutionArray<O>> {
    if !matches!(
        op,
        SolverOperation::Minimize
            | SolverOperation::Maximize
            | SolverOperation::Normalize
            | SolverOperation::Intersect
    ) {
        return Err(SolidError::invalid_input(
            OP_SP_LOCAL,
            "unsupported solver operation",
        ));
    }

    // init output
    let mut solutions = SolutionArray::<O>::new();

    // when asked to find Surface/Point solutions
    if solve_for_surface {
        // Find a guess point for LocalPointSolve.
        let guess = load_guesses(k, surface, uv_domain, point, op)?;

        // for every guess point (there is one)
        // Find NewtonRaphson solution for closest surf point to crPoint
        if let Some(r) = ignored(surface.local_point_solve::<O>(k, uv_domain, op, point, guess))? {
            let mut found = r.found;
            let mut solution = r.solution;

            // skip intersection solutions larger than d3dTolerance
            if found
                && op == SolverOperation::Intersect
                && solution.start.solution_value > tolerance
            {
                found = false;
            }

            if found {
                let mut keep = true;
                // When solution is exactly on domain boundary
                let uv = Point2::new(solution.start.param(0)?, solution.start.param(1)?);
                if uv_domain.is_point_on_boundary(uv, EFF_ZERO).0 {
                    // skip Solution Points which are not exactly (to within ScaledZero) on
                    // the SurfPoint's SurfaceNormal Line
                    if !super::p4::pass_sp_convergence_test(k, surface, uv, point, tolerance)? {
                        keep = false;
                    }
                }

                if keep {
                    // keep solution - set output
                    solution.num_variables = 2;
                    solutions.add(&solution);

                    // If intersecting we need only one intersection
                    if op == SolverOperation::Intersect || op == SolverOperation::Minimize {
                        return Ok(solutions);
                    }
                }
            }
        }
    }

    // arive here after skipping or adding all surfacePoint solutions

    // when looking for boundaryCurve/Point solutions
    if process_boundary_curves && op != SolverOperation::Normalize {
        // Iter lower and upper boundaries
        for kk in 0..2_usize {
            for dir in [SurfParamType::U, SurfParamType::V] {
                let conts = if dir == SurfParamType::U {
                    &u_conts
                } else {
                    &v_conts
                };
                let c = conts
                    .get(kk)
                    .copied()
                    .ok_or(SolidError::invalid_input(OP_SP_LOCAL, "continuity array"))?;

                //    When lower Boundary C0 or discontinuous
                // or when upper Boundary discontinuous
                let wanted = (kk == 0 && ct(c) <= ct(ContinuityType::C0))
                    || (kk == 1 && c == ContinuityType::Discontinuous);
                if !wanted {
                    continue;
                }

                // Build boundary isoParameter curve
                let (param_value, ivl, from_left) = if dir == SurfParamType::U {
                    let v = if kk == 0 {
                        uv_domain.min().x
                    } else {
                        uv_domain.max().x
                    };
                    (
                        v,
                        Extent1d::from_min_max(uv_domain.min().y, uv_domain.max().y),
                        kk == 0,
                    )
                } else {
                    let v = if kk == 0 {
                        uv_domain.min().y
                    } else {
                        uv_domain.max().y
                    };
                    (
                        v,
                        Extent1d::from_min_max(uv_domain.min().x, uv_domain.max().x),
                        kk == 0,
                    )
                };
                let iso = Curve::from(iso_curve_of(k, surface, dir, param_value, from_left)?);

                // for NonDegenerate Boundary Curves
                if !iso.is_degenerate(k, EFF_ZERO, None)? {
                    // look for a Curve/Point solution
                    let (mut found, mut solution) = iso.local_point_solve::<O>(
                        k,
                        &ivl,
                        op,
                        point,
                        &PointSolveOptions::default(),
                        ivl.evaluate(0.5),
                    )?;

                    // skip out of tolerance intersection solutions
                    if found
                        && op == SolverOperation::Intersect
                        && solution.start.solution_value > tolerance
                    {
                        found = false;
                    }

                    // add solutions to output
                    if found {
                        let t = solution.start.param(0)?;
                        if dir == SurfParamType::U {
                            *solution.start.param_mut(1)? = t;
                            *solution.start.param_mut(0)? = param_value;
                        } else {
                            *solution.start.param_mut(1)? = param_value;
                        }
                        solution.num_variables = 2;
                        solutions.add(&solution);
                    }
                }
            }
        }
    }

    Ok(solutions)
}

// ---- sm_FilterSolution --------------------------------------------------------------------------

/// `SmSurface::EvaluateNormalSection(uv, TRUE, TRUE, direction, ..)`: the curvature of the
/// normal section in `direction` and the unit surface normal.
fn surface_evaluate_normal_section(
    k: &Kernel,
    surface: &Surface,
    uv: Point2,
    direction: Vec3,
) -> Result<(f64, Vec3)> {
    let r = surface.evaluate_normal_section(k, uv, Side::Left, Side::Left, direction)?;
    Ok((r.curvature, r.normal))
}

/// Whether to keep a curve/surface solution found at the end of a subdivided curve
/// interval or surface domain (`sm_FilterSolution`): solutions good to zero or to a
/// scaled zero are kept, so are tangent solutions that are minima of the gap, and
/// solutions on discontinuity boundaries whose gap points out of the shape domain; the
/// others are expected from the neighbouring branch.
///
/// # Errors
///
/// Those of the evaluations and of unitizing the surface normal.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // upstream's list
pub fn filter_solution<O: SolutionObject>(
    k: &Kernel,
    surface: &Surface,
    uv_domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    u_conts: [ContinuityType; 2],
    v_conts: [ContinuityType; 2],
    t_conts: [ContinuityType; 2],
    solution: &Solution<O>,
) -> Result<bool> {
    // keep answers good to Zero
    if solution.start.solution_value < EFF_ZERO {
        return Ok(true);
    }

    // get solution CurvePoint and a random NearMid SurfacePoint
    let curve_pv = curve.evaluate(k, solution.start.param(0)?, 1, true, true)?;
    let curve_point = *curve_pv
        .first()
        .ok_or(SolidError::invalid_input(OP_FILTER, "curve evaluation"))?;
    let curve_tangent = *curve_pv
        .get(1)
        .ok_or(SolidError::invalid_input(OP_FILTER, "curve evaluation"))?;
    let random_point = surface.evaluate_point(k, uv_domain.evaluate(0.456, 0.6789))?;

    // Scale Zero to current size.
    let scaled_zero =
        10.0 * EFF_ZERO * (1.0 + max2(curve_point.max_dimension(), random_point.max_dimension()));

    // keep answers good to scaledZero
    if solution.start.solution_value < scaled_zero {
        return Ok(true);
    }

    // arrive here when we have an inexact answer (only found at interval discontinuities)

    // get solution SurfacePoint: sUV, sSurfacePoint, sDU, sDV, and sNorm
    let uv = Point2::new(solution.start.param(1)?, solution.start.param(2)?);
    let (surface_point, du, dv) = evaluate_1st_derivatives(k, surface, uv, Side::Left, Side::Left)?;
    let mut norm = du.cross(dv);
    if norm.length_squared() < EFF_ZERO_SQ {
        norm = surface.evaluate_normal(k, uv, Side::Left, Side::Left)?;
    }
    let norm = norm.unitize()?;

    // get SurfacePoint to CurvePoint Gap Vector (the error)
    let diff = curve_point - surface_point;

    // see if solution lies on curve end-point boundaries
    let c0_curve_min = are_same(interval.min(), solution.start.param(0)?);
    let c0_curve_max = are_same(interval.max(), solution.start.param(0)?);

    // Keep Solutions whose gaps are within 5 degrees of parallel to the SurfaceNormal and
    // within 5 degrees of perpendicular to the CurveTangent: a tangent closest point
    if norm.is_parallel_to(diff, 5.0) && curve_tangent.is_perpendicular_to(diff, 5.0) {
        // when the curve is continuous through this solution
        let g1g2 = ct(ContinuityType::G1G2);
        if (c0_curve_min && ct(t_conts[0]) >= g1g2) || (c0_curve_max && ct(t_conts[1]) >= g1g2) {
            // get the surface's normal section evaluation (upstream ignores the status;
            // outputs keep their default-constructed values)
            let (surface_curvature, surface_normal) = ignored(surface_evaluate_normal_section(
                k,
                surface,
                uv,
                curve_tangent,
            ))?
            .unwrap_or((0.0, Vec3::UNDEF));

            // compute the curve's position, tangent, curvature, and normal values (status
            // ignored: the frame keeps its default-constructed vectors)
            let frenet = ignored(curve_evaluate_geometric(
                k,
                curve,
                solution.start.param(0)?,
                2,
                true,
            ))?
            .unwrap_or_else(|| vec![Vec3::UNDEF; 3]);
            let curv_vec = frenet.get(2).copied().unwrap_or(Vec3::UNDEF);

            // look for lines
            let mut curve_curvature = curv_vec.length();
            let is_curve_line = curve_curvature.abs() < EFF_ZERO_SQRT;
            let is_surface_line = surface_curvature.abs() < EFF_ZERO_SQRT;

            // handle line cases now
            if is_curve_line && is_surface_line {
                return Ok(true);
            } else if is_curve_line {
                // keep solutions when surf is bending away from curve
                let bending_toward = (surface_curvature * diff.dot(surface_normal)) > 0.0;
                return Ok(!bending_toward);
            } else if is_surface_line {
                // keep solutions when curve is bending away from surface
                return Ok(diff.dot(curv_vec) > 0.0);
            }

            // arrive here when both SurfaceNormalSection and Curve are curving at the
            // solution: the signed curvature of the curve projected to the section plane
            curve_curvature = curv_vec.dot(surface_normal);

            let pos_g_dot_n = diff.dot(surface_normal) > 0.0;
            let pos_ks = surface_curvature > 0.0;
            let pos_kc = curve_curvature > 0.0;
            let kc_bigger_than_ks = curve_curvature.abs() > surface_curvature.abs();

            // (upstream leaves rbKeepSolution at TRUE when no case matches; none can
            // fail to)
            let mut keep = true;
            if (!pos_g_dot_n && pos_ks && pos_kc) || (pos_g_dot_n && !pos_ks && !pos_kc) {
                keep = !kc_bigger_than_ks;
            } else if (!pos_g_dot_n && pos_ks && !pos_kc) || (pos_g_dot_n && !pos_ks && pos_kc) {
                keep = true;
            } else if (!pos_g_dot_n && !pos_ks && pos_kc) || (pos_g_dot_n && pos_ks && !pos_kc) {
                keep = false;
            } else if (!pos_g_dot_n && !pos_ks && !pos_kc) || (pos_g_dot_n && pos_ks && pos_kc) {
                keep = kc_bigger_than_ks;
            }
            return Ok(keep);
        }

        return Ok(true);
    }

    // If here it is not a tangent Curve/Surface closest point solution case. The curve
    // has come within tolerance distance of a boundary.

    // see if solution lies on surface curve-boundaries
    let c0_surf_u_min = are_same(uv_domain.min().x, solution.start.param(1)?);
    let c0_surf_u_max = are_same(uv_domain.max().x, solution.start.param(1)?);
    let c0_surf_v_min = are_same(uv_domain.min().y, solution.start.param(2)?);
    let c0_surf_v_max = are_same(uv_domain.max().y, solution.start.param(2)?);

    // see which way the gap vector points relative to the curve and surface tangents
    let curve_dot_diff = curve_tangent.dot(diff);
    let surf_u_dot_diff = diff.dot(du);
    let surf_v_dot_diff = diff.dot(dv);

    let c0 = ct(ContinuityType::C0);
    let mut curve_save = (ct(t_conts[0]) <= c0 && c0_curve_min && curve_dot_diff > 0.0)
        || (ct(t_conts[1]) <= c0 && c0_curve_max && curve_dot_diff < 0.0);
    let mut surf_u_save = (ct(u_conts[0]) <= c0 && c0_surf_u_min && surf_u_dot_diff < 0.0)
        || (ct(u_conts[1]) <= c0 && c0_surf_u_max && surf_u_dot_diff > 0.0);
    let mut surf_v_save = (ct(v_conts[0]) <= c0 && c0_surf_v_min && surf_v_dot_diff < 0.0)
        || (ct(v_conts[1]) <= c0 && c0_surf_v_max && surf_v_dot_diff > 0.0);

    // pass any boundary solution that works on a boundary
    let mut keep = curve_save || surf_u_save || surf_v_save;

    // Ok, now check seams, if we have to.
    if keep && curve_save {
        // Can't use passed-in interval, it's just a cache subset.
        let crv_ivl = curve.natural_interval();
        if curve.is_closed(k, &crv_ivl, 0.0)? {
            curve_save = false;
        }
        keep = curve_save || surf_u_save || surf_v_save;
    }
    // Can't use passed-in interval, it's just a cache subset.
    let srf_domain = surface.natural_uv_domain_with_nurb(k)?;
    if keep && surf_u_save {
        if surface
            .is_closed(k, &srf_domain, SurfParamType::U, None, false)?
            .0
        {
            surf_u_save = false;
        }
        keep = curve_save || surf_u_save || surf_v_save;
    }
    if keep && surf_v_save {
        if surface
            .is_closed(k, &srf_domain, SurfParamType::V, None, false)?
            .0
        {
            surf_v_save = false;
        }
        keep = curve_save || surf_u_save || surf_v_save;
    }

    Ok(keep)
}

// ---- IdenticalSolutions of the surface global solvers ---------------------------------------------

/// The shared body of the four `IdenticalSolutions` overrides: the parent's verdict, then
/// the singularity/seam rules for solutions whose parameters differ.
///
/// `uv_at` is the index of `u` in the solutions' parameters (`1` after a curve or line
/// parameter, `0` for a point/surface solve) and `require_single` the extra
/// `SM_ST_SINGLE_VALUE` condition of the curve/surface solvers.
#[allow(clippy::too_many_arguments)]
fn surface_identical_solutions<O: SolutionObject>(
    k: &Kernel,
    base: &GlobalSolver<'_, O>,
    surface: Option<&Surface>,
    sol1: &Solution<O>,
    sol2: &Solution<O>,
    uv_at: usize,
    require_single: bool,
    domains: &dyn ParameterDomains<O>,
) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
    // If the parent thinks they're identical, then they are.
    let (pair, preferred) = base.identical_solutions_in(sol1, sol2, domains)?;
    if pair == SolutionPairType::Identical {
        return Ok((pair, preferred));
    }

    if pair == SolutionPairType::DiffParameters
        && (!require_single
            || (sol1.solution_type == SolutionType::SingleValue
                && sol2.solution_type == SolutionType::SingleValue))
    {
        let surf = surface.ok_or(SolidError::null_pointer(OP_SING))?;

        // At a singularity, the same 3d point can have different uv values.
        let uv1 = Point2::new(sol1.start.param(uv_at)?, sol1.start.param(uv_at + 1)?);
        let uv2 = Point2::new(sol2.start.param(uv_at)?, sol2.start.param(uv_at + 1)?);

        // Check positions (statuses ignored).
        let pt1 = ignored(surf.evaluate_point(k, uv1))?.unwrap_or(Vec3::UNDEF);
        let pt2 = ignored(surf.evaluate_point(k, uv2))?.unwrap_or(Vec3::UNDEF);

        let tol = base.tolerance_3d; // untested: see how it works in practice
        if !pt1.closer_than(tol, pt2) {
            return Ok((SolutionPairType::DiffParameters, None));
        }

        // 3d points are the same. Need to check seams and poles. Check poles first,
        // it's a cheaper check. If either is nonsingular, they're distinct.
        if !surf.is_singularity(k, uv1, EFF_ZERO, false)?.singular {
            return Ok((SolutionPairType::DiffParameters, None));
        }
        if !surf.is_singularity(k, uv2, EFF_ZERO, false)?.singular {
            return Ok((SolutionPairType::DiffParameters, None));
        }

        // We're at a pole. Distinct only if they're both on a seam. When one is on a seam
        // prefer that. Else prefer the best solution.
        let on_seam1 = surf.is_on_seam(k, uv1, None)?.on_seam;
        let on_seam2 = surf.is_on_seam(k, uv2, None)?.on_seam;
        if on_seam1 && !on_seam2 {
            return Ok((SolutionPairType::Identical, Some(PreferredSolution::First)));
        }
        if !on_seam1 && on_seam2 {
            return Ok((SolutionPairType::Identical, Some(PreferredSolution::Second)));
        }
        if !on_seam1 && !on_seam2 {
            let pick = if sol1.start.solution_value < sol2.start.solution_value {
                PreferredSolution::First
            } else {
                PreferredSolution::Second
            };
            return Ok((SolutionPairType::Identical, Some(pick)));
        }
    }

    // Identical positions, at a pole on a seam: call them distinct points.
    Ok((pair, preferred))
}

/// `SmCSGlobalSolver::IdenticalSolutions`: solutions are `[t, u, v]` of a curve and
/// `surface`.
///
/// # Errors
///
/// Those of the parent and of the singularity and seam tests.
pub fn cs_identical_solutions<O: SolutionObject>(
    k: &Kernel,
    base: &GlobalSolver<'_, O>,
    surface: Option<&Surface>,
    sol1: &Solution<O>,
    sol2: &Solution<O>,
) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
    cs_identical_solutions_in(k, base, surface, sol1, sol2, &OwnDomains)
}

/// [`cs_identical_solutions`] reading the parameter intervals of the solutions'
/// objects through `domains` (the solve's operands, which upstream's objects are).
///
/// # Errors
///
/// As [`cs_identical_solutions`].
pub fn cs_identical_solutions_in<O: SolutionObject>(
    k: &Kernel,
    base: &GlobalSolver<'_, O>,
    surface: Option<&Surface>,
    sol1: &Solution<O>,
    sol2: &Solution<O>,
    domains: &dyn ParameterDomains<O>,
) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
    surface_identical_solutions(k, base, surface, sol1, sol2, 1, true, domains)
}

pub(super) fn curve_operand<O: SolutionObject>(
    base: &GlobalSolver<'_, O>,
    slot: usize,
    curve: Option<&Curve>,
) -> Option<(O, super::p1::OperandDomain)> {
    base.tree(slot)
        .ok()
        .and_then(|t| t.owner_object().ok().flatten())
        .zip(curve)
        .map(|(h, c)| (h, super::p1::OperandDomain::Curve(c.natural_interval())))
}

pub(super) fn surface_operand<O: SolutionObject>(
    base: &GlobalSolver<'_, O>,
    slot: usize,
    cache: Option<&crate::surface::surface_cache::SurfaceCache<O>>,
    surface: Option<&Surface>,
) -> Option<(O, super::p1::OperandDomain)> {
    let handle = base
        .tree(slot)
        .ok()
        .and_then(|t| t.owner_object().ok().flatten())
        .or_else(|| cache.and_then(crate::surface::surface_cache::SurfaceCache::surface_handle));
    handle.zip(surface).map(|(h, s)| {
        (
            h,
            // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
            super::p1::OperandDomain::Surface(s.natural_uv_domain_if_built()),
        )
    })
}

/// `SmCSIGlobalSolver::IdenticalSolutions`: as [`cs_identical_solutions`].
///
/// # Errors
///
/// Those of the parent and of the singularity and seam tests.
pub fn csi_identical_solutions<O: SolutionObject>(
    k: &Kernel,
    base: &GlobalSolver<'_, O>,
    surface: Option<&Surface>,
    sol1: &Solution<O>,
    sol2: &Solution<O>,
) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
    csi_identical_solutions_in(k, base, surface, sol1, sol2, &OwnDomains)
}

/// [`csi_identical_solutions`] reading the parameter intervals of the solutions'
/// objects through `domains` (the solve's operands, which upstream's objects are).
///
/// # Errors
///
/// As [`csi_identical_solutions`].
pub fn csi_identical_solutions_in<O: SolutionObject>(
    k: &Kernel,
    base: &GlobalSolver<'_, O>,
    surface: Option<&Surface>,
    sol1: &Solution<O>,
    sol2: &Solution<O>,
    domains: &dyn ParameterDomains<O>,
) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
    surface_identical_solutions(k, base, surface, sol1, sol2, 1, true, domains)
}

impl<O: SolutionObject> LsiGlobalSolver<'_, '_, O> {
    /// `SmLSIGlobalSolver::IdenticalSolutions`: solutions are `[line t, u, v]`.
    ///
    /// # Errors
    ///
    /// Those of the parent and of the singularity and seam tests.
    pub fn surface_identical_solutions(
        &self,
        k: &Kernel,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        let domains = super::p1::SolveOperands {
            operands: [
                surface_operand(&self.base, 0, self.surface_cache, self.surface),
                None,
            ],
        };
        surface_identical_solutions(k, &self.base, self.surface, sol1, sol2, 1, true, &domains)
    }
}

impl<O: SolutionObject> SpGlobalSolver<'_, '_, O> {
    /// `SmSPGlobalSolver::IdenticalSolutions`: solutions are `[u, v]`.
    ///
    /// # Errors
    ///
    /// Those of the parent and of the singularity and seam tests.
    pub fn surface_identical_solutions(
        &self,
        k: &Kernel,
        sol1: &Solution<O>,
        sol2: &Solution<O>,
    ) -> Result<(SolutionPairType, Option<PreferredSolution>)> {
        let domains = super::p1::SolveOperands {
            operands: [
                surface_operand(&self.base, 1, self.surface_cache, self.surface),
                None,
            ],
        };
        surface_identical_solutions(k, &self.base, self.surface, sol1, sol2, 0, false, &domains)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::Line;
    use crate::geom::solution_array::ObjectKind;
    use crate::surface::plane::Plane;

    /// A solution object for the tests.
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Obj;

    impl SolutionObject for Obj {
        fn kind(&self) -> ObjectKind {
            ObjectKind::Surface
        }
    }

    fn xy_plane() -> Surface {
        Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0)))
    }

    #[test]
    fn plane_has_no_singularity() {
        let s = xy_plane();
        let r = s
            .is_singularity(&Kernel::default(), Point2::new(0.5, 0.5), EFF_ZERO, false)
            .unwrap();
        assert!(!r.singular);
        assert_eq!(r.direction, SurfParamType::Neither);
    }

    #[test]
    fn normal_off_a_singularity_is_the_surface_normal() {
        let s = xy_plane();
        let n = s
            .evaluate_normal_at_singularity(&Kernel::default(), Point2::new(0.25, 0.75), EFF_ZERO)
            .unwrap();
        assert_eq!(n, Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn intersect_drop_converges_on_a_plane() {
        let k = &Kernel::default();
        let s = xy_plane();
        let dom = s.natural_uv_domain_if_built();
        let p = Vec3::new(0.3, 0.4, 2.0);
        let r = s
            .local_point_solve_0::<Obj>(
                k,
                &dom,
                SolverOperation::Intersect,
                p,
                Point2::new(0.0, 0.0),
            )
            .unwrap();
        assert!(r.found);
        assert!((r.solution.start.solution_value - 2.0).abs() < 1e-9);
        assert_eq!(r.reason, TerminationReason::FoundAnswerConverged);
    }

    #[test]
    fn new_point_solver_projects_to_the_plane() {
        let s = xy_plane();
        let dom = s.natural_uv_domain_if_built();
        let r = s.local_point_solve::<Obj>(
            &Kernel::default(),
            &dom,
            SolverOperation::Minimize,
            Vec3::new(0.0, 0.0, 1.0),
            Point2::new(0.0, 0.0),
        );
        let result = r.unwrap();
        assert!(result.found);
        assert_eq!(result.solution.solution_type, SolutionType::SingleValue);
        assert_eq!(result.solution.start.parameters[0], 0.0);
        assert_eq!(result.solution.start.parameters[1], 0.0);
        assert_eq!(result.solution.start.solution_value, 1.0);
        // a maximise solve runs LocalPointSolve_0, whose Newton solve is not ported
        let r = s.local_point_solve::<Obj>(
            &Kernel::default(),
            &dom,
            SolverOperation::Maximize,
            Vec3::new(0.0, 0.0, 1.0),
            Point2::new(0.0, 0.0),
        );
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn drop_point_uses_the_fast_drop_of_a_plane() {
        let k = &Kernel::default();
        let s = xy_plane();
        let dom = s.natural_uv_domain_if_built();
        let c = dom.evaluate(0.5, 0.5);
        let p = s.evaluate_point(k, c).unwrap() + Vec3::new(0.0, 0.0, 1.5);
        let opts = DropPointOptions {
            operation: SolverOperation::Normalize,
            ..DropPointOptions::default()
        };
        let r = s
            .drop_point::<Obj>(&Kernel::default(), p, &dom, &opts)
            .unwrap();
        assert!(r.success);
        assert!((r.gap - 1.5).abs() < 1e-9);
        assert!((r.result_uv.x - c.x).abs() < 1e-9 && (r.result_uv.y - c.y).abs() < 1e-9);
    }

    #[test]
    fn filter_keeps_exact_solutions() {
        let k = &Kernel::default();
        let s = xy_plane();
        let dom = s.natural_uv_domain_if_built();
        let line =
            Line::create_line_segment(3, Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), None)
                .unwrap();
        let curve = Curve::from(line);
        let mut sol = Solution::<Obj>::new();
        sol.solution_type = SolutionType::SingleValue;
        sol.num_variables = 3;
        sol.start.solution_value = 0.0;
        let conts = [ContinuityType::C0; 2];
        let keep = filter_solution(
            k,
            &s,
            &dom,
            &curve,
            &Extent1d::from_min_max(0.0, 1.0),
            conts,
            conts,
            conts,
            &sol,
        )
        .unwrap();
        assert!(keep);
    }

    /// A context that lends no face: every cache is the surface's own plain one.
    struct PlainCaches;

    impl SurfaceSolveContext<NoObject> for PlainCaches {
        fn build_surface_cache(
            &mut self,
            k: &Kernel,
            surface: &Surface,
            handle: Option<NoObject>,
        ) -> Result<crate::surface::surface_cache::p1::SurfaceCache<NoObject>> {
            get_surface_cache(k, surface, handle)
        }

        fn face_queries(
            &self,
        ) -> Option<&dyn crate::surface::surface_cache::face_context::FaceQueries<NoObject>>
        {
            None
        }
    }

    /// Without faces, the context path of `CoverOtherCoincidentSurface` leaves the
    /// same results and the same (possibly expanded) surfaces as the plain path.
    #[test]
    fn cover_with_context_matches_the_plain_path_without_faces() {
        type Case = (fn() -> Surface, Option<Extent2d>);
        let k = Kernel::default();
        let tilted = || Surface::from(Plane::from_normal(Vec3::ZERO, Vec3::new(0.0, 1e-3, 1.0)));
        let sub = Extent2d::from_points(Point2::new(0.25, 0.25), Point2::new(0.5, 0.75));
        let pairs: [Case; 3] = [(xy_plane, None), (xy_plane, Some(sub)), (tilted, None)];
        for (other_of, this_domain) in pairs {
            for want_merged in [false, true] {
                let (mut a0, mut b0) = (xy_plane(), other_of());
                let plain = a0.cover_other_coincident_surface_deferring_trims(
                    &k,
                    &mut b0,
                    1e-6,
                    this_domain.as_ref(),
                    None,
                    want_merged,
                );
                let (mut a1, mut b1) = (xy_plane(), other_of());
                let mut caches = PlainCaches;
                let with = a1.cover_other_coincident_surface_deferring_trims_with_context(
                    &k,
                    &mut b1,
                    1e-6,
                    this_domain.as_ref(),
                    None,
                    want_merged,
                    CoverSolveContext {
                        context: &mut caches,
                        handles: [None, None],
                    },
                );
                match (plain, with) {
                    (Ok(p), Ok(w)) => {
                        assert_eq!(p.covering_is_other, w.covering_is_other);
                        assert_eq!(p.max_dist.to_bits(), w.max_dist.to_bits());
                        assert_eq!(p.merged_domain, w.merged_domain);
                        assert_eq!(p.remove_uv_trim_curves_of, w.remove_uv_trim_curves_of);
                    }
                    (Err(p), Err(w)) => assert_eq!(p, w),
                    (p, w) => panic!("plain {p:?} vs context {w:?}"),
                }
                assert_eq!(
                    a0.natural_uv_domain_if_built(),
                    a1.natural_uv_domain_if_built()
                );
                assert_eq!(
                    b0.natural_uv_domain_if_built(),
                    b1.natural_uv_domain_if_built()
                );
            }
        }
    }

    #[test]
    fn a_plane_covers_an_identical_plane() {
        let mut a = xy_plane();
        let mut b = xy_plane();
        let r = a
            .cover_other_coincident_surface_deferring_trims(
                &Kernel::default(),
                &mut b,
                1e-6,
                None,
                None,
                false,
            )
            .unwrap();
        assert_eq!(r.covering_is_other, Some(false));
        assert!(r.max_dist <= 1e-6);
    }
}
