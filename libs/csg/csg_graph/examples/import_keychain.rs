//! A keychain on an imported mesh: the figure joined with a ring and the
//! ring's hole cut (sharp booleans), and the same as one blended distance
//! field. Reports open edges and times for each.
//!
//! `cargo run --release -p makepad-csg-graph --example import_keychain -- <mesh.ply> [size_mm] [cell_mm] [blend_mm]`

use makepad_csg_graph::eval::Evaluator;
use makepad_csg_graph::{BoolMode, Graph, Op, SdfNode, SdfOp, SdfShape};
use makepad_csg_math::Vec3d;
use makepad_csg_mesh::mesh::TriMesh;
use makepad_csg_sdf::mesh_field::{MeshData, MeshField};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// A binary little-endian PLY: float x y z first, any other scalar vertex
/// properties skipped, faces as `list uchar uint`.
fn read_ply(bytes: &[u8]) -> MeshData {
    let end = bytes.windows(11).position(|w| w == b"end_header\n").expect("a PLY header") + 11;
    let header = std::str::from_utf8(&bytes[..end]).unwrap();
    let (mut verts, mut faces, mut stride, mut in_vertex) = (0usize, 0usize, 0usize, false);
    for line in header.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["element", "vertex", n] => (verts, in_vertex) = (n.parse().unwrap(), true),
            ["element", "face", n] => (faces, in_vertex) = (n.parse().unwrap(), false),
            ["property", ty, _] if in_vertex => stride += match *ty { "float" | "int" | "uint" => 4, "double" => 8, "uchar" | "char" => 1, "short" | "ushort" => 2, t => panic!("vertex property {t}") },
            _ => {}
        }
    }
    let f = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let positions = (0..verts).map(|i| [f(end + i * stride), f(end + i * stride + 4), f(end + i * stride + 8)]).collect();
    let mut at = end + verts * stride;
    let mut triangles = Vec::with_capacity(faces);
    for _ in 0..faces {
        let n = bytes[at] as usize;
        let idx: Vec<u32> = (0..n).map(|k| u32::from_le_bytes(bytes[at + 1 + 4 * k..at + 5 + 4 * k].try_into().unwrap())).collect();
        at += 1 + 4 * n;
        for k in 1..n.saturating_sub(1) {
            triangles.push([idx[0], idx[k], idx[k + 1]]);
        }
    }
    MeshData { positions, triangles }
}

