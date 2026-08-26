//! `ModelData` — the L0 → L1 contract. This is the **only** shape the format
//! parser (`libs/fab`, lane L0) has to produce, and the only thing
//! [`crate::model::Scene::from_model`] consumes. Keep it boring: plain `Vec`s, no
//! lifetimes, no traits, no GPU types.
//!
//! Conventions the parser must honour (the scene layer validates and
//! normalises, it does not guess):
//! * `units.source_to_meters` scales every position and every `transform`
//!   translation. Fab streams are **meters** → `1.0` (the default); other
//!   sources declare their own factor.
//! * `up_axis` declares the source up axis; source application is Z-up. Y-up sources are
//!   rotated on import so the [`crate::model::Scene`] is always Z-up.
//! * `handedness` is declared, not assumed. Left-handed sources get their
//!   triangle winding flipped on import.
//! * Triangles only, CCW front faces after normalisation, `u32` indices.
//! * `normals`/`uvs` may be empty; missing normals are computed (flat) on
//!   import, missing uvs become `(0,0)`.
//! * Mesh positions are in the element's **local** space; `ElementData::transform`
//!   places them in the world. Parsers that only have world-space meshes set
//!   `transform` to identity.
//! * Every element with geometry references at least one [`MeshRef`] — a mesh
//!   plus its **instance transform** (Fab places shared meshes by instance;
//!   drawing meshes without their instance transforms piles them at the
//!   origin). An element without mesh refs is a pure hierarchy/zone node.

use crate::model::ids::{ElementId, LayerId, MaterialId, MeshId, StoryId};
use crate::model::sheets::Sheet;
use crate::model::units::Units;
use makepad_math::Mat4f;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UpAxis {
    #[default]
    Z,
    Y,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Handedness {
    #[default]
    Right,
    Left,
}

/// A whole parsed file.
#[derive(Clone, Debug, Default)]
pub struct ModelData {
    pub name: String,
    pub source_path: Option<PathBuf>,
    pub units: Units,
    pub up_axis: UpAxis,
    pub handedness: Handedness,
    pub meshes: Vec<MeshData>,
    pub materials: Vec<MaterialData>,
    /// Decoded images stored once; materials reference this array by index.
    pub textures: Vec<TextureData>,
    pub elements: Vec<ElementData>,
    pub stories: Vec<StoryData>,
    pub layers: Vec<LayerData>,
    /// 2D sheets / layouts, already in sheet millimeters (see [`Sheet`]).
    pub sheets: Vec<Sheet>,
    /// Published viewpoints, grouped as the source grouped them (Fab camera
    /// sets / galleries). Positions in source units; normalised on import.
    pub camera_sets: Vec<CameraSetData>,
    /// The source's default viewpoint, if it published one.
    pub home_camera: Option<CameraData>,
    /// Free-form file metadata (author, source application version, project name …).
    pub metadata: Vec<(String, String)>,
}

/// A published viewpoint. `yaw`/`pitch` are radians in the *source's* frame;
/// [`crate::model::scene::SceneCamera`] carries the normalised eye/forward pair.
#[derive(Clone, Debug, Default)]
pub struct CameraData {
    pub name: String,
    /// Eye position, source units.
    pub position: [f32; 3],
    /// Rotation about the up axis, radians. 0 looks along +Y.
    pub yaw: f32,
    /// Elevation above the horizon, radians, positive = up.
    pub pitch: f32,
    pub roll: f32,
    /// Vertical field of view in radians. 0 = unknown (the scene fills in 60°).
    pub fov_y: f32,
    pub perspective: bool,
    /// Sun azimuth/altitude the viewpoint was published with, degrees.
    /// `None` when the source did not publish one.
    pub sun: Option<[f32; 2]>,
    /// Sun clock as published, free text (`"2023-08-28T12:03:53"`).
    pub sun_date_time: String,
}

#[derive(Clone, Debug, Default)]
pub struct CameraSetData {
    pub name: String,
    pub cameras: Vec<CameraData>,
}

/// One indexed triangle mesh in element-local space.
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    /// Same length as `positions` or empty.
    pub normals: Vec<[f32; 3]>,
    /// Same length as `positions` or empty.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle list, 3 indices per triangle.
    pub indices: Vec<u32>,
    /// Contour edges (index pairs into `positions`) — the authored black
    /// architecture outlines Fab carries per mesh. Feed the hidden-line /
    /// ink mode. May be empty.
    pub contour_edges: Vec<u32>,
    /// Contiguous index ranges by material. Covers all of `indices` in order.
    /// Empty means "one submesh, `MaterialId::NONE`".
    pub submeshes: Vec<SubMesh>,
}

/// One placement of a mesh inside an element (instancing). The final local
/// transform of the geometry is `ElementData::transform * MeshRef::transform`.
#[derive(Clone, Debug)]
pub struct MeshRef {
    pub mesh: MeshId,
    pub transform: Mat4f,
    /// Overrides the mesh's own submesh materials for this placement. Fab
    /// shares one mesh between elements of different classes, so appearance
    /// that is decided per *element* has to travel on the placement, not on the
    /// mesh. `None` = use the mesh's submeshes.
    pub material: Option<MaterialId>,
}

