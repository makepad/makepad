//! `SmSurfaceDropLib`, part 3: the general curve drop `SmSurface::DropCurve`
//! ([`Surface::drop_curve_base`]): the curve start point is dropped to the surface
//! (one start per seam side and pole direction), then every span of the curve cache is
//! walked with [`DropPt::get_next_point`], tested with [`DropPt::test_span_accuracy`]
//! and subdivided until it passes, and the passing spans are collected into UV
//! B-spline curves ([`add_to_curve`], [`flush_curve`]).
//!
//! Upstream: `source/SMLib/src/SmSurfaceDropLib.cpp`.
//!
//! | Rust                                  | Upstream                                  |
//! |---------------------------------------|-------------------------------------------|
//! | [`Surface::drop_curve`] | `SmSurface::DropCurve` (the virtual call: plane override, else the base body) |
//! | [`Surface::drop_curve_face_tested`] | `SmSurface::DropCurve` of a face-owned surface (the start drop's `PointClassify` trim test on the caller's face) |
//! | [`Surface::drop_curve_base`], [`DropCurveOut`] | `SmSurface::DropCurve` (the base body, called non-virtually) |
//!
//! Deviations:
//! - **Outputs.** `rdMaxDropToSurf`, `rdMaxApproxDev` and `rUVCurves` are returned in a
//!   [`DropCurveOut`] (on an error upstream deletes the curves and leaves both
//!   distances at their initial `0.0`, so nothing is lost). The optional failure
//!   record `pOptDropCurveFail` is a [`DropFailRecord`] (the record with the handles
//!   of the surface and the curve), filled as upstream on every path, also before an
//!   error is returned; `pUVCurvesArg` has no handle and stays `None`. Without a
//!   record nothing is recorded (upstream fills a local one nobody reads).
//!   `bCreatingUVTrimCurve` is not used upstream and is dropped.
//! - **Virtual call.** This is the base body: the `SmPlane` override is
//!   [`crate::surface::plane::Plane::drop_curve`], which calls this one for the curves
//!   it does not handle itself. [`Surface::drop_curve`] dispatches on the surface kind.
//! - **Statuses.** Checked (`SER`) as upstream: the start evaluation, the global point
//!   solve, `DropIsoCurve`, the span evaluations, `TestSpanAccuracy`, the span stack
//!   and `sm_FlushCrv`. Ignored as upstream: both `SetProperties` calls,
//!   `sm_AddToCurve` (`SE`), `FindDegenParamForDirection` (a failure, or a solve that
//!   did not converge, only records `SM_ERR_BAD_FIND_DEGEN_PARAM`),
//!   `EditParameterization` of the pole line, `SetMinMax` of the span intervals and
//!   `SurfaceCurveMaxDistanceBetween` (its outputs are `0.0` after a failure, as
//!   upstream initialises them). `GetNextPoint` returns a status upstream keeps
//!   (see [`DropPt::get_next_point`]); a Rust error from it becomes its code
//!   ([`SolidError::Upstream`]'s code, `SM_ERR_INVALID_INPUT` for
//!   [`SolidError::InvalidInput`], else `SM_ERR`). [`SolidError::Unported`] always
//!   propagates. `IsClosed` answers `false` when it fails (upstream returns a
//!   boolean).
//! - **Undefined upstream.** `SmBSplineCurve::CreateLineSegment`'s status is ignored
//!   before the new curve is dereferenced (a degenerate natural domain makes it
//!   `NULL`), and two curve `EvaluatePoint`s (the curve's scaled zero and the end gap
//!   check) are read after an ignored failure (uninitialised points): here those
//!   errors propagate (plan/upstream-issues.md UB-129).
//! - **`bDegenSurfV`** tests the domain's `XLength`, as upstream (plan/upstream-issues.md
//!   WR-100).
//! - **Temporary state.** The out-of-bounds convergence upstream enables on the surface
//!   for the call (`SmTemporaryChangeValue`) runs on an evaluation copy with the flag
//!   set. The surface cache flags (`m_bPointTestEnabled` off, `m_bProcessBoundaryCurves`
//!   on) are modelled by the start point drop keeping every solution
//!   ([`Surface::global_point_solve_point_test_off`], `SmSurfaceDropLib.cpp:4310-4316`;
//!   through the caller's context, its point-test-off variant) unless the caller's face
//!   test replaces it. The
//!   curve cache is built fresh (`make_curve_cache`); a failed build is
//!   `SM_ERR_NULL_POINTER` (`NER`).
//! - **Offset surface.** `SmOffsetSurface(1.0, *this, FALSE, lSingularities)` is a
//!   [`DropOffset`]; its evaluations (unit `surface.offset_surface`) return
//!   [`SolidError::Unported`], so every drop that reaches a non-degenerate curve tangent
//!   in `SetProperties` is unported.
//! - **Curve on surface.** The final distance check builds an `SmCrvOnSurf` of each UV
//!   curve on an evaluation copy of the surface (`clone_for_evaluation`, which keeps
//!   the owner and so the face tolerance).
//! - **Bounded subdivision.** The span stack of the subdivision loop is at most
//!   [`MAX_SPAN_DEPTH`] drop points deep (upstream fixtures reach about 15: a span
//!   passes once shorter than `1/8000` of the interval; upstream is unbounded,
//!   plan/upstream-issues.md HANG-38); deeper is
//!   [`SolidError::NotConverged`]. No compared case reaches the second `GetNextPoint` of
//!   a span yet (the offset surface defers first), so the bound, the span stack and
//!   `sm_AddToCurve` are untested until `surface.offset_surface` lands.
//! - **Not ported here.** The `SM_DEBUG_CODE` blocks, the `SM_ASSERT`s and the
//!   messages.

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p5::make_curve_cache;
use crate::curve::curve::Curve;
use crate::error::{ignore_status, ignored, Result, SolidError, SM_ERR};
use crate::geom::extent1d::{Extent1d, ExtentPointType};
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::point_set::PointContainment;
use crate::geom::solution_array::{SolutionArray, SolutionObject};
use crate::geom::tol::{approx_tol3d, scaled_zero_of_curve};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, min2, EFF_ZERO, EFF_ZERO_PARAM};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::surface::plane::PlaneDropCurve;
use crate::surface::surface::p1::SurfaceSolveContext;
use crate::surface::surface::p2::{
    CreateUvTrimCurvePathType, DropCurveFailArgs, SurfaceKind, UNSURE,
};
use crate::surface::surface::p4::FaceTest;
use crate::surface::surface::p7::approx_derivative_lengths;
use crate::surface::surface::Surface;
use crate::surface::surface_point_solve::p2::with_out_of_bounds_enabled;

use super::p1::{
    pop_stack, push_point, remove_duplicate_drop_point_solutions, stack_look, DropFailRecord,
    DropPt, SM_ERR_OUTSIDE_OF_DOMAIN, SM_SUCCESS,
};
use super::p2::{
    add_to_curve, flush_curve, DropOffset, SM_ERR_INVALID_INPUT, SM_ERR_NOT_WITHIN_TOLERANCE,
    SS_NONE,
};

/// `SM_ERR_NOT_CONVERGING` (`SmMessages.h`).
pub const SM_ERR_NOT_CONVERGING: i32 = 1022;

