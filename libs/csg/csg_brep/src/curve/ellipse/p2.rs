//! `SmEllipse`, part 2: the ellipse object ([`Ellipse`]) and its polar conversion
//! table ([`PolarConversion`]) with the table set-up and the angle to NURB parameter
//! conversion, STEP evaluation and inversion, the fast point drop, the circle/circle
//! and circle/line intersectors, transforms and the small queries.
//!
//! Upstream: `source/SMLib/src/SmEllipse.cpp`, `source/SMLib/inc/SmEllipse.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`PolarConversion`], [`PolarConversion::new`]     | `SmPolarConversion`, `SmPolarConversion()`       |
//! | `Drop` (implicit)                                 | `~SmPolarConversion`                             |
//! | [`PolarConversion::approx_eq`]                    | `SmPolarConversion::operator==`                  |
//! | [`PolarConversion::set_context`]                  | `SmPolarConversion::SetContext`                  |
//! | [`PolarConversion::is_polar_conversion_possible`], [`PolarConversion::set_polar_conversion_possible`] | `IsPolarConversionPossible`, `SetPolarConversionPossible` |
//! | [`PolarConversion::TYPE`], [`PolarConversion::class_string`], [`PolarConversion::class_type`] | `SmPolarConversion_TYPE`, `GetClassString`, `GetClassType` |
//! | [`Ellipse::set_up_polar_conversion`]              | `SmPolarConversion::SetUpPolarConversion(this, TRUE, ..)` |
//! | [`PolarConversion::convert_to_nurbs_parameter`]   | `SmPolarConversion::ConvertToNURBSParameter`     |
//! | [`PolarConversion::angle_param_to_nurb_param`]    | `SmPolarConversion::AngleParamToNurbParam` (private) |
//! | [`Ellipse`], [`Ellipse::empty`]                   | `SmEllipse`, `SmEllipse(lDimension, cpContext)`  |
//! | `Drop` (implicit)                                 | `~SmEllipse`                                     |
//! | [`Ellipse::new`], [`EllipseOptions`]              | `SmEllipse(Center, XAxis, YAxis, AnalDomain, ..)` (co-ported from `curve.ellipse_p1`) |
//! | [`Ellipse::approx_eq`]                            | `operator==`                                     |
//! | [`Ellipse::convert_to_3d`]                        | `ConvertTo3D`                                    |
//! | [`Ellipse::convert_t_from_step_to_nurbs`]         | `ConvertTFromSTEPToNURBS`                        |
//! | [`Ellipse::step_inversion`]                       | `STEPInversion`                                  |
//! | [`Ellipse::canonical`], [`EllipseCanonical`]      | `GetCanonical`                                   |
//! | [`Ellipse::max_analytic_domain`]                  | `GetMaxAnalyticDomain`                           |
//! | [`Ellipse::transform_point_to_start_plane`], [`EllipseStartPlanePoint`] | `TransformPointToStartPlane` |
//! | [`Ellipse::drop_point_fast`]                      | `DropPointFast`                                  |
//! | [`Ellipse::evaluate_step_point`], [`Ellipse::evaluate_step`] | `EvaluateSTEPPoint`, `EvaluateSTEP`   |
//! | [`Ellipse::intersect_with_ellipse`], [`EllipseIntersection`] | `IntersectWithEllipse`                |
//! | [`Ellipse::intersect_with_line`]                  | `IntersectWithLine`                              |
//! | [`Ellipse::is_circle`], [`Ellipse::is_degenerate`] | `IsCircle`, `IsDegenerate`                      |
//! | [`Ellipse::transform`]                            | `Transform`                                      |
//! | [`Ellipse::is_kind_of`]                           | `IsKindOf`                                       |
//! | [`Ellipse::TYPE`], [`Ellipse::type_code`], [`Ellipse::class_type`], [`Ellipse::type_string`], [`Ellipse::class_string`] | `GetType`, `GetClassType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`Ellipse::step_interval`], [`Ellipse::set_step_interval`], [`Ellipse::degree`] | `GetSTEPInterval`, `SetSTEPInterval`, `GetDegree` (inline) |
//! | [`Ellipse::start_angle_deg`], [`Ellipse::end_angle_deg`], [`Ellipse::position`] | `GetStartAngleDeg`, `GetEndAngleDeg`, `GetPosition` (inline) |
//! | [`Ellipse::x_radius`], [`Ellipse::y_radius`], [`Ellipse::inside_out`] | `GetXRadius`, `GetYRadius`, `GetInsideOut` (inline) |
//! | [`Ellipse::set_out_of_bounds_enabled`]             | inherited `SmBSplineCurve::SetOutOfBoundsEnabled` |
//! | [`Ellipse::is_analytic`], [`Ellipse::is_domain_boundary`] | `IsAnalytic`, `IsDomainBoundary` (inline)  |
//! | [`MAX_PERIOD_SHIFTS`]                             | no counterpart (loop bound)                      |
//!
//! Deviations:
//! - **Derived class.** `SmEllipse` derives from `SmBSplineCurve`; the base object is
//!   the [`Ellipse::base`] field; the ellipse (and `SmCircle`, unit `curve.ellipse_p1`)
//!   joins [`crate::curve::curve::Curve`] as [`curve_base::CurveKind::Ellipse`]. The
//!   methods here act on an ellipse without owner: `GetEdge` is `NULL`, so
//!   `STEPInversion` uses the context zone tolerance (here the kernel's,
//!   [`context_zone_tol_3d`] of no context: the default `SmContext` has the system
//!   tolerance).
//! - **Polar converter.** `m_cpCurve` always points at the owning ellipse
//!   ("borrowed"), so it is not stored: [`PolarConversion`] keeps only whether it has
//!   been attached to a curve, and its methods take the curve as an argument. The
//!   non-borrowed converters of the `SmPolarConversion` copy constructor, `CopyFrom`
//!   and `ReadFromDB` (unit `curve.ellipse_p1`) are not modelled, so
//!   `operator==` never compares curves (its `bHasCurve` is always false).
//!   `SetUpPolarConversion` changes the ellipse's STEP interval, so it is
//!   [`Ellipse::set_up_polar_conversion`]; its `crIntervalDeg` and `crPlacement`
//!   arguments are `NotUsed` and dropped. The `SmCircle::IsArc` branch for a curve
//!   that is not an ellipse is unreachable (the curve is always an [`Ellipse`]).
//!   An ellipse without NURB (only [`Ellipse::empty`]) is not set up: upstream makes
//!   the NURB lazily inside `GetKnots`, re-entering the set-up; here that is
//!   `SM_ERR`.
//!   Its two `while` loops that shift the NURB's angles by whole turns towards the
//!   STEP interval are bounded: where upstream would loop forever (a shift that no
//!   longer changes the midpoint, from STEP angles beyond about 2^61 (~2.3e18) degrees) or for more
//!   than [`MAX_PERIOD_SHIFTS`] shifts, the set-up returns
//!   [`SolidError::NotConverged`] with the converter left impossible and the saved
//!   STEP interval restored, so the temporary full-circle domain does not leak.
//! - **Notifications.** `SmBSplineCurve::SetFromGwNurb` and `Transform` send
//!   `SM_NO_POST_EDIT`, whose `SmEllipse::Notify` override rebuilds the polar
//!   converter (status ignored); the port calls [`Ellipse::set_up_polar_conversion`]
//!   at those points. The other notifications only drop caches and are omitted.
//! - **Co-port.** The geometric constructor belongs to unit `curve.ellipse_p1`; it is
//!   ported here because every method of this unit needs an ellipse with its NURB
//!   (made by `MakeNurb`, `p1`).
//! - **Arguments.** Out-params become return values; `SmContext` arguments and the
//!   arguments upstream marks `NotUsed` (`EvaluateSTEP`'s `bFromLeft` and zone
//!   tolerance, `IsDegenerate`'s interval, `ConvertToNURBSParameter`'s zone tolerance
//!   is passed through but unused) are dropped. Solutions name `this` (and the other
//!   curve) through optional caller-supplied object ids. Methods that read the
//!   context tolerance take the [`Kernel`].
//! - **Statuses.** Calls whose status upstream ignores keep upstream's values, except
//!   that [`SolidError::Unported`] is always propagated. Upstream outputs written
//!   before a failing status (partial solution arrays) are not returned.
//! - **Back-edges.** `SmCurve::LocalPropertyAnalysis` (`curve.curve_p5`) runs on the
//!   ellipse's B-spline base (the ellipse evaluates through it). `SmCurve::GlobalPointSolve` and
//!   `SmCurve::GlobalCoincidenceChecker` of the ellipse are
//!   [`curve_base::Curve::global_point_solve`] and
//!   [`curve_base::Curve::global_coincidence_checker`] on copies of the ellipses as
//!   curve kinds (taking the [`Kernel`]). The
//!   line's `GlobalPointSolve(SM_SO_INTERSECT)` of `IntersectWithLine` is the line's
//!   `DropPointFast`, which handles every intersection. `SmCurve::LocalCurveSolve` (unit
//!   `curve.curve_p3`) evaluates the ellipse through its B-spline base and the line
//!   through [`curve_base::Curve`] (its Newton equations, `SmFindCCNormalENFO`, are
//!   unit `curve.curve_p5`). `ConvertToNURBSParameter` always reports an exact
//!   conversion when it succeeds, so `DropPointFast`'s refinement call is never
//!   reached.

use crate::curve::curve::p1::GlobalPointSolveOptions;
use crate::curve::curve::p2::{self as curve_base, CIRCLE_TYPE, ELLIPSE_TYPE};
use crate::curve::curve::p3::local_curve_solve;
use crate::curve::line::{CurveLocationType, Line};
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::assert_array::labels::POLAR_CONVERSION_TYPE;
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p1::{
    circle_coplanar_circle_intersect, line_coplanar_circle_intersect, line_plane_intersect,
    point_project_to_plane, transform_point_to_start_plane, vector_project_to_plane, CircleCircle,
};
use crate::geom::geom_utility::p2::{intersect_two_planes, PlanesIntersection};
use crate::geom::global_solver::{SolutionRequested, SolverOperation};
use crate::geom::solution_array::{ObjectKind, Solution, SolutionObject, SolutionType};
use crate::geom::tol::{
    are_same, are_same_to_tol, context_zone_tol_3d, is_zero, is_zero_to_tol, SmallTopoType,
};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{
    max2, min2, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_DEG, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI, TWO_PI,
};
use crate::nurbs::b_spline_curve::{BSplineCurve, BSplineCurveForm};
use crate::nurbs::b_spline_surface::p1::SurfParamType;

const OP_SET_UP: &str = "curve::ellipse::p2::Ellipse::set_up_polar_conversion";

/// Largest number of whole-turn shifts [`Ellipse::set_up_polar_conversion`] applies
/// to move the NURB's angles next to the STEP interval (upstream loops without a
/// bound).
pub const MAX_PERIOD_SHIFTS: usize = 1 << 20;
const OP_TO_NURBS: &str = "curve::ellipse::p2::PolarConversion::convert_to_nurbs_parameter";
const OP_ANGLE: &str = "curve::ellipse::p2::PolarConversion::angle_param_to_nurb_param";
const OP_EVAL_STEP: &str = "curve::ellipse::p2::Ellipse::evaluate_step";
const OP_DROP: &str = "curve::ellipse::p2::Ellipse::drop_point_fast";
const OP_TRANSFORM: &str = "curve::ellipse::p2::Ellipse::transform";

/// `SM_SINE_10DEG`: the sine of 10 degrees (near-parallel checks).
const SINE_10DEG: f64 = 0.173_648_177_666_930_35;

/// An index into a table that upstream reads unchecked.
fn table(v: &[f64], i: usize, operation: &'static str) -> Result<f64> {
    v.get(i).copied().ok_or(SolidError::invalid_input(
        operation,
        "polar conversion table is shorter than its knot list",
    ))
}

