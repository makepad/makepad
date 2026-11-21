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

pub use eval::{text_width, Evaluator, Failure, PRINT_CHORD};
pub use makepad_csg_cad::{Seg, Shape, Sketch, Tolerance};
pub use makepad_csg_cad::bool3::disjoint_tool_bounds;
pub use makepad_csg_cad::split::{NoRoom as SplitNoRoom, Piece as SplitPiece, Pins as SplitPins};
pub use makepad_csg_mesh::mesh::TriMesh;
pub use makepad_csg_mesh::subdiv;

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

/// A planar ring with points in its (u, v) frame about `origin`, star-shaped about it
/// and counter-clockwise seen along u x v.
#[derive(Clone, Debug, PartialEq)]
pub struct Ring {
    pub origin: P3,
    pub u: P3,
    pub v: P3,
    pub points: Vec<[f64; 2]>,
    /// A sharp side edge at this ring: exact back ends split there; the mesh is unchanged.
    pub crease: bool,
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Open {
    Top,
    Bottom,
    Closed,
    /// The faces with these names (an exact body only).
    Faces(Vec<edges::FaceName>),
}

/// A mirror plane through a point: `Yz` flips X, `Xz` flips Y, `Xy` flips Z.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MirrorPlane {
    Yz,
    Xz,
    Xy,
}

/// One operation of the exact B-rep engine. Its input bodies are the [`Op::Brep`] node's
/// `inputs`, in order; frames and units are those of the other ops. Fillets and shells see
/// the exact edges and faces.
#[derive(Clone, Debug, PartialEq)]
pub enum BrepOp {
    Box { size: P3 },
    Sphere { radius: f64 },
    /// Along Y, centred; `top_radius` differs from `radius` for a cone.
    Cylinder { radius: f64, top_radius: f64, height: f64 },
    /// In the XZ plane.
    Torus { radius: f64, tube: f64 },
    Extrude { sketch: Sketch, plane: Plane, height: f64, both: bool },
    /// About Y: sketch u is the radius, v the height.
    Revolve { sketch: Sketch, angle: f64 },
    /// The sketch carried along a smooth path through `path`, `scale[i]` times its size at
    /// `path[i]` (empty: its own size all the way).
    Sweep { sketch: Sketch, path: Vec<P3>, scale: Vec<f64> },
    Loft { sections: Vec<LoftSection>, plane: Plane },
    /// The first input combined with every other in order.
    Boolean { mode: BoolMode },
    Fillet { radius: f64, edges: edges::EdgeSel },
    Chamfer { size: f64, edges: edges::EdgeSel },
    /// Hollow, walls `thickness` inward.
    Shell { thickness: f64, open: Open },
    Offset { distance: f64 },
    /// Scale, then rotate X, Y, Z (degrees), then move by `at`.
    Place { at: P3, rotate: P3, scale: P3 },
    /// The mirror image across the plane through `center`; `keep` joins it with the original.
    Mirror { plane: MirrorPlane, center: P3, keep: bool },
    /// One piece of the input split by the plane through `center` facing `normal`, with `pins`
    /// across the cut ([`makepad_csg_cad::split`]); the cut's faces are named `cut`.
    Split { center: P3, normal: P3, piece: SplitPiece, pins: SplitPins },
}

/// One step of a body's recipe: everything it takes, by key.
#[derive(Clone, Debug)]
pub struct BrepStep {
    pub key: Key,
    pub op: BrepOp,
    pub inputs: Vec<Key>,
}

/// A body's exact properties (mm, mm², mm³).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrepMeasure {
    pub volume: f64,
    pub area: f64,
    pub centroid: P3,
    pub min: P3,
    pub max: P3,
    pub faces: usize,
    pub edges: usize,
}

