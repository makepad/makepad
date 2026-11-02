//! Tori (`SmTorus`): a surface of revolution sweeping a minor circle about the axis of
//! its placement, with the STEP evaluation `S(u, v) = O + (R + r cos v)(cos u X +
//! sin u Y) + r sin v Z` (angles in degrees), the conversions between the STEP and
//! NURB parameters, the circular iso curves, the offset torus and the rebuild of the
//! analytic data from the NURB.
//!
//! Upstream: `source/SMLib/src/SmTorus.cpp`, `source/SMLib/inc/SmTorus.h`.
//!
//! | Rust                                         | Upstream                                     |
//! |----------------------------------------------|----------------------------------------------|
//! | [`Torus`], [`Torus::new`], [`TorusOptions`]  | `SmTorus`, `SmTorus(crOrigin, crXAxis, crYAxis, crAnalUVDomain, ..)` |
//! | [`Torus::empty`]                             | `SmTorus()`                                  |
//! | `Clone`, [`Torus::copy`]                     | `SmTorus(const SmTorus&)`, `Copy`            |
//! | `Drop` (implicit)                            | `~SmTorus`                                   |
//! | [`Torus::approx_eq`]                         | `operator==`                                 |
//! | [`Torus::create_canonical`]                  | `CreateCanonical`                            |
//! | [`Torus::make_nurb`]                         | `SmSurfOfRevolution::MakeNurb` on a torus (virtual iso curves) |
//! | [`Torus::reparameterize`], [`Torus::adjust_step_uv_domain`] | `SmSurfOfRevolution::Reparameterize`, `AdjustSTEPUVDomain` on a torus (virtual iso curves) |
//! | [`Torus::trim_with_domain`]                 | `SmSurfOfRevolution::TrimWithDomain` on a torus (virtual conversion and iso curves) |
//! | [`Torus::intersect_with_plane`]             | `SmSurfOfRevolution::IntersectWithPlane` on a torus (virtual iso curves) |
//! | [`Torus::create_offset_surface`]             | `CreateOffsetSurface`                        |
//! | [`Torus::convert_uv_from_step_to_nurbs`]     | `ConvertUVFromSTEPToNURBS`                   |
//! | [`Torus::convert_uv_from_nurbs_to_step`]     | `ConvertUVFromNURBSToSTEP`                   |
//! | [`Torus::evaluate_step`], [`Torus::evaluate_step_point`] | `EvaluateSTEP`, `EvaluateSTEPPoint` |
//! | [`Torus::create_iso_parametric_curve`]       | `CreateIsoParametricCurve`                   |
//! | [`Torus::is_nurb_surface_torus`]             | `IsNurbSurfaceTorus`                         |
//! | [`Torus::rebuild_step_from_nurb_parameters`] | `RebuildSTEPFromNURBParameters`              |
//! | [`Torus::transform`]                         | `Transform`                                  |
//! | [`Torus::read_from_db`]                      | `ReadFromDB`                                 |
//! | [`Torus::max_analytic_domain`]               | `GetMaxAnalyticDomain`                       |
//! | [`Torus::is_kind_of`], [`Torus::TYPE`], [`Torus::type_code`], [`Torus::type_string`], [`Torus::class_string`] | `IsKindOf`, `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`Torus::inside_out`], [`Torus::major_radius`], [`Torus::minor_radius`] | `GetInsideOut`, `GetMajorRadius`, `GetMinorRadius` (inline) |
//!
//! Deviations:
//! - **Derived class.** `SmTorus` derives from `SmSurfOfRevolution`; the base object is
//!   the [`Torus::base`] field ([`SurfOfRevolution`]). A
//!   [`crate::surface::surface::Surface`] holds it as
//!   [`crate::surface::surface::SurfaceKind::Torus`], whose dispatch runs the torus's
//!   overrides and the inherited `SmSurfOfRevolution` / `SmBSplineSurface` bodies on
//!   its base. Upstream's virtual `CreateIsoParametricCurve`, called by the base
//!   class's NURB and polar-converter rebuilds, reaches the torus override through
//!   [`Torus::make_nurb`], [`Torus::reparameterize`], [`Torus::adjust_step_uv_domain`]
//!   and the private `rebuild_polar_converter`. [`Torus::trim_with_domain`] also takes
//!   the torus's `ConvertUVFromNURBSToSTEP`, and returns a failed corner conversion
//!   (upstream ignores it and reads the corner it may not have written).
//!   [`Torus::intersect_with_plane`] runs the inherited `IntersectWithPlane` with the
//!   torus's iso circles. Opt-in fork fix: a minor iso circle whose start lies on the
//!   axis takes its direction from its midpoint, not the start point again
//!   (`Kernel::fixes.torus_iso_circle_axis_from_midpoint`, WR-504, #1514; off by
//!   default, upstream gives it no direction).
//! - **Iso endpoint angles.** Correct upstream's explicit-domain endpoint vectors
//!   to be relative to the iso circle's center, not the torus origin (#774).
//!   Extraction, periodic wrapping and interval-length policy remain unchanged.
//! - **Copies.** As upstream's copy constructor, which leaves `m_bInsideOut` at its
//!   default, `Clone` (and [`Torus::copy`]) resets [`Torus::inside_out`] to `false`.
//!   The constructor also leaves it `false` (its `bInsideOut` only orients the minor
//!   circle); only [`Torus::rebuild_step_from_nurb_parameters`] and
//!   [`Torus::read_from_db`] set it. With the opt-in
//!   `Kernel::fixes.torus_keeps_inside_out` (WR-500, #1490) the constructor sets it
//!   from `bInsideOut` and a torus made, rebuilt or read that way keeps it in its
//!   copies (`SmSurfOfRevolution` Test #11 compares it with the generator's).
//! - **Arguments.** Out-params become return values; `SmContext` arguments and the
//!   arguments upstream marks `NotUsed` (`EvaluateSTEP`'s side flags,
//!   `CreateIsoParametricCurve`'s approximation tolerance, `ReadFromDB`'s type) are
//!   dropped. `CreateIsoParametricCurve`'s optional UV iso line is forwarded to the
//!   B-spline surface. `EvaluateSTEP` entries it does not write are
//!   `0`. `CreateOffsetSurface` returns `None` where upstream succeeds without a surface
//!   (a new minor radius below the approximation tolerance).
//! - **Statuses.** Calls whose status upstream ignores keep upstream's values, except
//!   that [`SolidError::Unported`] always propagates and a failure whose output
//!   upstream would read uninitialised (a failed iso-curve extraction or evaluation, a
//!   missing generator, NURB or iso-circle output) is returned. A generator that is not
//!   an ellipse, which upstream casts to `SmCircle`, is `InvalidInput`.
//!   `RebuildSTEPFromNURBParameters` returns `FALSE` (`SM_SUCCESS`) when the
//!   constant-u iso curves are not circles; that is `Ok(())` with nothing changed.
//! - **Recognition.** [`Torus::is_nurb_surface_torus`] returns `Some` where upstream
//!   returns `TRUE` with a torus and an error where a `SER` call returns its status
//!   (a truthy result without torus). `Notify(SM_NO_COPY)` (attribute copying) has no
//!   equivalent. Two opt-in fork fixes: the torus keeps the tested NURB
//!   (`Kernel::fixes.found_torus_sphere_keep_tested_nurb`, WR-493; upstream keeps the
//!   NURB made from the fitted circles, ~1e-11 off the tested one), the generator
//!   interval is measured signed (`Kernel::fixes.torus_signed_gen_curve_interval`,
//!   WR-408; upstream's unsigned `AngleBetween` mirrors an interval below the equator;
//!   the zero tolerance is `SM_EFF_ZERO_DEG`, #1471), and the major radius is measured
//!   from the torus origin (`Kernel::fixes.torus_major_radius_from_origin`, WR-501; upstream
//!   measures it from the first sweep circle's centre, off the equator when the tube does
//!   not start there, #1470). All are off by default.
//! - **Parent calls.** `Transform` finishes in `SmSurfOfRevolution::Transform`
//!   (status ignored), its polar-converter rebuild given the torus's iso circles as
//!   upstream's virtual call does; `ReadFromDB` reads the parent record with
//!   [`SurfOfRevolution::read_from_db`] into the torus's base.

use core::ops::Div;

use crate::curve::curve::{Curve, CurveKind};
use crate::curve::ellipse::{Ellipse, EllipseOptions};
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::point_project_to_plane;
use crate::geom::tol::are_same;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, min2, EFF_ZERO, EFF_ZERO_DEG, EFF_ZERO_SQRT, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_surface::{
    BSplineSurface, BSplineSurfaceForm, IsoCurveOptions, SurfParamType,
};
use crate::nurbs::nurbs_crv::to_euclid;
use crate::nurbs::nurbs_srf::copy_nurb_surface;
use crate::surface::plane::p2::PlaneOwner;
use crate::surface::plane::Plane;
use crate::surface::surf_of_revolution::{
    test_for_iso_circles, RevolutionIsoCurve, Revolved, SurfOfRevolution,
};
use crate::surface::surface::p1::convert_domain_from_nurbs_to_step_with;
use crate::surface::surface::p2::{
    scale_step_angular_derivatives, SurfaceIntersection, TORUS_TYPE,
};
use crate::topology::database_io::{DatabaseReader, FileType, InputStream};