impl MeshRef {
    pub fn identity(mesh: MeshId) -> MeshRef {
        MeshRef {
            mesh,
            transform: Mat4f::default(),
            material: None,
        }
    }

    pub fn with_material(mesh: MeshId, transform: Mat4f, material: MaterialId) -> MeshRef {
        MeshRef {
            mesh,
            transform,
            material: Some(material),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubMesh {
    pub material: MaterialId,
    pub first_index: u32,
    pub index_count: u32,
}

/// PBR-ish material as far as Fab carries it. Unknown fields keep their
/// defaults; the renderer never needs anything beyond this.
#[derive(Clone, Debug)]
pub struct MaterialData {
    pub name: String,
    /// Linear RGBA, alpha < 1 means transparent.
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// Linear RGB emission (radiance scale, 0 = none).
    pub emissive: [f32; 3],
    /// Index of refraction for transmissive materials (glass ≈ 1.5).
    pub ior: f32,
    /// 0 = opaque, 1 = fully transmissive (glass). Distinct from alpha.
    pub transmission: f32,
    pub double_sided: bool,
    /// Index into [`ModelData::textures`].
    pub texture: Option<usize>,
    /// UV scale applied to the texture, in repeats per meter, when the source
    /// declares one. `None` means uvs are already final.
    pub texture_repeat_per_meter: Option<[f32; 2]>,
}

impl Default for MaterialData {
    fn default() -> Self {
        MaterialData {
            name: String::new(),
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            emissive: [0.0; 3],
            ior: 1.45,
            transmission: 0.0,
            double_sided: false,
            texture: None,
            texture_repeat_per_meter: None,
        }
    }
}

/// Decoded RGBA8 image, top row first.
#[derive(Clone, Debug, Default)]
pub struct TextureData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// IFC-flavoured classification. `Other` keeps the source's own string.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum ElementClass {
    Wall,
    Slab,
    Roof,
    Shell,
    Column,
    Beam,
    Door,
    Window,
    Skylight,
    Opening,
    Stair,
    Railing,
    CurtainWall,
    Furniture,
    Object,
    Lamp,
    Morph,
    Mesh,
    Zone,
    Site,
    /// Pure grouping node (story, layer, group) with no own geometry.
    Group,
    #[default]
    Unknown,
    Other(String),
}

impl ElementClass {
    /// Stable short label for UI columns and icons.
    pub fn label(&self) -> &str {
        match self {
            ElementClass::Wall => "Wall",
            ElementClass::Slab => "Slab",
            ElementClass::Roof => "Roof",
            ElementClass::Shell => "Shell",
            ElementClass::Column => "Column",
            ElementClass::Beam => "Beam",
            ElementClass::Door => "Door",
            ElementClass::Window => "Window",
            ElementClass::Skylight => "Skylight",
            ElementClass::Opening => "Opening",
            ElementClass::Stair => "Stair",
            ElementClass::Railing => "Railing",
            ElementClass::CurtainWall => "Curtain Wall",
            ElementClass::Furniture => "Furniture",
            ElementClass::Object => "Object",
            ElementClass::Lamp => "Lamp",
            ElementClass::Morph => "Morph",
            ElementClass::Mesh => "Mesh",
            ElementClass::Zone => "Zone",
            ElementClass::Site => "Site",
            ElementClass::Group => "Group",
            ElementClass::Unknown => "Element",
            ElementClass::Other(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropertyValue {
    Text(String),
    Number(f64),
    Integer(i64),
    Bool(bool),
    /// Meters.
    Length(f64),
    /// Square meters.
    Area(f64),
    /// Cubic meters.
    Volume(f64),
    /// Degrees.
    Angle(f64),
}

/// A named attribute in a group ("General", "IFC", "Classification", …).
#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    pub group: String,
    pub name: String,
    pub value: PropertyValue,
}

/// A quantity take-off value (length/area/volume/count). Kept separate from
/// properties because the Quantities tab and schedules sum them.
#[derive(Clone, Debug, PartialEq)]
pub struct Quantity {
    pub name: String,
    pub value: PropertyValue,
}

#[derive(Clone, Debug, Default)]
pub struct ElementData {
    pub id: ElementId,
    /// Source GUID if present, else empty.
    pub guid: String,
    pub name: String,
    pub class: ElementClass,
    pub story: Option<StoryId>,
    pub layer: Option<LayerId>,
    pub parent: Option<ElementId>,
    /// Local → world (source units; translation is scaled on import).
    pub transform: Mat4f,
    pub meshes: Vec<MeshRef>,
    pub properties: Vec<Property>,
    pub quantities: Vec<Quantity>,
}

#[derive(Clone, Debug, Default)]
pub struct StoryData {
    pub id: StoryId,
    pub name: String,
    /// Floor level, source units (scaled on import).
    pub elevation: f32,
    /// Story height, source units, 0 if unknown.
    pub height: f32,
}

#[derive(Clone, Debug, Default)]
pub struct LayerData {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
}