/// An exact edge or face, for tools that pick them.
#[derive(Clone, Debug, PartialEq)]
pub struct BrepEntity {
    pub id: u32,
    /// `line`, `circle`, `curve`; or a surface kind for a face.
    pub kind: &'static str,
    /// Length of an edge, area of a face.
    pub size: f64,
    /// An edge's midpoint, a face's centroid.
    pub at: P3,
    /// A flat face's outward normal.
    pub normal: Option<P3>,
    /// The edge's solid angle is under 180 degrees.
    pub convex: bool,
    /// The edge's two faces meet tangent.
    pub smooth: bool,
    /// An arc's radius.
    pub radius: Option<f64>,
    /// A face's names (see [`edges::FaceName`]).
    pub names: Vec<edges::FaceName>,
    /// An edge's two faces, by their `id`.
    pub faces: Vec<u32>,
    /// An edge's curve sampled from end to end, finely enough to draw (a line: its two ends);
    /// none for a face.
    pub points: Vec<P3>,
}

/// What of an exact body is at a point (see [`BrepEngine::at`]). Ids as
/// [`BrepEngine::entities`] lists them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrepAt {
    /// The face nearest the point.
    pub face: Option<u32>,
    /// How far the point is from that face (mm).
    pub off: f64,
    /// The sharp edge nearest the point, within the reach asked.
    pub edge: Option<u32>,
    /// That edge and the edges continuing it tangent at shared ends, it first.
    pub chain: Vec<u32>,
}

/// What one written selection of a step took when the step was built: an edge named by its two
/// faces (`edge(top, front)`, its `near` piece or its chain), or a face to leave open (`top`).
/// A selection that takes another count or kind of entity than at the last build drifted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrepTaken {
    /// The selection by the names it gives (no binding): `edge(top, front)`, `side(2)`.
    pub named: String,
    /// The kind of each entity it took, sorted: `line`, `circle`, `curve`; a face's surface.
    pub kinds: Vec<&'static str>,
}

/// What building a step leaves to tell besides its body: a split whose cut had no room for the
/// pins it asked for (the radius tried, and the largest that would stand).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StepNote {
    SplitWithoutPins(SplitNoRoom),
}

/// A step that failed: which one and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrepFailure {
    /// The failing step's key; `None` when it isn't one step's fault.
    pub step: Option<Key>,
    /// The step failed because a selector names nothing on its body any more: the selector's
    /// place among the names the step selects by (its named edges, or the faces it leaves open).
    pub lost: Option<usize>,
    pub message: String,
}

/// What runs [`Op::Brep`] nodes: a host that wants exact bodies gives the evaluator one, so
/// this graph (and whoever links it without bodies) never links the kernel.
pub trait BrepEngine: Send + Sync {
    /// The mesh of the last step's body within `chord` mm. `cancelled` is polled.
    fn mesh(&self, steps: Vec<BrepStep>, chord: f64, cancelled: Option<&dyn Fn() -> bool>) -> Result<Arc<TriMesh>, BrepFailure>;
    fn measure(&self, steps: Vec<BrepStep>) -> Result<BrepMeasure, BrepFailure>;
    /// Its exact (edges, faces).
    fn entities(&self, steps: Vec<BrepStep>) -> Result<(Vec<BrepEntity>, Vec<BrepEntity>), BrepFailure>;
    /// What of the last step's body is at point `p` (its frame, on or near its surface): the
    /// face nearest it, and the sharp edge within `reach` mm nearest it with its tangent chain.
    fn at(&self, steps: Vec<BrepStep>, p: P3, reach: f64) -> Result<BrepAt, BrepFailure>;
    /// Whether the engine holds the body of every step keyed in `keys`: a recipe of those steps
    /// builds nothing. Answered at once, whatever the engine is building.
    fn built(&self, keys: &[Key]) -> bool;
    /// What the written selections of step `key` took when it was built, in the order written;
    /// `None` when the engine does not hold its body. Answered at once, building nothing.
    fn taken(&self, key: Key) -> Option<Vec<BrepTaken>>;
    /// What building step `key` noted, kept past its body (a mesh made from it may still show);
    /// `None` for a step built without a note. Answered at once, building nothing.
    fn note(&self, key: Key) -> Option<StepNote>;
}

