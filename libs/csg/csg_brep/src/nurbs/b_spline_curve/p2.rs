//! `SmBSplineCurve`, part 2: bounding boxes, joining, point and offset curves,
//! canonical data in and out, line / piecewise-C0 / degeneracy queries, control point
//! access and editing, weight matching, degree elevation, parameter reversal, STEP
//! interval adjustment and reading from a database stream.
//!
//! Upstream: `source/SMLib/src/SmBSplineCurve.cpp`, `source/SMLib/inc/SmBSplineCurve.h`.
//!
//! | Rust                                              | Upstream                               |
//! |---------------------------------------------------|----------------------------------------|
//! | [`BSplineCurve::is_intersector_approx`]           | `IsIntersectorApprox`                  |
//! | [`BSplineCurve::calculate_bounding_box`], [`BoundingBoxRequest`], [`BoundingBoxes`] | `CalculateBoundingBox` |
//! | [`BSplineCurve::is_planar`], [`Planar`]     | `SmCurve::IsPlanar`                    |
//! | [`BSplineCurve::create_analytic_curve`], [`AnalyticCurve`] | `CreateAnalyticCurve`         |
//! | [`BSplineCurve::create_by_joining`]               | `CreateByJoining`                      |
//! | [`BSplineCurve::create_point_curve`]              | `CreatePointCurve`                     |
//! | [`BSplineCurve::create_simple_offset`]            | `CreateSimpleOffset`                   |
//! | [`BSplineCurve::has_repeated_end_control_points`] | `HasRepeatedEndControlPoints`          |
//! | [`BSplineCurve::canonical`], [`CurveCanonical`]   | `GetCanonical`                         |
//! | [`BSplineCurve::is_line`]                         | `IsLine`                               |
//! | [`BSplineCurve::is_piecewise_c0`]                 | `IsPiecewiseC0`                        |
//! | [`BSplineCurve::is_degenerate`]                   | `IsDegenerate`                         |
//! | [`BSplineCurve::control_points_expert`], [`ControlPointForm`] | `GetControlPointsExpert`, `SmControlPointFormType` |
//! | [`BSplineCurve::max_analytic_domain`]             | `GetMaxAnalyticDomain`                 |
//! | [`BSplineCurve::number_of_unique_knots`]          | `GetNumberOfUniqueKnots`               |
//! | [`BSplineCurve::polygon`]                         | `GetPolygon`                           |
//! | [`BSplineCurve::match_weights`]                   | `MatchWeights`                         |
//! | [`BSplineCurve::degree_elevate`]                  | `DegreeElevate`                        |
//! | [`BSplineCurve::reverse_parameterization`]        | `ReverseParameterization`              |
//! | [`BSplineCurve::set_canonical`]                   | `SetCanonical`                         |
//! | [`BSplineCurve::set_control_point`]               | `SetControlPoint`                      |
//! | [`BSplineCurve::adjust_step_interval`]            | `AdjustSTEPInterval`                   |
//! | [`BSplineCurve::read_from_db`]                    | `ReadFromDB`                           |
//! | [`BSplineCurve::write_to_db`]                     | `WriteToDB`                            |
//!
//! The other members of the file belong to `nurbs.b_spline_curve_p1`, `_p3`, ... and
//! `nurbs.b_spline_curve_n_lib`.
//!
//! Deviations:
//! - **Out-params, contexts, notifications.** Outputs are return values (`Option` for
//!   the `SmBoolean` + out-param queries and for a curve upstream may leave `NULL`);
//!   `SmContext` arguments only choose where upstream allocates and are dropped;
//!   `Notify` calls (cache drops and owner notification) have nothing to do here (see
//!   `p1`). `CalculateBoundingBox`'s unused `bExpandBox` flag is dropped and the boxes
//!   asked for come back in [`BoundingBoxes`] (fresh boxes, as a caller passing new
//!   `SmExtent3d` / `SmPseudoBox` / `SmPolarBox` objects gets).
//! - **Missing NURB.** Where upstream dereferences the `NULL` NURB of a curve built
//!   without one (`GetOrCreateGwNurbPointer` cannot make it for a plain B-spline), the
//!   port returns `SM_ERR` ([`SolidError::Upstream`]). That includes
//!   `CalculateBoundingBox`'s sampling fallback for derived types, whose `Evaluate`
//!   calls have no NURB to evaluate either.
//! - **Analytics.** `CreateAnalyticCurve` (with `USE_ANALYTICS`, defined in
//!   `SmConfig.h`) builds an `SmLine` ([`crate::curve::line::Line`]) or an
//!   `SmCircle`; `SmCircle` is not ported, so building one returns
//!   [`SolidError::Unported`].
//! - **Derived types.** `SetControlPoint`'s `IsKindOf(SmEllipse_TYPE)` only logs a
//!   message and is false for a plain B-spline; it is omitted (as `EditEndPoint`'s in
//!   `p1`). `IsDegenerate` asks `IsKindOf(SmCrvOnSurf_TYPE)` through
//!   [`BSplineCurve::is_kind_of`] (false for a plain B-spline).
//! - **Input checks.** Where upstream would read or write outside its arrays the port
//!   returns `InvalidInput`: `CreateByJoining` with no segments or fewer senses than
//!   segments, `GetControlPointsExpert` for a dimension below 2 (it writes two values
//!   per point whatever the stride), knot or span indices outside the curve. A
//!   `ReadFromDB` knot type / form outside its enum is `InvalidInput` (upstream:
//!   undefined values); counts are bounded by the bytes left (see `plan/findings.md`,
//!   "Header-declared sizes"). An unreadable header, degree or size upstream rejects
//!   itself (`SM_ERR_INVALID_INPUT`), as the port does.
//! - **`GetControlPointsExpert`** writes into the caller's array and leaves the slots it
//!   does not write alone; the port returns a vector of `stride * count` values whose
//!   unwritten slots are `0`; that vector is allocated fallibly (`InvalidInput`
//!   "output too large" for a stride or count whose output does not fit in memory).
//! - **`ReadFromDB`** always builds a new curve (the "fill this empty object" input of
//!   `rpNewCurve` is dropped, as is its unused type argument).
//! - **`ReverseParameterization`** sets the new interval with `SetMinMax`, whose
//!   failure (new min above new max, only for an inverted natural interval) upstream
//!   ignores, leaving the caller's interval; the port then returns the old interval.
//! - **Back-edges.** `IsLine` defers to `SmCurve::IsLine` (unit `curve.curve_p5`,
//!   co-ported by `curve.curve_cache` as [`crate::curve::curve::p5::is_line_base`])
//!   for rational curves and curves it cannot classify from the control points;
//!   `IsDegenerate` calls `SmCurve::IsDegenerate`
//!   ([`crate::curve::curve::p4::is_degenerate_base`]) for other intervals;
//!   `GetMaxAnalyticDomain` calls `SmCurve::IsClosed`
//!   ([`crate::curve::curve::p4::is_closed_base`]); `CreateAnalyticCurve` builds
//!   `SmCircle` (`curve.ellipse_p1`) and calls `SmCurve::IsArc`
//!   (`curve.curve_p5`, [`crate::curve::curve::p5::is_arc_base`]); `CreateSimpleOffset`
//!   calls `SmCurve::IsArc` too. It approximates an [`OffsetCurve`] with
//!   `SmCurve::ApproximateCurve` ([`Curve::approximate_curve`]), passing the
//!   caller's `bOptMatchParameterization` as `bOptCreateAnalytics` and `FALSE` for
//!   matching, as upstream.

use crate::curve::curve::{Curve, CurveKind};
use crate::curve::ellipse::{Ellipse, EllipseOptions};
use crate::curve::line::{BoundedLineOptions, Line};
use crate::curve::offset_curve::OffsetCurve;
use crate::error::{ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::triangle_area_squared;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::polar_box::PolarBox;
use crate::geom::pseudo_box::{Planar, PseudoBox};
use crate::geom::tol::{are_same, is_zero, is_zero_to_tol, ANG_TOL_DEG, ZONE_TOL_3D};
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::p1::{
    split_cubic_bezier, split_quadratic_bezier, BSplineCurve, BSplineCurveForm, CanonicalData,
    KnotType, NurbCircleParam,
};
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::geom::dist_cpt_cpt;
use crate::nurbs::knots::{Side, MAX_DEGREE};
use crate::nurbs::math::{CPoint, NOW, NOZ};
use crate::nurbs::nurbs_crv::{allocate_nurb_curve, is_ncrv_degenerate, to_euclid};
use crate::topology::database_io::{DatabaseReader, DatabaseWriter, FileType, InputStream};

/// `SmCrvOnSurf_TYPE` (`CURV_BASE_TYPE + 48`).
const CRV_ON_SURF_TYPE: i64 = BSplineCurve::TYPE - 33 + 48;

const OP_BBOX: &str = "nurbs::b_spline_curve::p2::BSplineCurve::calculate_bounding_box";
const OP_ANALYTIC: &str = "nurbs::b_spline_curve::p2::BSplineCurve::create_analytic_curve";
const OP_REPEATED: &str =
    "nurbs::b_spline_curve::p2::BSplineCurve::has_repeated_end_control_points";
const OP_IS_LINE: &str = "nurbs::b_spline_curve::p2::BSplineCurve::is_line";
const OP_UNIQUE: &str = "nurbs::b_spline_curve::p2::BSplineCurve::number_of_unique_knots";
const OP_MATCH: &str = "nurbs::b_spline_curve::p2::BSplineCurve::match_weights";
const OP_JOIN: &str = "nurbs::b_spline_curve::p2::BSplineCurve::create_by_joining";
const OP_OFFSET: &str = "nurbs::b_spline_curve::p2::BSplineCurve::create_simple_offset";
const OP_C0: &str = "nurbs::b_spline_curve::p2::BSplineCurve::is_piecewise_c0";
const OP_EXPERT: &str = "nurbs::b_spline_curve::p2::BSplineCurve::control_points_expert";
const OP_POLYGON: &str = "nurbs::b_spline_curve::p2::BSplineCurve::polygon";
const OP_ELEVATE: &str = "nurbs::b_spline_curve::p2::BSplineCurve::degree_elevate";
const OP_REVERSE: &str = "nurbs::b_spline_curve::p2::BSplineCurve::reverse_parameterization";
const OP_SET_CP: &str = "nurbs::b_spline_curve::p2::BSplineCurve::set_control_point";
const OP_READ: &str = "nurbs::b_spline_curve::p2::BSplineCurve::read_from_db";
/// `ReadFromDB`'s message when the stream fails part way (`ReadFailed`).
const READ_FAILED: &str = "NURBS curve data ends early or holds text that is not a number";

/// `SmControlPointFormType`: how control points are passed in or out; discriminants
/// are the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlPointForm {
    /// `SM_CP_NON_RATIONAL`: Euclidean coordinates, no weight.
    NonRational = 0,
    /// `SM_CP_HOMOGENEOUS_RATIONAL`: homogeneous coordinates (`w x, w y, w z`) and
    /// weight.
    HomogeneousRational = 1,
    /// `SM_CP_EUCLIDIAN_RATIONAL`: Euclidean coordinates and weight.
    EuclideanRational = 2,
}