// ---- back-edges -----------------------------------------------------------------------

/// `SmCurve::LocalPropertyAnalysis(Interval, SM_CP_POINT_INVERSION, dGuess, NULL,
/// &Point, bFoundAnswer, sSolution)` of `curve`: whether an answer was found and its
/// parameter.
fn local_point_inversion(
    k: &Kernel,
    curve: &Ellipse,
    interval: &Extent1d,
    guess: f64,
    point: Point3,
) -> Result<(bool, f64)> {
    // (the ellipse evaluates through its NURB)
    let mut solution: Solution<()> = Solution::new();
    let found = crate::curve::curve::p5::local_property_analysis(
        k,
        curve.base(),
        interval,
        curve_base::CurvePropertyType::PointInversion,
        guess,
        None,
        &[point],
        &mut solution,
    )?;
    Ok((found, solution.start.param(0)?))
}

/// `SmCurve::GlobalPointSolve(Interval, SM_SO_INTERSECT, Point, dTol, NULL, NULL,
/// SM_SR_ALL, rSolutions)` of the ellipse `curve` ([`curve_base::Curve::global_point_solve`]
/// on a copy of the ellipse as a curve kind); only the solution parameters are used,
/// the solutions name no object.
fn ellipse_global_point_solve<O: Copy>(
    k: &Kernel,
    curve: &Ellipse,
    interval: &Extent1d,
    point: Point3,
    tol: f64,
) -> Result<Vec<Solution<O>>> {
    let c = curve_base::Curve::from(curve.clone());
    let opts = GlobalPointSolveOptions {
        handle: Some(ParamHandle(curve.base.natural_interval())),
        ..GlobalPointSolveOptions::default()
    };
    let sols = c.global_point_solve(
        k,
        interval,
        SolverOperation::Intersect,
        point,
        tol,
        SolutionRequested::All,
        &opts,
    )?;
    Ok(sols
        .as_slice()
        .iter()
        .map(|s| {
            let mut r: Solution<O> = Solution::new();
            r.solution_type = s.solution_type;
            r.num_variables = s.num_variables;
            r.start = s.start;
            r.end = s.end;
            r.num_objects = s.num_objects;
            r.nodes = s.nodes;
            r
        })
        .collect())
}

/// The handle of an ellipse in its own global solve: a curve with the given natural
/// interval (the solver's duplicate test reads it).
#[derive(Debug, Clone, Copy, PartialEq)]
struct ParamHandle(Extent1d);

impl SolutionObject for ParamHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.0)
    }
}

/// `SmCurve::GlobalPointSolve(Interval, SM_SO_INTERSECT, Point, dTol, NULL, NULL,
/// SM_SR_ALL, rSolutions)` of the line `line`: for `SM_SO_INTERSECT` the solve
/// returns the line's `DropPointFast` (which never fails for that operation; its
/// solutions name no object here).
fn line_global_point_solve<O: Copy>(
    line: &Line,
    interval: &Extent1d,
    point: Point3,
    tol: f64,
) -> Result<Vec<Solution<O>>> {
    line.drop_point_fast(interval, SolverOperation::Intersect, point, tol, None, None)
}

/// `SmCurve::LocalCurveSolve(Interval, Other, OtherInterval, SM_SO_MINIMIZE, dTol,
/// &dDist, NULL, dGuess0, dGuess1, bFoundAnswer, sSol)` of `curve` with the ellipse
/// `other`: whether an answer was found and the solution.
fn local_curve_solve_ellipse<O: Copy>(
    k: &Kernel,
    curve: &Ellipse,
    interval: &Extent1d,
    other: &Ellipse,
    other_interval: &Extent1d,
    tol: f64,
    target_distance: f64,
    guesses: [f64; 2],
) -> Result<(bool, Solution<O>)> {
    // (the ellipses evaluate through their B-spline base)
    local_curve_solve(
        k,
        &curve.base,
        interval,
        &other.base,
        other_interval,
        SolverOperation::Minimize,
        tol,
        Some(target_distance),
        None,
        guesses,
        [None, None],
    )
}

/// [`local_curve_solve_ellipse`] with the line `other`.
fn local_curve_solve_line<O: Copy>(
    k: &Kernel,
    curve: &Ellipse,
    interval: &Extent1d,
    other: &Line,
    other_interval: &Extent1d,
    tol: f64,
    target_distance: f64,
    guesses: [f64; 2],
) -> Result<(bool, Solution<O>)> {
    // (the line evaluates through the curve dispatch, as its virtual Evaluate)
    let other = curve_base::Curve::from(other.clone());
    local_curve_solve(
        k,
        &curve.base,
        interval,
        &other,
        other_interval,
        SolverOperation::Minimize,
        tol,
        Some(target_distance),
        None,
        guesses,
        [None, None],
    )
}

/// `SmCurve::GlobalCoincidenceChecker(Interval, Other, OtherInterval, dTol,
/// bNeedsMoreWork, rSolutions, FALSE)` of `curve` with `other`
/// ([`curve_base::Curve::global_coincidence_checker`] on copies of the ellipses as
/// curve kinds); the solutions name `objects` (`[this, other]`).
fn global_coincidence_checker<O: Copy>(
    k: &Kernel,
    curve: &Ellipse,
    interval: &Extent1d,
    other: &Ellipse,
    other_interval: &Extent1d,
    tol: f64,
    objects: [Option<O>; 2],
) -> Result<(bool, Vec<Solution<O>>)> {
    let a = curve_base::Curve::from(curve.clone());
    let b = curve_base::Curve::from(other.clone());
    let handles = [
        Some(IndexHandle(0, curve.base.natural_interval())),
        Some(IndexHandle(1, other.base.natural_interval())),
    ];
    let r = a.global_coincidence_checker(k, interval, &b, other_interval, tol, false, handles)?;
    let name =
        |h: Option<IndexHandle>| h.and_then(|IndexHandle(i, _)| objects.get(i).copied().flatten());
    let sols = r
        .solutions
        .iter()
        .map(|s| {
            let mut out: Solution<O> = Solution::new();
            out.solution_type = s.solution_type;
            out.num_variables = s.num_variables;
            out.start = s.start;
            out.end = s.end;
            out.num_objects = s.num_objects;
            for (d, h) in out.objects.iter_mut().zip(s.objects) {
                *d = name(h);
            }
            out.nodes = s.nodes;
            out
        })
        .collect();
    Ok((r.additional_work_needed, sols))
}

/// A curve of a two-curve solve: its index (`0` this, `1` the other) and natural
/// interval (the solvers' duplicate tests read it).
#[derive(Debug, Clone, Copy, PartialEq)]
struct IndexHandle(usize, Extent1d);

impl SolutionObject for IndexHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.1)
    }
}

// ---- the polar conversion table -------------------------------------------------------

/// The table converting between the polar (STEP) angles of a circular degree 2
/// B-spline and its NURB parameters (`SmPolarConversion`): matching lists of the
/// natural knots and their angles in degrees, and per span whether the conversion is
/// exact (constant span weights).
///
/// Conversion within a span uses (CAGD, Hoschek/Lasser, p. 157)
/// `cos(angle) = ((1-t)^2 + 2t(1-t)cos(theta) + t^2 cos(2 theta)) / ((1-t)^2 +
/// 2t(1-t)cos(theta) + t^2)` with `theta` half the span angle.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolarConversion {
    /// `m_bPolarConversionPossible`: the tables are set up.
    pub(crate) possible: bool,
    /// `m_vNaturalKnots`.
    pub(crate) natural_knots: Vec<f64>,
    /// `m_vAngles`: the angle (degrees) of every natural knot.
    pub(crate) angles: Vec<f64>,
    /// `m_vExactConversion`: per span, `TRUE` when the span weights are constant.
    pub(crate) exact_conversion: Vec<bool>,
    /// `m_cpCurve != NULL` (the curve is always the owning ellipse, borrowed).
    pub(crate) has_curve: bool,
}

impl PolarConversion {
    /// `SmPolarConversion_TYPE` (`CONT_BASE_TYPE + 84`).
    pub const TYPE: i64 = POLAR_CONVERSION_TYPE;

