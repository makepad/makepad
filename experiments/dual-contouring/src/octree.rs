use crate::lerp::Lerp;
use crate::sdf_grid3::{SdfGrid3, Node};
use crate::vector3::Vector3;
use nalgebra::DMatrix;

pub enum Cell {
    Leaf {
        depth: usize,
        signs: [bool; 8],
        vertex: Option<Vector3>,
    },
    Branch {
        child_cells: Box<[Cell; 8]>,
    },
}

impl Cell {
    fn from_grid(node: &Node, min: Vector3, max: Vector3, depth: usize) -> Cell {
        match node {
            Node::Leaf { distances: ds, normals: ns } => {
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

                Cell::Leaf {
                    depth,
                    signs,
                    vertex: {
                        if signs.iter().all(|sign| *sign) || signs.iter().all(|sign| !*sign) {
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
                                Vector3 {
                                    x: min.x,
                                    y: min.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: min.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: min.x,
                                    y: max.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: max.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: min.x,
                                    y: min.y,
                                    z: max.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: min.y,
                                    z: max.z,
                                },
                                Vector3 {
                                    x: min.x,
                                    y: max.y,
                                    z: max.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: max.y,
                                    z: max.z,
                                },
                            ];

                            let mut pns = Vec::with_capacity(12);
                            for edge in EDGES.iter() {
                                if signs[edge[0]] == signs[edge[1]] {
                                    continue;
                                }
                                let d0 = ds[edge[0]];
                                let d1 = ds[edge[1]];
                                let p = ps[edge[0]].lerp(ps[edge[1]], d0 / (d0 - d1));
                                let n = ns[edge[0]].lerp(ns[edge[1]], d0 / (d0 - d1));
                                pns.push((p, n.normalize()));
                            }

                            Some(compute_vertex(&pns))
                        }
                    }
                }
            },
            Node::Branch { children } => {
                let mid = min.lerp(max, 0.5);

                Cell::Branch {
                    child_cells: Box::new([
                        Cell::from_grid(
                            &children[0b000],
                            Vector3 {
                                x: min.x,
                                y: min.y,
                                z: min.z,
                            },
                            Vector3 {
                                x: mid.x,
                                y: mid.y,
                                z: mid.z,
                            },
                            depth + 1,
                        ),
                        Cell::from_grid(
                            &children[0b001],
                            Vector3 {
                                x: mid.x,
                                y: min.y,
                                z: min.z,
                            },
                            Vector3 {
                                x: max.x,
                                y: mid.y,
                                z: mid.z,
                            },
                            depth + 1,
                        ),
                        Cell::from_grid(
                            &children[0b010],
                            Vector3 {
                                x: min.x,
                                y: mid.y,
                                z: min.z,
                            },
                            Vector3 {
                                x: mid.x,
                                y: max.y,
                                z: mid.z,
                            },
                            depth + 1,
                        ),
                        Cell::from_grid(
                            &children[0b011],
                            Vector3 {
                                x: mid.x,
                                y: mid.y,
                                z: min.z,
                            },
                            Vector3 {
                                x: max.x,
                                y: max.y,
                                z: mid.z,
                            },
                            depth + 1,
                        ),
                        Cell::from_grid(
                            &children[0b100],
                            Vector3 {
                                x: min.x,
                                y: min.y,
                                z: mid.z,
                            },
                            Vector3 {
                                x: mid.x,
                                y: mid.y,
                                z: max.z,
                            },
                            depth + 1,
                        ),
                        Cell::from_grid(
                            &children[0b101],
                            Vector3 {
                                x: mid.x,
                                y: min.y,
                                z: mid.z,
                            },
                            Vector3 {
                                x: max.x,
                                y: mid.y,
                                z: max.z,
                            },
                            depth + 1,
                        ),
                        Cell::from_grid(
                            &children[0b110],
                            Vector3 {
                                x: min.x,
                                y: mid.y,
                                z: mid.z,
                            },
                            Vector3 {
                                x: mid.x,
                                y: max.y,
                                z: max.z,
                            },
                            depth + 1,
                        ),
                        Cell::from_grid(
                            &children[0b0111],
                            Vector3 {
                                x: mid.x,
                                y: mid.y,
                                z: mid.z,
                            },
                            Vector3 {
                                x: max.x,
                                y: max.y,
                                z: max.z,
                            },
                            depth + 1,
                        ),
                    ])
                }
            }
        }
    }

    fn is_leaf(&self) -> bool {
        if let Cell::Leaf { .. } = self {
            true
        } else {
            false
        }
    }

    fn is_branch(&self) -> bool {
        if let Cell::Branch { .. } = self {
            true
        } else {
            false
        }
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

    fn vertex(&self) -> Option<Option<Vector3>> {
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

    fn child_cells(&self) -> Option<impl Iterator<Item = &Cell>> {
        if self.is_leaf() {
            None
        } else {
            Some((0..8).map(move |index| self.child_cell(index).unwrap()))
        }
    }

    fn child_face(&self, index: usize) -> Option<Face> {
        if self.is_leaf() {
            None
        } else {
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

            let (axis, indices) = FACES[index];
            Some(Face {
                axis,
                cells: [
                    self.child_cell(indices[0]).unwrap(),
                    self.child_cell(indices[1]).unwrap(),
                ],
            })
        }
    }

    fn child_faces(&self) -> Option<impl Iterator<Item = Face>> {
        if self.is_leaf() {
            None
        } else {
            Some((0..12).map(move |index| self.child_face(index).unwrap()))
        }
    }

    fn child_edge(&self, index: usize) -> Option<Edge> {
        if self.is_leaf() {
            None
        } else {
            static EDGES: [(usize, [usize; 4]); 6] = [
                (0, [0b000, 0b010, 0b100, 0b110]),
                (0, [0b001, 0b011, 0b101, 0b111]),
                (1, [0b000, 0b100, 0b001, 0b101]),
                (1, [0b010, 0b110, 0b011, 0b111]),
                (2, [0b000, 0b001, 0b010, 0b011]),
                (2, [0b100, 0b101, 0b110, 0b111]),
            ];

            let (axis, indices) = EDGES[index];
            Some(Edge {
                axis,
                cells: [
                    self.child_cell(indices[0]).unwrap(),
                    self.child_cell(indices[1]).unwrap(),
                    self.child_cell(indices[2]).unwrap(),
                    self.child_cell(indices[3]).unwrap(),
                ],
            })
        }
    }

    fn child_edges(&self) -> Option<impl Iterator<Item = Edge>> {
        if self.is_leaf() {
            None
        } else {
            Some((0..6).map(move |index| self.child_edge(index).unwrap()))
        }
    }

    pub fn traverse_leaf_edges(&self, f: &mut impl FnMut(Edge)) {
        if self.is_branch() {
            for cell in self.child_cells().unwrap() {
                cell.traverse_leaf_edges(f);
            }
            for face in self.child_faces().unwrap() {
                face.traverse_leaf_edges(f);
            }
            for edge in self.child_edges().unwrap() {
                edge.traverse_leaf_edges(f);
            }
        }
    }
}

struct Face<'a> {
    axis: usize,
    cells: [&'a Cell; 2],
}

impl<'a> Face<'a> {
    fn is_leaf(&self) -> bool {
        self.cells.iter().all(|cell| cell.is_leaf())
    }

    fn is_branch(self) -> bool {
        self.cells.iter().any(|cell| cell.is_branch())
    }

    fn child_face(self, index: usize) -> Option<Face<'a>> {
        if self.is_leaf() {
            None
        } else {
            static FACES: [[(usize, [usize; 2]); 4]; 3] = [
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

            let (axis, indices) = FACES[self.axis][index];
            Some(Face {
                axis,
                cells: [
                    self.cells[0].child_cell_or_self(indices[0]),
                    self.cells[1].child_cell_or_self(indices[1]),
                ],
            })
        }
    }

    fn child_faces(self) -> Option<impl Iterator<Item = Face<'a>>> {
        if self.is_leaf() {
            None
        } else {
            Some((0..4).map(move |index| self.child_face(index).unwrap()))
        }
    }

    fn child_edge(self, index: usize) -> Option<Edge<'a>> {
        if self.is_leaf() {
            None
        } else {
            static EDGES: [[(usize, [usize; 4], [usize; 4]); 4]; 3] = [
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

            let (axis, indices0, indices1) = EDGES[self.axis][index];
            Some(Edge {
                axis,
                cells: [
                    self.cells[indices0[0]].child_cell_or_self(indices1[0]),
                    self.cells[indices0[1]].child_cell_or_self(indices1[1]),
                    self.cells[indices0[2]].child_cell_or_self(indices1[2]),
                    self.cells[indices0[3]].child_cell_or_self(indices1[3]),
                ],
            })
        }
    }

    fn child_edges(self) -> Option<impl Iterator<Item = Edge<'a>>> {
        if self.is_leaf() {
            None
        } else {
            Some((0..4).map(move |index| self.child_edge(index).unwrap()))
        }
    }

    fn traverse_leaf_edges(self, f: &mut impl FnMut(Edge)) {
        if self.is_branch() {
            for face in self.child_faces().unwrap() {
                face.traverse_leaf_edges(f);
            }
            for edge in self.child_edges().unwrap() {
                edge.traverse_leaf_edges(f);
            }
        }
    }
}

