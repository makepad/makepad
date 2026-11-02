//! `SmSurfOfRevolution`, part 3: copying, reparameterisation, the STEP to NURB
//! parameter conversion, the fast point drop, the intersections with other surfaces
//! of revolution (and cones and spheres) about the same axis, the rebuild of the
//! analytic data from a modified NURB, the transform, the analytic domain update
//! after a NURB trim and the database reader.
//!
//! Upstream: `source/SMLib/src/SmSurfOfRevolution.cpp`,
//! `source/SMLib/inc/SmSurfOfRevolution.h`.
//!
//! | Rust                                                     | Upstream                                     |
//! |----------------------------------------------------------|----------------------------------------------|
//! | [`SurfOfRevolution::copy`]                               | `Copy`                                       |
//! | [`SurfOfRevolution::reparameterize`]                     | `Reparameterize`                             |
//! | [`SurfOfRevolution::make_nurb_with_sweep_params`]        | `MakeNurbWithSweepParams` (co-ported in `p1`) |
//! | [`SurfOfRevolution::convert_uv_from_step_to_nurbs`]      | `ConvertUVFromSTEPToNURBS`                   |
//! | [`SurfOfRevolution::drop_point_fast`]                    | `DropPointFast`                              |
//! | [`SurfOfRevolution::intersect_with_cone`]                | `IntersectWithCone`                          |
//! | [`SurfOfRevolution::intersect_with_sphere`]              | `IntersectWithSphere`                        |
//! | [`SurfOfRevolution::intersect_with_surf_of_revolution`], [`Revolved`] | `IntersectWithSurfOfRevolution` |
//! | [`SurfOfRevolution::rebuild_step_from_nurb_parameters`]  | `RebuildSTEPFromNURBParameters`              |
//! | [`SurfOfRevolution::transform`]                          | `Transform`                                  |
//! | [`SurfOfRevolution::update_analytical_domain`]           | `UpdateAnalyticalDomain`                     |
//! | [`SurfOfRevolution::read_from_db`]                       | `ReadFromDB`                                 |
//! | [`SurfOfRevolution::convert_domain_from_nurbs_to_step`]  | `SmSurface::ConvertDomainFromNURBSToSTEP` (body in `surface::surface::p1`) |
//!
//! Deviations:
//! - **Arguments.** Out-params become return values; `SmContext` arguments, the
//!   arguments upstream marks `NotUsed` (`IntersectWithSurfOfRevolution`'s edge flags
//!   and angle tolerance) and the `Notify` calls are dropped. The intersections return
//!   [`SurfaceIntersection`] (the null UV curves upstream appends are `None`) and
//!   whether the general intersector is needed. `DropPointFast` names the surface in
//!   its solutions by the caller's handle `this` (upstream: the `this` pointer).
//! - **Cones and spheres.** `SmCone` and `SmSphere` derive from `SmSurfOfRevolution`
//!   and are not ported yet; the intersections take the other surface through the
//!   [`Revolved`] trait (its `SmSurfOfRevolution` part and its virtual
//!   `ConvertDomainFromNURBSToSTEP`), which those kinds implement when ported.
//! - **Virtual iso curves.** The polar-converter rebuilds reach upstream's virtual
//!   `CreateIsoParametricCurve` through a `SweepIsoFn` (as in `p1`): `Transform` has
//!   a crate-private `transform_using` that a derived class (`SmTorus`) calls with
//!   its override.
//! - **Generator owner.** As in `p1`, the generator has no owner: its owner-driven
//!   reparameterisation (`AdjustSTEPInterval` of an analytic generator making its NURB
//!   on `GetGenDirParamExtent`) is not applied in `UpdateAnalyticalDomain`, and
//!   `EditParameterization` of an ellipse generator gets no owner.
//! - **Statuses.** Calls whose status upstream ignores keep upstream's values, except
//!   that [`SolidError::Unported`] always propagates and a failure whose output
//!   upstream would read uninitialised (a failed generator evaluation, an empty
//!   reference frame) is returned. `ConvertUVFromSTEPToNURBS` returns the errors of
//!   the calls upstream checks with `SER` (upstream then leaves its output at the
//!   STEP input).
//! - **`ReadFromDB`** always builds a new surface (the "fill the surface passed in"
//!   branch has no counterpart; the unused type argument is dropped). An ASCII value
//!   whose extraction fails at the stream sentry (upstream: uninitialised) is
//!   [`SolidError::InvalidInput`], as is a generator not derived from
//!   `SmBSplineCurve` (upstream casts it). The converter's stored sweep circle is read
//!   with [`PolarConversion::read_from_db_with_curve`]. The reader counts one nesting
//!   level ([`DatabaseReader::enter_nested`]).
//! - **Unported callees.** `SmSurface::LocalPointSolve` (unit `surface.surface_p5`) for
//!   the conversions without exact polar conversion (`ConvertUVFromSTEPToNURBS` without
//!   a polar converter, `DropPointFast` with an inexact angle) and
//!   `ConvertUVFromNURBSToSTEP` without exact conversion (`p1`) return
//!   [`SolidError::Unported`].
//! - **Zone tolerance.** The bounding box of `IntersectWithSurfOfRevolution` without
//!   approximation tolerance is the NURB's (the inherited
//!   `SmBSplineSurface::CalculateBoundingBox`); the NURB has no owner, so its zone
//!   tolerance expansion finds no face (`GetFace()` is `NULL`), as for a surface of
//!   revolution outside a face upstream.

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::CurvePropertyType;
use crate::curve::curve::p4::is_degenerate_base;
use crate::curve::curve::p5::local_property_analysis;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::Curve;
use crate::curve::ellipse::{Ellipse, EllipseOptions, PolarConversion};
use crate::curve::iso_curve::IsoCurve;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_point_distance;
use crate::geom::global_solver::{
    GlobalSolve, GlobalSolver, OperationCategory, SolutionRequested, SolverOperation,
};
use crate::geom::solution_array::{
    ObjectKind, Solution, SolutionArray, SolutionObject, SolutionType, SortKey,
};
use crate::geom::tol::are_same;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{min2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_DEG, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::{
    BSplineSurface, BoundingBoxRequest, IsoCurveOptions, SurfParamType,
};
use crate::nurbs::geom::Rectangle;
use crate::nurbs::srf_basic::Direction;
use crate::surface::surface::p2::{SurfaceIntersection, TsectCurveType};
use crate::topology::database_io::{DatabaseReader, FileType, InputStream};

use super::p1::{
    gen_adjust_step_interval, gen_base, gen_base_mut, gen_with_out_of_bounds, not_b_spline,
    SurfOfRevolution, SweepIsoFn, OP_GEN,
};

const OP_REPARAM: &str = "surface::surf_of_revolution::p3::SurfOfRevolution::reparameterize";
const OP_TO_NURBS: &str =
    "surface::surf_of_revolution::p3::SurfOfRevolution::convert_uv_from_step_to_nurbs";
const OP_DROP: &str = "surface::surf_of_revolution::p3::SurfOfRevolution::drop_point_fast";
const OP_REBUILD: &str =
    "surface::surf_of_revolution::p3::SurfOfRevolution::rebuild_step_from_nurb_parameters";
const OP_XFORM: &str = "surface::surf_of_revolution::p3::SurfOfRevolution::transform";
const OP_READ: &str = "surface::surf_of_revolution::p3::SurfOfRevolution::read_from_db";
const OP_ISECT: &str =
    "surface::surf_of_revolution::p3::SurfOfRevolution::intersect_with_surf_of_revolution";

/// `ANALYTIC_TOL_SCALE` (`SmConfig.h`).
const ANALYTIC_TOL_SCALE: f64 = 100.0;

/// A surface derived from `SmSurfOfRevolution` (the plain surface, and `SmCone` /
/// `SmSphere` / `SmTorus` once ported), as the other operand of the intersections.
pub trait Revolved {
    /// The `SmSurfOfRevolution` part of the surface.
    fn as_surf_of_revolution(&self) -> &SurfOfRevolution;

    /// The STEP domain of the NURB domain `nurb_domain`
    /// (`ConvertDomainFromNURBSToSTEP`, through the surface's virtual
    /// `ConvertUVFromNURBSToSTEP`).
    ///
    /// # Errors
    ///
    /// Those of the kind's conversion.
    fn convert_domain_from_nurbs_to_step(
        &self,
        k: &Kernel,
        nurb_domain: &Extent2d,
    ) -> Result<Extent2d>;
}

impl Revolved for SurfOfRevolution {
    fn as_surf_of_revolution(&self) -> &SurfOfRevolution {
        self
    }

    fn convert_domain_from_nurbs_to_step(
        &self,
        k: &Kernel,
        nurb_domain: &Extent2d,
    ) -> Result<Extent2d> {
        Self::convert_domain_from_nurbs_to_step(self, k, nurb_domain)
    }
}

/// A curve named by the solutions of the generator solves: which generator (`0`, `1`;
/// `2`, `3` for the analytic replacements their caches make) and its natural
/// interval (the solvers' duplicate tests read it).
#[derive(Debug, Clone, Copy, PartialEq)]
struct GenHandle(u8, Extent1d);

impl SolutionObject for GenHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.1)
    }
}

