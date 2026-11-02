//! Exact-kernel parts through the tessellator: watertight meshes, guarded errors, edge convexity.
use makepad_csg_brep_model::ops::{self, Body};
use makepad_csg_brep_model::query;
use makepad_csg_brep_model::{guard, new_kernel, stack_size, tessellate, BrepError, CancelToken, TessOptions};
use makepad_csg_cad::{Shape, Sketch};

/// Signed volume and open-edge count of a mesh.
fn mesh_check(m: &makepad_csg_mesh::mesh::TriMesh) -> (f64, usize) {
    let mut vol = 0.0;
    let mut edges = std::collections::HashMap::<(u32, u32), i32>::new();
    for t in &m.triangles {
        let (a, b, c) = (m.vertices[t[0] as usize], m.vertices[t[1] as usize], m.vertices[t[2] as usize]);
        vol += a.x * (b.y * c.z - b.z * c.y) + a.y * (b.z * c.x - b.x * c.z) + a.z * (b.x * c.y - b.y * c.x);
        for (x, y) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (lo, hi, s) = if x < y { (x, y, 1) } else { (y, x, -1) };
            *edges.entry((lo, hi)).or_default() += s;
        }
    }
    (vol / 6.0, edges.values().filter(|v| **v != 0).count())
}

/// Within the guard's margin: no stack to spare at all.
const STACK_MARGIN_FOR_TEST: usize = 1 << 20;

/// Watertight between faces: every primitive at both tolerances, and the mesh's volume
/// agrees with the exact one.
#[test]
fn primitives_are_watertight() {
    let handle = std::thread::Builder::new().stack_size(stack_size()).spawn(|| {
        guard::guarded(stack_size(), &CancelToken::new(), || {
            let mut k = new_kernel();
            let parts: Vec<(&str, Body)> = vec![
                ("box", ops::make_box(&mut k, [10.0, 20.0, 30.0])?),
                ("cylinder", ops::make_cylinder(&mut k, 5.0, 20.0)?),
                ("cone", ops::make_cone(&mut k, 5.0, 2.0, 20.0)?),
                ("sphere", ops::make_sphere(&mut k, 10.0)?),
                ("torus", ops::make_torus(&mut k, 20.0, 5.0)?),
            ];
            for (name, mut b) in parts {
                let measured = query::measure(&k, &mut b)?;
                let exact = measured.volume;
                for chord in [0.1, 0.01] {
                    let t = tessellate(&k, &mut b, &TessOptions { chord, ..TessOptions::default() })?;
                    let (vol, open) = mesh_check(&t.mesh);
                    assert_eq!(open, 0, "{name} at {chord}: open edges");
                    assert!(vol > 0.0, "{name} at {chord}: inside out ({vol})");
                    // Within the chord, over the whole surface.
                    assert!((vol - exact).abs() <= measured.area * chord, "{name} at {chord}: mesh {vol} exact {exact}");
                }
            }
            Ok(())
        })
    }).unwrap();
    handle.join().unwrap().unwrap();
}

/// Past its stack budget a kernel call comes back as an error, and it can be cancelled.
#[test]
fn stack_limit_and_cancel_are_errors() {
    let handle = std::thread::Builder::new().stack_size(stack_size()).spawn(|| {
        // A budget the very first probe is past: as if the kernel had recursed that deep.
        let deep = guard::guarded(STACK_MARGIN_FOR_TEST, &CancelToken::new(), || ops::make_box(&mut new_kernel(), [10.0, 10.0, 10.0]).map(|_| ()));
        assert!(matches!(deep, Err(BrepError::Internal(_))), "{deep:?}");
        let cancel = CancelToken::new();
        cancel.cancel();
        let cancelled = guard::guarded(stack_size(), &cancel, || ops::make_box(&mut new_kernel(), [10.0, 10.0, 10.0]).map(|_| ()));
        assert_eq!(cancelled, Err(BrepError::Cancelled));
        // The thread goes on working.
        let again = guard::guarded(stack_size(), &CancelToken::new(), || ops::make_box(&mut new_kernel(), [10.0, 10.0, 10.0]).map(|_| ()));
        assert_eq!(again, Ok(()));
    }).unwrap();
    handle.join().unwrap();
}

/// An L section has one concave edge, and a fillet of it or of the convex ones works.
#[test]
fn edges_know_concave_from_convex() {
    let handle = std::thread::Builder::new().stack_size(stack_size()).spawn(|| {
        guard::guarded(stack_size(), &CancelToken::new(), || {
            let s = Sketch::new(vec![Shape::Polygon { points: vec![[-20.0, 0.0], [20.0, 0.0], [20.0, 6.0], [-14.0, 6.0], [-14.0, 46.0], [-20.0, 46.0]], fillet: 0.0 }]);
            let frame = makepad_csg_brep_model::profile::Frame { origin: [-30.0, 0.0, 0.0], u: [0.0, 0.0, 1.0], v: [0.0, 1.0, 0.0] };
            let mut k = new_kernel();
            let mut body = ops::extrude(&mut k, &s, &frame, [1.0, 0.0, 0.0], 60.0)?;
            let edges = query::edges(&k, &mut body)?;
            let concave: Vec<_> = edges.iter().filter(|e| !e.convex).collect();
            assert_eq!(concave.len(), 1, "{edges:?}");
            assert!((concave[0].mid[1] - 6.0).abs() < 1e-9 && (concave[0].mid[2] + 14.0).abs() < 1e-9);
            ops::fillet(&mut k, &mut body, &[concave[0].id], 2.0)?;
            ops::check_solid(&body, "fillet")?;
            Ok(())
        })
    }).unwrap();
    handle.join().unwrap().unwrap();
}
