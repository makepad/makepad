//! Geometric utilities, part 2: triangle and segment distances, segment/segment
//! intersections (direct, projected, perspective), circle/plane, line/ellipse,
//! sphere-circle/sphere-circle and plane/plane intersections, the best-fit plane,
//! 4x4 matrix product and inverse, Frenet frames and curve continuity.
//!
//! Upstream: `source/SMLib/src/SmGeomUtility.cpp`, `source/SMLib/inc/SmGeomUtility.h`
//! (enums from `source/SMLib/inc/SmTypes.h`).
//!
//! | Rust                                     | Upstream                                   |
//! |------------------------------------------|--------------------------------------------|
//! | [`PointSetType`]                         | `SmPointSetType`                           |
//! | [`ContinuityType`]                       | `SmContinuityType`                         |
//! | [`CurveEval`]                            | (inputs of `smgu_EvaluateCurveContinuity`) |
//! | [`PerspectivePlane`]                     | (inputs of `smgu_SegmentSegmentPerpspectiveIntersect`) |
//! | [`CoplanarEllipse`]                      | (inputs of `smgu_LineCoPlanarEllipseIntersect`) |
//! | [`triangle_point_distance`]              | `smgu_TrianglePointDistance`               |
//! | [`line_line_at_distance`]                | `smgu_LineLineAtDistance`                  |
//! | [`is_point_on_segment`]                  | `smgu_IsPointOnSegment`                    |
//! | [`segment_segment_projected_intersect`]  | `smgu_SegmentSegmentProjectedIntersect`    |
//! | [`segment_segment_perspective_intersect`]| `smgu_SegmentSegmentPerpspectiveIntersect` |
//! | [`segment_segment_intersect`]            | `smgu_SegmentSegmentIntersect`             |
//! | [`segment_segment_closest_point`]        | `smgu_SegmentSegmentClosestPoint`          |
//! | [`circle_plane_intersect`]               | `smgu_CirclePlaneIntersect`                |
//! | [`line_coplanar_ellipse_intersect`]      | `smgu_LineCoPlanarEllipseIntersect`        |
//! | [`sphere_circle_sphere_circle_intersect`]| `smgu_SphereCircleSphereCircleIntersect`   |
//! | [`intersect_two_planes`]                 | `smgu_IntersectTwoPlanes`                  |
//! | [`best_fit_plane_to_points`]             | `smgu_BestFitPlaneToPoints`                |
//! | [`mult_4x4`]                             | `smgu_4x4Mult`                             |
//! | [`inverse_4x4`]                          | `smgu_4x4Inverse`                          |
//! | [`frenet_from_derivatives`]              | `smgu_FrenetFromDerivatives`               |
//! | [`evaluate_curve_continuity`]            | `smgu_EvaluateCurveContinuity`             |
//! | [`evaluate_curve_continuity_with`], [`ContinuityOptions`] | not upstream: opt-in geometric G2 (WR-380) |
//!
//! Deviations:
//! - Out-params and `SmStatus` become return values. `SM_ERR` is
//!   [`SolidError::Upstream`] with code 1001, `SM_ERR_INVALID_INPUT` is
//!   [`SolidError::InvalidInput`]. Output slots upstream leaves unset are `0.0` here
//!   (and outside `count` in [`SegmentHits`]).
//! - `smgu_4x4Copy` (the copy into the caller's matrix) disappears: the 4x4 functions
//!   return their result by value. [`inverse_4x4`] builds each cofactor's 3x3 minor in
//!   a loop instead of spelling out the 16 `smgu_3x3Determinate` calls (same
//!   coefficients, same `-det / det` arithmetic).
//! - The projection plane and eye of `smgu_SegmentSegmentPerpspectiveIntersect` and
//!   the ellipse of `smgu_LineCoPlanarEllipseIntersect` are passed as
//!   [`PerspectivePlane`] / [`CoplanarEllipse`] structs instead of positional
//!   arguments.
//! - [`intersect_two_planes`] reports parallel planes as
//!   [`PlanesIntersection::Parallel`] (upstream: `SM_ERR` plus the optional
//!   coincidence flag, which is always computed here).
//! - [`segment_segment_closest_point`] with a degenerate segment also reports the
//!   point/segment distance in `distances[0]` (upstream leaves it unset).
//! - [`frenet_from_derivatives`] returns only the requested `n + 1` vectors; on error
//!   the partially written frame upstream leaves behind is not returned.
//! - [`evaluate_curve_continuity`] skips the two `smgu_FrenetFromDerivatives` calls
//!   whose outputs and statuses upstream never reads (the opt-in
//!   [`ContinuityOptions::geometric_g2`] makes and uses them, WR-380), and the debug-only (and unused)
//!   `sDist` / projected-line computations of other functions are omitted.
//! - Calls into `geom.geom_utility_p1` use [`p1`] directly, through small private
//!   adapters where the output shapes differ (`sm_AddClosestPoint` on a
//!   [`SegmentHits`], `smgu_LineIntersectSphere` as a count and array,
//!   `smgu_BestFitLineToPoints` with its matrix, `smgu_3x3Determinate` on a 3x3 array).

use crate::error::{ignore_status, Result, SolidError};
use crate::geom::geom_utility::p1::{
    self, add_point, determinant_3_vectors, determinate_4x4, intersect_three_planes,
    line_closest_point, line_coplanar_circle_intersect, line_line_closest_point,
    line_point_distance, point_perspective_to_plane, point_project_to_plane,
    segment_point_distance, vec_lin_comb_two_vectors,
};
use crate::geom::matrix::Matrix;
use crate::geom::tol::{in_tol, is_zero, tol3d, ZONE_TOL_3D};
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::math::{max2, max3, rad_to_deg, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};

/// Upstream `SM_ERR` (generic error status).
const SM_ERR: i32 = 1001;

/// `SM_ERR` from `operation`.
fn sm_err(operation: &'static str) -> SolidError {
    SolidError::Upstream {
        operation,
        code: SM_ERR,
        entity: None,
    }
}

// ---- types ---------------------------------------------------------------------------

/// Shape classification of a point set (`SmPointSetType`), defined with
/// `smgu_BestFitPointToPoints` in [`p1`].
pub use p1::PointSetType;

/// Continuity between two curve evaluations (`SmContinuityType`); discriminants are
/// the upstream values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContinuityType {
    /// `SM_CT_UNDEFINED`.
    Undefined = 0,
    /// `SM_CT_DISCONTINUOUS`: discontinuous position.
    Discontinuous = 1,
    /// `SM_CT_C0`: continuous position.
    C0 = 2,
    /// `SM_CT_G1`: continuous first-derivative direction.
    G1 = 3,
    /// `SM_CT_G1R`: G1-ruled.
    G1R = 4,
    /// `SM_CT_G1_G2`: G1 plus continuous curvature vector.
    G1G2 = 5,
    /// `SM_CT_G1_G2_G3`: G1, G2 and G3.
    G1G2G3 = 6,
    /// `SM_CT_C1`: continuous first derivative.
    C1 = 7,
    /// `SM_CT_C1_G2`: C1 plus continuous curvature vector.
    C1G2 = 8,
    /// `SM_CT_C1_G2_G3`: C1, G2 and G3.
    C1G2G3 = 9,
    /// `SM_CT_C1_C2`: C1 plus continuous second derivative.
    C1C2 = 10,
    /// `SM_CT_C1_C2_G3`: C1, C2 and G3.
    C1C2G3 = 11,
    /// `SM_CT_C1_C2_C3`: continuous up to the third derivative.
    C1C2C3 = 12,
    /// `SM_CT_CINFINITY`: unlimited continuity.
    CInfinity = 13,
}

