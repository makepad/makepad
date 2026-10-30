//! makepad-csg-graph: the typed solid IR between a modelling language and the
//! CSG kernel.
//!
//! A [`Graph`] holds [`Op`] nodes in build order (every operand before the node
//! that reads it). Each node's [`Key`] is a canonical hash of its fields and its
//! operands' keys, computed when it is pushed, so equal sub-graphs have equal keys
//! wherever they appear. The [`eval::Evaluator`] builds a node into a closed,
//! outward-facing triangle mesh once per key and keeps the result, so an edit
//! rebuilds only the nodes whose keys changed.
//!
//! Units are the document's (Fab: millimetres); the model frame is Y up. Sketches
//! lie in a [`Plane`]; angles are degrees.

pub mod edges;
pub mod eval;
mod hash;

pub use eval::{Evaluator, Failure};
pub use makepad_csg_cad::{Seg, Shape, Sketch, Tolerance};
pub use makepad_csg_mesh::mesh::TriMesh;

use hash::Hasher;
pub use makepad_csg_sdf::{SdfKernel, SdfNode, SdfOp, SdfShape};
use std::sync::Arc;

pub type P3 = [f64; 3];
/// A node's canonical hash.
pub type Key = [u8; 32];

/// A node of a [`Graph`]: its index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// A sketch plane: (u, v) in the plane, w its normal (the extrusion direction).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plane {
    /// The ground: u = +X, v = -Z (to the back), extruding up +Y.
    Xz,
    /// The front: u = +X, v = +Y, extruding toward +Z.
    Xy,
    /// The side: u = +Z, v = +Y, extruding toward -X.
    Yz,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolMode {
    Union,
    Difference,
    Intersection,
}

/// A sweep's path.
#[derive(Clone, Debug, PartialEq)]
pub enum Path3 {
    Points { points: Vec<P3>, closed: bool },
    /// A helix about +Y.
    Helix { radius: f64, pitch: f64, turns: f64 },
}

/// One section of a loft: one outline, `at` along the plane's normal.
#[derive(Clone, Debug, PartialEq)]
pub struct LoftSection {
    pub sketch: Sketch,
    pub at: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    Sans,
    Mono,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// An involute spur gear (see `makepad_csg_cad::Gear`).
#[derive(Clone, Debug, PartialEq)]
pub struct GearSpec {
    pub module: f64,
    pub teeth: u32,
    pub thickness: f64,
    pub pressure: f64,
    pub backlash: f64,
    pub shift: f64,
    pub bore: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HoleKind {
    Simple,
    Counterbore { bore_diameter: f64, bore_depth: f64 },
    Countersink { sink_diameter: f64, angle: f64 },
    /// An ISO thread of the hole's diameter (`pitch` none: ISO coarse).
    Tapped { pitch: Option<f64>, clearance: f64 },
}

/// Which face a shell leaves open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    Top,
    Bottom,
    Closed,
}

/// A mirror plane through a point: `Yz` flips X, `Xz` flips Y, `Xy` flips Z.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MirrorPlane {
    Yz,
    Xz,
    Xy,
}

/// One operation. Positions of generators are their own frame (centred on the
/// origin, Y up); [`Op::Transform`] moves any result. Two ops are the same when
/// their nodes' keys are.
#[derive(Clone, Debug)]
pub enum Op {
    /// A box centred on the origin; `radius` rounds every edge (0: sharp).
    Box { size: P3, radius: f64 },
    Sphere { radius: f64, segments: u32, rings: u32 },
    /// Along Y, centred; `top_radius` differs from `radius` for a cone.
    Cylinder { radius: f64, top_radius: f64, height: f64, segments: u32 },
    /// In the XZ plane.
    Torus { radius: f64, tube: f64, segments: u32, sides: u32 },
    Extrude { sketch: Sketch, plane: Plane, height: f64, draft: f64, both: bool },
    /// About Y: sketch u is the radius, v the height.
    Revolve { sketch: Sketch, angle: f64 },
    Sweep { sketch: Sketch, path: Path3, twist: f64, scale: f64 },
    Loft { sections: Vec<LoftSection>, plane: Plane },
    Text { text: String, size: f64, depth: f64, spacing: f64, align: Align, font: Font, plane: Plane },
    /// `mesh_with`: beside another gear node (same module), `angle` degrees round it.
    Gear { gear: GearSpec, plane: Plane, mesh_with: Option<(NodeId, f64)> },
    /// An ISO metric threaded rod along +Y.
    Thread { diameter: f64, pitch: Option<f64>, length: f64, clearance: f64, left: bool },
    Fillet { source: NodeId, radius: f64, edges: edges::EdgeSel },
    Chamfer { source: NodeId, size: f64, edges: edges::EdgeSel },
    /// Holes drilled from each `at` along `dir` (into the material); depth none: through.
    Hole { source: NodeId, kind: HoleKind, diameter: f64, depth: Option<f64>, at: Vec<P3>, dir: P3 },
    Mirror { source: NodeId, plane: MirrorPlane, center: P3, keep: bool },
    /// Walls of `thickness`; the cavity is meshed from the source's distance at `cell`.
    Shell { source: NodeId, thickness: f64, open: Open, cell: Option<f64> },
    /// `a` combined with every `b` in order (n-ary). `tolerance` none: scaled to the part.
    Boolean { mode: BoolMode, a: NodeId, b: Vec<NodeId> },
    /// Scale, then rotate X, Y, Z (degrees), then move by `at`.
    Transform { source: NodeId, at: P3, rotate: P3, scale: P3 },
    /// A distance field meshed inside its box at `cell` (none: 80 cells over the box).
    Field { shape: SdfShape, cell: Option<f64> },
    /// A mesh given as data (an import).
    Mesh { mesh: Arc<TriMesh> },
}

