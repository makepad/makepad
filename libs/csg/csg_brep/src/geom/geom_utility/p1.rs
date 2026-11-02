//! Geometric utilities, part 1: polygon and triangle measures, line / plane / circle /
//! sphere closest points and intersections, circles through points, best-fit point
//! and line, small fixed-size matrix helpers, directional derivatives and sorting
//! helpers.
//!
//! Upstream: `source/SMLib/src/SmGeomUtility.cpp`, `source/SMLib/inc/SmGeomUtility.h`.
//!
//! | Rust                                  | Upstream                                         |
//! |---------------------------------------|--------------------------------------------------|
//! | [`add_point`]                         | `sm_AddPoint` (static)                           |
//! | [`ClosestPoints`], [`ClosestPoints::add`] | `sm_AddClosestPoint` (static) and its arrays |
//! | [`polygon_area`]                      | `smgu_PolygonArea`                               |
//! | [`triangle_area_squared`]             | `smgu_TriangleAreaSquared`                       |
//! | [`triangle_decomposition`]            | `smgu_TriangleDecomposition`                     |
//! | [`parallelogram_cross_vectors`]       | `smgu_ParallelogramCrossVectors`                 |
//! | [`line_point_distance`]               | `smgu_LinePointDistance` (3d)                    |
//! | [`line_line_closest_point`]           | `smgu_LineLineClosestPoint`                      |
//! | [`line_plane_intersect`]              | `smgu_LinePlaneIntersect` (point + normal)       |
//! | [`line_plane_intersect_uv`]           | `smgu_LinePlaneIntersect` (point + two tangents) |
//! | [`line_closest_point`]                | `smgu_LineClosestPoint`                          |
//! | [`segment_point_distance`]            | `smgu_SegmentPointDistance`                      |
//! | [`segment_point_angle_deg_to_perp`]   | `smgu_SegmentPointAngleDegToPerp`                |
//! | [`circle_center_from_3_points`]       | `smgu_CircleCenterFrom3Points`                   |
//! | [`circle_from_3_points`], [`Circle3Points`] | `smgu_CircleFrom3Points`                   |
//! | [`circle_find_silhouette_points`]     | `smgu_CircleFindSilhouettePoints`                |
//! | [`cones_are_disjoint_perpen`]         | `smgu_ConesAreDisjointPerpen`                    |
//! | [`line_coplanar_circle_intersect`]    | `smgu_LineCoPlanarCircleIntersect`               |
//! | [`circle_coplanar_circle_intersect`], [`CircleCircle`] | `smgu_CircleCoPlanarCircleIntersect` |
//! | [`line_intersect_sphere`], [`LineSphere`] | `smgu_LineIntersectSphere`                   |
//! | [`intersect_three_planes`]            | `smgu_IntersectThreePlanes`                      |
//! | [`plane_evaluate_point`]              | `smgu_PlaneEvaluatePoint`                        |
//! | [`plane_drop_point`]                  | `smgu_PlaneDropPoint`                            |
//! | [`plane_point_distance`]              | `smgu_PlanePointDistance`                        |
//! | [`point_project_to_plane`]            | `smgu_PointProjectToPlane`                       |
//! | [`vector_project_to_plane`]           | `smgu_VectorProjectToPlane`                      |
//! | [`transform_point_to_start_plane`], [`StartPlanePoint`] | `smgu_TransformPointToStartPlane` |
//! | [`point_perspective_to_plane`]        | `smgu_PointPerspectiveToPlane`                   |
//! | [`vec_lin_comb_two_vectors`]          | `smgu_VecLinCombTwoVectors`                      |
//! | [`PointSetType`]                      | `SmPointSetType`                                 |
//! | [`best_fit_point_to_points`], [`BestFitPoint`] | `smgu_BestFitPointToPoints`             |
//! | [`best_fit_line_to_points`], [`BestFitLine`]   | `smgu_BestFitLineToPoints`              |
//! | [`vec_triple_product`]                | `smgu_VecTripleProduct`                          |
//! | [`copy_3x3`]                          | `smgu_3x3Copy`                                   |
//! | [`determinate_3x3`]                   | `smgu_3x3Determinate` (nine coefficients)        |
//! | [`determinant_3_vectors`]             | `smgu_Determinant3Vectors`                       |
//! | [`inverse_3x3`]                       | `smgu_3x3Inverse`                                |
//! | [`copy_4x4`]                          | `smgu_4x4Copy`                                   |
//! | [`determinate_4x4`]                   | `smgu_4x4Determinate`                            |
//! | [`solve_quadratic_eqn`]               | `smgu_SolveQuadraticEqn`                         |
//! | [`directional_derivs`]                | `smgu_DirectionalDerivs`                         |
//! | [`vol_directional_derivs`]            | `smgu_VolDirectionalDerivs`                      |
//! | [`is_dog_leg`]                        | `smgu_IsDogLeg` (`SmVector2d` points)            |
//! | [`add_sorted`]                        | `smgu_AddSorted`                                 |
//! | [`shell_sort_double_array`]           | `smgu_ShellSort_double_array`                    |
//! | [`double_less_than`]                  | `smgu_DoubleLessThan`                            |
//!
//! Deviations:
//! - Out-params and `SmStatus` become return values. `SM_ERR_INVALID_INPUT` (also from
//!   `ZERO_VEC_ER` and a failed `Unitize`) is [`SolidError::InvalidInput`]; `SM_ERR` is
//!   [`SolidError::Upstream`] with code 1001, `SM_ERR_WARNING` code 1023. Where upstream
//!   returns `FALSE` as a status (`smgu_CircleCenterFrom3Points` for coincident
//!   points; `FALSE` is not a valid `SmStatus` and is not `SM_SUCCESS`) the port returns
//!   [`SolidError::Upstream`] with code 0.
//! - Upstream fixed-size output arrays plus counts (`ULONG&` + `double[2]`, ...) become
//!   `Vec`s, `Option`s or enums holding exactly the values upstream sets.
//! - Statuses upstream ignores (`SE(...)` is a no-op in the release build, or the call
//!   is not checked) are ignored the same way: an ignored `Unitize` is
//!   [`Vec3::unitize_or_z`] (or the unchanged [`Vec2`]); an ignored `AngleBetween`
//!   failure yields its angle of `0`.
//! - `smgu_CircleFrom3Points` ignores the status of `CCWAngleBetween`; that call cannot
//!   fail (the projected vectors are checked against `100 * SM_EFF_ZERO_SQ` first), so
//!   the port propagates it with `?` instead of reading an uninitialised angle.
//! - `smgu_BestFitLineToPoints`' optional `SmMatrix* pOptM` (the `M(LinePt)` matrix,
//!   written only when the points are scattered) is [`BestFitLine::m`].
//! - `smgu_PolygonArea` with no points reads `sPoints[-1]` upstream; the port returns
//!   [`SolidError::InvalidInput`]. The derivative functions check the derivative array
//!   length and consumed index arithmetic (upstream reads out of bounds), returning
//!   `InvalidInput` for unrepresentable indices, and return only the directional
//!   derivatives upstream writes.
//! - `smgu_AddSorted` takes `Vec`s for `SmTArray`s; `sm_AddPoint`'s `ULONG&` count and
//!   `SmPoint3d[2]` array are a `Vec` of at most two points.
//! - `SM_ASSERT*` and `SM_DEBUG_CODE` blocks are not compiled upstream (release build)
//!   and are not ported.

use crate::error::{Result, SolidError, SM_ERR_WARNING};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::matrix::Matrix;
use crate::geom::polynomial::{solve_quadratic, Roots};
use crate::geom::tol::{are_same_to_tol, is_zero};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{acos as sm_acos, sqrt as sm_sqrt, Point3, Vec3};
use crate::math::{rad_to_deg, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};

/// `ANALYTIC_TOL_SCALE` (`SmConfig.h`).
const ANALYTIC_TOL_SCALE: f64 = 100.0;

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// `ZERO_VEC_ER(vec)`: `SM_ERR_INVALID_INPUT` unless `|vec|^2 >= SM_EFF_ZERO_SQ`
/// (so also for NaN).
fn zero_vec_er(v: Vec3, operation: &'static str) -> Result<()> {
    if v.length_squared() >= EFF_ZERO_SQ {
        Ok(())
    } else {
        Err(invalid(operation, "zero-length vector"))
    }
}

// ---- static helpers ------------------------------------------------------------------

/// Adds `pnt` to `points` (at most two distinct points) unless a point closer than
/// `tol` is already there (`sm_AddPoint`, a helper of `smgu_SegmentSegmentIntersect`).
///
/// Errors: `SM_ERR` (code 1001) when `pnt` is new and `points` already holds two (or
/// more) points; `points` is then unchanged.
pub fn add_point(points: &mut Vec<Point3>, tol: f64, pnt: Point3) -> Result<()> {
    let tol2 = tol * tol;
    for p in points.iter() {
        if pnt.distance_squared(*p) < tol2 {
            return Ok(());
        }
    }
    if points.len() > 1 {
        return Err(SolidError::sm_err("geom::geom_utility::p1::add_point"));
    }
    points.push(pnt);
    Ok(())
}

/// Up to two closest-approach candidates, ordered by distance (the arrays of
/// `smgu_SegmentSegmentClosestPoint` maintained by `sm_AddClosestPoint`).
///
/// Only the first `count` slots are meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClosestPoints {
    /// Number of candidates, 0 to 2 (`rlNumClosestPoints`).
    pub count: usize,
    /// Parameters on the first segment (`adParamSeg1`).
    pub params1: [f64; 2],
    /// Parameters on the second segment (`adParamSeg2`).
    pub params2: [f64; 2],
    /// Distances (`adDistances`).
    pub distances: [f64; 2],
}

impl ClosestPoints {
    fn set(&mut self, slot: usize, dist: f64, param1: f64, param2: f64) {
        if let (Some(a), Some(b), Some(d)) = (
            self.params1.get_mut(slot),
            self.params2.get_mut(slot),
            self.distances.get_mut(slot),
        ) {
            *a = param1;
            *b = param2;
            *d = dist;
        }
    }

    /// Merges the candidate `(dist, param1, param2)` into the list: candidates more
    /// than `tolerance` worse than the current worst are skipped, one more than
    /// `tolerance` better than the best replaces the list, others fill or shift the
    /// two slots (`sm_AddClosestPoint`).
    pub fn add(&mut self, dist: f64, param1: f64, param2: f64, tolerance: f64) {
        if self.count == 0 {
            self.set(0, dist, param1, param2);
            self.count = 1;
            return;
        }
        let last = self
            .distances
            .get(self.count - 1)
            .copied()
            .unwrap_or(f64::NAN);
        if dist > last + tolerance {
            return; // Too big of a distance skip it
        }

        if dist >= last {
            // Just off the end either skip it or add to second slot.
            if self.count == 1 {
                self.set(1, dist, param1, param2);
                self.count += 1;
            }
            return;
        }

        // Must be TRUE if (dDist < adDistances[rlNumClosestPoints-1])
        let [first, _] = self.distances;
        if dist < first - tolerance {
            // Totally new one
            self.set(0, dist, param1, param2);
            self.count = 1;
            return;
        }

        if dist > first {
            // Must fall between 0 and 1
            self.set(1, dist, param1, param2);
            self.count = 2;
            return;
        }

        // Final case is where we make 2 from 1
        self.count = 2;
        let [p1, _] = self.params1;
        let [p2, _] = self.params2;
        self.set(1, first, p1, p2);
        self.set(0, dist, param1, param2);
    }
}

