//! `SmPlane`, part 1: the plane object ([`Plane`]: the analytic definition
//! `O + u * su * X + v * sv * Y` over a STEP domain next to its bilinear NURB), its
//! constructors, copies, evaluation, projection, point and line solvers, iso-curves,
//! curve drops, coincidence test, re-parameterisation, u/v swap and the analytic data
//! rebuilt from the NURB.
//!
//! Upstream: `source/SMLib/src/SmPlane.cpp`, `source/SMLib/inc/SmPlane.h`.
//!
//! | Rust                                          | Upstream                                          |
//! |-----------------------------------------------|---------------------------------------------------|
//! | [`Plane`]                                     | `SmPlane`                                         |
//! | [`Plane::new`]                                | `SmPlane(Origin, XAxis, YAxis, UVScale, UVDomain, cpContext)` |
//! | [`Plane::from_normal`]                        | `SmPlane(Origin, PlaneNormal, cpContext)`         |
//! | [`Plane::create_canonical`]                   | `CreateCanonical`                                 |
//! | `Clone`, [`Plane::copy`]                      | copy constructor, `Copy` (both overloads)         |
//! | `Drop` (implicit)                             | `~SmPlane`                                        |
//! | [`Plane::approx_eq`]                          | `operator==`                                      |
//! | [`Plane::adjust_step_uv_domain`]              | `AdjustSTEPUVDomain`                              |
//! | [`Plane::coincidence_check`], [`Coincidence`] | `CoincidenceCheck`                                |
//! | [`Plane::create_iso_parametric_curve`], [`PlaneIsoCurve`] | `CreateIsoParametricCurve`            |
//! | [`Plane::drop_curve`], [`PlaneDropCurve`], `drop_curve_owned_by` (crate) | `DropCurve`                |
//! | [`Plane::drop_point_fast`]                    | `DropPointFast`                                   |
//! | [`Plane::evaluate_point`], [`Plane::evaluate_point_fast`] | `EvaluatePoint`, `EvaluatePointFast`  |
//! | [`Plane::evaluate`], [`Plane::evaluate_simple`] | `Evaluate`, `EvaluateSimple` (inline)           |
//! | [`Plane::evaluate_normal`]                    | `EvaluateNormal`                                  |
//! | [`Plane::global_line_intersect`], [`Plane::global_line_intersect_in`] | `GlobalLineIntersect`                             |
//! | [`Plane::trim_plane_line_to_plane_domain`]    | `TrimPlaneLineToPlaneDomain`                      |
//! | [`is_grazing_intersection`]                   | `sm_IsGrazingIntersection` (static)               |
//! | [`Plane::is_bounded`]                         | `IsBounded`                                       |
//! | [`Plane::is_singularity`], [`Plane::is_singularity_with_poles`] | `IsSingularity` (both overloads) |
//! | [`Plane::is_planar`], [`Plane::is_on_seam`], [`Plane::is_closed`] | `IsPlanar`, `IsOnSeam`, `IsClosed` (inline) |
//! | [`Plane::make_nurb`]                          | `MakeNurb`                                        |
//! | [`Plane::project_point_to_uv_domain`]         | `ProjectPointToUVDomain`                          |
//! | [`Plane::reparameterize`]                     | `Reparameterize`                                  |
//! | [`Plane::toggle_swap_uv_bit`]                 | `ToggleSwapUVBit`                                 |
//! | [`Plane::rebuild_step_from_nurb_parameters`]  | `RebuildSTEPFromNURBParameters`                   |
//! | [`Plane::step_uv_domain`], [`Plane::position`], [`Plane::uv_scale`] | `GetSTEPUVDomain`, `GetPosition`, `GetUVScale` |
//! | [`Plane::is_kind_of`], [`Plane::TYPE`], [`Plane::type_string`], [`Plane::class_string`] | `IsKindOf`, `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`Plane::base`], [`Plane::base_with_nurb`], [`Plane::natural_uv_domain`] | the `SmBSplineSurface` part, `GetOrCreateGwNurbPointer`, `GetNaturalUVDomain` (inherited) |
//! | [`Plane::max_analytic_domain`], [`Plane::is_parallel_to_vector`], [`Plane::reverse`], [`Plane::swap_uv`] | `SmBSplineSurface::GetMaxAnalyticDomain`, `IsParallelToVector`, `Reverse`, `SwapUV` (inherited, re-dispatched) |
//! | [`Plane::create_extended_surface`], [`Plane::create_extended_surface_dir`] | `SmBSplineSurface::CreateExtendedSurface` (both overloads; inherited, re-dispatched to the `IsSingularity` / `IsClosed` overrides) |
//!
//! Deviations:
//! - **Derived class.** `SmPlane` derives from `SmBSplineSurface`; a [`Plane`] holds
//!   its B-spline part ([`Plane::base`]) next to the analytic data, and is the
//!   [`SurfaceKind::Plane`] kind of [`Surface`]. Inherited methods whose upstream
//!   bodies call `SmPlane` overrides through the virtual table (`MakeNurb`,
//!   `ToggleSwapUVBit`, `RebuildSTEPFromNURBParameters`, `IsPlanar`, `IsSingularity`,
//!   `IsClosed`) are re-dispatched here. A missing NURB is made with [`Plane::make_nurb`] where
//!   upstream calls `GetOrCreateGwNurbPointer`: `&mut self` methods make it in
//!   place; read-only methods (upstream `const` methods that make it through a
//!   `const_cast`, such as `GetNaturalUVDomain`) make it from the analytic data and
//!   keep it on the plane ([`Plane::base_with_nurb`]: the B-spline part's NURB slot is
//!   filled once from `&self`), so later code sees it as upstream does: the
//!   `IntersectWithPlane` "has a NURB" test (`surface::plane::p2`), `ToggleSwapUVBit`
//!   (which leaves the NURB alone), `operator==` (which compares NURBs) and the copy
//!   constructor, which makes the source's missing NURB and keeps it there
//!   ([`Plane::copy`]). Internal stand-in copies (`Surface::from(self.clone())`, for
//!   upstream's inherited methods on `this`) therefore also fill the plane's NURB, as
//!   the inherited bodies upstream make it on `this`.
//! - **Arguments.** Out-params become return values; `SmContext` arguments and the
//!   arguments upstream marks `NotUsed` (`DropPointFast`'s pointing vector and
//!   solution request, `Evaluate`'s sides and zero-tangent flags,
//!   `CreateIsoParametricCurve`'s tolerance, `DropCurve`'s fail record, the
//!   singularity tolerances and pole data) are dropped. Solutions of
//!   [`Plane::drop_point_fast`] name `this` through an optional caller handle.
//!   `CoincidenceCheck`'s containing surface is always `this` and is not returned.
//! - **`Evaluate`** writes beyond its array upstream when `bOnlyUpperHalf` is given
//!   with more `u` than `v` derivatives; the port returns `InvalidInput` there, and
//!   for derivative orders above `MAX_DERIVATIVE` (upstream writes into a caller-sized
//!   array; the port allocates it, as `BSplineSurface::evaluate` bounds it).
//!   Entries upstream leaves untouched are zero: the unwritten lower half is 0 here and
//!   `SM_UNDEF` upstream (a fresh caller's `SmVector3d` array); unreachable today (no
//!   caller reads it).
//! - **`Reparameterize`** computes a new origin only in debug code: the release
//!   build keeps the old origin while changing the scales and the domain, so the
//!   analytic plane moves unless the new domain starts where the old one did. The
//!   port keeps that (`plan/findings.md`). Its `Notify` calls only drop caches and
//!   inform owners and are omitted, as for B-splines.
//! - **Co-ported callees.** `SmTol::MapTo2d(SmTol3d, SmVector2d, SmSurface)` (unit
//!   `geom.tol`, waiting on `SmSurface::Evaluate1stDerivatives`) is ported here for a
//!   plane (the only surface it is called with). `Evaluate1stDerivatives` /
//!   `Evaluate2ndDerivatives` of other surfaces go through `surface::surface::p7`.
//! - **Unported callees.** `IsPlanar` of a curve-bounded surface (its base
//!   `CalculateBoundingBox`, `surface.surface_p1`), `DropPoint` (`surface.surface_p5`),
//!   the iso curves of `CreateNaturalUVTrimCurves` for kinds without a ported override
//!   (`surface.surface_p5`) return [`SolidError::Unported`]. The general
//!   `SmSurface::DropCurve` ([`Surface::drop_curve_base`]) runs on a copy of the plane
//!   with the caller's owner. The general `SmSurface::GlobalLineIntersect` is
//!   [`Surface::base_global_line_intersect`]. Statuses upstream ignores are propagated
//!   where upstream would go on with uninitialised values.

use std::borrow::Cow;

use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::iso_curve::IsoCurve;
use crate::curve::line::{BoundedLineOptions, Line};
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::{
    line_closest_point, line_point_distance, plane_point_distance,
};
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::global_solver::SolverOperation;
use crate::geom::object::ObjectRef;
use crate::geom::solution_array::{
    ObjectKind, Solution, SolutionArray, SolutionObject, SolutionType,
};
use crate::geom::tol::{are_same, scaled_zero, tol3d};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::sqrt;
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::surface_drop_lib::p3::DropSolveContext;
use crate::kernel::Kernel;
use crate::math::{
    max2, min2, rad_to_deg, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_DEG, EFF_ZERO_SQ, EFF_ZERO_SQRT,
    INFINITE_PARAMETER,
};
use crate::nurbs::b_spline_curve::{BSplineCurve, CanonicalData, KnotType};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::{
    BSplineSurface, BSplineSurfaceForm, CanonicalSurfaceData, SurfParamType,
};
use crate::nurbs::knots::Side;
use crate::nurbs::math::MAX_DERIVATIVE;
use crate::surface::surface::p1::{self as surface_p1, SecondDerivatives};
use crate::surface::surface::p2::{Surface, SurfaceKind, PLANE_TYPE};
use crate::surface::surface::p4::FaceTest;
use crate::surface::surface_cache::p1::SurfaceCache;

const OP_EVAL: &str = "surface::plane::p1::Plane::evaluate";
const OP_DROP: &str = "surface::plane::p1::Plane::drop_point_fast";
const OP_LINE: &str = "surface::plane::p1::Plane::global_line_intersect";

/// A plane (`SmPlane`): `P(u, v) = O + u * su * X + v * sv * Y` with the origin `O`
/// and unit axes `X`, `Y` of [`Plane::position`] and the scales `(su, sv)` of
/// [`Plane::uv_scale`], evaluated over the STEP domain [`Plane::step_uv_domain`],
/// next to the bilinear B-spline of that domain ([`Plane::base`]).
#[derive(Debug)]
pub struct Plane {
    /// The `SmBSplineSurface` part (the NURB representation).
    pub(super) base: BSplineSurface,
    /// `m_vPosition`: origin, unit X axis and unit Y axis.
    pub(super) position: Axis2Placement,
    /// `m_vAnalUVDomain`: the analytic (STEP) domain.
    pub(super) anal_uv_domain: Extent2d,
    /// `m_vUVScale`: the u and v scales.
    pub(super) uv_scale: Vec2,
}

/// The result of [`Plane::coincidence_check`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coincidence {
    /// `rbAreCoincident`.
    pub are_coincident: bool,
    /// `rdMaxDistanceBetween`: the largest distance measured (`-1` when none was).
    pub max_distance: f64,
}

/// The result of [`Plane::create_iso_parametric_curve`].
#[derive(Debug, Clone)]
pub struct PlaneIsoCurve {
    /// `rpNewIsoCurve`: the 3D iso line.
    pub curve: Line,
    /// `pOptMaxGap3d`: always `0` (the line is exact).
    pub max_gap_3d: f64,
    /// `pOptUVIsoCurve`: the 2D UV line when asked for and built.
    pub uv_iso_curve: Option<Curve>,
}

/// The result of [`Plane::drop_curve`].
#[derive(Debug, Clone)]
pub struct PlaneDropCurve {
    /// `rdMaxDropToSurf`: the largest distance between the curve and its drop.
    pub max_drop_to_surf: f64,
    /// `rdMaxApproxDev`: the deviation of the UV curves from the ideal drop.
    pub max_approx_dev: f64,
    /// `rUVCurves`: the UV curves (none, or one 2D line or B-spline).
    pub uv_curves: Vec<Curve>,
}

/// The copy constructor ([`Plane::copy`]).
impl Clone for Plane {
    fn clone(&self) -> Self {
        self.copy()
    }
}

