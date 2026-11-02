//! High-level curve creation (`SM_API` curves): line segments and lines, circles,
//! ellipses and arcs in the XY plane, helices, rectangles and regular polygons as
//! line segments, and cubic or canonical B-spline curves from control points.
//!
//! Upstream: `source/SM_API/src/SmApiCurves.cpp`, `source/SM_API/inc/SmApiCurves.h`.
//!
//! | Rust                         | Upstream                                              |
//! |------------------------------|-------------------------------------------------------|
//! | [`create_line_segment`]      | `SmApiCreateLineSegment`                              |
//! | [`create_line`]              | `SmApiCreateLine`                                     |
//! | `conic_inputs_valid` (private) | `SmApiConicInputsValid` (static)                    |
//! | `circle_inputs_valid` (private) | `SmApiCircleInputsValid` (static)                  |
//! | [`create_circle`]            | `SmApiCreateCircle` (`SmBSplineCurve*&` overload)     |
//! | [`create_analytic_circle`]   | `SmApiCreateCircle` (`SmCircle*&` overload)           |
//! | [`create_ellipse`]           | `SmApiCreateEllipse`                                  |
//! | [`create_arc`]               | `SmApiCreateArc`                                      |
//! | [`create_helix`], [`HelixParams`] | `SmApiCreateHelix`                               |
//! | [`create_rectangle`]         | `SmApiCreateRectangle`                                |
//! | [`create_regular_polygon`]   | `SmApiCreateRegularPolygon`                           |
//! | [`create_curve`]             | `SmApiCreateCurve`                                    |
//! | [`create_canonical_curve`], [`CanonicalCurveParams`] | `SmApiCreateCanonicalCurve`   |
//! | [`create_curve_approx_points`] | `SmApiCreateCurveApproxPoints` (unit `nlib.crv_fit_p4`) |
//! | [`remove_curve_knots`]       | `SmApiRemoveCurveKnots`                               |
//!
//! Deviations:
//! - **Out-params.** The created curve is the return value; on failure there is none
//!   (upstream sets `rpResult = NULL` or leaves it unset). [`create_rectangle`] and
//!   [`create_regular_polygon`] return the new segments instead of appending to a
//!   caller array (upstream rolls the array back on failure, so the observable result
//!   is the same).
//! - **Memory.** [`create_regular_polygon`] reserves its `num_sides` result slots
//!   fallibly up front: a side count whose segments cannot fit in memory, or in
//!   [`crate::limits::MAX_ALLOC_BYTES`], is `InvalidInput` (upstream keeps
//!   allocating segments until allocation fails).
//! - **Context.** `SmApiGetOrCreateContext()` is the caller's [`Kernel`]; the
//!   B-spline and line constructors take no context in the port.
//! - **Statuses.** `SM_ERR_INVALID_INPUT` is [`SolidError::InvalidInput`];
//!   `SM_ERR_NOT_WITHIN_TOLERANCE` and `SM_ERR` are [`SolidError::Upstream`] with
//!   [`SM_ERR_NOT_WITHIN_TOLERANCE`] / [`crate::error::SM_ERR`]. Errors of the called
//!   constructors pass through unchanged.
//! - `SmApiCreateHelix` returns the curve as an `SmBSplineCurve`; the fitter's
//!   `TWOPI` literal `6.2831853071795864` is [`core::f64::consts::TAU`] (the same
//!   double).
//! - **`SmApiRemoveCurveKnots`.** The curve is a `&mut` [`Curve`] (upstream's `NULL`
//!   check has no counterpart). Its `dynamic_cast<SmBSplineCurve*>` holds for the
//!   B-spline kinds that derive from `SmBSplineCurve` upstream: B-splines, lines,
//!   ellipses (circles), hyperbolas and parabolas. Their NURB is made first when they
//!   have none (`GetOrCreateGwNurbPointer`, status ignored); a curve still without one
//!   is `SM_ERR` (upstream dereferences the `NULL`). Only the NURB changes, as
//!   upstream swaps `m_pNurb` (the analytic data stays). The tolerance is
//!   `SmTol::GetApproxTol3d(pCurve)`: half the recorded edge tolerance
//!   ([`Curve::owner_zone_tol_3d`]), else half of [`zone_tol_3d_of_curve`].

use core::f64::consts::TAU;