// ---- polygons and triangles ----------------------------------------------------------

/// Signed area of the closed polygon `points` projected to the plane with unit normal
/// `unit_normal` (`smgu_PolygonArea`); a non-unit normal scales the result.
///
/// Errors: `InvalidInput` for an empty point list (upstream reads `sPoints[-1]`).
pub fn polygon_area(points: &[Point3], unit_normal: Vec3) -> Result<f64> {
    let Some(&first_last) = points.last() else {
        return Err(invalid("geom::geom_utility::p1::polygon_area", "no points"));
    };
    let mut last = first_last;
    let mut total_area = 0.0;
    for &curr in points {
        let cross = last.cross(curr);
        total_area += cross.dot(unit_normal);
        last = curr;
    }
    total_area /= 2.0;
    Ok(total_area)
}

/// Squared area of the triangle `p0 p1 p2` (`smgu_TriangleAreaSquared`).
#[must_use]
pub fn triangle_area_squared(p0: Point3, p1: Point3, p2: Point3) -> f64 {
    let ay_bz = (p1.y - p0.y) * (p2.z - p0.z);
    let az_bx = (p1.z - p0.z) * (p2.x - p0.x);
    let ax_by = (p1.x - p0.x) * (p2.y - p0.y);

    let az_by = (p1.z - p0.z) * (p2.y - p0.y);
    let ax_bz = (p1.x - p0.x) * (p2.z - p0.z);
    let ay_bx = (p1.y - p0.y) * (p2.x - p0.x);

    ((ay_bz - az_by) * (ay_bz - az_by)
        + (az_bx - ax_bz) * (az_bx - ax_bz)
        + (ax_by - ay_bx) * (ax_by - ay_bx))
        / 4.0
}

/// The apex of a triangle with the given base and leg lengths, as
/// `(distance along the base from the left vertex, height above the base)`
/// (`smgu_TriangleDecomposition`).
///
/// Errors: `InvalidInput` when the base is zero, the legs are too short to reach
/// (`left + right < base + SM_EFF_ZERO * (1 + base)`), or the foot of the apex is
/// beyond the right leg.
pub fn triangle_decomposition(
    base_length: f64,
    left_leg_length: f64,
    right_leg_length: f64,
) -> Result<(f64, f64)> {
    const OP: &str = "geom::geom_utility::p1::triangle_decomposition";
    if is_zero(base_length) {
        return Err(invalid(OP, "zero base length"));
    }

    if left_leg_length + right_leg_length < base_length + (EFF_ZERO * (1.0 + base_length)) {
        return Err(invalid(OP, "legs shorter than the base"));
    }

    let q = (left_leg_length * left_leg_length
        - right_leg_length * right_leg_length
        - base_length * base_length)
        / (-2.0 * base_length);

    if q.abs() > right_leg_length {
        return Err(invalid(OP, "apex foot beyond the right leg"));
    }

    let height = sm_sqrt(right_leg_length * right_leg_length - q * q);
    let along = base_length - q;
    Ok((along, height))
}

/// The diagonals `(lower-left to upper-right, lower-right to upper-left)` of the
/// parallelogram with side directions `base` and `side` whose opposite sides are
/// `top_to_bottom` and `between_sides` apart (`smgu_ParallelogramCrossVectors`).
///
/// Errors: `InvalidInput` when `base` or `side` has zero length.
pub fn parallelogram_cross_vectors(
    base: Vec3,
    side: Vec3,
    distance_top_to_bottom: f64,
    distance_between_sides: f64,
) -> Result<(Vec3, Vec3)> {
    let base = base.unitize()?;
    let side = side.unitize()?;
    let angle = base.angle_between(side)?;
    let sine_angle = angle.sin();
    let leng_side = distance_top_to_bottom / sine_angle;
    let leng_bot = distance_between_sides / sine_angle;
    let low_left_to_up_right = base * leng_bot + side * leng_side;
    let low_right_to_up_left = side * leng_side - base * leng_bot;
    Ok((low_left_to_up_right, low_right_to_up_left))
}

// ---- lines ---------------------------------------------------------------------------

/// Distance from `test_point` to the infinite line `line_pnt + t * line_vec`, and the
/// parameter `t` of the closest line point (`0` for a degenerate line)
/// (`smgu_LinePointDistance`, 3d).
#[must_use]
pub fn line_point_distance(line_pnt: Point3, line_vec: Vec3, test_point: Point3) -> (f64, f64) {
    let line_vec_length_squared = line_vec.length_squared();
    let param = if line_vec_length_squared < EFF_ZERO_SQ {
        // Just return the 'beginning' of the degenerate line.
        0.0
    } else {
        (test_point - line_pnt).dot(line_vec) / line_vec_length_squared
    };

    let pnt = line_pnt + param * line_vec;
    let distance = pnt.distance(test_point);
    (distance, param)
}

/// Parameters `(t1, t2)` of the closest points of two infinite lines
/// `pnt_i + t_i * vec_i` (`smgu_LineLineClosestPoint`).
///
/// Errors: `SM_ERR` (1001) for a zero-length vector, parallel lines, or when a
/// line/plane intersection fails.
pub fn line_line_closest_point(
    line_pnt1: Point3,
    line_vec1: Vec3,
    line_pnt2: Point3,
    line_vec2: Vec3,
) -> Result<(f64, f64)> {
    const OP: &str = "geom::geom_utility::p1::line_line_closest_point";
    // err - zero length vectors
    if line_vec1.length_squared() < EFF_ZERO_SQ || line_vec2.length_squared() < EFF_ZERO_SQ {
        return Err(SolidError::sm_err(OP));
    }

    // error - parallel or coincident lines
    let cross = line_vec1.cross(line_vec2);
    if cross.length_squared() < EFF_ZERO_SQ {
        return Err(SolidError::sm_err(OP));
    }

    // get minimum distance vector direction
    let cross = cross.unitize()?;

    // get vectors perp to line and minDistance vector
    let plane_v1 = cross.cross(line_vec1).unitize()?;
    let plane_v2 = cross.cross(line_vec2).unitize()?;

    let t1 = line_plane_intersect(line_pnt1, line_vec1, line_pnt2, plane_v2)
        .map_err(|_| SolidError::sm_err(OP))?;
    let t2 = line_plane_intersect(line_pnt2, line_vec2, line_pnt1, plane_v1)
        .map_err(|_| SolidError::sm_err(OP))?;
    Ok((t1, t2))
}

/// Parameter of the intersection of the line `line_pnt + t * line_vec` with the plane
/// through `plane_pnt` with normal `plane_norm` (`smgu_LinePlaneIntersect`).
///
/// Errors: `InvalidInput` for a zero-length vector; `SM_ERR` (1001) when the line is
/// parallel to the plane.
pub fn line_plane_intersect(
    line_pnt: Point3,
    line_vec: Vec3,
    plane_pnt: Point3,
    plane_norm: Vec3,
) -> Result<f64> {
    const OP: &str = "geom::geom_utility::p1::line_plane_intersect";
    zero_vec_er(line_vec, OP)?;
    zero_vec_er(plane_norm, OP)?;

    let plane_eq_d = -plane_norm.dot(plane_pnt);
    let tol = EFF_ZERO * line_vec.max_dimension() + EFF_ZERO_SQ;
    let denom = line_vec.dot(plane_norm);

    if denom.abs() < tol {
        return Err(SolidError::sm_err(OP)); // Parallel case
    }

    Ok(-(plane_eq_d + line_pnt.dot(plane_norm)) / denom)
}

/// Intersection of the line `line_pt + t * line_vec` with the plane
/// `plane_pt + u * plane_su + v * plane_sv`, as `(t, u, v)` by Cramer's rule
/// (`smgu_LinePlaneIntersect` with plane derivatives).
///
/// Errors: `InvalidInput` for a zero-length vector or parallel `su`, `sv`; `SM_ERR`
/// (1001) when the line is parallel to the plane.
pub fn line_plane_intersect_uv(
    line_pt: Point3,
    line_vec: Vec3,
    plane_pt: Point3,
    plane_su: Vec3,
    plane_sv: Vec3,
) -> Result<(f64, f64, f64)> {
    const OP: &str = "geom::geom_utility::p1::line_plane_intersect_uv";
    zero_vec_er(line_vec, OP)?;
    zero_vec_er(plane_su, OP)?;
    zero_vec_er(plane_sv, OP)?;

    let norm = plane_su.cross(plane_sv);
    zero_vec_er(norm, OP)?;

    let tol = EFF_ZERO * line_vec.max_dimension() + EFF_ZERO_SQ;

    let diff = line_pt - plane_pt;

    let denom = line_vec.dot(norm);

    if denom.abs() < tol {
        return Err(SolidError::sm_err(OP)); // Parallel case
    }

    // Cramer's rule:
    let numer_u = diff.cross(plane_sv).dot(line_vec);
    let numer_v = plane_su.cross(diff).dot(line_vec);
    let numer_t = -norm.dot(diff);

    Ok((numer_t / denom, numer_u / denom, numer_v / denom))
}

/// Parameter of the point of the line `line_pnt + t * line_vec` closest to
/// `test_point` (`0` for a degenerate line) (`smgu_LineClosestPoint`).
#[must_use]
pub fn line_closest_point(line_pnt: Point3, line_vec: Vec3, test_point: Point3) -> f64 {
    let vec_to_pt = test_point - line_pnt;
    let line_vec_length_squared = line_vec.length_squared();

    if line_vec_length_squared < EFF_ZERO_SQ {
        // Just return the 'beginning' of the degenerate line.
        0.0
    } else {
        vec_to_pt.dot(line_vec) / line_vec_length_squared
    }
}

/// Distance from `test_point` to the segment `line_start`..`line_end` and the
/// segment parameter in `[0, 1]` of the closest point (`smgu_SegmentPointDistance`).
#[must_use]
pub fn segment_point_distance(
    line_start: Point3,
    line_end: Point3,
    test_point: Point3,
) -> (f64, f64) {
    let line_vec = line_end - line_start;

    let mut param = line_closest_point(line_start, line_vec, test_point);

    // clip to specified endPoints
    param = param.clamp(0.0, 1.0); // same as the two upstream ifs, NaN included

    let found_pnt = line_start + param * line_vec;
    let distance = found_pnt.distance(test_point);
    (distance, param)
}

/// How far, in degrees, the gap from the segment `line_start`..`line_end` to
/// `test_point` is from perpendicular to the segment, and the segment parameter of
/// the closest point (`smgu_SegmentPointAngleDegToPerp`).
///
/// The angle is `0` when the point projects inside the segment (the parameter is
/// then not clamped), lies on the segment, or the angle cannot be computed.
#[must_use]
pub fn segment_point_angle_deg_to_perp(
    line_start: Point3,
    line_end: Point3,
    test_point: Point3,
) -> (f64, f64) {
    let line_vec = line_end - line_start;
    let param = line_closest_point(line_start, line_vec, test_point);
    let clip_ext = Extent1d::from_min_max(0.0, 1.0);
    if clip_ext.contains_value(param, 0.0) {
        return (0.0, param);
    }

    let param = clip_ext.clamp_value(param);
    let seg_cl_pnt = line_start + line_vec * param;
    let vec_to_pnt = test_point - seg_cl_pnt;

    // when targetPoint is on Segment - return angle = 0.0
    if vec_to_pnt.length_squared() < EFF_ZERO_SQ {
        return (0.0, param);
    }

    // get Gap/Segment angle (0 to Pi)
    let Ok(ang) = vec_to_pnt.angle_between(line_vec) else {
        return (0.0, param); // point is on the line
    };

    // get Angle from perpendicular in degrees
    ((ang * 180.0 / PI - 90.0).abs(), param)
}

