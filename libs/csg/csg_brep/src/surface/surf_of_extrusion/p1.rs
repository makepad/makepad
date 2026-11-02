//! `SmSurfOfExtrusion`, part 1: the surface object ([`SurfOfExtrusion`]) with its
//! constructors, equality, STEP domain adjustment and reparameterisation, the
//! STEP / NURB parameter conversions, STEP evaluation and inversion, the NURB build
//! (`MakeNurb`), the transform, directrices, the fast point drop, the small queries,
//! and the generator intersection of two parallel extrusions
//! ([`intersect_generators`]).
//!
//! Upstream: `source/SMLib/src/SmSurfOfExtrusion.cpp`,
//! `source/SMLib/inc/SmSurfOfExtrusion.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`SurfOfExtrusion`], [`CurveOrientType`]          | `SmSurfOfExtrusion`, `SmCurveOrientType`         |
//! | [`SurfOfExtrusion::new`]                          | `SmSurfOfExtrusion(pGenCurve, crOrigin, crExtrusionVec, crAnalUVDomain, bSwapUV)` |
//! | [`SurfOfExtrusion::new_infinite`]                 | `SmSurfOfExtrusion(pGenCurve, crExtrusionVec, bSwapUV)` |
//! | [`SurfOfExtrusion::empty`]                        | `SmSurfOfExtrusion()`                            |
//! | [`SurfOfExtrusion::copy`], `Clone`                | `SmSurfOfExtrusion(const SmSurfOfExtrusion&)`, `Copy` |
//! | `Drop` (implicit)                                 | `~SmSurfOfExtrusion`                             |
//! | [`SurfOfExtrusion::approx_eq`]                    | `operator==`                                     |
//! | [`SurfOfExtrusion::adjust_step_uv_domain`]        | `AdjustSTEPUVDomain`                             |
//! | [`SurfOfExtrusion::update_analytical_domain`]     | `UpdateAnalyticalDomain`                         |
//! | [`SurfOfExtrusion::reparameterize`]               | `Reparameterize`                                 |
//! | [`SurfOfExtrusion::step_inversion`]               | `STEPInversion`                                  |
//! | [`SurfOfExtrusion::convert_uv_from_step_to_nurbs`], [`SurfOfExtrusion::convert_uv_from_nurbs_to_step`] | `ConvertUVFromSTEPToNURBS`, `ConvertUVFromNURBSToSTEP` |
//! | [`SurfOfExtrusion::create_canonical`]             | `CreateCanonical`                                |
//! | [`SurfOfExtrusion::evaluate_step`], [`SurfOfExtrusion::evaluate_step_point`] | `EvaluateSTEP`, `EvaluateSTEPPoint` |
//! | [`SurfOfExtrusion::canonical`]                    | `GetCanonical`                                   |
//! | [`SurfOfExtrusion::step_linear_param_extent`]     | `GetSTEPLinearParamExtent`                       |
//! | [`SurfOfExtrusion::gen_dir_param_extent`]         | `GetGenDirParamExtent`                           |
//! | [`SurfOfExtrusion::max_analytic_domain`]          | `GetMaxAnalyticDomain`                           |
//! | [`SurfOfExtrusion::extrusion_vector`], [`SurfOfExtrusion::origin`] | `GetExtrusionVector`, `GetOrigin`  |
//! | [`SurfOfExtrusion::create_directrix_from_point`]  | `CreateDirectrixFromPoint`                       |
//! | [`SurfOfExtrusion::drop_point_fast`]              | `DropPointFast`                                  |
//! | [`intersect_generators`], [`GeneratorIntersection`] | `sm_IntersectGenerators` (file static)         |
//! | [`SurfOfExtrusion::make_nurb`]                    | `MakeNurb`                                       |
//! | [`SurfOfExtrusion::transform`]                    | `Transform`                                      |
//! | [`SurfOfExtrusion::is_kind_of`], [`SurfOfExtrusion::TYPE`], [`SurfOfExtrusion::type_code`], [`SurfOfExtrusion::type_string`], [`SurfOfExtrusion::class_string`] | `IsKindOf`, `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`SurfOfExtrusion::step_uv_domain`], [`SurfOfExtrusion::gen_curve`], [`SurfOfExtrusion::set_context`], [`SurfOfExtrusion::toggle_swap_uv_bit`] | `GetSTEPUVDomain`, `GetGenCurve`, `SetContext`, `ToggleSwapUVBit` (inline) |
//! | [`SurfOfExtrusion::set_curve_point`], [`SurfOfExtrusion::set_curve_vec`] | `SetCurvePoint`, `SetCurveVec` (inline, protected) |
//!
//! Deviations:
//! - **Derived class.** `SmSurfOfExtrusion` derives from `SmBSplineSurface`; the base
//!   object is the [`SurfOfExtrusion::base`] field. As a
//!   [`crate::surface::surface::SurfaceKind::SurfOfExtrusion`] the `Surface` dispatchers
//!   run its overrides, else the inherited bodies on that base.
//! - **Generator.** `m_pGenCurve` is a [`Curve`] of a kind derived from
//!   `SmBSplineCurve` (B-spline, ellipse, line, hyperbola, parabola); other kinds are
//!   rejected (`InvalidInput`) where upstream casts. It is `None` only for the empty
//!   I/O surface. Its owner is not set (a free surface has no
//!   [`crate::geom::object::ObjectRef`] to give it). The one place where upstream's
//!   owner matters for these methods, an `SmLine`, `SmHyperbola` or `SmParabola`
//!   generator whose `MakeNurb` (run by `AdjustSTEPInterval` when its interval
//!   changes, or by a lazy line's first `GetCanonical`) reparameterises its NURB to the owner's
//!   [`SurfOfExtrusion::gen_dir_param_extent`], is done here by the surface
//!   (see `adjust_generator_interval` and [`SurfOfExtrusion::make_nurb`]), also where `STEPInversion`'s
//!   `GlobalPointSolveSTEP` of a hyperbola or parabola generator first adjusts it to
//!   the solve interval (upstream on the owned generator itself, which `STEPInversion`
//!   leaves adjusted; here on a copy). An ellipse generator's owner step is
//!   overwritten by `AdjustSTEPInterval`'s own reparameterisation upstream, so it is
//!   not repeated (the knots then agree within rounding, not bit for bit).
//! - **Lazy line owner.** A constructor's `IsLinear` can build a lazy line and
//!   its owning surface through virtual `GetEnds`. `classify_generator` performs
//!   that owner build and retains the domain captured before `GetEnds` for the
//!   remaining `IsLinear` samples; the finite constructor may adjust the line's
//!   interval before storing its classification point.
//! - **Lazy NURB.** Upstream's `const` methods make a missing NURB through
//!   `GetOrCreateGwNurbPointer`; the `&self` methods here cannot. The parameter
//!   conversions return `SM_ERR` without a NURB (call
//!   [`SurfOfExtrusion::make_nurb`] first) instead of reading a `(0, 0, 0, 0)` domain;
//!   `DropPointFast` solves on an evaluation copy with the NURB made, as upstream's
//!   lazy `GetNaturalUVDomain` would;
//!   [`SurfOfExtrusion::gen_dir_param_extent`] reads the NURB as it is (an empty
//!   interval without one). `Reparameterize` and a non-identity `Transform` make it.
//!   Inherited `Surface` evaluation and iso-curve queries go through
//!   `base_with_nurb`, which makes a missing NURB once on a bounded exact copy and
//!   keeps it in the surface's NURB slot, as upstream keeps it on the object (later
//!   readers, copies and `operator==` see it). Upstream's in-place `MakeNurb` also
//!   adjusts the generator's STEP interval to the analytic U interval and makes a lazy
//!   line generator's NURB; that generator state stays on the copy and is not kept.
//!   `Clone` copies the NURB the surface has (knots fixed up, as the copy
//!   constructor), but cannot make a missing one first as upstream's copy constructor
//!   does (`MakeNurb` needs the [`Kernel`]). A lazy
//!   B-spline generator (as returned by the extrusion recogniser) uses the analytic
//!   domain, swapped when needed, for `Surface::natural_uv_domain_if_built`; other lazy
//!   generator kinds still require an explicit `MakeNurb` for that domain query.
//! - **Out-of-bounds evaluation.** `SmTemporaryChangeValue` gives the generator the
//!   surface's out-of-bounds flag for the evaluation / drop; only a B-spline generator
//!   reads it, so only that kind is copied with the flag changed. `STEPInversion`
//!   turns the generator's flag on (upstream permanently, through the `const`
//!   method's pointer); here a copy is used for the solve.
//! - **Arguments.** Out-params become return values; `SmContext` arguments and the
//!   arguments upstream does not use (`EvaluateSTEP`'s side flags) are dropped.
//!   `EvaluateSTEP` entries it does not write are `0` (upstream leaves the caller's
//!   values) and orders above 5 in either direction are `SM_ERR` (upstream only
//!   checks u; v sizes the caller's array). `DropPointFast` names the surface in its
//!   solutions with the caller's `this` handle.
//! - **Statuses.** Calls whose status upstream ignores keep upstream's values, except
//!   that [`SolidError::Unported`] always propagates and a failure whose output
//!   upstream would read uninitialised (a failed generator evaluation, plane test or
//!   parameter conversion) is returned. A failed `IntersectWithLine` in
//!   `CreateDirectrixFromPoint` asks for the general intersection (upstream reads an
//!   unset flag).
//! - **Transform nesting.** The generator transform carries the caller's geometry
//!   depth so volume appends remain subject to the whole-geometry
//!   [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING) bound.
//! - **Unported callees.** `STEPInversion` of a
//!   generator not perpendicular to the extrusion uses `SmBSplineSurface::STEPInversion`
//!   (whose solves are in `surface.surface_p1` / `surface.surface_p5`). Those return
//!   [`SolidError::Unported`].
//! - **`sm_IntersectGenerators`** reads `pOpt3DCurves->GetSize()` before checking the
//!   pointer and compares offset curves it may not have set; here a missing curve list
//!   has size 0 and unset offsets compare unequal.

use std::borrow::Cow;

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::p9::CurveSolveOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::line::Line;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::{
    line_closest_point, line_line_closest_point, line_plane_intersect,
};
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{
    ObjectKind, Solution, SolutionArray, SolutionObject, SolutionType,
};
use crate::geom::tol::{are_same, is_zero, is_zero_to_tol};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_RAD, EFF_ZERO_SQ, EFF_ZERO_SQRT, INFINITE_PARAMETER, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::{
    BSplineSurface, BSplineSurfaceForm, CanonicalSurfaceData, LocationType, SurfParamType,
};
use crate::nurbs::geom::Rectangle;
use crate::nurbs::srf_basic::Direction;
use crate::surface::surface::p2::{TsectCurveType, SURF_OF_EXTRUSION_TYPE};

const OP_GEN: &str = "surface::surf_of_extrusion::p1::SurfOfExtrusion::gen_curve";
const OP_COPY: &str = "surface::surf_of_extrusion::p1::SurfOfExtrusion::copy";
const OP_REPARAM: &str = "surface::surf_of_extrusion::p1::SurfOfExtrusion::reparameterize";
const OP_STEP_INV: &str = "surface::surf_of_extrusion::p1::SurfOfExtrusion::step_inversion";
const OP_EVAL_STEP: &str = "surface::surf_of_extrusion::p1::SurfOfExtrusion::evaluate_step";
const OP_DIRECTRIX: &str =
    "surface::surf_of_extrusion::p1::SurfOfExtrusion::create_directrix_from_point";
const OP_CONVERT: &str = "surface::surf_of_extrusion::p1::SurfOfExtrusion::convert_uv";
const OP_DROP: &str = "surface::surf_of_extrusion::p1::SurfOfExtrusion::drop_point_fast";
const OP_XFORM: &str = "surface::surf_of_extrusion::p1::SurfOfExtrusion::transform";
const OP_GENERATORS: &str = "surface::surf_of_extrusion::p1::intersect_generators";

/// `ANALYTIC_TOL_SCALE` (`SmConfig.h`).
const ANALYTIC_TOL_SCALE: f64 = 100.0;

/// `MAXDER` of NLib (`nurbsdef.h`): the highest STEP derivative order.
const MAX_DERIV: usize = 5;

// ---- the generator (a curve derived from SmBSplineCurve) -------------------------------

/// The `SmBSplineCurve` base object of a generator.
pub(super) fn gen_base(c: &Curve) -> Result<&BSplineCurve> {
    match c.kind() {
        CurveKind::BSpline(b) => Ok(b),
        CurveKind::Ellipse(e) => Ok(e.base()),
        CurveKind::Line(l) => Ok(l.base()),
        CurveKind::Hyperbola(h) => Ok(h.base()),
        CurveKind::Parabola(p) => Ok(p.base()),
        _ => Err(not_b_spline()),
    }
}

/// The `SmBSplineCurve` base object of a generator, mutably.
fn gen_base_mut(c: &mut Curve) -> Result<&mut BSplineCurve> {
    match c.kind_mut() {
        CurveKind::BSpline(b) => Ok(b),
        CurveKind::Ellipse(e) => Ok(&mut e.base),
        CurveKind::Line(l) => Ok(&mut l.base),
        CurveKind::Hyperbola(h) => Ok(&mut h.base),
        CurveKind::Parabola(p) => Ok(p.base_mut()),
        _ => Err(not_b_spline()),
    }
}

const fn not_b_spline() -> SolidError {
    SolidError::invalid_input(OP_GEN, "generator is not derived from SmBSplineCurve")
}

/// `GetSTEPInterval` of a generator.
fn gen_step_interval(c: &Curve) -> Result<Extent1d> {
    match c.kind() {
        CurveKind::BSpline(b) => Ok(b.step_interval()),
        CurveKind::Ellipse(e) => Ok(e.step_interval()),
        CurveKind::Line(l) => Ok(l.step_interval()),
        CurveKind::Hyperbola(h) => Ok(h.step_interval()),
        CurveKind::Parabola(p) => Ok(p.step_interval()),
        _ => Err(not_b_spline()),
    }
}