use crate::curve::curve::{Curve, CurveKind};
use crate::curve::ellipse::Ellipse;
use crate::curve::line::Line;
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::tol::zone_tol_3d_of_curve;
use crate::geom::vector3d::{is_valid_double, Point3, Vec3};
use crate::kernel::Kernel;
use crate::limits::try_reserve_total;
use crate::math::{BIG_DOUBLE, EFF_ZERO, PI};
use crate::nurbs::b_spline_curve::{
    BSplineCurve, BSplineCurveForm, CanonicalData, KnotType, NurbCircleParam,
};
use crate::nurbs::b_spline_curve_n_lib::HelixSpec;
use crate::nurbs::crv_fit::p1::ParamMethod;

/// Upstream `SmStatus` value `SM_ERR_NOT_WITHIN_TOLERANCE`, as carried by
/// [`SolidError::Upstream`].
pub const SM_ERR_NOT_WITHIN_TOLERANCE: i32 = 1021;

const OP_LINE: &str = "primitive::api_curves::create_line";
const OP_CIRCLE: &str = "primitive::api_curves::create_circle";
const OP_ANALYTIC_CIRCLE: &str = "primitive::api_curves::create_analytic_circle";
const OP_ELLIPSE: &str = "primitive::api_curves::create_ellipse";
const OP_HELIX: &str = "primitive::api_curves::create_helix";
const OP_RECTANGLE: &str = "primitive::api_curves::create_rectangle";
const OP_POLYGON: &str = "primitive::api_curves::create_regular_polygon";
const OP_CURVE: &str = "primitive::api_curves::create_curve";

/// `RANGE_ER(low, mid, high)`: `mid` within `[low, high]` (NaN fails).
fn in_range(low: f64, mid: f64, high: f64) -> bool {
    mid >= low && mid <= high
}