// ---- circles -------------------------------------------------------------------------

/// Center of the 2D circle through three points (`smgu_CircleCenterFrom3Points`).
///
/// Errors: [`SolidError::Upstream`] with code 0 (upstream returns `FALSE`) when two
/// points are within `SM_EFF_ZERO_SQRT * (1 + |A| + |C|)` of each other; `SM_ERR`
/// (1001) when the points are (nearly) collinear.
pub fn circle_center_from_3_points(a: Point2, b: Point2, c: Point2) -> Result<Point2> {
    const OP: &str = "geom::geom_utility::p1::circle_center_from_3_points";
    let tol = EFF_ZERO_SQRT * (1.0 + a.max_dimension() + c.max_dimension());
    let tol2 = tol * tol;
    let not_ok = || SolidError::Upstream {
        operation: OP,
        code: 0,
        entity: None,
    };
    // Do some checks for coincident points and colinear points
    if a.distance_squared(b) < tol2 || b.distance_squared(c) < tol2 || a.distance_squared(c) < tol2
    {
        return Err(not_ok());
    }
    let vec = Vec3::from(c - a);

    // Upstream calls the 3d smgu_LinePointDistance (points converted with z = 0).
    let (dist_to_line, _) = line_point_distance(Vec3::from(a), vec, Vec3::from(b));
    if dist_to_line < 2.0 * EFF_ZERO * a.distance_squared(c) {
        return Err(SolidError::sm_err(OP)); // Radius would be bigger than 1 / SM_EFF_ZERO.
    }

    // Set up the 2x2 as described.
    let a00 = 2.0 * (a.x - b.x);
    let a01 = 2.0 * (a.y - b.y);
    let a10 = 2.0 * (b.x - c.x);
    let a11 = 2.0 * (b.y - c.y);

    let b0 = a.dot(a) - b.dot(b);
    let b1 = b.dot(b) - c.dot(c);

    let determ = a00 * a11 - a10 * a01; // Cramer's rule
    let numer0 = b0 * a11 - b1 * a01;
    let numer1 = a00 * b1 - a10 * b0;

    if determ.abs() < numer0.abs() * EFF_ZERO || determ.abs() < numer1.abs() * EFF_ZERO {
        return Err(SolidError::sm_err(OP));
    }

    Ok(Point2::new(numer0 / determ, numer1 / determ))
}

/// A circular arc through three points (outputs of `smgu_CircleFrom3Points`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle3Points {
    /// Center point (`rCenter`).
    pub center: Point3,
    /// Unit X axis, from the center towards the start point (`rXAxis`).
    pub x_axis: Vec3,
    /// Unit Y axis, counter-clockwise from the X axis (`rYAxis`).
    pub y_axis: Vec3,
    /// Arc domain in degrees, `[0, angle]` (`rAnalDomain`).
    pub domain: Extent1d,
    /// Radius (`dRadius`).
    pub radius: f64,
}

/// The circle through `start`, `mid` and `end`, with the arc domain from `start` to
/// `end` in degrees (or `[0, 360]` when `closed`) (`smgu_CircleFrom3Points`). With
/// `dimension == 2` the `z` coordinates are taken as `0`.
///
/// Errors: `SM_ERR` (1001) when `dimension` is not 2 or 3; the status of
/// `SmMatrix::SolveLinearSystem` for collinear points (`SM_ERR_WARNING` (1023) for a
/// near-singular system, which upstream also returns).
pub fn circle_from_3_points(
    start: Point3,
    mid: Point3,
    end: Point3,
    dimension: u64,
    closed: bool,
) -> Result<Circle3Points> {
    const OP: &str = "geom::geom_utility::p1::circle_from_3_points";
    if !(2..=3).contains(&dimension) {
        return Err(SolidError::sm_err(OP));
    }

    let mut sp = start;
    let mut mp = mid;
    let mut ep = end;

    // When working in 2D set all Z values = 0.0
    if dimension == 2 {
        sp.z = 0.0;
        mp.z = 0.0;
        ep.z = 0.0;
    }

    // Plane data
    let p0 = (sp + mp) / 2.0;
    let n0 = mp - sp;
    let p1 = (mp + ep) / 2.0;
    let n1 = ep - mp;
    let p2 = (ep + sp) / 2.0;
    let n2 = n0.cross(n1);

    let a = Matrix::from_row_major(
        3,
        3,
        vec![n0.x, n0.y, n0.z, n1.x, n1.y, n1.z, n2.x, n2.y, n2.z],
    )?;
    let b = [p0.dot(n0), p1.dot(n1), p2.dot(n2)];

    // solve for Center Point
    let sol = a.solve_linear_system(&b)?;
    if sol.near_singular {
        return Err(SolidError::Upstream {
            operation: OP,
            code: SM_ERR_WARNING,
            entity: None,
        });
    }
    let [x0, x1, x2] =
        <[f64; 3]>::try_from(sol.x.as_slice()).map_err(|_| invalid(OP, "linear solution size"))?;

    let n2 = n2.unitize_or_z();

    let center = Point3::new(x0, x1, x2);
    let mut x_axis = sp - center;
    let y_axis = n2.cross(x_axis);
    let radius = x_axis.length();
    x_axis /= radius;
    let y_axis = y_axis.unitize_or_z();

    // get CCW angle from XAxis to (PE-CP) about sN2
    let mut ccw_angle = if closed {
        360.0
    } else {
        n2.ccw_angle_between(x_axis, ep - center)? * (180.0 / PI)
    };
    if ccw_angle < 0.0 {
        ccw_angle += 360.0;
    }

    Ok(Circle3Points {
        center,
        x_axis,
        y_axis,
        domain: Extent1d::from_raw(0.0, ccw_angle),
        radius,
    })
}

/// The two silhouette points of a circle seen from an eye point (`perspective`) or
/// along a view direction, or `None` when there are none
/// (`smgu_CircleFindSilhouettePoints`).
///
/// In the perspective case upstream returns the rotated unit directions scaled by the
/// radius (not offset by the center); the port does the same.
pub fn circle_find_silhouette_points(
    circle_center: Point3,
    circle_normal: Vec3,
    circle_radius: f64,
    perspective: bool,
    eye_point_or_vector: Vec3,
) -> Result<Option<(Point3, Point3)>> {
    if perspective {
        let proj_eye_point =
            point_project_to_plane(eye_point_or_vector, circle_center, circle_normal);
        let to_eye = circle_center - proj_eye_point;
        let length = to_eye.length();
        // If the point is inside of the radius than no silhouette
        // is possible.
        if length < circle_radius + EFF_ZERO {
            return Ok(None); // No answer
        }
        let to_eye = to_eye.unitize_or_z();

        let angle_rad = sm_acos(circle_radius / length);
        let mut a2p = Axis2Placement::default();
        a2p.set_canonical(circle_center, Vec3::X, Vec3::Y)?;
        a2p.rotate_about_axis(angle_rad, circle_normal);
        let to_p1 = a2p.transform_point(to_eye);
        a2p.set_canonical(circle_center, Vec3::X, Vec3::Y)?;
        a2p.rotate_about_axis(-angle_rad, circle_normal);
        let to_p2 = a2p.transform_point(to_eye);

        return Ok(Some((to_p1 * circle_radius, to_p2 * circle_radius)));
    }

    // Handle parallel projection.

    let orth_vector = circle_normal.cross(eye_point_or_vector);
    if orth_vector.length_squared() < EFF_ZERO_SQ {
        return Ok(None);
    }
    let orth_vector = orth_vector.unitize_or_z();

    Ok(Some((
        circle_center + orth_vector * circle_radius,
        circle_center - orth_vector * circle_radius,
    )))
}

/// True when two cones (axis, half angle in radians) are perpendicularly disjoint:
/// `|angle(axis1, axis2) - PI/2| > ang1 + ang2 + SM_EFF_ZERO_SQRT`
/// (`smgu_ConesAreDisjointPerpen`).
#[must_use]
pub fn cones_are_disjoint_perpen(
    cone1_vec: Vec3,
    cone1_ang_rad: f64,
    cone2_vec: Vec3,
    cone2_ang_rad: f64,
) -> bool {
    let angle = cone1_vec.angle_between(cone2_vec).unwrap_or(0.0);
    // Use angle which is less than 90 degrees.
    let angle = (angle - PI / 2.0).abs();
    angle > cone1_ang_rad + cone2_ang_rad + EFF_ZERO_SQRT
}

/// Line parameters of the intersections of a line, projected into the circle's plane,
/// with the circle (`smgu_LineCoPlanarCircleIntersect`): none, one (tangent or a miss
/// within `tolerance`) or two.
///
/// Errors: `SM_ERR` (1001) when the projected line vector is shorter than
/// `SM_EFF_ZERO * radius`.
pub fn line_coplanar_circle_intersect(
    line_pnt: Point3,
    line_vec: Vec3,
    circle_center: Point3,
    circle_normal: Vec3,
    circle_radius: f64,
    tolerance: f64,
) -> Result<Vec<f64>> {
    // project line to plane
    let line_pnt = point_project_to_plane(line_pnt, circle_center, circle_normal);
    let mut line_vec = vector_project_to_plane(line_vec, circle_normal);

    // Unitize the line vector.
    let scale = line_vec.length();
    if scale < EFF_ZERO * circle_radius {
        return Err(SolidError::sm_err(
            "geom::geom_utility::p1::line_coplanar_circle_intersect",
        ));
    }
    line_vec /= scale;

    // Drop the circle center to the line.
    let t = line_closest_point(line_pnt, line_vec, circle_center);
    let cl_pt = line_pnt + t * line_vec;

    let vec_to_cl_pt = cl_pt - circle_center;
    let dist_to_ctr_sq = vec_to_cl_pt.dot(vec_to_cl_pt);

    let dist_to_ctr = sm_sqrt(dist_to_ctr_sq);

    // 1. Check too far away.
    if dist_to_ctr > circle_radius + tolerance {
        return Ok(Vec::new());
    }

    // 2. Check for just tangent, or miss within tol: one int.
    if dist_to_ctr >= circle_radius {
        return Ok(vec![t / scale]);
    }

    let dist_to_move_sq = (circle_radius + dist_to_ctr) * (circle_radius - dist_to_ctr);
    let dist_to_move = sm_sqrt(dist_to_move_sq);

    if dist_to_ctr <= circle_radius - tolerance {
        // Here there are definitely two solutions.
        return Ok(vec![(t - dist_to_move) / scale, (t + dist_to_move) / scale]);
    }

    // Next test: if the resulting intersection points will be within
    // tolerance of each other, then it's definitely one intersection.
    if dist_to_move * 2.0 <= tolerance {
        return Ok(vec![t / scale]);
    }

    // Now we're in the fuzzy area.
    let t0 = t - dist_to_move; // these are still 3d distances.
    let t1 = t + dist_to_move;

    let int_pt0 = line_pnt + t0 * line_vec;
    let dist_to_int = int_pt0.distance(circle_center);

    let error_drop_pt = (circle_radius - dist_to_ctr).abs();
    let error_int_pts = (circle_radius - dist_to_int).abs();

    if error_drop_pt < error_int_pts {
        Ok(vec![t / scale])
    } else {
        Ok(vec![t0 / scale, t1 / scale])
    }
}

