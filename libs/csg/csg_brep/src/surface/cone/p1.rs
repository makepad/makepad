//! `SmCone` / `SmCylinder`, part 1: the cone object ([`Cone`], a surface of revolution
//! of a line) with its constructors, `CreateCanonical`, equality, the iso-parametric
//! curves (lines and circles), the STEP / NURB parameter conversions, NURB and STEP
//! evaluation, the apex and height queries, STEP inversion, transform and database
//! input, and the curve coincidence test of the cone intersector
//! ([`total_coincidence_checker`]).
//!
//! Upstream: `source/SMLib/src/SmCone.cpp`, `source/SMLib/inc/SmCone.h`,
//! `source/SMLib/src/SmCylinder.cpp`, `source/SMLib/inc/SmCylinder.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`Cone`]                                          | `SmCone`, `SmCylinder` (with [`Cone::is_cylinder_class`]) |
//! | [`Cone::empty`]                                   | `SmCone()`, `SmCylinder()`                       |
//! | [`Cone::new_unbounded`]                           | `SmCone(Origin, XAxis, YAxis, BotRadius, SemiAngleDeg, bSwapUV, bInsideOut)` |
//! | [`Cone::new_bounded`]                             | `SmCone(Origin, XAxis, YAxis, BotRadius, TopRadius, StartAngle, EndAngle, Height, bSwapUV, bInsideOut, bMakeNurbGenCurve)` |
//! | [`Cone::new_cylinder_unbounded`], [`Cone::new_cylinder_bounded`] | the two `SmCylinder` constructors  |
//! | `Clone`                                           | copy constructors, `Copy` (both classes)         |
//! | `Drop` (implicit)                                 | `~SmCone`, `~SmCylinder`                         |
//! | [`Cone::approx_eq`]                               | `operator==` (both classes)                      |
//! | [`Cone::create_canonical_with_semi_angle`], [`Cone::create_canonical`] | `CreateCanonical` (both overloads) |
//! | [`Cone::create_canonical_cylinder`]               | `SmCylinder::CreateCanonical`                    |
//! | [`Cone::make_nurb`]                               | `SmSurfOfRevolution::MakeNurb` (inherited; its polar converter rebuild calls the cone's iso curves) |
//! | [`Cone::adjust_step_uv_domain`]                   | `AdjustSTEPUVDomain`                             |
//! | [`Cone::apex`], [`Apex`]                          | `GetApex`                                        |
//! | [`Cone::canonical`], [`ConeCanonical`]            | `GetCanonical`                                   |
//! | [`Cone::create_iso_parametric_curve`]             | `CreateIsoParametricCurve`                       |
//! | [`Cone::create_iso_circle_from_point`]            | `CreateIsoCircleFromPoint`                       |
//! | [`Cone::convert_uv_from_step_to_nurbs`], [`Cone::convert_uv_from_nurbs_to_step`] | `ConvertUVFromSTEPToNURBS`, `ConvertUVFromNURBSToSTEP` |
//! | [`Cone::evaluate`], [`Cone::evaluate_point`]      | `Evaluate`, `EvaluatePoint`                      |
//! | [`Cone::evaluate_step`], [`Cone::evaluate_step_point`] | `EvaluateSTEP`, `EvaluateSTEPPoint`         |
//! | [`Cone::height`]                                  | `GetHeight`                                      |
//! | [`Cone::step_linear_param_extent`]                | `GetSTEPLinearParamExtent`                       |
//! | [`Cone::max_analytic_domain`]                     | `GetMaxAnalyticDomain` (both classes)            |
//! | [`Cone::step_inversion`], [`ConeStepInversion`]   | `STEPInversion` (with `SmSurfOfRevolution::STEPInversion`, co-ported) |
//! | [`Cone::positive_point`], [`PositivePoint`]       | `GetPositivePoint`                               |
//! | [`Cone::transform`]                               | `Transform`                                      |
//! | [`Cone::read_from_db`]                            | `ReadFromDB` (both classes)                      |
//! | [`Cone::is_kind_of`], [`Cone::type_code`], [`Cone::type_string`], [`Cone::class_string`] | `IsKindOf`, `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`, both classes) |
//! | [`Cone::radius`], [`Cone::bot_radius`], [`Cone::top_radius`], [`Cone::bot_height`], [`Cone::top_height`] | `GetRadius`, `GetBotRadius`, `GetTopRadius`, `GetBotHeight`, `GetTopHeight` (inline) |
//! | [`Cone::is_cylinder`], [`Cone::inside_out`], [`Cone::base_radius`], [`Cone::semi_angle_deg`] | `IsCylinder` (both classes), `GetInsideOut` (inline), the members |
//! | [`total_coincidence_checker`], [`Coincidence`]    | `sm_TotalCoincidenceChecker` (static)            |
//!
//! Deviations:
//! - **Derived classes.** `SmCone` derives from `SmSurfOfRevolution`; the base object
//!   is the [`Cone::rev`] field. `SmCylinder` adds no data: it is a [`Cone`] with
//!   [`Cone::is_cylinder_class`] set (its type, `IsCylinder` and
//!   `GetMaxAnalyticDomain` differ). The cone joins
//!   [`crate::surface::surface::SurfaceKind`] with the units porting the remaining
//!   `SmSurfOfRevolution` overrides (`surface.surf_of_revolution_p2`, `_p3`).
//! - **Virtual calls into the cone.** The inherited `MakeNurb` rebuilds the polar
//!   converter through the virtual `CreateIsoParametricCurve`; [`Cone::make_nurb`]
//!   runs it with the cone's override. The unbounded constructor's generator line has
//!   the cone as owner when its interval is adjusted: the line's `MakeNurb` then asks
//!   the cone for its generator-direction NURB interval, which makes the cone's NURB
//!   (`GetOrCreateGwNurbPointer`) and reparameterises the line to it; the constructor
//!   does these steps explicitly.
//! - **Generator.** The cone's generator is always an `SmLine` here (constructors);
//!   `GetApex`, `STEPInversion` and `GetMaxAnalyticDomain` dispatch on the curve kind.
//!   The `SmSurfOfRevolution` constructor runs with a `NULL` generator, so the
//!   generator is never tested for planarity (`m_bPlanarGenerator` stays `TRUE`).
//! - **`SmSurfOfRevolution::STEPInversion`** (unit `surface.surf_of_revolution_p2`) is
//!   co-ported for the planar generator every cone has; its other branch returns
//!   [`SolidError::Unported`].
//! - **Statuses.** Out-params become return values; `SmContext` arguments and the
//!   arguments upstream marks `NotUsed` are dropped. Calls whose status upstream
//!   ignores keep upstream's values, except that [`SolidError::Unported`] always
//!   propagates and a failure whose output upstream would read uninitialised (a failed
//!   point evaluation in `CreateIsoParametricCurve`, a failed STEP
//!   inversion in `CreateIsoCircleFromPoint`) is returned. `GetApex` of a cylinder and
//!   `STEPInversion` of a point outside the sweep return `SM_ERR` upstream with their
//!   outputs set (callers use them): here they return the outputs ([`Apex::is_cylinder`],
//!   [`ConeStepInversion::inside`]). `Evaluate` and `EvaluateSTEP` entries upstream
//!   does not write are `0`. `ConvertUVFromSTEPToNURBS` without polar curve (upstream
//!   dereferences `NULL`) is `InvalidInput`. The unbounded constructor propagates an
//!   error from `GetApex` (upstream ignores its status and then reads an
//!   uninitialised `dApexParam`); a cylinder's apex is not an error here.
//!   `GetHeight` ignores point-evaluation errors and retains `Vec3::UNDEF` for an
//!   unwritten point (the initialized default `SmPoint3d`), including zero height
//!   when both conversions fail because the polar curve is missing.
//!   [`Cone::apex`] propagates an error from the generator's STEP inversion of the
//!   apex, whereas `GetPositivePoint` and `STEPInversion` upstream ignore `GetApex`'s
//!   status and still use the apex point it wrote. This differs only for a generator
//!   whose inversion fails (e.g. a `MakeNurb` B-spline generator); no difftest case
//!   reaches it.
//!   `CreateIsoCircleFromPoint` ignores the status of `ConvertUVFromSTEPToNURBS` as
//!   upstream does when `swap_uv` is set and the polar converter already wrote the
//!   `x` value it then uses; otherwise (upstream reads an unset value) the error is
//!   returned. Both happen for unbounded cones with a semi-angle just above the
//!   cylinder test (e.g. `5e-11` degrees), whose polar circle lies far below the
//!   origin.
//! - **`STEPInversion` out of bounds.** Upstream's `const` `STEPInversion` calls
//!   `m_pGenCurve->SetOutOfBoundsEnabled(TRUE)`, changing the generator for good; here
//!   the inversion turns it on in a clone of the generator. Results are the same:
//!   [`Cone::set_out_of_bounds_enabled`] already propagates the flag to the generator.
//! - **ASCII `ReadFromDB`.** A failed numeric extraction leaves the field at its
//!   initial value (`UNDEF_DOUBLE` for the radius and semi-angle); C++11 `>>` stores `0`.
//!   The record then fails anyway: the surface of revolution record that follows
//!   cannot be read from the failed stream.
//! - **`sm_TotalCoincidenceChecker`** returns `FALSE` (`0`, not `SM_SUCCESS`) for two
//!   lines that are not coincident: that is the `SmStatus` 0 error here.
//! - **Unported callees.** `AdjustSTEPUVDomain` past its radius checks calls
//!   `SmSurfOfRevolution::AdjustSTEPUVDomain` (unit `surface.surf_of_revolution_p2`):
//!   [`SolidError::Unported`]. `Transform` finishes in `SmSurfOfRevolution::Transform`
//!   (its polar-converter rebuild given the cone's iso curves, as upstream's virtual
//!   call does) and `ReadFromDB` in [`SurfOfRevolution::read_from_db`].
//! - **NURB.** The queries expect the NURB made ([`Cone::make_nurb`], as the
//!   `CreateCanonical` functions do); upstream's `GetNaturalUVDomain` makes a missing
//!   one through the `const` object. A bare cone's domain is then `[0, 0] x [0, 0]`;
//!   a cone in a [`crate::surface::surface::Surface`] makes it and keeps it there
//!   (`Surface::kind_with_nurb`, `Surface::natural_uv_domain_with_nurb`), as upstream.
//! - **Debug code.** The `SM_DEBUG_CODE` checks and drawing are omitted.

use crate::curve::curve::p2::{CIRCLE_TYPE, LINE_TYPE};
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::ellipse::{Ellipse, EllipseOptions};
use crate::curve::line::{BoundedLineOptions, Line};
use crate::error::{ignore_status, ignored, Result, SolidError, SM_ERR};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::global_solver::SolverOperation;
use crate::geom::tol::{approx_tol3d, are_same, are_same_to_tol, is_zero};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max3, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, INFINITE_PARAMETER, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_surface::{
    BSplineSurfaceForm, IsoCurveOptions, LocationType, SurfParamType,
};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::surface::surf_of_revolution::p1::gen_max_analytic_domain;
use crate::surface::surf_of_revolution::{RevolutionIsoCurve, SurfOfRevolution};
use crate::surface::surface::p2::{scale_step_angular_derivatives, CONE_TYPE, CYLINDER_TYPE};
use crate::topology::database_io::{DatabaseReader, FileType};

