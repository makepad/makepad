use crate::{difference, union};
use makepad_csg_math::{dvec3, Mat4d};
use makepad_csg_mesh::{mesh::TriMesh, validate::validate_mesh, volume::mesh_volume};
use makepad_csg_primitives::{cube, cylinder};
use std::collections::{HashMap, HashSet};

fn check(mesh: &TriMesh, expected: f64, relative: f64) -> usize {
    let report = validate_mesh(mesh);
    assert!(report.is_closed && report.is_manifold && report.is_consistently_oriented, "{report:?}");
    assert_eq!(report.degenerate_triangles, 0, "{report:?}");
    let volume = mesh_volume(mesh);
    assert!((volume - expected).abs() <= relative * expected.abs(), "volume={volume:.17}, expected={expected:.17}");
    let mut edges = HashMap::<(u32, u32), Vec<usize>>::new();
    let mut incident = vec![Vec::new(); mesh.vertex_count()];
    for (ti, tri) in mesh.triangles.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            edges.entry((a.min(b), a.max(b))).or_default().push(ti);
            incident[a as usize].push(ti);
        }
    }
    let mut adjacent = vec![Vec::new(); mesh.triangle_count()];
    for group in edges.values() {
        assert_eq!(group.len(), 2);
        adjacent[group[0]].push(group[1]);
        adjacent[group[1]].push(group[0]);
    }
    for (vi, faces) in incident.iter().enumerate().filter(|(_, f)| !f.is_empty()) {
        let mut seen = HashSet::new();
        let mut pending = vec![faces[0]];
        while let Some(ti) = pending.pop() {
            if seen.insert(ti) {
                pending.extend(adjacent[ti].iter().copied().filter(|&tj| mesh.triangles[tj].contains(&(vi as u32))));
            }
        }
        assert_eq!(seen.len(), faces.len(), "multiple vertex fans at {vi}");
    }
    let mut seen = HashSet::new();
    let mut shells = 0;
    for start in 0..mesh.triangle_count() {
        if seen.contains(&start) { continue; }
        shells += 1;
        let mut pending = vec![start];
        while let Some(ti) = pending.pop() {
            if seen.insert(ti) { pending.extend(&adjacent[ti]); }
        }
    }
    shells
}

#[test]
fn outside_line_and_face_contact() {
    let a = cube(dvec3(1.0, 1.0, 1.0), true);
    for face in [false, true] {
        let mut b = cylinder(0.5, 0.5, 8, true);
        let angle = if face { std::f64::consts::PI / 8.0 } else { 0.0 };
        b.transform(Mat4d::rotation(dvec3(0.0, 1.0, 0.0), angle));
        b.transform(Mat4d::translation(dvec3(0.5 + 0.5 * angle.cos(), 0.0, 0.0)));
        assert_eq!(check(&difference(&a, &b), mesh_volume(&a), 1e-12), 1);
        assert_eq!(check(&union(&a, &b), mesh_volume(&a) + mesh_volume(&b), 1e-12), if face { 1 } else { 2 });
    }
}

#[test]
fn inside_disc_tangent_lines() {
    let a = cube(dvec3(1.0, 1.0, 1.0), true);
    for t in [1e-3, 1e-6] {
        for seg in [8, 9] {
            let b = cylinder(0.5, t, seg, true);
            let result = difference(&a, &b);
            eprintln!("disc t={t} seg={seg}");
            check(&result, mesh_volume(&a) - mesh_volume(&b), 1e-9);
        }
    }
}

#[test]
fn point_contact_union() {
    let a = cube(dvec3(1.0, 1.0, 1.0), true);
    let mut b = a.clone();
    b.transform(Mat4d::translation(dvec3(1.0, 1.0, 1.0)));
    assert_eq!(check(&union(&a, &b), 2.0, 1e-12), 2);
}

#[test]
fn near_face_contact_has_no_duplicate_wall() {
    // The face-pair scale is 1, but the operand-wide scale is 2. A weld at
    // the latter scale must not join faces the predicates treated as separate.
    let a = cube(dvec3(1.0, 1.0, 1.0), false);
    let mut b = a.clone();
    b.transform(Mat4d::translation(dvec3(1.0 + 1e-9, 0.0, 0.0)));
    assert_eq!(check(&union(&a, &b), 2.0, 2e-9), 1);
    let mut b = a.clone();
    b.transform(Mat4d::translation(dvec3(1.0 + 3e-9, 0.0, 0.0)));
    assert_eq!(check(&union(&a, &b), 2.0, 1e-12), 2);
}

#[test]
fn overlapping_edge_faces_are_not_separated() {
    let mut mesh = TriMesh::new();
    for p in [dvec3(0.0, 0.0, 0.0), dvec3(0.0, 0.0, 1.0),
        dvec3(1.0, 0.0, 0.0), dvec3(2.0, 0.0, 0.0),
        dvec3(-1.0, 0.0, 0.0), dvec3(0.0, 1.0, 0.0)] {
        mesh.add_vertex(p);
    }
    for tri in [[0, 1, 2], [1, 0, 3], [0, 1, 4], [1, 0, 5]] {
        mesh.triangles.push(tri);
    }
    let active = vec![true; mesh.vertex_count()];
    crate::contact::separate_contact_fans(&mut mesh, &active);
    assert_eq!(validate_mesh(&mesh).non_manifold_edges, 1,
        "opposite faces overlapping along one ray must stay flagged");
}

#[test]
fn tangent_segment_without_an_interior_vertex() {
    let a = cube(dvec3(1.0, 1.0, 1.0), true);
    let mut b = cylinder(0.5, 1e-3, 9, true);
    b.transform(Mat4d::translation(dvec3(0.0, 0.25, 0.0)));
    check(&difference(&a, &b), mesh_volume(&a) - mesh_volume(&b), 1e-9);
}

#[test]
fn contact_operand_with_distant_second_seam() {
    let a = cube(dvec3(1.0, 1.0, 1.0), true);
    let mut b = cylinder(0.5, 0.5, 8, true);
    b.transform(Mat4d::translation(dvec3(1.0, 0.0, 0.0)));
    let pinched = union(&a, &b);
    let volume = mesh_volume(&a) + mesh_volume(&b);
    assert_eq!(check(&pinched, volume, 1e-12), 2);
    let mut c = cube(dvec3(0.25, 0.25, 0.25), true);
    c.transform(Mat4d::translation(dvec3(-0.5, -0.5, -0.5)));
    // The weld rejoins the two shells' contact copies even though this cut
    // reaches only the opposite corner. Each output vertex must have one fan.
    assert_eq!(check(&union(&pinched, &c), volume + 7.0 / 512.0, 1e-12), 2);
}

#[test]
fn contact_operand_with_second_seam_through_pinch() {
    let a = cube(dvec3(1.0, 1.0, 1.0), true);
    let mut b = cylinder(0.5, 0.5, 8, true);
    b.transform(Mat4d::translation(dvec3(1.0, 0.0, 0.0)));
    let pinched = union(&a, &b);
    let volume = mesh_volume(&a) + mesh_volume(&b);
    assert_eq!(check(&pinched, volume, 1e-12), 2);
    let mut cutter = cube(dvec3(3.0, 0.25, 2.0), true);
    cutter.transform(Mat4d::translation(dvec3(0.5, 0.0, 0.0)));
    // Both new horizontal seams cross the tangent line; the remaining four
    // pieces each need a separate fan at the ends of their contact segments.
    assert_eq!(check(&difference(&pinched, &cutter), 0.75 + mesh_volume(&b) * 0.5, 1e-12), 4);
}