impl<'a> Clone for Face<'a> {
    fn clone(&self) -> Self {
        Face {
            axis: self.axis,
            cells: self.cells,
        }
    }
}

impl<'a> Copy for Face<'a> {}

pub struct Edge<'a> {
    axis: usize,
    cells: [&'a Cell; 4],
}

impl<'a> Edge<'a> {
    fn is_leaf(self) -> bool {
        self.cells.iter().all(|cell| cell.is_leaf())
    }

    fn is_branch(self) -> bool {
        self.cells.iter().any(|cell| cell.is_branch())
    }

    pub fn has_sign_change(self) -> Option<bool> {
        static EDGES: [[[usize; 2]; 4]; 3] = [
            [[6, 7], [4, 5], [2, 3], [0, 1]],
            [[5, 7], [1, 3], [4, 6], [0, 2]],
            [[3, 7], [2, 6], [1, 5], [0, 4]],
        ];

        if self.is_leaf() {
            let mut found = 0;
            for index in 1..self.cells.len() {
                if self.cells[index].depth().unwrap() > self.cells[found].depth().unwrap() {
                    found = index;
                }
            }
            let cell = self.cells[found];
            let indices = EDGES[self.axis][found];
            Some(cell.sign(indices[0]).unwrap() != cell.sign(indices[1]).unwrap())
        } else {
            None
        }
    }