const OP_CANONICAL: &str = "surface::cone::p1::Cone::create_canonical";
const OP_CYL_CANONICAL: &str = "surface::cone::p1::Cone::create_canonical_cylinder";
const OP_ADJUST: &str = "surface::cone::p1::Cone::adjust_step_uv_domain";
const OP_ISO: &str = "surface::cone::p1::Cone::create_iso_parametric_curve";
const OP_TO_NURBS: &str = "surface::cone::p1::Cone::convert_uv_from_step_to_nurbs";
const OP_TO_STEP: &str = "surface::cone::p1::Cone::convert_uv_from_nurbs_to_step";
const OP_EVAL: &str = "surface::cone::p1::Cone::evaluate";
const OP_EVAL_STEP: &str = "surface::cone::p1::Cone::evaluate_step";
const OP_GEN: &str = "surface::cone::p1::Cone::gen_curve";
const OP_TRANSFORM: &str = "surface::cone::p1::Cone::transform";

/// Upstream `SmStatus` value `SM_ERR_INVALID_INPUT`.
const SM_ERR_INVALID_INPUT: i32 = 1007;

/// `SM_DEG2RAD(a)` = `a * (SM_PI / 180.0)`.
fn deg2rad(a: f64) -> f64 {
    a * (PI / 180.0)
}

fn status(operation: &'static str, code: i32) -> SolidError {
    SolidError::Upstream {
        operation,
        code,
        entity: None,
    }
}

/// The apex of a cone ([`Cone::apex`], `GetApex`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Apex {
    /// The shape is a cylinder (`bIsCylinder`): no apex, `point` is the origin and
    /// `param` is `0` (upstream returns `SM_ERR` with these outputs).
    pub is_cylinder: bool,
    /// The apex point, or the cylinder's origin (`rApexPoint`).
    pub point: Point3,
    /// The STEP (height) parameter of `point` (`dApexParam`).
    pub param: f64,
}

/// The canonical data of a cone ([`Cone::canonical`], `GetCanonical`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConeCanonical {
    /// The placement: origin on the axis (bottom circle centre), 0 and 90 degree axes.
    pub origin: Axis2Placement,
    /// The radius at the origin (`rdRadius`).
    pub radius: f64,
    /// The semi-angle in degrees (`rdSemiAngleDeg`; `0` for a cylinder).
    pub semi_angle_deg: f64,
    /// The NURB's u is the linear direction (`pOptSwapUV`).
    pub swap_uv: bool,
    /// The NURB's linear direction runs from the top (`pOptInsideOut`).
    pub inside_out: bool,
}

/// A point mirrored through the apex ([`Cone::positive_point`], `GetPositivePoint`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositivePoint {
    /// The point was on the far side of the apex and has been reflected (the return
    /// value).
    pub negated: bool,
    /// The point, or its reflection through the apex (`rPositivePoint`).
    pub point: Point3,
    /// The apex parameter (`dApexParam`).
    pub apex_param: f64,
}

/// The STEP parameters of a point on the cone ([`Cone::step_inversion`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConeStepInversion {
    /// STEP parameters: angle in degrees, height (`rdAnalUVParameter`).
    pub uv: Point2,
    /// The point's location (`reLocation`).
    pub location: LocationType,
    /// The angle is inside the sweep (upstream returns `SM_ERR` with these outputs
    /// otherwise).
    pub inside: bool,
}

/// The result of [`total_coincidence_checker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coincidence {
    /// The curves coincide (`rbAreCoinc`).
    pub coincident: bool,
    /// Their senses are opposite (`rbReversed`); `None` is `UNSURE`.
    pub reversed: Option<bool>,
}

/// A cone (`SmCone`) or cylinder (`SmCylinder`): the surface of revolution of a line
/// at the semi-angle to the axis, `S(u, v) = O + R(v) (cos u X + sin u Y) + v Z` with
/// `R(v) = base_radius + tan(semi_angle) v`, `u` in degrees and `v` the height along
/// the axis from the origin.
#[derive(Debug, Clone)]
pub struct Cone {
    /// The `SmSurfOfRevolution` base object.
    pub(crate) rev: SurfOfRevolution,
    /// `m_dBaseRadius`: the radius at the origin.
    pub(crate) base_radius: f64,
    /// `m_dSemiAngleDeg`: the angle from the axis to the surface, in degrees
    /// (negative when the radius shrinks along the axis).
    pub(crate) semi_angle_deg: f64,
    /// `m_bInsideOut`: the NURB's linear direction runs from the top to the bottom.
    pub(crate) inside_out: bool,
    /// The object is an `SmCylinder`.
    pub(crate) cylinder: bool,
}

/// The cone's `ConvertUVFromNURBSToSTEP` on its base parts (the virtual the
/// inherited `UpdateAnalyticalDomain` calls); see [`Cone::convert_uv_from_nurbs_to_step`].
pub(crate) fn cone_convert_uv_from_nurbs_to_step(
    rev: &SurfOfRevolution,
    inside_out: bool,
    k: &Kernel,
    nurbs_uv: Point2,
) -> Result<Point2> {
    // swap NurbPoint when needed
    let step_ivl = rev.anal_uv_domain.v_interval();
    let nurb_domain = rev.base.natural_uv_domain();
    let (nurb_uv, nurb_ivl) = if rev.swap_uv {
        (
            Point2::new(nurbs_uv.y, nurbs_uv.x),
            nurb_domain.u_interval(),
        )
    } else {
        (nurbs_uv, nurb_domain.v_interval())
    };

    // convert sweep coordinate
    let circle = rev
        .polar_curve()
        .ok_or(SolidError::null_pointer(OP_TO_STEP))?;
    let (x, _exact) = rev.polar.convert_to_polar_parameter(k, circle, nurb_uv.x)?;

    // convert linear coordinate
    let y = if rev.is_bounded()? {
        if inside_out {
            step_ivl.min() + ((nurb_uv.y - nurb_ivl.max()) / -nurb_ivl.length() * step_ivl.length())
        } else {
            step_ivl.min() + ((nurb_uv.y - nurb_ivl.min()) / nurb_ivl.length() * step_ivl.length())
        }
    } else if inside_out {
        -nurb_uv.y
    } else {
        nurb_uv.y
    };
    Ok(Point2::new(x, y))
}

/// The cone's `CreateIsoParametricCurve` on its base parts (the virtual the
/// inherited NURB rebuild calls).
pub(crate) fn cone_iso_curve(
    rev: &SurfOfRevolution,
    inside_out: bool,
    k: &Kernel,
    surf_param: SurfParamType,
    iso_parameter: f64,
    domain: Option<&Extent2d>,
    uv_iso_curve: bool,
) -> Result<RevolutionIsoCurve> {
    // get the BSplineCurve
    let iso = rev.base.create_iso_parametric_curve(
        k,
        surf_param,
        iso_parameter,
        IsoCurveOptions {
            domain: domain.copied(),
            uv_iso_curve,
        },
    )?;

    // low work - when curve is degenerate don't bother making a circle or line
    if iso.curve.is_degenerate(k, EFF_ZERO, None)? {
        return Ok(RevolutionIsoCurve {
            curve: Curve::from(iso.curve),
            max_gap_3d: iso.max_gap_3d,
            uv_iso_curve: iso.uv_iso_curve,
        });
    }
    let iso_curve = &iso.curve;

    // Step Isoparameter direction
    let step_param = if rev.swap_uv {
        if surf_param == SurfParamType::U {
            SurfParamType::V
        } else {
            SurfParamType::U
        }
    } else {
        surf_param
    };

    // Cone locals
    let cone_origin = rev.position.origin();
    let cone_x_axis = rev.position.x_axis();
    let cone_y_axis = rev.position.y_axis();
    let cone_z_axis = rev.position.z_axis();

    // Nurb Surface size - trimmed to optional domain
    let nurb_domain = rev.base.natural_uv_domain();
    let nurb_span_size = if surf_param == SurfParamType::U {
        let min_y = domain.map_or(nurb_domain.min().y, |d| d.min().y);
        let max_y = domain.map_or(nurb_domain.max().y, |d| d.max().y);
        max_y - min_y
    } else {
        let min_x = domain.map_or(nurb_domain.min().x, |d| d.min().x);
        let max_x = domain.map_or(nurb_domain.max().x, |d| d.max().x);
        max_x - min_x
    };

    // pIsoCurve Sample Points - watch for reversed linear isoCurves (statuses ignored
    // upstream, whose points would then be uninitialised)
    let nurb_ivl = iso_curve.natural_interval();
    let (min_point, max_point) = if inside_out && step_param == SurfParamType::U {
        let max_point = iso_curve.evaluate_point(nurb_ivl.min())?;
        let min_point = iso_curve.evaluate_point(nurb_ivl.max())?;
        (min_point, max_point)
    } else {
        let min_point = iso_curve.evaluate_point(nurb_ivl.min())?;
        let max_point = iso_curve.evaluate_point(nurb_ivl.max())?;
        (min_point, max_point)
    };
    let mid_point = iso_curve.evaluate_point(nurb_ivl.mid())?;

    let scaled_zero = EFF_ZERO
        * (1.0
            + max3(
                min_point.max_dimension(),
                mid_point.max_dimension(),
                cone_origin.max_dimension(),
            ));

    // branch on direction to make and return a circle or a line
    let curve = if step_param == SurfParamType::V {
        // constant V, varying U is a circle

        // Offset vector = projection of MidPoint onto cone ZAxis (averaged)
        let offset_mid = (mid_point - cone_origin).dot(cone_z_axis);
        let offset_min = (min_point - cone_origin).dot(cone_z_axis);
        let offset_max = (max_point - cone_origin).dot(cone_z_axis);
        let offset = (offset_mid + offset_min + offset_max) / 3.0;

        // Center = O + offset * Z
        let center = cone_origin + offset * cone_z_axis;
        let start_vec = min_point - center;
        let end_vec = max_point - center;

        // Get isoCurve analytic domain (SetMinMax's status is ignored: a reversed
        // pair leaves the "inside out" init)
        let mut anal_interval = Extent1d::new();
        if let Some(_dom) = domain {
            // let Ivl = angles (deg) to min/max points measured in circle coordinate
            // system (statuses ignored upstream; a failed angle is 0)
            let z_axis = cone_z_axis;
            let angle_min_rad =
                ignored(z_axis.ccw_angle_between(cone_x_axis, start_vec))?.unwrap_or(0.0);
            let angle_max_rad =
                ignored(z_axis.ccw_angle_between(cone_x_axis, end_vec))?.unwrap_or(0.0);
            let angle_min_deg = angle_min_rad * 180.0 / PI;
            let mut angle_max_deg = angle_max_rad * 180.0 / PI;
            if angle_max_deg < angle_min_deg
                || (is_zero(angle_max_deg - angle_min_deg) && !is_zero(nurb_span_size))
            {
                angle_max_deg += 360.0;
            }
            let _ = anal_interval.set_min_max(angle_min_deg, angle_max_deg);
        } else {
            // let anlytic interval = cone sweep interval
            let _ =
                anal_interval.set_min_max(rev.anal_uv_domain.min().x, rev.anal_uv_domain.max().x);
        }

        // radius = averaged length of the point to center vectors
        let radius_min = start_vec.length();
        let radius_mid = (mid_point - center).length();
        let radius_max = end_vec.length();
        let radius = (radius_min + radius_mid + radius_max) / 3.0;

        // create the circle using the isoCurve parameterization
        let circle = Ellipse::new_circle(
            k,
            center,
            cone_x_axis,
            cone_y_axis,
            anal_interval,
            radius,
            &EllipseOptions {
                dim: 3,
                nurb: Some(iso_curve),
                inside_out: false,
            },
        )?;
        Curve::from(circle)
    } else {
        // contant U, varying V is a line

        // direction and scale
        let mut line_vec = max_point - min_point;
        let length = line_vec.length();
        if length < scaled_zero {
            return Err(status(OP_ISO, SM_ERR_INVALID_INPUT));
        }
        let scale = length / line_vec.dot(cone_z_axis);
        line_vec /= length;

        // Get isoCurve analytic domain (SetMinMax's status is ignored)
        let mut anal_interval = Extent1d::new();
        if domain.is_none() {
            // let anlytic interval = cone linear interval
            let _ =
                anal_interval.set_min_max(rev.anal_uv_domain.min().y, rev.anal_uv_domain.max().y);
        } else {
            // let Ivl = heights to min/max points
            let min_height = (min_point - cone_origin).dot(cone_z_axis);
            let max_height = (max_point - cone_origin).dot(cone_z_axis);
            let _ = anal_interval.set_min_max(min_height, max_height);
        }

        // pick StartPoint to set IsoCurveMinPoint = StartPoint + sAnalIvl.GetMin() * dScale * dVec
        let start_point = min_point - scale * anal_interval.min() * line_vec;

        // allocate the line - with analytic domain
        let mut line = Line::bounded(
            start_point,
            line_vec,
            &anal_interval,
            &BoundedLineOptions {
                scale,
                dim: 3,
                nurb: Some(iso_curve),
                inside_out,
            },
        );

        // set pLine Nurb domain (status ignored)
        ignore_status(line.edit_parameterization(&nurb_ivl))?;
        Curve::from(line)
    };
    Ok(RevolutionIsoCurve {
        curve,
        max_gap_3d: iso.max_gap_3d,
        uv_iso_curve: iso.uv_iso_curve,
    })
}