/// `SmSurface::LocalPointSolve(domain, operation, point, guess, ..)`: whether an answer
/// was found and the solution.
fn surface_local_point_solve<O: Copy>(
    k: &Kernel,
    surface: &SurfOfRevolution,
    domain: &Extent2d,
    operation: SolverOperation,
    point: Point3,
    guess: Point2,
) -> Result<(bool, Solution<O>)> {
    // the SmSurface body (surface.surface_p5), on a Surface holding a copy
    let surface = crate::surface::surface::Surface::from(surface.clone_exact());
    let r = surface.local_point_solve::<O>(k, domain, operation, point, guess)?;
    Ok((r.found, r.solution))
}

/// `SmIsoCurve::LocalPropertyAnalysis(ivl, SM_CP_POINT_INVERSION, guess, NULL, &point,
/// bFoundAnswer, sSol)`: whether an answer was found and the solution (the solution as
/// left by a failure, as upstream reads it then too).
fn iso_point_inversion(
    k: &Kernel,
    iso: &IsoCurve,
    interval: &Extent1d,
    guess: f64,
    point: Point3,
) -> Result<(Result<bool>, Solution<()>)> {
    let mut sol: Solution<()> = Solution::new();
    let r = local_property_analysis(
        k,
        iso,
        interval,
        CurvePropertyType::PointInversion,
        guess,
        None,
        &[point],
        &mut sol,
    );
    if let Err(e) = &r {
        if e.is_unported() {
            return Err(e.clone());
        }
    }
    Ok((r, sol))
}

/// An ASCII value read with `operator>>`; `None` (a failed sentry) is uninitialised
/// upstream.
fn ascii<T>(v: Option<T>) -> Result<T> {
    v.ok_or(SolidError::invalid_input(
        OP_READ,
        "unreadable ASCII value (uninitialised upstream)",
    ))
}

impl SurfOfRevolution {
    // ---- copies and edits -----------------------------------------------------------

    /// The `SmBSplineSurface` base object, mutably: an edit of the NURB alone (an
    /// explicit `SmBSplineSurface::` call upstream) leaves the analytic data as it was
    /// until [`Self::rebuild_step_from_nurb_parameters`].
    pub fn base_mut(&mut self) -> &mut BSplineSurface {
        &mut self.base
    }

    /// An exact copy (`Copy`: the copy constructor).
    #[must_use]
    pub fn copy(&self) -> Self {
        self.clone()
    }

    /// Maps the NURB domain onto `trim_domain` without changing the shape
    /// (`Reparameterize`, `N_SrfReparamToInterval` in both directions), then the
    /// generator's NURB interval onto the generator direction of `trim_domain` and
    /// rebuilds the polar converter (both statuses ignored). The NURB is made first
    /// when missing (status ignored).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without NURB; `InvalidInput` where NLib would write outside
    /// its knot arrays (upstream ignores its status).
    pub fn reparameterize(&mut self, k: &Kernel, trim_domain: &Extent2d) -> Result<()> {
        self.reparameterize_using(k, trim_domain, &Self::own_sweep_iso)
    }

