// Deterministic robustness fuzzer for the CSG boolean stack.
//
// Two modes:
//
//   fuzz --case <category> --seed <n>
//       Runs ONE case in-process and prints a single JSON-ish result line.
//
//   fuzz --driver [--cases N] [--cats a,b,c] [--jobs J]
//       Spawns itself once per (category, seed), enforces a wall-clock limit
//       (kill on timeout -> HANG, non-zero exit -> CRASH) and prints a
//       per-category failure table.
//
//   fuzz --repro <category> <seed>
//       Prints a self-contained Rust snippet reproducing the case.
//
// Everything is std-only with a local xorshift64* PRNG, so a given seed always
// produces the same geometry.
//
// The volume oracle is a Monte-Carlo estimate that uses an INDEPENDENT
// +X-ray parity point-in-mesh test against the operand meshes (not the
// library's own BVH). Sampling the operand meshes rather than the analytic
// primitives is deliberate: an 8-segment UV sphere has ~15% less volume than
// the ideal sphere, which would swamp a 2% volume tolerance and drown real
// defects in tessellation noise. Samples that land within 1e-9 of a triangle
// edge/plane are discarded so the parity count is never ambiguous.

use makepad_csg::{dvec3, Solid, TriMesh, Vec3d};
use std::collections::HashMap;
use std::env;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- RNG

/// xorshift64* — small, deterministic, seedable.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        // Splitmix-style spread so adjacent seeds are not correlated.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Rng(z | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

// ---------------------------------------------------------------- specs

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Union,
    Difference,
    Intersection,
}

