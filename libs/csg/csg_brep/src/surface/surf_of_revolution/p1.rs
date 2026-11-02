//! `SmSurfOfRevolution`, part 1: the surface object ([`SurfOfRevolution`]) with its
//! construction (co-ported `CreateCanonical` and `MakeNurb`), equality, the polar
//! converter rebuild, the iso-parametric curves (circles in the sweep direction),
//! directrices and generators, the NURB to STEP parameter conversion, STEP
//! evaluation, the plane intersection's special cases, the start-plane transform,
//! the small queries, and the iso-circle test of B-spline surfaces
//! ([`test_for_iso_circles`]).
//!
//! Upstream: `source/SMLib/src/SmSurfOfRevolution.cpp`,
//! `source/SMLib/inc/SmSurfOfRevolution.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`SurfOfRevolution`], [`SurfOfRevolution::new`]   | `SmSurfOfRevolution`, `SmSurfOfRevolution(pGenCurve, crOrigin, crXAxis, crYAxis, crAnalUVDomain, bSwapUV)` |
//! | [`SurfOfRevolution::empty`]                       | `SmSurfOfRevolution()`                           |
//! | `Clone`                                           | `SmSurfOfRevolution(const SmSurfOfRevolution&)`  |
//! | `Drop` (implicit)                                 | `~SmSurfOfRevolution`                            |
//! | [`SurfOfRevolution::approx_eq`]                   | `operator==`                                     |
//! | [`SurfOfRevolution::create_canonical`]            | `CreateCanonical` (co-ported from `surface.surf_of_revolution_p2`) |
//! | [`SurfOfRevolution::make_nurb`], [`SurfOfRevolution::make_nurb_with_sweep_params`] | `MakeNurb`, `MakeNurbWithSweepParams` (co-ported from `_p2`, `_p3`) |
//! | `rebuild_polar_converter` (crate)                 | `RebuildPolarConverter`                          |
//! | [`SurfOfRevolution::max_analytic_domain`]         | `GetMaxAnalyticDomain`                           |
//! | [`SurfOfRevolution::gen_dir_param_extent`]        | `GetGenDirParamExtent`                           |
//! | [`SurfOfRevolution::create_iso_parametric_curve`], [`RevolutionIsoCurve`] | `CreateIsoParametricCurve`     |
//! | [`SurfOfRevolution::create_directrix_at_generator_param`], [`DirectrixAtGeneratorParam`] | `CreateDirectrixAtGeneratorParam` |
//! | [`SurfOfRevolution::create_generator_from_angle`] | `CreateGeneratorFromAngle`                       |
//! | [`SurfOfRevolution::convert_uv_from_nurbs_to_step`] | `ConvertUVFromNURBSToSTEP`                     |
//! | [`SurfOfRevolution::evaluate_step`], [`SurfOfRevolution::evaluate_step_point`] | `EvaluateSTEP`, `EvaluateSTEPPoint` |
//! | [`SurfOfRevolution::intersect_with_plane`]        | `IntersectWithPlane`                             |
//! | [`SurfOfRevolution::is_bounded`]                  | `IsBounded`                                      |
//! | [`SurfOfRevolution::transform_point_to_start_plane`] | `TransformPointToStartPlane`                  |
//! | [`SurfOfRevolution::is_kind_of`], [`SurfOfRevolution::TYPE`], [`SurfOfRevolution::type_code`], [`SurfOfRevolution::type_string`], [`SurfOfRevolution::class_string`] | `IsKindOf`, `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`SurfOfRevolution::swap_uv`], [`SurfOfRevolution::step_uv_domain`], [`SurfOfRevolution::start_angle_deg`], [`SurfOfRevolution::end_angle_deg`], [`SurfOfRevolution::gen_curve`], [`SurfOfRevolution::position`], [`SurfOfRevolution::have_polar_conversion`] | `GetSwapUV`, `GetSTEPUVDomain`, `GetStartAngleDeg`, `GetEndAngleDeg`, `GetGenCurve`, `GetPosition`, `HavePolarConversion` (inline) |
//! | [`SurfOfRevolution::set_context`], [`SurfOfRevolution::toggle_swap_uv_bit`], [`SurfOfRevolution::set_out_of_bounds_enabled`] | `SetContext`, `ToggleSwapUVBit`, `SetOutOfBoundsEnabled` (inline) |
//! | [`test_for_iso_circles`], [`IsoCircles`]          | `sm_TestForIsoCircles`                           |
//!
//! Deviations:
//! - **Derived class.** `SmSurfOfRevolution` derives from `SmBSplineSurface`; the base
//!   object is the [`SurfOfRevolution::base`] field. The surface joins
//!   [`crate::surface::surface::SurfaceKind`] with the units porting its remaining
//!   virtual overrides (`_p2`, `_p3`). The NURB and polar-converter rebuilds reach
//!   `CreateIsoParametricCurve` through a `SweepIsoFn` so that a derived class
//!   (`SmTorus`, [`crate::surface::torus`]; `SmCone`, [`crate::surface::cone`])
//!   supplies its override, as upstream's virtual call does.
//! - **Generator.** `m_pGenCurve` is a [`Curve`] of a kind derived from
//!   `SmBSplineCurve` (B-spline, ellipse, line, hyperbola, parabola); the other kinds
//!   are rejected (`InvalidInput`) where upstream casts. It is only `None` for the
//!   empty I/O surface. Its owner is not set: a free surface has no
//!   [`crate::geom::object::ObjectRef`] to give it. Upstream's owner-driven step in the
//!   generator's `MakeNurb` (reparameterising an analytic generator to
//!   `GetGenDirParamExtent`) is therefore not applied when `MakeNurbWithSweepParams`
//!   adjusts the generator's STEP interval (only when the surface's V domain differs
//!   from it; never through [`SurfOfRevolution::create_canonical`]).
//! - **Polar converter.** Upstream's converter owns the sweep circle it was set up
//!   with (`bIsBorrowed = FALSE`); here that circle is the `polar_curve` field and the
//!   table is set up on it with the converter of the circle's own table left alone,
//!   as upstream. `operator==` compares the tables and, both being present, the
//!   circles.
//! - **Out-of-bounds evaluation.** `SmTemporaryChangeValue` gives the generator the
//!   surface's out-of-bounds flag for the evaluation; only a B-spline generator reads
//!   it (the analytic STEP evaluators ignore it), so only that kind is copied with the
//!   flag changed.
//! - **Arguments.** Out-params become return values; `SmContext` arguments and the
//!   arguments upstream marks `NotUsed` (`CreateIsoParametricCurve`'s approximation
//!   tolerance, `EvaluateSTEP`'s side flags, `IntersectWithPlane`'s UV domain and
//!   angle tolerance) are dropped. `EvaluateSTEP` entries it does not write are `0`
//!   (upstream leaves the caller's values). Unequal STEP derivative orders retain
//!   release C++ behavior despite its equal-order assertion: unevaluated generator
//!   derivatives keep `SmVector3d`'s default `UNDEF` values. Boundary iso curves
//!   require the sweep-only (`hv = 0`) case, whose outputs are fully evaluated.
//! - **Statuses.** Calls whose status upstream ignores keep upstream's values, except
//!   that [`SolidError::Unported`] always propagates and a failure whose output
//!   upstream would read uninitialised (a failed generator or surface evaluation, a
//!   failed iso curve in `RebuildPolarConverter`) is returned.
//! - **`RebuildPolarConverter`** makes a missing NURB first, as its upstream
//!   `GetNaturalUVDomain` does (a lazy surface that is transformed or edited gets its
//!   NURB there); called from `MakeNurbWithSweepParams` it does not, which bounds the
//!   recursion upstream would enter if that NURB were missing.
//! - **Unported callees.** `ConvertUVFromNURBSToSTEP` without exact polar conversion
//!   needs `SmSTEPSurface` (unit `surface.surface_p1`), which returns
//!   [`SolidError::Unported`].
//! - **`IntersectWithPlane`** (its body below the plane box ported by
//!   `surface.plane_p2`) takes a bare [`Plane`], which has no owner: its pseudo box is
//!   expanded by `SM_ZONE_TOL_3D` (upstream: the zone tolerance of the plane's face or
//!   B-rep, `SmSurface::GetFace`, `NULL` for a plane without one). Through
//!   `Surface::global_surface_intersect` the plane's owner is passed
//!   (`intersect_with_plane_owned`): a face-owned plane expands by the face tolerance
//!   its B-rep cached on the surface, and returns [`SolidError::Unported`]
//!   (`SmFace::GetTolerance`) without it. It takes a [`Kernel`] for the curve
//!   operations.
//! - **`sm_TestForIsoCircles`** returns its outputs in [`IsoCircles`]; the ones
//!   upstream leaves unwritten (no non-degenerate iso curve, no second plane) are
//!   `None`. With parallel iso planes `smgu_IntersectTwoPlanes` writes no line, so the
//!   common-axis test keeps comparing the last line found, as upstream.

