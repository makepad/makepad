use crate::boolean::{mesh_boolean_in, mesh_boolean_work, BoolOp, FinishParams};
use crate::{thread_pool, MeshAccel};
use makepad_csg_math::{dvec3, Mat4d, Vec3d};
use makepad_csg_mesh::TriMesh;
use makepad_csg_primitives::{cube, cylinder};

const OPS: [BoolOp; 3] = [BoolOp::Union, BoolOp::Difference, BoolOp::Intersection];

fn distinct_tags(mesh: &mut TriMesh, first: u32) {
    mesh.tags = (0..mesh.triangle_count()).map(|i| first + i as u32).collect();
}

fn untagged(mesh: &TriMesh) -> TriMesh {
    let mut mesh = mesh.clone();
    mesh.tags = Vec::new();
    mesh
}

// Closest point by the triangle's vertex, edge and interior Voronoi regions.
fn distance_sq(p: Vec3d, a: Vec3d, b: Vec3d, c: Vec3d) -> f64 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 { return p.distance_sq(a); }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 { return p.distance_sq(b); }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return p.distance_sq(a + ab * (d1 / (d1 - d3)));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 { return p.distance_sq(c); }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return p.distance_sq(a + ac * (d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let t = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return p.distance_sq(b + (c - b) * t);
    }
    let inv = 1.0 / (va + vb + vc);
    p.distance_sq(a + ab * (vb * inv) + ac * (vc * inv))
}

/// Check source ownership, tag storage and bit-for-bit geometry after all finishing passes.
fn check(case: &str, a: &TriMesh, b: &TriMesh, op: BoolOp, prepared: bool) -> (TriMesh, usize) {
    thread_pool::with_serial(|| {
        let finish = FinishParams::default();
        let (plain_a, plain_b) = (untagged(a), untagged(b));
        let ((out, took), plain) = if prepared {
            let (pa, pb) = (MeshAccel::build(a), MeshAccel::build(b));
            ((mesh_boolean_in(a, &pa, b, &pb, op, finish), a.triangle_count() + b.triangle_count()),
                mesh_boolean_in(&plain_a, &pa, &plain_b, &pb, op, finish))
        } else {
            let result = mesh_boolean_work(a, b, op, finish);
            let (plain, took) = mesh_boolean_work(&plain_a, &plain_b, op, finish);
            assert_eq!(result.1, took, "{case} {op:?}: tags changed participating triangles");
            (result, plain)
        };
        assert!(out.tags.is_empty() || out.tags.len() == out.triangle_count(), "{case} {op:?}: tag length invariant");
        if a.is_tagged() || b.is_tagged() {
            assert_eq!(out.tags.len(), out.triangle_count(), "{case} {op:?}: tagged output length");
        } else {
            assert!(out.tags.is_empty(), "{case} {op:?}: untagged output");
        }
        assert_eq!(plain.tags.capacity(), 0, "{case} {op:?}: untagged output allocated tags");
        let bits = |m: &TriMesh| m.vertices.iter().map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).collect::<Vec<_>>();
        assert_eq!(bits(&out), bits(&plain), "{case} {op:?}: vertex bits changed");
        assert_eq!(out.triangles, plain.triangles, "{case} {op:?}: triangles changed");
        for ti in 0..out.triangle_count() {
            let (x, y, z) = out.triangle_vertices(ti);
            let centroid = x + ((y - x) + (z - x)) / 3.0;
            let tag = out.tag(ti);
            let mut nearest = f64::INFINITY;
            for source in [a, b] {
                for si in 0..source.triangle_count() {
                    if source.tag(si) == tag {
                        let (x, y, z) = source.triangle_vertices(si);
                        nearest = nearest.min(distance_sq(centroid, x, y, z));
                    }
                }
            }
            assert!(nearest <= finish.tolerance * finish.tolerance,
                "{case} {op:?}: triangle {ti}, tag {tag}, source distance {} exceeds {}", nearest.sqrt(), finish.tolerance);
        }
        (out, took)
    })
}

fn check_distinct(case: &str, mut a: TriMesh, mut b: TriMesh) {
    distinct_tags(&mut a, 1);
    distinct_tags(&mut b, 1001);
    for op in OPS { check(case, &a, &b, op, false); }
}

fn overlap_boxes() -> (TriMesh, TriMesh) {
    let a = cube(dvec3(2.0, 2.0, 2.0), true);
    let mut b = a.clone();
    b.transform(Mat4d::translation(dvec3(0.7, 0.4, 0.3)));
    (a, b)
}

#[test]
fn face_tags_and_prepared_operands() {
    let (mut a, mut b) = overlap_boxes();
    a.tags = (0..a.triangle_count()).map(|i| 1 + i as u32 / 2).collect();
    b.tags = (0..b.triangle_count()).map(|i| 11 + i as u32 / 2).collect();
    for op in OPS {
        check("box faces", &a, &b, op, false);
        check("prepared box faces", &a, &b, op, true);
    }
}