    /// [`Self::reparameterize`] with the polar converter rebuilt from the sweep iso
    /// curves of `iso` (upstream's virtual `CreateIsoParametricCurve`: a derived class
    /// such as `SmCone` passes its override).
    ///
    /// # Errors
    ///
    /// Those of [`Self::reparameterize`].
    pub(crate) fn reparameterize_using(
        &mut self,
        k: &Kernel,
        trim_domain: &Extent2d,
        iso: SweepIsoFn<'_>,
    ) -> Result<()> {
        // new NURB locals
        let r = Rectangle::new(
            trim_domain.min().x,
            trim_domain.max().x,
            trim_domain.min().y,
            trim_domain.max().y,
        );

        // GetOrCreateGwNurbPointer
        if self.base.nurb.is_none() {
            ignore_status(self.make_nurb(k))?;
        }
        let sur = self
            .base
            .nurb
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_REPARAM))?;

        // set NURB Surface domain parameterization - no change in shape
        sur.reparam_to_interval(r, Direction::UV)?;

        // Rule 2: GenCurve NURBInterval == m_bSwapUV ? Surface->NURBUInterval :
        // Surface->NURBVInterval
        let gen_ivl = if self.swap_uv {
            trim_domain.u_interval()
        } else {
            trim_domain.v_interval()
        };
        if let Some(gen) = self.gen_curve.as_mut() {
            ignore_status(gen.edit_parameterization(k, &gen_ivl))?;
        }

        // Rule 5: Set PolarConverter from Nurb midPoint rotation isoParamCurve
        ignore_status(self.rebuild_polar_converter(k, iso))?;
        Ok(())
    }

    /// Scales the surface uniformly by `scale.x` about the world origin (when given),
    /// then rotates and moves it by `rotate_n_move` (`Transform`): the placement, the
    /// generator (status ignored) and the NURB; then the polar converter is rebuilt
    /// (status ignored). The identity frame with no or unit scale changes nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a non-uniform scale or a scale below `SM_EFF_ZERO`;
    /// `InvalidInput` for a placement axis of zero length; `SM_ERR_NULL_POINTER`
    /// without generator (upstream dereferences it); those of the NURB transform.
    pub fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        self.transform_at_depth(k, rotate_n_move, scale, 0)
    }

    pub(crate) fn transform_at_depth(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        depth: usize,
    ) -> Result<()> {
        self.transform_using_at_depth(k, rotate_n_move, scale, &Self::own_sweep_iso, depth)
    }

    /// [`Self::transform`] with the polar converter rebuilt from the sweep iso curves
    /// of `iso` (upstream's virtual `CreateIsoParametricCurve`: a derived class such
    /// as `SmTorus` passes its override).
    pub(crate) fn transform_using(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        iso: SweepIsoFn<'_>,
    ) -> Result<()> {
        self.transform_using_at_depth(k, rotate_n_move, scale, iso, 0)
    }

    fn transform_using_at_depth(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        iso: SweepIsoFn<'_>,
        depth: usize,
    ) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        // check input and get scaling value - only allow uniform scaling
        let mut d_scale = 1.0;
        if let Some(s) = scale {
            if !are_same(s.x, s.y) || !are_same(s.x, s.z) || s.x < EFF_ZERO {
                return Err(SolidError::sm_err(OP_XFORM));
            }
            d_scale = s.x;
        }

        // scale current surface placement
        let mut sphere_place = Axis2Placement::IDENTITY;
        sphere_place.set_canonical(
            self.position.origin() * d_scale,
            self.position.x_axis(),
            self.position.y_axis(),
        )?;

        // apply rotation to scaled placement, update surface placement
        self.position = sphere_place.transform_axis2_placement(rotate_n_move);

        // transform the GenCurve (status ignored)
        let gen = self
            .gen_curve
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_GEN))?;
        ignore_status(gen.transform_at_depth(k, rotate_n_move, scale, depth + 1))?;

        // apply transformation to underlying Nurb
        if self.base.nurb.is_some() {
            self.base.transform(rotate_n_move, scale)?;
        }

        // update PolarConverter
        ignore_status(self.rebuild_polar_converter(k, iso))?;
        Ok(())
    }

    /// Sets the STEP domain from the NURB domain `new_nurbs_domain` and adjusts the
    /// generator to it (`UpdateAnalyticalDomain`, after the NURB has been trimmed):
    /// the STEP domain spans the converted corners (conversion statuses ignored), the
    /// generator's STEP interval becomes its V interval and the generator's knots are
    /// rescaled to the generator direction of `new_nurbs_domain` (status ignored).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator; those of the generator's
    /// `AdjustSTEPInterval`; [`SolidError::Unported`] from the corner conversions.
    pub fn update_analytical_domain(
        &mut self,
        k: &Kernel,
        new_nurbs_domain: &Extent2d,
    ) -> Result<()> {
        // convert given NurbUVDomain corners into STEPUVPoints (statuses ignored)
        let min_anal_uv =
            self.convert_uv_from_nurbs_to_step_or_written(k, new_nurbs_domain.min())?;
        let max_anal_uv =
            self.convert_uv_from_nurbs_to_step_or_written(k, new_nurbs_domain.max())?;
        self.update_analytical_domain_from_step(k, new_nurbs_domain, min_anal_uv, max_anal_uv)
    }

    /// The rest of [`Self::update_analytical_domain`] once the corners of
    /// `new_nurbs_domain` are converted to the STEP parameters `min_anal_uv` and
    /// `max_anal_uv` (upstream's virtual `ConvertUVFromNURBSToSTEP`: a derived class
    /// such as `SmSphere` or `SmTorus` converts with its override).
    pub(crate) fn update_analytical_domain_from_step(
        &mut self,
        k: &Kernel,
        new_nurbs_domain: &Extent2d,
        min_anal_uv: Point2,
        max_anal_uv: Point2,
    ) -> Result<()> {
        // Build desired new STEP UVDomain (min might not be smaller, if swapped)
        self.anal_uv_domain.init();
        self.anal_uv_domain.add_point(min_anal_uv);
        self.anal_uv_domain.add_point(max_anal_uv);

        // set GenCurve analytic domain from V part of new surface domain
        let v_ivl = self.anal_uv_domain.v_interval();
        let gen = self
            .gen_curve
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_GEN))?;
        gen_adjust_step_interval(k, gen, &v_ivl)?;

        // make sure m_pGenCurve Nurb domain matches crNewNurbsDomain (status ignored)
        let gen_ivl = if self.swap_uv {
            new_nurbs_domain.u_interval()
        } else {
            new_nurbs_domain.v_interval()
        };
        let base = gen_base_mut(gen).ok_or_else(not_b_spline)?;
        ignore_status(base.scale_knot_vector(gen_ivl.min(), gen_ivl.max()))?;
        Ok(())
    }

    // ---- parameter conversion -------------------------------------------------------

    /// The NURB parameters of the STEP parameters `step_uv` (degrees, generator
    /// parameter) (`ConvertUVFromSTEPToNURBS`): the sweep parameter from the polar
    /// converter (refined by point inversion on the sweep iso curve when the
    /// conversion is not exact; that status is ignored and its solution used) and the
    /// generator's parameter conversion, swapped with [`Self::swap_uv`].
    ///
    /// # Errors
    ///
    /// Those of the conversions and the STEP evaluation;
    /// [`SolidError::Unported`] without polar conversion (`SmSurface::LocalPointSolve`,
    /// unit `surface.surface_p5`).
    pub fn convert_uv_from_step_to_nurbs(&self, k: &Kernel, step_uv: Point2) -> Result<Point2> {
        // (init output to something in case of failure: the port returns the error)
        if !self.have_polar_conversion() {
            let nat_domain = self.base.natural_uv_domain();
            let mapped =
                self.anal_uv_domain
                    .point_map_to_domain(step_uv, &nat_domain, self.swap_uv)?;

            // Use numerical techniques to convert to NURBS coordinates.
            let nurbs_guess = mapped;
            let pnt = self.evaluate_step_point(k, step_uv)?;
            // (status ignored upstream: the solution is always used)
            let sol: Solution<()> = ignored(surface_local_point_solve(
                k,
                self,
                &nat_domain,
                SolverOperation::Minimize,
                pnt,
                nurbs_guess,
            ))?
            .map_or_else(Solution::new, |(_found, sol)| sol);
            return Ok(Point2::new(sol.start.param(0)?, sol.start.param(1)?));
        }

        // arrive here when polar conversions are possible
        let circle = self
            .polar_curve()
            .ok_or(SolidError::null_pointer(OP_TO_NURBS))?;
        let (mut x, exact_conversion) = self
            .polar
            .convert_to_nurbs_parameter(k, circle, step_uv.x, 0.0)?;
        let y = self.gen()?.convert_t_from_step_to_nurbs(k, step_uv.y)?;

        // when the conversion was not exact
        if !exact_conversion {
            let srf_param = if self.swap_uv {
                SurfParamType::U
            } else {
                SurfParamType::V
            };
            let iso = IsoCurve::new(self.base.clone(), srf_param, y, true, true);
            let pnt = self.evaluate_step_point(k, step_uv)?;
            let ivl = iso.natural_interval();

            // do a local point solve (status ignored; the solution is always used)
            let (_found, sol3) = iso_point_inversion(k, &iso, &ivl, x, pnt)?;
            x = sol3.start.param(0)?;
        }

        // swap output when required
        Ok(if self.swap_uv {
            Point2::new(y, x)
        } else {
            Point2::new(x, y)
        })
    }

    /// The STEP domain spanned by the converted corners of the NURB domain
    /// `nurb_domain` (`SmSurface::ConvertDomainFromNURBSToSTEP`, the body shared with
    /// unit `surface.surface_p1`'s [`crate::surface::surface::Surface`]): the angles
    /// from the corners in order, the generator parameters sorted.
    ///
    /// # Errors
    ///
    /// Those of [`Self::convert_uv_from_nurbs_to_step`]; `InvalidInput` when the
    /// converted angles are reversed (`SetMinMax` fails).
    pub fn convert_domain_from_nurbs_to_step(
        &self,
        k: &Kernel,
        nurb_domain: &Extent2d,
    ) -> Result<Extent2d> {
        crate::surface::surface::p1::convert_domain_from_nurbs_to_step_with(nurb_domain, |uv| {
            self.convert_uv_from_nurbs_to_step(k, uv)
        })
    }

    // ---- point drop -----------------------------------------------------------------

    /// Drops `test_point` onto the surface over the NURB domain `uv_domain`
    /// (`DropPointFast`): the point is swung about the axis onto the generator's
    /// plane and dropped onto the generator over the domain's generator interval
    /// (`GlobalPointSolve` with `operation`, `distance_tolerance`, `target_distance`
    /// and `requested`); each drop at each swing angle (two on a seam or the axis,
    /// unless `in_pointing_vector` picks one; at the ends of a periodic generator the
    /// vector also picks between the ends) becomes a solution `(u, v)` named `this`
    /// with its distance as value, inside `uv_domain` (and, intersecting, within the
    /// tolerance), sorted by value in a minimising global solver.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without polar conversion, for a non-planar generator, for a point
    /// outside the sweep interval (callers then run a general solver) or when the
    /// iso curve inversion of an inexact angle fails; those of the start-plane
    /// transform, the generator solve, the conversions and evaluations;
    /// [`SolidError::Unported`] for an inexact angle (`SmSurface::LocalPointSolve`,
    /// unit `surface.surface_p5`).
    pub fn drop_point_fast<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        operation: SolverOperation,
        test_point: Point3,
        in_pointing_vector: Option<Vec3>,
        distance_tolerance: f64,
        target_distance: Option<f64>,
        requested: SolutionRequested,
        this: Option<O>,
    ) -> Result<SolutionArray<O>> {
        self.drop_point_fast_using(
            k,
            uv_domain,
            operation,
            test_point,
            in_pointing_vector,
            distance_tolerance,
            target_distance,
            requested,
            this,
            &|k, uv| self.evaluate_step_point(k, uv),
            &|_, uv| self.base.evaluate_point(uv),
            &|k, domain, operation, point, guess| {
                surface_local_point_solve(k, self, domain, operation, point, guess)
            },
        )
    }

    /// [`Self::drop_point_fast`] with upstream's virtual calls on `this` supplied by
    /// the derived class (`SmCone`, `SmSphere`, `SmTorus`): `step_point` is its
    /// `EvaluateSTEPPoint`, `evaluate_point` its `EvaluatePoint`, and `local_solve`
    /// its `LocalPointSolve` (`found` and the solution).
    ///
    /// # Errors
    ///
    /// Those of [`Self::drop_point_fast`] and of the callbacks.
    #[allow(clippy::too_many_arguments)] // upstream's argument list plus the virtuals
    #[allow(clippy::type_complexity)]
    pub(crate) fn drop_point_fast_using<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        operation: SolverOperation,
        test_point: Point3,
        in_pointing_vector: Option<Vec3>,
        distance_tolerance: f64,
        target_distance: Option<f64>,
        requested: SolutionRequested,
        this: Option<O>,
        step_point: &dyn Fn(&Kernel, Point2) -> Result<Point3>,
        evaluate_point: &dyn Fn(&Kernel, Point2) -> Result<Point3>,
        local_solve: &dyn Fn(
            &Kernel,
            &Extent2d,
            SolverOperation,
            Point3,
            Point2,
        ) -> Result<(bool, Solution<O>)>,
    ) -> Result<SolutionArray<O>> {
        // no work - can't convert to polar coordinates - does not have a planar generator
        if !self.have_polar_conversion() {
            return Err(SolidError::sm_err(OP_DROP));
        }
        if !self.planar_generator {
            return Err(SolidError::sm_err(OP_DROP));
        }

        // the generator with the surface's out-of-bounds flag
        let gen_own = self.gen()?;
        let tmp = gen_with_out_of_bounds(gen_own, self.base.is_out_of_bounds_enabled());
        let gen = tmp.as_ref().unwrap_or(gen_own);

        // construct and init global solver
        let mut gs: GlobalSolver<'_, O> = GlobalSolver::new();
        gs.set_3d_tolerance(distance_tolerance);
        gs.set_solver_operation(operation);
        gs.set_solution_requested(SolutionRequested::All);
        gs.set_operation_category(OperationCategory::Minimize);
        gs.set_solutions(SolutionArray::new());
        gs.set_best_answer_so_far_sq(BIG_DOUBLE);
        if let Some(t) = target_distance {
            gs.set_at_distance(t);
        }

        // swing the point back to the plane of the generator curve - return error for
        // exterior points
        let sp = self.transform_point_to_start_plane(test_point, distance_tolerance, false)?;
        let inside = sp.inside;
        let mut num_angles = sp.angles_deg.len();
        let mut angles_deg = [
            sp.angles_deg.first().copied().unwrap_or(0.0),
            sp.angles_deg.get(1).copied().unwrap_or(0.0),
        ];

        // when point is not in sweep interval
        if !inside {
            return Err(SolidError::sm_err(OP_DROP));
        }

        // For testPoints on seams and given a pointing vector - only keep the specified
        // solution
        if num_angles == 2 {
            if let Some(v) = in_pointing_vector {
                let dot = v.dot(self.position.y_axis());
                if dot > EFF_ZERO_SQRT {
                    // Keep start parameter of periodic surface
                    num_angles = 1;
                } else if dot < -EFF_ZERO_SQRT {
                    // Keep end parameter of periodic surface
                    num_angles = 1;
                    angles_deg[0] = angles_deg[1];
                }
            }
        }

        // get the generator curve's nurb interval (SetMinMax's status is ignored: a
        // reversed pair leaves the "inside out" init)
        let mut gen_nurb_ivl = Extent1d::new();
        let _ = if self.swap_uv {
            gen_nurb_ivl.set_min_max(uv_domain.min().x, uv_domain.max().x)
        } else {
            gen_nurb_ivl.set_min_max(uv_domain.min().y, uv_domain.max().y)
        };

        // drop the rotated point to the generator curve
        let gen_handle = GenHandle(0, gen.natural_interval());
        let opts = GlobalPointSolveOptions {
            target_distance,
            opt_vectors: None,
            handle: Some(gen_handle),
            analytic_handle: Some(GenHandle(2, gen_handle.1)),
            zone_tol_3d: None,
        };
        let gen_solutions = gen.global_point_solve(
            k,
            &gen_nurb_ivl,
            operation,
            sp.point,
            distance_tolerance,
            requested,
            &opts,
        )?;

        // init a solution object
        let mut sol: Solution<O> = Solution::new();
        sol.solution_type = SolutionType::SingleValue;
        sol.num_objects = 1;
        sol.objects[0] = this;
        sol.nodes[0] = None;
        sol.num_variables = 2;

        let circle = self
            .polar_curve()
            .ok_or(SolidError::null_pointer(OP_DROP))?;
        let num_gen = gen_solutions.len();

        // for every rotation angle (normally 1, 2 for points on seams)
        for &angle in angles_deg.iter().take(num_angles) {
            // convert the angles (degrees) to NURBS parameters
            let (mut u_parameter, exact_conversion) = self
                .polar
                .convert_to_nurbs_parameter(k, circle, angle, 0.0)?;

            // for every drop point to generator curve solution
            for cr_sol in gen_solutions.as_slice() {
                // get generator curve parameter
                let t = cr_sol.start.param(0)?;

                // Resolve ambiguities with cpOptInPointingVector if present.
                if !exact_conversion {
                    // get surface isoCurve in the generator curve param direction
                    let srf_param = if self.swap_uv {
                        SurfParamType::U
                    } else {
                        SurfParamType::V
                    };
                    let iso = IsoCurve::new(self.base.clone(), srf_param, t, true, true);
                    let nurb_ivl = iso.natural_interval();

                    // when isoCurve is degenerate - use Interval min
                    if is_degenerate_base(k, &iso, EFF_ZERO, None)? {
                        u_parameter = nurb_ivl.min();
                    } else {
                        // get the 3dDropPoint
                        let pnt = step_point(k, Point2::new(angle, t))?;

                        // transform the DropPoint coordinates
                        let (r, sol3) = iso_point_inversion(k, &iso, &nurb_ivl, u_parameter, pnt)?;
                        match r {
                            Ok(true) => u_parameter = sol3.start.param(0)?,
                            _ => return Err(SolidError::sm_err(OP_DROP)),
                        }
                    }
                }

                // when more than 1 drop point was found and a specified direction is
                // given for a periodic genCurve
                if num_gen > 1 {
                    if let Some(v) = in_pointing_vector {
                        if gen.is_periodic(k, &gen_nurb_ivl)? {
                            // skip solution when solution is ivl min and PointingVector
                            // opposes curve tangent
                            if are_same(t, gen_nurb_ivl.min()) {
                                let pv = gen.evaluate(k, t, 1, true, true)?;
                                let d1 = pv.get(1).copied().unwrap_or(Vec3::UNDEF);
                                if d1.dot(v) < -EFF_ZERO_SQRT {
                                    continue;
                                }
                            }

                            // skip solution when solution is ivl max and PointingVector
                            // does not oppose curve tangent
                            if are_same(t, gen_nurb_ivl.max()) {
                                let pv = gen.evaluate(k, t, 1, true, true)?;
                                let d1 = pv.get(1).copied().unwrap_or(Vec3::UNDEF);
                                if d1.dot(v) > EFF_ZERO_SQRT {
                                    continue;
                                }
                            }
                        }
                    }
                }

                // set the solution parameters
                if self.swap_uv {
                    sol.start.parameters[0] = t;
                    sol.start.parameters[1] = u_parameter;
                } else {
                    sol.start.parameters[0] = u_parameter;
                    sol.start.parameters[1] = t;
                }

                // get the surface UVpoint
                let mut uv = Point2::new(sol.start.parameters[0], sol.start.parameters[1]);
                if !exact_conversion {
                    let (found, s) = local_solve(k, uv_domain, operation, test_point, uv)?;
                    sol = s;
                    if found {
                        uv = Point2::new(sol.start.parameters[0], sol.start.parameters[1]);
                    }
                }

                // Skip solution if not in requested domain
                if !uv_domain.contains_point(uv, 0.0) {
                    continue;
                }

                // set SolutionValue = dropSurfacePoint to TestPoint distance
                let surf_pnt = evaluate_point(k, uv)?;
                sol.start.solution_value = test_point.distance(surf_pnt);

                // skip intersection solutions whose value exceed the DistanceTolerance
                if operation == SolverOperation::Intersect
                    && sol.start.solution_value > distance_tolerance
                {
                    continue;
                }

                // put the solution into the output
                gs.add_sorted_solution(&sol, SortKey::BySolutionValue, true)?;
            }
        }

        // all done
        Ok(gs.set_solutions(SolutionArray::new()))
    }

    // ---- intersections --------------------------------------------------------------

    /// The intersection with the cone `cone` (`IntersectWithCone`): for a planar
    /// generator [`Self::intersect_with_surf_of_revolution`], else the general
    /// intersector is needed. `cone` stands for the `SmCone` argument until that type
    /// is ported.
    ///
    /// # Errors
    ///
    /// Those of [`Self::intersect_with_surf_of_revolution`].
    pub fn intersect_with_cone<R: Revolved + ?Sized>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        cone: &R,
        cone_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        if !self.planar_generator {
            return Ok((true, SurfaceIntersection::default()));
        }
        self.intersect_with_surf_of_revolution(k, uv_domain, cone, cone_uv_domain, approx_tol_3d)
    }

    /// The intersection with the sphere `sphere` (`IntersectWithSphere`): for a planar
    /// generator [`Self::intersect_with_surf_of_revolution`], else the general
    /// intersector is needed. `sphere` stands for the `SmSphere` argument until that
    /// type is ported.
    ///
    /// # Errors
    ///
    /// Those of [`Self::intersect_with_surf_of_revolution`].
    pub fn intersect_with_sphere<R: Revolved + ?Sized>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        sphere: &R,
        sphere_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        // Can not intersect things with non planar generator curves here
        if !self.planar_generator {
            return Ok((true, SurfaceIntersection::default()));
        }
        self.intersect_with_surf_of_revolution(
            k,
            uv_domain,
            sphere,
            sphere_uv_domain,
            approx_tol_3d,
        )
    }

    /// The intersection with `other` when both rotate about the same axis
    /// (`IntersectWithSurfOfRevolution`): both generators are rotated into this
    /// surface's start plane and intersected (`GlobalCurveIntersect` within the
    /// approximation tolerance, else `SM_EFF_ZERO_SQRT * (1 + bounding box size)`);
    /// each intersection point sweeps a circle (a degenerate curve when the common
    /// sweep interval is shorter than the tolerance), each coincident generator range
    /// gives the generator (degenerate sweep) or the four boundaries of the common
    /// region, over every common sweep interval (split at this surface's sweep
    /// boundaries). Returns whether the general intersector is needed: for a non-planar
    /// generator, the same surface, or axes that are not parallel (within
    /// `SM_EFF_ZERO_DEG`) or not coincident (within a hundredth of the tolerance).
    ///
    /// # Errors
    ///
    /// Those of the bounding box, the domain conversions, the generator copies, the
    /// generator intersection, the circles and evaluations.
    pub fn intersect_with_surf_of_revolution<R: Revolved + ?Sized>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &R,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        intersect_revolved(self, k, uv_domain, other, other_uv_domain, approx_tol_3d)
    }

    /// [`Self::intersect_with_cone`] into `out` (reset first). On an error `out` keeps
    /// what was appended before it, as upstream leaves its output arrays.
    pub(crate) fn intersect_with_cone_into<R: Revolved + ?Sized>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        cone: &R,
        cone_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
        out: &mut SurfaceIntersection,
    ) -> Result<bool> {
        if !self.planar_generator {
            *out = SurfaceIntersection::default();
            return Ok(true);
        }
        intersect_revolved_into(self, k, uv_domain, cone, cone_uv_domain, approx_tol_3d, out)
    }

    /// [`Self::intersect_with_sphere`] into `out` (reset first). On an error `out` keeps
    /// what was appended before it, as upstream leaves its output arrays.
    pub(crate) fn intersect_with_sphere_into<R: Revolved + ?Sized>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        sphere: &R,
        sphere_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
        out: &mut SurfaceIntersection,
    ) -> Result<bool> {
        // Can not intersect things with non planar generator curves here
        if !self.planar_generator {
            *out = SurfaceIntersection::default();
            return Ok(true);
        }
        intersect_revolved_into(
            self,
            k,
            uv_domain,
            sphere,
            sphere_uv_domain,
            approx_tol_3d,
            out,
        )
    }

    // ---- analytic data from the NURB ------------------------------------------------

    /// Rebuilds the generator, placement, swap flag, STEP domain and polar converter
    /// from the NURB (`RebuildSTEPFromNURBParameters`): a degree 2 rational direction
    /// whose iso curves are coaxial circular arcs on parallel planes starting and
    /// ending on common planes ([`super::test_for_iso_circles`] at
    /// `1000 * ANALYTIC_TOL_SCALE * SM_EFF_ZERO * (1 + |corners|)` and
    /// `1000 * SM_EFF_ZERO_SQRT` radians; constant-v circles first) is the sweep; the
    /// iso curve at the start of the other direction becomes the generator, which must
    /// be planar and coplanar with the circles' axis.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a degree 2 rational direction, without circular iso curves, for
    /// a degenerate, non-planar or not coplanar generator; those of the evaluations,
    /// the iso-circle tests and the iso curve.
    pub fn rebuild_step_from_nurb_parameters(&mut self, k: &Kernel) -> Result<()> {
        // check m_pNurb - a degree 2 rational surface in at least 1 direction
        let deg_u = self.base.degree(SurfParamType::U);
        let deg_v = self.base.degree(SurfParamType::V);
        if (deg_u != 2 && deg_v != 2) || !self.base.is_rational() {
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // evaluate min/max domain points (statuses ignored upstream; the points are
        // read)
        let nurb_uv_domain = self.base.natural_uv_domain();
        let nurb_min_point = self.base.evaluate_point(nurb_uv_domain.min())?;
        let nurb_max_point = self.base.evaluate_point(nurb_uv_domain.max())?;

        // scaled zero value
        let tolerance_scale = 1000.0; // loose because surf is already labeled analytic
        let scaled_zero = tolerance_scale
            * ANALYTIC_TOL_SCALE
            * EFF_ZERO
            * (1.0 + nurb_min_point.max_dimension() + nurb_max_point.max_dimension());
        let ang_tol_rad = EFF_ZERO_SQRT * tolerance_scale;

        // test sequence of U direction (V Constant) isoCurves for circularity and
        // parallelism
        let mut circles_v = None;
        let mut is_revolution_no_swap_uv = false;
        if deg_u == 2 {
            let c = super::test_for_iso_circles(
                k,
                &self.base,
                SurfParamType::V,
                scaled_zero,
                ang_tol_rad,
            )?;
            is_revolution_no_swap_uv =
                c.found_circles && c.parallel_planes && c.coaxial_centers && c.planar_gen_curve;
            circles_v = Some(c);
        }

        // test sequence of V direction (U Constant) isoCurves
        let mut circles_u = None;
        let mut is_revolution_with_swap_uv = false;
        if !is_revolution_no_swap_uv && deg_v == 2 {
            let c = super::test_for_iso_circles(
                k,
                &self.base,
                SurfParamType::U,
                scaled_zero,
                ang_tol_rad,
            )?;
            is_revolution_with_swap_uv =
                c.found_circles && c.parallel_planes && c.coaxial_centers && c.planar_gen_curve;
            circles_u = Some(c);
        }

        // no work - no sweep direction
        if !is_revolution_with_swap_uv && !is_revolution_no_swap_uv {
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // Let GenCurve = iso-curve from the 'other' parameter
        let (gen_dir, swap_uv, param, circles) = if is_revolution_no_swap_uv {
            // constant V isoParamCurves are circles
            (SurfParamType::U, false, nurb_uv_domain.min().x, circles_v)
        } else {
            // constant U isoParamCurves are circles
            (SurfParamType::V, true, nurb_uv_domain.min().y, circles_u)
        };
        let circles = circles.ok_or(SolidError::sm_err(OP_REBUILD))?;
        let ref_frame = circles
            .reference_frame
            .ok_or(SolidError::sm_err(OP_REBUILD))?;
        let angles = circles.angles.ok_or(SolidError::sm_err(OP_REBUILD))?;
        let (min_ang_deg, max_ang_deg) = (angles.min(), angles.max());

        // create GenCurve isoParamCurve
        let iso_crv = self
            .base
            .create_iso_parametric_curve(k, gen_dir, param, IsoCurveOptions::default())?
            .curve;
        let interval = iso_crv.natural_interval();

        // no work - GenCurve is degenerate
        if iso_crv.is_degenerate(k, scaled_zero, None)? {
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // verify GenCurve is planar and coplanar with refFrame ZAxis
        let iso_curve = Curve::from(iso_crv);
        let planar = iso_curve.is_planar(k, scaled_zero)?;
        let linear = iso_curve.is_linear(k, EFF_ZERO, false)?;
        let is_planar = planar.is_planar;
        let is_linear = linear.linear;
        if !is_planar {
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // verify that plane containing genCurve also contains refFrame Z axis
        let plane_point = planar.point;
        let plane_normal = planar.normal;
        let origin_point_dist = (ref_frame.origin() - plane_point).dot(plane_normal).abs();
        let frame_plane_ortho = plane_normal.dot(ref_frame.z_axis()).abs();
        let frame_line_co_planar = if is_linear {
            (ref_frame.origin() - plane_point)
                .dot(ref_frame.z_axis().cross(linear.line_tangent))
                .abs()
        } else {
            1.0
        };
        // ((bLinear || dOriginPointDist < dScaledZero) && ((!bLinear && dFramePlaneOrtho
        // < dScaledZero) || (bLinear && dFrameLineCoPlanar < dScaledZero)))
        let is_co_planar = if is_linear {
            frame_line_co_planar < scaled_zero
        } else {
            origin_point_dist < scaled_zero && frame_plane_ortho < scaled_zero
        };
        if !is_co_planar {
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // set m_pGenCurve
        self.gen_curve = Some(iso_curve);

        // set Analytic parameters (SetMinMax's status is ignored: reversed bounds leave
        // the domain)
        self.planar_generator = is_planar;
        self.position = ref_frame;
        self.swap_uv = swap_uv;
        let _ = self.anal_uv_domain.set_min_max_coords(
            min_ang_deg,
            interval.min(),
            max_ang_deg,
            interval.max(),
        );

        // Set PolarConverter from Nurb midPoint rotation isoParamCurve (status ignored)
        ignore_status(self.rebuild_polar_converter(k, &Self::own_sweep_iso))?;
        Ok(())
    }

    // ---- database -------------------------------------------------------------------

    /// Reads a surface written by `WriteToDB` (`ReadFromDB`): the make-NURB and planar
    /// flags, the placement origin and axes, the STEP domain corners and the swap flag
    /// (one group per line in ASCII files), the generator (type header and record), the
    /// polar converter with its sweep circle, then the B-spline surface record.
    ///
    /// # Errors
    ///
    /// Those of the stream reads and of the generator, converter and B-spline readers;
    /// [`SolidError::InvalidInput`] for an unreadable ASCII value or type header
    /// (upstream: uninitialised), a generator not derived from `SmBSplineCurve` or
    /// nesting too deep.
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        db.enter_nested(OP_READ)?;
        let result = Self::read_body(k, db, version);
        db.leave_nested();
        result
    }

    /// The body of [`Self::read_from_db`].
    fn read_body(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        // init output object
        let mut surf = Self::empty();

        // file type
        let is_ascii = db.file_type() == FileType::Ascii;

        let (make_nurb_gen_curve, planar_generator, orig, x, y, anal_min, anal_max, swap_uv) =
            if is_ascii {
                let flag = |db: &mut DatabaseReader| -> Result<bool> {
                    let v = ascii(db.in_stream_mut().and_then(InputStream::extract_int))?;
                    db.go_to_next_line();
                    Ok(v != 0)
                };
                let vec = |db: &mut DatabaseReader| -> Result<Vec3> {
                    let s = db
                        .in_stream_mut()
                        .ok_or(SolidError::null_pointer(OP_READ))?;
                    let a = ascii(s.extract_double())?;
                    let b = ascii(s.extract_double())?;
                    let c = ascii(s.extract_double())?;
                    db.go_to_next_line();
                    Ok(Vec3::new(a, b, c))
                };
                let make_nurb_gen_curve = flag(db)?;
                let planar_generator = flag(db)?;
                let orig = vec(db)?;
                let x = vec(db)?;
                let y = vec(db)?;
                let s = db
                    .in_stream_mut()
                    .ok_or(SolidError::null_pointer(OP_READ))?;
                let a = ascii(s.extract_double())?;
                let b = ascii(s.extract_double())?;
                let c = ascii(s.extract_double())?;
                let d = ascii(s.extract_double())?;
                db.go_to_next_line();
                let swap_uv = flag(db)?;
                (
                    make_nurb_gen_curve,
                    planar_generator,
                    orig,
                    x,
                    y,
                    Point2::new(a, b),
                    Point2::new(c, d),
                    swap_uv,
                )
            } else {
                let make_nurb_gen_curve = db.read_boolean()? != 0;
                let planar_generator = db.read_boolean()? != 0;
                let orig = Vec3::new(db.read_double()?, db.read_double()?, db.read_double()?);
                let x = Vec3::new(db.read_double()?, db.read_double()?, db.read_double()?);
                let y = Vec3::new(db.read_double()?, db.read_double()?, db.read_double()?);
                let a = db.read_double()?;
                let b = db.read_double()?;
                let c = db.read_double()?;
                let d = db.read_double()?;
                let swap_uv = db.read_boolean()? != 0;
                (
                    make_nurb_gen_curve,
                    planar_generator,
                    orig,
                    x,
                    y,
                    Point2::new(a, b),
                    Point2::new(c, d),
                    swap_uv,
                )
            };

        // GenCurve
        if is_ascii {
            db.go_to_next_line();
        }
        let header = db.read_type(true)?;
        let (Some(gen_type), Some(gen_dim)) = (header.type_code, header.dim) else {
            return Err(SolidError::invalid_input(
                OP_READ,
                "generator type header not read (uninitialised upstream)",
            ));
        };
        let gen_curve = Curve::read_from_db(k, gen_type, db, gen_dim, version)?;
        // (m_pGenCurve is an SmBSplineCurve: the cast upstream)
        gen_base(&gen_curve)?;

        // PolarConverter - read directly into empty analytic
        if is_ascii {
            db.go_to_next_line();
        }
        let (polar, polar_curve) = PolarConversion::read_from_db_with_curve_object(k, db, version)?;
        surf.polar = polar;
        surf.polar_curve = polar_curve;

        // load values (the statuses of SetCanonical and SetMinMax are ignored)
        surf.gen_curve = Some(gen_curve);
        surf.make_nurb_gen_curve = make_nurb_gen_curve;
        surf.planar_generator = planar_generator;
        let _ = surf.position.set_canonical(orig, x, y);
        let _ = surf.anal_uv_domain.set_min_max(anal_min, anal_max);
        surf.swap_uv = swap_uv;

        // read the parent object
        surf.base = BSplineSurface::read_from_db(db, version)?;

        // all done
        Ok(surf)
    }
}

/// `new SmCircle(center, x, y, ivl, radius, 3)` for a radius above `SM_EFF_ZERO`, else
/// `CreateDegenerateCurve(3, center)`.
fn sweep_arc(
    k: &Kernel,
    center: Point3,
    x_axis: Vec3,
    y_axis: Vec3,
    ivl: Extent1d,
    radius: f64,
) -> Result<Curve> {
    if radius > EFF_ZERO {
        Ok(Curve::from(Ellipse::new_circle(
            k,
            center,
            x_axis,
            y_axis,
            ivl,
            radius,
            &EllipseOptions::default(),
        )?))
    } else {
        Ok(Curve::from(BSplineCurve::create_degenerate_curve(
            3, center,
        )?))
    }
}

/// [`SurfOfRevolution::intersect_with_surf_of_revolution`] of the surface of revolution
/// part of `this`, whose virtual `ConvertDomainFromNURBSToSTEP` converts this side's
/// domain (an `SmTorus` calling the inherited `IntersectWithSurfOfRevolution`).
///
/// # Errors
///
/// Those of [`SurfOfRevolution::intersect_with_surf_of_revolution`].
pub(crate) fn intersect_revolved<T: Revolved + ?Sized, R: Revolved + ?Sized>(
    this: &T,
    k: &Kernel,
    uv_domain: &Extent2d,
    other: &R,
    other_uv_domain: &Extent2d,
    approx_tol_3d: Option<f64>,
) -> Result<(bool, SurfaceIntersection)> {
    let mut out = SurfaceIntersection::default();
    let more = intersect_revolved_into(
        this,
        k,
        uv_domain,
        other,
        other_uv_domain,
        approx_tol_3d,
        &mut out,
    )?;
    Ok((more, out))
}

/// [`intersect_revolved`] into `out` (reset first). On an error `out` keeps what was appended
/// before it, as upstream leaves its output arrays.
pub(crate) fn intersect_revolved_into<T: Revolved + ?Sized, R: Revolved + ?Sized>(
    this: &T,
    k: &Kernel,
    uv_domain: &Extent2d,
    other: &R,
    other_uv_domain: &Extent2d,
    approx_tol_3d: Option<f64>,
    out: &mut SurfaceIntersection,
) -> Result<bool> {
    // init output
    let me = this.as_surf_of_revolution();
    *out = SurfaceIntersection::default();
    let other_rev = other.as_surf_of_revolution();

    // no work - non planar generator curves
    if !me.planar_generator || !other_rev.planar_generator {
        return Ok(true);
    }

    // no work - Self Intersection is best done by NURBS to get UV curves.
    if core::ptr::eq(me, other_rev) {
        return Ok(true);
    }

    // locals
    let angular_tolerance = EFF_ZERO_DEG;
    let other_position = &other_rev.position;
    let other_origin = other_position.origin();
    let other_x_axis = other_position.x_axis();
    let other_z_axis = other_position.z_axis();
    let origin = me.position.origin();
    let x_axis = me.position.x_axis();
    let y_axis = me.position.y_axis();
    let z_axis = me.position.z_axis();

    // no work - coordinate systems are not parallel
    if !z_axis.is_parallel_to(other_z_axis, angular_tolerance) {
        return Ok(true);
    }

    // arrive here when both rotation axes are parallel to one another

    // select tolerance
    let tol = match approx_tol_3d {
        None => {
            let req = BoundingBoxRequest {
                normal: true,
                expand_by_zone_tol: true,
                ..BoundingBoxRequest::default()
            };
            let boxes = me.base.calculate_bounding_box(k, uv_domain, &req)?;
            let bbox = boxes.normal.ok_or(SolidError::null_pointer(OP_ISECT))?;
            let size = bbox.size().length();
            EFF_ZERO_SQRT * (1.0 + size)
        }
        Some(t) => t,
    };

    // no work = rotation axes are not coincident
    let (distance, _) = line_point_distance(origin, z_axis, other_origin);
    if distance > tol / 100.0 {
        return Ok(true);
    }

    // map input NurbDomains to StepDomains
    let this_anal_domain = this.convert_domain_from_nurbs_to_step(k, uv_domain)?;
    let other_anal_domain = other.convert_domain_from_nurbs_to_step(k, other_uv_domain)?;

    // get input sweep domains for both surfaces in thisCurves coordinate system
    let this_sweep_ivl = this_anal_domain.u_interval();
    let other_sweep_ivl = other_anal_domain.u_interval();
    // (CCWAngleBetween's status is ignored upstream: a failed angle is 0)
    let other_x_angle_rad = ignored(z_axis.ccw_angle_between(x_axis, other_x_axis))?.unwrap_or(0.0);
    let other_x_angle_deg = other_x_angle_rad * 180.0 / PI;
    let same = z_axis.dot(other_z_axis) > 0.0; // should be +/-1.0
    let (other_start_ang_deg, other_end_ang_deg) = if same {
        (
            other_x_angle_deg + other_sweep_ivl.min(),
            other_x_angle_deg + other_sweep_ivl.max(),
        )
    } else {
        let mut s = other_x_angle_deg - other_sweep_ivl.max();
        let e = other_x_angle_deg - other_sweep_ivl.min();
        if s > e {
            s -= 360.0;
        }
        (s, e)
    };
    let other_mapped_ivl = Extent1d::from_min_max(other_start_ang_deg, other_end_ang_deg);

    // intersect the AnalDomains (split result at this sweep interval's boundaries;
    // status ignored)
    let ivls = ignored(this_sweep_ivl.intersect_periodic(&other_mapped_ivl, 360.0, true))?
        .unwrap_or_default();

    // no work - no sweep intersections
    if ivls.is_empty() {
        return Ok(false);
    }

    // get temp genCurves in the same plane
    let other_param = if same {
        -other_x_angle_deg
    } else {
        other_x_angle_deg
    };
    let this_gen = me.create_generator_from_angle(k, 0.0)?;
    let other_gen = other_rev.create_generator_from_angle(k, other_param)?;

    // intersect the genCurves together
    let this_ivl = this_gen.natural_interval();
    let other_ivl = other_gen.natural_interval();
    let opts = CurveIntersectOptions {
        skip_global_coincidence_check: false,
        handles: [Some(GenHandle(0, this_ivl)), Some(GenHandle(1, other_ivl))],
        analytic_handles: [Some(GenHandle(2, this_ivl)), Some(GenHandle(3, other_ivl))],
    };
    let solutions =
        this_gen.global_curve_intersect(k, &this_ivl, &other_gen, &other_ivl, tol, &opts)?;

    // for every genCurve/genCurve solution
    for solution in solutions.as_slice() {
        // get Solution Start Points (statuses ignored upstream; the points are read)
        let start_point = this_gen.evaluate(k, solution.start.param(0)?, 1, true, true)?;
        let other_point = other_gen.evaluate(k, solution.start.param(1)?, 1, true, true)?;
        let sp0 = start_point.first().copied().unwrap_or(Vec3::UNDEF);
        let sp1 = start_point.get(1).copied().unwrap_or(Vec3::UNDEF);
        let op0 = other_point.first().copied().unwrap_or(Vec3::UNDEF);
        let op1 = other_point.get(1).copied().unwrap_or(Vec3::UNDEF);

        // get intersection circle CenterPoint and type
        let circle_center = origin + z_axis.dot(sp0 - origin) * z_axis;
        let circle_radius = circle_center.distance(sp0);
        let gen_curve_type = if sp1.is_parallel_to(op1, 2.0) {
            TsectCurveType::Tangent
        } else {
            TsectCurveType::Crossing
        };
        let deviation = sp0.distance(op0);

        // for every intersecting sweep interval
        for ivl in &ivls {
            let degenerate_ivl = (ivl.length() / 180.0 * PI * min2(circle_radius, 1.0)) < tol;

            // build an intersection curve for every intersection point
            if solution.solution_type == SolutionType::SingleValue {
                // the solution is either a degenerate point or a circular arc
                let (crv, curve_type) = if degenerate_ivl {
                    // get point position
                    let sweep_point = circle_center
                        + circle_radius * (ivl.min() * PI / 180.0).cos() * x_axis
                        + circle_radius * (ivl.min() * PI / 180.0).sin() * y_axis;

                    // A Single value solution + degenerate interval means this
                    // srf-srf XSect is a point [B661]
                    (
                        Curve::from(BSplineCurve::create_degenerate_curve(3, sweep_point)?),
                        TsectCurveType::Touching,
                    )
                } else {
                    // The SS XSect curve type matches the GenCurve XSect [B661]
                    (
                        Curve::from(Ellipse::new_circle(
                            k,
                            circle_center,
                            x_axis,
                            y_axis,
                            *ivl,
                            circle_radius,
                            &EllipseOptions::default(),
                        )?),
                        gen_curve_type,
                    )
                };

                // add curve to output
                out.curves_3d.push(crv);
                out.curve_types.push(curve_type);
                out.deviations.push(deviation);
            } else {
                // A solution raised by a coincident generator segment is either a
                // surface region (its boundaries) or a generator segment

                // get GenCurve intersection interval
                let mut gen_curve_ivl =
                    Extent1d::from_min_max(solution.start.param(0)?, solution.end.param(0)?);

                // Trim a genCurve at the interval start angle to the intersection
                // interval (may snap the interval to existing knots; status ignored)
                let mut min_angle_gen = me.create_generator_from_angle(k, ivl.min())?;
                ignore_status(min_angle_gen.trim(k, &mut gen_curve_ivl, TrimMode::Notify))?;

                if degenerate_ivl {
                    // add Trimmed GenCurve to output
                    out.curves_3d.push(min_angle_gen);
                    out.curve_types.push(TsectCurveType::Coincident);
                    out.deviations.push(0.0);
                } else {
                    // add 4 boundary curves to output

                    // Trim a genCurve at the interval end angle
                    let mut max_angle_gen = me.create_generator_from_angle(k, ivl.max())?;
                    ignore_status(max_angle_gen.trim(k, &mut gen_curve_ivl, TrimMode::Notify))?;

                    // bottom Circle
                    let bot_sweep_arc =
                        sweep_arc(k, circle_center, x_axis, y_axis, *ivl, circle_radius)?;

                    // get Top Circle Center and Radius from Solution EndPoint (status
                    // ignored upstream; the point is read)
                    let end_point = this_gen.evaluate_point(k, solution.end.param(0)?)?;
                    let top_circle_center = origin + z_axis.dot(end_point - origin) * z_axis;
                    let top_circle_radius = top_circle_center.distance(end_point);
                    let top_sweep_arc = sweep_arc(
                        k,
                        top_circle_center,
                        x_axis,
                        y_axis,
                        *ivl,
                        top_circle_radius,
                    )?;

                    // add all 4 curves to the output
                    out.curves_3d.push(min_angle_gen);
                    out.curves_3d.push(max_angle_gen);
                    out.curves_3d.push(bot_sweep_arc);
                    out.curves_3d.push(top_sweep_arc);
                    for _ in 0..4 {
                        out.curve_types.push(TsectCurveType::RegionBoundary);
                        out.deviations.push(0.0);
                    }
                }
            }
        }
    }

    for _ in 0..out.curves_3d.len() {
        out.surface1_uv_curves.push(None);
        out.surface2_uv_curves.push(None);
    }

    // all done
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::curve::CurveKind;
    use crate::curve::line::Line;

    fn k() -> Kernel {
        Kernel::default()
    }

    fn cone() -> SurfOfRevolution {
        let mut l = Line::from_points(Vec3::new(2.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 3.0), 3);
        l.make_nurb().unwrap();
        SurfOfRevolution::create_canonical(
            &k(),
            Curve::from(l),
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 1.0),
            false,
        )
        .unwrap()
    }

    fn cylinder(r: f64, z0: f64, z1: f64) -> SurfOfRevolution {
        let mut l = Line::from_points(Vec3::new(r, 0.0, z0), Vec3::new(r, 0.0, z1), 3);
        l.make_nurb().unwrap();
        SurfOfRevolution::create_canonical(&k(), Curve::from(l), Vec3::ZERO, Vec3::Z, false)
            .unwrap()
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Me(Extent2d);

    impl SolutionObject for Me {
        fn kind(&self) -> ObjectKind {
            ObjectKind::Surface
        }
        fn natural_uv_domain(&self) -> Option<Extent2d> {
            Some(self.0)
        }
    }

    #[test]
    fn step_to_nurbs_round_trip() {
        let k = k();
        let s = cone();
        let nurbs = s
            .convert_uv_from_step_to_nurbs(&k, Point2::new(90.0, 0.5))
            .unwrap();
        let back = s.convert_uv_from_nurbs_to_step(&k, nurbs).unwrap();
        assert!((back.x - 90.0).abs() < 1e-9 && (back.y - 0.5).abs() < 1e-12);
        let d = s
            .convert_domain_from_nurbs_to_step(&k, &s.base().natural_uv_domain())
            .unwrap();
        assert!((d.max().x - 360.0).abs() < 1e-9);
    }

    #[test]
    fn drop_point_onto_the_cone() {
        let k = k();
        let s = cone();
        let dom = s.base().natural_uv_domain();
        let me = Me(dom);
        let sols = s
            .drop_point_fast(
                &k,
                &dom,
                SolverOperation::Minimize,
                Vec3::new(0.0, 3.0, 1.5),
                None,
                1e-6,
                None,
                SolutionRequested::All,
                Some(me),
            )
            .unwrap();
        assert!(!sols.is_empty());
        let best = sols.get_at(0).unwrap();
        let p = s
            .base()
            .evaluate_point(Point2::new(
                best.start.parameters[0],
                best.start.parameters[1],
            ))
            .unwrap();
        assert!(p.x.abs() < 1e-9 && p.y > 1.0);
        // a non-planar test is not this one's job: a point outside the sweep fails
        let mut half = cone();
        half.anal_uv_domain = Extent2d::from_bounds(0.0, 0.0, 90.0, 1.0);
        assert!(half
            .drop_point_fast::<Me>(
                &k,
                &dom,
                SolverOperation::Minimize,
                Vec3::new(0.0, -3.0, 1.5),
                None,
                1e-6,
                None,
                SolutionRequested::All,
                None,
            )
            .is_err());
    }

    #[test]
    fn coaxial_cylinders_meet_in_a_circle_region() {
        let k = k();
        let a = cylinder(2.0, 0.0, 2.0);
        let b = cylinder(2.0, 1.0, 3.0);
        let (more, x) = a
            .intersect_with_surf_of_revolution(
                &k,
                &a.base().natural_uv_domain(),
                &b,
                &b.base().natural_uv_domain(),
                Some(1e-6),
            )
            .unwrap();
        assert!(!more);
        assert!(!x.curves_3d.is_empty());
        assert_eq!(x.curves_3d.len(), x.surface1_uv_curves.len());
        // a cone meets the cylinder r = 1.5 in one circle
        let c = cone();
        let d = cylinder(1.5, -1.0, 4.0);
        let (more, x) = c
            .intersect_with_surf_of_revolution(
                &k,
                &c.base().natural_uv_domain(),
                &d,
                &d.base().natural_uv_domain(),
                Some(1e-6),
            )
            .unwrap();
        assert!(!more);
        assert_eq!(x.curve_types, vec![TsectCurveType::Crossing]);
        // the same surface needs the general intersector
        let (more, _) = c
            .intersect_with_surf_of_revolution(
                &k,
                &c.base().natural_uv_domain(),
                &c,
                &c.base().natural_uv_domain(),
                Some(1e-6),
            )
            .unwrap();
        assert!(more);
        // no tolerance: from the bounding box (no face, so no zone tolerance)
        let (more, x) = c
            .intersect_with_surf_of_revolution(
                &k,
                &c.base().natural_uv_domain(),
                &d,
                &d.base().natural_uv_domain(),
                None,
            )
            .unwrap();
        assert!(!more);
        assert_eq!(x.curve_types, vec![TsectCurveType::Crossing]);
    }

    #[test]
    fn rebuild_reparameterize_and_transform() {
        let k = k();
        let mut s = cone();
        s.rebuild_step_from_nurb_parameters(&k).unwrap();
        assert!(s.have_polar_conversion());
        assert!(matches!(
            s.gen_curve().unwrap().kind(),
            CurveKind::BSpline(_)
        ));
        let mut r = cone();
        r.reparameterize(&k, &Extent2d::from_bounds(0.0, 0.0, 2.0, 3.0))
            .unwrap();
        let d = r.base().natural_uv_domain();
        assert_eq!((d.max().x, d.max().y), (2.0, 3.0));
        let mut t = cone();
        let mut rf = Axis2Placement::IDENTITY;
        rf.rotate_about_axis_at_point(PI / 2.0, Vec3::ZERO, Vec3::X);
        t.transform(&k, &rf, Some(Vec3::new(2.0, 2.0, 2.0)))
            .unwrap();
        assert!(t.position().z_axis().y.abs() > 0.99);
        assert!(t
            .transform(&k, &rf, Some(Vec3::new(1.0, 2.0, 1.0)))
            .is_err());
        let c = s.copy();
        assert!(c.approx_eq(&k, &s).unwrap());
    }

    #[test]
    fn update_analytical_domain_follows_the_nurb() {
        let k = k();
        let mut s = cone();
        let d = s.base().natural_uv_domain();
        s.update_analytical_domain(&k, &d).unwrap();
        let a = s.step_uv_domain();
        assert!((a.max().x - 360.0).abs() < 1e-9 && (a.max().y - 1.0).abs() < 1e-12);
    }
}
