//! Meshing a distance field: dual contouring on a sparse uniform grid that
//! follows the surface.
//!
//! - **Sparse.** Blocks of the grid whose centre is farther from the surface
//!   than their half diagonal (with a margin) hold no surface and are never
//!   sampled; the surviving bricks seed the surface cells.
//! - **Surface following.** From the seeds, every cell across a sign-changing
//!   edge is visited (its corners sampled on demand), so a surface is traced
//!   whole even where the culling was wrong (a field that is not a distance):
//!   the output is closed. The grid's outer corners count as outside, so a
//!   surface the box clips is capped.
//! - **Manifold.** Each face's contour (a saddle decided by the mean of its
//!   corners, the same in both cells sharing the face) gives directed
//!   segments between edge crossings; a cell chains its faces' segments into
//!   polygons and fans each from its QEF point. A segment belongs to exactly
//!   the two cells sharing its face, run in opposite directions, so every
//!   edge of the result is shared by exactly two triangles, consistently
//!   wound. One vertex per crossing edge, shared by its four cells.
//!
//! Corner positions come from integer grid indices only, so a corner sampled
//! twice has the same value: the signs the cells see are consistent.

use crate::sdf::Sdf3;
use makepad_csg_math::fast_hash::{FastMap, FastSet};
use makepad_csg_math::Vec3d;
use makepad_csg_mesh::mesh::TriMesh;
use std::sync::Arc;

/// The most cells along one side; a finer cell is coarsened to fit.
pub const MAX_CELLS: usize = 1024;
/// Cells per side of a brick (the unit the culling keeps).
const BRICK: usize = 8;
/// A block is culled when its centre is farther than this many half diagonals.
const CULL: f64 = 1.5;

/// The grid: `n` cells per axis of `cell` from `origin`.
#[derive(Clone, Copy)]
struct Grid {
    origin: Vec3d,
    cell: f64,
    n: [usize; 3],
}

impl Grid {
    fn corner(&self, i: [usize; 3]) -> Vec3d {
        Vec3d::new(self.origin.x + i[0] as f64 * self.cell, self.origin.y + i[1] as f64 * self.cell, self.origin.z + i[2] as f64 * self.cell)
    }
    fn corner_key(&self, i: [usize; 3]) -> u64 {
        (i[0] as u64) | ((i[1] as u64) << 21) | ((i[2] as u64) << 42)
    }
    fn on_border(&self, i: [usize; 3]) -> bool {
        (0..3).any(|k| i[k] == 0 || i[k] == self.n[k])
    }
}

fn unpack(key: u64) -> [usize; 3] {
    [(key & 0x1f_ffff) as usize, ((key >> 21) & 0x1f_ffff) as usize, (key >> 42) as usize]
}

/// Corner `c` (bit 0 x, bit 1 y, bit 2 z) of cell `i`.
fn cell_corner(i: [usize; 3], c: usize) -> [usize; 3] {
    [i[0] + (c & 1), i[1] + ((c >> 1) & 1), i[2] + ((c >> 2) & 1)]
}

/// The twelve edges of a cell as corner pairs (low, high), with their axis.
const EDGES: [(usize, usize, usize); 12] = [
    (0, 1, 0),
    (2, 3, 0),
    (4, 5, 0),
    (6, 7, 0),
    (0, 2, 1),
    (1, 3, 1),
    (4, 6, 1),
    (5, 7, 1),
    (0, 4, 2),
    (1, 5, 2),
    (2, 6, 2),
    (3, 7, 2),
];

fn edge_of(a: usize, b: usize) -> usize {
    let (lo, hi) = (a.min(b), a.max(b));
    EDGES.iter().position(|e| e.0 == lo && e.1 == hi).unwrap()
}

/// The six faces, each as its four corners counter-clockwise seen from outside the cell.
const FACES: [[usize; 4]; 6] = [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]];

/// Where an edge's surface crossing is, as a fraction from its first corner:
/// kept a thousandth of the edge off both corners, so the crossings of the
/// edges meeting at a corner the surface passes through stay apart (a weld
/// downstream must not fuse them).
fn crossing_t(a: f64, b: f64) -> f64 {
    (a / (a - b)).clamp(1e-3, 1.0 - 1e-3)
}

