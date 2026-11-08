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
        guard::guarded(stack_size(), &CancelToken::new(), None, || {
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
        let deep = guard::guarded(STACK_MARGIN_FOR_TEST, &CancelToken::new(), None, || ops::make_box(&mut new_kernel(), [10.0, 10.0, 10.0]).map(|_| ()));
        assert!(matches!(deep, Err(BrepError::Internal(_))), "{deep:?}");
        let cancel = CancelToken::new();
        cancel.cancel();
        let cancelled = guard::guarded(stack_size(), &cancel, None, || ops::make_box(&mut new_kernel(), [10.0, 10.0, 10.0]).map(|_| ()));
        assert_eq!(cancelled, Err(BrepError::Cancelled));
        // The thread goes on working.
        let again = guard::guarded(stack_size(), &CancelToken::new(), None, || ops::make_box(&mut new_kernel(), [10.0, 10.0, 10.0]).map(|_| ()));
        assert_eq!(again, Ok(()));
    }).unwrap();
    handle.join().unwrap();
}

/// An L section has one concave edge, and a fillet of it or of the convex ones works.
#[test]
fn edges_know_concave_from_convex() {
    let handle = std::thread::Builder::new().stack_size(stack_size()).spawn(|| {
        guard::guarded(stack_size(), &CancelToken::new(), None, || {
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

/// One body per recipe, whatever the engine did before: a part meshed at print detail on an
/// engine that built nothing else, and at display detail on another, is byte for byte the part
/// meshed after a history of its own (another width's part built step by step, shown, measured
/// and listed; the part's own steps shown before it is built, then a drag's coarser mesh), and
/// its exact measure is the same.
#[test]
fn a_body_is_the_same_whatever_was_built_before() {
    use makepad_csg_brep_model::engine::Engine;
    use makepad_csg_graph::edges::{EdgeFilter, EdgeSel};
    use makepad_csg_graph::{BoolMode, BrepEngine, BrepOp, BrepStep};
    use makepad_csg_mesh::mesh::TriMesh;
    fn key(name: &str) -> [u8; 32] {
        let mut k = [0; 32];
        k[..name.len()].copy_from_slice(name.as_bytes());
        k
    }
    // A block with its top edges rounded and a bore through it: a fillet and a boolean, which draw
    // on the kernel's random numbers.
    let part = |width: f64| {
        let (block, round, bore, cut) = (key(&format!("block {width}")), key(&format!("round {width}")), key(&format!("bore {width}")), key(&format!("cut {width}")));
        vec![
            BrepStep { key: block, op: BrepOp::Box { size: [width, 20.0, 30.0] }, inputs: Vec::new() },
            BrepStep { key: round, op: BrepOp::Fillet { radius: 2.0, edges: EdgeSel { any: vec![EdgeFilter { top: true, ..EdgeFilter::default() }] } }, inputs: vec![block] },
            BrepStep { key: bore, op: BrepOp::Cylinder { radius: 5.0, top_radius: 5.0, height: 60.0 }, inputs: Vec::new() },
            BrepStep { key: cut, op: BrepOp::Boolean { mode: BoolMode::Difference }, inputs: vec![round, bore] },
        ]
    };
    let bits = |m: &TriMesh| {
        let words = |v: &[makepad_csg_math::Vec3d]| v.iter().flat_map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).collect::<Vec<u64>>();
        (words(&m.vertices), m.triangles.clone(), words(&m.corner_normals))
    };
    let part_40 = part(40.0);
    let print = Engine::default().mesh(part_40.clone(), 0.0125, None).expect("the part builds");
    let fresh = Engine::default();
    let display = fresh.mesh(part_40.clone(), 0.05, None).expect("the part builds");
    let measure = fresh.measure(part_40.clone()).expect("the part measures");
    let busy = Engine::default();
    let part_56 = part(56.0);
    for n in 1..=part_56.len() {
        busy.mesh(part_56[..n].to_vec(), 0.05, None).expect("the other part builds");
    }
    busy.measure(part_56.clone()).expect("the other part measures");
    busy.entities(part_56).expect("the other part lists its entities");
    for n in 1..part_40.len() {
        busy.mesh(part_40[..n].to_vec(), 0.05, None).expect("a step builds");
    }
    busy.mesh(part_40.clone(), 0.1, None).expect("the part builds");
    assert!(bits(&busy.mesh(part_40.clone(), 0.05, None).unwrap()) == bits(&display), "the display mesh depends on what was built before");
    assert_eq!(busy.measure(part_40.clone()).unwrap(), measure, "the measure depends on what was built before");
    busy.entities(part_40.clone()).expect("the part lists its entities");
    assert!(bits(&busy.mesh(part_40, 0.0125, None).unwrap()) == bits(&print), "the print mesh depends on what was built before");
}

/// A Background build gives way to a preview sent while it builds: the preview's answer comes
/// first, at the build's next checkpoint, and the build then goes on from the steps it finished
/// to the body it builds alone. An engine holding a body's steps says so at once, whatever it is
/// building meanwhile.
#[test]
fn a_background_build_gives_way_to_a_preview() {
    use makepad_csg_brep_model::engine::Engine;
    use makepad_csg_graph::edges::{EdgeFilter, EdgeSel};
    use makepad_csg_graph::{BrepEngine, BrepOp, BrepStep, Open};
    use std::sync::mpsc::channel;
    use std::sync::Arc;
    fn key(name: &str) -> [u8; 32] {
        let mut k = [0; 32];
        k[..name.len()].copy_from_slice(name.as_bytes());
        k
    }
    // A demo's housing: a block, its top edges rounded, hollowed with its bottom open.
    let (block, round, hollow) = (key("demo block"), key("demo round"), key("demo hollow"));
    let demo = vec![
        BrepStep { key: block, op: BrepOp::Box { size: [96.0, 30.0, 96.0] }, inputs: Vec::new() },
        BrepStep { key: round, op: BrepOp::Fillet { radius: 4.0, edges: EdgeSel { any: vec![EdgeFilter { top: true, ..EdgeFilter::default() }] } }, inputs: vec![block] },
        BrepStep { key: hollow, op: BrepOp::Shell { thickness: 2.4, open: Open::Bottom }, inputs: vec![round] },
    ];
    let preview = vec![BrepStep { key: key("preview block"), op: BrepOp::Box { size: [20.0, 20.0, 20.0] }, inputs: Vec::new() }];
    let bits = |m: &makepad_csg_mesh::mesh::TriMesh| (m.vertices.iter().flat_map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).collect::<Vec<u64>>(), m.triangles.clone());
    let alone = bits(&Engine::default().mesh(demo.clone(), 0.05, None).expect("the demo builds"));
    let engine = Arc::new(Engine::default());
    assert!(!engine.built(&[block]), "an engine that built nothing holds no body");
    let (done, answers) = channel();
    let background = {
        let (engine, demo, done) = (engine.clone(), demo.clone(), done.clone());
        std::thread::spawn(move || {
            makepad_cpu_budget::set_thread_class(makepad_cpu_budget::BACKGROUND);
            let mesh = engine.mesh(demo, 0.05, None).expect("the demo builds after the preview");
            done.send(("background", Some(mesh))).unwrap();
        })
    };
    // The preview is sent once the build is under way: its first body is in, the others to come.
    while !engine.built(&[block]) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let preview = {
        let engine = engine.clone();
        std::thread::spawn(move || {
            makepad_cpu_budget::set_thread_class(makepad_cpu_budget::INTERACTIVE);
            engine.mesh(preview, 0.05, None).expect("the preview builds");
            done.send(("preview", None)).unwrap();
        })
    };
    let (first, _) = answers.recv().unwrap();
    assert_eq!(first, "preview", "the preview waited for the background build");
    let (second, mesh) = answers.recv().unwrap();
    assert_eq!(second, "background");
    assert!(bits(&mesh.unwrap()) == alone, "the build that gave way made another body than one built alone");
    assert!(engine.built(&[block, round, hollow]), "the engine holds the demo's bodies");
    preview.join().unwrap();
    background.join().unwrap();
}

/// How far a point is from a mesh's surface.
fn nearest(m: &makepad_csg_mesh::mesh::TriMesh, p: makepad_csg_math::Vec3d) -> f64 {
    m.triangles.iter().map(|t| {
        let (a, b, c) = (m.vertices[t[0] as usize], m.vertices[t[1] as usize], m.vertices[t[2] as usize]);
        // The closest point of the triangle: inside it, else on one of its sides.
        let n = (b - a).cross(c - a);
        let side = |u: makepad_csg_math::Vec3d, v: makepad_csg_math::Vec3d| (v - u).cross(p - u).dot(n) >= 0.0;
        if n.length() > 0.0 && side(a, b) && side(b, c) && side(c, a) {
            return ((p - a).dot(n) / n.length()).abs();
        }
        [(a, b), (b, c), (c, a)].iter().map(|&(u, v)| {
            let e = v - u;
            let s = if e.dot(e) > 0.0 { ((p - u).dot(e) / e.dot(e)).clamp(0.0, 1.0) } else { 0.0 };
            (p - (u + e * s)).length()
        }).fold(f64::MAX, f64::min)
    }).fold(f64::MAX, f64::min)
}

/// A sweep runs through every point of its path at that point's size: a tapered lock from six
/// points, and paths of three and two points (a parabola, a line). A straight sweep from size 1
/// to 0.5 is the cone frustum between them.
#[test]
fn a_sweep_runs_through_its_points_at_their_sizes() {
    let handle = std::thread::Builder::new().stack_size(stack_size()).spawn(|| {
        guard::guarded(stack_size(), &CancelToken::new(), None, || {
            let r = 1.5;
            let round = Sketch::new(vec![Shape::Circle { center: [0.0, 0.0], r }]);
            let lock = vec![[0.0, 0.0, 0.0], [2.0, 6.0, 1.0], [6.0, 11.0, 0.0], [9.0, 17.0, -3.0], [8.0, 23.0, -6.0], [4.0, 27.0, -7.0]];
            let paths: [(&str, Vec<[f64; 3]>, Vec<f64>); 3] = [
                ("lock", lock, vec![1.0, 0.9, 0.75, 0.6, 0.4, 0.2]),
                ("three points", vec![[0.0, 0.0, 0.0], [0.0, 20.0, 5.0], [10.0, 40.0, 10.0]], vec![1.2, 1.0, 0.5]),
                ("two points", vec![[0.0, 0.0, 0.0], [0.0, 20.0, 0.0]], Vec::new()),
            ];
            for (name, path, scale) in paths {
                let mut k = new_kernel();
                let mut b = ops::sweep(&mut k, &round, &path, &scale)?;
                let measured = query::measure(&k, &mut b)?;
                let chord = 0.01;
                let t = tessellate(&k, &mut b, &TessOptions { chord, ..TessOptions::default() })?;
                let (vol, open) = mesh_check(&t.mesh);
                assert_eq!(open, 0, "{name}: open edges");
                assert!((vol - measured.volume).abs() <= measured.area * chord, "{name}: mesh {vol} exact {}", measured.volume);
                // The surface's nearest point to each inner path point is that point's radius away
                // (the end points lie on the flat end caps).
                for (i, p) in path.iter().enumerate().take(path.len() - 1).skip(1) {
                    let want = r * scale.get(i).copied().unwrap_or(1.0);
                    let near = nearest(&t.mesh, makepad_csg_math::Vec3d::new(p[0], p[1], p[2]));
                    assert!((near - want).abs() < 0.02 * r, "{name}: point {i} is {near} from the surface, its radius {want}");
                }
            }
            let mut k = new_kernel();
            let mut cone = ops::sweep(&mut k, &round, &[[0.0, 0.0, 0.0], [0.0, 20.0, 0.0]], &[1.0, 0.5])?;
            let frustum = std::f64::consts::PI * 20.0 / 3.0 * (r * r + r * r * 0.5 + r * r * 0.25);
            let volume = query::measure(&k, &mut cone)?.volume;
            assert!((volume - frustum).abs() < 1e-3 * frustum, "the taper holds {volume}, a frustum {frustum}");
            for (path, scale, why) in [
                (vec![[0.0, 0.0, 0.0]], vec![], "one point"),
                (vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]], vec![], "two points in one place"),
                (vec![[0.0, 0.0, 0.0], [0.0, 9.0, 0.0]], vec![1.0], "one size for two points"),
                (vec![[0.0, 0.0, 0.0], [0.0, 9.0, 0.0]], vec![1.0, 0.0], "a size of zero"),
            ] {
                assert!(matches!(ops::sweep(&mut k, &round, &path, &scale), Err(BrepError::Input(_))), "{why} is not a sweep");
            }
            Ok(())
        })
    }).unwrap();
    handle.join().unwrap().unwrap();
}
