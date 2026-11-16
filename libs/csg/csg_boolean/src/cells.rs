// Classification by cells.
//
// The two refined surfaces cut space into cells. A patch, a part of one surface between walls, has a
// cell at its front (its counterclockwise normal's side) and one at its back. At a wall edge the
// faces of both surfaces are ordered round the edge by exact signs, and the two sides facing each
// other across each sector between neighbours are one cell. A patch is kept when the operation
// selects one of its cells and not the other, so the result is the boundary of a set of cells:
// closed and oriented by index whatever the labels. The labels decide only which solid it is: a
// cell is inside an operand by the operand's own orientation (the back of its faces), a patch at a
// seam has the other operand's outside on both sides, and a part with neither takes one ray test.

use crate::boolean::BoolOp;
use crate::classes::Classes;
use crate::classify::InsideTest;
use crate::cut::KeyPool;
use crate::virtual_points::{determinant4, filter3d};
use makepad_csg_math::fast_hash::FastSet;
use makepad_csg_math::thread_pool;
use makepad_csg_mesh::mesh::TriMesh;
use std::sync::Arc;

/// A constructed position is within 2^-50 of its exact point per coordinate, relative.
const ROUNDING: f64 = 1.0 / (1u64 << 49) as f64;

/// The exact geometry corefinement leaves for classification.
#[derive(Clone)]
pub(crate) struct Exact {
    pub(crate) classes: Arc<Classes>,
    pub(crate) pool: Arc<KeyPool>,
    /// Both operands' input triangles on point ids.
    pub(crate) triangles: Arc<[Vec<[u32; 3]>; 2]>,
    /// The (A triangle, B triangle) pairs whose site found them coplanar in the virtual set.
    pub(crate) coplanar: FastSet<[u32; 2]>,
}

impl std::fmt::Debug for Exact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Exact")
            .field("points", &self.pool.len())
            .field("coplanar", &self.coplanar.len())
            .finish_non_exhaustive()
    }
}

impl Exact {
    /// The side of point `x` of input triangle `t`'s virtual plane: positive in front, as
    /// `VirtualPoints::orient3d`.
    fn side(&self, t: [u32; 3], x: u32, exact: &mut usize) -> i32 {
        let points = &self.classes.points;
        let Some(point) = self.pool.constructed(x) else {
            let (sign, tier) = points.orient3d_tier(t[0], t[1], t[2], x);
            *exact += usize::from(tier == 31);
            return sign;
        };
        let real = [points.real(t[0]), points.real(t[1]), points.real(t[2]), point.position];
        let delta = [
            points.displacement(t[0]),
            points.displacement(t[1]),
            points.displacement(t[2]),
            point.position.length() * ROUNDING,
        ];
        if let Some(sign) = filter3d(real, delta) {
            return sign;
        }
        *exact += 1;
        let [a, b, c] = t.map(|id| points.homogeneous(id));
        // The CSG sign is the opposite of the homogeneous determinant's.
        -determinant4([&a, &b, &c, &point.coordinates]).sign()
    }
}

/// What the cells decided, for the boolean's report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CellReport {
    /// Label groups with evidence both ways (an operand that is not a solid in the virtual set).
    pub(crate) label_conflicts: usize,
    /// Wall edges whose signs make no one cyclic order: each operand's faces are joined as at an
    /// ordinary edge.
    pub(crate) degenerate_walls: usize,
    /// Wall edges where an operand has faces, but not one each way.
    pub(crate) unpaired_walls: usize,
    /// Wall edges only one surface has (the other's cut broke there, or subdivides the segment
    /// otherwise): their faces join no cell there.
    pub(crate) lone_walls: usize,
    /// Exact evaluations of the signs at the walls.
    pub(crate) wall_exact: usize,
    /// Label groups with no wall and no seam, labelled by a ray test.
    pub(crate) ray_tests: usize,
}