/// Result of [`triangle_point_distance`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriangleDistance {
    /// Distance from the test point to the triangle.
    pub distance: f64,
    /// Closest point on the triangle.
    pub closest: Point3,
    /// `u` of `closest = p0 + u (p1 - p0) + v (p2 - p0)`.
    pub u: f64,
    /// `v` of `closest = p0 + u (p1 - p0) + v (p2 - p0)`.
    pub v: f64,
}

/// Result of [`is_point_on_segment`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointOnSegment {
    /// The point is within tolerance of the segment.
    pub on: bool,
    /// Point/segment distance.
    pub distance: f64,
    /// Parameter in `[0, 1]` of the nearest segment point.
    pub parameter: f64,
}

/// Up to two parameter pairs found on two segments (segment/segment intersections
/// and closest points). Entries at and beyond `count` are `0.0`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SegmentHits {
    /// 0 = none, 1 = one point, 2 = two points (coincident or parallel segments).
    pub count: usize,
    /// Normalised parameters on segment 1.
    pub params1: [f64; 2],
    /// Normalised parameters on segment 2.
    pub params2: [f64; 2],
    /// (Projected) distances at the hits.
    pub distances: [f64; 2],
}

impl SegmentHits {
    fn set(&mut self, i: usize, p1: f64, p2: f64, dist: f64) {
        if let (Some(a), Some(b), Some(d)) = (
            self.params1.get_mut(i),
            self.params2.get_mut(i),
            self.distances.get_mut(i),
        ) {
            *a = p1;
            *b = p2;
            *d = dist;
        }
    }
}

/// Result of [`circle_plane_intersect`] and [`sphere_circle_sphere_circle_intersect`].
#[derive(Debug, Clone, PartialEq)]
pub enum CurveIntersection {
    /// Zero, one or two intersection points (upstream count 0, 1, 2).
    Points(Vec<Point3>),
    /// The curves are coincident (upstream count 3).
    Coincident,
}

/// Result of [`intersect_two_planes`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlanesIntersection {
    /// The planes meet in the line `point + t * direction` (`direction` is a unit
    /// vector; `point` is where the line meets the plane through the midpoint of the
    /// two plane points).
    Line {
        /// Point on the line.
        point: Point3,
        /// Unit tangent of the line.
        direction: Vec3,
    },
    /// The planes are parallel (upstream `SM_ERR`).
    Parallel {
        /// Both plane points lie within the scaled zero of the other plane.
        coincident: bool,
    },
}

/// One curve evaluation for [`evaluate_curve_continuity`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveEval {
    /// Position.
    pub position: Point3,
    /// First derivative.
    pub first: Vec3,
    /// Second derivative.
    pub second: Vec3,
    /// Optional third derivative (upstream `pOpt3rd*`, `NULL` = `None`).
    pub third: Option<Vec3>,
}

/// Plane and eye point of a perspective projection
/// ([`segment_segment_perspective_intersect`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerspectivePlane {
    /// A point on the projection plane.
    pub point: Point3,
    /// Unit normal of the projection plane.
    pub unit_normal: Vec3,
    /// The eye point the segments are projected from.
    pub eye: Point3,
}

/// An ellipse `center + x_radius cos(s) x_axis + y_radius sin(s) y_axis` with unit,
/// orthogonal axes ([`line_coplanar_ellipse_intersect`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoplanarEllipse {
    /// Center.
    pub center: Point3,
    /// Unit major-parameter axis.
    pub x_axis: Vec3,
    /// Radius along `x_axis`.
    pub x_radius: f64,
    /// Unit axis orthogonal to `x_axis`.
    pub y_axis: Vec3,
    /// Radius along `y_axis`.
    pub y_radius: f64,
}

/// Result of [`best_fit_plane_to_points`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BestFitPlane {
    /// Classification of the point set.
    pub set_type: PointSetType,
    /// Centroid of the points (a point on the plane).
    pub point: Point3,
    /// Plane normal (`Planar`/`Scattered`), line tangent (`Linear`) or zero.
    pub normal: Vec3,
    /// Largest signed point/plane gap (point/line or point/centroid gap for the
    /// lower-dimensional classifications).
    pub max_plane_gap: f64,
    /// Largest point/centroid gap.
    pub max_centroid_gap: f64,
}

// ---- calls into geom.geom_utility_p1 ---------------------------------------------

/// Outputs of `smgu_BestFitLineToPoints` with its `M` matrix (written by upstream only
/// for scattered points, the only case [`best_fit_plane_to_points`] reads it).
struct BestFitLine {
    set_type: PointSetType,
    point: Point3,
    direction: Vec3,
    max_line_gap: f64,
    max_centroid_gap: f64,
    m: Matrix,
}

/// `smgu_BestFitLineToPoints` with `pOptM`.
fn best_fit_line_to_points(points: &[Point3], tol: f64) -> Result<BestFitLine> {
    let r = p1::best_fit_line_to_points(points, tol)?;
    Ok(BestFitLine {
        set_type: r.set_type,
        point: r.line_pt,
        direction: r.line_vec,
        max_line_gap: r.max_line_gap,
        max_centroid_gap: r.max_centroid_gap,
        m: r.m.unwrap_or_else(|| Matrix::new(3, 3)),
    })
}

/// `sm_AddClosestPoint` on the arrays of a [`SegmentHits`].
fn add_closest_point(dist: f64, param1: f64, param2: f64, tol: f64, hits: &mut SegmentHits) {
    let mut cp = p1::ClosestPoints {
        count: hits.count,
        params1: hits.params1,
        params2: hits.params2,
        distances: hits.distances,
    };
    cp.add(dist, param1, param2, tol);
    hits.count = cp.count;
    hits.params1 = cp.params1;
    hits.params2 = cp.params2;
    hits.distances = cp.distances;
}

/// `smgu_LineIntersectSphere` as upstream's `(rlNumberResults, adResults[3])`.
fn line_intersect_sphere(
    line_pnt: Point3,
    line_vec: Vec3,
    center: Point3,
    radius: f64,
    tol: f64,
) -> Result<(usize, [f64; 3])> {
    Ok(
        match p1::line_intersect_sphere(line_pnt, line_vec, center, radius, tol)? {
            p1::LineSphere::Miss => (0, [0.0; 3]),
            p1::LineSphere::Touch(t) => (1, [t, 0.0, 0.0]),
            p1::LineSphere::Through(t0, t1) => (2, [t0, t1, 0.0]),
            p1::LineSphere::Grazing(t0, t1, t2) => (3, [t0, t1, t2]),
        },
    )
}

/// `smgu_3x3Determinate` (coefficient overload) called with the nine entries of `m`
/// row by row.
fn determinant_3x3(m: &[[f64; 3]; 3]) -> f64 {
    let [[a00, a01, a02], [a10, a11, a12], [a20, a21, a22]] = *m;
    p1::determinate_3x3(a00, a01, a02, a10, a11, a12, a20, a21, a22)
}

// ---- distances -----------------------------------------------------------------------