/// Result of [`circle_coplanar_circle_intersect`] (`rlNumIntersections` and
/// `aIntersectionPoints`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CircleCircle {
    /// No intersection (count 0): too far apart, nested, or concentric.
    None,
    /// Tangent circles, within tolerance (count 1).
    Tangent(Point3),
    /// Two crossing points (count 2).
    Crossing(Point3, Point3),
    /// Coincident circles (count 3).
    Coincident,
}

impl CircleCircle {
    /// Upstream's `rlNumIntersections`.
    #[must_use]
    pub const fn count(&self) -> u32 {
        match self {
            Self::None => 0,
            Self::Tangent(_) => 1,
            Self::Crossing(..) => 2,
            Self::Coincident => 3,
        }
    }
}

/// Intersects two circles in the plane through circle 1 with normal `circle_normals`
/// (circle 2's center is projected into it) (`smgu_CircleCoPlanarCircleIntersect`).
///
/// Errors: from [`triangle_decomposition`] or a failed unitize in the two-point case.
pub fn circle_coplanar_circle_intersect(
    circle_normals: Vec3,
    circle1_center: Point3,
    circle1_radius: f64,
    circle2_center_in: Point3,
    circle2_radius: f64,
    tolerance: f64,
) -> Result<CircleCircle> {
    // project circle2 Center to circle1 Plane
    let circle2_center = point_project_to_plane(circle2_center_in, circle1_center, circle_normals);

    // check center/center distance
    let cc_dist = circle1_center.distance(circle2_center);

    // when centers are coincident
    if cc_dist < tolerance {
        return Ok(if (circle1_radius - circle2_radius).abs() < tolerance {
            CircleCircle::Coincident
        } else {
            CircleCircle::None
        });
    }

    if cc_dist > circle1_radius + circle2_radius + tolerance
        || cc_dist + circle1_radius < circle2_radius - tolerance
        || cc_dist + circle2_radius < circle1_radius - tolerance
    {
        return Ok(CircleCircle::None); // no intersections too far apart
    }

    let zero = EFF_ZERO * (1.0 + circle1_radius + circle2_radius);
    if cc_dist > circle1_radius + circle2_radius - zero
        || circle1_radius > cc_dist + circle2_radius - zero
        || circle2_radius > cc_dist + circle1_radius - zero
    {
        // Tangent intersection - from either inside or outside.
        let mut vec = circle2_center - circle1_center;
        vec /= cc_dist; // Unitize

        let scaled_zero = EFF_ZERO * (1.0 + circle1_center.max_dimension());
        let (pt1, pt2) = if cc_dist < circle1_radius + circle2_radius - scaled_zero {
            // Inside Cases
            if circle2_radius < circle1_radius {
                (
                    circle1_center + circle1_radius * vec,
                    circle2_center_in + circle2_radius * vec,
                )
            } else {
                (
                    circle1_center - circle1_radius * vec,
                    circle2_center_in - circle2_radius * vec,
                )
            }
        } else {
            // Outside Case
            (
                circle1_center + circle1_radius * vec,
                circle2_center_in - circle2_radius * vec,
            )
        };

        return Ok(CircleCircle::Tangent(0.5 * (pt1 + pt2)));
    }

    // arrive here when two circles intersect in two points.
    let (dist_along, height) = triangle_decomposition(cc_dist, circle1_radius, circle2_radius)?;

    // build Vec0
    let cc_vec = (circle2_center - circle1_center).unitize()?;

    // build Vec1
    let perp_vec = cc_vec.cross(circle_normals).unitize()?;

    Ok(CircleCircle::Crossing(
        circle1_center + dist_along * cc_vec + height * perp_vec,
        circle1_center + dist_along * cc_vec - height * perp_vec,
    ))
}

/// Result of [`line_intersect_sphere`] (`rlNumberResults` and `adResults`), as line
/// parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineSphere {
    /// The line is farther than the tolerance from the sphere (count 0).
    Miss,
    /// Touching, up to the tolerance outside the sphere (count 1).
    Touch(f64),
    /// Through the sphere by more than the tolerance (count 2).
    Through(f64, f64),
    /// Grazing: inside the sphere by less than the tolerance (count 3): first
    /// intersection, closest point, second intersection.
    Grazing(f64, f64, f64),
}

/// Intersects the line `line_pnt + t * line_vec` with a sphere, paying attention to
/// `tolerance` (`smgu_LineIntersectSphere`).
///
/// Errors: `InvalidInput` when `|line_vec| < SM_EFF_ZERO`; `SM_ERR` (1001) when the
/// discriminant of a two-point case is below `SM_EFF_ZERO`.
pub fn line_intersect_sphere(
    line_pnt: Vec3,
    line_vec: Vec3,
    sphere_center: Vec3,
    sphere_radius: f64,
    tolerance: f64,
) -> Result<LineSphere> {
    const OP: &str = "geom::geom_utility::p1::line_intersect_sphere";
    let length_v = line_vec.length();

    if length_v < EFF_ZERO {
        return Err(invalid(OP, "zero-length line vector"));
    }

    let unit_v = line_vec / length_v;

    let eo = sphere_center - line_pnt;
    let length_squared_eo = eo.length_squared();
    let v = eo.dot(unit_v); // projection of EO to line vector
    let b = sm_sqrt(length_squared_eo - (v * v));

    // no intersection - line further from Sphere Center than dTolerance
    if b > sphere_radius + tolerance {
        return Ok(LineSphere::Miss);
    }

    // tangent - exactly touching (SM_EFF_ZERO) to dTolerance outside the sphere
    if b > sphere_radius - EFF_ZERO * (1.0 + sphere_radius) {
        return Ok(LineSphere::Touch(v / length_v));
    }

    // compute 1/2 distance between the point solutions
    let disc = sphere_radius * sphere_radius - b * b;
    if disc < EFF_ZERO {
        return Err(SolidError::sm_err(OP)); // can't really happen
    }
    let d = sm_sqrt(disc);

    // line distance lies from just about touching (SM_EFF_ZERO) to inside Sphere by dTol
    if b > sphere_radius - tolerance {
        return Ok(LineSphere::Grazing(
            (v - d) / length_v,
            v / length_v,
            (v + d) / length_v,
        ));
    }

    Ok(LineSphere::Through((v - d) / length_v, (v + d) / length_v))
}

// ---- planes --------------------------------------------------------------------------

/// The common point of three planes given by points and unit normals
/// (`smgu_IntersectThreePlanes`).
///
/// Errors: `InvalidInput` when a normal is not unit length (within `SM_EFF_ZERO`) or
/// the normals are (nearly) dependent.
pub fn intersect_three_planes(
    p1: Point3,
    v1: Vec3,
    p2: Point3,
    v2: Vec3,
    p3: Point3,
    v3: Vec3,
) -> Result<Point3> {
    const OP: &str = "geom::geom_utility::p1::intersect_three_planes";
    // First make sure vectors are unitized
    if (v1.dot(v1) - 1.0).abs() > EFF_ZERO
        || (v2.dot(v2) - 1.0).abs() > EFF_ZERO
        || (v3.dot(v3) - 1.0).abs() > EFF_ZERO
    {
        return Err(invalid(OP, "plane normal is not unit length"));
    }

    let det = determinant_3_vectors(v1, v2, v3);

    // Planes do not intersect - must be parallel
    if det.abs() < EFF_ZERO {
        return Err(invalid(OP, "parallel planes"));
    }

    let work =
        (p1.dot(v1) * v2.cross(v3)) + (p2.dot(v2) * v3.cross(v1)) + (p3.dot(v3) * v1.cross(v2));
    Ok(work / det)
}

/// The point at `uv` on the plane of a reference frame: `origin + u X + v Y`
/// (`smgu_PlaneEvaluatePoint`).
#[must_use]
pub fn plane_evaluate_point(plane: &Axis2Placement, uv: Point2) -> Point3 {
    plane.origin() + uv.x * plane.x_axis() + uv.y * plane.y_axis()
}

/// The `(X, Y)` coordinates of `point` dropped onto the plane of a reference frame
/// (`smgu_PlaneDropPoint`).
#[must_use]
pub fn plane_drop_point(plane: &Axis2Placement, point: Point3) -> Point2 {
    let vec = point - plane.origin();
    Point2::new(plane.x_axis().dot(vec), plane.y_axis().dot(vec))
}

/// Distance from `test_point` to the plane through `plane_pnt` with unit normal
/// `plane_norm` (`smgu_PlanePointDistance`).
///
/// Errors: `SM_ERR` (1001) when `|plane_norm|^2 - 1 > SM_EFF_ZERO` (a too-short normal
/// is accepted, as upstream).
pub fn plane_point_distance(
    plane_pnt: Point3,
    plane_norm: Vec3,
    test_point: Point3,
) -> Result<f64> {
    if plane_norm.length_squared() - 1.0 > EFF_ZERO {
        return Err(SolidError::sm_err(
            "geom::geom_utility::p1::plane_point_distance",
        ));
    }

    let proj_vec = test_point - plane_pnt;
    let dot = proj_vec.dot(plane_norm);
    Ok(dot.abs())
}

/// Normal projection of `point` onto the plane through `plane_pnt` with (not
/// necessarily unit) normal `plane_norm` (`smgu_PointProjectToPlane`).
#[must_use]
pub fn point_project_to_plane(point: Point3, plane_pnt: Point3, plane_norm: Vec3) -> Point3 {
    let k = ((point.x - plane_pnt.x) * plane_norm.x
        + (point.y - plane_pnt.y) * plane_norm.y
        + (point.z - plane_pnt.z) * plane_norm.z)
        / plane_norm.dot(plane_norm);

    point - k * plane_norm
}

/// The component of `vector` not along the unit `plane_norm`
/// (`smgu_VectorProjectToPlane`).
#[must_use]
pub fn vector_project_to_plane(vector: Vec3, plane_norm: Vec3) -> Vec3 {
    vector - (vector.dot(plane_norm)) * plane_norm
}

/// Result of [`transform_point_to_start_plane`].
#[derive(Debug, Clone, PartialEq)]
pub struct StartPlanePoint {
    /// The point rotated about the Z axis onto the X/Z half plane (`rTransformedPoint`).
    pub point: Point3,
    /// Distance from the point to the Z axis (`rdDistToZAxis`).
    pub dist_to_z_axis: f64,
    /// One angle, or two for a point on the axis or on the seam (`rlNumAngles`,
    /// `adAnglesDeg`), in degrees.
    pub angles_deg: Vec<f64>,
    /// The angle is inside the sweep interval (`bInside`).
    pub inside: bool,
}