const OP_NEW: &str = "surface::torus::Torus::new";
const OP_CANONICAL: &str = "surface::torus::Torus::create_canonical";
const OP_OFFSET: &str = "surface::torus::Torus::create_offset_surface";
const OP_CONVERT: &str = "surface::torus::Torus::convert_uv";
const OP_EVAL_STEP: &str = "surface::torus::Torus::evaluate_step";
const OP_REBUILD: &str = "surface::torus::Torus::rebuild_step_from_nurb_parameters";
const OP_TRANSFORM: &str = "surface::torus::Torus::transform";
const OP_NURB_TORUS: &str = "surface::torus::Torus::is_nurb_surface_torus";

/// `ANALYTIC_TOL_SCALE` (`SmConfig.h`).
const ANALYTIC_TOL_SCALE: f64 = 100.0;

/// The optional constructor arguments of [`Torus::new`] and
/// [`Torus::create_canonical`].
#[derive(Debug, Clone, Default)]
pub struct TorusOptions {
    /// `bSwapUV`: the NURB's v is the sweep (major circle) direction.
    pub swap_uv: bool,
    /// `bInsideOut`: the minor circle's NURB runs against its STEP angle.
    pub inside_out: bool,
    /// `pOptCircleNurb`: the generator (minor circle) to use instead of making one;
    /// a curve derived from `SmBSplineCurve`, a circle for the conversions and the
    /// offset.
    pub circle_nurb: Option<Curve>,
}

/// The torus's own data, captured for the iso curves the base class makes.
#[derive(Debug, Clone, Copy)]
struct Shape {
    major_radius: f64,
    minor_radius: f64,
    inside_out: bool,
}

/// A torus (`SmTorus`): the surface of revolution of a minor circle of radius
/// [`Torus::minor_radius`] centred on the major circle of radius
/// [`Torus::major_radius`] about the Z axis of the placement. STEP parameters are the
/// major angle `u` (from the X axis about Z) and the minor angle `v`, both in degrees.
#[derive(Debug)]
pub struct Torus {
    /// The `SmSurfOfRevolution` base object.
    pub(crate) base: SurfOfRevolution,
    /// `m_dMajorRadius`.
    pub(crate) major_radius: f64,
    /// `m_dMinorRadius`.
    pub(crate) minor_radius: f64,
    /// `m_bInsideOut`.
    pub(crate) inside_out: bool,
    /// Not upstream (WR-500, #1490): made, rebuilt or read with
    /// `Kernel::fixes.torus_keeps_inside_out`, so the copy keeps
    /// [`Self::inside_out`].
    pub(crate) keeps_inside_out: bool,
}

/// The copy constructor: copies the surface of revolution and the radii; the
/// inside-out flag keeps its default (`false`), as upstream, unless the torus keeps it
/// (`Kernel::fixes.torus_keeps_inside_out`, WR-500).
impl Clone for Torus {
    fn clone(&self) -> Self {
        Self {
            base: self.base.clone(),
            major_radius: self.major_radius,
            minor_radius: self.minor_radius,
            inside_out: self.keeps_inside_out && self.inside_out,
            keeps_inside_out: self.keeps_inside_out,
        }
    }
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

impl Torus {
    /// `SmTorus_TYPE` (`SURF_BASE_TYPE + 15`).
    pub const TYPE: i64 = TORUS_TYPE;

    // ---- construction ---------------------------------------------------------------

