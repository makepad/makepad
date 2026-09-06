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