/// Rotates `point` about the Z axis of `position` onto its X/Z half plane and returns
/// the rotation angle(s) in degrees mapped into `sweep_ivl_deg`
/// (`smgu_TransformPointToStartPlane`). Points within `dist_tol_3d` of the seam of a
/// closed sweep give two angles (both snapped to the seam when `snap_to_seams`).
///
/// Errors: propagates `CCWAngleBetween`.
pub fn transform_point_to_start_plane(
    point: Point3,
    position: &Axis2Placement,
    sweep_ivl_deg: &Extent1d,
    dist_tol_3d: f64,
    snap_to_seams: bool,
) -> Result<StartPlanePoint> {
    let center = position.origin();
    let z_axis = position.z_axis();

    // get distance from target point to Z_Axis
    let (dist_to_z_axis, _) = line_point_distance(center, z_axis, point);

    let scaled_zero = EFF_ZERO * (1.0 + point.max_dimension());

    // When Point is on the axis of revolution - no unique solutions
    if dist_to_z_axis < scaled_zero {
        return Ok(StartPlanePoint {
            point,
            dist_to_z_axis,
            angles_deg: vec![sweep_ivl_deg.min(), sweep_ivl_deg.max()],
            inside: true,
        });
    }

    let x_axis = position.x_axis();
    let vec_to_pnt = point - center;
    let ang_tol_deg = ANALYTIC_TOL_SCALE * EFF_ZERO * (1.0 + 360.0);

    // get angle in degrees about Z_Axis from X_Axis to Target Point in degrees
    let mut angle_rad = z_axis.ccw_angle_between(x_axis, vec_to_pnt)?;

    // get rid of numeric noise at zero
    if angle_rad < EFF_ZERO {
        if angle_rad > -EFF_ZERO {
            angle_rad = 0.0;
        } else {
            angle_rad += 2.0 * PI;
        }
    }
    let angle_deg = rad_to_deg(angle_rad);

    // map periodic values into interval
    let contained = sweep_ivl_deg.contains_periodic_value(angle_deg, 360.0, -1.0);
    let inside = contained.contained;
    let mut angle_deg = contained.periodic_value;

    // watchout for periodicity
    if !inside && angle_deg < 0.0 {
        angle_deg += 360.0;
    }

    // Get an angle tolerance from 3d values.
    let bdry_tol_deg = rad_to_deg(dist_tol_3d / dist_to_z_axis);

    let angles_deg = if are_same_to_tol(sweep_ivl_deg.length(), 360.0, ang_tol_deg)
        && sweep_ivl_deg
            .is_value_on_periodic_boundary(angle_deg, 360.0, bdry_tol_deg)
            .0
    {
        if snap_to_seams {
            vec![sweep_ivl_deg.min(), sweep_ivl_deg.max()]
        } else if (sweep_ivl_deg.min() - angle_deg).abs() < (sweep_ivl_deg.max() - angle_deg).abs()
        {
            vec![angle_deg, sweep_ivl_deg.max()]
        } else {
            vec![sweep_ivl_deg.min(), angle_deg]
        }
    } else {
        vec![angle_deg]
    };

    // Now rotate the point about the Z axis to the Z, X plane
    let dot_z = z_axis.dot(vec_to_pnt);
    let xy_proj = vector_project_to_plane(vec_to_pnt, z_axis);
    let x_size = xy_proj.length();
    Ok(StartPlanePoint {
        point: center + z_axis * dot_z + x_axis * x_size,
        dist_to_z_axis,
        angles_deg,
        inside,
    })
}

/// Perspective projection of `point` from `eye_point` onto the plane through
/// `plane_point` with unit normal `plane_unit_norm` (`smgu_PointPerspectiveToPlane`).
#[must_use]
pub fn point_perspective_to_plane(
    point: Point3,
    plane_point: Point3,
    plane_unit_norm: Vec3,
    eye_point: Vec3,
) -> Point3 {
    point.perspective_project_point_to_plane(plane_point, plane_unit_norm, eye_point)
}

/// `(u, v)` with `vector = u * basis1 + v * basis2` for the projection of `vector`
/// into the plane of the basis (`smgu_VecLinCombTwoVectors`).
///
/// Errors: `SM_ERR` (1001) when the basis vectors do not span a plane.
pub fn vec_lin_comb_two_vectors(basis1: Vec3, basis2: Vec3, vector: Vec3) -> Result<Point2> {
    const OP: &str = "geom::geom_utility::p1::vec_lin_comb_two_vectors";
    let a00 = basis1.dot(basis1);
    let a01 = basis1.dot(basis2);
    let a10 = a01;
    let a11 = basis2.dot(basis2);

    let det = a00 * a11 - a10 * a01;

    let b0 = basis1.dot(vector);
    let b1 = basis2.dot(vector);

    let numer0 = b0 * a11 - b1 * a01;
    let numer1 = a00 * b1 - a10 * b0;

    // `det == 0.0` in case dNumer0 or dNumer1 is 0 exactly.
    if det == 0.0 || det.abs() < numer0.abs() * EFF_ZERO || det.abs() < numer1.abs() * EFF_ZERO {
        return Err(SolidError::sm_err(OP));
    }

    Ok(Point2::new(numer0 / det, numer1 / det))
}

// ---- best fit ------------------------------------------------------------------------

/// Classification of a point set (`SmPointSetType`); discriminants are the upstream
/// values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointSetType {
    /// `SM_PST_UNKNOWN`.
    Unknown = 0,
    /// `SM_PST_VOID`: empty point set.
    Void = 1,
    /// `SM_PST_POINTSIZED`: all points within tolerance of the centroid.
    PointSized = 2,
    /// `SM_PST_LINEAR`: all points within tolerance of a line through the centroid.
    Linear = 3,
    /// `SM_PST_PLANAR`: all points within tolerance of a plane through the centroid.
    Planar = 4,
    /// `SM_PST_SCATTERED`: none of the above.
    Scattered = 5,
}

impl PointSetType {
    /// Upstream enum value (`SM_PST_UNKNOWN` = 0 ... `SM_PST_SCATTERED` = 5).
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// Result of [`best_fit_point_to_points`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BestFitPoint {
    /// Void, point-sized or scattered (`eSetType`).
    pub set_type: PointSetType,
    /// Centroid (`rCenterPt`).
    pub center: Point3,
    /// Largest distance from a point to the centroid (`rdMaxGap`).
    pub max_gap: f64,
}

/// Centroid of `points` and the largest distance from it, classified against `tol_3d`
/// (`smgu_BestFitPointToPoints`).
#[must_use]
pub fn best_fit_point_to_points(points: &[Point3], tol_3d: f64) -> BestFitPoint {
    let mut center = Vec3::ZERO;
    let mut max_gap = 0.0;

    match points {
        [] => {
            return BestFitPoint {
                set_type: PointSetType::Void,
                center,
                max_gap,
            }
        }
        [only] => {
            return BestFitPoint {
                set_type: PointSetType::PointSized,
                center: *only,
                max_gap,
            }
        }
        _ => {}
    }

    for p in points {
        center += *p;
    }
    center /= points.len() as f64;

    for p in points {
        let dist2 = center.distance_squared(*p);
        if max_gap < dist2 {
            max_gap = dist2;
        }
    }
    max_gap = sm_sqrt(max_gap);

    BestFitPoint {
        set_type: if max_gap <= tol_3d {
            PointSetType::PointSized
        } else {
            PointSetType::Scattered
        },
        center,
        max_gap,
    }
}

/// Result of [`best_fit_line_to_points`].
#[derive(Debug, Clone, PartialEq)]
pub struct BestFitLine {
    /// Void, point-sized, linear or scattered (`eSetType`).
    pub set_type: PointSetType,
    /// Line point, the centroid (`rLinePt`).
    pub line_pt: Point3,
    /// Unit line direction; zero for void and point-sized sets (`rLineVec`).
    pub line_vec: Vec3,
    /// Largest distance from a point to the line (`0` for void and point-sized sets)
    /// (`rdMaxLineGap`).
    pub max_line_gap: f64,
    /// Largest distance from a point to the centroid (`rdMaxCentroidGap`).
    pub max_centroid_gap: f64,
    /// The 3x3 matrix `M(LinePt)` (upstream optional `pOptM`), computed only when the
    /// points are not point-sized.
    pub m: Option<Matrix>,
}

/// Least-squares line through `points`: the centroid and the third eigenvector
/// (`aEigenVectors[2]`, upstream's "largest eigenvalue") of `C = del I - M` from
/// [`Matrix::compute_3x3_eigen_vectors`], classified against `tol_3d`
/// (`smgu_BestFitLineToPoints`). As upstream, that vector is not always the
/// least-squares direction (for axis-aligned collinear points it is perpendicular to
/// them); see `plan/findings.md`.
///
/// Errors: from the matrix helpers (not for finite input).
pub fn best_fit_line_to_points(points: &[Point3], tol_3d: f64) -> Result<BestFitLine> {
    let fit = best_fit_point_to_points(points, tol_3d);
    let line_pt = fit.center;

    if fit.set_type != PointSetType::Scattered {
        return Ok(BestFitLine {
            set_type: fit.set_type,
            line_pt,
            line_vec: Vec3::ZERO,
            max_line_gap: 0.0,
            max_centroid_gap: fit.max_gap,
            m: None,
        });
    }

    let mut del = 0.0;
    let mut m = Matrix::new(3, 3);
    let mut c = Matrix::new(3, 3);

    for p in points {
        let yx = p.x - line_pt.x;
        let yy = p.y - line_pt.y;
        let yz = p.z - line_pt.z;

        m.add_to(0, 0, yx * yx)?;
        m.add_to(0, 1, yx * yy)?;
        m.add_to(0, 2, yx * yz)?;
        m.add_to(1, 1, yy * yy)?;
        m.add_to(1, 2, yy * yz)?;
        m.add_to(2, 2, yz * yz)?;

        del += yx * yx + yy * yy + yz * yz;
    }

    // finish full symmetric M matrix
    m.set_at(1, 0, m.at(0, 1)?)?;
    m.set_at(2, 0, m.at(0, 2)?)?;
    m.set_at(2, 1, m.at(1, 2)?)?;

    c.set_at(0, 0, del - m.at(0, 0)?)?;
    c.set_at(0, 1, m.at(0, 1)?)?;
    c.set_at(0, 2, m.at(0, 2)?)?;

    c.set_at(1, 0, m.at(1, 0)?)?;
    c.set_at(1, 1, del - m.at(1, 1)?)?;
    c.set_at(1, 2, m.at(1, 2)?)?;

    c.set_at(2, 0, m.at(2, 0)?)?;
    c.set_at(2, 1, m.at(2, 1)?)?;
    c.set_at(2, 2, del - m.at(2, 2)?)?;

    // Get Eigenvalues and EigenVectors of C(LinePt)
    let (_, [_, _, largest]) = c.compute_3x3_eigen_vectors()?;

    // LineVec = unitized(aEigenVectors[2]) -- largest Eigenvalue.
    let line_vec = largest.unitize_or_z();

    let mut max_line_gap = 0.0;
    for p in points {
        let (dist, _) = line_point_distance(line_pt, line_vec, *p);
        if max_line_gap < dist {
            max_line_gap = dist;
        }
    }

    Ok(BestFitLine {
        set_type: if max_line_gap <= tol_3d {
            PointSetType::Linear
        } else {
            PointSetType::Scattered
        },
        line_pt,
        line_vec,
        max_line_gap,
        max_centroid_gap: fit.max_gap,
        m: Some(m),
    })
}