    /// The empty torus `ReadFromDB` fills (`SmTorus()`): the empty surface of
    /// revolution and undefined radii.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            base: SurfOfRevolution::empty(),
            major_radius: UNDEF_DOUBLE,
            minor_radius: UNDEF_DOUBLE,
            inside_out: false,
            keeps_inside_out: false,
        }
    }

    /// The torus centred at `origin` with major circle start `x_axis` and 90 degree
    /// direction `y_axis` over `anal_uv_domain` (major by minor angles in degrees),
    /// without NURB (constructor). Without `opts.circle_nurb` the generator is the
    /// minor circle at `origin + major_radius x_axis` in the plane of `x_axis` and
    /// `x_axis x y_axis` over the domain's V interval, inside out with
    /// `opts.inside_out` (`SmCircle::CreateCanonical`); the generator is then not
    /// tested for planarity (it is marked planar). The surface's own inside-out flag
    /// stays `false`, as upstream.
    ///
    /// # Errors
    ///
    /// Those of the circle (a minor radius not above `SM_EFF_ZERO`, where upstream
    /// dereferences the null circle) and of [`SurfOfRevolution::new`] for a given
    /// generator.
    pub fn new(
        k: &Kernel,
        origin: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        anal_uv_domain: Extent2d,
        major_radius: f64,
        minor_radius: f64,
        opts: TorusOptions,
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
            base.base
                .set_b_spline_surface_form(BSplineSurfaceForm::SurfOfRevolution);

            // Domain limits
            let gen_ivl = anal_uv_domain.v_interval();

            // when not given a GenCurve to use: get minor circle parameters
            let circle_center = origin + major_radius * x_axis;
            let circle = Ellipse::create_canonical_circle(
                k,
                &Axis2Placement::new(circle_center, x_axis, x_axis.cross(y_axis)),
                minor_radius,
                Some(&gen_ivl),
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

        // (the self-intersection test of a major radius below the minor radius only
        // warns upstream); not upstream (WR-500, opt-in): the surface is inside out
        // with its generator
        let keeps_inside_out = k.fixes.torus_keeps_inside_out;
        Ok(Self {
            base,
            major_radius,
            minor_radius,
            inside_out: keeps_inside_out && opts.inside_out,
            keeps_inside_out,
        })
    }

    /// The torus of `origin` (centre, sweep start and 90 degree directions) and the two
    /// radii over `anal_uv_domain` (default `[0, 360] x [0, 360]`) with its NURB made
    /// (`CreateCanonical`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a radius not above `SM_EFF_ZERO` (or NaN) and when the NURB could
    /// not be made; those of [`Self::new`].
    pub fn create_canonical(
        k: &Kernel,
        origin: &Axis2Placement,
        major_radius: f64,
        minor_radius: f64,
        opts: TorusOptions,
        anal_uv_domain: Option<&Extent2d>,
    ) -> Result<Self> {
        // non-positive minor or major -> genuinely degenerate (no valid NURB)
        if minor_radius.is_nan()
            || minor_radius <= EFF_ZERO
            || major_radius.is_nan()
            || major_radius <= EFF_ZERO
        {
            return Err(SolidError::sm_err(OP_CANONICAL));
        }

        // In the future, allow user to specify angles
        let domain = anal_uv_domain
            .copied()
            .unwrap_or(Extent2d::from_bounds(0.0, 0.0, 360.0, 360.0));

        let mut torus = Self::new(
            k,
            origin.origin(),
            origin.x_axis(),
            origin.y_axis(),
            domain,
            major_radius,
            minor_radius,
            opts,
        )?;

        // Build NURB; fail cleanly rather than leak a null-NURB object.
        match torus.make_nurb(k) {
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::sm_err(OP_CANONICAL)),
            Ok(()) => {}
        }
        if torus.base.base.gw_nurb().is_none() {
            return Err(SolidError::sm_err(OP_CANONICAL));
        }
        Ok(torus)
    }

    /// Makes the NURB (`SmSurfOfRevolution::MakeNurb`), the polar converter rebuilt
    /// from the torus's iso circles.
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
    /// the torus's iso circles) and rebuilds the generator and the polar converter.
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::reparameterize`].
    pub fn reparameterize(&mut self, k: &Kernel, trim_domain: &Extent2d) -> Result<()> {
        // (GetOrCreateGwNurbPointer: the virtual MakeNurb when missing, status ignored)
        if self.base.base.gw_nurb().is_none() {
            ignore_status(self.make_nurb(k))?;
        }
        let shape = self.shape();
        self.base
            .reparameterize_using(k, trim_domain, &|s, k, p, t| {
                Ok(iso_curve(s, shape, k, p, t, None, false)?.curve)
            })
    }

    /// Sets the STEP domain and rebuilds the NURB and the polar converter
    /// (`SmSurfOfRevolution::AdjustSTEPUVDomain` on a torus: its virtual `MakeNurb`
    /// and `Reparameterize` make the torus's iso circles).
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

    /// The `SmSurfOfRevolution` base object, mutable.
    pub fn base_mut(&mut self) -> &mut SurfOfRevolution {
        &mut self.base
    }

    /// The special-case intersection with `plane` and whether the general intersector
    /// is still needed (`SmSurfOfRevolution::IntersectWithPlane` on a torus): the
    /// inherited body, its sweep iso circles (a plane normal to the axis) made by the
    /// torus's `CreateIsoParametricCurve` override. The plane is taken as without
    /// owner, as for [`SurfOfRevolution::intersect_with_plane`].
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::intersect_with_plane`] and of the torus's iso
    /// circles.
    pub fn intersect_with_plane(
        &self,
        k: &Kernel,
        plane: &Plane,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        self.intersect_with_plane_owned(
            k,
            plane,
            PlaneOwner::NoFace,
            other_uv_domain,
            approx_tol_3d,
        )
    }

    /// [`Torus::intersect_with_plane`] with the plane's owner: its box is expanded by
    /// the tolerance of the plane's face (`GetFace()->GetTolerance()`).
    ///
    /// # Errors
    ///
    /// Those of [`Torus::intersect_with_plane`]; [`SolidError::Unported`]
    /// (`SmFace::GetTolerance`) for a plane with a face whose tolerance is not cached.
    pub(crate) fn intersect_with_plane_owned(
        &self,
        k: &Kernel,
        plane: &Plane,
        plane_owner: PlaneOwner,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        let shape = self.shape();
        self.base.intersect_with_plane_owned_using(
            k,
            plane,
            plane_owner,
            other_uv_domain,
            approx_tol_3d,
            &|s, k, p, t| Ok(iso_curve(s, shape, k, p, t, None, false)?.curve),
        )
    }

    /// [`Torus::intersect_with_plane_owned`] into `out` (reset first). On an error `out`
    /// keeps what was appended before it, as upstream leaves its output arrays.
    pub(crate) fn intersect_with_plane_owned_into(
        &self,
        k: &Kernel,
        plane: &Plane,
        plane_owner: PlaneOwner,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
        out: &mut SurfaceIntersection,
    ) -> Result<bool> {
        let shape = self.shape();
        self.base.intersect_with_plane_owned_using_into(
            k,
            plane,
            plane_owner,
            other_uv_domain,
            approx_tol_3d,
            &|s, k, p, t| Ok(iso_curve(s, shape, k, p, t, None, false)?.curve),
            out,
        )
    }

    /// Trims to the NURB domain `new_nurb_uv_domain` (`SmSurfOfRevolution::TrimWithDomain`
    /// on a torus): the inherited body with the torus's `ConvertUVFromNURBSToSTEP` and iso
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

    /// `RebuildPolarConverter` with the torus's iso circles.
    fn rebuild_polar_converter(&mut self, k: &Kernel) -> Result<()> {
        let shape = self.shape();
        self.base.rebuild_polar_converter(k, &|s, k, p, t| {
            Ok(iso_curve(s, shape, k, p, t, None, false)?.curve)
        })
    }

    const fn shape(&self) -> Shape {
        Shape {
            major_radius: self.major_radius,
            minor_radius: self.minor_radius,
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

    /// The major radius (`GetMajorRadius`).
    #[must_use]
    pub const fn major_radius(&self) -> f64 {
        self.major_radius
    }

    /// The minor radius (`GetMinorRadius`).
    #[must_use]
    pub const fn minor_radius(&self) -> f64 {
        self.minor_radius
    }

    /// Whether the generator's NURB runs against the minor angle (`GetInsideOut`).
    #[must_use]
    pub const fn inside_out(&self) -> bool {
        self.inside_out
    }

    /// `[0, 360] x [0, 360]` (`GetMaxAnalyticDomain`).
    #[must_use]
    pub const fn max_analytic_domain(&self) -> Extent2d {
        Extent2d::from_bounds(0.0, 0.0, 360.0, 360.0)
    }

    /// `SmTorus_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmTorus"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmTorus"
    }

    /// `"SmTorus"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmTorus"
    }

    /// `SmTorus_TYPE` or a type the surface of revolution is (`IsKindOf`).
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
        crate::geom::tol::is_zero(self.major_radius - other.major_radius)
            && crate::geom::tol::is_zero(self.minor_radius - other.minor_radius)
            && self.inside_out == other.inside_out
    }

    // ---- offset ---------------------------------------------------------------------

    /// Not upstream (`SmSurface::OffsetCollapses`, WR-464): whether the offset minor
    /// radius is below `tol`, where [`Self::create_offset_surface`] gives no surface.
    #[must_use]
    pub fn offset_collapses(&self, signed_offset_distance: f64, tol: f64) -> bool {
        let sign = if self.base.swap_uv { -1.0 } else { 1.0 };
        self.minor_radius + sign * signed_offset_distance < tol
    }

    /// The torus offset by `signed_offset_distance` along the NURB normal
    /// (`CreateOffsetSurface`): a copy with the minor radius changed by the distance
    /// (negated with [`SurfOfRevolution::swap_uv`]), the generator scaled about its
    /// centre and every NURB control point scaled from the torus origin and moved back
    /// towards the axis to keep the major radius. `None` when the new minor radius is
    /// below `approx_tol_3d`.
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
        let offset_sign = 1.0 * (if self.base.swap_uv { -1.0 } else { 1.0 });

        // get new radius
        let old_min_radius = self.minor_radius;
        let new_min_radius = self.minor_radius + offset_sign * signed_offset_distance;

        // skip building degenerate or negative radius minor circles
        if new_min_radius < approx_tol_3d {
            return Ok(None);
        }

        // copy the current torus
        let mut off = self.clone();

        // change the minor radius
        off.minor_radius = new_min_radius;

        // scale the NewTorus->GenCurve about circle center
        if off.base.gen_curve.is_some() {
            let circle_origin = gen_circle(off.base.gen_curve.as_ref(), OP_OFFSET)?
                .position
                .origin();
            let scale = new_min_radius / old_min_radius;
            let move_vec = (1.0 - scale) * circle_origin;
            let scale_vec = Vec3::new(scale, scale, scale);
            let mut mv = Axis2Placement::IDENTITY;
            mv.translate(move_vec);
            if let Some(gen) = off.base.gen_curve.as_mut() {
                ignore_status(gen.transform(k, &mv, Some(scale_vec)))?;
            }
        }

        // rebuild the underlying Nurb Surface: just move the Nurb Points
        let origin = self.base.position.origin();
        let z_axis = self.base.position.z_axis();
        let mut first_vec_p_size = 0.0;

        // offset the nurb by combining a scale and a translate
        let scale = new_min_radius / old_min_radius;
        let trans = (1.0 - scale) * self.major_radius;

        let swap_uv = self.base.swap_uv;
        let off_nurb = off
            .base
            .base
            .nurb
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_OFFSET))?;
        let ix = off_nurb.indices();

        // For every m_pNurb ControlPoint - make inner iteration the Sweep direction
        let (i_cnt, j_cnt) = if swap_uv { (ix.n, ix.m) } else { (ix.m, ix.n) };
        for ii in 0..=i_cnt {
            // move every control point on this sweep curve row
            for jj in 0..=j_cnt {
                // get Control Point Euclidean position
                let (a, b) = if swap_uv { (ii, jj) } else { (jj, ii) };
                let cp = off_nurb.cpt_mut(a, b)?;
                let euclid = to_euclid(*cp);

                // get scale and move directions for this ControlPoint
                let vec = Vec3::new(
                    euclid.x - origin.x,
                    euclid.y - origin.y,
                    euclid.z - origin.z,
                );
                let mut vec_p = vec - vec.dot(z_axis) * z_axis;

                // scale VecP so that the translates are larger in the corners
                if jj == 0 {
                    first_vec_p_size = vec_p.length();
                }
                // (upstream's operator/, not the guarded operator/=)
                vec_p = Div::div(vec_p, first_vec_p_size);

                // move the ControlPoint
                cp.x = (origin.x + vec.x * scale + vec_p.x * trans) * cp.w;
                cp.y = (origin.y + vec.y * scale + vec_p.y * trans) * cp.w;
                cp.z = (origin.z + vec.z * scale + vec_p.z * trans) * cp.w;
            }
        }
        Ok(Some(off))
    }

    // ---- parameter conversion -------------------------------------------------------

    /// The NURB parameters of the STEP parameters `step_uv` (`ConvertUVFromSTEPToNURBS`):
    /// the major angle through the polar converter, the minor angle through the
    /// generator circle's conversion, swapped with [`SurfOfRevolution::swap_uv`].
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

        // (Torus GenCurves are centred on 0: the PolarParam is the StepParam)
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
        let maj_r = self.major_radius;
        let min_r = self.minor_radius;
        let min_highest = hu.min(hv);

        // per SMLib practice clamp out of bounds UV points
        if !self.base.anal_uv_domain.contains_point(uv, 0.0) {
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
            r_o + (maj_r + min_r * cos_v) * (cos_u * r_x + sin_u * r_y) + (min_r * sin_v * s_z),
        );

        // 1st U and V derivatives
        if hu >= 1 {
            set(
                hv + 1,
                (maj_r + min_r * cos_v) * (-sin_u * r_x + cos_u * r_y),
            );
        }
        if hv >= 1 {
            set(
                1,
                (min_r * -sin_v) * (cos_u * r_x + sin_u * r_y) + (min_r * cos_v * s_z),
            );
        }

        // cross derivative dUV
        if hu >= 1 && hv >= 1 && min_highest >= 2 {
            set(hv + 2, (min_r * -sin_v) * (-sin_u * r_x + cos_u * r_y));
        }

        //  cross derivative dUVV
        if hu >= 1 && hv >= 2 && min_highest >= 3 {
            set(hv + 3, (min_r * -cos_v) * (-sin_u * r_x + cos_u * r_y));
        }

        // 2nd U and V derivatives
        if hu >= 2 {
            set(
                2 * hv + 2,
                (maj_r + min_r * cos_v) * (-cos_u * r_x - sin_u * r_y),
            );
        }
        if hv >= 2 {
            set(
                2,
                (min_r * -cos_v) * (cos_u * r_x + sin_u * r_y) - (min_r * sin_v * s_z),
            );
        }

        // cross derivative dUUV
        if hu >= 2 && hv >= 1 && min_highest >= 3 {
            set(2 * hv + 3, (min_r * -sin_v) * (-cos_u * r_x - sin_u * r_y));
        }

        // 3rd U and V derivatives
        if hu >= 3 {
            set(
                3 * hv + 3,
                (maj_r + min_r * cos_v) * (sin_u * r_x - cos_u * r_y),
            );
        }
        if hv >= 3 {
            set(
                3,
                (min_r * sin_v) * (cos_u * r_x + sin_u * r_y) - (min_r * cos_v * s_z),
            );
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

    // ---- iso curves -----------------------------------------------------------------

    /// The iso-parametric curve at the NURB parameter `iso_parameter` in `surf_param`'s
    /// direction (`CreateIsoParametricCurve`): the B-spline extraction of the NURB
    /// (trimmed to `domain`) as a circle (`SmCircle`) with that NURB, a minor circle
    /// for constant STEP u and a circle about the axis otherwise, over the STEP
    /// interval or, with `domain`, the angles of the curve's ends. A degenerate
    /// extraction is returned as the B-spline.
    ///
    /// With the opt-in
    /// [`UpstreamFixes::torus_iso_circle_axis_from_midpoint`](crate::kernel::UpstreamFixes::torus_iso_circle_axis_from_midpoint)
    /// (WR-504) a minor circle whose start lies on the axis takes its direction from its
    /// midpoint, as upstream's fallback intends; by default it reuses the start vector.
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

    // ---- recognition ----------------------------------------------------------------

    /// The torus with the parameterisation of `test` when `test` is one
    /// (`IsNurbSurfaceTorus`): a rational surface of degree 2 both ways whose
    /// constant-u and constant-v iso curves are circles ([`test_for_iso_circles`] at
    /// `tolerance_scale * ANALYTIC_TOL_SCALE * SM_EFF_ZERO * (1 + |corner points|)` and
    /// the angle tolerance `SM_EFF_ZERO_SQRT * tolerance_scale`), those in one direction
    /// on parallel planes (not one plane) and those in the other on planes through a
    /// common line (the axis); both radii above `SM_EFF_ZERO`. The torus is built from
    /// the circles found, its generator the minor circle over the NURB's first iso
    /// curve, and takes the NURB domain of `test` (through [`Self::reparameterize`]).
    /// `None` when `test` is not a torus (upstream default scale `1`).
    ///
    /// With the opt-in
    /// [`UpstreamFixes::found_torus_sphere_keep_tested_nurb`](crate::kernel::UpstreamFixes::found_torus_sphere_keep_tested_nurb)
    /// (WR-493) the torus then keeps a copy of `test`'s NURB and rebuilds its analytic
    /// data from it (`SetFromGwNurb`); by default it keeps the NURB made from the
    /// fitted circles, as upstream. With
    /// [`UpstreamFixes::torus_signed_gen_curve_interval`](crate::kernel::UpstreamFixes::torus_signed_gen_curve_interval)
    /// (WR-408) the generator's interval is measured signed, and with
    /// [`UpstreamFixes::torus_major_radius_from_origin`](crate::kernel::UpstreamFixes::torus_major_radius_from_origin)
    /// (WR-501) the major radius is measured from the torus origin.
    ///
    /// # Errors
    ///
    /// Upstream's `SER` statuses (returned as a `TRUE` result without torus): those of
    /// the evaluations, the iso-circle tests, the iso curve and the circle, and of the
    /// constructor; `InvalidInput` when an iso-circle output upstream would read is
    /// missing.
    pub fn is_nurb_surface_torus(
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

        // evaluate Nurb min/max domain points
        let nurb_uv_domain = test.natural_uv_domain();
        let nurb_min_point = test.evaluate_point(nurb_uv_domain.min())?;
        let nurb_max_point = test.evaluate_point(nurb_uv_domain.max())?;

        // scale zero value
        let scaled_zero = tolerance_scale
            * ANALYTIC_TOL_SCALE
            * EFF_ZERO
            * (1.0 + nurb_min_point.max_dimension() + nurb_max_point.max_dimension());
        let ang_tol = EFF_ZERO_SQRT * tolerance_scale;

        // test sequence of V Direction (U Constant) isoCurves for circularity and
        // parallelism
        let iso_u = test_for_iso_circles(k, test, SurfParamType::U, scaled_zero, ang_tol)?;
        // quit when not a torus
        if !iso_u.found_circles {
            return Ok(None);
        }

        // test sequence of U Direction (V Constant) isoCurves for circularity and
        // parallelism
        let iso_v = test_for_iso_circles(k, test, SurfParamType::V, scaled_zero, ang_tol)?;
        // quit when not a torus
        if !iso_v.found_circles {
            return Ok(None);
        }

        // see if we have a U or a V oriented torus
        let is_torus_with_swap_uv = iso_u.parallel_planes && !iso_u.same_plane && iso_v.common_axis;
        let is_torus_no_swap_uv = iso_v.parallel_planes && !iso_v.same_plane && iso_u.common_axis;

        // quit when not a properly oriented torus
        if !is_torus_with_swap_uv && !is_torus_no_swap_uv {
            return Ok(None);
        }

        // locals
        let min_uv = nurb_uv_domain.evaluate(0.0, 0.0);
        let missing = || SolidError::invalid_input(OP_NURB_TORUS, "iso circle output not written");

        // Get torus position and parameters: the frame where all circles are parallel,
        // and the one where all circle planes intersect on a common line
        let (swap_uv, ref_iso, other_iso) = if is_torus_no_swap_uv {
            (false, &iso_v, &iso_u)
        } else {
            (true, &iso_u, &iso_v)
        };
        let ref_frame = ref_iso.reference_frame.ok_or_else(missing)?;
        let other_frame = other_iso.reference_frame.ok_or_else(missing)?;
        let min_rad = other_iso.radius.ok_or_else(missing)?;
        let sweep_ivl = ref_iso.angles.ok_or_else(missing)?;
        let common_point = other_iso.common_line_point.ok_or_else(missing)?;
        let common_vec = other_iso.common_line_vec.ok_or_else(missing)?;

        // get torus origin = MinorCircleCenter projected onto common line
        let torus_origin =
            common_point + common_vec.dot(other_frame.origin() - common_point) * common_vec;

        // get torus orientation and size
        // not upstream (WR-501, #1470, opt-in; source fork@6136a04): the major
        // radius runs from the torus origin, not from the first sweep circle's centre,
        // which lies off the equator when the tube does not start there
        let major_vec = if k.fixes.torus_major_radius_from_origin {
            other_frame.origin() - torus_origin
        } else {
            other_frame.origin() - ref_frame.origin()
        };
        let maj_rad = major_vec.length();
        let torus_x_axis = ref_frame.x_axis(); // the frame where all circles are parallel
        let torus_y_axis = ref_frame.y_axis();
        let torus_z_axis = ref_frame.z_axis();

        // torus must have positive radii
        if min_rad <= EFF_ZERO || maj_rad <= EFF_ZERO {
            return Ok(None);
        }

        // Check GenCurve minor circle rotation direction, its y axis is either +/-
        // sweepCircle z
        let minor_orientation = other_frame.z_axis().cross(major_vec);
        let inside_out = torus_z_axis.dot(minor_orientation) < 0.0;

        // project Nurb Min/Max Point to Torus XY plane to get vec to Center of each
        // GenCurve
        let min_plane_point = point_project_to_plane(nurb_min_point, torus_origin, torus_z_axis);
        let max_plane_point = point_project_to_plane(nurb_max_point, torus_origin, torus_z_axis);
        let start_for_min_angle = (min_plane_point - torus_origin).unitize_or_z();
        let start_for_max_angle = (max_plane_point - torus_origin).unitize_or_z();
        let min_origin = torus_origin + maj_rad * start_for_min_angle;
        let max_origin = torus_origin + maj_rad * start_for_max_angle;

        // Get Vec from Nurb Min/Max points to each genCurve origin
        let vec_min = nurb_min_point - min_origin;
        let vec_max = nurb_max_point - max_origin;

        // get GenCurve min and max angles measured from each GenCurve's origin and start
        // vector
        let (gen_angle_min, gen_angle_max) = if k.fixes.torus_signed_gen_curve_interval {
            // not upstream (WR-408, source issue #1112, opt-in): signed
            // angles; a failed angle is not a torus, as the fork. Zero is SM_EFF_ZERO_DEG
            // (source fork@40344bf, #1471): the ends carry the rounding of their
            // coordinates, so a closed tube's ends differ by ~1e-12 degrees
            match torus_gen_curve_interval(
                [start_for_min_angle, vec_min],
                [start_for_max_angle, vec_max],
                torus_z_axis,
                inside_out,
                EFF_ZERO_DEG,
            ) {
                Ok(ivl) => ivl,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Ok(None),
            }
        } else {
            // (statuses ignored upstream: 0 on failure)
            let mut gen_angle_min =
                ignored(start_for_min_angle.angle_between(vec_min))?.unwrap_or(0.0);
            let mut gen_angle_max =
                ignored(start_for_max_angle.angle_between(vec_max))?.unwrap_or(0.0);
            gen_angle_min = 180.0 * gen_angle_min / PI;
            gen_angle_max = 180.0 * gen_angle_max / PI;

            // snap to ends for small tolerances
            if gen_angle_min.abs() < EFF_ZERO {
                gen_angle_min = 0.0;
            }
            if gen_angle_max.abs() < EFF_ZERO || (gen_angle_max - 360.0).abs() < EFF_ZERO {
                gen_angle_max = 360.0;
            }
            if gen_angle_max < gen_angle_min {
                gen_angle_max += 360.0;
            }
            (gen_angle_min, gen_angle_max)
        };

        // define Torus domain
        let anal_uv_domain = Extent2d::from_bounds(
            sweep_ivl.min(),
            gen_angle_min,
            sweep_ivl.max(),
            gen_angle_max,
        );

        // Get GenCurve Isoparameter Curve
        let gen_curve_iso = test.create_iso_parametric_curve(
            k,
            if swap_uv {
                SurfParamType::V
            } else {
                SurfParamType::U
            },
            if swap_uv { min_uv.y } else { min_uv.x },
            IsoCurveOptions::default(),
        )?;
        let circle = Ellipse::new_circle(
            k,
            min_origin,
            start_for_min_angle,
            torus_z_axis,
            anal_uv_domain.v_interval(),
            min_rad,
            &EllipseOptions {
                dim: 3,
                nurb: Some(&gen_curve_iso.curve),
                inside_out,
            },
        )?;

        // build a new torus with given GenCurve to force parameterization compatibility
        //    constant U Curve orientations,
        //    constant V Curve centers and radius,
        //    and computed domain
        let mut torus = Self::new(
            k,
            torus_origin,
            torus_x_axis,
            torus_y_axis,
            anal_uv_domain,
            maj_rad,
            min_rad,
            TorusOptions {
                swap_uv,
                inside_out,
                circle_nurb: Some(Curve::from(circle)),
            },
        )?;
        // (error recovery: the constructor always stores the given generator)

        // reparameterize the NURB so that AnalyticSurface->NurbDomain ==
        // InputSurface->NurbDomain (status ignored)
        ignore_status(torus.reparameterize(k, &test.natural_uv_domain()))?;

        // not upstream (WR-493, opt-in; the source fork's source fork@7ea6a96):
        // the torus keeps the NURB tested (SetFromGwNurb, whose rebuild's status is
        // ignored)
        if k.fixes.found_torus_sphere_keep_tested_nurb {
            let nurb = test
                .gw_nurb()
                .ok_or_else(|| SolidError::null_pointer(OP_NURB_TORUS))?;
            torus.base.base.nurb = Some(copy_nurb_surface(nurb)).into();
            torus.base.base.nurb_is_borrowed = false;
            ignore_status(torus.rebuild_step_from_nurb_parameters(k))?;
        }

        // (Notify(SM_NO_COPY) copies the test surface's attributes: not modelled)
        Ok(Some(torus))
    }

    // ---- analytic data from the NURB ------------------------------------------------

    /// Rebuilds the generator, radii, inside-out flag, placement, swap flag, STEP
    /// domain and polar converter from the NURB (`RebuildSTEPFromNURBParameters`): the
    /// NURB must be rational of degree 2 both ways with circular iso curves, those in
    /// one direction on parallel planes and those in the other on planes through a
    /// common line (the axis). Nothing changes when the constant-u iso curves are not
    /// circles.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for another degree or a non-rational NURB, iso circles without the
    /// torus orientation or a degenerate generator; `InvalidInput` when an iso-circle
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

        // quit when not a torus (upstream returns FALSE, i.e. SM_SUCCESS)
        if !iso_u.found_circles {
            return Ok(());
        }

        // test sequence of U Direction (V Constant) isoCurves
        let iso_v = test_for_iso_circles(k, srf, SurfParamType::V, scaled_zero, ang_tol)?;

        // see if we have a U or a V oriented torus
        let is_torus_with_swap_uv = iso_u.parallel_planes && iso_v.common_axis;
        let is_torus_no_swap_uv = iso_v.parallel_planes && iso_u.common_axis;

        // quit when not a properly oriented torus
        if !is_torus_with_swap_uv && !is_torus_no_swap_uv {
            return Err(SolidError::sm_err(OP_REBUILD));
        }

        // locals
        let min_uv = nurb_uv_domain.evaluate(0.0, 0.0);
        let missing = || SolidError::invalid_input(OP_REBUILD, "iso circle output not written");

        // Get torus position and parameters
        let (swap_uv, ref_iso, other_iso) = if is_torus_no_swap_uv {
            // frame where all circles are parallel / where all circle planes intersect
            // on a common line
            (false, &iso_v, &iso_u)
        } else {
            (true, &iso_u, &iso_v)
        };
        let mut ref_frame = ref_iso.reference_frame.ok_or_else(missing)?;
        let other_frame = other_iso.reference_frame.ok_or_else(missing)?;
        let min_rad = other_iso.radius.ok_or_else(missing)?;
        let sweep_ivl = ref_iso.angles.ok_or_else(missing)?;
        let common_point = other_iso.common_line_point.ok_or_else(missing)?;
        let common_vec = other_iso.common_line_vec.ok_or_else(missing)?;

        // get torus origin = MinorCircleCenter projected onto common line
        let torus_origin =
            common_point + common_vec.dot(other_frame.origin() - common_point) * common_vec;

        // Install the origin into pRefFrame.  // [B328]
        let _ = ref_frame.set_canonical(torus_origin, ref_frame.x_axis(), ref_frame.y_axis());

        // get torus orientation and size
        let major_vec = other_frame.origin() - ref_frame.origin();
        let maj_rad = major_vec.length();

        // Check GenCurve minor circle rotation direction, its y axis is either +/-
        // sweepCircle z
        let torus_z_axis = ref_frame.z_axis();
        let minor_orientation = other_frame.z_axis().cross(major_vec);
        let inside_out = torus_z_axis.dot(minor_orientation) < 0.0;

        // project Nurb Min/Max Point to Torus XY plane to get vec to Center of each
        // GenCurve
        let min_plane_point = point_project_to_plane(nurb_min_point, torus_origin, torus_z_axis);
        let max_plane_point = point_project_to_plane(nurb_max_point, torus_origin, torus_z_axis);
        let start_for_min_angle = (min_plane_point - torus_origin).unitize_or_z();
        let start_for_max_angle = (max_plane_point - torus_origin).unitize_or_z();
        let min_origin = torus_origin + maj_rad * start_for_min_angle;
        let max_origin = torus_origin + maj_rad * start_for_max_angle;

        // Get Vec from Nurb Min/Max points to each genCurve origin
        let vec_min = nurb_min_point - min_origin;
        let vec_max = nurb_max_point - max_origin;

        // not upstream (WR-408, source issue #1112, opt-in): signed angles
        let (gen_angle_min, gen_angle_max) = if k.fixes.torus_signed_gen_curve_interval {
            torus_gen_curve_interval(
                [start_for_min_angle, vec_min],
                [start_for_max_angle, vec_max],
                torus_z_axis,
                inside_out,
                EFF_ZERO_DEG,
            )?
        } else {
            // get GenCurve min and max angles (statuses ignored upstream: 0 on failure)
            let mut gen_angle_min =
                ignored(start_for_min_angle.angle_between(vec_min))?.unwrap_or(0.0);
            let mut gen_angle_max =
                ignored(start_for_max_angle.angle_between(vec_max))?.unwrap_or(0.0);
            gen_angle_min = 180.0 * gen_angle_min / PI;
            gen_angle_max = 180.0 * gen_angle_max / PI;

            // snap to ends for small tolerances
            if gen_angle_min.abs() < EFF_ZERO_DEG {
                gen_angle_min = 0.0;
            }
            if gen_angle_max.abs() < EFF_ZERO_DEG || (gen_angle_max - 360.0).abs() < EFF_ZERO_DEG {
                gen_angle_max = 360.0;
            }
            if gen_angle_max < gen_angle_min + EFF_ZERO_DEG {
                gen_angle_max += 360.0;
            }

            (gen_angle_min, gen_angle_max)
        };

        // define Torus domain
        let anal_uv_domain = Extent2d::from_bounds(
            sweep_ivl.min(),
            gen_angle_min,
            sweep_ivl.max(),
            gen_angle_max,
        );

        // Get GenCurve Isoparameter Curve
        let gen_curve_iso = srf.create_iso_parametric_curve(
            k,
            if swap_uv {
                SurfParamType::V
            } else {
                SurfParamType::U
            },
            if swap_uv { min_uv.y } else { min_uv.x },
            IsoCurveOptions::default(),
        )?;
        let circle = Ellipse::new_circle(
            k,
            min_origin,
            start_for_min_angle,
            torus_z_axis,
            anal_uv_domain.v_interval(),
            min_rad,
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
        self.major_radius = maj_rad;
        self.minor_radius = min_rad;
        self.inside_out = inside_out;
        // not upstream (WR-500, opt-in): its copies keep the flag
        self.keeps_inside_out |= k.fixes.torus_keeps_inside_out;
        self.base.planar_generator = true;
        self.base.position = ref_frame;
        self.base.swap_uv = swap_uv;
        self.base.anal_uv_domain = anal_uv_domain;

        // Set PolarConverter from Nurb midPoint rotation isoParamCurve (status ignored)
        ignore_status(self.rebuild_polar_converter(k))?;
        Ok(())
    }

    // ---- transform ------------------------------------------------------------------

    /// Scales the radii by the optional uniform `scale`, then transforms the surface of
    /// revolution (`Transform`). The identity placement with no or unit scale changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a non-uniform scale or one below `SM_EFF_ZERO`;
    /// [`SolidError::Unported`] from `SmSurfOfRevolution::Transform` (whose other
    /// statuses upstream ignores), after the radii are scaled.
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
                return Err(SolidError::sm_err(OP_TRANSFORM));
            }
            d_scale = s.x;
        }

        // update cone size
        self.major_radius *= d_scale;
        self.minor_radius *= d_scale;

        // finish the call in the parent function (status ignored upstream), whose
        // polar-converter rebuild reaches the torus's iso circles
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

    /// Reads a torus record (`ReadFromDB`): the major radius, the minor radius and the
    /// inside-out flag (one per line in ASCII files), then the surface of revolution
    /// record. Unreadable ASCII values keep their defaults.
    ///
    /// # Errors
    ///
    /// A failed binary read; those of the surface of revolution record
    /// ([`SurfOfRevolution::read_from_db`]).
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        // init output object
        let mut torus = Self::empty();
        // not upstream (WR-500, opt-in): its copies keep the flag read
        torus.keeps_inside_out = k.fixes.torus_keeps_inside_out;

        if db.file_type() == FileType::Ascii {
            if let Some(s) = db.in_stream_mut() {
                extract(s, &mut torus.major_radius);
            }
            db.go_to_next_line();
            if let Some(s) = db.in_stream_mut() {
                extract(s, &mut torus.minor_radius);
            }
            db.go_to_next_line();
            if let Some(v) = db.in_stream_mut().and_then(InputStream::extract_int) {
                torus.inside_out = v != 0;
            }
            db.go_to_next_line();
        } else {
            torus.major_radius = db.read_double()?;
            torus.minor_radius = db.read_double()?;
            torus.inside_out = db.read_boolean()? != 0;
        }

        // read the parent object
        torus.base = SurfOfRevolution::read_from_db(k, db, version)?;

        // all done
        Ok(torus)
    }
}

