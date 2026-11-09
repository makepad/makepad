//! Compare every serialized Model field, preserving float bit patterns.
//!
//! With `--hull` the fields that depend on how a hull library triangulates a planar face
//! are compared as surfaces (see `hull`), and the values summed over the facets within
//! 1e-12; everything else stays bit for bit.
use makepad_dynamics::model_file::{load_model, visit_model_fields, ModelField};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

#[derive(Clone, Copy, PartialEq)]
enum Value {
    Size(usize),
    F64(u64),
    F32(u32),
    I64(i64),
    I32(i32),
    U8(u8),
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Size(v) => write!(f, "{v}"),
            Self::F64(v) => write!(f, "{:?} (0x{v:016x})", f64::from_bits(v)),
            Self::F32(v) => write!(f, "{:?} (0x{v:08x})", f32::from_bits(v)),
            Self::I64(v) => write!(f, "{v}"),
            Self::I32(v) => write!(f, "{v}"),
            Self::U8(v) => write!(f, "{v}"),
        }
    }
}

fn len(field: ModelField<'_>) -> usize {
    match field {
        ModelField::Size(_) => 1,
        ModelField::F64(v) => v.len(),
        ModelField::F32(v) => v.len(),
        ModelField::I64(v) => v.len(),
        ModelField::I32(v) => v.len(),
        ModelField::U8(v) => v.len(),
    }
}

fn value(field: ModelField<'_>, index: usize) -> Option<Value> {
    match field {
        ModelField::Size(v) => (index == 0).then_some(Value::Size(v)),
        ModelField::F64(v) => v.get(index).map(|v| Value::F64(v.to_bits())),
        ModelField::F32(v) => v.get(index).map(|v| Value::F32(v.to_bits())),
        ModelField::I64(v) => v.get(index).copied().map(Value::I64),
        ModelField::I32(v) => v.get(index).copied().map(Value::I32),
        ModelField::U8(v) => v.get(index).copied().map(Value::U8),
    }
}

fn diff(a: &Path, b: &Path, hull: bool) -> Result<bool, String> {
    let a = load_model(a)?;
    let b = load_model(b)?;
    if hull { return hull::diff(&a, &b); }
    let mut fields = Vec::new();
    visit_model_fields(&a, |name, field| fields.push((name, field)));
    let mut fields = fields.into_iter();
    let mut identical = true;
    visit_model_fields(&b, |name, b| {
        let (a_name, a) = fields.next().expect("shared Model field table");
        assert_eq!(a_name, name, "shared Model field order");
        let mut first = None;
        let mut count = 0;
        for i in 0..len(a).max(len(b)) {
            if value(a, i) != value(b, i) {
                first.get_or_insert(i);
                count += 1;
            }
        }
        if let Some(i) = first {
            // None explicitly denotes an entry missing from the shorter field.
            println!("{name}[{i}]: {:?} != {:?}; {count} differing entries (lengths {} / {})",
                value(a, i), value(b, i), len(a), len(b));
            identical = false;
        }
    });
    if identical { println!("identical"); }
    Ok(identical)
}

fn main() {
    let mut args: Vec<_> = std::env::args_os().skip(1).collect();
    let hull = args.first().map_or(false, |arg| arg == "--hull");
    if hull { args.remove(0); }
    if args.len() != 2 {
        eprintln!("usage: model_diff [--hull] <a.mjb> <b.mjb>");
        std::process::exit(1);
    }
    match diff(Path::new(&args[0]), Path::new(&args[1]), hull) {
        Ok(true) => (),
        Ok(false) => std::process::exit(1),
        Err(error) => { eprintln!("{error}"); std::process::exit(1); }
    }
}

mod hull {
    use super::*;
    use makepad_dynamics::model::Model;

    type Fields<'a> = HashMap<&'static str, ModelField<'a>>;

    fn fields(m: &Model) -> (Vec<&'static str>, Fields<'_>) {
        let mut order = Vec::new();
        let mut map = HashMap::new();
        visit_model_fields(m, |name, field| {
            order.push(name);
            map.insert(name, field);
        });
        (order, map)
    }

