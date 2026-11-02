//! Spheres (`SmSphere`): a surface of revolution sweeping a circular generator (a
//! meridian from pole to pole) about the axis of its placement, with the STEP
//! evaluation `S(u, v) = O + r (cos v (cos u X + sin u Y) + sin v Z)` (angles in
//! degrees), the conversions between the STEP and NURB parameters, the circular iso
//! curves, the offset sphere, the intersections with planes, spheres and cones, the
//! recognition of a sphere in a B-spline surface and the rebuild of the analytic data
//! from the NURB.
//!
//! Upstream: `source/SMLib/src/SmSphere.cpp`, `source/SMLib/inc/SmSphere.h`.
//!
//! | Rust                                         | Upstream                                     |
//! |----------------------------------------------|----------------------------------------------|
//! | [`Sphere`], [`Sphere::new`], [`SphereOptions`] | `SmSphere`, `SmSphere(crOrigin, crXAxis, crYAxis, crAnalUVDomain, dRadius, ..)` |
//! | [`Sphere::empty`]                            | `SmSphere()`                                 |
//! | `Clone`, [`Sphere::copy`]                    | `SmSphere(const SmSphere&)`, `Copy`          |
//! | `Drop` (implicit)                            | `~SmSphere`                                  |
//! | [`Sphere::approx_eq`]                        | `operator==`                                 |
//! | [`Sphere::create_canonical`]                 | `CreateCanonical`                            |
//! | [`Sphere::make_nurb`]                        | `SmSurfOfRevolution::MakeNurb` on a sphere (virtual iso curves) |
//! | [`Sphere::reparameterize`]                   | `SmSurfOfRevolution::Reparameterize` on a sphere (virtual iso curves) |
//! | [`Sphere::adjust_step_uv_domain`]            | `SmSurfOfRevolution::AdjustSTEPUVDomain` on a sphere (virtual iso curves) |
//! | [`Sphere::trim_with_domain`]                | `SmSurfOfRevolution::TrimWithDomain` on a sphere (virtual conversion and iso curves) |
//! | [`Sphere::global_surface_intersect_with`]     | `GlobalSurfaceIntersect` with the holding `SmSurface` |
//! | [`Sphere::create_offset_surface`]            | `CreateOffsetSurface`                        |
//! | [`Sphere::create_iso_parametric_curve`]      | `CreateIsoParametricCurve`                   |
//! | [`Sphere::convert_uv_from_step_to_nurbs`]    | `ConvertUVFromSTEPToNURBS`                   |
//! | [`Sphere::convert_uv_from_nurbs_to_step`]    | `ConvertUVFromNURBSToSTEP`                   |
//! | [`Sphere::evaluate_step`], [`Sphere::evaluate_step_point`] | `EvaluateSTEP`, `EvaluateSTEPPoint` |
//! | [`Sphere::evaluate_normal`]                  | `EvaluateNormal`                             |
//! | [`Sphere::step_uv_domain`]                   | `GetSTEPUVDomain`                            |
//! | [`Sphere::global_surface_intersect`]         | `GlobalSurfaceIntersect`                     |
//! | [`Sphere::intersect_with_plane`]             | `IntersectWithPlane`                         |
//! | [`Sphere::intersect_with_sphere`]            | `IntersectWithSphere`                        |
//! | [`Sphere::intersect_with_cone`]              | `IntersectWithCone`                          |
//! | [`Sphere::create_trimmed_circle_segments`]   | `CreateTrimmedCircleSegments`                |
//! | [`Sphere::is_nurb_surface_sphere`]           | `IsNurbSurfaceSphere`                        |
//! | [`Sphere::rebuild_step_from_nurb_parameters`] | `RebuildSTEPFromNURBParameters`             |
//! | [`Sphere::is_closed`], [`Sphere::is_closed_u`], [`Sphere::is_closed_v`] | `IsClosed`, `IsClosedU`, `IsClosedV` |
//! | [`Sphere::step_inversion`]                   | `STEPInversion`                              |
//! | [`Sphere::transform`]                        | `Transform`                                  |
//! | [`Sphere::read_from_db`]                     | `ReadFromDB`                                 |
//! | [`Sphere::max_analytic_domain`]              | `GetMaxAnalyticDomain`                       |
//! | [`Sphere::is_kind_of`], [`Sphere::TYPE`], [`Sphere::type_code`], [`Sphere::type_string`], [`Sphere::class_string`] | `IsKindOf`, `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`Sphere::radius`], [`Sphere::inside_out`]   | `GetRadius`, `GetInsideOut` (inline)         |
//!
//! Deviations:
//! - **Derived class.** `SmSphere` derives from `SmSurfOfRevolution`; the base object
//!   is the [`Sphere::base`] field ([`SurfOfRevolution`]). A [`Surface`] holds it as
//!   [`SurfaceKind::Sphere`], whose dispatch runs the sphere's overrides and the
//!   inherited `SmSurfOfRevolution` / `SmBSplineSurface` bodies on its base. Upstream's
//!   virtual `CreateIsoParametricCurve`, called by the base class's NURB and
//!   polar-converter rebuilds, reaches the sphere override through
//!   [`Sphere::make_nurb`], [`Sphere::reparameterize`],
//!   [`Sphere::adjust_step_uv_domain`] and the private `rebuild_polar_converter`. The sphere
//!   implements [`Revolved`] so the surface of revolution's `IntersectWithSphere` can
//!   take it as its operand.
//! - **Arguments.** Out-params become return values; `SmContext` arguments, the
//!   arguments upstream marks `NotUsed` (`EvaluateSTEP`'s side flags,
//!   `CreateIsoParametricCurve`'s approximation tolerance, `IsClosed`'s tolerance,
//!   `ReadFromDB`'s type) and the `Notify` call of `IsNurbSurfaceSphere` are dropped.
//!   `CreateIsoParametricCurve`'s optional UV iso line is forwarded to the B-spline
//!   surface. The
//!   intersections return [`SurfaceIntersection`] (the null UV curves upstream appends
//!   are `None`) and whether the general intersector is needed; an intersection with
//!   `pOpt3DCurves == NULL` (`SM_ERR` upstream for the plane and the cone) cannot be
//!   asked for. `EvaluateSTEP` entries it does not write are `0`.
//! - **Copies.** The copy constructor copies the radius and the inside-out flag, so
//!   `Clone` does too.
//! - **Statuses.** Calls whose status upstream ignores keep upstream's values, except
//!   that [`SolidError::Unported`] always propagates and a failure whose output
//!   upstream would read uninitialised (a failed iso-curve extraction or evaluation,
//!   a missing generator, NURB or iso-circle output) is returned. A generator that is
//!   not an ellipse, which upstream casts to `SmCircle`, is `InvalidInput`.
//!   `STEPInversion` returns `SM_ERR` after setting its outputs (a point off the
//!   sphere); that is [`SphereStepInversion::on_surface`] `false`, not an error, so the
//!   outputs stay available to the callers that ignore the status.
//!   `RebuildSTEPFromNURBParameters` returns `FALSE` (`SM_SUCCESS`) when the
//!   constant-u iso curves are not circles; that is `Ok(())` with nothing changed.
//!   `IsNurbSurfaceSphere` returns the status of a failed `SER` call as a `TRUE`
//!   result without sphere; that is the error. The degenerate-curve branches of
//!   `IntersectWithSphere` and `IntersectWithCone` ignore `EvaluatePoint`'s status as
//!   upstream: a failure leaves the default-constructed `SmPoint3d`
//!   ([`Vec3::UNDEF`], assuming the failing call does not write it).
//! - **`GlobalSurfaceIntersect`.** The special cases switch on the other surface's
//!   kind (plane, cone, sphere, surface of revolution); the fallback
//!   `SmSurface::GlobalSurfaceIntersect` runs on the [`Surface`] holding the sphere
//!   ([`Sphere::global_surface_intersect_with`] takes it from the caller;
//!   [`Sphere::global_surface_intersect`] wraps an unowned copy). `DropPointFast`
//!   through [`SurfaceKind::Sphere`] runs the inherited body with the sphere's
//!   `EvaluateSTEPPoint`; [`Sphere::trim_with_domain`] runs the inherited body with
//!   the sphere's `ConvertUVFromNURBSToSTEP` and iso circles, and returns a failed
//!   corner conversion (upstream ignores it and reads the corner it may not have
//!   written).
//! - **Cone intersection.** The cone's iso circle through a point
//!   (`SmCone::CreateIsoCircleFromPoint`) is [`Cone::create_iso_circle_from_point`];
//!   the cone is taken without owner (its zone tolerance finds no face).
//! - **Opt-in fix** ([`UpstreamFixes::sphere_small_circle_not_touching`](crate::kernel::UpstreamFixes::sphere_small_circle_not_touching),
//!   WR-79, #396): `IntersectWithPlane` and `IntersectWithSphere` return a touching
//!   point only when the intersection circle's own radius is within the tolerance, not
//!   when the gap to tangency is, which collapses circles of radius up to about
//!   `sqrt(2 R tol)`. Off by default (upstream behaviour). No input-driven recursion.
//! - **Opt-in fix** ([`UpstreamFixes::found_torus_sphere_keep_tested_nurb`](crate::kernel::UpstreamFixes::found_torus_sphere_keep_tested_nurb),
//!   WR-493, the source fork's source fork@7ea6a96): `IsNurbSurfaceSphere`
//!   tests the half meridian within `180 + 360 * SM_EFF_ZERO` degrees and the sphere it
//!   finds keeps the tested NURB (`SetFromGwNurb`). Off by default (upstream).
//! - **Parent calls.** `Transform` finishes in `SmSurfOfRevolution::Transform` (status
//!   ignored), its polar-converter rebuild given the sphere's iso circles as upstream's
//!   virtual call does; `ReadFromDB` reads the parent record with
//!   [`SurfOfRevolution::read_from_db`] into the sphere's base.

use crate::curve::curve::{Curve, CurveKind};
use crate::curve::ellipse::{Ellipse, EllipseOptions};
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::{
    line_closest_point, line_coplanar_circle_intersect, point_project_to_plane,
    shell_sort_double_array, triangle_decomposition,
};
use crate::geom::geom_utility::p2::{
    sphere_circle_sphere_circle_intersect, ContinuityType, CurveIntersection,
};
use crate::geom::tol::{are_same, are_same_to_tol, is_zero};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{asin, sqrt, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, min2, EFF_ZERO, EFF_ZERO_PARAM, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_surface::{
    BSplineSurface, BoundingBoxRequest, IsoCurveOptions, LocationType, SurfParamType,
};
use crate::nurbs::nurbs_crv::to_euclid;
use crate::nurbs::nurbs_srf::copy_nurb_surface;
use crate::surface::cone::Cone;
use crate::surface::plane::p2::PlaneOwner;
use crate::surface::plane::Plane;
use crate::surface::surf_of_revolution::{
    test_for_iso_circles, RevolutionIsoCurve, Revolved, SurfOfRevolution,
};
use crate::surface::surface::p1::convert_domain_from_nurbs_to_step_with;
use crate::surface::surface::p2::{
    scale_step_angular_derivatives, Surface, SurfaceIntersection, SurfaceKind, TsectCurveType,
    SPHERE_TYPE,
};
use crate::topology::database_io::{DatabaseReader, FileType, InputStream};

const OP_NEW: &str = "surface::sphere::Sphere::new";
const OP_CANONICAL: &str = "surface::sphere::Sphere::create_canonical";
const OP_CONVERT: &str = "surface::sphere::Sphere::convert_uv";
const OP_EVAL_STEP: &str = "surface::sphere::Sphere::evaluate_step";
const OP_REBUILD: &str = "surface::sphere::Sphere::rebuild_step_from_nurb_parameters";
const OP_TRANSFORM: &str = "surface::sphere::Sphere::transform";
const OP_OFFSET: &str = "surface::sphere::Sphere::create_offset_surface";
const OP_NURB_SPHERE: &str = "surface::sphere::Sphere::is_nurb_surface_sphere";
const OP_PLANE: &str = "surface::sphere::Sphere::intersect_with_plane";
const OP_CONE: &str = "surface::sphere::Sphere::intersect_with_cone";
const OP_TRIM: &str = "surface::sphere::Sphere::create_trimmed_circle_segments";

/// `ANALYTIC_TOL_SCALE` (`SmConfig.h`).
const ANALYTIC_TOL_SCALE: f64 = 100.0;

/// The optional constructor arguments of [`Sphere::new`].
#[derive(Debug, Clone, Default)]
pub struct SphereOptions {
    /// `bSwapUV`: the NURB's v is the sweep (longitude) direction.
    pub swap_uv: bool,
    /// `bInsideOut`: the generator runs from the +90 pole to the -90 pole.
    pub inside_out: bool,
    /// `pOptCircleNurb`: the generator (meridian) to use instead of making one; a
    /// curve derived from `SmBSplineCurve`, a circle for the conversions and the
    /// offset.
    pub circle_nurb: Option<Curve>,
}

/// The sphere's own data, captured for the iso curves the base class makes.
#[derive(Debug, Clone, Copy)]
struct Shape {
    radius: f64,
    inside_out: bool,
}

/// The result of [`Sphere::step_inversion`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SphereStepInversion {
    /// `rdAnalUVParameter`: the STEP parameters of the point, degrees about the axis
    /// and degrees of latitude.
    pub uv: Point2,
    /// `reLocation`: [`LocationType::Pole`], [`LocationType::USeam`],
    /// [`LocationType::Interior`] or [`LocationType::Exterior`].
    pub location: LocationType,
    /// The status was `SM_SUCCESS`: the point is within the tolerance of the
    /// surface (`SM_ERR` for a point farther, or at the sphere's centre, where the
    /// outputs are `(0, 0)` and exterior).
    pub on_surface: bool,
}

/// A sphere (`SmSphere`): the surface of revolution of a meridian circle of radius
/// [`Sphere::radius`] about the Z axis of the placement. STEP parameters are the
/// longitude `u` (from the X axis about Z) and the latitude `v` (from the equator
/// towards the Z axis), both in degrees.
#[derive(Debug, Clone)]
pub struct Sphere {
    /// The `SmSurfOfRevolution` base object.
    pub(crate) base: SurfOfRevolution,
    /// `m_dRadius`.
    pub(crate) radius: f64,
    /// `m_bInsideOut`.
    pub(crate) inside_out: bool,
}

/// `is >> value` of a double: stores the extracted value unless the stream's sentry
/// fails.
fn extract(s: &mut InputStream, slot: &mut f64) {
    if let Some(v) = s.extract_double() {
        *slot = v;
    }
}

