use crate::boolean::{difference, union, mesh_boolean_with, BoolOp, FinishParams};
use makepad_csg_math::{dvec3, Mat4d};
use makepad_csg_mesh::{validate::validate_mesh, volume::{mesh_surface_area, mesh_volume}, TriMesh};
use makepad_csg_primitives::{cube, cylinder, sphere};
use std::f64::consts::PI;

fn dome_columns(columns: usize, top_flush: bool) {
    let radius = 4.8;
    let mut floor = cube(dvec3(100.0, 20.0, 100.0), true);
    floor.transform(Mat4d::translation(dvec3(0.0, -10.0, 0.0)));
    // An odd ring count makes the subtraction cut triangles across the bed.
    let mut dome = difference(&sphere(radius, 40, 21), &floor);
    let dome_report = validate_mesh(&dome);
    assert_eq!(dome_report.boundary_edges, 0, "dome: {dome_report:?}");
    assert_eq!(dome_report.non_manifold_edges, 0, "dome: {dome_report:?}");
    let reflect = |mesh: &mut TriMesh| {
        if top_flush {
            for p in &mut mesh.vertices {
                p.y = -p.y;
            }
            for t in &mut mesh.triangles {
                t.swap(1, 2);
            }
        }
    };
    reflect(&mut dome);
    for i in 0..columns {
        let mut column = cylinder(radius / 2.0, radius, 40, false);
        column.transform(Mat4d::translation(dvec3(if i == 0 { radius / 2.0 } else { -radius / 2.0 }, 0.0, 0.0)));
        reflect(&mut column);
        dome = union(&dome, &column);
    }
    // A radius-R/2 cylinder centered at x=R/2 has polar footprint
    // 0 <= r <= R*cos(theta), -pi/2 <= theta <= pi/2. Integrating
    // sqrt(R^2-r^2)*r gives hemisphere overlap R^3*(pi/3 - 4/9).
    // The two footprints have disjoint interiors, so the increments add.
    let expected = radius.powi(3) * (2.0 * PI / 3.0 + columns as f64 * (4.0 / 9.0 - PI / 12.0));
    let report = validate_mesh(&dome);
    let volume = mesh_volume(&dome);
    eprintln!("columns={columns} top_flush={top_flush}: open={} nonmanifold={} volume={volume:.9} analytic={expected:.9}", report.boundary_edges, report.non_manifold_edges);
    assert_eq!(report.boundary_edges, 0, "{report:?}");
    assert_eq!(report.non_manifold_edges, 0, "{report:?}");
    assert_eq!(report.degenerate_triangles, 0, "{report:?}");
    assert!(report.is_consistently_oriented, "{report:?}");
    // The reference is smooth; the operands use inscribed polygonal surfaces.
    assert!((volume - expected).abs() < expected * 0.02, "volume={volume}, analytic={expected}");
}

#[test]
fn coplanar_dome_one_column() {
    dome_columns(1, false);
}

#[test]
fn coplanar_dome_two_columns() {
    dome_columns(2, false);
}

#[test]
fn coplanar_dome_top_flush() {
    dome_columns(1, true);
}

fn flush_ellipsoid(columns: usize, top_flush: bool) {
    let mut body = sphere(1.0, 40, 21);
    for p in &mut body.vertices {
        *p = dvec3(p.x * 4.5 - 4.5, p.y * 4.8, p.z * 3.6);
    }
    let mut floor = cube(dvec3(100.0, 20.0, 100.0), true);
    floor.transform(Mat4d::translation(dvec3(0.0, -10.0, 0.0)));
    let mut body = difference(&body, &floor);
    let reflect = |mesh: &mut TriMesh| {
        if top_flush {
            for p in &mut mesh.vertices { p.y = -p.y; }
            for t in &mut mesh.triangles { t.swap(1, 2); }
        }
    };
    reflect(&mut body);
    for i in 0..columns {
        let mut column = cylinder(3.6, if i == 0 { 4.8 } else { 4.2 }, 48, false);
        column.transform(Mat4d::translation(dvec3(if i == 0 { 0.0 } else { -9.0 }, 0.0, 0.0)));
        reflect(&mut column);
        let bounds = body.bounding_box().union(column.bounding_box());
        let tolerance = ((bounds.max - bounds.min).length() * 1e-6).clamp(1e-5, 1e-3);
        body = mesh_boolean_with(&body, &column, BoolOp::Union, FinishParams { tolerance });
    }
    let report = validate_mesh(&body);
    eprintln!("ellipsoid columns={columns} top_flush={top_flush}: open={} nonmanifold={} volume={}", report.boundary_edges, report.non_manifold_edges, mesh_volume(&body));
    assert_eq!(report.boundary_edges, 0, "{report:?}");
    assert_eq!(report.non_manifold_edges, 0, "{report:?}");
    assert_eq!(report.degenerate_triangles, 0, "{report:?}");
}