/// Edges not shared by exactly two triangles (after welding), and of those how many are shared by more.
fn open_edges(m: &TriMesh) -> (usize, usize) {
    let mut m = m.clone();
    m.weld_vertices(1e-7);
    let mut count: HashMap<(u32, u32), i32> = HashMap::new();
    for t in &m.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    (count.values().filter(|c| **c != 2).count(), count.values().filter(|c| **c > 2).count())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("a .ply");
    let size: f64 = args.get(2).map_or(40.0, |s| s.parse().unwrap());
    let cell: f64 = args.get(3).map_or(0.4, |s| s.parse().unwrap());
    let blend: f64 = args.get(4).map_or(1.0, |s| s.parse().unwrap());
    let mut data = read_ply(&std::fs::read(path).unwrap());
    // Scaled so its largest side is `size` mm, standing on the plate, centred.
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &data.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let ext = (0..3).map(|k| hi[k] - lo[k]).fold(0.0f32, f32::max);
    let s = size as f32 / ext;
    for p in &mut data.positions {
        *p = [(p[0] - (lo[0] + hi[0]) * 0.5) * s, (p[1] - lo[1]) * s, (p[2] - (lo[2] + hi[2]) * 0.5) * s];
    }
    let height = (hi[1] - lo[1]) * s;
    let mut tri = TriMesh::new();
    for p in &data.positions {
        tri.add_vertex(Vec3d::new(p[0] as f64, p[1] as f64, p[2] as f64));
    }
    for t in &data.triangles {
        tri.add_triangle(t[0], t[1], t[2]);
    }
    let t0 = Instant::now();
    let field = Arc::new(MeshField::new(data));
    println!("mesh: {} verts, {} tris, open {:?}, watertight {}, {:.1} mm high, field prepared in {} ms", tri.vertices.len(), tri.triangles.len(), open_edges(&tri), field.watertight, height, t0.elapsed().as_millis());
    // The ring: a torus standing up (in XY) on top of the figure, half sunk in.
    let (ring_r, tube) = (size * 0.12, size * 0.035);
    let ring_at = [0.0, height as f64 + ring_r * 0.5, 0.0];

    // Sharp: (owl + ring) - hole.
    let mut g = Graph::new();
    let owl = g.push(Op::Mesh { mesh: Arc::new(tri) }).unwrap();
    let torus = g.push(Op::Torus { radius: ring_r, tube, segments: 64, sides: 24 }).unwrap();
    let ring = g.push(Op::Transform { source: torus, at: ring_at, rotate: [90.0, 0.0, 0.0], scale: [1.0; 3] }).unwrap();
    let cyl = g.push(Op::Cylinder { radius: ring_r - tube * 1.5, top_radius: ring_r - tube * 1.5, height: tube * 4.0, segments: 48 }).unwrap();
    let hole = g.push(Op::Transform { source: cyl, at: ring_at, rotate: [90.0, 0.0, 0.0], scale: [1.0; 3] }).unwrap();
    let joined = g.push(Op::Boolean { mode: BoolMode::Union, a: owl, b: vec![ring] }).unwrap();
    let cut = g.push(Op::Boolean { mode: BoolMode::Difference, a: joined, b: vec![hole] }).unwrap();
    let mut e = Evaluator::new();
    if std::env::var("KEYCHAIN_DIAG").is_ok() {
        use makepad_csg_boolean::boolean::{mesh_boolean_with, BoolOp, FinishParams};
        let owl_m = e.mesh(&g, owl, None).unwrap();
        let ring_m = e.mesh(&g, ring, None).unwrap();
        for (label, tol) in [("tol 5e-5", 5.5e-5), ("tol 1e-7", 1e-7), ("tol 1e-3", 1e-3)] {
            let t0 = Instant::now();
            let m = mesh_boolean_with(&owl_m, &ring_m, BoolOp::Union, FinishParams { tolerance: tol });
            println!("diag union {label}: {} tris, open {:?}, {} ms", m.triangles.len(), open_edges(&m), t0.elapsed().as_millis());
            if label == "tol 1e-3" {
                // Where the open edges are: length and distance to the ring's surface.
                let mut w = m.clone();
                w.weld_vertices(1e-7);
                let mut count: HashMap<(u32, u32), i32> = HashMap::new();
                for t in &w.triangles {
                    for k in 0..3 {
                        let (a, b) = (t[k], t[(k + 1) % 3]);
                        *count.entry((a.min(b), a.max(b))).or_default() += 1;
                    }
                }
                let torus_d = |p: Vec3d| {
                    let q = Vec3d::new(p.x - ring_at[0], p.y - ring_at[1], p.z - ring_at[2]);
                    // The torus turned 90 deg about X: its axis is Z.
                    let r = (q.x * q.x + q.y * q.y).sqrt() - ring_r;
                    (r * r + q.z * q.z).sqrt() - tube
                };
                let mut shown = 0;
                for ((a, b), c) in &count {
                    if *c == 2 { continue; }
                    let (pa, pb) = (w.vertices[*a as usize], w.vertices[*b as usize]);
                    if shown < 12 {
                        println!("  open edge x{c}: len {:.2e}, torus dist {:.2e} / {:.2e}", (pa - pb).length(), torus_d(pa), torus_d(pb));
                    }
                    shown += 1;
                }
            }
        }
    }
    let t0 = Instant::now();
    match e.mesh(&g, joined, None) {
        Ok(m) => println!("sharp union: {} tris, open {:?}, {} ms", m.triangles.len(), open_edges(&m), t0.elapsed().as_millis()),
        Err(f) => println!("sharp union failed: {}", f.message),
    }
    let t0 = Instant::now();
    match e.mesh(&g, cut, None) {
        Ok(m) => println!("sharp union - hole: {} tris, open {:?}, {} ms", m.triangles.len(), open_edges(&m), t0.elapsed().as_millis()),
        Err(f) => println!("sharp cut failed: {}", f.message),
    }

    // Blended: one field, the ring blended on and the hole carved.
    let place = |of: SdfNode| SdfNode::Transform { at: ring_at, rotate: [90.0, 0.0, 0.0], scale: 1.0, of: Box::new(of) };
    let root = SdfNode::Combine {
        op: SdfOp::Difference,
        blend: blend * 0.5,
        of: vec![
            SdfNode::Combine { op: SdfOp::Union, blend, of: vec![SdfNode::Mesh(field.clone()), place(SdfNode::Torus { radius: ring_r, tube })] },
            place(SdfNode::Cylinder { radius: ring_r - tube * 1.5, top_radius: ring_r - tube * 1.5, height: tube * 4.0 }),
        ],
    };
    let b = makepad_csg_sdf::node_bounds(&root).unwrap();
    let shape = SdfShape { root, min: b.0, max: b.1, materials: Vec::new() };
    let mut g = Graph::new();
    let f = g.push(Op::Field { shape, cell: Some(cell) }).unwrap();
    let t0 = Instant::now();
    match Evaluator::new().mesh(&g, f, None) {
        Ok(m) => println!("blend cell {cell}: {} tris, open {:?}, {} ms", m.triangles.len(), open_edges(&m), t0.elapsed().as_millis()),
        Err(f) => println!("blend failed: {}", f.message),
    }
}