/// `EvaluateSTEP(dT, lNumDerivs, TRUE, aDerivs)` of a generator (virtual: the
/// analytic STEP evaluators, else the NURB evaluation).
fn gen_evaluate_step(k: &Kernel, c: &Curve, t: f64, num_derivs: usize) -> Result<Vec<Vec3>> {
    let _ = k;
    match c.kind() {
        CurveKind::Ellipse(e) => e.evaluate_step(t, num_derivs),
        CurveKind::Line(l) => Ok(l.evaluate_step(t, num_derivs)),
        CurveKind::Hyperbola(h) => h.evaluate_step(t, num_derivs),
        CurveKind::Parabola(p) => p.evaluate_step(t, num_derivs),
        CurveKind::BSpline(_) => crate::curve::curve::p2::evaluate_step(k, c, t, num_derivs, true),
        _ => Err(not_b_spline()),
    }
}

/// `GetMaxAnalyticDomain` of a generator.
fn gen_max_analytic_domain(k: &Kernel, c: &Curve) -> Result<Extent1d> {
    match c.kind() {
        CurveKind::BSpline(b) => b.max_analytic_domain(k),
        CurveKind::Ellipse(e) => Ok(e.max_analytic_domain()),
        CurveKind::Line(l) => Ok(l.max_analytic_domain()),
        CurveKind::Hyperbola(h) => Ok(h.max_analytic_domain()),
        CurveKind::Parabola(p) => Ok(p.max_analytic_domain()),
        _ => Err(not_b_spline()),
    }
}

/// The generator `c` with the out-of-bounds flag `enabled`
/// (`SmTemporaryChangeValue<SmBoolean>(m_pGenCurve->GetOutOfBoundsEnabled(), ..)`):
/// only the B-spline evaluation reads the flag, so only that kind is copied.
pub(super) fn gen_with_out_of_bounds(c: &Curve, enabled: bool) -> Option<Curve> {
    match c.kind() {
        CurveKind::BSpline(b) if b.is_out_of_bounds_enabled() != enabled => {
            let mut b = b.clone();
            b.set_out_of_bounds_enabled(enabled);
            Some(Curve::from(b))
        }
        _ => None,
    }
}

/// The first entry of an evaluation.
fn first(v: &[Vec3]) -> Result<Vec3> {
    v.first()
        .copied()
        .ok_or(SolidError::sm_err("surface::surf_of_extrusion::p1::first"))
}

/// A handle naming a curve in the curve solves' solutions (upstream's curve
/// pointer): `id` tells the objects apart (the generator, its cache's analytic
/// replacement, the directrix line), `ivl` answers `GetNaturalInterval`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CurveHandle {
    id: u8,
    ivl: Extent1d,
}

impl CurveHandle {
    /// The generator `c` (`id` 0) and its cache's analytic replacement (`id` 1).
    fn gen_pair(c: &Curve) -> (Self, Self) {
        let ivl = c.natural_interval();
        (Self { id: 0, ivl }, Self { id: 1, ivl })
    }
}

impl SolutionObject for CurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.ivl)
    }
}

// ---- results ---------------------------------------------------------------------------

/// The classification of the generator relative to the extrusion vector
/// (`SmCurveOrientType`). The discriminants are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CurveOrientType {
    /// `SM_CO_NOT_PLANAR`: the generator is not planar.
    NotPlanar = 0,
    /// `SM_CO_PERPENDICULAR`: planar, its plane perpendicular to the extrusion vector.
    Perpendicular = 1,
    /// `SM_CO_INDEPENDENT`: planar, its plane at an angle to the extrusion vector.
    Independent = 2,
    /// `SM_CO_COPLANAR`: planar, its plane containing the extrusion vector.
    Coplanar = 3,
    /// `SM_CO_LINEAR`: the generator is a line.
    Linear = 4,
    /// `SM_CO_UNDEFINED`: not yet set.
    #[default]
    Undefined = 5,
}

/// The outputs of [`intersect_generators`] (`sm_IntersectGenerators`).
#[derive(Debug, Clone, Default)]
pub struct GeneratorIntersection {
    /// `rbNeedsMoreIntersections`: the case is too complicated, use the general
    /// surface/surface intersector.
    pub needs_more_intersections: bool,
}

// ---- the surface -----------------------------------------------------------------------

/// A surface of extrusion (`SmSurfOfExtrusion`): `S(u, v) = C(u) + v E` for the
/// generator `C` (its STEP parameter `u`) and the extrusion vector `E`, over the STEP
/// domain [`SurfOfExtrusion::step_uv_domain`], plus its B-spline surface (the
/// generator direction is NURB `u`, or `v` with `swap_uv`).
#[derive(Debug, Clone)]
pub struct SurfOfExtrusion {
    /// The `SmBSplineSurface` base object.
    pub(crate) base: BSplineSurface,
    /// `m_pGenCurve`: the generating curve (`None` only for [`Self::empty`]).
    pub(crate) gen_curve: Option<Curve>,
    /// `m_vOrigin`: a point on the generator, origin of the extrusion parameter.
    pub(crate) origin: Point3,
    /// `m_vExtrusionVec`: the sweep direction and speed.
    pub(crate) extrusion_vec: Vec3,
    /// `m_vAnalUVDomain`: generator parameters (x) by extrusion parameters (y).
    pub(crate) anal_uv_domain: Extent2d,
    /// `m_eOrientation`.
    pub(crate) orientation: CurveOrientType,
    /// `m_bSwapUV`: the NURB's v is the generator direction.
    pub(crate) swap_uv: bool,
    /// `m_vCurvePoint`: a point of the generator's plane or line.
    pub(crate) curve_point: Point3,
    /// `m_vCurveVec`: the generator's plane normal or line direction.
    pub(crate) curve_vec: Vec3,
}