    fn i32s<'a>(f: &Fields<'a>, name: &str) -> &'a [i32] {
        match f[name] { ModelField::I32(v) => v, _ => panic!("{name} is not an int field") }
    }

    fn f32s<'a>(f: &Fields<'a>, name: &str) -> &'a [f32] {
        match f[name] { ModelField::F32(v) => v, _ => panic!("{name} is not a float field") }
    }

    fn f64s<'a>(f: &Fields<'a>, name: &str) -> &'a [f64] {
        match f[name] { ModelField::F64(v) => v, _ => panic!("{name} is not a double field") }
    }

    fn size(f: &Fields<'_>, name: &str) -> usize {
        match f[name] { ModelField::Size(v) => v, _ => panic!("{name} is not a size") }
    }

    // Fields checked by `check_*` below instead of entry by entry. The reference compiler
    // triangulates a planar face with diagonals of its own choosing, and sums over the
    // faces in its own order.
    const CUSTOM: [&str; 12] = [
        "mesh_face", "mesh_facenormal", "mesh_normal", "mesh_graph", "mesh_polyvertadr",
        "mesh_polyvertnum", "mesh_polyvert", "mesh_polynormal", "mesh_polymap", "mesh_extrema",
        "bvh_nodeid", "bvh_aabb",
    ];

    // Values that come out of sums over the facets of a mesh (its centre of mass and principal
    // frame, through which the geom's pose, size, box and bound are compiled, and what the
    // constants of the model take from those): within 1e-12 relative, absolute under 1.
    const SUMMED: [&str; 11] = [
        "mesh_pos", "mesh_quat", "geom_pos", "geom_quat", "geom_size", "geom_aabb", "geom_rbound",
        "stat.center", "stat.extent", "stat.meansize", "dof_length",
    ];

    fn close(x: f64, y: f64) -> bool {
        (x - y).abs() <= 1e-12 * x.abs().max(y.abs()).max(1.0)
    }

    struct Mesh {
        verts: Vec<[f32; 3]>,
        faces: Vec<[i32; 3]>,
        normals: Vec<[f32; 3]>,
        facenormals: Vec<[i32; 3]>,
        graph: Vec<i32>,
        polygons: Vec<(Vec<i32>, [f64; 3])>,
        polymap: Vec<Vec<i32>>,
        extrema: Vec<i32>,
        bvhadr: i32,
        bvhnum: usize,
    }

    fn triples<T: Copy>(v: &[T], adr: usize, num: usize) -> Vec<[T; 3]> {
        (adr..adr + num).map(|i| [v[3 * i], v[3 * i + 1], v[3 * i + 2]]).collect()
    }

    fn mesh(f: &Fields<'_>, i: usize) -> Mesh {
        let at = |name: &str| i32s(f, name)[i] as usize;
        let (vertadr, vertnum) = (at("mesh_vertadr"), at("mesh_vertnum"));
        let (faceadr, facenum) = (at("mesh_faceadr"), at("mesh_facenum"));
        let (normaladr, normalnum) = (at("mesh_normaladr"), at("mesh_normalnum"));
        let graphadr = i32s(f, "mesh_graphadr")[i];
        let graph_all = i32s(f, "mesh_graph");
        let graph = if graphadr < 0 { Vec::new() } else {
            let adr = graphadr as usize;
            let (numvert, numface) = (graph_all[adr] as usize, graph_all[adr + 1] as usize);
            graph_all[adr..adr + 2 + 3 * numvert + 6 * numface].to_vec()
        };
        let (polyadr, polynum) = (at("mesh_polyadr"), at("mesh_polynum"));
        let polyvert = i32s(f, "mesh_polyvert");
        let polynormal = f64s(f, "mesh_polynormal");
        let polygons = (polyadr..polyadr + polynum).map(|p| {
            let adr = i32s(f, "mesh_polyvertadr")[p] as usize;
            let num = i32s(f, "mesh_polyvertnum")[p] as usize;
            (polyvert[adr..adr + num].to_vec(), [polynormal[3 * p], polynormal[3 * p + 1], polynormal[3 * p + 2]])
        }).collect();
        let polymap = (vertadr..vertadr + vertnum).map(|v| {
            let adr = i32s(f, "mesh_polymapadr")[v] as usize;
            let num = i32s(f, "mesh_polymapnum")[v] as usize;
            i32s(f, "mesh_polymap")[adr..adr + num].to_vec()
        }).collect();
        Mesh {
            verts: triples(f32s(f, "mesh_vert"), vertadr, vertnum),
            faces: triples(i32s(f, "mesh_face"), faceadr, facenum),
            normals: triples(f32s(f, "mesh_normal"), normaladr, normalnum),
            facenormals: triples(i32s(f, "mesh_facenormal"), faceadr, facenum),
            graph, polygons, polymap,
            extrema: i32s(f, "mesh_extrema")[27 * i..27 * i + 27].to_vec(),
            bvhadr: i32s(f, "mesh_bvhadr")[i],
            bvhnum: at("mesh_bvhnum"),
        }
    }

    // The ring of a polygon or triangle, rotated to start at its smallest point id.
    fn canon(ring: &[i32]) -> Vec<i32> {
        let start = (0..ring.len()).min_by_key(|&i| ring[i]).unwrap();
        (0..ring.len()).map(|i| ring[(start + i) % ring.len()]).collect()
    }

    fn vert(m: &Mesh, i: i32) -> [f64; 3] {
        let v = m.verts[i as usize];
        [v[0] as f64, v[1] as f64, v[2] as f64]
    }

    // Normal (unit) and area of a triangle, from the compiled vertices.
    fn triangle(m: &Mesh, t: [i32; 3]) -> ([f64; 3], f64) {
        let (a, b, c) = (vert(m, t[0]), vert(m, t[1]), vert(m, t[2]));
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        ([n[0] / len, n[1] / len, n[2] / len], 0.5 * len)
    }

    struct Plane {
        normal: [f64; 3],
        ids: BTreeSet<i32>,
        area: f64,
        triangles: usize,
    }

    // The faces grouped by their normal within 1e-9: the surface of the mesh, per plane.
    fn planes(m: &Mesh) -> Vec<Plane> {
        let mut planes: Vec<Plane> = Vec::new();
        for &t in &m.faces {
            let (normal, area) = triangle(m, t);
            let found = planes.iter().position(|p| (0..3).all(|k| (p.normal[k] - normal[k]).abs() < 1e-9));
            let plane = match found {
                Some(i) => &mut planes[i],
                None => {
                    planes.push(Plane { normal, ids: BTreeSet::new(), area: 0.0, triangles: 0 });
                    planes.last_mut().unwrap()
                }
            };
            plane.ids.extend(t);
            plane.area += area;
            plane.triangles += 1;
        }
        planes
    }

    fn check_mesh(i: usize, a: &Mesh, b: &Mesh, out: &mut Vec<String>) {
        let mut fail = |what: String| out.push(format!("mesh {i}: {what}"));
        if a.faces.len() != b.faces.len() { return fail("mesh_face: triangle counts differ".into()); }
        if a.verts.len() != b.verts.len() { return fail("mesh_vert: vertex counts differ".into()); }
        let (pa, pb) = (planes(a), planes(b));

        // A mesh with no coplanar faces (every plane holds one triangle) is compared triangle by
        // triangle, each up to rotation; any other as a surface, per plane.
        let plain = pa.iter().all(|p| p.triangles == 1);
        if plain {
            let set = |m: &Mesh| m.faces.iter().map(|t| canon(t)).collect::<BTreeSet<_>>();
            if set(a) != set(b) { fail("mesh_face: the triangle sets differ".into()); }
        } else {
            if pa.len() != pb.len() { fail(format!("mesh_face: {} planes != {} planes", pa.len(), pb.len())); }
            for p in &pa {
                match pb.iter().find(|q| (0..3).all(|k| (p.normal[k] - q.normal[k]).abs() < 1e-9)) {
                    None => fail(format!("mesh_face: no plane with normal {:?}", p.normal)),
                    Some(q) => {
                        if p.ids != q.ids { fail(format!("mesh_face: plane {:?} holds points {:?} != {:?}", p.normal, p.ids, q.ids)); }
                        if (p.area - q.area).abs() > 1e-12 * p.area.max(q.area) {
                            fail(format!("mesh_face: plane {:?} area {} != {}", p.normal, p.area, q.area));
                        }
                    }
                }
            }
        }

        // mesh_facenormal: the compiler gives a vertex-only mesh one normal per vertex, so a
        // face's normal indices are its point ids, and the matching of faces carries over.
        for (what, m) in [("reference", a), ("ours", b)] {
            if m.facenormals != m.faces { fail(format!("mesh_facenormal differs from mesh_face in {what}")); }
        }

        // mesh_normal: a vertex normal sums the normals of the triangles at the vertex, weighted
        // by area, and drops the faces more than ~37 degrees off the sum, so for planes holding
        // several triangles it depends on the diagonals chosen. Those meshes are compared by
        // count and unit length; the others per vertex, within float rounding.
        if a.normals.len() != b.normals.len() { fail("mesh_normal: counts differ".into()); }
        for (v, (x, y)) in a.normals.iter().zip(&b.normals).enumerate() {
            for (what, n) in [("reference", x), ("ours", y)] {
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if (len - 1.0).abs() > 1e-5 { fail(format!("mesh_normal[{v}] in {what} has length {len}")); }
            }
            if plain && (0..3).any(|k| (x[k] - y[k]).abs() > 1e-5) {
                fail(format!("mesh_normal[{v}]: {x:?} != {y:?}"));
            }
        }

        // mesh_graph: vertex and face counts, the hull vertices as a set, and for a plain mesh
        // each vertex's neighbours.
        let (ga, gb) = (&a.graph, &b.graph);
        if ga.is_empty() != gb.is_empty() { return fail("mesh_graph: one is empty".into()); }
        if !ga.is_empty() {
            if ga[..2] != gb[..2] { fail(format!("mesh_graph: numvert, numface {:?} != {:?}", &ga[..2], &gb[..2])); }
            else {
                let (numvert, numface) = (ga[0] as usize, ga[1] as usize);
                let ids = |g: &Vec<i32>| g[2 + numvert..2 + 2 * numvert].to_vec();
                let sorted = |mut v: Vec<i32>| { v.sort(); v };
                if sorted(ids(ga)) != sorted(ids(gb)) { fail("mesh_graph: vert_globalid sets differ".into()); }
                let neighbours = |g: &Vec<i32>| {
                    let mut map = BTreeMap::new();
                    for v in 0..numvert {
                        let mut set = BTreeSet::new();
                        let mut j = 2 + 2 * numvert + g[2 + v] as usize;
                        while g[j] >= 0 { set.insert(g[2 + numvert + g[j] as usize]); j += 1; }
                        map.insert(g[2 + numvert + v], set);
                    }
                    map
                };
                if plain && neighbours(ga) != neighbours(gb) { fail("mesh_graph: edge_localid sets differ".into()); }
                // the graph's faces are the mesh's faces
                for (what, m) in [("reference", a), ("ours", b)] {
                    let faces = &m.graph[2 + 3 * numvert + 3 * numface..];
                    if faces.len() != 3 * m.faces.len() || faces.iter().zip(m.faces.iter().flatten()).any(|(x, y)| x != y) {
                        fail(format!("mesh_graph faces differ from mesh_face in {what}"));
                    }
                }
            }
        }

        // polygons as a set, each up to rotation; normals within 1e-12; the map per vertex
        // through the polygons
        let polys = |m: &Mesh| m.polygons.iter().map(|(ring, n)| (canon(ring), *n)).collect::<Vec<_>>();
        let (qa, qb) = (polys(a), polys(b));
        let keys = |q: &Vec<(Vec<i32>, [f64; 3])>| q.iter().map(|(r, _)| r.clone()).collect::<BTreeSet<_>>();
        if qa.len() != qb.len() || keys(&qa) != keys(&qb) { fail("mesh_polyvert: the polygon sets differ".into()); }
        else {
            for (ring, n) in &qa {
                let m = &qb.iter().find(|(r, _)| r == ring).unwrap().1;
                if (0..3).any(|k| (n[k] - m[k]).abs() > 1e-12) { fail(format!("mesh_polynormal {ring:?}: {n:?} != {m:?}")); }
            }
            let through = |m: &Mesh, q: &Vec<(Vec<i32>, [f64; 3])>| {
                m.polymap.iter().map(|polys| polys.iter().map(|&p| q[p as usize].0.clone()).collect::<BTreeSet<_>>()).collect::<Vec<_>>()
            };
            if through(a, &qa) != through(b, &qb) { fail("mesh_polymap: the polygons at some vertex differ".into()); }
        }

        // mesh_extrema: the vertex farthest along each of the 27 directions of a unit cube's
        // features, through the graph; ties between vertices pick by hull order, so compare the
        // extremal value, not the id.
        if !ga.is_empty() {
            let value = |m: &Mesh, k: usize| {
                let numvert = m.graph[0] as usize;
                let id = m.graph[2 + numvert + m.extrema[k] as usize] as usize;
                let (cx, cy, cz) = ((k / 9) as i32 - 1, (k / 3 % 3) as i32 - 1, (k % 3) as i32 - 1);
                let v = m.verts[id];
                v[0] * cx as f32 + v[1] * cy as f32 + v[2] * cz as f32
            };
            for k in 0..27 {
                if value(a, k) != value(b, k) { fail(format!("mesh_extrema[{k}]: extremal values {} != {}", value(a, k), value(b, k))); }
            }
        }
    }

    pub fn diff(a: &Model, b: &Model) -> Result<bool, String> {
        let (order, fa) = fields(a);
        let (_, fb) = fields(b);
        let mut out: Vec<String> = Vec::new();

        // everything but the facet-dependent fields, entry by entry
        for name in &order {
            if CUSTOM.contains(name) { continue; }
            let (x, y) = (fa[name], fb[name]);
            let tolerant = SUMMED.contains(name);
            let mut first = None;
            let mut count = 0;
            for i in 0..len(x).max(len(y)) {
                let same = match (x, y) {
                    (ModelField::F64(p), ModelField::F64(q)) if tolerant => match (p.get(i), q.get(i)) {
                        (Some(&u), Some(&v)) => close(u, v),
                        (None, None) => true,
                        _ => false,
                    },
                    _ => value(x, i) == value(y, i),
                };
                if !same {
                    first.get_or_insert(i);
                    count += 1;
                }
            }
            if let Some(i) = first {
                out.push(format!("{name}[{i}]: {:?} != {:?}; {count} differing entries (lengths {} / {})",
                    value(x, i), value(y, i), len(x), len(y)));
            }
        }

        // the facet-dependent fields as surfaces; only when the sizes agree, else they differ above
        let sizes = ["nmesh", "nmeshvert", "nmeshnormal", "nmeshface", "nmeshgraph", "nmeshpoly", "nmeshpolyvert", "nmeshpolymap", "nbvhstatic"];
        if sizes.iter().all(|n| size(&fa, n) == size(&fb, n)) {
            let nmesh = size(&fa, "nmesh");
            let meshes: Vec<(Mesh, Mesh)> = (0..nmesh).map(|i| (mesh(&fa, i), mesh(&fb, i))).collect();
            for (i, (x, y)) in meshes.iter().enumerate() { check_mesh(i, x, y, &mut out); }

            // the bounding volume hierarchies: the shape of each (bvh_child, bvh_depth, compared
            // above) is a function of the node count; a body's boxes come from its geoms, whose poses
            // are summed over facets; a mesh's tree holds face ids that follow the face order (a
            // permutation of the same set) and boxes of faces, so its root box is compared
            let (id_a, id_b) = (i32s(&fa, "bvh_nodeid"), i32s(&fb, "bvh_nodeid"));
            let (box_a, box_b) = (f64s(&fa, "bvh_aabb"), f64s(&fb, "bvh_aabb"));
            let mut in_mesh = vec![false; id_a.len()];
            for (i, (x, y)) in meshes.iter().enumerate() {
                if x.bvhadr != y.bvhadr || x.bvhnum != y.bvhnum { out.push(format!("mesh {i}: bvh address or count differs")); continue; }
                if x.bvhnum == 0 { continue; }
                let adr = x.bvhadr as usize;
                let range = adr..adr + x.bvhnum;
                for j in range.clone() { in_mesh[j] = true; }
                let sorted = |v: &[i32]| { let mut v = v.to_vec(); v.sort(); v };
                if sorted(&id_a[range.clone()]) != sorted(&id_b[range.clone()]) { out.push(format!("mesh {i}: bvh_nodeid sets differ")); }
                if (0..6).any(|k| !close(box_a[6 * adr + k], box_b[6 * adr + k])) {
                    out.push(format!("mesh {i}: root bvh_aabb {:?} != {:?}", &box_a[6 * adr..6 * adr + 6], &box_b[6 * adr..6 * adr + 6]));
                }
            }
            for j in 0..id_a.len() {
                if in_mesh[j] { continue; }
                if id_a[j] != id_b[j] { out.push(format!("bvh_nodeid[{j}]: {} != {}", id_a[j], id_b[j])); }
                if (0..6).any(|k| !close(box_a[6 * j + k], box_b[6 * j + k])) {
                    out.push(format!("bvh_aabb[{j}]: {:?} != {:?}", &box_a[6 * j..6 * j + 6], &box_b[6 * j..6 * j + 6]));
                }
            }
        }
        for line in &out { println!("{line}"); }
        if out.is_empty() { println!("identical"); }
        Ok(out.is_empty())
    }
}