/// The distances at `points`, in parallel chunks.
fn sample(field: &Arc<dyn Sdf3 + Send + Sync>, points: Vec<Vec3d>) -> Vec<f64> {
    if points.is_empty() {
        return Vec::new();
    }
    let chunks = (makepad_csg_math::thread_count() * 4).max(1);
    let size = points.len().div_ceil(chunks).max(64);
    let points = Arc::new(points);
    let tasks: Vec<_> = (0..points.len().div_ceil(size))
        .map(|c| {
            let (field, points) = (field.clone(), points.clone());
            move || {
                let part = &points[c * size..((c + 1) * size).min(points.len())];
                let mut out = vec![0.0; part.len()];
                field.distance_many(part, &mut out);
                out
            }
        })
        .collect();
    makepad_csg_math::thread_pool::parallel_for(tasks).concat()
}

/// One surface cell: its polygons, each a cycle of crossing edges (local
/// edge indices) and the point its fan meets at.
struct SurfaceCell {
    polygons: Vec<(Vec<u8>, Vec3d)>,
}

/// The polygons of a cell with corner values `v` and corner positions `pos`:
/// the face segments (one per contour piece on each face, directed so the
/// polygons face out of the solid) chained into cycles.
fn surface_cell(field: &dyn Sdf3, pos: [Vec3d; 8], v: [f64; 8]) -> SurfaceCell {
    let inside = v.map(|x| x < 0.0);
    let mut next = [u8::MAX; 12];
    for face in FACES {
        let e: [usize; 4] = std::array::from_fn(|k| edge_of(face[k], face[(k + 1) % 4]));
        let cross: Vec<usize> = (0..4).filter(|k| inside[face[*k]] != inside[face[(*k + 1) % 4]]).collect();
        // Walking the face counter-clockwise, a segment runs from where the walk enters the inside to where it
        // leaves it: the polygons then face out of the solid.
        let mut segment = |from: usize, to: usize| {
            let (a, b) = if inside[face[from]] { (to, from) } else { (from, to) };
            next[e[a]] = e[b] as u8;
        };
        match cross.len() {
            2 => segment(cross[0], cross[1]),
            4 => {
                // A saddle: the corners that share the centre's side are joined through it,
                // the other two are cut off, each by the segment over its two edges.
                let centre = face.iter().map(|c| v[*c]).sum::<f64>() < 0.0;
                if centre == inside[face[0]] {
                    segment(0, 1);
                    segment(2, 3);
                } else {
                    segment(3, 0);
                    segment(1, 2);
                }
            }
            _ => {}
        }
    }
    let mut done = [false; 12];
    let mut polygons = Vec::new();
    for start in 0..12 {
        if next[start] == u8::MAX || done[start] {
            continue;
        }
        let mut cycle = Vec::new();
        let mut at = start;
        while !done[at] {
            done[at] = true;
            cycle.push(at as u8);
            at = next[at] as usize;
        }
        let pns: Vec<(Vec3d, Vec3d)> = cycle
            .iter()
            .map(|k| {
                let (a, b, _) = EDGES[*k as usize];
                let t = crossing_t(v[a], v[b]);
                let p = pos[a].lerp(pos[b], t);
                (p, field.normal(p))
            })
            .collect();
        // The fan's point: the cycle's QEF point, kept inside the cell.
        let (lo, hi) = (pos[0], pos[7]);
        let eps = (hi.x - lo.x) * 1e-3;
        let q = qef_vertex(&pns);
        let q = Vec3d::new(q.x.clamp(lo.x + eps, hi.x - eps), q.y.clamp(lo.y + eps, hi.y - eps), q.z.clamp(lo.z + eps, hi.z - eps));
        polygons.push((cycle, q));
    }
    SurfaceCell { polygons }
}

