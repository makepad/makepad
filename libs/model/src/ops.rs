use crate::{
    mesh::{CornerId, EdgeAttributes, EdgeKey, FaceId, JointWeight, VertexId},
    AnimationClip, Skeleton,
};

/// Commands contain stable element identities, never transient render indices.
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    SoftBody(crate::SoftBodyBind),
    /// Detach simulation while retaining the editable skeleton and animation.
    SoftBodyUnbind,
    Selection(crate::SelectionOperation),
    Construction(crate::ConstructionOperation),
    Rig(crate::RigOperation),
    Scene(crate::SceneOperation),
    Surface(crate::SurfaceOperation),
    MeshEditing(crate::MeshEditingOperation),
    Cube {
        object: String,
        size: [f64; 3],
    },
    Plane {
        object: String,
        size: [f64; 2],
    },
    ImportMesh {
        object: String,
        source: Vec<u8>,
    },
    DeleteObject {
        object: String,
    },
    Transform {
        object: String,
        vertices: Vec<VertexId>,
        matrix: [[f64; 4]; 4],
    },
    Extrude {
        object: String,
        face: FaceId,
        offset: [f64; 3],
    },
    DeleteFaces {
        object: String,
        faces: Vec<FaceId>,
    },
    Mirror {
        object: String,
        axis: u8,
        offset: f64,
    },
    SetUv {
        object: String,
        corner: CornerId,
        uv: [f64; 2],
    },
    SetWeights {
        object: String,
        vertex: VertexId,
        weights: Vec<JointWeight>,
    },
    AssignMaterial {
        object: String,
        faces: Vec<FaceId>,
        material: u32,
    },
    SetMaterial {
        material: u32,
        value: Material,
    },
    Inset {
        object: String,
        face: FaceId,
        distance: f64,
    },
    Weld {
        object: String,
        vertices: Vec<VertexId>,
        distance: f64,
    },
    EdgeAttributes {
        object: String,
        edge: EdgeKey,
        attributes: EdgeAttributes,
    },
    CornerNormal {
        object: String,
        corner: CornerId,
        normal: Option<[f64; 3]>,
    },
    TextureSolid {
        material: u32,
        width: u32,
        height: u32,
        color: [u8; 3],
    },
    PaintTexture {
        material: u32,
        center: [f64; 2],
        radius: f64,
        color: [u8; 3],
    },
    SetSkeleton {
        skeleton: Skeleton,
    },
    SetClip {
        clip: AnimationClip,
    },
    DeleteClip {
        name: String,
    },
    AutoWeights {
        object: String,
    },
    Smooth {
        object: String,
        vertices: Vec<VertexId>,
        iterations: u32,
        factor: f64,
        preserve_boundary: bool,
    },
    Subdivide {
        object: String,
        levels: u32,
    },
    ProjectUv {
        object: String,
        faces: Vec<FaceId>,
        axis: u8,
        scale: [f64; 2],
        offset: [f64; 2],
    },
    Brush {
        object: String,
        vertices: Vec<VertexId>,
        center: [f64; 3],
        radius: f64,
        delta: [f64; 3],
        max_displacement: f64,
    },
}

/// The first portable material contract: opaque base colour and optional PNG.
/// Unsupported PBR channels must be added explicitly, never silently discarded.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub color: [f64; 3],
    pub base_color_png: Vec<u8>,
}
impl Default for Material {
    fn default() -> Self {
        Self {
            color: [1.0; 3],
            base_color_png: Vec::new(),
        }
    }
}

/// Semantic output selections survive triangulation and can feed the next edit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OperationResult {
    pub object: String,
    pub faces: Vec<FaceId>,
    pub vertices: Vec<VertexId>,
    pub metrics: std::collections::BTreeMap<String, usize>,
}

impl Operation {
    pub(crate) fn object_name(&self) -> Option<&str> {
        match self {
            Self::SoftBody(op) => Some(&op.object),
            Self::Selection(op) => Some(op.object_name()),
            Self::Construction(op) => Some(op.object_name()),
            Self::Rig(op) => op.object_name(),
            Self::Scene(op) => op.object_name(),
            Self::MeshEditing(op) => Some(op.object_name()),
            Self::Surface(op) => op.object_name(),
            Self::Cube { object, .. }
            | Self::Plane { object, .. }
            | Self::ImportMesh { object, .. }
            | Self::DeleteObject { object }
            | Self::Transform { object, .. }
            | Self::Extrude { object, .. }
            | Self::DeleteFaces { object, .. }
            | Self::Mirror { object, .. }
            | Self::SetUv { object, .. }
            | Self::SetWeights { object, .. }
            | Self::AssignMaterial { object, .. }
            | Self::Inset { object, .. }
            | Self::Weld { object, .. }
            | Self::EdgeAttributes { object, .. }
            | Self::CornerNormal { object, .. }
            | Self::AutoWeights { object }
            | Self::Smooth { object, .. }
            | Self::Subdivide { object, .. }
            | Self::ProjectUv { object, .. }
            | Self::Brush { object, .. } => Some(object),
            _ => None,
        }
    }
    pub(crate) fn memory_bytes(&self) -> usize {
        let variable = match self {
            Self::SoftBody(op) => op.memory_bytes(),
            Self::Selection(op) => op.memory_bytes(),
            Self::Construction(op) => op.memory_bytes(),
            Self::Rig(op) => op.memory_bytes(),
            Self::Scene(op) => op.memory_bytes(),
            Self::MeshEditing(op) => op.memory_bytes(),
            Self::Surface(op) => op.memory_bytes(),
            Self::ImportMesh { source, .. } => source.len(),
            Self::Transform { vertices, .. }
            | Self::Weld { vertices, .. }
            | Self::Smooth { vertices, .. }
            | Self::Brush { vertices, .. } => vertices.len().saturating_mul(8),
            Self::DeleteFaces { faces, .. }
            | Self::AssignMaterial { faces, .. }
            | Self::ProjectUv { faces, .. } => faces.len().saturating_mul(8),
            Self::SetWeights { weights, .. } => weights
                .len()
                .saturating_mul(std::mem::size_of::<JointWeight>()),
            Self::SetMaterial { value, .. } => value.base_color_png.len(),
            Self::SetSkeleton { skeleton } => skeleton.memory_bytes(),
            Self::SetClip { clip } => clip.memory_bytes(),
            Self::DeleteClip { name } => name.len(),
            _ => 0,
        };
        std::mem::size_of::<Self>()
            .saturating_add(variable.saturating_mul(2))
            .saturating_add(self.object_name().map_or(0, str::len).saturating_mul(2))
    }
}