/// The boxes [`BSplineCurve::calculate_bounding_box`] computes (its non-`NULL`
/// output pointers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BoundingBoxRequest {
    /// The axis aligned box (`pNormalBox`).
    pub normal: bool,
    /// The non-axis aligned box (`pPseudoBox`).
    pub pseudo: bool,
    /// The box of the control polygon leg directions (`pPolarBox`).
    pub polar: bool,
}

/// Boxes of [`BSplineCurve::calculate_bounding_box`]; `None` where not asked for.
#[derive(Debug, Clone, Default)]
pub struct BoundingBoxes {
    /// Axis aligned box.
    pub normal: Option<Extent3d>,
    /// Non-axis aligned box.
    pub pseudo: Option<PseudoBox>,
    /// Direction box.
    pub polar: Option<PolarBox>,
}

/// The STEP canonical data of a curve ([`BSplineCurve::canonical`]).
#[derive(Debug, Clone, PartialEq)]
pub struct CurveCanonical {
    /// Image-space dimension.
    pub dim: u64,
    /// Degree.
    pub degree: usize,
    /// Euclidean control points.
    pub control_points: Vec<Point3>,
    /// STEP form.
    pub form: BSplineCurveForm,
    /// Multiplicity of each unique knot.
    pub knot_multiplicities: Vec<usize>,
    /// Unique knots.
    pub knots: Vec<f64>,
    /// STEP knot type.
    pub knot_type: KnotType,
    /// Weights (empty for a non-rational curve).
    pub weights: Vec<f64>,
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

// ---- back-edges -------------------------------------------------------------------

/// `SmCurve::IsLine` of `curve`: the line point and vector when the curve is a
/// constant-speed line (co-ported by unit `curve.curve_cache` into
/// [`crate::curve::curve::p5`]).
fn curve_is_line(
    curve: &BSplineCurve,
    num_samples: u64,
    tol: f64,
) -> Result<Option<(Point3, Vec3)>> {
    crate::curve::curve::p5::is_line_base(curve, num_samples, tol)
}

/// `SmCurve::IsDegenerate` of `curve` over `interval`.
fn curve_is_degenerate(
    k: &Kernel,
    curve: &BSplineCurve,
    tol: f64,
    interval: Option<&Extent1d>,
) -> Result<bool> {
    crate::curve::curve::p4::is_degenerate_base(k, curve, tol, interval)
}

/// `SmCurve::IsClosed(crInterval, dTolerance = 0.0)` of `curve`.
fn curve_is_closed(k: &Kernel, curve: &BSplineCurve, interval: &Extent1d) -> Result<bool> {
    crate::curve::curve::p4::is_closed_base(k, curve, interval, 0.0)
}

/// A circular arc found by `SmCurve::IsArc`: frame, radius, start and end angle
/// (degrees).
pub(crate) type ArcData = (Axis2Placement, f64, f64, f64);

/// A virtual `IsArc(lNumberOfSamplePoints, dTol, ..)` of a class derived from
/// `SmBSplineCurve`.
pub(crate) type IsArcFn<'a> = dyn Fn(&BSplineCurve, u64, f64) -> Result<Option<ArcData>> + 'a;

/// `SmCurve::IsArc(lNumberOfSamplePoints, dTol, ...)` of `curve` (its virtual
/// `IsDegenerate` is [`BSplineCurve::is_degenerate`]).
fn curve_is_arc(
    k: &Kernel,
    curve: &BSplineCurve,
    num_samples: u64,
    tol: f64,
) -> Result<Option<ArcData>> {
    crate::curve::curve::p5::is_arc_base(
        k,
        curve,
        |c, tol| c.is_degenerate(k, tol, None),
        num_samples,
        tol,
    )
}

/// `SmOffsetCurve(3, curve, ivl, normal, dist)` approximated with
/// `ApproximateCurve(ctx, {ivl.min, ivl.max}, tol, gap, approx, bMatch, FALSE)`: the
/// approximation and its maximum gap.
fn offset_curve_approximation(
    k: &Kernel,
    curve: &BSplineCurve,
    interval: &Extent1d,
    normal: Vec3,
    distance: f64,
    approx_tol_3d: f64,
    match_parameterization: bool,
) -> Result<(BSplineCurve, f64)> {
    let off = OffsetCurve::new(
        3,
        Curve::from(curve.clone()),
        *interval,
        normal,
        distance,
        false,
    )?;
    let breaks = [interval.min(), interval.max()];
    approximate_curve(
        k,
        &Curve::from(off),
        &breaks,
        approx_tol_3d,
        match_parameterization,
    )
}

/// `SmCurve::ApproximateCurve(ctx, breaks, tol, gap, approx, bMatch, FALSE)` of
/// `curve`: the approximation and its maximum gap. Upstream passes the caller's
/// `bOptMatchParameterization` in the `bOptCreateAnalytics` slot and `FALSE` for
/// matching the parameterization; kept.
fn approximate_curve(
    k: &Kernel,
    curve: &Curve,
    breaks: &[f64],
    approx_tol_3d: f64,
    match_parameterization: bool,
) -> Result<(BSplineCurve, f64)> {
    const OP: &str = "nurbs::b_spline_curve::p2::approximate_curve";
    let opts = crate::curve::curve::p1::ApproximateCurveOptions {
        create_analytics: match_parameterization,
        match_parameterization: false,
        just_copy_bsplines: false,
    };
    let r = curve.approximate_curve(k, breaks, approx_tol_3d, &opts)?;
    // (a degenerate offset leaves upstream's dMaxGap3d uninitialized)
    let gap = r.achieved_tol.ok_or(SolidError::InvalidInput {
        operation: OP,
        reason: "maximum gap never written (degenerate offset curve)",
    })?;
    match r.curve.into_kind() {
        CurveKind::BSpline(c) => Ok((c, gap)),
        _ => Err(SolidError::InvalidInput {
            operation: OP,
            reason: "approximation is not a B-spline",
        }),
    }
}

/// An analytic curve [`BSplineCurve::create_analytic_curve`] makes (`SmLine`,
/// `SmCircle`).
#[derive(Debug, Clone)]
pub enum AnalyticCurve {
    /// An `SmLine`.
    Line(Line),
    /// An `SmCircle`.
    Circle(Box<Ellipse>),
}

impl AnalyticCurve {
    /// The point at NURB parameter `t` (`EvaluatePoint` of the analytic curve).
    fn evaluate_point(&self, t: f64) -> Result<Point3> {
        match self {
            Self::Line(l) => Ok(l.evaluate_point(t)),
            Self::Circle(c) => c.base().evaluate_point(t),
        }
    }
}

/// `new SmLine(origin, vector, interval, scale, dim)`.
fn make_line(
    origin: Point3,
    vector: Vec3,
    interval: &Extent1d,
    scale: f64,
    dim: u64,
) -> AnalyticCurve {
    AnalyticCurve::Line(Line::bounded(
        origin,
        vector,
        interval,
        &BoundedLineOptions {
            scale,
            dim,
            ..BoundedLineOptions::default()
        },
    ))
}

/// `new SmCircle(origin, x, y, angle_ivl, radius, dim, ctx, curve)`.
fn make_circle(
    k: &Kernel,
    frame: &Axis2Placement,
    angle_ivl: &Extent1d,
    radius: f64,
    dim: u64,
    curve: &BSplineCurve,
) -> Result<AnalyticCurve> {
    let c = Ellipse::new_circle(
        k,
        frame.origin(),
        frame.x_axis(),
        frame.y_axis(),
        *angle_ivl,
        radius,
        &EllipseOptions {
            dim,
            nurb: Some(curve),
            inside_out: false,
        },
    )?;
    Ok(AnalyticCurve::Circle(Box::new(c)))
}

impl ControlPointForm {
    /// The form with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::NonRational),
            1 => Some(Self::HomogeneousRational),
            2 => Some(Self::EuclideanRational),
            _ => None,
        }
    }
}

/// `SmKnotType` with upstream value `code`, if any.
fn knot_type_from_code(code: u64) -> Option<KnotType> {
    Some(match code {
        0 => KnotType::UniformKnots,
        1 => KnotType::Unspecified,
        2 => KnotType::QuasiUniformKnots,
        3 => KnotType::PiecewiseBezierKnots,
        4 => KnotType::Unknown,
        _ => return None,
    })
}

/// `SmBSplineCurveForm` with upstream value `code`, if any.
fn form_from_code(code: u64) -> Option<BSplineCurveForm> {
    Some(match code {
        0 => BSplineCurveForm::PolylineForm,
        1 => BSplineCurveForm::CircularArc,
        2 => BSplineCurveForm::EllipticArc,
        3 => BSplineCurveForm::ParabolicArc,
        4 => BSplineCurveForm::HyperbolicArc,
        5 => BSplineCurveForm::HelicalArc,
        6 => BSplineCurveForm::Unspecified,
        _ => return None,
    })
}