/// The canonical XY frame at `origin` (`SmAxis2Placement` default constructor, then
/// `SetCanonical(origin, (1,0,0), (0,1,0))`, status ignored as upstream).
fn xy_frame(origin: Point3) -> Axis2Placement {
    let mut frame = Axis2Placement::default();
    let _ = frame.set_canonical(origin, Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
    frame
}

/// The analytic line segment from `start` to `end` over `[0, 1]`, dimension 3
/// (`SmApiCreateLineSegment`).
///
/// # Errors
///
/// Those of [`Line::create_line_segment`] (`InvalidInput` for coincident points).
pub fn create_line_segment(start: Point3, end: Point3) -> Result<Line> {
    Line::create_line_segment(3, start, end, None)
}

/// The infinite analytic line through `start` along `direction` (`SmApiCreateLine`).
///
/// # Errors
///
/// `InvalidInput` for a direction with a non-finite or out-of-range component, or
/// of length below `SM_EFF_ZERO`.
pub fn create_line(start: Point3, direction: Vec3) -> Result<Line> {
    if !is_valid_double(direction.x)
        || !is_valid_double(direction.y)
        || !is_valid_double(direction.z)
        || direction.is_zero(EFF_ZERO)
    {
        return Err(SolidError::invalid_input(OP_LINE, "invalid direction"));
    }

    Ok(Line::create_canonical(start, direction))
}

/// `SmApiConicInputsValid`: a circle or ellipse at `center` with `radius_x` along X
/// and `radius_y` along Y is valid input: finite, in-range center and radii, radii
/// above `SM_EFF_ZERO`, and representable extents that do not collapse onto the
/// center.
fn conic_inputs_valid(center: Point3, radius_x: f64, radius_y: f64) -> bool {
    if center.is_undef() {
        return false;
    }

    // Center coordinates and radius must be valid, in-range doubles.
    if !is_valid_double(center.x)
        || !is_valid_double(center.y)
        || !is_valid_double(center.z)
        || !is_valid_double(radius_x)
        || !is_valid_double(radius_y)
    {
        return false;
    }

    // Explicit minimum radius, matching SmCircle::CreateCanonical.
    if radius_x <= EFF_ZERO || radius_y <= EFF_ZERO {
        return false;
    }

    // Derived extents must be representable and must not collapse onto the
    // center in double precision (which would yield a zero-length circle).
    let extent_x_hi = center.x + radius_x;
    let extent_x_lo = center.x - radius_x;
    let extent_y_hi = center.y + radius_y;
    let extent_y_lo = center.y - radius_y;
    if !is_valid_double(extent_x_hi)
        || !is_valid_double(extent_x_lo)
        || !is_valid_double(extent_y_hi)
        || !is_valid_double(extent_y_lo)
    {
        return false;
    }
    if extent_x_hi == center.x
        || extent_x_lo == center.x
        || extent_y_hi == center.y
        || extent_y_lo == center.y
    {
        return false;
    }

    true
}

/// `SmApiCircleInputsValid`.
fn circle_inputs_valid(center: Point3, radius: f64) -> bool {
    conic_inputs_valid(center, radius, radius)
}

/// The full quadratic B-spline circle of `radius` about `center` in the XY plane
/// (`SmApiCreateCircle`, B-spline overload).
///
/// # Errors
///
/// `InvalidInput` when `SmApiCircleInputsValid` rejects the input; those of
/// [`BSplineCurve::create_circle_segment`].
pub fn create_circle(center: Point3, radius: f64) -> Result<BSplineCurve> {
    if !circle_inputs_valid(center, radius) {
        return Err(SolidError::invalid_input(OP_CIRCLE, "invalid circle"));
    }

    let frame = xy_frame(center);
    BSplineCurve::create_circle_segment(3, &frame, radius, 0.0, 360.0, NurbCircleParam::Quadratic)
}

/// The analytic circle of `radius` about `center` in the XY plane
/// (`SmApiCreateCircle`, `SmCircle` overload).
///
/// # Errors
///
/// `InvalidInput` when `SmApiCircleInputsValid` rejects the input; those of
/// [`Ellipse::create_canonical_circle`].
pub fn create_analytic_circle(k: &Kernel, center: Point3, radius: f64) -> Result<Ellipse> {
    if !circle_inputs_valid(center, radius) {
        return Err(SolidError::invalid_input(
            OP_ANALYTIC_CIRCLE,
            "invalid circle",
        ));
    }

    let frame = xy_frame(center);
    Ellipse::create_canonical_circle(k, &frame, radius, None, None)
}

/// The analytic ellipse about `origin` in the XY plane with `radius1` along X and
/// `radius2` along Y (`SmApiCreateEllipse`).
///
/// # Errors
///
/// `InvalidInput` when `SmApiConicInputsValid` rejects the input; those of
/// [`Ellipse::create_canonical`].
pub fn create_ellipse(k: &Kernel, origin: Point3, radius1: f64, radius2: f64) -> Result<Ellipse> {
    if !conic_inputs_valid(origin, radius1, radius2) {
        return Err(SolidError::invalid_input(OP_ELLIPSE, "invalid ellipse"));
    }

    let frame = xy_frame(origin);
    Ellipse::create_canonical(k, &frame, radius1, radius2, None, None)
}

/// The quadratic B-spline arc of `radius` about `center` in the XY plane from
/// `start_deg` to `end_deg` (`SmApiCreateArc`). The input is not validated here.
///
/// # Errors
///
/// Those of [`BSplineCurve::create_circle_segment`].
pub fn create_arc(
    center: Point3,
    radius: f64,
    start_deg: f64,
    end_deg: f64,
) -> Result<BSplineCurve> {
    let frame = xy_frame(center);
    BSplineCurve::create_circle_segment(
        3,
        &frame,
        radius,
        start_deg,
        end_deg,
        NurbCircleParam::Quadratic,
    )
}

/// The inputs of [`create_helix`] (`SmApiCreateHelix`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HelixParams {
    /// Base point; the helix winds about the axis through it parallel to +Z and
    /// starts on the +X side.
    pub origin: Point3,
    /// Axial length along +Z, in `[1e-7, 1e18]`.
    pub height: f64,
    /// Radius at the base, in `[1e-7, 1e18]`.
    pub radius_start: f64,
    /// Radius at `height`, in `[1e-7, 1e18]`.
    pub radius_end: f64,
    /// Number of 360-degree turns, in `[1e-7, 124.25]`.
    pub num_turns: f64,
    /// Right-handed (`TRUE`) or left-handed winding.
    pub right_handed: bool,
    /// Largest allowed 3D deviation from the exact helix, in `[1e-6, 1e18]`.
    pub tolerance: f64,
}