// ---- small matrices ------------------------------------------------------------------

/// `a · (b × c)` of three 3-vectors given as arrays (`smgu_VecTripleProduct`).
#[must_use]
pub fn vec_triple_product(a: &[f64; 3], b: &[f64; 3], c: &[f64; 3]) -> f64 {
    let [a0, a1, a2] = *a;
    let [b0, b1, b2] = *b;
    let [c0, c1, c2] = *c;
    a0 * (b1 * c2 - c1 * b2) + a1 * (b2 * c0 - c2 * b0) + a2 * (b0 * c1 - c0 * b1)
}

/// A copy of a 3x3 matrix (`smgu_3x3Copy`).
#[must_use]
pub fn copy_3x3(from: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    *from
}

/// Determinant of the 3x3 matrix with rows `(a00 a01 a02)`, `(a10 a11 a12)`,
/// `(a20 a21 a22)` (`smgu_3x3Determinate`, coefficient overload).
#[must_use]
pub fn determinate_3x3(
    a00: f64,
    a01: f64,
    a02: f64,
    a10: f64,
    a11: f64,
    a12: f64,
    a20: f64,
    a21: f64,
    a22: f64,
) -> f64 {
    a00 * (a11 * a22 - a12 * a21) + a01 * (a12 * a20 - a10 * a22) + a02 * (a10 * a21 - a11 * a20)
}

/// Determinant of the matrix with rows `v1`, `v2`, `v3` (`smgu_Determinant3Vectors`).
#[must_use]
pub fn determinant_3_vectors(v1: Vec3, v2: Vec3, v3: Vec3) -> f64 {
    v1.x * v2.y * v3.z - v1.x * v2.z * v3.y + v1.y * v2.z * v3.x - v1.y * v2.x * v3.z
        + v1.z * v2.x * v3.y
        - v1.z * v2.y * v3.x
}

/// Inverse of the 3x3 matrix with rows `a0`, `a1`, `a2` (`smgu_3x3Inverse`).
///
/// Errors: `SM_ERR` (1001) when the determinant is zero (`SM_IS_ZERO`).
pub fn inverse_3x3(a0: &[f64; 3], a1: &[f64; 3], a2: &[f64; 3]) -> Result<[[f64; 3]; 3]> {
    let det = vec_triple_product(a0, a1, a2);

    if is_zero(det) {
        return Err(SolidError::sm_err("geom::geom_utility::p1::inverse_3x3"));
    }

    let [a00, a01, a02] = *a0;
    let [a10, a11, a12] = *a1;
    let [a20, a21, a22] = *a2;

    Ok([
        [
            (a22 * a11 - a21 * a12) / det,
            -(a22 * a01 - a21 * a02) / det,
            (a12 * a01 - a11 * a02) / det,
        ],
        [
            -(a22 * a10 - a20 * a12) / det,
            (a22 * a00 - a20 * a02) / det,
            -(a12 * a00 - a10 * a02) / det,
        ],
        [
            (a21 * a10 - a20 * a11) / det,
            -(a21 * a00 - a20 * a01) / det,
            (a11 * a00 - a10 * a01) / det,
        ],
    ])
}

/// A copy of a 4x4 matrix (`smgu_4x4Copy`).
#[must_use]
pub fn copy_4x4(from: &[[f64; 4]; 4]) -> [[f64; 4]; 4] {
    *from
}

/// Determinant of a 4x4 matrix by cofactor expansion along the first row
/// (`smgu_4x4Determinate`).
#[must_use]
pub fn determinate_4x4(a: &[[f64; 4]; 4]) -> f64 {
    let [[a00, a01, a02, a03], [a10, a11, a12, a13], [a20, a21, a22, a23], [a30, a31, a32, a33]] =
        *a;
    a00 * (a11 * (a22 * a33 - a23 * a32)
        + a12 * (a23 * a31 - a21 * a33)
        + a13 * (a21 * a32 - a22 * a31))
        - a01
            * (a10 * (a22 * a33 - a23 * a32)
                + a12 * (a23 * a30 - a20 * a33)
                + a13 * (a20 * a32 - a22 * a30))
        + a02
            * (a10 * (a21 * a33 - a23 * a31)
                + a11 * (a23 * a30 - a20 * a33)
                + a13 * (a20 * a31 - a21 * a30))
        - a03
            * (a10 * (a21 * a32 - a22 * a31)
                + a11 * (a22 * a30 - a20 * a32)
                + a12 * (a20 * a31 - a21 * a30))
}

/// Real roots of `A x^2 + B x + C = 0` with `coefficients = [C, B, A]`
/// (`smgu_SolveQuadraticEqn`, a wrapper of `SmPolynomial::SolveQuadraticEqn`).
#[must_use]
pub fn solve_quadratic_eqn(coefficients: &[f64; 3], zero_tolerance: f64) -> Roots {
    solve_quadratic(coefficients, zero_tolerance)
}

// ---- directional derivatives ---------------------------------------------------------

fn deriv_at(derivs: &[Vec3], i: usize, op: &'static str) -> Result<Vec3> {
    derivs
        .get(i)
        .copied()
        .ok_or_else(|| invalid(op, "derivative array too short"))
}

/// Surface position and directional derivatives (up to the third) in the UV
/// direction `dir` (unitized), from `SmSurface::Evaluate` output `derivs` laid out
/// as `[(highest_u + 1) x (highest_v + 1)]` (`smgu_DirectionalDerivs`).
///
/// Returns `min(highest_u, highest_v, 3) + 1` vectors: the position, then the
/// derivatives upstream sets.
///
/// Errors: `InvalidInput` when a consumed derivative index overflows or `derivs` is
/// too short for the requested derivatives.
pub fn directional_derivs(
    dir: Vec2,
    highest_u_deriv: usize,
    highest_v_deriv: usize,
    derivs: &[Vec3],
) -> Result<Vec<Vec3>> {
    const OP: &str = "geom::geom_utility::p1::directional_derivs";
    let d = |u: usize, v: usize| {
        let i = highest_v_deriv
            .checked_add(1)
            .and_then(|stride| u.checked_mul(stride))
            .and_then(|i| i.checked_add(v))
            .ok_or_else(|| invalid(OP, "derivative index overflow"))?;
        deriv_at(derivs, i, OP)
    };

    // unitize sDir (status ignored: a zero vector stays unchanged)
    let dir = dir.unitize().unwrap_or(dir);
    let (x, y) = (dir.x, dir.y);

    let mut out = vec![deriv_at(derivs, 0, OP)?];

    if highest_u_deriv >= 1 && highest_v_deriv >= 1 {
        out.push(x * d(1, 0)? + y * d(0, 1)?);
    }

    if highest_u_deriv >= 2 && highest_v_deriv >= 2 {
        out.push(x * x * d(2, 0)? + 2.0 * x * y * d(1, 1)? + y * y * d(0, 2)?);
    }

    if highest_u_deriv >= 3 && highest_v_deriv >= 3 {
        out.push(
            x * x * x * d(3, 0)?
                + 3.0 * x * x * y * d(2, 1)?
                + 3.0 * x * y * y * d(1, 2)?
                + y * y * y * d(0, 3)?,
        );
    }

    Ok(out)
}

/// Volume position and directional derivatives (up to the third) in the UVW
/// direction `dir` (unitized; a zero vector becomes `(0, 0, 1)`), from evaluations
/// `derivs` laid out as `D[u][v][w]`, `[(n + 1)^3]` with `n = highest_deriv`
/// (`smgu_VolDirectionalDerivs`).
///
/// Returns `min(highest_deriv, 3) + 1` vectors.
///
/// Errors: `InvalidInput` when the layout stride or a consumed derivative index
/// overflows, or `derivs` is too short for the requested derivatives.
pub fn vol_directional_derivs(
    dir: Vec3,
    highest_deriv: usize,
    derivs: &[Vec3],
) -> Result<Vec<Vec3>> {
    const OP: &str = "geom::geom_utility::p1::vol_directional_derivs";
    let n1 = highest_deriv
        .checked_add(1)
        .ok_or_else(|| invalid(OP, "derivative index overflow"))?;
    let sv = |u: usize, v: usize, w: usize| {
        let i = u
            .checked_mul(n1)
            .and_then(|i| i.checked_add(v))
            .and_then(|i| i.checked_mul(n1))
            .and_then(|i| i.checked_add(w))
            .ok_or_else(|| invalid(OP, "derivative index overflow"))?;
        deriv_at(derivs, i, OP)
    };

    let dir = dir.unitize_or_z();
    let (x, y, z) = (dir.x, dir.y, dir.z);

    let mut out = vec![sv(0, 0, 0)?];

    if highest_deriv >= 1 {
        out.push(x * sv(1, 0, 0)? + y * sv(0, 1, 0)? + z * sv(0, 0, 1)?);
    }

    if highest_deriv >= 2 {
        out.push(
            x * x * sv(2, 0, 0)?
                + 2.0 * x * y * sv(1, 1, 0)?
                + 2.0 * x * z * sv(1, 0, 1)?
                + y * y * sv(0, 2, 0)?
                + 2.0 * y * z * sv(0, 1, 1)?
                + z * z * sv(0, 0, 2)?,
        );
    }

    if highest_deriv >= 3 {
        out.push(
            x * x * x * sv(3, 0, 0)?
                + 3.0 * x * x * y * sv(2, 1, 0)?
                + 3.0 * x * x * z * sv(2, 0, 1)?
                + 3.0 * x * y * y * sv(1, 2, 0)?
                + 6.0 * x * y * z * sv(1, 1, 1)?
                + 3.0 * x * z * z * sv(1, 0, 2)?
                + y * y * y * sv(0, 3, 0)?
                + 3.0 * y * y * z * sv(0, 2, 1)?
                + 3.0 * y * z * z * sv(0, 1, 2)?
                + z * z * z * sv(0, 0, 3)?,
        );
    }

    Ok(out)
}

/// True when the 2D point sequence `p0 p1 p2 p3` forms a dog leg (a 'z' shape):
/// the first and last segments are within `parall_dev_deg` of parallel and within
/// `bend_ang_deg` of perpendicular to the middle one, and the middle segment is
/// within `parall_dev_deg` of parallel to `p0 p3` (`smgu_IsDogLeg`, `SmVector2d`
/// overload).
#[must_use]
pub fn is_dog_leg(
    p0: Vec2,
    p1: Vec2,
    p2: Vec2,
    p3: Vec2,
    bend_ang_deg: f64,
    parall_dev_deg: f64,
) -> bool {
    let beg_vec = Vec3::from(p1 - p0);
    let mid_vec = Vec3::from(p2 - p1);
    let end_vec = Vec3::from(p3 - p2);
    let tot_vec = Vec3::from(p3 - p0);
    let beg_end_ang_rad = beg_vec.angle_between(end_vec).unwrap_or(0.0); // range:[0 Pi]

    rad_to_deg(beg_end_ang_rad) < parall_dev_deg
        && beg_vec.is_perpendicular_to(mid_vec, bend_ang_deg)
        && end_vec.is_perpendicular_to(mid_vec, bend_ang_deg)
        && tot_vec.is_parallel_to(mid_vec, parall_dev_deg)
}

// ---- sorting -------------------------------------------------------------------------