impl Plane {
    /// `SmPlane_TYPE`.
    pub const TYPE: i64 = PLANE_TYPE;

    // ---- construction -----------------------------------------------------------------

    /// The bounded plane `origin + u * uv_scale.x * X + v * uv_scale.y * Y` over
    /// `uv_domain` (`SmPlane(Origin, XAxis, YAxis, UVScale, UVDomain, cpContext)`):
    /// the axes are set with `SmAxis2Placement::SetCanonical` (made unit and
    /// perpendicular; its status is ignored). No NURB is made.
    #[must_use]
    pub fn new(
        origin: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        uv_scale: Vec2,
        uv_domain: &Extent2d,
    ) -> Self {
        let mut position = Axis2Placement::IDENTITY;
        // SE: the status is only logged
        let _ = position.set_canonical(origin, x_axis, y_axis);
        Self {
            base: BSplineSurface::new(),
            position,
            anal_uv_domain: *uv_domain,
            uv_scale,
        }
    }

    /// The infinite plane through `origin` with normal `normal` (`SmPlane(Origin,
    /// PlaneNormal, cpContext)`): unit scales, the domain
    /// `[-SM_INFINITE_PARAMETER, SM_INFINITE_PARAMETER]^2` and the axes of
    /// `MakeUnitOrthoVectors` (undefined for a zero normal, as upstream). No NURB is
    /// made.
    #[must_use]
    pub fn from_normal(origin: Point3, normal: Vec3) -> Self {
        let (s_y, s_z) = match normal.make_unit_ortho_vectors(None) {
            Ok([_, y, z]) => (y, z),
            // SE: the outputs keep their default (undefined) values
            Err(_) => (Vec3::UNDEF, Vec3::UNDEF),
        };
        let mut position = Axis2Placement::IDENTITY;
        let _ = position.set_canonical(origin, s_y, s_z);

        // Make the plane m_vAnalUVDomain essentially infinite
        let anal_uv_domain = Extent2d::from_bounds(
            -INFINITE_PARAMETER,
            -INFINITE_PARAMETER,
            INFINITE_PARAMETER,
            INFINITE_PARAMETER,
        );
        Self {
            base: BSplineSurface::new(),
            position,
            anal_uv_domain,
            uv_scale: Vec2::new(1.0, 1.0),
        }
    }

    /// The infinite plane of `origin` (its origin and axes, unit scales, domain
    /// `[-SM_INFINITE_PARAMETER, SM_INFINITE_PARAMETER]^2`) with its NURB
    /// (`CreateCanonical`).
    ///
    /// # Errors
    ///
    /// Those of [`Plane::make_nurb`].
    pub fn create_canonical(origin: &Axis2Placement) -> Result<Self> {
        let uv_scale = Vec2::new(1.0, 1.0);
        let uv_domain = Extent2d::from_bounds(
            -INFINITE_PARAMETER,
            -INFINITE_PARAMETER,
            INFINITE_PARAMETER,
            INFINITE_PARAMETER,
        );
        let mut plane = Self::new(
            origin.origin(),
            origin.x_axis(),
            origin.y_axis(),
            uv_scale,
            &uv_domain,
        );
        plane.make_nurb()?;
        Ok(plane)
    }

    /// An exact copy (`Copy`, the copy constructor): the B-spline part is copied
    /// with the NURB of [`Plane::base_with_nurb`] (the `SmBSplineSurface` copy
    /// constructor makes the source's missing NURB first, through a `const_cast`, and
    /// the source keeps it; a failed make copies none).
    #[must_use]
    pub fn copy(&self) -> Self {
        let _ = self.base_with_nurb();
        self.member_copy()
    }

    /// An evaluation copy that stands for this plane object (upstream's pointer to it):
    /// exact, and sharing this plane's NURB cell, so a NURB a read-only query makes
    /// through the copy is kept on this plane, as upstream keeps it on the object.
    pub(crate) fn clone_shared(&self) -> Self {
        Self {
            base: self.base.clone_exact_shared(),
            position: self.position,
            anal_uv_domain: self.anal_uv_domain,
            uv_scale: self.uv_scale,
        }
    }

    /// Detaches a NURB cell shared with evaluation copies ([`Plane::clone_shared`]).
    pub(crate) fn detach_nurb(&mut self) {
        self.base.detach_nurb();
    }

    /// A copy of every member as is (no NURB is made).
    fn member_copy(&self) -> Self {
        Self {
            base: self.base.copy(),
            position: self.position,
            anal_uv_domain: self.anal_uv_domain,
            uv_scale: self.uv_scale,
        }
    }

    /// Upstream `operator==`: the B-spline parts are equal
    /// ([`BSplineSurface::approx_eq`]) and the positions, domains and scales are
    /// equal within upstream's tolerances.
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        // low work
        if core::ptr::eq(self, other) {
            return true;
        }

        // first check the base
        if !self.base.approx_eq(&other.base) {
            return false;
        }

