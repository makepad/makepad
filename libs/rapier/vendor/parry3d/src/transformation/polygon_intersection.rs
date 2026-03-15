use crate::math::Vector2;
use alloc::{vec, vec::Vec};
use alloc::format;

use crate::math::Real;
use crate::shape::{SegmentPointLocation, Triangle, TriangleOrientation};
use crate::utils::hashmap::HashMap;
use crate::utils::{self, SegmentsIntersection};
use core::fmt;

#[derive(Copy, Clone, Debug)]
struct TotalOrdReal(Real);

impl PartialEq for TotalOrdReal {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for TotalOrdReal {}

impl PartialOrd for TotalOrdReal {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TotalOrdReal {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// Tolerances for polygon intersection algorithms.
///
/// These tolerances control how the intersection algorithm handles numerical precision
/// issues when determining geometric relationships between points, lines, and polygons.
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::transformation::PolygonIntersectionTolerances;
/// // Use default tolerances (recommended for most cases)
/// let default_tol = PolygonIntersectionTolerances::default();
///
/// // Or create custom tolerances for special cases
/// let custom_tol = PolygonIntersectionTolerances {
///     collinearity_epsilon: 1.0e-5,
/// };
/// # }
/// ```
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct PolygonIntersectionTolerances {
    /// The epsilon given to [`Triangle::orientation2d`] for detecting if three points are collinear.
    ///
    /// Three points forming a triangle with an area smaller than this value are considered collinear.
    pub collinearity_epsilon: Real,
}

impl Default for PolygonIntersectionTolerances {
    fn default() -> Self {
        Self {
            collinearity_epsilon: Real::EPSILON * 100.0,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum InFlag {
    // The current neighborhood of the traversed point on poly1 is inside poly2.
    Poly1IsInside,
    // The current neighborhood of the traversed point on poly2 is inside poly1.
    Poly2IsInside,
    Unknown,
}

/// Location of a point on a polyline.
///
/// This enum represents where a point lies on a polygon's boundary. It's used by
/// the intersection algorithms to precisely describe intersection points.
///
/// # Variants
///
/// * `OnVertex(i)` - The point is exactly on vertex `i` of the polygon
/// * `OnEdge(i, j, bcoords)` - The point lies on the edge between vertices `i` and `j`,
///   with barycentric coordinates `bcoords` where `bcoords[0] + bcoords[1] = 1.0`
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::transformation::PolylinePointLocation;
/// # use parry2d::math::Vector;
/// let polygon = vec![
///     Vector::ZERO,
///     Vector::new(2.0, 0.0),
///     Vector::new(2.0, 2.0),
///     Vector::new(0.0, 2.0),
/// ];
///
/// // A point on vertex 0
/// let loc1 = PolylinePointLocation::OnVertex(0);
/// let pt1 = loc1.to_point(&polygon);
/// assert_eq!(pt1, Vector::ZERO);
///
/// // A point halfway along the edge from vertex 0 to vertex 1
/// let loc2 = PolylinePointLocation::OnEdge(0, 1, [0.5, 0.5]);
/// let pt2 = loc2.to_point(&polygon);
/// assert_eq!(pt2, Vector::new(1.0, 0.0));
/// # }
/// ```
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PolylinePointLocation {
    /// Vector on a vertex.
    OnVertex(usize),
    /// Vector on an edge.
    OnEdge(usize, usize, [Real; 2]),
}

impl PolylinePointLocation {
    /// The barycentric coordinates such that the point in the intersected segment `[a, b]` is
    /// equal to `a + (b - a) * centered_bcoords`.
    fn centered_bcoords(&self, edge: [usize; 2]) -> Real {
        match self {
            Self::OnVertex(vid) => {
                if *vid == edge[0] {
                    0.0
                } else {
                    1.0
                }
            }
            Self::OnEdge(ia, ib, bcoords) => {
                assert_eq!([*ia, *ib], edge);
                bcoords[1]
            }
        }
    }

    /// Computes the point corresponding to this location.
    ///
    /// Given a polygon (as a slice of points), this method converts the location
    /// into an actual 2D point coordinate.
    ///
    /// # Arguments
    ///
    /// * `pts` - The vertices of the polygon
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::transformation::PolylinePointLocation;
    /// # use parry2d::math::Vector;
    /// let polygon = vec![
    ///     Vector::ZERO,
    ///     Vector::new(4.0, 0.0),
    ///     Vector::new(4.0, 4.0),
    /// ];
    ///
    /// let loc = PolylinePointLocation::OnEdge(0, 1, [0.75, 0.25]);
    /// let point = loc.to_point(&polygon);
    /// assert_eq!(point, Vector::new(1.0, 0.0)); // 75% of vertex 0 + 25% of vertex 1
    /// # }
    /// ```
    pub fn to_point(self, pts: &[Vector2]) -> Vector2 {
        match self {
            PolylinePointLocation::OnVertex(i) => pts[i],
            PolylinePointLocation::OnEdge(i1, i2, bcoords) => {
                pts[i1] * bcoords[0] + pts[i2] * bcoords[1]
            }
        }
    }

    fn from_segment_point_location(a: usize, b: usize, loc: SegmentPointLocation) -> Self {
        match loc {
            SegmentPointLocation::OnVertex(0) => PolylinePointLocation::OnVertex(a),
            SegmentPointLocation::OnVertex(1) => PolylinePointLocation::OnVertex(b),
            SegmentPointLocation::OnVertex(_) => unreachable!(),
            SegmentPointLocation::OnEdge(bcoords) => PolylinePointLocation::OnEdge(a, b, bcoords),
        }
    }
}

/// Computes the intersection points of two convex polygons.
///
/// This function takes two convex polygons and computes their intersection, returning
/// the vertices of the resulting intersection polygon. The result is added to the `out` vector.
///
/// # Important Notes
///
/// - **Convex polygons only**: Both input polygons must be convex. For non-convex polygons,
///   use [`polygons_intersection_points`] instead.
/// - **Counter-clockwise winding**: Input polygons should be oriented counter-clockwise.
/// - **Default tolerances**: Uses default numerical tolerances. For custom tolerances,
///   use [`convex_polygons_intersection_points_with_tolerances`].
///
/// # Arguments
///
/// * `poly1` - First convex polygon as a slice of vertices
/// * `poly2` - Second convex polygon as a slice of vertices
/// * `out` - Output vector where intersection vertices will be appended
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::transformation::convex_polygons_intersection_points;
/// # use parry2d::math::Vector;
/// // Define two overlapping squares
/// let square1 = vec![
///     Vector::ZERO,
///     Vector::new(2.0, 0.0),
///     Vector::new(2.0, 2.0),
///     Vector::new(0.0, 2.0),
/// ];
///
/// let square2 = vec![
///     Vector::new(1.0, 1.0),
///     Vector::new(3.0, 1.0),
///     Vector::new(3.0, 3.0),
///     Vector::new(1.0, 3.0),
/// ];
///
/// let mut intersection = Vec::new();
/// convex_polygons_intersection_points(&square1, &square2, &mut intersection);
///
/// // The intersection should be a square from (1,1) to (2,2)
/// assert_eq!(intersection.len(), 4);
/// # }
/// ```
///
/// # See Also
///
/// * [`convex_polygons_intersection`] - For closure-based output
/// * [`polygons_intersection_points`] - For non-convex polygons
pub fn convex_polygons_intersection_points(
    poly1: &[Vector2],
    poly2: &[Vector2],
    out: &mut Vec<Vector2>,
) {
    convex_polygons_intersection_points_with_tolerances(poly1, poly2, Default::default(), out);
}

/// Computes the intersection points of two convex polygons with custom tolerances.
///
/// This is the same as [`convex_polygons_intersection_points`] but allows you to specify
/// custom numerical tolerances for the intersection computation.
///
/// # Arguments
///
/// * `poly1` - First convex polygon as a slice of vertices
/// * `poly2` - Second convex polygon as a slice of vertices
/// * `tolerances` - Custom tolerances for numerical precision
/// * `out` - Output vector where intersection vertices will be appended
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::transformation::{convex_polygons_intersection_points_with_tolerances, PolygonIntersectionTolerances};
/// # use parry2d::math::Vector;
/// let triangle1 = vec![
///     Vector::ZERO,
///     Vector::new(4.0, 0.0),
///     Vector::new(2.0, 3.0),
/// ];
///
/// let triangle2 = vec![
///     Vector::new(1.0, 0.5),
///     Vector::new(3.0, 0.5),
///     Vector::new(2.0, 2.5),
/// ];
///
/// let mut intersection = Vec::new();
/// let tolerances = PolygonIntersectionTolerances {
///     collinearity_epsilon: 1.0e-6,
/// };
///
/// convex_polygons_intersection_points_with_tolerances(
///     &triangle1,
///     &triangle2,
///     tolerances,
///     &mut intersection
/// );
///
/// // The triangles overlap, so we should get intersection points
/// assert!(intersection.len() >= 3);
/// # }
/// ```
pub fn convex_polygons_intersection_points_with_tolerances(
    poly1: &[Vector2],
    poly2: &[Vector2],
    tolerances: PolygonIntersectionTolerances,
    out: &mut Vec<Vector2>,
) {
    convex_polygons_intersection_with_tolerances(poly1, poly2, tolerances, |loc1, loc2| {
        if let Some(loc1) = loc1 {
            out.push(loc1.to_point(poly1))
        } else if let Some(loc2) = loc2 {
            out.push(loc2.to_point(poly2))
        }
    })
}

/// Computes the intersection of two convex polygons with closure-based output.
///
/// This function is similar to [`convex_polygons_intersection_points`] but provides more
/// flexibility by calling a closure for each intersection vertex. The closure receives
/// the location of the vertex on each polygon (if applicable).
///
/// This is useful when you need to track which polygon each intersection vertex comes from,
/// or when you want to process vertices as they're computed rather than collecting them all.
///
/// # Arguments
///
/// * `poly1` - First convex polygon as a slice of vertices
/// * `poly2` - Second convex polygon as a slice of vertices
/// * `out` - Closure called for each intersection vertex with its location on both polygons
///
/// # Closure Arguments
///
/// The closure receives `(Option<PolylinePointLocation>, Option<PolylinePointLocation>)`:
/// - If the point comes from `poly1`, the first option contains its location on `poly1`
/// - If the point comes from `poly2`, the second option contains its location on `poly2`
/// - At least one of the options will always be `Some`
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::transformation::convex_polygons_intersection;
/// # use parry2d::math::Vector;
/// let square = vec![
///     Vector::ZERO,
///     Vector::new(2.0, 0.0),
///     Vector::new(2.0, 2.0),
///     Vector::new(0.0, 2.0),
/// ];
///
/// let diamond = vec![
///     Vector::new(1.0, -0.5),
///     Vector::new(2.5, 1.0),
///     Vector::new(1.0, 2.5),
///     Vector::new(-0.5, 1.0),
/// ];
///
/// let mut intersection_points = Vec::new();
/// convex_polygons_intersection(&square, &diamond, |loc1, loc2| {
///     if let Some(loc) = loc1 {
///         intersection_points.push(loc.to_point(&square));
///     } else if let Some(loc) = loc2 {
///         intersection_points.push(loc.to_point(&diamond));
///     }
/// });
///
/// // The intersection should have multiple vertices
/// assert!(intersection_points.len() >= 3);
/// # }
/// ```
///
/// # See Also
///
/// * [`convex_polygons_intersection_points`] - Simpler vector-based output
/// * [`convex_polygons_intersection_with_tolerances`] - With custom tolerances
pub fn convex_polygons_intersection(
    poly1: &[Vector2],
    poly2: &[Vector2],
    out: impl FnMut(Option<PolylinePointLocation>, Option<PolylinePointLocation>),
) {
    convex_polygons_intersection_with_tolerances(poly1, poly2, Default::default(), out)
}

/// Computes the intersection of two convex polygons with custom tolerances and closure-based output.
///
/// This is the most flexible version of the convex polygon intersection function, combining
/// custom tolerances with closure-based output for maximum control.
///
/// # Arguments
///
/// * `poly1` - First convex polygon as a slice of vertices
/// * `poly2` - Second convex polygon as a slice of vertices
/// * `tolerances` - Custom tolerances for numerical precision
/// * `out` - Closure called for each intersection vertex
///
/// # Algorithm
///
/// This function implements the Sutherland-Hodgman-like algorithm for convex polygon
/// intersection. It works by:
/// 1. Traversing the edges of both polygons simultaneously
/// 2. Detecting edge-edge intersections
/// 3. Determining which vertices are inside the other polygon
/// 4. Outputting the vertices of the intersection polygon in order
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::transformation::{convex_polygons_intersection_with_tolerances, PolygonIntersectionTolerances};
/// # use parry2d::math::Vector;
/// let hexagon = vec![
///     Vector::new(2.0, 0.0),
///     Vector::new(1.0, 1.732),
///     Vector::new(-1.0, 1.732),
///     Vector::new(-2.0, 0.0),
///     Vector::new(-1.0, -1.732),
///     Vector::new(1.0, -1.732),
/// ];
///
/// let square = vec![
///     Vector::new(-1.0, -1.0),
///     Vector::new(1.0, -1.0),
///     Vector::new(1.0, 1.0),
///     Vector::new(-1.0, 1.0),
/// ];
///
/// let tolerances = PolygonIntersectionTolerances::default();
/// let mut intersection_points = Vec::new();
///
/// convex_polygons_intersection_with_tolerances(
///     &hexagon,
///     &square,
///     tolerances,
///     |loc1, loc2| {
///         if let Some(loc) = loc1 {
///             intersection_points.push(loc.to_point(&hexagon));
///         } else if let Some(loc) = loc2 {
///             intersection_points.push(loc.to_point(&square));
///         }
///     }
/// );
///
/// // The intersection should form a polygon
/// assert!(intersection_points.len() >= 3);
/// # }
/// ```
pub fn convex_polygons_intersection_with_tolerances(
    poly1: &[Vector2],
    poly2: &[Vector2],
    tolerances: PolygonIntersectionTolerances,
    mut out: impl FnMut(Option<PolylinePointLocation>, Option<PolylinePointLocation>),
) {
    // TODO: this does not handle correctly the case where the
    // first triangle of the polygon is degenerate.
    let rev1 = poly1.len() > 2
        && Triangle::orientation2d(
            poly1[0],
            poly1[1],
            poly1[2],
            tolerances.collinearity_epsilon,
        ) == TriangleOrientation::Clockwise;
    let rev2 = poly2.len() > 2
        && Triangle::orientation2d(
            poly2[0],
            poly2[1],
            poly2[2],
            tolerances.collinearity_epsilon,
        ) == TriangleOrientation::Clockwise;

    let len1 = poly1.len();
    let len2 = poly2.len();

    let mut i1 = 0; // Current index on the first polyline.
    let mut i2 = 0; // Current index on the second polyline.
    let mut nsteps1 = 0; // Number of times we advanced on the first polyline.
    let mut nsteps2 = 0; // Number of times we advanced on the second polyline.
    let mut inflag = InFlag::Unknown;
    let mut first_point_found = false;

    // Quit when both adv. indices have cycled, or one has cycled twice.
    while (nsteps1 < len1 || nsteps2 < len2) && nsteps1 < 2 * len1 && nsteps2 < 2 * len2 {
        let (a1, b1) = if rev1 {
            ((len1 - i1) % len1, len1 - i1 - 1)
        } else {
            // Vector before `i1`, and point at `i1`.
            ((i1 + len1 - 1) % len1, i1)
        };

        let (a2, b2) = if rev2 {
            ((len2 - i2) % len2, len2 - i2 - 1)
        } else {
            // Vector before `i2`, and point at `i2`.
            ((i2 + len2 - 1) % len2, i2)
        };

        let dir_edge1 = poly1[b1] - poly1[a1];
        let dir_edge2 = poly2[b2] - poly2[a2];

        // If there is an intersection, this will determine if the edge from poly2 is transitioning
        // Left -> Right (CounterClockwise) or Right -> Left (Clockwise) relative to the edge from
        // poly1.
        let cross = Triangle::orientation2d(
            Vector2::ZERO,
            Vector2::from(dir_edge1),
            Vector2::from(dir_edge2),
            tolerances.collinearity_epsilon,
        );
        // Determines if b1 is left (CounterClockwise) or right (Clockwise) of [a2, b2].
        let a2_b2_b1 = Triangle::orientation2d(
            poly2[a2],
            poly2[b2],
            poly1[b1],
            tolerances.collinearity_epsilon,
        );
        // Determines if b2 is left (CounterClockwise) or right (Clockwise) of [a1, b1].
        let a1_b1_b2 = Triangle::orientation2d(
            poly1[a1],
            poly1[b1],
            poly2[b2],
            tolerances.collinearity_epsilon,
        );

        // If edge1 & edge2 intersect, update inflag.
        if let Some(inter) = utils::segments_intersection2d(
            poly1[a1],
            poly1[b1],
            poly2[a2],
            poly2[b2],
            tolerances.collinearity_epsilon,
        ) {
            match inter {
                SegmentsIntersection::Point { loc1, loc2 } => {
                    if a2_b2_b1 != TriangleOrientation::Degenerate
                        && a1_b1_b2 != TriangleOrientation::Degenerate
                    {
                        let loc1 = PolylinePointLocation::from_segment_point_location(a1, b1, loc1);
                        let loc2 = PolylinePointLocation::from_segment_point_location(a2, b2, loc2);
                        out(Some(loc1), Some(loc2));

                        if inflag == InFlag::Unknown && !first_point_found {
                            // This is the first point, reset the number of steps since we are
                            // effectively starting the actual traversal now.
                            nsteps1 = 0;
                            nsteps2 = 0;
                            first_point_found = true;
                        }

                        if a2_b2_b1 == TriangleOrientation::CounterClockwise {
                            // The point b1 is left of [a2, b2] so it is inside poly2 ???
                            inflag = InFlag::Poly1IsInside;
                        } else if a1_b1_b2 == TriangleOrientation::CounterClockwise {
                            // The point b2 is left of [a1, b1] so it is inside poly1 ???
                            inflag = InFlag::Poly2IsInside;
                        }
                    }
                }
                SegmentsIntersection::Segment {
                    first_loc1,
                    first_loc2,
                    second_loc1,
                    second_loc2,
                } => {
                    if dir_edge1.dot(dir_edge2) < 0.0 {
                        // Special case: edge1 & edge2 overlap and oppositely oriented. The
                        //               intersection is degenerate (equals to a segment). Output
                        //               the segment and exit.
                        let loc1 =
                            PolylinePointLocation::from_segment_point_location(a1, b1, first_loc1);
                        let loc2 =
                            PolylinePointLocation::from_segment_point_location(a2, b2, first_loc2);
                        out(Some(loc1), Some(loc2));

                        let loc1 =
                            PolylinePointLocation::from_segment_point_location(a1, b1, second_loc1);
                        let loc2 =
                            PolylinePointLocation::from_segment_point_location(a2, b2, second_loc2);
                        out(Some(loc1), Some(loc2));
                        return;
                    }
                }
            }
        }

        // Special case: edge1 & edge2 parallel and separated.
        if cross == TriangleOrientation::Degenerate
            && a2_b2_b1 == TriangleOrientation::Clockwise
            && a1_b1_b2 == TriangleOrientation::Clockwise
        // TODO: should this also include any case where a2_b2_b1 and a1_b1_b2 are both different from Degenerate?
        {
            return;
        }
        // Special case: edge1 & edge2 collinear.
        else if cross == TriangleOrientation::Degenerate
            && a2_b2_b1 == TriangleOrientation::Degenerate
            && a1_b1_b2 == TriangleOrientation::Degenerate
        {
            // Advance but do not output point.
            if inflag == InFlag::Poly1IsInside {
                i2 = advance(i2, &mut nsteps2, len2);
            } else {
                i1 = advance(i1, &mut nsteps1, len1);
            }
        }
        // Generic cases.
        else if cross == TriangleOrientation::CounterClockwise {
            if a1_b1_b2 == TriangleOrientation::CounterClockwise {
                if inflag == InFlag::Poly1IsInside {
                    out(Some(PolylinePointLocation::OnVertex(b1)), None)
                }
                i1 = advance(i1, &mut nsteps1, len1);
            } else {
                if inflag == InFlag::Poly2IsInside {
                    out(None, Some(PolylinePointLocation::OnVertex(b2)))
                }
                i2 = advance(i2, &mut nsteps2, len2);
            }
        } else {
            // We have cross == TriangleOrientation::Clockwise.
            if a2_b2_b1 == TriangleOrientation::CounterClockwise {
                if inflag == InFlag::Poly2IsInside {
                    out(None, Some(PolylinePointLocation::OnVertex(b2)))
                }
                i2 = advance(i2, &mut nsteps2, len2);
            } else {
                if inflag == InFlag::Poly1IsInside {
                    out(Some(PolylinePointLocation::OnVertex(b1)), None)
                }
                i1 = advance(i1, &mut nsteps1, len1);
            }
        }
    }

    if !first_point_found {
        // No intersection: test if one polygon completely encloses the other.
        let mut orient = TriangleOrientation::Degenerate;
        let mut ok = true;

        // TODO: avoid the O(n²) complexity.
        for a in 0..len1 {
            for p2 in poly2 {
                let a_minus_1 = (a + len1 - 1) % len1; // a - 1
                let new_orient = Triangle::orientation2d(
                    poly1[a_minus_1],
                    poly1[a],
                    *p2,
                    tolerances.collinearity_epsilon,
                );

                if orient == TriangleOrientation::Degenerate {
                    orient = new_orient
                } else if new_orient != orient && new_orient != TriangleOrientation::Degenerate {
                    ok = false;
                    break;
                }
            }
        }

        if ok {
            for mut b in 0..len2 {
                if rev2 {
                    b = len2 - b - 1;
                }
                out(None, Some(PolylinePointLocation::OnVertex(b)))
            }
        }

        let mut orient = TriangleOrientation::Degenerate;
        let mut ok = true;

        // TODO: avoid the O(n²) complexity.
        for b in 0..len2 {
            for p1 in poly1 {
                let b_minus_1 = (b + len2 - 1) % len2; // = b - 1
                let new_orient = Triangle::orientation2d(
                    poly2[b_minus_1],
                    poly2[b],
                    *p1,
                    tolerances.collinearity_epsilon,
                );

                if orient == TriangleOrientation::Degenerate {
                    orient = new_orient
                } else if new_orient != orient && new_orient != TriangleOrientation::Degenerate {
                    ok = false;
                    break;
                }
            }
        }

        if ok {
            for mut a in 0..len1 {
                if rev1 {
                    a = len1 - a - 1;
                }
                out(Some(PolylinePointLocation::OnVertex(a)), None)
            }
        }
    }
}

#[inline]
fn advance(a: usize, aa: &mut usize, n: usize) -> usize {
    *aa += 1;
    (a + 1) % n
}

/// Error type for polygon intersection operations.
///
/// This error can occur when computing intersections of non-convex polygons.
#[derive(Debug)]
pub enum PolygonsIntersectionError {
    /// An infinite loop was detected during intersection computation.
    ///
    /// This typically indicates that the input polygons are ill-formed, such as:
    /// - Self-intersecting polygons
    /// - Polygons with duplicate or degenerate edges
    /// - Polygons with inconsistent winding order
    InfiniteLoop,
}

impl fmt::Display for PolygonsIntersectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InfiniteLoop => {
                f.write_str("Infinite loop detected; input polygons are ill-formed.")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for PolygonsIntersectionError {}

/// Computes the intersection points of two possibly non-convex polygons.
///
/// This function handles both convex and **non-convex (concave)** polygons, making it more
/// general than [`convex_polygons_intersection_points`]. However, it requires that:
/// - Neither polygon self-intersects
/// - Both polygons are oriented counter-clockwise
///
/// The result is a vector of polygons, where each polygon represents one connected component
/// of the intersection. In most cases there will be only one component, but complex intersections
/// can produce multiple separate regions.
///
/// # Important Notes
///
/// - **Non-convex support**: This function works with concave polygons
/// - **Multiple components**: Returns a `Vec<Vec<Vector2>>` because the intersection
///   of two concave polygons can produce multiple separate regions
/// - **No self-intersection**: Input polygons must not self-intersect
/// - **Counter-clockwise winding**: Both polygons must be oriented counter-clockwise
/// - **Performance**: Slower than [`convex_polygons_intersection_points`]. If both polygons
///   are convex, use that function instead.
///
/// # Arguments
///
/// * `poly1` - First polygon as a slice of vertices
/// * `poly2` - Second polygon as a slice of vertices
///
/// # Returns
///
/// * `Ok(Vec<Vec<Vector2>>)` - A vector of intersection polygons (usually just one)
/// * `Err(PolygonsIntersectionError::InfiniteLoop)` - If the polygons are ill-formed
///
/// # Examples
///
/// ## Example 1: Two non-convex polygons
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::transformation::polygons_intersection_points;
/// # use parry2d::math::Vector;
/// // L-shaped polygon
/// let l_shape = vec![
///     Vector::ZERO,
///     Vector::new(3.0, 0.0),
///     Vector::new(3.0, 1.0),
///     Vector::new(1.0, 1.0),
///     Vector::new(1.0, 3.0),
///     Vector::new(0.0, 3.0),
/// ];
///
/// // Square overlapping the L-shape
/// let square = vec![
///     Vector::new(0.5, 0.5),
///     Vector::new(2.5, 0.5),
///     Vector::new(2.5, 2.5),
///     Vector::new(0.5, 2.5),
/// ];
///
/// let result = polygons_intersection_points(&l_shape, &square).unwrap();
///
/// // Should have intersection regions
/// assert!(!result.is_empty());
/// # }
/// ```
///
/// ## Example 2: Convex polygons (also works)
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::transformation::polygons_intersection_points;
/// # use parry2d::math::Vector;
/// let triangle = vec![
///     Vector::ZERO,
///     Vector::new(4.0, 0.0),
///     Vector::new(2.0, 3.0),
/// ];
///
/// let square = vec![
///     Vector::new(1.0, 0.5),
///     Vector::new(3.0, 0.5),
///     Vector::new(3.0, 2.0),
///     Vector::new(1.0, 2.0),
/// ];
///
/// let result = polygons_intersection_points(&triangle, &square).unwrap();
/// assert_eq!(result.len(), 1); // One intersection region
/// # }
/// ```
///
/// # Errors
///
/// Returns `PolygonsIntersectionError::InfiniteLoop` if:
/// - Either polygon self-intersects
/// - The polygons have degenerate or duplicate edges
/// - The winding order is inconsistent
///
/// # See Also
///
/// * [`convex_polygons_intersection_points`] - Faster version for convex polygons only
/// * [`polygons_intersection`] - Closure-based version with more control
pub fn polygons_intersection_points(
    poly1: &[Vector2],
    poly2: &[Vector2],
) -> Result<Vec<Vec<Vector2>>, PolygonsIntersectionError> {
    let mut result = vec![];
    let mut curr_poly = vec![];
    polygons_intersection(poly1, poly2, |loc1, loc2| {
        if let Some(loc1) = loc1 {
            curr_poly.push(loc1.to_point(poly1))
        } else if let Some(loc2) = loc2 {
            curr_poly.push(loc2.to_point(poly2))
        } else if !curr_poly.is_empty() {
            result.push(core::mem::take(&mut curr_poly));
        }
    })?;

    Ok(result)
}

/// Computes the intersection of two possibly non-convex polygons with closure-based output.
///
/// This is the closure-based version of [`polygons_intersection_points`], providing more
/// control over how intersection vertices are processed. It handles non-convex (concave)
/// polygons but requires they do not self-intersect.
///
/// # Arguments
///
/// * `poly1` - First polygon as a slice of vertices
/// * `poly2` - Second polygon as a slice of vertices
/// * `out` - Closure called for each intersection vertex or component separator
///
/// # Closure Arguments
///
/// The closure receives `(Option<PolylinePointLocation>, Option<PolylinePointLocation>)`:
/// - When **both are `Some`**: An edge-edge intersection point
/// - When **one is `Some`**: A vertex from one polygon inside the other
/// - When **both are `None`**: Marks the end of a connected component
///
/// # Algorithm
///
/// This function uses a graph-based traversal algorithm:
/// 1. Finds all edge-edge intersection points between the two polygons
/// 2. Builds a graph where vertices are intersection points
/// 3. Traverses the graph, alternating between polygons at intersection points
/// 4. Outputs vertices that form the intersection boundary
///
/// # Examples
///
/// ## Example 1: Collecting intersection points
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::transformation::polygons_intersection;
/// # use parry2d::math::Vector;
/// let pentagon = vec![
///     Vector::new(1.0, 0.0),
///     Vector::new(0.309, 0.951),
///     Vector::new(-0.809, 0.588),
///     Vector::new(-0.809, -0.588),
///     Vector::new(0.309, -0.951),
/// ];
///
/// let square = vec![
///     Vector::new(-0.5, -0.5),
///     Vector::new(0.5, -0.5),
///     Vector::new(0.5, 0.5),
///     Vector::new(-0.5, 0.5),
/// ];
///
/// let mut components = Vec::new();
/// let mut current_component = Vec::new();
///
/// polygons_intersection(&pentagon, &square, |loc1, loc2| {
///     if loc1.is_none() && loc2.is_none() {
///         // End of a component
///         if !current_component.is_empty() {
///             components.push(std::mem::take(&mut current_component));
///         }
///     } else if let Some(loc) = loc1 {
///         current_component.push(loc.to_point(&pentagon));
///     } else if let Some(loc) = loc2 {
///         current_component.push(loc.to_point(&square));
///     }
/// }).unwrap();
///
/// // Should have at least one intersection component
/// assert!(!components.is_empty());
/// # }
/// ```
///
/// ## Example 2: Counting intersection vertices
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # use parry2d::transformation::polygons_intersection;
/// # use parry2d::math::Vector;
/// let hexagon = vec![
///     Vector::new(2.0, 0.0),
///     Vector::new(1.0, 1.732),
///     Vector::new(-1.0, 1.732),
///     Vector::new(-2.0, 0.0),
///     Vector::new(-1.0, -1.732),
///     Vector::new(1.0, -1.732),
/// ];
///
/// let circle_approx = vec![
///     Vector::new(1.0, 0.0),
///     Vector::new(0.707, 0.707),
///     Vector::new(0.0, 1.0),
///     Vector::new(-0.707, 0.707),
///     Vector::new(-1.0, 0.0),
///     Vector::new(-0.707, -0.707),
///     Vector::new(0.0, -1.0),
///     Vector::new(0.707, -0.707),
/// ];
///
/// let mut vertex_count = 0;
/// polygons_intersection(&hexagon, &circle_approx, |loc1, loc2| {
///     if loc1.is_some() || loc2.is_some() {
///         vertex_count += 1;
///     }
/// }).unwrap();
///
/// assert!(vertex_count > 0);
/// # }
/// ```
///
/// # Errors
///
/// Returns `PolygonsIntersectionError::InfiniteLoop` if the polygons are ill-formed.
///
/// # See Also
///
/// * [`polygons_intersection_points`] - Simpler vector-based output
/// * [`convex_polygons_intersection`] - Faster version for convex polygons
pub fn polygons_intersection(
    poly1: &[Vector2],
    poly2: &[Vector2],
    mut out: impl FnMut(Option<PolylinePointLocation>, Option<PolylinePointLocation>),
) -> Result<(), PolygonsIntersectionError> {
    let tolerances = PolygonIntersectionTolerances::default();

    #[derive(Debug)]
    struct ToTraverse {
        poly: usize,
        edge: EdgeId,
    }

    let (intersections, num_intersections) =
        compute_sorted_edge_intersections(poly1, poly2, tolerances.collinearity_epsilon);
    let mut visited = vec![false; num_intersections];
    let segment = |eid: EdgeId, poly: &[Vector2]| [poly[eid], poly[(eid + 1) % poly.len()]];

    // Traverse all the intersections.
    for inters in intersections[0].values() {
        for inter in inters {
            if visited[inter.id] {
                continue;
            }

            // We found an intersection we haven’t visited yet, traverse the loop, alternating
            // between poly1 and poly2 when reaching an intersection.
            let [a1, b1] = segment(inter.edges[0], poly1);
            let [a2, b2] = segment(inter.edges[1], poly2);
            let poly_to_traverse =
                match Triangle::orientation2d(a1, b1, a2, tolerances.collinearity_epsilon) {
                    TriangleOrientation::Clockwise => 1,
                    TriangleOrientation::CounterClockwise => 0,
                    TriangleOrientation::Degenerate => {
                        match Triangle::orientation2d(a1, b1, b2, tolerances.collinearity_epsilon) {
                            TriangleOrientation::Clockwise => 0,
                            TriangleOrientation::CounterClockwise => 1,
                            TriangleOrientation::Degenerate => {
                                log::debug!("Unhandled edge-edge overlap case.");
                                0
                            }
                        }
                    }
                };

            #[derive(Debug)]
            enum TraversalStatus {
                OnVertex,
                OnIntersection(usize),
            }

            let polys = [poly1, poly2];
            let mut to_traverse = ToTraverse {
                poly: poly_to_traverse,
                edge: inter.edges[poly_to_traverse],
            };

            let mut status = TraversalStatus::OnIntersection(inter.id);

            for loop_id in 0.. {
                if loop_id > poly1.len() * poly2.len() {
                    return Err(PolygonsIntersectionError::InfiniteLoop);
                }

                let empty = Vec::new();
                let edge_inters = intersections[to_traverse.poly]
                    .get(&to_traverse.edge)
                    .unwrap_or(&empty);

                match status {
                    TraversalStatus::OnIntersection(inter_id) => {
                        let (curr_inter_pos, curr_inter) = edge_inters
                            .iter()
                            .enumerate()
                            .find(|(_, inter)| inter.id == inter_id)
                            .unwrap_or_else(|| unreachable!());

                        if visited[curr_inter.id] {
                            // We already saw this intersection: we looped back to the start of
                            // the intersection polygon.
                            out(None, None);
                            break;
                        }

                        out(Some(curr_inter.locs[0]), Some(curr_inter.locs[1]));
                        visited[curr_inter.id] = true;

                        if curr_inter_pos + 1 < edge_inters.len() {
                            // There are other intersections after this one.
                            // Move forward to the next intersection point and move on to traversing
                            // the other polygon.
                            let next_inter = &edge_inters[curr_inter_pos + 1];
                            to_traverse.poly = (to_traverse.poly + 1) % 2;
                            to_traverse.edge = next_inter.edges[to_traverse.poly];
                            status = TraversalStatus::OnIntersection(next_inter.id);
                        } else {
                            // This was the last intersection, move to the next vertex on the
                            // same polygon.
                            to_traverse.edge =
                                (to_traverse.edge + 1) % polys[to_traverse.poly].len();
                            status = TraversalStatus::OnVertex;
                        }
                    }
                    TraversalStatus::OnVertex => {
                        let location = PolylinePointLocation::OnVertex(to_traverse.edge);

                        if to_traverse.poly == 0 {
                            out(Some(location), None);
                        } else {
                            out(None, Some(location))
                        };

                        if let Some(first_intersection) = edge_inters.first() {
                            // Jump on the first intersection and move on to the other polygon.
                            to_traverse.poly = (to_traverse.poly + 1) % 2;
                            to_traverse.edge = first_intersection.edges[to_traverse.poly];
                            status = TraversalStatus::OnIntersection(first_intersection.id);
                        } else {
                            // Move forward to the next vertex/edge on the same polygon.
                            to_traverse.edge =
                                (to_traverse.edge + 1) % polys[to_traverse.poly].len();
                        }
                    }
                }
            }
        }
    }

    // If there are no intersection, check if one polygon is inside the other.
    if intersections[0].is_empty() {
        if utils::point_in_poly2d(poly1[0], poly2) {
            for pt_id in 0..poly1.len() {
                out(Some(PolylinePointLocation::OnVertex(pt_id)), None)
            }
            out(None, None);
        } else if utils::point_in_poly2d(poly2[0], poly1) {
            for pt_id in 0..poly2.len() {
                out(None, Some(PolylinePointLocation::OnVertex(pt_id)))
            }
            out(None, None);
        }
    }

    Ok(())
}

type EdgeId = usize;
type IntersectionId = usize;

#[derive(Copy, Clone, Debug)]
struct IntersectionVector {
    id: IntersectionId,
    edges: [EdgeId; 2],
    locs: [PolylinePointLocation; 2],
}

fn compute_sorted_edge_intersections(
    poly1: &[Vector2],
    poly2: &[Vector2],
    eps: Real,
) -> ([HashMap<EdgeId, Vec<IntersectionVector>>; 2], usize) {
    let mut inter1: HashMap<EdgeId, Vec<IntersectionVector>> = HashMap::default();
    let mut inter2: HashMap<EdgeId, Vec<IntersectionVector>> = HashMap::default();
    let mut id = 0;

    // Find the intersections.
    // TODO: this is a naive O(n²) check. Could use an acceleration structure for large polygons.
    for i1 in 0..poly1.len() {
        let j1 = (i1 + 1) % poly1.len();

        for i2 in 0..poly2.len() {
            let j2 = (i2 + 1) % poly2.len();

            let Some(inter) =
                utils::segments_intersection2d(poly1[i1], poly1[j1], poly2[i2], poly2[j2], eps)
            else {
                continue;
            };

            match inter {
                SegmentsIntersection::Point { loc1, loc2 } => {
                    let loc1 = PolylinePointLocation::from_segment_point_location(i1, j1, loc1);
                    let loc2 = PolylinePointLocation::from_segment_point_location(i2, j2, loc2);
                    let intersection = IntersectionVector {
                        id,
                        edges: [i1, i2],
                        locs: [loc1, loc2],
                    };
                    inter1.entry(i1).or_default().push(intersection);
                    inter2.entry(i2).or_default().push(intersection);
                    id += 1;
                }
                SegmentsIntersection::Segment { .. } => {
                    // TODO
                    log::debug!(
                        "Collinear segment-segment intersections not properly handled yet."
                    );
                }
            }
        }
    }

    // Sort the intersections.
    for inters in inter1.values_mut() {
        inters.sort_by_key(|a| {
            let edge = [a.edges[0], (a.edges[0] + 1) % poly1.len()];
            TotalOrdReal(a.locs[0].centered_bcoords(edge))
        });
    }

    for inters in inter2.values_mut() {
        inters.sort_by_key(|a| {
            let edge = [a.edges[1], (a.edges[1] + 1) % poly2.len()];
            TotalOrdReal(a.locs[1].centered_bcoords(edge))
        });
    }

    ([inter1, inter2], id)
}

#[cfg(all(test, feature = "dim2"))]
mod test {
    use crate::math::Vector;
    use crate::query::PointQuery;
    use crate::shape::Triangle;
    use crate::transformation::convex_polygons_intersection_points_with_tolerances;
    use crate::transformation::polygon_intersection::PolygonIntersectionTolerances;
    use alloc::vec::Vec;
    use std::println;

    #[test]
    fn intersect_triangle_common_vertex() {
        let tris = [
            (
                Triangle::new(
                    Vector::new(-0.0008759537858568062, -2.0103871966663305),
                    Vector::new(0.3903908709629763, -1.3421764825890266),
                    Vector::new(1.3380817875388151, -2.0098007857739013),
                ),
                Triangle::new(
                    Vector::new(0.0, -0.0),
                    Vector::new(-0.0008759537858568062, -2.0103871966663305),
                    Vector::new(1.9991979155226394, -2.009511242880474),
                ),
            ),
            (
                Triangle::new(
                    Vector::new(0.7319315811016305, -0.00004046981523721891),
                    Vector::new(2.0004914907008944, -0.00011061077714557787),
                    Vector::new(1.1848406021956144, -0.8155712451545468),
                ),
                Triangle::new(
                    Vector::ZERO,
                    Vector::new(0.00011061077714557787, -2.000024893134292),
                    Vector::new(2.0004914907008944, -0.00011061077714557787),
                ),
            ),
            (
                Triangle::new(
                    Vector::new(-0.000049995168258705205, -0.9898801451981707),
                    Vector::new(0.0, -0.0),
                    Vector::new(0.583013294019752, -1.4170136900568633),
                ),
                Triangle::new(
                    Vector::new(0.0, -0.0),
                    Vector::new(-0.00010101395240669591, -2.000027389553894),
                    Vector::new(2.000372544168497, 0.00010101395240669591),
                ),
            ),
            (
                Triangle::new(
                    Vector::new(-0.940565646581769, -0.939804943675256),
                    Vector::new(0.0, -0.0),
                    Vector::new(-0.001533592366792066, -0.9283586484736431),
                ),
                Triangle::new(
                    Vector::new(0.0, -0.0),
                    Vector::new(-2.00752629829582, -2.0059026672784825),
                    Vector::new(-0.0033081650580626698, -2.0025945022204197),
                ),
            ),
        ];

        for (tri1, tri2) in tris {
            let mut inter = Vec::new();
            let tolerances = PolygonIntersectionTolerances {
                collinearity_epsilon: 1.0e-5,
            };
            convex_polygons_intersection_points_with_tolerances(
                tri1.vertices(),
                tri2.vertices(),
                tolerances,
                &mut inter,
            );

            println!("----------");
            println!("inter: {:?}", inter);
            println!(
                "tri1 is in tri2: {}",
                tri1.vertices().iter().all(|pt| tri2
                    .project_local_point(*pt, false)
                    .is_inside_eps(*pt, 1.0e-5))
            );
            println!(
                "tri2 is in tri1: {}",
                tri2.vertices().iter().all(|pt| tri1
                    .project_local_point(*pt, false)
                    .is_inside_eps(*pt, 1.0e-5))
            );
            for pt in &inter {
                let proj1 = tri1.project_local_point(*pt, false);
                let proj2 = tri2.project_local_point(*pt, false);
                assert!(proj1.is_inside_eps(*pt, 1.0e-5));
                assert!(proj2.is_inside_eps(*pt, 1.0e-5));
            }
        }
    }
}