impl BSplineCurve {
    /// Whether the curve has the form the general intersector makes: degree 3 and
    /// piecewise C0 (`IsIntersectorApprox`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn is_intersector_approx(&self) -> Result<bool> {
        Ok(self.degree()? == 3 && self.is_piecewise_c0()?)
    }

    /// Bounding boxes of the part of the curve over `interval` (clamped to the natural
    /// interval), built from the control points of the spans it covers
    /// (`CalculateBoundingBox`). The pseudo box takes its basis from the control point
    /// triple of largest triangle area among up to 7 sampled points; the polar box
    /// holds the control polygon legs. A single Bezier span that is a line gives the
    /// boxes of its end points (evaluated at the ends of `interval`); a quadratic or
    /// cubic Bezier span is split once for tighter boxes.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB or when a span cannot be found.
    pub fn calculate_bounding_box(
        &self,
        interval: &Extent1d,
        req: BoundingBoxRequest,
    ) -> Result<BoundingBoxes> {
        self.calculate_bounding_box_onto(interval, req, PseudoBox::new())
    }

    /// `IsPlanar` (`SmCurve::IsPlanar` on a B-spline curve): [`PseudoBox::is_planar`] of
    /// the curve's pseudo box over its natural interval, with the plane point, the
    /// (not unitised) plane normal and the deviation (`tol == EFF_ZERO` lets the box pick
    /// its scaled zero).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from [`BSplineCurve::calculate_bounding_box`] (whose
    /// other failures leave the default box, as upstream ignores the status).
    pub fn is_planar(&self, tol: f64) -> Result<Planar> {
        let req = BoundingBoxRequest {
            normal: false,
            pseudo: true,
            polar: false,
        };
        let pseudo = ignored(self.calculate_bounding_box(&self.natural_interval(), req))?
            .and_then(|b| b.pseudo)
            .unwrap_or_else(PseudoBox::new);
        Ok(pseudo.is_planar(tol))
    }

    /// [`BSplineCurve::calculate_bounding_box`] onto the caller's pseudo box (upstream's
    /// in/out `pPseudoBox`): its intervals are reset and its basis is kept unless the
    /// control points set one, which a failed `SetBasis` does not (a degenerate span
    /// whose squared extent overflows).
    ///
    /// # Errors
    ///
    /// As [`BSplineCurve::calculate_bounding_box`].
    pub fn calculate_bounding_box_onto(
        &self,
        interval: &Extent1d,
        req: BoundingBoxRequest,
        initial_pseudo: PseudoBox,
    ) -> Result<BoundingBoxes> {
        let mut out = BoundingBoxes::default();

        // no work - nothing to compute
        if !req.normal && !req.pseudo && !req.polar {
            return Ok(out);
        }

        // locals
        let natural_interval = self.natural_interval();
        let cur = self.nurb_ref()?;
        let pw = cur.cpts();
        let p = cur.degree();
        let n = cur.n();

        // Build target interval by trimming input interval to natural interval
        let target = Extent1d::from_min_max(
            natural_interval.clamp_value(interval.min()),
            natural_interval.clamp_value(interval.max()),
        );

        // get the startPoint and the numberPoints of the control polygon
        // that contribute to the target interval's shape
        let (start_point, mut num_points) =
            if natural_interval.are_equal(&target, EFF_ZERO) || n == 0 {
                (0, n + 1)
            } else {
                // use control polygon which contributes to the included spans
                let knt = cur.knot_vector().map_err(SolidError::nl_ser(OP_BBOX))?;
                let start_span = knt
                    .find_span(p, target.min(), Side::Left)
                    .map_err(SolidError::nl_ser(OP_BBOX))?;
                let end_span = knt
                    .find_span(p, target.max(), Side::Right)
                    .map_err(SolidError::nl_ser(OP_BBOX))?;
                let start = start_span
                    .checked_sub(p)
                    .ok_or_else(|| invalid(OP_BBOX, "span below the degree"))?;
                let num = end_span
                    .checked_add(1)
                    .and_then(|e| e.checked_sub(start))
                    .ok_or_else(|| invalid(OP_BBOX, "end span before start span"))?;
                (start, num)
            };

        if num_points < 1 {
            return Err(SolidError::sm_err(OP_BBOX));
        }

        let cpt = |i: usize| {
            pw.get(i)
                .copied()
                .ok_or_else(|| invalid(OP_BBOX, "control point index out of range"))
        };

        // init PseudoBox: Compute the basis vectors of the PseudoBox using the control
        // polygon running from the start span to the end span
        if req.pseudo {
            // init intervals - leave basis vectors alone
            let mut pseudo = initial_pseudo;
            pseudo.init();

            // locals to find control point triple with a near-max cross product
            let (mut i_max, mut j_max, mut k_max) = (1usize, 2usize, 3usize);
            let mut max_cross = 0.0;
            let max_smp = 7usize; // max number of control points to check
            let smp_cnt = num_points.min(max_smp); // actual number of control points to check
            let inc = num_points / smp_cnt;

            // look for a good non-colinear set of control points
            match num_points {
                0 | 1 => (i_max, j_max, k_max) = (0, 0, 0),
                2 => (i_max, j_max, k_max) = (0, 0, 1),
                3 => (i_max, j_max, k_max) = (0, 1, 2),
                _ => {
                    for ii in 0..smp_cnt - 2 {
                        let p0 = to_euclid(cpt(ii * inc)?);
                        for jj in ii + 1..smp_cnt - 1 {
                            let p1 = to_euclid(cpt(jj * inc)?);
                            for kk in jj + 1..smp_cnt {
                                let p2 = to_euclid(cpt(kk * inc)?);
                                let this_cross = triangle_area_squared(p0, p1, p2);
                                if this_cross > max_cross {
                                    max_cross = this_cross;
                                    i_max = ii;
                                    j_max = jj;
                                    k_max = kk;
                                }
                            }
                        }
                    }
                }
            }

            let p0 = to_euclid(cpt(i_max * inc)?);
            let p1 = to_euclid(cpt(j_max * inc)?);
            let p2 = to_euclid(cpt(k_max * inc)?);

            let mut v0 = p1 - p0;
            let mut v1 = p2 - p0;
            let mut v0_len_sq = v0.length_squared();
            let v1_len_sq = v1.length_squared();

            // set sV0 larger than sV1
            if v1_len_sq > v0_len_sq {
                core::mem::swap(&mut v0, &mut v1);
                v0_len_sq = v1_len_sq;
            }

            // Note that we will just use the standard basis vectors for degenerate curves
            if v0_len_sq < EFF_ZERO_SQ {
                // use default basis vectors
                pseudo = PseudoBox::new();
            } else {
                let [b1, b2, b3] = v0
                    .make_unit_ortho_vectors(Some(v1))
                    .unwrap_or([Vec3::UNDEF; 3]);
                // (SetBasis status ignored upstream)
                let _ = pseudo.set_basis(b1, b2, b3);
            }
            out.pseudo = Some(pseudo);
        }

        // init polarBox
        if req.polar {
            out.polar = Some(PolarBox::new());
        }

        // the control-polygon vertices used: the spans' own, or a refined Bezier polygon
        let mut refined: Option<Vec<CPoint>> = None;

        // if pCur is a bezier curve
        if p == n && p == num_points - 1 {
            let pnt = to_euclid(cpt(start_point)?);

            // we have a Bezier span, if line then recompute from start,end
            let tol = EFF_ZERO * (1.0 + pnt.max_dimension());

            // note: can be a degree 2 or 3 'line'
            if let Some((is_line_pnt, _)) = self.is_line(4, tol)? {
                // compute line's bboxes and return (EvaluatePoint statuses ignored)
                let line_pnt = self.evaluate_point(interval.min()).unwrap_or(is_line_pnt);
                if req.normal {
                    out.normal = Some(Extent3d::from_point(line_pnt));
                }
                if let Some(pb) = out.pseudo.as_mut() {
                    pb.add_point(line_pnt);
                }

                let line_pnt2 = self.evaluate_point(interval.max()).unwrap_or(Vec3::UNDEF);
                if let Some(nb) = out.normal.as_mut() {
                    nb.add_point(line_pnt2);
                }
                if let Some(pb) = out.pseudo.as_mut() {
                    pb.add_point(line_pnt2);
                }
                if let Some(pb) = out.polar.as_mut() {
                    pb.add_vector(line_pnt2 - line_pnt, None);
                }
                return Ok(out);
            }

            if p == 2 {
                // make 5 vertex control-polygon after splitting current 3 point
                // control-polygon, then remove the middle point (the average of points
                // 1 and 3, it adds no information to any bounding box)
                let [a0, a1, _, a3, a4] = split_quadratic_bezier(cpt(0)?, cpt(1)?, cpt(2)?);
                refined = Some(vec![a0, a1, a3, a4]);
                num_points = 4;
            } else if p == 3 {
                // make 7 vertex control-polygon after splitting current 4 point
                // control-polygon, then remove the middle point (the average of points
                // 2 and 4)
                let [a0, a1, a2, _, a4, a5, a6] =
                    split_cubic_bezier(cpt(0)?, cpt(1)?, cpt(2)?, cpt(3)?);
                refined = Some(vec![a0, a1, a2, a4, a5, a6]);
                num_points = 6;
            }
        }

        let poly: &[CPoint] = match &refined {
            Some(r) => r,
            None => pw
                .get(start_point..)
                .ok_or_else(|| invalid(OP_BBOX, "control point index out of range"))?,
        };
        let poly = poly
            .get(..num_points)
            .ok_or_else(|| invalid(OP_BBOX, "control point index out of range"))?;

        // init pBBox with the starting control-polygon vertex
        if req.normal {
            let first = poly
                .first()
                .ok_or_else(|| invalid(OP_BBOX, "empty control polygon"))?;
            out.normal = Some(Extent3d::from_point(to_euclid(*first)));
        }

        // Build the BBoxes by adding in remaining control-polygon vertices
        let mut last_point = Vec3::UNDEF;
        for (i, &cp) in poly.iter().enumerate() {
            let point = to_euclid(cp);
            if let Some(nb) = out.normal.as_mut() {
                nb.add_point(point);
            }
            if let Some(pb) = out.pseudo.as_mut() {
                pb.add_point(point);
            }
            if let Some(pb) = out.polar.as_mut() {
                if i > 0 {
                    pb.add_vector(point - last_point, None);
                }
                last_point = point;
            }
        }

        Ok(out)
    }

    /// An analytic (line or circle) equivalent of the curve over `interval` when the
    /// curve is one to the scaled zero of its point at `0.456789` of the interval
    /// (`CreateAnalyticCurve`; `USE_ANALYTICS` is defined in `SmConfig.h`): a degree 1
    /// curve with two control points that is a line, or a rational quadratic that is a
    /// circular arc (its single interior knots doubled), whose ends over `interval`
    /// match. `None` for other curves.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB; those of the arc test of rational quadratics
    /// (`SmCurve::IsArc`).
    pub fn create_analytic_curve(
        &self,
        k: &Kernel,
        interval: &Extent1d,
    ) -> Result<Option<AnalyticCurve>> {
        // use midPoint to set dScaledZero
        let mid_pnt = self.evaluate_point(interval.evaluate(0.456789))?;
        let scaled_zero = EFF_ZERO * (1.0 + mid_pnt.max_dimension());

        let mut analytic: Option<AnalyticCurve> = None;

        // check for a line bspline (degree = 1; controlPointCount == 2)
        if self.degree()? == 1 && self.number_control_points()? == 2 {
            // the IsLine check will always pass because of the previous definition check
            if let Some((line_pnt, line_vec)) = self.is_line(10, scaled_zero)? {
                // lineVec = EndPoint - StartPoint and if lineVec=0 IsLine=FALSE
                let line_origin = line_pnt - interval.min() * (line_vec / interval.length());
                let line_vector = line_vec / line_vec.length();
                let scale = line_vec.length() / interval.length();
                analytic = Some(make_line(
                    line_origin,
                    line_vector,
                    interval,
                    scale,
                    self.dim,
                ));
            }
        } else if self.degree()? == 2 && self.is_rational()? {
            // check for circular arcs
            if let Some((frame, radius, mut start_angle, mut end_angle)) =
                curve_is_arc(k, self, 20, scaled_zero)?
            {
                // whole curve is circular arc
                let nurb_ivl = self.natural_interval();

                // when given interval is less than natural interval
                //      compute analytic interval for given interval
                if !nurb_ivl.is_contained_by(interval, 0.0) {
                    // evaluate endPoints and endVecs (statuses ignored upstream)
                    let start_point = self.evaluate_point(interval.min())?;
                    let end_point = self.evaluate_point(interval.max())?;
                    let start_vec = start_point - frame.origin();
                    let end_vec = end_point - frame.origin();

                    // get angleRad to start and end Vecs
                    let z_axis = frame.z_axis();
                    let start_ang_rad = z_axis.ccw_angle_between(frame.x_axis(), start_vec)?;
                    let end_ang_rad = z_axis.ccw_angle_between(frame.x_axis(), end_vec)?;

                    // get angleDeg in interval [0 360] for start and end vecs
                    let start_ang_deg = if start_ang_rad < 0.0 {
                        start_ang_rad * 180.0 / PI
                    } else {
                        start_ang_rad * 180.0 / PI + 360.0
                    };
                    let mut end_ang_deg = if end_ang_rad < 0.0 {
                        end_ang_rad * 180.0 / PI
                    } else {
                        end_ang_rad * 180.0 / PI + 360.0
                    };
                    if end_ang_deg < start_ang_deg {
                        end_ang_deg += 360.0;
                    }

                    // set the anal interval bounds
                    start_angle = start_ang_deg;
                    end_angle = end_ang_deg;
                }

                // SMLib rule: circles are represented as degree 2 rational BSplines with
                // all internal knots doubled.
                let mut crv: Option<Self> = None;
                let (knots, multiplicities) = self.knots(None)?;
                for i in 1..multiplicities.len().saturating_sub(1) {
                    // copy the BSplineCurve and make its knots double
                    if multiplicities.get(i).copied() != Some(2) {
                        let c = crv.get_or_insert_with(|| self.copy());
                        let knot = knots
                            .get(i)
                            .copied()
                            .ok_or_else(|| invalid(OP_ANALYTIC, "knot index out of range"))?;
                        // (status ignored upstream)
                        let _ = c.insert_one_knot(knot, 1);
                    }
                }

                // create SmCircle from this SmBSplineCurve
                let angle_ivl = Extent1d::from_min_max(start_angle, end_angle);
                analytic = Some(make_circle(
                    k,
                    &frame,
                    &angle_ivl,
                    radius,
                    self.dim,
                    crv.as_ref().unwrap_or(self),
                )?);
            }
        }

        // when an analytic derived object was created
        let Some(a) = analytic else {
            return Ok(None);
        };

        // if original and analytic given interval begin endpoints are too far apart
        let or_pnt = self.evaluate_point(interval.min())?;
        let an_pnt = a.evaluate_point(interval.min())?;
        if or_pnt.distance(an_pnt) > scaled_zero {
            // don't return the analytic object
            return Ok(None);
        }

        // if original and analytic end given interval endpoints are too far apart
        let or_pnt = self.evaluate_point(interval.max())?;
        let an_pnt = a.evaluate_point(interval.max())?;
        if or_pnt.distance(an_pnt) > scaled_zero {
            // don't return the analytic object
            return Ok(None);
        }

        // get here when orig endpoints are close to analytic endpoints
        Ok(Some(a))
    }

    /// Joins end-to-end connected `segments` into one curve (`CreateByJoining`). The
    /// segments are brought to the highest degree (in place) and, when their
    /// dimensions differ, 3D segments are made 2D (in place). `senses[i] == false`
    /// takes segment `i` reversed. The first segment keeps its parameterisation, the
    /// others are shifted to follow on; end weights of rational segments are matched
    /// by scaling.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for no segments, fewer senses than segments, segments whose
    /// ends are further apart than `1000 * sqrt(SM_EFF_ZERO) * (1 + max|P|)` or of
    /// different dimension or degree; `SM_ERR` without a NURB, for a zero end weight or
    /// when degree elevation or the canonical construction fails.
    pub fn create_by_joining(segments: &mut [Self], senses: Option<&[bool]>) -> Result<Self> {
        let mut ctrl_pts: Vec<Point3> = Vec::new();
        let mut weights: Vec<f64> = Vec::new();
        let mut knot_mult: Vec<usize> = Vec::new();
        let mut knots: Vec<f64> = Vec::new();
        let mut dimension = 0u64;
        let mut degree = 0usize;
        let mut last_weight = 0.0;
        let mut is_rational = false;
        let mut max_degree = 0usize;

        let mut max_dim = 0u64;
        let mut min_dim = 5u64;

        // for every segment - find the max degree, dim, and rationality
        for seg in segments.iter() {
            if seg.is_rational()? {
                is_rational = true;
            }
            if seg.degree()? > max_degree {
                max_degree = seg.degree()?;
            }
            if seg.dim > max_dim {
                max_dim = seg.dim;
            }
            if seg.dim < min_dim {
                min_dim = seg.dim;
            }
        }

        // When curves come from mixed dimensions: Convert 3D curves to 2D
        if max_dim != min_dim {
            for seg in segments.iter_mut() {
                if seg.dim == 3 {
                    // (status ignored upstream)
                    let _ = seg.convert_to_2d();
                }
            }
        }

        // For every curve, add knots and weights to joined curve arrays
        for (ii, seg) in segments.iter_mut().enumerate() {
            // 1st make all curves the same degree
            if seg.degree()? < max_degree {
                seg.degree_elevate(max_degree)?;
            }

            // Get curve canonical reps - watch for reversed curves as needed
            let reversed = match senses {
                Some(s) => !*s
                    .get(ii)
                    .ok_or_else(|| invalid(OP_JOIN, "fewer senses than segments"))?,
                None => false,
            };
            let canon = if reversed {
                let mut bsc = seg.copy();
                let ivl = bsc.natural_interval();
                bsc.reverse_parameterization(&ivl)?;
                bsc.canonical()?
            } else {
                seg.canonical()?
            };
            let CurveCanonical {
                dim: seg_dim,
                degree: seg_deg,
                control_points: mut seg_pts,
                knot_multiplicities: mut seg_knot_mult,
                knots: seg_knots,
                weights: mut seg_weights,
                ..
            } = canon;

            // when needed - build an array of rational weights for this curve
            if is_rational && !seg.is_rational()? {
                // Set the rationality of the segment to TRUE
                seg_weights = vec![1.0; seg_pts.len()];
            }

            let first_of = |v: &[f64]| {
                v.first()
                    .copied()
                    .ok_or_else(|| invalid(OP_JOIN, "empty canonical data"))
            };
            let first_pt = *seg_pts
                .first()
                .ok_or_else(|| invalid(OP_JOIN, "empty canonical data"))?;

            let last_point;
            if ii == 0 {
                // add an extra knot, CtrlPnt, and sSegKNotMult at the beginning
                dimension = seg_dim;
                degree = seg_deg;
                last_point = first_pt;
                if is_rational {
                    last_weight = first_of(&seg_weights)?;
                    weights.push(last_weight);
                }
                ctrl_pts.push(last_point);
                knots.push(first_of(&seg_knots)?);
                knot_mult.push(
                    *seg_knot_mult
                        .first()
                        .ok_or_else(|| invalid(OP_JOIN, "empty canonical data"))?,
                );
            } else {
                // get last CtrlPt and Weight
                last_point = *ctrl_pts
                    .last()
                    .ok_or_else(|| invalid(OP_JOIN, "empty joined data"))?;
                if is_rational {
                    last_weight = *weights
                        .last()
                        .ok_or_else(|| invalid(OP_JOIN, "empty joined data"))?;
                }
            }

            // Make sure curve dimensions are the same - should already be fixed
            if dimension != seg_dim {
                return Err(invalid(OP_JOIN, "segment dimensions differ"));
            }

            // Make sure degrees of curves are same - should already be fixed
            if degree != seg_deg {
                // GWC: this looks like a dead code branch
                if seg_deg == 1 {
                    let second = *seg_pts
                        .get(1)
                        .ok_or_else(|| invalid(OP_JOIN, "linear segment without two points"))?;
                    let last_mult = seg_knot_mult.len().saturating_sub(1);
                    // Increase degree of curve to 2 or 3
                    // (the raised segment degree is not used afterwards)
                    if degree == 2 {
                        seg_knot_mult.fill(2);
                        if let Some(m) = seg_knot_mult.first_mut() {
                            *m = 3;
                        }
                        if let Some(m) = seg_knot_mult.get_mut(last_mult) {
                            *m = 3;
                        }
                        let mid = (first_pt + second) / 2.0;
                        seg_pts.insert(1, mid);
                    } else if degree == 3 {
                        seg_knot_mult.fill(3);
                        if let Some(m) = seg_knot_mult.first_mut() {
                            *m = 4;
                        }
                        if let Some(m) = seg_knot_mult.get_mut(last_mult) {
                            *m = 4;
                        }
                        let mid1 = first_pt + (second - first_pt) / 3.0;
                        let mid2 = first_pt + 2.0 * (second - first_pt) / 3.0;
                        seg_pts.insert(1, mid1);
                        seg_pts.insert(2, mid2);
                    }
                } else {
                    return Err(invalid(OP_JOIN, "segment degrees differ"));
                }
            }
            // Make sure end-points are the same
            let scaled_zero = EFF_ZERO_SQRT * 1000.0 * (1.0 + last_point.max_dimension());
            if last_point.distance(first_pt) > scaled_zero {
                return Err(invalid(OP_JOIN, "segments not end to end"));
            }

            // make sure end-weights are the same
            if is_rational {
                let w0 = first_of(&seg_weights)?;
                if !is_zero(last_weight - w0) {
                    if last_weight.abs() < EFF_ZERO || w0.abs() < EFF_ZERO {
                        return Err(SolidError::sm_err(OP_JOIN));
                    }

                    // Scale the weights
                    let scale = last_weight / w0;
                    for jj in 0..seg_pts.len() {
                        let w = seg_weights
                            .get_mut(jj)
                            .ok_or_else(|| invalid(OP_JOIN, "fewer weights than points"))?;
                        *w *= scale;
                    }
                }
            }

            // Now append CtrlPt, Knot, and Weight arrays.

            // Knots
            let knot_offset = knots.last().copied().unwrap_or(0.0) - first_of(&seg_knots)?;
            let seg_knot_count = seg_knots.len();
            for (jj, &k) in seg_knots.iter().enumerate().skip(1) {
                knots.push(k + knot_offset);
                let mult = *seg_knot_mult
                    .get(jj)
                    .ok_or_else(|| invalid(OP_JOIN, "fewer multiplicities than knots"))?;
                if jj == seg_knot_count - 1 {
                    // remove extra multiplicity from last knot
                    knot_mult.push(
                        mult.checked_sub(1)
                            .ok_or_else(|| invalid(OP_JOIN, "zero end multiplicity"))?,
                    );
                } else {
                    // handle interior knots
                    knot_mult.push(mult);
                }
            }

            // Weights and CtrlPts
            for (jj, &pt) in seg_pts.iter().enumerate().skip(1) {
                if is_rational {
                    weights.push(
                        *seg_weights
                            .get(jj)
                            .ok_or_else(|| invalid(OP_JOIN, "fewer weights than points"))?,
                    );
                }
                ctrl_pts.push(pt);
            }
        }

        // Clamp that last knot
        let last = knot_mult
            .last_mut()
            .ok_or_else(|| invalid(OP_JOIN, "no segments"))?;
        *last += 1;

        // Create output from canonical data
        Self::create_canonical(&CanonicalData {
            dim: dimension,
            degree,
            control_points: &ctrl_pts,
            form: BSplineCurveForm::Unspecified,
            knot_multiplicities: &knot_mult,
            knots: &knots,
            knot_type: KnotType::Unspecified,
            weights: if is_rational { Some(&weights) } else { None },
            trim: None,
        })
    }

    /// A zero-length 3D curve at `point` parameterised over `[0, 1]`
    /// (`CreatePointCurve`, through `CreateDegenerateCurve`).
    ///
    /// # Errors
    ///
    /// The errors of [`BSplineCurve::create_degenerate_curve`].
    pub fn create_point_curve(point: Point3) -> Result<Self> {
        // pass the call along
        Self::create_degenerate_curve(3, point)
    }

    /// An approximation of the offset of the curve by `offset_distance` in the plane
    /// with normal `offset_plane_normal` (to the right seen from its positive side),
    /// within `approx_tol_3d`, and the maximum gap of the approximation
    /// (`CreateSimpleOffset`). Arcs in the offset plane are offset exactly (a radius
    /// below the tolerance gives a point curve).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a zero normal or a curve with a knot of continuity C0 or
    /// less; `SM_ERR` without a NURB; [`SolidError::Unported`] for curves with interior
    /// knots (`SmCurve::EvaluateContinuity`), the arc test (`SmCurve::IsArc`) and the
    /// offset approximation (`SmOffsetCurve`).
    pub fn create_simple_offset(
        &self,
        k: &Kernel,
        approx_tol_3d: f64,
        offset_plane_normal: Vec3,
        offset_distance: f64,
        match_parameterization: bool,
    ) -> Result<(Self, f64)> {
        self.create_simple_offset_with(
            k,
            approx_tol_3d,
            offset_plane_normal,
            offset_distance,
            match_parameterization,
            &|c, n, tol| curve_is_arc(k, c, n, tol),
        )
    }

    /// [`BSplineCurve::create_simple_offset`] run for a class derived from
    /// `SmBSplineCurve` whose virtual `IsArc(lNumberOfSamplePoints, dTol, ..)` is
    /// `is_arc` (frame, radius, start and end angle in degrees).
    ///
    /// # Errors
    ///
    /// As [`BSplineCurve::create_simple_offset`], and those of `is_arc`.
    pub(crate) fn create_simple_offset_with(
        &self,
        k: &Kernel,
        approx_tol_3d: f64,
        offset_plane_normal: Vec3,
        offset_distance: f64,
        match_parameterization: bool,
        is_arc: &IsArcFn<'_>,
    ) -> Result<(Self, f64)> {
        // return SM_ERR_INVALID_INPUT when length of crOffsetPlaneNormal is zero
        let normal_len_sq = offset_plane_normal.length_squared();
        if normal_len_sq < EFF_ZERO_SQ || normal_len_sq.is_nan() {
            return Err(invalid(OP_OFFSET, "zero offset plane normal"));
        }

        // return SM_ERR_INVALID_INPUT when curve's internal continuities are less than C0
        let (min_continuity, _) = self.calculate_continuities(k, ANG_TOL_DEG)?;
        if (min_continuity as i32) <= (ContinuityType::C0 as i32) {
            return Err(invalid(OP_OFFSET, "curve continuity C0 or less"));
        }

        // do arc-offset directly
        if let Some((mut frame, mut radius, start_ang, end_ang)) =
            is_arc(self, 11, approx_tol_3d / 100.0)?
        {
            let z = frame.z_axis();
            let angle = z.angle_between(offset_plane_normal)?;
            if angle < EFF_ZERO_SQRT || angle > PI - EFF_ZERO_SQRT {
                if z.dot(offset_plane_normal) > 0.0 {
                    radius += offset_distance;
                } else {
                    radius -= offset_distance;
                }
                if radius < 0.0 {
                    radius *= -1.0;
                    let (origin, x_axis, y_axis) = frame.canonical();
                    // (status ignored upstream)
                    let _ = frame.set_canonical(origin, -x_axis, -y_axis);
                }
                if radius < approx_tol_3d {
                    // Offset is a degenerate curve.
                    let c = Self::create_degenerate_curve(3, frame.origin())?;
                    return Ok((c, 0.0));
                }
                let mut c = Self::create_circle_segment(
                    3,
                    &frame,
                    radius,
                    start_ang,
                    end_ang,
                    NurbCircleParam::Quadratic,
                )?;
                let orig_ivl = self.natural_interval();
                c.edit_parameterization(&orig_ivl)?;
                return Ok((c, 0.0));
            }
        }

        // Create an implicit offset and approximate it
        let ivl = self.natural_interval();
        offset_curve_approximation(
            k,
            self,
            &ivl,
            offset_plane_normal,
            offset_distance,
            approx_tol_3d,
            match_parameterization,
        )
    }

    /// Whether the curve is not degenerate and has coincident control points (within
    /// `tol`, or `SM_ZONE_TOL_3D` when `tol <= 0`) at either end
    /// (`HasRepeatedEndControlPoints`; upstream default `tol` is `SM_ZONE_TOL_3D`).
    /// False without a NURB.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from [`BSplineCurve::is_degenerate`].
    pub fn has_repeated_end_control_points(&self, k: &Kernel, tol: f64) -> Result<bool> {
        // no work - no BSplineCurve representation
        let Some(cur) = self.nurb.as_ref() else {
            return Ok(false);
        };

        // no work - degenerate curve
        if self.is_degenerate(k, EFF_ZERO, None)? {
            return Ok(false);
        }

        // no work - less than three control points
        let i_n = cur.n();
        if i_n < 2 {
            return Ok(false);
        }

        // pick tolerance
        let tolerance = if tol > 0.0 { tol } else { ZONE_TOL_3D };

        // spacing between CPts at either end of the curve
        let pw = cur.cpts();
        let at = |i: usize| {
            pw.get(i)
                .copied()
                .ok_or_else(|| invalid(OP_REPEATED, "control point index out of range"))
        };
        let start_cpt_dist = dist_cpt_cpt(at(0)?, at(1)?);
        let end_cpt_dist = dist_cpt_cpt(at(i_n - 1)?, at(i_n)?);

        // CPt pairs at either end of the curve are coincident checks
        let start_repeat = is_zero_to_tol(start_cpt_dist, tolerance);
        let end_repeat = is_zero_to_tol(end_cpt_dist, tolerance);
        Ok(start_repeat || end_repeat)
    }

    /// The STEP canonical data (`GetCanonical`): Euclidean control points, unique knots
    /// with multiplicities, weights for a rational curve.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn canonical(&self) -> Result<CurveCanonical> {
        let degree = self.nurb_ref()?.degree();
        let (knots, knot_multiplicities) = self.knots(None)?;
        let (control_points, weights) = self.control_polygon()?;
        Ok(CurveCanonical {
            dim: self.dim,
            degree,
            control_points,
            form: self.form,
            knot_multiplicities,
            knots,
            knot_type: self.knot_type,
            weights,
        })
    }

    /// The line point and vector (start to end) when the curve is a constant-speed line
    /// (`IsLine`; at least 3 samples). A non-rational curve with two control points is
    /// a line unless shorter than `tol`; a Bezier curve with evenly spaced control
    /// points (within `tol`) is one; other curves go to `SmCurve::IsLine`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn is_line(&self, num_samples: u64, tol: f64) -> Result<Option<(Point3, Vec3)>> {
        let num_samples = num_samples.max(3);

        // Too much to worry about with rational curves:
        if self.is_rational()? {
            return curve_is_line(self, num_samples, tol);
        }

        let pts = self.control_points()?;
        let num_pts = pts.len();
        let vec_of = |c: &CPoint| {
            let z = if c.z == NOZ { 0.0 } else { c.z };
            let v = Vec3::new(c.x, c.y, z);
            if c.w == NOW {
                v
            } else {
                Vec3::new(v.x / c.w, v.y / c.w, v.z / c.w)
            }
        };

        // Return False for a zero-length line; a closed curve is not a line either.
        let (Some(first), Some(last)) = (pts.first(), pts.last()) else {
            return Err(invalid(OP_IS_LINE, "empty control polygon"));
        };
        let end_vec = vec_of(last);
        let start_vec = vec_of(first);
        let base_vec = end_vec - start_vec;

        if base_vec.length_squared() < tol * tol {
            return Ok(None);
        }

        // special case - BsplineCurve with two Control Point is always a line
        if num_pts == 2 {
            return Ok(Some((start_vec, base_vec)));
        }

        // A Bezier curve with evenly spaced control points is a constant-speed line;
        // on a violation defer to the base class method.
        if self.degree()? == num_pts - 1 {
            // All adjacent point diffs should match this:
            let standard_vec = base_vec / (num_pts - 1) as f64;

            let mut previous_vec = start_vec;
            for c in pts.iter().skip(1) {
                let current_vec = vec_of(c);
                let test_vec = current_vec - previous_vec;
                previous_vec = current_vec;
                let error_vec = test_vec - standard_vec;
                if error_vec.length_squared() > tol * tol {
                    return curve_is_line(self, num_samples, tol);
                }
            }

            // Point diffs all match.
            return Ok(Some((start_vec, base_vec)));
        }

        // No more optimizations.
        curve_is_line(self, num_samples, tol)
    }

    /// Whether the curve is non-rational with end knots of multiplicity `p + 1` and
    /// interior knots of multiplicity `p` (`IsPiecewiseC0`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn is_piecewise_c0(&self) -> Result<bool> {
        // Query this Bspline
        let degree = self.degree()?;
        let Ok((knots, multiplicities)) = self.knots(None) else {
            return Ok(false);
        };
        let unique_knot_count = multiplicities.len();
        let mult = |i: usize| {
            multiplicities
                .get(i)
                .copied()
                .ok_or_else(|| invalid(OP_C0, "no knots"))
        };

        let Some(degree_plus) = degree.checked_add(1) else {
            return Ok(false);
        };

        // when every knot is of multiplicity degree (+1 on the ends)
        // and the curve is non-rational
        let mut is_piecewise_c0 = !self.is_rational()?
            && mult(0)? == degree_plus
            && mult(unique_knot_count.wrapping_sub(1))? == degree_plus;

        // Check all internal knot multiplicities equal lDegree
        if is_piecewise_c0 {
            for ii in 1..knots.len().saturating_sub(1) {
                if degree != mult(ii)? {
                    is_piecewise_c0 = false;
                    break;
                }
            }
        }
        Ok(is_piecewise_c0)
    }

    /// Whether the curve's largest bounding box side is below `max(tol3d, scaled
    /// zero)` (`IsDegenerate`; upstream defaults `SM_EFF_ZERO` and the natural
    /// interval). Another interval goes to `SmCurve::IsDegenerate`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB (natural interval).
    pub fn is_degenerate(
        &self,
        k: &Kernel,
        tol3d: f64,
        interval: Option<&Extent1d>,
    ) -> Result<bool> {
        // low work - for nonNaturalInterval intervals pass the call along to the
        // general SmCurve::IsDegenerate
        let other_interval = interval.is_some_and(|i| !i.are_equal(&self.natural_interval(), 0.0));
        if other_interval || self.is_kind_of(CRV_ON_SURF_TYPE)? {
            return curve_is_degenerate(k, self, tol3d, interval);
        }

        // ensure existence of m_pNurb (a plain B-spline cannot make one)
        let cur = self.nurb_ref()?;

        // pass the call along
        is_ncrv_degenerate(cur, tol3d)
    }

    /// Control points `start..=end` in `form`, `stride` values apart
    /// (`GetControlPointsExpert`): `x y [z]` for [`ControlPointForm::NonRational`]
    /// (divided by the weight for a rational curve), `wx wy [wz] w` for
    /// [`ControlPointForm::HomogeneousRational`], `x y [z] w` for
    /// [`ControlPointForm::EuclideanRational`]; `z` only for dimensions above 2, a
    /// missing `z` is `0` and a missing weight `1`. Slots not written are `0`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for `start > end`, `end > n`, a stride below the point size or a
    /// dimension below 2; `SM_ERR` without a NURB.
    pub fn control_points_expert(
        &self,
        form: ControlPointForm,
        start: usize,
        end: usize,
        stride: usize,
    ) -> Result<Vec<f64>> {
        let cur = self.nurb_ref()?;
        let is_rational = self.is_rational()?;
        let dim = self.dim;
        let min_stride = dim.saturating_add(u64::from(form != ControlPointForm::NonRational));
        let ok_stride = stride as u64 >= min_stride;

        // check inputs - consistent indexing and strides
        if start > end || end > cur.n() || !ok_stride {
            return Err(invalid(OP_EXPERT, "inconsistent input args"));
        }
        // upstream writes two values per point even for a smaller stride
        if dim < 2 {
            return Err(invalid(OP_EXPERT, "dimension below 2"));
        }

        let count = end - start + 1;
        let len = stride
            .checked_mul(count)
            .ok_or_else(|| invalid(OP_EXPERT, "output too large"))?;
        // the caller's output array upstream: allocated fallibly here
        let mut out = Vec::new();
        out.try_reserve_exact(len)
            .map_err(|_| invalid(OP_EXPERT, "output too large"))?;
        out.resize(len, 0.0);
        let mut address = 0usize;
        let three_d = dim > 2;

        // for every requested control point
        for cpt in cur
            .cpts()
            .get(start..=end)
            .ok_or_else(|| invalid(OP_EXPERT, "control point index out of range"))?
        {
            let x = cpt.x;
            let y = cpt.y;
            let z = if cpt.z == NOZ { 0.0 } else { cpt.z };
            let w = if cpt.w == NOW { 1.0 } else { cpt.w };

            let vals: [Option<f64>; 4] = match form {
                ControlPointForm::NonRational => {
                    if is_rational {
                        [Some(x / w), Some(y / w), three_d.then(|| z / w), None]
                    } else {
                        [Some(x), Some(y), three_d.then_some(z), None]
                    }
                }
                ControlPointForm::HomogeneousRational => {
                    if three_d {
                        [Some(x), Some(y), Some(z), Some(w)]
                    } else {
                        [Some(x), Some(y), Some(w), None]
                    }
                }
                ControlPointForm::EuclideanRational => {
                    if three_d {
                        [Some(x / w), Some(y / w), Some(z / w), Some(w)]
                    } else {
                        [Some(x / w), Some(y / w), Some(w), None]
                    }
                }
            };
            for (k, v) in vals.iter().enumerate() {
                if let Some(v) = v {
                    let slot = out
                        .get_mut(address + k)
                        .ok_or_else(|| invalid(OP_EXPERT, "stride too small"))?;
                    *slot = *v;
                }
            }
            address += stride;
        }
        Ok(out)
    }

    /// The largest reasonable domain for extensions (`GetMaxAnalyticDomain`): the
    /// natural interval, expanded relatively by 100 (degree 1), 1 (degree 2) or 0.5
    /// unless the curve is closed.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn max_analytic_domain(&self, k: &Kernel) -> Result<Extent1d> {
        let mut dom = self.natural_interval();

        // Do not extend closed curves.
        if curve_is_closed(k, self, &dom)? {
            return Ok(dom);
        }

        // Expansion depends on degree.
        let deg = self.degree()?;
        if deg == 1 {
            dom.expand_relative(100.0);
        } else if deg == 2 {
            dom.expand_relative(1.0);
        } else {
            dom.expand_relative(0.5);
        }
        Ok(dom)
    }

    /// The number of unique knots in the natural interval: the spans `U[j] < U[j+1]`
    /// (beyond `SM_EFF_ZERO`) for `p <= j < m - p`, plus one
    /// (`GetNumberOfUniqueKnots`).
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB.
    pub fn number_of_unique_knots(&self) -> Result<usize> {
        let cur = self.nurb_ref()?;
        let u = cur.knots();
        let p = cur.degree();
        // Copy unique knots and compute multiplicities
        let mut count = 0usize;
        for j in p..cur.m().saturating_sub(p) {
            let (Some(&a), Some(&b)) = (u.get(j), u.get(j + 1)) else {
                return Err(invalid(OP_UNIQUE, "knot index out of range"));
            };
            if !is_zero(a - b) {
                count += 1;
            }
        }

        // There is one more than the number of spans which we just counted.
        Ok(count + 1)
    }

    /// The Euclidean control points of the spans covering `interval`, all of them
    /// for the natural interval or a single control point (`GetPolygon`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an interval not inside the natural interval (within
    /// `SM_EFF_ZERO`); `SM_ERR` without a NURB or when a span cannot be found.
    pub fn polygon(&self, interval: &Extent1d) -> Result<Vec<Point3>> {
        let natural_interval = self.natural_interval();
        let cur = self.nurb_ref()?;
        let n = cur.n();
        let p = cur.degree();

        // First make sure interval sent in is valid (skipped for a degenerate curve)
        if n > 0 && !interval.is_contained_by(&natural_interval, EFF_ZERO) {
            return Err(invalid(OP_POLYGON, "interval outside the natural interval"));
        }

        let (start_point, num_points) = if (natural_interval.min() == interval.min()
            && natural_interval.max() == interval.max())
            || n == 0
        {
            (0, n + 1)
        } else {
            // use control polygon which contributes to the included spans
            let knt = cur.knot_vector().map_err(SolidError::nl_ser(OP_POLYGON))?;
            let start_span = knt
                .find_span(p, interval.min(), Side::Left)
                .map_err(SolidError::nl_ser(OP_POLYGON))?;
            let end_span = knt
                .find_span(p, interval.max(), Side::Right)
                .map_err(SolidError::nl_ser(OP_POLYGON))?;
            let start = start_span
                .checked_sub(p)
                .ok_or_else(|| invalid(OP_POLYGON, "span below the degree"))?;
            let num = end_span
                .checked_add(1)
                .and_then(|e| e.checked_sub(start))
                .ok_or_else(|| invalid(OP_POLYGON, "end span before start span"))?;
            (start, num)
        };

        let pts = cur
            .cpts()
            .get(start_point..)
            .and_then(|s| s.get(..num_points))
            .ok_or_else(|| invalid(OP_POLYGON, "control point index out of range"))?;
        Ok(pts.iter().map(|&c| to_euclid(c)).collect())
    }

    /// Makes the weights equal to `other`'s, scaling each homogeneous control point by
    /// the weight ratio where the weights differ (`MatchWeights`; the curves must be
    /// compatible).
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a different number of control points or a curve without NURB.
    pub fn match_weights(&mut self, other: &Self) -> Result<()> {
        let other_pts = other.control_points()?;
        let our_pts = self.nurb_ref_mut()?.cpts_mut();
        if other_pts.len() != our_pts.len() {
            return Err(SolidError::sm_err(OP_MATCH));
        }

        for (ours, theirs) in our_pts.iter_mut().zip(other_pts) {
            if !are_same(ours.w, theirs.w) {
                let ratio = theirs.w / ours.w;
                ours.x *= ratio;
                ours.y *= ratio;
                ours.z *= ratio;
                ours.w = theirs.w;
            }
        }
        Ok(())
    }

    /// Raises the degree to `new_degree` (`DegreeElevate`, `N_CrvElevateDegree`); the
    /// NURB is replaced by a compact copy of the elevated curve with fixed-up knots.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for a new degree below the current one, without a NURB, or when the
    /// elevation fails.
    pub fn degree_elevate(&mut self, new_degree: usize) -> Result<()> {
        // no work - no degree increase needed
        let degree = self.degree()?;
        if new_degree < degree {
            return Err(SolidError::sm_err(OP_ELEVATE));
        }
        if new_degree == degree {
            return Ok(());
        }

        let increment = new_degree - degree;
        let cur = self
            .nurb_ref()?
            .elevate_degree(increment)
            .map_err(SolidError::nl_ser(OP_ELEVATE))?;

        // Swap Nurbs with this
        self.nurb = Self::from_gw_nurb(self.dim, &cur).nurb;
        Ok(())
    }

    /// Reverses the parameterisation, keeping the natural interval
    /// (`ReverseParameterization`, `N_CrvReverse`): the new curve at `t` is the old one
    /// at `a + b - t`. Returns the new domain of `old_interval`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without a NURB or when the reversal fails.
    pub fn reverse_parameterization(&mut self, old_interval: &Extent1d) -> Result<Extent1d> {
        let ivl = self.natural_interval();
        let t_new_max = 1.0 - (old_interval.min() - ivl.min()) / (ivl.max() - ivl.min());
        let t_new_min = 1.0 - (old_interval.max() - ivl.min()) / (ivl.max() - ivl.min());

        self.nurb_ref_mut()?
            .reverse()
            .map_err(SolidError::nl_ser(OP_REVERSE))?;

        let new_ivl = self.natural_interval();
        let (a, b) = (new_ivl.evaluate(t_new_min), new_ivl.evaluate(t_new_max));
        // SetMinMax (status ignored upstream: an inverted pair leaves the interval)
        if a > b {
            return Ok(*old_interval);
        }
        Ok(Extent1d::from_raw(a, b))
    }

    /// Replaces the NURB and dimension by those of the curve
    /// [`BSplineCurve::create_canonical`] builds from `data` (`SetCanonical`); the knot
    /// type and form stay as they were, the NURB is no longer borrowed.
    ///
    /// # Errors
    ///
    /// The errors of [`BSplineCurve::create_canonical`] (the curve is then unchanged).
    pub fn set_canonical(&mut self, data: &CanonicalData<'_>) -> Result<()> {
        // Create a temporary one
        let tmp = Self::create_canonical(data)?;

        // Swap Nurbs with this, and the dimension
        self.nurb = tmp.nurb;
        self.dim = tmp.dim;
        if self.nurb_is_borrowed {
            self.nurb_is_borrowed = false;
        }
        Ok(())
    }

    /// Sets control point `index` (`SetControlPoint`). With
    /// [`ControlPointForm::EuclideanRational`] the point is multiplied by `weight` and
    /// every non-rational control point gets weight 1; with
    /// [`ControlPointForm::NonRational`] the weight is 1. `x y z` are stored as given
    /// (also for a 2D curve), the weight only on a rational curve.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for an index above `n` or without a NURB.
    pub fn set_control_point(
        &mut self,
        form: ControlPointForm,
        index: usize,
        point: Point3,
        weight: f64,
    ) -> Result<()> {
        let pw = self.nurb_ref_mut()?.cpts_mut();
        if index >= pw.len() {
            return Err(SolidError::sm_err(OP_SET_CP));
        }

        let mut control_point = point;
        let mut weight = weight;

        // They're stored in homogeneous rational form:
        if form == ControlPointForm::EuclideanRational {
            control_point.x *= weight;
            control_point.y *= weight;
            control_point.z *= weight;

            // make sure all the other control points are also rational
            for c in pw.iter_mut() {
                if c.w == NOW {
                    c.w = 1.0;
                }
            }
        }
        if form == ControlPointForm::NonRational {
            weight = 1.0;
        }

        let rational = pw.first().is_some_and(|c| c.w != NOW);
        let cpt = pw
            .get_mut(index)
            .ok_or_else(|| SolidError::sm_err(OP_SET_CP))?;
        cpt.x = control_point.x;
        cpt.y = control_point.y;
        cpt.z = control_point.z;
        if rational {
            cpt.w = weight;
        }
        Ok(())
    }

    /// Adjusts the curve to `new_interval` (`AdjustSTEPInterval`): ends outside the
    /// natural interval (beyond `SM_EFF_ZERO * (1 + max)`) are reached by precise
    /// polynomial extension ([`BSplineCurve::create_extended_curve`], failures ignored),
    /// then the curve is trimmed ([`BSplineCurve::trim`]).
    ///
    /// # Errors
    ///
    /// The errors of [`BSplineCurve::trim`].
    pub fn adjust_step_interval(&mut self, k: &Kernel, new_interval: &Extent1d) -> Result<()> {
        let ivl = self.natural_interval();

        // See if it needs extension.
        let tol = EFF_ZERO * (1.0 + ivl.max());

        if self.nurb.is_some() && !new_interval.is_contained_by(&ivl, tol) {
            // Start of curve, then end of curve:
            for param in [new_interval.min(), new_interval.max()] {
                if !ivl.contains_value(param, tol) {
                    match self.create_extended_curve(param, ContinuityType::CInfinity, true) {
                        Ok(Some(ext)) if ext.nurb.is_some() => self.nurb = ext.nurb,
                        Err(e @ SolidError::Unported { .. }) => return Err(e),
                        _ => {}
                    }
                }
            }
        }

        // Now trim if necessary.
        let mut trim_ivl = *new_interval;
        self.trim(k, &mut trim_ivl, TrimMode::Notify)
    }

    /// Reads a curve written by `WriteToDB` (`ReadFromDB`): `n`, `p`, the rational
    /// flag, the dimension, for `version > 32` the knot type and form, then `n+1`
    /// control points (`x y`, plus `z` for a 3D file, plus `w` when the flag is
    /// non-zero) and `n+p+2` knots (one group per line in ASCII files). The curve gets
    /// the file's dimension, its knots are not fixed up; a 3D file read for `dim == 2`
    /// is made 2D.
    ///
    /// # Errors
    ///
    /// `InvalidInput` (`SM_ERR_INVALID_INPUT`) when a read fails part way (data ends
    /// early or is not a number), for a degree outside `1..=NL_MAXDEG`, fewer control
    /// points than degree + 1 or too many to size, a rational flag other than 0 or 1,
    /// a knot type or form outside its enum, or a stream shorter than the header
    /// declares; `SM_ERR` from the 2D conversion.
    pub fn read_from_db(db: &mut DatabaseReader, dim: u64, version: u64) -> Result<Self> {
        let bad = |reason: &'static str| invalid(OP_READ, reason);

        let mut knot_type_code: u64 = KnotType::Unspecified as u64;
        let mut form_code: u64 = BSplineCurveForm::Unspecified as u64;

        // Get parameters from the top
        let ascii = db.file_type() == FileType::Ascii;
        let (n, p, rat, read_dim) = if ascii {
            let n = db.in_stream_mut().and_then(InputStream::extract_long);
            db.go_to_next_line();
            let p = db.in_stream_mut().and_then(InputStream::extract_short);
            db.go_to_next_line();
            let rat = db.in_stream_mut().and_then(InputStream::extract_short);
            db.go_to_next_line();
            let read_dim = db.in_stream_mut().and_then(InputStream::extract_short);
            db.go_to_next_line();
            if version > 32 {
                if let Some(v) = db.in_stream_mut().and_then(InputStream::extract_ulong) {
                    knot_type_code = v;
                }
                db.go_to_next_line();
                if let Some(v) = db.in_stream_mut().and_then(InputStream::extract_ulong) {
                    form_code = v;
                }
                db.go_to_next_line();
            }
            match (n, p, rat, read_dim) {
                (Some(n), Some(p), Some(rat), Some(d)) => (n, p, rat, d),
                _ => return Err(bad(READ_FAILED)),
            }
        } else {
            let n = db.read_long()?;
            let p = db.read_short()?;
            let rat = db.read_short()?;
            let read_dim = db.read_short()?;
            if version > 32 {
                knot_type_code = db.read_ulong()?;
                form_code = db.read_ulong()?;
            }
            (n, p, rat, read_dim)
        };
        if db.read_failed() {
            return Err(bad(READ_FAILED));
        }

        // A curve above NL_MAXDEG evaluates, but a surface built from it (an extrusion or
        // a sweep) has that degree and overflows NLib's surface evaluators, which size
        // their stack arrays by NL_MAXDEG. A degree-0 curve has a zero tangent. NLib's own
        // curve checks also cap curves at NL_MAXDEG, so reject anything outside
        // [1, NL_MAXDEG].
        if p < 1 || i64::from(p) > MAX_DEGREE as i64 {
            return Err(bad(
                "NURBS curve degree is outside the supported range 1 to NL_MAXDEG",
            ));
        }

        // The count comes straight from the file too, as the highest index. A curve needs
        // at least degree + 1 control points, and the NURB is one block whose byte size is
        // a ULONG, so reject a count that would overflow it before it sizes anything.
        if n < i64::from(p) {
            return Err(bad(
                "NURBS curve control point count is fewer than degree + 1",
            ));
        }
        if n as u64 >= u64::MAX / (4 * 32) {
            return Err(bad("NURBS curve control point count is too large"));
        }
        // Any nonzero flag would otherwise select the rational read below.
        if rat != 0 && rat != 1 {
            return Err(bad("NURBS curve rational flag is neither NL_NO nor NL_YES"));
        }

        // knot count
        let n = usize::try_from(n).map_err(|_| bad("negative size"))?;
        let p = usize::try_from(p).map_err(|_| bad("negative degree"))?;
        let m = n
            .checked_add(p)
            .and_then(|x| x.checked_add(1))
            .ok_or_else(|| bad("size overflow"))?;

        // Get different types of input
        let rational = rat != 0;
        let two_d = read_dim == 2;
        let per = 2 + usize::from(!two_d) + usize::from(rational);

        // Bound the header against the bytes left before allocating (binary: 8 bytes
        // per double; ASCII: at least one byte per value).
        let values = n
            .checked_add(1)
            .and_then(|k| k.checked_mul(per))
            .and_then(|k| k.checked_add(m + 1))
            .ok_or_else(|| bad("size overflow"))?;
        db.ensure_available(values, if ascii { 1 } else { 8 }, OP_READ)?;

        // alloc m_pNurb memory
        let mut cur = allocate_nurb_curve(n, p, m)?;

        // Read in data
        let mut vals = [f64::NAN; 4];
        for slot in cur.cpts_mut().iter_mut() {
            if ascii {
                let s = db.in_stream_mut().ok_or_else(|| bad("no input stream"))?;
                for v in vals.iter_mut().take(per) {
                    if let Some(x) = s.extract_double() {
                        *v = x;
                    }
                }
                db.go_to_next_line();
            } else {
                for v in vals.iter_mut().take(per) {
                    *v = db.read_double()?;
                }
            }
            if db.read_failed() {
                return Err(bad(READ_FAILED));
            }
            let [wx, wy, a, b] = vals;
            *slot = match (two_d, rational) {
                // 2-D non-rational
                (true, false) => CPoint::new(wx, wy, NOZ, NOW),
                // 3-D non-rational
                (false, false) => CPoint::new(wx, wy, a, NOW),
                // 2-D rational
                (true, true) => CPoint::new(wx, wy, NOZ, a),
                // 3-D rational
                (false, true) => CPoint::new(wx, wy, a, b),
            };
        }

        // for every knot
        for slot in cur.knots_mut() {
            if ascii {
                let x = db.in_stream_mut().and_then(InputStream::extract_double);
                db.go_to_next_line();
                if let Some(x) = x {
                    *slot = x;
                }
            } else {
                *slot = db.read_double()?;
            }
            if db.read_failed() {
                return Err(bad(READ_FAILED));
            }
        }

        // Set output
        let knot_type = knot_type_from_code(knot_type_code)
            .ok_or_else(|| bad("knot type outside SmKnotType"))?;
        let form =
            form_from_code(form_code).ok_or_else(|| bad("form outside SmBSplineCurveForm"))?;
        let read_dim = u64::try_from(read_dim).map_err(|_| bad("negative dimension"))?;
        let mut new_curve = Self {
            dim: read_dim,
            nurb: Some(cur),
            nurb_is_borrowed: false,
            out_of_bounds_enabled: false,
            knot_type,
            form,
        };

        // fix 2d curve representations
        if dim == 2 && read_dim == 3 {
            new_curve.convert_to_2d()?;
        }
        Ok(new_curve)
    }

    /// `WriteToDB`: writes the NURB's header (last control point index, degree,
    /// rational flag and dimension, then for `version > 32` the knot type and form),
    /// its control points (`x y`, `x y z`, `wx wy w` or `wx wy wz w`: Euclidean for a
    /// non-rational NURB, weighted for a rational one) and its knots.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without NURB (`GetOrCreateGwNurbPointer` cannot make one for a
    /// B-spline; the analytic kinds always hold theirs).
    pub fn write_to_db(&self, db: &mut DatabaseWriter, version: u64) -> Result<()> {
        let cur = self.nurb_ref()?;
        let n = cur.n() as i64;
        let p = cur.degree() as i16;
        let knot_type = self.knot_type as u64;
        let form = self.form as u64;

        // set type flag for different types of output
        let rat: i16 = i16::from(cur.is_rational());
        let dim: i16 = if cur.is_3d() { 3 } else { 2 };

        // output header, n, p, rat, and dim
        let ascii = db.file_type() == FileType::Ascii;
        if ascii {
            db.out_i64(n).out_str(" Number of Control Points \n");
            db.out_i64(p.into()).out_str(" Degree \n");
            db.out_i64(rat.into()).out_str(" RationalFlag \n");
            db.out_i64(dim.into()).out_str(" Dimension \n");
            if version > 32 {
                db.out_u64(knot_type);
                db.out_str(
                    " Knot Type Enum: 1=2d NonRational, 2=3d NonRational, 3=2d Rational, \
                     4=3d Rational\n",
                );
                db.out_u64(form).out_str(" BSplineCurveForm Enum\n");
            }
        } else {
            db.write_long(n);
            db.write_short(p);
            db.write_short(rat);
            db.write_short(dim);
            if version > 32 {
                db.write_long(knot_type as i64);
                db.write_long(form as i64);
            }
        }

        // for every control point, by curve type
        for pw in cur.cpts() {
            let [x, y, z] = cpt_to_xyz(*pw);
            let values = match (rat == 1, dim == 3) {
                // 2-D non-rational
                (false, false) => [x, y, 0.0, 0.0],
                // 3-D non-rational
                (false, true) => [x, y, z, 0.0],
                // 2-D rational
                (true, false) => [pw.x, pw.y, pw.w, 0.0],
                // 3-D rational
                (true, true) => [pw.x, pw.y, pw.z, pw.w],
            };
            let count = 2 + usize::from(dim == 3) + usize::from(rat == 1);
            db.reals(values.get(..count).unwrap_or_default());
        }

        // for every knot
        for &u in cur.knots() {
            db.reals(&[u]);
        }
        Ok(())
    }
}