/// Distance from `test` to the triangle `(p0, p1, p2)` and the closest triangle
/// point with its `(u, v)` coordinates w.r.t. `p0` and the edges to `p1` and `p2`
/// (`smgu_TrianglePointDistance`). A degenerate triangle is treated as the nearer
/// of its two edges from `p0`.
///
/// # Errors
///
/// Only propagates errors of its callees.
pub fn triangle_point_distance(
    p0: Point3,
    p1: Point3,
    p2: Point3,
    test: Point3,
) -> Result<TriangleDistance> {
    let vec_x = p1 - p0;
    let vec_y = p2 - p0;
    let test_vec = test - p0;

    let uv = match vec_lin_comb_two_vectors(vec_x, vec_y, test_vec) {
        Ok(uv) => uv,
        Err(e @ SolidError::Unported { .. }) => return Err(e),
        Err(_) => {
            // collinear points: nearest of the two edges from p0
            let (dist1, u) = segment_point_distance(p0, p1, test);
            let (dist2, v) = segment_point_distance(p0, p2, test);
            let (distance, closest) = if dist1 <= dist2 {
                (dist1, p0 + u * vec_x)
            } else {
                (dist2, p0 + v * vec_y)
            };
            return Ok(TriangleDistance {
                distance,
                closest,
                u,
                v,
            });
        }
    };

    let mut u = uv.x;
    let mut v = uv.y;
    if u <= 0.0 || v <= 0.0 {
        u = u.clamp(0.0, 1.0);
        v = v.clamp(0.0, 1.0);
    } else if u + v > 1.0 {
        // the distance is recomputed below
        let (_, t) = segment_point_distance(p1, p2, test);
        v = t;
        u = 1.0 - v;
    }

    let closest = p0 + u * vec_x + v * vec_y;
    Ok(TriangleDistance {
        distance: test.distance(closest),
        closest,
        u,
        v,
    })
}

/// The four parameter pairs on two lines whose points are `at_distance` apart with
/// equal angles to both lines (`smgu_LineLineAtDistance`): `(params1, params2)`.
///
/// # Errors
///
/// `SM_ERR` when the lines are parallel or further apart than `at_distance`;
/// [`SolidError::InvalidInput`] for a zero line vector.
pub fn line_line_at_distance(
    line_pnt1: Point3,
    line_vec1: Vec3,
    line_pnt2: Point3,
    line_vec2: Vec3,
    at_distance: f64,
) -> Result<([f64; 4], [f64; 4])> {
    const OP: &str = "geom::geom_utility::p2::line_line_at_distance";
    let (cl1, cl2) = match line_line_closest_point(line_pnt1, line_vec1, line_pnt2, line_vec2) {
        Ok(v) => v,
        Err(e @ SolidError::Unported { .. }) => return Err(e),
        Err(_) => return Err(sm_err(OP)),
    };
    let p1 = line_pnt1 + cl1 * line_vec1;
    let p2 = line_pnt2 + cl2 * line_vec2;
    let dist_between = p1.distance(p2);
    let mut beta = line_vec1.angle_between(line_vec2)?;
    beta /= 2.0;
    if dist_between > at_distance {
        return Err(sm_err(OP));
    }
    let half_proj = sqrt(at_distance * at_distance - dist_between * dist_between) / 2.0;
    let dist_along_a = half_proj / beta.sin();
    let dist_along_b = half_proj / (PI - beta).sin();
    let scale1 = 1.0 / line_vec1.length();
    let a_on1 = dist_along_a * scale1;
    let b_on1 = dist_along_b * scale1;
    let scale2 = 1.0 / line_vec2.length();
    let a_on2 = dist_along_a * scale2;
    let b_on2 = dist_along_b * scale2;

    Ok((
        [cl1 + a_on1, cl1 - a_on1, cl1 + b_on1, cl1 - b_on1],
        [cl2 + a_on2, cl2 - a_on2, cl2 + b_on2, cl2 - b_on2],
    ))
}

/// Whether `test` lies within `tolerance` of the segment `[start, end]`, with the
/// point/segment distance and the clamped parameter of the nearest segment point
/// (`smgu_IsPointOnSegment`).
///
/// # Errors
///
/// Only propagates errors of its callees.
pub fn is_point_on_segment(
    start: Point3,
    end: Point3,
    test: Point3,
    tolerance: f64,
) -> Result<PointOnSegment> {
    let (distance, parameter) = segment_point_distance(start, end, test);
    Ok(PointOnSegment {
        on: distance <= tolerance,
        distance,
        parameter,
    })
}

// ---- segment / segment ---------------------------------------------------------------

/// Shared tail of the projected and perspective segment intersections (upstream
/// duplicates it in both functions) on segments already mapped into one plane.
fn intersect_mapped_segments(
    start1: Point3,
    end1: Point3,
    start2: Point3,
    end2: Point3,
    tol: f64,
) -> Result<SegmentHits> {
    let tol2 = tol * tol;
    let mut hits = SegmentHits::default();

    if start1.distance_squared(end1) < tol2 {
        // curve 1 degenerates to a point
        if start2.distance_squared(end2) < tol2 {
            // both points: disjoint or fully coincident
            if start1.distance_squared(start2) > tol2 {
                return Ok(hits);
            }
            let d = start1.distance(start2);
            hits.count = 2;
            hits.set(0, 0.0, 0.0, d);
            hits.set(1, 1.0, 1.0, d);
            return Ok(hits);
        }
        let (dist, param) = segment_point_distance(start2, end2, start1);
        if dist > tol {
            return Ok(hits);
        }
        hits.count = 1;
        hits.set(0, 0.0, param, dist);
        return Ok(hits);
    }

    if start2.distance_squared(end2) < tol2 {
        // curve 2 degenerates to a point
        let (dist, param) = segment_point_distance(start1, end1, start2);
        if dist > tol {
            return Ok(hits);
        }
        hits.count = 1;
        hits.set(0, param, 0.0, dist);
        return Ok(hits);
    }

    // both are lines
    let points = segment_segment_intersect(start1, end1, start2, end2, tol)?;
    let Some(&first) = points.first() else {
        return Ok(hits);
    };
    let (_, param1) = segment_point_distance(start1, end1, first);
    let (dist, param2) = segment_point_distance(start2, end2, first);
    hits.count = 1;
    hits.set(0, param1, param2, dist);

    if let Some(&second) = points.get(1) {
        // coincident segments
        hits.count = 2;
        let (_, param1) = segment_point_distance(start1, end1, second);
        let (dist, param2) = segment_point_distance(start2, end2, second);
        hits.set(1, param1, param2, dist);
    }
    Ok(hits)
}

/// Intersects two segments after projecting them parallel to `plane_normal` (need
/// not be unit) onto the plane through `start1`
/// (`smgu_SegmentSegmentProjectedIntersect`). `count` 2 means coincident segments.
///
/// # Errors
///
/// Only propagates errors of its callees.
pub fn segment_segment_projected_intersect(
    start1: Point3,
    end1: Point3,
    start2: Point3,
    end2: Point3,
    plane_normal: Vec3,
    tolerance: f64,
) -> Result<SegmentHits> {
    let s1 = start1;
    let e1 = point_project_to_plane(end1, s1, plane_normal);
    let s2 = point_project_to_plane(start2, s1, plane_normal);
    let e2 = point_project_to_plane(end2, s1, plane_normal);
    intersect_mapped_segments(s1, e1, s2, e2, tolerance)
}