/// Mesh `field` over the box `min..max` with cubic cells of `cell` (coarser
/// when the box would need more than [`MAX_CELLS`] along a side). The grid has
/// two cells of margin round the box. Triangles are wound outwards; the result
/// is closed and edge-manifold.
pub fn mesh_surface(field: Arc<dyn Sdf3 + Send + Sync>, min: Vec3d, max: Vec3d, cell: f64) -> TriMesh {
    let size = max - min;
    let extent = size.x.max(size.y).max(size.z).max(1e-9);
    let cell = cell.max(extent / (MAX_CELLS - 4) as f64).max(1e-9);
    let n = [size.x, size.y, size.z].map(|s| (s / cell).ceil().max(1.0) as usize + 4);
    let grid = Grid { origin: min - Vec3d::new(2.0 * cell, 2.0 * cell, 2.0 * cell), cell, n };

    // ---- the corner values, sampled on demand
    let mut values: FastMap<u64, f64> = Default::default();
    let fill = |values: &mut FastMap<u64, f64>, keys: Vec<u64>| {
        let points = keys.iter().map(|k| grid.corner(unpack(*k))).collect();
        let got = sample(&field, points);
        for (k, d) in keys.into_iter().zip(got) {
            // The grid's outer corners are outside: a clipped surface is capped.
            let d = if grid.on_border(unpack(k)) { d.max(cell * 1e-3) } else { d };
            values.insert(k, d);
        }
    };

    // ---- culling: blocks (in bricks) whose centre is far from the surface
    let nb = n.map(|x| x.div_ceil(BRICK));
    let mut blocks: Vec<([usize; 3], [usize; 3])> = vec![([0; 3], nb)];
    let mut bricks: Vec<[usize; 3]> = Vec::new();
    while !blocks.is_empty() {
        let centres = blocks
            .iter()
            .map(|(lo, hi)| {
                let a = grid.corner([lo[0] * BRICK, lo[1] * BRICK, lo[2] * BRICK]);
                let b = grid.corner([hi[0] * BRICK, hi[1] * BRICK, hi[2] * BRICK]);
                (a + b) * 0.5
            })
            .collect();
        let d = sample(&field, centres);
        let mut next = Vec::new();
        for ((lo, hi), d) in blocks.into_iter().zip(d) {
            let half = ((0..3).map(|k| (((hi[k] - lo[k]) * BRICK) as f64 * cell).powi(2)).sum::<f64>()).sqrt() * 0.5;
            if d.abs() > CULL * half + cell {
                continue;
            }
            if (0..3).all(|k| hi[k] - lo[k] == 1) {
                bricks.push(lo);
                continue;
            }
            // Split each side longer than one brick.
            let mids: [Vec<(usize, usize)>; 3] = std::array::from_fn(|k| if hi[k] - lo[k] > 1 { let m = (lo[k] + hi[k]) / 2; vec![(lo[k], m), (m, hi[k])] } else { vec![(lo[k], hi[k])] });
            for x in &mids[0] {
                for y in &mids[1] {
                    for z in &mids[2] {
                        next.push(([x.0, y.0, z.0], [x.1, y.1, z.1]));
                    }
                }
            }
        }
        blocks = next;
    }

    // ---- the bricks' cells: their corners, and the surface cells among them
    let cell_key = |i: [usize; 3]| grid.corner_key(i);
    let mut queue: Vec<[usize; 3]> = Vec::new();
    let mut seen: FastSet<u64> = Default::default();
    for b in &bricks {
        for z in 0..BRICK {
            for y in 0..BRICK {
                for x in 0..BRICK {
                    let i = [b[0] * BRICK + x, b[1] * BRICK + y, b[2] * BRICK + z];
                    if (0..3).all(|k| i[k] < n[k]) && seen.insert(cell_key(i)) {
                        queue.push(i);
                    }
                }
            }
        }
    }

    // ---- follow the surface: visit every cell across a crossing edge
    let mut surface: Vec<[usize; 3]> = Vec::new();
    while !queue.is_empty() {
        let mut need: Vec<u64> = Vec::new();
        let mut asked: FastSet<u64> = Default::default();
        for i in &queue {
            for c in 0..8 {
                let k = grid.corner_key(cell_corner(*i, c));
                if !values.contains_key(&k) && asked.insert(k) {
                    need.push(k);
                }
            }
        }
        fill(&mut values, need);
        let mut next = Vec::new();
        for i in queue {
            let v: [f64; 8] = std::array::from_fn(|c| values[&grid.corner_key(cell_corner(i, c))]);
            let inside = v.map(|x| x < 0.0);
            if inside.iter().all(|s| *s) || inside.iter().all(|s| !*s) {
                continue;
            }
            surface.push(i);
            for &(a, b, axis) in &EDGES {
                if inside[a] == inside[b] {
                    continue;
                }
                // The four cells round this edge.
                let at = cell_corner(i, a);
                let (u, w) = ((axis + 1) % 3, (axis + 2) % 3);
                for (du, dw) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)] {
                    if at[u] < du || at[w] < dw {
                        continue;
                    }
                    let mut j = at;
                    j[u] -= du;
                    j[w] -= dw;
                    if (0..3).all(|k| j[k] < n[k]) && seen.insert(cell_key(j)) {
                        next.push(j);
                    }
                }
            }
        }
        queue = next;
    }

    // ---- each surface cell's polygons, in parallel
    let surface = Arc::new(surface);
    let corner_values: Arc<Vec<[f64; 8]>> = Arc::new(surface.iter().map(|i| std::array::from_fn(|c| values[&grid.corner_key(cell_corner(*i, c))])).collect());
    let chunks = (makepad_csg_math::thread_count() * 4).max(1);
    let size = surface.len().div_ceil(chunks).max(16);
    let g = grid;
    let tasks: Vec<_> = (0..surface.len().div_ceil(size))
        .map(|c| {
            let (field, surface, corner_values) = (field.clone(), surface.clone(), corner_values.clone());
            move || {
                (c * size..((c + 1) * size).min(surface.len()))
                    .map(|s| surface_cell(field.as_ref(), std::array::from_fn(|k| g.corner(cell_corner(surface[s], k))), corner_values[s]))
                    .collect::<Vec<_>>()
            }
        })
        .collect();
    let cells: Vec<SurfaceCell> = makepad_csg_math::thread_pool::parallel_for(tasks).into_iter().flatten().collect();

    // ---- the mesh: one vertex per crossing edge (shared by its four cells), a fan per polygon
    let mut mesh = TriMesh::new();
    let mut crossing: FastMap<(u64, u8), u32> = Default::default();
    for (s, sc) in cells.iter().enumerate() {
        let i = surface[s];
        let v = corner_values[s];
        for (cycle, centre) in &sc.polygons {
            let ids: Vec<u32> = cycle
                .iter()
                .map(|k| {
                    let (a, b, axis) = EDGES[*k as usize];
                    let (ca, cb) = (cell_corner(i, a), cell_corner(i, b));
                    *crossing.entry((grid.corner_key(ca), axis as u8)).or_insert_with(|| {
                        let t = crossing_t(v[a], v[b]);
                        mesh.add_vertex(grid.corner(ca).lerp(grid.corner(cb), t))
                    })
                })
                .collect();
            if ids.len() == 3 {
                mesh.add_triangle(ids[0], ids[1], ids[2]);
                continue;
            }
            let c = mesh.add_vertex(*centre);
            for k in 0..ids.len() {
                mesh.add_triangle(c, ids[k], ids[(k + 1) % ids.len()]);
            }
        }
    }
    mesh
}