impl Op {
    /// The nodes it reads.
    pub fn inputs(&self) -> Vec<NodeId> {
        match self {
            Op::Fillet { source, .. } | Op::Chamfer { source, .. } | Op::Hole { source, .. } | Op::Mirror { source, .. } | Op::Shell { source, .. } | Op::Transform { source, .. } => vec![*source],
            Op::Boolean { a, b, .. } => std::iter::once(*a).chain(b.iter().copied()).collect(),
            Op::Gear { mesh_with: Some((g, _)), .. } => vec![*g],
            _ => Vec::new(),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Op::Box { .. } => "box",
            Op::Sphere { .. } => "sphere",
            Op::Cylinder { .. } => "cylinder",
            Op::Torus { .. } => "torus",
            Op::Extrude { .. } => "extrude",
            Op::Revolve { .. } => "revolve",
            Op::Sweep { .. } => "sweep",
            Op::Loft { .. } => "loft",
            Op::Text { .. } => "text",
            Op::Gear { .. } => "gear",
            Op::Thread { .. } => "thread",
            Op::Fillet { .. } => "fillet",
            Op::Chamfer { .. } => "chamfer",
            Op::Hole { .. } => "hole",
            Op::Mirror { .. } => "mirror",
            Op::Shell { .. } => "shell",
            Op::Boolean { mode: BoolMode::Union, .. } => "union",
            Op::Boolean { mode: BoolMode::Difference, .. } => "difference",
            Op::Boolean { mode: BoolMode::Intersection, .. } => "intersection",
            Op::Transform { .. } => "transform",
            Op::Field { .. } => "sdf",
            Op::Mesh { .. } => "mesh",
        }
    }
}

/// One node: its op and its key.
#[derive(Clone, Debug)]
pub struct Node {
    pub op: Op,
    pub key: Key,
}

/// Solids in build order.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
}

impl Graph {
    pub fn new() -> Graph {
        Graph::default()
    }