        // check equivalence of these objects
        self.position.approx_eq(&other.position)
            && self.anal_uv_domain.are_same(&other.anal_uv_domain)
            && self.uv_scale.approx_eq(other.uv_scale)
    }

    // ---- accessors --------------------------------------------------------------------

    /// The `SmBSplineSurface` part.
    #[must_use]
    pub const fn base(&self) -> &BSplineSurface {
        &self.base
    }

    /// Whether out-of-bounds evaluation is enabled (`GetOutOfBoundsEnabled` of the
    /// `SmBSplineSurface` part).
    #[must_use]
    pub fn is_out_of_bounds_enabled(&self) -> bool {
        self.base.is_out_of_bounds_enabled()
    }

    /// Sets out-of-bounds evaluation (through `GetOutOfBoundsEnabled`'s reference).
    pub fn set_out_of_bounds_enabled(&mut self, enabled: bool) {
        *self.base.out_of_bounds_enabled_mut() = enabled;
    }

    /// The B-spline part with a NURB (`GetOrCreateGwNurbPointer` from a `const`
    /// method): the stored one, or the one [`Plane::make_nurb`] makes when the plane has
    /// none, which the plane then keeps, as upstream keeps it on the object (made on a
    /// copy of the analytic data and stored once).
    ///
    /// # Errors
    ///
    /// Those of [`Plane::make_nurb`] (the plane stays without NURB).
    pub fn base_with_nurb(&self) -> Result<Cow<'_, BSplineSurface>> {
        if self.base.gw_nurb().is_none() {
            let mut tmp = self.member_copy();
            tmp.make_nurb()?;
            if let Some(made) = tmp.base.nurb.take() {
                self.base.nurb.fill(made);
            }
        }
        Ok(Cow::Borrowed(&self.base))
    }

    /// The B-spline part for an inherited query: with a NURB when one can be made,
    /// else the part without NURB (upstream's `NULL` NURB).
    pub(crate) fn base_for_query(&self) -> Cow<'_, BSplineSurface> {
        self.base_with_nurb().unwrap_or(Cow::Borrowed(&self.base))
    }

    /// Makes the NURB when missing (`GetOrCreateGwNurbPointer` of a non-`const`
    /// method; a failure leaves the plane without NURB).
    pub(super) fn ensure_nurb(&mut self) {
        if self.base.gw_nurb().is_none() {
            let _ = self.make_nurb();
        }
    }

    /// The position: origin and unit axes (`GetPosition`).
    #[must_use]
    pub const fn position(&self) -> &Axis2Placement {
        &self.position
    }

    /// The u and v scales (`GetUVScale`).
    #[must_use]
    pub const fn uv_scale(&self) -> Vec2 {
        self.uv_scale
    }

    /// The analytic (STEP) domain (`GetSTEPUVDomain`).
    #[must_use]
    pub const fn step_uv_domain(&self) -> Extent2d {
        self.anal_uv_domain
    }

    /// The out-of-bounds evaluation flag of the `SmBSplineSurface` part, mutable
    /// (`SmBSplineSurface::GetOutOfBoundsEnabled`, inherited).
    pub fn out_of_bounds_enabled_mut(&mut self) -> &mut bool {
        self.base.out_of_bounds_enabled_mut()
    }

    /// The natural (NURB) domain (`SmBSplineSurface::GetNaturalUVDomain`, inherited):
    /// the knot range of the NURB, made on a copy when missing; `(0, 0, 0, 0)` when
    /// none can be made.
    #[must_use]
    pub fn natural_uv_domain(&self) -> Extent2d {
        self.base_for_query().natural_uv_domain()
    }

    /// [`Plane::natural_uv_domain`] without keeping a made NURB: a missing one is made on
    /// a copy (for the port's own bookkeeping, where upstream makes none).
    pub(crate) fn natural_uv_domain_without_keeping(&self) -> Extent2d {
        if self.base.gw_nurb().is_some() {
            return self.base.natural_uv_domain();
        }
        let mut tmp = self.member_copy();
        match tmp.make_nurb() {
            Ok(()) => tmp.base.natural_uv_domain(),
            Err(_) => self.base.natural_uv_domain(),
        }
    }

    /// `SmPlane_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmPlane_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmPlane_TYPE"
    }

    /// `"SmPlane"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmPlane"
    }

    /// True for `SmPlane_TYPE` and the base types of `SmBSplineSurface` (`IsKindOf`).
    ///
    /// # Errors
    ///
    /// Those of [`BSplineSurface::is_kind_of`].
    pub fn is_kind_of(&self, t: i64) -> Result<bool> {
        if t == Self::TYPE {
            return Ok(true);
        }
        self.base.is_kind_of(t)
    }

    // ---- simple queries ---------------------------------------------------------------

    /// Whether the analytic domain is bounded (`IsBounded`).
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        self.anal_uv_domain.is_bounded()
    }

    /// `IsPlanar`: always planar; the plane point (the origin), the normal (the Z
    /// axis) and the deviation `0`.
    #[must_use]
    pub fn is_planar(&self) -> (Point3, Vec3, f64) {
        (self.position.origin(), self.position.z_axis(), 0.0)
    }

    /// `IsOnSeam`: a plane has no seam (`false`, seam direction
    /// [`SurfParamType::Neither`], no cross-seam points).
    #[must_use]
    pub const fn is_on_seam(&self) -> (bool, SurfParamType) {
        (false, SurfParamType::Neither)
    }

    /// `IsClosed`: never closed, continuity [`ContinuityType::Discontinuous`].
    #[must_use]
    pub const fn is_closed(&self) -> (bool, ContinuityType) {
        (false, ContinuityType::Discontinuous)
    }

    /// `IsSingularity` (point test): a plane has none (`false`,
    /// [`SurfParamType::Neither`]).
    #[must_use]
    pub const fn is_singularity(&self) -> (bool, SurfParamType) {
        (false, SurfParamType::Neither)
    }

    /// `IsSingularity` (with known singular sides and pole points): a plane has none
    /// (`false`, [`SurfParamType::Neither`]).
    #[must_use]
    pub const fn is_singularity_with_poles(&self) -> (bool, SurfParamType) {
        (false, SurfParamType::Neither)
    }

    /// `SmBSplineSurface::CreateExtendedSurface(dDist, ..)` (inherited): the plane's
    /// NURB extended by `dist` across each side, through the `SmPlane` overrides of
    /// `IsSingularity` and `IsClosed` (none, never closed).
    ///
    /// # Errors
    ///
    /// Those of [`Plane::create_extended_surface_dir`].
    pub fn create_extended_surface(
        &self,
        dist: f64,
        extension_continuity: ContinuityType,
    ) -> Result<BSplineSurface> {
        // pass the call along
        self.create_extended_surface_dir(SurfParamType::Both, dist, extension_continuity)
    }

    /// `SmBSplineSurface::CreateExtendedSurface(eExtDirection, dDist, ..)` (inherited):
    /// [`BSplineSurface::create_extended_surface_dir`] of the plane's NURB, with the
    /// virtual `IsSingularity` and `IsClosed` dispatched to the `SmPlane` overrides
    /// ([`Plane::is_singularity`], [`Plane::is_closed`]) as upstream.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when no NURB can be made (upstream copies a `NULL` NURB) and the
    /// errors of the extension.
    pub fn create_extended_surface_dir(
        &self,
        ext_direction: SurfParamType,
        dist: f64,
        extension_continuity: ContinuityType,
    ) -> Result<BSplineSurface> {
        self.base_for_query().create_extended_surface_dispatched(
            ext_direction,
            dist,
            extension_continuity,
            &|_| Ok(self.is_singularity()),
            &|_, _| Ok(self.is_closed().0),
        )
    }

    /// `SmBSplineSurface::GetMaxAnalyticDomain` (inherited): planar, so the natural
    /// domain expanded relatively by 100.
    #[must_use]
    pub fn max_analytic_domain(&self) -> Extent2d {
        let mut dom = self.natural_uv_domain();
        dom.expand_relative(100.0);
        dom
    }

    /// `SmBSplineSurface::IsParallelToVector` (inherited), its `SmPlane` branch: the
    /// normal is perpendicular to `vector` within `SM_EFF_ZERO * (1 + |O|max)`;
    /// `true` for a zero vector (upstream returns the failed `Unitize` status).
    #[must_use]
    pub fn is_parallel_to_vector(&self, vector: Vec3) -> bool {
        let Ok(test_vec) = vector.unitize() else {
            return true;
        };
        let z_axis = self.position.z_axis();
        let tol1 = EFF_ZERO * (1.0 + self.position.origin().max_dimension());
        z_axis.dot(test_vec).abs() < tol1
    }

    // ---- evaluation -------------------------------------------------------------------

    /// The point at `uv`, clamped to the analytic domain unless out-of-bounds
    /// evaluation is enabled (`EvaluatePoint`).
    #[must_use]
    pub fn evaluate_point(&self, uv: Point2) -> Point3 {
        let mut s_uv = uv;

        // clamp UVPoint to UVDomain
        if !self.base.is_out_of_bounds_enabled() && !self.anal_uv_domain.contains_point(s_uv, 0.0) {
            s_uv = self.anal_uv_domain.clamp_point(s_uv);
        }

        // evaluate
        self.position.origin()
            + s_uv.x * self.uv_scale.x * self.position.x_axis()
            + s_uv.y * self.uv_scale.y * self.position.y_axis()
    }

    /// The point at `step_uv`, not clamped (`EvaluatePointFast`).
    #[must_use]
    pub fn evaluate_point_fast(&self, step_uv: Point2) -> Point3 {
        self.position.origin()
            + step_uv.x * self.position.x_axis() * self.uv_scale.x
            + step_uv.y * self.position.y_axis() * self.uv_scale.y
    }

    /// The point and partial derivatives at `uv` (`Evaluate`): `D[i][j]` is the
    /// derivative `i` times in `u` and `j` times in `v`; only the point and the first
    /// derivatives are nonzero. `uv` is clamped as in [`Plane::evaluate_point`].
    ///
    /// # Errors
    ///
    /// `InvalidInput` for `only_upper_half` with more `u` than `v` derivatives
    /// (upstream writes outside its array).
    pub fn evaluate(
        &self,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        only_upper_half: bool,
    ) -> Result<Vec<Vec<Vec3>>> {
        if only_upper_half && highest_u_deriv > highest_v_deriv {
            return Err(SolidError::invalid_input(
                OP_EVAL,
                "only upper half with more u than v derivatives",
            ));
        }
        // upstream writes into a caller-sized array; bound the allocation here (as
        // BSplineSurface::evaluate does)
        if highest_u_deriv > MAX_DERIVATIVE || highest_v_deriv > MAX_DERIVATIVE {
            return Err(SolidError::invalid_input(
                OP_EVAL,
                "derivative order too high",
            ));
        }

        // local
        let mut s_uv = uv;

        // when appropriate - clamp uncontained points
        if !self.base.is_out_of_bounds_enabled() && !self.anal_uv_domain.contains_point(s_uv, 0.0) {
            s_uv = self.anal_uv_domain.clamp_point(s_uv);
        }

        // init output - every entry zero
        let mut d = vec![vec![Vec3::ZERO; highest_v_deriv + 1]; highest_u_deriv + 1];

        // position
        if let Some(p) = d.get_mut(0).and_then(|r| r.get_mut(0)) {
            *p = self.position.origin()
                + s_uv.x * self.position.x_axis() * self.uv_scale.x
                + s_uv.y * self.position.y_axis() * self.uv_scale.y;
        }

        // 1st derivs
        if highest_v_deriv > 0 {
            if let Some(p) = d.get_mut(0).and_then(|r| r.get_mut(1)) {
                *p = self.position.y_axis() * self.uv_scale.y;
            }
        }
        if highest_u_deriv > 0 {
            if let Some(p) = d.get_mut(1).and_then(|r| r.get_mut(0)) {
                *p = self.position.x_axis() * self.uv_scale.x;
            }
        }

        // higher order derivatives already set to zero - no more work to do.
        Ok(d)
    }

    /// [`Plane::evaluate`] (`EvaluateSimple`).
    ///
    /// # Errors
    ///
    /// Those of [`Plane::evaluate`].
    pub fn evaluate_simple(
        &self,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        only_upper_half: bool,
    ) -> Result<Vec<Vec<Vec3>>> {
        self.evaluate(uv, highest_u_deriv, highest_v_deriv, only_upper_half)
    }

    /// The unit normal, the Z axis (`EvaluateNormal`).
    #[must_use]
    pub fn evaluate_normal(&self) -> Vec3 {
        self.position.z_axis()
    }

    /// `SmSurface::Evaluate1stDerivatives(uv, TRUE, TRUE, ..)` of the plane: the point,
    /// `S_u` and `S_v` of [`Plane::evaluate`] (upper half, non-zero tangents).
    fn evaluate_1st_derivatives(&self, uv: Point2) -> Result<(Point3, Vec3, Vec3)> {
        let m = self.evaluate(uv, 1, 1, true)?;
        let at = |i: usize, j: usize| {
            m.get(i)
                .and_then(|r| r.get(j))
                .copied()
                .unwrap_or(Vec3::ZERO)
        };
        Ok((at(0, 0), at(1, 0), at(0, 1)))
    }

    /// `SmTol::MapTo2d(SmTol3d(tol_3d), uv, *this)`: the 3D tolerance mapped to the
    /// UV space of the plane in the direction of its larger first derivative.
    fn map_to_2d(&self, tol_3d: f64, uv: Point2) -> Result<f64> {
        // evaluate SurfacePt = rSurface(rUVTrimCurve(dParam))
        let (_, du, dv) = self.evaluate_1st_derivatives(uv)?;

        // pick the max 1st deriv direction
        let uv_dir = if du.length_squared() > dv.length_squared() {
            Vec2::new(1.0, 0.0)
        } else {
            Vec2::new(0.0, 1.0)
        };

        // pass the call along for Max 1st deriv (yields Min Tol2d value)
        let (_, du, dv) = self.evaluate_1st_derivatives(uv)?;

        // locals
        let uv_domain = self.natural_uv_domain();
        let scaled_zero = scaled_zero();
        let du_size = du.length();
        let dv_size = dv.length();

        // Scale dTol3d to dTolUV in the U and V directions
        let scaled_u = if du_size < scaled_zero {
            uv_domain.u_interval().length() / 100.0
        } else {
            tol_3d / du_size
        };
        let scaled_v = if dv_size < scaled_zero {
            uv_domain.v_interval().length() / 100.0
        } else {
            tol_3d / dv_size
        };

        // get directional Tol2d value when [dDirU dDirV] is not the zero vector
        let size = sqrt(uv_dir.x * uv_dir.x + uv_dir.y * uv_dir.y);
        Ok(if size < scaled_zero {
            min2(scaled_u, scaled_v)
        } else {
            uv_dir.x.abs() / size * scaled_u + uv_dir.y.abs() / size * scaled_v
        })
    }

    /// The UV coordinates of `point` projected onto the infinite plane
    /// (`ProjectPointToUVDomain`).
    #[must_use]
    pub fn project_point_to_uv_domain(&self, point: Point3) -> Point2 {
        let vec = point - self.position.origin();
        let u_projection = self.position.x_axis().dot(vec);
        let v_projection = self.position.y_axis().dot(vec);
        Point2::new(
            u_projection / self.uv_scale.x,
            v_projection / self.uv_scale.y,
        )
    }

    // ---- NURB -------------------------------------------------------------------------

    /// Replaces the NURB by the bilinear B-spline through the four corners of the
    /// analytic domain, knots `[umin, umin, umax, umax] x [vmin, vmin, vmax, vmax]`
    /// (`MakeNurb`).
    ///
    /// # Errors
    ///
    /// Those of [`BSplineSurface::create_canonical`] (the NURB is unchanged).
    pub fn make_nurb(&mut self) -> Result<()> {
        // evaluate and save the 4 3d corner positions of the m_vAnalUVDomain
        let d = self.anal_uv_domain;
        let ctrl_pts = [
            self.evaluate_point_fast(d.evaluate(0.0, 0.0)),
            self.evaluate_point_fast(d.evaluate(0.0, 1.0)),
            self.evaluate_point_fast(d.evaluate(1.0, 0.0)),
            self.evaluate_point_fast(d.evaluate(1.0, 1.0)),
        ];

        // set knot vectors to the corners of the m_vAnalUVDomain extent
        let u_knots = [d.min().x, d.max().x];
        let v_knots = [d.min().y, d.max().y];

        // set endPoint knot multiplicities
        let mults = [2usize, 2];

        // create temporary planar Bspline surface
        let tmp = BSplineSurface::create_canonical(&CanonicalSurfaceData {
            u_degree: 1,
            v_degree: 1,
            control_points: &ctrl_pts,
            form: BSplineSurfaceForm::PlaneSurf,
            u_multiplicities: &mults,
            v_multiplicities: &mults,
            u_knots: &u_knots,
            v_knots: &v_knots,
            knot_type: KnotType::Unspecified,
            weights: None,
            uv_domain: None,
        })?;

        // Swap nurbs - old nurb is deleted with the temporary
        self.base.nurb = tmp.nurb;
        Ok(())
    }

    /// Sets the analytic domain to `new_step_uv_domain` and remakes the NURB, unless
    /// the two domains contain each other within `SM_EFF_ZERO` (`AdjustSTEPUVDomain`).
    ///
    /// # Errors
    ///
    /// Those of [`Plane::make_nurb`].
    pub fn adjust_step_uv_domain(&mut self, new_step_uv_domain: &Extent2d) -> Result<()> {
        // no work - new domain is equal to current domain
        if new_step_uv_domain.is_contained_by(&self.anal_uv_domain, EFF_ZERO)
            && self
                .anal_uv_domain
                .is_contained_by(new_step_uv_domain, EFF_ZERO)
        {
            return Ok(());
        }

        // save the new UVDomain
        self.anal_uv_domain = *new_step_uv_domain;

        // replace the m_pNurb pointer with a new Planar Nurb with the new Extent
        self.make_nurb()
    }

    /// Maps the domain onto `trim_domain` (`Reparameterize`): the NURB is
    /// re-parameterised without changing its shape; the analytic domain becomes
    /// `trim_domain` and the scales `su * |old u| / |new u|`, `sv * |old v| / |new
    /// v|`; the origin is kept (see the module docs).
    ///
    /// # Errors
    ///
    /// Those of [`BSplineSurface::reparameterize`] (`SM_ERR_NULL_POINTER` when no
    /// NURB can be made).
    pub fn reparameterize(&mut self, trim_domain: &Extent2d) -> Result<()> {
        // Get new Analytic parameters
        let scale_x = (self.uv_scale.x * self.anal_uv_domain.x_length()) / trim_domain.x_length();
        let scale_y = (self.uv_scale.y * self.anal_uv_domain.y_length()) / trim_domain.y_length();
        // (sPointMax, sPointMin and sOriginMax are only used by debug code)

        // set NURB domain parameterization - no change in shape
        self.ensure_nurb();
        self.base.reparameterize(trim_domain)?;

        // Set Analytic domain parameterization - no change in shape
        self.anal_uv_domain = *trim_domain;
        self.uv_scale = Vec2::new(scale_x, scale_y);
        Ok(())
    }

    /// Transposes the STEP definition so that `Plane(u, v) = SwapPlane(v, u)`: swaps
    /// the axes, the domain directions and the scales (`ToggleSwapUVBit`). The NURB
    /// is not changed.
    pub fn toggle_swap_uv_bit(&mut self) {
        // m_vPosition swaps XAxis with YAxis
        let origin = self.position.origin();
        let x_axis = self.position.x_axis();
        let y_axis = self.position.y_axis();
        let _ = self.position.set_canonical(origin, y_axis, x_axis);

        // transpose the m_vAnalUVDomain
        self.anal_uv_domain.transpose();

        // Swap the scale factors
        self.uv_scale = Vec2::new(self.uv_scale.y, self.uv_scale.x);
    }

    /// Rebuilds the analytic data from the NURB (`RebuildSTEPFromNURBParameters`): the
    /// domain becomes the natural domain, the scales and axes come from three
    /// corners of the NURB, and the origin is moved so that the domain minimum
    /// evaluates to the first corner (using the old axes).
    ///
    /// # Errors
    ///
    /// Those of the B-spline evaluation ([`BSplineSurface::evaluate_point`]).
    pub fn rebuild_step_from_nurb_parameters(&mut self) -> Result<()> {
        // Make our analytic data match the Nurbs shape.
        self.ensure_nurb();
        let nurb_domain = self.base.natural_uv_domain();

        // Get three corner 3d points, to get position and length scales.
        let pt00 = self.base.evaluate_point(nurb_domain.evaluate(0.0, 0.0))?;
        let pt01 = self.base.evaluate_point(nurb_domain.evaluate(0.0, 1.0))?;
        let pt10 = self.base.evaluate_point(nurb_domain.evaluate(1.0, 0.0))?;

        // Set our scale first: 3d length divided by domain length.
        let mut u_vec = pt10 - pt00;
        let len_3d = u_vec.length();
        self.uv_scale.x = len_3d / nurb_domain.x_length();
        u_vec /= len_3d; // Unitize, for later.

        // V-scale:
        let mut v_vec = pt01 - pt00;
        let len_3d = v_vec.length();
        self.uv_scale.y = len_3d / nurb_domain.y_length();
        v_vec /= len_3d; // Unitize, for later.

        // The origin is where (0,0) would evaluate to.
        let start_vec = nurb_domain.min().x * self.uv_scale.x * self.position.x_axis()
            + nurb_domain.min().y * self.uv_scale.y * self.position.y_axis();

        // The new origin would be the start point minus that vector.
        let _ = self.position.set_canonical(pt00 - start_vec, u_vec, v_vec);

        // Finally update the domain.
        self.anal_uv_domain = nurb_domain;
        Ok(())
    }

    /// `SmBSplineSurface::Reverse` (inherited): reverses the NURB in `param`'s
    /// direction, then rebuilds the analytic data from it.
    ///
    /// # Errors
    ///
    /// Those of [`BSplineSurface::reverse`] and
    /// [`Plane::rebuild_step_from_nurb_parameters`].
    pub fn reverse(&mut self, param: SurfParamType) -> Result<()> {
        self.ensure_nurb();
        self.base.reverse(param)?;

        // give analytics a chance to update their nested STEP data
        self.rebuild_step_from_nurb_parameters()
    }

    /// `SmBSplineSurface::SwapUV` (inherited): records the swap
    /// ([`Plane::toggle_swap_uv_bit`]), transposes the NURB, then rebuilds the
    /// analytic data from it.
    ///
    /// # Errors
    ///
    /// Those of [`BSplineSurface::canonical`], [`BSplineSurface::set_canonical`] (the
    /// swap bit is then restored) and [`Plane::rebuild_step_from_nurb_parameters`].
    pub fn swap_uv(&mut self) -> Result<()> {
        self.swap_uv_notifying(&mut |_| Ok(()))
    }

    /// [`Plane::swap_uv`] with `SetCanonical`'s edit notifications sent through
    /// `notify` ([`BSplineSurface::set_canonical_notifying`]).
    ///
    /// # Errors
    ///
    /// Those of [`Plane::swap_uv`] and of `notify`.
    pub(crate) fn swap_uv_notifying(
        &mut self,
        notify: &mut crate::nurbs::b_spline_surface::p2::EditNotify<'_>,
    ) -> Result<()> {
        // get current surface description
        self.ensure_nurb();
        let c = self.base.canonical()?;

        // let analytic surfaces remember that they have been swapped
        self.toggle_swap_uv_bit();

        // Transpose Weights and control points arrays
        let num_v_cpts = self.base.number_control_points(SurfParamType::V);
        let mut cpts = c.control_points;
        let mut weights = c.weights;
        transpose(&mut cpts, num_v_cpts);
        transpose(&mut weights, num_v_cpts);

        // Transpose Surface definition; on failure, restore the bit
        let status = self.base.set_canonical_notifying(
            &CanonicalSurfaceData {
                u_degree: c.v_degree,
                v_degree: c.u_degree,
                control_points: &cpts,
                form: c.form,
                u_multiplicities: &c.v_multiplicities,
                v_multiplicities: &c.u_multiplicities,
                u_knots: &c.v_knots,
                v_knots: &c.u_knots,
                knot_type: c.knot_type,
                weights: Some(&weights),
                uv_domain: None,
            },
            notify,
        );
        if let Err(e) = status {
            self.toggle_swap_uv_bit();
            return Err(e);
        }
        self.rebuild_step_from_nurb_parameters()
    }

    // ---- solvers ----------------------------------------------------------------------

    /// The point of the plane closest to `test_point` within `uv_domain`
    /// (`DropPointFast`): the projection clamped to the domain, as a single-value
    /// solution `(u, v)` with the distance as value, naming `this`. No solution when
    /// normalising and the projection is outside the domain (by more than the 3D
    /// tolerance mapped to UV), when intersecting farther than
    /// `distance_tolerance`, or when minimising or normalising farther than
    /// `target_distance`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for another operation.
    pub fn drop_point_fast<O: SolutionObject>(
        &self,
        uv_domain: &Extent2d,
        operation: SolverOperation,
        test_point: Point3,
        distance_tolerance: f64,
        target_distance: Option<f64>,
        this: Option<O>,
    ) -> Result<SolutionArray<O>> {
        // check input
        if operation != SolverOperation::Minimize
            && operation != SolverOperation::Normalize
            && operation != SolverOperation::Intersect
        {
            return Err(SolidError::sm_err(OP_DROP));
        }

        // init output
        let mut solutions = SolutionArray::new();

        // Project targetPoint to plane
        let mut uv_found = self.project_point_to_uv_domain(test_point);

        // when NORMALIZING - only use project solutions within the plane domain
        if operation == SolverOperation::Normalize {
            let tol_uv = self.map_to_2d(tol3d(distance_tolerance), uv_found)?;
            if !uv_domain.contains_point(uv_found, tol_uv) {
                return Ok(solutions); // No answer found
            }
        }

        // clamp projectPoint to planeDomain
        uv_found = uv_domain.clamp_point(uv_found);

        // get 3dProjectPoint/TargetPoint gap
        let pnt = self.evaluate_point_fast(uv_found);
        let dist = pnt.distance(test_point);

        // when INTERSECTING skip solutions > DistanceTolerance
        if operation == SolverOperation::Intersect && dist > distance_tolerance {
            return Ok(solutions); // No answer found
        }

        // when MINIMIZING or NORMALIZING skip solutions > TargetDistance
        if let Some(target) = target_distance {
            if (operation == SolverOperation::Minimize || operation == SolverOperation::Normalize)
                && dist > target
            {
                return Ok(solutions); // No answer found
            }
        }

        let mut sol = Solution::new();
        sol.solution_type = SolutionType::SingleValue;
        sol.num_objects = 1;
        sol.objects[0] = this;
        sol.nodes[0] = None;
        sol.num_variables = 2;
        sol.start.solution_value = dist;
        sol.start.parameters[0] = uv_found.x;
        sol.start.parameters[1] = uv_found.y;
        solutions.add(&sol);
        Ok(solutions)
    }

    /// Intersections of the line `line_point + t * line_vector` with the plane over
    /// `uv_domain` (`GlobalLineIntersect`): the line is infinite, a segment over
    /// `line_interval`, or a ray from `t = 0` (`fire_ray`). A line not parallel to
    /// the plane gives at most one single-value solution `(t, u, v)` (Cramer's rule,
    /// `uv` clamped to the domain, within `distance_tolerance`); a line in the plane
    /// gives the range of its part over the domain. Solutions name no objects, except
    /// those of the general intersector (a line in the plane meeting the domain in
    /// a single line parameter), which name `surface`, this plane's handle.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a zero line vector; `SM_ERR` for a vanishing determinant;
    /// [`SolidError::Unported`] from the unported callees of the general
    /// `SmSurface::GlobalLineIntersect` (a line in the plane meeting the domain in a
    /// single line parameter).
    pub fn global_line_intersect<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: O,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        self.global_line_intersect_in(
            k,
            Some(surface),
            None,
            uv_domain,
            line_point,
            line_vector,
            line_interval,
            fire_ray,
            distance_tolerance,
        )
    }

    /// [`Plane::global_line_intersect`] whose general intersector runs over `cache`,
    /// this plane's surface cache when the caller holds it (see
    /// [`Surface::global_line_intersect_in`]); `surface` (optional) is the plane's
    /// handle.
    ///
    /// # Errors
    ///
    /// Those of [`Plane::global_line_intersect`].
    pub fn global_line_intersect_in<O: SolutionObject>(
        &self,
        k: &Kernel,
        surface: Option<O>,
        cache: Option<&mut SurfaceCache<O>>,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
    ) -> Result<SolutionArray<O>> {
        self.global_line_intersect_with_base(
            uv_domain,
            line_point,
            line_vector,
            line_interval,
            fire_ray,
            distance_tolerance,
            |sub_interval| {
                Surface::from(self.clone()).base_global_line_intersect_in(
                    k,
                    surface,
                    cache,
                    uv_domain,
                    line_point,
                    line_vector,
                    Some(sub_interval),
                    fire_ray,
                    distance_tolerance,
                )
            },
        )
    }

    /// The body of [`Plane::global_line_intersect_in`]: `base` is
    /// `SmSurface::GlobalLineIntersect` over the given line interval, which the plane
    /// calls on itself (a caller holding the plane's [`Surface`] runs it there, with
    /// the surface's owner and the caller's cache and face context).
    ///
    /// # Errors
    ///
    /// Those of [`Plane::global_line_intersect`] and of `base`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn global_line_intersect_with_base<O: SolutionObject>(
        &self,
        uv_domain: &Extent2d,
        line_point: Point3,
        line_vector: Vec3,
        line_interval: Option<&Extent1d>,
        fire_ray: bool,
        distance_tolerance: f64,
        base: impl FnOnce(&Extent1d) -> Result<SolutionArray<O>>,
    ) -> Result<SolutionArray<O>> {
        if line_vector.length_squared() < EFF_ZERO_SQ {
            return Err(SolidError::invalid_input(OP_LINE, "zero line vector"));
        }

        // init output
        let mut solutions = SolutionArray::new();

        // locals
        let plane_normal = self.evaluate_normal();
        let angle_tol = 0.0001; // 1/10000 of a degree

        // check for plane/curve coincidence
        if plane_normal.is_perpendicular_to(line_vector, angle_tol) {
            // Line and plane are nearly parallel - have either coincidence or miss.
            // get interval to trim - given, infinite, or semi-infinite (for ray firing)
            let mut ivl = if let Some(i) = line_interval {
                *i
            } else {
                // Drop plane corners to line, to get param range of possible intersection.
                let mut ivl = Extent1d::new();
                let lo = uv_domain.min();
                let hi = uv_domain.max();
                for uv in [
                    lo,
                    Point2::new(lo.x, hi.y),
                    Point2::new(hi.x, lo.y),
                    Point2::new(hi.x, hi.y),
                ] {
                    let pt = self.evaluate_point_fast(uv);
                    let line_param = line_closest_point(line_point, line_vector, pt);
                    ivl.add_value(line_param);
                }
                ivl.expand_relative(1.2);
                ivl
            };
            if fire_ray {
                let _ = ivl.set_min_max(0.0, max2(ivl.max(), 0.0));
            }

            // get untrimmed interval endPoints
            let mut plane_start_pt = line_point + ivl.min() * line_vector;
            let mut plane_end_pt = line_point + ivl.max() * line_vector;

            // project untrimmed interval endPoints to plane UV params
            let uv_start = self.project_point_to_uv_domain(plane_start_pt);
            let uv_end = self.project_point_to_uv_domain(plane_end_pt);

            // build UVLine vector
            let uv_vec = uv_end - uv_start;

            // find interval for UVLine that maps to plane domain
            let Some(trim_ivl) =
                uv_domain.intersect_with_infinite_line(uv_start, uv_vec, None, None)
            else {
                return Ok(solutions);
            };

            // handle round off - max UVLine interval is 0.0 to 1.0
            let d_min = max2(trim_ivl.min(), 0.0);
            let d_max = min2(trim_ivl.max(), 1.0);

            if d_min > d_max + EFF_ZERO {
                return Ok(solutions); // no trimmed portion in area of interest
            }

            // get UVLine 2D trimmed endPoints
            let trim_start = uv_start + d_min * uv_vec;
            let trim_end = uv_start + d_max * uv_vec;

            // get associated 3D trimmed endPoints on Plane
            plane_start_pt = self.evaluate_point_fast(trim_start);
            plane_end_pt = self.evaluate_point_fast(trim_end);

            // get closest point on Line to trimmed plane start point
            let t_start = line_closest_point(line_point, line_vector, plane_start_pt);
            let test_start = line_point + t_start * line_vector;
            let start_dist = test_start.distance(plane_start_pt);

            // quit when plane points are not close to line points
            if start_dist > distance_tolerance {
                return Ok(solutions);
            }

            // get closest point on Line to trimmed plane end point
            let t_end = line_closest_point(line_point, line_vector, plane_end_pt);
            let test_end = line_point + t_end * line_vector;
            let end_dist = test_end.distance(plane_end_pt);

            // quit when plane points are not close to line points
            if end_dist > distance_tolerance {
                return Ok(solutions);
            }

            // when line parameters are the same
            if are_same(t_start, t_end) {
                // the interval is [tStart - 1, tEnd + 1], or the caller's interval
                let sub_interval = line_interval
                    .copied()
                    .unwrap_or(Extent1d::from_min_max(t_start - 1.0, t_end + 1.0));
                // try global surface/curve solver (SmSurface::GlobalLineIntersect: the
                // solutions name `surface`)
                return base(&sub_interval);
            }

            // test that 3D Points project to UVDomain
            let uv_start = self.project_point_to_uv_domain(plane_start_pt);
            let uv_end = self.project_point_to_uv_domain(plane_end_pt);

            // build a coincident solution
            let mut sol = Solution::new();
            sol.num_variables = 3;
            sol.num_objects = 0;
            sol.solution_type = SolutionType::RangeOfValues;
            sol.start.solution_value = start_dist;
            sol.start.parameters[0] = ivl.clamp_value(t_start);
            sol.start.parameters[1] = uv_start.x;
            sol.start.parameters[2] = uv_start.y;

            sol.end.solution_value = end_dist;
            sol.end.parameters[0] = ivl.clamp_value(t_end);
            sol.end.parameters[1] = uv_end.x;
            sol.end.parameters[2] = uv_end.y;

            // set the output
            solutions.add(&sol);
        } else {
            // Single intersection
            // Set the curve domain.
            let mut curve_domain = Extent1d::from_min_max(-1.0e10, 1.0e10);
            if let Some(i) = line_interval {
                curve_domain = *i;
            }
            if fire_ray {
                let _ = curve_domain.set_min_max(0.0, BIG_DOUBLE);
            }

            // u*Pu + v*Pv - t*Lt = (L0 - P0), solved by Cramer's rule.
            let pu = self.position.x_axis() * self.uv_scale.x;
            let pv = self.position.y_axis() * self.uv_scale.y;
            let lt = -line_vector;
            let diff = line_point - self.position.origin();

            let det = pu.cross(pv).dot(lt);
            if det.abs() < EFF_ZERO {
                // should never happen, we're in the non-parallel case here.
                return Err(SolidError::sm_err(OP_LINE));
            }

            // Cramer's rule:
            let numer_t = pu.cross(pv).dot(diff); // triple product
            let t_param = numer_t / det;

            // Check curve param vs. domain.
            if !curve_domain.contains_value(t_param, 0.0) {
                return Ok(solutions); // but no solution.
            }

            let numer_u = diff.cross(pv).dot(lt);
            let numer_v = pu.cross(diff).dot(lt);
            let mut uv = Point2::new(numer_u / det, numer_v / det);

            // Eval curve and plane, and check whether they're within tol.
            let line_pnt = line_point + t_param * line_vector;

            uv = uv_domain.clamp_point(uv);
            let srf_pnt = self.evaluate_point_fast(uv);
            let dist = srf_pnt.distance(line_pnt);
            if dist > distance_tolerance {
                return Ok(solutions); // but no solution.
            }

            // Good to go, set up solution and get out.
            let mut sol = Solution::new();
            sol.num_variables = 3;
            sol.num_objects = 0;
            sol.solution_type = SolutionType::SingleValue;
            sol.start.solution_value = dist;
            sol.start.parameters[0] = t_param;
            sol.start.parameters[1] = uv.x;
            sol.start.parameters[2] = uv.y;
            solutions.add(&sol);
        }

        Ok(solutions)
    }

    /// The part of the segment `start_pt`-`end_pt` (projected onto the plane) over
    /// `uv_domain` enlarged by `tol_3d` (mapped to UV with the larger scale)
    /// (`TrimPlaneLineToPlaneDomain`): its trimmed end points, or `None` when the
    /// segment misses the domain. An end of the segment touching the opposite end of
    /// the domain interval within `SM_EFF_ZERO` gives that single point.
    #[must_use]
    pub fn trim_plane_line_to_plane_domain(
        &self,
        uv_domain: &Extent2d,
        start_pt: Point3,
        end_pt: Point3,
        tol_3d: f64,
    ) -> Option<(Point3, Point3)> {
        let this_ivl = Extent1d::from_min_max(0.0, 1.0);
        let uv_start = self.project_point_to_uv_domain(start_pt);
        let uv_end = self.project_point_to_uv_domain(end_pt);
        let uv_vec = uv_end - uv_start;

        // Find thisPlane and otherPlane line/domain intersection interval
        let tol_uv = tol_3d / self.uv_scale.max_dimension();
        let plane_ivl =
            uv_domain.intersect_with_infinite_line(uv_start, uv_vec, Some(tol_uv), None)?;

        // find the intersection between intersection result and input segment
        let ivl = if (this_ivl.max() - plane_ivl.min()).abs() < EFF_ZERO {
            // 1. degenerate overlap on ThisIvl max end
            Extent1d::from_value(this_ivl.max())
        } else if (this_ivl.min() - plane_ivl.max()).abs() < EFF_ZERO {
            // 2. degenerate overlap on ThisIvl min end
            Extent1d::from_value(this_ivl.min())
        } else {
            // 3. intersection of the two intervals (4. none: no intersection)
            this_ivl.intersect(&plane_ivl).ok()?
        };

        // set outputs
        let start = (1.0 - ivl.min()) * start_pt + ivl.min() * end_pt;
        let end = (1.0 - ivl.max()) * start_pt + ivl.max() * end_pt;
        Some((start, end))
    }

    // ---- curves -----------------------------------------------------------------------

    /// The iso line of the plane at `iso_parameter` in `surf_param`'s direction
    /// (constant `u` for [`SurfParamType::U`], else constant `v`) over the natural
    /// domain or `domain` (`CreateIsoParametricCurve`): a 3D [`Line`] parameterised by
    /// the other parameter. With `uv_iso_curve` the UV line of `SmIsoCurve`
    /// (`CreateUVIsoLine`, built only for constant `u`; its status is ignored) is
    /// returned too.
    ///
    /// # Errors
    ///
    /// Those of [`Line::create_line_segment`] (a degenerate line) and
    /// [`Line::edit_parameterization`].
    pub fn create_iso_parametric_curve(
        &self,
        surf_param: SurfParamType,
        iso_parameter: f64,
        domain: Option<&Extent2d>,
        uv_iso_curve: bool,
    ) -> Result<PlaneIsoCurve> {
        // get natural or optional domain min/max points
        let uv_domain = self.natural_uv_domain();
        let mut p_min = uv_domain.min();
        let mut p_max = uv_domain.max();
        if let Some(d) = domain {
            p_min = d.min();
            p_max = d.max();
        }

        // when asked - build associated 2d UVTrimCurve line (different parameterization)
        let uv_curve = if uv_iso_curve {
            // define the isoParameter Curve as an object
            let base = self.base_for_query().into_owned();
            let iso = IsoCurve::new(base, surf_param, iso_parameter, true, true);
            // build OptUVIsoCurve (status ignored)
            iso.create_uv_iso_line().ok()
        } else {
            None
        };

        // move min/max points to isoParameter Line endPoints
        let mut line_ivl = Extent1d::new();
        if surf_param == SurfParamType::U {
            p_min.x = iso_parameter;
            p_max.x = iso_parameter;
            let _ = line_ivl.set_min_max(p_min.y, p_max.y);
        } else {
            p_min.y = iso_parameter;
            p_max.y = iso_parameter;
            let _ = line_ivl.set_min_max(p_min.x, p_max.x);
        }

        // evaluate Line 3D endPoints
        let st_pt = self.evaluate_point_fast(p_min);
        let end_pt = self.evaluate_point_fast(p_max);

        // create the line
        let mut line = Line::create_line_segment(3, st_pt, end_pt, None)?;
        line.edit_parameterization(&line_ivl)?;

        Ok(PlaneIsoCurve {
            curve: line,
            max_gap_3d: 0.0,
            uv_iso_curve: uv_curve,
        })
    }

    /// Drops `curve` over `interval` onto the plane over `uv_domain` (`DropCurve`).
    ///
    /// A line (an `SmLine`, or a curve `IsLine` finds straight) gives the UV line of
    /// its projection trimmed to the domain (none when it misses the domain or is
    /// degenerate), kept when `keep_all` or its end points drop within
    /// `approx_tol`; unless `creating_uv_trim_curve`, a trimmed line also trims the
    /// curve interval. Otherwise a B-spline kind is trimmed to `interval` and its
    /// control points are projected and clamped to the domain: the UV B-spline of
    /// those points is exact when no clamp moves a point off its normal line by more
    /// than `approx_tol`, and is kept when `keep_all` or every control point drops
    /// within `approx_tol`. Everything else is the general `SmSurface::DropCurve`, run
    /// on the plane with its `owner` (the plane's `m_pOwner`, which a bare [`Plane`]
    /// does not carry: pass the owning [`Surface`]'s [`Surface::owner`]; without the
    /// face tolerance its B-rep records on that surface, which the crate's
    /// `drop_curve_owned_by` also passes, a face owner makes the general drop return
    /// `Unported`).
    ///
    /// # Errors
    ///
    /// Those of the curve evaluations and constructions and of the general
    /// `SmSurface::DropCurve` ([`Surface::drop_curve_base`]).
    pub fn drop_curve(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol: f64,
        keep_all: bool,
        creating_uv_trim_curve: bool,
        owner: Option<ObjectRef>,
    ) -> Result<PlaneDropCurve> {
        self.drop_curve_in(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol,
            keep_all,
            creating_uv_trim_curve,
            owner,
            None,
            None::<DropSolveContext<'_, crate::surface::surface::p4::NoObject>>,
            None,
        )
    }

    /// [`Plane::drop_curve`] of the plane held by `surface`: the general
    /// `SmSurface::DropCurve` runs on the plane with `surface`'s owner and the face
    /// tolerance its B-rep recorded on it ([`Surface::owner_zone_tol_3d`]), as upstream
    /// runs it on the same `SmPlane` object.
    ///
    /// # Errors
    ///
    /// As [`Plane::drop_curve`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn drop_curve_owned_by(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol: f64,
        keep_all: bool,
        creating_uv_trim_curve: bool,
        surface: &Surface,
    ) -> Result<PlaneDropCurve> {
        self.drop_curve_owned_by_with_context::<crate::surface::surface::p4::NoObject>(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol,
            keep_all,
            creating_uv_trim_curve,
            surface,
            None,
        )
    }

    /// [`Plane::drop_curve_owned_by`] whose general fallback's start point drop tests
    /// the face's trim boundary with `classify`.
    ///
    /// # Errors
    ///
    /// As [`Plane::drop_curve`], and those of `classify`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn drop_curve_owned_by_face_tested(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol: f64,
        keep_all: bool,
        creating_uv_trim_curve: bool,
        surface: &Surface,
        classify: FaceTest<'_>,
    ) -> Result<PlaneDropCurve> {
        self.drop_curve_in::<crate::surface::surface::p4::NoObject>(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol,
            keep_all,
            creating_uv_trim_curve,
            surface.owner(),
            surface.owner_zone_tol_3d(),
            None,
            Some(classify),
        )
    }

    /// [`Plane::drop_curve_owned_by`] whose general fallback solves under the caller's
    /// face context.
    ///
    /// # Errors
    ///
    /// As [`Plane::drop_curve`], and those of the context.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn drop_curve_owned_by_with_context<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol: f64,
        keep_all: bool,
        creating_uv_trim_curve: bool,
        surface: &Surface,
        context: Option<DropSolveContext<'_, O>>,
    ) -> Result<PlaneDropCurve> {
        self.drop_curve_in(
            k,
            uv_domain,
            curve,
            interval,
            approx_tol,
            keep_all,
            creating_uv_trim_curve,
            surface.owner(),
            surface.owner_zone_tol_3d(),
            context,
            None,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn drop_curve_in<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        interval: &Extent1d,
        approx_tol: f64,
        keep_all: bool,
        creating_uv_trim_curve: bool,
        owner: Option<ObjectRef>,
        owner_zone_tol_3d: Option<f64>,
        context: Option<DropSolveContext<'_, O>>,
        face_test: Option<FaceTest<'_>>,
    ) -> Result<PlaneDropCurve> {
        // init output
        let mut out = PlaneDropCurve {
            max_drop_to_surf: 0.0,
            max_approx_dev: 0.0,
            uv_curves: Vec::new(),
        };

        // locals
        let is_line =
            matches!(curve.kind(), CurveKind::Line(_)) || curve.is_line(k, 5, EFF_ZERO)?.is_some();
        let mut ivl = *interval;

        // Check the fairly trivial case of projecting a line to a plane.
        if is_line {
            // get 3d line endPts
            let mut st_pnt_3d = curve.evaluate_point(k, ivl.min())?;
            let mut end_pnt_3d = curve.evaluate_point(k, ivl.max())?;

            // Get Line 2d endPt projections to infinite plane
            let mut st_pnt_2d = self.project_point_to_uv_domain(st_pnt_3d);
            let mut end_pnt_2d = self.project_point_to_uv_domain(end_pnt_3d);
            let vec_2d = end_pnt_2d - st_pnt_2d;

            // trim drop line to plane domain
            let tol_u = approx_tol / self.uv_scale.x; // 3d tol to 2d
            let tol_v = approx_tol / self.uv_scale.y;
            let found =
                uv_domain.intersect_with_infinite_line(st_pnt_2d, vec_2d, Some(tol_u), Some(tol_v));

            // no DropCurve without UVDomain intersection
            let Some(found) = found else {
                return Ok(out);
            };

            // union the trimIvl with the line ivl
            let line_ivl = Extent1d::from_min_max(0.0, 1.0);
            let mut final_ivl = ivl;
            let trim_ivl = found
                .intersect(&line_ivl)
                .unwrap_or_else(|_| Extent1d::new());
            if trim_ivl.is_init() {
                return Ok(out);
            }

            // adjust for partial drops
            if !trim_ivl.are_equal_bounds(0.0, 1.0, EFF_ZERO) {
                // reevaluate srf[UVPts]
                end_pnt_2d = st_pnt_2d + trim_ivl.max() * vec_2d; // order dependent code
                st_pnt_2d += trim_ivl.min() * vec_2d;

                if !creating_uv_trim_curve {
                    // reevaluate crv[ivl]
                    let lo = final_ivl.evaluate(trim_ivl.min());
                    let hi = final_ivl.evaluate(trim_ivl.max());
                    let _ = final_ivl.set_min_max(lo, hi);

                    // reevaluate crv[endpts]
                    st_pnt_3d = curve.evaluate_point(k, final_ivl.min())?;
                    end_pnt_3d = curve.evaluate_point(k, final_ivl.max())?;
                }
            }

            // get srf[endPts]
            let new_st_3d = self.evaluate_point_fast(st_pnt_2d);
            let new_end_3d = self.evaluate_point_fast(end_pnt_2d);

            // Max drop distance to surface
            let dist1 = new_st_3d.distance(st_pnt_3d);
            let dist2 = new_end_3d.distance(end_pnt_3d);
            out.max_drop_to_surf = max2(dist1, dist2);

            // when the drop curve is good enough - save it to the output
            if keep_all || out.max_drop_to_surf <= approx_tol {
                // build dropCurve as a UVline
                let vector_2d = end_pnt_2d - st_pnt_2d;
                let scale = vector_2d.length();

                // skip degenerate solutions
                if scale > EFF_ZERO {
                    // compose the UVTrimCurve - same cr3dCurve parameterization
                    let vector_2d = vector_2d.unitize()?;
                    let line_interval = Extent1d::from_min_max(0.0, 1.0);
                    let mut line_2d = Line::bounded(
                        Vec3::from(st_pnt_2d),
                        Vec3::from(vector_2d),
                        &line_interval,
                        &BoundedLineOptions {
                            scale,
                            dim: 2,
                            ..BoundedLineOptions::default()
                        },
                    );
                    line_2d.edit_parameterization(&final_ivl)?;

                    // all done - set output and return
                    out.uv_curves.push(Curve::from(line_2d));
                    out.max_approx_dev = 0.0; // the UVLine is an exact rep of the UVDropCurve
                }
                return Ok(out);
            }
        }

        // arrive here for all nonLine 3d curves
        // when the curve is a BSpline Curve
        if curve.is_kind_of(BSplineCurve::TYPE) {
            let Some(b_spline) = b_spline_part(curve) else {
                return surface_drop_curve(
                    k,
                    self,
                    uv_domain,
                    curve,
                    &ivl,
                    approx_tol,
                    keep_all,
                    owner,
                    owner_zone_tol_3d,
                    context,
                    face_test,
                );
            };

            // copy and trim the original crv to the given interval
            let mut crv_copy_3d = b_spline.clone();
            ignore_status(crv_copy_3d.trim(k, &mut ivl, TrimMode::Notify))?; // may snap ivl

            // get the BSpline representation of the curve
            let c = crv_copy_3d.canonical()?;
            let mut ctrl_pts = c.control_points.clone();

            // loop locals
            let mut max_drop_sq = 0.0;

            // for every control point
            for cp in &mut ctrl_pts {
                let p_3d = *cp;

                // project to infinite plane
                let mut uv_pnt = self.project_point_to_uv_domain(p_3d);

                // clamp control points
                uv_pnt = uv_domain.clamp_point(uv_pnt);

                // save largest control point drop
                let on_srf_3d = self.evaluate_point_fast(uv_pnt);
                let drop_to_surf_sq = on_srf_3d.distance_squared(p_3d);
                if drop_to_surf_sq > max_drop_sq {
                    max_drop_sq = drop_to_surf_sq;
                }

                // find out how far clamping moved the control point from the drop point
                let plane_normal = self.evaluate_normal(); // unit normal
                let (dev_from_srf_nrm_line, _) = line_point_distance(on_srf_3d, plane_normal, p_3d);

                // if clamping moves the control point - give up - pass the call along
                if dev_from_srf_nrm_line > approx_tol {
                    return surface_drop_curve(
                        k,
                        self,
                        uv_domain,
                        curve,
                        &ivl,
                        approx_tol,
                        keep_all,
                        owner,
                        owner_zone_tol_3d,
                        context,
                        face_test,
                    );
                }

                // save the dropped control point
                *cp = Point3::new(uv_pnt.x, uv_pnt.y, 0.0);
            }

            // arrive here when all control points drop nicely within the plane trim boundaries
            if keep_all || max_drop_sq < approx_tol * approx_tol {
                // build 2d UVDropCurve
                let rational = b_spline.is_rational()?;
                let uv_curve = BSplineCurve::create_canonical(&CanonicalData {
                    dim: 2,
                    degree: c.degree,
                    control_points: &ctrl_pts,
                    form: c.form,
                    knot_multiplicities: &c.knot_multiplicities,
                    knots: &c.knots,
                    knot_type: c.knot_type,
                    weights: if rational { Some(&c.weights) } else { None },
                    trim: None,
                })?;

                // set output
                out.uv_curves.push(Curve::from(uv_curve));
                out.max_drop_to_surf = sqrt(max_drop_sq);
                out.max_approx_dev = 0.0; // ControlPt projection produces an exact UVDropCurve
            }

            // all done
            return Ok(out);
        }

        // else pass the call along to the general DropCurve method
        surface_drop_curve(
            k,
            self,
            uv_domain,
            curve,
            &ivl,
            approx_tol,
            keep_all,
            owner,
            owner_zone_tol_3d,
            context,
            face_test,
        )
    }

    // ---- coincidence ------------------------------------------------------------------

    /// Whether `other` coincides with the plane within `tol_3d` (`CoincidenceCheck`):
    /// over both whole domains (`whole_surface`) or over any overlap.
    ///
    /// `other` must be planar; another plane must be parallel (within the angle that
    /// moves this plane's far end by `tol_3d`) and through this plane's origin. For
    /// whole surfaces the four corners must match and either `other` is bilinear or
    /// five interior samples lie on the plane with parallel normals. For an overlap
    /// of two planes, a corner of `other` must project inside this domain, an edge of
    /// `other` must cross it over at least `tol_3d`, or this plane's middle must
    /// project inside `other`'s domain. For an overlap with another surface, a
    /// boundary curve of one must meet one of the other with matching normals and
    /// in-plane second derivatives, or the middle of one must drop onto the other.
    ///
    /// # Errors
    ///
    /// Those of the evaluations of `other`; [`SolidError::Unported`] from `IsPlanar` of
    /// `other` (see [`Surface::is_planar`]) and the unported callees of the overlap
    /// test.
    pub fn coincidence_check(
        &self,
        k: &Kernel,
        other: &Surface,
        tol_3d: f64,
        whole_surface: bool,
    ) -> Result<Coincidence> {
        // init output
        let mut res = Coincidence {
            are_coincident: false,
            max_distance: -1.0,
        };

        // low work - surfaces are the same
        if let SurfaceKind::Plane(p) = other.kind() {
            if core::ptr::eq(p, self) {
                res.are_coincident = true;
                res.max_distance = 0.0;
                return Ok(res);
            }
        }

        // Quick check: other surface would have to be planar.
        if !surface_is_planar(k, other, tol_3d)? {
            return Ok(res);
        }

        // locals
        let mut dist_sq = 0.0;
        let mut max_dist = -1.0;

        let domain0 = self.natural_uv_domain();
        let domain1 = other.natural_uv_domain_with_nurb(k)?;

        let plane_pnt0 = self.position.origin();
        let plane_norm0 = self.position.z_axis();

        // find an angle tolerance that makes a plane this size deviate by the given tol.
        let size = Vec2::new(
            self.anal_uv_domain.u_interval().length() * self.uv_scale.x,
            self.anal_uv_domain.v_interval().length() * self.uv_scale.y,
        );
        let ang_tol_deg = rad_to_deg(tol_3d / size.length());

        // Special for planes: rule out cases where infinite planes don't coincide.
        let other_plane = match other.kind() {
            SurfaceKind::Plane(p) => Some(p),
            _ => None,
        };

        if let Some(op) = other_plane {
            let other_pos = op.position();

            // Check normals.
            let other_norm = other_pos.z_axis();
            if !other_norm.is_parallel_to(plane_norm0, ang_tol_deg) {
                return Ok(res);
            }

            // Check positions.
            let dist = plane_point_distance(plane_pnt0, plane_norm0, other_pos.origin())?;
            if dist > tol_3d {
                return Ok(res);
            }
        }

        if whole_surface {
            res.are_coincident = false;

            // First, all four corner points must match.
            let mut corners0 = Vec::with_capacity(4);
            let mut corners1 = Vec::with_capacity(4);
            for ii in 0..2 {
                for jj in 0..2 {
                    let (fi, fj) = (f64::from(ii), f64::from(jj));
                    corners0.push(self.evaluate_point(domain0.evaluate(fi, fj)));
                    corners1.push(other.evaluate_point(k, domain1.evaluate(fi, fj))?);
                }
            }

            // All four points of other must match one of ours.
            let tol_sq = tol_3d * tol_3d;
            for c0 in &corners0 {
                for c1 in &corners1 {
                    dist_sq = c0.distance_squared(*c1);
                    if dist_sq <= tol_sq {
                        break; // Found a match.
                    }
                }
                if dist_sq > tol_sq {
                    break; // this-plane point did not find a match.
                }
            }

            if dist_sq > tol_sq {
                return Ok(res); // Some corner did not match.
            }

            // If the other surface is a plane as well, we're done (bilinear suffices).
            if other.is_bilinear(k) {
                res.max_distance = sqrt(dist_sq);
                res.are_coincident = true;
                return Ok(res);
            }

            // Match some points.  Midpoint and four around, not exact.
            let uvs = [
                Point2::new(0.51, 0.48),
                Point2::new(0.27, 0.23),
                Point2::new(0.27, 0.77),
                Point2::new(0.72, 0.77),
                Point2::new(0.72, 0.23),
            ];
            for uv1 in uvs {
                let pt1 = other.evaluate_point(k, uv1)?;
                let dist = plane_point_distance(plane_pnt0, plane_norm0, pt1)?;

                if dist > max_dist {
                    max_dist = dist;
                    res.max_distance = dist;
                }

                if dist > tol_3d {
                    return Ok(res);
                }

                // Also check normals.
                let norm1 = surface_evaluate_normal(k, other, uv1)?;
                if !norm1.is_parallel_to(plane_norm0, ang_tol_deg) {
                    return Ok(res);
                }
            }

            // Still here -- must be coincident.
            res.are_coincident = true;
            return Ok(res);
        }

        // Now, check for any coincident overlap.
        if let Some(op) = other_plane {
            return Ok(self.plane_overlap(op, &domain0, &domain1, tol_3d, res));
        }

        // Checking partial overlap, and the other surface is not an SmPlane.
        self.surface_overlap(k, other, &domain0, &domain1, tol_3d, ang_tol_deg, res)
    }

    /// The plane/plane overlap test of [`Plane::coincidence_check`] (two coplanar
    /// rectangles).
    fn plane_overlap(
        &self,
        op: &Self,
        domain0: &Extent2d,
        domain1: &Extent2d,
        tol_3d: f64,
        mut res: Coincidence,
    ) -> Coincidence {
        let mut other_uvs = Vec::with_capacity(5);
        let mut other_extent = Extent2d::new();

        for ii in 0..2 {
            for jj in 0..2 {
                // plane1 corners to plane2 distances
                let uv1 = if ii == 0 {
                    domain1.evaluate(f64::from(ii), f64::from(jj))
                } else {
                    domain1.evaluate(f64::from(ii), f64::from(1 - jj)) // To go around in order.
                };
                let pt1 = op.evaluate_point_fast(uv1);
                let uv0 = self.project_point_to_uv_domain(pt1);

                // First check max distance, for the output value.
                let pt0 = self.evaluate_point_fast(uv0);
                let dist = pt1.distance(pt0);

                if dist > res.max_distance {
                    res.max_distance = dist;
                }

                if dist > tol_3d {
                    // (Shouldn't happen.)
                    res.are_coincident = false;
                    return res;
                }

                // Now check whether it's internal (not on boundary: negative tol).
                if domain0.contains_point(uv0, -EFF_ZERO_SQRT) {
                    res.are_coincident = true;
                    return res;
                }

                other_uvs.push(uv0);
                other_extent.add_point(uv0);
            }
        }

        // Now do the bounding box check (negative tol: don't catch just-touching cases).
        if other_extent.are_disjoint(domain0, -EFF_ZERO_SQRT) {
            res.are_coincident = false;
            return res;
        }

        // Intersect the four sides of the other with our domain.
        if let Some(&first) = other_uvs.first() {
            other_uvs.push(first);
        }
        let unit_ivl = Extent1d::from_min_max(0.0, 1.0);
        for pair in other_uvs.windows(2) {
            let (Some(&a), Some(&b)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            let vec = b - a;
            if let Some(int_ivl) = domain0.intersect_with_infinite_line(a, vec, None, None) {
                // Intersect it with the unit interval (the segment between the corners).
                if let Ok(int_ivl) = unit_ivl.intersect(&int_ivl) {
                    if int_ivl.length() >= tol_3d {
                        res.are_coincident = true;
                        return res;
                    }
                }
            }
        }

        // Either they're disjoint, or this plane is totally contained in the other.
        // Test our midpoint.
        let uv0 = domain0.evaluate(0.5, 0.5);
        let pt0 = self.evaluate_point(uv0);
        let uv1 = op.project_point_to_uv_domain(pt0);

        // Also check max distance, for the output value.
        let pt1 = op.evaluate_point_fast(uv1);
        let dist = pt1.distance(pt0);

        if dist > res.max_distance {
            res.max_distance = dist;
        }

        if dist > tol_3d {
            // (Shouldn't happen.)
            res.are_coincident = false;
            return res;
        }

        // Now check internal; if so, we're done.  But not on boundary: negative tol.
        if domain1.contains_point(uv1, -EFF_ZERO_SQRT) {
            res.are_coincident = true;
            return res;
        }

        // Didn't find any overlap.
        res.are_coincident = false;
        res
    }

    /// The overlap test of [`Plane::coincidence_check`] with a planar surface that is
    /// not an `SmPlane`: boundary curve intersections, then mutual middle drops.
    fn surface_overlap(
        &self,
        k: &Kernel,
        other: &Surface,
        domain0: &Extent2d,
        domain1: &Extent2d,
        tol_3d: f64,
        ang_tol_deg: f64,
        mut res: Coincidence,
    ) -> Result<Coincidence> {
        // Create two sets of boundary curves.
        let bdrys_3d0 = create_natural_uv_trim_curves(k, &Surface::from(self.clone()), domain0)?;
        let bdrys_3d1 = create_natural_uv_trim_curves(k, other, domain1)?;

        // We'll be checking normals, and this plane's normal never changes.
        let norm0 = self.position.z_axis();

        // Also, our first derivatives never change.
        let su0 = self.position.x_axis() * self.uv_scale.x;
        let sv0 = self.position.y_axis() * self.uv_scale.y;

        // degenerate inner curves, computed once
        let mut crv1_degen = Vec::with_capacity(bdrys_3d1.len());
        for c in &bdrys_3d1 {
            crv1_degen.push(match c {
                Some(c) => c.is_degenerate(k, EFF_ZERO, None)?,
                None => false,
            });
        }

        // out-of-plane test of the other surface's derivatives
        let out_of_plane = |v: Vec3| norm0.dot(v).abs() > ang_tol_deg;

        // Set this True as default, so that we can just return if found.
        res.are_coincident = true;

        for (ii, crv0) in bdrys_3d0.iter().enumerate() {
            let Some(crv0) = crv0 else { continue };
            if crv0.is_degenerate(k, EFF_ZERO, None)? {
                continue;
            }

            let crv_dom0 = if ii == 0 || ii == 2 {
                domain0.u_interval()
            } else {
                domain0.v_interval()
            };

            for (jj, crv1) in bdrys_3d1.iter().enumerate() {
                let Some(crv1) = crv1 else { continue };
                if crv1_degen.get(jj).copied().unwrap_or(false) {
                    continue;
                }

                let crv_dom1 = if jj == 0 || jj == 2 {
                    domain1.u_interval()
                } else {
                    domain1.v_interval()
                };

                // Check for an intersection.
                let sols = match crv0.global_curve_intersect(
                    k,
                    &crv_dom0,
                    crv1,
                    &crv_dom1,
                    tol_3d,
                    &CurveIntersectOptions::<NoObject>::default(),
                ) {
                    Ok(s) => s,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => continue,
                };
                if sols.is_empty() {
                    continue;
                }

                for sol in sols.as_slice() {
                    // use an end point of the solution
                    let t0 = sol.start.parameters[0];
                    let t1 = sol.start.parameters[1];

                    // Get the surface uv's from the edge curve params.
                    let (_uv0, uv_binorm0) = boundary_uv(domain0, ii, t0);
                    let (uv1, uv_binorm1) = boundary_uv(domain1, jj, t1);
                    let norm1 = surface_evaluate_normal(k, other, uv1)?;

                    if !norm1.is_parallel_to(norm0, ang_tol_deg) {
                        continue;
                    }

                    // Have to check 2nd order terms: cyl with seam in plane.
                    let d2 = evaluate_2nd_derivatives(k, other, uv1)?;
                    if out_of_plane(d2.du)
                        || out_of_plane(d2.dv)
                        || out_of_plane(d2.duu)
                        || out_of_plane(d2.duv)
                        || out_of_plane(d2.dvv)
                    {
                        continue;
                    }

                    if sol.solution_type == SolutionType::SingleValue {
                        // If this hits the interior of both boundaries
                        if !crv_dom0.is_value_on_boundary(t0, EFF_ZERO).0
                            && !crv_dom1.is_value_on_boundary(t1, EFF_ZERO).0
                        {
                            return Ok(res);
                        }
                    } else {
                        // Range solution: edge-to-edge. Need to check binormals.
                        let binorm_3d0 = uv_binorm0.x * su0 + uv_binorm0.y * sv0;
                        let (_, su1, sv1) = evaluate_1st_derivatives(k, other, uv1)?;
                        let binorm_3d1 = uv_binorm1.x * su1 + uv_binorm1.y * sv1;
                        if binorm_3d0.dot(binorm_3d1) > 0.0 {
                            return Ok(res);
                        }
                    }
                }
            }
        }

        // If there is coincidence, then one surface completely contains the other.
        // Drop to this first because planes are easy.
        let uv1 = domain1.mid();
        let pt1 = other.evaluate_point(k, uv1)?;
        let (success, _uv0, dist) =
            surface_drop_point(k, &Surface::from(self.clone()), pt1, domain0)?;

        // Also set output argument:
        if dist > res.max_distance {
            res.max_distance = dist;
        }

        if success && dist <= tol_3d {
            // Point coincidence, check 2nd order terms, not just normals.
            let d2 = evaluate_2nd_derivatives(k, other, uv1)?;
            if in_plane(norm0, &d2, ang_tol_deg) {
                return Ok(res);
            }
        }

        // Repeat the other way around.
        let uv0 = domain0.mid();
        let pt0 = self.evaluate_point(uv0);
        let (success, uv1, dist) = surface_drop_point(k, other, pt0, domain1)?;

        // Also set output argument:
        if dist > res.max_distance {
            res.max_distance = dist;
        }

        if success && dist <= tol_3d {
            let d2 = evaluate_2nd_derivatives(k, other, uv1)?;
            if in_plane(norm0, &d2, ang_tol_deg) {
                return Ok(res);
            }
        }

        // If we arrive here, we haven't found coincidence.
        res.are_coincident = false;
        Ok(res)
    }
}

/// `sm_IsGrazingIntersection`: whether the curve/line intersection at `curve_param`
/// / `line_param` is a graze, i.e. the tangents are parallel and the curve has
/// curvature there.
///
/// # Errors
///
/// Those of the curve evaluation (upstream ignores its status).
pub fn is_grazing_intersection(
    k: &Kernel,
    curve: &Curve,
    curve_param: f64,
    line: &Line,
    line_param: f64,
) -> Result<bool> {
    // Method: check tangents.  If parallel, then as long as the curve
    // has any curvature, it's a graze.
    let curve_eval = curve.evaluate(k, curve_param, 2, false, true)?;
    let line_eval = line.evaluate(line_param, 1);
    let c1 = curve_eval.get(1).copied().unwrap_or(Vec3::ZERO);
    let c2 = curve_eval.get(2).copied().unwrap_or(Vec3::ZERO);
    let l1 = line_eval.get(1).copied().unwrap_or(Vec3::ZERO);
    if !c1.is_parallel_to(l1, EFF_ZERO_DEG) {
        return Ok(false);
    }

    // Check curvature.  If the 2nd deriv has a component that's not parallel
    // to the 1st deriv, there is curvature.
    let cross = c1.cross(c2);
    Ok(cross.length_squared() > EFF_ZERO_SQ)
}

// ---- helpers ------------------------------------------------------------------------------

/// A solution handle naming nothing (the curve/curve solutions of
/// `CoincidenceCheck` only use their parameters).
#[derive(Debug, Clone, Copy, PartialEq)]
struct NoObject;

impl SolutionObject for NoObject {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }
}