impl Cone {
    /// `SmCone_TYPE` (`SURF_BASE_TYPE + 12`).
    pub const TYPE: i64 = CONE_TYPE;
    /// `SmCylinder_TYPE` (`SURF_BASE_TYPE + 13`).
    pub const CYLINDER_TYPE: i64 = CYLINDER_TYPE;

    // ---- construction ---------------------------------------------------------------

    /// The empty cone (or cylinder) `ReadFromDB` fills (`SmCone()`, `SmCylinder()`):
    /// the empty surface of revolution, undefined radius and semi-angle.
    #[must_use]
    pub fn empty(cylinder: bool) -> Self {
        Self {
            rev: SurfOfRevolution::empty(),
            base_radius: UNDEF_DOUBLE,
            semi_angle_deg: UNDEF_DOUBLE,
            inside_out: false,
            cylinder,
        }
    }

    /// The `SmSurfOfRevolution(NULL, origin, x, y, domain, swap_uv)` base object.
    fn rev_base(
        origin: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        domain: Extent2d,
        swap_uv: bool,
    ) -> SurfOfRevolution {
        let mut rev = SurfOfRevolution::empty();
        rev.planar_generator = true;
        rev.position = Axis2Placement::new(origin, x_axis, y_axis);
        rev.anal_uv_domain = domain;
        rev.swap_uv = swap_uv;

        // set form factor
        rev.base
            .set_b_spline_surface_form(BSplineSurfaceForm::SurfOfRevolution);
        rev
    }

    /// The infinitely long cone with radius `base_radius` at `origin` and semi-angle
    /// `semi_angle_deg` (protected constructor `SmCone(Origin, XAxis, YAxis,
    /// BotRadius, SemiAngleDeg, bSwapUV, bInsideOut)`), without NURB: angles `[0,
    /// 360]` by heights `[apex, SM_INFINITE_PARAMETER]` (`[-SM_INFINITE_PARAMETER,
    /// SM_INFINITE_PARAMETER]` for a cylinder). The generator line starts at `origin +
    /// base_radius X`, along `X x Y` rotated about `Y` by the semi-angle, with speed
    /// `1 / cos(semi_angle)` so that its parameter is the height. Adjusting a cone's
    /// line to its apex makes the cone's NURB (see the module notes).
    ///
    /// # Errors
    ///
    /// Those of the apex (the generator's inversion), and [`SolidError::Unported`]
    /// from the NURB made for the line's owner.
    pub fn new_unbounded(
        k: &Kernel,
        origin: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        base_radius: f64,
        semi_angle_deg: f64,
        swap_uv: bool,
        inside_out: bool,
    ) -> Result<Self> {
        let mut cone = Self {
            rev: Self::rev_base(origin, x_axis, y_axis, Extent2d::new(), swap_uv),
            base_radius,
            semi_angle_deg,
            inside_out,
            cylinder: false,
        };

        // Create generator curve (SmLine)
        let line_pnt = origin + cone.base_radius * x_axis;
        let z_axis = x_axis.cross(y_axis);
        let mut line_vec = z_axis;
        let angle_rad = deg2rad(cone.semi_angle_deg);
        let scale = 1.0 / angle_rad.cos();
        if angle_rad > EFF_ZERO {
            let pnt = origin + z_axis * scale;
            let mut rf = Axis2Placement::IDENTITY; // m_vPosition leads to bug
            rf.rotate_about_axis_at_point(angle_rad, origin, y_axis);
            let pnt = rf.transform_point(pnt);
            line_vec = pnt - origin;
        }
        // unitize LineVec (status ignored)
        let line_vec = line_vec.unitize_or_z();

        // init domain to +/- infinity for cylinders - cones will be adjusted
        let _ = cone.rev.anal_uv_domain.set_min_max_coords(
            0.0,
            -INFINITE_PARAMETER,
            360.0,
            INFINITE_PARAMETER,
        );

        // build generator curve
        let line = Line::bounded(
            line_pnt,
            line_vec,
            &cone.rev.anal_uv_domain.v_interval(),
            &BoundedLineOptions {
                scale,
                dim: 3,
                nurb: None,
                inside_out,
            },
        );
        cone.rev.gen_curve = Some(Curve::from(line));

        // The parameterization of the generator is now the height from the origin.
        // (status ignored)
        let apex = cone.apex(k)?;

        // infinite cylinders run between +/- infinity
        // infinite cones     run between apex and +infinity
        if !apex.is_cylinder {
            let _ = cone.rev.anal_uv_domain.set_min_max_coords(
                0.0,
                apex.param,
                360.0,
                INFINITE_PARAMETER,
            );
            // adjust generator curve domain
            cone.adjust_gen_line_with_owner(
                k,
                &Extent1d::from_min_max(apex.param, INFINITE_PARAMETER),
            )?;
        }
        Ok(cone)
    }

    /// `SmLine::AdjustSTEPInterval` of the generator line owned by this cone: when the
    /// interval changes, the line's NURB is rebuilt and, its owner being a surface of
    /// revolution, reparameterised to the cone's generator-direction NURB interval
    /// (whose query makes the cone's NURB when missing).
    fn adjust_gen_line_with_owner(&mut self, k: &Kernel, ivl: &Extent1d) -> Result<()> {
        let Some(CurveKind::Line(line)) = self.rev.gen_curve.as_mut().map(Curve::kind_mut) else {
            return Err(SolidError::null_pointer(OP_GEN));
        };
        let dom = line.step_interval();
        if ivl.is_contained_by(&dom, EFF_ZERO) && dom.is_contained_by(ivl, EFF_ZERO) {
            return Ok(());
        }
        let had_nurb = line.base().gw_nurb().is_some();
        line.adjust_step_interval(ivl)?;
        if !had_nurb {
            return Ok(());
        }

        // (SmLine::MakeNurb, owner SmSurfOfRevolution) GetGenDirParamExtent ->
        // GetNaturalUVDomain -> GetOrCreateGwNurbPointer (status ignored)
        if self.rev.base.gw_nurb().is_none() {
            ignore_status(self.make_nurb(k))?;
        }
        let (nurb_ivl, _) = self.rev.gen_dir_param_extent();
        if let Some(CurveKind::Line(line)) = self.rev.gen_curve.as_mut().map(Curve::kind_mut) {
            // (EditParameterization's status ignored)
            ignore_status(line.edit_parameterization(&nurb_ivl))?;
        }
        Ok(())
    }

    /// The bounded cone from the bottom circle of radius `bot_radius` at `origin` to
    /// the top circle of radius `top_radius` at `height` along `X x Y`, over the angles
    /// `[start_angle, end_angle]` (constructor `SmCone(Origin, XAxis, YAxis, BotRadius,
    /// TopRadius, StartAngle, EndAngle, Height, bSwapUV, bInsideOut,
    /// bMakeNurbGenCurve)`), without NURB. The semi-angle is
    /// `atan2(top - bot, height)` in degrees (`0` when the radii agree within
    /// `SM_EFF_ZERO`); the generator line runs from `origin + bot_radius X` over the
    /// heights `[0, height]`.
    #[must_use]
    pub fn new_bounded(
        origin: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        bot_radius: f64,
        top_radius: f64,
        start_angle: f64,
        end_angle: f64,
        height: f64,
        swap_uv: bool,
        inside_out: bool,
        make_nurb_gen_curve: bool,
    ) -> Self {
        let mut cone = Self {
            rev: Self::rev_base(
                origin,
                x_axis,
                y_axis,
                Extent2d::from_bounds(start_angle, 0.0, end_angle, height),
                swap_uv,
            ),
            base_radius: bot_radius,
            semi_angle_deg: 0.0,
            inside_out,
            cylinder: false,
        };

        // Specify GenCurve type
        cone.rev.make_nurb_gen_curve = make_nurb_gen_curve;

        // modify m_dSemiAngleDeg for nonCylinder shapes
        let radius_diff = top_radius - bot_radius;
        if radius_diff.abs() > EFF_ZERO {
            cone.semi_angle_deg = radius_diff.atan2(height) * 180.0 / PI;
        }

        // The parameterization of the generator is the height from the origin.
        let z_axis = x_axis.cross(y_axis).unitize_or_z();
        let line_vec = height * z_axis + (top_radius - bot_radius) * x_axis;
        let length = line_vec.length();
        let line = Line::bounded(
            origin + bot_radius * x_axis,
            line_vec / length,
            &Extent1d::from_min_max(0.0, height),
            &BoundedLineOptions {
                scale: length / height,
                dim: 3,
                nurb: None,
                inside_out,
            },
        );
        cone.rev.gen_curve = Some(Curve::from(line));
        cone
    }

    /// The infinite cylinder of radius `radius` about the axis through `origin`
    /// (protected constructor `SmCylinder(Origin, XAxis, YAxis, Radius, bSwapUV,
    /// bInsideOut)`: [`Self::new_unbounded`] with semi-angle `0`).
    ///
    /// # Errors
    ///
    /// Those of [`Self::new_unbounded`].
    pub fn new_cylinder_unbounded(
        k: &Kernel,
        origin: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        radius: f64,
        swap_uv: bool,
        inside_out: bool,
    ) -> Result<Self> {
        let mut c =
            Self::new_unbounded(k, origin, x_axis, y_axis, radius, 0.0, swap_uv, inside_out)?;
        c.cylinder = true;
        Ok(c)
    }

    /// The bounded cylinder (constructor `SmCylinder(Origin, XAxis, YAxis, Radius,
    /// StartAngleDeg, EndAngleDeg, Height, bSwapUV, bInsideOut, bMakeNurbGenCurve)`:
    /// [`Self::new_bounded`] with equal radii).
    #[must_use]
    pub fn new_cylinder_bounded(
        origin: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
        height: f64,
        swap_uv: bool,
        inside_out: bool,
        make_nurb_gen_curve: bool,
    ) -> Self {
        let mut c = Self::new_bounded(
            origin,
            x_axis,
            y_axis,
            radius,
            radius,
            start_angle,
            end_angle,
            height,
            swap_uv,
            inside_out,
            make_nurb_gen_curve,
        );
        c.cylinder = true;
        c
    }