/// A cubic B-spline helix (`SmApiCreateHelix`): the input domain is validated, the
/// helix is fitted by [`BSplineCurve::create_helix_segment`] and its deviation from
/// the analytic helix is measured at `max(1024, min(20000, 200*turns + 16)) + 1`
/// parameters, each curve point paired with the helix point at the same height.
///
/// # Errors
///
/// `InvalidInput` for an input outside the documented domain (also NaN and
/// infinities) or derived extents beyond `±1e18`; those of
/// [`BSplineCurve::create_helix_segment`]; `SM_ERR` when a point evaluation fails;
/// `SM_ERR_NOT_WITHIN_TOLERANCE` when the measured deviation exceeds `tolerance`.
pub fn create_helix(params: &HelixParams) -> Result<BSplineCurve> {
    let HelixParams {
        origin,
        height,
        radius_start,
        radius_end,
        num_turns,
        right_handed,
        tolerance,
    } = *params;
    let bad = || SolidError::invalid_input(OP_HELIX, "helix input outside its domain");

    let min_height = 1.0e-7; // model tol; height must be positive
    let min_radius = 1.0e-7; // reject zero/negative radii (both-zero is degenerate)
    let min_turns = 1.0e-7; // fitter requires NumTurns >= NL_MTOL
    let min_tolerance = 1.0e-6; // effective floor of the fit tolerance
    let max_magnitude = BIG_DOUBLE / 100.0; // matches SM_IS_VALID_DOUBLE's finite cap (1e18)
    let max_spiral_spans = 994.0; // NL_CCPLIM (1000) - 6, the fitter's control-point cap
    let max_turns = max_spiral_spans / 8.0; // seed spans = 8*turns must stay <= cap (~124.25)

    // origin.is_undef() rejects NaN / +/-Inf / undef in any component.
    if origin.is_undef() {
        return Err(bad());
    }

    if !in_range(min_height, height, max_magnitude)
        || !in_range(min_radius, radius_start, max_magnitude)
        || !in_range(min_radius, radius_end, max_magnitude)
        || !in_range(min_turns, num_turns, max_turns)
        || !in_range(min_tolerance, tolerance, max_magnitude)
    {
        return Err(bad());
    }

    // Reject inputs whose derived extents are not finite before any placement math.
    let max_radius = if radius_start > radius_end {
        radius_start
    } else {
        radius_end
    };
    if !is_valid_double(origin.z + height)
        || !is_valid_double(origin.x + max_radius)
        || !is_valid_double(origin.x - max_radius)
        || !is_valid_double(origin.y + max_radius)
        || !is_valid_double(origin.y - max_radius)
    {
        return Err(bad());
    }

    let frame = xy_frame(origin);
    let helix = BSplineCurve::create_helix_segment(&HelixSpec {
        frame,
        height,
        radius_start,
        radius_end,
        num_turns,
        right_handed,
        approx_tol: tolerance,
    })?;

    // Enforce tolerance as a hard contract: measure the 3D deviation of the
    // fitted curve from the analytic helix.
    let two_pi_turns = TAU * num_turns; // total sweep angle
    let rl = if right_handed { 1.0 } else { -1.0 }; // right/left winding about +Z
    let ivl = helix.natural_interval();
    let param_min = ivl.min();
    let param_span = ivl.max() - param_min;

    // Dense sampling: enough points per fitter span.
    // `(long)(200.0 * dNumTurns)`: truncation toward zero (small and positive here).
    // The two upstream bound checks are an integer clamp to [1024, 20000].
    let num_samples = ((200.0 * num_turns) as i64 + 16).clamp(1024, 20000);

    let mut max_dev = 0.0;
    for i in 0..=num_samples {
        let u = param_min + param_span * (i as f64 / num_samples as f64);
        let on_curve = helix.evaluate_point(u).map_err(|e| {
            if e.is_unported() {
                e
            } else {
                SolidError::sm_err(OP_HELIX)
            }
        })?;

        // Pair each curve point with the analytic helix point at the same
        // axial height.
        let mut theta = ((on_curve.z - origin.z) * two_pi_turns) / height;
        if theta < 0.0 {
            theta = 0.0;
        }
        if theta > two_pi_turns {
            theta = two_pi_turns;
        }

        let radius = (radius_start * (two_pi_turns - theta) + radius_end * theta) / two_pi_turns;
        let ax = origin.x + radius * theta.cos();
        let ay = origin.y + rl * radius * theta.sin();
        let az = origin.z + height * theta / two_pi_turns;

        let ddx = on_curve.x - ax;
        let ddy = on_curve.y - ay;
        let ddz = on_curve.z - az;
        let dev = (ddx * ddx + ddy * ddy + ddz * ddz).sqrt();
        if dev > max_dev {
            max_dev = dev;
        }
    }

    if max_dev > tolerance {
        return Err(SolidError::Upstream {
            operation: OP_HELIX,
            code: SM_ERR_NOT_WITHIN_TOLERANCE,
            entity: None,
        });
    }

    Ok(helix)
}