/// The refined surfaces and what the cells read of them.
pub(crate) struct Surfaces<'a> {
    pub(crate) meshes: [&'a TriMesh; 2],
    pub(crate) origins: [&'a [u32]; 2],
    pub(crate) walls: &'a FastSet<[u32; 2]>,
    pub(crate) exact: &'a Exact,
    /// Per operand, by point id: a vertex it shares with its triangles outside the other operand's box
    /// (empty when it has none).
    pub(crate) seams: [&'a [bool]; 2],
    /// Per operand, the solid it bounds.
    pub(crate) inside: [&'a InsideTest; 2],
}

/// Per operand, per refined face: kept, and kept with its winding reversed.
pub(crate) struct Selection {
    pub(crate) keep: [Vec<bool>; 2],
    pub(crate) reverse: [Vec<bool>; 2],
    pub(crate) report: CellReport,
}

/// A face at a wall edge.
#[derive(Clone, Copy)]
struct WallFace {
    operand: usize,
    face: u32,
    /// Its vertex off the edge.
    w: u32,
    /// Its winding runs from the edge's lower id to the higher.
    forward: bool,
}

/// A face at a wall edge, with its input triangle.
struct Face {
    operand: usize,
    origin: u32,
    triangle: [u32; 3],
    /// The axis its input triangle's normal is largest on.
    axis: usize,
    w: u32,
    forward: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Turn {
    /// Within half a turn in the positive sense (right-handed round the edge from its lower id).
    Positive,
    Negative,
    /// The same half-plane.
    Tie,
    /// The opposite half-plane.
    Half,
    /// On the face's plane, with no side of the edge in its projection.
    Degenerate,
}

#[derive(Clone)]
struct UnionFind(Vec<u32>);

impl UnionFind {
    fn new(n: usize) -> UnionFind {
        UnionFind((0..n as u32).collect())
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] as usize != x {
            self.0[x] = self.0[self.0[x] as usize];
            x = self.0[x] as usize;
        }
        x
    }

    /// The lower root stays, so a set is named by its lowest node whatever the order of the joins.
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b) as u32;
        }
    }
}

/// Patches across non-wall edges and the wall faces, in edge, operand, face order.
fn patches_and_walls(
    meshes: [&TriMesh; 2],
    walls: &FastSet<[u32; 2]>,
) -> ([Vec<u32>; 2], [usize; 2], Vec<([u32; 2], WallFace)>) {
    struct HalfEdge {
        edge: [u32; 2],
        face: u32,
        w: u32,
        operand: u8,
        forward: bool,
    }

    let mut on_wall = vec![false; meshes[0].vertex_count()];
    for edge in walls {
        on_wall[edge[0] as usize] = true;
        on_wall[edge[1] as usize] = true;
    }
    let mut half_edges = Vec::with_capacity(3 * (meshes[0].triangle_count() + meshes[1].triangle_count()));
    for (operand, mesh) in meshes.iter().enumerate() {
        for (face, t) in mesh.triangles.iter().enumerate() {
            for k in 0..3 {
                let (u, v, w) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                half_edges.push(HalfEdge {
                    edge: [u.min(v), u.max(v)],
                    face: face as u32,
                    w,
                    operand: operand as u8,
                    forward: u < v,
                });
            }
        }
    }
    // Pack the (edge, operand, face) order into two integer comparisons.
    half_edges.sort_unstable_by_key(|h| {
        (
            ((h.edge[0] as u64) << 32) | h.edge[1] as u64,
            ((h.operand as u64) << 32) | h.face as u64,
        )
    });

    let mut parents = meshes.map(|mesh| UnionFind::new(mesh.triangle_count()));
    let mut table = Vec::new();
    let mut start = 0;
    while start < half_edges.len() {
        let edge = half_edges[start].edge;
        let end = start + half_edges[start..].iter().take_while(|h| h.edge == edge).count();
        if on_wall[edge[0] as usize] && on_wall[edge[1] as usize] && walls.contains(&edge) {
            for h in &half_edges[start..end] {
                table.push((edge, WallFace {
                    operand: h.operand as usize,
                    face: h.face,
                    w: h.w,
                    forward: h.forward,
                }));
            }
        } else {
            let mut first = [None; 2];
            for h in &half_edges[start..end] {
                let operand = h.operand as usize;
                if let Some(face) = first[operand] {
                    parents[operand].union(face as usize, h.face as usize);
                } else {
                    first[operand] = Some(h.face);
                }
            }
        }
        start = end;
    }

    let mut patch = meshes.map(|mesh| vec![u32::MAX; mesh.triangle_count()]);
    let mut counts = [0usize; 2];
    for operand in 0..2 {
        for face in 0..patch[operand].len() {
            let root = parents[operand].find(face);
            // The root is the lowest face, so its patch id is assigned first.
            if root == face {
                patch[operand][face] = counts[operand] as u32;
                counts[operand] += 1;
            } else {
                patch[operand][face] = patch[operand][root];
            }
        }
    }
    (patch, counts, table)
}

