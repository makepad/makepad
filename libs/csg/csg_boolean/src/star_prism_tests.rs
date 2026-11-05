use crate::boolean::{mesh_boolean_with, BoolOp, FinishParams};
use crate::thread_pool;
use makepad_csg_math::{dvec3, Mat4d};
use makepad_csg_mesh::{validate::validate_mesh, volume::mesh_volume, TriMesh};

// csg_graph's cylinder primitive and Y-axis transform, as used by mod.model.
// Coordinates are millimetres; the two cap planes are initially y = 0 and 8.
fn prism(segments: u32, degrees: f64, lift: f64) -> TriMesh {
    let mut mesh = makepad_csg_primitives::cylinder(3.0, 8.0, segments, true);
    let (s, c) = degrees.to_radians().sin_cos();
    for p in &mut mesh.vertices {
        *p = dvec3(c * p.x + s * p.z, p.y + 4.0 + lift, -s * p.x + c * p.z);
    }
    mesh
}

fn check_solid(mesh: &TriMesh, expected: f64, context: &str) {
    let report = validate_mesh(mesh);
    assert_eq!(report.boundary_edges, 0, "{context}: {report:?}");
    assert_eq!(report.non_manifold_edges, 0, "{context}: {report:?}");
    assert!(report.is_consistently_oriented, "{context}: {report:?}");
    assert_eq!(report.degenerate_triangles, 0, "{context}: {report:?}");
    let volume = mesh_volume(mesh);
    assert!(
        (volume - expected).abs() < expected * 0.001,
        "{context}: volume={volume}, expected={expected}"
    );

    // An open surface's signed-volume sum changes on placement. This is
    // how an apparently reasonable local volume can turn negative on a plate.
    let mut placed = mesh.clone();
    // Printer [x, y, z] is model [x, -z, y].
    placed.transform(Mat4d::translation(dvec3(112.0, 0.0, -100.0)));
    assert!(
        (mesh_volume(&placed) - volume).abs() < expected * 1e-10,
        "{context}: placement changed volume"
    );
}

#[test]
fn star_prism_union() {
    for (name, segments, angle_a, angle_b, lift, expected) in [
        ("hexagram", 3, 90.0, 270.0, 0.0, 72.0 * 3.0_f64.sqrt()),
        // The 0.01 degree perturbation changes the analytic hexagram volume
        // by less than 0.000001 mm^3, well below the 0.1% volume assertion.
        ("off_axis", 3, 90.01, 270.0, 0.0, 72.0 * 3.0_f64.sqrt()),
        (
            "shifted_cap",
            3,
            90.0,
            270.0,
            0.01,
            (72.0 + 0.045) * 3.0_f64.sqrt(),
        ),
        ("octagram", 4, 0.0, 45.0, 0.0, 288.0 * (2.0 - 2.0_f64.sqrt())),
    ] {
        let a = prism(segments, angle_a, 0.0);
        let b = prism(segments, angle_b, lift);
        // Exercise both the public boolean default and csg_cad's part-scaled
        // finishing tolerance, which the script evaluator uses.
        let bounds = a.bounding_box().union(b.bounding_box());
        let script_tolerance = ((bounds.max - bounds.min).length() * 1e-6).clamp(1e-5, 1e-3);
        for tolerance in [FinishParams::default().tolerance, script_tolerance] {
            let finish = FinishParams { tolerance };
            let run = |a: &TriMesh, b: &TriMesh| mesh_boolean_with(a, b, BoolOp::Union, finish);
            let baseline = thread_pool::with_serial(|| run(&a, &b));
            check_solid(&baseline, expected, name);
            let bits = |mesh: &TriMesh| {
                mesh.vertices.iter().map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).collect::<Vec<_>>()
            };
            let baseline_bits = bits(&baseline);
            let tasks = (0..20).map(|_| {
                let (a, b) = (a.clone(), b.clone());
                move || mesh_boolean_with(&a, &b, BoolOp::Union, finish)
            }).collect();
            for (i, mesh) in thread_pool::parallel_for(tasks).iter().enumerate() {
                let context = format!("{name}, tolerance={tolerance}, run={i}");
                check_solid(mesh, expected, &context);
                assert_eq!(bits(mesh), baseline_bits, "{context}: vertex bits changed");
                assert_eq!(mesh.triangles, baseline.triangles, "{context}: triangles changed");
            }

            // Triangle and vertex indices are local to each operand. Reordering
            // them used to change which operand won an aliased edge-cache key,
            // changing even the volume and closedness of the same solid.
            let mut reordered = a.clone();
            reordered.triangles.reverse();
            check_solid(&run(&reordered, &b), expected, &format!("{name}: triangle order"));
            reordered.vertices.reverse();
            let last = reordered.vertices.len() as u32 - 1;
            for tri in &mut reordered.triangles {
                *tri = tri.map(|v| last - v);
            }
            check_solid(&run(&reordered, &b), expected, &format!("{name}: vertex order"));
            eprintln!("{name}: 20 identical unions, volume={:.9}, open=0, tolerance={tolerance}", mesh_volume(&baseline));
        }
    }
    thread_pool::shutdown();
}