impl SurfOfExtrusion {
    /// An evaluation copy with an exact, owner-preserving generator. `depth` is
    /// the generator's depth in the shared `MAX_GEOMETRY_NESTING` (32) geometry traversal.
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        Ok(Self {
            base: self.base.clone_exact(),
            gen_curve: self
                .gen_curve
                .as_ref()
                .map(|c| c.clone_for_evaluation_depth(depth))
                .transpose()?,
            origin: self.origin,
            extrusion_vec: self.extrusion_vec,
            anal_uv_domain: self.anal_uv_domain,
            orientation: self.orientation,
            swap_uv: self.swap_uv,
            curve_point: self.curve_point,
            curve_vec: self.curve_vec,
        })
    }

    /// [`SurfOfExtrusion::clone_for_evaluation`] for a copy that stands for this surface
    /// object (upstream's pointer to it): it shares this surface's NURB cell, so a NURB a
    /// read-only query makes through the copy is kept on this surface too.
    pub(crate) fn clone_for_evaluation_shared(&self, depth: usize) -> Result<Self> {
        let mut copy = self.clone_for_evaluation(depth)?;
        copy.base = self.base.clone_exact_shared();
        Ok(copy)
    }

    /// Detaches a NURB cell shared with evaluation copies
    /// ([`SurfOfExtrusion::clone_for_evaluation_shared`]).
    pub(crate) fn detach_nurb(&mut self) {
        self.base.detach_nurb();
    }

    /// `SmSurfOfExtrusion_TYPE` (`SURF_BASE_TYPE + 30`).
    pub const TYPE: i64 = SURF_OF_EXTRUSION_TYPE;

    // ---- construction ---------------------------------------------------------------

    /// The empty surface `ReadFromDB` fills (`SmSurfOfExtrusion()`): no generator, no
    /// NURB, undefined vectors and an "inside out" domain.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            base: BSplineSurface::new(),
            gen_curve: None,
            origin: Vec3::UNDEF,
            extrusion_vec: Vec3::UNDEF,
            anal_uv_domain: Extent2d::new(),
            orientation: CurveOrientType::Undefined,
            swap_uv: false,
            curve_point: Vec3::UNDEF,
            curve_vec: Vec3::UNDEF,
        }
    }

    /// The finite surface sweeping `gen_curve` along `extrusion_vec` over
    /// `anal_uv_domain` (generator parameters by extrusion parameters) from `origin`
    /// (a point on the generator), normally without NURB (constructor). A lazy
    /// line may build its owner's NURB during the linearity test. The generator's STEP
    /// interval should equal the domain's U interval and it should not be inside out
    /// (upstream only logs otherwise). The orientation is classified with the tolerance
    /// `ANALYTIC_TOL_SCALE * SM_EFF_ZERO * (1 + |origin| + |P|)` for the generator
    /// point `P` at 45.678% of its natural interval.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a generator not derived from `SmBSplineCurve`; those of the
    /// generator's evaluation and tests (see `Self::classify`).
    pub fn new(
        k: &Kernel,
        gen_curve: Curve,
        origin: Point3,
        extrusion_vec: Vec3,
        anal_uv_domain: Extent2d,
        swap_uv: bool,
    ) -> Result<Self> {
        gen_base(&gen_curve)?;

        // (the inside-out and STEP interval checks only log upstream)

        // select tol based on origin and mid Point evaluation locations
        let pnt = gen_curve.evaluate_point(k, gen_curve.natural_interval().evaluate(0.45678))?;
        let tol =
            ANALYTIC_TOL_SCALE * EFF_ZERO * (1.0 + origin.max_dimension() + pnt.max_dimension());

        let mut surf = Self {
            base: BSplineSurface::new(),
            gen_curve: None,
            origin,
            extrusion_vec,
            anal_uv_domain,
            orientation: CurveOrientType::Undefined,
            swap_uv,
            curve_point: Vec3::UNDEF,
            curve_vec: Vec3::UNDEF,
        };
        surf.classify_generator(k, gen_curve, tol, pnt)?;
        Ok(surf)
    }

    /// The surface sweeping `gen_curve` infinitely along `extrusion_vec`, without
    /// NURB (constructor, except for the lazy-line owner build described in the module
    /// docs): the origin is the generator's STEP point at 45.678% of its
    /// STEP interval, the domain that interval by `[-SM_INFINITE_PARAMETER,
    /// SM_INFINITE_PARAMETER]`. The orientation is classified with the tolerance
    /// `ANALYTIC_TOL_SCALE * SM_EFF_ZERO * (1 + |origin| + |P|)` for the generator's
    /// natural start point `P`, the perpendicular test through the origin.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a generator not derived from `SmBSplineCurve`; those of the
    /// generator's evaluations and tests (see `Self::classify`).
    pub fn new_infinite(
        k: &Kernel,
        gen_curve: Curve,
        extrusion_vec: Vec3,
        swap_uv: bool,
    ) -> Result<Self> {
        // (the inside-out check only logs upstream)

        // set m_vOrigin
        let crv_ivl = gen_step_interval(&gen_curve)?;
        let origin = first(&gen_evaluate_step(
            k,
            &gen_curve,
            crv_ivl.evaluate(0.45678),
            0,
        )?)?;

        // set m_vAnalUVDomain for an infinitely long extrusion
        let anal_uv_domain = Extent2d::from_points(
            Point2::new(crv_ivl.min(), -INFINITE_PARAMETER),
            Point2::new(crv_ivl.max(), INFINITE_PARAMETER),
        );

        // select tol based on origin and mid Point evaluation locations
        let pnt = gen_curve.evaluate_point(k, gen_curve.natural_interval().evaluate(0.0))?;
        let tol =
            ANALYTIC_TOL_SCALE * EFF_ZERO * (1.0 + origin.max_dimension() + pnt.max_dimension());

        let mut surf = Self {
            base: BSplineSurface::new(),
            gen_curve: None,
            origin,
            extrusion_vec,
            anal_uv_domain,
            orientation: CurveOrientType::Undefined,
            swap_uv,
            curve_point: Vec3::UNDEF,
            curve_vec: Vec3::UNDEF,
        };
        surf.classify_generator(k, gen_curve, tol, origin)?;
        Ok(surf)
    }

    /// `GetEnds` in `IsLinear` materializes a lazy line's NURB. Its owner call
    /// builds this surface, possibly adjusting the line's STEP interval first.
    fn classify_generator(
        &mut self,
        k: &Kernel,
        mut gen: Curve,
        tol: f64,
        plane_point: Point3,
    ) -> Result<()> {
        let lazy_line =
            matches!(gen.kind(), CurveKind::Line(line) if line.base().gw_nurb().is_none());
        let linearity = if lazy_line {
            // IsLinear snapshots the domain before its virtual GetEnds call.
            let domain = gen.natural_interval();
            self.gen_curve = Some(gen);
            ignore_status(self.make_nurb(k))?;
            gen = self
                .gen_curve
                .take()
                .ok_or(SolidError::null_pointer(OP_GEN))?;
            let (start, end) = ignored(gen.ends(k))?
                .map_or((Vec3::UNDEF, Vec3::UNDEF), |ends| (ends.start, ends.end));
            Some(gen.is_linear_after_ends(k, tol, false, domain, start, end)?)
        } else {
            None
        };
        self.classify(k, &gen, tol, plane_point, linearity)?;
        self.gen_curve = Some(gen);
        Ok(())
    }

    /// The constructors' orientation classification: linear (`IsLinear(tol)`, its
    /// point and direction kept), not planar (`IsPlanar(tol)`, whose plane point and
    /// normal are kept either way), else by the angle between the plane normal and the
    /// extrusion vector: within `SM_EFF_ZERO_RAD` of `0` or `PI` perpendicular when
    /// the generator lies within `tol` of the plane through `plane_point` normal to
    /// the extrusion (else independent), within it of `PI / 2` coplanar, else
    /// independent. A failed angle is `0`; a zero extrusion vector is unitised to
    /// `(0, 0, 1)` (statuses ignored upstream).
    ///
    /// # Errors
    ///
    /// Those of the linearity and planarity tests and of `IsOnPlane` (whose output
    /// upstream reads even on failure).
    fn classify(
        &mut self,
        k: &Kernel,
        gen: &Curve,
        tol: f64,
        plane_point: Point3,
        linearity: Option<crate::curve::curve::p5::Linearity>,
    ) -> Result<()> {
        // if GenCurve is linear
        let lin = match linearity {
            Some(linearity) => linearity,
            None => gen.is_linear(k, tol, false)?,
        };
        if lin.linear {
            self.curve_point = lin.line_point;
            self.curve_vec = lin.line_tangent;
            self.orientation = CurveOrientType::Linear;
            return Ok(());
        }

        // if GenCurve is non planarity
        let planar = gen.is_planar(k, tol)?;
        self.curve_point = planar.point;
        self.curve_vec = planar.normal;
        if !planar.is_planar {
            self.orientation = CurveOrientType::NotPlanar;
            return Ok(());
        }

        // get angle between curve planeNormal and ExtrusionVec (status ignored)
        let angle_rad = self
            .curve_vec
            .angle_between(self.extrusion_vec)
            .unwrap_or(0.0);

        // When close to perpendicular check by testing curve against plane perp to
        // extrusion vector
        if is_zero_to_tol(angle_rad, EFF_ZERO_RAD) || is_zero_to_tol(angle_rad - PI, EFF_ZERO_RAD) {
            // normalize the extrusion vec (status ignored)
            let extrusion_normal = self.extrusion_vec.unitize_or_z();

            // see if the curve lies on the plane perpendicular to the extrusion vec
            let on = gen_base(gen)?.is_on_plane(plane_point, extrusion_normal, tol, None)?;
            self.orientation = if on.on_plane {
                CurveOrientType::Perpendicular
            } else {
                CurveOrientType::Independent
            };
        } else if is_zero_to_tol(angle_rad - PI / 2.0, EFF_ZERO_RAD) {
            // ExtrusionVec is in plane of ExtrusionVec
            self.orientation = CurveOrientType::Coplanar;
        } else {
            // angle is independent but not perpendicular to ExtrusionVec
            self.orientation = CurveOrientType::Independent;
        }
        Ok(())
    }

    /// The copy (copy constructor, `Copy`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator (upstream signals an error and then
    /// dereferences the null copy).
    pub fn copy(&self) -> Result<Self> {
        if self.gen_curve.is_none() {
            return Err(SolidError::null_pointer(OP_COPY));
        }
        Ok(self.clone())
    }

    /// The infinite extrusion of `swept_curve` along `extrusion_vec`
    /// ([`Self::new_infinite`], not swapped) with its NURB ([`Self::make_nurb`])
    /// (`CreateCanonical`).
    ///
    /// # Errors
    ///
    /// Those of [`Self::new_infinite`] and [`Self::make_nurb`].
    pub fn create_canonical(k: &Kernel, swept_curve: Curve, extrusion_vec: Vec3) -> Result<Self> {
        // make an infinitly long extrusion of pSweptCurve
        let mut surf = Self::new_infinite(k, swept_curve, extrusion_vec, false)?;

        // Make the associated m_pNurb Surface
        surf.make_nurb(k)?;
        Ok(surf)
    }

    /// Builds the NURB (`MakeNurb`): the generator trimmed to the domain's U interval
    /// (status ignored), its control points translated by the extrusion vector times
    /// the V domain ends (knots `{v0, v0, v1, v1}`, degree 1), over the generator's
    /// natural interval by the V domain, form `SM_SF_SURF_OF_LINEAR_EXTRUSION`, then
    /// swapped with [`Self::swap_uv`]. The surface's own knot type and form are left
    /// alone, as upstream.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator; `InvalidInput` for a generator not
    /// derived from `SmBSplineCurve`; those of the generator's canonical data, of
    /// [`BSplineSurface::create_canonical`] and [`BSplineSurface::swap_uv`].
    pub fn make_nurb(&mut self, k: &Kernel) -> Result<()> {
        let anal_u_ivl = self.anal_uv_domain.u_interval();

        // Trim the m_pGenCurve to the current m_vAnalUVDomain.UInterval() (status
        // ignored)
        ignore_status(self.adjust_gen_step_interval(k, &anal_u_ivl))?;

        // GetCanonical calls the generator's virtual GetOrCreateGwNurbPointer.
        // SmLine(StartPoint, EndPoint) deliberately starts without a NURB.
        let owner_ivl = self.owner_ivl();
        if let Some(CurveKind::Line(line)) = self.gen_curve.as_mut().map(Curve::kind_mut) {
            if line.base().gw_nurb().is_none() {
                ignore_status(line.make_nurb())?;
                if let Some(ivl) = owner_ivl {
                    ignore_status(line.edit_parameterization_owned(None, &ivl))?;
                }
            }
        }

        // Extract the m_pGenCurve BSplineCurve parameters
        let bsc = gen_base(self.gen()?)?;
        let canon = bsc.canonical()?;
        let crv_ivl = bsc.natural_interval();
        let is_gen_crv_rational = bsc.is_rational()?;

        // NewNurbSurface V direction knot vector
        let v_knots = [self.anal_uv_domain.min().y, self.anal_uv_domain.max().y];
        let v_knot_mult = [2usize, 2];

        // NewNurbSurface control points
        let mut ctrl_pts: Vec<Point3> = Vec::new();
        let mut weights: Vec<f64> = Vec::new();
        let extrusion = [
            self.extrusion_vec * v_knots[0],
            self.extrusion_vec * v_knots[1],
        ];
        for (ii, &cp) in canon.control_points.iter().enumerate() {
            for e in extrusion {
                ctrl_pts.push(cp + e);
                if is_gen_crv_rational {
                    weights.push(canon.weights.get(ii).copied().unwrap_or(f64::NAN));
                }
            }
        }

        // NewNurbSurface UVDomain: UInterval = Curve Interval, VInterval = Extrusion
        let uv_domain = Extent2d::from_points(
            Point2::new(crv_ivl.min(), v_knots[0]),
            Point2::new(crv_ivl.max(), v_knots[1]),
        );

        // Build NewNurbSurface
        let mut tmp = BSplineSurface::create_canonical(&CanonicalSurfaceData {
            u_degree: canon.degree,
            v_degree: 1,
            control_points: &ctrl_pts,
            form: BSplineSurfaceForm::SurfOfLinearExtrusion,
            u_multiplicities: &canon.knot_multiplicities,
            v_multiplicities: &v_knot_mult,
            u_knots: &canon.knots,
            v_knots: &v_knots,
            knot_type: canon.knot_type,
            weights: Some(&weights),
            uv_domain: Some(uv_domain),
        })?;

        // remember to swap U with V when m_bSwapUV == TRUE
        if self.swap_uv {
            tmp.swap_uv()?;
        }

        // swap NewNurbSurface m_pNurb with this surface old m_pNurb pointer
        self.base.nurb = tmp.nurb;
        Ok(())
    }

    /// `AdjustSTEPInterval` of the generator, plus the step upstream's `SmLine`,
    /// `SmHyperbola` and `SmParabola` `MakeNurb` take for an owning extrusion: when the
    /// curve rebuilds its NURB (its interval changes; a line only when it has one), the
    /// NURB is reparameterised to the surface's
    /// [`Self::gen_dir_param_extent`] (status ignored; skipped while the surface has
    /// no NURB, where upstream's owner call builds the surface from the line itself and
    /// reparameterises it to its own interval).
    fn adjust_gen_step_interval(&mut self, k: &Kernel, ivl: &Extent1d) -> Result<()> {
        let owner_ivl = self.owner_ivl();
        let gen = self
            .gen_curve
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_GEN))?;
        adjust_generator_interval(k, gen, ivl, owner_ivl)
    }

    /// The interval an owned generator's `MakeNurb` reparameterises its NURB to
    /// ([`Self::gen_dir_param_extent`]); `None` while the surface has no NURB.
    fn owner_ivl(&self) -> Option<Extent1d> {
        self.base
            .gw_nurb()
            .is_some()
            .then(|| self.gen_dir_param_extent().0)
    }

    // ---- access ---------------------------------------------------------------------

    /// The `SmBSplineSurface` base object.
    #[must_use]
    pub const fn base(&self) -> &BSplineSurface {
        &self.base
    }

    /// The `SmBSplineSurface` base object, mutably.
    pub fn base_mut(&mut self) -> &mut BSplineSurface {
        &mut self.base
    }

    /// The inherited NURB for a query (`GetOrCreateGwNurbPointer` from a `const`
    /// method): an existing (possibly reparameterised) NURB is borrowed unchanged; a
    /// missing one is made by [`SurfOfExtrusion::make_nurb`] on a bounded exact copy
    /// and kept on this surface, as upstream keeps it on the object (the copy's
    /// generator adjustments are not kept; see the module docs, **Lazy NURB**).
    pub(crate) fn base_with_nurb(&self, k: &Kernel) -> Result<Cow<'_, BSplineSurface>> {
        if self.base.gw_nurb().is_none() {
            let mut copy = self.clone_for_evaluation(0)?;
            copy.make_nurb(k)?;
            if let Some(made) = copy.base.nurb.take() {
                self.base.nurb.fill(made);
            }
        }
        Ok(Cow::Borrowed(&self.base))
    }

    /// The inherited NURB as `GetOrCreateGwNurbPointer` leaves it for a query whose
    /// missing NURB reads as a failure (`GetDegree`, `GetNumberControlPoints`,
    /// `GetKnots`, ...): [`SurfOfExtrusion::base_with_nurb`], or the base itself, still
    /// without a NURB, when the make fails (upstream ignores `MakeNurb`'s status).
    /// [`SolidError::Unported`] is passed on.
    pub(crate) fn base_with_nurb_or_base(&self, k: &Kernel) -> Result<Cow<'_, BSplineSurface>> {
        match self.base_with_nurb(k) {
            Ok(b) => Ok(b),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => Ok(Cow::Borrowed(&self.base)),
        }
    }

    /// The recogniser's lazy B-spline generator has the STEP interval as its NURB
    /// interval. Read that domain without making a NURB or choosing a kernel.
    pub(crate) fn natural_uv_domain_for_query(&self) -> Extent2d {
        if self.base.gw_nurb().is_none()
            && matches!(
                self.gen_curve.as_ref().map(Curve::kind),
                Some(CurveKind::BSpline(_))
            )
        {
            let min = self.anal_uv_domain.min();
            let max = self.anal_uv_domain.max();
            return if self.swap_uv {
                Extent2d::from_bounds(min.y, min.x, max.y, max.x)
            } else {
                self.anal_uv_domain
            };
        }
        self.base.natural_uv_domain()
    }

    /// A temporary evaluation copy: the surface's and the generator's NURBS data are
    /// kept exactly (no copy-constructor knot fix-up), as
    /// [`crate::surface::surf_of_revolution::SurfOfRevolution::clone_exact`].
    #[must_use]
    pub fn clone_exact(&self) -> Self {
        let mut copy = Self {
            base: self.base.clone_exact(),
            ..self.clone()
        };
        if let (Some(dst), Some(src)) = (
            copy.gen_curve.as_mut().and_then(|g| gen_base_mut(g).ok()),
            self.gen_curve.as_ref().and_then(|g| gen_base(g).ok()),
        ) {
            *dst = src.clone_exact();
        }
        copy
    }

    fn gen(&self) -> Result<&Curve> {
        self.gen_curve
            .as_ref()
            .ok_or(SolidError::null_pointer(OP_GEN))
    }

    /// The generating curve (`GetGenCurve`).
    #[must_use]
    pub const fn gen_curve(&self) -> Option<&Curve> {
        self.gen_curve.as_ref()
    }

    /// The generator and the extrusion vector (`GetCanonical`).
    #[must_use]
    pub const fn canonical(&self) -> (Option<&Curve>, Vec3) {
        (self.gen_curve.as_ref(), self.extrusion_vec)
    }

    /// The point on the generator the extrusion parameter starts from (`GetOrigin`).
    #[must_use]
    pub const fn origin(&self) -> Point3 {
        self.origin
    }

    /// The extrusion vector (`GetExtrusionVector`).
    #[must_use]
    pub const fn extrusion_vector(&self) -> Vec3 {
        self.extrusion_vec
    }

    /// The STEP domain: generator parameters by extrusion parameters
    /// (`GetSTEPUVDomain`).
    #[must_use]
    pub const fn step_uv_domain(&self) -> Extent2d {
        self.anal_uv_domain
    }

    /// The generator's orientation to the extrusion (`m_eOrientation`).
    #[must_use]
    pub const fn orientation(&self) -> CurveOrientType {
        self.orientation
    }

    /// The generator's plane point or line point (`GetCurvePoint`).
    #[must_use]
    pub const fn curve_point(&self) -> Point3 {
        self.curve_point
    }

    /// The generator's plane normal or line direction (`GetCurveVec`).
    #[must_use]
    pub const fn curve_vec(&self) -> Vec3 {
        self.curve_vec
    }

    /// `SetCurvePoint`.
    pub const fn set_curve_point(&mut self, p: Point3) {
        self.curve_point = p;
    }

    /// `SetCurveVec`.
    pub const fn set_curve_vec(&mut self, v: Vec3) {
        self.curve_vec = v;
    }

    /// Whether the NURB's v is the generator direction (`GetSwapUVBit`).
    #[must_use]
    pub const fn swap_uv(&self) -> bool {
        self.swap_uv
    }

    /// Flips [`Self::swap_uv`] (`ToggleSwapUVBit`).
    pub const fn toggle_swap_uv_bit(&mut self) {
        self.swap_uv = !self.swap_uv;
    }

    /// Whether out-of-bounds evaluation is enabled (`IsOutOfBoundsEnabled`).
    #[must_use]
    pub fn is_out_of_bounds_enabled(&self) -> bool {
        self.base.is_out_of_bounds_enabled()
    }

    /// Enables or disables out-of-bounds evaluation of the surface and of its
    /// generator (`SetOutOfBoundsEnabled`, which sets the generator's
    /// `SmBSplineCurve` flag too).
    pub fn set_out_of_bounds_enabled(&mut self, enabled: bool) {
        *self.base.out_of_bounds_enabled_mut() = enabled;
        if let Some(gen) = self.gen_curve.as_mut() {
            if let Ok(b) = gen_base_mut(gen) {
                b.set_out_of_bounds_enabled(enabled);
            }
        }
    }

    /// `SetContext`: contexts are not modelled (no effect).
    pub const fn set_context(&self) {}

    /// `SmSurfOfExtrusion_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmSurfOfExtrusion"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmSurfOfExtrusion"
    }

    /// `"SmSurfOfExtrusion"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmSurfOfExtrusion"
    }

    /// `SmSurfOfExtrusion_TYPE` or a type the B-spline surface is (`IsKindOf`).
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

    /// The STEP extrusion interval and its STEP parameter, always v
    /// (`GetSTEPLinearParamExtent`).
    #[must_use]
    pub fn step_linear_param_extent(&self) -> (Extent1d, SurfParamType) {
        (self.anal_uv_domain.v_interval(), SurfParamType::V)
    }

    /// The NURB interval of the generator direction and its NURB parameter: u, or v
    /// with [`Self::swap_uv`] (`GetGenDirParamExtent`).
    #[must_use]
    pub fn gen_dir_param_extent(&self) -> (Extent1d, SurfParamType) {
        // get m_pNurb UVDomain
        let nurb_uv_domain = self.base.natural_uv_domain();
        if self.swap_uv {
            (nurb_uv_domain.v_interval(), SurfParamType::V)
        } else {
            (nurb_uv_domain.u_interval(), SurfParamType::U)
        }
    }

    /// The generator's maximum analytic domain by the STEP V interval expanded by 100
    /// times its length (`GetMaxAnalyticDomain`; a reversed result leaves the STEP
    /// domain, as upstream's `SetMinMax`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator; those of the generator's domain.
    pub fn max_analytic_domain(&self, k: &Kernel) -> Result<Extent2d> {
        let mut dom = self.anal_uv_domain;

        // Expand u and v separately.
        let u_dom = gen_max_analytic_domain(k, self.gen()?)?;
        let mut v_dom = dom.v_interval();
        v_dom.expand_relative(100.0);

        let _ = dom.set_min_max_coords(u_dom.min(), v_dom.min(), u_dom.max(), v_dom.max());
        Ok(dom)
    }

    // ---- equality -------------------------------------------------------------------

    /// `operator==`: equal B-spline surfaces, generators, origins, extrusion vectors,
    /// STEP domains, orientations, swap flags and generator plane / line data.
    ///
    /// # Errors
    ///
    /// Those of the generator comparison.
    pub fn approx_eq(&self, k: &Kernel, other: &Self) -> Result<bool> {
        // low work
        if core::ptr::eq(self, other) {
            return Ok(true);
        }

        // first check the base
        if !self.base.approx_eq(&other.base) {
            return Ok(false);
        }

        // check equivalence of these objects
        let gen_eq = match (&self.gen_curve, &other.gen_curve) {
            (None, None) => true,
            (Some(a), Some(b)) => a.approx_eq(k, b)?,
            _ => false,
        };
        Ok(gen_eq
            && self.origin.approx_eq(other.origin)
            && self.extrusion_vec.approx_eq(other.extrusion_vec)
            && self.anal_uv_domain.are_same(&other.anal_uv_domain)
            && self.orientation == other.orientation
            && self.swap_uv == other.swap_uv
            && self.curve_point.approx_eq(other.curve_point)
            && self.curve_vec.approx_eq(other.curve_vec))
    }

    // ---- domains --------------------------------------------------------------------

    /// Trims the surface to the STEP domain `new_domain` and rebuilds the NURB
    /// (`AdjustSTEPUVDomain`): nothing when the domains agree within `SM_EFF_ZERO`;
    /// else the domain is set, the generator's STEP interval adjusted to its U
    /// interval, the NURB reparameterised to it (transposed with
    /// [`Self::swap_uv`]; status ignored) and remade.
    ///
    /// # Errors
    ///
    /// Those of the generator's interval adjustment and [`Self::make_nurb`].
    pub fn adjust_step_uv_domain(&mut self, k: &Kernel, new_domain: &Extent2d) -> Result<()> {
        // no work - new UVDomain == current UVDomain
        if new_domain.is_contained_by(&self.anal_uv_domain, EFF_ZERO)
            && self.anal_uv_domain.is_contained_by(new_domain, EFF_ZERO)
        {
            return Ok(());
        }

        // set the new domain
        self.anal_uv_domain = *new_domain;

        // set GenCurve analytic domain from U part of new surface domain
        let u_ivl = self.anal_uv_domain.u_interval();
        self.adjust_gen_step_interval(k, &u_ivl)?;

        // build Nurb domain from Analytic domain
        let mut nurb_domain = *new_domain;
        if self.swap_uv {
            nurb_domain.transpose();
        }

        // set Surface and GenCurve nurb domain (status ignored)
        ignore_status(self.reparameterize(k, &nurb_domain))?;

        // rebuild the associated m_pNurb
        self.make_nurb(k)
    }

    /// Sets the STEP domain from the NURB domain `new_nurbs_domain` without
    /// rebuilding the NURB (`UpdateAnalyticalDomain`): the domain spans the STEP
    /// images of its corners, the generator's STEP interval is adjusted to its U
    /// interval, the generator's knots scaled to the NURB generator interval (status
    /// ignored) and the origin moved to the generator's natural start (status
    /// ignored).
    ///
    /// # Errors
    ///
    /// Those of the corner conversions (upstream ignores them and reads the unset
    /// points) and of the generator's interval adjustment.
    pub fn update_analytical_domain(
        &mut self,
        k: &Kernel,
        new_nurbs_domain: &Extent2d,
    ) -> Result<()> {
        // convert given NurbUVDomain corners into STEPUVPoints
        let min_anal_uv = self.convert_uv_from_nurbs_to_step(k, new_nurbs_domain.min())?;
        let max_anal_uv = self.convert_uv_from_nurbs_to_step(k, new_nurbs_domain.max())?;

        // Build desired new STEP UVDomain
        self.anal_uv_domain.init();
        self.anal_uv_domain.add_point(min_anal_uv);
        self.anal_uv_domain.add_point(max_anal_uv);

        // set GenCurve analytic domain from U part of new surface Anal domain.
        let u_ivl = self.anal_uv_domain.u_interval();
        self.adjust_gen_step_interval(k, &u_ivl)?;

        // make sure m_pGenCurve Nurb domain matches crNewNurbsDomain (status ignored)
        let gen_ivl = if self.swap_uv {
            new_nurbs_domain.v_interval()
        } else {
            new_nurbs_domain.u_interval()
        };
        let gen = self
            .gen_curve
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_GEN))?;
        ignore_status(gen_base_mut(gen)?.scale_knot_vector(gen_ivl.min(), gen_ivl.max()))?;

        // recompute a m_vOrigin point to ensure that the origin is still on the genCurve
        if let Some(p) = ignored(gen.evaluate_point(k, gen.natural_interval().min()))? {
            self.origin = p;
        }
        Ok(())
    }

    /// Reparameterises the NURB to `trim_domain` without changing its shape
    /// (`Reparameterize`; `N_SrfReparamToInterval`, status ignored), and the
    /// generator's NURB to its generator interval (status ignored). Nothing when the
    /// STEP domain equals `trim_domain` (as upstream compares them, even swapped). A
    /// missing NURB is made first.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when there is no NURB to reparameterise.
    pub fn reparameterize(&mut self, k: &Kernel, trim_domain: &Extent2d) -> Result<()> {
        if self.anal_uv_domain.is_contained_by(trim_domain, 0.0)
            && trim_domain.is_contained_by(&self.anal_uv_domain, 0.0)
        {
            return Ok(());
        }

        // new NURB locals
        let rect = Rectangle {
            ul: trim_domain.min().x,
            ur: trim_domain.max().x,
            vb: trim_domain.min().y,
            vt: trim_domain.max().y,
        };

        // GetOrCreateGwNurbPointer: the virtual MakeNurb (status unchecked upstream)
        if self.base.nurb.is_none() {
            ignore_status(self.make_nurb(k))?;
        }
        let sur = self
            .base
            .nurb
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_REPARAM))?;

        // set NURB Surface domain parameterization - no change in shape
        let _ = sur.reparam_to_interval(rect, Direction::UV);

        // Rule 2 (status ignored)
        let gen_ivl = if self.swap_uv {
            trim_domain.v_interval()
        } else {
            trim_domain.u_interval()
        };
        let gen = self
            .gen_curve
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_GEN))?;
        ignore_status(gen.edit_parameterization(k, &gen_ivl))?;
        Ok(())
    }

    // ---- parameter conversions --------------------------------------------------------

    /// The NURB's natural UV domain (`GetNaturalUVDomain`); `SM_ERR` without NURB, where
    /// upstream's `const` method would make it.
    fn nurb_uv_domain(&self, op: &'static str) -> Result<Extent2d> {
        if self.base.gw_nurb().is_none() {
            return Err(SolidError::sm_err(op));
        }
        Ok(self.base.natural_uv_domain())
    }

    /// The NURB parameters of the STEP parameters `step_uv`
    /// (`ConvertUVFromSTEPToNURBS`): the generator's conversion in its direction, the
    /// linear map of the STEP V interval onto the NURB extrusion interval in the
    /// other, swapped with [`Self::swap_uv`].
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB (upstream makes it); `SM_ERR_NULL_POINTER` without
    /// generator; those of the generator's conversion.
    pub fn convert_uv_from_step_to_nurbs(&self, k: &Kernel, step_uv: Point2) -> Result<Point2> {
        // locals
        let natural_uv_domain = self.nurb_uv_domain(OP_CONVERT)?;
        let step_uv_domain = self.anal_uv_domain;

        // Map the STEP u-param to Nurbs u:
        let mut nurbs_uv = Point2::new(0.0, 0.0);
        nurbs_uv.x = self.gen()?.convert_t_from_step_to_nurbs(k, step_uv.x)?;

        // Map the STEP v-param to Nurbs v: a linear mapping
        let fraction = (step_uv.y - step_uv_domain.min().y) / step_uv_domain.y_length();
        let nurbs_extrusion_domain = if self.swap_uv {
            natural_uv_domain.u_interval()
        } else {
            natural_uv_domain.v_interval()
        };
        nurbs_uv.y = nurbs_extrusion_domain.evaluate(fraction);

        // when SwapUV is true, remember to swap the NurbsUV values
        if self.swap_uv {
            core::mem::swap(&mut nurbs_uv.x, &mut nurbs_uv.y);
        }
        Ok(nurbs_uv)
    }

    /// The STEP parameters of the NURB parameters `nurbs_uv`
    /// (`ConvertUVFromNURBSToSTEP`): the inverse of
    /// [`Self::convert_uv_from_step_to_nurbs`].
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB (upstream makes it); `SM_ERR_NULL_POINTER` without
    /// generator; those of the generator's conversion.
    pub fn convert_uv_from_nurbs_to_step(&self, k: &Kernel, nurbs_uv: Point2) -> Result<Point2> {
        // locals
        let mut s_nurbs_uv = nurbs_uv;

        // when SwapUV is true, remember to transpose the NurbsUV values
        if self.swap_uv {
            core::mem::swap(&mut s_nurbs_uv.x, &mut s_nurbs_uv.y);
        }

        // STEP u-param == gen curve param, possibly with its own mapping:
        let mut step_uv = Point2::new(0.0, 0.0);
        step_uv.x = self.gen()?.convert_t_from_nurbs_to_step(k, s_nurbs_uv.x)?;

        // STEP v-param: linear mapping:
        let step_uv_domain = self.anal_uv_domain.v_interval();
        let natural_uv_domain = self.nurb_uv_domain(OP_CONVERT)?;
        let nurb_uv_domain = if self.swap_uv {
            natural_uv_domain.u_interval()
        } else {
            natural_uv_domain.v_interval()
        };
        let fraction = (s_nurbs_uv.y - nurb_uv_domain.min()) / nurb_uv_domain.length();
        step_uv.y = step_uv_domain.evaluate(fraction);
        Ok(step_uv)
    }

    // ---- evaluation -----------------------------------------------------------------

    /// The point and derivatives at the STEP parameters `uv` (`EvaluateSTEP`):
    /// `(hu + 1) * (hv + 1)` vectors, entry `i * (hv + 1) + j` the derivative of order
    /// `i` in u and `j` in v. Only the position, `Sv = E`, the pure u derivatives (the
    /// generator's STEP derivatives at `uv.x`, evaluated with the surface's
    /// out-of-bounds flag) are non-zero. `uv` is not clamped (upstream clamps a copy it
    /// does not use).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an order above 5 (`MAXDER`); `SM_ERR_NULL_POINTER` without
    /// generator; those of the generator's evaluation.
    pub fn evaluate_step(&self, k: &Kernel, uv: Point2, hu: usize, hv: usize) -> Result<Vec<Vec3>> {
        // (an order above the limit is refused before the array is read)
        let n = if hu <= 5 && hv <= 5 {
            (hu + 1) * (hv + 1)
        } else {
            0
        };
        let mut d = vec![Vec3::ZERO; n];
        self.evaluate_step_into(k, uv, hu, hv, &mut d)?;
        Ok(d)
    }

    /// [`Self::evaluate_step`] into the caller's array `d` (`(hu + 1) * (hv + 1)` entries):
    /// the entries upstream does not write keep the caller's values, as upstream's
    /// `aDerivatives` does (a fresh `SmVector3d` array holds `SM_UNDEF_DOUBLE`).
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_step`]; `InvalidInput` when `d` is too short.
    pub(crate) fn evaluate_step_into(
        &self,
        k: &Kernel,
        uv: Point2,
        hu: usize,
        hv: usize,
        d: &mut [Vec3],
    ) -> Result<()> {
        // check state:
        if hu > MAX_DERIV || hv > MAX_DERIV {
            return Err(SolidError::sm_err(OP_EVAL_STEP));
        }

        let n = (hu + 1) * (hv + 1);
        let d = d
            .get_mut(..n)
            .ok_or_else(|| SolidError::invalid_input(OP_EVAL_STEP, "derivative array too short"))?;

        // Set zero derivative values - anything with a partial V term (only those to
        // be set: the rest of the caller's array is left untouched)
        for i in 0..=hu {
            for j in 0..=hv {
                if j > 0 && i + j <= hu && i + j <= hv {
                    if let Some(slot) = d.get_mut(i * (hv + 1) + j) {
                        *slot = Vec3::ZERO;
                    }
                }
            }
        }

        // (per SMLib practice upstream clamps a copy of uv it never reads)

        // the generator with the surface's out-of-bounds flag
        let gen_own = self.gen()?;
        let tmp = gen_with_out_of_bounds(gen_own, self.base.is_out_of_bounds_enabled());
        let gen = tmp.as_ref().unwrap_or(gen_own);

        // get GenCurve position and requested derivative values
        let pv = gen_evaluate_step(k, gen, uv.x, hu)?;
        let pv_at = |i: usize| pv.get(i).copied().unwrap_or(Vec3::UNDEF);

        let mut set = |i: usize, v: Vec3| {
            if let Some(slot) = d.get_mut(i) {
                *slot = v;
            }
        };

        // position
        set(0, pv_at(0) + uv.y * self.extrusion_vec);

        // 1st derivatives
        if hv >= 1 {
            set(1, self.extrusion_vec);
        }

        // all U derivatives
        for i in 1..=hu {
            set(i * (hv + 1), pv_at(i));
        }
        Ok(())
    }

    /// The point at the STEP parameters `uv` (`EvaluateSTEPPoint`): the generator's
    /// STEP point at `uv.x` (with its own out-of-bounds flag) plus `uv.y` times the
    /// extrusion vector. `uv` is not clamped (upstream clamps a copy it does not use).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator; those of the generator's evaluation.
    pub fn evaluate_step_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        let pv = first(&gen_evaluate_step(k, self.gen()?, uv.x, 0)?)?;
        Ok(pv + uv.y * self.extrusion_vec)
    }

    // ---- inversion and drops ----------------------------------------------------------

    /// The STEP parameters of `point_on_surf` (which must lie on the surface within
    /// `tol_3d`) and its location (`STEPInversion`). A generator perpendicular to the
    /// extrusion projects the point onto the extrusion line through the origin (v) and
    /// back onto the generator's plane, then intersects the generator
    /// (`GlobalPointSolveSTEP`, over its maximum analytic domain with out-of-bounds
    /// evaluation): an interior point, or with several solutions on the boundary of a
    /// closed generator a U seam (set to the domain end nearer `uv_guess.x`). Any
    /// other orientation inverts the NURB over the NURB image of `anal_domain`
    /// ([`BSplineSurface::step_inversion`]) and converts back.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the projected point is not on the generator; those of the
    /// generator's solve; [`SolidError::Unported`] from
    /// [`BSplineSurface::step_inversion`].
    pub fn step_inversion(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<(Point2, LocationType)> {
        // When the extrusion was not perpendicular to the genCurve plane
        if self.orientation != CurveOrientType::Perpendicular {
            // convert analytical domain to NURBS domain (statuses ignored upstream)
            let nurbs_uv_min = self.convert_uv_from_step_to_nurbs(k, anal_domain.min())?;
            let nurbs_uv_max = self.convert_uv_from_step_to_nurbs(k, anal_domain.max())?;
            let nurbs_domain = Extent2d::from_points(nurbs_uv_min, nurbs_uv_max);

            // map 3dPoint to NurbUV parameter
            let (nurbs_param, location) =
                self.base
                    .step_inversion(k, &nurbs_domain, point_on_surf, tol_3d, uv_guess)?;

            // convert NurbUV point to StepUV point
            let uv = self.convert_uv_from_nurbs_to_step(k, nurbs_param)?;
            return Ok((uv, location));
        }

        // init output
        let mut location = LocationType::Interior;
        let mut uv = Point2::new(0.0, 0.0);

        // build extrusion line off of m_vOrigin that spans extrusion surface
        let z_axis = self.extrusion_vec;
        let extrusion_length = self.anal_uv_domain.size().y;
        let line_vec = z_axis * extrusion_length;
        let line_origin = self.origin + self.anal_uv_domain.min().y * self.extrusion_vec;

        // find UVAnal.X value by projecting PointOnSurf onto Sweep Line
        let line_param = line_closest_point(line_origin, line_vec, point_on_surf);
        let anal_v_domain = self.anal_uv_domain.v_interval();
        uv.y = anal_v_domain.min() + line_param * anal_v_domain.length();

        // Project PointOnSurf along sweep direction back to plane containing GenCurve
        let point_in_plane_param = line_closest_point(point_on_surf, line_vec, self.origin);
        let point_in_plane = point_on_surf + point_in_plane_param * line_vec;

        // Find where the projected point intersects the GenCurve
        let mut gen = self.gen()?.clone();
        let mut ivl = gen_step_interval(&gen)?;
        if self.is_out_of_bounds_enabled() {
            ivl = gen_max_analytic_domain(k, &gen)?;
            gen_base_mut(&mut gen)?.set_out_of_bounds_enabled(true);
        }

        // SmHyperbola / SmParabola::GlobalPointSolveSTEP first adjusts the (owned)
        // generator to the interval; the MakeNurb that runs then reparameterises the
        // rebuilt NURB to this surface's generator interval (the solve's own adjustment
        // below is then a no-op).
        if matches!(gen.kind(), CurveKind::Hyperbola(_) | CurveKind::Parabola(_)) {
            adjust_generator_interval(k, &mut gen, &ivl, self.owner_ivl())?;
        }
        let (this, analytic) = CurveHandle::gen_pair(&gen);
        let solutions = gen.global_point_solve_step::<CurveHandle>(
            k,
            &ivl,
            SolverOperation::Intersect,
            point_in_plane,
            tol_3d,
            None,
            None,
            SolutionRequested::All,
            Some(this),
            Some(analytic),
        )?;

        // failure - projected point is not on GenCurve
        let Some(sol) = solutions.first() else {
            return Err(SolidError::sm_err(OP_STEP_INV));
        };

        // set outputs
        uv.x = sol.start.parameters[0];

        // when point falls on a seam - remember that in the output
        if solutions.len() > 1
            && gen.is_closed(k, &ivl, tol_3d)?
            && ivl.is_value_on_boundary(uv.x, EFF_ZERO).0
        {
            if location == LocationType::Interior {
                location = LocationType::USeam;
            }

            // On a seam.  If a guess was given, set u-parameter to what's closer to the
            // guess.
            if let Some(guess) = uv_guess {
                let anal_u_domain = self.anal_uv_domain.u_interval();
                if guess.x > anal_u_domain.mid() {
                    uv.x = anal_u_domain.max();
                }
            }
        }
        Ok((uv, location))
    }

    /// The fast point drop of a generator perpendicular to the extrusion
    /// (`DropPointFast`): the sweep parameter is the test point's projection onto the
    /// extrusion line (clamped to the domain, mapped onto the NURB domain's diagonal),
    /// the generator parameters solve `op` for the test point projected onto the
    /// generator's plane over `uv_domain`'s generator interval (with the surface's
    /// out-of-bounds flag). With several solutions on a periodic generator and
    /// `in_pointing_vector`, the interval-end solutions whose tangent points against
    /// it (by more than `SM_EFF_ZERO_SQRT`) are skipped. Each solution names `this`
    /// and carries the NURB parameters and the 3D distance to the test point.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for any other orientation (the caller then drops generally);
    /// `SM_ERR_NULL_POINTER` without generator; those of [`Self::make_nurb`] (on an
    /// evaluation copy when the NURB is missing), the generator's solve and the
    /// evaluations.
    #[allow(clippy::too_many_arguments)] // upstream's DropPointFast signature
    pub fn drop_point_fast<O: SolutionObject>(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        op: SolverOperation,
        test_point: Point3,
        in_pointing_vector: Option<Vec3>,
        dist_tol: f64,
        target_distance: Option<f64>,
        requested: SolutionRequested,
        this: Option<O>,
    ) -> Result<SolutionArray<O>> {
        // return SM_ERR when this function can't compute a solution.
        if self.orientation != CurveOrientType::Perpendicular {
            return Err(SolidError::sm_err(OP_DROP));
        }

        // upstream's GetNaturalUVDomain makes a missing NURB (GetOrCreateGwNurbPointer);
        // solve on an evaluation copy that has it, as the inherited queries do
        if self.base.gw_nurb().is_none() {
            let mut copy = self.clone_for_evaluation(0)?;
            copy.make_nurb(k)?;
            if copy.base.gw_nurb().is_none() {
                return Err(SolidError::sm_err(OP_DROP));
            }
            return copy.drop_point_fast(
                k,
                uv_domain,
                op,
                test_point,
                in_pointing_vector,
                dist_tol,
                target_distance,
                requested,
                this,
            );
        }

        // init output
        let mut out: SolutionArray<O> = SolutionArray::new();

        // locals
        let nat_domain = self.nurb_uv_domain(OP_DROP)?;
        let line_vec = self.anal_uv_domain.y_length() * self.extrusion_vec;
        let line_origin = self.origin + self.anal_uv_domain.min().y * self.extrusion_vec;
        let ivl = if self.swap_uv {
            uv_domain.v_interval()
        } else {
            uv_domain.u_interval()
        };

        // If we are allowing out-of-bounds eval, then the gen curve needs to too.
        let gen_own = self.gen()?;
        let tmp = gen_with_out_of_bounds(gen_own, self.base.is_out_of_bounds_enabled());
        let gen = tmp.as_ref().unwrap_or(gen_own);

        // Get and save the sweep param value
        let line_param = line_closest_point(line_origin, line_vec, test_point).clamp(0.0, 1.0);
        let mut uv_point = nat_domain.evaluate(line_param, line_param);

        // Project TestPoint to GenCurve Plane
        let point_in_plane_param = line_closest_point(test_point, line_vec, self.origin);
        let point_in_plane = test_point + point_in_plane_param * line_vec;

        // Now solve the problem for TestPoint projected to GenCurvePlane
        let (gen_handle, analytic_handle) = CurveHandle::gen_pair(gen);
        let solutions = gen.global_point_solve::<CurveHandle>(
            k,
            &ivl,
            op,
            point_in_plane,
            dist_tol,
            requested,
            &GlobalPointSolveOptions {
                target_distance,
                opt_vectors: None,
                handle: Some(gen_handle),
                analytic_handle: Some(analytic_handle),
                zone_tol_3d: None,
            },
        )?;

        let mut sol: Solution<O> = Solution::new();
        sol.solution_type = SolutionType::SingleValue;
        sol.num_objects = 1;
        sol.objects[0] = this;
        sol.nodes[0] = None;
        sol.num_variables = 2;

        // for every solution - add a solution to the output solution array
        let n_sols = solutions.len();
        for cr_sol in solutions.as_slice() {
            let t = cr_sol.start.parameters[0];

            // when given an option interior pointing vector and we have a possible set
            // of seam solutions - skip solutions not wanted
            if let Some(in_vec) = in_pointing_vector {
                if n_sols > 1 && gen.is_periodic(k, &ivl)? {
                    // when solution is at start of curve
                    if are_same(t, ivl.min()) {
                        let pv = gen.evaluate(k, t, 1, true, false)?;
                        let dot = pv.get(1).copied().unwrap_or(Vec3::UNDEF).dot(in_vec);
                        if dot < -EFF_ZERO_SQRT {
                            continue;
                        }
                    }

                    // when solution is at end of curve
                    if are_same(t, ivl.max()) {
                        let pv = gen.evaluate(k, t, 1, true, false)?;
                        let dot = pv.get(1).copied().unwrap_or(Vec3::UNDEF).dot(in_vec);
                        if dot > EFF_ZERO_SQRT {
                            continue;
                        }
                    }
                }
            }

            // set the output GenCurve parameter value (sweep param value is already set)
            if self.swap_uv {
                uv_point.y = t;
            } else {
                uv_point.x = t;
            }

            // compute and save the drop point distance as the m_dSolutionValue
            let point_on_surf = self.base.evaluate_point(uv_point)?;
            sol.start.solution_value = point_on_surf.distance(test_point);

            // save the UVPoint value as the Solution Param values
            sol.start.parameters[0] = uv_point.x;
            sol.start.parameters[1] = uv_point.y;

            // add solution to output array
            out.add(&sol);
        }
        Ok(out)
    }

    /// The directrix through `point_on_surface`: the B-spline line segment along the
    /// extrusion vector spanning the STEP V interval (`CreateDirectrixFromPoint`). The
    /// point's height (extrusion parameter) is found from the orientation: the
    /// closest points of the extrusion and generator lines (linear), the projection
    /// onto the extrusion line (perpendicular), the line/plane intersection
    /// (independent), else the intersection of the generator over its STEP interval
    /// with the line through the point along the extrusion (`IntersectWithLine`, the
    /// general curve intersection when that needs more or finds nothing; line domain
    /// `[-10000, 10000]`). A height within `tol` of an end of the interval is snapped
    /// to it.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the line misses the generator; those of the closest-point and
    /// plane computations and of [`BSplineCurve::create_line_segment`].
    pub fn create_directrix_from_point(
        &self,
        k: &Kernel,
        point_on_surface: Point3,
        tol: f64,
    ) -> Result<BSplineCurve> {
        let mut current_height = 0.0;

        // switch on GenCurve Orientation
        match self.orientation {
            CurveOrientType::Linear => {
                // get distance along line:[m_vOrigin, ExtrusionVec] to the curve line
                current_height = line_line_closest_point(
                    self.origin,
                    self.extrusion_vec,
                    self.curve_point,
                    self.curve_vec,
                )?
                .0;
            }
            CurveOrientType::Perpendicular => {
                // get closest point on line:[origin, ExtrusionVec] to rPointOnSurface
                current_height =
                    line_closest_point(self.origin, self.extrusion_vec, point_on_surface);
            }
            CurveOrientType::Independent => {
                // Intersect line:[PointOnSurface,ExtrusionVec] with plane containing curve.
                let line_param = line_plane_intersect(
                    point_on_surface,
                    self.extrusion_vec,
                    self.origin,
                    self.curve_vec,
                )?;
                current_height = -line_param;
            }
            CurveOrientType::Coplanar | CurveOrientType::NotPlanar => {
                // line and curve locals
                let line = Line::infinite(point_on_surface, self.extrusion_vec, 3, false);
                let line_domain = Extent1d::from_min_max(-10000.0, 10000.0); // sort of infinite
                let curve_domain = self.anal_uv_domain.u_interval();
                let gen = self.gen()?;

                // Get Curve/Line intersection (status ignored)
                let (needs_more, mut params) = match ignored(gen.intersect_with_line(
                    k,
                    &curve_domain,
                    &line,
                    &line_domain,
                    tol,
                ))? {
                    Some(r) => (
                        r.needs_more_intersections,
                        r.solutions
                            .iter()
                            .map(|s| s.start.parameters[1])
                            .collect::<Vec<f64>>(),
                    ),
                    None => (true, Vec::new()),
                };

                if needs_more || params.is_empty() {
                    let line_curve = Curve::from(line);
                    let (gen_handle, gen_analytic) = CurveHandle::gen_pair(gen);
                    let line_ivl = line_curve.natural_interval();
                    let line_handle = CurveHandle {
                        id: 2,
                        ivl: line_ivl,
                    };
                    let line_analytic = CurveHandle {
                        id: 3,
                        ivl: line_ivl,
                    };
                    params = match ignored(gen.global_curve_intersect::<CurveHandle>(
                        k,
                        &curve_domain,
                        &line_curve,
                        &line_domain,
                        tol,
                        &CurveIntersectOptions {
                            skip_global_coincidence_check: false,
                            handles: [Some(gen_handle), Some(line_handle)],
                            analytic_handles: [Some(gen_analytic), Some(line_analytic)],
                        },
                    ))? {
                        Some(sols) => sols
                            .as_slice()
                            .iter()
                            .map(|s| s.start.parameters[1])
                            .collect(),
                        None => Vec::new(),
                    };
                }

                // CurrentHeight equals negative Line Param solution
                match params.first() {
                    Some(&p) => current_height = -p,
                    None => return Err(SolidError::sm_err(OP_DIRECTRIX)),
                }
            }
            CurveOrientType::Undefined => {}
        }

        // get surface sweep parameter range
        let height_ivl = self.step_linear_param_extent().0;

        // snap projected points close to Domain boundaries
        if height_ivl.is_value_on_boundary(current_height, tol).0 {
            current_height = height_ivl.clamp_value(current_height);
        }

        // create line running through rPointOnSurface from domain start to end
        let bot_pt = point_on_surface - (current_height - height_ivl.min()) * self.extrusion_vec;
        let top_pt = point_on_surface + (height_ivl.max() - current_height) * self.extrusion_vec;

        BSplineCurve::create_line_segment(3, bot_pt, top_pt)
    }

    // ---- transform ------------------------------------------------------------------

    /// Scales (about the origin, isotropically) and then rotates and moves the
    /// surface (`Transform`): the NURB, the generator (status ignored), the origin,
    /// extrusion vector and generator plane / line data. The identity frame with no or
    /// unit scale changes nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a non-isotropic scale (nothing changed); those of
    /// [`BSplineSurface::transform`].
    pub fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        self.transform_at_depth(k, rotate_n_move, scale, 0)
    }

    /// Carries the caller's whole-geometry depth into the generator transform.
    pub(crate) fn transform_at_depth(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
        depth: usize,
    ) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        // check input - disallow non-isotropic scaling
        let mut d_scale = 1.0;
        if let Some(s) = scale {
            if s.distance_squared(identity_scale) > EFF_ZERO_SQ {
                // when scale factors are not all the same
                d_scale = s.x;
                if !are_same(s.y, d_scale) || !are_same(s.z, d_scale) {
                    return Err(SolidError::sm_err(OP_XFORM));
                }
            }
        }

        // transform m_pNurb
        // SmBSplineSurface::Transform reaches the virtual GetOrCreateGwNurbPointer.
        if self.base.gw_nurb().is_none() {
            ignore_status(self.make_nurb(k))?;
        }
        self.base.transform(rotate_n_move, scale)?;

        // transform the rest of the 3d data (status ignored)
        if let Some(gen) = self.gen_curve.as_mut() {
            ignore_status(gen.transform_at_depth(k, rotate_n_move, scale, depth + 1))?;
        }

        // Scale first.
        if d_scale != 1.0 {
            self.origin *= d_scale;
            self.extrusion_vec *= d_scale;
            self.curve_point *= d_scale;
        }

        self.origin = rotate_n_move.transform_point(self.origin);
        self.extrusion_vec = rotate_n_move.transform_vector(self.extrusion_vec);
        self.curve_point = rotate_n_move.transform_point(self.curve_point);
        self.curve_vec = rotate_n_move.transform_vector(self.curve_vec);
        Ok(())
    }
}

/// `AdjustSTEPInterval` of the generator `gen` owned by a surface of extrusion whose
/// generator-direction NURB interval is `owner_ivl` (`None` while it has no NURB): the
/// `SmLine`, `SmHyperbola` and `SmParabola` `MakeNurb` run by the adjustment then
/// reparameterise the rebuilt NURB to that interval (status ignored).
fn adjust_generator_interval(
    k: &Kernel,
    gen: &mut Curve,
    ivl: &Extent1d,
    owner_ivl: Option<Extent1d>,
) -> Result<()> {
    match gen.kind_mut() {
        CurveKind::BSpline(b) => b.adjust_step_interval(k, ivl),
        CurveKind::Ellipse(e) => e.adjust_step_interval(k, ivl, None),
        CurveKind::Hyperbola(h) => {
            let anal = h.step_interval();
            let no_work =
                ivl.is_contained_by(&anal, EFF_ZERO) && anal.is_contained_by(ivl, EFF_ZERO);
            h.adjust_step_interval(ivl, None)?;
            if !no_work {
                if let Some(nurb_ivl) = owner_ivl {
                    // MakeNurb's owner step: EditParameterization(sNURBIvl, FALSE)
                    // (status ignored)
                    ignore_status(h.base.edit_parameterization(&nurb_ivl))?;
                }
            }
            Ok(())
        }
        CurveKind::Parabola(p) => {
            let anal = p.step_interval();
            let no_work = ivl.is_contained_by(&anal, 0.0) && anal.is_contained_by(ivl, 0.0);
            p.adjust_step_interval(ivl, None)?;
            if !no_work {
                if let Some(nurb_ivl) = owner_ivl {
                    // MakeNurb's owner step: EditParameterization(sNURBIvl, FALSE)
                    // (status ignored)
                    ignore_status(p.base_mut().edit_parameterization(&nurb_ivl))?;
                }
            }
            Ok(())
        }
        CurveKind::Line(l) => {
            let anal = l.step_interval();
            let no_work =
                ivl.is_contained_by(&anal, EFF_ZERO) && anal.is_contained_by(ivl, EFF_ZERO);
            let rebuilds = !no_work && l.base().gw_nurb().is_some();
            l.adjust_step_interval(ivl)?;
            if rebuilds {
                if let Some(nurb_ivl) = owner_ivl {
                    // EditParameterization(sNURBIvl, FALSE) (status ignored)
                    ignore_status(l.edit_parameterization_owned(None, &nurb_ivl))?;
                }
            }
            Ok(())
        }
        _ => Err(not_b_spline()),
    }
}

// ---- sm_IntersectGenerators -------------------------------------------------------------

/// Which generator an offset boundary of a range solution comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    /// The extrusion surface's generator (`pExtrusionGenCurve`).
    This,
    /// The other surface's generator (`&rOtherGenCurve`).
    Other,
}

/// `SmCurve::GlobalCurveSolve(.., SM_SO_PROJECTED_INTERSECT, .., &unitVec, SM_SR_ALL,
/// ..)` of the two generators.
fn global_curve_solve_projected(
    k: &Kernel,
    gen: &Curve,
    gen_ivl: &Extent1d,
    other: &Curve,
    other_ivl: &Extent1d,
    tol: f64,
    unit_vec: Vec3,
) -> Result<SolutionArray<CurveHandle>> {
    let (gen_handle, _) = CurveHandle::gen_pair(gen);
    let other_handle = CurveHandle {
        id: 4,
        ivl: other.natural_interval(),
    };
    let vecs = [unit_vec];
    gen.global_curve_solve::<CurveHandle>(
        k,
        gen_ivl,
        other,
        other_ivl,
        SolverOperation::ProjectedIntersect,
        tol,
        &CurveSolveOptions {
            target_distance: None,
            opt_vectors: Some(&vecs),
            handles: [Some(gen_handle), Some(other_handle)],
        },
        SolutionRequested::All,
    )
}

/// The intersection curves of `surf` with another surface swept from `other_gen`
/// along `other_vec` over `other_ivl` (`sm_IntersectGenerators`): `needs_more` when
/// the vectors are not parallel (within `SM_EFF_ZERO_RAD`, taken as degrees as
/// upstream; the outputs are then untouched) or a range solution is too complicated.
/// Otherwise the generators' intersections projected along the extrusion each give a
/// line along it over the overlap of the two sweeps (or a degenerate curve where they
/// only touch, nothing where they miss); a range solution bounded by two lines and
/// the same translated generator at both ends adds the two translated, trimmed
/// generators, one touching at both ends the one generator segment.
///
/// # Errors
///
/// Those of the projected curve solve (`SmCurve::GlobalCurveSolve`, [`SolidError::Unported`]
/// included), the evaluations and curve constructions.
#[allow(clippy::too_many_lines)] // one upstream function
pub fn intersect_generators(
    k: &Kernel,
    surf: &SurfOfExtrusion,
    tol: f64,
    other_gen: &Curve,
    other_vec: Vec3,
    other_ivl: &Extent1d,
    mut curves: Option<&mut Vec<Curve>>,
    mut types: Option<&mut Vec<TsectCurveType>>,
) -> Result<GeneratorIntersection> {
    // locals
    let extrusion_ivl = surf.step_uv_domain().v_interval();
    let (gen, extrusion_vec) = surf.canonical();
    let ext_gen = gen.ok_or(SolidError::null_pointer(OP_GENERATORS))?;
    let ext_gen_ivl = ext_gen.natural_interval();
    let extrusion_length = extrusion_vec.length();
    let other_length = other_vec.length();

    // check parallel extrusion vectors
    if !other_vec.is_parallel_to(extrusion_vec, EFF_ZERO_RAD) {
        return Ok(GeneratorIntersection {
            needs_more_intersections: true,
        });
    }

    // locals for range solutions: which generator defines the top and bottom of the
    // overlap at the ends of the range solution ([beg/end][bot/top])
    let mut offset_curve: [[Option<Which>; 2]; 2] = [[None; 2]; 2];
    let mut offset_length: [[Option<f64>; 2]; 2] = [[None; 2]; 2];

    // init output
    if let Some(c) = curves.as_deref_mut() {
        c.clear();
    }
    if let Some(t) = types.as_deref_mut() {
        t.clear();
    }
    let mut result = GeneratorIntersection {
        needs_more_intersections: false,
    };

    // Intersect 2 genCurves along a projection of the of sExtrusionVec (Unitize status
    // ignored)
    let unit_extrusion_vec = extrusion_vec.unitize_or_z();
    let solutions = global_curve_solve_projected(
        k,
        ext_gen,
        &ext_gen_ivl,
        other_gen,
        &other_gen.natural_interval(),
        tol,
        unit_extrusion_vec,
    )?;

    // all done - no solutions
    if solutions.is_empty() {
        return Ok(result);
    }

    // map OtherIvl range onto line=OtherGenCurvePoint + param * ThisExtrusionVec
    let same_extrusion_dir = other_vec.dot(extrusion_vec) > 0.0;
    let other_line_ivl = if same_extrusion_dir {
        Extent1d::from_min_max(
            other_ivl.min() * other_length / extrusion_length,
            other_ivl.max() * other_length / extrusion_length,
        )
    } else {
        Extent1d::from_min_max(
            other_ivl.max() * -other_length / extrusion_length,
            other_ivl.min() * -other_length / extrusion_length,
        )
    };

    // for every curve/curve intersection solution
    for sol in solutions.as_slice() {
        let mut new_curve_count = 0usize;
        let is_range = sol.solution_type == SolutionType::RangeOfValues;

        // once for point solutions and twice for range solutions
        let ends = if is_range { 2 } else { 1 };
        for jj in 0..ends {
            let end = if jj == 0 { &sol.start } else { &sol.end };
            let extrusion_end = end.parameters[0];
            let other_end = end.parameters[1];

            // Get points on each curve that fall on the same sweep vector
            let ep = ext_gen.evaluate_point(k, extrusion_end)?;
            let op = other_gen.evaluate_point(k, other_end)?;

            // the parameter of the sOtherPoint on line = sExtrusionPoint + t * ExtrusionVec
            let line_param = line_closest_point(ep, extrusion_vec, op);

            // map the other line interval onto the extrusion generator's line
            let other_mapped_ivl = Extent1d::from_min_max(
                other_line_ivl.min() + line_param,
                other_line_ivl.max() + line_param,
            );

            // intersect the MappedIvl with the ExtrusionIvl to get overlapIvl
            let xsect_ivl = extrusion_ivl
                .intersect(&other_mapped_ivl)
                .unwrap_or_else(|_| Extent1d::new());

            // no overlap - no output curve
            if xsect_ivl.is_init() {
                continue;
            }

            // when working on a range solution - remember which GenCurve defines the
            // top and bottom of the XSectIvl
            if is_range {
                let pick = |value: f64| -> Option<(Which, f64)> {
                    if value == extrusion_ivl.min() {
                        Some((Which::This, extrusion_ivl.min()))
                    } else if value == extrusion_ivl.max() {
                        Some((Which::This, extrusion_ivl.max()))
                    } else if value == other_mapped_ivl.min() {
                        Some((
                            Which::Other,
                            if same_extrusion_dir {
                                other_ivl.min()
                            } else {
                                other_ivl.max()
                            },
                        ))
                    } else if value == other_mapped_ivl.max() {
                        Some((
                            Which::Other,
                            if same_extrusion_dir {
                                other_ivl.max()
                            } else {
                                other_ivl.min()
                            },
                        ))
                    } else {
                        None
                    }
                };
                for (kk, value) in [xsect_ivl.min(), xsect_ivl.max()].into_iter().enumerate() {
                    if let Some((w, len)) = pick(value) {
                        if let Some(row) = offset_curve.get_mut(jj) {
                            if let Some(slot) = row.get_mut(kk) {
                                *slot = Some(w);
                            }
                        }
                        if let Some(row) = offset_length.get_mut(jj) {
                            if let Some(slot) = row.get_mut(kk) {
                                *slot = Some(len);
                            }
                        }
                    }
                }
            }

            // build an intersection line running parallel to the extrusion vector
            let (line_or_degenerate, tsect_type) = if xsect_ivl.length() * extrusion_length < tol {
                // single point of contact: build degenerate curve
                new_curve_count += 1;
                (
                    BSplineCurve::create_degenerate_curve(3, ep + xsect_ivl.mid() * extrusion_vec)?,
                    TsectCurveType::Touching,
                )
            } else {
                // overlap: build line
                new_curve_count += 1;
                (
                    BSplineCurve::create_line_segment(
                        3,
                        ep + xsect_ivl.min() * extrusion_vec,
                        ep + xsect_ivl.max() * extrusion_vec,
                    )?,
                    if sol.solution_type == SolutionType::SingleValue {
                        TsectCurveType::Crossing
                    } else {
                        TsectCurveType::RegionBoundary
                    },
                )
            };

            // set outputs
            if let Some(c) = curves.as_deref_mut() {
                c.push(Curve::from(line_or_degenerate));
            }
            if let Some(t) = types.as_deref_mut() {
                t.push(tsect_type);
            }
        }

        // for range Solutions - need to add in segments of the GenCurves to finish
        // bounding the coincident region
        if is_range {
            let size = curves.as_deref().map_or(0, Vec::len);
            let type_at = |types: &Option<&mut Vec<TsectCurveType>>, back: usize| {
                types
                    .as_deref()
                    .and_then(|t| size.checked_sub(back).and_then(|i| t.get(i).copied()))
            };
            let both = |want: TsectCurveType| {
                type_at(&types, 1) == Some(want) && type_at(&types, 2) == Some(want)
            };
            let same_offset = |kk: usize| {
                let c0 = offset_curve
                    .first()
                    .and_then(|r| r.get(kk))
                    .copied()
                    .flatten();
                let c1 = offset_curve
                    .get(1)
                    .and_then(|r| r.get(kk))
                    .copied()
                    .flatten();
                let l0 = offset_length
                    .first()
                    .and_then(|r| r.get(kk))
                    .copied()
                    .flatten();
                let l1 = offset_length
                    .get(1)
                    .and_then(|r| r.get(kk))
                    .copied()
                    .flatten();
                match (c0, c1, l0, l1) {
                    (Some(a), Some(b), Some(x), Some(y)) => a == b && is_zero(x - y),
                    _ => false,
                }
            };

            // first simple case - coincident region bounded by 4 curves
            if curves.is_some()
                && new_curve_count == 2
                && both(TsectCurveType::RegionBoundary)
                && same_offset(0)
                && same_offset(1)
            {
                // Add translated and trimmed copy of the appropriate GenCurve for the
                // bottom (kk=0) and top (kk==1) boundary
                for kk in 0..2 {
                    let which = offset_curve
                        .first()
                        .and_then(|r| r.get(kk))
                        .copied()
                        .flatten()
                        .unwrap_or(Which::This);
                    let len = offset_length
                        .first()
                        .and_then(|r| r.get(kk))
                        .copied()
                        .flatten()
                        .unwrap_or(0.0);
                    let tgt_index = usize::from(which == Which::Other);
                    let (s, e) = (
                        sol.start.parameters.get(tgt_index).copied().unwrap_or(0.0),
                        sol.end.parameters.get(tgt_index).copied().unwrap_or(0.0),
                    );
                    let mut trim = Extent1d::from_min_max(s.min(e), s.max(e));
                    let copied = translated_trimmed(
                        k,
                        if which == Which::Other {
                            other_gen
                        } else {
                            ext_gen
                        },
                        len * if tgt_index == 0 {
                            extrusion_vec
                        } else {
                            other_vec
                        },
                        &mut trim,
                    )?;
                    if let Some(c) = curves.as_deref_mut() {
                        c.push(copied);
                    }
                    if let Some(t) = types.as_deref_mut() {
                        t.push(TsectCurveType::RegionBoundary);
                    }
                }
            }
            // Second simple case - the projected intersection is a 3d intersection
            else if curves.is_some() && new_curve_count == 2 && both(TsectCurveType::Touching) {
                // remove and delete the last two degenerate curves
                for _ in 0..2 {
                    if let Some(c) = curves.as_deref_mut() {
                        c.pop();
                    }
                    if let Some(t) = types.as_deref_mut() {
                        t.pop();
                    }
                }

                // add in one translated and trimmed copy of the bottom GenCurve
                let which = offset_curve
                    .first()
                    .and_then(|r| r.first())
                    .copied()
                    .flatten()
                    .unwrap_or(Which::This);
                let len = offset_length
                    .first()
                    .and_then(|r| r.first())
                    .copied()
                    .flatten()
                    .unwrap_or(0.0);
                let tgt_index = usize::from(which == Which::Other);
                let mut trim = Extent1d::from_min_max(
                    sol.start.parameters.get(tgt_index).copied().unwrap_or(0.0),
                    sol.end.parameters.get(tgt_index).copied().unwrap_or(0.0),
                );
                let copied = translated_trimmed(
                    k,
                    if which == Which::Other {
                        other_gen
                    } else {
                        ext_gen
                    },
                    len * if tgt_index == 0 {
                        extrusion_vec
                    } else {
                        other_vec
                    },
                    &mut trim,
                )?;
                if let Some(c) = curves.as_deref_mut() {
                    c.push(copied);
                }
                if let Some(t) = types.as_deref_mut() {
                    t.push(TsectCurveType::RegionBoundary);
                }
            }
            // else - the situation is too complicated, free any results and ask for a
            // full surf/surf intersection
            else {
                if let Some(c) = curves.as_deref_mut() {
                    c.clear();
                }
                if let Some(t) = types.as_deref_mut() {
                    t.clear();
                }
                result.needs_more_intersections = true;
                return Ok(result);
            }
        }
    }

    Ok(result)
}

/// A copy of `curve` translated by `shift` and trimmed to `trim` (`Copy`,
/// `Transform`, `Trim`; the last two statuses ignored).
fn translated_trimmed(
    k: &Kernel,
    curve: &Curve,
    shift: Vec3,
    trim: &mut Extent1d,
) -> Result<Curve> {
    let mut shift_rf = Axis2Placement::IDENTITY;
    shift_rf.translate(shift);
    let mut copied = curve.clone();
    ignore_status(copied.transform(k, &shift_rf, None))?;
    ignore_status(copied.trim(k, trim, TrimMode::Notify))?; // may snap the interval
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::crv_basic::Curve as NurbCurve;
    use crate::nurbs::math::{CPoint, NOW};

    fn k() -> Kernel {
        Kernel::default()
    }

    #[test]
    fn lazy_transform_matches_materialized_transform() {
        let k = k();
        let mut lazy = SurfOfExtrusion::new(
            &k,
            arc_gen(),
            Point3::ZERO,
            Vec3::new(0.0, 0.0, 2.0),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            false,
        )
        .unwrap();
        assert!(lazy.base().gw_nurb().is_none());
        lazy.transform(&k, &Axis2Placement::IDENTITY, None).unwrap();
        assert!(lazy.base().gw_nurb().is_none());
        assert!(lazy
            .transform(
                &k,
                &Axis2Placement::IDENTITY,
                Some(Vec3::new(2.0, 3.0, 2.0))
            )
            .is_err());
        assert!(lazy.base().gw_nurb().is_none());

        let mut ready = lazy.clone_exact();
        ready.make_nurb(&k).unwrap();
        let mut transform = Axis2Placement::IDENTITY;
        transform.translate(Vec3::new(3.0, -2.0, 1.0));
        lazy.transform(&k, &transform, None).unwrap();
        ready.transform(&k, &transform, None).unwrap();
        assert!(lazy.base().gw_nurb().is_some());
        assert_eq!(lazy.origin(), ready.origin());
        for uv in [
            Point2::new(0.0, 0.0),
            Point2::new(0.25, 0.75),
            Point2::new(1.0, 1.0),
        ] {
            assert_eq!(
                lazy.base().evaluate_point(uv).unwrap(),
                ready.base().evaluate_point(uv).unwrap()
            );
        }
    }

    #[test]
    fn kept_made_nurb_is_bit_identical_to_a_fresh_one() {
        use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
        use crate::surface::surface::{Surface, SurfaceKind};
        let k = k();
        let lazy = SurfOfExtrusion::new(
            &k,
            arc_gen(),
            Point3::ZERO,
            Vec3::new(0.0, 0.0, 2.0),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            false,
        )
        .unwrap();
        assert!(lazy.base().gw_nurb().is_none());
        let surf = Surface::from(lazy);
        // a lazy exact copy: makes its own NURB afresh
        let fresh = |surf: &Surface| {
            let SurfaceKind::SurfOfExtrusion(e) = surf.kind() else {
                panic!("an extrusion");
            };
            let mut c = e.clone_exact();
            c.base.nurb.take();
            Surface::from(c)
        };
        let eval = |surf: &Surface, uv: Point2| -> Vec<u64> {
            let mut out = Vec::new();
            let p = surf.evaluate_point(&k, uv).unwrap();
            out.extend([p.x, p.y, p.z].map(f64::to_bits));
            let flags = EvaluateFlags::default();
            for row in surf.evaluate(&k, uv, 2, 2, flags).unwrap() {
                for v in row {
                    out.extend([v.x, v.y, v.z].map(f64::to_bits));
                }
            }
            out
        };
        let uvs = [
            Point2::new(0.0, 0.0),
            Point2::new(0.25, 0.75),
            Point2::new(0.6, 0.1),
            Point2::new(1.0, 1.0),
        ];
        for round in 0..2 {
            for &uv in &uvs {
                assert_eq!(
                    eval(&surf, uv),
                    eval(&fresh(&surf), uv),
                    "round {round} at {uv:?}"
                );
            }
        }
        // the made NURB is kept on the extrusion itself (no side memo), as upstream
        // keeps it on the object
        assert!(!surf.has_made_nurb());
        let SurfaceKind::SurfOfExtrusion(e) = surf.kind() else {
            panic!("an extrusion");
        };
        assert!(e.base().gw_nurb().is_some(), "the made NURB is kept");
        // a copy carries it (the copy constructor copies the source's NURB)
        let SurfaceKind::SurfOfExtrusion(c) = surf.clone().into_kind() else {
            panic!("an extrusion");
        };
        assert!(c.base().gw_nurb().is_some());
    }

    /// A planar quadratic B-spline in the z = 0 plane.
    fn arc_gen() -> Curve {
        let pw = vec![
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(1.0, 1.0, 0.0, NOW),
            CPoint::new(2.0, 0.0, 0.0, NOW),
        ];
        let cur = NurbCurve::new(pw, 2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).unwrap();
        Curve::from(BSplineCurve::from_nurb(cur, 3, false))
    }

    #[test]
    fn canonical_extrusion_makes_a_lazy_line_nurb() {
        let k = k();
        let line = Line::from_points(Vec3::ZERO, Vec3::new(1.0, 0.0, 1.0), 3);
        assert!(line.base().gw_nurb().is_none());
        let mut s = SurfOfExtrusion::create_canonical(&k, Curve::from(line), Vec3::Z).unwrap();
        assert!(s.base().gw_nurb().is_some());
        assert!(gen_base(s.gen().unwrap()).unwrap().gw_nurb().is_some());
        s.adjust_step_uv_domain(&k, &Extent2d::from_bounds(0.0, -2.0, 1.0, 3.0))
            .unwrap();
        let p = s.evaluate_step_point(&k, Point2::new(0.25, 0.5)).unwrap();
        assert!((p - Vec3::new(0.25, 0.0, 0.75)).length() < 1e-12);

        let line = Line::from_points(Vec3::ZERO, Vec3::new(1.0, 0.0, 1.0), 3);
        let s = SurfOfExtrusion::new(
            &k,
            Curve::from(line),
            Vec3::ZERO,
            Vec3::Z,
            Extent2d::from_bounds(-0.5, -2.0, 2.0, 3.0),
            true,
        )
        .unwrap();
        assert!(s.base().gw_nurb().is_some());
        assert!((s.curve_point - Vec3::new(-0.5, 0.0, -0.5)).length() < 1e-12);
    }

    #[test]
    fn canonical_perpendicular_extrusion() {
        let k = k();
        let s = SurfOfExtrusion::create_canonical(&k, arc_gen(), Vec3::new(0.0, 0.0, 2.0)).unwrap();
        assert_eq!(s.orientation(), CurveOrientType::Perpendicular);
        assert!(s.is_kind_of(SurfOfExtrusion::TYPE).unwrap());
        assert_eq!(s.type_string(), "SmSurfOfExtrusion");
        let d = s.step_uv_domain();
        assert_eq!(d.min().y, -INFINITE_PARAMETER);
        let p = s.evaluate_step_point(&k, Point2::new(0.5, 1.0)).unwrap();
        assert!((p.z - 2.0).abs() < 1e-12);
        let ders = s.evaluate_step(&k, Point2::new(0.5, 1.0), 2, 2).unwrap();
        assert_eq!(ders.len(), 9);
        assert_eq!(ders[1], Vec3::new(0.0, 0.0, 2.0));
        assert!(s.evaluate_step(&k, Point2::new(0.5, 1.0), 6, 6).is_err());
        // order 5 of a B-spline generator: upstream overruns NL_MAXDER (UB-80)
        assert!(s.evaluate_step(&k, Point2::new(0.5, 1.0), 5, 5).is_err());
    }

    #[test]
    fn conversions_round_trip() {
        let k = k();
        let mut s = SurfOfExtrusion::new(
            &k,
            arc_gen(),
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 1.0),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 3.0),
            true,
        )
        .unwrap();
        s.make_nurb(&k).unwrap();
        assert_eq!(s.orientation(), CurveOrientType::Independent);
        let n = s
            .convert_uv_from_step_to_nurbs(&k, Point2::new(0.25, 1.5))
            .unwrap();
        let back = s.convert_uv_from_nurbs_to_step(&k, n).unwrap();
        assert!((back.x - 0.25).abs() < 1e-12 && (back.y - 1.5).abs() < 1e-12);
        assert!(s.approx_eq(&k, &s.copy().unwrap()).unwrap());
    }

    #[test]
    fn conversions_need_the_nurb() {
        let k = Kernel::default();
        let mut s = SurfOfExtrusion::new(
            &k,
            arc_gen(),
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 1.0),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 3.0),
            false,
        )
        .unwrap();
        assert!(s
            .convert_uv_from_step_to_nurbs(&k, Point2::new(0.25, 1.5))
            .is_err());
        assert!(s
            .convert_uv_from_nurbs_to_step(&k, Point2::new(0.25, 1.5))
            .is_err());
        s.make_nurb(&k).unwrap();
        assert!(s
            .convert_uv_from_step_to_nurbs(&k, Point2::new(0.25, 1.5))
            .is_ok());
    }

    #[test]
    fn empty_surface_has_no_generator() {
        let s = SurfOfExtrusion::empty();
        assert!(s.copy().is_err());
        assert!(s.max_analytic_domain(&k()).is_err());
    }

    fn full_circle(k: &Kernel) -> Curve {
        use crate::curve::ellipse::{Ellipse, EllipseOptions};
        let e = Ellipse::new_circle(
            k,
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Extent1d::from_min_max(0.0, 360.0),
            1.0,
            &EllipseOptions::default(),
        )
        .unwrap();
        Curve::from(e)
    }

    /// A closed planar B-spline (degree 1 square) in the z = 0 plane.
    fn closed_gen() -> Curve {
        let pw = vec![
            CPoint::new(1.0, 0.0, 0.0, NOW),
            CPoint::new(0.0, 1.0, 0.0, NOW),
            CPoint::new(-1.0, 0.0, 0.0, NOW),
            CPoint::new(0.0, -1.0, 0.0, NOW),
            CPoint::new(1.0, 0.0, 0.0, NOW),
        ];
        let cur = NurbCurve::new(pw, 1, vec![0.0, 0.0, 0.25, 0.5, 0.75, 1.0, 1.0]).unwrap();
        Curve::from(BSplineCurve::from_nurb(cur, 3, false))
    }

    #[test]
    fn seam_inversion_follows_the_guess() {
        let k = k();
        for (gen, seam) in [(full_circle(&k), false), (closed_gen(), true)] {
            let s = SurfOfExtrusion::create_canonical(&k, gen, Vec3::new(0.0, 0.0, 2.0)).unwrap();
            assert_eq!(s.orientation(), CurveOrientType::Perpendicular);
            let dom = s.step_uv_domain();
            let p = Point3::new(1.0, 0.0, 0.5);
            let (uv, loc) = s.step_inversion(&k, &dom, p, 1e-6, None).unwrap();
            let want = if seam {
                LocationType::USeam
            } else {
                LocationType::Interior
            };
            assert_eq!(loc, want);
            assert!((uv.y - 0.25).abs() < 1e-9);
            let (uv, _) = s
                .step_inversion(&k, &dom, p, 1e-6, Some(Point2::new(300.0, 0.0)))
                .unwrap();
            if seam {
                assert_eq!(uv.x, dom.max().x);
            }
            assert!(s
                .step_inversion(&k, &dom, Point3::new(2.0, 0.0, 0.5), 1e-6, None)
                .is_err());
        }
    }

    #[test]
    fn directrix_spans_the_extrusion() {
        let k = k();
        let mut s = SurfOfExtrusion::new(
            &k,
            arc_gen(),
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Extent2d::from_bounds(0.0, -1.0, 1.0, 2.0),
            false,
        )
        .unwrap();
        s.make_nurb(&k).unwrap();
        let d = s
            .create_directrix_from_point(&k, Point3::new(1.0, 0.5, 0.25), 1e-6)
            .unwrap();
        let a = d.evaluate_point(d.natural_interval().min()).unwrap();
        let b = d.evaluate_point(d.natural_interval().max()).unwrap();
        assert!((a.z + 1.0).abs() < 1e-12 && (b.z - 2.0).abs() < 1e-12);
        // a non-isotropic scale is refused, an isotropic one scales the vector
        assert!(s
            .transform(
                &k,
                &Axis2Placement::IDENTITY,
                Some(Vec3::new(1.0, 2.0, 1.0))
            )
            .is_err());
        s.transform(
            &k,
            &Axis2Placement::IDENTITY,
            Some(Vec3::new(2.0, 2.0, 2.0)),
        )
        .unwrap();
        assert_eq!(s.extrusion_vector(), Vec3::new(0.0, 0.0, 2.0));
    }

    /// A closed tangent-continuous cubic in the z = 0 plane (periodic).
    fn periodic_gen() -> Curve {
        let pw = vec![
            CPoint::new(1.0, 0.0, 0.0, NOW),
            CPoint::new(1.0, 1.0, 0.0, NOW),
            CPoint::new(-1.0, 1.0, 0.0, NOW),
            CPoint::new(-1.0, -1.0, 0.0, NOW),
            CPoint::new(1.0, -1.0, 0.0, NOW),
            CPoint::new(1.0, 0.0, 0.0, NOW),
        ];
        let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0, 1.0, 1.0];
        let cur = NurbCurve::new(pw, 3, knots).unwrap();
        Curve::from(BSplineCurve::from_nurb(cur, 3, false))
    }

    /// `GetDegree` makes a lazy extrusion's NURB first: a straight generator extruded is
    /// bilinear, so a long step on it crosses no seam (`sm_CrossedSeamOrPole`), and it is
    /// no analytic-low-degree surface for the cache tolerances unless its NURB says so.
    #[test]
    fn lazy_extrusion_degree_callers_see_the_made_nurb() {
        use crate::surface::surface::p1::crossed_seam_or_pole;
        use crate::surface::surface::Surface;
        let k = k();
        let pw = vec![
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(2.0, 1.0, 0.0, NOW),
        ];
        let line = NurbCurve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        let gen = Curve::from(BSplineCurve::from_nurb(line, 1, false));
        let lazy = SurfOfExtrusion::new(
            &k,
            gen,
            Point3::ZERO,
            Vec3::new(0.0, 0.0, 2.0),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            false,
        )
        .unwrap();
        assert!(lazy.base().gw_nurb().is_none());
        let surf = Surface::from(lazy);
        assert_eq!(
            surf.degree(SurfParamType::U),
            0,
            "the stored base has no NURB"
        );
        assert_eq!(surf.degree_with_nurb(&k, SurfParamType::U).unwrap(), 1);
        assert_eq!(surf.degree_with_nurb(&k, SurfParamType::V).unwrap(), 1);
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let crossed = crossed_seam_or_pole(
            &k,
            &surf,
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 1.0),
            &dom,
        )
        .unwrap();
        assert!(!crossed, "bilinear: never a seam");
    }

    /// `GetKnots` makes a lazy extrusion's NURB first, also through a STEP surface's
    /// base `GetDegree`: `knots_with_nurb` answers the made NURB's knots.
    #[test]
    fn lazy_extrusion_knots_read_the_made_nurb() {
        use crate::surface::surface::{StepSurface, Surface};
        let k = k();
        let lazy = SurfOfExtrusion::new(
            &k,
            arc_gen(),
            Point3::ZERO,
            Vec3::new(0.0, 0.0, 2.0),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            false,
        )
        .unwrap();
        assert!(lazy.base().gw_nurb().is_none());
        let made = lazy.clone_exact().base_with_nurb(&k).unwrap().into_owned();
        let surf = Surface::from(lazy.clone_exact());
        for param in [SurfParamType::U, SurfParamType::V] {
            let want = made.knots(param, None).unwrap();
            if param == SurfParamType::U {
                assert!(surf.knots(param, None).is_err(), "the stored base has none");
            }
            assert_eq!(surf.knots_with_nurb(&k, param, None).unwrap(), want);
            // the made NURB is kept: the stored base now answers it too
            assert_eq!(surf.knots(param, None).ok(), Some(want));
        }
        // a STEP surface's knots: its domain ends, each `degree + 1` times
        let step = Surface::from(StepSurface::new(Surface::from(lazy), true).unwrap());
        let (_, mults) = step.knots_with_nurb(&k, SurfParamType::U, None).unwrap();
        let degree = made.degree(SurfParamType::U);
        assert!(degree > 0);
        assert!(mults.iter().all(|&m| m == degree + 1), "{mults:?}");
    }

    /// The virtual `GetKnots` of `SurfaceMaxDistanceBetween` (through `SurfaceEvaluate`),
    /// `SnapToKnots` and `GetDomainWithContinuity` make a lazy extrusion's NURB first
    /// instead of failing on the NURB-less base.
    #[test]
    fn lazy_extrusion_knot_callers_make_the_nurb() {
        use crate::geom::geom_utility::p2::ContinuityType;
        use crate::math::Vec2;
        use crate::surface::surface::p2::SurfaceEvaluate;
        use crate::surface::surface::Surface;
        let k = k();
        let lazy = SurfOfExtrusion::new(
            &k,
            arc_gen(),
            Point3::ZERO,
            Vec3::new(0.0, 0.0, 2.0),
            Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
            false,
        )
        .unwrap();
        assert!(lazy.base().gw_nurb().is_none());
        let made = lazy.clone_exact().base_with_nurb(&k).unwrap().into_owned();
        let surf = Surface::from(lazy.clone_exact());
        assert!(
            surf.knots(SurfParamType::U, None).is_err(),
            "the stored base has none"
        );
        for param in [SurfParamType::U, SurfParamType::V] {
            let want = made.knots(param, None).unwrap();
            assert_eq!(
                SurfaceEvaluate::knots(&surf, &k, param, None).unwrap(),
                want
            );
        }
        let (u_knots, _) = made.knots(SurfParamType::U, None).unwrap();
        let first = u_knots[0];
        let snapped = surf
            .snap_to_knots(&k, Point2::new(first + 1e-12, 0.5), 1e-9)
            .unwrap();
        assert_eq!(snapped.x, first);
        let dom = surf
            .domain_with_continuity(
                &k,
                ContinuityType::G1,
                Point2::new(0.5, 0.5),
                Vec2::new(1.0, 0.0),
            )
            .unwrap();
        assert!(dom.min().x <= 0.5 && dom.max().x >= 0.5, "{dom:?}");
    }

    #[test]
    fn seam_drop_keeps_the_inward_end() {
        let k = k();
        let s = SurfOfExtrusion::create_canonical(&k, periodic_gen(), Vec3::new(0.0, 0.0, 2.0))
            .unwrap();
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let drop = |v: Option<Vec3>| {
            s.drop_point_fast::<CurveHandle>(
                &k,
                &dom,
                SolverOperation::Intersect,
                Point3::new(1.0, 0.0, 0.2),
                v,
                1e-6,
                None,
                SolutionRequested::All,
                None,
            )
            .unwrap()
        };
        let all = drop(None);
        assert_eq!(all.len(), 2);
        let ins = drop(Some(Vec3::new(0.0, 1.0, 0.0)));
        let outs = drop(Some(Vec3::new(0.0, -1.0, 0.0)));
        assert_eq!(ins.len(), 1);
        assert_eq!(outs.len(), 1);
        assert_eq!(ins.as_slice()[0].start.parameters[0], 0.0);
        assert_eq!(outs.as_slice()[0].start.parameters[0], 1.0);
    }
}
