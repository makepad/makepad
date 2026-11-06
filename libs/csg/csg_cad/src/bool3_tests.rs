use crate::{bool3, measure, solids, Region};
use makepad_csg_math::dvec3;
use makepad_csg_mesh::{mesh::TriMesh, validate::validate_mesh};

fn prism(y0: f64, height: f64, degrees: f64) -> TriMesh {
    let outer = (0..12)
        .map(|i| {
            let angle = i as f64 * std::f64::consts::TAU / 12.0;
            let radius = if i % 2 == 0 { 3.0 } else { 2.0 };
            [radius * angle.cos(), radius * angle.sin()]
        })
        .collect();
    let mut mesh = solids::extrude(&[Region { outer, holes: Vec::new() }], height, 0.0, false).unwrap();
    let (s, c) = degrees.to_radians().sin_cos();
    for p in &mut mesh.vertices {
        // Rotate in the sketch plane, then map +Z to +Y without reflecting.
        *p = dvec3(c * p.x - s * p.y, p.z + y0, -(s * p.x + c * p.y));
    }
    mesh
}

fn check(label: &str, mesh: &TriMesh, expected_volume: f64) {
    let shells = measure::shells(mesh).len();
    let report = validate_mesh(mesh);
    let volume = solids::signed_volume(mesh);
    println!("{label}: shells={shells}, volume={volume:.17}, expected={expected_volume:.17}, {report:?}");
    assert_eq!(shells, 1, "{label}: shell count");
    assert!(report.is_closed, "{label}: {report:?}");
    assert!(report.is_manifold, "{label}: {report:?}");
    assert!(report.is_consistently_oriented, "{label}: {report:?}");
    assert!((volume - expected_volume).abs() < expected_volume.abs() * 1e-9, "{label}: volume");
}

#[test]
fn two_twisted_prisms_touching_one_ulp_apart_fuse() {
    let height = (0.01 / 25.0) * 1000.0;
    let lower = prism((6.0 * 0.01 / 25.0) * 1000.0, height, 0.0);
    assert_eq!(lower.bounding_box().max.y, 2.8);
    // (k*h/n)*1000 + (h/n)*1000 and ((k+1)*h/n)*1000 round
    // to 2.8 and its next f64 respectively for k = 6, h = 0.01, n = 25.
    // Check both controls before the gap so the RED run records them too.
    for (label, bottom) in [
        ("equal", 2.8),
        ("overlap", f64::from_bits(2.8f64.to_bits() - 1)),
        ("gap", f64::from_bits(2.8f64.to_bits() + 1)),
    ] {
        let upper = prism(bottom, height, 1.6);
        let expected = solids::signed_volume(&lower) + solids::signed_volume(&upper);
        check(label, &bool3::union(&lower, &upper), expected);
        let prepared = bool3::boolean_in(
            &lower, &std::sync::OnceLock::new(),
            &upper, &std::sync::OnceLock::new(), crate::BoolOp::Union,
        );
        check(&format!("{label} prepared"), &prepared, expected);
    }
}

#[test]
fn a_twisted_stack_is_one_shell() {
    let height = (0.01 / 25.0) * 1000.0;
    let mut mesh = TriMesh::new();
    let mut expected = 0.0;
    for k in 0..25 {
        let slab = prism((k as f64 * 0.01 / 25.0) * 1000.0, height, k as f64 * 1.6);
        expected += solids::signed_volume(&slab);
        mesh = bool3::union(&mesh, &slab);
    }
    check("stack", &mesh, expected);
}

/// The bit pattern of each triangle of `m` whose corners all satisfy `keep`, sorted.
fn triangles_where(m: &TriMesh, keep: impl Fn(makepad_csg_math::Vec3d) -> bool) -> Vec<[[u64; 3]; 3]> {
    let mut tris: Vec<_> = m.triangles.iter().filter_map(|t| {
        let p = t.map(|i| m.vertices[i as usize]);
        p.iter().all(|&p| keep(p)).then(|| p.map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]))
    }).collect();
    tris.sort_unstable();
    tris
}

#[test]
fn a_local_cut_preserves_distant_triangles() {
    let mut near = prism(0.0, 1.0, 0.0);
    let mut far = prism(0.0, 1.0, 0.0);
    for p in &mut far.vertices {
        p.x += 20.0;
    }
    // A legitimate small face on a distant cap. Finishing the whole mesh
    // flips its diagonal even though this cut cannot reach that cap.
    let [a, b, c] = far.triangles.remove(0);
    let p = far.vertices[a as usize] * 0.5
        + far.vertices[b as usize] * (0.5 - 1e-8)
        + far.vertices[c as usize] * 1e-8;
    let d = far.add_vertex(p);
    far.triangles.extend([[a, b, d], [b, c, d], [c, a, d]]);
    near.merge(&far);
    let cutter = prism(0.5, 1.0, 13.0);
    let result = bool3::difference(&near, &cutter);
    let distant = |p: makepad_csg_math::Vec3d| p.x > 10.0;
    assert_eq!(triangles_where(&result, distant), triangles_where(&near, distant), "a distant cap must pass through bit for bit");
    let report = validate_mesh(&result);
    assert!(report.is_closed && report.is_manifold && report.is_consistently_oriented, "{report:?}");
    assert_eq!(measure::shells(&result).len(), 2);
}

/// A cutter inside a solid, far from all of its faces: no triangle of the solid takes part, and the cutter's
/// side is told from reference points alone. The result is the solid with a closed cavity.
#[test]
fn a_cavity_far_from_every_face_is_cut() {
    let outer = prism(0.0, 10.0, 0.0);
    let mut cavity = prism(4.0, 1.0, 7.0);
    for p in &mut cavity.vertices {
        p.x *= 0.3;
        p.z *= 0.3;
    }
    let result = bool3::difference(&outer, &cavity);
    let report = validate_mesh(&result);
    assert!(report.is_closed && report.is_manifold && report.is_consistently_oriented, "{report:?}");
    assert_eq!(measure::shells(&result).len(), 2, "the solid and its cavity");
    let expected = solids::signed_volume(&outer) - solids::signed_volume(&cavity);
    assert!((solids::signed_volume(&result) - expected).abs() < expected * 1e-12, "volume");
    // And a union of the same: the cutter is inside, nothing of it stays.
    let merged = bool3::union(&outer, &cavity);
    assert_eq!(triangles_where(&merged, |_| true), triangles_where(&outer, |_| true));
}
