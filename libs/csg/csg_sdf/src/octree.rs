use crate::grid::{Node, SdfGrid3};
use crate::sdf::Sdf3;
use makepad_csg_math::Vec3d;
use makepad_csg_mesh::mesh::TriMesh;
use std::sync::Arc;

/// Convert an SDF to a triangle mesh via dual contouring.
///
/// - `sdf`: the signed distance field to mesh (wrapped in Arc for thread-safe sharing)
/// - `min`, `max`: bounding box for the meshing volume
/// - `depth`: octree subdivision depth (6-8 is typical; higher = more detail)
///
/// Returns a `TriMesh` compatible with the CSG boolean stack.
pub fn sdf_to_mesh(
    sdf: impl Sdf3 + Send + Sync + 'static,
    min: Vec3d,
    max: Vec3d,
    depth: usize,
) -> TriMesh {
    let sdf: Arc<dyn Sdf3 + Send + Sync> = Arc::new(sdf);
    let grid = SdfGrid3::from_sdf(sdf.clone(), min, max, depth);
    // The cells of the top two levels' 64 subtrees are built in parallel
    // (each one's vertices need normals: several field evaluations each);
    // the result is the sequential octree.
    let octree = cells_parallel(grid.root, grid.min, grid.max, 0, sdf.clone(), 2);
    mesh_from_cells(&octree)
}

fn cells_parallel(node: Node, min: Vec3d, max: Vec3d, depth: usize, sdf: Arc<dyn Sdf3 + Send + Sync>, levels: usize) -> Cell {
    let children = match node {
        Node::Branch { children } if levels > 0 => *children,
        node => return Cell::from_grid(&node, min, max, depth, sdf.as_ref()),
    };
    let mid = min.lerp(max, 0.5);
    let tasks: Vec<_> = children
        .into_iter()
        .enumerate()
        .map(|(i, child)| {
            let (lo, hi) = child_box(min, mid, max, i);
            let sdf = sdf.clone();
            move || cells_parallel(child, lo, hi, depth + 1, sdf, levels - 1)
        })
        .collect();
    let cells: Vec<Cell> = makepad_csg_math::thread_pool::parallel_for(tasks);
    let cells: [Cell; 8] = cells.try_into().unwrap_or_else(|_| unreachable!("eight children"));
    Cell::Branch { child_cells: Box::new(cells) }
}

/// Child `i`'s box (bit 0: x, bit 1: y, bit 2: z upper half), as in [`Cell::from_grid`].
fn child_box(min: Vec3d, mid: Vec3d, max: Vec3d, i: usize) -> (Vec3d, Vec3d) {
    let pick = |bit: usize, lo: f64, m: f64, hi: f64| if i & bit == 0 { (lo, m) } else { (m, hi) };
    let (x0, x1) = pick(1, min.x, mid.x, max.x);
    let (y0, y1) = pick(2, min.y, mid.y, max.y);
    let (z0, z1) = pick(4, min.z, mid.z, max.z);
    (Vec3d::new(x0, y0, z0), Vec3d::new(x1, y1, z1))
}

/// Sequential counterpart of [`sdf_to_mesh`] for a borrowed field. The
/// generated mesh is identical; only grid construction stays on the caller's
/// thread. This is what lets the `csg.implicit` interpreter fallback borrow
/// its capability-stripped splash VM without making that VM `Send`.
pub fn sdf_to_mesh_ref(
    sdf: &dyn Sdf3,
    min: Vec3d,
    max: Vec3d,
    depth: usize,
) -> TriMesh {
    let grid = SdfGrid3::from_sdf_ref(sdf, min, max, depth);
    mesh_from_cells(&from_grid(&grid, sdf))
}

