//! Editable, format-neutral scene graph shared by every Fab loader and editor.

use crate::model::{
    CameraData, CameraSetData, ElementClass, ElementData, Handedness, LayerData,
    MaterialData, MeshData, MeshRef, ModelData, Property as RuntimeProperty,
    PropertyValue as RuntimeValue, Quantity, StoryData, SubMesh, TextureData, Units, UpAxis,
};
use makepad_math::Mat4f;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);
    };
}

id_type!(ObjectId);
id_type!(CollectionId);
id_type!(MeshId);
id_type!(MaterialId);
id_type!(TextureId);

/// A typed value retained without teaching the core about source-specific data.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Number(f64),
    Bool(bool),
    Vec2([f64; 2]),
    Vec3([f64; 3]),
    Vec4([f64; 4]),
    Colour([f32; 4]),
    Enum(String),
}

impl Value {
    pub fn number(&self) -> Option<f64> {
        match self {
            Value::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Value::String(value) | Value::Enum(value) => Some(value),
            _ => None,
        }
    }
}

pub type PropertyBag = BTreeMap<String, Value>;

/// Local-to-parent object transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub matrix: Mat4f,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            matrix: Mat4f::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Texture {
    pub id: TextureId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgba8: Arc<[u8]>,
    pub properties: PropertyBag,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextureSlot {
    pub texture: TextureId,
    pub tex_coord: u32,
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

impl TextureSlot {
    pub fn new(texture: TextureId) -> Self {
        Self {
            texture,
            tex_coord: 0,
            scale: [1.0, 1.0],
            offset: [0.0, 0.0],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PbrMaterial {
    pub id: MaterialId,
    pub name: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub ior: f32,
    pub transmission: f32,
    pub double_sided: bool,
    pub base_color_texture: Option<TextureSlot>,
    pub normal_texture: Option<TextureSlot>,
    pub metallic_roughness_texture: Option<TextureSlot>,
    pub emissive_texture: Option<TextureSlot>,
    pub tags: Vec<String>,
    pub properties: PropertyBag,
}

impl PbrMaterial {
    pub fn new(id: MaterialId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            emissive: [0.0; 3],
            ior: 1.45,
            transmission: 0.0,
            double_sided: false,
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            emissive_texture: None,
            tags: Vec::new(),
            properties: PropertyBag::new(),
        }
    }
}

/// Indexed triangle mesh. Material slot values index `material_slots` per face.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub id: MeshId,
    pub name: String,
    pub positions: Arc<[[f32; 3]]>,
    pub normals: Arc<[[f32; 3]]>,
    pub uvs: Arc<[[f32; 2]]>,
    pub colours: Vec<[f32; 4]>,
    pub indices: Arc<[u32]>,
    pub material_slots: Vec<MaterialId>,
    pub face_material_slots: Vec<u32>,
    pub crease_edges: Vec<[u32; 2]>,
    pub tags: Vec<String>,
    pub properties: PropertyBag,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeshInstance {
    pub mesh: MeshId,
    pub transform: Transform,
    pub material_overrides: Vec<Option<MaterialId>>,
}

impl MeshInstance {
    pub fn new(mesh: MeshId) -> Self {
        Self {
            mesh,
            transform: Transform::default(),
            material_overrides: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    Directional,
    Point,
    Spot,
    Area,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    pub kind: LightKind,
    pub colour: [f32; 3],
    pub intensity: f32,
    pub range: Option<f32>,
    pub spot_angles: Option<[f32; 2]>,
    pub area_size: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub perspective: bool,
    pub fov_y: f32,
    pub orthographic_height: f32,
    pub near: f32,
    pub far: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub id: ObjectId,
    pub name: String,
    pub parent: Option<ObjectId>,
    pub transform: Transform,
    pub collections: Vec<CollectionId>,
    pub tags: Vec<String>,
    pub visible: bool,
    pub selected: bool,
    pub properties: PropertyBag,
    pub meshes: Vec<MeshInstance>,
    pub light: Option<Light>,
    pub camera: Option<Camera>,
}

impl Object {
    pub fn new(id: ObjectId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            parent: None,
            transform: Transform::default(),
            collections: Vec::new(),
            tags: Vec::new(),
            visible: true,
            selected: false,
            properties: PropertyBag::new(),
            meshes: Vec::new(),
            light: None,
            camera: None,
        }
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|candidate| candidate.eq_ignore_ascii_case(tag))
    }

    pub fn semantic_class(&self) -> Option<&str> {
        self.properties
            .get("arch.kind")
            .and_then(Value::text)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Collection {
    pub id: CollectionId,
    pub name: String,
    pub visible: bool,
    pub properties: PropertyBag,
}

impl Collection {
    pub fn new(id: CollectionId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            visible: true,
            properties: PropertyBag::new(),
        }
    }

    /// Architecture treats a collection with an elevation as a building level.
    pub fn level(&self) -> Option<f64> {
        self.properties
            .get("arch.elevation_m")
            .and_then(Value::number)
            .or_else(|| self.properties.get("arch.level").and_then(Value::number))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct DocumentState {
    objects: Vec<Object>,
    collections: Vec<Collection>,
    meshes: Vec<Mesh>,
    materials: Vec<PbrMaterial>,
    textures: Vec<Texture>,
}

/// Every mutation that can enter undo history.
#[derive(Clone, Debug)]
pub enum Edit {
    Transform { object: ObjectId, transform: Transform },
    Rename { object: ObjectId, name: String },
    SetProperty { object: ObjectId, key: String, value: Option<Value> },
    SetCollectionProperty {
        collection: CollectionId,
        key: String,
        value: Option<Value>,
    },
    SetMaterial {
        object: ObjectId,
        mesh: usize,
        slot: usize,
        material: Option<MaterialId>,
    },
    SetVisibility { object: ObjectId, visible: bool },
    SetSelection { object: ObjectId, selected: bool },
    SetTags { object: ObjectId, tags: Vec<String> },
    SetCollections { object: ObjectId, collections: Vec<CollectionId> },
    AddObject(Object),
    RemoveObject(ObjectId),
    /// Backwards-compatible spelling of `RemoveObject`.
    Delete(ObjectId),
    AddCollection(Collection),
    RemoveCollection(CollectionId),
    AddMesh(Mesh),
    RemoveMesh(MeshId),
    AddMaterial(PbrMaterial),
    RemoveMaterial(MaterialId),
    AddTexture(Texture),
    RemoveTexture(TextureId),
    #[doc(hidden)]
    RestoreMaterialOverride {
        object: ObjectId,
        mesh: usize,
        slot: usize,
        material: Option<MaterialId>,
        len: usize,
    },
    #[doc(hidden)]
    RestoreObject {
        index: usize,
        object: Object,
        children: Vec<ObjectId>,
    },
    #[doc(hidden)]
    RestoreCollection { index: usize, collection: Collection },
    #[doc(hidden)]
    RestoreMesh { index: usize, mesh: Mesh },
    #[doc(hidden)]
    RestoreMaterial { index: usize, material: PbrMaterial },
    #[doc(hidden)]
    RestoreTexture { index: usize, texture: Texture },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    MissingObject(ObjectId),
    MissingCollection(CollectionId),
    MissingMesh(MeshId),
    MissingMaterial(MaterialId),
    MissingTexture(TextureId),
    DuplicateId(&'static str, u64),
    MissingMeshInstance { object: ObjectId, index: usize },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for EditError {}

/// The canonical editable document. Optimized render scenes are projections of it.
#[derive(Clone, Debug)]
pub struct Document {
    name: String,
    source_path: Option<PathBuf>,
    units: Units,
    up_axis: UpAxis,
    handedness: Handedness,
    metadata: PropertyBag,
    state: DocumentState,
    undo_depth: usize,
    undo: VecDeque<Edit>,
    redo: VecDeque<Edit>,
}

impl Document {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source_path: None,
            units: Units::default(),
            up_axis: UpAxis::Z,
            handedness: Handedness::Right,
            metadata: PropertyBag::new(),
            state: DocumentState::default(),
            undo_depth: Self::UNDO_DEPTH,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
        }
    }

    pub fn name(&self) -> &str { &self.name }
    pub fn source_path(&self) -> Option<&std::path::Path> { self.source_path.as_deref() }
    pub fn units(&self) -> Units { self.units }
    pub fn up_axis(&self) -> UpAxis { self.up_axis }
    pub fn handedness(&self) -> Handedness { self.handedness }
    pub fn metadata(&self) -> &PropertyBag { &self.metadata }
    pub fn objects(&self) -> &[Object] { &self.state.objects }
    pub fn collections(&self) -> &[Collection] { &self.state.collections }
    pub fn meshes(&self) -> &[Mesh] { &self.state.meshes }
    pub fn materials(&self) -> &[PbrMaterial] { &self.state.materials }
    pub fn textures(&self) -> &[Texture] { &self.state.textures }

    /// Live material edit (colour picker): base colour by materials-list
    /// index — the same order the runtime scene's material table uses.
    /// Deliberately not an undo step: the picker's drag publishes dozens of
    /// values a second and owns its own revert (Escape).
    pub fn set_material_base_color_by_index(&mut self, index: usize, rgba: [f32; 4]) -> bool {
        match self.state.materials.get_mut(index) {
            Some(m) => {
                m.base_color = rgba;
                true
            }
            None => false,
        }
    }
    pub fn can_undo(&self) -> bool { !self.undo.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo.is_empty() }

    pub fn object(&self, id: ObjectId) -> Option<&Object> {
        self.state.objects.iter().find(|object| object.id == id)
    }

    pub fn collection(&self, id: CollectionId) -> Option<&Collection> {
        self.state.collections.iter().find(|collection| collection.id == id)
    }

    pub fn mesh(&self, id: MeshId) -> Option<&Mesh> {
        self.state.meshes.iter().find(|mesh| mesh.id == id)
    }

    /// Default maximum number of inverse edits retained for undo.
    pub const UNDO_DEPTH: usize = 256;

    pub fn undo_depth(&self) -> usize {
        self.undo_depth
    }

    pub fn set_undo_depth(&mut self, undo_depth: usize) {
        self.undo_depth = undo_depth;
        self.trim_history();
    }

    pub fn apply(&mut self, edit: Edit) -> Result<(), EditError> {
        let inverse = self.apply_and_invert(edit)?;
        if self.undo_depth != 0 {
            self.undo.push_back(inverse);
            self.trim_history();
        }
        self.redo.clear();
        Ok(())
    }

    /// Apply without recording history: what a loader's builder does while
    /// it assembles the document. Recording every added mesh used to clone
    /// the whole growing document twice per mesh — quadratic time and
    /// ~100 GB of retained snapshots on a 1200-mesh building.
    pub(crate) fn apply_untracked(&mut self, edit: Edit) -> Result<(), EditError> {
        self.apply_and_invert(edit).map(drop)
    }

    pub fn undo(&mut self) -> Option<&Edit> {
        let inverse = self.undo.pop_back()?;
        let forward = self
            .apply_and_invert(inverse)
            .expect("an undo inverse must remain valid");
        self.redo.push_back(forward);
        self.redo.back()
    }

    pub fn redo(&mut self) -> Option<&Edit> {
        let forward = self.redo.pop_back()?;
        let inverse = self
            .apply_and_invert(forward)
            .expect("a redo edit must remain valid");
        self.undo.push_back(inverse);
        self.trim_history();
        self.undo.back()
    }

    fn trim_history(&mut self) {
        while self.undo.len() > self.undo_depth {
            self.undo.pop_front();
        }
        while self.redo.len() > self.undo_depth {
            self.redo.pop_front();
        }
    }

    fn apply_and_invert(&mut self, edit: Edit) -> Result<Edit, EditError> {
        match edit {
            Edit::Transform { object, transform } => {
                let old = std::mem::replace(&mut self.object_mut(object)?.transform, transform);
                Ok(Edit::Transform { object, transform: old })
            }
            Edit::Rename { object, name } => {
                let old = std::mem::replace(&mut self.object_mut(object)?.name, name);
                Ok(Edit::Rename { object, name: old })
            }
            Edit::SetProperty { object, key, value } => {
                let old = replace_property(&mut self.object_mut(object)?.properties, &key, value);
                Ok(Edit::SetProperty { object, key, value: old })
            }
            Edit::SetCollectionProperty { collection, key, value } => {
                let collection = self
                    .state
                    .collections
                    .iter_mut()
                    .find(|candidate| candidate.id == collection)
                    .ok_or(EditError::MissingCollection(collection))?;
                let old = replace_property(&mut collection.properties, &key, value);
                Ok(Edit::SetCollectionProperty {
                    collection: collection.id,
                    key,
                    value: old,
                })
            }
            Edit::SetMaterial { object, mesh, slot, material } => {
                if let Some(material) = material {
                    if !self.state.materials.iter().any(|candidate| candidate.id == material) {
                        return Err(EditError::MissingMaterial(material));
                    }
                }
                let instance = self
                    .object_mut(object)?
                    .meshes
                    .get_mut(mesh)
                    .ok_or(EditError::MissingMeshInstance { object, index: mesh })?;
                let old_len = instance.material_overrides.len();
                let old = instance.material_overrides.get(slot).copied().unwrap_or(None);
                if old_len <= slot {
                    instance.material_overrides.resize(slot + 1, None);
                }
                instance.material_overrides[slot] = material;
                Ok(Edit::RestoreMaterialOverride {
                    object,
                    mesh,
                    slot,
                    material: old,
                    len: old_len,
                })
            }
            Edit::RestoreMaterialOverride { object, mesh, slot, material, len } => {
                if let Some(material) = material {
                    if !self.state.materials.iter().any(|candidate| candidate.id == material) {
                        return Err(EditError::MissingMaterial(material));
                    }
                }
                let instance = self
                    .object_mut(object)?
                    .meshes
                    .get_mut(mesh)
                    .ok_or(EditError::MissingMeshInstance { object, index: mesh })?;
                let old_len = instance.material_overrides.len();
                let old = instance.material_overrides.get(slot).copied().unwrap_or(None);
                instance.material_overrides.resize(len, None);
                if slot < len {
                    instance.material_overrides[slot] = material;
                }
                Ok(Edit::RestoreMaterialOverride {
                    object,
                    mesh,
                    slot,
                    material: old,
                    len: old_len,
                })
            }
            Edit::SetVisibility { object, visible } => {
                let old = std::mem::replace(&mut self.object_mut(object)?.visible, visible);
                Ok(Edit::SetVisibility { object, visible: old })
            }
            Edit::SetSelection { object, selected } => {
                let old = std::mem::replace(&mut self.object_mut(object)?.selected, selected);
                Ok(Edit::SetSelection { object, selected: old })
            }
            Edit::SetTags { object, tags } => {
                let old = std::mem::replace(&mut self.object_mut(object)?.tags, tags);
                Ok(Edit::SetTags { object, tags: old })
            }
            Edit::SetCollections { object, collections } => {
                for collection in &collections {
                    if !self.state.collections.iter().any(|candidate| candidate.id == *collection) {
                        return Err(EditError::MissingCollection(*collection));
                    }
                }
                let old = std::mem::replace(&mut self.object_mut(object)?.collections, collections);
                Ok(Edit::SetCollections { object, collections: old })
            }
            Edit::AddObject(object) => {
                ensure_unique(
                    self.state.objects.iter().map(|candidate| candidate.id.0),
                    object.id.0,
                    "object",
                )?;
                for instance in &object.meshes {
                    if !self.state.meshes.iter().any(|candidate| candidate.id == instance.mesh) {
                        return Err(EditError::MissingMesh(instance.mesh));
                    }
                }
                let id = object.id;
                self.state.objects.push(object);
                Ok(Edit::RemoveObject(id))
            }
            Edit::RemoveObject(object) | Edit::Delete(object) => {
                let index = self
                    .state
                    .objects
                    .iter()
                    .position(|candidate| candidate.id == object)
                    .ok_or(EditError::MissingObject(object))?;
                let removed = self.state.objects.remove(index);
                let mut children = Vec::new();
                for child in &mut self.state.objects {
                    if child.parent == Some(object) {
                        children.push(child.id);
                        child.parent = None;
                    }
                }
                Ok(Edit::RestoreObject { index, object: removed, children })
            }
            Edit::RestoreObject { index, object, children } => {
                ensure_unique(
                    self.state.objects.iter().map(|candidate| candidate.id.0),
                    object.id.0,
                    "object",
                )?;
                for instance in &object.meshes {
                    if !self.state.meshes.iter().any(|candidate| candidate.id == instance.mesh) {
                        return Err(EditError::MissingMesh(instance.mesh));
                    }
                }
                for child in &children {
                    if !self.state.objects.iter().any(|candidate| candidate.id == *child) {
                        return Err(EditError::MissingObject(*child));
                    }
                }
                let id = object.id;
                debug_assert!(index <= self.state.objects.len());
                self.state.objects.insert(index.min(self.state.objects.len()), object);
                for child in children {
                    self.object_mut(child)?.parent = Some(id);
                }
                Ok(Edit::RemoveObject(id))
            }
            Edit::AddCollection(collection) => {
                ensure_unique(
                    self.state.collections.iter().map(|candidate| candidate.id.0),
                    collection.id.0,
                    "collection",
                )?;
                let id = collection.id;
                self.state.collections.push(collection);
                Ok(Edit::RemoveCollection(id))
            }
            Edit::RemoveCollection(collection) => {
                let index = self
                    .state
                    .collections
                    .iter()
                    .position(|candidate| candidate.id == collection)
                    .ok_or(EditError::MissingCollection(collection))?;
                let collection = self.state.collections.remove(index);
                Ok(Edit::RestoreCollection { index, collection })
            }
            Edit::RestoreCollection { index, collection } => {
                ensure_unique(
                    self.state.collections.iter().map(|candidate| candidate.id.0),
                    collection.id.0,
                    "collection",
                )?;
                let id = collection.id;
                debug_assert!(index <= self.state.collections.len());
                self.state.collections.insert(index.min(self.state.collections.len()), collection);
                Ok(Edit::RemoveCollection(id))
            }
            Edit::AddMesh(mesh) => {
                ensure_unique(
                    self.state.meshes.iter().map(|candidate| candidate.id.0),
                    mesh.id.0,
                    "mesh",
                )?;
                let id = mesh.id;
                self.state.meshes.push(mesh);
                Ok(Edit::RemoveMesh(id))
            }
            Edit::RemoveMesh(mesh) => {
                let index = self
                    .state
                    .meshes
                    .iter()
                    .position(|candidate| candidate.id == mesh)
                    .ok_or(EditError::MissingMesh(mesh))?;
                let mesh = self.state.meshes.remove(index);
                Ok(Edit::RestoreMesh { index, mesh })
            }
            Edit::RestoreMesh { index, mesh } => {
                ensure_unique(
                    self.state.meshes.iter().map(|candidate| candidate.id.0),
                    mesh.id.0,
                    "mesh",
                )?;
                let id = mesh.id;
                debug_assert!(index <= self.state.meshes.len());
                self.state.meshes.insert(index.min(self.state.meshes.len()), mesh);
                Ok(Edit::RemoveMesh(id))
            }
            Edit::AddMaterial(material) => {
                ensure_unique(
                    self.state.materials.iter().map(|candidate| candidate.id.0),
                    material.id.0,
                    "material",
                )?;
                let id = material.id;
                self.state.materials.push(material);
                Ok(Edit::RemoveMaterial(id))
            }
            Edit::RemoveMaterial(material) => {
                let index = self
                    .state
                    .materials
                    .iter()
                    .position(|candidate| candidate.id == material)
                    .ok_or(EditError::MissingMaterial(material))?;
                let material = self.state.materials.remove(index);
                Ok(Edit::RestoreMaterial { index, material })
            }
            Edit::RestoreMaterial { index, material } => {
                ensure_unique(
                    self.state.materials.iter().map(|candidate| candidate.id.0),
                    material.id.0,
                    "material",
                )?;
                let id = material.id;
                debug_assert!(index <= self.state.materials.len());
                self.state.materials.insert(index.min(self.state.materials.len()), material);
                Ok(Edit::RemoveMaterial(id))
            }
            Edit::AddTexture(texture) => {
                ensure_unique(
                    self.state.textures.iter().map(|candidate| candidate.id.0),
                    texture.id.0,
                    "texture",
                )?;
                let id = texture.id;
                self.state.textures.push(texture);
                Ok(Edit::RemoveTexture(id))
            }
            Edit::RemoveTexture(texture) => {
                let index = self
                    .state
                    .textures
                    .iter()
                    .position(|candidate| candidate.id == texture)
                    .ok_or(EditError::MissingTexture(texture))?;
                let texture = self.state.textures.remove(index);
                Ok(Edit::RestoreTexture { index, texture })
            }
            Edit::RestoreTexture { index, texture } => {
                ensure_unique(
                    self.state.textures.iter().map(|candidate| candidate.id.0),
                    texture.id.0,
                    "texture",
                )?;
                let id = texture.id;
                debug_assert!(index <= self.state.textures.len());
                self.state.textures.insert(index.min(self.state.textures.len()), texture);
                Ok(Edit::RemoveTexture(id))
            }
        }
    }

    fn object_mut(&mut self, id: ObjectId) -> Result<&mut Object, EditError> {
        self.state
            .objects
            .iter_mut()
            .find(|object| object.id == id)
            .ok_or(EditError::MissingObject(id))
    }

    /// Compatibility importer for parsers being moved to the document seam.
    pub fn from_model_data(model: ModelData) -> Self {
        DocumentBuilder::from_model_data(model).finish()
    }

    /// Project the editable graph into the packed scene builder's plain input.
    pub(crate) fn into_model_data(self) -> ModelData {
        let object_ids: HashMap<ObjectId, crate::model::ElementId> = self
            .state
            .objects
            .iter()
            .enumerate()
            .map(|(index, object)| (object.id, crate::model::ElementId::from_index(index)))
            .collect();
        let level_collections: Vec<&Collection> = self
            .state
            .collections
            .iter()
            .filter(|collection| collection.level().is_some())
            .collect();
        let layer_collections: Vec<&Collection> = self
            .state
            .collections
            .iter()
            .filter(|collection| collection.level().is_none())
            .collect();
        let story_ids: HashMap<CollectionId, crate::model::StoryId> = level_collections
            .iter()
            .enumerate()
            .map(|(index, collection)| (collection.id, crate::model::StoryId::from_index(index)))
            .collect();
        let layer_ids: HashMap<CollectionId, crate::model::LayerId> = layer_collections
            .iter()
            .enumerate()
            .map(|(index, collection)| (collection.id, crate::model::LayerId::from_index(index)))
            .collect();
        let mesh_ids: HashMap<MeshId, crate::model::MeshId> = self
            .state
            .meshes
            .iter()
            .enumerate()
            .map(|(index, mesh)| (mesh.id, crate::model::MeshId::from_index(index)))
            .collect();
        let material_ids: HashMap<MaterialId, crate::model::MaterialId> = self
            .state
            .materials
            .iter()
            .enumerate()
            .map(|(index, material)| (material.id, crate::model::MaterialId::from_index(index)))
            .collect();
        let texture_index_by_id: HashMap<TextureId, usize> = self
            .state
            .textures
            .iter()
            .enumerate()
            .map(|(index, texture)| (texture.id, index))
            .collect();
        let textures: Vec<TextureData> = self
            .state
            .textures
            .iter()
            .map(|texture| TextureData {
                width: texture.width,
                height: texture.height,
                rgba: texture.rgba8.to_vec(),
            })
            .collect();

        let materials = self
            .state
            .materials
            .iter()
            .map(|material| MaterialData {
                name: material.name.clone(),
                base_color: material.base_color,
                metallic: material.metallic,
                roughness: material.roughness,
                emissive: material.emissive,
                ior: material.ior,
                transmission: material.transmission,
                double_sided: material.double_sided,
                texture: material
                    .base_color_texture
                    .as_ref()
                    .and_then(|slot| texture_index_by_id.get(&slot.texture).copied()),
                texture_repeat_per_meter: material.base_color_texture.as_ref().map(|slot| slot.scale),
            })
            .collect();

        let meshes = self
            .state
            .meshes
            .iter()
            .map(|mesh| MeshData {
                positions: mesh.positions.to_vec(),
                normals: mesh.normals.to_vec(),
                uvs: mesh.uvs.to_vec(),
                indices: mesh.indices.to_vec(),
                contour_edges: mesh
                    .crease_edges
                    .iter()
                    .flat_map(|edge| edge.iter().copied())
                    .collect(),
                submeshes: material_runs(mesh, &material_ids),
            })
            .collect();

        let elements = self
            .state
            .objects
            .iter()
            .map(|object| {
                let class = runtime_class(object.semantic_class());
                let properties = object
                    .properties
                    .iter()
                    .map(|(name, value)| RuntimeProperty {
                        group: "Properties".to_string(),
                        name: name.clone(),
                        value: runtime_value(value),
                    })
                    .collect();
                let story = object.collections.iter().find_map(|id| story_ids.get(id).copied());
                let layer = object.collections.iter().find_map(|id| layer_ids.get(id).copied());
                let meshes = object
                    .meshes
                    .iter()
                    .filter_map(|instance| {
                        let mesh = *mesh_ids.get(&instance.mesh)?;
                        let material = instance
                            .material_overrides
                            .first()
                            .and_then(|material| material.as_ref())
                            .and_then(|material| material_ids.get(material))
                            .copied();
                        Some(MeshRef {
                            mesh,
                            transform: instance.transform.matrix,
                            material,
                        })
                    })
                    .collect();
                ElementData {
                    id: object_ids[&object.id],
                    guid: object
                        .properties
                        .get("arch.id")
                        .and_then(Value::text)
                        .unwrap_or_default()
                        .to_string(),
                    name: object.name.clone(),
                    class,
                    story,
                    layer,
                    parent: object.parent.and_then(|parent| object_ids.get(&parent).copied()),
                    transform: object.transform.matrix,
                    meshes,
                    properties,
                    quantities: Vec::new(),
                }
            })
            .collect();

        let stories = level_collections
            .iter()
            .enumerate()
            .map(|(index, collection)| StoryData {
                id: crate::model::StoryId::from_index(index),
                name: collection.name.clone(),
                elevation: collection.level().unwrap_or(0.0) as f32,
                height: collection
                    .properties
                    .get("arch.height_m")
                    .and_then(Value::number)
                    .unwrap_or(0.0) as f32,
            })
            .collect();
        let layers = layer_collections
            .iter()
            .enumerate()
            .map(|(index, collection)| LayerData {
                id: crate::model::LayerId::from_index(index),
                name: collection.name.clone(),
                visible: collection.visible,
            })
            .collect();
        let cameras: Vec<(bool, CameraData)> = self
            .state
            .objects
            .iter()
            .filter_map(|object| {
                let camera = object.camera.as_ref()?;
                Some((
                    object.has_tag("home-camera"),
                    CameraData {
                        name: object.name.clone(),
                        position: [
                            object.transform.matrix.v[12],
                            object.transform.matrix.v[13],
                            object.transform.matrix.v[14],
                        ],
                        yaw: object
                            .properties
                            .get("yaw")
                            .and_then(Value::number)
                            .unwrap_or(0.0) as f32,
                        pitch: object
                            .properties
                            .get("pitch")
                            .and_then(Value::number)
                            .unwrap_or(0.0) as f32,
                        roll: object
                            .properties
                            .get("roll")
                            .and_then(Value::number)
                            .unwrap_or(0.0) as f32,
                        fov_y: camera.fov_y,
                        perspective: camera.perspective,
                        ..Default::default()
                    },
                ))
            })
            .collect();
        let home_camera = cameras
            .iter()
            .find(|(home, _)| *home)
            .map(|(_, camera)| camera.clone());
        let published_cameras: Vec<_> = cameras
            .iter()
            .filter(|(home, _)| !*home)
            .map(|(_, camera)| camera.clone())
            .collect();

        ModelData {
            name: self.name,
            source_path: self.source_path,
            units: self.units,
            up_axis: self.up_axis,
            handedness: self.handedness,
            meshes,
            materials,
            textures,
            elements,
            stories,
            layers,
            camera_sets: if published_cameras.is_empty() {
                Vec::new()
            } else {
                vec![CameraSetData {
                    name: "Cameras".to_string(),
                    cameras: published_cameras,
                }]
            },
            home_camera,
            metadata: self
                .metadata
                .into_iter()
                .map(|(key, value)| (key, value_string(&value)))
                .collect(),
            ..Default::default()
        }
    }
}

fn replace_property(
    properties: &mut PropertyBag,
    key: &str,
    value: Option<Value>,
) -> Option<Value> {
    match value {
        Some(value) => properties.insert(key.to_string(), value),
        None => properties.remove(key),
    }
}

fn ensure_unique(
    ids: impl Iterator<Item = u64>,
    id: u64,
    kind: &'static str,
) -> Result<(), EditError> {
    if ids.into_iter().any(|candidate| candidate == id) {
        Err(EditError::DuplicateId(kind, id))
    } else {
        Ok(())
    }
}

/// Loader-oriented façade. It uses the same edits as interactive tools, then
/// drops construction history when the immutable import transaction finishes.
pub struct DocumentBuilder {
    document: Document,
    next_object: u64,
    next_collection: u64,
    next_mesh: u64,
    next_material: u64,
    next_texture: u64,
}

impl DocumentBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            document: Document::new(name),
            next_object: 1,
            next_collection: 1,
            next_mesh: 1,
            next_material: 1,
            next_texture: 1,
        }
    }

    pub fn source_path(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.document.source_path = Some(path.into());
        self
    }

    pub fn coordinates(&mut self, units: Units, up_axis: UpAxis, handedness: Handedness) -> &mut Self {
        self.document.units = units;
        self.document.up_axis = up_axis;
        self.document.handedness = handedness;
        self
    }

    pub fn metadata(&mut self, key: impl Into<String>, value: Value) -> &mut Self {
        self.document.metadata.insert(key.into(), value);
        self
    }

    pub fn add_texture(&mut self, mut texture: Texture) -> TextureId {
        let id = TextureId(self.next_texture);
        self.next_texture += 1;
        texture.id = id;
        self.document.apply_untracked(Edit::AddTexture(texture)).expect("builder texture id is unique");
        id
    }

    pub fn add_material(&mut self, mut material: PbrMaterial) -> MaterialId {
        let id = MaterialId(self.next_material);
        self.next_material += 1;
        material.id = id;
        self.document.apply_untracked(Edit::AddMaterial(material)).expect("builder material id is unique");
        id
    }

    pub fn add_mesh(&mut self, mut mesh: Mesh) -> MeshId {
        let id = MeshId(self.next_mesh);
        self.next_mesh += 1;
        mesh.id = id;
        self.document.apply_untracked(Edit::AddMesh(mesh)).expect("builder mesh id is unique");
        id
    }

    pub fn add_collection(&mut self, mut collection: Collection) -> CollectionId {
        let id = CollectionId(self.next_collection);
        self.next_collection += 1;
        collection.id = id;
        self.document
            .apply_untracked(Edit::AddCollection(collection))
            .expect("builder collection id is unique");
        id
    }

    pub fn add_object(&mut self, mut object: Object) -> ObjectId {
        let id = ObjectId(self.next_object);
        self.next_object += 1;
        object.id = id;
        self.document.apply_untracked(Edit::AddObject(object)).expect("builder object is valid");
        id
    }

    pub fn finish(mut self) -> Document {
        self.document.undo.clear();
        self.document.redo.clear();
        self.document
    }

    fn from_model_data(model: ModelData) -> Self {
        let mut builder = DocumentBuilder::new(model.name.clone());
        if let Some(path) = model.source_path.clone() {
            builder.source_path(path);
        }
        builder.coordinates(model.units, model.up_axis, model.handedness);
        for (key, value) in &model.metadata {
            builder.metadata(key.clone(), Value::String(value.clone()));
        }

        let texture_ids: Vec<TextureId> = model
            .textures
            .iter()
            .enumerate()
            .map(|(index, texture)| {
                builder.add_texture(Texture {
                    name: format!("texture {index}"),
                    width: texture.width,
                    height: texture.height,
                    rgba8: texture.rgba.clone().into(),
                    ..Default::default()
                })
            })
            .collect();
        let mut material_ids = Vec::with_capacity(model.materials.len());
        for material in &model.materials {
            let mut out = PbrMaterial::new(MaterialId::default(), material.name.clone());
            out.base_color = material.base_color;
            out.metallic = material.metallic;
            out.roughness = material.roughness;
            out.emissive = material.emissive;
            out.ior = material.ior;
            out.transmission = material.transmission;
            out.double_sided = material.double_sided;
            out.base_color_texture = material.texture.and_then(|index| texture_ids.get(index).copied()).map(|texture| {
                let mut slot = TextureSlot::new(texture);
                slot.scale = material.texture_repeat_per_meter.unwrap_or([1.0, 1.0]);
                slot
            });
            material_ids.push(builder.add_material(out));
        }

        let mut mesh_ids = Vec::with_capacity(model.meshes.len());
        for mesh in &model.meshes {
            let mut slots = Vec::new();
            let mut faces = vec![0; mesh.indices.len() / 3];
            for submesh in &mesh.submeshes {
                let Some(material) = material_ids.get(submesh.material.index()).copied() else { continue };
                let slot = slots.iter().position(|candidate| *candidate == material).unwrap_or_else(|| {
                    slots.push(material);
                    slots.len() - 1
                });
                let first = submesh.first_index as usize / 3;
                let end = first + submesh.index_count as usize / 3;
                for face in first..end.min(faces.len()) {
                    faces[face] = slot as u32;
                }
            }
            mesh_ids.push(builder.add_mesh(Mesh {
                positions: mesh.positions.clone().into(),
                normals: mesh.normals.clone().into(),
                uvs: mesh.uvs.clone().into(),
                indices: mesh.indices.clone().into(),
                material_slots: slots,
                face_material_slots: faces,
                crease_edges: mesh
                    .contour_edges
                    .chunks_exact(2)
                    .map(|edge| [edge[0], edge[1]])
                    .collect(),
                ..Default::default()
            }));
        }

        let mut story_collections = Vec::with_capacity(model.stories.len());
        for story in &model.stories {
            let mut collection = Collection::new(CollectionId::default(), story.name.clone());
            collection.properties.insert("arch.level".to_string(), Value::Number(story.id.0 as f64));
            collection.properties.insert("arch.elevation_m".to_string(), Value::Number(story.elevation as f64));
            collection.properties.insert("arch.height_m".to_string(), Value::Number(story.height as f64));
            story_collections.push(builder.add_collection(collection));
        }
        let mut layer_collections = Vec::with_capacity(model.layers.len());
        for layer in &model.layers {
            let mut collection = Collection::new(CollectionId::default(), layer.name.clone());
            collection.visible = layer.visible;
            layer_collections.push(builder.add_collection(collection));
        }

        let object_ids: Vec<ObjectId> = (0..model.elements.len())
            .map(|index| ObjectId(index as u64 + 1))
            .collect();
        for element in &model.elements {
            let mut object = Object::new(ObjectId::default(), element.name.clone());
            object.parent = element.parent.and_then(|parent| object_ids.get(parent.index()).copied());
            object.transform.matrix = element.transform;
            let class = element.class.label().to_ascii_lowercase();
            object.tags.push(class.clone());
            object.properties.insert("arch.kind".to_string(), Value::Enum(class));
            if !element.guid.is_empty() {
                object.properties.insert("arch.id".to_string(), Value::String(element.guid.clone()));
            }
            if let Some(story) = element.story.and_then(|id| story_collections.get(id.index())).copied() {
                object.collections.push(story);
            }
            if let Some(layer) = element.layer.and_then(|id| layer_collections.get(id.index())).copied() {
                object.collections.push(layer);
            }
            for property in &element.properties {
                let key = if property.group.is_empty() {
                    property.name.clone()
                } else {
                    format!("{}.{}", property.group, property.name)
                };
                object.properties.insert(key, document_value(&property.value));
            }
            for Quantity { name, value } in &element.quantities {
                object.properties.insert(format!("quantity.{name}"), document_value(value));
            }
            object.meshes = element
                .meshes
                .iter()
                .filter_map(|instance| {
                    let mesh = *mesh_ids.get(instance.mesh.index())?;
                    Some(MeshInstance {
                        mesh,
                        transform: Transform { matrix: instance.transform },
                        material_overrides: instance
                            .material
                            .and_then(|material| material_ids.get(material.index()).copied())
                            .map(|material| vec![Some(material)])
                            .unwrap_or_default(),
                    })
                })
                .collect();
            builder.add_object(object);
        }
        for camera_set in &model.camera_sets {
            for camera in &camera_set.cameras {
                builder.add_camera_object(camera, Some(&camera_set.name), false);
            }
        }
        if let Some(camera) = &model.home_camera {
            builder.add_camera_object(camera, None, true);
        }
        builder
    }

    fn add_camera_object(&mut self, camera: &CameraData, set: Option<&str>, home: bool) {
        let mut object = Object::new(ObjectId::default(), camera.name.clone());
        object.transform.matrix.v[12] = camera.position[0];
        object.transform.matrix.v[13] = camera.position[1];
        object.transform.matrix.v[14] = camera.position[2];
        object.tags.push("camera".to_string());
        if home {
            object.tags.push("home-camera".to_string());
        }
        if let Some(set) = set {
            object
                .properties
                .insert("camera_set".to_string(), Value::String(set.to_string()));
        }
        object
            .properties
            .insert("yaw".to_string(), Value::Number(camera.yaw as f64));
        object
            .properties
            .insert("pitch".to_string(), Value::Number(camera.pitch as f64));
        object
            .properties
            .insert("roll".to_string(), Value::Number(camera.roll as f64));
        object.camera = Some(Camera {
            perspective: camera.perspective,
            fov_y: camera.fov_y,
            orthographic_height: 0.0,
            near: 0.01,
            far: 10_000.0,
        });
        self.add_object(object);
    }
}

fn material_runs(mesh: &Mesh, material_ids: &HashMap<MaterialId, crate::model::MaterialId>) -> Vec<SubMesh> {
    if mesh.indices.is_empty() || mesh.material_slots.is_empty() {
        return Vec::new();
    }
    let face_count = mesh.indices.len() / 3;
    let slot_at = |face: usize| mesh.face_material_slots.get(face).copied().unwrap_or(0) as usize;
    let mut runs = Vec::new();
    let mut first = 0usize;
    let mut slot = slot_at(0);
    for face in 1..face_count {
        let next = slot_at(face);
        if next != slot {
            if let Some(material) = mesh.material_slots.get(slot).and_then(|id| material_ids.get(id)).copied() {
                runs.push(SubMesh {
                    material,
                    first_index: (first * 3) as u32,
                    index_count: ((face - first) * 3) as u32,
                });
            }
            first = face;
            slot = next;
        }
    }
    if let Some(material) = mesh.material_slots.get(slot).and_then(|id| material_ids.get(id)).copied() {
        runs.push(SubMesh {
            material,
            first_index: (first * 3) as u32,
            index_count: ((face_count - first) * 3) as u32,
        });
    }
    runs
}

fn runtime_class(class: Option<&str>) -> ElementClass {
    match class.unwrap_or_default().to_ascii_lowercase().as_str() {
        "wall" => ElementClass::Wall,
        "slab" => ElementClass::Slab,
        "roof" => ElementClass::Roof,
        "shell" => ElementClass::Shell,
        "column" => ElementClass::Column,
        "beam" => ElementClass::Beam,
        "door" => ElementClass::Door,
        "window" => ElementClass::Window,
        "skylight" => ElementClass::Skylight,
        "opening" => ElementClass::Opening,
        "stair" => ElementClass::Stair,
        "railing" => ElementClass::Railing,
        "curtain wall" | "curtainwall" => ElementClass::CurtainWall,
        "furniture" => ElementClass::Furniture,
        "object" => ElementClass::Object,
        "lamp" | "light" => ElementClass::Lamp,
        "morph" => ElementClass::Morph,
        "zone" | "room" => ElementClass::Zone,
        "site" => ElementClass::Site,
        "group" | "collection" => ElementClass::Group,
        "mesh" => ElementClass::Mesh,
        "" => ElementClass::Unknown,
        other => ElementClass::Other(other.to_string()),
    }
}

fn document_value(value: &RuntimeValue) -> Value {
    match value {
        RuntimeValue::Text(value) => Value::String(value.clone()),
        RuntimeValue::Number(value)
        | RuntimeValue::Length(value)
        | RuntimeValue::Area(value)
        | RuntimeValue::Volume(value)
        | RuntimeValue::Angle(value) => Value::Number(*value),
        RuntimeValue::Integer(value) => Value::Number(*value as f64),
        RuntimeValue::Bool(value) => Value::Bool(*value),
    }
}

fn runtime_value(value: &Value) -> RuntimeValue {
    match value {
        Value::String(value) | Value::Enum(value) => RuntimeValue::Text(value.clone()),
        Value::Number(value) => RuntimeValue::Number(*value),
        Value::Bool(value) => RuntimeValue::Bool(*value),
        Value::Vec2(value) => RuntimeValue::Text(format!("{}, {}", value[0], value[1])),
        Value::Vec3(value) => RuntimeValue::Text(format!("{}, {}, {}", value[0], value[1], value[2])),
        Value::Vec4(value) => RuntimeValue::Text(format!("{}, {}, {}, {}", value[0], value[1], value[2], value[3])),
        Value::Colour(value) => RuntimeValue::Text(format!("{}, {}, {}, {}", value[0], value[1], value[2], value[3])),
    }
}

fn value_string(value: &Value) -> String {
    match runtime_value(value) {
        RuntimeValue::Text(value) => value,
        RuntimeValue::Number(value) => value.to_string(),
        RuntimeValue::Integer(value) => value.to_string(),
        RuntimeValue::Bool(value) => value.to_string(),
        RuntimeValue::Length(value) => value.to_string(),
        RuntimeValue::Area(value) => value.to_string(),
        RuntimeValue::Volume(value) => value.to_string(),
        RuntimeValue::Angle(value) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next_random(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state
    }

    #[test]
    fn edits_undo_and_redo_without_side_channels() {
        let mut builder = DocumentBuilder::new("editable");
        let object = builder.add_object(Object::new(ObjectId::default(), "Cube"));
        let mut document = builder.finish();
        document
            .apply(Edit::Rename {
                object,
                name: "Part".to_string(),
            })
            .unwrap();
        document
            .apply(Edit::SetProperty {
                object,
                key: "mass".to_string(),
                value: Some(Value::Number(4.0)),
            })
            .unwrap();
        assert_eq!(document.object(object).unwrap().properties["mass"], Value::Number(4.0));
        document.undo();
        assert!(!document.object(object).unwrap().properties.contains_key("mass"));
        document.redo();
        assert_eq!(document.object(object).unwrap().name, "Part");
        assert_eq!(document.object(object).unwrap().properties["mass"], Value::Number(4.0));
    }

    #[test]
    fn random_edit_sequences_restore_the_exact_initial_and_final_state() {
        for sequence in 0..8 {
            let mut builder = DocumentBuilder::new("property history");
            let texture = builder.add_texture(Texture {
                name: "base texture".to_string(),
                width: 2,
                height: 2,
                rgba8: vec![0, 1, 2, 3, 4, 5, 6, 7].into(),
                ..Default::default()
            });
            let mut first_material = PbrMaterial::new(MaterialId::default(), "first");
            first_material.base_color_texture = Some(TextureSlot::new(texture));
            let first_material = builder.add_material(first_material);
            let second_material = builder.add_material(PbrMaterial::new(
                MaterialId::default(),
                "second",
            ));
            let mesh = builder.add_mesh(Mesh {
                name: "triangle".to_string(),
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].into(),
                normals: vec![[0.0, 0.0, 1.0]; 3].into(),
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]].into(),
                indices: vec![0, 1, 2].into(),
                material_slots: vec![first_material, second_material],
                ..Default::default()
            });
            let first_collection =
                builder.add_collection(Collection::new(CollectionId::default(), "first"));
            let second_collection =
                builder.add_collection(Collection::new(CollectionId::default(), "second"));
            let mut root = Object::new(ObjectId::default(), "root");
            root.collections.push(first_collection);
            root.meshes.push(MeshInstance::new(mesh));
            let root = builder.add_object(root);
            let mut child = Object::new(ObjectId::default(), "child");
            child.parent = Some(root);
            builder.add_object(child);
            let mut document = builder.finish();
            let initial = document.state.clone();

            let mut random = 0x9e37_79b9_7f4a_7c15 ^ sequence;
            let mut next_object = 10_000;
            let mut next_collection = 20_000;
            let mut next_mesh = 30_000;
            let mut next_material = 40_000;
            let mut next_texture = 50_000;
            let mut objects = Vec::new();
            let mut collections = Vec::new();
            let mut meshes = Vec::new();
            let mut materials = Vec::new();
            let mut textures = Vec::new();

            for step in 0..220 {
                let choice = next_random(&mut random) % 20;
                let edit = match choice {
                    0 => Edit::Rename {
                        object: root,
                        name: format!("root-{sequence}-{step}"),
                    },
                    1 => Edit::SetProperty {
                        object: root,
                        key: format!("property-{}", step % 5),
                        value: Some(Value::Number(next_random(&mut random) as f64)),
                    },
                    2 => Edit::SetProperty {
                        object: root,
                        key: format!("property-{}", step % 5),
                        value: None,
                    },
                    3 => {
                        let mut transform = Transform::default();
                        transform.matrix.v[12] = (next_random(&mut random) % 1000) as f32;
                        Edit::Transform { object: root, transform }
                    }
                    4 => Edit::SetVisibility {
                        object: root,
                        visible: next_random(&mut random) & 1 == 0,
                    },
                    5 => Edit::SetSelection {
                        object: root,
                        selected: next_random(&mut random) & 1 == 0,
                    },
                    6 => Edit::SetTags {
                        object: root,
                        tags: vec![format!("tag-{}", next_random(&mut random) % 7)],
                    },
                    7 => Edit::SetCollections {
                        object: root,
                        collections: if next_random(&mut random) & 1 == 0 {
                            vec![first_collection]
                        } else {
                            vec![second_collection, first_collection]
                        },
                    },
                    8 => Edit::SetCollectionProperty {
                        collection: first_collection,
                        key: format!("property-{}", step % 3),
                        value: (next_random(&mut random) & 1 == 0)
                            .then(|| Value::Bool(step & 1 == 0)),
                    },
                    9 => Edit::SetMaterial {
                        object: root,
                        mesh: 0,
                        slot: (next_random(&mut random) % 5) as usize,
                        material: match next_random(&mut random) % 3 {
                            0 => None,
                            1 => Some(first_material),
                            _ => Some(second_material),
                        },
                    },
                    10 if !objects.is_empty() => {
                        let index = next_random(&mut random) as usize % objects.len();
                        Edit::RemoveObject(objects.remove(index))
                    }
                    10 => {
                        let id = ObjectId(next_object);
                        next_object += 1;
                        objects.push(id);
                        Edit::AddObject(Object::new(id, format!("object-{}", id.0)))
                    }
                    11 if !collections.is_empty() => {
                        let index = next_random(&mut random) as usize % collections.len();
                        Edit::RemoveCollection(collections.remove(index))
                    }
                    11 => {
                        let id = CollectionId(next_collection);
                        next_collection += 1;
                        collections.push(id);
                        Edit::AddCollection(Collection::new(id, format!("collection-{}", id.0)))
                    }
                    12 if !meshes.is_empty() => {
                        let index = next_random(&mut random) as usize % meshes.len();
                        Edit::RemoveMesh(meshes.remove(index))
                    }
                    12 => {
                        let id = MeshId(next_mesh);
                        next_mesh += 1;
                        meshes.push(id);
                        Edit::AddMesh(Mesh {
                            id,
                            name: format!("mesh-{}", id.0),
                            positions: vec![[step as f32, 0.0, 0.0]].into(),
                            indices: Vec::new().into(),
                            ..Default::default()
                        })
                    }
                    13 if !materials.is_empty() => {
                        let index = next_random(&mut random) as usize % materials.len();
                        Edit::RemoveMaterial(materials.remove(index))
                    }
                    13 => {
                        let id = MaterialId(next_material);
                        next_material += 1;
                        materials.push(id);
                        Edit::AddMaterial(PbrMaterial::new(id, format!("material-{}", id.0)))
                    }
                    14 if !textures.is_empty() => {
                        let index = next_random(&mut random) as usize % textures.len();
                        Edit::RemoveTexture(textures.remove(index))
                    }
                    _ => {
                        let id = TextureId(next_texture);
                        next_texture += 1;
                        textures.push(id);
                        Edit::AddTexture(Texture {
                            id,
                            name: format!("texture-{}", id.0),
                            width: 1,
                            height: 1,
                            rgba8: vec![step as u8; 4].into(),
                            ..Default::default()
                        })
                    }
                };
                document.apply(edit).unwrap();
            }

            let final_state = document.state.clone();
            let mut undone = 0;
            while document.can_undo() {
                document.undo().unwrap();
                undone += 1;
            }
            assert_eq!(undone, 220);
            assert_eq!(document.state, initial, "sequence {sequence} did not undo exactly");

            let mut redone = 0;
            while document.can_redo() {
                document.redo().unwrap();
                redone += 1;
            }
            assert_eq!(redone, 220);
            assert_eq!(document.state, final_state, "sequence {sequence} did not redo exactly");
        }
    }

    #[test]
    fn removing_a_parent_restores_child_links_and_order() {
        let mut builder = DocumentBuilder::new("hierarchy");
        let before = builder.add_object(Object::new(ObjectId::default(), "before"));
        let parent = builder.add_object(Object::new(ObjectId::default(), "parent"));
        let mut child = Object::new(ObjectId::default(), "child");
        child.parent = Some(parent);
        let child = builder.add_object(child);
        let mut document = builder.finish();
        let initial = document.state.clone();

        document.apply(Edit::RemoveObject(parent)).unwrap();
        assert_eq!(document.object(child).unwrap().parent, None);
        document.undo().unwrap();
        assert_eq!(document.state, initial);
        assert_eq!(document.objects()[0].id, before);
        assert_eq!(document.objects()[1].id, parent);
    }

    #[test]
    fn undo_depth_is_configurable() {
        let mut builder = DocumentBuilder::new("depth");
        let object = builder.add_object(Object::new(ObjectId::default(), "zero"));
        let mut document = builder.finish();
        document.set_undo_depth(2);
        for name in ["one", "two", "three"] {
            document
                .apply(Edit::Rename { object, name: name.to_string() })
                .unwrap();
        }
        document.undo().unwrap();
        document.undo().unwrap();
        assert!(!document.can_undo());
        assert_eq!(document.object(object).unwrap().name, "one");
    }

    #[test]
    fn storeys_are_plain_collections_with_a_level() {
        let mut builder = DocumentBuilder::new("house");
        let mut storey = Collection::new(CollectionId::default(), "Ground");
        storey.properties.insert("arch.elevation_m".to_string(), Value::Number(0.0));
        let id = builder.add_collection(storey);
        let document = builder.finish();
        assert_eq!(document.collection(id).unwrap().level(), Some(0.0));
    }
}