/// `SM_ERR_DEGENERATE_SURFACE` (`SmMessages.h`): the drop domain is degenerate.
pub const SM_ERR_DEGENERATE_SURFACE: i32 = 1034;

/// `SM_ERR_BAD_FIND_DEGEN_PARAM` (`SmMessages.h`): `FindDegenParamForDirection` failed
/// for a start point on a pole (recorded only).
pub const SM_ERR_BAD_FIND_DEGEN_PARAM: i32 = 1036;

/// Most drop points the span subdivision stack of [`Surface::drop_curve_base`] holds
/// (upstream: unbounded).
pub const MAX_SPAN_DEPTH: usize = 64;

const OP: &str = "intersect::surface_drop_lib::Surface::drop_curve_base";

/// The caller's face-aware solving service for a drop: the context that builds the
/// surface's cache (and classifies against its face) and the surface's handle in it.
pub struct DropSolveContext<'a, O: SolutionObject> {
    /// The operation-scoped cache and face service.
    pub solver: &'a mut dyn SurfaceSolveContext<O>,
    /// The handle naming the dropped-onto surface in its cache.
    pub handle: Option<O>,
}

/// What [`Surface::drop_curve_base`] made (`DropCurve`'s outputs).
#[derive(Debug, Clone)]
pub struct DropCurveOut {
    /// `rdMaxDropToSurf`: the largest distance between the curve and the surface along
    /// the dropped curves.
    pub max_drop_to_surf: f64,
    /// `rdMaxApproxDev`: the largest deviation of the curve from the surface normal
    /// lines through the dropped curves.
    pub max_approx_dev: f64,
    /// `rUVCurves`: the UV curves (usually one; two for a curve along a seam).
    pub uv_curves: Vec<BSplineCurve>,
}

/// The status upstream stores for a failed `GetNextPoint` (see the module notes).
fn status_of(e: &SolidError) -> i32 {
    match e {
        SolidError::Upstream { code, .. } => *code,
        SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. } => SM_ERR_INVALID_INPUT,
        SolidError::NotConverged { .. } => SM_ERR_NOT_CONVERGING,
        _ => SM_ERR,
    }
}

/// An `SmBoolean` member read as a truth value (`UNSURE` is true).
const fn truthy(b: Option<bool>) -> bool {
    !matches!(b, Some(false))
}

/// An `SmBoolean` argument of `SmDropCurveFail::Set`.
fn sm_bool(b: bool) -> i32 {
    i32::from(b)
}

/// The start UV of solution `i` (`sSolutions[i].m_vStart[0..2]`).
fn start_uv<O: SolutionObject>(sols: &SolutionArray<O>, i: usize) -> Result<Point2> {
    sols.get(i)
        .map(|s| Point2::new(s.start.parameters[0], s.start.parameters[1]))
        .ok_or_else(|| SolidError::sm_err(OP))
}

/// Sets parameter `p` of solution `i`'s start.
fn set_start_param<O: SolutionObject>(
    sols: &mut SolutionArray<O>,
    i: usize,
    p: usize,
    value: f64,
) -> Result<()> {
    let s = sols.get_mut(i).ok_or_else(|| SolidError::sm_err(OP))?;
    let slot = s
        .start
        .parameters
        .get_mut(p)
        .ok_or_else(|| SolidError::sm_err(OP))?;
    *slot = value;
    Ok(())
}

/// `sSolutions.Add(sSolutions[i])`: the index of the copy.
fn add_copy<O: SolutionObject>(sols: &mut SolutionArray<O>, i: usize) -> Result<usize> {
    let s = sols.get_at(i)?;
    sols.add(&s);
    Ok(sols.len() - 1)
}

/// The locals the `SM_SET*_DROP_CURVE_FAIL` macros record.
struct FailState<'a, O> {
    uv_domain: Extent2d,
    interval: Extent1d,
    approx_tol_3d: f64,
    keep_all: i32,
    degen_curve: i32,
    degen_surf_u: i32,
    degen_surf_v: i32,
    contained: i32,
    closed_u: i32,
    closed_v: i32,
    closure: SurfParamType,
    pole_points: Option<&'a [Point3]>,
    pole_normals: Option<&'a [Vec3]>,
    singularities: u64,
    approx_singularities: u64,
    surface: Option<O>,
    curve: Option<O>,
}

/// The output values the macros record at the time of the call.
struct FailOut<'a, O> {
    rd_max_drop: f64,
    rd_max_dev: f64,
    max_drop: f64,
    solutions: Option<&'a SolutionArray<O>>,
    ctrl_poly: Option<&'a [Point3]>,
    knots: Option<&'a [f64]>,
    uv_curves: Option<&'a [BSplineCurve]>,
}

impl<O: SolutionObject> FailState<'_, O> {
    /// `SM_SET<level>_DROP_CURVE_FAIL(status, path)`: level 1 records the inputs and the
    /// degeneracy checks, level 2 adds the surface closure and poles, levels 3 and 4 add
    /// the solutions and UV curves (level 3 also the growing polygon and knots).
    fn set(
        &self,
        record: &mut Option<DropFailRecord<'_, O>>,
        level: u8,
        status: i32,
        path: CreateUvTrimCurvePathType,
        out: &FailOut<'_, O>,
    ) {
        let Some(r) = record.as_mut() else {
            return;
        };
        let mut args = DropCurveFailArgs {
            fail_status: status,
            from_call: 1,
            create_path_type: path,
            has_context: true,
            uv_domain: self.uv_domain,
            surface: self.surface,
            curve: self.curve,
            interval: self.interval,
            approx_tol_3d: self.approx_tol_3d,
            max_drop_to_surf_arg: out.rd_max_drop,
            max_approx_dev: out.rd_max_dev,
            uv_curves_arg: None,
            keep_all_drop_curves: self.keep_all,
            degen_curve: self.degen_curve,
            degen_surf_u: self.degen_surf_u,
            degen_surf_v: self.degen_surf_v,
            drop_domain_is_contained: self.contained,
            max_drop_to_surf: out.max_drop,
            ..DropCurveFailArgs::default()
        };
        if level >= 2 {
            args.closed_u = self.closed_u;
            args.closed_v = self.closed_v;
            args.srf_closure = self.closure;
            args.srf_pole_points = self.pole_points;
            args.srf_pole_normals = self.pole_normals;
            args.singularities = self.singularities;
            args.approx_singularities = self.approx_singularities;
        }
        if level >= 3 {
            args.solutions = out.solutions;
            args.uv_curves = out.uv_curves;
        }
        if level == 3 {
            args.ctrl_poly = out.ctrl_poly;
            args.knots = out.knots;
        }
        r.fail.set(&args);
    }
}

/// The current path type of the record (`pDropCurveFail->m_eCreatePathType`).
fn path_of<O>(record: Option<&DropFailRecord<'_, O>>) -> CreateUvTrimCurvePathType {
    record.map_or(CreateUvTrimCurvePathType::Unknown, |r| {
        r.fail.create_path_type
    })
}