impl Op {
    fn name(self) -> &'static str {
        match self {
            Op::Union => "union",
            Op::Difference => "difference",
            Op::Intersection => "intersection",
        }
    }
    fn apply(self, a: &Solid, b: &Solid) -> Solid {
        match self {
            Op::Union => a.union(b),
            Op::Difference => a.difference(b),
            Op::Intersection => a.intersection(b),
        }
    }
    fn inside(self, ia: bool, ib: bool) -> bool {
        match self {
            Op::Union => ia || ib,
            Op::Difference => ia && !ib,
            Op::Intersection => ia && ib,
        }
    }
    fn pick(rng: &mut Rng) -> Op {
        match rng.below(3) {
            0 => Op::Union,
            1 => Op::Difference,
            _ => Op::Intersection,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Prim {
    Cube { x: f64, y: f64, z: f64, center: bool },
    Sphere { r: f64, seg: u32, rings: u32 },
    Cylinder { r: f64, h: f64, seg: u32, center: bool },
    Cone { r: f64, h: f64, seg: u32, center: bool },
    Torus { rmaj: f64, rmin: f64, maj: u32, min: u32 },
}

impl Prim {
    fn solid(self) -> Solid {
        match self {
            Prim::Cube { x, y, z, center } => Solid::cube(x, y, z, center),
            Prim::Sphere { r, seg, rings } => Solid::sphere(r, seg, rings),
            Prim::Cylinder { r, h, seg, center } => Solid::cylinder(r, h, seg, center),
            Prim::Cone { r, h, seg, center } => Solid::cone(r, h, seg, center),
            Prim::Torus { rmaj, rmin, maj, min } => Solid::torus(rmaj, rmin, maj, min),
        }
    }
    fn code(self) -> String {
        match self {
            Prim::Cube { x, y, z, center } => {
                format!("Solid::cube({:.6}, {:.6}, {:.6}, {})", x, y, z, center)
            }
            Prim::Sphere { r, seg, rings } => {
                format!("Solid::sphere({:.6}, {}, {})", r, seg, rings)
            }
            Prim::Cylinder { r, h, seg, center } => {
                format!("Solid::cylinder({:.6}, {:.6}, {}, {})", r, h, seg, center)
            }
            Prim::Cone { r, h, seg, center } => {
                format!("Solid::cone({:.6}, {:.6}, {}, {})", r, h, seg, center)
            }
            Prim::Torus { rmaj, rmin, maj, min } => {
                format!("Solid::torus({:.6}, {:.6}, {}, {})", rmaj, rmin, maj, min)
            }
        }
    }
    /// Half-extents of the local-frame axis-aligned bounding box, and its centre.
    fn local_box(self) -> (Vec3d, Vec3d) {
        match self {
            Prim::Cube { x, y, z, center } => {
                let h = dvec3(x * 0.5, y * 0.5, z * 0.5);
                let c = if center { dvec3(0.0, 0.0, 0.0) } else { h };
                (c, h)
            }
            Prim::Sphere { r, .. } => (dvec3(0.0, 0.0, 0.0), dvec3(r, r, r)),
            Prim::Cylinder { r, h, center, .. } | Prim::Cone { r, h, center, .. } => {
                let hh = h * 0.5;
                let c = if center { dvec3(0.0, 0.0, 0.0) } else { dvec3(0.0, hh, 0.0) };
                (c, dvec3(r, hh, r))
            }
            Prim::Torus { rmaj, rmin, .. } => {
                (dvec3(0.0, 0.0, 0.0), dvec3(rmaj + rmin, rmin, rmaj + rmin))
            }
        }
    }
    fn random(rng: &mut Rng, scale: f64) -> Prim {
        match rng.below(5) {
            0 => Prim::Cube {
                x: rng.range(0.5, 2.0) * scale,
                y: rng.range(0.5, 2.0) * scale,
                z: rng.range(0.5, 2.0) * scale,
                center: true,
            },
            1 => Prim::Sphere {
                r: rng.range(0.4, 1.2) * scale,
                seg: 8 + rng.below(57) as u32,
                rings: 4 + rng.below(29) as u32,
            },
            2 => Prim::Cylinder {
                r: rng.range(0.3, 1.0) * scale,
                h: rng.range(0.5, 2.0) * scale,
                seg: 6 + rng.below(43) as u32,
                center: true,
            },
            3 => Prim::Cone {
                r: rng.range(0.3, 1.0) * scale,
                h: rng.range(0.5, 2.0) * scale,
                seg: 6 + rng.below(43) as u32,
                center: true,
            },
            _ => {
                let rmaj = rng.range(0.6, 1.2) * scale;
                Prim::Torus {
                    rmaj,
                    rmin: rmaj * rng.range(0.2, 0.45),
                    maj: 8 + rng.below(25) as u32,
                    min: 6 + rng.below(15) as u32,
                }
            }
        }
    }
}

/// Scale -> rotate-about-axis -> translate, applied in that order.
#[derive(Clone, Copy, Debug)]
struct Xform {
    s: Vec3d,
    axis: Vec3d,
    deg: f64,
    t: Vec3d,
}

impl Xform {
    fn identity() -> Xform {
        Xform {
            s: dvec3(1.0, 1.0, 1.0),
            axis: dvec3(0.0, 1.0, 0.0),
            deg: 0.0,
            t: dvec3(0.0, 0.0, 0.0),
        }
    }
    fn translated(t: Vec3d) -> Xform {
        Xform { t, ..Xform::identity() }
    }
    fn apply(&self, s: &Solid) -> Solid {
        let mut r = s.clone();
        if self.s != dvec3(1.0, 1.0, 1.0) {
            r = r.scale(self.s.x, self.s.y, self.s.z);
        }
        if self.deg != 0.0 {
            r = r.rotate(self.axis, self.deg);
        }
        if self.t != dvec3(0.0, 0.0, 0.0) {
            r = r.translate(self.t.x, self.t.y, self.t.z);
        }
        r
    }
    fn code(&self) -> String {
        let mut s = String::new();
        if self.s != dvec3(1.0, 1.0, 1.0) {
            s.push_str(&format!(
                "\n        .scale({:.6}, {:.6}, {:.6})",
                self.s.x, self.s.y, self.s.z
            ));
        }
        if self.deg != 0.0 {
            s.push_str(&format!(
                "\n        .rotate(dvec3({:.6}, {:.6}, {:.6}), {:.6})",
                self.axis.x, self.axis.y, self.axis.z, self.deg
            ));
        }
        if self.t != dvec3(0.0, 0.0, 0.0) {
            s.push_str(&format!(
                "\n        .translate({:.10}, {:.10}, {:.10})",
                self.t.x, self.t.y, self.t.z
            ));
        }
        s
    }
    /// World-space AABB of the operand (conservative: transformed local AABB corners).
    fn world_box(&self, p: Prim) -> (Vec3d, Vec3d) {
        let (c, h) = p.local_box();
        let probe = Solid::cube(2.0 * h.x.max(1e-9), 2.0 * h.y.max(1e-9), 2.0 * h.z.max(1e-9), true)
            .translate(c.x, c.y, c.z);
        let bb = self.apply(&probe).bounding_box();
        (bb.min, bb.max)
    }
    fn random(rng: &mut Rng) -> Xform {
        Xform {
            s: dvec3(rng.range(0.7, 1.4), rng.range(0.7, 1.4), rng.range(0.7, 1.4)),
            axis: dvec3(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0))
                + dvec3(0.0, 1e-3, 0.0),
            deg: rng.range(0.0, 360.0),
            t: dvec3(0.0, 0.0, 0.0),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Operand {
    prim: Prim,
    xf: Xform,
}

impl Operand {
    fn plain(prim: Prim) -> Operand {
        Operand { prim, xf: Xform::identity() }
    }
    fn solid(&self) -> Solid {
        self.xf.apply(&self.prim.solid())
    }
    fn code(&self) -> String {
        format!("{}{}", self.prim.code(), self.xf.code())
    }
}

#[derive(Clone, Debug)]
enum Recipe {
    /// One operand pair evaluated under one or more ops.
    Pair { a: Operand, b: Operand, ops: Vec<Op>, note: String },
    /// Start from `base`, apply `steps` booleans in sequence.
    Cascade { base: Operand, steps: Vec<(Operand, Op)> },
    /// A = merge(a1, a2) (a deliberately self-intersecting, non-manifold operand).
    Merge { a1: Operand, a2: Operand, b: Operand, op: Op },
}

impl Recipe {
    fn code(&self) -> String {
        match self {
            Recipe::Pair { a, b, ops, note } => {
                let mut s = format!("// {}\nlet a = {};\nlet b = {};\n", note, a.code(), b.code());
                for op in ops {
                    s.push_str(&format!("let r_{} = a.{}(&b);\n", op.name(), op.name()));
                }
                s
            }
            Recipe::Cascade { base, steps } => {
                let mut s = format!("let mut r = {};\n", base.code());
                for (i, (o, op)) in steps.iter().enumerate() {
                    s.push_str(&format!(
                        "// step {}\nr = r.{}(&({}));\n",
                        i + 1,
                        op.name(),
                        o.code()
                    ));
                }
                s
            }
            Recipe::Merge { a1, a2, b, op } => format!(
                "// self-intersecting operand built with merge() (no boolean)\n\
                 let a = ({}).merge(&({}));\nlet b = {};\nlet r = a.{}(&b);\n",
                a1.code(),
                a2.code(),
                b.code(),
                op.name()
            ),
        }
    }
}

// ---------------------------------------------------------------- oracle

/// Independent point-in-mesh oracle: +X-ray parity with a (y,z) bucket grid.
struct Oracle {
    tris: Vec<[Vec3d; 3]>,
    cells: HashMap<(i32, i32), Vec<u32>>,
    inv_cell: f64,
    empty: bool,
}

impl Oracle {
    fn build(mesh: &TriMesh) -> Oracle {
        let tris: Vec<[Vec3d; 3]> = (0..mesh.triangle_count())
            .map(|i| {
                let (a, b, c) = mesh.triangle_vertices(i);
                [a, b, c]
            })
            .collect();
        if tris.is_empty() {
            return Oracle {
                tris,
                cells: HashMap::new(),
                inv_cell: 1.0,
                empty: true,
            };
        }
        let bb = mesh.bounding_box();
        let ext = ((bb.max.y - bb.min.y).max(bb.max.z - bb.min.z)).max(1e-12);
        let n = ((tris.len() as f64).sqrt() * 0.5).clamp(4.0, 64.0);
        let cell = ext / n;
        let inv_cell = 1.0 / cell;
        let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (ti, t) in tris.iter().enumerate() {
            let y0 = t[0].y.min(t[1].y).min(t[2].y);
            let y1 = t[0].y.max(t[1].y).max(t[2].y);
            let z0 = t[0].z.min(t[1].z).min(t[2].z);
            let z1 = t[0].z.max(t[1].z).max(t[2].z);
            let (cy0, cy1) = ((y0 * inv_cell).floor() as i64, (y1 * inv_cell).floor() as i64);
            let (cz0, cz1) = ((z0 * inv_cell).floor() as i64, (z1 * inv_cell).floor() as i64);
            // Guard against pathological spans (huge meshes / tiny cells).
            if (cy1 - cy0 + 1) * (cz1 - cz0 + 1) > 4096 {
                cells.entry((i32::MIN, i32::MIN)).or_default().push(ti as u32);
                continue;
            }
            for cy in cy0..=cy1 {
                for cz in cz0..=cz1 {
                    cells
                        .entry((cy as i32, cz as i32))
                        .or_default()
                        .push(ti as u32);
                }
            }
        }
        Oracle { tris, cells, inv_cell, empty: false }
    }

    /// Returns Some(inside) or None if the sample is too close to a face/edge
    /// for the parity count to be trustworthy.
    fn inside(&self, p: Vec3d) -> Option<bool> {
        if self.empty {
            return Some(false);
        }
        let cy = (p.y * self.inv_cell).floor() as i32;
        let cz = (p.z * self.inv_cell).floor() as i32;
        let mut crossings = 0u32;
        let scan = |idx: &Vec<u32>, crossings: &mut u32| -> bool {
            for &ti in idx {
                let t = &self.tris[ti as usize];
                match ray_x_tri(p, t) {
                    Hit::Miss => {}
                    Hit::Cross => *crossings += 1,
                    Hit::Ambiguous => return false,
                }
            }
            true
        };
        if let Some(idx) = self.cells.get(&(cy, cz)) {
            if !scan(idx, &mut crossings) {
                return None;
            }
        }
        if let Some(idx) = self.cells.get(&(i32::MIN, i32::MIN)) {
            if !scan(idx, &mut crossings) {
                return None;
            }
        }
        Some(crossings % 2 == 1)
    }
}

enum Hit {
    Miss,
    Cross,
    Ambiguous,
}

/// Möller-Trumbore specialised to direction (1,0,0).
fn ray_x_tri(p: Vec3d, t: &[Vec3d; 3]) -> Hit {
    const EPS: f64 = 1e-9;
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    // h = dir x e2 = (0, -e2.z, e2.y)
    let det = -e1.y * e2.z + e1.z * e2.y;
    let scale = (e1.length() * e2.length()).max(1e-30);
    if det.abs() < EPS * scale {
        // Ray parallel to the triangle plane. Only ambiguous if the ray line
        // actually grazes the triangle; otherwise a clean miss.
        let ny = e1.z * e2.x - e1.x * e2.z;
        let nz = e1.x * e2.y - e1.y * e2.x;
        let d = (p - t[0]).y * ny + (p - t[0]).z * nz;
        let nlen = (ny * ny + nz * nz).sqrt().max(1e-30);
        if (d / nlen).abs() < EPS * scale.sqrt().max(1.0) {
            return Hit::Ambiguous;
        }
        return Hit::Miss;
    }
    let f = 1.0 / det;
    let s = p - t[0];
    let u = f * (-s.y * e2.z + s.z * e2.y);
    let v = f * (s.y * e1.z - s.z * e1.y);
    if u < -EPS || u > 1.0 + EPS || v < -EPS || u + v > 1.0 + EPS {
        return Hit::Miss;
    }
    if u < EPS || v < EPS || u + v > 1.0 - EPS {
        return Hit::Ambiguous; // grazing an edge/vertex: parity unreliable
    }
    // q = s x e1 ; t_hit = f * (e2 . q)
    let qx = s.y * e1.z - s.z * e1.y;
    let qy = s.z * e1.x - s.x * e1.z;
    let qz = s.x * e1.y - s.y * e1.x;
    let th = f * (e2.x * qx + e2.y * qy + e2.z * qz);
    if th.abs() < EPS * scale {
        return Hit::Ambiguous; // sample sits on the surface
    }
    if th > 0.0 {
        Hit::Cross
    } else {
        Hit::Miss
    }
}

/// Monte-Carlo volume of `pred(inside_a, inside_b)` over the union bbox.
/// Returns (volume, sigma, valid_samples, discarded_samples).
fn mc_volume(
    oa: &Oracle,
    ob: &Oracle,
    bmin: Vec3d,
    bmax: Vec3d,
    op: Op,
    seed: u64,
) -> (f64, f64, u32, u32) {
    const N: usize = 27; // 27^3 = 19683 stratified samples
    let mut rng = Rng::new(seed ^ 0xA5A5_5A5A_1234_9876);
    let d = bmax - bmin;
    let bbox_vol = d.x * d.y * d.z;
    let mut hits = 0u32;
    let mut valid = 0u32;
    let mut skipped = 0u32;
    for i in 0..N {
        for j in 0..N {
            for k in 0..N {
                let p = dvec3(
                    bmin.x + d.x * (i as f64 + rng.unit()) / N as f64,
                    bmin.y + d.y * (j as f64 + rng.unit()) / N as f64,
                    bmin.z + d.z * (k as f64 + rng.unit()) / N as f64,
                );
                let ia = match oa.inside(p) {
                    Some(v) => v,
                    None => {
                        skipped += 1;
                        continue;
                    }
                };
                let ib = match ob.inside(p) {
                    Some(v) => v,
                    None => {
                        skipped += 1;
                        continue;
                    }
                };
                valid += 1;
                if op.inside(ia, ib) {
                    hits += 1;
                }
            }
        }
    }
    if valid == 0 {
        return (0.0, bbox_vol, 0, skipped);
    }
    let frac = hits as f64 / valid as f64;
    let vol = frac * bbox_vol;
    let sigma = bbox_vol * (frac * (1.0 - frac) / valid as f64).sqrt();
    (vol, sigma, valid, skipped)
}

// ---------------------------------------------------------------- topology

struct Topo {
    euler: i64,
    components: usize,
}

fn topology(mesh: &TriMesh) -> Topo {
    let mut used: HashMap<u32, u32> = HashMap::new();
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for &[a, b, c] in &mesh.triangles {
        for v in [a, b, c] {
            let n = used.len() as u32;
            used.entry(v).or_insert(n);
        }
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let key = if u < v { (u, v) } else { (v, u) };
            *edges.entry(key).or_insert(0) += 1;
        }
    }
    // Connected components over triangles sharing an undirected edge.
    let mut edge_tris: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (ti, &[a, b, c]) in mesh.triangles.iter().enumerate() {
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let key = if u < v { (u, v) } else { (v, u) };
            edge_tris.entry(key).or_default().push(ti);
        }
    }
    let n = mesh.triangles.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut Vec<usize>, mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for group in edge_tris.values() {
        for w in group.windows(2) {
            let (a, b) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
            if a != b {
                parent[a] = b;
            }
        }
    }
    let mut roots: HashMap<usize, ()> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        roots.insert(r, ());
    }
    let v = used.len();
    let e = edges.len();
    let f = mesh.triangles.len();
    Topo {
        euler: v as i64 - e as i64 + f as i64,
        components: roots.len(),
    }
}

// ---------------------------------------------------------------- checks

#[derive(Clone)]
struct CheckOut {
    label: String,
    flags: Vec<&'static str>,
    tris: usize,
    vol: f64,
    oracle: f64,
    sigma: f64,
    tol: f64,
    euler: i64,
    components: usize,
    boundary_edges: usize,
    non_manifold_edges: usize,
    degenerate: usize,
    skipped: u32,
}

fn check_bool(label: &str, a: &Solid, b: &Solid, op: Op, seed: u64) -> CheckOut {
    let res = op.apply(a, b);
    check_result(label, a, b, op, &res, seed)
}

fn check_result(
    label: &str,
    a: &Solid,
    b: &Solid,
    op: Op,
    res: &Solid,
    seed: u64,
) -> CheckOut {
    let ba = a.bounding_box();
    let bb = b.bounding_box();
    let mut bmin = ba.min.min(bb.min);
    let mut bmax = ba.max.max(bb.max);
    let pad = (bmax - bmin).length() * 0.01 + 1e-12;
    bmin = bmin - dvec3(pad, pad, pad);
    bmax = bmax + dvec3(pad, pad, pad);

    let oa = Oracle::build(a.mesh());
    let ob = Oracle::build(b.mesh());
    let (oracle, sigma, _valid, skipped) = mc_volume(&oa, &ob, bmin, bmax, op, seed);

    let rep = res.validate();
    let topo = topology(res.mesh());
    let vol = res.volume();
    let vol_a = a.volume().abs();
    let vol_b = b.volume().abs();
    let bigger = vol_a.max(vol_b).max(1e-300);
    let tol = (0.02 * bigger).max(3.0 * sigma);

    let mut flags: Vec<&'static str> = Vec::new();
    let nan = res.mesh().vertices.iter().any(|v| !v.is_finite());
    if nan {
        flags.push("NAN");
    }
    if res.triangle_count() == 0 && oracle > tol {
        flags.push("EMPTY_RESULT");
    }
    if res.triangle_count() > 0 {
        if rep.boundary_edges > 0 {
            flags.push("NOT_CLOSED");
        }
        if rep.non_manifold_edges > 0 {
            flags.push("NON_MANIFOLD");
        }
        if !rep.is_consistently_oriented || vol < -tol {
            flags.push("BAD_ORIENTATION");
        }
        if rep.degenerate_triangles > 0 {
            flags.push("DEGENERATE_TRI");
        }
        if topo.euler % 2 != 0 {
            flags.push("ODD_EULER");
        }
    }
    if !nan && (vol - oracle).abs() > tol {
        flags.push("WRONG_VOLUME");
    }

    CheckOut {
        label: label.to_string(),
        flags,
        tris: res.triangle_count(),
        vol,
        oracle,
        sigma,
        tol,
        euler: topo.euler,
        components: topo.components,
        boundary_edges: rep.boundary_edges,
        non_manifold_edges: rep.non_manifold_edges,
        degenerate: rep.degenerate_triangles,
        skipped,
    }
    .with_op(op)
}

impl CheckOut {
    fn with_op(mut self, op: Op) -> CheckOut {
        if self.label.is_empty() {
            self.label = op.name().to_string();
        }
        self
    }
    fn json(&self) -> String {
        format!(
            "{{\"op\":\"{}\",\"tris\":{},\"vol\":{:.9e},\"oracle\":{:.9e},\"sigma\":{:.3e},\
             \"tol\":{:.3e},\"euler\":{},\"comp\":{},\"bnd\":{},\"nmf\":{},\"deg\":{},\
             \"mc_skip\":{},\"flags\":[{}]}}",
            self.label,
            self.tris,
            self.vol,
            self.oracle,
            self.sigma,
            self.tol,
            self.euler,
            self.components,
            self.boundary_edges,
            self.non_manifold_edges,
            self.degenerate,
            self.skipped,
            self.flags
                .iter()
                .map(|f| format!("\"{}\"", f))
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

// ---------------------------------------------------------------- categories

const CATEGORIES: &[&str] = &[
    "generic", "coplanar", "touching", "tiny", "scale", "cascade", "nested", "selfint", "thin",
];

fn make_recipe(cat: &str, seed: u64) -> Recipe {
    let mut rng = Rng::new(seed ^ (cat.bytes().fold(0u64, |a, b| a.wrapping_mul(131).wrapping_add(b as u64))));
    match cat {
        "generic" => generic_recipe(&mut rng, 1.0),
        "scale" => {
            let f = if seed % 2 == 0 { 1000.0 } else { 0.001 };
            generic_recipe(&mut rng, f)
        }
        "coplanar" => coplanar_recipe(&mut rng, seed),
        "touching" => touching_recipe(&mut rng, seed),
        "tiny" => tiny_recipe(&mut rng, seed),
        "cascade" => cascade_recipe(&mut rng),
        "nested" => nested_recipe(&mut rng, seed),
        "selfint" => selfint_recipe(&mut rng),
        "thin" => thin_recipe(&mut rng, seed),
        _ => panic!("unknown category {}", cat),
    }
}

fn generic_recipe(rng: &mut Rng, scale: f64) -> Recipe {
    let pa = Prim::random(rng, scale);
    let pb = Prim::random(rng, scale);
    let mut xa = Xform::random(rng);
    xa.t = dvec3(0.0, 0.0, 0.0);
    let mut xb = Xform::random(rng);
    // Place B so that its centre lands somewhere inside A's bbox: guarantees overlap.
    let (amin, amax) = xa.world_box(pa);
    let (bmin, bmax) = xb.world_box(pb);
    let bcen = (bmin + bmax) / 2.0;
    let target = dvec3(
        rng.range(amin.x, amax.x),
        rng.range(amin.y, amax.y),
        rng.range(amin.z, amax.z),
    );
    xb.t = target - bcen;
    Recipe::Pair {
        a: Operand { prim: pa, xf: xa },
        b: Operand { prim: pb, xf: xb },
        ops: vec![Op::Union, Op::Intersection, Op::Difference],
        note: format!("generic overlapping pair (scale {:e})", scale),
    }
}

fn coplanar_recipe(rng: &mut Rng, seed: u64) -> Recipe {
    let s = rng.range(0.5, 3.0);
    let cfg = (seed % 8) as usize;
    let a = Prim::Cube { x: s, y: s, z: s, center: false };
    let (b, bx, note) = match cfg {
        0 => (a, dvec3(s, 0.0, 0.0), "cubes sharing a full face"),
        1 => (a, dvec3(s, s, 0.0), "cubes sharing an edge"),
        2 => (a, dvec3(s, s, s), "cubes sharing a vertex"),
        3 => (a, dvec3(0.0, 0.0, 0.0), "identical cubes (A op A)"),
        4 => (
            Prim::Cube { x: s * 0.5, y: s * 0.5, z: s * 0.5, center: false },
            dvec3(0.0, 0.0, 0.0),
            "small cube in corner of big cube: 3 coincident faces",
        ),
        5 => (
            Prim::Cube { x: s, y: s * 0.5, z: s * 0.5, center: false },
            dvec3(0.0, 0.0, 0.0),
            "slab inside cube: 5 coincident faces",
        ),
        6 => (
            Prim::Cube { x: s, y: s, z: s * 0.5, center: false },
            dvec3(0.0, 0.0, s * 0.5),
            "top half slab: 5 coincident faces",
        ),
        _ => (
            Prim::Cube { x: s * 0.5, y: s * 0.5, z: s, center: false },
            dvec3(s * 0.5, s * 0.5, 0.0),
            "quarter column sharing 2 faces + interior corner",
        ),
    };
    Recipe::Pair {
        a: Operand::plain(a),
        b: Operand { prim: b, xf: Xform::translated(bx) },
        ops: vec![Op::pick(rng)],
        note: note.to_string(),
    }
}

fn touching_recipe(rng: &mut Rng, seed: u64) -> Recipe {
    let cfg = (seed % 4) as usize;
    let seg = 8 + rng.below(25) as u32;
    let rings = 4 + rng.below(13) as u32 * 2; // even -> equator ring exists
    match cfg {
        0 => Recipe::Pair {
            a: Operand::plain(Prim::Cube { x: 2.0, y: 2.0, z: 2.0, center: true }),
            b: Operand {
                prim: Prim::Sphere { r: 1.0, seg, rings },
                xf: Xform::translated(dvec3(2.0, 0.0, 0.0)),
            },
            ops: vec![Op::pick(rng)],
            note: "sphere externally tangent to a cube face".to_string(),
        },
        1 => Recipe::Pair {
            a: Operand::plain(Prim::Cube { x: 2.0, y: 2.0, z: 2.0, center: true }),
            b: Operand {
                prim: Prim::Cylinder { r: 0.5, h: 1.0, seg, center: false },
                xf: Xform::translated(dvec3(0.0, 1.0, 0.0)),
            },
            ops: vec![Op::pick(rng)],
            note: "cylinder flat cap exactly on a cube face".to_string(),
        },
        2 => Recipe::Pair {
            a: Operand::plain(Prim::Cube { x: 2.0, y: 2.0, z: 2.0, center: true }),
            b: Operand {
                prim: Prim::Cylinder { r: 0.5, h: 1.0, seg, center: false },
                xf: Xform::translated(dvec3(0.9, 1.0, 0.0)),
            },
            ops: vec![Op::pick(rng)],
            note: "cylinder cap on cube face, overhanging the edge".to_string(),
        },
        _ => Recipe::Pair {
            a: Operand::plain(Prim::Cube { x: 2.0, y: 2.0, z: 2.0, center: true }),
            b: Operand::plain(Prim::Sphere { r: 1.0, seg, rings }),
            ops: vec![Op::pick(rng)],
            note: "sphere internally tangent to all 6 cube faces".to_string(),
        },
    }
}

fn tiny_recipe(rng: &mut Rng, seed: u64) -> Recipe {
    let eps = match (seed / 8) % 4 {
        0 => 1e-3,
        1 => 1e-5,
        2 => 1e-7,
        _ => 1e-9,
    };
    let sign = if (seed / 32) % 2 == 0 { 1.0 } else { -1.0 };
    let s = 1.0;
    let cfg = (seed % 4) as usize;
    let a = Prim::Cube { x: s, y: s, z: s, center: false };
    let (b, base, note) = match cfg {
        0 => (a, dvec3(s, 0.0, 0.0), "face-sharing cubes"),
        1 => (a, dvec3(0.0, 0.0, 0.0), "identical cubes"),
        2 => (
            Prim::Cube { x: s * 0.5, y: s * 0.5, z: s * 0.5, center: false },
            dvec3(0.0, 0.0, 0.0),
            "corner-nested cubes",
        ),
        _ => (a, dvec3(s, s, 0.0), "edge-sharing cubes"),
    };
    Recipe::Pair {
        a: Operand::plain(a),
        b: Operand {
            prim: b,
            xf: Xform::translated(base + dvec3(sign * eps, 0.0, 0.0)),
        },
        ops: vec![Op::pick(rng)],
        note: format!("{} perturbed by {:e}", note, sign * eps),
    }
}

fn cascade_recipe(rng: &mut Rng) -> Recipe {
    let base = Operand::plain(Prim::Cube { x: 2.0, y: 2.0, z: 2.0, center: true });
    let mut steps = Vec::new();
    for _ in 0..10 {
        let prim = match rng.below(4) {
            0 => Prim::Cube {
                x: rng.range(0.3, 1.2),
                y: rng.range(0.3, 1.2),
                z: rng.range(0.3, 1.2),
                center: true,
            },
            1 => Prim::Sphere {
                r: rng.range(0.3, 0.9),
                seg: 8 + rng.below(17) as u32,
                rings: 4 + rng.below(9) as u32,
            },
            2 => Prim::Cylinder {
                r: rng.range(0.2, 0.7),
                h: rng.range(0.8, 3.0),
                seg: 6 + rng.below(19) as u32,
                center: true,
            },
            _ => Prim::Cone {
                r: rng.range(0.2, 0.7),
                h: rng.range(0.8, 2.5),
                seg: 6 + rng.below(19) as u32,
                center: true,
            },
        };
        let mut xf = Xform::random(rng);
        xf.s = dvec3(1.0, 1.0, 1.0);
        xf.t = dvec3(rng.range(-1.2, 1.2), rng.range(-1.2, 1.2), rng.range(-1.2, 1.2));
        let op = if rng.below(3) == 0 { Op::Union } else { Op::Difference };
        steps.push((Operand { prim, xf }, op));
    }
    Recipe::Cascade { base, steps }
}

fn nested_recipe(rng: &mut Rng, seed: u64) -> Recipe {
    let cfg = (seed % 4) as usize;
    let r = rng.range(0.5, 1.4);
    let seg = 10 + rng.below(23) as u32;
    let rings = 6 + rng.below(11) as u32;
    let big = Prim::Cube { x: 4.0, y: 4.0, z: 4.0, center: true };
    let inside_t = dvec3(rng.range(-0.4, 0.4), rng.range(-0.4, 0.4), rng.range(-0.4, 0.4));
    match cfg {
        0 => Recipe::Pair {
            a: Operand::plain(big),
            b: Operand {
                prim: Prim::Sphere { r, seg, rings },
                xf: Xform::translated(inside_t),
            },
            ops: vec![Op::Difference],
            note: "cube minus fully-enclosed sphere (should be a closed cavity)".to_string(),
        },
        1 => Recipe::Pair {
            a: Operand::plain(big),
            b: Operand {
                prim: Prim::Sphere { r, seg, rings },
                xf: Xform::translated(dvec3(8.0, 0.0, 0.0)),
            },
            ops: vec![Op::pick(rng)],
            note: "sphere entirely outside the cube".to_string(),
        },
        2 => Recipe::Pair {
            a: Operand::plain(big),
            b: Operand {
                prim: Prim::Sphere { r, seg, rings },
                xf: Xform::translated(inside_t),
            },
            ops: vec![Op::Intersection],
            note: "cube AND fully-enclosed sphere (should be the sphere)".to_string(),
        },
        _ => Recipe::Pair {
            a: Operand::plain(big),
            b: Operand {
                prim: Prim::Sphere { r, seg, rings },
                xf: Xform::translated(inside_t),
            },
            ops: vec![Op::Union],
            note: "cube OR fully-enclosed sphere (should be the cube)".to_string(),
        },
    }
}

fn selfint_recipe(rng: &mut Rng) -> Recipe {
    let s = rng.range(0.8, 1.5);
    let a1 = Operand::plain(Prim::Cube { x: s, y: s, z: s, center: true });
    let a2 = Operand {
        prim: Prim::Cube { x: s, y: s, z: s, center: true },
        xf: Xform::translated(dvec3(s * rng.range(0.3, 0.7), 0.0, 0.0)),
    };
    let b = Operand {
        prim: Prim::random(rng, 1.0),
        xf: Xform::translated(dvec3(rng.range(-0.5, 1.0), rng.range(-0.4, 0.4), rng.range(-0.4, 0.4))),
    };
    Recipe::Merge { a1, a2, b, op: Op::pick(rng) }
}

fn thin_recipe(rng: &mut Rng, seed: u64) -> Recipe {
    let t = match (seed / 4) % 4 {
        0 => 1e-3,
        1 => 1e-4,
        2 => 1e-5,
        _ => 1e-6,
    };
    let cfg = (seed % 4) as usize;
    match cfg {
        0 => Recipe::Pair {
            a: Operand::plain(Prim::Cube { x: 1.0, y: t, z: 1.0, center: false }),
            b: Operand {
                prim: Prim::Cube { x: 1.0, y: t, z: 1.0, center: false },
                xf: Xform::translated(dvec3(0.5, 0.0, 0.5)),
            },
            ops: vec![Op::pick(rng)],
            note: format!("two overlapping {:e}-thick plates", t),
        },
        1 => Recipe::Pair {
            a: Operand::plain(Prim::Cube { x: 1.0, y: 1.0, z: 1.0, center: false }),
            b: Operand {
                prim: Prim::Cube { x: 1.0, y: 1.0, z: 1.0, center: false },
                xf: Xform::translated(dvec3(0.0, 0.0, t)),
            },
            ops: vec![Op::Difference],
            note: format!("difference leaving a {:e}-thick wall", t),
        },
        2 => Recipe::Pair {
            a: Operand::plain(Prim::Cube { x: 1.0, y: 1.0, z: 1.0, center: true }),
            b: Operand {
                prim: Prim::Cylinder { r: 0.5, h: t, seg: 6 + rng.below(19) as u32, center: true },
                xf: Xform::identity(),
            },
            ops: vec![Op::pick(rng)],
            note: format!("cube vs {:e}-tall cylinder disc", t),
        },
        _ => Recipe::Pair {
            a: Operand::plain(Prim::Cube { x: 1.0, y: 1.0, z: 1.0, center: true }),
            b: Operand {
                prim: Prim::Cube { x: 1.0 - 2.0 * t, y: 2.0, z: 1.0 - 2.0 * t, center: true },
                xf: Xform::identity(),
            },
            ops: vec![Op::Difference],
            note: format!("hollow-out leaving {:e}-thick side walls", t),
        },
    }
}

// ---------------------------------------------------------------- single case

fn run_case(cat: &str, seed: u64) -> i32 {
    let t0 = Instant::now();
    let recipe = make_recipe(cat, seed);
    let mut outs: Vec<CheckOut> = Vec::new();
    let mut extra: Vec<&'static str> = Vec::new();
    let mut break_step: i64 = -1;

    match &recipe {
        Recipe::Pair { a, b, ops, .. } => {
            let sa = a.solid();
            let sb = b.solid();
            let mut vols: HashMap<&'static str, f64> = HashMap::new();
            for op in ops {
                let o = check_bool(op.name(), &sa, &sb, *op, seed);
                vols.insert(op.name(), o.vol);
                outs.push(o);
            }
            if ops.len() == 3 {
                // Inclusion-exclusion: vol(A|B) + vol(A&B) == vol(A) + vol(B)
                let va = sa.volume();
                let vb = sb.volume();
                let vu = vols["union"];
                let vi = vols["intersection"];
                let vd = vols["difference"];
                let tol = 0.02 * va.abs().max(vb.abs());
                if (vu + vi - va - vb).abs() > tol {
                    extra.push("IE_VIOLATION");
                }
                if (vd - (va - vi)).abs() > tol {
                    extra.push("DIFF_IDENTITY");
                }
            }
        }
        Recipe::Cascade { base, steps } => {
            let mut cur = base.solid();
            for (i, (o, op)) in steps.iter().enumerate() {
                let sb = o.solid();
                let res = op.apply(&cur, &sb);
                let out = check_result(
                    &format!("step{}:{}", i + 1, op.name()),
                    &cur,
                    &sb,
                    *op,
                    &res,
                    seed.wrapping_add(i as u64),
                );
                let bad = !out.flags.is_empty();
                outs.push(out);
                if bad && break_step < 0 {
                    break_step = (i + 1) as i64;
                }
                cur = res;
                if cur.triangle_count() == 0 {
                    break;
                }
            }
        }
        Recipe::Merge { a1, a2, b, op } => {
            let sa = a1.solid().merge(&a2.solid());
            let sb = b.solid();
            outs.push(check_bool(op.name(), &sa, &sb, *op, seed));
        }
    }

    let ms = t0.elapsed().as_millis();
    let mut all: Vec<&str> = Vec::new();
    for o in &outs {
        for f in &o.flags {
            if !all.contains(f) {
                all.push(f);
            }
        }
    }
    for f in &extra {
        if !all.contains(f) {
            all.push(f);
        }
    }
    println!(
        "{{\"cat\":\"{}\",\"seed\":{},\"ms\":{},\"break_step\":{},\"flags\":[{}],\"results\":[{}]}}",
        cat,
        seed,
        ms,
        break_step,
        all.iter().map(|f| format!("\"{}\"", f)).collect::<Vec<_>>().join(","),
        outs.iter().map(|o| o.json()).collect::<Vec<_>>().join(",")
    );
    0
}

// ---------------------------------------------------------------- driver

const FLAG_COLS: &[&str] = &[
    "WRONG_VOLUME",
    "NOT_CLOSED",
    "NON_MANIFOLD",
    "BAD_ORIENTATION",
    "EMPTY_RESULT",
    "ODD_EULER",
    "DEGENERATE_TRI",
    "NAN",
    "IE_VIOLATION",
    "DIFF_IDENTITY",
    "HANG",
    "CRASH",
];

struct CaseReport {
    cat: String,
    seed: u64,
    ms: u128,
    flags: Vec<String>,
    line: String,
}

fn parse_flags(line: &str) -> Vec<String> {
    // top-level "flags":[...] is the first occurrence in the line
    let Some(p) = line.find("\"flags\":[") else {
        return Vec::new();
    };
    let rest = &line[p + 9..];
    let Some(e) = rest.find(']') else { return Vec::new() };
    rest[..e]
        .split(',')
        .filter_map(|s| {
            let s = s.trim().trim_matches('"');
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        })
        .collect()
}

fn run_driver(cases: usize, cats: Vec<String>, jobs: usize, limit_ms: u128) {
    let exe = env::current_exe().expect("current_exe");
    let mut queue: Vec<(String, u64)> = Vec::new();
    for c in &cats {
        for s in 0..cases as u64 {
            queue.push((c.clone(), s));
        }
    }
    let total = queue.len();
    let mut next = 0usize;
    let mut running: Vec<(std::process::Child, String, u64, Instant)> = Vec::new();
    let mut reports: Vec<CaseReport> = Vec::new();
    let start = Instant::now();

    while next < total || !running.is_empty() {
        while running.len() < jobs && next < total {
            let (cat, seed) = queue[next].clone();
            next += 1;
            let child = Command::new(&exe)
                .arg("--case")
                .arg(&cat)
                .arg("--seed")
                .arg(seed.to_string())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn");
            running.push((child, cat, seed, Instant::now()));
        }

        let mut i = 0;
        while i < running.len() {
            let elapsed = running[i].3.elapsed().as_millis();
            let status = running[i].0.try_wait().expect("try_wait");
            match status {
                Some(st) => {
                    let (mut child, cat, seed, _) = running.remove(i);
                    let mut out = String::new();
                    if let Some(mut s) = child.stdout.take() {
                        let _ = s.read_to_string(&mut out);
                    }
                    let mut err = String::new();
                    if let Some(mut s) = child.stderr.take() {
                        let _ = s.read_to_string(&mut err);
                    }
                    let line = out
                        .lines()
                        .find(|l| l.starts_with('{'))
                        .unwrap_or("")
                        .to_string();
                    let mut flags: Vec<String> = parse_flags(&line);
                    if !st.success() || line.is_empty() {
                        flags.push("CRASH".to_string());
                    }
                    reports.push(CaseReport {
                        cat: cat.clone(),
                        seed,
                        ms: elapsed,
                        flags,
                        line: if line.is_empty() {
                            format!(
                                "{{\"cat\":\"{}\",\"seed\":{},\"ms\":{},\"flags\":[\"CRASH\"],\"stderr\":{:?}}}",
                                cat,
                                seed,
                                elapsed,
                                err.lines().last().unwrap_or("")
                            )
                        } else {
                            line
                        },
                    });
                }
                None if elapsed > limit_ms => {
                    let (mut child, cat, seed, _) = running.remove(i);
                    let _ = child.kill();
                    let _ = child.wait();
                    reports.push(CaseReport {
                        cat: cat.clone(),
                        seed,
                        ms: elapsed,
                        flags: vec!["HANG".to_string()],
                        line: format!(
                            "{{\"cat\":\"{}\",\"seed\":{},\"ms\":{},\"flags\":[\"HANG\"]}}",
                            cat, seed, elapsed
                        ),
                    });
                }
                None => i += 1,
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    }

    // ---- raw lines
    for r in &reports {
        println!("RAW {}", r.line);
    }

    // ---- aggregate
    println!();
    println!(
        "# {} cases across {} categories in {:.1}s (jobs={}, limit={}ms)",
        total,
        cats.len(),
        start.elapsed().as_secs_f64(),
        jobs,
        limit_ms
    );
    print!("{:<10} {:>6} {:>6}", "category", "total", "pass");
    for f in FLAG_COLS {
        print!(" {:>7}", &f[..f.len().min(7)]);
    }
    println!(" {:>8} {:>8}", "med_ms", "max_ms");
    println!("{}", "-".repeat(10 + 14 + FLAG_COLS.len() * 8 + 18));

    for cat in &cats {
        let rs: Vec<&CaseReport> = reports.iter().filter(|r| &r.cat == cat).collect();
        if rs.is_empty() {
            continue;
        }
        let pass = rs.iter().filter(|r| r.flags.is_empty()).count();
        print!("{:<10} {:>6} {:>6}", cat, rs.len(), pass);
        for f in FLAG_COLS {
            let n = rs.iter().filter(|r| r.flags.iter().any(|x| x == f)).count();
            print!(" {:>7}", n);
        }
        let mut times: Vec<u128> = rs.iter().map(|r| r.ms).collect();
        times.sort();
        let med = times[times.len() / 2];
        let max = *times.last().unwrap();
        println!(" {:>8} {:>8}", med, max);
    }

    println!();
    println!("# failing seeds (first 40 per category)");
    for cat in &cats {
        let bad: Vec<&CaseReport> = reports
            .iter()
            .filter(|r| &r.cat == cat && !r.flags.is_empty())
            .collect();
        if bad.is_empty() {
            continue;
        }
        let list: Vec<String> = bad
            .iter()
            .take(40)
            .map(|r| format!("{}({})", r.seed, r.flags.join("+")))
            .collect();
        println!("{}: {}", cat, list.join(" "));
    }
}

// ---------------------------------------------------------------- main

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut i = 1;
    let mut mode_case: Option<String> = None;
    let mut seed: u64 = 0;
    let mut driver = false;
    let mut cases = 200usize;
    let mut jobs = 6usize;
    let mut limit_ms = 20_000u128;
    let mut cats: Vec<String> = CATEGORIES.iter().map(|s| s.to_string()).collect();
    let mut repro: Option<(String, u64)> = None;

    while i < args.len() {
        match args[i].as_str() {
            "--case" => {
                mode_case = Some(args[i + 1].clone());
                i += 2;
            }
            "--seed" => {
                seed = args[i + 1].parse().unwrap();
                i += 2;
            }
            "--driver" => {
                driver = true;
                i += 1;
            }
            "--cases" => {
                cases = args[i + 1].parse().unwrap();
                i += 2;
            }
            "--jobs" => {
                jobs = args[i + 1].parse().unwrap();
                i += 2;
            }
            "--limit-ms" => {
                limit_ms = args[i + 1].parse().unwrap();
                i += 2;
            }
            "--cats" => {
                cats = args[i + 1].split(',').map(|s| s.to_string()).collect();
                i += 2;
            }
            "--repro" => {
                repro = Some((args[i + 1].clone(), args[i + 2].parse().unwrap()));
                i += 3;
            }
            other => {
                eprintln!("unknown arg {}", other);
                std::process::exit(2);
            }
        }
    }

    if let Some((cat, s)) = repro {
        let r = make_recipe(&cat, s);
        println!("// fuzz --case {} --seed {}", cat, s);
        println!("use makepad_csg::{{dvec3, Solid}};");
        println!("{}", r.code());
        return;
    }
    if driver {
        run_driver(cases, cats, jobs, limit_ms);
        return;
    }
    if let Some(cat) = mode_case {
        std::process::exit(run_case(&cat, seed));
    }
    eprintln!(
        "usage:\n  fuzz --case <{}> --seed <n>\n  fuzz --driver [--cases N] [--cats a,b] [--jobs J] [--limit-ms M]\n  fuzz --repro <category> <seed>",
        CATEGORIES.join("|")
    );
    std::process::exit(2);
}