/// The uv of parameter `t` on boundary `index` of `domain` (0: v min, 1: u max,
/// 2: v max, 3: u min) and the inward binormal of that boundary.
fn boundary_uv(domain: &Extent2d, index: usize, t: f64) -> (Point2, Vec2) {
    match index {
        0 => (Point2::new(t, domain.v_min()), Vec2::new(0.0, 1.0)),
        1 => (Point2::new(domain.u_max(), t), Vec2::new(-1.0, 0.0)),
        2 => (Point2::new(t, domain.v_max()), Vec2::new(0.0, -1.0)),
        3 => (Point2::new(domain.u_min(), t), Vec2::new(1.0, 0.0)),
        // the switch upstream leaves the values unset; never more than 4 boundaries
        _ => (Point2::new(t, domain.v_min()), Vec2::new(0.0, 0.0)),
    }
}

/// Every derivative of `d2` lies in the plane of normal `norm0` within `ang_tol`.
fn in_plane(norm0: Vec3, d2: &SecondDerivatives, ang_tol: f64) -> bool {
    norm0.dot(d2.du).abs() < ang_tol
        && norm0.dot(d2.dv).abs() < ang_tol
        && norm0.dot(d2.duu).abs() < ang_tol
        && norm0.dot(d2.duv).abs() < ang_tol
        && norm0.dot(d2.dvv).abs() < ang_tol
}

