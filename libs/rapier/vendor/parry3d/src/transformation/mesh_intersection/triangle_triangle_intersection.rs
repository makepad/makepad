use super::EPS;
use crate::math::Vector2;
use crate::math::{Real, Vector};
use crate::query;
use crate::query::PointQuery;
use crate::shape::{FeatureId, Segment, Triangle};
use crate::transformation::polygon_intersection::{
    PolygonIntersectionTolerances, PolylinePointLocation,
};
use crate::utils::WBasis;
use alloc::{vec, vec::Vec};

#[derive(Copy, Clone, Debug, Default)]
pub struct TriangleTriangleIntersectionVector {
    pub p1: Vector,
}

#[derive(Clone, Debug)]
pub enum TriangleTriangleIntersection {
    Segment {
        a: TriangleTriangleIntersectionVector,
        b: TriangleTriangleIntersectionVector,
    },
    Polygon(Vec<TriangleTriangleIntersectionVector>),
}

impl Default for TriangleTriangleIntersection {
    fn default() -> Self {
        Self::Segment {
            a: Default::default(),
            b: Default::default(),
        }
    }
}

pub(crate) fn triangle_triangle_intersection(
    tri1: &Triangle,
    tri2: &Triangle,
    collinearity_epsilon: Real,
) -> Option<TriangleTriangleIntersection> {
    let normal1 = tri1.robust_normal();
    let normal2 = tri2.robust_normal();

    if let Some(intersection_dir) = normal1.cross(normal2).try_normalize() {
        let mut range1 = [
            (Real::MAX, Vector::ZERO, FeatureId::Unknown),
            (-Real::MAX, Vector::ZERO, FeatureId::Unknown),
        ];
        let mut range2 = [
            (Real::MAX, Vector::ZERO, FeatureId::Unknown),
            (-Real::MAX, Vector::ZERO, FeatureId::Unknown),
        ];

        let hits1 = [
            segment_plane_intersection(tri2.a, normal2, &Segment::new(tri1.a, tri1.b), 0, (0, 1))
                .map(|(p, feat)| (intersection_dir.dot(p), p, feat)),
            segment_plane_intersection(tri2.a, normal2, &Segment::new(tri1.b, tri1.c), 1, (1, 2))
                .map(|(p, feat)| (intersection_dir.dot(p), p, feat)),
            segment_plane_intersection(tri2.a, normal2, &Segment::new(tri1.c, tri1.a), 2, (2, 0))
                .map(|(p, feat)| (intersection_dir.dot(p), p, feat)),
        ];

        for hit1 in hits1.into_iter().flatten() {
            if hit1.0 < range1[0].0 {
                range1[0] = hit1;
            }
            if hit1.0 > range1[1].0 {
                range1[1] = hit1;
            }
        }

        if range1[0].0 >= range1[1].0 {
            // The first triangle doesn’t intersect the second plane.
            return None;
        }

        let hits2 = [
            segment_plane_intersection(tri1.a, normal1, &Segment::new(tri2.a, tri2.b), 0, (0, 1))
                .map(|(p, feat)| (intersection_dir.dot(p), p, feat)),
            segment_plane_intersection(tri1.a, normal1, &Segment::new(tri2.b, tri2.c), 1, (1, 2))
                .map(|(p, feat)| (intersection_dir.dot(p), p, feat)),
            segment_plane_intersection(tri1.a, normal1, &Segment::new(tri2.c, tri2.a), 2, (2, 0))
                .map(|(p, feat)| (intersection_dir.dot(p), p, feat)),
        ];

        for hit2 in hits2.into_iter().flatten() {
            if hit2.0 < range2[0].0 {
                range2[0] = hit2;
            }
            if hit2.0 > range2[1].0 {
                range2[1] = hit2;
            }
        }

        if range2[0].0 >= range2[1].0 {
            // The second triangle doesn’t intersect the first plane.
            return None;
        }

        if range1[1].0 <= range2[0].0 + EPS || range2[1].0 <= range1[0].0 + EPS {
            // The two triangles intersect each others’ plane, but these intersections are disjoint.
            return None;
        }

        let a = if range2[0].0 > range1[0].0 + EPS {
            TriangleTriangleIntersectionVector { p1: range2[0].1 }
        } else {
            TriangleTriangleIntersectionVector { p1: range1[0].1 }
        };

        let b = if range2[1].0 < range1[1].0 - EPS {
            TriangleTriangleIntersectionVector { p1: range2[1].1 }
        } else {
            TriangleTriangleIntersectionVector { p1: range1[1].1 }
        };

        Some(TriangleTriangleIntersection::Segment { a, b })
    } else {
        let unit_normal2 = normal2.normalize();
        if (tri1.a - tri2.a).dot(unit_normal2) < EPS {
            let basis = unit_normal2.orthonormal_basis();
            let proj = |vect: Vector| Vector2::new(vect.dot(basis[0]), vect.dot(basis[1]));

            let mut intersections = vec![];

            let pts1 = tri1.vertices();
            let pts2 = tri2.vertices();
            let poly1 = [
                proj(tri1.a - tri2.a),
                proj(tri1.b - tri2.a),
                proj(tri1.c - tri2.a),
            ];
            let poly2 = [
                proj(Vector::ZERO), // = proj(tri2.a - tri2.a)
                proj(tri2.b - tri2.a),
                proj(tri2.c - tri2.a),
            ];

            let convert_loc = |loc, pts: &[Vector; 3]| match loc {
                PolylinePointLocation::OnVertex(vid) => (FeatureId::Vertex(vid as u32), pts[vid]),
                PolylinePointLocation::OnEdge(vid1, vid2, bcoords) => (
                    match (vid1, vid2) {
                        (0, 1) | (1, 0) => FeatureId::Edge(0),
                        (1, 2) | (2, 1) => FeatureId::Edge(1),
                        (2, 0) | (0, 2) => FeatureId::Edge(2),
                        _ => unreachable!(),
                    },
                    pts[vid1] * bcoords[0] + pts[vid2] * bcoords[1],
                ),
            };

            crate::transformation::convex_polygons_intersection_with_tolerances(
                &poly1,
                &poly2,
                PolygonIntersectionTolerances {
                    collinearity_epsilon,
                },
                |pt1, pt2| {
                    let intersection = match (pt1, pt2) {
                        (Some(loc1), Some(loc2)) => {
                            let (_f1, p1) = convert_loc(loc1, pts1);
                            let (_f2, _p2) = convert_loc(loc2, pts2);
                            TriangleTriangleIntersectionVector { p1 }
                        }
                        (Some(loc1), None) => {
                            let (_f1, p1) = convert_loc(loc1, pts1);
                            TriangleTriangleIntersectionVector { p1 }
                        }
                        (None, Some(loc2)) => {
                            let (_f2, p2) = convert_loc(loc2, pts2);
                            TriangleTriangleIntersectionVector { p1: p2 }
                        }
                        (None, None) => unreachable!(),
                    };
                    intersections.push(intersection);
                },
            );

            #[cfg(feature = "std")]
            {
                // NOTE: set this to `true` to automatically check if the computed intersection is
                //       valid, and print debug infos if it is not.
                const DEBUG_INTERSECTIONS: bool = false;
                if DEBUG_INTERSECTIONS {
                    debug_check_intersections(tri1, tri2, &basis, &poly1, &poly2, &intersections);
                }
            }

            Some(TriangleTriangleIntersection::Polygon(intersections))
        } else {
            None
        }
    }
}