struct Order<'a> {
    exact: &'a Exact,
    op: BoolOp,
    evaluations: usize,
}

impl Order<'_> {
    /// The turn from face i to face j round the edge.
    fn turn(&mut self, faces: &[Face], i: usize, j: usize) -> Turn {
        let (f, g) = (&faces[i], &faces[j]);
        let known_coplanar = (f.operand == g.operand && f.origin == g.origin)
            || f.w == g.w
            || (f.operand != g.operand && self.exact.coplanar.contains(&if f.operand == 0 {
                [f.origin, g.origin]
            } else {
                [g.origin, f.origin]
            }));
        let direction = |face: &Face| if face.forward { 1 } else { -1 };
        let side = if known_coplanar { 0 } else { self.exact.side(f.triangle, g.w, &mut self.evaluations) };
        match direction(f) * side {
            1 => Turn::Positive,
            -1 => Turn::Negative,
            _ => {
                // Both on f's plane: which side of the edge each lies on in one projection is its
                // direction times its input triangle's winding there (a piece winds as its triangle).
                let points = &self.exact.classes.points;
                let [a, b, c] = f.triangle;
                let [d, e, h] = g.triangle;
                let of_f = direction(f) * points.orient2d_along(f.axis, a, b, c);
                let of_g = direction(g) * points.orient2d_along(f.axis, d, e, h);
                if of_f == 0 || of_g == 0 {
                    Turn::Degenerate
                } else if of_f == of_g {
                    Turn::Tie
                } else {
                    Turn::Half
                }
            }
        }
    }

    /// Faces within half a turn of the reference on one side, in turning order, as bundles of faces
    /// in one half-plane.
    fn sorted(&mut self, faces: &[Face], members: Vec<usize>) -> Option<Vec<Vec<usize>>> {
        let mut bundles: Vec<Vec<usize>> = Vec::new();
        'next: for j in members {
            for b in 0..bundles.len() {
                match self.turn(faces, bundles[b][0], j) {
                    Turn::Negative => {
                        bundles.insert(b, vec![j]);
                        continue 'next;
                    }
                    Turn::Tie => {
                        bundles[b].push(j);
                        continue 'next;
                    }
                    Turn::Positive => {}
                    Turn::Half | Turn::Degenerate => return None,
                }
            }
            bundles.push(vec![j]);
        }
        Some(bundles)
    }

    /// The faces round the edge in turning order, or None when the signs make no one cyclic order
    /// (a third vertex on the edge's line, a bundle of more than one face of each operand).
    fn ring(&mut self, faces: &[Face]) -> Option<Vec<usize>> {
        if faces.len() <= 2 {
            return Some((0..faces.len()).collect());
        }
        let (mut zero, mut half, mut plus, mut minus) = (vec![0], Vec::new(), Vec::new(), Vec::new());
        for j in 1..faces.len() {
            match self.turn(faces, 0, j) {
                Turn::Tie => zero.push(j),
                Turn::Half => half.push(j),
                Turn::Positive => plus.push(j),
                Turn::Negative => minus.push(j),
                Turn::Degenerate => return None,
            }
        }
        if half.len() == 2 && self.turn(faces, half[0], half[1]) != Turn::Tie {
            return None;
        }
        let plus = self.sorted(faces, plus)?;
        let minus = self.sorted(faces, minus)?;
        let mut ring = Vec::with_capacity(faces.len());
        let half = (!half.is_empty()).then_some(half);
        for bundle in std::iter::once(zero).chain(plus).chain(half).chain(minus) {
            match bundle[..] {
                [x] => ring.push(x),
                [x, y] if faces[x].operand != faces[y].operand => {
                    // Coincident faces: the thin sector between them lies at A's back in a union and at
                    // its front otherwise. The sector after a face is on its positive side, its front
                    // when it runs forward.
                    let (a, b) = if faces[x].operand == 0 { (x, y) } else { (y, x) };
                    if faces[a].forward == (self.op != BoolOp::Union) {
                        ring.extend([a, b]);
                    } else {
                        ring.extend([b, a]);
                    }
                }
                _ => return None,
            }
        }
        Some(ring)
    }
}