/// `SmSurface::Evaluate1stDerivatives(uv, TRUE, TRUE, ..)`.
fn evaluate_1st_derivatives(k: &Kernel, s: &Surface, uv: Point2) -> Result<(Point3, Vec3, Vec3)> {
    surface_p1::evaluate_1st_derivatives(k, s, uv, Side::Left, Side::Left)
}

/// `SmSurface::Evaluate2ndDerivatives(uv, TRUE, TRUE, ..)`.
fn evaluate_2nd_derivatives(k: &Kernel, s: &Surface, uv: Point2) -> Result<SecondDerivatives> {
    surface_p1::evaluate_2nd_derivatives(k, s, uv, Side::Left, Side::Left)
}

/// The `SmBSplineCurve` part of a curve of a B-spline kind (upstream's slicing copy
/// `SmBSplineCurve(crBSplineCurve)`).
fn b_spline_part(curve: &Curve) -> Option<&BSplineCurve> {
    match curve.kind() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Line(l) => Some(l.base()),
        CurveKind::Ellipse(e) => Some(e.base()),
        CurveKind::Hyperbola(h) => Some(h.base()),
        CurveKind::Parabola(p) => Some(p.base()),
        CurveKind::Hermite(_)
        | CurveKind::Conic(_)
        | CurveKind::Projected(_)
        | CurveKind::Iso(_)
        | CurveKind::Offset(_)
        | CurveKind::CrvInVolume(_)
        | CurveKind::CrvOnSurf(_)
        | CurveKind::TangentField(_)
        | CurveKind::Composite(_) => None,
    }
}

