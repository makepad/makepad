use crate::boolean::{mesh_boolean_with, BoolOp, FinishParams};
use makepad_csg_math::{dvec3, Mat4d};
use makepad_csg_mesh::{validate::validate_mesh, volume::mesh_volume};

#[test]
fn twisted_star_exact_face_contact() {
    // twisted_stack seed 287: the first pair already exposed a cap seam.
    let outline = [
        [3.5322014275280327, 0.0],
        [1.7868923570464759, 1.7868923570464756],
        [2.1628495860829584e-16, 3.5322014275280327],
        [-1.7868923570464756, 1.7868923570464759],
        [-3.5322014275280327, 4.3256991721659167e-16],
        [-1.786892357046476, -1.7868923570464756],
        [-6.488548758248875e-16, -3.5322014275280327],
        [1.7868923570464752, -1.786892357046476],
    ];
    let height = (0.01231549013223493 / 5.0) * 1000.0;
    let a = makepad_csg_primitives::extrude(&outline, height);
    let mut b = a.clone();
    b.transform(Mat4d::rotation(
        dvec3(0.0, 1.0, 0.0),
        1.1435348187473924_f64.to_radians(),
    ));
    b.transform(Mat4d::translation(dvec3(0.0, height, 0.0)));
    assert_eq!(a.bounding_box().max.y, b.bounding_box().min.y);
    let expected = mesh_volume(&a) + mesh_volume(&b);
    let mesh = mesh_boolean_with(&a, &b, BoolOp::Union, FinishParams::default());
    let report = validate_mesh(&mesh);
    let volume = mesh_volume(&mesh);
    eprintln!("exact contact: {report:?}, volume={volume:.17}, expected={expected:.17}");
    assert!(report.is_closed, "{report:?}");
    assert!(report.is_manifold, "{report:?}");
    assert!(report.is_consistently_oriented, "{report:?}");
    assert_eq!(report.degenerate_triangles, 0, "{report:?}");
    assert!(
        (volume - expected).abs() <= expected.abs() * 1e-9,
        "volume={volume}, expected={expected}"
    );

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
    assert!(seen.into_iter().all(|s| s), "expected one connected shell");
}