/// Intersects two segments after perspective-projecting them from `view.eye` onto
/// the plane `(view.point, view.unit_normal)`
/// (`smgu_SegmentSegmentPerpspectiveIntersect`). `count` 2 means coincident
/// segments.
///
/// # Errors
///
/// Only propagates errors of its callees.
pub fn segment_segment_perspective_intersect(
    start1: Point3,
    end1: Point3,
    start2: Point3,
    end2: Point3,
    view: &PerspectivePlane,
    tolerance: f64,
) -> Result<SegmentHits> {
    let project = |p| point_perspective_to_plane(p, view.point, view.unit_normal, view.eye);
    let s1 = project(start1);
    let e1 = project(end1);
    let s2 = project(start2);
    let e2 = project(end2);
    intersect_mapped_segments(s1, e1, s2, e2, tolerance)
}

/// Intersects two segments (`smgu_SegmentSegmentIntersect`): no point, one point, or
/// two points bounding a coincident stretch (near-coincident points lie on the first
/// segment). `tolerance` is raised to at least `SM_EFF_ZERO`.
///
/// # Errors
///
/// Only propagates errors of its callees.
pub fn segment_segment_intersect(
    line_start: Point3,
    line_end: Point3,
    other_start: Point3,
    other_end: Point3,
    tolerance: f64,
) -> Result<Vec<Point3>> {
    let tol = max2(tolerance, EFF_ZERO);
    let mut points: Vec<Point3> = Vec::with_capacity(2);

    // other start / end on this segment
    let on = is_point_on_segment(line_start, line_end, other_start, tol)?;
    if on.on {
        let pnt = line_start + on.parameter * (line_end - line_start);
        ignore_status(add_point(&mut points, tol, pnt))?;
    }
    let on = is_point_on_segment(line_start, line_end, other_end, tol)?;
    if on.on {
        let pnt = line_start + on.parameter * (line_end - line_start);
        ignore_status(add_point(&mut points, tol, pnt))?;
    }

    // this start / end on the other segment
    if is_point_on_segment(other_start, other_end, line_start, tol)?.on {
        ignore_status(add_point(&mut points, tol, line_start))?;
    }
    if is_point_on_segment(other_start, other_end, line_end, tol)?.on {
        ignore_status(add_point(&mut points, tol, line_end))?;
    }

    if points.len() > 1 {
        return Ok(points);
    }

    // interior intersection
    let line_vec = line_end - line_start;
    let other_vec = other_end - other_start;
    let (mut param1, mut param2) =
        match line_line_closest_point(line_start, line_vec, other_start, other_vec) {
            Ok(v) => v,
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            // parallel lines
            Err(_) => return Ok(points),
        };

    let mut trunc1 = false;
    if param1 < 0.0 {
        trunc1 = true;
        param1 = 0.0;
    }
    if param1 > 1.0 {
        trunc1 = true;
        param1 = 1.0;
    }
    let mut pnt = line_start + param1 * line_vec;
    if trunc1 {
        param2 = line_closest_point(other_start, other_vec, pnt);
    }

    let mut trunc2 = false;
    if param2 < 0.0 {
        trunc2 = true;
        param2 = 0.0;
    }
    if param2 > 1.0 {
        trunc2 = true;
        param2 = 1.0;
    }
    let pnt2 = other_start + param2 * other_vec;
    if trunc2 {
        param1 = line_closest_point(line_start, line_vec, pnt2);
        param1 = param1.clamp(0.0, 1.0);
        pnt = line_start + param1 * line_vec;
    }

    let dist = pnt.distance(pnt2);
    if dist > tol {
        return Ok(points);
    }

    if let Some(first) = points.first_mut() {
        // one end point already found: small coincident stretch or nearly parallel
        let dist_end_pt = pnt.distance(*first);
        if dist_end_pt > tol * 2.0 {
            let dist_int_pt = pnt.distance(pnt2);
            if dist_end_pt < dist_int_pt * 100.0 {
                points.push(pnt);
                return Ok(points);
            }
        }
        // use the analytic intersection instead of the end point
        *first = pnt;
        return Ok(points);
    }

    points.push(pnt);
    Ok(points)
}

/// Closest point(s) between two segments as normalised parameters
/// (`smgu_SegmentSegmentClosestPoint`); `count` 2 for parallel overlapping
/// segments.
///
/// # Errors
///
/// Only propagates errors of its callees.
pub fn segment_segment_closest_point(
    line_start: Point3,
    line_end: Point3,
    other_start: Point3,
    other_end: Point3,
    tolerance: f64,
) -> Result<SegmentHits> {
    let mut hits = SegmentHits::default();

    // degenerate segments: use their start
    let line_vec = line_end - line_start;
    if line_vec.length_squared() < EFF_ZERO_SQ {
        let (dist, param) = segment_point_distance(other_start, other_end, line_start);
        hits.count = 1;
        hits.set(0, 0.0, param, dist);
        return Ok(hits);
    }
    let other_vec = other_end - other_start;
    if other_vec.length_squared() < EFF_ZERO_SQ {
        let (dist, param) = segment_point_distance(line_start, line_end, other_start);
        hits.count = 1;
        hits.set(0, param, 0.0, dist);
        return Ok(hits);
    }

    let (dist, param) = segment_point_distance(line_start, line_end, other_start);
    add_closest_point(dist, param, 0.0, tolerance, &mut hits);
    let (dist, param) = segment_point_distance(line_start, line_end, other_end);
    add_closest_point(dist, param, 1.0, tolerance, &mut hits);
    let (dist, param) = segment_point_distance(other_start, other_end, line_start);
    add_closest_point(dist, 0.0, param, tolerance, &mut hits);
    let (dist, param) = segment_point_distance(other_start, other_end, line_end);
    add_closest_point(dist, 1.0, param, tolerance, &mut hits);

    // interior closest point
    let (mut param1, mut param2) =
        match line_line_closest_point(line_start, line_vec, other_start, other_vec) {
            Ok(v) => v,
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            // parallel lines
            Err(_) => return Ok(hits),
        };
    param1 = param1.clamp(0.0, 1.0);
    param2 = param2.clamp(0.0, 1.0);
    let pnt = line_start + param1 * line_vec;
    let pnt2 = other_start + param2 * other_vec;
    let dist = pnt.distance(pnt2);
    add_closest_point(dist, param1, param2, tolerance, &mut hits);
    Ok(hits)
}

// ---- circles, ellipses, spheres, planes ------------------------------------------------

/// Intersects the circle `(origin, unit normal, radius)` with the infinite plane
/// `(plane_point, unit plane_normal)` (`smgu_CirclePlaneIntersect`).
/// [`CurveIntersection::Coincident`] when the circle lies in the plane.
///
/// # Errors
///
/// Propagates [`intersect_two_planes`] (`SM_ERR_INVALID_INPUT` for non-unit
/// normals) and the line/circle intersection.
pub fn circle_plane_intersect(
    tolerance: f64,
    radius: f64,
    circle_origin: Point3,
    circle_normal: Vec3,
    plane_point: Point3,
    plane_normal: Vec3,
) -> Result<CurveIntersection> {
    const OP: &str = "geom::geom_utility::p2::circle_plane_intersect";
    let cross = circle_normal.cross(plane_normal);
    if cross.length_squared() < EFF_ZERO {
        let on_pt = point_project_to_plane(circle_origin, plane_point, plane_normal);
        return Ok(if on_pt.distance(circle_origin) < tolerance {
            CurveIntersection::Coincident
        } else {
            CurveIntersection::Points(Vec::new())
        });
    }

    let (line_pt, line_vec) =
        match intersect_two_planes(circle_origin, circle_normal, plane_point, plane_normal)? {
            PlanesIntersection::Line { point, direction } => (point, direction),
            PlanesIntersection::Parallel { .. } => return Err(sm_err(OP)),
        };

    let params = line_coplanar_circle_intersect(
        line_pt,
        line_vec,
        circle_origin,
        circle_normal,
        radius,
        tolerance,
    )?;
    Ok(CurveIntersection::Points(
        params.iter().map(|&t| line_pt + t * line_vec).collect(),
    ))
}