    /// Makes the NURB (the inherited `SmSurfOfRevolution::MakeNurb`), its polar
    /// converter rebuilt with the cone's iso curves.
    ///
    /// # Errors
    ///
    /// Those of `SmSurfOfRevolution::MakeNurb`.
    pub fn make_nurb(&mut self, k: &Kernel) -> Result<()> {
        let inside_out = self.inside_out;
        self.rev
            .make_nurb_with_sweep_params_using(k, None, &|s, k, p, t| {
                Ok(cone_iso_curve(s, inside_out, k, p, t, None, false)?.curve)
            })
    }

    /// Builds the NURB of a new cone (the tail of `CreateCanonical`): `SM_ERR` when
    /// `MakeNurb` fails or leaves no NURB.
    fn finish_canonical(mut self, k: &Kernel, operation: &'static str) -> Result<Self> {
        // Build NURB; fail cleanly if degenerate (single zero-radius apex is valid).
        match self.make_nurb(k) {
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::sm_err(operation)),
            Ok(()) => {}
        }
        if self.rev.base.gw_nurb().is_none() {
            return Err(SolidError::sm_err(operation));
        }
        Ok(self)
    }

    /// The cone of radius `bot_radius` at the placement's origin and semi-angle
    /// `semi_angle_deg` (`CreateCanonical(Origin, BotRadius, SemiAngleDeg, pOptHeight,
    /// bSwapUV, bInsideOut)`): infinitely long without `height` ([`Self::new_unbounded`]),
    /// else bounded by the top circle at `height` ([`Self::new_bounded`] over `[0,
    /// 360]`; upstream passes `FALSE` as its `bInsideOut` and `inside_out` as its
    /// `bMakeNurbGenCurve`), with its NURB.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the NURB cannot be made; those of [`Self::new_unbounded`].
    pub fn create_canonical_with_semi_angle(
        k: &Kernel,
        origin: &Axis2Placement,
        bot_radius: f64,
        semi_angle_deg: f64,
        height: Option<f64>,
        swap_uv: bool,
        inside_out: bool,
    ) -> Result<Self> {
        // construct parameterized cone
        let cone = match height {
            None => Self::new_unbounded(
                k,
                origin.origin(),
                origin.x_axis(),
                origin.y_axis(),
                bot_radius,
                semi_angle_deg,
                swap_uv,
                inside_out,
            )?,
            Some(h) => {
                let top_radius = bot_radius + deg2rad(semi_angle_deg).tan() * h;
                Self::new_bounded(
                    origin.origin(),
                    origin.x_axis(),
                    origin.y_axis(),
                    bot_radius,
                    top_radius,
                    0.0,
                    360.0,
                    h,
                    swap_uv,
                    false,
                    inside_out,
                )
            }
        };
        cone.finish_canonical(k, OP_CANONICAL)
    }

    /// The cone from the bottom circle of radius `bot_radius` at the placement's origin
    /// to the top circle of radius `top_radius` at `height` (`CreateCanonical(Origin,
    /// BotRadius, TopRadius, Height, bSwapUV, bInsideOut)`), over `[0, 360]`, with its
    /// NURB. For `height <= 0` upstream makes the infinite cone of
    /// [`Self::new_unbounded`] with `top_radius` as the semi-angle in degrees.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the NURB cannot be made; those of [`Self::new_unbounded`].
    pub fn create_canonical(
        k: &Kernel,
        origin: &Axis2Placement,
        bot_radius: f64,
        top_radius: f64,
        height: f64,
        swap_uv: bool,
        inside_out: bool,
    ) -> Result<Self> {
        // construct parameterized cone
        let cone = if height <= 0.0 {
            Self::new_unbounded(
                k,
                origin.origin(),
                origin.x_axis(),
                origin.y_axis(),
                bot_radius,
                top_radius,
                swap_uv,
                inside_out,
            )?
        } else {
            Self::new_bounded(
                origin.origin(),
                origin.x_axis(),
                origin.y_axis(),
                bot_radius,
                top_radius,
                0.0,
                360.0,
                height,
                swap_uv,
                inside_out,
                false,
            )
        };
        cone.finish_canonical(k, OP_CANONICAL)
    }

    /// The infinite cylinder of radius `radius` about the placement's Z axis
    /// (`SmCylinder::CreateCanonical`), with its NURB.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a radius not above `SM_EFF_ZERO` or when the NURB cannot be made;
    /// those of [`Self::new_cylinder_unbounded`].
    pub fn create_canonical_cylinder(
        k: &Kernel,
        origin: &Axis2Placement,
        radius: f64,
    ) -> Result<Self> {
        // Reject non-positive radius (ctor assert is a no-op in release).
        if radius.partial_cmp(&EFF_ZERO) != Some(core::cmp::Ordering::Greater) {
            return Err(SolidError::sm_err(OP_CYL_CANONICAL));
        }
        let cylinder = Self::new_cylinder_unbounded(
            k,
            origin.origin(),
            origin.x_axis(),
            origin.y_axis(),
            radius,
            false,
            false,
        )?;
        cylinder.finish_canonical(k, OP_CYL_CANONICAL)
    }

    // ---- access ---------------------------------------------------------------------

    /// The `SmSurfOfRevolution` base object.
    #[must_use]
    pub const fn rev(&self) -> &SurfOfRevolution {
        &self.rev
    }

    /// Enables or disables out-of-bounds evaluation of the surface and of its
    /// generator (`SmSurfOfRevolution::SetOutOfBoundsEnabled`, inherited).
    pub fn set_out_of_bounds_enabled(&mut self, enabled: bool) {
        self.rev.set_out_of_bounds_enabled(enabled);
    }

    /// The object is an `SmCylinder` (not only a cone of semi-angle 0).
    #[must_use]
    pub const fn is_cylinder_class(&self) -> bool {
        self.cylinder
    }

    /// The radius at the origin (`m_dBaseRadius`).
    #[must_use]
    pub const fn base_radius(&self) -> f64 {
        self.base_radius
    }

    /// The semi-angle in degrees (`m_dSemiAngleDeg`).
    #[must_use]
    pub const fn semi_angle_deg(&self) -> f64 {
        self.semi_angle_deg
    }

    /// The NURB's linear direction runs from the top (`GetInsideOut`).
    #[must_use]
    pub const fn inside_out(&self) -> bool {
        self.inside_out
    }

    /// The radius at the height `height` (`GetRadius`).
    #[must_use]
    pub fn radius(&self, height: f64) -> f64 {
        self.base_radius + deg2rad(self.semi_angle_deg).tan() * height
    }

    /// The radius at the bottom of the STEP domain (`GetBotRadius`; `SmCylinder`'s
    /// `GetRadius()`).
    #[must_use]
    pub fn bot_radius(&self) -> f64 {
        self.radius(self.rev.anal_uv_domain.min().y)
    }

    /// The radius at the top of the STEP domain (`GetTopRadius`).
    #[must_use]
    pub fn top_radius(&self) -> f64 {
        self.radius(self.rev.anal_uv_domain.max().y)
    }

    /// The bottom height (`GetBotHeight`).
    #[must_use]
    pub const fn bot_height(&self) -> f64 {
        self.rev.anal_uv_domain.min().y
    }

    /// The top height (`GetTopHeight`).
    #[must_use]
    pub const fn top_height(&self) -> f64 {
        self.rev.anal_uv_domain.max().y
    }

    /// `IsCylinder`: always for an `SmCylinder`; for a cone, a semi-angle below
    /// `SM_EFF_ZERO` in magnitude.
    #[must_use]
    pub fn is_cylinder(&self) -> bool {
        self.cylinder || self.semi_angle_deg.abs() < EFF_ZERO
    }

    /// The canonical data (`GetCanonical`).
    #[must_use]
    pub const fn canonical(&self) -> ConeCanonical {
        ConeCanonical {
            origin: self.rev.position,
            radius: self.base_radius,
            semi_angle_deg: self.semi_angle_deg,
            swap_uv: self.rev.swap_uv,
            inside_out: self.inside_out,
        }
    }

    /// `SmCone_TYPE` or `SmCylinder_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        if self.cylinder {
            CYLINDER_TYPE
        } else {
            CONE_TYPE
        }
    }

    /// `"SmCone_TYPE"` or `"SmCylinder_TYPE"` (`GetTypeString`: `SM_COMMON`'s `#ty`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        if self.cylinder {
            "SmCylinder_TYPE"
        } else {
            "SmCone_TYPE"
        }
    }

    /// `"SmCone"` or `"SmCylinder"` (`GetClassString`: `SM_COMMON`'s `#cl`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        if self.cylinder {
            "SmCylinder"
        } else {
            "SmCone"
        }
    }

    /// `IsKindOf`: the cylinder type (for an `SmCylinder`), the cone type, or a type
    /// the surface of revolution is.
    ///
    /// # Errors
    ///
    /// Those of [`SurfOfRevolution::is_kind_of`].
    pub fn is_kind_of(&self, t: i64) -> Result<bool> {
        if self.cylinder && t == CYLINDER_TYPE {
            return Ok(true);
        }
        if t == CONE_TYPE {
            return Ok(true);
        }
        self.rev.is_kind_of(t)
    }

    fn gen(&self) -> Result<&Curve> {
        self.rev
            .gen_curve
            .as_ref()
            .ok_or(SolidError::null_pointer(OP_GEN))
    }

    // ---- equality -------------------------------------------------------------------

    /// `operator==`: the same type, equal NURBs (`SmBSplineSurface::operator==`), and
    /// base radii and semi-angles equal to the scaled zero, equal inside-out flags.
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        // low work
        if core::ptr::eq(self, other) {
            return true;
        }

        // first check the base (SmSurface: the types; SmBSplineSurface: the NURBs)
        if self.type_code() != other.type_code() || !self.rev.base.approx_eq(&other.rev.base) {
            return false;
        }

        // check equivalence of these objects
        is_zero(self.base_radius - other.base_radius)
            && is_zero(self.semi_angle_deg - other.semi_angle_deg)
            && self.inside_out == other.inside_out
    }

    // ---- domains --------------------------------------------------------------------

    /// Sets the STEP domain (`AdjustSTEPUVDomain`), unless a new height end would lie
    /// past the apex (a radius below `-SM_EFF_ZERO`).
    ///
    /// # Errors
    ///
    /// `SM_ERR_INVALID_INPUT` past the apex; otherwise [`SolidError::Unported`]
    /// (`SmSurfOfRevolution::AdjustSTEPUVDomain`, unit `surface.surf_of_revolution_p2`).
    pub fn adjust_step_uv_domain(
        &mut self,
        k: &Kernel,
        new_step_uv_domain: &Extent2d,
    ) -> Result<()> {
        // Method: just call the parent's version,
        // unless the new domain would result in going past the apex.
        let v_domain = new_step_uv_domain.v_interval();
        if self.radius(v_domain.min()) < -EFF_ZERO {
            return Err(status(OP_ADJUST, SM_ERR_INVALID_INPUT));
        }
        if self.radius(v_domain.max()) < -EFF_ZERO {
            return Err(status(OP_ADJUST, SM_ERR_INVALID_INPUT));
        }

        // the parent's version, with this cone's iso curves for the polar converter
        let inside_out = self.inside_out;
        self.rev
            .adjust_step_uv_domain_using(k, new_step_uv_domain, &|s, k, p, t| {
                Ok(cone_iso_curve(s, inside_out, k, p, t, None, false)?.curve)
            })
    }

    /// The apex (`GetApex`): for a cylinder (a semi-angle zero to the scaled zero) the
    /// origin with parameter `0`; otherwise the point at height `-base_radius /
    /// tan(semi_angle)` on the axis and its generator STEP parameter.
    ///
    /// # Errors
    ///
    /// Those of the generator's `STEPInversion`.
    pub fn apex(&self, k: &Kernel) -> Result<Apex> {
        let is_cylinder = is_zero(self.semi_angle_deg);
        if is_cylinder {
            return Ok(Apex {
                is_cylinder,
                point: self.rev.position.origin(),
                param: 0.0,
            });
        }
        let height = -self.base_radius / (self.semi_angle_deg * PI / 180.0).tan();
        let point = self.rev.position.origin() + height * self.rev.position.z_axis();
        let param = self.gen()?.step_inversion(k, point)?;
        Ok(Apex {
            is_cylinder,
            point,
            param,
        })
    }

    /// The height (`GetHeight`): the STEP domain's height span without NURB, else the
    /// height difference along the axis of the points at the NURB domain's top and
    /// bottom corners (swapped when inside out).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from evaluation. Other evaluation failures retain
    /// the default [`Vec3::UNDEF`] point, as upstream's ignored statuses do.
    pub fn height(&self, k: &Kernel) -> Result<f64> {
        if self.rev.base.gw_nurb().is_none() {
            return Ok(self.rev.anal_uv_domain.max().y - self.rev.anal_uv_domain.min().y);
        }
        let nurb_domain = self.rev.base.natural_uv_domain();
        let (top_uv, bot_uv) = if self.inside_out {
            (nurb_domain.min(), nurb_domain.max())
        } else {
            (nurb_domain.max(), nurb_domain.min())
        };
        let top = ignored(self.evaluate_point(k, top_uv))?.unwrap_or(Vec3::UNDEF);
        let bot = ignored(self.evaluate_point(k, bot_uv))?.unwrap_or(Vec3::UNDEF);
        Ok((top - bot).dot(self.rev.position.z_axis()))
    }

    /// The STEP height interval and its STEP direction, always v
    /// (`GetSTEPLinearParamExtent`).
    #[must_use]
    pub fn step_linear_param_extent(&self) -> (Extent1d, SurfParamType) {
        (self.rev.anal_uv_domain.v_interval(), SurfParamType::V)
    }

    /// The largest analytic domain (`GetMaxAnalyticDomain`): `[0, 360]` by the
    /// generator's maximum domain, limited at the apex when that is not inside the
    /// current height interval (to `SM_EFF_ZERO`); an `SmCylinder` is never limited.
    ///
    /// # Errors
    ///
    /// Those of the generator's domain and of [`Self::apex`].
    pub fn max_analytic_domain(&self, k: &Kernel) -> Result<Extent2d> {
        if self.cylinder {
            // The v-domain is the domain of the generator curve, which is a line.
            let v_dom = gen_max_analytic_domain(k, self.gen()?)?;
            return Ok(Extent2d::from_bounds(0.0, v_dom.min(), 360.0, v_dom.max()));
        }

        // The v-domain is the domain of the generator curve, limited to our apex.
        let (mut v_dom, _) = self.step_linear_param_extent();

        // Get the apex parameter (status ignored).
        let apex = self.apex(k)?;

        let v_dom_big = gen_max_analytic_domain(k, self.gen()?)?;
        if apex.is_cylinder {
            v_dom = v_dom_big;
        } else if apex.param <= v_dom.min() + EFF_ZERO {
            let _ = v_dom.set_min_max(apex.param, v_dom_big.max());
        } else if apex.param >= v_dom.max() - EFF_ZERO {
            let _ = v_dom.set_min_max(v_dom_big.min(), apex.param);
        }

        // u-domain is 0 to 360.
        Ok(Extent2d::from_bounds(0.0, v_dom.min(), 360.0, v_dom.max()))
    }

    // ---- iso curves -----------------------------------------------------------------

    /// The iso-parametric curve at the NURB parameter `iso_parameter` in `surf_param`'s
    /// direction (`CreateIsoParametricCurve`): the B-spline extraction of the base
    /// surface ([`crate::nurbs::b_spline_surface::BSplineSurface::create_iso_parametric_curve`], trimmed to `domain`,
    /// with its UV line when `uv_iso_curve`), kept when degenerate, else replaced by
    /// the circle (sweep direction) or line (linear direction) with that NURB. The
    /// circle's centre and radius average the curve's start, middle and end points; its
    /// angular domain is the STEP sweep, or with `domain` the angles of its end points.
    /// The line's height domain is the STEP one, or with `domain` the heights of its
    /// end points; its NURB keeps the curve's interval.
    ///
    /// # Errors
    ///
    /// `SM_ERR_INVALID_INPUT` for a line shorter than the scaled zero; those of the
    /// extraction, the point evaluations and the circle.
    pub fn create_iso_parametric_curve(
        &self,
        k: &Kernel,
        surf_param: SurfParamType,
        iso_parameter: f64,
        domain: Option<&Extent2d>,
        uv_iso_curve: bool,
    ) -> Result<RevolutionIsoCurve> {
        cone_iso_curve(
            &self.rev,
            self.inside_out,
            k,
            surf_param,
            iso_parameter,
            domain,
            uv_iso_curve,
        )
    }

    /// The sweep iso circle through the height of `reference_point`
    /// (`CreateIsoCircleFromPoint`): `None` when the point's projection onto the axis
    /// is not within `tol_3d` of the height interval; otherwise the iso curve (circle,
    /// or degenerate B-spline at the apex) through the cone point at that height in the
    /// point's direction (the X axis for a point on the axis).
    ///
    /// # Errors
    ///
    /// Those of [`Self::step_inversion`] that leave no parameters (status ignored
    /// upstream), [`Self::convert_uv_from_step_to_nurbs`] (status ignored upstream;
    /// returned unless `swap_uv` is set and the polar converter wrote the value used)
    /// and [`Self::create_iso_parametric_curve`].
    pub fn create_iso_circle_from_point(
        &self,
        k: &Kernel,
        reference_point: Point3,
        tol_3d: f64,
    ) -> Result<Option<Curve>> {
        // project Point to RevolutionAxis
        let origin = self.rev.position.origin();
        let z_axis = self.rev.position.z_axis();
        let height_step_ivl = self.rev.anal_uv_domain.v_interval();
        let param = line_closest_point(origin, z_axis, reference_point);

        // skip points not within the height bounds of the cone
        if !height_step_ivl.contains_value(param, tol_3d) {
            return Ok(None);
        }

        // build Step Point on Surface for Point
        let radius = self.radius(param);
        let center = origin + param * z_axis;
        let mut vec = reference_point - center;
        if vec.length_squared() < 2.0 * EFF_ZERO_SQ {
            // Given point is on our axis.
            vec = self.rev.position.x_axis();
        }
        let vec = vec.unitize_or_z();
        let point_on_surface = center + radius * vec;

        // map Point to StepUV. Upstream ignores the status. When the generator drop finds no
        // solution (a point beyond the end by up to `tol_3d`, #1067) the height of its
        // StepUV is never written and the circle is built there (UB-87); the port uses the
        // height of the point, clamped to the end, which is where the point lies within
        // `tol_3d`. Every point whose inversion succeeds is unchanged.
        let step_uv = match self.step_inversion(k, point_on_surface, tol_3d, None) {
            Ok(inv) => inv.uv,
            Err(SolidError::Upstream { code, .. }) if code == SM_ERR => Point2::new(
                self.rev.anal_uv_domain.min().x,
                height_step_ivl.clamp_value(param),
            ),
            Err(e) => return Err(e),
        };

        // map StepUV to NurbUV (status ignored: after a failed polar conversion
        // rNURBSUV.x holds what the converter wrote and rNURBSUV.y is unset)
        let (param_type, iso) = match self.convert_uv_from_step_to_nurbs(k, step_uv) {
            Ok(nurb_uv) if self.rev.swap_uv => (SurfParamType::U, nurb_uv.x),
            Ok(nurb_uv) => (SurfParamType::V, nurb_uv.y),
            Err(e) if e.is_unported() || !self.rev.swap_uv => return Err(e),
            Err(e) => match self.sweep_nurbs_param_written(k, step_uv.x)? {
                Some(x) => (SurfParamType::U, x),
                None => return Err(e),
            },
        };

        // build and return the isoParametercurve
        Ok(Some(
            self.create_iso_parametric_curve(k, param_type, iso, None, false)?
                .curve,
        ))
    }

    // ---- parameter conversion -------------------------------------------------------

    /// The NURB parameters of the STEP parameters `step_uv` (`ConvertUVFromSTEPToNURBS`):
    /// the sweep through the polar converter, the height mapped linearly onto the
    /// NURB's linear interval (reversed when inside out) for a bounded cone, else
    /// equal (negated when inside out); swapped with `swap_uv`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` without polar curve (upstream dereferences `NULL`); those of the
    /// polar conversion.
    pub fn convert_uv_from_step_to_nurbs(&self, k: &Kernel, step_uv: Point2) -> Result<Point2> {
        // convert sweep coordinate
        let circle = self.rev.polar_curve().ok_or(SolidError::invalid_input(
            OP_TO_NURBS,
            "polar converter has no curve",
        ))?;
        let (x, _exact) = self
            .rev
            .polar
            .convert_to_nurbs_parameter(k, circle, step_uv.x, 0.0)?;
        let mut nurbs_uv = Point2::new(x, 0.0);

        // get current STEP and NURB intervals switching on m_bSwapUV as needed
        let step_ivl = self.rev.anal_uv_domain.v_interval();
        let nurb_domain = self.rev.base.natural_uv_domain();
        let nurb_ivl = if self.rev.swap_uv {
            nurb_domain.u_interval()
        } else {
            nurb_domain.v_interval()
        };

        // convert linear coordinate
        nurbs_uv.y = if self.rev.is_bounded()? {
            if self.inside_out {
                nurb_ivl.max()
                    + ((step_uv.y - step_ivl.min()) / step_ivl.length() * -nurb_ivl.length())
            } else {
                nurb_ivl.min()
                    + ((step_uv.y - step_ivl.min()) / step_ivl.length() * nurb_ivl.length())
            }
        } else if self.inside_out {
            -step_uv.y
        } else {
            step_uv.y
        };

        // m_bSwapUV - switch Nurb point coordinates when needed
        if self.rev.swap_uv {
            nurbs_uv = Point2::new(nurbs_uv.y, nurbs_uv.x);
        }
        Ok(nurbs_uv)
    }

    /// The sweep NURB parameter `ConvertUVFromSTEPToNURBS` leaves in `rNURBSUV.x` for
    /// the STEP angle `step_u` when a caller ignores its status: the converted value,
    /// or after a failed conversion the value the polar converter had already written
    /// (`None`: nothing written, an uninitialised value upstream).
    ///
    /// # Errors
    ///
    /// `InvalidInput` without polar curve; [`SolidError::Unported`].
    fn sweep_nurbs_param_written(&self, k: &Kernel, step_u: f64) -> Result<Option<f64>> {
        let circle = self.rev.polar_curve().ok_or(SolidError::invalid_input(
            OP_TO_NURBS,
            "polar converter has no curve",
        ))?;
        self.rev
            .polar
            .convert_to_nurbs_parameter_or_written(k, circle, step_u, 0.0)
    }

    /// The STEP parameters of the NURB parameters `nurbs_uv`
    /// (`ConvertUVFromNURBSToSTEP`): the inverse of
    /// [`Self::convert_uv_from_step_to_nurbs`].
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without polar curve; those of the polar conversion.
    pub fn convert_uv_from_nurbs_to_step(&self, k: &Kernel, nurbs_uv: Point2) -> Result<Point2> {
        cone_convert_uv_from_nurbs_to_step(&self.rev, self.inside_out, k, nurbs_uv)
    }

    // ---- evaluation -----------------------------------------------------------------

    /// The point and derivatives at the NURB parameters `uv` (`Evaluate`): the
    /// derivatives of the base NURB (when any is asked for), with the point replaced
    /// by the STEP evaluation when the parameters convert. Laid out `[D, Dv, .., Du,
    /// Duv, .., Duu, ..]` (`(hu + 1) * (hv + 1)` entries, index `i * (hv + 1) + j`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the NURB evaluation fails.
    pub fn evaluate(
        &self,
        k: &Kernel,
        uv: Point2,
        hu: usize,
        hv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec3>> {
        let rows = self.evaluate_rows(k, uv, hu, hv, flags)?;
        let mut d = Vec::with_capacity((hu + 1) * (hv + 1));
        for row in rows {
            d.extend(row);
        }
        Ok(d)
    }

    /// [`Self::evaluate`] as `D[i][j]` rows: exactly `hu + 1` rows of `hv + 1` entries
    /// (the base evaluation's rows, zero-padded), so a caller wanting rows does not
    /// flatten and re-split them.
    ///
    /// # Errors
    ///
    /// As [`Self::evaluate`].
    pub fn evaluate_rows(
        &self,
        k: &Kernel,
        uv: Point2,
        hu: usize,
        hv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        // when asked for derivatives - pass the call along to base class (which
        // rejects orders above NL_MAXDER before anything is allocated here)
        let mut d = if hu > 0 || hv > 0 {
            match self.rev.base.evaluate(uv, hu, hv, flags) {
                Ok(mut rows) => {
                    rows.truncate(hu + 1);
                    rows.resize_with(hu + 1, Vec::new);
                    for row in &mut rows {
                        row.truncate(hv + 1);
                        row.resize(hv + 1, Vec3::ZERO);
                    }
                    rows
                }
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Err(SolidError::sm_err(OP_EVAL)),
            }
        } else {
            vec![vec![Vec3::ZERO]]
        };

        // use the step position for tighter tolerances (statuses only select it)
        if let Some(step_uv) = ignored(self.convert_uv_from_nurbs_to_step(k, uv))? {
            if let Some(p) = ignored(self.evaluate_step_point(step_uv))? {
                if let Some(slot) = d.first_mut().and_then(|row| row.first_mut()) {
                    *slot = p;
                }
            }
        }
        Ok(d)
    }

    /// The point at the NURB parameters `uv` (`EvaluatePoint`): the STEP point of the
    /// converted parameters.
    ///
    /// # Errors
    ///
    /// Those of [`Self::convert_uv_from_nurbs_to_step`].
    pub fn evaluate_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        // convert to STEP parameter
        let step_uv = self.convert_uv_from_nurbs_to_step(k, uv)?;

        // pass the call along
        self.evaluate_step_point(step_uv)
    }

    /// The point and triangular derivatives at the STEP parameters `uv` (degrees,
    /// height) (`EvaluateSTEP`): `uv` is clamped to the STEP domain unless
    /// out-of-bounds evaluation is enabled; the derivatives are per degree in u. The
    /// result is laid out as [`Self::evaluate`]'s; entries upstream does not write
    /// are `0`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for orders above 3.
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
            return Err(SolidError::sm_err(OP_EVAL_STEP)); // Not implemented
        }

        // locals
        let mut uv = uv;
        let r_o = self.rev.position.origin();
        let r_x = self.rev.position.x_axis();
        let r_y = self.rev.position.y_axis();
        let s_z = self.rev.position.z_axis();
        let linear_ivl = self.rev.anal_uv_domain.v_interval();
        let min_highest = hu.min(hv);

        // per SMLib practice clamp out of bounds UV points
        if !self.rev.is_out_of_bounds_enabled() && !self.rev.anal_uv_domain.contains_point(uv, 0.0)
        {
            uv = self.rev.anal_uv_domain.clamp_point(uv);
        }

        // evaluate expensive radius and trigonometric functions
        let v = uv.y;
        let r = self.radius(v);
        let rv =
            (self.radius(linear_ivl.max()) - self.radius(linear_ivl.min())) / linear_ivl.length();
        let cos_u = (uv.x * PI / 180.0).cos();
        let sin_u = (uv.x * PI / 180.0).sin();

        let n = (hu + 1) * (hv + 1);
        let d = d
            .get_mut(..n)
            .ok_or_else(|| SolidError::invalid_input(OP_EVAL_STEP, "derivative array too short"))?;
        let mut set = |i: usize, val: Vec3| {
            if let Some(slot) = d.get_mut(i) {
                *slot = val;
            }
        };

        // evaluate position
        set(0, r_o + r * (cos_u * r_x + sin_u * r_y) + v * s_z);

        // 1st U and V derivatives
        if hu >= 1 {
            set(hv + 1, r * (-sin_u * r_x + cos_u * r_y));
        }
        if hv >= 1 {
            set(1, rv * (cos_u * r_x + sin_u * r_y) + s_z);
        }

        // cross derivative dUV
        if hu >= 1 && hv >= 1 && min_highest >= 2 {
            set(hv + 2, rv * (-sin_u * r_x + cos_u * r_y));
        }

        //  cross derivative dUVV
        if hu >= 1 && hv >= 2 && min_highest >= 3 {
            set(hv + 3, Vec3::ZERO);
        }

        // 2nd U and V derivatives
        if hu >= 2 {
            set(2 * hv + 2, r * (-cos_u * r_x - sin_u * r_y));
        }
        if hv >= 2 {
            set(2, Vec3::ZERO);
        }

        // cross derivative dUUV
        if hu >= 2 && hv >= 1 && min_highest >= 3 {
            set(2 * hv + 3, rv * (-cos_u * r_x - sin_u * r_y));
        }

        // 3rd U and V derivatives
        if hu >= 3 {
            set(3 * hv + 3, r * (sin_u * r_x - cos_u * r_y));
        }
        if hv >= 3 {
            set(3, Vec3::ZERO);
        }

        // The formulas above are per radian; STEP U is in degrees.
        scale_step_angular_derivatives(d, hu, hv, true, false)?;
        Ok(())
    }

    /// The point at the STEP parameters `uv` (`EvaluateSTEPPoint`).
    ///
    /// # Errors
    ///
    /// None in practice (those of [`Self::evaluate_step`]).
    pub fn evaluate_step_point(&self, uv: Point2) -> Result<Point3> {
        self.evaluate_step(uv, 0, 0)?
            .first()
            .copied()
            .ok_or(SolidError::invalid_input(OP_EVAL_STEP, "empty evaluation"))
    }

    // ---- points ---------------------------------------------------------------------

    /// `GetPositivePoint`: a point on the far side of the apex plane (below it for a
    /// positive semi-angle, above for a negative one) reflected through the apex.
    ///
    /// # Errors
    ///
    /// Those of [`Self::apex`].
    pub fn positive_point(&self, k: &Kernel, test_point: Point3) -> Result<PositivePoint> {
        // see if point lies on far side of cone apex (status ignored)
        let apex = self.apex(k)?;
        let z_axis = self.rev.position.z_axis();
        let negate = !apex.is_cylinder
            && ((self.semi_angle_deg > 0.0 && (test_point - apex.point).dot(z_axis) < 0.0)
                || (self.semi_angle_deg < 0.0 && (test_point - apex.point).dot(z_axis) > 0.0));

        // when point is on far side of apex - reflect it through the Apex Point
        let point = if negate {
            2.0 * apex.point - test_point
        } else {
            test_point
        };
        Ok(PositivePoint {
            negated: negate,
            point,
            apex_param: apex.param,
        })
    }

    /// The STEP parameters and location of `point_on_surf` (`STEPInversion`): the point
    /// is mirrored to the positive side of the apex, inverted on the surface of
    /// revolution (rotated into the start plane for the angle, dropped onto the
    /// generator within `tol_3d` for the height), and the height mirrored back. A
    /// point on the axis is a pole, one on the seam a u seam; on a seam the guess picks
    /// the side.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the point does not drop onto the generator (upstream leaves the
    /// height unset); those of the start-plane transform and the generator solve; for a
    /// non-planar generator, those of the B-spline inversion and the conversions.
    pub fn step_inversion(
        &self,
        k: &Kernel,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<ConeStepInversion> {
        self.step_inversion_on_domain(k, &self.rev.anal_uv_domain, point_on_surf, tol_3d, uv_guess)
    }

    pub(crate) fn step_inversion_on_domain(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<ConeStepInversion> {
        // mirror points on the negative side of the apex to the positive side
        let pos = self.positive_point(k, point_on_surf)?;

        // do the conversion
        let mut inv = self.rev_step_inversion(k, anal_domain, pos.point, tol_3d, uv_guess)?;

        // when point is on far side of apex - flip the Z coordinate about the apex point
        if pos.negated {
            inv.uv.y = 2.0 * pos.apex_param - inv.uv.y;
        }
        Ok(inv)
    }

    /// `SmSurfOfRevolution::STEPInversion` on the cone's base parts
    /// ([`SurfOfRevolution::step_inversion_with_outputs`], unit
    /// `surface.surf_of_revolution_p2`), over the cone's STEP domain. A non-planar
    /// generator (a cone record read with that flag) converts through the cone's own
    /// `ConvertUVFromSTEPToNURBS` / `ConvertUVFromNURBSToSTEP`, which upstream calls
    /// virtually.
    fn rev_step_inversion(
        &self,
        k: &Kernel,
        anal_domain: &Extent2d,
        point_on_surf: Point3,
        tol_3d: f64,
        uv_guess: Option<Point2>,
    ) -> Result<ConeStepInversion> {
        let rev = &self.rev;
        let r = if rev.planar_generator {
            rev.step_inversion_with_outputs(k, anal_domain, point_on_surf, tol_3d, uv_guess)?
        } else {
            rev.step_inversion_non_planar(
                k,
                anal_domain,
                point_on_surf,
                tol_3d,
                uv_guess,
                None,
                |uv| self.convert_uv_from_step_to_nurbs(k, uv),
                |uv| self.convert_uv_from_nurbs_to_step(k, uv),
            )?
        };
        Ok(ConeStepInversion {
            uv: r.uv,
            location: r.location,
            inside: r.inside,
        })
    }

    // ---- transform ------------------------------------------------------------------

    /// Moves the cone by `rotate_n_move` after an optional uniform scale about the
    /// origin (`Transform`): nothing for the identity; the base surface of revolution
    /// is transformed, the base radius and height domain scaled, and a generator line
    /// whose STEP span no longer matches the height span gets the semi-angle's speed
    /// and the height domain.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a non-uniform or non-positive scale; [`SolidError::Unported`]
    /// from `SmSurfOfRevolution::Transform` (whose other statuses upstream ignores).
    pub fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        opt_scale: Option<Vec3>,
    ) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO)
            && opt_scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        // Check input and get scaling value - allow only uniform scaling.
        let mut scale = 1.0;
        if let Some(s) = opt_scale {
            if !are_same(s.x, s.y) || !are_same(s.x, s.z) || s.x < EFF_ZERO {
                // Unable to scale analytical surfaces non-uniformly
                return Err(SolidError::sm_err(OP_TRANSFORM));
            }
            scale = s.x;
        }

        // finish the call in the parent function (status ignored upstream), whose
        // polar-converter rebuild reaches the cone's iso curves (the virtual
        // CreateIsoParametricCurve)
        let inside_out = self.inside_out;
        ignore_status(
            self.rev
                .transform_using(k, rotate_n_move, opt_scale, &|s, k, p, t| {
                    Ok(cone_iso_curve(s, inside_out, k, p, t, None, false)?.curve)
                }),
        )?;

        // update cone size
        self.base_radius *= scale;

        // Our analytical v-domain is a linear distance, so we have to scale it.
        if (scale - 1.0).abs() > EFF_ZERO {
            // Scale it about v==0: that's where the base is.
            let mut domain_v = self.rev.anal_uv_domain.v_interval();
            domain_v.scale(scale);
            self.rev.anal_uv_domain.set_v_interval(&domain_v);
        }

        // change the GenCurve so that it shares the same analytic domain as the
        // height domain and has a LineScale determined by the semi-angle
        let v_ivl = self.rev.anal_uv_domain.v_interval();
        let semi_angle_deg = self.semi_angle_deg;
        if let Some(CurveKind::Line(gen_line)) = self.rev.gen_curve.as_mut().map(Curve::kind_mut) {
            let mut scaled_zero = EFF_ZERO * (1.0 + v_ivl.length());
            if !are_same_to_tol(
                v_ivl.length(),
                gen_line.step_interval().length(),
                scaled_zero,
            ) {
                let angle_rad = deg2rad(semi_angle_deg);
                let speed = 1.0 / angle_rad.cos();
                let line_scale = gen_line.line_scale();
                scaled_zero = EFF_ZERO * (1.0 + line_scale);
                if !are_same_to_tol(line_scale, speed, scaled_zero) {
                    // Don't call SetCanonical: that recreates m_pNurb.
                    gen_line.set_line_scale(speed);
                    gen_line.set_analytic_domain(v_ivl);
                }
            }
        }
        Ok(())
    }

    // ---- database -------------------------------------------------------------------

    /// Reads a cone (or, with `cylinder`, a cylinder, which has no data of its own)
    /// (`ReadFromDB`): the base radius, semi-angle and inside-out flag, then the
    /// surface of revolution.
    ///
    /// # Errors
    ///
    /// Those of the binary reads and of the surface of revolution record
    /// ([`SurfOfRevolution::read_from_db`]).
    pub fn read_from_db(
        k: &Kernel,
        db: &mut DatabaseReader,
        version: u64,
        cylinder: bool,
    ) -> Result<Self> {
        // init output object
        let mut cone = Self::empty(cylinder);

        // file type
        if db.file_type() == FileType::Ascii {
            if let Some(v) = db
                .in_stream_mut()
                .and_then(crate::topology::database_io::InputStream::extract_double)
            {
                cone.base_radius = v;
            }
            db.go_to_next_line();
            if let Some(v) = db
                .in_stream_mut()
                .and_then(crate::topology::database_io::InputStream::extract_double)
            {
                cone.semi_angle_deg = v;
            }
            db.go_to_next_line();
            if let Some(v) = db
                .in_stream_mut()
                .and_then(crate::topology::database_io::InputStream::extract_int)
            {
                cone.inside_out = v != 0;
            }
            db.go_to_next_line();
        } else {
            cone.base_radius = db.read_double()?;
            cone.semi_angle_deg = db.read_double()?;
            cone.inside_out = db.read_boolean()? != 0;
        }

        // read the parent object
        cone.rev = SurfOfRevolution::read_from_db(k, db, version)?;

        // all done
        Ok(cone)
    }
}