fn segment_plane_intersection(
    plane_center: Vector,
    plane_normal: Vector,
    segment: &Segment,
    eid: u32,
    vids: (u32, u32),
) -> Option<(Vector, FeatureId)> {
    let dir = segment.b - segment.a;
    let dir_norm = dir.length();

    let time_of_impact =
        query::details::line_toi_with_halfspace(plane_center, plane_normal, segment.a, dir)?;
    let scaled_toi = time_of_impact * dir_norm;

    if scaled_toi < -EPS || scaled_toi > dir_norm + EPS {
        None
    } else if scaled_toi <= EPS {
        Some((segment.a, FeatureId::Vertex(vids.0)))
    } else if scaled_toi >= dir_norm - EPS {
        Some((segment.b, FeatureId::Vertex(vids.1)))
    } else {
        Some((segment.a + dir * time_of_impact, FeatureId::Edge(eid)))
    }
}

/// Prints debug information if the calculated intersection of two triangles is detected to be
/// invalid.
///
/// If the intersection is valid, this prints nothing. If it isn't valid, this will print a few
/// lines to copy/paste into the Desmos online graphing tool (for visual debugging), as well as
/// some rust code to add to the `tris` array in the `intersect_triangle_common_vertex` test for
/// regression checking.
#[cfg(feature = "std")]
fn debug_check_intersections(
    tri1: &Triangle,
    tri2: &Triangle,
    basis: &[Vector; 2],
    poly1: &[Vector2], // Projection of tri1 on the basis `basis1` with the origin at tri2.a.
    poly2: &[Vector2], // Projection of tri2 on the basis `basis2` with the origin at tri2.a.
    intersections: &[TriangleTriangleIntersectionVector],
) {
    use std::{print, println};

    let proj = |vect: Vector| Vector2::new(vect.dot(basis[0]), vect.dot(basis[1]));
    let mut incorrect = false;
    for pt in intersections {
        if !tri1
            .project_local_point(pt.p1, false)
            .is_inside_eps(pt.p1, 1.0e-5)
        {
            incorrect = true;
            break;
        }

        if !tri2
            .project_local_point(pt.p1, false)
            .is_inside_eps(pt.p1, 1.0e-5)
        {
            incorrect = true;
            break;
        }
    }

    if incorrect {
        let proj_inter: Vec<_> = intersections.iter().map(|p| proj(p.p1 - tri2.a)).collect();
        println!("-------- (copy/paste the following on Desmos graphing)");
        println!("A=({:.2},{:.2})", poly1[0].x, poly1[0].y);
        println!("B=({:.2},{:.2})", poly1[1].x, poly1[1].y);
        println!("C=({:.2},{:.2})", poly1[2].x, poly1[2].y);
        println!("D=({:.2},{:.2})", poly2[0].x, poly2[0].y);
        println!("E=({:.2},{:.2})", poly2[1].x, poly2[1].y);
        println!("F=({:.2},{:.2})", poly2[2].x, poly2[2].y);

        let lbls = ["G", "H", "I", "J", "K", "L", "M", "N", "O"];
        for (i, inter) in proj_inter.iter().enumerate() {
            println!("{}=({:.2},{:.2})", lbls[i], inter.x, inter.y);
        }

        // polygons
        println!("X=polygon(A,B,C)");
        println!("Y=polygon(D,E,F)");
        print!("Z=polygon({}", lbls[0]);
        for lbl in lbls.iter().skip(1) {
            print!(",{lbl}");
        }
        println!(")");

        println!("~~~~~~~ (copy/paste the following input in the `intersect_triangle_common_vertex` test)");
        println!("(Triangle::new(");
        for pt1 in poly1 {
            println!("    Vector::new({},{}),", pt1.x, pt1.y);
        }
        println!("),");
        println!("Triangle::new(");
        for pt2 in poly2 {
            println!("    Vector::new({},{}),", pt2.x, pt2.y);
        }
        println!("),),");
    }
}