/// Which refined faces the operation keeps, and which of them it reverses.
pub(crate) fn select(surfaces: &Surfaces, op: BoolOp) -> Selection {
    let mut report = CellReport::default();
    // Cell patches include every face, regardless of the cut flags.
    let Surfaces { meshes, walls, .. } = surfaces;
    let ([patch_a, patch_b], [count_a, count_b], table) = patches_and_walls(*meshes, walls);
    let face_patch = [&patch_a, &patch_b];
    let first = [0, count_a];
    let total = count_a + count_b;
    let patch_of = |operand: usize, face: usize| first[operand] + face_patch[operand][face] as usize;
    let own = |operand: usize| first[operand]..first[operand] + [count_a, count_b][operand];

    // Per patch: twice its area, its largest face (operand, face), and whether it reaches a seam.
    let mut area = vec![0.0f64; total];
    let mut largest = vec![(-1.0f64, 0usize, 0usize); total];
    let mut seam = vec![false; total];
    for operand in 0..2 {
        let mesh = surfaces.meshes[operand];
        let seams = surfaces.seams[operand];
        for (face, t) in mesh.triangles.iter().enumerate() {
            let (a, b, c) = mesh.triangle_vertices(face);
            let twice = (b - a).cross(c - a).length();
            let p = patch_of(operand, face);
            area[p] += twice;
            if twice > largest[p].0 {
                largest[p] = (twice, operand, face);
            }
            if !seams.is_empty() && t.iter().any(|&v| seams[v as usize]) {
                seam[p] = true;
            }
        }
    }

    // Cells: a patch's front is node 2p, its back 2p + 1. A face's positive side is its front when it
    // runs forward.
    let node = |face: &WallFace, positive: bool| {
        2 * patch_of(face.operand, face.face as usize) + usize::from(positive != face.forward)
    };
    let mut cells = UnionFind::new(2 * total);
    let mut order = Order { exact: surfaces.exact, op, evaluations: 0 };
    let points = &surfaces.exact.classes.points;
    let mut start = 0;
    while start < table.len() {
        if thread_pool::cancelled() {
            break;
        }
        let edge = table[start].0;
        let end = start + table[start..].iter().take_while(|entry| entry.0 == edge).count();
        let at_edge: Vec<WallFace> = table[start..end].iter().map(|&(_, face)| face).collect();
        start = end;
        let paired = (0..2).map(|operand| {
            let forward = at_edge.iter().filter(|f| f.operand == operand && f.forward).count();
            let backward = at_edge.iter().filter(|f| f.operand == operand && !f.forward).count();
            (forward, backward)
        });
        let paired: Vec<_> = paired.collect();
        // Where only one surface has the edge, the other passes along it unseen: its faces there
        // join nothing.
        if paired.iter().any(|&counts| counts == (0, 0)) {
            report.lone_walls += 1;
            continue;
        }
        if paired.iter().any(|&counts| counts != (1, 1)) {
            report.unpaired_walls += 1;
        }
        let faces: Vec<Face> = at_edge.iter().map(|f| {
            let origin = surfaces.origins[f.operand][f.face as usize];
            let triangle = surfaces.exact.triangles[f.operand][origin as usize];
            let [a, b, c] = triangle.map(|id| points.approx(id));
            Face {
                operand: f.operand,
                origin,
                triangle,
                axis: (b - a).cross(c - a).dominant_axis(),
                w: f.w,
                forward: f.forward,
            }
        }).collect();
        match order.ring(&faces) {
            Some(ring) => {
                for i in 0..ring.len() {
                    let (x, y) = (&at_edge[ring[i]], &at_edge[ring[(i + 1) % ring.len()]]);
                    cells.union(node(x, true), node(y, false));
                }
            }
            None => {
                report.degenerate_walls += 1;
                for operand in 0..2 {
                    if paired[operand] != (1, 1) {
                        continue;
                    }
                    let mut own_faces = at_edge.iter().filter(|f| f.operand == operand);
                    let (x, y) = (own_faces.next().unwrap(), own_faces.next().unwrap());
                    cells.union(node(x, true), node(y, false));
                    cells.union(node(y, true), node(x, false));
                }
            }
        }
    }
    report.wall_exact = order.evaluations;

    // Labels: per node, inside each operand. Crossing the other operand's patches cannot change it.
    let mut inside = [Vec::new(), Vec::new()];
    for operand in 0..2 {
        let other = 1 - operand;
        let mut groups = cells.clone();
        for p in own(other) {
            groups.union(2 * p, 2 * p + 1);
        }
        // Per group root: [inside, outside] evidence as (faces' twice area, patches), and a seam.
        let mut evidence = vec![[(0.0f64, 0usize); 2]; 2 * total];
        let mut at_seam = vec![false; 2 * total];
        for p in own(operand) {
            for (n, way) in [(2 * p + 1, 0), (2 * p, 1)] {
                let root = groups.find(n);
                evidence[root][way].0 += area[p];
                evidence[root][way].1 += 1;
            }
        }
        for p in own(other).filter(|&p| seam[p]) {
            let root = groups.find(2 * p);
            at_seam[root] = true;
        }
        let mut biggest = vec![(-1.0f64, 0usize, 0usize); 2 * total];
        for p in 0..total {
            let root = groups.find(2 * p);
            if largest[p].0 > biggest[root].0 {
                biggest[root] = largest[p];
            }
        }
        let mut label: Vec<Option<bool>> = vec![None; 2 * total];
        let mut of_node = vec![false; 2 * total];
        for n in 0..2 * total {
            let root = groups.find(n);
            if label[root].is_none() {
                let [(in_area, ins), (out_area, outs)] = evidence[root];
                let decided = if at_seam[root] {
                    report.label_conflicts += usize::from(ins > 0);
                    false
                } else if ins > 0 && outs > 0 {
                    report.label_conflicts += 1;
                    (in_area, ins) > (out_area, outs)
                } else if ins > 0 || outs > 0 {
                    ins > 0
                } else {
                    // No wall and no seam: one ray test from the centroid of the group's largest face.
                    report.ray_tests += 1;
                    let (_, face_operand, face) = biggest[root];
                    let (a, b, c) = surfaces.meshes[face_operand].triangle_vertices(face);
                    surfaces.inside[operand].inside((a + b + c) / 3.0)
                };
                label[root] = Some(decided);
            }
            of_node[n] = label[root].unwrap();
        }
        inside[operand] = of_node;
    }

    let selected = |n: usize| {
        let (a, b) = (inside[0][n], inside[1][n]);
        match op {
            BoolOp::Union => a || b,
            BoolOp::Intersection => a && b,
            BoolOp::Difference => a && !b,
        }
    };
    let mut keep = [Vec::new(), Vec::new()];
    let mut reverse = [Vec::new(), Vec::new()];
    for operand in 0..2 {
        for face in 0..surfaces.meshes[operand].triangle_count() {
            let p = patch_of(operand, face);
            let (front, back) = (selected(2 * p), selected(2 * p + 1));
            keep[operand].push(front != back);
            reverse[operand].push(front);
        }
    }
    Selection { keep, reverse, report }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::virtual_points::VirtualPoints;
    use makepad_csg_math::fast_hash::FastMap;
    use makepad_csg_math::Vec3d;

    /// Faces at the edge 0 -> 1 (the z axis), each its own input triangle (operand, corners).
    fn ring(points: &[[f64; 3]], faces: &[(usize, [u32; 3])], op: BoolOp) -> Option<Vec<usize>> {
        let real: Vec<Vec3d> = points.iter().map(|&[x, y, z]| Vec3d::new(x, y, z)).collect();
        let mut triangles = [Vec::new(), Vec::new()];
        for &(operand, t) in faces {
            triangles[operand].push(t);
        }
        let exact = Exact {
            classes: Arc::new(Classes {
                points: VirtualPoints::new(real.clone()),
                triangle_class: [vec![None; triangles[0].len()], vec![None; triangles[1].len()]],
                alias: FastMap::default(),
                refused: [0; 4],
                count: 0,
            }),
            pool: Arc::new(KeyPool::new(real.len() as u32)),
            triangles: Arc::new(triangles),
            coplanar: FastSet::default(),
        };
        let mut origins = [0u32; 2];
        let faces: Vec<Face> = faces.iter().map(|&(operand, t)| {
            let origin = origins[operand];
            origins[operand] += 1;
            let k = t.iter().position(|&v| v > 1).unwrap();
            let [a, b, c] = t.map(|v| real[v as usize]);
            Face {
                operand,
                origin,
                triangle: t,
                axis: (b - a).cross(c - a).dominant_axis(),
                w: t[k],
                forward: t[(k + 1) % 3] == 0,
            }
        }).collect();
        Order { exact: &exact, op, evaluations: 0 }.ring(&faces)
    }

    #[test]
    fn faces_round_an_edge_in_turning_order() {
        // 0 and 1 span the edge; the others are third vertices at angles round it (0 deg = +x, 90 = +y).
        let points = [
            [0.0, 0.0, 0.0], [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.5], [-1.0, 0.0, 0.5], [0.0, 1.0, 0.5], [0.0, -1.0, 0.5],
            [1.0, 0.0, 0.3], [-1.0, 1.0, 0.5], [1.0, 1.0, 0.5], [0.0, 0.0, 2.0],
        ];
        let (a0, a180) = ((0, [0, 1, 2]), (0, [1, 0, 3]));
        // Transversal: A at 0 and 180 degrees, B at 90 and 270.
        assert_eq!(ring(&points, &[a0, a180, (1, [0, 1, 4]), (1, [1, 0, 5])], BoolOp::Union), Some(vec![0, 2, 1, 3]),
            "transversal faces alternate");
        // A fold: A at 0 and 135, B at 45 and 270; the two within half a turn sort between them.
        assert_eq!(ring(&points, &[a0, (0, [1, 0, 7]), (1, [0, 1, 8]), (1, [1, 0, 5])], BoolOp::Union),
            Some(vec![0, 2, 1, 3]), "faces on one side sort by their turn");
        // Coincident faces, equal and opposite winding: the thin sector is at A's front in an
        // intersection and a difference, at its back in a union; the half turn comes after them.
        for b0 in [(1, [0, 1, 6]), (1, [1, 0, 6])] {
            let faces = [a0, a180, b0, (1, [1, 0, 5])];
            for op in [BoolOp::Intersection, BoolOp::Difference] {
                assert_eq!(ring(&points, &faces, op), Some(vec![0, 2, 1, 3]), "A's front sector, {b0:?} {op:?}");
            }
            assert_eq!(ring(&points, &faces, BoolOp::Union), Some(vec![2, 0, 1, 3]), "A's back sector, {b0:?}");
        }
        // A third vertex on the edge's line: no order.
        assert_eq!(ring(&points, &[a0, a180, (1, [0, 1, 9]), (1, [1, 0, 5])], BoolOp::Union), None,
            "a face along the edge's line is degenerate");
    }
}