    /// Add a node; its operands must be in the graph already.
    pub fn push(&mut self, op: Op) -> Result<NodeId, String> {
        let n = self.nodes.len() as u32;
        for i in op.inputs() {
            if i.0 >= n {
                return Err(format!("{} reads node {} that is not built before it", op.name(), i.0));
            }
        }
        let key = self.key_of(&op);
        self.nodes.push(Node { op, key });
        Ok(NodeId(n))
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn key_of(&self, op: &Op) -> Key {
        let mut h = Hasher::new();
        h.str(op.name());
        let child = |h: &mut Hasher, id: NodeId| h.bytes(&self.nodes[id.0 as usize].key);
        match op {
            Op::Box { size, radius } => {
                h.p3(*size);
                h.f(*radius);
            }
            Op::Sphere { radius, segments, rings } => {
                h.f(*radius);
                h.u(*segments as u64);
                h.u(*rings as u64);
            }
            Op::Cylinder { radius, top_radius, height, segments } => {
                h.f(*radius);
                h.f(*top_radius);
                h.f(*height);
                h.u(*segments as u64);
            }
            Op::Torus { radius, tube, segments, sides } => {
                h.f(*radius);
                h.f(*tube);
                h.u(*segments as u64);
                h.u(*sides as u64);
            }
            Op::Extrude { sketch, plane, height, draft, both } => {
                h.sketch(sketch);
                h.u(*plane as u64);
                h.f(*height);
                h.f(*draft);
                h.u(*both as u64);
            }
            Op::Revolve { sketch, angle } => {
                h.sketch(sketch);
                h.f(*angle);
            }
            Op::Sweep { sketch, path, twist, scale } => {
                h.sketch(sketch);
                match path {
                    Path3::Points { points, closed } => {
                        h.u(0);
                        h.u(points.len() as u64);
                        points.iter().for_each(|p| h.p3(*p));
                        h.u(*closed as u64);
                    }
                    Path3::Helix { radius, pitch, turns } => {
                        h.u(1);
                        h.f(*radius);
                        h.f(*pitch);
                        h.f(*turns);
                    }
                }
                h.f(*twist);
                h.f(*scale);
            }
            Op::Loft { sections, plane } => {
                h.u(sections.len() as u64);
                for s in sections {
                    h.sketch(&s.sketch);
                    h.f(s.at);
                }
                h.u(*plane as u64);
            }
            Op::Text { text, size, depth, spacing, align, font, plane } => {
                h.str(text);
                h.f(*size);
                h.f(*depth);
                h.f(*spacing);
                h.u(*align as u64);
                h.u(*font as u64);
                h.u(*plane as u64);
            }
            Op::Gear { gear, plane, mesh_with } => {
                h.f(gear.module);
                h.u(gear.teeth as u64);
                h.f(gear.thickness);
                h.f(gear.pressure);
                h.f(gear.backlash);
                h.f(gear.shift);
                h.f(gear.bore);
                h.u(*plane as u64);
                match mesh_with {
                    Some((g, angle)) => {
                        h.u(1);
                        child(&mut h, *g);
                        h.f(*angle);
                    }
                    None => h.u(0),
                }
            }
            Op::Thread { diameter, pitch, length, clearance, left } => {
                h.f(*diameter);
                h.opt(*pitch);
                h.f(*length);
                h.f(*clearance);
                h.u(*left as u64);
            }
            Op::Fillet { source, radius, edges } => {
                child(&mut h, *source);
                h.f(*radius);
                edges.hash(&mut h);
            }
            Op::Chamfer { source, size, edges } => {
                child(&mut h, *source);
                h.f(*size);
                edges.hash(&mut h);
            }
            Op::Hole { source, kind, diameter, depth, at, dir } => {
                child(&mut h, *source);
                match kind {
                    HoleKind::Simple => h.u(0),
                    HoleKind::Counterbore { bore_diameter, bore_depth } => {
                        h.u(1);
                        h.f(*bore_diameter);
                        h.f(*bore_depth);
                    }
                    HoleKind::Countersink { sink_diameter, angle } => {
                        h.u(2);
                        h.f(*sink_diameter);
                        h.f(*angle);
                    }
                    HoleKind::Tapped { pitch, clearance } => {
                        h.u(3);
                        h.opt(*pitch);
                        h.f(*clearance);
                    }
                }
                h.f(*diameter);
                h.opt(*depth);
                h.u(at.len() as u64);
                at.iter().for_each(|p| h.p3(*p));
                h.p3(*dir);
            }
            Op::Mirror { source, plane, center, keep } => {
                child(&mut h, *source);
                h.u(*plane as u64);
                h.p3(*center);
                h.u(*keep as u64);
            }
            Op::Shell { source, thickness, open, cell } => {
                child(&mut h, *source);
                h.f(*thickness);
                h.u(*open as u64);
                h.opt(*cell);
            }
            Op::Boolean { mode, a, b } => {
                h.u(*mode as u64);
                child(&mut h, *a);
                h.u(b.len() as u64);
                b.iter().for_each(|x| child(&mut h, *x));
            }
            Op::Transform { source, at, rotate, scale } => {
                child(&mut h, *source);
                h.p3(*at);
                h.p3(*rotate);
                h.p3(*scale);
            }
            Op::Field { shape, cell } => {
                h.bytes(&shape.key());
                h.opt(*cell);
            }
            Op::Mesh { mesh } => {
                h.u(mesh.vertices.len() as u64);
                for v in &mesh.vertices {
                    h.p3([v.x, v.y, v.z]);
                }
                h.u(mesh.triangles.len() as u64);
                for t in &mesh.triangles {
                    h.u(t[0] as u64);
                    h.u(t[1] as u64);
                    h.u(t[2] as u64);
                }
            }
        }
        h.finish()
    }
}

#[cfg(test)]
mod tests;