fn mesh_from_cells(octree: &Cell) -> TriMesh {

    let mut vertices: Vec<Vec3d> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut ids: makepad_csg_math::fast_hash::FastMap<[u64; 3], u32> = Default::default();

    octree.traverse_leaf_edges(&mut |edge: Edge| {
        let Some(Some(inside_low)) = edge.sign_change_direction() else { return };
        let Some(vs) = edge.vertices() else { return };
        // A cell's vertex is shared by every quad around it: index it once by
        // its exact position (what the weld below would do first).
        let mut index = |p: Vec3d| -> u32 {
            let next = vertices.len() as u32;
            *ids.entry([p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).or_insert_with(|| {
                vertices.push(p);
                next
            })
        };
        if vs.len() == 4 {
            // The four cells sit in a 2x2 layout around the edge (cyclic order
            // 0, 1, 3, 2) and each vertex inside its own cell, so the quad's
            // area along the edge's axis gives the layout's turn; the surface
            // faces from the inside end of the edge to the outside end. Both
            // triangles of the quad, and every quad, wind by that rule alone.
            let along = {
                let a = (vs[3].0 - vs[0].0).cross(vs[2].0 - vs[1].0);
                [a.x, a.y, a.z][edge.axis]
            };
            if along != 0.0 {
                let ids = [index(vs[0].0), index(vs[1].0), index(vs[2].0), index(vs[3].0)];
                if (along > 0.0) == inside_low {
                    triangles.push([ids[0], ids[1], ids[2]]);
                    triangles.push([ids[1], ids[3], ids[2]]);
                } else {
                    triangles.push([ids[0], ids[2], ids[1]]);
                    triangles.push([ids[1], ids[2], ids[3]]);
                }
                return;
            }
        }
        // A degenerate layout: wind by the field's normals.
        let mut emit_tri = |a: (Vec3d, Vec3d), b: (Vec3d, Vec3d), c: (Vec3d, Vec3d)| {
            let (ia, ib, ic) = (index(a.0), index(b.0), index(c.0));
            let geo_normal = (b.0 - a.0).cross(c.0 - a.0);
            let avg_sdf_normal = a.1 + b.1 + c.1;
            if geo_normal.dot(avg_sdf_normal) < 0.0 {
                triangles.push([ia, ic, ib]);
            } else {
                triangles.push([ia, ib, ic]);
            }
        };
        match vs.len() {
            3 => emit_tri(vs[0], vs[1], vs[2]),
            4 => {
                emit_tri(vs[0], vs[1], vs[2]);
                emit_tri(vs[2], vs[1], vs[3]);
            }
            _ => {}
        }
    });

    let mut mesh = TriMesh::with_capacity(vertices.len(), triangles.len());
    for v in &vertices {
        mesh.add_vertex(*v);
    }
    for t in &triangles {
        mesh.add_triangle(t[0], t[1], t[2]);
    }
    mesh.weld_vertices(1e-10);
    mesh
}

// ---- Octree dual contouring internals ----

enum Cell {
    Leaf {
        depth: usize,
        signs: [bool; 8],
        vertex: Option<(Vec3d, Vec3d)>, // (position, average surface normal)
    },
    Branch {
        child_cells: Box<[Cell; 8]>,
    },
}

impl Cell {
    fn from_grid(node: &Node, min: Vec3d, max: Vec3d, depth: usize, sdf: &dyn Sdf3) -> Cell {
        match node {
            Node::Leaf { distances: ds } => {
                let signs = [
                    ds[0] < 0.0,
                    ds[1] < 0.0,
                    ds[2] < 0.0,
                    ds[3] < 0.0,
                    ds[4] < 0.0,
                    ds[5] < 0.0,
                    ds[6] < 0.0,
                    ds[7] < 0.0,
                ];

                let vertex = if signs.iter().all(|&s| s) || signs.iter().all(|&s| !s) {
                    None
                } else {
                    const EDGES: [[usize; 2]; 12] = [
                        [0, 1],
                        [2, 3],
                        [4, 5],
                        [6, 7],
                        [0, 2],
                        [4, 6],
                        [1, 3],
                        [5, 7],
                        [0, 4],
                        [1, 5],
                        [2, 6],
                        [3, 7],
                    ];

                    let ps = [
                        Vec3d::new(min.x, min.y, min.z),
                        Vec3d::new(max.x, min.y, min.z),
                        Vec3d::new(min.x, max.y, min.z),
                        Vec3d::new(max.x, max.y, min.z),
                        Vec3d::new(min.x, min.y, max.z),
                        Vec3d::new(max.x, min.y, max.z),
                        Vec3d::new(min.x, max.y, max.z),
                        Vec3d::new(max.x, max.y, max.z),
                    ];

                    let mut pns = Vec::with_capacity(12);
                    for edge in &EDGES {
                        if signs[edge[0]] == signs[edge[1]] {
                            continue;
                        }
                        let d0 = ds[edge[0]];
                        let d1 = ds[edge[1]];
                        let t = d0 / (d0 - d1);
                        let p = ps[edge[0]].lerp(ps[edge[1]], t);
                        let n = sdf.normal(p);
                        pns.push((p, n));
                    }

                    let avg_normal = {
                        let mut n = Vec3d::ZERO;
                        for (_, ni) in &pns {
                            n = n + *ni;
                        }
                        n.normalize()
                    };
                    // Kept inside its cell: the quads around it then wind
                    // consistently (see mesh_from_cells).
                    let v = compute_vertex(&pns);
                    let v = Vec3d::new(v.x.clamp(min.x, max.x), v.y.clamp(min.y, max.y), v.z.clamp(min.z, max.z));
                    Some((v, avg_normal))
                };

                Cell::Leaf {
                    depth,
                    signs,
                    vertex,
                }
            }
            Node::Branch { children } => {
                let mid = min.lerp(max, 0.5);
                Cell::Branch {
                    child_cells: Box::new([
                        Cell::from_grid(
                            &children[0b000],
                            Vec3d::new(min.x, min.y, min.z),
                            Vec3d::new(mid.x, mid.y, mid.z),
                            depth + 1,
                            sdf,
                        ),
                        Cell::from_grid(
                            &children[0b001],
                            Vec3d::new(mid.x, min.y, min.z),
                            Vec3d::new(max.x, mid.y, mid.z),
                            depth + 1,
                            sdf,
                        ),
                        Cell::from_grid(
                            &children[0b010],
                            Vec3d::new(min.x, mid.y, min.z),
                            Vec3d::new(mid.x, max.y, mid.z),
                            depth + 1,
                            sdf,
                        ),
                        Cell::from_grid(
                            &children[0b011],
                            Vec3d::new(mid.x, mid.y, min.z),
                            Vec3d::new(max.x, max.y, mid.z),
                            depth + 1,
                            sdf,
                        ),
                        Cell::from_grid(
                            &children[0b100],
                            Vec3d::new(min.x, min.y, mid.z),
                            Vec3d::new(mid.x, mid.y, max.z),
                            depth + 1,
                            sdf,
                        ),
                        Cell::from_grid(
                            &children[0b101],
                            Vec3d::new(mid.x, min.y, mid.z),
                            Vec3d::new(max.x, mid.y, max.z),
                            depth + 1,
                            sdf,
                        ),
                        Cell::from_grid(
                            &children[0b110],
                            Vec3d::new(min.x, mid.y, mid.z),
                            Vec3d::new(mid.x, max.y, max.z),
                            depth + 1,
                            sdf,
                        ),
                        Cell::from_grid(
                            &children[0b111],
                            Vec3d::new(mid.x, mid.y, mid.z),
                            Vec3d::new(max.x, max.y, max.z),
                            depth + 1,
                            sdf,
                        ),
                    ]),
                }
            }
        }
    }

    fn is_leaf(&self) -> bool {
        matches!(self, Cell::Leaf { .. })
    }

    fn is_branch(&self) -> bool {
        matches!(self, Cell::Branch { .. })
    }

    fn depth(&self) -> Option<usize> {
        if let &Cell::Leaf { depth, .. } = self {
            Some(depth)
        } else {
            None
        }
    }

    fn sign(&self, index: usize) -> Option<bool> {
        if let Cell::Leaf { signs, .. } = self {
            Some(signs[index])
        } else {
            None
        }
    }

    fn vertex(&self) -> Option<Option<(Vec3d, Vec3d)>> {
        if let &Cell::Leaf { vertex, .. } = self {
            Some(vertex)
        } else {
            None
        }
    }

    fn child_cell(&self, index: usize) -> Option<&Cell> {
        if let Cell::Branch { child_cells } = self {
            Some(&child_cells[index])
        } else {
            None
        }
    }

    fn child_cell_or_self(&self, index: usize) -> &Cell {
        self.child_cell(index).unwrap_or(self)
    }

    fn traverse_leaf_edges(&self, f: &mut impl FnMut(Edge)) {
        if self.is_branch() {
            for i in 0..8 {
                self.child_cell(i).unwrap().traverse_leaf_edges(f);
            }
            static FACES: [(usize, [usize; 2]); 12] = [
                (0, [0b000, 0b001]),
                (0, [0b010, 0b011]),
                (0, [0b100, 0b101]),
                (0, [0b110, 0b111]),
                (1, [0b000, 0b010]),
                (1, [0b100, 0b110]),
                (1, [0b001, 0b011]),
                (1, [0b101, 0b111]),
                (2, [0b000, 0b100]),
                (2, [0b001, 0b101]),
                (2, [0b010, 0b110]),
                (2, [0b011, 0b111]),
            ];
            for &(axis, indices) in &FACES {
                let face = Face {
                    axis,
                    cells: [
                        self.child_cell(indices[0]).unwrap(),
                        self.child_cell(indices[1]).unwrap(),
                    ],
                };
                face.traverse_leaf_edges(f);
            }
            static EDGES: [(usize, [usize; 4]); 6] = [
                (0, [0b000, 0b010, 0b100, 0b110]),
                (0, [0b001, 0b011, 0b101, 0b111]),
                (1, [0b000, 0b100, 0b001, 0b101]),
                (1, [0b010, 0b110, 0b011, 0b111]),
                (2, [0b000, 0b001, 0b010, 0b011]),
                (2, [0b100, 0b101, 0b110, 0b111]),
            ];
            for &(axis, indices) in &EDGES {
                let edge = Edge {
                    axis,
                    cells: [
                        self.child_cell(indices[0]).unwrap(),
                        self.child_cell(indices[1]).unwrap(),
                        self.child_cell(indices[2]).unwrap(),
                        self.child_cell(indices[3]).unwrap(),
                    ],
                };
                edge.traverse_leaf_edges(f);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Face<'a> {
    axis: usize,
    cells: [&'a Cell; 2],
}

impl<'a> Face<'a> {
    fn is_branch(self) -> bool {
        self.cells.iter().any(|c| c.is_branch())
    }

    fn traverse_leaf_edges(self, f: &mut impl FnMut(Edge)) {
        if !self.is_branch() {
            return;
        }

        static CHILD_FACES: [[(usize, [usize; 2]); 4]; 3] = [
            [
                (0, [0b001, 0b000]),
                (0, [0b011, 0b010]),
                (0, [0b101, 0b100]),
                (0, [0b111, 0b110]),
            ],
            [
                (1, [0b010, 0b000]),
                (1, [0b110, 0b100]),
                (1, [0b011, 0b001]),
                (1, [0b111, 0b101]),
            ],
            [
                (2, [0b100, 0b000]),
                (2, [0b101, 0b001]),
                (2, [0b110, 0b010]),
                (2, [0b111, 0b011]),
            ],
        ];

        for &(axis, indices) in &CHILD_FACES[self.axis] {
            let child_face = Face {
                axis,
                cells: [
                    self.cells[0].child_cell_or_self(indices[0]),
                    self.cells[1].child_cell_or_self(indices[1]),
                ],
            };
            child_face.traverse_leaf_edges(f);
        }

        static CHILD_EDGES: [[(usize, [usize; 4], [usize; 4]); 4]; 3] = [
            [
                (1, [0, 0, 1, 1], [0b001, 0b101, 0b000, 0b100]),
                (1, [0, 0, 1, 1], [0b011, 0b111, 0b010, 0b110]),
                (2, [0, 1, 0, 1], [0b001, 0b000, 0b011, 0b010]),
                (2, [0, 1, 0, 1], [0b101, 0b100, 0b111, 0b110]),
            ],
            [
                (2, [0, 0, 1, 1], [0b010, 0b011, 0b000, 0b001]),
                (2, [0, 0, 1, 1], [0b110, 0b111, 0b100, 0b101]),
                (0, [0, 1, 0, 1], [0b010, 0b000, 0b110, 0b100]),
                (0, [0, 1, 0, 1], [0b011, 0b001, 0b111, 0b101]),
            ],
            [
                (0, [0, 0, 1, 1], [0b100, 0b110, 0b000, 0b010]),
                (0, [0, 0, 1, 1], [0b101, 0b111, 0b001, 0b011]),
                (1, [0, 1, 0, 1], [0b100, 0b000, 0b101, 0b001]),
                (1, [0, 1, 0, 1], [0b110, 0b010, 0b111, 0b011]),
            ],
        ];

        for &(axis, ref src, ref idx) in &CHILD_EDGES[self.axis] {
            let edge = Edge {
                axis,
                cells: [
                    self.cells[src[0]].child_cell_or_self(idx[0]),
                    self.cells[src[1]].child_cell_or_self(idx[1]),
                    self.cells[src[2]].child_cell_or_self(idx[2]),
                    self.cells[src[3]].child_cell_or_self(idx[3]),
                ],
            };
            edge.traverse_leaf_edges(f);
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Edge<'a> {
    pub(crate) axis: usize,
    cells: [&'a Cell; 4],
}

impl<'a> Edge<'a> {
    fn is_leaf(self) -> bool {
        self.cells.iter().all(|c| c.is_leaf())
    }

    pub(crate) fn sign_change_direction(self) -> Option<Option<bool>> {
        static EDGE_SIGNS: [[[usize; 2]; 4]; 3] = [
            [[6, 7], [4, 5], [2, 3], [0, 1]],
            [[5, 7], [1, 3], [4, 6], [0, 2]],
            [[3, 7], [2, 6], [1, 5], [0, 4]],
        ];

        if !self.is_leaf() {
            return None;
        }

        let mut found = 0;
        for i in 1..4 {
            if self.cells[i].depth().unwrap() > self.cells[found].depth().unwrap() {
                found = i;
            }
        }
        let indices = EDGE_SIGNS[self.axis][found];
        let s0 = self.cells[found].sign(indices[0]).unwrap();
        let s1 = self.cells[found].sign(indices[1]).unwrap();
        if s0 == s1 {
            Some(None)
        } else {
            Some(Some(s0))
        }
    }

    pub(crate) fn vertices(self) -> Option<Vec<(Vec3d, Vec3d)>> {
        if !self.is_leaf() {
            return None;
        }
        Some(
            self.cells
                .iter()
                .filter_map(|c| c.vertex().unwrap())
                .collect(),
        )
    }

    fn traverse_leaf_edges(self, f: &mut impl FnMut(Edge)) {
        if self.is_leaf() {
            f(self);
        } else {
            static CHILD_EDGES: [[(usize, [usize; 4]); 2]; 3] = [
                [
                    (0, [0b111, 0b101, 0b011, 0b001]),
                    (0, [0b110, 0b100, 0b010, 0b000]),
                ],
                [
                    (1, [0b111, 0b011, 0b110, 0b010]),
                    (1, [0b101, 0b001, 0b100, 0b000]),
                ],
                [
                    (2, [0b111, 0b110, 0b101, 0b100]),
                    (2, [0b011, 0b010, 0b001, 0b000]),
                ],
            ];
            for &(axis, ref indices) in &CHILD_EDGES[self.axis] {
                let child = Edge {
                    axis,
                    cells: [
                        self.cells[0].child_cell_or_self(indices[0]),
                        self.cells[1].child_cell_or_self(indices[1]),
                        self.cells[2].child_cell_or_self(indices[2]),
                        self.cells[3].child_cell_or_self(indices[3]),
                    ],
                };
                child.traverse_leaf_edges(f);
            }
        }
    }
}

fn from_grid(grid: &SdfGrid3, sdf: &dyn Sdf3) -> Cell {
    Cell::from_grid(&grid.root, grid.min, grid.max, 0, sdf)
}

/// Compute the optimal vertex position inside a cell using QEF (quadratic error function).
fn compute_vertex(pns: &[(Vec3d, Vec3d)]) -> Vec3d {
    let n = pns.len() as f64;
    let c = Vec3d::new(
        pns.iter().map(|(p, _)| p.x).sum::<f64>() / n,
        pns.iter().map(|(p, _)| p.y).sum::<f64>() / n,
        pns.iter().map(|(p, _)| p.z).sum::<f64>() / n,
    );

    let mut ata = [[0.0f64; 3]; 3];
    let mut atb = [0.0f64; 3];

    for (p, normal) in pns {
        let d = *p - c;
        let bi = d.dot(*normal);
        let nx = normal.x;
        let ny = normal.y;
        let nz = normal.z;

        ata[0][0] += nx * nx;
        ata[0][1] += nx * ny;
        ata[0][2] += nx * nz;
        ata[1][1] += ny * ny;
        ata[1][2] += ny * nz;
        ata[2][2] += nz * nz;

        atb[0] += nx * bi;
        atb[1] += ny * bi;
        atb[2] += nz * bi;
    }
    ata[1][0] = ata[0][1];
    ata[2][0] = ata[0][2];
    ata[2][1] = ata[1][2];

    let lambda = 0.1;
    ata[0][0] += lambda;
    ata[1][1] += lambda;
    ata[2][2] += lambda;

    if let Some(x) = solve_3x3(&ata, &atb) {
        c + Vec3d::new(x[0], x[1], x[2])
    } else {
        c
    }
}

fn solve_3x3(a: &[[f64; 3]; 3], b: &[f64; 3]) -> Option<[f64; 3]> {
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);

    if det.abs() < 1e-15 {
        return None;
    }

    let inv_det = 1.0 / det;

    let x0 = (b[0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (b[1] * a[2][2] - a[1][2] * b[2])
        + a[0][2] * (b[1] * a[2][1] - a[1][1] * b[2]))
        * inv_det;

    let x1 = (a[0][0] * (b[1] * a[2][2] - a[1][2] * b[2])
        - b[0] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * b[2] - b[1] * a[2][0]))
        * inv_det;

    let x2 = (a[0][0] * (a[1][1] * b[2] - b[1] * a[2][1])
        - a[0][1] * (a[1][0] * b[2] - b[1] * a[2][0])
        + b[0] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]))
        * inv_det;

    Some([x0, x1, x2])
}