/// The rectangle of `length` (X) by `width` (Y) centred on `center` in its XY
/// plane, as four line segments counter-clockwise from the `(-x, -y)` corner
/// (`SmApiCreateRectangle`).
///
/// # Errors
///
/// `InvalidInput` for a size not above zero (or NaN) or a corner coordinate that is
/// not a valid double; `SM_ERR` when a segment cannot be made (corners that
/// coincide in double precision).
pub fn create_rectangle(center: Point3, length: f64, width: f64) -> Result<Vec<Line>> {
    // Negated test so a NaN size is rejected too.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if !(length > 0.0) || !(width > 0.0) {
        return Err(SolidError::invalid_input(
            OP_RECTANGLE,
            "size not above zero",
        ));
    }

    let half_l = length * 0.5;
    let half_w = width * 0.5;

    let corners = [
        Point3::new(center.x - half_l, center.y - half_w, center.z),
        Point3::new(center.x + half_l, center.y - half_w, center.z),
        Point3::new(center.x + half_l, center.y + half_w, center.z),
        Point3::new(center.x - half_l, center.y + half_w, center.z),
    ];

    // Rejects a NaN or infinite center or size, and corners outside the
    // valid double range.
    for c in &corners {
        if !is_valid_double(c.x) || !is_valid_double(c.y) || !is_valid_double(c.z) {
            return Err(SolidError::invalid_input(OP_RECTANGLE, "invalid corner"));
        }
    }

    segments(&corners, OP_RECTANGLE)
}

/// The closed polygon through `points` (segment `i` from point `i` to point
/// `i + 1`, wrapping), each through `SmLine::CreateLineSegment`; `SM_ERR` on the
/// first failure.
fn segments(points: &[Point3], operation: &'static str) -> Result<Vec<Line>> {
    let mut result = Vec::with_capacity(points.len());
    for (ii, &p0) in points.iter().enumerate() {
        let p1 = points
            .get((ii + 1) % points.len())
            .copied()
            .ok_or_else(|| SolidError::sm_err(operation))?;
        match Line::create_line_segment(3, p0, p1, None) {
            Ok(seg) => result.push(seg),
            Err(e) if e.is_unported() => return Err(e),
            // Drop the segments this call already made.
            Err(_) => return Err(SolidError::sm_err(operation)),
        }
    }
    Ok(result)
}

/// The regular polygon with `num_sides` vertices on the circle of `radius` about
/// `center` in its XY plane, starting on +X, as line segments
/// (`SmApiCreateRegularPolygon`).
///
/// # Errors
///
/// `InvalidInput` for fewer than 3 sides, a radius not above zero (or NaN), a
/// center / extent that is not a valid double, or more sides than fit in memory or in
/// [`crate::limits::MAX_ALLOC_BYTES`];
/// `SM_ERR` when a segment cannot be
/// made (vertices that coincide in double precision).
pub fn create_regular_polygon(center: Point3, num_sides: u64, radius: f64) -> Result<Vec<Line>> {
    // Negated test so a NaN radius is rejected too.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if num_sides < 3 || !(radius > 0.0) {
        return Err(SolidError::invalid_input(
            OP_POLYGON,
            "bad side count or radius",
        ));
    }

    // Rejects a NaN or infinite center or radius, and vertices outside the
    // valid double range.
    if !is_valid_double(center.x - radius)
        || !is_valid_double(center.x + radius)
        || !is_valid_double(center.y - radius)
        || !is_valid_double(center.y + radius)
        || !is_valid_double(center.z)
    {
        return Err(SolidError::invalid_input(
            OP_POLYGON,
            "invalid center or radius",
        ));
    }

    let angle_step = 2.0 * PI / num_sides as f64;

    // Upstream appends every segment, whatever the count, until memory runs out;
    // reserve the whole result fallibly (at most `MAX_ALLOC_BYTES`) so an absurd side
    // count is an error.
    let too_many = || SolidError::invalid_input(OP_POLYGON, "too many sides for memory");
    let mut result = Vec::new();
    if !try_reserve_total(
        &mut result,
        usize::try_from(num_sides).map_err(|_| too_many())?,
    ) {
        return Err(too_many());
    }
    for ii in 0..num_sides {
        let a0 = angle_step * ii as f64;
        let a1 = angle_step * (ii + 1) as f64;

        let pt0 = Point3::new(
            center.x + radius * a0.cos(),
            center.y + radius * a0.sin(),
            center.z,
        );
        let pt1 = Point3::new(
            center.x + radius * a1.cos(),
            center.y + radius * a1.sin(),
            center.z,
        );

        match Line::create_line_segment(3, pt0, pt1, None) {
            Ok(seg) => result.push(seg),
            Err(e) if e.is_unported() => return Err(e),
            // Drop the segments this call already made.
            Err(_) => return Err(SolidError::sm_err(OP_POLYGON)),
        }
    }

    Ok(result)
}