impl Operation {
    /// The family or kind of an operation, for messages.
    pub fn family_name(&self) -> &'static str {
        match self {
            Self::SoftBody(_) | Self::SoftBodyUnbind => "soft_body",
            Self::Selection(_) => "selection",
            Self::Construction(_) => "construction",
            Self::Rig(_) => "rig",
            Self::Scene(_) => "scene",
            Self::Surface(_) => "surface",
            Self::MeshEditing(_) => "mesh_edit",
            _ => "operation",
        }
    }
}

impl Operation {
    /// Everything an operation must satisfy before it executes, however it was made: built
    /// in Rust or decoded from a binary source. Execution checks
    /// against the document (missing objects, stale ids) come later.
    pub fn check(&self, limits: &crate::Limits) -> crate::Result<()> {
        use crate::Error;
        let budget = |n: usize, max: usize, what: &'static str| if n > max { Err(Error::Budget(what)) } else { Ok(()) };
        let axis = |a: u8| if a > 2 { Err(Error::Invalid("axis must be 0, 1 or 2")) } else { Ok(()) };
        match self {
            Self::SoftBody(op) => return op.check(limits),
            Self::Selection(op) => return op.check(limits),
            Self::Construction(op) => return op.check(limits),
            Self::Rig(op) => return op.check(limits),
            Self::Scene(op) => return op.check(limits),
            Self::Surface(op) => return op.check(limits),
            Self::MeshEditing(op) => return op.check(limits),
            _ => {}
        }
        if let Some(object) = self.object_name() {
            budget(object.len(), limits.max_name_bytes, "object name")?;
        }
        match self {
            Self::Transform { vertices, .. } | Self::Weld { vertices, .. } | Self::Smooth { vertices, .. } | Self::Brush { vertices, .. } => budget(vertices.len(), limits.mesh.max_vertices, "selection size"),
            Self::DeleteFaces { faces, .. } | Self::AssignMaterial { faces, .. } => budget(faces.len(), limits.mesh.max_faces, "selection size"),
            Self::ProjectUv { faces, axis: a, .. } => { budget(faces.len(), limits.mesh.max_faces, "selection size")?; axis(*a) }
            Self::Mirror { axis: a, .. } => axis(*a),
            Self::SetWeights { weights, .. } => budget(weights.len(), limits.mesh.max_weights_per_vertex, "joint influences"),
            Self::SetSkeleton { skeleton } => {
                budget(skeleton.joints.len(), limits.max_joints, "joints")?;
                skeleton.joints.iter().try_for_each(|j| budget(j.name.len(), limits.max_name_bytes, "joint name"))
            }
            Self::SetClip { clip } => {
                budget(clip.name.len(), limits.max_name_bytes, "clip name")?;
                budget(clip.channels.len(), limits.max_joints.saturating_mul(3), "clip channels")?;
                budget(clip.channels.iter().map(|c| c.keys.len()).sum(), limits.max_keyframes, "keyframes")
            }
            Self::ImportMesh { source, .. } => budget(source.len(), limits.max_transaction_bytes, "generated transaction bytes"),
            _ => Ok(()),
        }
    }
}

impl Operation {
    /// A typed import operation for a UV sphere mesh.
    pub fn sphere(object: impl Into<String>, radius: f64, segments: u32, rings: u32, smooth: bool, limits: &crate::Limits) -> crate::Result<Self> {
        Ok(Self::ImportMesh { object: object.into(), source: crate::primitives::sphere(radius, segments, rings, smooth, limits)? })
    }
    /// A typed import operation for a capped cylinder mesh.
    pub fn cylinder(object: impl Into<String>, radius: f64, height: f64, segments: u32, smooth: bool, limits: &crate::Limits) -> crate::Result<Self> {
        Ok(Self::ImportMesh { object: object.into(), source: crate::primitives::cylinder(radius, height, segments, smooth, limits)? })
    }
    /// A typed import operation for a mesh built from positions and polygons.
    pub fn polygon_mesh(object: impl Into<String>, positions: &[[f64; 3]], polygons: &[crate::mesh::Polygon], limits: &crate::Limits) -> crate::Result<Self> {
        if positions.len() > limits.mesh.max_vertices || polygons.len() > limits.mesh.max_faces { return Err(crate::Error::Budget("polygon mesh size")); }
        let mut ctx = crate::mesh::Context::new(limits.mesh.clone(), None);
        Ok(Self::ImportMesh { object: object.into(), source: crate::mesh::Mesh::from_polygons(positions, polygons, &mut ctx)?.to_bytes(&mut ctx)? })
    }
}