/// Inserts `value` into the sorted `sorted_values` before the first element that is
/// greater (`increasing`) or smaller (otherwise), and `idx` at the same position of
/// `sort_indices` (`smgu_AddSorted`).
pub fn add_sorted(
    value: f64,
    idx: u64,
    increasing: bool,
    sorted_values: &mut Vec<f64>,
    sort_indices: Option<&mut Vec<u64>>,
) {
    let found_idx = sorted_values
        .iter()
        .position(|&s| if increasing { s > value } else { s < value })
        .unwrap_or(sorted_values.len());

    sorted_values.insert(found_idx, value);
    if let Some(indices) = sort_indices {
        // `SmTArray::InsertAt` past the end grows the array with zeros.
        if found_idx >= indices.len() {
            indices.resize(found_idx, 0);
            indices.push(idx);
        } else {
            indices.insert(found_idx, idx);
        }
    }
}

/// Sorts `values` into ascending order with upstream's shell sort (gap halving, then
/// bubble passes until no swap) (`smgu_ShellSort_double_array`).
pub fn shell_sort_double_array(values: &mut [f64]) {
    let count = values.len();
    let mut flag = true;
    let mut d = count;

    while flag || d > 1 {
        flag = false; // reset flag to 0 to check for future swaps
        d = d.div_ceil(2);
        let mut ii = 0;
        while ii + d < count {
            if let (Some(&hi), Some(&lo)) = (values.get(ii + d), values.get(ii)) {
                if hi < lo {
                    values.swap(ii, ii + d);
                    flag = true; // tells swap has occurred
                }
            }
            ii += 1;
        }
    }
}