/// The cubic B-spline with control points `points`, clamped end knots 0 and 1 and
/// interior knots `i / (n - 2)` for `i` in `1..n - 3` (`n` points)
/// (`SmApiCreateCurve`).
///
/// # Errors
///
/// `InvalidInput` for fewer than 4 points; those of
/// [`BSplineCurve::create_canonical`].
pub fn create_curve(points: &[Point3]) -> Result<BSplineCurve> {
    let degree: usize = 3;
    let num_control_points = points.len();

    // A cubic needs degree+1 control points; fewer would underflow num_knots.
    if num_control_points < degree + 1 {
        return Err(SolidError::invalid_input(OP_CURVE, "fewer than 4 points"));
    }

    let num_knots = num_control_points - degree + 1;

    let mut knots = Vec::with_capacity(num_knots);
    knots.push(0.0);
    for ii in 1..num_knots - 1 {
        knots.push(ii as f64 / num_knots as f64);
    }
    knots.push(1.0);

    let mut knot_mult = Vec::with_capacity(num_knots);
    knot_mult.push(degree + 1);
    knot_mult.extend(core::iter::repeat_n(1, num_knots - 2));
    knot_mult.push(degree + 1);

    BSplineCurve::create_canonical(&CanonicalData {
        dim: 3,
        degree: 3,
        control_points: points,
        form: BSplineCurveForm::Unspecified,
        knot_multiplicities: &knot_mult,
        knots: &knots,
        knot_type: KnotType::Unspecified,
        weights: None,
        trim: None,
    })
}

/// The inputs of [`create_canonical_curve`] (`SmApiCreateCanonicalCurve`).
#[derive(Debug, Clone, Copy)]
pub struct CanonicalCurveParams<'a> {
    /// Euclidean control points.
    pub points: &'a [Point3],
    /// Unique knots in ascending order.
    pub knots: &'a [f64],
    /// Multiplicity of each knot: `degree + 1` at the ends, at most `degree` inside.
    pub knot_multiplicities: &'a [usize],
    /// Optional weight of each control point.
    pub weights: Option<&'a [f64]>,
    /// Degree.
    pub degree: usize,
    /// STEP form of the new curve.
    pub form: BSplineCurveForm,
}

/// The 3D B-spline from STEP-style data with unspecified knot type
/// (`SmApiCreateCanonicalCurve`, through [`BSplineCurve::create_canonical`]).
///
/// # Errors
///
/// Those of [`BSplineCurve::create_canonical`].
pub fn create_canonical_curve(params: &CanonicalCurveParams<'_>) -> Result<BSplineCurve> {
    BSplineCurve::create_canonical(&CanonicalData {
        dim: 3,
        degree: params.degree,
        control_points: params.points,
        form: params.form,
        knot_multiplicities: params.knot_multiplicities,
        knots: params.knots,
        knot_type: KnotType::Unspecified,
        weights: params.weights,
        trim: None,
    })
}