/// Transposes the `num_rows` x `len / num_rows` row-major array `a` in place.
fn transpose<T: Copy>(a: &mut [T], num_rows: usize) {
    if num_rows < 1 {
        return;
    }
    let num_columns = a.len() / num_rows;
    let temp = a.to_vec();
    for ii in 0..num_rows {
        for jj in 0..num_columns {
            if let (Some(dst), Some(&src)) = (
                a.get_mut(ii * num_columns + jj),
                temp.get(jj * num_rows + ii),
            ) {
                *dst = src;
            }
        }
    }
}

// ---- unported callees ---------------------------------------------------------------------

/// `crOther.IsPlanar(tol)`: a plane is planar; other kinds run `SmSurface::IsPlanar`.
fn surface_is_planar(k: &Kernel, s: &Surface, tol: f64) -> Result<bool> {
    Ok(s.is_planar(k, tol)?.is_planar)
}

/// `crOther.EvaluateNormal(uv, TRUE, TRUE, ..)`: the plane override; other kinds run
/// `SmSurface::EvaluateNormal`.
fn surface_evaluate_normal(k: &Kernel, s: &Surface, uv: Point2) -> Result<Vec3> {
    s.evaluate_normal(k, uv, Side::Left, Side::Left)
}

/// `SmSurface::CreateNaturalUVTrimCurves(ctx, domain, SM_SP_V, FALSE, ..)`: the 3D
/// boundary curves.
fn create_natural_uv_trim_curves(
    k: &Kernel,
    s: &Surface,
    domain: &Extent2d,
) -> Result<Vec<Option<Curve>>> {
    let t = s.create_natural_uv_trim_curves(k, domain, SurfParamType::V, false)?;
    Ok(t.curves_3d.into_iter().map(Some).collect())
}