    /// An empty table without curve; conversion not possible (`SmPolarConversion()`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            possible: false,
            natural_knots: Vec::new(),
            angles: Vec::new(),
            exact_conversion: Vec::new(),
            has_curve: false,
        }
    }

    /// Whether the tables are set up (`IsPolarConversionPossible`).
    #[must_use]
    pub const fn is_polar_conversion_possible(&self) -> bool {
        self.possible
    }

    /// Sets the "tables are set up" flag (`SetPolarConversionPossible`).
    pub fn set_polar_conversion_possible(&mut self, possible: bool) {
        self.possible = possible;
    }

    /// `SetContext`: only a non-borrowed curve takes the context; the curve here is
    /// always borrowed and contexts are not modelled, so nothing changes.
    pub fn set_context(&mut self) {}

    /// `"SmPolarConversion"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmPolarConversion"
    }

    /// `SmPolarConversion_TYPE` (`GetClassType`).
    #[must_use]
    pub const fn class_type(&self) -> i64 {
        Self::TYPE
    }

    /// The natural knots (`m_vNaturalKnots`).
    #[must_use]
    pub fn natural_knots(&self) -> &[f64] {
        &self.natural_knots
    }

    /// The angle in degrees of every natural knot (`m_vAngles`).
    #[must_use]
    pub fn angles(&self) -> &[f64] {
        &self.angles
    }

    /// Per span, whether the conversion is exact (`m_vExactConversion`).
    #[must_use]
    pub fn exact_conversion(&self) -> &[bool] {
        &self.exact_conversion
    }

    /// Whether the table has been attached to a curve (`m_cpCurve != NULL`).
    #[must_use]
    pub const fn has_curve(&self) -> bool {
        self.has_curve
    }

    /// `operator==`: same flag and table sizes, knots and angles equal within
    /// `SM_EFF_ZERO` (the exactness flags are only counted, as upstream).
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        // low work
        if core::ptr::eq(self, other) {
            return true;
        }

        // check equivalence of these objects (bHasCurve is FALSE: the curve is borrowed)
        let mut rtn = self.possible == other.possible
            && self.natural_knots.len() == other.natural_knots.len()
            && self.angles.len() == other.angles.len()
            && self.exact_conversion.len() == other.exact_conversion.len();

        // check real array values
        if rtn {
            for (a, b) in self.natural_knots.iter().zip(&other.natural_knots) {
                if !rtn {
                    break;
                }
                rtn &= is_zero(a - b);
            }
            for (a, b) in self.angles.iter().zip(&other.angles) {
                if !rtn {
                    break;
                }
                rtn &= is_zero(a - b);
            }
        }
        rtn
    }

    /// The NURB parameter of the angle `angle_deg` in the span from
    /// `start_nurb`/`start_angle_deg` to `end_nurb`/`end_angle_deg`
    /// (`AngleParamToNurbParam`): the root in `[0, 1]` of the span's quadratic, scaled
    /// to the span. Angles within `SM_EFF_ZERO_DEG` of an end give that end.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the angle is outside the span, the quadratic has no real root or
    /// no root in `[0, 1]`.
    pub fn angle_param_to_nurb_param(
        start_nurb: f64,
        end_nurb: f64,
        start_angle_deg: f64,
        end_angle_deg: f64,
        angle_deg: f64,
    ) -> Result<f64> {
        // locals
        let theta = (end_angle_deg - start_angle_deg) / 2.0 * PI / 180.0;
        let angle_tol = EFF_ZERO_DEG;

        // watch out for periodicity
        let ivl = Extent1d::from_min_max(
            min2(start_angle_deg, end_angle_deg),
            max2(start_angle_deg, end_angle_deg),
        );
        let inside = ivl.contains_value(angle_deg, 0.0); // Note, don't check 'periodic'. [B90]
        let angle_deg = ivl.clamp_value(angle_deg);

        if !inside {
            return Err(SolidError::sm_err(OP_ANGLE));
        }

        // quick convert end parameters
        if (angle_deg - start_angle_deg).abs() < angle_tol {
            return Ok(start_nurb);
        } else if (angle_deg - end_angle_deg).abs() < angle_tol {
            return Ok(end_nurb);
        }

        // shift angle to measure from StartAngle
        let angle_rad = (angle_deg - start_angle_deg) * PI / 180.0;

        // get coefficients of quadratic equation
        let cos_angle = angle_rad.cos();
        let cos_theta = theta.cos();
        let a = 2.0 * cos_angle * (1.0 - cos_theta) - 1.0 + 2.0 * cos_theta - (2.0 * theta).cos();
        let b = 2.0 * cos_angle * (-1.0 + cos_theta) + 2.0 - 2.0 * cos_theta;
        let c = cos_angle - 1.0;

        // quadratic equation determinate
        let mut d = b * b - 4.0 * a * c;

        // check determinate for error condition
        if d < 0.0 {
            return Err(SolidError::sm_err(OP_ANGLE));
        }

        // Solve quadratic Equation for Normalized T - get solution in region [0.0 to 1.0]
        d = d.sqrt();
        let mut t = (-b + d) / (2.0 * a);

        if !(0.0..=1.0).contains(&t) {
            t = (-b - d) / (2.0 * a);

            if !(0.0..=1.0).contains(&t) {
                return Err(SolidError::sm_err(OP_ANGLE));
            }
        }

        // scale normalized T to Nurb interval
        Ok((end_nurb - start_nurb) * t + start_nurb)
    }

    /// The NURB parameter of the polar (STEP) angle `polar_param_deg` of `curve` (the
    /// ellipse owning this table) and whether the conversion is exact
    /// (`ConvertToNURBSParameter`). The first guess maps the STEP interval linearly
    /// onto the knot range; with the tables set up, the angle (clamped to their range)
    /// is converted in its span. The guess is kept when its point is within
    /// `SM_EFF_ZERO` of the STEP point, otherwise refined by point inversion. The zone
    /// tolerance is not used.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a table without curve or knots (upstream dereferences
    /// them); the errors of [`Self::angle_param_to_nurb_param`] and of the
    /// evaluations and of the point inversion (`SmCurve::LocalPropertyAnalysis`).
    pub fn convert_to_nurbs_parameter(
        &self,
        k: &Kernel,
        curve: &Ellipse,
        polar_param_deg: f64,
        zone_tol: f64,
    ) -> Result<(f64, bool)> {
        let mut written = None;
        self.convert_to_nurbs_parameter_into(k, curve, polar_param_deg, zone_tol, &mut written)
    }

    /// [`Self::convert_to_nurbs_parameter`] for a caller that ignores its status: the
    /// parameter, or after a failure the value upstream had already written to
    /// `rdNurbsParameter` (the polar table's guess; `None` when nothing was written,
    /// an uninitialised value upstream).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] only.
    pub fn convert_to_nurbs_parameter_or_written(
        &self,
        k: &Kernel,
        curve: &Ellipse,
        polar_param_deg: f64,
        zone_tol: f64,
    ) -> Result<Option<f64>> {
        let mut written = None;
        match self.convert_to_nurbs_parameter_into(
            k,
            curve,
            polar_param_deg,
            zone_tol,
            &mut written,
        ) {
            Ok((t, _)) => Ok(Some(t)),
            Err(e) if e.is_unported() => Err(e),
            Err(_) => Ok(written),
        }
    }

    /// The body of [`Self::convert_to_nurbs_parameter`]; `written` follows upstream's
    /// writes to the out-parameter `rdNurbsParameter`.
    fn convert_to_nurbs_parameter_into(
        &self,
        k: &Kernel,
        curve: &Ellipse,
        polar_param_deg: f64,
        _zone_tol: f64,
        written: &mut Option<f64>,
    ) -> Result<(f64, bool)> {
        if !self.has_curve {
            return Err(SolidError::InvalidInput {
                operation: OP_TO_NURBS,
                reason: "polar converter has no curve",
            });
        }

        // init output (rbExactConversion = FALSE; every successful path below ends
        // with an exact conversion)
        let mut polar_deg = polar_param_deg;

        // locals
        let step_ivl = curve.step_interval();
        let inside_out = curve.inside_out();
        let angle_tol = EFF_ZERO_DEG;
        let first_knot = table(&self.natural_knots, 0, OP_TO_NURBS)?;
        let last_knot = table(
            &self.natural_knots,
            self.natural_knots.len().saturating_sub(1),
            OP_TO_NURBS,
        )?;

        // when bInsideOut the NurbDomain is opposite to the StepDomain
        let mut nurbs_guess = if inside_out {
            last_knot
                + (polar_deg - step_ivl.min()) / (step_ivl.max() - step_ivl.min())
                    * (first_knot - last_knot)
        } else {
            first_knot
                + (polar_deg - step_ivl.min()) / (step_ivl.max() - step_ivl.min())
                    * (last_knot - first_knot)
        };

        // when exact conversions tables are initialized - get a translation or a better
        // guess (rdNurbsParameter and the exactness found here are overwritten below)
        if self.possible {
            let first_angle = table(&self.angles, 0, OP_TO_NURBS)?;
            let last_angle = table(
                &self.angles,
                self.angles.len().saturating_sub(1),
                OP_TO_NURBS,
            )?;
            let mut angle_range = Extent1d::from_value(first_angle);
            angle_range.add_value(last_angle);
            let inside = angle_range.contains_value(polar_deg, angle_tol);
            if !inside {
                // Note, don't clamp 'periodic'. [B90]
                polar_deg = angle_range.clamp_value(polar_deg);
            }

            // check first angle boundary
            if are_same_to_tol(polar_deg, first_angle, angle_tol) {
                nurbs_guess = first_knot;
                *written = Some(nurbs_guess);
            } else {
                // for every arc interval
                for ii in 1..self.angles.len() {
                    let angle = table(&self.angles, ii, OP_TO_NURBS)?;

                    // check interval other bound
                    if are_same_to_tol(polar_deg, angle, angle_tol) {
                        nurbs_guess = table(&self.natural_knots, ii, OP_TO_NURBS)?;
                        *written = Some(nurbs_guess);
                        break;
                    }

                    // check interval interior
                    if (!inside_out && polar_deg <= angle) || (inside_out && polar_deg >= angle) {
                        // Try cheap exact conversion when possible
                        nurbs_guess = Self::angle_param_to_nurb_param(
                            table(&self.natural_knots, ii - 1, OP_TO_NURBS)?,
                            table(&self.natural_knots, ii, OP_TO_NURBS)?,
                            table(&self.angles, ii - 1, OP_TO_NURBS)?,
                            angle,
                            polar_deg,
                        )?;
                        *written = Some(nurbs_guess);
                        break;
                    }
                }
            }
        }

        // for tolerance purposes - always refine the current guess with a NewtonRaphson
        // solution: get Target 3DPoint and see if current guess is good enough
        let step_pnt = curve.step_point(polar_deg);
        let nurb_pnt = curve.base.evaluate_point(nurbs_guess)?;
        let dist = (nurb_pnt - step_pnt).length();

        let nurbs_param = if dist < EFF_ZERO * (1.0 + nurb_pnt.max_dimension()) {
            // current guess is good enough
            nurbs_guess
        } else {
            // Find closest point on Curve to GuessPoint
            let (found, param) = local_point_inversion(
                k,
                curve,
                &curve.base.natural_interval(),
                nurbs_guess,
                step_pnt,
            )?;
            if !found {
                return Err(SolidError::sm_err(OP_TO_NURBS));
            }
            param
        };
        *written = Some(nurbs_param);
        Ok((nurbs_param, true))
    }
}

// ---- the ellipse ----------------------------------------------------------------------

/// The optional inputs of [`Ellipse::new`] (the defaulted constructor arguments).
#[derive(Debug, Clone, Copy)]
pub struct EllipseOptions<'a> {
    /// Image-space dimension, 2 or 3 (default `3`).
    pub dim: u64,
    /// A B-spline whose NURB is copied instead of making one (`pOptNurb`): its ends
    /// are the ellipse's ends, it lies in the ellipse plane. It is kept only when its
    /// polar conversion can be set up.
    pub nurb: Option<&'a BSplineCurve>,
    /// `TRUE`: the NURB tangent is the negated STEP tangent (sphere and torus
    /// generating curves).
    pub inside_out: bool,
}

impl Default for EllipseOptions<'_> {
    fn default() -> Self {
        Self {
            dim: 3,
            nurb: None,
            inside_out: false,
        }
    }
}

/// The canonical data of an ellipse ([`Ellipse::canonical`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EllipseCanonical {
    /// Center, unit X axis (to the 0 degree point) and unit Y axis (to the 90 degree
    /// point).
    pub position: Axis2Placement,
    /// Radius at the 0 degree point.
    pub radius_x: f64,
    /// Radius at the 90 degree point.
    pub radius_y: f64,
    /// The STEP (angular) domain in degrees.
    pub domain: Extent1d,
    /// Whether the NURB and STEP parameterisations run in opposite directions.
    pub inside_out: bool,
}

/// Result of [`Ellipse::transform_point_to_start_plane`].
#[derive(Debug, Clone, PartialEq)]
pub struct EllipseStartPlanePoint {
    /// The point rotated about the ellipse's Z axis onto its X/Z half plane.
    pub point: Point3,
    /// `rlNumAngles`: `0` for a point on the Z axis, `1` off the seam, `2` on the seam
    /// of a closed ellipse.
    pub num_angles: usize,
    /// The angle(s) in degrees rotating the plane point back to the point (as
    /// `smgu_TransformPointToStartPlane` wrote them; two for a point on the axis).
    pub angles_deg: Vec<f64>,
    /// The angle is inside the STEP domain.
    pub inside: bool,
}

/// Result of [`Ellipse::intersect_with_ellipse`] and [`Ellipse::intersect_with_line`].
#[derive(Debug, Clone, PartialEq)]
pub struct EllipseIntersection<O> {
    /// Intersections found (`start.parameters`: `[this param, other param]`, both NURB
    /// parameters).
    pub solutions: Vec<Solution<O>>,
    /// Whether the general curve/curve intersector must be called.
    pub needs_more_intersections: bool,
}

/// An ellipse or circle (`SmEllipse`): `E(s) = O + rx cos(s) X + ry sin(s) Y` for the
/// angle `s` (degrees) in the STEP domain (`-360 <= min <= max <= 360`, at most 360
/// long), plus its rational quadratic B-spline and the polar conversion table between
/// the two parameterisations. `inside_out` ellipses (sphere and torus generating
/// curves) run their NURB opposite to the STEP domain.
#[derive(Debug, Clone)]
pub struct Ellipse {
    /// The `SmBSplineCurve` base object.
    pub(crate) base: BSplineCurve,
    /// `m_vPosition`: origin, X axis and Y axis.
    pub(crate) position: Axis2Placement,
    /// `m_vAnalDomain`: the angular domain in degrees.
    pub(crate) anal_domain: Extent1d,
    /// `m_dRadiusAtXAxis`.
    pub(crate) radius_x: f64,
    /// `m_dRadiusAtYAxis`.
    pub(crate) radius_y: f64,
    /// `m_vPolarConverter`.
    pub(crate) polar: PolarConversion,
    /// `m_bInsideOut`.
    pub(crate) inside_out: bool,
    /// The object is an `SmCircle` (the derived class; unit `curve.ellipse_p1`).
    pub(crate) circle: bool,
}