/// A cubic 3D B-spline approximating `points` with uniform parameters
/// (`SmApiCreateCurveApproxPoints`, through [`BSplineCurve::create_approximating_curve`]):
/// 4 control points, `n / 2` for more than 8 points, `n / 4` for more than 16.
///
/// # Errors
///
/// Those of [`BSplineCurve::create_approximating_curve`].
pub fn create_curve_approx_points(points: &[Point3]) -> Result<BSplineCurve> {
    // Determine number of points -- This algorithm can be improved
    let mut max_cntrl_pts = 4; // min number of points
    if points.len() > 8 {
        max_cntrl_pts = points.len() / 2; // increase relative to number of input points
    }
    if points.len() > 16 {
        max_cntrl_pts = points.len() / 4; // increase relative to number of input points
    }

    BSplineCurve::create_approximating_curve(
        max_cntrl_pts,
        ParamMethod::Uniform,
        3,
        3,
        points,
        &[],
        None,
    )
}

/// Removes the knots of a curve's NURB that can go within its approximation tolerance,
/// keeping no end derivatives (`SmApiRemoveCurveKnots`, through
/// [`BSplineCurve::remove_knots`]); a curve that is not an `SmBSplineCurve` upstream is
/// left as it is.
///
/// # Errors
///
/// `SM_ERR` when the curve has (and gets) no NURB or the removal fails; those of the
/// tolerance ([`zone_tol_3d_of_curve`]) and of making the NURB.
pub fn remove_curve_knots(k: &Kernel, curve: &mut Curve) -> Result<()> {
    // double dTol = SmTol::GetApproxTol3d(pCurve);
    let tol = match curve.owner_zone_tol_3d() {
        Some(zone_tol_3d) => 0.5 * zone_tol_3d,
        None => 0.5 * zone_tol_3d_of_curve(k, curve)?,
    };
    let owner = curve.owner();
    // GetOrCreateGwNurbPointer: the virtual MakeNurb when there is no NURB (status
    // ignored)
    let base = match curve.kind_mut() {
        CurveKind::BSpline(c) => c,
        CurveKind::Line(l) => {
            if l.base().nurb.is_none() {
                ignore_status(l.make_nurb_owned(owner))?;
            }
            l.base_mut()
        }
        CurveKind::Ellipse(e) => {
            if e.base().nurb.is_none() {
                ignore_status(e.make_nurb_owned(k, owner))?;
            }
            e.base_mut()
        }
        CurveKind::Hyperbola(h) => {
            if h.base().nurb.is_none() {
                ignore_status(h.make_nurb(owner))?;
            }
            h.base_mut()
        }
        CurveKind::Parabola(p) => {
            if p.base().nurb.is_none() {
                ignore_status(p.make_nurb(owner))?;
            }
            p.base_mut()
        }
        // not an SmBSplineCurve: SM_SUCCESS
        _ => return Ok(()),
    };
    base.remove_knots(tol, false, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SM_ERR;

    /// `TestSmRemoveCurveKnots` (`tests/SM_API_test/src/TestSmCurves.cpp:1122`): the
    /// interior knot of a straight degree 1 polyline goes, leaving the four clamped end
    /// knots; a bent one stays.
    #[test]
    fn remove_curve_knots_of_a_straight_polyline() {
        let k = Kernel::default();
        let make = |y: f64| {
            Curve::from(
                create_canonical_curve(&CanonicalCurveParams {
                    points: &[p(0.0, 0.0, 0.0), p(1.0, y, 0.0), p(2.0, 0.0, 0.0)],
                    knots: &[0.0, 0.5, 1.0],
                    knot_multiplicities: &[2, 1, 2],
                    weights: None,
                    degree: 1,
                    form: BSplineCurveForm::Unspecified,
                })
                .unwrap(),
            )
        };
        let knot_count = |c: &Curve| match c.kind() {
            CurveKind::BSpline(b) => b.gw_nurb().unwrap().knots().len(),
            _ => panic!("not a B-spline"),
        };
        let mut straight = make(0.0);
        remove_curve_knots(&k, &mut straight).unwrap();
        assert_eq!(knot_count(&straight), 4);
        let mut bent = make(1.0);
        remove_curve_knots(&k, &mut bent).unwrap();
        assert_eq!(knot_count(&bent), 5);
    }

    fn p(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    #[test]
    fn line_segment_and_zero_direction() {
        assert!(create_line_segment(p(0.0, 0.0, 0.0), p(10.0, 10.0, 0.0)).is_ok());
        assert!(matches!(
            create_line(p(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0)),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(create_line(p(0.0, 0.0, 0.0), Vec3::new(1.0, 2.0, 0.0)).is_ok());
    }

    #[test]
    fn ellipse_radius_checks() {
        let k = Kernel::default();
        assert!(create_ellipse(&k, p(0.0, 0.0, 0.0), -2.0, 1.0).is_err());
        let far = p(0.0, 1.0e16, 0.0);
        assert!(create_ellipse(&k, far, 0.5, 1.0e10).is_ok());
        assert!(matches!(
            create_ellipse(&k, far, 1.0e10, 0.5),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn circle_bounds() {
        let k = Kernel::default();
        let c = p(0.0, 0.0, 0.0);
        assert!(create_circle(c, EFF_ZERO).is_err());
        assert!(create_analytic_circle(&k, c, EFF_ZERO).is_err());
        assert!(create_circle(c, 2.0 * EFF_ZERO).is_ok());
        assert!(create_analytic_circle(&k, c, 2.0 * EFF_ZERO).is_ok());
        assert!(create_circle(p(1.0e18, 0.0, 0.0), 1.0).is_err());
        assert!(create_circle(c, 10.0).is_ok());
        assert!(create_arc(c, 10.0, 0.0, 135.0).is_ok());
    }

    #[test]
    fn rectangle_and_polygon() {
        let c = p(0.0, 0.0, 0.0);
        assert_eq!(
            create_rectangle(c, 10.0, 6.0).map(|v| v.len()).ok(),
            Some(4)
        );
        assert!(matches!(
            create_rectangle(p(0.0, 1.0e10, 0.0), 10.0, 1.0e-9),
            Err(SolidError::Upstream { code: SM_ERR, .. })
        ));
        assert!(matches!(
            create_rectangle(c, f64::NAN, 6.0),
            Err(SolidError::InvalidInput { .. })
        ));
        assert_eq!(
            create_regular_polygon(c, 6, 5.0).map(|v| v.len()).ok(),
            Some(6)
        );
        assert!(matches!(
            create_regular_polygon(p(1.0e10, 1.0e10, 0.0), 5, 1.5e-6),
            Err(SolidError::Upstream { code: SM_ERR, .. })
        ));
        assert!(create_regular_polygon(c, 6, f64::INFINITY).is_err());
    }

    #[test]
    fn curves() {
        let pts = [
            p(0.0, 0.0, 0.0),
            p(10.0, -3.0, 0.0),
            p(20.0, 3.0, 0.0),
            p(30.0, -6.0, 0.0),
        ];
        let c = create_curve(&pts).unwrap();
        assert_eq!(c.knot_type(), KnotType::Unspecified);
        for keep in 0..4 {
            assert!(create_curve(&pts[..keep]).is_err());
        }
        let two = [p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0)];
        assert!(create_canonical_curve(&CanonicalCurveParams {
            points: &two,
            knots: &[0.0, 1.0],
            knot_multiplicities: &[4, 4],
            weights: None,
            degree: 3,
            form: BSplineCurveForm::Unspecified,
        })
        .is_err());
    }

    #[test]
    fn helix_valid_and_invalid() {
        let base = HelixParams {
            origin: p(0.0, 0.0, 0.0),
            height: 10.0,
            radius_start: 2.0,
            radius_end: 2.0,
            num_turns: 5.0,
            right_handed: true,
            tolerance: 1.0e-3,
        };
        assert!(create_helix(&base).is_ok());
        for bad in [
            HelixParams {
                height: 0.0,
                ..base
            },
            HelixParams {
                radius_start: f64::NAN,
                ..base
            },
            HelixParams {
                num_turns: 125.0,
                ..base
            },
            HelixParams {
                tolerance: 1.0e-9,
                ..base
            },
            HelixParams {
                origin: p(1.0e19, 0.0, 0.0),
                ..base
            },
        ] {
            assert!(matches!(
                create_helix(&bad),
                Err(SolidError::InvalidInput { .. })
            ));
        }
    }

    #[test]
    fn polygon_with_absurd_side_count_is_an_error() {
        // fuzz regression (primitive.api_curves fuzz00060): 2^32 sides exhausted
        // memory segment by segment
        let c = p(1.0, 2.0, 3.0);
        // (beyond the 47-bit address space, so no test machine can grant it)
        assert!(create_regular_polygon(c, 1 << 47, 5.0).is_err());
        assert!(create_regular_polygon(c, u64::MAX, 5.0).is_err());
    }
}