/// `SmSurface::DropPoint(pt, domain, NULL, bSuccess, uv, dist, bIsMulti,
/// SM_SO_NORMALIZE)`: success, uv and distance.
fn surface_drop_point(
    k: &Kernel,
    s: &Surface,
    pt: Point3,
    domain: &Extent2d,
) -> Result<(bool, Point2, f64)> {
    let opts = crate::surface::surface::p5::DropPointOptions {
        operation: SolverOperation::Normalize,
        ..Default::default()
    };
    let r = s.drop_point::<crate::surface::surface::p4::NoObject>(k, pt, domain, &opts)?;
    Ok((r.success, r.result_uv, r.gap))
}

/// The general `SmSurface::DropCurve(ctx, domain, curve, interval, tol, .., keep_all,
/// NULL, ..)` on the plane (a copy with the plane's `owner`, carrying the owning face's
/// tolerance `owner_zone_tol_3d` when the caller has it).
#[allow(clippy::too_many_arguments)]
fn surface_drop_curve<O: SolutionObject>(
    k: &Kernel,
    plane: &Plane,
    uv_domain: &Extent2d,
    curve: &Curve,
    interval: &Extent1d,
    approx_tol: f64,
    keep_all: bool,
    owner: Option<ObjectRef>,
    owner_zone_tol_3d: Option<f64>,
    context: Option<DropSolveContext<'_, O>>,
    face_test: Option<FaceTest<'_>>,
) -> Result<PlaneDropCurve> {
    let mut surface = Surface::from(plane.clone());
    *surface.owner_mut() = owner;
    surface.set_owner_zone_tol_3d(owner_zone_tol_3d);
    let r = match (context, face_test) {
        (None, Some(classify)) => surface.drop_curve_base_face_tested::<O>(
            k, uv_domain, curve, interval, approx_tol, keep_all, None, classify,
        )?,
        (Some(context), _) => surface.drop_curve_base_with_context::<O>(
            k, uv_domain, curve, interval, approx_tol, keep_all, None, context,
        )?,
        (None, None) => surface
            .drop_curve_base::<O>(k, uv_domain, curve, interval, approx_tol, keep_all, None)?,
    };
    Ok(PlaneDropCurve {
        max_drop_to_surf: r.max_drop_to_surf,
        max_approx_dev: r.max_approx_dev,
        uv_curves: r.uv_curves.into_iter().map(Curve::from).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_plane() -> Plane {
        Plane::new(
            Point3::new(1.0, 2.0, 3.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(2.0, 3.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        )
    }

    #[test]
    fn evaluate_and_project() {
        let p = unit_plane();
        let pt = p.evaluate_point(Point2::new(0.5, 0.5));
        assert_eq!(pt, Point3::new(2.0, 3.5, 3.0));
        // clamped
        let pt = p.evaluate_point(Point2::new(2.0, -1.0));
        assert_eq!(pt, Point3::new(3.0, 2.0, 3.0));
        let uv = p.project_point_to_uv_domain(Point3::new(2.0, 3.5, 7.0));
        assert_eq!(uv, Point2::new(0.5, 0.5));
        let d = p.evaluate(Point2::new(0.2, 0.3), 2, 2, false).unwrap();
        assert_eq!(d[1][0], Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(d[0][1], Vec3::new(0.0, 3.0, 0.0));
        assert_eq!(d[1][1], Vec3::ZERO);
        assert!(p.evaluate(Point2::new(0.0, 0.0), 2, 1, true).is_err());
        assert_eq!(p.evaluate_normal(), Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn nurb_and_queries() {
        let mut p = unit_plane();
        assert!(p.base().gw_nurb().is_none());
        assert_eq!(
            p.natural_uv_domain(),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0)
        );
        // GetNaturalUVDomain makes the NURB and the plane keeps it
        assert!(p.base().gw_nurb().is_some());
        p.make_nurb().unwrap();
        assert!(p.base().is_bilinear());
        assert!(p.is_kind_of(Plane::TYPE).unwrap());
        assert!(p.is_kind_of(BSplineSurface::TYPE).unwrap());
        assert!(p.is_bounded());
        assert!(!Plane::from_normal(Point3::ZERO, Vec3::new(0.0, 0.0, 2.0)).is_bounded());
        assert!(p.approx_eq(&p.copy()));
    }

    #[test]
    fn drop_point_and_line() {
        let p = unit_plane();
        let dom = p.step_uv_domain();
        let sols = p
            .drop_point_fast::<NoObject>(
                &dom,
                SolverOperation::Minimize,
                Point3::new(2.0, 3.5, 5.0),
                1e-6,
                None,
                None,
            )
            .unwrap();
        assert_eq!(sols.len(), 1);
        assert_eq!(sols.as_slice()[0].start.solution_value, 2.0);
        let sols = p
            .global_line_intersect::<NoObject>(
                &Kernel::default(),
                NoObject,
                &dom,
                Point3::new(2.0, 3.5, 5.0),
                Vec3::new(0.0, 0.0, -1.0),
                None,
                false,
                1e-6,
            )
            .unwrap();
        assert_eq!(sols.len(), 1);
        assert_eq!(sols.as_slice()[0].start.parameters[0], 2.0);
        assert!(p
            .drop_point_fast::<NoObject>(
                &dom,
                SolverOperation::Maximize,
                Point3::ZERO,
                1e-6,
                None,
                None
            )
            .is_err());
    }

    #[test]
    fn toggle_and_trim() {
        let mut p = unit_plane();
        let before = p.evaluate_point(Point2::new(0.25, 0.75));
        p.toggle_swap_uv_bit();
        let after = p.evaluate_point(Point2::new(0.75, 0.25));
        assert!(before.distance(after) < 1e-12);
        let p = unit_plane();
        let (a, b) = p
            .trim_plane_line_to_plane_domain(
                &p.step_uv_domain(),
                Point3::new(0.0, 3.0, 3.0),
                Point3::new(5.0, 3.0, 3.0),
                0.0,
            )
            .unwrap();
        assert!(a.distance(Point3::new(1.0, 3.0, 3.0)) < 1e-12);
        assert!(b.distance(Point3::new(3.0, 3.0, 3.0)) < 1e-12);
    }

    #[test]
    fn coincidence_of_planes() {
        let k = Kernel::default();
        let p = unit_plane();
        let q = Surface::from(Plane::new(
            Point3::new(1.5, 2.0, 3.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(2.0, 3.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        ));
        let c = p.coincidence_check(&k, &q, 1e-6, false).unwrap();
        assert!(c.are_coincident);
        let c = p.coincidence_check(&k, &q, 1e-6, true).unwrap();
        assert!(!c.are_coincident);
        let b = Surface::from(p.base_with_nurb().unwrap().into_owned());
        // (a B-spline other surface runs SmSurface::IsPlanar, surface.surface_p7; since
        // SMLib v1.0.1 its pseudo box is unpadded (SmSurface.cpp:22371 at 3fd5b95), so the
        // plane's own B-spline passes a 1e-6 planarity test and is coincident)
        assert!(
            p.coincidence_check(&k, &b, 1e-6, true)
                .unwrap()
                .are_coincident
        );
    }

    #[test]
    fn evaluate_rejects_huge_derivative_orders() {
        // regression: usize::MAX overflowed the size computation and usize::MAX / 8
        // overflowed the allocation (both panicked)
        let p = Plane::new(
            Point3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        );
        let uv = Point2::new(0.0, 0.0);
        assert!(p.evaluate(uv, usize::MAX, 0, false).is_err());
        assert!(p.evaluate(uv, 0, usize::MAX / 8, false).is_err());
        assert!(p.evaluate(uv, MAX_DERIVATIVE + 1, 0, false).is_err());
        assert!(p.evaluate(uv, 0, MAX_DERIVATIVE + 1, true).is_err());
        assert!(p
            .evaluate(uv, MAX_DERIVATIVE, MAX_DERIVATIVE, false)
            .is_ok());
    }

    #[test]
    fn extension_uses_plane_overrides() {
        // the B-spline IsSingularity / IsClosed are unported; the plane's are not
        let p = Plane::new(
            Point3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        );
        let e = p.create_extended_surface(0.5, ContinuityType::G1).unwrap();
        let d = e.natural_uv_domain();
        assert!(d.max().x - d.min().x > 1.0);
        let e = p
            .create_extended_surface_dir(SurfParamType::UMax, 0.5, ContinuityType::G1)
            .unwrap();
        assert!(e.gw_nurb().is_some());
    }
}