/// One operation. Positions of generators are their own frame (centred on the
/// origin, Y up); [`Op::Transform`] moves any result. Two ops are the same when
/// their nodes' keys are.
#[derive(Clone, Debug)]
pub enum Op {
    /// A box centred on the origin; `radius` rounds every edge (0: sharp).
    Box { size: P3, radius: f64 },
    /// The convex hull of points, with exact facets and outward winding.
    Hull { points: Vec<P3> },
    /// The convex hull of the sources' mesh vertices and `points`: parts wrapped in one convex
    /// skin (a draft-dependent source makes it one too).
    HullOf { sources: Vec<NodeId>, points: Vec<P3> },
    /// Skin planar rings of equal point count, with flat caps fanned from the end origins.
    /// `prism` marks a two-ring straight extrusion with sharp outline corners for exact back ends.
    RingLoft { rings: Vec<Ring>, prism: bool },
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
    /// `runout`: the length each end fades over (none: one pitch); `internal`: a threaded hole's
    /// tool, fading out to the major radius instead of the minor.
    Thread { diameter: f64, pitch: Option<f64>, length: f64, clearance: f64, left: bool, runout: Option<f64>, internal: bool },
    Fillet { source: NodeId, radius: f64, edges: edges::EdgeSel },
    Chamfer { source: NodeId, size: f64, edges: edges::EdgeSel },
    /// Holes drilled from each `at` along `dir` (into the material); depth none: through.
    Hole { source: NodeId, kind: HoleKind, diameter: f64, depth: Option<f64>, at: Vec<P3>, dir: P3 },
    Mirror { source: NodeId, plane: MirrorPlane, center: P3, keep: bool },
    /// One piece of `source` split by the plane through `center` facing `normal`, with `pins`
    /// across the cut ([`makepad_csg_cad::split`]).
    Split { source: NodeId, center: P3, normal: P3, piece: SplitPiece, pins: SplitPins },
    /// Walls of `thickness`; the cavity is meshed from the source's distance at `cell`.
    Shell { source: NodeId, thickness: f64, open: Open, cell: Option<f64> },
    /// The surface moved out by `distance` (in, negative), meshed from the source's distance at `cell`.
    Offset { source: NodeId, distance: f64, cell: Option<f64> },
    /// `a` combined with every `b` in order (n-ary). `tolerance` none: scaled to the part.
    Boolean { mode: BoolMode, a: NodeId, b: Vec<NodeId> },
    /// Scale, then rotate X, Y, Z (degrees), then move by `at`.
    Transform { source: NodeId, at: P3, rotate: P3, scale: P3 },
    /// Give every source triangle `tag`; 0 clears the tag channel.
    Tag { source: NodeId, tag: u32 },
    /// A distance field meshed inside its box at `cell` (none: 80 cells over the box).
    Field { shape: SdfShape, cell: Option<f64> },
    /// A mesh given as data (an import).
    Mesh { mesh: Arc<TriMesh> },
    /// A body of the exact engine; as a mesh, its tessellation.
    Brep { op: BrepOp, inputs: Vec<NodeId> },
    /// Separate links (chains, chainmail) as tubes of `sides` facets round. Their colours are
    /// paint, not shape: a fill gives each link one material.
    Links { links: Arc<makepad_csg_sdf::Links>, sides: u32 },
    /// The Catmull-Clark surface over a closed cage of points and faces (each face's points
    /// counter-clockwise seen from outside): `levels` steps, or (none) as many as bring it within
    /// an exact body's chord of its limit surface.
    Cage { points: Vec<P3>, faces: Vec<Vec<u32>>, levels: Option<u32> },
}