/// `dval1 < dval2` (`smgu_DoubleLessThan`).
#[must_use]
pub fn double_less_than(dval1: f64, dval2: f64) -> bool {
    dval1 < dval2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(x, y, z)
    }

    #[test]
    fn polygon_area_unit_square() {
        let pts = [v(0., 0., 0.), v(1., 0., 0.), v(1., 1., 0.), v(0., 1., 0.)];
        assert_eq!(polygon_area(&pts, Vec3::Z).unwrap(), 1.0);
        assert_eq!(polygon_area(&pts, -Vec3::Z).unwrap(), -1.0);
        assert!(polygon_area(&[], Vec3::Z).is_err());
    }

    #[test]
    fn triangle_measures() {
        let a2 = triangle_area_squared(v(0., 0., 0.), v(2., 0., 0.), v(0., 2., 0.));
        assert_eq!(a2, 4.0);
        let (along, h) = triangle_decomposition(2.0, 5.0f64.sqrt(), 5.0f64.sqrt()).unwrap();
        assert!((along - 1.0).abs() < 1e-12 && (h - 2.0).abs() < 1e-12);
        assert!(triangle_decomposition(0.0, 1.0, 1.0).is_err());
        assert!(triangle_decomposition(4.0, 1.0, 1.0).is_err());
    }

    #[test]
    fn line_helpers() {
        let (d, t) = line_point_distance(Vec3::ZERO, v(2., 0., 0.), v(1., 1., 0.));
        assert_eq!((d, t), (1.0, 0.5));
        let (d, t) = line_point_distance(Vec3::ZERO, Vec3::ZERO, v(0., 3., 4.));
        assert_eq!((d, t), (5.0, 0.0));
        let (t1, t2) =
            line_line_closest_point(Vec3::ZERO, Vec3::X, v(0., 0., 1.), Vec3::Y).unwrap();
        assert!(t1.abs() < 1e-15 && t2.abs() < 1e-15);
        assert!(line_line_closest_point(Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::X).is_err());
        assert_eq!(
            line_plane_intersect(Vec3::ZERO, Vec3::Z, v(0., 0., 3.), Vec3::Z).unwrap(),
            3.0
        );
        assert!(matches!(
            line_plane_intersect(Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z),
            Err(SolidError::InvalidInput { .. })
        ));
        assert!(matches!(
            line_plane_intersect(Vec3::ZERO, Vec3::X, Vec3::ZERO, Vec3::Z),
            Err(SolidError::Upstream { code: 1001, .. })
        ));
        let (t, u, w) =
            line_plane_intersect_uv(v(1., 2., 5.), -Vec3::Z, Vec3::ZERO, Vec3::X, Vec3::Y).unwrap();
        assert_eq!((t, u, w), (5.0, 1.0, 2.0));
        let (d, p) = segment_point_distance(Vec3::ZERO, Vec3::X, v(2., 1., 0.));
        assert_eq!(p, 1.0);
        assert!((d - 2f64.sqrt()).abs() < 1e-15);
        let (a, p) = segment_point_angle_deg_to_perp(Vec3::ZERO, Vec3::X, v(0.5, 1., 0.));
        assert_eq!((a, p), (0.0, 0.5));
        let (a, p) = segment_point_angle_deg_to_perp(Vec3::ZERO, Vec3::X, v(2., 1., 0.));
        assert_eq!(p, 1.0);
        assert!((a - 45.0).abs() < 1e-12);
    }

    #[test]
    fn circles() {
        let c = circle_center_from_3_points(
            Point2::new(1., 0.),
            Point2::new(0., 1.),
            Point2::new(-1., 0.),
        )
        .unwrap();
        assert!(c.x.abs() < 1e-15 && c.y.abs() < 1e-15);
        assert!(matches!(
            circle_center_from_3_points(
                Point2::new(1., 0.),
                Point2::new(1., 0.),
                Point2::new(2., 0.)
            ),
            Err(SolidError::Upstream { code: 0, .. })
        ));
        let c =
            circle_from_3_points(v(1., 0., 0.), v(0., 1., 0.), v(-1., 0., 0.), 3, false).unwrap();
        assert!((c.radius - 1.0).abs() < 1e-15);
        assert!((c.domain.max() - 180.0).abs() < 1e-9);
        assert!(
            circle_from_3_points(v(1., 0., 0.), v(0., 1., 0.), v(-1., 0., 0.), 4, false).is_err()
        );
        assert!(
            circle_from_3_points(v(0., 0., 0.), v(1., 0., 0.), v(2., 0., 0.), 3, false).is_err()
        );

        let hits =
            line_coplanar_circle_intersect(v(-5., 0., 1.), Vec3::X, Vec3::ZERO, Vec3::Z, 1.0, 1e-6)
                .unwrap();
        assert_eq!(hits, vec![4.0, 6.0]);
        let r =
            circle_coplanar_circle_intersect(Vec3::Z, Vec3::ZERO, 1.0, v(2., 0., 0.), 1.0, 1e-6)
                .unwrap();
        assert_eq!(r, CircleCircle::Tangent(v(1., 0., 0.)));
        let r = circle_coplanar_circle_intersect(Vec3::Z, Vec3::ZERO, 1.0, Vec3::ZERO, 1.0, 1e-6)
            .unwrap();
        assert_eq!(r.count(), 3);
        let r =
            circle_coplanar_circle_intersect(Vec3::Z, Vec3::ZERO, 1.0, v(1., 0., 0.), 1.0, 1e-6)
                .unwrap();
        assert_eq!(r.count(), 2);
    }

    #[test]
    fn sphere_and_planes() {
        let r = line_intersect_sphere(v(-5., 0., 0.), Vec3::X, Vec3::ZERO, 1.0, 1e-6).unwrap();
        assert_eq!(r, LineSphere::Through(4.0, 6.0));
        let r = line_intersect_sphere(v(-5., 2., 0.), Vec3::X, Vec3::ZERO, 1.0, 1e-6).unwrap();
        assert_eq!(r, LineSphere::Miss);
        assert!(line_intersect_sphere(Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, 1.0, 1e-6).is_err());
        let p = intersect_three_planes(
            v(1., 0., 0.),
            Vec3::X,
            v(0., 2., 0.),
            Vec3::Y,
            v(0., 0., 3.),
            Vec3::Z,
        )
        .unwrap();
        assert_eq!(p, v(1., 2., 3.));
        assert!(intersect_three_planes(
            Vec3::ZERO,
            Vec3::X,
            Vec3::ZERO,
            Vec3::X,
            Vec3::ZERO,
            Vec3::Z
        )
        .is_err());
        assert_eq!(
            plane_point_distance(Vec3::ZERO, Vec3::Z, v(1., 1., -2.)).unwrap(),
            2.0
        );
        assert!(plane_point_distance(Vec3::ZERO, v(0., 0., 2.), Vec3::Z).is_err());
        assert_eq!(
            point_project_to_plane(v(1., 1., 5.), Vec3::ZERO, v(0., 0., 2.)),
            v(1., 1., 0.)
        );
        let a2p = Axis2Placement::new(v(1., 1., 1.), Vec3::X, Vec3::Y);
        let p = plane_evaluate_point(&a2p, Point2::new(2., 3.));
        assert_eq!(p, v(3., 4., 1.));
        assert_eq!(plane_drop_point(&a2p, p), Point2::new(2., 3.));
    }

    #[test]
    fn start_plane() {
        let a2p = Axis2Placement::new(Vec3::ZERO, Vec3::X, Vec3::Y);
        let sweep = Extent1d::from_min_max(0.0, 360.0);
        let r = transform_point_to_start_plane(v(0., 2., 1.), &a2p, &sweep, 1e-6, true).unwrap();
        assert_eq!(r.angles_deg.len(), 1);
        assert!((r.angles_deg[0] - 90.0).abs() < 1e-12);
        assert!((r.point - v(2., 0., 1.)).length() < 1e-12);
        let r = transform_point_to_start_plane(v(2., 0., 1.), &a2p, &sweep, 1e-6, true).unwrap();
        assert_eq!(r.angles_deg, vec![0.0, 360.0]);
        let r = transform_point_to_start_plane(v(0., 0., 1.), &a2p, &sweep, 1e-6, true).unwrap();
        assert_eq!(r.angles_deg, vec![0.0, 360.0]);
    }

    #[test]
    fn best_fit() {
        let r = best_fit_point_to_points(&[], 1e-6);
        assert_eq!(r.set_type, PointSetType::Void);
        let pts = [v(0., 0., 0.), v(1., 0., 0.), v(2., 0., 0.), v(3., 0., 0.)];
        let l = best_fit_line_to_points(&pts, 1e-6).unwrap();
        // As upstream: for axis-aligned collinear points `aEigenVectors[2]` is
        // perpendicular to the points (see plan/findings.md), so they are scattered.
        assert_eq!(l.set_type, PointSetType::Scattered);
        assert!(l.line_vec.x.abs() < 1e-12);
        assert!((l.max_line_gap - 1.5).abs() < 1e-12);
        assert!(l.m.is_some());
        let pts = [v(0., 0., 0.), v(1., 0., 0.), v(0., 1., 0.)];
        let l = best_fit_line_to_points(&pts, 1e-6).unwrap();
        assert_eq!(l.set_type, PointSetType::Scattered);
    }

    #[test]
    fn small_matrices() {
        let m = [[2.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 8.0]];
        assert_eq!(copy_3x3(&m), m);
        assert_eq!(determinate_3x3(2., 0., 0., 0., 4., 0., 0., 0., 8.), 64.0);
        let inv = inverse_3x3(&m[0], &m[1], &m[2]).unwrap();
        assert_eq!(inv, [[0.5, 0.0, 0.0], [0.0, 0.25, 0.0], [0.0, 0.0, 0.125]]);
        assert!(inverse_3x3(&[1., 0., 0.], &[1., 0., 0.], &[0., 0., 1.]).is_err());
        let mut id4 = [[0.0; 4]; 4];
        for (i, row) in id4.iter_mut().enumerate() {
            row[i] = 2.0;
        }
        assert_eq!(determinate_4x4(&id4), 16.0);
        assert_eq!(copy_4x4(&id4), id4);
        assert_eq!(determinant_3_vectors(Vec3::X, Vec3::Y, Vec3::Z), 1.0);
        assert_eq!(
            vec_triple_product(&[1., 0., 0.], &[0., 1., 0.], &[0., 0., 1.]),
            1.0
        );
        assert_eq!(
            &*solve_quadratic_eqn(&[-1.0, 0.0, 1.0], 1e-12),
            &[1.0, -1.0]
        );
    }

    #[test]
    fn derivs() {
        // f(u, v) = (u, v, u*v): layout [u][v] with hu = hv = 1.
        let d = [Vec3::ZERO, Vec3::Y, Vec3::X, Vec3::Z];
        let r = directional_derivs(Vec2::new(3.0, 0.0), 1, 1, &d).unwrap();
        assert_eq!(r, vec![Vec3::ZERO, Vec3::X]);
        assert!(directional_derivs(Vec2::new(1.0, 0.0), 2, 2, &d).is_err());
        let mut vd = vec![Vec3::ZERO; 8];
        vd[4] = Vec3::X; // Du
        vd[2] = Vec3::Y; // Dv
        vd[1] = Vec3::Z; // Dw
        let r = vol_directional_derivs(v(0., 0., 2.), 1, &vd).unwrap();
        assert_eq!(r, vec![Vec3::ZERO, Vec3::Z]);
    }

    #[test]
    fn derivs_address_overflow() {
        let surface_op = "geom::geom_utility::p1::directional_derivs";
        let volume_op = "geom::geom_utility::p1::vol_directional_derivs";
        assert_eq!(
            directional_derivs(Vec2::new(1.0, 0.0), 1, usize::MAX, &[Vec3::ZERO; 2]),
            Err(invalid(surface_op, "derivative index overflow"))
        );
        assert_eq!(
            vol_directional_derivs(Vec3::Z, usize::MAX, &[Vec3::ZERO; 4]),
            Err(invalid(volume_op, "derivative index overflow"))
        );
        let k = 1usize << (usize::BITS / 2);
        assert_eq!(
            vol_directional_derivs(Vec3::Z, k, &[Vec3::ZERO]),
            Err(invalid(volume_op, "derivative index overflow"))
        );
        // Zero coefficients still consume the same derivative entries.
        assert_eq!(
            directional_derivs(Vec2::ZERO, 1, usize::MAX, &[Vec3::ZERO; 2]),
            Err(invalid(surface_op, "derivative index overflow"))
        );
        // Surface position is read before its branch-local stride arithmetic.
        assert_eq!(
            directional_derivs(Vec2::ZERO, 1, usize::MAX, &[]),
            Err(invalid(surface_op, "derivative array too short"))
        );
        // Volume checks n+1 before reading position, but not unused products.
        assert_eq!(
            vol_directional_derivs(Vec3::ZERO, usize::MAX, &[]),
            Err(invalid(volume_op, "derivative index overflow"))
        );
        assert_eq!(
            vol_directional_derivs(Vec3::ZERO, k, &[]),
            Err(invalid(volume_op, "derivative array too short"))
        );
    }

    #[test]
    fn derivs_position_short_and_zero_directions() {
        let p = v(7.0, -2.0, 5.0);
        let surface_op = "geom::geom_utility::p1::directional_derivs";
        let volume_op = "geom::geom_utility::p1::vol_directional_derivs";
        for (hu, hv) in [(0, 0), (0, usize::MAX), (usize::MAX, 0), (0, 4), (4, 0)] {
            assert_eq!(directional_derivs(Vec2::ZERO, hu, hv, &[p]), Ok(vec![p]));
            assert_eq!(
                directional_derivs(Vec2::ZERO, hu, hv, &[]),
                Err(invalid(surface_op, "derivative array too short"))
            );
        }
        assert_eq!(vol_directional_derivs(Vec3::ZERO, 0, &[p]), Ok(vec![p]));
        assert_eq!(
            vol_directional_derivs(Vec3::ZERO, 0, &[]),
            Err(invalid(volume_op, "derivative array too short"))
        );
        for order in 1..=4 {
            assert_eq!(
                directional_derivs(Vec2::ZERO, order, order, &[p]),
                Err(invalid(surface_op, "derivative array too short"))
            );
            assert_eq!(
                vol_directional_derivs(Vec3::ZERO, order, &[p]),
                Err(invalid(volume_op, "derivative array too short"))
            );
        }
        let mut surface = [v(2.0, 3.0, 4.0); 16];
        surface[0] = p;
        assert_eq!(
            directional_derivs(Vec2::ZERO, 3, 3, &surface),
            Ok(vec![p, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO])
        );
        let volume = [v(2.0, 3.0, 4.0); 64];
        assert_eq!(
            vol_directional_derivs(Vec3::ZERO, 3, &volume),
            vol_directional_derivs(Vec3::Z, 3, &volume)
        );
        assert_eq!(
            vol_directional_derivs(Vec3::ZERO, 3, &volume).unwrap(),
            vec![volume[0]; 4]
        );
    }

    fn mixed_deriv(u: usize, v: usize, w: usize) -> Vec3 {
        let (u, v, w) = (u as f64, v as f64, w as f64);
        Vec3::new(
            1.0 + u + 4.0 * v + 16.0 * w,
            2.0 + u * v + w,
            3.0 + u - v * w,
        )
    }

    #[test]
    fn derivs_surface_mixed_orders_and_strides() {
        let dir = Vec2::new(3.0, -4.0);
        let unit = dir.unitize().unwrap();
        let (x, y) = (unit.x, unit.y);
        let d = |u, v| mixed_deriv(u, v, 0);
        let expected = [
            d(0, 0),
            x * d(1, 0) + y * d(0, 1),
            x * x * d(2, 0) + 2.0 * x * y * d(1, 1) + y * y * d(0, 2),
            x * x * x * d(3, 0)
                + 3.0 * x * x * y * d(2, 1)
                + 3.0 * x * y * y * d(1, 2)
                + y * y * y * d(0, 3),
        ];
        // Order four is a Rust layout control, not an upstream order-four guarantee.
        for (hu, hv) in [(1, 1), (2, 2), (3, 3), (4, 4), (1, 4), (4, 2)] {
            let mut buffer = [Vec3::ZERO; 25];
            for u in 0..=hu {
                for v in 0..=hv {
                    buffer[u * (hv + 1) + v] = d(u, v);
                }
            }
            let order = hu.min(hv).min(3);
            // Only the consumed prefix is needed, not the full declared rectangle.
            let end = order * (hv + 1) + 1;
            assert_eq!(
                directional_derivs(dir, hu, hv, &buffer[..end]).unwrap(),
                expected[..=order]
            );
        }
    }

    #[test]
    fn derivs_volume_mixed_orders_and_strides() {
        let dir = v(2.0, -3.0, 6.0);
        let unit = dir.unitize_or_z();
        let (x, y, z) = (unit.x, unit.y, unit.z);
        let d = mixed_deriv;
        let expected = [
            d(0, 0, 0),
            x * d(1, 0, 0) + y * d(0, 1, 0) + z * d(0, 0, 1),
            x * x * d(2, 0, 0)
                + 2.0 * x * y * d(1, 1, 0)
                + 2.0 * x * z * d(1, 0, 1)
                + y * y * d(0, 2, 0)
                + 2.0 * y * z * d(0, 1, 1)
                + z * z * d(0, 0, 2),
            x * x * x * d(3, 0, 0)
                + 3.0 * x * x * y * d(2, 1, 0)
                + 3.0 * x * x * z * d(2, 0, 1)
                + 3.0 * x * y * y * d(1, 2, 0)
                + 6.0 * x * y * z * d(1, 1, 1)
                + 3.0 * x * z * z * d(1, 0, 2)
                + y * y * y * d(0, 3, 0)
                + 3.0 * y * y * z * d(0, 2, 1)
                + 3.0 * y * z * z * d(0, 1, 2)
                + z * z * z * d(0, 0, 3),
        ];
        // Order four preserves the existing Rust stride, not an upstream guarantee.
        for order in 1usize..=4 {
            let n1 = order + 1;
            let mut buffer = [Vec3::ZERO; 125];
            for u in 0..=order {
                for v in 0..=order {
                    for w in 0..=order {
                        buffer[(u * n1 + v) * n1 + w] = d(u, v, w);
                    }
                }
            }
            let consumed_order = order.min(3);
            let end = consumed_order * n1 * n1 + 1;
            assert_eq!(
                vol_directional_derivs(dir, order, &buffer[..end]).unwrap(),
                expected[..=consumed_order]
            );
        }
    }

    #[test]
    fn misc() {
        assert!(cones_are_disjoint_perpen(Vec3::X, 0.1, Vec3::X, 0.1));
        assert!(!cones_are_disjoint_perpen(Vec3::X, 0.1, Vec3::Y, 0.1));
        let mut s = vec![1.0, 3.0];
        let mut ix = vec![0, 1];
        add_sorted(2.0, 7, true, &mut s, Some(&mut ix));
        assert_eq!(s, vec![1.0, 2.0, 3.0]);
        assert_eq!(ix, vec![0, 7, 1]);
        let mut a = [5.0, -1.0, 3.0, 3.0, 0.0, 9.0, -7.0];
        shell_sort_double_array(&mut a);
        assert_eq!(a, [-7.0, -1.0, 0.0, 3.0, 3.0, 5.0, 9.0]);
        assert!(double_less_than(1.0, 2.0));
        assert!(is_dog_leg(
            Vec2::new(0., 0.),
            Vec2::new(0., 1.),
            Vec2::new(3., 1.),
            Vec2::new(3., 2.),
            35.0,
            45.0
        ));
        let mut pts = Vec::new();
        add_point(&mut pts, 1e-6, Vec3::ZERO).unwrap();
        add_point(&mut pts, 1e-6, Vec3::ZERO).unwrap();
        add_point(&mut pts, 1e-6, Vec3::X).unwrap();
        assert!(add_point(&mut pts, 1e-6, Vec3::Y).is_err());
        assert_eq!(pts.len(), 2);
        let mut cp = ClosestPoints::default();
        cp.add(2.0, 0.1, 0.2, 1e-3);
        cp.add(1.0, 0.3, 0.4, 1e-3);
        assert_eq!(cp.count, 1);
        assert_eq!(cp.distances[0], 1.0);
        cp.add(1.0005, 0.5, 0.6, 1e-3);
        assert_eq!(cp.count, 2);
        let (p1, p2) = circle_find_silhouette_points(Vec3::ZERO, Vec3::Z, 1.0, false, Vec3::X)
            .unwrap()
            .unwrap();
        assert_eq!((p1, p2), (Vec3::Y, -Vec3::Y));
        assert!(
            circle_find_silhouette_points(Vec3::ZERO, Vec3::Z, 1.0, true, v(0.5, 0., 1.))
                .unwrap()
                .is_none()
        );
    }
}