/// Sets the record's path type when it is still unknown.
fn default_path<O>(record: &mut Option<DropFailRecord<'_, O>>, path: CreateUvTrimCurvePathType) {
    if let Some(r) = record.as_mut() {
        if r.fail.create_path_type == CreateUvTrimCurvePathType::Unknown {
            r.fail.create_path_type = path;
        }
    }
}

/// `SmCrvOnSurf(*pBSC, *this, NULL, 0).SurfaceCurveMaxDistanceBetween(ivl, curve,
/// ivl.min, ivl.max, 20, tol, NULL, ..)` (status ignored: `0.0, 0.0` after a failure).
fn uv_curve_gap(
    k: &Kernel,
    surface: &Surface,
    uv_curve: &BSplineCurve,
    curve: &Curve,
    approx_tol_3d: f64,
) -> Result<(f64, f64)> {
    let ivl = uv_curve.natural_interval();
    let crv_on_surf = CrvOnSurf::new(
        k,
        Curve::from(uv_curve.clone()),
        surface.clone_for_evaluation()?,
        None,
        0,
    )?;
    let gap = ignored(crv_on_surf.surface_curve_max_distance_between(
        k,
        &ivl,
        curve,
        ivl.min(),
        ivl.max(),
        20,
        approx_tol_3d,
        None,
    ))?;
    Ok(gap.map_or((0.0, 0.0), |g| (g.max_dist, g.max_dev)))
}