// ---- sm_TotalCoincidenceChecker ------------------------------------------------------

/// Whether two lines or two circles coincide over the given domains
/// (`sm_TotalCoincidenceChecker`; `tol` is converted as upstream's `SmApproxTol3d`
/// parameter: its magnitude, `SM_USE_DEFAULT` half the zone tolerance): equal curves
/// are coincident and not reversed;
/// curves of different types are not coincident. Lines coincide when their ends match
/// within `tol` in the same or the opposite order. Circles must match at both ends
/// (closed curves: the first derivatives decide the sense) and at the middle (dropped
/// onto the other circle when reversed). `reversed` stays `UNSURE` (`None`) unless
/// coincident.
///
/// # Errors
///
/// Those of the evaluations and of the curve comparison.
pub fn total_coincidence_checker(
    k: &Kernel,
    this: &Curve,
    this_domain: &Extent1d,
    other: &Curve,
    other_domain: &Extent1d,
    tol: f64,
) -> Result<Coincidence> {
    // the `const SmApproxTol3d` parameter (callers pass a double: `fabs`, and
    // `SM_USE_DEFAULT` becomes half the zone tolerance)
    let tol = approx_tol3d(k, tol);

    // Init return flags for no coincidence.
    let none = Coincidence {
        coincident: false,
        reversed: None,
    };

    // First try equality operator.
    if this.approx_eq(k, other)? {
        return Ok(Coincidence {
            coincident: true,
            reversed: Some(false),
        });
    }

    if this.type_code() != other.type_code() {
        return Ok(none);
    }

    // Using end points, set bRev flag, and quit if not coincident (statuses ignored).
    let pt00 = this.evaluate_point(k, this_domain.min())?;
    let mut pt01 = this.evaluate_point(k, this_domain.max())?;
    let pt10 = other.evaluate_point(k, other_domain.min())?;
    let mut pt11 = other.evaluate_point(k, other_domain.max())?;

    let dist_s_s = pt00.distance(pt10);
    let dist_s_e = pt00.distance(pt11);

    if this.is_kind_of(LINE_TYPE) {
        let dist_e_e = pt01.distance(pt11);
        if dist_s_s < tol && dist_e_e < tol {
            return Ok(Coincidence {
                coincident: true,
                reversed: Some(false),
            });
        }
        let dist_e_s = pt01.distance(pt10);
        if dist_s_e < tol && dist_e_s < tol {
            return Ok(Coincidence {
                coincident: true,
                reversed: Some(true),
            });
        }
        return Ok(none);
    }

    // Not a line.  If not a circle, return no coincidence.
    if !this.is_kind_of(CIRCLE_TYPE) {
        return Ok(none);
    }

    let rev = if dist_s_s < tol && dist_s_e < tol {
        // Start pt 1 coinc with both start pt2 and end pt2: crv 2 is closed.
        let dist_e_s = pt01.distance(pt10);
        if dist_e_s > tol {
            return Ok(none);
        }

        // Both curves closed, same endpoints.  Check 1st derivs for orientation.
        let pv1 = this.evaluate(k, this_domain.min(), 1, true, true)?;
        let pv2 = other.evaluate(k, other_domain.min(), 1, true, true)?;
        let d1 = pv1.get(1).copied().unwrap_or(Vec3::ZERO);
        let d2 = pv2.get(1).copied().unwrap_or(Vec3::ZERO);
        let dot = d1.dot(d2);
        dot < -EFF_ZERO
    } else if dist_s_s < tol {
        // Start points coincident, start/end not.  Check end points.
        let dist_e_e = pt01.distance(pt11);
        if dist_e_e > tol {
            return Ok(none);
        }
        false
    } else if dist_s_e < tol {
        // Start pt 1 == end pt 2, start pt 2 distinct.
        let dist_e_s = pt01.distance(pt10);
        if dist_e_s > tol {
            return Ok(none);
        }
        true
    } else {
        // No endpoint coincidence.
        return Ok(none);
    };

    // Check interior.  For circles, one interior point is sufficient.
    pt01 = this.evaluate_point(k, this_domain.evaluate(0.5))?;

    // If reversed, parameterization could vary.
    let t2 = other_domain.evaluate(0.5);
    let dist = if rev {
        // (DropPoint's status ignored: a failure leaves bOk FALSE)
        match ignored(other.drop_point(
            k,
            other_domain,
            pt01,
            None,
            tol,
            Some(t2),
            SolverOperation::Intersect,
        ))? {
            Some(d) if d.success => d.distance,
            _ => BIG_DOUBLE,
        }
    } else {
        pt11 = other.evaluate_point(k, t2)?;
        pt01.distance(pt11)
    };

    if dist > tol {
        return Ok(none);
    }

    // All good: coincident curves.
    Ok(Coincidence {
        coincident: true,
        reversed: Some(rev),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::curve::p2::ELLIPSE_TYPE;
    use crate::error::SM_ERR;

    fn k() -> Kernel {
        Kernel::default()
    }

    fn std_placement() -> Axis2Placement {
        Axis2Placement::new(Vec3::ZERO, Vec3::X, Vec3::Y)
    }

    /// `SmCone::CreateCanonical(1, 2, 3)` (`TestSmGeneral.cpp`).
    fn cone123() -> Cone {
        Cone::create_canonical(&k(), &std_placement(), 1.0, 2.0, 3.0, false, false).unwrap()
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-12
    }

    #[test]
    fn canonical_cone_shape() {
        let c = cone123();
        assert!(!c.is_cylinder());
        assert!((c.semi_angle_deg() - 1.0f64.atan2(3.0).to_degrees()).abs() < 1e-12);
        assert!((c.bot_radius() - 1.0).abs() < 1e-12);
        assert!((c.top_radius() - 2.0).abs() < 1e-12);
        assert_eq!(c.type_code(), CONE_TYPE);
        assert_eq!(c.type_string(), "SmCone_TYPE");
        assert_eq!(c.class_string(), "SmCone");
        assert!(c.is_kind_of(CONE_TYPE).unwrap());
        assert!(!c.is_kind_of(CYLINDER_TYPE).unwrap());
        assert!(c.rev().have_polar_conversion());
        let p = c.evaluate_step_point(Point2::new(0.0, 1.5)).unwrap();
        assert!(close(p, Vec3::new(1.5, 0.0, 1.5)));
        let p = c.evaluate_step_point(Point2::new(90.0, 3.0)).unwrap();
        assert!(close(p, Vec3::new(0.0, 2.0, 3.0)));
        assert!((c.height(&k()).unwrap() - 3.0).abs() < 1e-12);
        let apex = c.apex(&k()).unwrap();
        assert!(!apex.is_cylinder);
        assert!((apex.param + 3.0).abs() < 1e-12);
    }

    #[test]
    fn step_nurbs_round_trip_and_points() {
        for (swap, inside) in [(false, false), (true, false), (false, true), (true, true)] {
            let c = Cone::create_canonical(&k(), &std_placement(), 1.0, 2.0, 3.0, swap, inside)
                .unwrap();
            for step in [Point2::new(37.0, 0.5), Point2::new(250.0, 2.5)] {
                let nurb = c.convert_uv_from_step_to_nurbs(&k(), step).unwrap();
                let back = c.convert_uv_from_nurbs_to_step(&k(), nurb).unwrap();
                assert!((back - step).length() < 1e-9, "{swap} {inside}");
                let p = c.evaluate_point(&k(), nurb).unwrap();
                let q = c.evaluate_step_point(step).unwrap();
                assert!(p.distance(q) < 1e-9);
            }
        }
    }

    #[test]
    fn iso_curves_are_circles_and_lines() {
        let c = cone123();
        let dom = c.rev().base().natural_uv_domain();
        let circle = c
            .create_iso_parametric_curve(
                &k(),
                SurfParamType::V,
                dom.v_interval().mid(),
                None,
                false,
            )
            .unwrap();
        assert!(circle.curve.is_kind_of(CIRCLE_TYPE));
        let line = c
            .create_iso_parametric_curve(&k(), SurfParamType::U, dom.u_interval().mid(), None, true)
            .unwrap();
        assert!(line.curve.is_kind_of(LINE_TYPE));
        assert!(line.uv_iso_curve.is_some());
        assert!(!line.curve.is_kind_of(ELLIPSE_TYPE));
    }

    /// A reference point beyond a cone's end but within the tolerance (#1067, UB-87): the
    /// inversion of the point finds no solution, and upstream builds the circle at a height it
    /// never set. The circle is at the end instead. Inside the end, or beyond the tolerance, it
    /// is as upstream.
    #[test]
    fn iso_circle_beyond_the_end_within_tolerance_is_at_the_end() {
        for (swap_uv, inside_out) in [(false, false), (true, false), (false, true), (true, true)] {
            let c = Cone::create_canonical_with_semi_angle(
                &k(),
                &std_placement(),
                0.5,
                70.0,
                Some(1.0),
                swap_uv,
                inside_out,
            )
            .unwrap();
            let tol = 1e-3;
            let height_of = |z: f64| {
                let circle = c
                    .create_iso_circle_from_point(&k(), Vec3::new(c.radius(z), 0.0, z), tol)
                    .unwrap_or_else(|e| panic!("z = {z}: {e:?}"))?;
                let start = circle
                    .evaluate_point(&k(), circle.natural_interval().min())
                    .unwrap();
                // the circle lies on the cone: its radius is the cone's at its height
                let radius = (start.x * start.x + start.y * start.y).sqrt();
                assert!(
                    (radius - c.radius(start.z)).abs() < 1e-9,
                    "z = {z}: circle at {start:?}"
                );
                Some(start.z)
            };
            for (z, want) in [
                (0.9995, 0.9995),
                (1.0, 1.0),
                (1.0005, 1.0),
                (1.000684, 1.0),
                (1.0009, 1.0),
            ] {
                let got = height_of(z).unwrap_or_else(|| panic!("z = {z}: no circle"));
                assert!((got - want).abs() < 1e-9, "z = {z}: circle at height {got}");
            }
            assert!(height_of(1.002).is_none());
        }
    }

    #[test]
    fn iso_circle_from_point() {
        let c = cone123();
        let circle = c
            .create_iso_circle_from_point(&k(), Vec3::new(5.0, 0.0, 1.5), 1e-9)
            .unwrap()
            .unwrap();
        assert!(circle.is_kind_of(CIRCLE_TYPE));
        // above the cone: no circle
        assert!(c
            .create_iso_circle_from_point(&k(), Vec3::new(5.0, 0.0, 4.0), 1e-9)
            .unwrap()
            .is_none());
    }

    #[test]
    fn step_inversion_and_positive_point() {
        let c = cone123();
        let p = c.evaluate_step_point(Point2::new(120.0, 1.25)).unwrap();
        let inv = c.step_inversion(&k(), p, 1e-9, None).unwrap();
        assert!(inv.inside);
        assert_eq!(inv.location, LocationType::Interior);
        assert!((inv.uv - Point2::new(120.0, 1.25)).length() < 1e-9);

        // the far side of the apex of an infinite 45 degree cone
        let c45 = Cone::create_canonical_with_semi_angle(
            &k(),
            &std_placement(),
            0.0,
            45.0,
            None,
            false,
            false,
        )
        .unwrap();
        let pp = c45.positive_point(&k(), Vec3::new(1.0, 0.0, -1.0)).unwrap();
        assert!(pp.negated);
        assert!(close(pp.point, Vec3::new(-1.0, 0.0, 1.0)));
        let inv = c45
            .step_inversion(&k(), Vec3::new(1.0, 0.0, -1.0), 1e-9, None)
            .unwrap();
        assert!((inv.uv.y + 1.0).abs() < 1e-9);
    }

    #[test]
    fn cylinders() {
        assert!(matches!(
            Cone::create_canonical_cylinder(&k(), &std_placement(), 0.0),
            Err(SolidError::Upstream { code: SM_ERR, .. })
        ));
        let c = Cone::create_canonical_cylinder(&k(), &std_placement(), 2.0).unwrap();
        assert!(c.is_cylinder() && c.is_cylinder_class());
        assert_eq!(c.type_code(), CYLINDER_TYPE);
        assert_eq!(c.type_string(), "SmCylinder_TYPE");
        assert_eq!(c.class_string(), "SmCylinder");
        assert!(c.is_kind_of(CONE_TYPE).unwrap());
        assert!(c.apex(&k()).unwrap().is_cylinder);
        let d = c.max_analytic_domain(&k()).unwrap();
        // the generator line's maximum domain, not limited at an apex
        assert_eq!(d.min().y, -d.max().y);
        assert!(d.max().y > 0.0);
        // a cone of the same shape is not equal to the cylinder (types differ)
        let cone = Cone::new_bounded(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            2.0,
            2.0,
            0.0,
            360.0,
            3.0,
            false,
            false,
            false,
        );
        let cyl = Cone::new_cylinder_bounded(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            2.0,
            0.0,
            360.0,
            3.0,
            false,
            false,
            false,
        );
        assert!(!cone.approx_eq(&cyl));
        assert!(cone.approx_eq(&cone.clone()));
    }

    #[test]
    fn negative_radius_cylinder_has_no_polar_curve() {
        // MakeNurb ignores the polar converter rebuild's failure (a negative radius)
        let mut c = Cone::new_cylinder_bounded(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            -1.8,
            0.0,
            360.0,
            4.3,
            false,
            false,
            false,
        );
        c.make_nurb(&k()).unwrap();
        assert!(c.rev().polar_curve().is_none());
        assert_eq!(c.height(&k()).unwrap(), 0.0);
        assert!(c
            .convert_uv_from_step_to_nurbs(&k(), Point2::new(10.0, 1.0))
            .is_err());
        assert!(matches!(
            c.convert_uv_from_nurbs_to_step(&k(), Point2::new(0.5, 0.5)),
            Err(SolidError::Upstream {
                code: crate::error::SM_ERR_NULL_POINTER,
                ..
            })
        ));
    }

    #[test]
    fn unbounded_cone_domain_starts_at_apex() {
        let c = Cone::create_canonical_with_semi_angle(
            &k(),
            &std_placement(),
            1.0,
            45.0,
            None,
            false,
            false,
        )
        .unwrap();
        let d = c.rev().step_uv_domain();
        assert!((d.min().y + 1.0).abs() < 1e-12);
        assert_eq!(d.max().y, INFINITE_PARAMETER);
        assert!(c.rev().base().gw_nurb().is_some());
    }

    #[test]
    fn statuses_and_unported() {
        let mut c = cone123();
        assert!(matches!(
            c.adjust_step_uv_domain(&k(), &Extent2d::from_bounds(0.0, -4.0, 360.0, 3.0)),
            Err(SolidError::Upstream {
                code: SM_ERR_INVALID_INPUT,
                ..
            })
        ));
        // the parent's AdjustSTEPUVDomain is ported (surf_of_revolution_p2)
        assert!(c
            .adjust_step_uv_domain(&k(), &Extent2d::from_bounds(0.0, 0.0, 360.0, 1.0))
            .is_ok());
        assert!(c.transform(&k(), &Axis2Placement::IDENTITY, None).is_ok());
        assert!(matches!(
            c.transform(
                &k(),
                &Axis2Placement::IDENTITY,
                Some(Vec3::new(1.0, 2.0, 1.0))
            ),
            Err(SolidError::Upstream { code: SM_ERR, .. })
        ));
        assert!(c
            .transform(
                &k(),
                &Axis2Placement::IDENTITY,
                Some(Vec3::new(2.0, 2.0, 2.0))
            )
            .is_ok());
        // huge orders fail cleanly
        assert!(c
            .evaluate_step(Point2::new(0.0, 1.0), usize::MAX, 1)
            .is_err());
        assert!(c
            .evaluate(
                &k(),
                Point2::new(0.5, 0.5),
                usize::MAX,
                usize::MAX,
                EvaluateFlags::default()
            )
            .is_err());
    }

    #[test]
    fn read_from_db() {
        let mut db = DatabaseReader::new();
        db.open_for_read(vec![0u8; 5], FileType::Binary);
        let e = Cone::read_from_db(&k(), &mut db, 30, false).unwrap_err();
        assert!(!e.is_unported());
        let mut db = DatabaseReader::new();
        db.open_for_read(b"1.5\n30\n1\n".to_vec(), FileType::Ascii);
        // the surface of revolution record is missing
        assert!(!Cone::read_from_db(&k(), &mut db, 30, true)
            .unwrap_err()
            .is_unported());
    }

    #[test]
    fn coincidence() {
        let k = k();
        let line = |a: Vec3, b: Vec3| {
            let mut l = Line::from_points(a, b, 3);
            l.make_nurb().unwrap();
            Curve::from(l)
        };
        let a = line(Vec3::ZERO, Vec3::new(1.0, 2.0, 3.0));
        let unit = Extent1d::from_min_max(0.0, 1.0);
        let same = total_coincidence_checker(&k, &a, &unit, &a.clone(), &unit, 1e-6).unwrap();
        assert_eq!(
            same,
            Coincidence {
                coincident: true,
                reversed: Some(false)
            }
        );
        let rev = line(Vec3::new(1.0, 2.0, 3.0), Vec3::ZERO);
        assert_eq!(
            total_coincidence_checker(&k, &a, &unit, &rev, &unit, 1e-6).unwrap(),
            Coincidence {
                coincident: true,
                reversed: Some(true)
            }
        );
        let apart = line(Vec3::new(5.0, 0.0, 0.0), Vec3::new(6.0, 0.0, 0.0));
        assert_eq!(
            total_coincidence_checker(&k, &a, &unit, &apart, &unit, 1e-6).unwrap(),
            Coincidence {
                coincident: false,
                reversed: None
            }
        );
        let circle = |y: Vec3| {
            Curve::from(
                Ellipse::new_circle(
                    &k,
                    Vec3::ZERO,
                    Vec3::X,
                    y,
                    Extent1d::from_min_max(0.0, 360.0),
                    1.0,
                    &EllipseOptions::default(),
                )
                .unwrap(),
            )
        };
        let full = Extent1d::from_min_max(0.0, 360.0);
        let r =
            total_coincidence_checker(&k, &circle(Vec3::Y), &full, &circle(-Vec3::Y), &full, 1e-6)
                .unwrap();
        assert!(r.coincident);
        assert_eq!(r.reversed, Some(true));
    }

    /// A cone record read with its generator flagged non-planar takes the inherited
    /// general-solver branch of `STEPInversion`, converting through the cone's own
    /// parameter conversions: it finds the parameters the analytic branch finds.
    #[test]
    fn non_planar_flag_step_inversion_converts_through_the_cone() {
        let k = k();
        for swap in [false, true] {
            for inside in [false, true] {
                let planar =
                    Cone::create_canonical(&k, &std_placement(), 1.0, 2.0, 3.0, swap, inside)
                        .unwrap();
                let mut non_planar = planar.clone();
                non_planar.rev.planar_generator = false;
                let dom = planar.rev.anal_uv_domain;
                let (u, v) = (dom.u_interval(), dom.v_interval());
                for (fu, fv) in [(0.3, 0.4), (0.7, 0.15), (0.55, 0.8)] {
                    let step = Point2::new(
                        u.min() + fu * (u.max() - u.min()),
                        v.min() + fv * (v.max() - v.min()),
                    );
                    let p = planar.evaluate_step_point(step).unwrap();
                    let a = planar.step_inversion(&k, p, 1e-7, None).unwrap();
                    let b = non_planar.step_inversion(&k, p, 1e-7, None).unwrap();
                    assert!((a.uv - step).length() < 1e-6, "{:?} {step:?}", a.uv);
                    assert!((a.uv - b.uv).length() < 1e-6, "{:?} {:?}", a.uv, b.uv);
                    assert_eq!(a.location, b.location);
                    assert!(b.inside);
                }
            }
        }
    }
}