/// The torus is a surface of revolution (an operand of the inherited
/// intersections, converting its domains through its own `ConvertUVFromNURBSToSTEP`).
impl Revolved for Torus {
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

/// `SmTorus::CreateIsoParametricCurve` of the surface of revolution `rev` of a torus
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
    let torus_origin = rev.position.origin();
    let torus_x_axis = rev.position.x_axis();
    let torus_y_axis = rev.position.y_axis();
    let torus_z_axis = rev.position.z_axis();

    // Get IsoCurve Nurb endPoints (statuses ignored upstream)
    let nurb_ivl = iso.curve.natural_interval();
    let nurb_min_point = iso.curve.evaluate_point(nurb_ivl.min())?;
    let nurb_max_point = iso.curve.evaluate_point(nurb_ivl.max())?;
    let nurb_min_vec = nurb_min_point - torus_origin;

    // get iso circle parameters
    let (circle_center, x_axis, y_axis, radius, inside_out, anal_domain) = if step_param
        == SurfParamType::U
    {
        // return minor isoCurve circle
        let mut x_axis = torus_z_axis.cross(nurb_min_vec.cross(torus_z_axis));
        let mut length = x_axis.length();
        if length < EFF_ZERO_SQRT {
            if k.fixes.torus_iso_circle_axis_from_midpoint {
                // WR-504 (#1514, opt-in): the direction comes from the mid point, as the
                // fallback intends; it is read now, so a failed evaluation propagates (the
                // fork's SER, smlib@672a437ee; v1.0.1 ignores it)
                let mid_point = iso.curve.evaluate_point(nurb_ivl.evaluate(0.5))?;
                let mid_vec = mid_point - torus_origin;
                x_axis = torus_z_axis.cross(mid_vec.cross(torus_z_axis));
            } else {
                // (the mid point is evaluated but not used upstream; the start vector is
                // used again, which is zero on the axis)
                let _ = ignored(iso.curve.evaluate_point(nurb_ivl.evaluate(0.5)))?;
                x_axis = torus_z_axis.cross(nurb_min_vec).cross(torus_z_axis);
            }
            length = x_axis.length();
        }
        x_axis = Div::div(x_axis, length);
        (
            rev.position.origin() + shape.major_radius * x_axis,
            x_axis,
            torus_z_axis,
            shape.minor_radius,
            shape.inside_out,
            rev.anal_uv_domain.v_interval(),
        )
    } else {
        // return sweep isoCurve circle
        let circle_center = rev.position.origin() + nurb_min_vec.dot(torus_z_axis) * torus_z_axis;
        (
            circle_center,
            torus_x_axis,
            torus_y_axis,
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
        let circle_min_vec = nurb_min_point - circle_center;
        let circle_max_vec = nurb_max_point - circle_center;
        let min_ang_rad =
            ignored(circle_z_axis.ccw_angle_between(x_axis, circle_min_vec))?.unwrap_or(0.0);
        let max_ang_rad =
            ignored(circle_z_axis.ccw_angle_between(x_axis, circle_max_vec))?.unwrap_or(0.0);
        let min_ang_deg = anal_domain
            .contains_periodic_value(min_ang_rad * 180.0 / PI, 360.0, -1.0)
            .periodic_value;
        let mut max_ang_deg = anal_domain
            .contains_periodic_value(max_ang_rad * 180.0 / PI, 360.0, -1.0)
            .periodic_value;

        // when both values are on a closed interval seam
        if anal_domain.is_closed(360.0)
            && anal_domain
                .is_value_on_periodic_boundary(min_ang_deg, 360.0, EFF_ZERO)
                .0
            && anal_domain
                .is_value_on_periodic_boundary(max_ang_deg, 360.0, EFF_ZERO)
                .0
        {
            // set interval to whole domain
            anal_domain
        } else {
            // set isoCurve analytic domain accounting for m_binside reversals
            if (max_ang_deg - min_ang_deg).abs() < anal_domain.length() - EFF_ZERO {
                max_ang_deg = min_ang_deg + anal_domain.length();
            }
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
/// torus (the trim's `UpdateAnalyticalDomain` reaches it through its base).
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

/// Not upstream (WR-408, source issue #1112; the source fork's
/// `sm_TorusGenCurveInterval`): the generating circle's analytic interval, in degrees,
/// from the NURB's first and last points, each given as `[x, vec]`: its circle's unit X
/// axis (from the torus axis out to the circle's centre) and the vector from that centre
/// to the point. Each angle is measured counter-clockwise from X toward the torus's
/// `z_axis` (about `x * z_axis`), as `EvaluateSTEP` measures V, over a whole turn; an
/// inside-out torus runs its NURB the other way round, so its last point starts the
/// interval. Angles within `zero_deg` of 0 snap to it, and the end is lifted a turn
/// when it is not above the start by `zero_deg`. Both callers pass `SM_EFF_ZERO_DEG`
/// (the rebuild, and [`Torus::is_nurb_surface_torus`] as of source fork@40344bf,
/// #1471). Upstream measured both with the unsigned `AngleBetween`, which mirrors an
/// interval below the equator or past 180 degrees (-90..90 became 90..450, the inner
/// half of the tube).
///
/// # Errors
///
/// Those of `CCWAngleBetween`.
fn torus_gen_curve_interval(
    first: [Vec3; 2],
    last: [Vec3; 2],
    z_axis: Vec3,
    inside_out: bool,
    zero_deg: f64,
) -> Result<(f64, f64)> {
    let [x_first, vec_first] = first;
    let [x_last, vec_last] = last;
    let first_rad = x_first
        .cross(z_axis)
        .ccw_angle_between(x_first, vec_first)?;
    let last_rad = x_last.cross(z_axis).ccw_angle_between(x_last, vec_last)?;
    let first_deg = 180.0 * first_rad / PI;
    let last_deg = 180.0 * last_rad / PI;
    let (mut low, mut high) = if inside_out {
        (last_deg, first_deg)
    } else {
        (first_deg, last_deg)
    };
    if low.abs() < zero_deg {
        low = 0.0;
    }
    if high.abs() < zero_deg {
        high = 0.0;
    }
    if high < low + zero_deg {
        high += 360.0;
    }
    Ok((low, high))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(k: &Kernel, swap: bool, inside_out: bool) -> Torus {
        let frame = Axis2Placement::new(
            Vec3::new(1.0, 2.0, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        Torus::create_canonical(
            k,
            &frame,
            3.0,
            1.0,
            TorusOptions {
                swap_uv: swap,
                inside_out,
                circle_nurb: None,
            },
            None,
        )
        .expect("torus")
    }

    #[test]
    fn iso_circle_full_turn_domains_and_conversions() {
        let k = Kernel::default();
        let frames = [
            Axis2Placement::IDENTITY,
            Axis2Placement::new(
                Vec3::new(1.0, 2.0, 0.5),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ),
        ];
        for frame in &frames {
            for shift in [0.0, 45.0] {
                let analytic = Extent2d::from_bounds(shift, shift, shift + 360.0, shift + 360.0);
                for swap in [false, true] {
                    for inside_out in [false, true] {
                        let mut t = Torus::create_canonical(
                            &k,
                            frame,
                            3.0,
                            1.0,
                            TorusOptions {
                                swap_uv: swap,
                                inside_out,
                                circle_nurb: None,
                            },
                            Some(&analytic),
                        )
                        .expect("torus");

                        // The constructor orients the generator but
                        // deliberately leaves the torus flag false. Exercise
                        // consistently oriented iso circles here.
                        t.inside_out = inside_out;
                        let domain = t.base.base.natural_uv_domain();
                        for minor in [true, false] {
                            let param = if minor == swap {
                                SurfParamType::V
                            } else {
                                SurfParamType::U
                            };
                            let fixed = if param == SurfParamType::U {
                                domain.u_interval().evaluate(0.25)
                            } else {
                                domain.v_interval().evaluate(0.25)
                            };
                            for explicit in [false, true] {
                                let iso = t
                                    .create_iso_parametric_curve(
                                        &k,
                                        param,
                                        fixed,
                                        explicit.then_some(&domain),
                                        false,
                                    )
                                    .expect("iso circle");
                                let CurveKind::Ellipse(circle) = iso.curve.kind() else {
                                    panic!("expected circle");
                                };
                                let interval = circle.step_interval();
                                assert!(
                                    (interval.min() - shift).abs() < 1e-9
                                        && (interval.max() - (shift + 360.0)).abs() < 1e-9,
                                    "minor={minor} explicit={explicit} swap={swap} inside_out={inside_out}: {interval:?}"
                                );
                                for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
                                    let step = interval.evaluate(fraction);
                                    let nurb = circle
                                        .convert_t_from_step_to_nurbs(&k, step)
                                        .expect("STEP to NURBS");
                                    assert!(nurb.is_finite());
                                    let back = circle
                                        .convert_t_from_nurbs_to_step(&k, nurb)
                                        .expect("NURBS to STEP");
                                    assert!(back.is_finite());
                                    let p =
                                        circle.base().evaluate_point(nurb).expect("NURBS point");
                                    let requested = circle
                                        .evaluate_step_point(step)
                                        .expect("requested STEP point");
                                    let returned = circle
                                        .evaluate_step_point(back)
                                        .expect("returned STEP point");
                                    assert!((p - requested).length() < 1e-9);
                                    assert!((p - returned).length() < 1e-9);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn step_and_nurb_points_agree() {
        let k = Kernel::default();
        for swap in [false, true] {
            for inside_out in [false, true] {
                let t = canonical(&k, swap, inside_out);
                assert!(t.base.have_polar_conversion());
                for (u, v) in [(10.0, 20.0), (100.0, 250.0), (359.0, 1.0), (0.0, 0.0)] {
                    let step = Point2::new(u, v);
                    let p = t.evaluate_step_point(step).expect("eval");
                    let nurb = t.convert_uv_from_step_to_nurbs(&k, step).expect("to nurbs");
                    let q = t.base.base.evaluate_point(nurb).expect("nurb eval");
                    assert!((p - q).length() < 1e-9, "{swap} {inside_out} {u} {v}");
                    let back = t.convert_uv_from_nurbs_to_step(&k, nurb).expect("to step");
                    let r = t.evaluate_step_point(back).expect("eval back");
                    assert!((p - r).length() < 1e-9);
                }
            }
        }
    }

    #[test]
    fn trim_with_domain_keeps_the_shape() {
        let k = Kernel::default();
        for swap in [false, true] {
            let mut t = canonical(&k, swap, false);
            let d = t.base.base.natural_uv_domain();
            let (lo, hi) = (d.min(), d.max());
            let trim = Extent2d::from_bounds(
                lo.x + 0.25 * (hi.x - lo.x),
                lo.y + 0.125 * (hi.y - lo.y),
                lo.x + 0.75 * (hi.x - lo.x),
                lo.y + 0.5 * (hi.y - lo.y),
            );
            t.trim_with_domain(&k, &trim).expect("trim");
            let nd = t.base.base.natural_uv_domain();
            assert!(nd.is_contained_by(&trim, 1e-12) && trim.is_contained_by(&nd, 1e-12));
            let p = t.base.base.evaluate_point(trim.min()).expect("eval");
            let back = t
                .convert_uv_from_nurbs_to_step(&k, trim.min())
                .expect("to step");
            let q = t.evaluate_step_point(back).expect("eval step");
            assert!((p - q).length() < 1e-9, "{swap}");
            let big = Extent2d::from_bounds(lo.x - 1.0, lo.y, hi.x, hi.y);
            assert!(t.trim_with_domain(&k, &big).is_err());
        }
    }

    #[test]
    fn rejects_degenerate_radii() {
        let k = Kernel::default();
        let frame = Axis2Placement::IDENTITY;
        for (a, b) in [(0.0, 1.0), (1.0, 0.0), (f64::NAN, 1.0), (-1.0, 0.5)] {
            let r = Torus::create_canonical(&k, &frame, a, b, TorusOptions::default(), None);
            assert!(matches!(r, Err(SolidError::Upstream { .. })));
        }
    }

    #[test]
    fn offset_keeps_major_radius() {
        let k = Kernel::default();
        let t = canonical(&k, false, false);
        let off = t
            .create_offset_surface(&k, 0.25, 1e-6)
            .expect("offset")
            .expect("surface");
        assert!((off.minor_radius() - 1.25).abs() < 1e-15);
        let nurb = off
            .convert_uv_from_step_to_nurbs(&k, Point2::new(30.0, 60.0))
            .expect("uv");
        let p = off.base.base.evaluate_point(nurb).expect("eval");
        let q = off
            .evaluate_step_point(Point2::new(30.0, 60.0))
            .expect("step");
        assert!((p - q).length() < 1e-9);
        assert!(t
            .create_offset_surface(&k, -2.0, 1e-6)
            .expect("offset")
            .is_none());
    }

    #[test]
    fn rebuild_recovers_the_torus() {
        let k = Kernel::default();
        let t = canonical(&k, false, false);
        let mut r = t.clone();
        r.rebuild_step_from_nurb_parameters(&k).expect("rebuild");
        assert!((r.major_radius() - 3.0).abs() < 1e-9);
        assert!((r.minor_radius() - 1.0).abs() < 1e-9);
        assert!(t.approx_eq(&r));
    }

    #[test]
    fn read_reaches_the_base_record() {
        let mut db = DatabaseReader::new();
        let r = Torus::read_from_db(&Kernel::default(), &mut db, 0);
        assert!(r.is_err());
    }

    /// A quarter turn of a torus whose generating interval runs below its equator
    /// (-90 to 90 degrees, the outer half of the tube), rebuilt from its NURB as
    /// `SmFace::SwapUV` does: with the opt-in the interval is the outer half again (270 to
    /// 450, the same arc); upstream's unsigned angles give 90 to 450, the inner half
    /// (WR-408).
    #[test]
    fn issue_1112_rebuilt_torus_keeps_an_interval_below_its_equator() {
        let mut k = Kernel::default();
        k.fixes.torus_signed_gen_curve_interval = true;
        let frame = Axis2Placement::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let t = Torus::create_canonical(
            &k,
            &frame,
            14.0,
            7.0,
            TorusOptions::default(),
            Some(&Extent2d::from_bounds(0.0, -90.0, 90.0, 90.0)),
        )
        .unwrap();
        let mut r = t.clone();
        r.rebuild_step_from_nurb_parameters(&k).expect("rebuild");
        let v = r.base.anal_uv_domain.v_interval();
        assert!((v.length() - 180.0).abs() < 1e-9, "v {v:?}");
        let turn = (v.min() - 270.0).rem_euclid(360.0);
        assert!(turn < 1e-9 || turn > 360.0 - 1e-9, "v {v:?}");
        // the same points as the original's
        for (u, f) in [(10.0, 0.25), (45.0, 0.5), (80.0, 0.75)] {
            let p = t
                .evaluate_step_point(Point2::new(u, -90.0 + 180.0 * f))
                .unwrap();
            let q = r
                .evaluate_step_point(Point2::new(u, v.min() + v.length() * f))
                .unwrap();
            assert!((p - q).length() < 1e-9, "{p:?} {q:?}");
        }
    }

    /// A torus record read with its generator flagged non-planar takes the inherited
    /// general-solver branch of `STEPInversion`, converting through the torus's own
    /// parameter conversions: it finds the parameters the analytic branch finds.
    #[test]
    fn non_planar_flag_step_inversion_converts_through_the_torus() {
        use crate::surface::surface::Surface;
        let k = Kernel::default();
        for swap in [false, true] {
            for inside_out in [false, true] {
                let planar = canonical(&k, swap, inside_out);
                let mut non_planar = planar.clone();
                non_planar.base.planar_generator = false;
                let dom = planar.base.anal_uv_domain;
                let (u, v) = (dom.u_interval(), dom.v_interval());
                let sp = Surface::from(planar.clone());
                let sn = Surface::from(non_planar);
                for (fu, fv) in [(0.3, 0.4), (0.7, 0.15), (0.55, 0.8)] {
                    let step = Point2::new(
                        u.min() + fu * (u.max() - u.min()),
                        v.min() + fv * (v.max() - v.min()),
                    );
                    let p = planar.evaluate_step_point(step).unwrap();
                    let (a, la) = sp.step_inversion(&k, &dom, p, 1e-7, None).unwrap();
                    let (b, lb) = sn.step_inversion(&k, &dom, p, 1e-7, None).unwrap();
                    assert!((a - step).length() < 1e-6, "{a:?} {step:?}");
                    assert!(
                        (a - b).length() < 1e-6,
                        "{a:?} {b:?} swap={swap} io={inside_out}"
                    );
                    assert_eq!(la, lb);
                }
            }
        }
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

    fn plain(t: &Torus) -> BSplineSurface {
        BSplineSurface::from_gw_nurb(t.base().base().gw_nurb().expect("nurb"))
    }

    /// source issue #1433: the torus R 10, r 3 is found in its NURB form;
    /// with `found_torus_sphere_keep_tested_nurb` it keeps exactly that NURB.
    #[test]
    fn issue_1433_torus_found_in_its_nurb_keeps_it() {
        let mut k = Kernel::default();
        let t = Torus::create_canonical(
            &k,
            &Axis2Placement::default(),
            10.0,
            3.0,
            TorusOptions::default(),
            None,
        )
        .unwrap();
        let nurbs = plain(&t);

        // upstream: found, with the NURB made from the fitted circles
        let found = Torus::is_nurb_surface_torus(&k, &nurbs, 1.0)
            .unwrap()
            .expect("a torus");
        assert!((found.major_radius() - 10.0).abs() < 1e-9);
        assert!((found.minor_radius() - 3.0).abs() < 1e-9);

        k.fixes.found_torus_sphere_keep_tested_nurb = true;
        let found = Torus::is_nurb_surface_torus(&k, &nurbs, 1.0)
            .unwrap()
            .expect("a torus");
        assert!(same_nurb(found.base().base(), &nurbs));
        assert!((found.major_radius() - 10.0).abs() < 1e-9);
        assert!((found.minor_radius() - 3.0).abs() < 1e-9);
        let z = found.base().position().z_axis();
        assert!(z.is_parallel_to(Vec3::new(0.0, 0.0, 1.0), 1e-9), "{z:?}");
        let o = found.base().position().origin();
        assert!(o.length() < 1e-9, "{o:?}");
    }

    /// Partial tori (swapped, inside out, below the equator, scaled, tiny) are found
    /// with the flags on and keep their NURB; a perturbed NURB is not a torus.
    #[test]
    fn torus_recognition_partial_scaled_and_non_matching() {
        let mut k = Kernel::default();
        k.fixes.found_torus_sphere_keep_tested_nurb = true;
        k.fixes.torus_signed_gen_curve_interval = true;
        let frame = Axis2Placement::new(
            Vec3::new(1.0, 2.0, 0.5),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        );
        for (scale, dom) in [
            (1.0, Extent2d::from_bounds(10.0, 0.0, 100.0, 90.0)),
            (1.0, Extent2d::from_bounds(0.0, -90.0, 360.0, 90.0)),
            (1e3, Extent2d::from_bounds(0.0, 30.0, 270.0, 300.0)),
            (1e-3, Extent2d::from_bounds(45.0, 0.0, 135.0, 360.0)),
        ] {
            for (swap, inside) in [(false, false), (true, false), (false, true), (true, true)] {
                let t = Torus::create_canonical(
                    &k,
                    &frame,
                    10.0 * scale,
                    3.0 * scale,
                    TorusOptions {
                        swap_uv: swap,
                        inside_out: inside,
                        circle_nurb: None,
                    },
                    Some(&dom),
                )
                .unwrap();
                let nurbs = plain(&t);
                let found = Torus::is_nurb_surface_torus(&k, &nurbs, 1.0)
                    .unwrap()
                    .expect("a torus");
                assert!(same_nurb(found.base().base(), &nurbs));
                assert!((found.major_radius() - 10.0 * scale).abs() < 1e-9 * scale);
                assert!((found.minor_radius() - 3.0 * scale).abs() < 1e-9 * scale);
                // the found torus is the tested surface
                let d = nurbs.natural_uv_domain();
                for (a, b) in [(0.25, 0.5), (0.5, 0.75), (0.9, 0.1)] {
                    let uv = d.evaluate(a, b);
                    let p = nurbs.evaluate_point(uv).unwrap();
                    let step = found.convert_uv_from_nurbs_to_step(&k, uv).unwrap();
                    let q = found.evaluate_step_point(step).unwrap();
                    assert!((p - q).length() < 1e-9 * scale, "{p:?} {q:?}");
                }
            }
        }

        // (a sphere's NURB passes upstream's torus test too, its meridians' planes
        // through the axis: the C++ comparison covers it in the surface.torus difftest)

        let t = canonical(&k, false, false);
        let mut bent = plain(&t);
        let p = bent.get_or_create_gw_nurb().unwrap().cpt_mut(2, 3).unwrap();
        p.z += 0.05;
        assert!(Torus::is_nurb_surface_torus(&k, &bent, 1.0)
            .unwrap()
            .is_none());
    }

    /// source issue #1470 and #1471 (WR-501, WR-408): a torus R 13.1934, r 1.25,
    /// 80 mm out, whose NURB runs once round its tube from the bottom (-90 to 270
    /// degrees). Its first sweep circle sits at the bottom, off the equator, so upstream
    /// measures R as sqrt(R^2 + r^2) (13.2525); with `torus_major_radius_from_origin` R is
    /// measured from the torus origin. The two tube ends differ by rounding only, which
    /// `torus_signed_gen_curve_interval` must take for a full turn, not a sliver.
    #[test]
    fn issue_1470_1471_tube_turn_from_bottom_is_found_with_its_radii() {
        let (big, small) = (13.1934, 1.25);
        let frame = Axis2Placement::new(
            Vec3::new(80.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let dom = Extent2d::from_bounds(10.0, -90.0, 100.0, 270.0);
        for swap in [false, true] {
            let t = Torus::create_canonical(
                &Kernel::default(),
                &frame,
                big,
                small,
                TorusOptions {
                    swap_uv: swap,
                    inside_out: false,
                    circle_nurb: None,
                },
                Some(&dom),
            )
            .unwrap();
            let nurbs = plain(&t);

            let mut k = Kernel::default();
            k.fixes.torus_major_radius_from_origin = true;
            k.fixes.torus_signed_gen_curve_interval = true;
            let found = Torus::is_nurb_surface_torus(&k, &nurbs, 1.0)
                .unwrap()
                .expect("a torus");
            assert!(
                (found.major_radius() - big).abs() < 1e-9 * big,
                "swap {swap}: major {}",
                found.major_radius()
            );
            assert!(
                (found.minor_radius() - small).abs() < 1e-9 * small,
                "swap {swap}: minor {}",
                found.minor_radius()
            );
            let v = found.base.anal_uv_domain.v_interval();
            assert!(
                (v.min() + 90.0).abs() < 1e-9 && (v.length() - 360.0).abs() < 1e-9,
                "swap {swap}: tube interval {v:?}"
            );

            // flags off: upstream's R from the first sweep circle, sqrt(R^2 + (r sin -90)^2)
            let off = Torus::is_nurb_surface_torus(&Kernel::default(), &nurbs, 1.0)
                .unwrap()
                .expect("a torus");
            let upstream_major = (big * big + small * small).sqrt();
            assert!(
                (off.major_radius() - upstream_major).abs() < 1e-9 * upstream_major,
                "swap {swap}: upstream major {}",
                off.major_radius()
            );
        }
    }

    /// source issue #1514 (WR-504; source fork#40): a horn torus (R 5,
    /// r 5) whose tube starts at 180 degrees touches the axis at the torus origin, so each
    /// minor iso circle starts there. `CreateIsoParametricCurve` finds no direction from
    /// the start point and falls back to the midpoint; with
    /// `torus_iso_circle_axis_from_midpoint` the circle gets that direction and its
    /// analytic domain, and it starts at the origin.
    #[test]
    fn issue_1514_horn_torus_minor_iso_circle_takes_direction_from_midpoint() {
        let frame = Axis2Placement::IDENTITY;
        let dom = Extent2d::from_bounds(0.0, 180.0, 360.0, 540.0);
        let t = Torus::create_canonical(
            &Kernel::default(),
            &frame,
            5.0,
            5.0,
            TorusOptions {
                swap_uv: false,
                inside_out: false,
                circle_nurb: None,
            },
            Some(&dom),
        )
        .expect("horn torus");
        let domain = t.base.base.natural_uv_domain();
        let fixed = domain.u_interval().evaluate(0.25);

        let mut k = Kernel::default();
        k.fixes.torus_iso_circle_axis_from_midpoint = true;
        let iso = t
            .create_iso_parametric_curve(&k, SurfParamType::U, fixed, None, false)
            .expect("minor iso circle");
        let CurveKind::Ellipse(circle) = iso.curve.kind() else {
            panic!("expected a circle");
        };
        let x = circle.position().x_axis();
        assert!((x.length() - 1.0).abs() < 1e-9, "x axis not unit: {x:?}");
        assert!(
            (circle.x_radius() - 5.0).abs() < 1e-9,
            "radius {}",
            circle.x_radius()
        );
        assert!(circle.is_analytic(), "no analytic domain");
        let ivl = circle.step_interval();
        assert!(
            (ivl.length() - 360.0).abs() < 1e-9,
            "analytic domain {ivl:?}"
        );
        let start = circle.evaluate_step_point(ivl.min()).expect("start point");
        assert!(
            (start - frame.origin()).length() < 1e-9,
            "start {start:?} is not the torus origin"
        );

        // flag off (upstream): the start vector is zero on the axis, so the circle has
        // no direction
        let off = t
            .create_iso_parametric_curve(&Kernel::default(), SurfParamType::U, fixed, None, false)
            .expect("minor iso circle");
        let CurveKind::Ellipse(off_circle) = off.curve.kind() else {
            panic!("expected a circle");
        };
        assert!(
            !off_circle.position().x_axis().x.is_finite(),
            "upstream direction {:?}",
            off_circle.position().x_axis()
        );
    }
}