impl Surface {
    /// `SmSurface::DropCurve(context, uv_domain, curve, interval, approx_tol_3d,
    /// rdMaxDropToSurf, rdMaxApproxDev, rUVCurves, bKeepAllDropCurves,
    /// pOptDropCurveFail, bCreatingUVTrimCurve)`, the virtual call dispatched on this
    /// surface's kind: `SmPlane` overrides it ([`crate::surface::plane::Plane::drop_curve`],
    /// run with this surface's [`Surface::owner`]; it ignores `record`, passing `NULL` on
    /// to the general body), every other kind runs the base body
    /// ([`Surface::drop_curve_base`]; `creating_uv_trim_curve` is unused there, as
    /// upstream). The declaration's defaults are `keep_all_drop_curves = true`,
    /// `record = None` and `creating_uv_trim_curve = true`.
    ///
    /// The UV curves are returned as [`Curve`]s: a plane's drop can give an `SmLine`
    /// (upstream keeps the `SmLine` behind the `SmBSplineCurve*`), the base body's are
    /// B-splines.
    ///
    /// # Errors
    ///
    /// Those of [`crate::surface::plane::Plane::drop_curve`] and
    /// [`Surface::drop_curve_base`].
    pub fn drop_curve<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        keep_all_drop_curves: bool,
        record: Option<DropFailRecord<'_, O>>,
        creating_uv_trim_curve: bool,
    ) -> Result<PlaneDropCurve> {
        if let SurfaceKind::Plane(p) = self.kind() {
            return p.drop_curve_owned_by(
                k,
                uv_domain,
                curve,
                interval,
                approx_tol_3d,
                keep_all_drop_curves,
                creating_uv_trim_curve,
                self,
            );
        }
        let d = self.drop_curve_base::<O>(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol_3d,
            keep_all_drop_curves,
            record,
        )?;
        Ok(PlaneDropCurve {
            max_drop_to_surf: d.max_drop_to_surf,
            max_approx_dev: d.max_approx_dev,
            uv_curves: d.uv_curves.into_iter().map(Curve::from).collect(),
        })
    }

    /// Drops `curve` over `interval` onto this surface within `uv_domain`
    /// (`SmSurface::DropCurve`, the base body): UV B-spline curves with about the
    /// curve's parameterisation, one per start point the curve start drops to (two for
    /// a curve along a seam), and the largest drop distance and normal-line deviation
    /// seen. Curves that wander off the surface give no curve; with
    /// `keep_all_drop_curves` false and more than one curve, only those within
    /// `approx_tol_3d` are kept. `record` receives the failure (or success) data.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a degenerate surface domain, a domain outside the natural domain
    /// or a start point that drops nowhere; those of the checked callees (see the
    /// module notes); [`SolidError::NotConverged`] past [`MAX_SPAN_DEPTH`];
    /// [`SolidError::Unported`] (the offset surface, a face-owned surface).
    pub fn drop_curve_base<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        keep_all_drop_curves: bool,
        record: Option<DropFailRecord<'_, O>>,
    ) -> Result<DropCurveOut> {
        self.drop_curve_base_in(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol_3d,
            keep_all_drop_curves,
            record,
            None,
            None,
        )
    }

    /// [`Surface::drop_curve_base`] whose start point drop's trim test is `classify`.
    ///
    /// # Errors
    ///
    /// As [`Surface::drop_curve_base`], and those of `classify`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn drop_curve_base_face_tested<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        keep_all_drop_curves: bool,
        record: Option<DropFailRecord<'_, O>>,
        classify: FaceTest<'_>,
    ) -> Result<DropCurveOut> {
        self.drop_curve_base_in(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol_3d,
            keep_all_drop_curves,
            record,
            Some(classify),
            None,
        )
    }

    /// [`Surface::drop_curve_base`] under the caller's face context: the start point
    /// drop's cache lookup (`smsurf_GetSurfaceCache` of a face-owned surface) is built
    /// by `context`, so the face's trim boundary test is available.
    ///
    /// # Errors
    ///
    /// As [`Surface::drop_curve_base`], and those of the context.
    #[allow(clippy::too_many_arguments)]
    pub fn drop_curve_base_with_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        keep_all_drop_curves: bool,
        record: Option<DropFailRecord<'_, O>>,
        context: DropSolveContext<'_, O>,
    ) -> Result<DropCurveOut> {
        self.drop_curve_base_in(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol_3d,
            keep_all_drop_curves,
            record,
            None,
            Some(context),
        )
    }

    /// [`Surface::drop_curve`] under the caller's face context (see
    /// [`Surface::drop_curve_base_with_context`]); the plane override runs with the
    /// same context for its general fallback.
    ///
    /// # Errors
    ///
    /// As [`Surface::drop_curve`], and those of the context.
    #[allow(clippy::too_many_arguments)]
    pub fn drop_curve_with_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        keep_all_drop_curves: bool,
        record: Option<DropFailRecord<'_, O>>,
        creating_uv_trim_curve: bool,
        context: DropSolveContext<'_, O>,
    ) -> Result<PlaneDropCurve> {
        if let SurfaceKind::Plane(p) = self.kind() {
            return p.drop_curve_owned_by_with_context(
                k,
                uv_domain,
                curve,
                interval,
                approx_tol_3d,
                keep_all_drop_curves,
                creating_uv_trim_curve,
                self,
                Some(context),
            );
        }
        let d = self.drop_curve_base_with_context::<O>(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol_3d,
            keep_all_drop_curves,
            record,
            context,
        )?;
        Ok(PlaneDropCurve {
            max_drop_to_surf: d.max_drop_to_surf,
            max_approx_dev: d.max_approx_dev,
            uv_curves: d.uv_curves.into_iter().map(Curve::from).collect(),
        })
    }

    /// [`Surface::drop_curve`] of a face's surface by a caller that holds the face: the
    /// start point drop's trim boundary test (`pFace->PointClassify` in
    /// `GlobalPointSolve`'s fast drop) is `classify`, as in
    /// [`Surface::global_point_solve_face_tested`].
    ///
    /// # Errors
    ///
    /// As [`Surface::drop_curve`], and those of `classify`.
    #[allow(clippy::too_many_arguments)]
    pub fn drop_curve_face_tested<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        keep_all_drop_curves: bool,
        record: Option<DropFailRecord<'_, O>>,
        creating_uv_trim_curve: bool,
        classify: FaceTest<'_>,
    ) -> Result<PlaneDropCurve> {
        if let SurfaceKind::Plane(p) = self.kind() {
            return p.drop_curve_owned_by_face_tested(
                k,
                uv_domain,
                curve,
                interval,
                approx_tol_3d,
                keep_all_drop_curves,
                creating_uv_trim_curve,
                self,
                classify,
            );
        }
        let d = self.drop_curve_base_in::<O>(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol_3d,
            keep_all_drop_curves,
            record,
            Some(classify),
            None,
        )?;
        Ok(PlaneDropCurve {
            max_drop_to_surf: d.max_drop_to_surf,
            max_approx_dev: d.max_approx_dev,
            uv_curves: d.uv_curves.into_iter().map(Curve::from).collect(),
        })
    }

    /// [`Surface::drop_curve_base`] whose start point drop's trim test is `face_test`
    /// (`None`: the surface's own, as [`Surface::global_point_solve`]).
    #[allow(clippy::too_many_arguments)]
    fn drop_curve_base_in<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol_3d: f64,
        keep_all_drop_curves: bool,
        mut record: Option<DropFailRecord<'_, O>>,
        face_test: Option<FaceTest<'_>>,
        context: Option<DropSolveContext<'_, O>>,
    ) -> Result<DropCurveOut> {
        // `SmApproxTol3d sApproxTol3d`: the constructor maps SM_USE_DEFAULT to zone/2
        // and stores fabs (SmTol.cpp:109, SmTol.h:167-168), so upstream runs with |tol|.
        let approx_tol_3d = approx_tol3d(k, approx_tol_3d);

        // init output
        let mut out = DropCurveOut {
            max_drop_to_surf: 0.0,
            max_approx_dev: 0.0,
            uv_curves: Vec::new(),
        };
        if let Some(r) = record.as_mut() {
            r.fail.un_init();
        }

        // locals
        let s_interval = *interval;
        let curve_scaled_zero = scaled_zero_of_curve(k, curve)?;
        let degen_curve = curve.is_degenerate(k, curve_scaled_zero, Some(interval))?;
        let mut start = DropPt::new(s_interval.min(), Some(degen_curve));
        let mut end = DropPt::new(s_interval.max(), Some(degen_curve));
        let mut max_drop_to_surf = 0.0;
        let mut max_approx_dev = 0.0;

        // watch out for degenerate UVDomains
        let s_uv_domain = if uv_domain.x_length() >= EFF_ZERO && uv_domain.y_length() >= EFF_ZERO {
            *uv_domain
        } else {
            self.natural_uv_domain_with_nurb(k)?
        };
        let degen_surf_u = s_uv_domain.x_length() < EFF_ZERO;
        let degen_surf_v = s_uv_domain.x_length() < EFF_ZERO; // (sic) WR-100
        let contained =
            s_uv_domain.is_contained_by(&self.natural_uv_domain_with_nurb(k)?, EFF_ZERO);

        let mut fs: FailState<'_, O> = FailState {
            uv_domain: *uv_domain,
            interval: *interval,
            approx_tol_3d,
            keep_all: sm_bool(keep_all_drop_curves),
            degen_curve: sm_bool(degen_curve),
            degen_surf_u: sm_bool(degen_surf_u),
            degen_surf_v: sm_bool(degen_surf_v),
            contained: sm_bool(contained),
            closed_u: UNSURE,
            closed_v: UNSURE,
            closure: SurfParamType::Unknown,
            pole_points: None,
            pole_normals: None,
            singularities: 0,
            approx_singularities: 0,
            surface: record.as_ref().and_then(|r| r.surface),
            curve: record.as_ref().and_then(|r| r.curve),
        };
        let early = FailOut {
            rd_max_drop: 0.0,
            rd_max_dev: 0.0,
            max_drop: 0.0,
            solutions: None,
            ctrl_poly: None,
            knots: None,
            uv_curves: None,
        };

        // no work - degen surf domain
        if degen_surf_u || degen_surf_v {
            fs.set(
                &mut record,
                1,
                SM_ERR_DEGENERATE_SURFACE,
                CreateUvTrimCurvePathType::DegenSurfaceFail,
                &early,
            );
            return Err(SolidError::sm_err(OP));
        }

        // check state - given UVDomain must fit within the Surface's Natural UVDomain
        if !contained {
            fs.set(
                &mut record,
                1,
                SM_ERR_OUTSIDE_OF_DOMAIN,
                CreateUvTrimCurvePathType::DropDomainUncontainedFail,
                &early,
            );
            return Err(SolidError::sm_err(OP));
        }

        // UV Tolerances - proportional to surface 3d/UV mapping size
        let srf_nat_domain = self.natural_uv_domain_with_nurb(k)?;
        let project_size = approx_derivative_lengths(k, self, &srf_nat_domain)?;
        let uv_tol_x = approx_tol_3d / (project_size.x + 1.0);
        let uv_tol_y = approx_tol_3d / (project_size.y + 1.0);

        // temporarily enable slight out-of-bounds convergence (on an evaluation copy)
        let oob = with_out_of_bounds_enabled(self)?;
        let this = oob.as_ref().unwrap_or(self);

        // surface singularities and their pole points
        let sings = this.get_singularities(k, true, true)?;
        let singularities = sings.singular;
        let pole_points: [Point3; 4] = sings.pole_points.unwrap_or([Point3::UNDEF; 4]);
        let pole_normals: [Vec3; 4] = sings.pole_normals.unwrap_or([Vec3::UNDEF; 4]);

        // to reduce tolerances - define an offset surface
        let mut offset = DropOffset::new(1.0, singularities);

        // Characterize Surface seams and singularities
        let mut singular_direction = SurfParamType::Neither;
        let closed_u = ignored(this.is_closed(
            k,
            &srf_nat_domain,
            SurfParamType::U,
            Some(approx_tol_3d),
            false,
        ))?
        .is_some_and(|r| r.0);
        let closed_v = ignored(this.is_closed(
            k,
            &srf_nat_domain,
            SurfParamType::V,
            Some(approx_tol_3d),
            false,
        ))?
        .is_some_and(|r| r.0);
        let srf_closure = if closed_u && closed_v {
            SurfParamType::Both
        } else if closed_u {
            SurfParamType::U
        } else if closed_v {
            SurfParamType::V
        } else {
            SurfParamType::Neither
        };
        fs.closed_u = sm_bool(closed_u);
        fs.closed_v = sm_bool(closed_v);
        fs.closure = srf_closure;
        fs.pole_points = Some(&pole_points);
        fs.pole_normals = Some(&pole_normals);
        fs.singularities = u64::from(singularities);
        fs.approx_singularities = u64::from(sings.approx_singular);

        // set sStart's Curve Start Point, 1stDeriv, and 2nd Deriv
        let pd = curve.evaluate(k, start.t, 2, true, true)?;
        for (dst, src) in start.crv_pd.iter_mut().zip(pd.iter()) {
            *dst = *src;
        }

        // Drop curve StartPoint to surface (SM_SO_MINIMIZE, all solutions; the cache's
        // point testing is off for the whole call, `sChange1`, SmSurfaceDropLib.cpp:4315:
        // the face's cache comes from the context, its trims are not tested)
        let mut solutions: SolutionArray<O> = if let Some(ctx) = context {
            let mut solutions = SolutionArray::new();
            this.global_point_solve_into_with_context_point_test_off(
                k,
                ctx.handle,
                None,
                ctx.solver,
                &s_uv_domain,
                SolverOperation::Minimize,
                start.crv_pd[0],
                max2(approx_tol_3d / 100.0, EFF_ZERO),
                None,
                SolutionRequested::All,
                &mut solutions,
            )?;
            solutions
        } else if let Some(classify) = face_test {
            this.global_point_solve_face_tested(
                k,
                fs.surface,
                &s_uv_domain,
                SolverOperation::Minimize,
                start.crv_pd[0],
                max2(approx_tol_3d / 100.0, EFF_ZERO),
                None,
                SolutionRequested::All,
                classify,
            )?
        } else {
            // the cache's point testing is off for the whole call (`sChange1`,
            // SmSurfaceDropLib.cpp:4315): GlobalPointSolve keeps every solution
            this.global_point_solve_point_test_off(
                k,
                fs.surface, // `this`'s handle when the caller gave a record (minimize ignores it)
                &s_uv_domain,
                SolverOperation::Minimize,
                start.crv_pd[0],
                max2(approx_tol_3d / 100.0, EFF_ZERO),
                None,
                SolutionRequested::All,
            )?
        };

        // there should always be at least one solution
        if solutions.is_empty() {
            return Err(SolidError::sm_err(OP));
        }

        // Classify the 1st start DropSolution point (status ignored upstream)
        start.uv_pd[0] = start_uv(&solutions, 0)?;
        ignore_status(start.set_properties(
            k,
            this,
            &mut offset,
            singularities,
            approx_tol_3d,
            &s_uv_domain,
            true,
        ))?;

        // the first drop point is not over the surface
        if start.good_drop == Some(false) {
            fs.set(
                &mut record,
                2,
                SM_ERR_OUTSIDE_OF_DOMAIN,
                CreateUvTrimCurvePathType::FirstDropUncontainedFail,
                &early,
            );
            return Ok(out);
        }

        // remove duplicate solutions
        remove_duplicate_drop_point_solutions(&mut solutions, uv_tol_x, uv_tol_y);

        // load 1st solution into sStart
        start.uv_pd[0] = start_uv(&solutions, 0)?;

        // Characterize DropPoint loc: bOnPole
        let on_pole = singularities != SS_NONE && {
            let s = this.is_singularity_with_poles(
                k,
                start.uv_pd[0],
                u64::from(singularities),
                Some(&pole_points),
                approx_tol_3d,
                false,
            )?;
            singular_direction = s.direction;
            s.singular
        };

        // For DropToPolePts, finish setting sSolutions[ii].m_vStart UV values
        if on_pole {
            if degen_curve {
                // Drop DegenCurve to Pole: the UV line along the singular boundary
                let uv_min = srf_nat_domain.min();
                let uv_mid = srf_nat_domain.mid();
                let uv_max = srf_nat_domain.max();
                let (uv_start, uv_end) = if singular_direction == SurfParamType::U
                    || singular_direction == SurfParamType::Both
                {
                    if start.uv_pd[0].y < uv_mid.y {
                        (
                            Point3::new(uv_min.x, uv_min.y, 0.0),
                            Point3::new(uv_max.x, uv_min.y, 0.0),
                        )
                    } else {
                        (
                            Point3::new(uv_max.x, uv_max.y, 0.0),
                            Point3::new(uv_min.x, uv_max.y, 0.0),
                        )
                    }
                } else if start.uv_pd[0].x > uv_mid.x {
                    (
                        Point3::new(uv_max.x, uv_min.y, 0.0),
                        Point3::new(uv_max.x, uv_max.y, 0.0),
                    )
                } else {
                    (
                        Point3::new(uv_min.x, uv_max.y, 0.0),
                        Point3::new(uv_min.x, uv_min.y, 0.0),
                    )
                };

                // (upstream ignores this status and dereferences the curve)
                let mut uv_trim_curve = BSplineCurve::create_line_segment(2, uv_start, uv_end)?;
                ignore_status(uv_trim_curve.edit_parameterization(interval))?;

                // set output (no record: upstream returns before the success record)
                out.uv_curves.push(uv_trim_curve);
                out.max_drop_to_surf = start.drop_to_surf;
                out.max_approx_dev = start.approx_dev;
                return Ok(out);
            }

            // Drop NonDegen Curve to Pole
            for ii in 0..solutions.len() {
                start.uv_pd[0] = start_uv(&solutions, ii)?;

                // find degenParam value at a singularity that matches the dropCurve UV
                // direction
                let found = match this.find_degen_param_for_direction(
                    k,
                    start.uv_pd[0],
                    start.crv_pd[1],
                    true,
                    singular_direction,
                    0.0001,
                ) {
                    Ok(d) => {
                        start.uv_pd[0] = d.uv;
                        d.converged
                    }
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => false,
                };

                if found {
                    if singular_direction == SurfParamType::U {
                        set_start_param(&mut solutions, ii, 0, start.uv_pd[0].x)?;
                    }
                    if singular_direction == SurfParamType::V {
                        set_start_param(&mut solutions, ii, 1, start.uv_pd[0].y)?;
                    }
                } else {
                    fs.set(
                        &mut record,
                        4,
                        SM_ERR_BAD_FIND_DEGEN_PARAM,
                        CreateUvTrimCurvePathType::BadFindDegenParamWarning,
                        &FailOut {
                            solutions: Some(&solutions),
                            uv_curves: Some(&out.uv_curves),
                            ..early
                        },
                    );
                }
            }

            // remove duplicate solutions
            remove_duplicate_drop_point_solutions(&mut solutions, uv_tol_x, uv_tol_y);

            // load 1st solution into sStart
            start.uv_pd[0] = start_uv(&solutions, 0)?;
        }

        // Characterize DropPoint loc: eOnSeam
        let (mut u_type, mut v_type) =
            srf_nat_domain.classify_point(start.uv_pd[0], uv_tol_x, Some(uv_tol_y));
        let on_end = |t: ExtentPointType| t == ExtentPointType::Start || t == ExtentPointType::End;
        let on_seam = if on_end(u_type) && closed_u && on_end(v_type) && closed_v {
            SurfParamType::Both
        } else if on_end(u_type) && closed_u {
            SurfParamType::U
        } else if on_end(v_type) && closed_v {
            SurfParamType::V
        } else {
            SurfParamType::Neither
        };

        // For DropToSeamPts, copy the solution across the seam when the curve runs
        // along it
        if on_seam != SurfParamType::Neither {
            let n_orig_sols = solutions.len();
            for ii in 0..n_orig_sols {
                start.uv_pd[0] = start_uv(&solutions, ii)?;
                (u_type, v_type) =
                    s_uv_domain.classify_point(start.uv_pd[0], uv_tol_x, Some(uv_tol_y));
                let mut poc_u = if on_seam == SurfParamType::U || on_seam == SurfParamType::Both {
                    start.is_moving_along_boundary(
                        k,
                        this,
                        &srf_nat_domain,
                        uv_tol_x,
                        uv_tol_y,
                        5.0,
                        SurfParamType::U,
                    )?
                } else {
                    PointContainment::Unknown
                };
                let mut poc_v = if on_seam == SurfParamType::V || on_seam == SurfParamType::Both {
                    start.is_moving_along_boundary(
                        k,
                        this,
                        &srf_nat_domain,
                        uv_tol_x,
                        uv_tol_y,
                        5.0,
                        SurfParamType::V,
                    )?
                } else {
                    PointContainment::Unknown
                };

                // leaving across the seam: start on the other side
                if poc_u == PointContainment::Outside {
                    let u = if u_type == ExtentPointType::Start {
                        srf_nat_domain.max().x
                    } else {
                        srf_nat_domain.min().x
                    };
                    set_start_param(&mut solutions, ii, 0, u)?;
                }
                if poc_v == PointContainment::Outside {
                    let v = if v_type == ExtentPointType::Start {
                        srf_nat_domain.max().y
                    } else {
                        srf_nat_domain.min().y
                    };
                    set_start_param(&mut solutions, ii, 1, v)?;
                }

                // unclassified: treat as moving along the seam
                if poc_u == PointContainment::Unknown
                    && (on_seam == SurfParamType::U || on_seam == SurfParamType::Both)
                {
                    poc_u = PointContainment::OnBoundary;
                }
                if poc_v == PointContainment::Unknown
                    && (on_seam == SurfParamType::V || on_seam == SurfParamType::Both)
                {
                    poc_v = PointContainment::OnBoundary;
                }

                // moving along the seam: one start on each side
                if poc_u == PointContainment::OnBoundary {
                    let jj = add_copy(&mut solutions, ii)?;
                    set_start_param(&mut solutions, ii, 0, srf_nat_domain.min().x)?;
                    set_start_param(&mut solutions, jj, 0, srf_nat_domain.max().x)?;
                }
                if poc_v == PointContainment::OnBoundary {
                    let jj = add_copy(&mut solutions, ii)?;
                    set_start_param(&mut solutions, ii, 1, srf_nat_domain.min().y)?;
                    set_start_param(&mut solutions, jj, 1, srf_nat_domain.max().y)?;
                }
            }

            // remove duplicate solutions
            remove_duplicate_drop_point_solutions(&mut solutions, uv_tol_x, uv_tol_y);
            start.uv_pd[0] = start_uv(&solutions, 0)?;
        }

        // Trace a UVTrimCurve for every start drop point
        let num_drop_points = solutions.len();
        let mut curve_cache = None;
        for ii in 0..num_drop_points {
            // a trace from this start point that is abandoned must not leave its drop
            // distances in the reported maximums
            let max_drop_to_surf_before_trace = max_drop_to_surf;
            let max_approx_dev_before_trace = max_approx_dev;

            // set Start
            start.t = s_interval.min();
            start.uv_pd[0] = start_uv(&solutions, ii)?;

            // skip start points outside the drop domain
            let uv_tol = EFF_ZERO_PARAM
                * (1.0
                    + min2(
                        s_uv_domain.u_interval().length(),
                        s_uv_domain.v_interval().length(),
                    ));
            if !s_uv_domain.contains_point(start.uv_pd[0], uv_tol) {
                continue;
            }

            // try the iso curve shortcut
            let iso = this.drop_iso_curve(
                k,
                &s_uv_domain,
                curve,
                &s_interval,
                start.uv_pd[0],
                approx_tol_3d,
            )?;
            start.drop_to_surf = iso.max_drop_to_surf;
            start.approx_dev = iso.max_approx_dev;
            if iso.successful {
                if let Some(c) = iso.iso_uv_curve {
                    out.uv_curves.push(c);
                }
                if max_drop_to_surf < start.drop_to_surf {
                    max_drop_to_surf = start.drop_to_surf;
                }
                if max_approx_dev < start.approx_dev {
                    max_approx_dev = start.approx_dev;
                }
                continue;
            }

            // the UV curve being built
            let mut ctrl_poly: Vec<Point3> = Vec::new();
            let mut knots: Vec<f64> = Vec::new();
            let mut have_start_uv_tan = false;
            let mut fix_bc_change = false;

            // the curve cache (made once)
            if curve_cache.is_none() {
                curve_cache = Some(match make_curve_cache::<()>(k, curve, None) {
                    Ok(c) => c,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => return Err(SolidError::null_pointer(OP)),
                });
            }
            let Some(cache) = curve_cache.as_ref() else {
                return Err(SolidError::null_pointer(OP));
            };
            let span_count = cache.span_count();

            // drop every curve cache span
            for jj in 0..span_count {
                let mut ivl = *cache.span(jj)?.interval();

                // skip spans before the start
                if ivl.max() <= start.t {
                    continue;
                }

                // start the span at the start (bNULLIvl stays FALSE: `&=`)
                let null_ivl = false;
                if ivl.min() > start.t {
                    ignore_status(ivl.set_min_max(start.t, ivl.max()))?;
                }

                // skip spans outside the interval
                if null_ivl || s_interval.are_disjoint(&ivl, 0.0) {
                    continue;
                }

                // trim spans to the interval
                if !ivl.is_contained_by(&s_interval, EFF_ZERO) {
                    let Ok(tmp_ivl) = ivl.intersect(&s_interval) else {
                        continue;
                    };

                    // skip tiny pieces
                    if curve.approximate_length(k, &tmp_ivl, 5, false)?.0 < approx_tol_3d {
                        continue;
                    }
                    ivl = tmp_ivl;
                }

                // span ends
                start.t = ivl.min();
                end.t = ivl.max();

                // span start curve point and derivatives
                let pd = curve.evaluate(k, start.t, 2, true, true)?;
                for (dst, src) in start.crv_pd.iter_mut().zip(pd.iter()) {
                    *dst = *src;
                }

                // drop the span start (status ignored upstream)
                if !have_start_uv_tan {
                    ignore_status(start.set_properties(
                        k,
                        this,
                        &mut offset,
                        singularities,
                        approx_tol_3d,
                        &s_uv_domain,
                        true,
                    ))?;
                    if start.drop_to_surf.abs() > max_drop_to_surf {
                        max_drop_to_surf = start.drop_to_surf.abs();
                    }
                    if start.approx_dev > max_approx_dev {
                        max_approx_dev = start.approx_dev;
                    }
                }

                // drop the span end
                let mut ret_status = match start.get_next_point(
                    k,
                    this,
                    &mut offset,
                    singularities,
                    Some(&pole_points),
                    &s_uv_domain,
                    srf_closure,
                    curve,
                    approx_tol_3d,
                    end.t,
                    &mut end,
                ) {
                    Ok(c) => c,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(e) => status_of(&e),
                };
                if ret_status == SM_SUCCESS {
                    if end.drop_to_surf.abs() > max_drop_to_surf {
                        max_drop_to_surf = end.drop_to_surf.abs();
                    }
                    if end.approx_dev > max_approx_dev {
                        max_approx_dev = end.approx_dev;
                    }
                }

                // leaving the domain or hitting a pole in the middle of the curve
                if (truthy(end.on_pole) || truthy(end.leaving_domain))
                    && !interval.is_value_on_boundary(end.t, EFF_ZERO).0
                {
                    ret_status = SM_ERR_OUTSIDE_OF_DOMAIN;
                }

                if ret_status != SM_SUCCESS {
                    // walked off the edge of the surface close to the curve end
                    if ret_status == SM_ERR_OUTSIDE_OF_DOMAIN
                        && start.on_boundary == Some(true)
                        && start.leaving_domain == Some(true)
                        && knots.len() > 1
                    {
                        let end_ivl = Extent1d::from_min_max(start.t, s_interval.max());
                        let approx_length = curve.approximate_length(k, &end_ivl, 5, false)?.0;
                        if approx_length < approx_tol_3d {
                            let curve_end = curve.evaluate_point(k, s_interval.max())?;
                            let end_gap_length = start.pd_3d[0].distance(curve_end);
                            if end_gap_length < approx_tol_3d {
                                if let Some(last) = knots.last_mut() {
                                    *last = s_interval.max();
                                }
                            }
                            break;
                        }
                    }

                    // a drop point not quite on the surface normal line
                    if ret_status == SM_ERR_NOT_CONVERGING
                        || ret_status == SM_ERR_NOT_WITHIN_TOLERANCE
                    {
                        if keep_all_drop_curves && end.approx_dev < approx_tol_3d {
                            ret_status = SM_SUCCESS;
                            end.good_drop = Some(true);
                        } else {
                            fs.set(
                                &mut record,
                                3,
                                SM_ERR_NOT_WITHIN_TOLERANCE,
                                CreateUvTrimCurvePathType::KeptBeyondTolDrop,
                                &FailOut {
                                    rd_max_drop: out.max_drop_to_surf,
                                    rd_max_dev: out.max_approx_dev,
                                    max_drop: max_drop_to_surf,
                                    solutions: Some(&solutions),
                                    ctrl_poly: Some(&ctrl_poly),
                                    knots: Some(&knots),
                                    uv_curves: Some(&out.uv_curves),
                                },
                            );
                        }
                    }

                    // still failing: give up on this start point
                    if ret_status != SM_SUCCESS {
                        let path = path_of(record.as_ref());
                        fs.set(
                            &mut record,
                            3,
                            ret_status,
                            path,
                            &FailOut {
                                rd_max_drop: out.max_drop_to_surf,
                                rd_max_dev: out.max_approx_dev,
                                max_drop: max_drop_to_surf,
                                solutions: Some(&solutions),
                                ctrl_poly: Some(&ctrl_poly),
                                knots: Some(&knots),
                                uv_curves: Some(&out.uv_curves),
                            },
                        );
                        default_path(&mut record, CreateUvTrimCurvePathType::DropStartPointFail);
                        ctrl_poly.clear();
                        knots.clear();
                        max_drop_to_surf = max_drop_to_surf_before_trace;
                        max_approx_dev = max_approx_dev_before_trace;
                        break;
                    }
                }

                // subdivide the span until every piece passes the accuracy test
                let mut stack: Vec<f64> = Vec::new();
                push_point(end.t, end.uv_pd, &mut stack);
                let mut stop_spans = false;
                while !stack.is_empty() {
                    // set SpanEnd values from the top of the stack
                    (end.t, end.uv_pd) = stack_look(&stack)?;
                    ignore_status(ivl.set_min_max(start.t, end.t))?;

                    // fix boundary tangents
                    let _fix_bt_change = start.fix_boundary_tangents(
                        &s_uv_domain,
                        &mut end,
                        uv_tol_x,
                        uv_tol_y,
                        approx_tol_3d,
                    );

                    // test the span
                    let acc = start.test_span_accuracy(
                        k,
                        this,
                        &s_uv_domain,
                        curve,
                        &end,
                        approx_tol_3d,
                        uv_tol_x,
                        uv_tol_y,
                        true,
                    )?;
                    let mut span_passes = acc.passes;
                    let drop_to_surf = acc.max_drop_to_surf;
                    let approx_dev = acc.max_approx_dev;

                    // accept very short spans
                    if !span_passes && (end.t - start.t).abs() < s_interval.length() / 8000.0 {
                        span_passes = true;
                    }

                    if span_passes {
                        if max_drop_to_surf < drop_to_surf {
                            max_drop_to_surf = drop_to_surf;
                        }
                        if max_approx_dev < approx_dev {
                            max_approx_dev = approx_dev;
                        }

                        // add the span to the curve (status ignored, SE)
                        ignore_status(add_to_curve(
                            k,
                            this,
                            &s_uv_domain,
                            &start,
                            &end,
                            &mut ctrl_poly,
                            &mut knots,
                            approx_tol_3d,
                            uv_tol_x,
                            uv_tol_y,
                            &mut out.uv_curves,
                            &mut fix_bc_change,
                        ))?;

                        // next span starts at this end
                        start = end;
                        start.from_left_u = Some(!truthy(end.from_left_u));
                        start.from_left_v = Some(!truthy(end.from_left_v));
                        have_start_uv_tan = true;
                        (end.t, end.uv_pd) = pop_stack(&mut stack)?;
                        continue;
                    }

                    // the span fails: find a split point that drops
                    let mut found_good_one = false;
                    for _kk in 0..6 {
                        end.t = (start.t + end.t) / 2.0;
                        ret_status = match start.get_next_point(
                            k,
                            this,
                            &mut offset,
                            singularities,
                            Some(&pole_points),
                            &s_uv_domain,
                            srf_closure,
                            curve,
                            approx_tol_3d,
                            end.t,
                            &mut end,
                        ) {
                            Ok(c) => c,
                            Err(e) if e.is_unported() => return Err(e),
                            Err(e) => status_of(&e),
                        };
                        if ret_status == SM_SUCCESS {
                            found_good_one = true;
                            break;
                        }
                    }

                    // no split point drops: give up on this start point
                    if !found_good_one {
                        fs.set(
                            &mut record,
                            3,
                            ret_status,
                            CreateUvTrimCurvePathType::SplitDropStepFail,
                            &FailOut {
                                rd_max_drop: out.max_drop_to_surf,
                                rd_max_dev: out.max_approx_dev,
                                max_drop: max_drop_to_surf,
                                solutions: Some(&solutions),
                                ctrl_poly: Some(&ctrl_poly),
                                knots: Some(&knots),
                                uv_curves: Some(&out.uv_curves),
                            },
                        );
                        ctrl_poly.clear();
                        knots.clear();
                        max_drop_to_surf = max_drop_to_surf_before_trace;
                        max_approx_dev = max_approx_dev_before_trace;
                        stop_spans = true;
                        break;
                    }

                    if max_drop_to_surf < end.drop_to_surf {
                        max_drop_to_surf = end.drop_to_surf;
                    }
                    if max_approx_dev < end.approx_dev {
                        max_approx_dev = end.approx_dev;
                    }

                    // the split point becomes the new span end
                    if stack.len() / 5 >= MAX_SPAN_DEPTH {
                        return Err(SolidError::NotConverged {
                            operation: OP,
                            iterations: MAX_SPAN_DEPTH as u32,
                        });
                    }
                    push_point(end.t, end.uv_pd, &mut stack);
                }
                if stop_spans {
                    break;
                }
            }

            // output the curve built for this start point
            flush_curve(
                &s_uv_domain,
                &mut ctrl_poly,
                &mut knots,
                uv_tol_x,
                uv_tol_y,
                &mut out.uv_curves,
                &mut fix_bc_change,
            )?;
        }

        // no curves (no record, as upstream)
        if out.uv_curves.is_empty() {
            return Ok(out);
        }

        // set output
        out.max_drop_to_surf = max_drop_to_surf;
        out.max_approx_dev = max_approx_dev;

        // measure every curve against the 3D curve
        let mut distances: Vec<f64> = Vec::new();
        let mut deviations: Vec<f64> = Vec::new();
        for uv_curve in &out.uv_curves {
            let (drop_to_surf, approx_dev) = uv_curve_gap(k, this, uv_curve, curve, approx_tol_3d)?;
            if drop_to_surf > out.max_drop_to_surf {
                out.max_drop_to_surf = drop_to_surf;
            }
            if approx_dev > out.max_approx_dev {
                out.max_approx_dev = approx_dev;
            }
            distances.push(drop_to_surf);
            deviations.push(approx_dev);
        }

        // keep only the curves within tolerance
        if !keep_all_drop_curves
            && (out.max_drop_to_surf > approx_tol_3d || out.max_approx_dev > approx_tol_3d)
            && out.uv_curves.len() > 1
        {
            let mut better = Vec::new();
            let mut drop_to_surf = 0.0;
            let mut approx_dev = 0.0;
            for ((c, &dist), &dev) in out.uv_curves.drain(..).zip(&distances).zip(&deviations) {
                if dist < approx_tol_3d {
                    better.push(c);
                    if drop_to_surf < dist {
                        drop_to_surf = dist;
                    }
                    if approx_dev < dev {
                        approx_dev = dev;
                    }
                }
            }
            out.max_drop_to_surf = drop_to_surf;
            out.max_approx_dev = approx_dev;
            out.uv_curves = better;
        }

        // capture the output for debug and heal purposes
        let path = path_of(record.as_ref());
        fs.set(
            &mut record,
            4,
            SM_SUCCESS,
            path,
            &FailOut {
                rd_max_drop: out.max_drop_to_surf,
                rd_max_dev: out.max_approx_dev,
                max_drop: max_drop_to_surf,
                solutions: Some(&solutions),
                ctrl_poly: None,
                knots: None,
                uv_curves: Some(&out.uv_curves),
            },
        );
        default_path(&mut record, CreateUvTrimCurvePathType::DropCurveOkay);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::vector2d::Vec2;
    use crate::nurbs::crv_basic::Curve as NurbCurve;
    use crate::nurbs::math::{CPoint, NOW};
    use crate::surface::plane::Plane;
    use crate::surface::surface::p2::DropCurveFail;
    use crate::surface::surface::p4::NoObject;

    fn plane() -> Surface {
        Surface::from(Plane::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ))
    }

    /// A degree 1 B-spline collapsed to the point `p`.
    fn point_curve(p: Point3) -> Curve {
        let pw = vec![
            CPoint::new(p.x, p.y, p.z, NOW),
            CPoint::new(p.x, p.y, p.z, NOW),
        ];
        let nurb = NurbCurve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        Curve::from(BSplineCurve::from_nurb(nurb, 3, false))
    }

    #[test]
    fn uncontained_domain_is_recorded() {
        let k = Kernel::default();
        let mut fail: DropCurveFail<NoObject> = DropCurveFail::default();
        let r = plane().drop_curve_base(
            &k,
            &Extent2d::from_bounds(-1.0, 0.0, 1.0, 1.0),
            &point_curve(Point3::new(0.5, 0.5, 0.0)),
            &Extent1d::from_min_max(0.0, 1.0),
            1e-4,
            true,
            Some(DropFailRecord {
                fail: &mut fail,
                surface: None,
                curve: None,
            }),
        );
        assert!(matches!(r, Err(SolidError::Upstream { code: SM_ERR, .. })));
        assert_eq!(fail.fail_status, SM_ERR_OUTSIDE_OF_DOMAIN);
        assert_eq!(
            fail.create_path_type,
            CreateUvTrimCurvePathType::DropDomainUncontainedFail
        );
        assert_eq!(fail.drop_domain_is_contained, 0);
        assert_eq!(fail.closed_u, UNSURE);
    }

    /// The start drops along the normal (a good first drop), but a point curve's
    /// `GetNextPoint` never succeeds, so no curve is made (the C++ agrees: difftest
    /// fixture `plane_point_above`).
    #[test]
    fn point_off_the_plane_gives_no_curve() {
        let k = Kernel::default();
        let mut fail: DropCurveFail<NoObject> = DropCurveFail::default();
        let out = plane()
            .drop_curve_base(
                &k,
                &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
                &point_curve(Point3::new(0.5, 0.25, 0.5)),
                &Extent1d::from_min_max(0.0, 1.0),
                1e-4,
                true,
                Some(DropFailRecord {
                    fail: &mut fail,
                    surface: None,
                    curve: None,
                }),
            )
            .unwrap();
        assert!(out.uv_curves.is_empty());
        assert_eq!(
            fail.create_path_type,
            CreateUvTrimCurvePathType::DropStartPointFail
        );
        assert_eq!(fail.degen_curve, 1);
        assert_eq!(fail.srf_closure, SurfParamType::Neither);
    }

    /// `SmApproxTol3d` takes `fabs` (and maps `SM_USE_DEFAULT`) on entry
    /// (SmTol.cpp:109, SmTol.h:167-168): a negative tolerance runs as its absolute
    /// value (review of PR #136).
    #[test]
    fn negative_tolerance_runs_as_its_absolute_value() {
        let k = Kernel::default();
        let run = |tol: f64| {
            let mut fail: DropCurveFail<NoObject> = DropCurveFail::default();
            let r = plane().drop_curve_base(
                &k,
                &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
                &point_curve(Point3::new(0.5, 0.5, 0.1)),
                &Extent1d::from_min_max(0.0, 1.0),
                tol,
                true,
                Some(DropFailRecord {
                    fail: &mut fail,
                    surface: None,
                    curve: None,
                }),
            );
            let o = r.map(|o| (o.uv_curves.len(), o.max_drop_to_surf, o.max_approx_dev));
            (format!("{o:?}"), fail.fail_status, fail.create_path_type)
        };
        assert_eq!(run(-1.0), run(1.0));
        assert_eq!(run(-1.0e-4), run(1.0e-4));
        assert_eq!(
            run(crate::geom::tol::USE_DEFAULT),
            run(k.tolerances.zone_tol_3d / 2.0)
        );
    }
}