/// Intersects a line with a coplanar ellipse as line parameters
/// (`smgu_LineCoPlanarEllipseIntersect`): the problem is scaled along `x_axis` into
/// a line/circle intersection of radius `y_radius`.
///
/// # Errors
///
/// Only propagates errors of its callees.
pub fn line_coplanar_ellipse_intersect(
    line_pnt: Point3,
    line_vec: Vec3,
    ellipse: &CoplanarEllipse,
    tolerance: f64,
) -> Result<Vec<f64>> {
    let CoplanarEllipse {
        center,
        x_axis,
        x_radius,
        y_axis,
        y_radius,
    } = *ellipse;
    let normal = x_axis.cross(y_axis).unitize_or_z();

    // upstream also projects the line to the ellipse plane (always SM_SUCCESS; the
    // results are only used by debug code), which is omitted here

    // z = w + x * ((w-ctr).x) * (ry-rx)/rx maps the ellipse to a circle of radius ry
    let scale = (y_radius - x_radius) / x_radius;
    let t_line_pnt = line_pnt + x_axis * (line_pnt - center).dot(x_axis) * scale;
    let t_line_vec = line_vec + x_axis * line_vec.dot(x_axis) * scale;

    line_coplanar_circle_intersect(t_line_pnt, t_line_vec, center, normal, y_radius, tolerance)
}

/// Intersects two circles lying on one sphere, each given by its center and plane
/// normal (`smgu_SphereCircleSphereCircleIntersect`). Circles within `tol3d` of one
/// another without crossing give one point on circle 1;
/// [`CurveIntersection::Coincident`] for coincident circle planes.
///
/// # Errors
///
/// `SM_ERR` on an unexpected parallel-normal configuration; propagates errors of
/// its callees.
pub fn sphere_circle_sphere_circle_intersect(
    sphere_center: Point3,
    sphere_radius: f64,
    cir1_center: Point3,
    cir1_normal: Vec3,
    cir2_center: Point3,
    cir2_normal: Vec3,
    tol3d: f64,
) -> Result<CurveIntersection> {
    const OP: &str = "geom::geom_utility::p2::sphere_circle_sphere_circle_intersect";

    let planes = match intersect_two_planes(cir1_center, cir1_normal, cir2_center, cir2_normal) {
        Ok(PlanesIntersection::Line { point, direction }) => Some((point, direction)),
        Err(e @ SolidError::Unported { .. }) => return Err(e),
        Ok(PlanesIntersection::Parallel { .. }) | Err(_) => None,
    };
    let Some((line_pt, line_vec)) = planes else {
        // parallel planes: coincident or disjoint
        let dot = (cir1_center - cir2_center).dot(cir1_normal);
        return Ok(if dot.abs() < tol3d {
            CurveIntersection::Coincident
        } else {
            CurveIntersection::Points(Vec::new())
        });
    };

    let (mut count, mut params) =
        match line_intersect_sphere(line_pt, line_vec, sphere_center, sphere_radius, tol3d) {
            Ok(v) => v,
            Err(e @ SolidError::Unported { .. }) => return Err(e),
            // only for a zero line vector, which cannot happen here
            Err(_) => return Ok(CurveIntersection::Points(Vec::new())),
        };

    if count == 3 {
        // grazing: [first, mid, second] -> drop the mid point
        count = 2;
        params[1] = params[2];
    }

    if count == 0 || count == 1 {
        // tolerant check: nearest approach of the two circles in the plane of the
        // sphere center and both circle centers
        let mut dir1 = cir1_normal.normal_in_plane(cir2_normal);
        let mut dir2 = cir2_normal.normal_in_plane(cir1_normal);
        if dir1.length() <= EFF_ZERO || dir2.length() <= EFF_ZERO {
            // upstream: WARN("unexpected branch - needs review")
            return Err(sm_err(OP));
        }
        dir1 = dir1.unitize_or_z();
        dir2 = dir2.unitize_or_z();

        let r1_sq = sphere_radius * sphere_radius - cir1_center.distance_squared(sphere_center);
        let r2_sq = sphere_radius * sphere_radius - cir2_center.distance_squared(sphere_center);
        let r1 = if r1_sq >= 0.0 { sqrt(r1_sq) } else { 0.0 };
        let r2 = if r2_sq >= 0.0 { sqrt(r2_sq) } else { 0.0 };

        // closest of the four in-plane point pairs
        // (upstream also keeps the circle-2 point, only for an unused gap vector)
        let mut best_p1 = cir1_center + r1 * dir1;
        let mut best_len = best_p1.distance_squared(cir2_center + r2 * dir2);
        for ii in 1..4u32 {
            let s1 = if ii > 1 { -1.0 } else { 1.0 };
            let s2 = if ii % 2 == 0 { -1.0 } else { 1.0 };
            let p1 = cir1_center + (s1 * r1) * dir1;
            let p2 = cir2_center + (s2 * r2) * dir2;
            let len = p1.distance_squared(p2);
            if len < best_len {
                best_len = len;
                best_p1 = p1;
            }
        }
        let best_len = sqrt(best_len);
        return Ok(if best_len > tol3d {
            CurveIntersection::Points(Vec::new())
        } else {
            CurveIntersection::Points(vec![best_p1])
        });
    }

    // two crossing points, stored in reverse order as upstream does
    Ok(CurveIntersection::Points(
        params
            .iter()
            .take(count)
            .rev()
            .map(|&t| line_pt + t * line_vec)
            .collect(),
    ))
}

/// Intersects the planes `(p1, unit v1)` and `(p2, unit v2)`
/// (`smgu_IntersectTwoPlanes`). The line point lies on the plane through the
/// midpoint of `p1` and `p2` normal to the line.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] when a normal is not unit length (within
/// `SM_EFF_ZERO` on its squared length); propagates the three-plane intersection.
pub fn intersect_two_planes(
    p1: Point3,
    v1: Vec3,
    p2: Point3,
    v2: Vec3,
) -> Result<PlanesIntersection> {
    if (v1.dot(v1) - 1.0).abs() > EFF_ZERO || (v2.dot(v2) - 1.0).abs() > EFF_ZERO {
        return Err(SolidError::InvalidInput {
            operation: "geom::geom_utility::p2::intersect_two_planes",
            reason: "plane normal is not a unit vector",
        });
    }

    // a third plane orthogonal to both
    let plane3_pnt = (p1 + p2) / 2.0;
    let plane3_norm = v1.cross(v2);

    if plane3_norm.length_squared() < EFF_ZERO_SQ {
        let scaled_zero = EFF_ZERO * (1.0 + max2(p1.max_dimension(), p2.max_dimension()));
        let dist1 = (p2 - p1).dot(v1);
        let dist2 = (p1 - p2).dot(v2);
        return Ok(PlanesIntersection::Parallel {
            coincident: dist1 < scaled_zero && dist2 < scaled_zero,
        });
    }
    let plane3_norm = plane3_norm.unitize()?;

    let point = intersect_three_planes(p1, v1, p2, v2, plane3_pnt, plane3_norm)?;
    Ok(PlanesIntersection::Line {
        point,
        direction: plane3_norm,
    })
}