impl Ellipse {
    /// Exact evaluation copy without the B-spline copy constructor's knot fix-up.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        Self {
            base: self.base.clone_exact(),
            position: self.position,
            anal_domain: self.anal_domain,
            radius_x: self.radius_x,
            radius_y: self.radius_y,
            polar: self.polar.clone(),
            inside_out: self.inside_out,
            circle: self.circle,
        }
    }

    /// `SmEllipse_TYPE` (`CURV_BASE_TYPE + 22`).
    pub const TYPE: i64 = ELLIPSE_TYPE;

    // ---- construction -----------------------------------------------------------------

    /// The empty ellipse of dimension `dim` that `ReadFromDB` fills (`SmEllipse(lDim,
    /// cpContext)`): no NURB, the identity placement, the "inside out" domain, zero
    /// radii, an empty polar table; `m_bInsideOut`, uninitialised upstream, is
    /// `false`.
    #[must_use]
    pub const fn empty(dim: u64) -> Self {
        Self {
            base: BSplineCurve::new(dim),
            position: Axis2Placement::IDENTITY,
            anal_domain: Extent1d::new(),
            radius_x: 0.0,
            radius_y: 0.0,
            polar: PolarConversion::new(),
            inside_out: false,
            circle: false,
        }
    }

    /// The ellipse centred at `center` with radius `radius_x` along `x_axis` and
    /// `radius_y` along `y_axis` (axes made orthonormal by `SetCanonical`, status
    /// ignored) over the angular domain `domain` (degrees) (constructor, co-ported
    /// from unit `curve.ellipse_p1`). The NURB is copied from `opts.nurb` when its
    /// polar conversion can be set up, else made by [`Self::make_nurb`] (status
    /// ignored: a failure leaves no NURB); the form is `SM_CF_ELLIPTIC_ARC`.
    ///
    /// # Errors
    ///
    /// None but [`SolidError::Unported`] (never produced for an ellipse without
    /// owner).
    pub fn new(
        k: &Kernel,
        center: Point3,
        x_axis: Vec3,
        y_axis: Vec3,
        domain: Extent1d,
        radius_x: f64,
        radius_y: f64,
        opts: &EllipseOptions<'_>,
    ) -> Result<Self> {
        let mut e = Self {
            base: BSplineCurve::new(opts.dim),
            position: Axis2Placement::new(center, x_axis, y_axis),
            anal_domain: domain,
            radius_x,
            radius_y,
            polar: PolarConversion::new(),
            inside_out: opts.inside_out,
            circle: false,
        };

        // make the associated NURB curve
        let mut call_make_nurb = true;
        if let Some(nurb) = opts.nurb {
            // copy and save the gw_CURVE
            let stat = e.set_from_gw_nurb(k, nurb.gw_nurb());

            // If problems there, call MakeNurb().
            if stat.is_ok() && e.polar.is_polar_conversion_possible() {
                call_make_nurb = false;
            }
        }

        if call_make_nurb {
            ignore_status(e.make_nurb(k))?;
        }

        // set form factor
        e.base.form = BSplineCurveForm::EllipticArc;
        Ok(e)
    }

    /// `SmBSplineCurve::SetFromGwNurb` followed by the `SM_NO_POST_EDIT` notification,
    /// which rebuilds the polar converter (status ignored).
    pub(crate) fn set_from_gw_nurb(
        &mut self,
        k: &Kernel,
        gw_nurb: Option<&crate::nurbs::crv_basic::Curve>,
    ) -> Result<()> {
        self.base.set_from_gw_nurb(gw_nurb)?;
        self.notify_post_edit(k)
    }

    /// `Notify(SM_NO_POST_EDIT, ..)`: recompute the polar converter (status ignored).
    pub(crate) fn notify_post_edit(&mut self, k: &Kernel) -> Result<()> {
        ignore_status(self.set_up_polar_conversion(k))
    }

    // ---- access -----------------------------------------------------------------------

    /// The `SmBSplineCurve` base object.
    #[must_use]
    pub const fn base(&self) -> &BSplineCurve {
        &self.base
    }

    /// Enables evaluation beyond the natural interval, without changing the NURB
    /// or STEP domain (inherited `SmBSplineCurve::SetOutOfBoundsEnabled`).
    pub fn set_out_of_bounds_enabled(&mut self, enabled: bool) {
        self.base.set_out_of_bounds_enabled(enabled);
    }

    /// The `SmBSplineCurve` base object, mutable (for the base-class edits).
    pub(crate) fn base_mut(&mut self) -> &mut BSplineCurve {
        &mut self.base
    }

    /// The polar conversion table (`m_vPolarConverter`).
    #[must_use]
    pub const fn polar_converter(&self) -> &PolarConversion {
        &self.polar
    }

    /// The STEP (angular) domain in degrees (`GetSTEPInterval`).
    #[must_use]
    pub const fn step_interval(&self) -> Extent1d {
        self.anal_domain
    }

    /// Replaces the STEP domain without touching the NURB (`SetSTEPInterval`; can
    /// make the curve inconsistent, meant for very temporary changes).
    pub fn set_step_interval(&mut self, domain: Extent1d) {
        self.anal_domain = domain;
    }

    /// Always `2` (`GetDegree`).
    #[must_use]
    pub const fn degree(&self) -> usize {
        2
    }

    /// The start of the STEP domain in degrees (`GetStartAngleDeg`).
    #[must_use]
    pub const fn start_angle_deg(&self) -> f64 {
        self.anal_domain.min()
    }

    /// The end of the STEP domain in degrees (`GetEndAngleDeg`).
    #[must_use]
    pub const fn end_angle_deg(&self) -> f64 {
        self.anal_domain.max()
    }

    /// The placement: center, X axis and Y axis (`GetPosition`).
    #[must_use]
    pub const fn position(&self) -> &Axis2Placement {
        &self.position
    }

    /// The radius at the 0 degree point (`GetXRadius`).
    #[must_use]
    pub const fn x_radius(&self) -> f64 {
        self.radius_x
    }

    /// The radius at the 90 degree point (`GetYRadius`).
    #[must_use]
    pub const fn y_radius(&self) -> f64 {
        self.radius_y
    }

    /// Whether the NURB runs opposite to the STEP domain (`GetInsideOut`).
    #[must_use]
    pub const fn inside_out(&self) -> bool {
        self.inside_out
    }

    /// Always `true` (`IsAnalytic`).
    #[must_use]
    pub const fn is_analytic(&self) -> bool {
        true
    }

    /// Always [`SurfParamType::Neither`]: an ellipse is never a domain boundary
    /// (`IsDomainBoundary`).
    #[must_use]
    pub const fn is_domain_boundary(&self) -> SurfParamType {
        SurfParamType::Neither
    }

    /// `SmEllipse_TYPE`, or `SmCircle_TYPE` for a circle (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        if self.circle {
            CIRCLE_TYPE
        } else {
            Self::TYPE
        }
    }

    /// `SmEllipse_TYPE` (`GetClassType`).
    #[must_use]
    pub const fn class_type() -> i64 {
        Self::TYPE
    }

    /// `"SmEllipse_TYPE"`, or `"SmCircle_TYPE"` for a circle (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        if self.circle {
            "SmCircle_TYPE"
        } else {
            "SmEllipse_TYPE"
        }
    }

    /// `"SmEllipse"`, or `"SmCircle"` for a circle (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        if self.circle {
            "SmCircle"
        } else {
            "SmEllipse"
        }
    }

    /// True for `SmEllipse_TYPE` and the types of its bases (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(t: i64) -> bool {
        t == Self::TYPE || t == BSplineCurve::TYPE || curve_base::is_kind_of(t)
    }

    /// The canonical data: placement, radii, STEP domain and inside-out flag
    /// (`GetCanonical`).
    #[must_use]
    pub const fn canonical(&self) -> EllipseCanonical {
        EllipseCanonical {
            position: self.position,
            radius_x: self.radius_x,
            radius_y: self.radius_y,
            domain: self.anal_domain,
            inside_out: self.inside_out,
        }
    }

    /// The largest analytic domain (`GetMaxAnalyticDomain`): the domain when it spans
    /// 360 degrees, `[0, 360]` when it starts at or after 0, `[-360, 0]` when it ends at
    /// or before 0, else `[min, min + 360]`.
    #[must_use]
    pub fn max_analytic_domain(&self) -> Extent1d {
        let ivl = self.anal_domain;

        if ivl.length() > 360.0 - EFF_ZERO {
            return ivl;
        }

        if ivl.min() > 0.0 - EFF_ZERO {
            return Extent1d::from_min_max(0.0, 360.0);
        }

        if ivl.max() < 0.0 + EFF_ZERO {
            return Extent1d::from_min_max(-360.0, 0.0);
        }

        let min = ivl.min();
        Extent1d::from_min_max(min, min + 360.0)
    }

    /// True when the radii agree within the scaled zero (`IsCircle`).
    #[must_use]
    pub fn is_circle(&self) -> bool {
        let scaled_zero = EFF_ZERO * (1.0 + max2(self.x_radius().abs(), self.y_radius().abs()));

        // TRUE when radii are equal
        (self.x_radius() - self.y_radius()).abs() < scaled_zero
    }

    /// True when the larger radius is below half of `tol` or the arc (larger radius
    /// times the swept angle) is shorter than `tol`, with `tol` at least `SM_EFF_ZERO`
    /// (`IsDegenerate`; the interval argument is not used upstream).
    #[must_use]
    pub fn is_degenerate(&self, tol: f64) -> bool {
        let tol = max2(tol, EFF_ZERO);

        // characterize the arc radius and length
        let max_radius = max2(self.radius_x, self.radius_y);
        let arc_length = max_radius * self.anal_domain.length() * PI / 180.0;

        // check for small radius or short interval
        (max_radius < tol / 2.0) || (arc_length < tol)
    }

    // ---- comparison -------------------------------------------------------------------

    /// `operator==`: equal B-spline bases, placements equal within the scaled zero,
    /// equal domains, radii equal within `SM_EFF_ZERO`, equal polar tables and the same
    /// inside-out flag.
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
            && self.anal_domain.are_same(&other.anal_domain)
            && is_zero(self.radius_x - other.radius_x)
            && is_zero(self.radius_y - other.radius_y)
            && self.polar.approx_eq(&other.polar)
            && self.inside_out == other.inside_out
    }

    // ---- polar conversion -------------------------------------------------------------

    /// Sets up the polar conversion table from the NURB (`m_vPolarConverter
    /// .SetUpPolarConversion(this, TRUE, ..)`), assuming the zero angle is the start
    /// of the curve. The table is attached and marked impossible; it stays so when the
    /// interior knots are not all double. Otherwise, for a domain shorter than 360
    /// degrees, the STEP domain is re-derived from the NURB end points (moved to the
    /// period overlapping the old domain most, ends within `1000 SM_EFF_ZERO` snapped
    /// back); then every natural knot gets its angle (ends mapped to the domain ends,
    /// others snapped to -360, 0 or 360 within `SM_EFF_ZERO_DEG`) and every span its
    /// exactness flag, and the STEP domain becomes the range of the first and last
    /// angles.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB, when the end knots are not triple or the tables come
    /// out inconsistent; the errors of the evaluations (the STEP domain may already
    /// be changed, as upstream).
    pub fn set_up_polar_conversion(&mut self, k: &Kernel) -> Result<()> {
        let mut pc = core::mem::take(&mut self.polar);
        let r = set_up_polar_conversion(&mut pc, self, k);
        self.polar = pc;
        r
    }

    /// The NURB parameter of the STEP angle `step_param` in degrees
    /// (`ConvertTFromSTEPToNURBS`: the polar converter's
    /// [`PolarConversion::convert_to_nurbs_parameter`]).
    ///
    /// # Errors
    ///
    /// As [`PolarConversion::convert_to_nurbs_parameter`].
    pub fn convert_t_from_step_to_nurbs(&self, k: &Kernel, step_param: f64) -> Result<f64> {
        Ok(self
            .polar
            .convert_to_nurbs_parameter(k, self, step_param, 0.0)?
            .0)
    }

    // ---- evaluation -------------------------------------------------------------------

    /// The point and derivatives `1..=num_derivs` (at most 3) at the STEP angle
    /// `step_param` (degrees), mapped into the domain periodically or else clamped to
    /// it; derivatives are per degree (`EvaluateSTEP`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for more than 3 derivatives.
    pub fn evaluate_step(&self, step_param: f64, num_derivs: usize) -> Result<Vec<Vec3>> {
        if num_derivs > 3 {
            return Err(SolidError::sm_err(OP_EVAL_STEP));
        }
        Ok(self.eval_step(step_param, num_derivs))
    }

    /// The point at the STEP angle `step_param` (degrees) (`EvaluateSTEPPoint`).
    ///
    /// # Errors
    ///
    /// None (the status of [`Self::evaluate_step`] with no derivatives).
    pub fn evaluate_step_point(&self, step_param: f64) -> Result<Point3> {
        Ok(self.step_point(step_param))
    }

    /// [`Self::evaluate_step`] without derivatives.
    pub(crate) fn step_point(&self, step_param: f64) -> Point3 {
        self.eval_step(step_param, 0)
            .first()
            .copied()
            .unwrap_or(Vec3::UNDEF)
    }

    /// The body of `EvaluateSTEP` for `num_derivs <= 3`.
    fn eval_step(&self, step_param: f64, num_derivs: usize) -> Vec<Vec3> {
        let mut t = step_param;
        let c = self.anal_domain.contains_periodic_value(t, 360.0, -1.0);
        t = c.periodic_value;
        if !c.contained {
            t = self.anal_domain.clamp_periodic_value(t, 360.0, -1.0);
        }

        // get input in radians
        let angle_rad = t * PI / 180.0;
        let cos_u = angle_rad.cos();
        let sin_u = angle_rad.sin();
        let o = self.position.origin();
        let x = self.position.x_axis();
        let y = self.position.y_axis();
        let rx = self.radius_x;
        let ry = self.radius_y;

        let mut out = Vec::with_capacity(num_derivs + 1);

        // eval position
        out.push(o + rx * cos_u * x + ry * sin_u * y);

        if num_derivs > 0 {
            let un_scaled = -rx * sin_u * x + ry * cos_u * y;
            out.push(un_scaled * PI / 180.0);
        }

        if num_derivs > 1 {
            let un_scaled = -rx * cos_u * x - ry * sin_u * y;
            out.push(un_scaled * PI / 180.0 * PI / 180.0);
        }

        if num_derivs > 2 {
            let un_scaled = rx * sin_u * x - ry * cos_u * y;
            out.push(un_scaled * PI / 180.0 * PI / 180.0 * PI / 180.0);
        }
        out
    }

    /// The ellipse angle in degrees of `point` (in the ellipse plane) and, when
    /// `classify`, its location (`STEPInversion`). The angle is
    /// `atan2(rx / ry * y, x)` of the point's local coordinates, in `[0, 360)`, mapped
    /// periodically into the domain; on a closed ellipse a point within the
    /// approximation tolerance (`zone_tol / 100`; `zone_tol == 0` uses the context
    /// zone tolerance) of the end point at the domain end returns the domain start.
    /// The location is exterior outside the arc or farther than the scaled zero from
    /// the ellipse, the seam at the start of a closed ellipse, else interior.
    #[must_use]
    pub fn step_inversion(
        &self,
        k: &Kernel,
        point: Point3,
        classify: bool,
        zone_tol: f64,
    ) -> (f64, Option<CurveLocationType>) {
        let center = self.position.origin();
        let x_axis = self.position.x_axis();
        let y_axis = self.position.y_axis();

        // First, find its local coordinates with respect to axes of ellipse
        let vec = point - center;
        let dx = vec.dot(x_axis);
        let dy = vec.dot(y_axis);

        // get dAngleParam in radians [-Pi, +Pi]
        let mut angle_param = 0.0;
        if self.radius_y != 0.0 {
            angle_param = (self.radius_x / self.radius_y * dy).atan2(dx);
        }

        // convert to degrees, in [ 0, 360 ) strictly positive result [B630]
        if angle_param < 0.0 {
            angle_param += TWO_PI;
        }
        let mut angular_param = angle_param * (180.0 / PI);

        // watch out for periodicity - map output to proper period
        let c = self
            .anal_domain
            .contains_periodic_value(angular_param, 360.0, -1.0);
        let in_arc = c.contained;
        angular_param = c.periodic_value;

        // Get 3d tolerance [B630] (no edge)
        let mut z_tol = 0.0;
        if zone_tol != 0.0 {
            z_tol = zone_tol;
        }
        if z_tol == 0.0 {
            // Because sometimes an edge has 0.0 tol
            z_tol = context_zone_tol_3d(k, None, SmallTopoType::NormalSize);
        }
        let approx_tol = z_tol / 100.;

        // Calculate end point on Ellipse
        let angle_rad = self.anal_domain.max() * PI / 180.0;
        let cos_u = angle_rad.cos();
        let sin_u = angle_rad.sin();
        let end = self.position.origin()
            + self.radius_x * cos_u * self.position.x_axis()
            + self.radius_y * sin_u * self.position.y_axis();

        // return min seam values for closed ellipses
        if is_zero_to_tol(end.distance(point), approx_tol)
            && are_same_to_tol(self.anal_domain.max(), angular_param, EFF_ZERO_DEG)
            && are_same_to_tol(self.anal_domain.length(), 360.0, EFF_ZERO_DEG)
        {
            angular_param = self.anal_domain.min();
        }

        // when asked - classify the point
        let loc = classify.then(|| {
            // 1st check point is in angular sweep
            let mut loc = if in_arc {
                // then check point/curve distance
                let ellipse_point = self.step_point(angular_param);
                let dist = ellipse_point.distance(point);
                let scaled_zero = EFF_ZERO * (1.0 + ellipse_point.max_dimension());
                if dist > scaled_zero {
                    CurveLocationType::Exterior
                } else {
                    CurveLocationType::Interior
                }
            } else {
                CurveLocationType::Exterior
            };

            // check for seam values
            if are_same(self.anal_domain.min(), angular_param)
                && are_same(self.anal_domain.length(), 360.0)
            {
                loc = CurveLocationType::Seam;
            }
            loc
        });

        (angular_param, loc)
    }

    /// Rotates `point` about the ellipse's Z axis onto its X/Z half plane
    /// (`TransformPointToStartPlane`: `smgu_TransformPointToStartPlane` over the
    /// placement and STEP domain); a point within the scaled zero of the axis has no
    /// angle (`num_angles` 0).
    ///
    /// # Errors
    ///
    /// The errors of `smgu_TransformPointToStartPlane`.
    pub fn transform_point_to_start_plane(
        &self,
        point: Point3,
        dist_tol_3d: f64,
        snap_to_seams: bool,
    ) -> Result<EllipseStartPlanePoint> {
        // pass the call along
        let sp = transform_point_to_start_plane(
            point,
            &self.position,
            &self.anal_domain,
            dist_tol_3d,
            snap_to_seams,
        )?;
        let mut num_angles = sp.angles_deg.len();

        // low work - point is on the axis to within tolerance - e.g. an ambiguous center point
        let scaled_zero = EFF_ZERO * (1.0 + point.max_dimension());
        if sp.dist_to_z_axis < scaled_zero {
            // Point is on the axis of revolution - no unique solutions
            num_angles = 0;
        }

        Ok(EllipseStartPlanePoint {
            point: sp.point,
            num_angles,
            angles_deg: sp.angles_deg,
            inside: sp.inside,
        })
    }

    // ---- solvers ----------------------------------------------------------------------

    /// The closest point(s) of a circle or circular arc over `interval` (NURB domain)
    /// to `test_point` (`DropPointFast`): the point's angle about the axis converted to
    /// a NURB parameter and clamped to `interval` (both seam angles on a closed circle
    /// unless `in_pointing` picks one; both domain ends for a point on the axis). For
    /// [`SolverOperation::Minimize`] a solution farther than `target_distance + tol`
    /// (when given) is dropped, for [`SolverOperation::Intersect`] one farther than
    /// `tol`; [`SolutionRequested::Single`] keeps the nearer of two (the first on a
    /// tie). The solutions name `this`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the polar conversion is not set up, the ellipse is not a circle or
    /// the operation is neither (callers then run the general point solver); the
    /// errors of the conversions and evaluations.
    pub fn drop_point_fast<O: Copy>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        operation: SolverOperation,
        test_point: Point3,
        in_pointing: Option<Vec3>,
        tol: f64,
        target_distance: Option<f64>,
        requested: SolutionRequested,
        this: Option<O>,
    ) -> Result<Vec<Solution<O>>> {
        // init outputs
        let mut solutions = Vec::new();

        // no work - no polar conversion
        if !self.polar.is_polar_conversion_possible() {
            return Err(SolidError::sm_err(OP_DROP));
        }

        // no work - not a circle or circular arc
        if !self.is_circle() {
            return Err(SolidError::sm_err(OP_DROP));
        }

        // no work - operation not minimize or intersect
        if operation != SolverOperation::Minimize && operation != SolverOperation::Intersect {
            return Err(SolidError::sm_err(OP_DROP));
        }

        // get angle of TestPoint in ellipse coordinate system as a side effect of
        // rotating TestPoint about ellipse's Z-Axis to the X/Z Plane
        let sp = self.transform_point_to_start_plane(test_point, tol, false)?;
        let mut num_angles = sp.num_angles;
        let mut angles_deg = [
            sp.angles_deg.first().copied().unwrap_or(0.0),
            sp.angles_deg.get(1).copied().unwrap_or(0.0),
        ];

        // lNumAngles == 2 TestPoint is on seam of closed ellipse
        // and we are given an cpOptInPointingVector
        if num_angles == 2 {
            // when Optional Interior Pointing Vector is given - use it to select desired solution
            if let Some(v) = in_pointing {
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

        // Check for case where point is on Z_AXIS of the ellipse
        if num_angles == 0 {
            angles_deg = [self.start_angle_deg(), self.end_angle_deg()];
            num_angles = 2;
        }

        // initialize solution object
        let mut sol = Solution::new();
        sol.solution_type = SolutionType::SingleValue;
        sol.num_objects = 1;
        sol.objects[0] = this;
        sol.nodes[0] = None;
        sol.num_variables = 1;

        // Parameter-space tolerance for inside-domain checks
        let param_tol = EFF_ZERO_DEG;

        // In case of two solutions, if one is better, use it: compare solution values. [220629]
        let mut dists = [0.0; 2];

        // For every angle, convert the angle to curve parameter and set up the Solutions.
        for (angle, dist) in angles_deg.iter().zip(dists.iter_mut()).take(num_angles) {
            // clamp angle to full curve analytic extent - so convert angle will work
            let clamped_angle = if num_angles == 1 {
                self.anal_domain
                    .clamp_periodic_value(*angle, 360.0, param_tol)
            } else {
                self.anal_domain.clamp_value(*angle)
            };

            // convert angle to curve parameter value
            let (mut t, exact) =
                self.polar
                    .convert_to_nurbs_parameter(k, self, clamped_angle, tol)?;

            // clamp parameter to given Nurb interval - which may be a subset of the full curve interval
            t = interval.clamp_value(t);

            // when the conversion is not exact
            if !exact {
                let pnt = self.step_point(clamped_angle);

                // refine parameter value with NewtonRaphson iteration
                let (found, refined) = local_point_inversion(k, self, interval, t, pnt)?;
                if found {
                    t = refined;
                }
            }

            // reclamp point and get 3DPoint
            t = interval.clamp_value(t);
            let pnt = self.base.evaluate_point(t)?;

            // get testPoint/ProjectedPoint gap
            *dist = pnt.distance(test_point);

            // skip points too far away
            if (operation == SolverOperation::Minimize
                && target_distance.is_some_and(|d| *dist > d + tol))
                || (operation == SolverOperation::Intersect && *dist > tol)
            {
                continue;
            }

            // save the solution
            sol.start.solution_value = *dist;
            sol.start.parameters[0] = t;
            solutions.push(sol);
        }

        // Chose the value to use when only one solution is requested
        if requested == SolutionRequested::Single && solutions.len() > 1 {
            // Note: if the two values are equal, use the first.
            if dists[1] < dists[0] {
                solutions.remove(0);
            } else {
                solutions.truncate(1);
            }
        }
        Ok(solutions)
    }

    /// Intersections of this ellipse over `interval` with `other` over
    /// `other_interval` (NURB domains) within `tol` (`IntersectWithEllipse`; circles
    /// only). Non-circles, and circles in planes less than 10 degrees apart but not
    /// parallel, need the general intersector; the same curve has no intersections.
    /// Parallel planes farther apart than `tol` and circles that miss give none.
    /// Otherwise each candidate point is dropped onto both curves and refined with the
    /// local curve solver. Solutions name `this_obj` and `other_obj`.
    ///
    /// # Errors
    ///
    /// The errors of the plane and circle intersections; [`SolidError::Unported`]
    /// from the general point solver and the local curve solver (their Newton equations,
    /// `curve.curve_p5`) and the coincidence checker (`curve.curve_p6`).
    pub fn intersect_with_ellipse<O: Copy>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        tol: f64,
        this_obj: Option<O>,
        other_obj: Option<O>,
    ) -> Result<EllipseIntersection<O>> {
        // init output
        let mut out = EllipseIntersection {
            solutions: Vec::new(),
            needs_more_intersections: true,
        };

        // Right now this method only works with two circles.
        if !self.is_circle() || !other.is_circle() {
            return Ok(out);
        }

        // no work - the same curve does not self intersect
        if core::ptr::eq(self, other) {
            out.needs_more_intersections = false;
            return Ok(out);
        }

        // locals
        let this_normal = self.position.z_axis();
        let this_center = self.position.origin();
        let other_normal = other.position.z_axis();
        let other_center = other.position.origin();

        let mut intersection_points: Vec<Point3> = Vec::new();

        // Check for coplanar circles: use the sine of the angle, not the cosine.
        let cross = this_normal.cross(other_normal);
        let sine_ang = cross.length();

        let exactly_parallel = sine_ang <= EFF_ZERO_SQRT; // about 0.0000573 degrees
        let nearly_parallel = sine_ang < SINE_10DEG;

        if exactly_parallel {
            // Circles are on the same or parallel planes

            // Check whether it's the same plane.  Project OtherCenter to ThisPlane.
            let proj_pnt = point_project_to_plane(other_center, this_center, this_normal);
            let dist_from_plane = proj_pnt.distance(other_center);

            // no work - no intersections
            if dist_from_plane > tol {
                out.needs_more_intersections = false;
                return Ok(out);
            }

            // intersect two coplanar circles
            match circle_coplanar_circle_intersect(
                this_normal,
                this_center,
                self.x_radius(),
                other_center,
                other.x_radius(),
                tol,
            )? {
                // no intersections - all done
                CircleCircle::None => {
                    out.needs_more_intersections = false;
                    return Ok(out);
                }
                CircleCircle::Tangent(p) => intersection_points.push(p),
                CircleCircle::Crossing(p0, p1) => {
                    intersection_points.push(p0);
                    intersection_points.push(p1);
                }
                // check for coincident circles
                CircleCircle::Coincident => {
                    let (_needs_more_work, sols) = global_coincidence_checker(
                        k,
                        self,
                        interval,
                        other,
                        other_interval,
                        tol,
                        [this_obj, other_obj],
                    )?;
                    out.solutions = sols;
                    out.needs_more_intersections = false;
                    return Ok(out);
                }
            }
        } else {
            if nearly_parallel {
                // Don't use analytical intersection because the angle is too small
                // between the planes.
                return Ok(out);
            }

            // Get the line of intersection of the two planes and see if it intersects
            // this ellipse.
            let (line_pnt, line_vec) =
                match intersect_two_planes(this_center, this_normal, other_center, other_normal)? {
                    PlanesIntersection::Line { point, direction } => (point, direction),
                    PlanesIntersection::Parallel { .. } => {
                        return Err(SolidError::sm_err(
                            "curve::ellipse::p2::Ellipse::intersect_with_ellipse",
                        ))
                    }
                };

            // intersect circle with line of intersections
            let line_params = line_coplanar_circle_intersect(
                line_pnt,
                line_vec,
                this_center,
                this_normal,
                self.x_radius(),
                tol * 10.0,
            )?;

            // no intersections
            if line_params.is_empty() {
                out.needs_more_intersections = false;
                return Ok(out);
            }

            // Use each line intersection as a guess point for iteration.
            for p in &line_params {
                // get the 3d intersection point
                intersection_points.push(line_pnt + *p * line_vec);
            }
        }

        // Drop point onto each curve to get the intersection point.
        let mut sol = Solution::new();
        sol.num_objects = 2;
        sol.objects[0] = this_obj;
        sol.objects[1] = other_obj;
        sol.num_variables = 2;
        sol.solution_type = SolutionType::SingleValue;

        for ip in &intersection_points {
            let solutions1: Vec<Solution<O>> =
                ellipse_global_point_solve(k, self, interval, *ip, tol * 10.0)?;

            if solutions1.is_empty() {
                continue;
            }

            let solutions2: Vec<Solution<O>> =
                ellipse_global_point_solve(k, other, other_interval, *ip, tol * 10.0)?;

            // Check every combination
            for s1 in &solutions1 {
                for s2 in &solutions2 {
                    sol.start.parameters[0] = s1.start.parameters[0];
                    sol.start.parameters[1] = s2.start.parameters[0];

                    let mut p1 = self.base.evaluate_point(sol.start.parameters[0])?;
                    let mut p2 = other.base.evaluate_point(sol.start.parameters[1])?;

                    let mut dist = p1.distance(p2);

                    let scaled_zero = EFF_ZERO * (1.0 + p1.max_dimension());

                    // Do refinement in any case.
                    if dist < tol * 100.0 // close enough to be a contender
                        && dist > tol / 100.0
                    // far enough to need refinement
                    {
                        let (_found, sol2): (bool, Solution<O>) = local_curve_solve_ellipse(
                            k,
                            self,
                            interval,
                            other,
                            other_interval,
                            tol,
                            dist,
                            [sol.start.parameters[0], sol.start.parameters[1]],
                        )?;

                        // If the result of the refinement is better than the original
                        // point-drops, use it when the parameters are within range.
                        let t1 = sol2.start.parameters[0];
                        let t2 = sol2.start.parameters[1];
                        if interval.contains_value(t1, EFF_ZERO)
                            && other_interval.contains_value(t2, EFF_ZERO)
                        {
                            p1 = self.base.evaluate_point(t1)?;
                            p2 = other.base.evaluate_point(t2)?;

                            let dist2 = p1.distance(p2);

                            if dist2 < dist {
                                dist = dist2;
                                sol.start.parameters[0] = sol2.start.parameters[0];
                                sol.start.parameters[1] = sol2.start.parameters[1];
                            }
                        }
                    }

                    // All refined if necessary; see about using this result.
                    if dist <= tol + scaled_zero {
                        sol.start.solution_value = dist;
                        out.solutions.push(sol);
                    }
                }
            }
        }

        out.needs_more_intersections = false;
        Ok(out)
    }

    /// Intersections of this ellipse over `interval` with `line` over
    /// `other_interval` (NURB domains) within `tol` (`IntersectWithLine`; circles
    /// only). A line more than 10 degrees out of the circle's plane is tried at its
    /// plane crossing; otherwise the line projected into the plane is intersected with
    /// the circle (a line perpendicular to the plane needs the general intersector,
    /// one that misses gives none). Each candidate point is dropped onto both curves
    /// and, when the line is not exactly in the plane, refined with the local curve
    /// solver. Solutions name `this_obj` and `line_obj`; `start.parameters` holds
    /// `[ellipse param, line param]`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` from the plane intersection of a line parallel to the plane or a zero
    /// line vector; [`SolidError::Unported`] from the general point solver and the
    /// local curve solver (their Newton equations, `curve.curve_p5`).
    pub fn intersect_with_line<O: Copy>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        line: &Line,
        other_interval: &Extent1d,
        tol: f64,
        this_obj: Option<O>,
        line_obj: Option<O>,
    ) -> Result<EllipseIntersection<O>> {
        // init output
        let mut out = EllipseIntersection {
            solutions: Vec::new(),
            needs_more_intersections: true,
        };

        // Right now this method works only with circles.
        if !self.is_circle() {
            return Ok(out);
        }

        let this_normal = self.position.z_axis();
        let this_center = self.position.origin();

        let start = line.line_point();
        let line_vec = line.line_vector() * line.line_scale();

        let mut intersection_points: Vec<Point3> = Vec::new();
        let mut may_need_refinement = false;

        // Check to see if circles and line are somewhat on the same plane.
        let d_tol = EFF_ZERO * (1.0 + start.max_dimension());
        let dot = if line_vec.length_squared() < d_tol * d_tol {
            BIG_DOUBLE
        } else {
            let tmp_lv = line_vec.unitize()?;
            this_normal.dot(tmp_lv)
        };

        if dot.abs() > SINE_10DEG {
            // Line is not within 10 degrees of this ellipse's plane: only one
            // intersection possible, where the line intersects the plane.
            let line_param = line_plane_intersect(start, line_vec, this_center, this_normal)?;
            intersection_points.push(start + line_param * line_vec);
            may_need_refinement = true;
        } else {
            if dot.abs() > EFF_ZERO_SQRT {
                may_need_refinement = true; // not exactly in plane.
            }

            // Project the line to the plane of the ellipse.
            let line_plane_pnt = point_project_to_plane(start, this_center, this_normal);
            let line_plane_vec = vector_project_to_plane(line_vec, this_normal);

            if line_plane_vec.length_squared() < EFF_ZERO_SQ {
                out.needs_more_intersections = true; // Line is perp to plane.
                return Ok(out);
            }

            // Intersect the projected line with the ellipse.
            let line_params = line_coplanar_circle_intersect(
                line_plane_pnt,
                line_plane_vec,
                this_center,
                this_normal,
                self.x_radius(),
                tol * 10.0,
            )?;
            if line_params.is_empty() {
                out.needs_more_intersections = false;
                return Ok(out);
            }
            for p in &line_params {
                intersection_points.push(line_plane_pnt + *p * line_plane_vec);
            }
        }

        // Drop point onto each curve to get the intersection point.
        let mut sol = Solution::new();
        sol.num_objects = 2;
        sol.objects[0] = this_obj;
        sol.objects[1] = line_obj;
        sol.num_variables = 2;
        sol.solution_type = SolutionType::SingleValue;

        for ip in &intersection_points {
            // (a failing status returns SM_SUCCESS upstream; Unported propagates)
            let solutions2: Vec<Solution<O>> =
                match line_global_point_solve(line, other_interval, *ip, tol * 10.0) {
                    Ok(s) => s,
                    Err(e @ SolidError::Unported { .. }) => return Err(e),
                    Err(_) => return Ok(out),
                };

            if solutions2.is_empty() {
                continue;
            }

            let solutions1: Vec<Solution<O>> =
                match ellipse_global_point_solve(k, self, interval, *ip, tol * 10.0) {
                    Ok(s) => s,
                    Err(e @ SolidError::Unported { .. }) => return Err(e),
                    Err(_) => return Ok(out),
                };

            // Check every combination
            for s1 in &solutions1 {
                for s2 in &solutions2 {
                    sol.start.parameters[0] = s1.start.parameters[0]; // ellipse
                    sol.start.parameters[1] = s2.start.parameters[0]; // line

                    let p1 = self.base.evaluate_point(sol.start.parameters[0])?;
                    let p2 = line.evaluate_point(sol.start.parameters[1]);

                    let mut dist = p1.distance(p2);

                    let scaled_zero = EFF_ZERO * (1.0 + p1.max_dimension());

                    if may_need_refinement
                        && dist < tol * 10.0 // close enough to be a contender
                        && dist > tol / 100.0
                    // far enough to need refinement
                    {
                        let (found, sol2): (bool, Solution<O>) = local_curve_solve_line(
                            k,
                            self,
                            interval,
                            line,
                            other_interval,
                            tol,
                            dist,
                            [sol.start.parameters[0], sol.start.parameters[1]],
                        )?;

                        // note: sSol2.m_vStart.m_dSolutionValue is positive definite
                        if found && sol2.start.solution_value <= tol + scaled_zero {
                            dist = sol2.start.solution_value;
                            sol.start.parameters[0] = sol2.start.parameters[0];
                            sol.start.parameters[1] = sol2.start.parameters[1];
                        }
                    }

                    if dist <= tol + scaled_zero {
                        sol.start.solution_value = dist;
                        out.solutions.push(sol);
                    }
                }
            }
        }

        out.needs_more_intersections = false;
        Ok(out)
    }

    // ---- edits ------------------------------------------------------------------------

    /// Makes the ellipse 3D (`ConvertTo3D`): nothing for a 3D ellipse; else the
    /// dimension becomes 3 and the NURB's `z` values `0` (`SmBSplineCurve::ConvertTo3D`,
    /// whose status is ignored: an ellipse without NURB only changes its dimension).
    pub fn convert_to_3d(&mut self) {
        if self.base.dim == 3 {
            return;
        }
        self.base.dim = 3;

        // convert the underlying Nurb
        let _ = self.base.convert_to_3d();
    }

    /// Scales the ellipse uniformly by `scale` about the global origin (optional),
    /// then rotates and moves it by `rotate_n_move` (`Transform`): the placement and
    /// radii change, and the NURB (if any) is transformed, rebuilding the polar
    /// converter. The identity placement with no or unit scale changes nothing.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a non-uniform or zero scale, or when the scaled placement or the
    /// NURB transform fails.
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
            if !are_same(s.x, s.y) || !are_same(s.x, s.z) || s.x.abs() < EFF_ZERO {
                return Err(SolidError::sm_err(OP_TRANSFORM));
            }
            d_scale = s.x;
        }

        // scale current ellipse placement
        let mut ell_place = Axis2Placement::IDENTITY;
        ell_place
            .set_canonical(
                self.position.origin() * d_scale,
                self.position.x_axis(),
                self.position.y_axis(),
            )
            .map_err(|_| SolidError::sm_err(OP_TRANSFORM))?;

        // apply rotation to scaled placement
        let tmp = ell_place.transform_axis2_placement(rotate_n_move);

        // update ellipse placement
        self.position = tmp;

        // update ellipse size
        self.radius_x *= d_scale;
        self.radius_y *= d_scale;

        // apply transformation to underlying Nurb, which notifies the polar converter
        if self.base.nurb.is_some() {
            self.base
                .transform(rotate_n_move, scale)
                .map_err(|_| SolidError::sm_err(OP_TRANSFORM))?;
            self.notify_post_edit(k)?;
        }
        Ok(())
    }
}