#[test]
fn coplanar_ellipsoid_one_column() { flush_ellipsoid(1, false); }
#[test]
fn coplanar_ellipsoid_two_columns() { flush_ellipsoid(2, false); }
#[test]
fn coplanar_ellipsoid_top_flush() { flush_ellipsoid(1, true); }

#[test]
fn coplanar_boolean_orientation() {
    use crate::{classify_triangles, intersection, TriLocation};
    let a = cube(dvec3(1.0, 1.0, 1.0), true);
    assert!(classify_triangles(&a, &a, &vec![true; a.triangle_count()]).iter().all(|c| *c == TriLocation::OnBoundary));
    for (label, offset, volumes) in [
        ("identical", dvec3(0.0, 0.0, 0.0), [1.0, 0.0, 1.0]),
        ("same facing partial overlap", dvec3(0.25, 0.0, 0.0), [1.25, 0.25, 0.75]),
        ("opposite facing contact", dvec3(0.0, 1.0, 0.0), [2.0, 1.0, 0.0]),
    ] {
        let mut b = a.clone();
        b.transform(Mat4d::translation(offset));
        for (left,right) in [(&a,&b),(&b,&a)] {
            for (mesh, expected) in [union(left,right), difference(left,right), intersection(left,right)].into_iter().zip(volumes) {
                let report = validate_mesh(&mesh);
                assert_eq!((report.boundary_edges,report.non_manifold_edges,report.degenerate_triangles), (0,0,0), "{label}: {report:?}");
                assert!(report.is_consistently_oriented, "{label}: {report:?}");
                assert!((mesh_volume(&mesh) - expected).abs() < 1e-9, "{label}: volume={} expected={expected}", mesh_volume(&mesh));
            }
        }
    }
}

fn face_touching_boxes(op: BoolOp) {
    let a = cube(dvec3(4.0, 4.0, 4.0), true);
    let mut b = cube(dvec3(2.0, 2.0, 2.0), true);
    b.transform(Mat4d::translation(dvec3(0.0, -3.0, 0.0)));
    for (left, right, volume) in [(&a, &b, 64.0), (&b, &a, 8.0)] {
        let mesh = crate::mesh_boolean(left, right, op);
        let report = validate_mesh(&mesh);
        eprintln!("{op:?}: {report:?} volume={}", mesh_volume(&mesh));
        assert_eq!((report.boundary_edges, report.non_manifold_edges, report.degenerate_triangles), (0, 0, 0), "{op:?}: {report:?}");
        assert!(report.is_consistently_oriented, "{op:?}: {report:?}");
        let expected = match op {
            BoolOp::Union => 72.0,
            BoolOp::Difference => volume,
            BoolOp::Intersection => 0.0,
        };
        assert_eq!(mesh_volume(&mesh), expected, "{op:?}: volume");
        let expected_area = match op {
            BoolOp::Union => 112.0,
            BoolOp::Difference => mesh_surface_area(left),
            BoolOp::Intersection => 0.0,
        };
        assert_eq!(mesh_surface_area(&mesh), expected_area, "{op:?}: surface area");
        if op == BoolOp::Difference {
            assert_eq!(mesh.bounding_box().min, left.bounding_box().min);
            assert_eq!(mesh.bounding_box().max, left.bounding_box().max);
        }
        if op == BoolOp::Intersection {
            assert!(mesh.triangles.is_empty(), "touching solids have no volume intersection");
            continue;
        }
        // Follow triangle edges, so two closed but disconnected operands cannot
        // pass merely by having the correct sum of their volumes.
        let mut edges = std::collections::HashMap::new();
        let mut adjacent = vec![Vec::new(); mesh.triangle_count()];
        for (ti, &[a, b, c]) in mesh.triangles.iter().enumerate() {
            for (a, b) in [(a, b), (b, c), (c, a)] {
                if let Some(other) = edges.insert((a.min(b), a.max(b)), ti) {
                    adjacent[ti].push(other);
                    adjacent[other].push(ti);
                }
            }
        }
        let mut seen = vec![false; mesh.triangle_count()];
        let mut pending = vec![0];
        while let Some(ti) = pending.pop() {
            if !seen[ti] {
                seen[ti] = true;
                pending.extend(&adjacent[ti]);
            }
        }
        assert!(seen.into_iter().all(|s| s), "{op:?}: expected one closed shell");
        if op == BoolOp::Union {
            assert!(!mesh.triangles.iter().any(|t| {
                t.iter().all(|&i| {
                    let p = mesh.vertices[i as usize];
                    p.y == -2.0 && p.x.abs() <= 1.0 && p.z.abs() <= 1.0
                })
            }), "union retained a shared face");
        }
    }
}

#[test]
fn face_touching_boxes_union() { face_touching_boxes(BoolOp::Union); }

#[test]
fn face_touching_boxes_difference() { face_touching_boxes(BoolOp::Difference); }

#[test]
fn face_touching_boxes_intersection() { face_touching_boxes(BoolOp::Intersection); }