// ---- best fit ------------------------------------------------------------------------

/// Least-squares best-fit plane through `points` (`smgu_BestFitPlaneToPoints`): the
/// centroid and the eigenvector of the smallest eigenvalue of the scatter matrix.
/// Void, point-sized and linear sets return the best-fit line result unchanged.
///
/// # Errors
///
/// Only propagates errors of its callees.
pub fn best_fit_plane_to_points(points: &[Point3], tol3d: f64) -> Result<BestFitPlane> {
    // upstream ignores this status
    let line = best_fit_line_to_points(points, tol3d)?;
    let mut fit = BestFitPlane {
        set_type: line.set_type,
        point: line.point,
        normal: line.direction,
        max_plane_gap: line.max_line_gap,
        max_centroid_gap: line.max_centroid_gap,
    };
    if fit.set_type != PointSetType::Scattered {
        return Ok(fit);
    }

    // smallest eigenvalue's eigenvector of M (built by the best-fit line, symmetric)
    let mut m = line.m;
    let (_, vectors) = m.compute_3x3_eigen_vectors()?;
    let [smallest, _, _] = vectors;
    fit.normal = smallest.unitize_or_z();

    let mut max_gap = 0.0;
    for &p in points {
        let dist = fit.normal.dot(p - fit.point);
        if max_gap < dist {
            max_gap = dist;
        }
    }
    fit.max_plane_gap = max_gap;
    fit.set_type = if max_gap <= tol3d {
        PointSetType::Planar
    } else {
        PointSetType::Scattered
    };
    Ok(fit)
}

// ---- 4x4 matrices --------------------------------------------------------------------

/// `a * b` for 4x4 row-major matrices (`smgu_4x4Mult`).
#[must_use]
pub fn mult_4x4(a: &[[f64; 4]; 4], b: &[[f64; 4]; 4]) -> [[f64; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for (out_row, a_row) in out.iter_mut().zip(a) {
        for (jj, cell) in out_row.iter_mut().enumerate() {
            let mut sum = 0.0;
            for (a_k, b_row) in a_row.iter().zip(b) {
                if let Some(b_kj) = b_row.get(jj) {
                    sum += a_k * b_kj;
                }
            }
            *cell = sum;
        }
    }
    out
}

/// Inverse of a 4x4 matrix through cofactors (`smgu_4x4Inverse`).
///
/// # Errors
///
/// `SM_ERR` when the determinant is within `SM_EFF_ZERO` of zero.
pub fn inverse_4x4(a: &[[f64; 4]; 4]) -> Result<[[f64; 4]; 4]> {
    let det = determinate_4x4(a);
    if is_zero(det) {
        return Err(sm_err("geom::geom_utility::p2::inverse_4x4"));
    }

    // inv[r][c] = (-1)^(r+c) * det(minor without row c, column r) / det
    let mut inv = [[0.0; 4]; 4];
    for (r, inv_row) in inv.iter_mut().enumerate() {
        for (c, cell) in inv_row.iter_mut().enumerate() {
            let minor = minor_3x3(a, c, r);
            let cofactor = determinant_3x3(&minor);
            let signed = if (r + c) % 2 == 1 {
                -cofactor
            } else {
                cofactor
            };
            *cell = signed / det;
        }
    }
    Ok(inv)
}

/// `a` without row `row` and column `col`, remaining entries in order.
fn minor_3x3(a: &[[f64; 4]; 4], row: usize, col: usize) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    let rows = a.iter().enumerate().filter(|&(i, _)| i != row);
    for (m_row, (_, a_row)) in m.iter_mut().zip(rows) {
        let cols = a_row.iter().enumerate().filter(|&(j, _)| j != col);
        for (cell, (_, &v)) in m_row.iter_mut().zip(cols) {
            *cell = v;
        }
    }
    m
}

// ---- curve differential geometry -------------------------------------------------------

/// Frenet quantities from curve derivatives `vals = [P, P', P'', P''']`
/// (`smgu_FrenetFromDerivatives`): returns `n + 1` vectors `[position, first
/// derivative, curvature vector, torsion vector]` (`n <= 3`); the curvature vector
/// has magnitude `k` along the normal, the torsion vector magnitude `tau` along the
/// binormal.
///
/// # Errors
///
/// [`SolidError::InvalidInput`] when `n > 3` or `vals` has fewer than `n + 1`
/// entries; `SM_ERR` for a zero first derivative (`n >= 2`) or a zero second
/// derivative or straight curve (`n == 3`).
pub fn frenet_from_derivatives(vals: &[Vec3], n: usize) -> Result<Vec<Vec3>> {
    const OP: &str = "geom::geom_utility::p2::frenet_from_derivatives";
    if n > 3 || vals.len() <= n {
        return Err(SolidError::InvalidInput {
            operation: OP,
            reason: "need n <= 3 and n + 1 derivative vectors",
        });
    }
    let get = |i: usize| vals.get(i).copied().unwrap_or(Vec3::ZERO);
    let mut out = vec![get(0)];
    if n == 0 {
        return Ok(out);
    }
    let d1 = get(1);
    out.push(d1);
    if n == 1 {
        return Ok(out);
    }

    let tan_len_sq = d1.length_squared();
    if tan_len_sq < EFF_ZERO_SQ {
        return Err(sm_err(OP));
    }
    let tan_len_fourth = tan_len_sq * tan_len_sq;
    let d2 = get(2);
    let curvature = d1.cross(d2).cross(d1) / tan_len_fourth;
    out.push(curvature);
    if n == 2 {
        return Ok(out);
    }

    if is_zero(d2.length()) {
        return Err(sm_err(OP));
    }
    let cross_len = d1.cross(d2).length();
    let denominator = cross_len * cross_len;
    if is_zero(denominator) {
        return Err(sm_err(OP));
    }
    let numerator = determinant_3_vectors(d1, d2, get(3));
    let torsion = numerator / denominator;

    let mut torsion_vec = d1.cross(curvature);
    let vec_len = torsion_vec.length();
    if vec_len < EFF_ZERO {
        return Err(sm_err(OP));
    }
    torsion_vec *= torsion / vec_len;
    out.push(torsion_vec);
    Ok(out)
}

/// Continuity between two curve evaluations `(pos, first, second[, third])`
/// (`smgu_EvaluateCurveContinuity`): C0 by scaled position gap, G1 by tangent angle
/// (`continuity_angle_deg`), C1/C2/C3 by derivative gaps against
/// `SM_ZONE_TOL_3D / 10`, G2 by curvature gap and G3 (both third derivatives
/// given) by a Taylor-series test.
///
/// # Errors
///
/// Only propagates errors of its callees.
pub fn evaluate_curve_continuity(
    eval1: &CurveEval,
    eval2: &CurveEval,
    continuity_angle_deg: f64,
) -> Result<ContinuityType> {
    evaluate_curve_continuity_with(
        eval1,
        eval2,
        continuity_angle_deg,
        &ContinuityOptions::default(),
    )
}