/// The generator as the `SmCircle` upstream casts it to.
fn gen_circle<'a>(gen: Option<&'a Curve>, op: &'static str) -> Result<&'a Ellipse> {
    match gen.map(Curve::kind) {
        Some(CurveKind::Ellipse(e)) => Ok(e),
        Some(_) => Err(SolidError::invalid_input(op, "generator is not a circle")),
        None => Err(SolidError::null_pointer(op)),
    }
}

/// `SM_ARE_SAME_TO_TOL(len, full, tol)` of the closed tests, `tol` in degrees.
fn closed_tol_deg(radius: f64) -> f64 {
    EFF_ZERO / max2(1.0, radius) * 180.0 / PI
}

impl Sphere {
    /// `SmSphere_TYPE` (`SURF_BASE_TYPE + 14`).
    pub const TYPE: i64 = SPHERE_TYPE;

    // ---- construction ---------------------------------------------------------------

    /// The empty sphere `ReadFromDB` fills (`SmSphere()`): the empty surface of
    /// revolution and an undefined radius.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            base: SurfOfRevolution::empty(),
            radius: UNDEF_DOUBLE,
            inside_out: false,
        }
    }

    /// The sphere centred at `origin` with longitude start `x_axis` and 90 degree
    /// direction `y_axis` over `anal_uv_domain` (longitude by latitude in degrees),
    /// without NURB (constructor). Without `opts.circle_nurb` the generator is the
    /// meridian circle at `origin` in the plane of `x_axis` and `x_axis x y_axis`
    /// over the domain's V interval, inside out with `opts.inside_out`
    /// (`SmCircle::CreateCanonical`); the generator is then not tested for planarity
    /// (it is marked planar).
    ///
    /// # Errors
    ///
    /// Those of the circle (a radius not above `SM_EFF_ZERO`, where upstream
    /// dereferences the null circle) and of [`SurfOfRevolution::new`] for a given
    /// generator.
    pub fn new(
        k: &Kernel,
        origin: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        anal_uv_domain: Extent2d,
        radius: f64,
        opts: SphereOptions,
    ) -> Result<Self> {
        let base = if let Some(gen) = opts.circle_nurb {
            SurfOfRevolution::new(k, gen, origin, x_axis, y_axis, anal_uv_domain, opts.swap_uv)?
        } else {
            // (the base constructor without generator)
            let mut base = SurfOfRevolution::empty();
            base.planar_generator = true;
            base.position = Axis2Placement::new(origin, x_axis, y_axis);
            base.anal_uv_domain = anal_uv_domain;
            base.swap_uv = opts.swap_uv;
            base.base.set_b_spline_surface_form(
                crate::nurbs::b_spline_surface::BSplineSurfaceForm::SurfOfRevolution,
            );

            // build and store generator curve when needed
            let v_ivl = anal_uv_domain.v_interval();
            let circle = Ellipse::create_canonical_circle(
                k,
                &Axis2Placement::new(origin, x_axis, x_axis.cross(y_axis)),
                radius,
                Some(&v_ivl),
                Some(opts.inside_out),
            )
            .map_err(|e| {
                if e.is_unported() {
                    e
                } else {
                    SolidError::null_pointer(OP_NEW)
                }
            })?;
            base.gen_curve = Some(Curve::from(circle));
            base
        };

        // (m_bMakeNurbGenCurve = FALSE: the base's default)
        Ok(Self {
            base,
            radius,
            inside_out: opts.inside_out,
        })
    }

    /// The sphere of `origin` (centre, longitude start and 90 degree directions) and
    /// `radius` over `[0, 360] x [-90, 90]` with its NURB made (`CreateCanonical`). A
    /// negative radius makes the inside-out sphere of its absolute value.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a radius whose absolute value is not above `SM_EFF_ZERO` (or NaN)
    /// and when the NURB could not be made; those of [`Self::new`].
    pub fn create_canonical(k: &Kernel, origin: &Axis2Placement, radius: f64) -> Result<Self> {
        // negative radius -> inside-out sphere of the absolute radius
        let inside_out = radius < 0.0;
        let abs_radius = radius.abs();
        if abs_radius.is_nan() || abs_radius <= EFF_ZERO {
            return Err(SolidError::sm_err(OP_CANONICAL));
        }

        // set analytic domain = [0 to 360, -90 to 90]
        let anal_uv_domain = Extent2d::from_bounds(0.0, -90.0, 360.0, 90.0);

        // construct step parameterized sphere
        let mut sphere = Self::new(
            k,
            origin.origin(),
            origin.x_axis(),
            origin.y_axis(),
            anal_uv_domain,
            abs_radius,
            SphereOptions {
                swap_uv: false,
                inside_out,
                circle_nurb: None,
            },
        )?;

        // Build NURB; fail cleanly rather than leak a null-NURB object.
        match sphere.make_nurb(k) {
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::sm_err(OP_CANONICAL)),
            Ok(()) => {}
        }
        if sphere.base.base.gw_nurb().is_none() {
            return Err(SolidError::sm_err(OP_CANONICAL));
        }
        Ok(sphere)
    }

    /// Makes the NURB (`SmSurfOfRevolution::MakeNurb`), the polar converter rebuilt
    /// from the sphere's iso circles.
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::make_nurb`].
    pub fn make_nurb(&mut self, k: &Kernel) -> Result<()> {
        let shape = self.shape();
        self.base
            .make_nurb_with_sweep_params_using(k, None, &|s, k, p, t| {
                Ok(iso_curve(s, shape, k, p, t, None, false)?.curve)
            })
    }

    /// Maps the NURB domain onto `trim_domain` without changing the shape
    /// (`SmSurfOfRevolution::Reparameterize`, the NURB made first when missing with
    /// the sphere's iso circles) and rebuilds the generator and the polar converter.
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::reparameterize`].
    pub fn reparameterize(&mut self, k: &Kernel, trim_domain: &Extent2d) -> Result<()> {
        // (GetOrCreateGwNurbPointer: the virtual MakeNurb when missing, status ignored)
        if self.base.base.nurb.is_none() {
            ignore_status(self.make_nurb(k))?;
        }
        let shape = self.shape();
        self.base
            .reparameterize_using(k, trim_domain, &|s, k, p, t| {
                Ok(iso_curve(s, shape, k, p, t, None, false)?.curve)
            })
    }

    /// Sets the STEP domain and rebuilds the NURB and the polar converter
    /// (`SmSurfOfRevolution::AdjustSTEPUVDomain` on a sphere: its virtual `MakeNurb`
    /// and `Reparameterize` make the sphere's iso circles).
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::adjust_step_uv_domain`].
    pub fn adjust_step_uv_domain(
        &mut self,
        k: &Kernel,
        new_step_uv_domain: &Extent2d,
    ) -> Result<()> {
        let shape = self.shape();
        self.base
            .adjust_step_uv_domain_using(k, new_step_uv_domain, &|s, k, p, t| {
                Ok(iso_curve(s, shape, k, p, t, None, false)?.curve)
            })
    }

    /// Trims to the NURB domain `new_nurb_uv_domain` (`SmSurfOfRevolution::TrimWithDomain`
    /// on a sphere): the inherited body with the sphere's `ConvertUVFromNURBSToSTEP` and iso
    /// circles as its virtual calls.
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::trim_with_domain`] and of
    /// [`Self::convert_uv_from_nurbs_to_step`] (upstream ignores that status and
    /// reads the corner it may not have written).
    pub fn trim_with_domain(&mut self, k: &Kernel, new_nurb_uv_domain: &Extent2d) -> Result<()> {
        let shape = self.shape();
        self.base.trim_with_domain_using(
            k,
            new_nurb_uv_domain,
            &|s, k, uv| convert_uv_from_nurbs_to_step(s, k, uv),
            &|s, k, p, t| Ok(iso_curve(s, shape, k, p, t, None, false)?.curve),
        )
    }

    /// `RebuildPolarConverter` with the sphere's iso circles.
    fn rebuild_polar_converter(&mut self, k: &Kernel) -> Result<()> {
        let shape = self.shape();
        self.base.rebuild_polar_converter(k, &|s, k, p, t| {
            Ok(iso_curve(s, shape, k, p, t, None, false)?.curve)
        })
    }

    const fn shape(&self) -> Shape {
        Shape {
            radius: self.radius,
            inside_out: self.inside_out,
        }
    }

    /// A copy (`Copy`: the copy constructor, see `Clone`).
    #[must_use]
    pub fn copy(&self) -> Self {
        self.clone()
    }

    // ---- access ---------------------------------------------------------------------

    /// The `SmSurfOfRevolution` base object.
    #[must_use]
    pub const fn base(&self) -> &SurfOfRevolution {
        &self.base
    }

    /// The `SmSurfOfRevolution` base object, mutable.
    pub fn base_mut(&mut self) -> &mut SurfOfRevolution {
        &mut self.base
    }

    /// The radius (`GetRadius`).
    #[must_use]
    pub const fn radius(&self) -> f64 {
        self.radius
    }

    /// Whether the generator runs from the +90 pole to the -90 pole (`GetInsideOut`).
    #[must_use]
    pub const fn inside_out(&self) -> bool {
        self.inside_out
    }

    /// `[0, 360] x [-90, 90]` (`GetMaxAnalyticDomain`).
    #[must_use]
    pub const fn max_analytic_domain(&self) -> Extent2d {
        Extent2d::from_bounds(0.0, -90.0, 360.0, 90.0)
    }

    /// The STEP domain (`GetSTEPUVDomain`).
    #[must_use]
    pub const fn step_uv_domain(&self) -> Extent2d {
        self.base.anal_uv_domain
    }

    /// `SmSphere_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmSphere"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmSphere"
    }

    /// `"SmSphere"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmSphere"
    }

    /// `SmSphere_TYPE` or a type the surface of revolution is (`IsKindOf`).
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::is_kind_of`].
    pub fn is_kind_of(&self, t: i64) -> Result<bool> {
        if t == Self::TYPE {
            return Ok(true);
        }
        self.base.is_kind_of(t)
    }

    // ---- equality -------------------------------------------------------------------

    /// `operator==`: equal B-spline surfaces (`SmBSplineSurface::operator==`), radii
    /// equal within `SM_EFF_ZERO` and the same inside-out flag.
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        // low work
        if core::ptr::eq(self, other) {
            return true;
        }

        // first check the base
        if !self.base.base.approx_eq(&other.base.base) {
            return false;
        }

        // check equivalence of these objects
        is_zero(self.radius - other.radius) && self.inside_out == other.inside_out
    }

    // ---- offset ---------------------------------------------------------------------

    /// Not upstream (`SmSurface::OffsetCollapses`, WR-464): whether the offset radius is
    /// below `tol`, where [`Self::create_offset_surface`] gives no surface.
    #[must_use]
    pub fn offset_collapses(&self, signed_offset_distance: f64, tol: f64) -> bool {
        let sign =
            if self.base.swap_uv { -1.0 } else { 1.0 } * if self.inside_out { -1.0 } else { 1.0 };
        self.radius + sign * signed_offset_distance < tol
    }

    /// The sphere offset by `signed_offset_distance` along the NURB normal
    /// (`CreateOffsetSurface`): a copy with the radius changed by the distance
    /// (negated with [`SurfOfRevolution::swap_uv`] and with [`Self::inside_out`]), the
    /// generator scaled about the sphere's origin and every NURB control point
    /// scaled from it. `None` when the new radius is below `approx_tol_3d`.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without NURB; `InvalidInput` for a generator that is not
    /// a circle (upstream casts it); those of the control points.
    pub fn create_offset_surface(
        &self,
        k: &Kernel,
        signed_offset_distance: f64,
        approx_tol_3d: f64,
    ) -> Result<Option<Self>> {
        // When UVs are swapped - reverse orientation of offset
        // When InsideOut       - reverse orientation of offset
        let offset_sign = 1.0
            * (if self.base.swap_uv { -1.0 } else { 1.0 })
            * (if self.inside_out { -1.0 } else { 1.0 });

        // get new radius
        let old_radius = self.radius;
        let new_radius = self.radius + offset_sign * signed_offset_distance;

        // skip building degenerate or negative radius surfaces
        if new_radius < approx_tol_3d {
            return Ok(None);
        }

        // copy the current sphere
        let mut off = self.clone();

        // change the radius
        off.radius = new_radius;

        // scale the NewSphere->GenCurve about current origin
        let origin = self.base.position.origin();
        if off.base.gen_curve.is_some() {
            let scale = new_radius / old_radius;
            let move_vec = (1.0 - scale) * origin;
            let scale_vec = Vec3::new(scale, scale, scale);
            let mut mv = Axis2Placement::IDENTITY;
            mv.translate(move_vec);
            if let Some(gen) = off.base.gen_curve.as_mut() {
                ignore_status(gen.transform(k, &mv, Some(scale_vec)))?;
            }
        }

        // rebuild the underlying Nurb Surface: just move the Nurb Points
        // (GetOrCreateGwNurbPointer: the virtual MakeNurb when missing)
        if off.base.base.nurb.is_none() {
            ignore_status(off.make_nurb(k))?;
        }
        let off_nurb = off
            .base
            .base
            .nurb
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_OFFSET))?;
        let ix = off_nurb.indices();

        // for every m_pNurb Control Point
        for ii in 0..=ix.n {
            for jj in 0..=ix.m {
                // get Control Point Euclidean position
                let cp = off_nurb.cpt_mut(ii, jj)?;
                let euclid = to_euclid(*cp);

                // move the ControlPoint
                let vec = Vec3::new(
                    euclid.x - origin.x,
                    euclid.y - origin.y,
                    euclid.z - origin.z,
                );
                let scale = old_radius / new_radius;
                cp.x = (origin.x + vec.x / scale) * cp.w;
                cp.y = (origin.y + vec.y / scale) * cp.w;
                cp.z = (origin.z + vec.z / scale) * cp.w;
            }
        }
        Ok(Some(off))
    }

    // ---- parameter conversion -------------------------------------------------------

    /// The NURB parameters of the STEP parameters `step_uv` (`ConvertUVFromSTEPToNURBS`):
    /// the longitude through the polar converter, the latitude through the generator
    /// circle's conversion, swapped with [`SurfOfRevolution::swap_uv`].
    ///
    /// # Errors
    ///
    /// `InvalidInput` without converter circle (upstream dereferences it) or for a
    /// generator that is not a circle; those of the conversions.
    pub fn convert_uv_from_step_to_nurbs(&self, k: &Kernel, step_uv: Point2) -> Result<Point2> {
        // convert sweep coordinate
        let polar_curve = self.base.polar_curve().ok_or(SolidError::invalid_input(
            OP_CONVERT,
            "polar converter has no curve",
        ))?;
        let (mut nurbs_x, _exact) =
            self.base
                .polar
                .convert_to_nurbs_parameter(k, polar_curve, step_uv.x, 0.0)?;

        // (Sphere GenCurves are centred on 0: the PolarParam is the StepParam)
        let polar_param = step_uv.y;

        // convert GenCurve PolarParam to NurbsParam
        let circle = gen_circle(self.base.gen_curve.as_ref(), OP_CONVERT)?;
        let mut nurbs_y = circle.convert_t_from_step_to_nurbs(k, polar_param)?;

        // m_bSwapUV - Nurb and STEP parameters are swapped
        if self.base.swap_uv {
            core::mem::swap(&mut nurbs_x, &mut nurbs_y);
        }
        Ok(Point2::new(nurbs_x, nurbs_y))
    }

    /// The STEP parameters of the NURB parameters `nurbs_uv` (`ConvertUVFromNURBSToSTEP`):
    /// the sweep parameter through the polar converter, the generator parameter
    /// through the generator circle's conversion.
    ///
    /// # Errors
    ///
    /// `InvalidInput` without converter circle or for a generator that is not a
    /// circle; those of the conversions.
    pub fn convert_uv_from_nurbs_to_step(&self, k: &Kernel, nurbs_uv: Point2) -> Result<Point2> {
        convert_uv_from_nurbs_to_step(&self.base, k, nurbs_uv)
    }

    // ---- STEP evaluation ------------------------------------------------------------

    /// The point and triangular derivatives at the STEP parameters `uv` (degrees),
    /// clamped to the STEP domain (`EvaluateSTEP`); derivatives are per degree. The
    /// result is laid out `[D, Dv, .., Du, Duv, .., Duu, ..]` (`(hu + 1) * (hv + 1)`
    /// entries, index `i * (hv + 1) + j`); mixed derivatives above the lower order
    /// are `0`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for orders above 3; `InvalidInput` for unequal orders (upstream asserts
    /// and then indexes as if they were equal).
    pub fn evaluate_step(&self, uv: Point2, hu: usize, hv: usize) -> Result<Vec<Vec3>> {
        // (an order above the limit is refused before the array is read)
        let n = if hu <= 5 && hv <= 5 {
            (hu + 1) * (hv + 1)
        } else {
            0
        };
        let mut d = vec![Vec3::ZERO; n];
        self.evaluate_step_into(uv, hu, hv, &mut d)?;
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
        uv: Point2,
        hu: usize,
        hv: usize,
        d: &mut [Vec3],
    ) -> Result<()> {
        // check input
        if hu > 3 || hv > 3 {
            return Err(SolidError::sm_err(OP_EVAL_STEP));
        }
        if hu != hv {
            return Err(SolidError::invalid_input(
                OP_EVAL_STEP,
                "derivative orders must be equal",
            ));
        }

        // locals
        let mut uv = uv;
        let r_o = self.base.position.origin();
        let r_x = self.base.position.x_axis();
        let r_y = self.base.position.y_axis();
        let s_z = self.base.position.z_axis();
        let d_r = self.radius;
        let min_highest = hu.min(hv);

        // per SMLib practice clamp out of bounds UV points
        if !self.base.is_out_of_bounds_enabled()
            && !self.base.anal_uv_domain.contains_point(uv, 0.0)
        {
            uv = self.base.anal_uv_domain.clamp_point(uv);
        }

        // evaluate expensive trigonometric functions
        let cos_u = (uv.x * PI / 180.0).cos();
        let sin_u = (uv.x * PI / 180.0).sin();
        let cos_v = (uv.y * PI / 180.0).cos();
        let sin_v = (uv.y * PI / 180.0).sin();

        let n = (hu + 1) * (hv + 1);
        let d = d
            .get_mut(..n)
            .ok_or_else(|| SolidError::invalid_input(OP_EVAL_STEP, "derivative array too short"))?;
        let mut set = |i: usize, v: Vec3| {
            if let Some(slot) = d.get_mut(i) {
                *slot = v;
            }
        };

        // evaluate position
        set(
            0,
            r_o + d_r * (cos_v * (cos_u * r_x + sin_u * r_y) + sin_v * s_z),
        );

        // 1st U and V derivatives
        if hu >= 1 {
            set(hv + 1, d_r * cos_v * (-sin_u * r_x + cos_u * r_y));
        }
        if hv >= 1 {
            set(
                1,
                d_r * (-sin_v * (cos_u * r_x + sin_u * r_y) + cos_v * s_z),
            );
        }

        // cross derivative dUV
        if hu >= 1 && hv >= 1 && min_highest >= 2 {
            set(hv + 2, d_r * -sin_v * (-sin_u * r_x + cos_u * r_y));
        }

        //  cross derivative dUVV
        if hu >= 1 && hv >= 2 && min_highest >= 3 {
            set(hv + 3, d_r * -cos_v * (-sin_u * r_x + cos_u * r_y));
        }

        // 2nd U and V derivatives
        if hu >= 2 {
            set(2 * hv + 2, d_r * cos_v * (-cos_u * r_x - sin_u * r_y));
        }
        if hv >= 2 {
            set(
                2,
                d_r * (-cos_v * (cos_u * r_x + sin_u * r_y) - sin_v * s_z),
            );
        }

        // cross derivative dUUV
        if hu >= 2 && hv >= 1 && min_highest >= 3 {
            set(2 * hv + 3, d_r * -sin_v * (-cos_u * r_x - sin_u * r_y));
        }

        // 3rd U and V derivatives
        if hu >= 3 {
            set(3 * hv + 3, d_r * cos_v * (sin_u * r_x - cos_u * r_y));
        }
        if hv >= 3 {
            set(3, d_r * (sin_v * (cos_u * r_x + sin_u * r_y) - cos_v * s_z));
        }

        // The formulas above are per radian; STEP parameters are in degrees.
        scale_step_angular_derivatives(d, hu, hv, true, true)?;
        Ok(())
    }

    /// The point at the STEP parameters `uv` (`EvaluateSTEPPoint`:
    /// [`Self::evaluate_step`] without derivatives).
    ///
    /// # Errors
    ///
    /// Those of [`Self::evaluate_step`].
    pub fn evaluate_step_point(&self, uv: Point2) -> Result<Point3> {
        self.evaluate_step(uv, 0, 0)?
            .first()
            .copied()
            .ok_or(SolidError::invalid_input(OP_EVAL_STEP, "empty evaluation"))
    }

    /// The unit surface normal at the NURB parameters `uv` (`EvaluateNormal`): away
    /// from the origin, reversed when [`SurfOfRevolution::swap_uv`] differs from
    /// [`Self::inside_out`].
    ///
    /// # Errors
    ///
    /// Those of the NURB's point evaluation; `SM_ERR` for a point at the origin.
    pub fn evaluate_normal(&self, uv: Point2) -> Result<Vec3> {
        let surf_pt = self.base.base.evaluate_point(uv)?;
        let normal = surf_pt - self.base.position.origin();

        // Note: could just divide by the radius, but that can lose a bit or two of
        // precision.
        let mut normal = normal.unitize()?;

        if self.base.swap_uv != self.inside_out {
            normal = -normal;
        }
        Ok(normal)
    }

    // ---- iso curves -----------------------------------------------------------------

    /// The iso-parametric curve at the NURB parameter `iso_parameter` in `surf_param`'s
    /// direction (`CreateIsoParametricCurve`): the B-spline extraction of the NURB
    /// (trimmed to `domain`) as a circle (`SmCircle`) with that NURB, a meridian from
    /// pole to pole for constant STEP u and a circle about the axis otherwise, over the
    /// STEP interval or, with `domain`, the angles of the curve's ends. A degenerate
    /// extraction is returned as the B-spline.
    ///
    /// # Errors
    ///
    /// Those of the extraction, the end-point evaluations and the circle.
    pub fn create_iso_parametric_curve(
        &self,
        k: &Kernel,
        surf_param: SurfParamType,
        iso_parameter: f64,
        domain: Option<&Extent2d>,
        want_uv_iso_curve: bool,
    ) -> Result<RevolutionIsoCurve> {
        iso_curve(
            &self.base,
            self.shape(),
            k,
            surf_param,
            iso_parameter,
            domain,
            want_uv_iso_curve,
        )
    }

    // ---- intersections --------------------------------------------------------------

    /// The bounding box of the NURB over `uv_domain` (`CalculateBoundingBox`, the
    /// inherited `SmBSplineSurface` override with its default zone-tolerance
    /// expansion). The NURB has no owner, so the expansion finds no face.
    fn normal_box(&self, k: &Kernel, uv_domain: &Extent2d) -> Result<Extent3d> {
        let req = BoundingBoxRequest {
            normal: true,
            expand_by_zone_tol: true,
            ..BoundingBoxRequest::default()
        };
        self.base
            .base
            .calculate_bounding_box(k, uv_domain, &req)?
            .normal
            .ok_or(SolidError::null_pointer(OP_PLANE))
    }

    /// The intersection curves of this sphere over `uv_domain` with `other` over
    /// `other_uv_domain` (`GlobalSurfaceIntersect`): the special cases of a plane or
    /// a surface of revolution ([`Self::intersect_with_plane`],
    /// [`SurfOfRevolution::intersect_with_sphere`]); where they need more, and for any
    /// other surface, the general intersection is called.
    ///
    /// # Errors
    ///
    /// Those of the special cases; [`SolidError::Unported`] for the general
    /// intersection (see the module docs).
    #[allow(clippy::too_many_arguments)] // upstream's argument list
    pub fn global_surface_intersect(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Surface,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
    ) -> Result<SurfaceIntersection> {
        // (the sphere as the `this` of the general intersection: an unowned copy)
        Surface::from(self.clone()).global_surface_intersect(
            k,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
        )
    }

    /// [`Self::global_surface_intersect`] with the general intersection
    /// (`SmSurface::GlobalSurfaceIntersect` of the [`Surface`] holding this sphere)
    /// supplied by the caller: `general` is called with the kernel when no special
    /// case settles the intersection.
    ///
    /// # Errors
    ///
    /// Those of the special cases and of `general`.
    #[allow(clippy::too_many_arguments)] // upstream's argument list
    pub fn global_surface_intersect_with(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Surface,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
        general: impl FnOnce(&Kernel) -> Result<SurfaceIntersection>,
    ) -> Result<SurfaceIntersection> {
        let mut out = SurfaceIntersection::default();
        self.global_surface_intersect_with_into(
            k,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
            |k, out| {
                *out = general(k)?;
                Ok(())
            },
            &mut out,
        )?;
        Ok(out)
    }

    /// [`Self::global_surface_intersect_with`] into `out`, with `general` filling it.
    /// On an error `out` keeps what the failing special case appended before it, as
    /// upstream leaves its output arrays.
    #[allow(clippy::too_many_arguments)] // upstream's argument list
    pub(crate) fn global_surface_intersect_with_into(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Surface,
        other_uv_domain: &Extent2d,
        _use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        _ang_tol_rad: Option<f64>,
        general: impl FnOnce(&Kernel, &mut SurfaceIntersection) -> Result<()>,
        out: &mut SurfaceIntersection,
    ) -> Result<()> {
        // check for special cases
        let mut need_nurb_intersection = true;
        // (each special case resets the outputs on entry)
        *out = SurfaceIntersection::default();
        match other.kind_with_nurb(k)? {
            SurfaceKind::Plane(p) => {
                let (more, found) =
                    self.intersect_with_plane(k, uv_domain, p, other_uv_domain, approx_tol_3d)?;
                need_nurb_intersection = more;
                *out = found;
            }
            // (an SmCylinder is not SmCone_TYPE: the default branch)
            SurfaceKind::Cone(c) if c.type_code() == crate::surface::surface::p2::CONE_TYPE => {
                need_nurb_intersection = self.intersect_with_cone_into(
                    k,
                    uv_domain,
                    c,
                    other_uv_domain,
                    approx_tol_3d,
                    out,
                )?;
            }
            SurfaceKind::Sphere(o) => {
                need_nurb_intersection = self.intersect_with_sphere_into(
                    k,
                    uv_domain,
                    o,
                    other_uv_domain,
                    approx_tol_3d,
                    out,
                )?;
            }
            SurfaceKind::SurfOfRevolution(o) => {
                // the surface of revolution's first surface is the other one (its UV
                // curves are swapped, also in a partial output)
                let more = o.intersect_with_sphere_into(
                    k,
                    other_uv_domain,
                    self,
                    uv_domain,
                    approx_tol_3d,
                    out,
                );
                core::mem::swap(&mut out.surface1_uv_curves, &mut out.surface2_uv_curves);
                need_nurb_intersection = more?;
            }
            SurfaceKind::BSpline(_)
            | SurfaceKind::Cone(_)
            | SurfaceKind::Torus(_)
            | SurfaceKind::SurfOfExtrusion(_)
            | SurfaceKind::CurveBounded(_)
            | SurfaceKind::Step(_)
            | SurfaceKind::SrfInVolume(_)
            | SurfaceKind::OffsetSurface(_) => {}
        }

        // arrive here with bNeedNurbIntersection == TRUE, then analytic intersections
        // were not found
        if need_nurb_intersection {
            return general(k, out);
        }
        Ok(())
    }

    /// The associated outputs of every 3D curve of `out`: no UV curves, a touching
    /// type for a degenerate curve and a crossing one otherwise, deviation `0`.
    fn associate_outputs(k: &Kernel, out: &mut SurfaceIntersection) -> Result<()> {
        for curve in &out.curves_3d {
            let degenerate = ignored(curve.is_degenerate(k, EFF_ZERO, None))?.unwrap_or(false);
            out.surface1_uv_curves.push(None);
            out.surface2_uv_curves.push(None);
            out.curve_types.push(if degenerate {
                TsectCurveType::Touching
            } else {
                TsectCurveType::Crossing
            });
            out.deviations.push(0.0);
        }
        Ok(())
    }

    /// The intersection of this sphere over `uv_domain` with `plane` over
    /// `other_uv_domain` (`IntersectWithPlane`), and whether the general intersector
    /// is still needed (never: `rbNeedsMoreIntersections` stays `FALSE`). A plane
    /// beyond the radius (within the tolerance) gives nothing, one touching it a
    /// degenerate curve ([`TsectCurveType::Touching`], deviation the gap to the
    /// radius), else the circle trimmed to the sphere's domain and to the plane's
    /// when that is bounded. The tolerance is `approx_tol_3d`, else `SM_EFF_ZERO_SQRT`.
    ///
    /// # Errors
    ///
    /// Those of the bounding boxes, the degenerate curve, the circle trimming and the
    /// plane trim.
    pub fn intersect_with_plane(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        plane: &Plane,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        self.intersect_with_plane_owned(
            k,
            uv_domain,
            plane,
            PlaneOwner::NoFace,
            other_uv_domain,
            approx_tol_3d,
        )
    }

    /// [`Sphere::intersect_with_plane`] with the plane's owner: its box is expanded by
    /// the tolerance of the plane's face (`GetFace()->GetTolerance()`), else by
    /// `SM_ZONE_TOL_3D`.
    ///
    /// # Errors
    ///
    /// Those of [`Sphere::intersect_with_plane`]; [`SolidError::Unported`]
    /// (`SmFace::GetTolerance`) for a plane with a face whose tolerance is not cached.
    pub(crate) fn intersect_with_plane_owned(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        plane: &Plane,
        plane_owner: PlaneOwner,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        // init output
        let mut out = SurfaceIntersection::default();

        // locals
        let plane_normal = plane.position().z_axis();
        let plane_origin = plane.position().origin();
        let sph_origin = self.base.position.origin();
        let tol = approx_tol_3d.unwrap_or(EFF_ZERO_SQRT);

        // get distance from sphereOrigin to Plane
        let projected = point_project_to_plane(sph_origin, plane_origin, plane_normal);
        let dist_to_plane = sph_origin.distance(projected);

        // skip planes that don't intersect sphere
        if dist_to_plane > self.radius + tol {
            return Ok((false, out));
        }

        // Do a little bounding box computation to see if things really intersect.
        if plane.is_bounded() {
            // get min distance from sphereOrigin to planeBoundingBox
            let (bbox, _) = plane.bounding_boxes(k, other_uv_domain, plane_owner)?;
            let sph_point = Extent3d::from_points(sph_origin, sph_origin);
            let dist = bbox.min_distance(&sph_point);

            // skip trimPlanes that don't intersect the sphere
            if dist > self.radius + tol {
                return Ok((false, out));
            }

            // get shpere bounding box
            let mut bbox2 = self.normal_box(k, uv_domain)?;
            bbox2.expand_absolute(tol);

            // skip planeBox/SphereBox cases that don't intersect
            if bbox2.are_disjoint(&bbox, 0.0) {
                return Ok((false, out));
            }
        }

        // See if we are intersecting at a singularity point
        let touching = if k.fixes.sphere_small_circle_not_touching {
            // the circle collapses to a point only when its radius sqrt(R^2 - d^2) is
            // within the tolerance (the fork's source fork@30d25bf)
            let gap = self.radius - dist_to_plane;
            gap <= 0.0 || gap * (self.radius + dist_to_plane) <= tol * tol
        } else {
            dist_to_plane > self.radius - tol
        };
        if touching {
            // build degenerate curve
            let bsc = BSplineCurve::create_degenerate_curve(3, projected)?;

            // set outputs
            out.curves_3d.push(Curve::from(bsc));
            out.surface1_uv_curves.push(None);
            out.surface2_uv_curves.push(None);
            out.curve_types.push(TsectCurveType::Touching);
            out.deviations.push((dist_to_plane - self.radius).abs());

            // all done
            return Ok((false, out));
        }

        // arrive here when plane intersects sphere in a circle

        // build circle center coordinate system from plane's coordinate system
        let x_axis = plane.position().x_axis();
        let y_axis = plane.position().y_axis();

        // build circle arcs trimmed to Sphere boundary
        let circle_ivl = Extent1d::from_min_max(0.0, 360.0);
        let sphere_curves =
            self.create_trimmed_circle_segments(k, projected, x_axis, y_axis, &circle_ivl, tol)?;

        // trim intersection curves to plane
        for curve in sphere_curves {
            if plane.is_bounded() {
                // trim curves to plane domain (status ignored)
                let trimmed =
                    ignored(plane.trim_curve_to_plane_domain(k, other_uv_domain, &curve, tol))?
                        .unwrap_or_default();

                // add trim curves to output
                out.curves_3d.extend(trimmed);
            } else {
                // add whole curve to output
                out.curves_3d.push(curve);
            }
        }

        // set associated output for each trimmed intersection curve
        Self::associate_outputs(k, &mut out)?;

        // all done
        Ok((false, out))
    }

    /// The intersection of this sphere over `uv_domain` with `other` over
    /// `other_uv_domain` (`IntersectWithSphere`), and whether the general intersector
    /// is needed (for coincident spheres). Spheres apart, nested, with disjoint
    /// bounding boxes or concentric give nothing; touching ones a degenerate curve
    /// ([`TsectCurveType::Touching`], deviation the gap of the touch points), else
    /// the circle trimmed to this sphere and to `other`. The tolerance is
    /// `approx_tol_3d`, else `SM_EFF_ZERO_SQRT`.
    ///
    /// # Errors
    ///
    /// Those of the bounding boxes, the triangle decomposition, the circle trimming and
    /// the degenerate curve.
    pub fn intersect_with_sphere(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Self,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        let mut out = SurfaceIntersection::default();
        let more = self.intersect_with_sphere_into(
            k,
            uv_domain,
            other,
            other_uv_domain,
            approx_tol_3d,
            &mut out,
        )?;
        Ok((more, out))
    }

    /// [`Self::intersect_with_sphere`] into `out` (reset first). On an error `out` keeps
    /// what was appended before it, as upstream leaves its output arrays.
    pub(crate) fn intersect_with_sphere_into(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Self,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
        out: &mut SurfaceIntersection,
    ) -> Result<bool> {
        // init output
        *out = SurfaceIntersection::default();

        // select working tolerance
        let tol = approx_tol_3d.unwrap_or(EFF_ZERO_SQRT);

        // locals
        let center = self.base.position.origin();
        let other_center = other.base.position.origin();
        let radius = self.radius;
        let other_radius = other.radius;

        // get center/center gap
        let mut vec = other_center - center;
        let dist = vec.length();

        // Check for no intersections - outside or inside one another
        if dist - tol > radius + other_radius || dist + tol < (radius - other_radius).abs() {
            return Ok(false);
        }

        // check bounding boxes to eliminate candidate intersections
        let bbox1 = self.normal_box(k, uv_domain)?;
        let mut bbox2 = other.normal_box(k, other_uv_domain)?;
        bbox2.expand_absolute(tol);

        // when bounding boxes don't intersect - no intersection
        if bbox1.are_disjoint(&bbox2, 0.0) {
            return Ok(false);
        }

        // check for coincident CenterPoints
        if dist < max2(tol, EFF_ZERO) {
            if (radius - other_radius).abs() < tol {
                // Coincident sphere case. - need 1 sphere surface for every overlapping
                // bit of domain
                return Ok(true);
            }
            // concentric spheres: no intersection case
            return Ok(false);
        }

        // Take care of touching situations
        let (touching, touching_inside) = if k.fixes.sphere_small_circle_not_touching {
            // the circle collapses to a point only when its radius h is within the
            // tolerance: (2 d h)^2 = (d+r1+r2)(r1+r2-d)(d-r1+r2)(d+r1-r2), each factor
            // formed directly (the fork's source fork@30d25bf)
            let out_gap = radius + other_radius - dist;
            let in_gap = dist - (radius - other_radius).abs();
            let height4 = (dist + radius + other_radius)
                * out_gap
                * (dist - radius + other_radius)
                * (dist + radius - other_radius);
            (
                out_gap <= 0.0 || in_gap <= 0.0 || height4 <= 4.0 * tol * tol * dist * dist,
                in_gap < out_gap,
            )
        } else {
            (
                dist + tol > radius + other_radius || dist - tol < (radius - other_radius).abs(),
                dist - tol < (radius - other_radius).abs(),
            )
        };
        if touching {
            let mut sign = 1.0;
            // when touching inside
            if touching_inside {
                // when OtherSphere is larger than thisSphere
                if radius < other_radius {
                    // let Vec run from OtherCenter to ThisCenter
                    vec = -vec;
                }
            } else {
                sign = -1.0;
            }

            // Touching case - return degenerate curve
            let vec = vec.unitize()?;
            let touch_point = center + radius * vec;
            let other_point = other_center + sign * other_radius * vec;
            let distance = touch_point.distance(other_point);
            let bsc = BSplineCurve::create_degenerate_curve(3, touch_point)?;

            // set output
            out.curves_3d.push(Curve::from(bsc));
            out.surface1_uv_curves.push(None);
            out.surface2_uv_curves.push(None);
            out.curve_types.push(TsectCurveType::Touching);
            out.deviations.push(distance);

            // all done
            return Ok(false);
        }

        // arrive here - there may be a circular intersection
        // build and return circle intersection trimmed to spheres

        // get circle size and position relative to 1st Sphere center and gap vector
        let (dist_along_base, _height) = triangle_decomposition(dist, radius, other_radius)?;
        let vec = vec.unitize()?;
        let circle_cent = center + dist_along_base * vec;

        // get orthogonal vectors:
        // let ZAxis = circle normal vector, the vector between sphere centers.
        //     X and Y Axis = any two vectors orthogonal to Z and each other
        let [_z_axis, x_axis, y_axis] = vec.make_unit_ortho_vectors(None)?;

        // build circle arcs trimmed to this Sphere boundary
        let circle_ivl = Extent1d::from_min_max(0.0, 360.0);
        let sphere_curves =
            self.create_trimmed_circle_segments(k, circle_cent, x_axis, y_axis, &circle_ivl, tol)?;

        // trim intersection curves to Other Sphere
        for curve in sphere_curves {
            match curve.kind() {
                // when curve is a circle (the other possibility is a degenerate curve)
                CurveKind::Ellipse(circle) if circle.is_circle() => {
                    // get Curve's analytic interval
                    let step_ivl = circle.step_interval();

                    // make circle segments trimmed to Other Sphere
                    let trimmed = other.create_trimmed_circle_segments(
                        k,
                        circle.position.origin(),
                        circle.position.x_axis(),
                        circle.position.y_axis(),
                        &step_ivl,
                        tol,
                    )?;

                    // add trim curves to output
                    out.curves_3d.extend(trimmed);
                }
                // curve is degenerate
                _ => {
                    // see if degenerateCurvePoint is inside of other surface
                    // (EvaluatePoint's status is ignored: a failure leaves the
                    // default-constructed point)
                    let test_point =
                        ignored(curve.evaluate_point(k, curve.natural_interval().min()))?
                            .unwrap_or(Vec3::UNDEF);
                    let location = if other.is_closed_u() && other.is_closed_v() {
                        // Every point is interior.
                        LocationType::Interior
                    } else {
                        other
                            .step_inversion(&other.base.anal_uv_domain, test_point, EFF_ZERO, None)
                            .location
                    };

                    // when degenerateCurvePoint is inside of other surface
                    if location != LocationType::Exterior {
                        out.curves_3d.push(curve);
                    }
                }
            }
        }

        // set output
        Self::associate_outputs(k, out)?;

        // all done
        Ok(false)
    }

    /// The trimmed circular arcs of the circle through `circle_center` with axes
    /// `circle_x_axis` and `circle_y_axis` over `circle_ivl` (degrees) that lie within
    /// this sphere's STEP domain (`CreateTrimmedCircleSegments`); the circle is the
    /// intersection of the sphere with the plane of the axes, so its radius follows
    /// from the centre. A circle shorter than the tolerance gives a degenerate curve at
    /// its centre point, tangent contacts degenerate curves at the contact points, a
    /// whole circle inside the domain the circle itself.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a centre beyond the sphere (radius squared below
    /// `-(SM_EFF_ZERO * (1 + radius))`); those of the circles and degenerate curves.
    pub fn create_trimmed_circle_segments(
        &self,
        k: &Kernel,
        circle_center: Point3,
        circle_x_axis: Vec3,
        circle_y_axis: Vec3,
        circle_ivl: &Extent1d,
        tol_3d: f64,
    ) -> Result<Vec<Curve>> {
        // init output
        let mut curves: Vec<Curve> = Vec::new();

        // circle locals
        let circle_placement = Axis2Placement::new(circle_center, circle_x_axis, circle_y_axis);
        let circle_norm = circle_x_axis.cross(circle_y_axis);
        let circle_off_vec = circle_center - self.base.position.origin();
        let circle_off_dist2 = circle_off_vec.length_squared();
        let circle_radius2 = self.radius * self.radius - circle_off_dist2;
        let circle_radius = sqrt(circle_radius2);
        let scaled_zero = EFF_ZERO * (1.0 + self.radius);
        let circle_min_deg = circle_ivl.min();
        let circle_max_deg = circle_ivl.max();
        let mut circle_open = false;

        // Sphere locals
        let sphere_origin = self.base.position.origin();
        let sphere_x_axis = self.base.position.x_axis();
        let sphere_y_axis = self.base.position.y_axis();
        let sphere_z_axis = self.base.position.z_axis();
        let sphere_closed_u = self.is_closed_u();
        let sphere_closed_v = self.is_closed_v();
        let anal_domain = self.base.anal_uv_domain;

        // check input
        if circle_radius < -scaled_zero {
            return Err(SolidError::invalid_input(
                OP_TRIM,
                "circle centre is beyond the sphere",
            ));
        }

        // handle degenerate circles
        if circle_radius < scaled_zero {
            // Point is contained when sphere is closed or when Point is contained
            // within the trimmed sphere boundary
            let test_point = circle_center + circle_radius * circle_x_axis;
            let is_contained = if sphere_closed_u && sphere_closed_v {
                true
            } else {
                self.step_inversion(&anal_domain, test_point, EFF_ZERO, None)
                    .location
                    != LocationType::Exterior
            };

            // When TestPoint is in Sphere analUVDomain - the curve is inside
            if is_contained {
                // Add 1 degenerate curve to the output
                let new_curve = BSplineCurve::create_degenerate_curve(3, test_point)?;
                curves.push(Curve::from(new_curve));
            }

            // all done
            return Ok(curves);
        }

        // get UVBoundaries
        let min_u = anal_domain.min().x;
        let max_u = anal_domain.max().x;
        let min_v = anal_domain.min().y;
        let max_v = anal_domain.max().y;

        // local array of circle end and intersection point angles
        let mut ang_degs: Vec<f64> = Vec::new();

        // add endPoint Angles when circle is open
        if (360.0 - circle_ivl.length()) * circle_radius > tol_3d {
            ang_degs.push(circle_min_deg);
            ang_degs.push(circle_max_deg);
            circle_open = true;
        }

        // for every boundary - get angles of all boundary/circle intersections
        for ii in 0..4_usize {
            // skip boundaries in closed sphere directions
            if ((ii == 0 || ii == 1) && sphere_closed_v)
                || ((ii == 2 || ii == 3) && sphere_closed_u)
            {
                continue;
            }

            // get angle (radians) for this boundary
            let ang_deg = match ii {
                0 => max_v,
                1 => min_v,
                2 => max_u,
                _ => min_u,
            };
            let ang_rad = ang_deg * PI / 180.0;

            // get circle boundary arc description
            let (center, x, y) = if ii == 0 || ii == 1 {
                // min/max V boundaries - Varying U isoParam Circles
                (
                    sphere_origin + self.radius * sphere_z_axis * ang_rad.sin(),
                    sphere_x_axis,
                    sphere_y_axis,
                )
            } else {
                // min/max U boundries - Varying V isoParam Circles
                (
                    sphere_origin,
                    ang_rad.cos() * sphere_x_axis + ang_rad.sin() * sphere_y_axis,
                    sphere_z_axis,
                )
            };

            // get SphereCircle/SphereCircle intersections (status ignored: no
            // intersections)
            let hit = match sphere_circle_sphere_circle_intersect(
                sphere_origin,
                self.radius,
                circle_center, // circ1 - circle being trimmed
                circle_norm,   // circ1
                center,        // circ2 - sphere trim boundary
                x.cross(y),    // circ2
                tol_3d,
            ) {
                Ok(h) => h,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => CurveIntersection::Points(Vec::new()),
            };

            // No need to add angles for coincident cases - trim limits will be found
            // with other boundary/circle intersections
            let CurveIntersection::Points(points) = hit else {
                continue;
            };

            // add every intersection point angle to ordered list
            for p in points {
                // get new circle angle for circle point
                let cir_vec = p - circle_center;
                let d_x = cir_vec.dot(circle_x_axis);
                let d_y = cir_vec.dot(circle_y_axis);
                let angle_deg = d_y.atan2(d_x) * 180.0 / PI;

                // accumulate angles
                ang_degs.push(angle_deg);
            }
        }

        // arrive here when circle must be trimmed into 1 or more circular arcs.
        // CircleAngles for all Circle/SphereBoundary intersections are in the array
        // ang_degs.

        // move all angles into given periodic range
        for a in &mut ang_degs {
            if *a < circle_min_deg {
                *a += 360.0;
            } else if *a > circle_max_deg {
                *a -= 360.0;
            }
        }

        // sort the ang_degs values
        shell_sort_double_array(&mut ang_degs);

        // remove duplicate and out of range values from ang_degs
        let at = |v: &[f64], i: usize| v.get(i).copied().unwrap_or(0.0);
        let mut tgt = 0_usize;
        for ii in 0..ang_degs.len() {
            let cur = at(&ang_degs, ii);

            // skip out of bound values
            if cur < circle_min_deg || cur > circle_max_deg {
                continue;
            }

            // skip repeat values, if prev was within range [my_offset_demo iter 13]
            if ii > 0 {
                let prev = at(&ang_degs, ii - 1);
                if prev >= circle_min_deg && are_same(cur, prev) {
                    continue;
                }
            }

            // copy current value into compressed array
            if let Some(slot) = ang_degs.get_mut(tgt) {
                *slot = cur;
            }
            tgt += 1;
        }

        // set compressed ang_degs array size
        ang_degs.truncate(tgt);
        let ang_count = ang_degs.len();

        // Now ang_degs is sorted and has no duplicate angles.

        // If there are no SphereBoundary/Circle intersections and circle is closed,
        // then the entire circle is either in or out; no splitting is necessary.
        // Note, that's also the case if there is exacatly one intersection. [B194]
        if ang_count == 0 || ang_count == 1 {
            // circle is contained for closed spheres or when any point on the circle is
            // contained within the trimmed sphere boundary
            let is_contained = if sphere_closed_u && sphere_closed_v {
                true
            } else {
                // If not closed, need to check whether the whole circle is in or out.
                // Test one point on the circle.
                let mut test_point = circle_center + circle_radius * circle_x_axis;

                // If there is one intersection, then one point will be on the boundary
                // whether the whole circle is inside or out.  Make sure that the point
                // we select is not the inetersection point.
                if ang_count == 1 {
                    // ang_degs[0] is the intersection.
                    let mut ang = at(&ang_degs, 0) + 42.0; // random...
                    ang = deg2rad(ang);
                    test_point = circle_center
                        + circle_radius * (ang.cos() * circle_x_axis + ang.sin() * circle_y_axis);
                }

                // Test the point. Tol loosened [B661]
                self.step_inversion(&anal_domain, test_point, EFF_ZERO_PARAM, None)
                    .location
                    != LocationType::Exterior
            };

            // When TestPoint is in Sphere analUVDomain - the curve is inside
            if is_contained {
                // Add 1 full circle to the output
                let new_circle = Ellipse::create_canonical_circle(
                    k,
                    &circle_placement,
                    circle_radius,
                    None,
                    None,
                )?;
                curves.push(Curve::from(new_circle));
            }

            // all done
            return Ok(curves);
        }

        // for every circle segment - classify in/out and make circular arcs as
        // appropriate
        let mut arc_inside = vec![false; ang_count];
        for ii in 0..ang_count {
            // get arc angle limits
            let end_angle = at(&ang_degs, ii);
            let start_angle = if ii == 0 {
                let last = at(&ang_degs, ang_count - 1);
                if last > end_angle {
                    last - 360.0
                } else {
                    last
                }
            } else {
                at(&ang_degs, ii - 1)
            };

            // when circle is open - the first interval is always False
            if ii == 0 && circle_open {
                continue;
            }

            // arc angle (watch out for arcs that span periodic boundary)
            let arc_angle_deg = if end_angle < start_angle {
                end_angle - start_angle + 360.0
            } else {
                end_angle - start_angle
            };

            // get MidArcPoint
            let mid_angle_rad = (start_angle + arc_angle_deg / 2.0) * PI / 180.0;
            let mid_arc_point = circle_center
                + circle_radius
                    * (mid_angle_rad.cos() * circle_x_axis + mid_angle_rad.sin() * circle_y_axis);

            // when circle arcLength is shorter than tolerance
            if deg2rad(arc_angle_deg) * circle_radius < tol_3d {
                if let Some(slot) = arc_inside.get_mut(ii) {
                    *slot = true;
                }

                // output a degenerate curve at the arcCenter
                let new_bsp = BSplineCurve::create_degenerate_curve(3, mid_arc_point)?;
                curves.push(Curve::from(new_bsp));
            } else {
                // classify nonDegenerate circular arc branch: check arc midPoint to
                // see if arc is inside or outside
                // project the point into Spherical position Coordinates
                let loc = self
                    .step_inversion(&anal_domain, mid_arc_point, tol_3d, None)
                    .location;

                // when test point is on Surface and within domain boundaries
                if loc != LocationType::Exterior {
                    // remember arc is inside
                    if let Some(slot) = arc_inside.get_mut(ii) {
                        *slot = true;
                    }

                    // Add trimmed circular arc to the output
                    let ivl = Extent1d::from_min_max(start_angle, end_angle);
                    let new_circle = Ellipse::create_canonical_circle(
                        k,
                        &circle_placement,
                        circle_radius,
                        Some(&ivl),
                        None,
                    )?;
                    curves.push(Curve::from(new_circle));
                }
                // else remember arc is outside (the default)
            }
        }

        // arrive here - need to add degenerate curves for all tangent intersections

        // for every intersection arcAngle - see if it represents a tangent intersection
        for ii in 0..ang_count {
            // get upper and lower arc inside/outside classifications
            let lower_arc_inside = arc_inside.get(ii).copied().unwrap_or(false);
            let upper_arc_inside = if ii == ang_count - 1 {
                arc_inside.first().copied().unwrap_or(false)
            } else {
                arc_inside.get(ii + 1).copied().unwrap_or(false)
            };

            // skip points attached to inside intervals
            if lower_arc_inside || upper_arc_inside {
                continue;
            }

            // arrive here for all tangent intersection points and tangent/outside
            // OpenCircle endPoints - these last have to be checked

            // get the ArcPoint for this angle
            let arc_ang_rad = at(&ang_degs, ii) * PI / 180.0;
            let arc_point = circle_center
                + circle_radius
                    * (arc_ang_rad.cos() * circle_x_axis + arc_ang_rad.sin() * circle_y_axis);

            // Actually, check all circle-circle intersections.  There will be some
            // outside of the surface patch.
            // get spherical coordinates for ArcPoint
            let loc = self
                .step_inversion(&anal_domain, arc_point, tol_3d, None)
                .location;

            // skip points not within sphere boundaries
            if loc == LocationType::Exterior {
                continue;
            }

            // mark the tangency with a degenerate curve
            let new_bsp = BSplineCurve::create_degenerate_curve(3, arc_point)?;
            curves.push(Curve::from(new_bsp));
        }

        // all done
        Ok(curves)
    }

    /// The intersection of this sphere over `uv_domain` with `cone` over
    /// `other_uv_domain` (`IntersectWithCone`), and whether the general intersector is
    /// needed (when the sphere centre is not on the cone's axis): the circles of the
    /// cone through the points where the cone's straight boundary meets the sphere's
    /// great circle, trimmed to this sphere. A cone farther than the sphere's reach
    /// (or enclosing it) gives nothing. The tolerance is `approx_tol_3d`, else
    /// `SM_EFF_ZERO_SQRT`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the middle of the cone's boundary is on its axis; those of the
    /// bounding boxes, the cone's evaluations and iso circles, the circle line
    /// intersection and the circle trimming.
    pub fn intersect_with_cone(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        cone: &Cone,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        let mut out = SurfaceIntersection::default();
        let more = self.intersect_with_cone_into(
            k,
            uv_domain,
            cone,
            other_uv_domain,
            approx_tol_3d,
            &mut out,
        )?;
        Ok((more, out))
    }

    /// [`Self::intersect_with_cone`] into `out` (reset first). On an error `out` keeps
    /// what was appended before it, as upstream leaves its output arrays.
    pub(crate) fn intersect_with_cone_into(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        cone: &Cone,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
        out: &mut SurfaceIntersection,
    ) -> Result<bool> {
        // init output
        *out = SurfaceIntersection::default();

        // local tolerance
        let tol = approx_tol_3d.unwrap_or(EFF_ZERO_SQRT);

        // The best way to go about solving this problem is to do everything in a plane
        // and then put it back into 3D.
        let sph_cent = self.base.position.origin();
        let cone_pos = cone.rev.position;
        let cone_orig = cone_pos.origin();
        let cone_axis = cone_pos.z_axis();

        // Find the sphere center line/cone axis distance
        let param = line_closest_point(cone_orig, cone_axis, sph_cent);
        let closest_pt = cone_orig + param * cone_axis;
        let dist = closest_pt.distance(sph_cent);

        // Is Sphere too far away for intersetions
        if dist - tol > self.radius + max2(cone.bot_radius(), cone.top_radius()) {
            // Sphere is too far away from cone
            return Ok(false);
        }

        // Is sphere inside of cone and no intersections
        if dist + tol + self.radius < min2(cone.bot_radius(), cone.top_radius()) {
            return Ok(false);
        }

        // If the sphere center is not on the axis of the cone then just use the
        // traditional intersector.
        if dist > tol {
            return Ok(true);
        }

        // eliminate intersections using bounding boxes.
        let bbox1 = self.normal_box(k, uv_domain)?;
        let req = BoundingBoxRequest {
            normal: true,
            expand_by_zone_tol: true,
            ..BoundingBoxRequest::default()
        };
        let mut bbox2 = cone
            .rev
            .base
            .calculate_bounding_box(k, other_uv_domain, &req)?
            .normal
            .ok_or(SolidError::null_pointer(OP_CONE))?;
        bbox2.expand_absolute(tol);
        if bbox1.are_disjoint(&bbox2, 0.0) {
            return Ok(false);
        }

        // arrive here the sphere is on the center of the axis of the cylinder.  We can
        // compute analytical intersections.

        // Get the line segment that runs along a straight-line boundary of the cone.
        // (This will be the seam, if it's closed.)
        // We will intersect that with the sphere.
        let mut uv = other_uv_domain.evaluate(0.0, 0.0); // Lower-left corner of domain.
        let line_start = cone.evaluate_point(k, uv)?;
        // Upper-left corner of analytical domain:
        uv = if cone.rev.swap_uv {
            other_uv_domain.evaluate(1.0, 0.0)
        } else {
            other_uv_domain.evaluate(0.0, 1.0)
        };
        let line_end = cone.evaluate_point(k, uv)?;
        let line_vec = line_end - line_start;

        // To intersect the cone's line segment with the sphere, since the sphere
        // center is on the cone's axis, we can just intersect the line segment with the
        // great circle of the sphere that is in the plane of the axis and the cone's
        // line segment. Its normal would be the cross product of that on-cone line and
        // the axis -- if it's not parallel (cylinder).  In that case, we could use the
        // vector from any point on the axis to any point on the line segment. So let's
        // just use the vector from the origin to the midpoint of that line segment,
        // which cannot be on the axis. [cf. B304 B194]
        let mid_pt = (line_start + line_end) / 2.0;
        let vec = mid_pt - cone_orig;
        let mut circle_norm = vec.cross(cone_axis);
        let length = circle_norm.length();
        if length < EFF_ZERO {
            return Err(SolidError::sm_err(OP_CONE)); // Should never happen.
        }
        circle_norm /= length;

        let line_int = line_coplanar_circle_intersect(
            line_start,
            line_vec,
            sph_cent,
            circle_norm,
            self.radius,
            tol,
        )?;

        // for every ConeLine/SphereCircle intersection
        for t in line_int {
            // intersection point
            let pnt = line_start + t * line_vec;

            // skip intersections outside of cone upper/lower limits
            if t > 1.0 {
                let distance = line_end.distance(sph_cent);
                if (distance - self.radius).abs() > tol {
                    continue;
                }
                // (adLineInt[i] = 1.0: truncate to start of cylinder; the point is
                // already built)
            } else if t < 0.0 {
                let distance = line_start.distance(sph_cent);
                if (distance - self.radius).abs() > tol {
                    continue;
                }
                // (adLineInt[i] = 0.0: truncate to start of cylinder)
            }

            // build cone circle for intersection point
            let Some(curve) = cone.create_iso_circle_from_point(k, pnt, tol)? else {
                continue;
            };
            match curve.kind() {
                CurveKind::Ellipse(circle) if circle.is_circle() => {
                    // get Curve's analytic interval
                    let step_ivl = circle.step_interval();
                    let circle_position = circle.position;

                    // trim circle segment to Sphere
                    let trimmed = self.create_trimmed_circle_segments(
                        k,
                        circle_position.origin(),
                        circle_position.x_axis(),
                        circle_position.y_axis(),
                        &step_ivl,
                        tol,
                    )?;

                    // add trim curves to output
                    out.curves_3d.extend(trimmed);
                }
                // curve is degenerate
                _ => {
                    // (EvaluatePoint's status is ignored: a failure leaves the
                    // default-constructed point)
                    let point = ignored(curve.evaluate_point(k, 0.0))?.unwrap_or(Vec3::UNDEF);
                    let scaled_zero = EFF_ZERO * (1.0 + point.max_dimension());

                    // get UV point for Sphere
                    let location = self
                        .step_inversion(&self.base.anal_uv_domain, point, scaled_zero, None)
                        .location;

                    // add degenerate curve to output
                    if location != LocationType::Exterior {
                        out.curves_3d.push(curve);
                    }
                }
            }
        }

        // set associated output for each trimmed intersection curve
        Self::associate_outputs(k, out)?;

        // all done
        Ok(false)
    }

    // ---- recognition ----------------------------------------------------------------

    /// The sphere with the parameterisation of `test` when `test` is one
    /// (`IsNurbSurfaceSphere`): a rational surface of degree 2 both ways whose
    /// constant-u and constant-v iso curves are circles ([`test_for_iso_circles`] at
    /// `tolerance_scale * ANALYTIC_TOL_SCALE * SM_EFF_ZERO * (1 + |corner points|)`),
    /// those in one direction on parallel planes (not one plane) and those in the
    /// other with a common centre and at most half a turn; its radius above
    /// `SM_EFF_ZERO` and the generator within the poles. The sphere takes the NURB
    /// domain of `test` (through [`Self::reparameterize`]). `None` when `test` is not
    /// a sphere.
    ///
    /// With the opt-in
    /// [`UpstreamFixes::found_torus_sphere_keep_tested_nurb`](crate::kernel::UpstreamFixes::found_torus_sphere_keep_tested_nurb)
    /// (WR-493) the half turn is tested within `180 + 360 * SM_EFF_ZERO` degrees and the
    /// sphere then keeps a copy of `test`'s NURB, its analytic data rebuilt from it
    /// (`SetFromGwNurb`); by default `180 + SM_EFF_ZERO` and the NURB made from the
    /// fitted circles, as upstream.
    ///
    /// # Errors
    ///
    /// Upstream's `SER` statuses (returned as a `TRUE` result without sphere): those of
    /// the evaluations, the iso-circle tests, the iso curve and the circle, and of the
    /// constructor; `InvalidInput` when an iso-circle output upstream would read is
    /// missing.
    pub fn is_nurb_surface_sphere(
        k: &Kernel,
        test: &BSplineSurface,
        tolerance_scale: f64,
    ) -> Result<Option<Self>> {
        // no work - not a degree 2 rational surface in both directions
        if test.degree(SurfParamType::U) != 2
            || test.degree(SurfParamType::V) != 2
            || !test.is_rational()
        {
            return Ok(None);
        }

        // evaluate min/max domain points
        let nurb_uv_domain = test.natural_uv_domain();
        let min_pnt = test.evaluate_point(nurb_uv_domain.min())?;
        let max_pnt = test.evaluate_point(nurb_uv_domain.max())?;

        // scale zero value
        let scaled_zero = tolerance_scale
            * ANALYTIC_TOL_SCALE
            * EFF_ZERO
            * (1.0 + min_pnt.max_dimension() + max_pnt.max_dimension());
        let ang_tol = EFF_ZERO_SQRT * tolerance_scale;

        // test sequence of V Direction (U Constant) isoCurves for circularity and
        // parallelism
        let iso_u = test_for_iso_circles(k, test, SurfParamType::U, scaled_zero, ang_tol)?;
        // quit when not a sphere
        if !iso_u.found_circles {
            return Ok(None);
        }

        // test sequence of U Direction (V Constant) isoCurves for circularity and
        // parallelism
        let iso_v = test_for_iso_circles(k, test, SurfParamType::V, scaled_zero, ang_tol)?;
        // quit when not a sphere
        if !iso_v.found_circles {
            return Ok(None);
        }

        // see if we have a U or a V oriented sphere
        let angles_u = iso_u.angles.unwrap_or(Extent1d::new());
        let angles_v = iso_v.angles.unwrap_or(Extent1d::new());
        // not upstream (WR-493, opt-in; source fork@7ea6a96): the half-meridian
        // tolerance compensates for degrees, as RebuildSTEPFromNURBParameters [B279]
        let half_meridian_tol = if k.fixes.found_torus_sphere_keep_tested_nurb {
            360.0 * EFF_ZERO
        } else {
            EFF_ZERO
        };
        let is_sphere_with_swap_uv = iso_u.parallel_planes
            && !iso_u.same_plane // [ 080625 ]
            && iso_v.coaxial_centers
            && angles_v.length() <= 180.0 + half_meridian_tol;
        let is_sphere_no_swap_uv = iso_v.parallel_planes
            && !iso_v.same_plane // [ 080625 ]
            && iso_u.coaxial_centers
            && angles_u.length() <= 180.0 + half_meridian_tol;

        // quit when not a properly oriented sphere
        if !is_sphere_with_swap_uv && !is_sphere_no_swap_uv {
            return Ok(None);
        }

        // locals
        let nurb_min_uv = nurb_uv_domain.evaluate(0.0, 0.0);
        let missing = || SolidError::invalid_input(OP_NURB_SPHERE, "iso circle output not written");

        // Get sphere position and parameters
        let (swap_uv, ref_iso, other_iso) = if is_sphere_no_swap_uv {
            (false, &iso_v, &iso_u)
        } else {
            (true, &iso_u, &iso_v)
        };
        let ref_frame = ref_iso.reference_frame.ok_or_else(missing)?;
        let other_frame = other_iso.reference_frame.ok_or_else(missing)?;
        let rad = other_iso.radius.ok_or_else(missing)?;
        let sweep_ivl = ref_iso.angles.ok_or_else(missing)?;

        // don't make negative radius spheres
        if rad <= EFF_ZERO {
            return Ok(None);
        }

        // get sphere orientation and size
        let sphere_origin = other_frame.origin(); // the frame where all circles have common center
        let sphere_x_axis = ref_frame.x_axis(); // the fame where all circles are parallel
        let sphere_y_axis = ref_frame.y_axis();
        let sphere_z_axis = ref_frame.z_axis();

        // Check GenCurve rotation direction, its 'y' axis is either +/- sweepCircle z
        // bInsideOut: TRUE = GenCurve starts at +90 Pole and ends at -90 Pole
        //             FALSE= GenCurve starts at -90 Pole and ends at +90 Pole
        // assumes: OtherFrame is valid for 1st Nurb isoparameter GenCurve
        let minor_orientation = other_frame.z_axis().cross(sphere_x_axis);
        let inside_out = sphere_z_axis.dot(minor_orientation) < 0.0;

        // measure genCurve angles from sphere -Z axis
        let vec_min = min_pnt - sphere_origin;
        let vec_max = max_pnt - sphere_origin;

        // get GenCurve min and max angles measured from Sphere's bottom pole in degrees
        let neg_z_axis = -sphere_z_axis;
        let mut angle_min = ignored(neg_z_axis.angle_between(vec_min))?.unwrap_or(0.0);
        let mut angle_max = ignored(neg_z_axis.angle_between(vec_max))?.unwrap_or(0.0);
        angle_min = 180.0 * angle_min / PI;
        angle_max = 180.0 * angle_max / PI;

        // adjust param range from [Umin_to_UMax, 0_to_180] to [Umin_to_UMax, -90_to_90]
        //  acounting for InsideOut reversals
        let adjust_angle = -90.0;
        let mut step_min = Point2::new(sweep_ivl.min(), min2(angle_min, angle_max) + adjust_angle);
        let mut step_max = Point2::new(sweep_ivl.max(), max2(angle_min, angle_max) + adjust_angle);

        // snap to ends for small tolerances - quit for genCurves that extend beyond the
        // poles
        if step_min.y < -90.0 {
            if step_min.y < -90.0 - EFF_ZERO_SQRT {
                return Ok(None);
            }
            step_min.y = -90.0;
        }
        if step_max.y > 90.0 {
            if step_max.y > 90.0 + EFF_ZERO_SQRT {
                return Ok(None);
            }
            step_max.y = 90.0;
        }

        // define Sphere domain
        let anal_uv_domain = Extent2d::from_bounds(step_min.x, step_min.y, step_max.x, step_max.y);

        // Get GenCurve Isoparameter Curve
        let gen_curve_iso = test.create_iso_parametric_curve(
            k,
            if swap_uv {
                SurfParamType::V
            } else {
                SurfParamType::U
            },
            if swap_uv {
                nurb_min_uv.y
            } else {
                nurb_min_uv.x
            },
            IsoCurveOptions::default(),
        )?;

        let circle = Ellipse::new_circle(
            k,
            sphere_origin,
            sphere_x_axis,
            sphere_z_axis,
            anal_uv_domain.v_interval(),
            rad,
            &EllipseOptions {
                dim: 3,
                nurb: Some(&gen_curve_iso.curve),
                inside_out,
            },
        )?;

        // If pCircle's STEP interval does not match the v-interval of sAnalUVDomain,
        // then the Sphere constructor will complain and the resulting bad surface can
        // lead to crashes.  [B414]
        if !circle
            .step_interval()
            .are_equal(&anal_uv_domain.v_interval(), EFF_ZERO)
        {
            // upstream: WARN("Bad result in IsNurbSurfaceSphere(): GenCurve STEP
            // interval does not match sAnalUVDomain VInterval")
            return Ok(None);
        }

        // build a new sphere with given GenCurve to force parameterization
        // compatibility: constant U Curve orientations, constant V Curve centers and
        // radius, and computed domain
        let mut sphere = Self::new(
            k,
            sphere_origin,
            sphere_x_axis,
            sphere_y_axis,
            anal_uv_domain,
            rad,
            SphereOptions {
                swap_uv,
                inside_out,
                circle_nurb: Some(Curve::from(circle)),
            },
        )?;

        // reparameterize the NURB so that AnalyticSurface->NurbDomain ==
        // InputSurface->NurbDomain
        match sphere.reparameterize(k, &test.natural_uv_domain()) {
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(None), // [B414]
            Ok(()) => {}
        }

        // not upstream (WR-493, opt-in; source fork@7ea6a96): the sphere keeps
        // the NURB tested (SetFromGwNurb, whose rebuild's status is ignored)
        if k.fixes.found_torus_sphere_keep_tested_nurb {
            let nurb = test
                .gw_nurb()
                .ok_or_else(|| SolidError::null_pointer(OP_NURB_SPHERE))?;
            sphere.base.base.nurb = Some(copy_nurb_surface(nurb)).into();
            sphere.base.base.nurb_is_borrowed = false;
            ignore_status(sphere.rebuild_step_from_nurb_parameters(k))?;
        }

        // (Notify(SM_NO_COPY) copies the test surface's attributes: not modelled)
        Ok(Some(sphere))
    }

    /// Rebuilds the generator, radius, inside-out flag, placement, swap flag, STEP
    /// domain and polar converter from the NURB (`RebuildSTEPFromNURBParameters`): the
    /// NURB must be rational of degree 2 both ways with circular iso curves, those in
    /// one direction on parallel planes and those in the other with a common centre.
    /// Nothing changes when the constant-u iso curves are not circles or the generator
    /// extends beyond the poles.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for another degree or a non-rational NURB, iso circles without the
    /// sphere orientation or a degenerate generator; `InvalidInput` when an iso-circle
    /// output upstream would read is missing; those of the iso-circle tests, the
    /// evaluations, the generator extraction and the circle.
    pub fn rebuild_step_from_nurb_parameters(&mut self, k: &Kernel) -> Result<()> {
        let srf = &self.base.base;

        // check m_pNurb - a degree 2 rational surface in at least 1 direction
        if srf.degree(SurfParamType::U) != 2
            || srf.degree(SurfParamType::V) != 2
            || !srf.is_rational()
        {
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // evaluate min/max domain points
        let nurb_uv_domain = srf.natural_uv_domain();
        let nurb_min_point = srf.evaluate_point(nurb_uv_domain.min())?;
        let nurb_max_point = srf.evaluate_point(nurb_uv_domain.max())?;

        // scaled zero value
        let tolerance_scale = 1000.0; // loose because surf is already labeled analytic
        let scaled_zero = tolerance_scale
            * ANALYTIC_TOL_SCALE
            * EFF_ZERO
            * (1.0 + nurb_min_point.max_dimension() + nurb_max_point.max_dimension());
        let ang_tol = EFF_ZERO_SQRT * tolerance_scale;

        // test sequence of V Direction (U Constant) isoCurves for circularity and
        // parallelism
        let iso_u = test_for_iso_circles(k, srf, SurfParamType::U, scaled_zero, ang_tol)?;

        // quit when not a sphere (upstream returns FALSE, i.e. SM_SUCCESS)
        if !iso_u.found_circles {
            return Ok(());
        }

        // test sequence of U Direction (V Constant) isoCurves for circularity and
        // parallelism
        let iso_v = test_for_iso_circles(k, srf, SurfParamType::V, scaled_zero, ang_tol)?;

        // see if we have a U or a V oriented sphere
        // Tol: compensate for using degrees.  [B279]
        let angles_u = iso_u.angles.unwrap_or(Extent1d::new());
        let angles_v = iso_v.angles.unwrap_or(Extent1d::new());
        let is_sphere_with_swap_uv = iso_u.parallel_planes
            && !iso_v.same_plane // [ 080625 ]
            && iso_v.coaxial_centers
            && angles_v.length() <= 180.0 + 360.0 * EFF_ZERO;
        let is_sphere_no_swap_uv = iso_v.parallel_planes
            && !iso_v.same_plane // [ 080625 ]
            && iso_u.coaxial_centers
            && angles_u.length() <= 180.0 + 360.0 * EFF_ZERO;

        // quit when not a properly oriented sphere
        if !is_sphere_with_swap_uv && !is_sphere_no_swap_uv {
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // locals
        let nurb_min_uv = nurb_uv_domain.evaluate(0.0, 0.0);
        let missing = || SolidError::invalid_input(OP_REBUILD, "iso circle output not written");

        // Get sphere position and parameters
        let (swap_uv, ref_iso, other_iso) = if is_sphere_no_swap_uv {
            (false, &iso_v, &iso_u)
        } else {
            (true, &iso_u, &iso_v)
        };
        let ref_frame = ref_iso.reference_frame.ok_or_else(missing)?;
        let other_frame = other_iso.reference_frame.ok_or_else(missing)?;
        let rad = other_iso.radius.ok_or_else(missing)?;
        let sweep_ivl = ref_iso.angles.ok_or_else(missing)?;

        // get sphere orientation and size
        let sphere_origin = other_frame.origin();
        let sphere_x_axis = ref_frame.x_axis();
        let sphere_y_axis = ref_frame.y_axis();
        let sphere_z_axis = ref_frame.z_axis();

        // Check GenCurve rotation direction, its 'y' axis is either +/- sweepCircle z
        // bInsideOut: TRUE = GenCurve starts at +90 Pole and ends at -90 Pole
        //             FALSE= GenCurve starts at -90 Pole and ends at +90 Pole
        // assumes: OterhFrame is valid for 1st Nurb isoparameter GenCurve
        let minor_orientation = other_frame.z_axis().cross(sphere_x_axis);
        let inside_out = sphere_z_axis.dot(minor_orientation) < 0.0;

        // measure genCurve angles from sphere -Z axis
        let vec_min = nurb_min_point - sphere_origin;
        let vec_max = nurb_max_point - sphere_origin;

        let neg_z_axis = -sphere_z_axis;
        let mut angle_min = ignored(neg_z_axis.angle_between(vec_min))?.unwrap_or(0.0);
        let mut angle_max = ignored(neg_z_axis.angle_between(vec_max))?.unwrap_or(0.0);
        angle_min = 180.0 * angle_min / PI;
        angle_max = 180.0 * angle_max / PI;

        // adjust param range from [Umin_to_UMax, 0_to_180] to [Umin_to_UMax, -90_to_90]
        //  acounting for InsideOut reversals
        let adjust_angle = -90.0;
        let mut step_min = Point2::new(sweep_ivl.min(), min2(angle_min, angle_max) + adjust_angle);
        let mut step_max = Point2::new(sweep_ivl.max(), max2(angle_min, angle_max) + adjust_angle);

        // snap to ends for small tolerances - quit for genCurves that extend beyond the
        // poles (upstream returns FALSE, i.e. SM_SUCCESS)
        if step_min.y < -90.0 {
            if step_min.y < -90.0 - EFF_ZERO_SQRT {
                return Ok(());
            }
            step_min.y = -90.0;
        }
        if step_max.y > 90.0 {
            if step_max.y > 90.0 + EFF_ZERO_SQRT {
                return Ok(());
            }
            step_max.y = 90.0;
        }

        // define Sphere domain
        let anal_uv_domain = Extent2d::from_bounds(step_min.x, step_min.y, step_max.x, step_max.y);

        // Get GenCurve Isoparameter Curve
        let gen_curve_iso = srf.create_iso_parametric_curve(
            k,
            if swap_uv {
                SurfParamType::V
            } else {
                SurfParamType::U
            },
            if swap_uv {
                nurb_min_uv.y
            } else {
                nurb_min_uv.x
            },
            IsoCurveOptions::default(),
        )?;
        let circle = Ellipse::new_circle(
            k,
            sphere_origin,
            sphere_x_axis,
            sphere_z_axis,
            anal_uv_domain.v_interval(),
            rad,
            &EllipseOptions {
                dim: 3,
                nurb: Some(&gen_curve_iso.curve),
                inside_out,
            },
        )?;

        // no work - GenCurve is degenerate
        if circle.is_degenerate(scaled_zero) {
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // set m_pGenCurve
        self.base.gen_curve = Some(Curve::from(circle));

        // set Analytic parameters
        self.radius = rad;
        self.inside_out = inside_out;
        self.base.planar_generator = true;
        let _ = self
            .base
            .position
            .set_canonical(sphere_origin, sphere_x_axis, sphere_y_axis);
        self.base.swap_uv = swap_uv;
        self.base.anal_uv_domain = anal_uv_domain;

        // Set PolarConverter from Nurb midPoint rotation isoParamCurve (status ignored)
        ignore_status(self.rebuild_polar_converter(k))?;
        Ok(())
    }

    // ---- closed directions ----------------------------------------------------------

    /// Whether the surface is closed in the NURB direction `surf_param` over
    /// `uv_domain` and its continuity (`IsClosed`): only around the equator (the
    /// longitude direction, NURB v with [`SurfOfRevolution::swap_uv`], else u), when
    /// the STEP domain covers 360 degrees and `uv_domain` is the NURB's domain there;
    /// the continuity is then [`ContinuityType::C1G2`], else
    /// [`ContinuityType::Discontinuous`]. Never for [`SurfParamType::Both`].
    #[must_use]
    pub fn is_closed(
        &self,
        uv_domain: &Extent2d,
        surf_param: SurfParamType,
    ) -> (bool, ContinuityType) {
        // Init output
        let mut continuity = ContinuityType::Discontinuous;

        // A sphere can't be closed in both U and V:
        if surf_param == SurfParamType::Both {
            return (false, continuity);
        }

        // Also, we're not closed in our north-south direction.
        if !self.base.swap_uv && surf_param == SurfParamType::V {
            return (false, continuity);
        }
        if self.base.swap_uv && surf_param == SurfParamType::U {
            return (false, continuity);
        }

        // Ok, here we're checking around our equator. (Either they're asking for U, or
        // we're swapped.) We can't be closed if our analytic domain does not cover a
        // complete circle. The east-west direction is always U in the analytic
        // representation.
        let arc_length_deg_u = self.base.anal_uv_domain.u_interval().length();
        let tol_deg = closed_tol_deg(self.radius);
        if !are_same_to_tol(arc_length_deg_u, 360.0, tol_deg) {
            return (false, continuity);
        }

        // The only thing left to check is that the passed-in domain matches our Nurbs
        // domain, in the direction of interest.
        let nurb_domain = self.base.base.natural_uv_domain();
        let (nurbs_sweep_ivl, given_sweep_ivl) = if surf_param == SurfParamType::U {
            (nurb_domain.u_interval(), uv_domain.u_interval())
        } else {
            (nurb_domain.v_interval(), uv_domain.v_interval())
        };

        let closed_sweep = nurbs_sweep_ivl.is_contained_by(&given_sweep_ivl, 0.0)
            && given_sweep_ivl.is_contained_by(&nurbs_sweep_ivl, 0.0);

        if closed_sweep {
            continuity = ContinuityType::C1G2;
        }
        (closed_sweep, continuity)
    }

    /// Whether all sweep latitude circles are closed: the STEP u interval is 360
    /// degrees long (`IsClosedU`; not [`Self::is_closed`]: it works in the analytic
    /// definition).
    #[must_use]
    pub fn is_closed_u(&self) -> bool {
        let arc_length_deg_u = self.base.anal_uv_domain.u_interval().length();
        let tol_deg = closed_tol_deg(self.radius);
        are_same_to_tol(arc_length_deg_u, 360.0, tol_deg)
    }

    /// Whether all longitude circles run from pole to pole: the STEP v interval is 180
    /// degrees long (`IsClosedV`).
    #[must_use]
    pub fn is_closed_v(&self) -> bool {
        let arc_length_deg_v = self.base.anal_uv_domain.v_interval().length();
        let tol_deg = closed_tol_deg(self.radius);
        are_same_to_tol(arc_length_deg_v, 180.0, tol_deg)
    }

    // ---- point inversion ------------------------------------------------------------

    /// The STEP parameters of `point_on_surf` (dropped onto the sphere along the ray
    /// from the centre), whether it is inside `anal_domain` and this sphere's domain
    /// and its class (`STEPInversion`): [`LocationType::Pole`] within the angular
    /// tolerance of a pole, [`LocationType::USeam`] on the seam of a closed domain,
    /// else [`LocationType::Interior`]; [`LocationType::Exterior`] outside. The
    /// longitude is moved into `anal_domain` when it is periodic within it, and on a
    /// seam with `uv_guess` the upper end when the guess is above the domain's middle.
    /// `d3_tolerance` is the largest allowed distance of the point from the sphere,
    /// else [`SphereStepInversion::on_surface`] is `false` (`SM_ERR`).
    #[must_use]
    pub fn step_inversion(
        &self,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        d3_tolerance: f64,
        uv_guess: Option<Point2>,
    ) -> SphereStepInversion {
        // Sphere locals
        let origin = self.base.position.origin();
        let x_axis = self.base.position.x_axis();
        let y_axis = self.base.position.y_axis();
        let z_axis = self.base.position.z_axis();
        let tol_2d = d3_tolerance * 180.0 / PI / self.radius;
        let closed = (self.base.anal_uv_domain.x_length() - 360.0).abs() < tol_2d;
        let mut on_surf = true;
        let u_domain = Extent1d::from_min_max(
            self.base.anal_uv_domain.min().x,
            self.base.anal_uv_domain.max().x,
        );
        let scaled_zero = EFF_ZERO * (1.0 + origin.max_dimension());

        // get Point-SphereCenter vector
        let mut vec = point_on_surf - origin;
        let length = vec.length();

        // Origin Points are errors
        if length < scaled_zero {
            return SphereStepInversion {
                uv: Point2::new(0.0, 0.0),
                location: LocationType::Exterior,
                on_surface: false,
            };
        }

        // drop point to Sphere - Remember when Point was not initially on Surface
        vec *= self.radius / length;
        if (length - self.radius).abs() > d3_tolerance {
            on_surf = false;
        }

        // get Object coordinates for the point
        let d_x = vec.dot(x_axis);
        let d_y = vec.dot(y_axis);
        let d_z = vec.dot(z_axis);

        // Get Spherical Coordinates (degrees)
        let d_u = d_y.atan2(d_x) * 180.0 / PI; //  dU range:[-180 to 180]
        let d_v = asin(d_z / self.radius) * 180.0 / PI; //  dV range:[-90 to 90]

        // set output
        let mut uv = Point2::new(d_u, d_v);

        // classify point
        let on_pole = d_v > 90.0 - tol_2d || d_v < -90.0 + tol_2d;
        let on_seam = closed && u_domain.is_value_on_boundary(d_u, tol_2d).0;

        // return error for points off Sphere or outside of domain
        // side effect: move rdAnalUVParameter into crAnalDomain if appropriate [B606]
        let (in_given, moved) = anal_domain.contains_periodic_point(uv, 360.0, 0.0, tol_2d);
        uv = moved;
        let inside = in_given
            && self
                .base
                .anal_uv_domain
                .contains_periodic_point(uv, 360.0, 0.0, tol_2d)
                .0;

        let location = if inside && on_pole {
            LocationType::Pole
        } else if inside && on_seam {
            LocationType::USeam
        } else if inside {
            LocationType::Interior
        } else {
            LocationType::Exterior
        };

        // If a guess was given and we're on the seam, set u-parameter to what's closer
        // to the guess.
        if on_seam {
            if let Some(guess) = uv_guess {
                if guess.x > u_domain.mid() {
                    uv.x = u_domain.max();
                }
            }
        }

        // all done
        SphereStepInversion {
            uv,
            location,
            on_surface: on_surf,
        }
    }

    // ---- transform ------------------------------------------------------------------

    /// Scales the radius by the optional uniform `scale`, then transforms the surface of
    /// revolution (`Transform`). The identity placement with no or unit scale changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a non-uniform scale or one below `SM_EFF_ZERO`;
    /// [`SolidError::Unported`] from `SmSurfOfRevolution::Transform` (whose other
    /// statuses upstream ignores), after the radius is scaled.
    pub fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
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
                // Unable to scale analytical surfaces non-uniformly
                return Err(SolidError::sm_err(OP_TRANSFORM));
            }
            d_scale = s.x;
        }

        // update sphere size
        self.radius *= d_scale;

        // finish the call in the parent function (status ignored upstream), whose
        // polar-converter rebuild reaches the sphere's iso circles
        let shape = self.shape();
        ignore_status(
            self.base
                .transform_using(k, rotate_n_move, scale, &|s, k, p, t| {
                    Ok(iso_curve(s, shape, k, p, t, None, false)?.curve)
                }),
        )?;

        // all done
        Ok(())
    }

    // ---- I/O ------------------------------------------------------------------------

    /// Reads a sphere record (`ReadFromDB`): the radius and the inside-out flag (one
    /// per line in ASCII files), then the surface of revolution record. Unreadable
    /// ASCII values keep their defaults.
    ///
    /// # Errors
    ///
    /// A failed binary read; those of the surface of revolution record
    /// ([`SurfOfRevolution::read_from_db`]).
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        // init output object
        let mut sphere = Self::empty();

        if db.file_type() == FileType::Ascii {
            if let Some(s) = db.in_stream_mut() {
                extract(s, &mut sphere.radius);
            }
            db.go_to_next_line();
            if let Some(v) = db.in_stream_mut().and_then(InputStream::extract_int) {
                sphere.inside_out = v != 0;
            }
            db.go_to_next_line();
        } else {
            sphere.radius = db.read_double()?;
            sphere.inside_out = db.read_boolean()? != 0;
        }

        // read the parent object
        sphere.base = SurfOfRevolution::read_from_db(k, db, version)?;

        // all done
        Ok(sphere)
    }
}

/// `SM_DEG2RAD(a)` = `a * (SM_PI / 180.0)`.
fn deg2rad(a: f64) -> f64 {
    a * (PI / 180.0)
}

/// The sphere is a surface of revolution (the other operand of the intersections).
impl Revolved for Sphere {
    fn as_surf_of_revolution(&self) -> &SurfOfRevolution {
        &self.base
    }

    fn convert_domain_from_nurbs_to_step(
        &self,
        k: &Kernel,
        nurb_domain: &Extent2d,
    ) -> Result<Extent2d> {
        convert_domain_from_nurbs_to_step_with(nurb_domain, |uv| {
            self.convert_uv_from_nurbs_to_step(k, uv)
        })
    }
}

/// `SmSphere::CreateIsoParametricCurve` of the surface of revolution `rev` of a sphere
/// of `shape` (also called from the base class's rebuilds, where it is virtual).
fn iso_curve(
    rev: &SurfOfRevolution,
    shape: Shape,
    k: &Kernel,
    surf_param: SurfParamType,
    iso_parameter: f64,
    domain: Option<&Extent2d>,
    want_uv_iso_curve: bool,
) -> Result<RevolutionIsoCurve> {
    // get the BSplineCurve (status ignored upstream, which then reads the curve)
    let iso = rev.base.create_iso_parametric_curve(
        k,
        surf_param,
        iso_parameter,
        IsoCurveOptions {
            domain: domain.copied(),
            uv_iso_curve: want_uv_iso_curve,
        },
    )?;

    // low work - when curve is degenerate don't bother making a circle
    if iso.curve.is_degenerate(k, EFF_ZERO, None)? {
        return Ok(RevolutionIsoCurve {
            curve: Curve::from(iso.curve),
            max_gap_3d: iso.max_gap_3d,
            uv_iso_curve: iso.uv_iso_curve,
        });
    }

    // don't use converters to get Step Params because Polar Converters call this
    // function when getting themselves set up to date.

    // locals
    let step_param = if rev.swap_uv {
        if surf_param == SurfParamType::U {
            SurfParamType::V
        } else {
            SurfParamType::U
        }
    } else {
        surf_param
    };
    let sphere_origin = rev.position.origin();
    let sphere_x_axis = rev.position.x_axis();
    let sphere_y_axis = rev.position.y_axis();
    let sphere_z_axis = rev.position.z_axis();

    // Get IsoCurve Nurb endPoints (statuses ignored upstream)
    let nurb_ivl = iso.curve.natural_interval();
    let nurb_min_point = iso.curve.evaluate_point(nurb_ivl.min())?;
    let nurb_max_point = iso.curve.evaluate_point(nurb_ivl.max())?;
    let nurb_min_vec = nurb_min_point - sphere_origin;
    let nurb_max_vec = nurb_max_point - sphere_origin;

    // get iso circle parameters
    let (circle_center, x_axis, y_axis, radius, inside_out, anal_domain) = if step_param
        == SurfParamType::U
    {
        // return Pole_to_Pole isoCurve circle
        let nurb_mid_point = iso.curve.evaluate_point(nurb_ivl.evaluate(0.5))?;
        let nurb_mid_vec = nurb_mid_point - sphere_origin;

        // (the length is only asserted upstream; Unitize's status is ignored)
        let x_axis = sphere_z_axis
            .cross(nurb_mid_vec.cross(sphere_z_axis))
            .unitize_or_z();
        (
            rev.position.origin(),
            x_axis,
            sphere_z_axis,
            shape.radius,
            shape.inside_out,
            rev.anal_uv_domain.v_interval(),
        )
    } else {
        // return sweep isoCurve circle
        let circle_center = rev.position.origin() + nurb_min_vec.dot(sphere_z_axis) * sphere_z_axis;
        (
            circle_center,
            sphere_x_axis,
            sphere_y_axis,
            (nurb_min_point - circle_center).length(),
            false,
            rev.anal_uv_domain.u_interval(),
        )
    };

    // get isoParamCurve domain
    let circle_anal_domain = if domain.is_none() {
        // when using the current domain
        anal_domain
    } else {
        // get angles to min/max points inside periodic interval (statuses ignored
        // upstream; a failed angle is 0)
        let circle_z_axis = x_axis.cross(y_axis);
        let min_ang_rad =
            ignored(circle_z_axis.ccw_angle_between(x_axis, nurb_min_vec))?.unwrap_or(0.0);
        let max_ang_rad =
            ignored(circle_z_axis.ccw_angle_between(x_axis, nurb_max_vec))?.unwrap_or(0.0);
        let min_ang_deg = anal_domain
            .contains_periodic_value(min_ang_rad * 180.0 / PI, 360.0, -1.0)
            .periodic_value;
        let max_ang_deg = anal_domain
            .contains_periodic_value(max_ang_rad * 180.0 / PI, 360.0, -1.0)
            .periodic_value;

        // when both values are on a closed interval seam
        // (Use same tolerance as in ContainsPeriodicValue(), which gets called here.)
        let period = 360.0;
        // Tol: Even 10x still a little too tight: a340retLink_1.
        let tol = EFF_ZERO * 100.0 * (1.0 + period);
        if anal_domain.is_closed(period)
            && anal_domain
                .is_value_on_periodic_boundary(min_ang_deg, period, tol)
                .0
            && anal_domain
                .is_value_on_periodic_boundary(max_ang_deg, period, tol)
                .0
        {
            // set interval to whole domain
            anal_domain
        } else {
            // set isoCurve analytic domain accounting for m_binside reversals
            let mut ivl = Extent1d::new();
            let _ = ivl.set_min_max(
                min2(min_ang_deg, max_ang_deg),
                max2(min_ang_deg, max_ang_deg),
            );
            ivl
        }
    };

    // create the circle using the isoCurve parameterization
    let circle = Ellipse::new_circle(
        k,
        circle_center,
        x_axis,
        y_axis,
        circle_anal_domain,
        radius,
        &EllipseOptions {
            dim: 3,
            nurb: Some(&iso.curve),
            inside_out,
        },
    )?;
    Ok(RevolutionIsoCurve {
        curve: Curve::from(circle),
        max_gap_3d: iso.max_gap_3d,
        uv_iso_curve: iso.uv_iso_curve,
    })
}

/// The body of `ConvertUVFromNURBSToSTEP` on the surface of revolution `base` of a
/// sphere (the trim's `UpdateAnalyticalDomain` reaches it through its base).
fn convert_uv_from_nurbs_to_step(
    base: &SurfOfRevolution,
    k: &Kernel,
    nurbs_uv: Point2,
) -> Result<Point2> {
    // m_bSwapUV - Nurb and STEP parameters are swapped
    let nurb_uv = if base.swap_uv {
        Point2::new(nurbs_uv.y, nurbs_uv.x)
    } else {
        nurbs_uv
    };

    // convert sweep coordinate
    let polar_curve = base.polar_curve().ok_or(SolidError::invalid_input(
        OP_CONVERT,
        "polar converter has no curve",
    ))?;
    let (step_x, _exact) = base
        .polar
        .convert_to_polar_parameter(k, polar_curve, nurb_uv.x)?;

    // convert genCurve NurbParam to PolarParam
    let circle = gen_circle(base.gen_curve.as_ref(), OP_CONVERT)?;
    let polar_param = circle.convert_t_from_nurbs_to_step(k, nurb_uv.y)?;
    Ok(Point2::new(step_x, polar_param))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Axis2Placement {
        Axis2Placement::new(
            Vec3::new(1.0, 2.0, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        )
    }

    fn canonical(k: &Kernel, radius: f64) -> Sphere {
        Sphere::create_canonical(k, &frame(), radius).expect("sphere")
    }

    #[test]
    fn trim_with_domain_keeps_the_shape() {
        let k = Kernel::default();
        let mut sphere = canonical(&k, 2.0);
        let d = sphere.base.base.natural_uv_domain();
        let (lo, hi) = (d.min(), d.max());
        let trim = Extent2d::from_bounds(
            lo.x + 0.25 * (hi.x - lo.x),
            lo.y + 0.125 * (hi.y - lo.y),
            lo.x + 0.75 * (hi.x - lo.x),
            lo.y + 0.5 * (hi.y - lo.y),
        );
        sphere.trim_with_domain(&k, &trim).expect("trim");
        let nd = sphere.base.base.natural_uv_domain();
        assert!(nd.is_contained_by(&trim, 1e-12) && trim.is_contained_by(&nd, 1e-12));
        let p = sphere.base.base.evaluate_point(trim.max()).expect("eval");
        assert!(((p - frame().origin()).length() - 2.0).abs() < 1e-9);
        let back = sphere
            .convert_uv_from_nurbs_to_step(&k, trim.max())
            .expect("to step");
        let q = sphere.evaluate_step_point(back).expect("eval step");
        assert!((p - q).length() < 1e-9);
    }

    #[test]
    fn offset_surface_intersection_reaches_the_general_intersection() {
        let k = Kernel::default();
        let sphere = canonical(&k, 2.0);
        let domain = sphere.base.base.natural_uv_domain();
        let offset = crate::surface::offset_surface::OffsetSurface::new(
            &k,
            0.25,
            Surface::from(sphere.base.clone()),
            false,
            0,
        )
        .unwrap();
        let other = Surface::from(offset);
        // the sphere is a SurfaceKind: no special case, so the general intersection
        // runs with the sphere as `this` (no dispatch gap)
        let r = sphere.global_surface_intersect(
            &k,
            &domain,
            &other,
            &other.natural_uv_domain_if_built(),
            [true, true],
            None,
            None,
        );
        assert!(!matches!(
            r,
            Err(SolidError::Unported {
                unit: "surface.sphere",
                ..
            })
        ));
    }

    #[test]
    fn step_and_nurb_points_agree() {
        let k = Kernel::default();
        for radius in [2.0, -2.0] {
            let s = canonical(&k, radius);
            assert!(s.base.have_polar_conversion());
            assert_eq!(s.inside_out(), radius < 0.0);
            for (u, v) in [(10.0, 20.0), (100.0, -50.0), (359.0, 80.0), (0.0, 0.0)] {
                let step = Point2::new(u, v);
                let p = s.evaluate_step_point(step).expect("eval");
                assert!(((p - frame().origin()).length() - 2.0).abs() < 1e-12);
                let nurb = s.convert_uv_from_step_to_nurbs(&k, step).expect("to nurbs");
                let q = s.base.base.evaluate_point(nurb).expect("nurb eval");
                assert!((p - q).length() < 1e-9, "{radius} {u} {v}");
                let back = s.convert_uv_from_nurbs_to_step(&k, nurb).expect("to step");
                let r = s.evaluate_step_point(back).expect("eval back");
                assert!((p - r).length() < 1e-9);
            }
        }
    }

    #[test]
    fn rejects_degenerate_radii() {
        let k = Kernel::default();
        for r in [0.0, 1e-13, f64::NAN, -1e-13] {
            let res = Sphere::create_canonical(&k, &frame(), r);
            assert!(matches!(res, Err(SolidError::Upstream { .. })));
        }
    }

    #[test]
    fn offset_moves_the_surface() {
        let k = Kernel::default();
        let s = canonical(&k, 2.0);
        let off = s
            .create_offset_surface(&k, 0.5, 1e-6)
            .expect("offset")
            .expect("surface");
        assert!((off.radius() - 2.5).abs() < 1e-15);
        let nurb = off
            .convert_uv_from_step_to_nurbs(&k, Point2::new(30.0, 40.0))
            .expect("uv");
        let p = off.base.base.evaluate_point(nurb).expect("eval");
        assert!(((p - frame().origin()).length() - 2.5).abs() < 1e-9);
        assert!(s
            .create_offset_surface(&k, -3.0, 1e-6)
            .expect("offset")
            .is_none());
    }

    #[test]
    fn rebuild_recovers_the_sphere() {
        let k = Kernel::default();
        let s = canonical(&k, 2.0);
        let mut r = s.clone();
        r.rebuild_step_from_nurb_parameters(&k).expect("rebuild");
        assert!((r.radius() - 2.0).abs() < 1e-9);
        assert!(s.approx_eq(&r));
    }

    #[test]
    fn closed_directions() {
        let k = Kernel::default();
        let s = canonical(&k, 2.0);
        assert!(s.is_closed_u());
        assert!(s.is_closed_v());
        let nurb = s.base.base.natural_uv_domain();
        assert!(s.is_closed(&nurb, SurfParamType::U).0);
        assert!(!s.is_closed(&nurb, SurfParamType::V).0);
        assert!(!s.is_closed(&nurb, SurfParamType::Both).0);
    }

    #[test]
    fn step_inversion_finds_the_point() {
        let k = Kernel::default();
        let s = canonical(&k, 2.0);
        let step = Point2::new(100.0, 30.0);
        let p = s.evaluate_step_point(step).expect("eval");
        let inv = s.step_inversion(&s.base.anal_uv_domain, p, 1e-9, None);
        assert!(inv.on_surface);
        assert_eq!(inv.location, LocationType::Interior);
        assert!((inv.uv.x - 100.0).abs() < 1e-9 && (inv.uv.y - 30.0).abs() < 1e-9);
        let pole = s.step_inversion(
            &s.base.anal_uv_domain,
            frame().origin() + Vec3::new(0.0, 0.0, 2.0),
            1e-9,
            None,
        );
        assert_eq!(pole.location, LocationType::Pole);
    }

    #[test]
    fn spheres_meet_in_a_circle() {
        let k = Kernel::default();
        let a = canonical(&k, 2.0);
        let other = Sphere::create_canonical(
            &k,
            &Axis2Placement::new(
                Vec3::new(2.0, 2.0, 0.5),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ),
            2.0,
        )
        .expect("sphere");
        let (more, out) = a
            .intersect_with_sphere(
                &k,
                &a.base.base.natural_uv_domain(),
                &other,
                &other.base.base.natural_uv_domain(),
                None,
            )
            .expect("intersect");
        assert!(!more);
        assert_eq!(out.curves_3d.len(), 1);
        assert_eq!(out.curve_types, vec![TsectCurveType::Crossing]);
    }

    #[test]
    fn read_reaches_the_base_record() {
        let mut db = DatabaseReader::new();
        let r = Sphere::read_from_db(&Kernel::default(), &mut db, 0);
        assert!(r.is_err());
    }

    /// The control points and knots of two NURBs agree bit for bit.
    fn same_nurb(a: &BSplineSurface, b: &BSplineSurface) -> bool {
        match (a.gw_nurb(), b.gw_nurb()) {
            (Some(a), Some(b)) => {
                let rows = |s: &crate::nurbs::srf_basic::Surface| {
                    s.rows()
                        .flat_map(|r| r.iter().map(|p| [p.x, p.y, p.z, p.w]))
                        .collect::<Vec<_>>()
                };
                rows(a) == rows(b) && a.u_knots() == b.u_knots() && a.v_knots() == b.v_knots()
            }
            _ => false,
        }
    }

    /// source fork@7ea6a96 (WR-493): with `found_torus_sphere_keep_tested_nurb`
    /// the sphere found in a NURB keeps exactly that NURB.
    #[test]
    fn wr493_sphere_found_in_its_nurb_keeps_it() {
        let mut k = Kernel::default();
        k.fixes.found_torus_sphere_keep_tested_nurb = true;
        for radius in [2.0, -2.0, 1e-3, 500.0] {
            let s = canonical(&k, radius);
            let nurbs = BSplineSurface::from_gw_nurb(s.base().base().gw_nurb().unwrap());
            let found = Sphere::is_nurb_surface_sphere(&k, &nurbs, 1.0)
                .unwrap()
                .expect("a sphere");
            assert!(same_nurb(found.base().base(), &nurbs));
            assert!((found.radius() - radius.abs()).abs() < 1e-9 * radius.abs());
        }
    }
}