/// `SmPolarConversion::SetUpPolarConversion(cpQuadCircle, .., ..)` of `pc` for the
/// ellipse `e`: the ellipse's own table (taken out for the call), or the table of a
/// surface of revolution that owns the circle
/// ([`crate::surface::surf_of_revolution::SurfOfRevolution`]).
pub(crate) fn set_up_polar_conversion(
    pc: &mut PolarConversion,
    e: &mut Ellipse,
    k: &Kernel,
) -> Result<()> {
    // init state
    pc.possible = false;

    // store the curve
    pc.has_curve = true;

    // no work - not a degree 2 curve (GetDegree is always 2)

    // get curve knots (a NURB-less ellipse would make its NURB here, re-entering)
    if e.base.nurb.is_none() {
        return Err(SolidError::sm_err(OP_SET_UP));
    }
    let (knots, multiplicities) = e.base.knots(None)?;
    pc.natural_knots = knots;

    // no work - start/end knots are not full multiplicities
    if multiplicities.first().copied() != Some(3) || multiplicities.last().copied() != Some(3) {
        return Err(SolidError::sm_err(OP_SET_UP));
    }

    let cnt = pc.natural_knots.len();

    // no work - internal knots not double - no conversion possible
    for m in multiplicities.iter().take(cnt.saturating_sub(1)).skip(1) {
        if *m != 2 {
            return Ok(());
        }
    }

    // locals
    let inside_out = e.inside_out();
    let mut span_start_angle;
    let mut span_end_angle = 0.0;

    // We have to make sure that the interval contains all of the angles that the
    // Nurbs curve requires. If it's already a full circle, then there's no problem.
    let mut anal_ivl = e.step_interval();

    if anal_ivl.length() < 360.0 - EFF_ZERO {
        let idx = if inside_out { cnt.saturating_sub(1) } else { 0 };
        let lowest_angle_param = table(&pc.natural_knots, idx, OP_SET_UP)?;
        let idx = if inside_out { 0 } else { cnt.saturating_sub(1) };
        let highest_angle_param = table(&pc.natural_knots, idx, OP_SET_UP)?;

        // Temporarily set the curve's domain to a full circle, so that STEPInversion()
        // won't bump into the limits.
        e.set_step_interval(Extent1d::from_min_max(0.0, 360.0));

        // Get Polar angle about ZAxis from XAxis in degrees.
        let pnt1 = e.base.evaluate_point(lowest_angle_param)?;
        let (mut lowest_deg, _) = e.step_inversion(k, pnt1, false, 0.0);

        let pnt2 = e.base.evaluate_point(highest_angle_param)?;
        let (mut highest_deg, _) = e.step_inversion(k, pnt2, false, 0.0);

        // STEPInversion will snap dHighestDeg to Min param according to tolerance.
        // Only account for noise here. [B630]
        if highest_deg <= lowest_deg + EFF_ZERO {
            highest_deg += 360.0; // closed curve.
        }

        // Move to period of greatest overlap with input sAnalIvl.
        let input_mid = anal_ivl.mid();
        let mut this_mid = (lowest_deg + highest_deg) / 2.0;

        // Upstream loops without a bound; a shift that no longer changes the midpoint
        // (beyond about 2^61 degrees) would never end.
        let stuck = || SolidError::NotConverged {
            operation: OP_SET_UP,
            iterations: u32::try_from(MAX_PERIOD_SHIFTS).unwrap_or(u32::MAX),
        };
        let mut shifts = 0usize;
        while this_mid > input_mid + 180.0 + EFF_ZERO {
            let next_mid = this_mid - 360.0;
            shifts += 1;
            if next_mid == this_mid || shifts > MAX_PERIOD_SHIFTS {
                e.set_step_interval(anal_ivl);
                return Err(stuck());
            }
            lowest_deg -= 360.0;
            highest_deg -= 360.0;
            this_mid = next_mid;
        }
        while this_mid < input_mid - 180.0 - EFF_ZERO {
            let next_mid = this_mid + 360.0;
            shifts += 1;
            if next_mid == this_mid || shifts > MAX_PERIOD_SHIFTS {
                e.set_step_interval(anal_ivl);
                return Err(stuck());
            }
            lowest_deg += 360.0;
            highest_deg += 360.0;
            this_mid = next_mid;
        }

        // watch out for tolerance creep
        let low_change_deg = lowest_deg - anal_ivl.min();
        if low_change_deg < EFF_ZERO * 1000.0 && low_change_deg > -EFF_ZERO * 1000.0 {
            lowest_deg = anal_ivl.min();
        }

        let high_change_deg = highest_deg - anal_ivl.max();
        if high_change_deg < EFF_ZERO * 1000.00 && high_change_deg > -EFF_ZERO * 1000.0 {
            highest_deg = anal_ivl.max();
        }

        // (SetMinMax's status is ignored upstream)
        let _ = anal_ivl.set_min_max(lowest_deg, highest_deg);
        e.set_step_interval(anal_ivl);
    }

    // Tolerance is around 6 decimal places of accuracy (unused for an ellipse: the
    // placement and radius come from the curve).
    let nurb_ivl = e.base.natural_interval();
    let _pnt1 = e.base.evaluate_point(nurb_ivl.min())?;
    let _pnt2 = e.base.evaluate_point(nurb_ivl.evaluate(0.4567))?;

    // get curve control points and weights
    let (control_polygon, weights) = e.base.control_polygon()?;

    // UPSTREAM QUIRK: for a non-rational NURB, SmBSplineCurve::GetControlPolygon sizes the
    // weights array and then resets it, which zeroes the buffer and sets its size to 0;
    // the loop below still reads the buffer, so it sees weights of 0.0 (a constant span,
    // an exact conversion).
    let weights = if weights.is_empty() {
        vec![0.0; control_polygon.len()]
    } else {
        weights
    };

    // Clear cached array values
    pc.angles.clear();
    pc.exact_conversion.clear();

    let angle_tol = EFF_ZERO_DEG;

    // for every span boundary
    for ii in 0..cnt {
        // get Polar angle about ZAxis from XAxis in degrees watching out for periodicity
        let pnt2 = e
            .base
            .evaluate_point(table(&pc.natural_knots, ii, OP_SET_UP)?)?;
        let (mut angle_deg, _) = e.step_inversion(k, pnt2, false, 0.0);

        // when knots match up with Interval endPoints - use the sAnalIvl end value when appropriate
        let mut end_pt = false;
        if ii == 0
            && ((angle_deg - anal_ivl.min()).abs() < angle_tol
                || (angle_deg - anal_ivl.max()).abs() < angle_tol)
        {
            // bInsideOut: the Nurb and Step parameterizations are switched
            angle_deg = if inside_out {
                anal_ivl.max()
            } else {
                anal_ivl.min()
            };
            end_pt = true;
        }
        if ii + 1 == cnt
            && ((angle_deg - anal_ivl.max()).abs() < angle_tol
                || (angle_deg - anal_ivl.min()).abs() < angle_tol)
        {
            angle_deg = if inside_out {
                anal_ivl.min()
            } else {
                anal_ivl.max()
            };
            end_pt = true;
        }

        if !end_pt {
            // snap values to -360, 0, and 360
            if (angle_deg + 360.0).abs() < angle_tol {
                angle_deg = -360.0;
            } else if angle_deg.abs() < angle_tol {
                angle_deg = 0.0;
            } else if (angle_deg - 360.0).abs() < angle_tol {
                angle_deg = 360.0;
            }
        }

        // save the angle value in the angle array
        span_start_angle = span_end_angle;
        span_end_angle = angle_deg;
        pc.angles.push(angle_deg);

        // skip interval classification for 1st boundary
        if ii == 0 {
            continue;
        }

        // set the exactConversion status for each interval (not Boundary)
        // to TRUE when weights are constant - FALSE otherwise
        let weight_a = table(&weights, ii * 2 - 2, OP_SET_UP)?;
        let weight_b = table(&weights, ii * 2 - 1, OP_SET_UP)?;
        let weight_c = table(&weights, ii * 2, OP_SET_UP)?;
        let cos_half_ang = ((span_end_angle - span_start_angle) / 2.0 * PI / 180.0).cos();
        pc.exact_conversion
            .push(are_same(weight_a, weight_c) && are_same(weight_b / weight_c, cos_half_ang));
    }

    // quit - something went wrong
    if pc.angles.len() != pc.natural_knots.len() || pc.exact_conversion.len() + 1 != pc.angles.len()
    {
        return Err(SolidError::sm_err(OP_SET_UP));
    }

    // Reset the curve's analytic interval to what we've calculated.
    anal_ivl.init();
    anal_ivl.add_value(table(&pc.angles, 0, OP_SET_UP)?);
    anal_ivl.add_value(table(&pc.angles, cnt.saturating_sub(1), OP_SET_UP)?);
    e.set_step_interval(anal_ivl);

    // all done - conversion tables are set up
    pc.possible = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel() -> Kernel {
        Kernel::default()
    }

    fn circle(k: &Kernel, d0: f64, d1: f64, r: f64, inside_out: bool) -> Ellipse {
        let opts = EllipseOptions {
            inside_out,
            ..EllipseOptions::default()
        };
        Ellipse::new(
            k,
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::X,
            Vec3::Y,
            Extent1d::from_min_max(d0, d1),
            r,
            r,
            &opts,
        )
        .expect("circle")
    }

    #[test]
    fn full_circle_has_polar_tables() {
        let k = kernel();
        let c = circle(&k, 0.0, 360.0, 2.0, false);
        assert!(c.base().gw_nurb().is_some());
        let pc = c.polar_converter();
        assert!(pc.is_polar_conversion_possible());
        assert_eq!(pc.angles().len(), pc.natural_knots().len());
        assert_eq!(pc.exact_conversion().len() + 1, pc.angles().len());
        assert_eq!(pc.angles().first().copied(), Some(0.0));
        assert_eq!(pc.angles().last().copied(), Some(360.0));
        assert!(pc.exact_conversion().iter().all(|&b| b));
        assert!(c.is_circle());
        assert_eq!(c.base().form(), BSplineCurveForm::EllipticArc);
    }

    #[test]
    fn step_and_nurb_parameters_agree() {
        let k = kernel();
        for io in [false, true] {
            let c = circle(&k, -30.0, 200.0, 1.5, io);
            for a in [-30.0, 0.0, 45.0, 90.0, 133.3, 200.0] {
                let t = c.convert_t_from_step_to_nurbs(&k, a).expect("convert");
                let p = c.base().evaluate_point(t).expect("eval");
                let q = c.evaluate_step_point(a).expect("step");
                assert!(p.distance(q) < 1e-9, "{a} {io}: {p:?} {q:?}");
            }
        }
    }

    #[test]
    fn polar_set_up_bounds_period_shifts() {
        let k = kernel();
        // A quarter circle whose STEP interval is moved far away after its NURB is
        // made, as the cone's iso-curves are.
        let arc = |start: f64| {
            let mut e = Ellipse::new_circle(
                &k,
                Vec3::ZERO,
                Vec3::X,
                Vec3::Y,
                Extent1d::from_min_max(0.0, 90.0),
                1.0,
                &EllipseOptions::default(),
            )
            .expect("circle");
            e.set_step_interval(Extent1d::from_min_max(start, start + 90.0));
            e
        };
        // Far but reachable: the angles shift into place as upstream.
        let mut e = arc(1e8);
        e.set_up_polar_conversion(&k).expect("set-up");
        assert!(e.polar.is_polar_conversion_possible());
        assert!((e.step_interval().mid() - (1e8 + 45.0)).abs() <= 180.0);
        // The largest angles that shift within the bound.
        let edge = 360.0 * (MAX_PERIOD_SHIFTS - 3) as f64;
        for (lo, hi) in [(edge - 90.0, edge), (-edge, 90.0 - edge)] {
            let mut e = arc(lo);
            e.set_step_interval(Extent1d::from_min_max(lo, hi));
            e.set_up_polar_conversion(&k)
                .expect("largest angles that shift within MAX_PERIOD_SHIFTS");
            assert!(e.polar.is_polar_conversion_possible());
        }
        // Upstream never ends: adding a turn no longer changes the midpoint.
        // Representable shifts beyond the bound stop as well.
        for start in [
            9.223_372_036_854_776e18,
            -9.223_372_036_854_776e18,
            1e12,
            -1e12,
        ] {
            let mut e = arc(start);
            let original = e.step_interval();
            for _ in 0..2 {
                let err = e.set_up_polar_conversion(&k).unwrap_err();
                assert!(matches!(err, SolidError::NotConverged { .. }), "{err:?}");
                assert!(!e.polar.is_polar_conversion_possible());
                assert_eq!(e.step_interval().min(), original.min());
                assert_eq!(e.step_interval().max(), original.max());
            }
        }
    }

    #[test]
    fn step_inversion_round_trips() {
        let k = kernel();
        let e = Ellipse::new(
            &k,
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Extent1d::from_min_max(0.0, 360.0),
            3.0,
            1.0,
            &EllipseOptions::default(),
        )
        .expect("ellipse");
        assert!(!e.is_circle());
        for a in [10.0, 100.0, 250.0] {
            let p = e.evaluate_step_point(a).expect("p");
            let (b, loc) = e.step_inversion(&k, p, true, 0.0);
            assert!((a - b).abs() < 1e-9);
            assert_eq!(loc, Some(CurveLocationType::Interior));
        }
        // the seam returns the domain start
        let p = e.evaluate_step_point(360.0).expect("p");
        let (b, loc) = e.step_inversion(&k, p, true, 0.0);
        assert_eq!(b, 0.0);
        assert_eq!(loc, Some(CurveLocationType::Seam));
    }

    #[test]
    fn max_analytic_domain_cases() {
        let mut e = Ellipse::empty(3);
        for (d, m) in [
            ((0.0, 360.0), (0.0, 360.0)),
            ((10.0, 20.0), (0.0, 360.0)),
            ((-50.0, -20.0), (-360.0, 0.0)),
            ((-50.0, 20.0), (-50.0, 310.0)),
        ] {
            e.set_step_interval(Extent1d::from_min_max(d.0, d.1));
            let r = e.max_analytic_domain();
            assert_eq!((r.min(), r.max()), m);
        }
    }

    #[test]
    fn drop_point_on_circle() {
        let k = kernel();
        let c = circle(&k, 0.0, 360.0, 2.0, false);
        let ivl = c.base().natural_interval();
        let p = Vec3::new(1.0, 2.0, 3.0) + Vec3::new(0.0, 5.0, 1.0);
        let sols = c
            .drop_point_fast::<i64>(
                &k,
                &ivl,
                SolverOperation::Minimize,
                p,
                None,
                1e-6,
                None,
                SolutionRequested::All,
                Some(7),
            )
            .expect("drop");
        assert_eq!(sols.len(), 1);
        let s = sols.first().expect("sol");
        let q = c.base().evaluate_point(s.start.parameters[0]).expect("q");
        assert!(q.distance(Vec3::new(1.0, 4.0, 3.0)) < 1e-9);
        assert_eq!(s.objects[0], Some(7));

        // a point on the axis gives both ends
        let sols = c
            .drop_point_fast::<i64>(
                &k,
                &ivl,
                SolverOperation::Minimize,
                Vec3::new(1.0, 2.0, 9.0),
                None,
                1e-6,
                None,
                SolutionRequested::All,
                None,
            )
            .expect("drop");
        assert_eq!(sols.len(), 2);

        // ellipses are not supported
        let e = Ellipse::new(
            &k,
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Extent1d::from_min_max(0.0, 360.0),
            3.0,
            1.0,
            &EllipseOptions::default(),
        )
        .expect("ellipse");
        assert!(e
            .drop_point_fast::<i64>(
                &k,
                &ivl,
                SolverOperation::Minimize,
                p,
                None,
                1e-6,
                None,
                SolutionRequested::All,
                None,
            )
            .is_err());
    }

    #[test]
    fn angle_param_conversion() {
        let t = PolarConversion::angle_param_to_nurb_param(0.0, 1.0, 0.0, 90.0, 45.0).expect("t");
        assert!((t - 0.5).abs() < 1e-12);
        assert_eq!(
            PolarConversion::angle_param_to_nurb_param(0.0, 1.0, 0.0, 90.0, 0.0).expect("t"),
            0.0
        );
        assert!(PolarConversion::angle_param_to_nurb_param(0.0, 1.0, 0.0, 90.0, 120.0).is_err());
    }

    #[test]
    fn intersections_early_outs() {
        let k = kernel();
        let a = circle(&k, 0.0, 360.0, 1.0, false);
        let ivl = a.base().natural_interval();
        // same object
        let r = a
            .intersect_with_ellipse::<i64>(&k, &ivl, &a, &ivl, 1e-6, None, None)
            .expect("same");
        assert!(!r.needs_more_intersections);
        // parallel planes far apart
        let b = Ellipse::new(
            &k,
            Vec3::new(1.0, 2.0, 10.0),
            Vec3::X,
            Vec3::Y,
            Extent1d::from_min_max(0.0, 360.0),
            1.0,
            1.0,
            &EllipseOptions::default(),
        )
        .expect("b");
        let r = a
            .intersect_with_ellipse::<i64>(&k, &ivl, &b, &ivl, 1e-6, None, None)
            .expect("far");
        assert!(!r.needs_more_intersections);
        assert!(r.solutions.is_empty());
        // crossing circles: two intersections through the point solver
        let c = Ellipse::new(
            &k,
            Vec3::new(2.0, 2.0, 3.0),
            Vec3::X,
            Vec3::Y,
            Extent1d::from_min_max(0.0, 360.0),
            1.5,
            1.5,
            &EllipseOptions::default(),
        )
        .expect("c");
        let r = a
            .intersect_with_ellipse::<i64>(&k, &ivl, &c, &ivl, 1e-6, None, None)
            .expect("crossing");
        assert_eq!(r.solutions.len(), 2);
    }

    #[test]
    fn transform_moves_and_scales() {
        let k = kernel();
        let mut c = circle(&k, 0.0, 180.0, 1.0, false);
        let mv = Axis2Placement::new(Vec3::new(5.0, 0.0, 0.0), Vec3::X, Vec3::Y);
        c.transform(&k, &mv, Some(Vec3::new(2.0, 2.0, 2.0)))
            .expect("transform");
        assert_eq!(c.x_radius(), 2.0);
        let p = c.evaluate_step_point(0.0).expect("p");
        let t = c.base().natural_interval().min();
        let q = c.base().evaluate_point(t).expect("q");
        assert!(p.distance(q) < 1e-9);
        assert!(c
            .transform(&k, &mv, Some(Vec3::new(1.0, 2.0, 1.0)))
            .is_err());
    }

    #[test]
    fn degenerate_and_types() {
        let k = kernel();
        let c = circle(&k, 0.0, 1e-3, 1.0, false);
        assert!(c.is_degenerate(1e-3));
        assert!(!c.is_degenerate(1e-6));
        assert!(Ellipse::is_kind_of(Ellipse::TYPE));
        assert!(Ellipse::is_kind_of(BSplineCurve::TYPE));
        assert_eq!(c.type_string(), "SmEllipse_TYPE");
        assert_eq!(c.class_string(), "SmEllipse");
        assert_eq!(c.is_domain_boundary(), SurfParamType::Neither);
        assert!(c.approx_eq(&c.clone()));
    }

    #[test]
    fn out_of_bounds_flag_controls_evaluation_without_editing_geometry() {
        let k = kernel();
        let mut c = circle(&k, 15.0, 225.0, 2.0, false);
        let nurb = c.base().gw_nurb().unwrap().clone();
        let interval = c.step_interval();
        let clamped = c.base().evaluate_point(-0.1).unwrap();
        c.set_out_of_bounds_enabled(true);
        let extended = c.base().evaluate_point(-0.1).unwrap();
        assert_ne!(clamped, extended);
        assert_eq!(c.base().gw_nurb(), Some(&nurb));
        assert_eq!(c.step_interval(), interval);
        c.set_out_of_bounds_enabled(false);
        assert_eq!(c.base().evaluate_point(-0.1).unwrap(), clamped);
    }
}