/// The vertex minimising the squared distances to the crossings' tangent
/// planes, regularised towards their mean.
fn qef_vertex(pns: &[(Vec3d, Vec3d)]) -> Vec3d {
    let n = pns.len() as f64;
    let c = pns.iter().fold(Vec3d::ZERO, |a, (p, _)| a + *p) * (1.0 / n);
    let mut ata = [[0.0f64; 3]; 3];
    let mut atb = [0.0f64; 3];
    for (p, nrm) in pns {
        let b = (*p - c).dot(*nrm);
        let nv = [nrm.x, nrm.y, nrm.z];
        for r in 0..3 {
            for k in 0..3 {
                ata[r][k] += nv[r] * nv[k];
            }
            atb[r] += nv[r] * b;
        }
    }
    for (k, row) in ata.iter_mut().enumerate() {
        row[k] += 0.1;
    }
    let det = |m: &[[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = det(&ata);
    if d.abs() < 1e-15 {
        return c;
    }
    let x: [f64; 3] = std::array::from_fn(|col| {
        let mut m = ata;
        for r in 0..3 {
            m[r][col] = atb[r];
        }
        det(&m) / d
    });
    c + Vec3d::new(x[0], x[1], x[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::{SdfNode, SdfOp};
    use std::collections::HashMap;

    struct Node(SdfNode, f64);
    impl Sdf3 for Node {
        fn distance(&self, p: Vec3d) -> f64 {
            crate::shape::distance(&self.0, p) * self.1
        }
    }

    /// Every edge shared by exactly two triangles in opposite directions.
    fn closed(m: &TriMesh) -> bool {
        let (open, many, flipped) = defects(m);
        if open + many + flipped > 0 {
            eprintln!("defects: open {open}, more than two {many}, same direction {flipped}");
        }
        !m.triangles.is_empty() && open + many + flipped == 0
    }

    fn defects(m: &TriMesh) -> (usize, usize, usize) {
        let mut d: HashMap<(u32, u32), i32> = HashMap::new();
        let mut u: HashMap<(u32, u32), i32> = HashMap::new();
        for t in &m.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *d.entry((a, b)).or_default() += 1;
                *u.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let open = u.values().filter(|n| **n == 1).count();
        let many = u.values().filter(|n| **n > 2).count();
        let flipped = d.values().filter(|n| **n > 1).count();
        (open, many, flipped)
    }

    fn volume(m: &TriMesh) -> f64 {
        m.triangles.iter().map(|t| {
            let (a, b, c) = (m.vertices[t[0] as usize], m.vertices[t[1] as usize], m.vertices[t[2] as usize]);
            a.dot(b.cross(c)) / 6.0
        }).sum()
    }

    fn mesh(n: SdfNode, scale: f64, lo: f64, hi: f64, cell: f64) -> TriMesh {
        mesh_surface(Arc::new(Node(n, scale)), Vec3d::new(lo, lo, lo), Vec3d::new(hi, hi, hi), cell)
    }

    fn at(p: [f64; 3], of: SdfNode) -> SdfNode {
        SdfNode::Transform { at: p, rotate: [0.0; 3], scale: 1.0, of: Box::new(of) }
    }

    #[test]
    fn a_sphere_is_closed_with_its_volume() {
        let m = mesh(SdfNode::Sphere { radius: 10.0 }, 1.0, -10.0, 10.0, 0.5);
        assert!(closed(&m));
        let v = 4.0 / 3.0 * std::f64::consts::PI * 1000.0;
        assert!((volume(&m) - v).abs() < v * 0.01, "{}", volume(&m));
    }

    #[test]
    fn touching_sheets_stay_manifold() {
        // Two spheres a hair apart and a thin torus: saddle faces and two sheets per cell.
        for gap in [0.01, 0.13, 0.27, 0.5] {
            let n = SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: vec![at([-3.0 - gap * 0.5, 0.0, 0.0], SdfNode::Sphere { radius: 3.0 }), at([3.0 + gap * 0.5, 0.1, 0.05], SdfNode::Sphere { radius: 3.0 })] };
            assert!(closed(&mesh(n, 1.0, -7.0, 7.0, 0.7)), "gap {gap}");
        }
        assert!(closed(&mesh(SdfNode::Torus { radius: 5.0, tube: 0.3 }, 1.0, -6.0, 6.0, 0.45)));
    }

    #[test]
    fn a_clipped_surface_is_capped() {
        // A gyroid fills its box: the box's faces close it.
        let m = mesh(SdfNode::Gyroid { period: 4.0, wall: 0.6 }, 1.0, -4.0, 4.0, 0.3);
        assert!(closed(&m));
    }

    #[test]
    fn a_field_that_is_no_distance_is_still_traced_whole() {
        // Distances scaled by 0.2: the culling undershoots, the surface following does not.
        let full = mesh(SdfNode::Sphere { radius: 10.0 }, 1.0, -10.0, 10.0, 0.5);
        let slow = mesh(SdfNode::Sphere { radius: 10.0 }, 0.2, -10.0, 10.0, 0.5);
        assert!(closed(&slow));
        assert_eq!(full.triangles.len(), slow.triangles.len());
    }
}
