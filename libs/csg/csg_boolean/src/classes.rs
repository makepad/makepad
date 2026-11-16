use crate::tri_tri::plane_quality;
use crate::virtual_points::{ClassId, VirtualPoints};
use makepad_csg_math::fast_hash::{FastMap, FastSet};
use makepad_csg_math::{orient3d, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use std::collections::VecDeque;

/// A vertex may move at most REACH * tolerance, covering two class planes meeting down to a few degrees.
pub(crate) const REACH: f64 = 32.0;

/// The classes of one boolean: planes that faces of both operands share within its tolerance,
/// and each class vertex's virtual position on the classes it was accepted into.
pub(crate) struct Classes {
    /// Both operands' vertices, A's then B's: B's vertex `v` is point `a_vertex_count + v`.
    pub(crate) points: VirtualPoints,
    /// Per operand, per input triangle: its class.
    pub(crate) triangle_class: [Vec<Option<ClassId>>; 2],
    /// Point id of a B vertex -> point id of the A vertex at the same virtual position.
    pub(crate) alias: FastMap<u32, u32>,
    /// Refused (vertex, class) acceptances: [too far or a plane parallel to a held one, a triangle
    /// degenerates or flips, an own vertex coincides, a fourth class the placed point is not exactly on].
    pub(crate) refused: [usize; 4],
    pub(crate) count: usize,
}

struct Candidate {
    operand: usize,
    triangle: usize,
    corners: [Vec3d; 3],
    normal_len: f64,
    quality: f64,
}

pub(crate) fn build_classes(
    mesh_a: &TriMesh,
    mesh_b: &TriMesh,
    coplanar: &[(u32, u32)],
    tolerance: f64,
) -> Classes {
    let meshes = [mesh_a, mesh_b];
    let offsets = [0, mesh_a.vertices.len() as u32];
    let mut real = mesh_a.vertices.clone();
    real.extend_from_slice(&mesh_b.vertices);
    let mut points = VirtualPoints::new(real);
    let mut triangle_class = meshes.map(|mesh| vec![None; mesh.triangles.len()]);
    // Seeds: the triangles the coplanar pairs name, each linked to its partners.
    let mut partners: [FastMap<u32, Vec<u32>>; 2] = [FastMap::default(), FastMap::default()];
    let mut linked = FastSet::default();
    for &(a, b) in coplanar {
        if linked.insert((a, b)) {
            partners[0].entry(a).or_default().push(b);
            partners[1].entry(b).or_default().push(a);
        }
    }
    let corners_of = |operand: usize, triangle: usize| {
        let (a, b, c) = meshes[operand].triangle_vertices(triangle);
        [a, b, c]
    };
    let mut candidates = Vec::new();
    for operand in 0..2 {
        let mut named: Vec<u32> = partners[operand].keys().copied().collect();
        named.sort_unstable();
        for triangle in named {
            let [a, b, c] = corners_of(operand, triangle as usize);
            let normal = (b - a).cross(c - a);
            if normal == Vec3d::ZERO {
                continue;
            }
            let normal_len = normal.length();
            candidates.push(Candidate {
                operand,
                triangle: triangle as usize,
                corners: [a, b, c],
                normal_len,
                quality: plane_quality(normal_len, a, b, c),
            });
        }
    }
    // No coplanar pair names a triangle with a normal: no class, no point moves, nothing aliases.
    if candidates.is_empty() {
        return Classes { points, triangle_class, alias: FastMap::default(), refused: [0; 4], count: 0 };
    }
    // A class is the connected flat face, not only its paired triangles: it floods over each operand's
    // own edge neighbours too, so a vertex of the face beside a partner's classed edge lies on the class.
    let incidence = meshes.map(Incidence::new);

    candidates.sort_unstable_by(|a, b| {
        b.quality.total_cmp(&a.quality)
            .then_with(|| a.operand.cmp(&b.operand))
            .then_with(|| a.triangle.cmp(&b.triangle))
    });
    let mut class_vertices = Vec::new();
    // Per operand, per triangle: the last class (plus one) whose flood reached it.
    let mut visited = meshes.map(|mesh| vec![0u32; mesh.triangles.len()]);
    let mut queue = VecDeque::new();
    let mut neighbours = Vec::new();
    for candidate in &candidates {
        if triangle_class[candidate.operand][candidate.triangle].is_some() {
            continue;
        }
        let Some(class) = points.add_class(candidate.corners) else { continue };
        triangle_class[candidate.operand][candidate.triangle] = Some(class);
        visited[candidate.operand][candidate.triangle] = class + 1;
        queue.push_back((candidate.operand, candidate.triangle));
        let [s0, s1, s2] = candidate.corners;
        let bound = tolerance * candidate.normal_len;
        let mut vertices = Vec::new();
        while let Some((operand, triangle)) = queue.pop_front() {
            let ids = meshes[operand].triangles[triangle];
            vertices.extend(ids.map(|v| offsets[operand] + v));
            neighbours.clear();
            if let Some(others) = partners[operand].get(&(triangle as u32)) {
                neighbours.extend(others.iter().map(|&t| (1 - operand, t as usize)));
            }
            for (a, b) in [(ids[0], ids[1]), (ids[1], ids[2]), (ids[2], ids[0])] {
                for &t in incidence[operand].around(a) {
                    if t as usize != triangle && meshes[operand].triangles[t as usize].contains(&b) {
                        neighbours.push((operand, t as usize));
                    }
                }
            }
            for &(next_operand, next) in &neighbours {
                if triangle_class[next_operand][next].is_some() || visited[next_operand][next] == class + 1 {
                    continue;
                }
                visited[next_operand][next] = class + 1;
                let corners = corners_of(next_operand, next);
                let [a, b, c] = corners;
                if (b - a).cross(c - a) != Vec3d::ZERO
                    && corners.iter().all(|&p| orient3d(s0, s1, s2, p).abs() <= bound)
                {
                    triangle_class[next_operand][next] = Some(class);
                    queue.push_back((next_operand, next));
                }
            }
        }
        vertices.sort_unstable();
        vertices.dedup();
        class_vertices.push(vertices);
    }
    // Every seed was refused: no point moves.
    if class_vertices.is_empty() {
        return Classes { points, triangle_class, alias: FastMap::default(), refused: [0; 4], count: 0 };
    }

    // Only class vertices move: their incident triangles (with each one's real projected winding) and
    // a grid of class vertices are all the acceptance needs, whatever the operands' size.
    let mut moving: Vec<u32> = class_vertices.iter().flatten().copied().collect();
    moving.sort_unstable();
    moving.dedup();
    let mut incident: FastMap<u32, Vec<(usize, usize, i32)>> = FastMap::default();
    for &v in &moving {
        let operand = usize::from(v >= offsets[1]);
        let around = incidence[operand].around(v - offsets[operand]).iter().map(|&triangle| {
            let ids = meshes[operand].triangles[triangle as usize].map(|id| offsets[operand] + id);
            let [a, b, c] = ids.map(|id| points.real(id));
            let k = (b - a).cross(c - a).dominant_axis();
            (triangle as usize, k, points.orient2d_along(k, ids[0], ids[1], ids[2]))
        });
        incident.insert(v, around.collect());
    }

    let reach = REACH * tolerance;
    let cell = 2.0 * reach;
    let mut grid: FastMap<[i64; 3], Vec<u32>> = FastMap::default();
    for &v in &moving {
        grid.entry(cell_of(points.real(v), cell)).or_default().push(v);
    }
    let mut refused = [0; 4];
    for (class, vertices) in class_vertices.iter().enumerate() {
        for &v in vertices {
            let previous = points.defining_of(v).to_vec();
            if previous.len() == 3 {
                // Three classes fix the point. A further plane it already lies on exactly (a facet
                // round a vertex that seeds it) is a membership: no move, nothing to check. A class on
                // the very plane of one it holds is refused: two classes of a point must be two planes.
                let class = class as ClassId;
                if points.side(class, v) != 0 {
                    refused[3] += 1;
                } else if points.classes_of(v).iter().any(|&held| points.parallel(held, class)) {
                    refused[0] += 1;
                } else {
                    points.add_membership(v, class);
                }
                continue;
            }
            let mut accepted = previous.clone();
            accepted.push(class as ClassId);
            let Some(placement) = points.place(v, &accepted).filter(|point| point.delta() <= reach) else {
                refused[0] += 1;
                continue;
            };
            points.set(v, Some(placement));
            let operand = usize::from(v >= offsets[1]);
            let invalid = incident[&v].iter().any(|&(triangle, k, real_sign)| {
                let [a, b, c] = meshes[operand].triangles[triangle].map(|id| offsets[operand] + id);
                let virtual_sign = points.orient2d_along(k, a, b, c);
                virtual_sign == 0 || virtual_sign != real_sign
            });
            let reason = if invalid {
                Some(1)
            } else if coincident(&points, &grid, cell, v, |other| {
                other != v && usize::from(other >= offsets[1]) == operand
            }).is_some() {
                Some(2)
            } else {
                None
            };
            if let Some(reason) = reason {
                let restored = points.place(v, &previous);
                points.set(v, restored);
                refused[reason] += 1;
            }
        }
    }

    let mut alias = FastMap::default();
    for &v in moving.iter().filter(|&&v| v >= offsets[1]) {
        if points.classes_of(v).is_empty() {
            continue;
        }
        if let Some(a) = coincident(&points, &grid, cell, v, |other| {
            other < offsets[1] && !points.classes_of(other).is_empty()
        }) {
            alias.insert(v, a);
        }
    }
    Classes {
        points,
        triangle_class,
        alias,
        refused,
        count: class_vertices.len(),
    }
}

/// The triangles around each vertex of a mesh, in triangle order.
#[derive(Default)]
struct Incidence {
    offsets: Vec<u32>,
    triangles: Vec<u32>,
}

impl Incidence {
    fn new(mesh: &TriMesh) -> Incidence {
        let mut offsets = vec![0u32; mesh.vertices.len() + 1];
        for t in &mesh.triangles {
            for &v in t {
                offsets[v as usize + 1] += 1;
            }
        }
        for v in 0..mesh.vertices.len() {
            offsets[v + 1] += offsets[v];
        }
        let mut fill = offsets.clone();
        let mut triangles = vec![0u32; offsets[mesh.vertices.len()] as usize];
        for (index, t) in mesh.triangles.iter().enumerate() {
            for &v in t {
                triangles[fill[v as usize] as usize] = index as u32;
                fill[v as usize] += 1;
            }
        }
        Incidence { offsets, triangles }
    }

    fn around(&self, v: u32) -> &[u32] {
        &self.triangles[self.offsets[v as usize] as usize..self.offsets[v as usize + 1] as usize]
    }
}

fn cell_of(point: Vec3d, cell: f64) -> [i64; 3] {
    // Zero tolerance permits no motion; a single cell still finds every exact coincidence.
    if cell == 0.0 {
        return [0; 3];
    }
    [point.x, point.y, point.z].map(|value| (value / cell).floor() as i64)
}

fn coincident(
    points: &VirtualPoints,
    grid: &FastMap<[i64; 3], Vec<u32>>,
    cell: f64,
    v: u32,
    accept: impl Fn(u32) -> bool,
) -> Option<u32> {
    let key = cell_of(points.real(v), cell);
    for dx in -1..=1 {
        let Some(x) = key[0].checked_add(dx) else { continue };
        for dy in -1..=1 {
            let Some(y) = key[1].checked_add(dy) else { continue };
            for dz in -1..=1 {
                let Some(z) = key[2].checked_add(dz) else { continue };
                if let Some(vertices) = grid.get(&[x, y, z]) {
                    for &other in vertices {
                        if accept(other) && points.coincide(v, other) {
                            return Some(other);
                        }
                    }
                }
            }
        }
    }
    None
}