use crate::curve::curve::p7::PropertyAnalysisOptions;
use crate::curve::curve::{Curve, CurveKind, CurvePropertyType};
use crate::curve::ellipse::p2::set_up_polar_conversion;
use crate::curve::ellipse::{Ellipse, EllipseOptions, PolarConversion};
use crate::error::{ignore_status, ignored, Result, SolidError, SM_ERR};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::{
    line_closest_point, line_point_distance, transform_point_to_start_plane, StartPlanePoint,
};
use crate::geom::geom_utility::p2::{intersect_two_planes, PlanesIntersection};
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::{ObjectKind, SolutionObject, SolutionType};
use crate::geom::tol::{are_same, is_zero, ZONE_TOL_3D};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{rad_to_deg, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::{BSplineCurve, BoundingBoxRequest as CurveBoxRequest};
use crate::nurbs::b_spline_surface::{
    BSplineSurface, BSplineSurfaceForm, IsoCurveOptions, SurfParamType,
};
use crate::surface::plane::p2::PlaneOwner;
use crate::surface::plane::Plane;
use crate::surface::surface::p2::{
    scale_step_angular_derivatives, SurfaceIntersection, TsectCurveType, SURF_OF_REVOLUTION_TYPE,
};

pub(crate) const OP_GEN: &str = "surface::surf_of_revolution::p1::SurfOfRevolution::gen_curve";
const OP_CANONICAL: &str = "surface::surf_of_revolution::p1::SurfOfRevolution::create_canonical";
const OP_REBUILD: &str =
    "surface::surf_of_revolution::p1::SurfOfRevolution::rebuild_polar_converter";
const OP_DIRECTRIX: &str =
    "surface::surf_of_revolution::p1::SurfOfRevolution::create_directrix_at_generator_param";
const OP_EVAL_STEP: &str = "surface::surf_of_revolution::p1::SurfOfRevolution::evaluate_step";
const OP_ISO_CIRCLES: &str = "surface::surf_of_revolution::p1::test_for_iso_circles";
const OP_XPLANE: &str = "surface::surf_of_revolution::p1::SurfOfRevolution::intersect_with_plane";

/// The generator's handle in `IntersectWithPlane`'s property analysis: its natural
/// interval (what upstream reads through the curve pointer).
#[derive(Debug, Clone, Copy, PartialEq)]
struct GenHandle(Extent1d);

impl SolutionObject for GenHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

/// `ANALYTIC_TOL_SCALE` (`SmConfig.h`).
const ANALYTIC_TOL_SCALE: f64 = 100.0;

// ---- the generator (a curve derived from SmBSplineCurve) -------------------------------

/// The `SmBSplineCurve` base object of a generator.
pub(crate) fn gen_base(c: &Curve) -> Result<&BSplineCurve> {
    match c.kind() {
        CurveKind::BSpline(b) => Ok(b),
        CurveKind::Ellipse(e) => Ok(e.base()),
        CurveKind::Line(l) => Ok(l.base()),
        CurveKind::Hyperbola(h) => Ok(h.base()),
        CurveKind::Parabola(p) => Ok(p.base()),
        _ => Err(not_b_spline()),
    }
}

/// The `SmBSplineCurve` base object of a generator, mutably (`None` for a kind not
/// derived from `SmBSplineCurve`).
pub(crate) fn gen_base_mut(c: &mut Curve) -> Option<&mut BSplineCurve> {
    match c.kind_mut() {
        CurveKind::BSpline(b) => Some(b),
        CurveKind::Ellipse(e) => Some(&mut e.base),
        CurveKind::Line(l) => Some(&mut l.base),
        CurveKind::Hyperbola(h) => Some(&mut h.base),
        CurveKind::Parabola(p) => Some(p.base_mut()),
        _ => None,
    }
}

pub(crate) const fn not_b_spline() -> SolidError {
    SolidError::invalid_input(OP_GEN, "generator is not derived from SmBSplineCurve")
}

/// `GetSTEPInterval` of a generator.
pub(crate) fn gen_step_interval(c: &Curve) -> Result<Extent1d> {
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
pub(crate) fn gen_max_analytic_domain(k: &Kernel, c: &Curve) -> Result<Extent1d> {
    match c.kind() {
        CurveKind::BSpline(b) => b.max_analytic_domain(k),
        CurveKind::Ellipse(e) => Ok(e.max_analytic_domain()),
        CurveKind::Line(l) => Ok(l.max_analytic_domain()),
        CurveKind::Hyperbola(h) => Ok(h.max_analytic_domain()),
        CurveKind::Parabola(p) => Ok(p.max_analytic_domain()),
        _ => Err(not_b_spline()),
    }
}

/// `AdjustSTEPInterval` of a generator (without owner, see the module notes).
pub(crate) fn gen_adjust_step_interval(k: &Kernel, c: &mut Curve, ivl: &Extent1d) -> Result<()> {
    match c.kind_mut() {
        CurveKind::BSpline(b) => b.adjust_step_interval(k, ivl),
        CurveKind::Ellipse(e) => e.adjust_step_interval(k, ivl, None),
        CurveKind::Line(l) => l.adjust_step_interval(ivl),
        CurveKind::Hyperbola(h) => h.adjust_step_interval(ivl, None),
        CurveKind::Parabola(p) => p.adjust_step_interval(ivl, None),
        _ => Err(not_b_spline()),
    }
}

/// `GetOrCreateGwNurbPointer` of a generator: makes the NURB of an analytic kind when
/// missing (status ignored).
fn gen_ensure_nurb(k: &Kernel, c: &mut Curve) -> Result<()> {
    match c.kind_mut() {
        CurveKind::BSpline(_) => Ok(()),
        CurveKind::Ellipse(e) => {
            if e.base().gw_nurb().is_none() {
                ignore_status(e.make_nurb(k))?;
            }
            Ok(())
        }
        CurveKind::Line(l) => {
            if l.base().gw_nurb().is_none() {
                ignore_status(l.make_nurb())?;
            }
            Ok(())
        }
        CurveKind::Hyperbola(h) => {
            if h.base().gw_nurb().is_none() {
                ignore_status(h.make_nurb(None))?;
            }
            Ok(())
        }
        CurveKind::Parabola(p) => {
            if p.base().gw_nurb().is_none() {
                ignore_status(p.make_nurb(None))?;
            }
            Ok(())
        }
        _ => Err(not_b_spline()),
    }
}

/// The generator `c` with the out-of-bounds flag `enabled`
/// (`SmTemporaryChangeValue<SmBoolean>(m_pGenCurve->GetOutOfBoundsEnabled(), ..)`):
/// only the B-spline evaluation reads the flag, so only that kind is copied.
pub(crate) fn gen_with_out_of_bounds(c: &Curve, enabled: bool) -> Option<Curve> {
    match c.kind() {
        CurveKind::BSpline(b) if b.is_out_of_bounds_enabled() != enabled => {
            let mut b = b.clone();
            b.set_out_of_bounds_enabled(enabled);
            Some(Curve::from(b))
        }
        _ => None,
    }
}

/// The sweep iso-curve maker of the NURB and polar-converter rebuilds: upstream's
/// virtual `CreateIsoParametricCurve(.., eSurfParam, dIsoParameter, ..)` without trim
/// domain, of [`SurfOfRevolution`] or of a derived class (`SmTorus`).
pub(crate) type SweepIsoFn<'a> =
    &'a dyn Fn(&SurfOfRevolution, &Kernel, SurfParamType, f64) -> Result<Curve>;

// ---- results ---------------------------------------------------------------------------

/// The result of [`SurfOfRevolution::create_iso_parametric_curve`].
#[derive(Debug, Clone)]
pub struct RevolutionIsoCurve {
    /// The 3D iso-parametric curve (`rpNewIsoCurve`): a circle (an [`Ellipse`] of
    /// class `SmCircle`) in the sweep direction, else the B-spline extraction.
    pub curve: Curve,
    /// The achieved maximum gap (`pOptMaxGap3d`), always `0`.
    pub max_gap_3d: f64,
    /// The 2D UV line when asked for and built (`pOptUVIsoCurve`).
    pub uv_iso_curve: Option<Curve>,
}

/// The result of [`SurfOfRevolution::create_directrix_at_generator_param`].
#[derive(Debug, Clone)]
pub struct DirectrixAtGeneratorParam {
    /// The circular arc through the generator point (`rpDirectrix`).
    pub directrix: Curve,
    /// Its radius (`dRadius`).
    pub radius: f64,
    /// Its centre on the axis (`rAxisLocation`).
    pub axis_point: Point3,
}

/// The outputs of [`test_for_iso_circles`] (`sm_TestForIsoCircles`).
#[allow(clippy::struct_excessive_bools)] // upstream's six independent out-flags
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IsoCircles {
    /// Every tested non-degenerate iso curve is a circular arc (`rbFoundCircles`).
    pub found_circles: bool,
    /// They lie on parallel planes (`rbParallelPlanes`).
    pub parallel_planes: bool,
    /// They lie on one plane (`rbSamePlane`).
    pub same_plane: bool,
    /// Their centres are on one axis (`rbCoaxialCenters`).
    pub coaxial_centers: bool,
    /// Their planes share one line (`rbCommonAxis`, torus minor circles).
    pub common_axis: bool,
    /// They all start and end on the same planes (`rbPlanarGenCurve`).
    pub planar_gen_curve: bool,
    /// Radius of the first non-degenerate iso curve (`rdRadius`).
    pub radius: Option<f64>,
    /// Its start and end angles in degrees (`rAngles`).
    pub angles: Option<Extent1d>,
    /// Its reference frame (`rReferenceFrame`).
    pub reference_frame: Option<Axis2Placement>,
    /// A point of the common line of the iso planes (`rCommonLinePoint`).
    pub common_line_point: Option<Point3>,
    /// Its unit direction (`rCommonLineVec`).
    pub common_line_vec: Option<Vec3>,
}

// ---- the surface -----------------------------------------------------------------------

/// A surface of revolution (`SmSurfOfRevolution`): `S(u, v) = C(v) + cos(u) A(v) +
/// sin(u) B(v)` for the generator point `P(v)`, its projection `C(v)` onto the axis,
/// `A = P - C` and `B = Z x A`, with `u` the angle in degrees from the X axis about
/// the Z axis of [`SurfOfRevolution::position`] and `v` the generator's STEP
/// parameter, plus its B-spline surface (the sweep direction is NURB `u`, or `v` with
/// `swap_uv`) and the polar conversion table of the sweep direction.
#[derive(Debug, Clone)]
pub struct SurfOfRevolution {
    /// The `SmBSplineSurface` base object.
    pub(crate) base: BSplineSurface,
    /// `m_pGenCurve`: the generating curve (`None` only for [`Self::empty`]).
    pub(crate) gen_curve: Option<Curve>,
    /// `m_bMakeNurbGenCurve`.
    pub(crate) make_nurb_gen_curve: bool,
    /// `m_bPlanarGenerator`: the generator is planar and on the X/Z plane.
    pub(crate) planar_generator: bool,
    /// `m_vPosition`: a point on the axis, the 0 degree direction and the axis (Z).
    pub(crate) position: Axis2Placement,
    /// `m_vAnalUVDomain`: angles in degrees (x) by generator parameters (y).
    pub(crate) anal_uv_domain: Extent2d,
    /// `m_bSwapUV`: the NURB's v is the sweep direction.
    pub(crate) swap_uv: bool,
    /// `m_vPolarConverter`: the sweep direction's conversion table.
    pub(crate) polar: PolarConversion,
    /// The original sweep curve `m_vPolarConverter` owns (`m_cpCurve`), retaining
    /// its object identity while numerical users borrow the ellipse kind.
    pub(crate) polar_curve: Option<Curve>,
}

impl SurfOfRevolution {
    /// An evaluation copy with an exact, owner-preserving generator. `depth` is
    /// the generator's depth in the shared `MAX_GEOMETRY_NESTING` (32) geometry traversal.
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        let gen_curve = self
            .gen_curve
            .as_ref()
            .map(|c| c.clone_for_evaluation_depth(depth))
            .transpose()?;
        let polar_curve = self.polar_curve.as_ref().map(|curve| {
            let mut copy = curve.clone();
            if let (CurveKind::Ellipse(copy), CurveKind::Ellipse(original)) =
                (copy.kind_mut(), curve.kind())
            {
                *copy.base_mut() = original.base().clone_exact();
            }
            copy
        });
        Ok(Self {
            base: self.base.clone_exact(),
            gen_curve,
            make_nurb_gen_curve: self.make_nurb_gen_curve,
            planar_generator: self.planar_generator,
            position: self.position,
            anal_uv_domain: self.anal_uv_domain,
            swap_uv: self.swap_uv,
            polar: self.polar.clone(),
            polar_curve,
        })
    }

    /// `SmSurfOfRevolution_TYPE` (`SURF_BASE_TYPE + 20`).
    pub const TYPE: i64 = SURF_OF_REVOLUTION_TYPE;

    /// A copy whose B-spline objects (the base surface, the generator and the sweep
    /// circle) are kept exactly ([`BSplineSurface::clone_exact`],
    /// [`BSplineCurve::clone_exact`]): `Clone` goes through the `copy`s, which fix the
    /// knots up, while upstream works on the original objects.
    #[must_use]
    pub fn clone_exact(&self) -> Self {
        Self {
            base: self.base.clone_exact(),
            gen_curve: self.gen_curve.as_ref().map(Curve::clone_exact_bounded),
            make_nurb_gen_curve: self.make_nurb_gen_curve,
            planar_generator: self.planar_generator,
            position: self.position,
            anal_uv_domain: self.anal_uv_domain,
            swap_uv: self.swap_uv,
            polar: self.polar.clone(),
            polar_curve: self.polar_curve.as_ref().map(Curve::clone_exact_bounded),
        }
    }

    // ---- construction ---------------------------------------------------------------

    /// The empty surface `ReadFromDB` fills (`SmSurfOfRevolution()`): no generator, no
    /// NURB, the identity placement and an "inside out" domain.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            base: BSplineSurface::new(),
            gen_curve: None,
            make_nurb_gen_curve: false,
            planar_generator: false,
            position: Axis2Placement::IDENTITY,
            anal_uv_domain: Extent2d::new(),
            swap_uv: false,
            polar: PolarConversion::new(),
            polar_curve: None,
        }
    }

    /// The surface sweeping `gen_curve` about the axis through `origin` normal to
    /// `x_axis` and `y_axis` over `anal_uv_domain` (angles in degrees by generator
    /// parameters), without NURB (constructor). The generator's STEP interval should
    /// equal the domain's V interval (upstream only logs otherwise). The generator
    /// is marked planar when it is planar within `ANALYTIC_TOL_SCALE * SM_EFF_ZERO *
    /// (1 + |mid point|)` and on the X/Z plane.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a generator not derived from `SmBSplineCurve`; those of the
    /// generator's mid-point evaluation and plane test (statuses upstream ignores but
    /// whose outputs it then reads).
    pub fn new(
        k: &Kernel,
        gen_curve: Curve,
        origin: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        anal_uv_domain: Extent2d,
        swap_uv: bool,
    ) -> Result<Self> {
        let base_gen = gen_base(&gen_curve)?;
        let mut surf = Self {
            base: BSplineSurface::new(),
            gen_curve: None,
            make_nurb_gen_curve: false,
            planar_generator: true,
            position: Axis2Placement::new(origin, x_axis, y_axis),
            anal_uv_domain,
            swap_uv,
            polar: PolarConversion::new(),
            polar_curve: None,
        };

        // (the STEP interval / VInterval mismatch only logs upstream)

        // set form factor
        surf.base
            .set_b_spline_surface_form(BSplineSurfaceForm::SurfOfRevolution);

        // When given a generator curve - see if its on the X/Z plane
        let pnt = gen_curve.evaluate_point(k, gen_curve.natural_interval().evaluate(0.5))?;
        let tol_3d = ANALYTIC_TOL_SCALE * EFF_ZERO * (1.0 + pnt.max_dimension());

        // when curve is not planar
        // (when curve is not planar, mark that; else see if it is on the X/Z plane)
        if gen_curve.is_planar(k, tol_3d)?.is_planar {
            let on = base_gen.is_on_plane(origin, y_axis, tol_3d, None)?;
            if !on.on_plane {
                surf.planar_generator = false;
            }
        } else {
            surf.planar_generator = false;
        }

        surf.gen_curve = Some(gen_curve);
        Ok(surf)
    }

    /// The surface of one full revolution of `swept_curve` about the axis through
    /// `axis_point` along `axis_direction` (`CreateCanonical`, co-ported from unit
    /// `surface.surf_of_revolution_p2`): the X axis points from the axis point to the
    /// curve's STEP start point (or, when that is on the axis, its point at 45%), the
    /// domain is `[0, 360]` by the curve's STEP interval; the NURB is made by
    /// [`Self::make_nurb`]. A zero axis (squared length below `SM_EFF_ZERO_SQ`) is
    /// taken as `(0, 0, 1)`, as upstream's unchecked `Unitize`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the curve lies on the axis (no Y axis); those of the curve's STEP
    /// evaluation, [`Self::new`] and [`Self::make_nurb`].
    pub fn create_canonical(
        k: &Kernel,
        swept_curve: Curve,
        axis_point: Point3,
        axis_direction: Vec3,
        swap_uv: bool,
    ) -> Result<Self> {
        // unitize the rotation axis (status ignored upstream)
        let z_vec = axis_direction.unitize_or_z();

        // locals
        let crv_ivl = gen_step_interval(&swept_curve)?;

        // SweptCurve start point
        let mut pnt = first(&gen_evaluate_step(k, &swept_curve, crv_ivl.min(), 0)?)?;

        // Vector from SweptCurve Start to RotAxis point
        let mut x_vec = pnt - axis_point;

        // when start point is on rot axis
        if x_vec.length() < EFF_ZERO || x_vec.is_parallel_to(z_vec, 1.0) {
            // try SweptCurve nearMidPoint
            pnt = first(&gen_evaluate_step(
                k,
                &swept_curve,
                crv_ivl.evaluate(0.45),
                0,
            )?)?;
            x_vec = pnt - axis_point;
        }

        // Y vec of rotCoordSystem
        let y_vec = z_vec.cross(x_vec);

        // sYVec can be 0,0,0 if revolving a line coincident with the axis
        if y_vec.length_squared() < crate::math::EFF_ZERO_SQ {
            return Err(SolidError::sm_err(OP_CANONICAL));
        }
        let y_vec = y_vec.unitize()?;

        // Z vec of rotCoordSystem
        let x_vec = y_vec.cross(z_vec);

        // allocate full rotation SurfOfRevolution object
        let anal_uv_domain = Extent2d::from_bounds(0.0, crv_ivl.min(), 360.0, crv_ivl.max());
        let mut surf = Self::new(
            k,
            swept_curve,
            axis_point,
            x_vec,
            y_vec,
            anal_uv_domain,
            swap_uv,
        )?;

        // Make Nurb
        surf.make_nurb(k)?;
        Ok(surf)
    }

    /// Makes the NURB (`MakeNurb`, co-ported from unit `surface.surf_of_revolution_p2`):
    /// [`Self::make_nurb_with_sweep_params`] without sweep curve.
    ///
    /// # Errors
    ///
    /// Those of [`Self::make_nurb_with_sweep_params`].
    pub fn make_nurb(&mut self, k: &Kernel) -> Result<()> {
        self.make_nurb_with_sweep_params(k, None)
    }

    /// Makes the NURB (`MakeNurbWithSweepParams`, co-ported from unit
    /// `surface.surf_of_revolution_p3`): the generator rotated to the start angle
    /// (trimmed with the stored generator to the V domain when their intervals
    /// differ) swept by the domain's angle span
    /// ([`BSplineSurface::create_surf_of_revolution_with_line_vector`], whose sweep
    /// parameterisation follows `opt_sweep_curve` when given, else is by arc length for
    /// a line generator parallel to the axis); then the polar converter is rebuilt
    /// (status ignored).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator; those of the generator rotation, the
    /// interval adjustments and the sweep.
    pub fn make_nurb_with_sweep_params(
        &mut self,
        k: &Kernel,
        opt_sweep_curve: Option<&Curve>,
    ) -> Result<()> {
        self.make_nurb_with_sweep_params_using(k, opt_sweep_curve, &Self::own_sweep_iso)
    }

    /// [`Self::make_nurb_with_sweep_params`] with the polar converter rebuilt from the
    /// sweep iso curves of `iso` (upstream's virtual `CreateIsoParametricCurve`: a
    /// derived class such as `SmTorus` passes its override).
    pub(crate) fn make_nurb_with_sweep_params_using(
        &mut self,
        k: &Kernel,
        opt_sweep_curve: Option<&Curve>,
        iso: SweepIsoFn<'_>,
    ) -> Result<()> {
        // locals
        let surf_sweep_step_ivl = self.anal_uv_domain.u_interval();
        let surf_gen_step_ivl = self.anal_uv_domain.v_interval();
        let gen_curve_step_ivl = gen_step_interval(self.gen()?)?;

        // get a GenCurve
        let mut gen_curve = self.create_generator_from_angle(k, surf_sweep_step_ivl.min())?;

        // When SurfaceCurve and generator intervals are not the same
        if !surf_gen_step_ivl.is_contained_by(&gen_curve_step_ivl, 0.0)
            || !gen_curve_step_ivl.is_contained_by(&surf_gen_step_ivl, 0.0)
        {
            // Trimming is needed.
            let own = self
                .gen_curve
                .as_mut()
                .ok_or(SolidError::null_pointer(OP_GEN))?;
            gen_adjust_step_interval(k, own, &surf_gen_step_ivl)?;
            gen_adjust_step_interval(k, &mut gen_curve, &surf_gen_step_ivl)?;
        }

        // Check if we need to create GenCurve's Nurb form
        gen_ensure_nurb(k, &mut gen_curve)?;

        // build the Nurb Surface (an SmLine generator parallel to the axis gets an
        // arc length sweep parameterisation)
        let line_vector = match gen_curve.kind() {
            CurveKind::Line(l) => Some(l.line_vector()),
            _ => None,
        };
        let tmp = BSplineSurface::create_surf_of_revolution_with_line_vector(
            k,
            gen_base(&gen_curve)?,
            line_vector,
            self.position.origin(),
            self.position.z_axis(),
            surf_sweep_step_ivl.length(),
            self.swap_uv,
            opt_sweep_curve,
        )?;

        // Swap nurbs so that the old m_pNurb is deallocated
        self.base.nurb = tmp.nurb;

        // Set up the Polar conversion - create a sweep isoParametric curve
        ignore_status(self.rebuild_polar_converter_from(k, iso, false))?;
        Ok(())
    }

    /// This class's `CreateIsoParametricCurve` without trim domain, as a [`SweepIsoFn`].
    pub(crate) fn own_sweep_iso(
        &self,
        k: &Kernel,
        surf_param: SurfParamType,
        iso_parameter: f64,
    ) -> Result<Curve> {
        Ok(self
            .create_iso_parametric_curve(k, surf_param, iso_parameter, None)?
            .curve)
    }

    /// Rebuilds the polar converter from the sweep iso curve at the NURB domain's
    /// generator-direction start (or, when that circle is degenerate, at 45% of the
    /// domain) (`RebuildPolarConverter`), the iso curves made by `iso` (upstream's
    /// virtual `CreateIsoParametricCurve`: this class's
    /// [`Self::create_iso_parametric_curve`] or a derived class's override).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when both iso curves are degenerate; those of the iso curves (upstream
    /// then dereferences a null curve) and of the table set-up; `InvalidInput` when the
    /// iso curve is not a circle (upstream casts it).
    pub(crate) fn rebuild_polar_converter(
        &mut self,
        k: &Kernel,
        iso: SweepIsoFn<'_>,
    ) -> Result<()> {
        self.rebuild_polar_converter_from(k, iso, true)
    }

    /// [`Self::rebuild_polar_converter`]; `make_nurb` says whether a missing NURB is
    /// made first, as upstream's `GetNaturalUVDomain` does
    /// (`GetOrCreateGwNurbPointer` calls the virtual `MakeNurb`, the
    /// `MakeNurbWithSweepParams` of `iso`'s class, which rebuilds the converter itself;
    /// its status is ignored). [`Self::make_nurb_with_sweep_params_using`] passes
    /// `false`: it has just stored its NURB, and upstream would recurse without bound
    /// if that NURB were missing.
    fn rebuild_polar_converter_from(
        &mut self,
        k: &Kernel,
        iso: SweepIsoFn<'_>,
        make_nurb: bool,
    ) -> Result<()> {
        // Try the beginning of the curve if possible (GetNaturalUVDomain makes a missing
        // NURB)
        if make_nurb && self.base.gw_nurb().is_none() {
            ignore_status(self.make_nurb_with_sweep_params_using(k, None, iso))?;
        }
        let nurb_domain = self.base.natural_uv_domain();
        let nurb_min_uv = nurb_domain.min();
        self.polar.set_polar_conversion_possible(false);

        let (param, sweep_param) = if self.swap_uv {
            (SurfParamType::U, nurb_min_uv.x)
        } else {
            (SurfParamType::V, nurb_min_uv.y)
        };
        let mut sweep_iso = iso(self, k, param, sweep_param)?;
        let mut iso_degenerate = sweep_iso.is_degenerate(k, EFF_ZERO, None)?;

        if iso_degenerate {
            // Start point was on the axis. Evaluate at the same fraction as
            // CreateCanonical
            let nurb_mid_uv = nurb_domain.evaluate(0.45, 0.45);
            self.polar.set_polar_conversion_possible(false);
            let sweep_param = if self.swap_uv {
                nurb_mid_uv.x
            } else {
                nurb_mid_uv.y
            };
            sweep_iso = iso(self, k, param, sweep_param)?;
            iso_degenerate = sweep_iso.is_degenerate(k, EFF_ZERO, None)?;
        }

        // update PolarConverter
        if iso_degenerate {
            return Err(SolidError::sm_err(OP_REBUILD));
        }
        let CurveKind::Ellipse(circle) = sweep_iso.kind_mut() else {
            return Err(SolidError::invalid_input(
                OP_REBUILD,
                "the sweep iso curve is not a circle",
            ));
        };
        let r = set_up_polar_conversion(&mut self.polar, circle, k);
        self.polar_curve = Some(sweep_iso);
        r
    }

    // ---- access ---------------------------------------------------------------------

    /// The `SmBSplineSurface` base object.
    #[must_use]
    pub const fn base(&self) -> &BSplineSurface {
        &self.base
    }

    pub(crate) fn gen(&self) -> Result<&Curve> {
        self.gen_curve
            .as_ref()
            .ok_or(SolidError::null_pointer(OP_GEN))
    }

    /// The generating curve (`GetGenCurve`).
    #[must_use]
    pub const fn gen_curve(&self) -> Option<&Curve> {
        self.gen_curve.as_ref()
    }

    /// Whether the NURB's v is the sweep direction (`GetSwapUV`).
    #[must_use]
    pub const fn swap_uv(&self) -> bool {
        self.swap_uv
    }

    /// Flips [`Self::swap_uv`] (`ToggleSwapUVBit`).
    pub fn toggle_swap_uv_bit(&mut self) {
        self.swap_uv = !self.swap_uv;
    }

    /// The STEP domain: angles in degrees by generator parameters (`GetSTEPUVDomain`).
    #[must_use]
    pub const fn step_uv_domain(&self) -> Extent2d {
        self.anal_uv_domain
    }

    /// The start angle in degrees (`GetStartAngleDeg`).
    #[must_use]
    pub const fn start_angle_deg(&self) -> f64 {
        self.anal_uv_domain.min().x
    }

    /// The end angle in degrees (`GetEndAngleDeg`).
    #[must_use]
    pub const fn end_angle_deg(&self) -> f64 {
        self.anal_uv_domain.max().x
    }

    /// The axis placement (`GetPosition`).
    #[must_use]
    pub const fn position(&self) -> &Axis2Placement {
        &self.position
    }

    /// Whether out-of-bounds evaluation is enabled (`IsOutOfBoundsEnabled`).
    #[must_use]
    pub fn is_out_of_bounds_enabled(&self) -> bool {
        self.base.is_out_of_bounds_enabled()
    }

    /// Enables or disables out-of-bounds evaluation of the surface and of its
    /// generator (`SetOutOfBoundsEnabled`, which sets the generator's
    /// `SmBSplineCurve` flag too; the empty I/O surface has no generator to set).
    pub fn set_out_of_bounds_enabled(&mut self, enabled: bool) {
        *self.base.out_of_bounds_enabled_mut() = enabled;
        if let Some(gen) = self.gen_curve.as_mut() {
            if let Some(b) = gen_base_mut(gen) {
                b.set_out_of_bounds_enabled(enabled);
            }
        }
    }

    /// `m_bMakeNurbGenCurve`: whether an equivalent NURB is made for an unbounded
    /// analytic generator.
    #[must_use]
    pub const fn make_nurb_gen_curve(&self) -> bool {
        self.make_nurb_gen_curve
    }

    /// The sweep circle the polar converter was set up with (`m_cpCurve`).
    #[must_use]
    pub const fn polar_curve(&self) -> Option<&Ellipse> {
        match self.polar_curve.as_ref() {
            Some(curve) => match curve.kind() {
                CurveKind::Ellipse(ellipse) => Some(ellipse),
                _ => None,
            },
            None => None,
        }
    }

    /// Whether the generator is planar and on the X/Z plane (`m_bPlanarGenerator`).
    #[must_use]
    pub const fn planar_generator(&self) -> bool {
        self.planar_generator
    }

    /// The polar conversion table of the sweep direction (`m_vPolarConverter`).
    #[must_use]
    pub const fn polar_converter(&self) -> &PolarConversion {
        &self.polar
    }

    /// Whether the sweep direction converts exactly (`HavePolarConversion`).
    #[must_use]
    pub const fn have_polar_conversion(&self) -> bool {
        self.polar.is_polar_conversion_possible()
    }

    /// `SetContext`: contexts are not modelled (no effect).
    pub const fn set_context(&self) {}

    /// `SmSurfOfRevolution_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmSurfOfRevolution"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmSurfOfRevolution"
    }

    /// `"SmSurfOfRevolution"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmSurfOfRevolution"
    }

    /// `SmSurfOfRevolution_TYPE` or a type the B-spline surface is (`IsKindOf`).
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

    /// Whether the generator is bounded (`IsBounded`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator.
    pub fn is_bounded(&self) -> Result<bool> {
        Ok(self.gen()?.is_bounded())
    }

    /// `[0, 360]` by the generator's maximum analytic domain (`GetMaxAnalyticDomain`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator; those of the generator's domain.
    pub fn max_analytic_domain(&self, k: &Kernel) -> Result<Extent2d> {
        let v_dom = gen_max_analytic_domain(k, self.gen()?)?;

        // u-domain is 0 to 360.
        Ok(Extent2d::from_bounds(0.0, v_dom.min(), 360.0, v_dom.max()))
    }

    /// The NURB interval of the generator direction and its NURB parameter: v, or u
    /// with [`Self::swap_uv`] (`GetGenDirParamExtent`).
    #[must_use]
    pub fn gen_dir_param_extent(&self) -> (Extent1d, SurfParamType) {
        // get m_pNurb UVDomain
        let nurb_uv_domain = self.base.natural_uv_domain();
        if self.swap_uv {
            (nurb_uv_domain.u_interval(), SurfParamType::U)
        } else {
            (nurb_uv_domain.v_interval(), SurfParamType::V)
        }
    }

    // ---- equality -------------------------------------------------------------------

    /// `operator==`: equal B-spline surfaces, generators, planarity flags, placements,
    /// STEP domains, swap flags and polar converters (tables, and the converters'
    /// circles).
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
            && self.planar_generator == other.planar_generator
            && self.position.approx_eq(&other.position)
            && self.anal_uv_domain.are_same(&other.anal_uv_domain)
            && self.swap_uv == other.swap_uv
            && self.polar_eq(k, other))
    }

    /// `SmPolarConversion::operator==` of the converters, which own their circles.
    fn polar_eq(&self, k: &Kernel, other: &Self) -> bool {
        let _ = k;
        let (a, b) = (&self.polar, &other.polar);
        let has_curve = self.polar_curve.is_some();
        let curves_eq = !has_curve
            || match (self.polar_curve(), other.polar_curve()) {
                (None, None) => true,
                (Some(c), Some(d)) => c.approx_eq(d),
                _ => false,
            };
        a.approx_eq(b) && curves_eq
    }

    // ---- iso curves, directrices and generators -------------------------------------

    /// The iso-parametric curve at the NURB parameter `iso_parameter` in `surf_param`'s
    /// direction (`CreateIsoParametricCurve`): the B-spline extraction of the base
    /// surface ([`BSplineSurface::create_iso_parametric_curve`], trimmed to `domain`),
    /// replaced in the sweep direction by the circle (`SmCircle`) about the axis with
    /// that NURB, its angular domain measured from the curve's end points (moved up a
    /// period when reversed, equal with a non-zero span, or centred below the STEP
    /// domain). A degenerate sweep curve is returned as the B-spline.
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
    ) -> Result<RevolutionIsoCurve> {
        // get the BSplineCurve
        let iso = self.base.create_iso_parametric_curve(
            k,
            surf_param,
            iso_parameter,
            IsoCurveOptions {
                domain: domain.copied(),
                uv_iso_curve: false,
            },
        )?;

        // If eSurfParam corresponds to the direction of revolution, make a circle.
        let step_param = if self.swap_uv {
            if surf_param == SurfParamType::U {
                SurfParamType::V
            } else {
                SurfParamType::U
            }
        } else {
            surf_param
        };

        // done when building a gencurve directed isoParameter or a degerenate circle
        if step_param == SurfParamType::U || iso.curve.is_degenerate(k, EFF_ZERO, None)? {
            return Ok(RevolutionIsoCurve {
                curve: Curve::from(iso.curve),
                max_gap_3d: iso.max_gap_3d,
                uv_iso_curve: iso.uv_iso_curve,
            });
        }

        // Surface locals and vectors to IsoCurve NurbEndPoints
        let nurb_domain = self.base.natural_uv_domain();
        let center = self.position.origin();
        let revolution_z_axis = self.position.z_axis();

        // Nurb Surface size - trimmed to optional domain
        let (min_uv, max_uv) = if surf_param == SurfParamType::U {
            (
                Point2::new(
                    iso_parameter,
                    domain.map_or(nurb_domain.min().y, |d| d.min().y),
                ),
                Point2::new(
                    iso_parameter,
                    domain.map_or(nurb_domain.max().y, |d| d.max().y),
                ),
            )
        } else {
            (
                Point2::new(
                    domain.map_or(nurb_domain.min().x, |d| d.min().x),
                    iso_parameter,
                ),
                Point2::new(
                    domain.map_or(nurb_domain.max().x, |d| d.max().x),
                    iso_parameter,
                ),
            )
        };

        let nurb_span_size = (max_uv - min_uv).length();
        let min_point = self.base.evaluate_point(min_uv)?;
        let max_point = self.base.evaluate_point(max_uv)?;
        let vec_min = min_point - center;
        let vec_max = max_point - center;

        // let Ivl = angles (deg) to min/max points measured in circle coordinate system
        // (statuses ignored upstream; a failed angle is 0)
        let angle_min_rad =
            ignored(revolution_z_axis.ccw_angle_between(self.position.x_axis(), vec_min))?
                .unwrap_or(0.0);
        let angle_max_rad =
            ignored(revolution_z_axis.ccw_angle_between(self.position.x_axis(), vec_max))?
                .unwrap_or(0.0);
        let mut angle_min_deg = angle_min_rad * 180.0 / PI;
        let mut angle_max_deg = angle_max_rad * 180.0 / PI;
        if angle_max_deg < angle_min_deg
            || (is_zero(angle_max_deg - angle_min_deg) && !is_zero(nurb_span_size))
        {
            angle_max_deg += 360.0;
        }

        // If they are centered below our AnalyticDomain, move them up. [B171]
        if (angle_min_deg + angle_max_deg) / 2.0 < self.anal_uv_domain.min().x {
            angle_min_deg += 360.0;
            angle_max_deg += 360.0;
        }

        // (SetMinMax's status is ignored: a reversed pair leaves the "inside out" init)
        let mut anal_domain = Extent1d::new();
        let _ = anal_domain.set_min_max(angle_min_deg, angle_max_deg);

        // use distance between circle and refFrame centers to get radius and circle center
        let disp = revolution_z_axis.dot(vec_min);
        let circle_center = center + disp * revolution_z_axis;
        let radius = sqrt((vec_min.length_squared() - disp * disp).abs());

        // create the circle using the isoCurve parameterization
        let circle = Ellipse::new_circle(
            k,
            circle_center,
            self.position.x_axis(),
            self.position.y_axis(),
            anal_domain,
            radius,
            &EllipseOptions {
                dim: 3,
                nurb: Some(&iso.curve),
                inside_out: false,
            },
        )?;
        Ok(RevolutionIsoCurve {
            curve: Curve::from(circle),
            max_gap_3d: iso.max_gap_3d,
            uv_iso_curve: iso.uv_iso_curve,
        })
    }

    /// The directrix (sweep circle) through the generator point at the generator NURB
    /// parameter `gen_param` (`CreateDirectrixAtGeneratorParam`): the sweep iso curve
    /// at that parameter, plus the radius and axis point of that generator point.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a parameter outside the generator's natural interval; those of the
    /// generator evaluation and [`Self::create_iso_parametric_curve`].
    pub fn create_directrix_at_generator_param(
        &self,
        k: &Kernel,
        gen_param: f64,
    ) -> Result<DirectrixAtGeneratorParam> {
        let gen = self.gen()?;

        // Get GeneratorPoint for given Param
        let generator_domain = gen.natural_interval();
        if !generator_domain.contains_value(gen_param, 0.0) {
            return Err(SolidError::sm_err(OP_DIRECTRIX));
        }
        let generator_point = gen.evaluate_point(k, gen_param)?;

        // SurfOfRevolution locals
        let origin = self.position.origin();
        let z_axis = self.position.z_axis();

        // Find the centerpoint of the circle to be created
        let arc_center_parameter = line_closest_point(origin, z_axis, generator_point);
        let axis_point = origin + arc_center_parameter * z_axis;

        // set radius
        let radius = (generator_point - axis_point).length();

        // pass call along
        let param = if self.swap_uv {
            SurfParamType::U
        } else {
            SurfParamType::V
        };
        let directrix = self
            .create_iso_parametric_curve(k, param, gen_param, None)?
            .curve;
        Ok(DirectrixAtGeneratorParam {
            directrix,
            radius,
            axis_point,
        })
    }

    /// A copy of the generator rotated by `angle_deg` degrees about the axis
    /// (`CreateGeneratorFromAngle`; no rotation within `SM_EFF_ZERO` of 0).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator; those of the copy and the transform.
    pub fn create_generator_from_angle(&self, k: &Kernel, angle_deg: f64) -> Result<Curve> {
        // copy the current GenCurve
        let mut new_curve = self.gen()?.copy(k)?;

        // when rotating the genCurve
        if angle_deg.abs() > EFF_ZERO {
            // define the rotation matrix
            let mut rf = Axis2Placement::IDENTITY;
            rf.rotate_about_axis_at_point(
                angle_deg * PI / 180.0,
                self.position.origin(),
                self.position.z_axis(),
            );

            // rotate the GenCurve Copy
            new_curve.transform(k, &rf, None)?;
        }
        Ok(new_curve)
    }

    // ---- parameter conversion and STEP evaluation -----------------------------------

    /// The STEP parameters (degrees, generator parameter) of the NURB parameters
    /// `nurbs_uv` (`ConvertUVFromNURBSToSTEP`): the sweep angle from the polar
    /// converter and the generator parameter from the generator's conversion.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the clamped point does not map to the STEP domain; those of the
    /// conversions; [`SolidError::Unported`] from the local solve on the
    /// `SmSTEPSurface` of this surface (without exact polar conversion).
    pub fn convert_uv_from_nurbs_to_step(&self, k: &Kernel, nurbs_uv: Point2) -> Result<Point2> {
        let mut written = Point2::new(0.0, 0.0);
        self.convert_uv_from_nurbs_to_step_into(k, nurbs_uv, &mut written)
    }

    /// [`Self::convert_uv_from_nurbs_to_step`] for a caller that ignores its status:
    /// the STEP parameters, or after a failure the value upstream had written to
    /// `rSTEPUV` by then (it starts as the swapped input).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] only.
    pub(crate) fn convert_uv_from_nurbs_to_step_or_written(
        &self,
        k: &Kernel,
        nurbs_uv: Point2,
    ) -> Result<Point2> {
        let mut written = Point2::new(0.0, 0.0);
        match self.convert_uv_from_nurbs_to_step_into(k, nurbs_uv, &mut written) {
            Ok(p) => Ok(p),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => Ok(written),
        }
    }

    /// The body of [`Self::convert_uv_from_nurbs_to_step`]; `written` follows
    /// upstream's writes to the out-parameter `rSTEPUV`.
    fn convert_uv_from_nurbs_to_step_into(
        &self,
        k: &Kernel,
        nurbs_uv: Point2,
        written: &mut Point2,
    ) -> Result<Point2> {
        // init output to something in case of failure
        *written = if self.swap_uv {
            Point2::new(nurbs_uv.y, nurbs_uv.x)
        } else {
            nurbs_uv
        };

        // get approximate NURB to Step MAP using a linear approximation
        let nat_domain = self.base.natural_uv_domain();

        // Clamp NURBSUV value, then convert to get the guess.
        let clamped_nurbs_uv = nat_domain.clamp_point(nurbs_uv);
        let mut step_uv =
            nat_domain.point_map_to_domain(clamped_nurbs_uv, &self.anal_uv_domain, self.swap_uv)?;
        *written = step_uv;

        // swap the NURB point coordinates when needed
        let swapped = if self.swap_uv {
            Point2::new(nurbs_uv.y, nurbs_uv.x)
        } else {
            nurbs_uv
        };

        // when polar conversion is possible
        let mut exact_conversion = false;
        if self.have_polar_conversion() {
            let circle = self.polar_curve().ok_or(SolidError::null_pointer(OP_GEN))?;
            let (x, exact) = self
                .polar
                .convert_to_polar_parameter(k, circle, swapped.x)?;
            step_uv.x = x;
            written.x = x;
            exact_conversion = exact;
            step_uv.y = self.gen()?.convert_t_from_nurbs_to_step(k, swapped.y)?;
            written.y = step_uv.y;
        }

        // when conversion was not exact - use solution as input to a local solve
        if !exact_conversion {
            // Use numerical techniques to convert to polar coordinates: a local point
            // solve on an SmSTEPSurface of this surface.
            let anal_guess = step_uv;
            let step_surface = crate::surface::surface::Surface::from(
                crate::surface::surface::p1::StepSurface::new(
                    crate::surface::surface::Surface::from(self.clone_exact()),
                    false,
                )?,
            );
            let point_3d = self.base.evaluate_point(nurbs_uv)?;
            let domain = step_surface.natural_uv_domain_with_nurb(k)?;
            // Always set output (the status is ignored): LocalPointSolve leaves a default
            // solution when it fails.
            let (u, v) = match ignored(step_surface.local_point_solve::<()>(
                k,
                &domain,
                crate::geom::global_solver::SolverOperation::Intersect,
                point_3d,
                anal_guess,
            ))? {
                Some(r) => (r.solution.start.param(0)?, r.solution.start.param(1)?),
                None => (0.0, 0.0),
            };
            step_uv = Point2::new(u, v);
            *written = step_uv;
        }
        Ok(step_uv)
    }

    /// The point and triangular derivatives at the STEP parameters `uv` (angle in
    /// degrees, generator parameter) (`EvaluateSTEP`): `uv` is clamped to the STEP
    /// domain unless out-of-bounds evaluation is enabled; the derivatives are per
    /// degree in u. The result is laid out `[D, Dv, .., Du, Duv, .., Duu, ..]`
    /// (`(hu + 1) * (hv + 1)` entries, index `i * (hv + 1) + j`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for orders above 3; those of the generator evaluation. Unequal
    /// orders retain release C++ behavior (including default `UNDEF` generator
    /// derivatives beyond the smaller requested order).
    pub fn evaluate_step(&self, k: &Kernel, uv: Point2, hu: usize, hv: usize) -> Result<Vec<Vec3>> {
        let _ = k;
        self.evaluate_step_at(k, uv, hu, hv)
    }

    /// [`Self::evaluate_step`] without the (unused) kernel, for the [`Surface`]
    /// dispatch (`SmSurface::EvaluateSTEP`, which has no kernel).
    pub(crate) fn evaluate_step_at(
        &self,
        k: &Kernel,
        uv: Point2,
        hu: usize,
        hv: usize,
    ) -> Result<Vec<Vec3>> {
        // (an order above the limit is refused before the array is read)
        let n = if hu <= 5 && hv <= 5 {
            (hu + 1) * (hv + 1)
        } else {
            0
        };
        let mut d = vec![Vec3::ZERO; n];
        self.evaluate_step_at_into(k, uv, hu, hv, &mut d)?;
        Ok(d)
    }

    /// [`Self::evaluate_step_at`] into the caller's array `d` (`(hu + 1) * (hv + 1)` entries):
    /// the entries upstream does not write keep the caller's values, as upstream's
    /// `aDerivatives` does (a fresh `SmVector3d` array holds `SM_UNDEF_DOUBLE`).
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate_step_at`]; `InvalidInput` when `d` is too short.
    pub(crate) fn evaluate_step_at_into(
        &self,
        k: &Kernel,
        uv: Point2,
        hu: usize,
        hv: usize,
        d: &mut [Vec3],
    ) -> Result<()> {
        // check input
        if hu > 3 || hv > 3 {
            return Err(SolidError::sm_err(OP_EVAL_STEP));
        }

        // locals
        let mut uv = uv;
        let r_o = self.position.origin();
        let r_x = self.position.x_axis();
        let r_y = self.position.y_axis();
        let s_z = self.position.z_axis();
        let min_highest = hu.min(hv);

        // per SMLib practice, clamp out of bounds UV points
        if !self.base.is_out_of_bounds_enabled() && !self.anal_uv_domain.contains_point(uv, 0.0) {
            uv = self.anal_uv_domain.clamp_point(uv);
        }

        // the generator with the surface's out-of-bounds flag
        let gen_own = self.gen()?;
        let tmp = gen_with_out_of_bounds(gen_own, self.base.is_out_of_bounds_enabled());
        let gen = tmp.as_ref().unwrap_or(gen_own);

        // evaluate expensive trigonometric functions
        let cos_u = (uv.x * PI / 180.0).cos();
        let sin_u = (uv.x * PI / 180.0).sin();

        let pv_eval = gen_evaluate_step(k, gen, uv.y, min_highest)?;
        let pv = |i: usize| pv_eval.get(i).copied().unwrap_or(Vec3::UNDEF);

        let p = pv(0) - r_o;
        let px = p.dot(r_x);
        let py = p.dot(r_y);
        let pz = p.dot(s_z);

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
            r_o + (cos_u * px + -sin_u * py) * r_x + (sin_u * px + cos_u * py) * r_y + pz * s_z,
        );

        // 1st U and V derivatives
        if hu >= 1 {
            set(
                hv + 1,
                (-sin_u * px + -cos_u * py) * r_x + (cos_u * px + -sin_u * py) * r_y,
            );
        }
        if hv >= 1 {
            let (a, b) = (pv(1).dot(r_x), pv(1).dot(r_y));
            set(
                1,
                (cos_u * a + -sin_u * b) * r_x
                    + (sin_u * a + cos_u * b) * r_y
                    + pv(1).dot(s_z) * s_z,
            );
        }
        // cross derivative dUV
        if hu >= 1 && hv >= 1 && min_highest >= 2 {
            let (a, b) = (pv(1).dot(r_x), pv(1).dot(r_y));
            set(
                hv + 2,
                (-sin_u * a + -cos_u * b) * r_x + (cos_u * a + -sin_u * b) * r_y,
            );
        }

        //  cross derivative dUVV
        if hu >= 1 && hv >= 2 && min_highest >= 3 {
            let (a, b) = (pv(2).dot(r_x), pv(2).dot(r_y));
            set(
                hv + 3,
                (-sin_u * a + -cos_u * b) * r_x + (cos_u * a + -sin_u * b) * r_y,
            );
        }

        // 2nd U and V derivatives
        if hu >= 2 {
            set(
                2 * hv + 2,
                (-cos_u * px + sin_u * py) * r_x + (-sin_u * px + -cos_u * py) * r_y,
            );
        }
        if hv >= 2 {
            let (a, b) = (pv(2).dot(r_x), pv(2).dot(r_y));
            set(
                2,
                (cos_u * a + -sin_u * b) * r_x
                    + (sin_u * a + cos_u * b) * r_y
                    + pv(2).dot(s_z) * s_z,
            );
        }

        // cross derivative dUUV
        if hu >= 2 && hv >= 1 && min_highest >= 3 {
            let (a, b) = (pv(1).dot(r_x), pv(1).dot(r_y));
            set(
                2 * hv + 3,
                (-cos_u * a + sin_u * b) * r_x + (-sin_u * a + -cos_u * b) * r_y,
            );
        }
        // 3rd U and V derivatives
        if hu >= 3 {
            set(
                3 * hv + 3,
                (sin_u * px + cos_u * py) * r_x + (-cos_u * px + sin_u * py) * r_y,
            );
        }
        if hv >= 3 {
            let (a, b) = (pv(3).dot(r_x), pv(3).dot(r_y));
            set(
                3,
                (cos_u * a + -sin_u * b) * r_x
                    + (sin_u * a + cos_u * b) * r_y
                    + pv(3).dot(s_z) * s_z,
            );
        }

        // The formulas above are per radian; STEP U is in degrees.
        scale_step_angular_derivatives(d, hu, hv, true, false)?;
        Ok(())
    }

    /// The point at the STEP parameters `uv` (`EvaluateSTEPPoint`: [`Self::evaluate_step`]
    /// without derivatives).
    ///
    /// # Errors
    ///
    /// Those of [`Self::evaluate_step`].
    pub fn evaluate_step_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        first(&self.evaluate_step(k, uv, 0, 0)?)
    }

    /// [`Self::evaluate_step_point`] without the (unused) kernel, for the [`Surface`]
    /// dispatch.
    pub(crate) fn evaluate_step_point_at(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        first(&self.evaluate_step_at(k, uv, 0, 0)?)
    }

    // ---- intersections and points -----------------------------------------------------

    /// The special cases of the intersection with `plane` (`IntersectWithPlane`):
    /// whether the general intersector is needed, and the curves found (no UV curves,
    /// deviations `0`, degenerate curves typed [`TsectCurveType::Touching`]). A
    /// non-planar generator, a plane cutting the axis at an angle or one parallel to the
    /// axis but not through it, and a tangent plane at a flat generator point need the
    /// general intersector. A plane containing the axis gets the generator rotated into
    /// it on either side (inside the sweep); a plane normal to the axis gets the sweep
    /// iso circles at its intersections with the generator (tangent ones typed
    /// [`TsectCurveType::Tangent`]); both are trimmed to `other_uv_domain`
    /// ([`Plane::trim_curve_to_plane_domain`]). The plane is taken as without owner (its
    /// box is expanded by `SM_ZONE_TOL_3D`, see `surface::plane::p2`).
    ///
    /// # Errors
    ///
    /// Those of the angle measures, the generator transform, property analysis and
    /// evaluation and of the iso curves (upstream `SER`); `SM_ERR_NULL_POINTER` without
    /// generator.
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

    /// [`SurfOfRevolution::intersect_with_plane`] with the plane's owner: its box is
    /// expanded by the tolerance of the plane's face (`GetFace()->GetTolerance()`).
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::intersect_with_plane`]; [`SolidError::Unported`]
    /// (`SmFace::GetTolerance`) for a plane with a face whose tolerance is not cached.
    pub(crate) fn intersect_with_plane_owned(
        &self,
        k: &Kernel,
        plane: &Plane,
        plane_owner: PlaneOwner,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        self.intersect_with_plane_owned_using(
            k,
            plane,
            plane_owner,
            other_uv_domain,
            approx_tol_3d,
            &Self::own_sweep_iso,
        )
    }

    /// [`SurfOfRevolution::intersect_with_plane_owned`] with the sweep iso circles of
    /// a plane normal to the axis made by `iso` (upstream's virtual
    /// `CreateIsoParametricCurve`: a derived class such as `SmTorus`, which inherits
    /// `IntersectWithPlane`, passes its override).
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::intersect_with_plane_owned`] and of `iso`.
    #[allow(clippy::too_many_lines)] // one upstream function
    pub(crate) fn intersect_with_plane_owned_using(
        &self,
        k: &Kernel,
        plane: &Plane,
        plane_owner: PlaneOwner,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
        iso: SweepIsoFn<'_>,
    ) -> Result<(bool, SurfaceIntersection)> {
        let mut out = SurfaceIntersection::default();
        let more = self.intersect_with_plane_owned_using_into(
            k,
            plane,
            plane_owner,
            other_uv_domain,
            approx_tol_3d,
            iso,
            &mut out,
        )?;
        Ok((more, out))
    }

    /// [`Self::intersect_with_plane_owned_using`] into `out` (reset first). On an error
    /// `out` keeps what was appended before it, as upstream leaves its output arrays.
    #[allow(clippy::too_many_lines)] // one upstream function
    pub(crate) fn intersect_with_plane_owned_using_into(
        &self,
        k: &Kernel,
        plane: &Plane,
        plane_owner: PlaneOwner,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
        iso: SweepIsoFn<'_>,
        out: &mut SurfaceIntersection,
    ) -> Result<bool> {
        *out = SurfaceIntersection::default();

        // use general intersection for nonPlanar generators
        if !self.planar_generator {
            return Ok(true);
        }

        // plane and RevolutionSurface locals
        let plane_normal = plane.position().z_axis();
        let plane_origin = plane.position().origin();
        let axis = self.position.z_axis();
        let ang_tol_rad = EFF_ZERO_SQRT;
        let tol = approx_tol_3d.unwrap_or(EFF_ZERO_SQRT);

        // the generator's out-of-bounds flag is the surface's during the call
        // (SmTemporaryChangeValue)
        let mut gen = self.gen()?.clone();
        if let Some(b) = gen_base_mut(&mut gen) {
            b.set_out_of_bounds_enabled(self.base.is_out_of_bounds_enabled());
        }

        // get plane bounding box increased by tolerance (status ignored)
        let mut plane_pbox =
            ignored(plane.bounding_boxes(k, &plane.natural_uv_domain(), plane_owner))?
                .map(|(_, p)| p)
                .unwrap_or_default();
        plane_pbox.expand_absolute(tol);

        // The curve pieces of `crv` inside the plane domain when its pseudo box meets
        // the plane's (statuses ignored).
        let trim = |crv: &Curve, plane_pbox: &PseudoBox| -> Result<Vec<Curve>> {
            let req = CurveBoxRequest {
                pseudo: true,
                ..CurveBoxRequest::default()
            };
            let curve_pbox =
                ignored(crv.calculate_bounding_box(k, &crv.natural_interval(), req, true))?
                    .and_then(|b| b.pseudo)
                    .unwrap_or_default();
            if plane_pbox.are_disjoint(&curve_pbox, None) {
                return Ok(Vec::new());
            }
            Ok(
                ignored(plane.trim_curve_to_plane_domain(k, other_uv_domain, crv, tol))?
                    .unwrap_or_default(),
            )
        };

        // when RevolutionAxis is perpendicular to the plane normal
        if axis.is_perpendicular_to(plane_normal, rad_to_deg(ang_tol_rad)) {
            // when the plane does not contain the axis
            let vec_between = self.position.origin() - plane_origin;
            let dist_plane_to_center = vec_between.dot(plane_normal);
            if dist_plane_to_center.abs() > tol {
                return Ok(true);
            }

            let start_ang_rad = self.start_angle_deg() * PI / 180.0;
            let end_ang_rad = self.end_angle_deg() * PI / 180.0;

            // the generator rotated into the plane on its positive, then negative side
            let mut target_vec = axis.cross(plane_normal);
            for side in 0..2 {
                if side == 1 {
                    target_vec *= -1.0;
                }
                let mut angle_rad = axis
                    .ccw_angle_between(self.position.x_axis(), target_vec)
                    .map_err(|_| SolidError::sm_err(OP_XPLANE))?;
                if angle_rad < 0.0 {
                    angle_rad += 2.0 * PI;
                }

                if angle_rad > start_ang_rad - ang_tol_rad && angle_rad < end_ang_rad + ang_tol_rad
                {
                    // the generator's SmBSplineCurve part, rotated about the axis
                    let mut crv = gen_base(&gen)?.copy();
                    let rot_ang_rad = angle_rad; // was dAngleRad-dStartAngRad [B143]
                    let mut trans = Axis2Placement::IDENTITY;
                    trans.translate(-self.position.origin());
                    trans.rotate_about_axis(rot_ang_rad, axis);
                    trans.translate(self.position.origin());
                    crv.transform(&trans, None)?;

                    if side == 1 {
                        // (upstream expands the plane box a second time here)
                        plane_pbox.expand_absolute(tol);
                    }
                    for c in trim(&Curve::from(crv), &plane_pbox)? {
                        out.curves_3d.push(c);
                        out.curve_types.push(TsectCurveType::Crossing);
                    }
                }
            }
        }
        // when RevolutionAxis is parallel to the plane normal
        else if axis.is_parallel_to(plane_normal, rad_to_deg(ang_tol_rad)) {
            let surf_param = if self.swap_uv {
                SurfParamType::U
            } else {
                SurfParamType::V
            };

            // the generator's intersections with the plane
            let d = -plane_origin.dot(plane_normal);
            let gen_ivl = gen.natural_interval();
            let normals = [plane_normal];
            let sols = gen
                .global_property_analysis(
                    k,
                    &gen_ivl,
                    CurvePropertyType::PlaneIntersection,
                    ZONE_TOL_3D,
                    &PropertyAnalysisOptions {
                        property_value: Some(d),
                        opt_vectors: &normals,
                        want_comb: false,
                        handle: Some(GenHandle(gen_ivl)),
                    },
                )?
                .solutions;

            let is_closed = gen.is_closed(k, &gen_ivl, 0.0)?;

            for sol in sols.as_slice() {
                let iso_param = sol.start.param(0)?;
                let mut is_tangent = false;
                let pv = gen.evaluate(k, iso_param, 2, true, true)?;
                let d1 = pv.get(1).copied().unwrap_or(Vec3::UNDEF);
                let d2 = pv.get(2).copied().unwrap_or(Vec3::UNDEF);

                if sol.solution_type == SolutionType::RangeOfValues
                    || d1.is_perpendicular_to(plane_normal, rad_to_deg(ang_tol_rad))
                {
                    is_tangent = true;
                    // a flat tangency: the general surf/surf solver is needed (upstream
                    // empties only the 3D curves: the curve types found so far stay)
                    if d2.length() < EFF_ZERO_SQRT {
                        out.curves_3d.clear();
                        return Ok(true);
                    }
                }

                if is_closed && are_same(iso_param, gen_ivl.max()) {
                    continue;
                }

                let crv = iso(self, k, surf_param, iso_param)?;
                for c in trim(&crv, &plane_pbox)? {
                    out.curves_3d.push(c);
                    out.curve_types.push(if is_tangent {
                        TsectCurveType::Tangent
                    } else {
                        TsectCurveType::Crossing
                    });
                }
            }
        }
        // Plane cuts RevolutionSurface at an angle
        else {
            return Ok(true);
        }

        // (the curve types are appended with the curves, as upstream's)
        for (c, t) in out.curves_3d.iter().zip(out.curve_types.iter_mut()) {
            out.surface1_uv_curves.push(None);
            out.surface2_uv_curves.push(None);
            if c.is_degenerate(k, EFF_ZERO, None)? {
                *t = TsectCurveType::Touching;
            }
            out.deviations.push(0.0);
        }
        Ok(false)
    }

    /// Rotates `point` about the axis onto the start (X/Z) half plane and returns the
    /// rotation angle(s) in degrees mapped into the sweep interval
    /// (`TransformPointToStartPlane`: `smgu_TransformPointToStartPlane` with the
    /// placement and the STEP U interval).
    ///
    /// # Errors
    ///
    /// Those of `smgu_TransformPointToStartPlane`.
    pub fn transform_point_to_start_plane(
        &self,
        point: Point3,
        dist_tol_3d: f64,
        snap_to_seams: bool,
    ) -> Result<StartPlanePoint> {
        let sweep_ivl_deg = self.anal_uv_domain.u_interval();
        transform_point_to_start_plane(
            point,
            &self.position,
            &sweep_ivl_deg,
            dist_tol_3d,
            snap_to_seams,
        )
    }
}

/// The first entry of an evaluation.
pub(crate) fn first(v: &[Vec3]) -> Result<Vec3> {
    v.first()
        .copied()
        .ok_or(SolidError::invalid_input(OP_EVAL_STEP, "empty evaluation"))
}

// ---- sm_TestForIsoCircles --------------------------------------------------------------

/// Tests whether the iso curves of `surface` in `surf_param`'s direction are circular
/// arcs sharing a frame (`sm_TestForIsoCircles`): at every knot and two points in each
/// knot span, the non-degenerate (within `tol_3d`) iso curves must be arcs; the first
/// gives the radius, angles and frame; later ones are compared with it (parallel and
/// common planes, centres on its axis, start and end angles within `ang_tol_rad`, the
/// line common to their planes). Returns early with `found_circles` false at the first
/// iso curve that is not an arc.
///
/// # Errors
///
/// `SM_ERR` when every iso curve is degenerate; those of the knots, the iso curves and
/// the arc test.
pub fn test_for_iso_circles(
    k: &Kernel,
    surface: &BSplineSurface,
    surf_param: SurfParamType,
    tol_3d: f64,
    ang_tol_rad: f64,
) -> Result<IsoCircles> {
    // init outputs
    let mut out = IsoCircles {
        found_circles: true,
        parallel_planes: true,
        same_plane: true,
        coaxial_centers: true,
        planar_gen_curve: true,
        common_axis: true,
        radius: None,
        angles: None,
        reference_frame: None,
        common_line_point: None,
        common_line_vec: None,
    };

    // locals
    let num_check = 3usize;
    let mut first_curve = true;
    let mut second = false;
    let mut z_axis = Vec3::ZERO;
    let mut x_axis = Vec3::ZERO;
    let mut ref_origin = Point3::ZERO;
    let mut this_common_line_point = Vec3::UNDEF;
    let mut this_common_line_vec = Vec3::UNDEF;
    let mut start_ang_deg1 = 0.0;
    let mut end_ang_deg1 = 0.0;

    // get surface knots in test directon
    let (knots, _) = surface.knots(surf_param, None)?;

    // Build testValues: for every knot value + lNumCheck sample points in between
    let mut test_vals: Vec<f64> = Vec::new();
    let Some(&k0) = knots.first() else {
        return Err(SolidError::invalid_input(OP_ISO_CIRCLES, "no knots"));
    };
    test_vals.push(k0);
    for pair in knots.windows(2) {
        let (last_knot, curr_knot) = match pair {
            [a, b] => (*a, *b),
            _ => continue,
        };
        for ll in 0..num_check - 1 {
            let t = (ll as f64 + 1.0) / num_check as f64;
            test_vals.push(last_knot + t * (curr_knot - last_knot));
        }
        test_vals.push(curr_knot);
    }

    // for every test value
    let mut n_degen = 0usize;
    for &val in &test_vals {
        // make isoParameter Curve
        let iso = surface
            .create_iso_parametric_curve(k, surf_param, val, IsoCurveOptions::default())?
            .curve;

        // skip degenerate curves
        if iso.is_degenerate(k, tol_3d, None)? {
            n_degen += 1;
            continue;
        }

        // when isoParameterCurve is not a circular Arc
        let Some((this_ref_frame, rad, start_ang_deg, end_ang_deg)) =
            Curve::from(iso).is_arc(k, 5, tol_3d)?
        else {
            out.found_circles = false;
            return Ok(out);
        };

        // The first time through get output reference frame, radius and angle extent
        if first_curve {
            first_curve = false;
            second = true;
            out.radius = Some(rad);
            out.angles = Some(Extent1d::from_min_max(start_ang_deg, end_ang_deg));
            out.reference_frame = Some(this_ref_frame);
            z_axis = this_ref_frame.z_axis();
            x_axis = this_ref_frame.x_axis();
            ref_origin = this_ref_frame.origin();
            start_ang_deg1 = start_ang_deg;
            end_ang_deg1 = end_ang_deg;
            continue;
        }

        // compare 1st reference frame to current one
        // when Z axes are not parallel - isoParameter curves are not on parallel planes
        if out.parallel_planes && z_axis.dot(this_ref_frame.z_axis()) < 1.0 - EFF_ZERO {
            out.parallel_planes = false;
            out.same_plane = false;
        }

        // (SamePlane is already false if ParallelPlanes is.)
        if out.same_plane {
            let mut diff = this_ref_frame.origin() - ref_origin;
            let len = diff.length();
            if len >= tol_3d {
                diff /= len;
                if diff.dot(z_axis).abs() > tol_3d {
                    out.same_plane = false;
                }
            }
        }

        // Check centers all on same axis
        if out.coaxial_centers {
            let (dist_from_axis, _) =
                line_point_distance(ref_origin, z_axis, this_ref_frame.origin());
            if dist_from_axis > tol_3d {
                out.coaxial_centers = false;
            }
        }

        // when arcs do not start and stop on same plane
        if out.planar_gen_curve {
            // (AngleBetween's status is ignored upstream: a failure gives 0)
            let ang_rad = ignored(x_axis.angle_between(this_ref_frame.x_axis()))?.unwrap_or(0.0);
            // angle test only
            if ang_rad.abs() > ang_tol_rad
                || (start_ang_deg1 - start_ang_deg).abs() > ang_tol_rad
                || (end_ang_deg1 - end_ang_deg).abs() > ang_tol_rad
            {
                out.planar_gen_curve = false;
            }

            // position test
            if (rad * ang_rad).abs() > tol_3d
                || (rad * ((start_ang_deg1 - start_ang_deg) * PI / 180.0)).abs() > tol_3d
                || (rad * ((end_ang_deg1 - end_ang_deg) * PI / 180.0)).abs() > tol_3d
            {
                out.planar_gen_curve = false;
            }
        }

        // find line of intersection between two isoCurve planes
        if out.common_axis {
            let (success, coincident) = match intersect_two_planes(
                ref_origin,
                z_axis,
                this_ref_frame.origin(),
                this_ref_frame.z_axis(),
            ) {
                Ok(PlanesIntersection::Line { point, direction }) => {
                    this_common_line_point = point;
                    this_common_line_vec = direction;
                    (true, false)
                }
                Ok(PlanesIntersection::Parallel { coincident }) => (false, coincident),
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => (false, false),
            };

            // when this is the 2nd isoParamCurve
            if second {
                if success {
                    // when planes intersected uniquely - save common line
                    second = false;
                    out.common_line_point = Some(this_common_line_point);
                    out.common_line_vec = Some(this_common_line_vec);
                } else if !coincident {
                    // when planes were parallel but not coincident
                    out.common_axis = false;
                }
            } else {
                // compare subsequent common lines against 1st common line
                let line_point = out.common_line_point.unwrap_or(Vec3::UNDEF);
                let line_vec = out.common_line_vec.unwrap_or(Vec3::UNDEF);
                let (dist1, _) = line_point_distance(line_point, line_vec, this_common_line_point);
                let (dist2, _) =
                    line_point_distance(this_common_line_point, this_common_line_vec, line_point);
                if dist1 > tol_3d || dist2 > tol_3d {
                    out.common_axis = false;
                }
            }
        }
    }

    // all done
    if n_degen == test_vals.len() {
        return Err(SolidError::Upstream {
            operation: OP_ISO_CIRCLES,
            code: SM_ERR,
            entity: None,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::Line;
    use crate::nurbs::crv_basic::Curve as NurbCurve;
    use crate::nurbs::math::{CPoint, NOW};

    fn k() -> Kernel {
        Kernel::default()
    }

    /// The cubic generator of `TestSmGeneral.cpp` (`CreateCanonical` about Z).
    fn bezier() -> Curve {
        let pts = [
            [2.0, 0.0, 0.0],
            [2.5, 0.4, 0.5],
            [3.2, -0.2, 1.7],
            [2.7, 0.3, 3.0],
        ];
        let pw = pts
            .iter()
            .map(|&[x, y, z]| CPoint::new(x, y, z, NOW))
            .collect();
        let cur = NurbCurve::new(pw, 3, vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]).unwrap();
        Curve::from(BSplineCurve::from_nurb(cur, 3, false))
    }

    fn cone() -> SurfOfRevolution {
        let mut l = Line::from_points(Vec3::new(2.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 3.0), 3);
        l.make_nurb().unwrap();
        SurfOfRevolution::create_canonical(
            &k(),
            Curve::from(l),
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 3.0),
            false,
        )
        .unwrap()
    }

    #[test]
    fn polar_rebuild_retains_the_original_curve_identity() {
        let mut kernel = k();
        let mut revolution = cone();
        let mut original = revolution.polar_curve.take().unwrap();
        let key = kernel
            .context
            .geometry_key(original.notify_identity_slot())
            .unwrap();
        let before = original.evaluate_point(&kernel, 0.25).unwrap();
        let original = std::cell::RefCell::new(Some(original));
        revolution
            .rebuild_polar_converter(&kernel, &|_, _, _, _| {
                Ok(original.borrow_mut().take().unwrap())
            })
            .unwrap();
        assert!(original.borrow().is_none());
        let mut moved = Vec::new();
        moved.push(revolution);
        for _ in 0..16 {
            moved.push(cone());
        }
        let retained = &moved[0];
        let curve = retained.polar_curve.as_ref().unwrap();
        assert_eq!(curve.notify_key(), Some(key));
        assert_eq!(curve.evaluate_point(&kernel, 0.25).unwrap(), before);
        assert!(retained.polar_curve().is_some());
        for copy in [
            retained.clone(),
            retained.clone_exact(),
            retained.clone_for_evaluation(0).unwrap(),
        ] {
            let copied = copy.polar_curve.as_ref().unwrap();
            assert_eq!(copied.notify_key(), None);
            assert_eq!(copied.evaluate_point(&kernel, 0.25).unwrap(), before);
        }
        assert_eq!(
            retained.polar_curve.as_ref().unwrap().notify_key(),
            Some(key)
        );
    }

    #[test]
    fn canonical_line_is_a_cone() {
        let s = cone();
        assert!(s.planar_generator());
        assert!(s.have_polar_conversion());
        assert_eq!(
            s.step_uv_domain(),
            Extent2d::from_bounds(0.0, 0.0, 360.0, 1.0)
        );
        assert!(s.is_kind_of(SurfOfRevolution::TYPE).unwrap());
        assert!(s.is_bounded().unwrap());
        // half way up, a quarter turn: radius 1.5 at height 1.5 on the Y axis
        let p = s.evaluate_step_point(&k(), Point2::new(90.0, 0.5)).unwrap();
        assert!((p - Vec3::new(0.0, 1.5, 1.5)).length() < 1e-12);
        // the STEP domain clamps without out-of-bounds evaluation
        let q = s.evaluate_step_point(&k(), Point2::new(90.0, 2.0)).unwrap();
        assert!((q - Vec3::new(0.0, 1.0, 3.0)).length() < 1e-12);
        let (ivl, param) = s.gen_dir_param_extent();
        assert_eq!(param, SurfParamType::V);
        assert_eq!((ivl.min(), ivl.max()), (0.0, 1.0));
        let m = s.max_analytic_domain(&k()).unwrap();
        assert_eq!((m.min().x, m.max().x), (0.0, 360.0));
    }

    #[test]
    fn step_derivatives_are_per_degree() {
        let s = cone();
        let d = s.evaluate_step(&k(), Point2::new(0.0, 0.0), 1, 1).unwrap();
        assert_eq!(d.len(), 4);
        // Du at u = 0 on radius 2: 2 * PI / 180 along Y
        let du = d.get(2).copied().unwrap();
        assert!((du - Vec3::new(0.0, 2.0 * PI / 180.0, 0.0)).length() < 1e-12);
        assert!(s.evaluate_step(&k(), Point2::new(0.0, 0.0), 4, 4).is_err());
        // Boundary iso curves request pure U derivatives, despite C++'s
        // debug-only equal-order assertion. These are all defined in release.
        let sweep = s.evaluate_step(&k(), Point2::new(0.0, 0.0), 3, 0).unwrap();
        assert_eq!(sweep.len(), 4);
        assert_eq!(sweep[1], du);
        assert!((sweep[2] - Vec3::new(-2.0 * (PI / 180.0).powi(2), 0.0, 0.0)).length() < 1e-12);
        assert!((sweep[3] - Vec3::new(0.0, -2.0 * (PI / 180.0).powi(3), 0.0)).length() < 1e-12);
    }

    #[test]
    fn sweep_iso_curve_is_a_circle() {
        let s = cone();
        let iso = s
            .create_iso_parametric_curve(&k(), SurfParamType::V, 0.5, None)
            .unwrap();
        let CurveKind::Ellipse(e) = iso.curve.kind() else {
            panic!("not a circle");
        };
        assert!(e.is_circle_class());
        assert!((e.radius() - 1.5).abs() < 1e-12);
        let d = s.create_directrix_at_generator_param(&k(), 0.5).unwrap();
        assert!((d.radius - 1.5).abs() < 1e-12);
        assert!((d.axis_point - Vec3::new(0.0, 0.0, 1.5)).length() < 1e-12);
        assert!(s.create_directrix_at_generator_param(&k(), 1.5).is_err());
        // the generator direction is a B-spline
        let gen = s
            .create_iso_parametric_curve(&k(), SurfParamType::U, 0.5, None)
            .unwrap();
        assert!(matches!(gen.curve.kind(), CurveKind::BSpline(_)));
    }

    #[test]
    fn nurbs_to_step_round_trip() {
        let s = cone();
        let step = s
            .convert_uv_from_nurbs_to_step(&k(), Point2::new(0.0, 0.25))
            .unwrap();
        assert!(step.x.abs() < 1e-12);
        assert!((step.y - 0.25).abs() < 1e-12);
    }

    #[test]
    fn generator_rotation_and_equality() {
        let k = k();
        let s =
            SurfOfRevolution::create_canonical(&k, bezier(), Vec3::ZERO, Vec3::Z, false).unwrap();
        assert!(!s.planar_generator());
        let g = s.create_generator_from_angle(&k, 90.0).unwrap();
        let p = g.evaluate_point(&k, 0.0).unwrap();
        assert!((p - Vec3::new(0.0, 2.0, 0.0)).length() < 1e-12);
        let c = s.clone();
        assert!(s.approx_eq(&k, &c).unwrap());
        let mut t = s.clone();
        t.toggle_swap_uv_bit();
        assert!(!s.approx_eq(&k, &t).unwrap());
        // a non-planar generator needs the general intersector
        let plane = Plane::from_normal(Vec3::ZERO, Vec3::Z);
        let (more, x) = s
            .intersect_with_plane(
                &k,
                &plane,
                &Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0),
                None,
            )
            .unwrap();
        assert!(more && x.curves_3d.is_empty());
        // a planar one is intersected here (the plane has no owner)
        let mut plane = plane;
        plane.make_nurb().unwrap();
        let r = cone().intersect_with_plane(
            &k,
            &plane,
            &Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0),
            None,
        );
        assert!(r.is_ok_and(|(_, x)| x.curves_3d.len() == x.curve_types.len()));
    }

    #[test]
    fn line_on_axis_has_no_frame() {
        let mut l = Line::from_points(Vec3::ZERO, Vec3::new(0.0, 0.0, 2.0), 3);
        l.make_nurb().unwrap();
        let r =
            SurfOfRevolution::create_canonical(&k(), Curve::from(l), Vec3::ZERO, Vec3::Z, false);
        assert!(matches!(r, Err(SolidError::Upstream { code: SM_ERR, .. })));
    }

    #[test]
    fn start_plane_of_an_axis_point_has_two_angles() {
        let s = cone();
        let r = s
            .transform_point_to_start_plane(Vec3::new(0.0, 0.0, 1.0), 1e-6, false)
            .unwrap();
        assert_eq!(r.angles_deg.len(), 2);
    }

    #[test]
    fn iso_circles_of_a_revolved_surface() {
        let k = k();
        let s =
            SurfOfRevolution::create_canonical(&k, bezier(), Vec3::ZERO, Vec3::Z, false).unwrap();
        let r = test_for_iso_circles(&k, s.base(), SurfParamType::V, 1e-6, 1e-6).unwrap();
        assert!(r.found_circles && r.parallel_planes && r.coaxial_centers);
        assert!(!r.same_plane);
        assert!(r.radius.is_some() && r.reference_frame.is_some());
        // the generator direction is not circular
        let g = test_for_iso_circles(&k, s.base(), SurfParamType::U, 1e-6, 1e-6).unwrap();
        assert!(!g.found_circles);
    }
}