impl Op {
    /// The nodes it reads.
    pub fn inputs(&self) -> Vec<NodeId> {
        match self {
            Op::Fillet { source, .. } | Op::Chamfer { source, .. } | Op::Hole { source, .. } | Op::Mirror { source, .. } | Op::Split { source, .. } | Op::Shell { source, .. } | Op::Offset { source, .. } | Op::Transform { source, .. } | Op::Tag { source, .. } => vec![*source],
            Op::Boolean { a, b, .. } => std::iter::once(*a).chain(b.iter().copied()).collect(),
            Op::Gear { mesh_with: Some((g, _)), .. } => vec![*g],
            Op::Brep { inputs, .. } => inputs.clone(),
            Op::HullOf { sources, .. } => sources.clone(),
            _ => Vec::new(),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Op::Box { .. } => "box",
            Op::Hull { .. } => "hull",
            Op::HullOf { .. } => "hull_of",
            Op::RingLoft { .. } => "ring_loft",
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
            Op::Split { .. } => "split",
            Op::Shell { .. } => "shell",
            Op::Offset { .. } => "offset",
            Op::Boolean { mode: BoolMode::Union, .. } => "union",
            Op::Boolean { mode: BoolMode::Difference, .. } => "difference",
            Op::Boolean { mode: BoolMode::Intersection, .. } => "intersection",
            Op::Transform { .. } => "transform",
            Op::Tag { .. } => "tag",
            Op::Field { .. } => "sdf",
            Op::Mesh { .. } => "mesh",
            Op::Links { .. } => "links",
            Op::Cage { .. } => "cage",
            Op::Brep { .. } => "brep",
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
            Op::Hull { points } => {
                h.u(points.len() as u64);
                points.iter().for_each(|p| h.p3(*p));
            }
            Op::HullOf { sources, points } => {
                h.u(sources.len() as u64);
                sources.iter().for_each(|s| child(&mut h, *s));
                h.u(points.len() as u64);
                points.iter().for_each(|p| h.p3(*p));
            }
            Op::RingLoft { rings, prism } => {
                h.u(rings.len() as u64);
                for ring in rings {
                    h.p3(ring.origin);
                    h.p3(ring.u);
                    h.p3(ring.v);
                    h.u(ring.points.len() as u64);
                    for p in &ring.points {
                        h.f(p[0]);
                        h.f(p[1]);
                    }
                    h.u(ring.crease as u64);
                }
                h.u(*prism as u64);
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
            Op::Thread { diameter, pitch, length, clearance, left, runout, internal } => {
                h.f(*diameter);
                h.opt(*pitch);
                h.f(*length);
                h.f(*clearance);
                h.u(*left as u64);
                h.opt(*runout);
                h.u(*internal as u64);
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
            Op::Split { source, center, normal, piece, pins } => {
                child(&mut h, *source);
                h.p3(*center);
                h.p3(*normal);
                h.u(*piece as u64);
                h.u(pins.count as u64);
                h.opt(pins.radius);
                h.opt(pins.depth);
                h.u(pins.dowels as u64);
                h.f(pins.clearance);
                h.f(pins.clearance_end);
                h.f(pins.wall);
            }
            Op::Shell { source, thickness, open, cell } => {
                child(&mut h, *source);
                h.f(*thickness);
                match open {
                    Open::Top => h.u(0),
                    Open::Bottom => h.u(1),
                    Open::Closed => h.u(2),
                    Open::Faces(names) => {
                        h.u(3);
                        h.u(names.len() as u64);
                        for n in names {
                            n.hash(&mut h);
                        }
                    }
                }
                h.opt(*cell);
            }
            Op::Offset { source, distance, cell } => {
                child(&mut h, *source);
                h.f(*distance);
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
            Op::Tag { source, tag } => {
                child(&mut h, *source);
                h.u(*tag as u64);
            }
            Op::Field { shape, cell } => {
                h.bytes(&shape.key());
                h.opt(*cell);
            }
            Op::Links { links, sides, .. } => {
                h.bytes(&links.key());
                h.u(*sides as u64);
            }
            Op::Cage { points, faces, levels } => {
                h.u(points.len() as u64);
                for p in points {
                    h.p3(*p);
                }
                h.u(faces.len() as u64);
                for f in faces {
                    h.u(f.len() as u64);
                    f.iter().for_each(|v| h.u(*v as u64));
                }
                h.u(levels.map_or(0, |n| n as u64 + 1));
            }
            // Debug writes every field, floats exactly; the inputs hash by their keys.
            Op::Brep { op, inputs } => {
                h.str(&format!("{op:?}"));
                for i in inputs {
                    child(&mut h, *i);
                }
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
                if mesh.is_tagged() {
                    h.str("tags");
                    for tag in &mesh.tags {
                        h.u(*tag as u64);
                    }
                }
            }
        }
        h.finish()
    }
}

#[cfg(test)]
mod tests;