/// `N_CPtToXYZ`: the Euclidean coordinates of a control point (`z` of a 2D point is
/// `0`; a weighted point is divided by its weight).
fn cpt_to_xyz(pw: CPoint) -> [f64; 3] {
    let mut x = pw.x;
    let mut y = pw.y;
    let mut z = if pw.z == NOZ { 0.0 } else { pw.z };
    if pw.w != NOW {
        let w = pw.w;
        x /= w;
        y /= w;
        z /= w;
    }
    [x, y, z]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::crv_basic::Curve;

    fn line(a: Point3, b: Point3) -> BSplineCurve {
        BSplineCurve::create_line_segment(3, a, b).unwrap()
    }

    fn cubic() -> BSplineCurve {
        let cpts = vec![
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(1.0, 2.0, 0.0, NOW),
            CPoint::new(3.0, 2.0, 1.0, NOW),
            CPoint::new(4.0, 0.0, 0.0, NOW),
            CPoint::new(5.0, -1.0, 0.0, NOW),
        ];
        let u = vec![0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0];
        BSplineCurve::from_nurb(Curve::new(cpts, 3, u).unwrap(), 3, false)
    }

    #[test]
    fn line_is_line() {
        let c = line(Point3::new(0.0, 0.0, 0.0), Point3::new(2.0, 0.0, 0.0));
        let (p, v) = c.is_line(3, 1e-9).unwrap().unwrap();
        assert_eq!(p, Point3::new(0.0, 0.0, 0.0));
        assert_eq!(v, Vec3::new(2.0, 0.0, 0.0));
        let z = BSplineCurve::create_point_curve(Point3::new(1.0, 1.0, 1.0)).unwrap();
        assert!(z.is_line(3, 1e-9).unwrap().is_none());
    }

    #[test]
    fn non_bezier_line_test_samples_the_curve() {
        // a curved cubic: SmCurve::IsLine (co-ported) rejects it
        assert!(cubic().is_line(3, 1e-9).unwrap().is_none());
    }

    #[test]
    fn unique_knots_and_c0() {
        let c = cubic();
        assert_eq!(c.number_of_unique_knots().unwrap(), 3);
        assert!(!c.is_piecewise_c0().unwrap());
        assert!(!c.is_intersector_approx().unwrap());
    }

    #[test]
    fn piecewise_c0_rejects_unrepresentable_endpoint_multiplicity() {
        let nurb =
            Curve::new(vec![CPoint::new(0.0, 0.0, 0.0, NOW)], usize::MAX, vec![0.0]).unwrap();
        let c = BSplineCurve::from_nurb(nurb, 3, false);
        assert_eq!(c.is_piecewise_c0(), Ok(false));
    }

    #[test]
    fn elevate_and_reverse() {
        let mut c = line(Point3::new(0.0, 0.0, 0.0), Point3::new(3.0, 0.0, 0.0));
        c.degree_elevate(3).unwrap();
        assert_eq!(c.degree().unwrap(), 3);
        assert!(c.degree_elevate(2).is_err());
        let ivl = c.natural_interval();
        let r = c.reverse_parameterization(&ivl).unwrap();
        assert_eq!(r, ivl);
        let s = c.evaluate_point(0.0).unwrap();
        assert!((s.x - 3.0).abs() < 1e-12);
    }

    #[test]
    fn join_two_lines() {
        let mut segs = vec![
            line(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)),
            line(Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0)),
        ];
        let j = BSplineCurve::create_by_joining(&mut segs, None).unwrap();
        assert_eq!(j.number_control_points().unwrap(), 3);
        assert_eq!(j.natural_interval(), Extent1d::from_min_max(0.0, 2.0));
        let mut gap = vec![
            line(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)),
            line(Point3::new(2.0, 0.0, 0.0), Point3::new(3.0, 0.0, 0.0)),
        ];
        assert!(BSplineCurve::create_by_joining(&mut gap, None).is_err());
        assert!(BSplineCurve::create_by_joining(&mut [], None).is_err());
    }

    /// A span whose squared extent overflows sets no basis (`MakeUnitOrthoVectors`
    /// fails, `SetBasis` refuses the result), so the caller's basis stays: zero for a
    /// curve cache span (zero-filled slot), the constructor's for a fresh box.
    #[test]
    fn bounding_box_keeps_the_callers_basis_when_none_can_be_set() {
        let c = line(Point3::new(0.77, 0.05, 0.0), Point3::new(1e300, 0.38, 0.0));
        let req = BoundingBoxRequest {
            normal: true,
            pseudo: true,
            polar: false,
        };
        let ivl = c.natural_interval();
        let span = c
            .calculate_bounding_box_onto(&ivl, req, PseudoBox::zero_filled())
            .unwrap();
        assert_eq!(span.pseudo.unwrap().basis(), &[Vec3::ZERO; 3]);
        let fresh = c.calculate_bounding_box(&ivl, req).unwrap();
        assert_eq!(fresh.pseudo.unwrap().basis(), &[Vec3::X, Vec3::Y, Vec3::Z]);
    }

    #[test]
    fn bounding_box_of_cubic() {
        let c = cubic();
        let req = BoundingBoxRequest {
            normal: true,
            pseudo: true,
            polar: true,
        };
        let b = c
            .calculate_bounding_box(&c.natural_interval(), req)
            .unwrap();
        let nb = b.normal.unwrap();
        assert_eq!(nb.min(), Point3::new(0.0, -1.0, 0.0));
        assert_eq!(nb.max(), Point3::new(5.0, 2.0, 1.0));
        assert!(b.pseudo.is_some() && b.polar.is_some());
    }

    #[test]
    fn expert_and_set_control_point() {
        let mut c = cubic();
        let v = c
            .control_points_expert(ControlPointForm::EuclideanRational, 1, 2, 5)
            .unwrap();
        assert_eq!(v, vec![1.0, 2.0, 0.0, 1.0, 0.0, 3.0, 2.0, 1.0, 1.0, 0.0]);
        assert!(c
            .control_points_expert(ControlPointForm::EuclideanRational, 1, 2, 3)
            .is_err());
        c.set_control_point(
            ControlPointForm::EuclideanRational,
            1,
            Point3::new(1.0, 1.0, 1.0),
            2.0,
        )
        .unwrap();
        assert!(c.is_rational().unwrap());
        assert_eq!(
            c.control_points().unwrap()[1],
            CPoint::new(2.0, 2.0, 2.0, 2.0)
        );
        assert!(c
            .set_control_point(
                ControlPointForm::NonRational,
                9,
                Point3::new(0.0, 0.0, 0.0),
                1.0
            )
            .is_err());
    }

    #[test]
    fn read_ascii() {
        let text = "1\n1\n0\n3\n0 0 0\n1 2 3\n0\n0\n1\n1\n";
        let mut db = DatabaseReader::new();
        db.open_for_read(text.as_bytes().to_vec(), FileType::Ascii);
        let c = BSplineCurve::read_from_db(&mut db, 3, 30).unwrap();
        assert_eq!(c.dim(), 3);
        assert_eq!(
            c.control_points().unwrap()[1],
            CPoint::new(1.0, 2.0, 3.0, NOW)
        );
        let mut db = DatabaseReader::new();
        db.open_for_read(text.as_bytes().to_vec(), FileType::Ascii);
        let c = BSplineCurve::read_from_db(&mut db, 2, 30).unwrap();
        assert_eq!(c.dim(), 2);
    }

    #[test]
    fn analytic() {
        let c = cubic();
        assert!(c
            .create_analytic_curve(&Kernel::default(), &c.natural_interval())
            .unwrap()
            .is_none());
        let l = line(Point3::new(0.0, 0.0, 0.0), Point3::new(2.0, 0.0, 0.0));
        let Some(AnalyticCurve::Line(a)) = l
            .create_analytic_curve(&Kernel::default(), &l.natural_interval())
            .unwrap()
        else {
            panic!("expected a line");
        };
        assert_eq!(a.evaluate_point(1.0), Point3::new(2.0, 0.0, 0.0));
    }

    #[test]
    fn control_points_expert_absurd_stride_is_an_error() {
        // fuzz regression (nurbs.b_spline_curve_p2 fuzz00841): a 2^32 stride aborted
        // the output allocation
        let c = cubic();
        let r = c.control_points_expert(ControlPointForm::NonRational, 0, 4, 1 << 62);
        assert!(r.is_err());
        assert!(c
            .control_points_expert(ControlPointForm::NonRational, 0, 4, usize::MAX)
            .is_err());
        assert_eq!(
            c.control_points_expert(ControlPointForm::NonRational, 0, 4, 3)
                .unwrap()
                .len(),
            15
        );
    }
}
