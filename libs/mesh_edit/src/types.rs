use std::collections::BTreeMap;

macro_rules! id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);
    };
}
id!(VertexId);
id!(FaceId);
id!(CornerId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EdgeKey(pub VertexId, pub VertexId);
impl EdgeKey {
    pub fn new(a: VertexId, b: VertexId) -> Self {
        if a < b {
            Self(a, b)
        } else {
            Self(b, a)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ElementId {
    Vertex(VertexId),
    Face(FaceId),
    Corner(CornerId),
    Edge(EdgeKey),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointWeight {
    pub joint: u32,
    pub weight: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vertex {
    pub id: VertexId,
    pub position: [f64; 3],
    pub weights: Vec<JointWeight>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Corner {
    pub id: CornerId,
    pub vertex: VertexId,
    pub uv: [f64; 2],
    pub normal: Option<[f64; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub id: FaceId,
    pub first_corner: u32,
    pub corner_count: u32,
    pub material: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeAttributes {
    pub seam: bool,
    pub crease: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeData {
    pub attributes: EdgeAttributes,
    pub loose: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Polygon {
    pub vertices: Vec<u32>,
    pub uvs: Vec<[f64; 2]>,
    pub material: u32,
}
impl Polygon {
    pub fn new(vertices: Vec<u32>) -> Self {
        Self {
            vertices,
            uvs: Vec::new(),
            material: 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangeSet {
    pub created: Vec<ElementId>,
    pub retained: Vec<ElementId>,
    pub deleted: Vec<ElementId>,
    /// Explicit one-to-many / many-to-one provenance; identity is not proximity.
    pub remapped: Vec<(ElementId, Vec<ElementId>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExtrudeResult {
    pub cap: FaceId,
    pub side_faces: Vec<FaceId>,
    pub rim_edges: Vec<EdgeKey>,
    pub changes: ChangeSet,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegionExtrudeResult {
    pub caps: Vec<FaceId>,
    pub side_faces: Vec<FaceId>,
    pub changes: ChangeSet,
}

#[derive(Clone,Debug,PartialEq)]
pub enum Deformation {
    Bend {axis:usize,angle:f64,range:[f64;2]},
    Twist {axis:usize,angle:f64,range:[f64;2]},
    Taper {axis:usize,scales:[f64;2],range:[f64;2]},
}

#[derive(Clone,Debug,PartialEq)]
pub struct DecimateResult {
    pub changes:ChangeSet,
    pub achieved_faces:usize,
    pub collapses:usize,
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum SculptKind {Inflate,Flatten,Crease}
#[derive(Clone,Debug,PartialEq)]
pub struct SculptBrush {
    pub kind:SculptKind,
    pub center:[f64;3],
    pub normal:[f64;3],
    pub radius:f64,
    pub strength:f64,
    pub max_displacement:f64,
    /// Zero is fully editable; one is fully protected. Unlisted vertices use 0.
    pub masks:Vec<(VertexId,f64)>,
    /// Reflect the brush in planes through the origin: X=1, Y=2, Z=4.
    pub symmetry:u8,
}

#[derive(Clone,Debug,PartialEq)]
pub struct UvIsland {
    pub faces:Vec<FaceId>,
    pub corners:Vec<CornerId>,
    pub bounds:[[f64;2];2],
    pub area:f64,
    pub pinned:usize,
}

#[derive(Clone,Debug,PartialEq)]
pub struct AppendResult {
    pub changes:ChangeSet,
    /// Source IDs belong to the imported mesh's identity scope.
    pub imported:Vec<(ElementId,ElementId)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InsetResult {
    pub inset: FaceId,
    pub ring_faces: Vec<FaceId>,
    pub changes: ChangeSet,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TriangleVertex {
    pub position: [f64; 3],
    pub normal: [f64; 3],
    pub uv: [f64; 2],
    pub weights: Vec<JointWeight>,
    pub source_vertex: VertexId,
    pub source_corner: CornerId,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Triangle {
    pub indices: [u32; 3],
    pub source_face: FaceId,
    pub material: u32,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriangleMesh {
    pub vertices: Vec<TriangleVertex>,
    pub triangles: Vec<Triangle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeUse {
    pub face: FaceId,
    pub corner: CornerId,
    pub from: VertexId,
    pub to: VertexId,
}

/// Built only when requested. Callers may cache this against a document revision.
/// No shared locks, interior mutability or persistent adjacency cycles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Adjacency {
    pub edges: BTreeMap<EdgeKey, Vec<EdgeUse>>,
    pub vertex_faces: BTreeMap<VertexId, Vec<FaceId>>,
    pub vertex_edges: BTreeMap<VertexId, Vec<EdgeKey>>,
}
impl Adjacency {
    pub fn radial(&self, edge: EdgeKey) -> &[EdgeUse] {
        self.edges.get(&edge).map(Vec::as_slice).unwrap_or(&[])
    }
}