/// Options of [`evaluate_curve_continuity_with`]; the default is upstream's test.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContinuityOptions {
    /// Not upstream (WR-380, source issue #933, #838): judge G2 by the
    /// geometry. Upstream takes the curvatures as the lengths of the raw second
    /// derivatives (which depend on the parameter speed), compares them with an
    /// absolute `SM_EFF_ZERO_SQRT / 10`, and measures the angle of the first normal
    /// with itself (always 0). With this on, the curvature vectors are the Frenet ones
    /// (`smgu_FrenetFromDerivatives`, `|P' x P''| / |P'|^3` along the normal), their
    /// lengths are compared relative to the larger (`SM_EFF_ZERO_SQRT / 10 * (1 +
    /// max)`), and the angle is between the two normals.
    pub geometric_g2: bool,
}

/// [`evaluate_curve_continuity`] with [`ContinuityOptions`].
///
/// # Errors
///
/// Only propagates errors of its callees.
pub fn evaluate_curve_continuity_with(
    eval1: &CurveEval,
    eval2: &CurveEval,
    continuity_angle_deg: f64,
    options: &ContinuityOptions,
) -> Result<ContinuityType> {
    let CurveEval {
        position: pos1,
        first: vec1,
        second: second1,
        third: third1,
    } = *eval1;
    let CurveEval {
        position: pos2,
        first: vec2,
        second: second2,
        third: third2,
    } = *eval2;
    let param_eff_zero = ZONE_TOL_3D / 10.0;

    let pos1_max = pos1.max_dimension();
    let vec1_max = vec1.max_dimension();
    let second1_max = second1.max_dimension();
    let third1_max = third1.map_or(1.0, Vec3::max_dimension);
    let pos2_max = pos2.max_dimension();
    let vec2_max = vec2.max_dimension();
    let second2_max = second2.max_dimension();
    let third2_max = third2.map_or(1.0, Vec3::max_dimension);

    let pos12_max = max2(pos1_max, pos2_max);
    let vec12_max = max2(vec1_max, vec2_max);
    let second12_max = max2(second1_max, second2_max);
    let third12_max = max2(third1_max, third2_max);

    // C0
    let scaled_zero = EFF_ZERO * (1.0 + pos12_max);
    if pos1.distance(pos2) > scaled_zero {
        return Ok(ContinuityType::Discontinuous);
    }
    let mut kind = ContinuityType::C0;

    // zero tangents stay C0
    let vec1_len = vec1.length();
    if vec1_len < EFF_ZERO * (1.0 + vec1_max) {
        return Ok(kind);
    }
    let vec2_len = vec2.length();
    if vec2_len < EFF_ZERO * (1.0 + vec2_max) {
        return Ok(kind);
    }

    // G1
    let mut unit1 = vec1;
    unit1 /= vec1_len;
    let mut unit2 = vec2;
    unit2 /= vec2_len;
    let ang_rad = unit1.angle_between(unit2).unwrap_or(0.0);
    if rad_to_deg(ang_rad) > continuity_angle_deg {
        return Ok(kind);
    }
    kind = ContinuityType::G1;

    // C1
    if vec1.distance(vec2) < param_eff_zero * (1.0 + vec12_max) {
        kind = ContinuityType::C1;
    }

    // G2 (upstream measures the angle of normal 1 with itself)
    let not_g2 = if options.geometric_g2 {
        // the Frenet curvature vectors (both tangents are non-zero here)
        let k1 = frenet_from_derivatives(&[pos1, vec1, second1], 2)?
            .get(2)
            .copied()
            .unwrap_or(Vec3::ZERO);
        let k2 = frenet_from_derivatives(&[pos2, vec2, second2], 2)?
            .get(2)
            .copied()
            .unwrap_or(Vec3::ZERO);
        let curvature1 = k1.length();
        let curvature2 = k2.length();
        let gap_tol = EFF_ZERO_SQRT / 10.0 * (1.0 + max2(curvature1, curvature2));
        let mut ang_rad = 0.0;
        if !is_zero(curvature1) && !is_zero(curvature2) {
            let mut normal1 = k1;
            normal1 /= curvature1;
            let mut normal2 = k2;
            normal2 /= curvature2;
            ang_rad = normal1.angle_between(normal2).unwrap_or(0.0);
        }
        !in_tol(curvature1 - curvature2, gap_tol) || rad_to_deg(ang_rad) > continuity_angle_deg
    } else {
        let curvature1 = second1.length();
        let curvature2 = second2.length();
        let curvature_gap = curvature1 - curvature2;
        let mut ang_rad = 0.0;
        if !is_zero(curvature1) && !is_zero(curvature2) {
            let mut normal1 = second1;
            normal1 /= curvature1;
            ang_rad = normal1.angle_between(normal1).unwrap_or(0.0);
        }
        // (as upstream's condition, NaN included)
        !in_tol(curvature_gap, tol3d(EFF_ZERO_SQRT / 10.0))
            || rad_to_deg(ang_rad) > continuity_angle_deg
    };
    if not_g2 {
        return Ok(kind);
    }
    kind = if kind == ContinuityType::G1 {
        ContinuityType::G1G2
    } else {
        ContinuityType::C1G2
    };

    // C2
    if kind == ContinuityType::C1G2
        && second1.distance(second2) < param_eff_zero * (1.0 + second12_max)
    {
        kind = ContinuityType::C1C2;
    }

    // G3 / C3: algebraic Taylor-series test with a tolerance from the G1/G2 gaps
    if let (Some(t1), Some(t2)) = (third1, third2) {
        let mut scaled_zero = ZONE_TOL_3D / 10.0 * (1.0 + vec12_max);
        let (g1_gap, a1) = line_point_distance(Point3::ZERO, vec2, vec1);
        if scaled_zero < g1_gap {
            scaled_zero = g1_gap;
        }

        let g2_target = second1 - a1 * a1 * second2;
        let (g2_gap, a2) = line_point_distance(Point3::ZERO, vec2, g2_target);
        if scaled_zero < g2_gap {
            scaled_zero = g2_gap;
        }

        let this_scaled_zero =
            ZONE_TOL_3D / 10.0 * (1.0 + max3(vec12_max, second12_max, third12_max));
        scaled_zero = max2(scaled_zero, this_scaled_zero);
        let g3_target = t1 - a1 * a1 * a1 * t2 - 3.0 * a1 * a2 * second2;
        let (g3_gap, _) = line_point_distance(Point3::ZERO, vec2, g3_target);
        if g3_gap > scaled_zero {
            return Ok(kind);
        }
        kind = match kind {
            ContinuityType::C1G2 => ContinuityType::C1G2G3,
            ContinuityType::C1C2 => ContinuityType::C1C2G3,
            _ => ContinuityType::G1G2G3,
        };

        if kind == ContinuityType::C1C2G3 && t1.distance(t2) < param_eff_zero * (1.0 + third12_max)
        {
            kind = ContinuityType::C1C2C3;
        }
    }
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(x, y, z)
    }

    #[test]
    fn mult_4x4_identity_and_product() {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let a = [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
            [13.0, 14.0, 15.0, 16.0],
        ];
        assert_eq!(mult_4x4(&a, &id), a);
        assert_eq!(mult_4x4(&id, &a), a);
        let sq = mult_4x4(&a, &a);
        assert_eq!(sq[0], [90.0, 100.0, 110.0, 120.0]);
        assert_eq!(sq[3], [426.0, 484.0, 542.0, 600.0]);
    }

    #[test]
    fn minor_skips_row_and_column() {
        let a = [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
            [13.0, 14.0, 15.0, 16.0],
        ];
        assert_eq!(
            minor_3x3(&a, 1, 0),
            [[2.0, 3.0, 4.0], [10.0, 11.0, 12.0], [14.0, 15.0, 16.0]]
        );
        assert_eq!(
            minor_3x3(&a, 3, 2),
            [[1.0, 2.0, 4.0], [5.0, 6.0, 8.0], [9.0, 10.0, 12.0]]
        );
    }

    #[test]
    fn frenet_low_orders() {
        let vals = [v(1.0, 2.0, 3.0), v(2.0, 0.0, 0.0), v(0.0, 4.0, 0.0)];
        assert_eq!(frenet_from_derivatives(&vals, 0).unwrap(), vec![vals[0]]);
        assert_eq!(
            frenet_from_derivatives(&vals, 1).unwrap(),
            vec![vals[0], vals[1]]
        );
        let f = frenet_from_derivatives(&vals, 2).unwrap();
        // ((2,0,0) x (0,4,0)) x (2,0,0) / 16 = (0,16,0)/16
        assert_eq!(f[2], v(0.0, 1.0, 0.0));
        assert!(frenet_from_derivatives(&vals, 3).is_err());
        assert!(frenet_from_derivatives(&vals, 4).is_err());
        let flat = [Vec3::ZERO, Vec3::ZERO, v(1.0, 0.0, 0.0)];
        assert!(matches!(
            frenet_from_derivatives(&flat, 2),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
    }

    #[test]
    fn frenet_torsion() {
        // straight curve: zero second derivative -> SM_ERR
        let vals = [Vec3::ZERO, v(1.0, 0.0, 0.0), Vec3::ZERO, v(0.0, 0.0, 1.0)];
        assert!(matches!(
            frenet_from_derivatives(&vals, 3),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        let vals = [
            Vec3::ZERO,
            v(1.0, 0.0, 0.0),
            v(0.0, 1.0, 0.0),
            v(0.0, 0.0, 1.0),
        ];
        let f = frenet_from_derivatives(&vals, 3).unwrap();
        assert_eq!(f.len(), 4);
    }

    #[test]
    fn continuity_levels() {
        let p = v(1.0, 1.0, 0.0);
        let t = v(1.0, 0.0, 0.0);
        let k = v(0.0, 2.0, 0.0);
        let ev = |position, first, second, third| CurveEval {
            position,
            first,
            second,
            third,
        };
        let e1 = ev(p, t, k, None);
        let c = |p2, t2, k2| evaluate_curve_continuity(&e1, &ev(p2, t2, k2, None), 1.0);
        assert_eq!(
            c(v(2.0, 1.0, 0.0), t, k).unwrap(),
            ContinuityType::Discontinuous
        );
        assert_eq!(c(p, Vec3::ZERO, k).unwrap(), ContinuityType::C0);
        assert_eq!(c(p, v(0.0, 1.0, 0.0), k).unwrap(), ContinuityType::C0);
        assert_eq!(c(p, t * 2.0, k * 3.0).unwrap(), ContinuityType::G1);
        assert_eq!(c(p, t * 2.0, k).unwrap(), ContinuityType::G1G2);
        assert_eq!(c(p, t, v(0.0, -2.0, 0.0)).unwrap(), ContinuityType::C1G2);
        assert_eq!(c(p, t, k).unwrap(), ContinuityType::C1C2);
        let third = Some(v(0.0, 0.0, 1.0));
        assert!(evaluate_curve_continuity(&ev(p, t, k, third), &ev(p, t, k, third), 1.0).is_ok());
    }

    #[test]
    fn two_planes_parallel_and_invalid() {
        let z = v(0.0, 0.0, 1.0);
        assert_eq!(
            intersect_two_planes(Point3::ZERO, z, v(5.0, 0.0, 0.0), z).unwrap(),
            PlanesIntersection::Parallel { coincident: true }
        );
        assert_eq!(
            intersect_two_planes(Point3::ZERO, z, v(0.0, 0.0, 1.0), -z).unwrap(),
            PlanesIntersection::Parallel { coincident: false }
        );
        assert!(matches!(
            intersect_two_planes(Point3::ZERO, z * 2.0, Point3::ZERO, z),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            intersect_two_planes(Point3::ZERO, z, Point3::ZERO, v(1.0, 0.0, 0.0)).unwrap(),
            PlanesIntersection::Line { .. }
        ));
    }

    #[test]
    fn sphere_circles_parallel() {
        let z = v(0.0, 0.0, 1.0);
        let r = sphere_circle_sphere_circle_intersect(
            Point3::ZERO,
            2.0,
            v(0.0, 0.0, 1.0),
            z,
            v(0.0, 0.0, 1.0),
            z,
            1e-6,
        );
        assert_eq!(r.unwrap(), CurveIntersection::Coincident);
        let r = sphere_circle_sphere_circle_intersect(
            Point3::ZERO,
            2.0,
            v(0.0, 0.0, 1.0),
            z,
            v(0.0, 0.0, -1.0),
            z,
            1e-6,
        );
        assert_eq!(r.unwrap(), CurveIntersection::Points(Vec::new()));
    }

    #[test]
    fn calls_into_p1() {
        let p = Point3::ZERO;
        let x = v(1.0, 0.0, 0.0);
        let y = v(0.0, 1.0, 0.0);
        assert_eq!(triangle_point_distance(p, x, y, p).unwrap().distance, 0.0);
        assert_eq!(
            segment_segment_intersect(p, x, p, y, 1e-6).unwrap(),
            vec![p]
        );
        let fit = best_fit_plane_to_points(&[p, x, y, v(1.0, 1.0, 0.0)], 1e-6).unwrap();
        assert_eq!(fit.set_type, PointSetType::Planar);
        let zero = mult_4x4(&[[1.0; 4]; 4], &[[0.0; 4]; 4]);
        assert!(inverse_4x4(&zero).is_err());
    }

    /// source issue #933, #838 (WR-380): upstream's G2 test compares the first
    /// normal with itself and the raw second-derivative lengths; the opt-in compares the
    /// Frenet curvatures and the two normals.
    #[test]
    fn issue_933_geometric_g2_continuity() {
        let ev = |first, second| CurveEval {
            position: v(1.0, 1.0, 0.0),
            first,
            second,
            third: None,
        };
        let on = ContinuityOptions { geometric_g2: true };
        let off = ContinuityOptions::default();
        let c = |a: &CurveEval, b: &CurveEval, o: &ContinuityOptions| {
            evaluate_curve_continuity_with(a, b, 1.0, o).unwrap()
        };

        // #933: equal tangents and curvatures, opposite normals: not G2
        let up = ev(v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0));
        let down = ev(v(1.0, 0.0, 0.0), v(0.0, -1.0, 0.0));
        assert_eq!(c(&up, &down, &off), ContinuityType::C1G2);
        assert_eq!(c(&up, &down, &on), ContinuityType::C1);
        assert_eq!(
            evaluate_curve_continuity(&up, &down, 1.0).unwrap(),
            ContinuityType::C1G2
        );

        // #838: the same circle of radius 1 at twice the parameter speed (|P''| = 4):
        // geometrically G2
        let fast = ev(v(2.0, 0.0, 0.0), v(0.0, 4.0, 0.0));
        assert_eq!(c(&up, &fast, &off), ContinuityType::G1);
        assert_eq!(c(&up, &fast, &on), ContinuityType::G1G2);

        // the same curve: G2 either way
        assert_eq!(c(&up, &up, &on), c(&up, &up, &off));
    }
}