    pub fn vertices(self) -> Option<Vec<Vector3>> {
        if self.is_leaf() {
            Some(
                [
                    self.cells[0].vertex().unwrap(),
                    self.cells[1].vertex().unwrap(),
                    self.cells[2].vertex().unwrap(),
                    self.cells[3].vertex().unwrap(),
                ]
                .iter()
                .filter_map(|v| *v)
                .collect()
            )
        } else {
            None
        }
    }

    fn child_edge(self, index: usize) -> Option<Edge<'a>> {
        if self.is_leaf() {
            None
        } else {
            static EDGES: [[(usize, [usize; 4]); 2]; 3] = [
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

            let (axis, indices) = EDGES[self.axis][index];
            Some(Edge {
                axis,
                cells: [
                    self.cells[0].child_cell_or_self(indices[0]),
                    self.cells[1].child_cell_or_self(indices[1]),
                    self.cells[2].child_cell_or_self(indices[2]),
                    self.cells[3].child_cell_or_self(indices[3]),
                ],
            })
        }
    }

    fn child_edges(self) -> Option<impl Iterator<Item = Edge<'a>>> {
        if self.is_leaf() {
            None
        } else {
            Some((0..2).map(move |index| self.child_edge(index).unwrap()))
        }
    }

    fn traverse_leaf_edges(self, f: &mut impl FnMut(Edge)) {
        if self.is_leaf() {
            f(self);
        } else {
            for edge in self.child_edges().unwrap() {
                edge.traverse_leaf_edges(f);
            }
        }
    }
}

impl<'a> Clone for Edge<'a> {
    fn clone(&self) -> Self {
        Edge {
            axis: self.axis,
            cells: self.cells,
        }
    }
}

impl<'a> Copy for Edge<'a> {}

pub fn from_grid(grid: &SdfGrid3) -> Cell {
    Cell::from_grid(&grid.root, grid.min, grid.max, 0)
}

fn compute_vertex(pns: &[(Vector3, Vector3)]) -> Vector3 {
    let c = Vector3 {
        x: pns.iter().map(|(p, _)| p.x).sum(),
        y: pns.iter().map(|(p, _)| p.y).sum(),
        z: pns.iter().map(|(p, _)| p.z).sum(),
    } / pns.len() as f32;
    let mut a = DMatrix::<f32>::zeros(pns.len(), 3);
    for (i, (_, n)) in pns.iter().enumerate() {
        a[(i, 0)] = n.x;
        a[(i, 1)] = n.y;
        a[(i, 2)] = n.z;
    }
    let mut b = DMatrix::<f32>::zeros(pns.len(), 1);
    for (i, (p, n)) in pns.iter().enumerate() {
        b[i] = (*p - c).dot(*n);
    }
    let p = a.pseudo_inverse(0.1).unwrap() * b;
    Vector3 { x: p[0], y: p[1], z: p[2] } + c
}