#[test]
fn tags_through_rounding_coplanar_and_contact_fixtures() {
    let a = crate::rounding_tests::rounded_grid_cap_mesh();
    check_distinct("rounded grid cap", a, cylinder(1.4, 3.0, 16, false));
    let (a, b) = crate::zero_contact_tests::tangent_segment_meshes();
    check_distinct("tangent segment", a, b);
    let (a, b) = crate::coplanar_tests::face_touching_box_meshes();
    check_distinct("face touching boxes", a, b);
    let (a, b) = crate::twisted_contact_tests::twisted_star_meshes();
    check_distinct("twisted exact face contact", a, b);

    for (case, segments, angle_a, angle_b, lift) in [
        ("hexagram", 3, 90.0, 270.0, 0.0),
        ("off axis", 3, 90.01, 270.0, 0.0),
        ("shifted cap", 3, 90.0, 270.0, 0.01),
        ("octagram", 4, 0.0, 45.0, 0.0),
    ] {
        check_distinct(case, crate::star_prism_tests::prism(segments, angle_a, 0.0),
            crate::star_prism_tests::prism(segments, angle_b, lift));
    }
    let a = cube(dvec3(1.0, 1.0, 1.0), true);
    for (case, offset) in [
        ("identical", dvec3(0.0, 0.0, 0.0)),
        ("same facing partial overlap", dvec3(0.25, 0.0, 0.0)),
        ("opposite facing contact", dvec3(0.0, 1.0, 0.0)),
        ("point contact", dvec3(1.0, 1.0, 1.0)),
        ("near face contact", dvec3(1.0 + 1e-9, 0.0, 0.0)),
    ] {
        let mut b = a.clone();
        b.transform(Mat4d::translation(offset));
        check_distinct(case, a.clone(), b);
    }
    for (case, face) in [("outside line contact", false), ("outside face contact", true)] {
        let mut b = cylinder(0.5, 0.5, 8, true);
        let angle = if face { std::f64::consts::PI / 8.0 } else { 0.0 };
        b.transform(Mat4d::rotation(dvec3(0.0, 1.0, 0.0), angle));
        b.transform(Mat4d::translation(dvec3(0.5 + 0.5 * angle.cos(), 0.0, 0.0)));
        check_distinct(case, a.clone(), b);
    }
}

#[test]
fn far_triangle_tags() {
    let mut a = cube(dvec3(10.0, 10.0, 10.0), true);
    // Four segments per original edge: most faces are outside the corner cutter's reach.
    for _ in 0..2 {
        let mut next = TriMesh::new();
        next.vertices = a.vertices.clone();
        let mut midpoints = std::collections::HashMap::new();
        for &[x, y, z] in &a.triangles {
            let mut midpoint = |x: u32, y: u32| {
                *midpoints.entry((x.min(y), x.max(y))).or_insert_with(||
                    next.add_vertex((a.vertices[x as usize] + a.vertices[y as usize]) * 0.5))
            };
            let (xy, yz, zx) = (midpoint(x, y), midpoint(y, z), midpoint(z, x));
            for [x, y, z] in [[x, xy, zx], [xy, y, yz], [zx, yz, z], [xy, yz, zx]] {
                next.add_triangle(x, y, z);
            }
        }
        a = next;
    }
    let mut b = cube(dvec3(1.0, 1.0, 1.0), true);
    b.transform(Mat4d::translation(dvec3(4.8, 4.7, 4.6)));
    distinct_tags(&mut a, 1);
    distinct_tags(&mut b, 1001);
    for op in OPS {
        let (_, took) = check("subdivided box corner", &a, &b, op, false);
        assert!(took < a.triangle_count() + b.triangle_count(), "subdivided box corner {op:?}: no far triangles");
    }
}

#[test]
fn staged_tags_and_zero_tags() {
    let (mut a, mut b) = overlap_boxes();
    let mut c = a.clone();
    c.transform(Mat4d::translation(dvec3(-0.6, 0.2, 0.8)));
    distinct_tags(&mut a, 1);
    distinct_tags(&mut b, 1001);
    distinct_tags(&mut c, 2001);
    let (ab, _) = check("staged A union B", &a, &b, BoolOp::Union, false);
    for op in OPS { check("staged AB with C", &ab, &c, op, false); }

    for op in OPS {
        for (case, a, b) in [
            ("only A tagged", a.clone(), untagged(&b)),
            ("only B tagged", untagged(&a), b.clone()),
        ] {
            let (out, _) = check(case, &a, &b, op, false);
            assert!(out.tags.contains(&0), "{case} {op:?}: missing untagged surface");
            assert!(out.tags.iter().any(|&tag| tag != 0), "{case} {op:?}: missing tagged surface");
        }
        check("both untagged", &untagged(&a), &untagged(&b), op, false);
        let mut zeros = a.clone();
        zeros.tags.fill(0);
        let (out, _) = check("explicit zero tags", &zeros, &untagged(&b), op, false);
        assert!(out.tags.iter().all(|&tag| tag == 0), "explicit zero tags {op:?}: nonzero tag");
        let mut far = b.clone();
        far.transform(Mat4d::translation(dvec3(10.0, 0.0, 0.0)));
        let (out, _) = check("tagged far operand", &untagged(&a), &far, op, false);
        if op == BoolOp::Difference {
            assert!(!out.tags.is_empty() && out.tags.iter().all(|&tag| tag == 0),
                "tagged far operand {op:?}: discarded tagged input still requires explicit zero tags");
        }
    }
}
