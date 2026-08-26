//! glTF/GLB reference loader for the editable Fab document seam.

use fab::document::{
    Camera, Collection, CollectionId, Document, DocumentBuilder, Light, LightKind, MaterialId,
    Mesh, MeshInstance, Object, ObjectId, PbrMaterial, Texture, TextureId, TextureSlot, Transform,
    Value,
};
use fab::model::makepad_math::Mat4f;
use fab::model::{Handedness, LoadCancel, LoadError, LoadProgress, Loader, Units, UpAxis};
use makepad_gltf::{
    GltfNode, JsonValue, LoadedGltf, GLTF_MODE_LINES, GLTF_MODE_TRIANGLES,
};
use makepad_raytrace::SceneInput;
use std::path::Path;

#[derive(Default)]
pub struct GltfLoader;

impl Loader for GltfLoader {
    fn extensions(&self) -> &[&str] {
        &["glb", "gltf"]
    }

    fn probe(&self, bytes: &[u8]) -> bool {
        makepad_gltf::is_glb_bytes(bytes)
            || std::str::from_utf8(bytes)
                .ok()
                .map(str::trim_start)
                .is_some_and(|text| text.starts_with('{') && text.contains("\"asset\""))
    }

    fn load_cancellable(
        &self,
        path: &Path,
        progress: &mut dyn FnMut(LoadProgress),
        cancel: LoadCancel,
    ) -> Result<Document, LoadError> {
        progress(LoadProgress::Opening);
        if cancel() {
            return Err(LoadError::Cancelled);
        }
        progress(LoadProgress::Parsing(0.1));
        let traced = makepad_raytrace::glb::load_glb(path)
            .map_err(|error| LoadError::Corrupt(error.to_string()))?;
        let loaded = makepad_gltf::load_gltf_from_path(path)
            .map_err(|error| LoadError::Corrupt(format!("{error:?}")))?;
        if cancel() {
            return Err(LoadError::Cancelled);
        }
        progress(LoadProgress::Meshing {
            done: loaded.document.meshes_slice().len(),
            total: loaded.document.meshes_slice().len(),
        });
        let document = build_document(path, traced, &loaded)?;
        progress(LoadProgress::Done);
        Ok(document)
    }
}

fn build_document(path: &Path, traced: SceneInput, loaded: &LoadedGltf) -> Result<Document, LoadError> {
    let name = path
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Model".to_string());
    let mut builder = DocumentBuilder::new(name.clone());
    builder
        .source_path(path)
        .coordinates(Units::default(), UpAxis::Y, Handedness::Right)
        .metadata("format", Value::Enum("gltf".to_string()));
    let mut level_collections = scene_metadata(&mut builder, loaded);

    let texture_ids: Vec<TextureId> = traced
        .images
        .iter()
        .enumerate()
        .map(|(index, image)| {
            builder.add_texture(Texture {
                name: format!("Image {}", index + 1),
                width: image.width as u32,
                height: image.height as u32,
                rgba8: rgba8(image).into(),
                ..Default::default()
            })
        })
        .collect();

    let material_ids: Vec<MaterialId> = traced
        .materials
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let source_doc = loaded.document.materials_slice().get(index);
            let mut material = PbrMaterial::new(
                MaterialId::default(),
                source_doc
                    .and_then(|material| material.name.clone())
                    .unwrap_or_else(|| format!("Material {}", index + 1)),
            );
            material.base_color = source_doc
                .and_then(|material| material.pbr_metallic_roughness.as_ref())
                .and_then(|pbr| pbr.base_color_factor)
                .unwrap_or([source.albedo[0], source.albedo[1], source.albedo[2], 1.0]);
            material.metallic = source.metal;
            material.roughness = source.roughness;
            material.emissive = source.emission;
            material.ior = source.ior;
            material.transmission = source.transmission;
            material.double_sided = source.two_sided;
            material.base_color_texture = source
                .texture
                .and_then(|slot| texture_ids.get(slot).copied())
                .map(TextureSlot::new);
            if let Some(extensions) = source_doc.and_then(|material| material.extensions.as_ref()) {
                if let Some(value) = nested_number(
                    extensions,
                    &["KHR_materials_transmission", "transmissionFactor"],
                ) {
                    material.transmission = value as f32;
                }
                if let Some(value) = nested_number(extensions, &["KHR_materials_ior", "ior"]) {
                    material.ior = value as f32;
                }
            }
            if material.transmission > 0.0 {
                // The path tracer's glazing contract is a two-sided thin
                // sheet even when the source omits doubleSided.
                material.double_sided = true;
            }
            let material_class = source_doc
                .and_then(|material| material.extras.as_ref())
                .and_then(|extras| nested(extras, &["arch", "material_class"]))
                .and_then(json_string)
                .map(str::to_string)
                .or_else(|| (material.transmission > 0.0).then(|| "glass".to_string()));
            if let Some(material_class) = material_class.as_deref() {
                material.properties.insert(
                    "arch.material_class".to_string(),
                    Value::Enum(material_class.to_string()),
                );
            }
            if material.transmission > 0.0
                || material_class.is_some_and(|class| class.eq_ignore_ascii_case("glass"))
            {
                material.tags.push("glass".to_string());
            }
            builder.add_material(material)
        })
        .collect();

    let mut source_meshes: Vec<Vec<fab::document::MeshId>> = Vec::new();
    for (mesh_index, source_mesh) in loaded.document.meshes_slice().iter().enumerate() {
        let line_sets = source_mesh
            .primitives
            .iter()
            .enumerate()
            .filter(|(_, primitive)| primitive.mode() == GLTF_MODE_LINES)
            .map(|(primitive_index, _)| decode_lines(loaded, mesh_index, primitive_index))
            .collect::<Result<Vec<_>, _>>()?;
        let mut primitives = Vec::new();
        for (primitive_index, source_primitive) in source_mesh.primitives.iter().enumerate() {
            if source_primitive.mode() != GLTF_MODE_TRIANGLES {
                continue;
            }
            let primitive = makepad_gltf::decode_mesh_primitive(loaded, mesh_index, primitive_index)
                .map_err(|error| LoadError::Corrupt(format!("{error:?}")))?;
            let material = primitive
                .material
                .and_then(|index| material_ids.get(index).copied())
                .or_else(|| material_ids.first().copied());
            let crease_edges = crease_edges(&primitive.positions, &line_sets);
            primitives.push(builder.add_mesh(Mesh {
                name: source_mesh
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Mesh {}.{}", mesh_index + 1, primitive_index + 1)),
                positions: primitive.positions.into(),
                normals: primitive.normals.unwrap_or_default().into(),
                uvs: primitive.texcoords0.unwrap_or_default().into(),
                colours: primitive.colors0.unwrap_or_default(),
                indices: primitive.indices.into(),
                material_slots: material.into_iter().collect(),
                face_material_slots: Vec::new(),
                crease_edges,
                tags: Vec::new(),
                properties: Default::default(),
                ..Default::default()
            }));
        }
        source_meshes.push(primitives);
    }

    let nodes = loaded.document.nodes_slice();
    if nodes.is_empty() {
        let mesh = builder.add_mesh(flattened_mesh(&traced, &material_ids));
        let mut object = Object::new(ObjectId::default(), name);
        object.meshes.push(MeshInstance::new(mesh));
        builder.add_object(object);
        return Ok(builder.finish());
    }

    let parents = node_parents(nodes);
    let worlds = node_worlds(nodes, &parents);
    for (index, node) in nodes.iter().enumerate() {
        let node_name = node
            .name
            .clone()
            .unwrap_or_else(|| format!("Object {}", index + 1));
        let mut object = Object::new(ObjectId::default(), node_name.clone());
        object.parent = parents[index].map(|parent| ObjectId(parent as u64 + 1));
        object.transform = Transform { matrix: worlds[index] };
        apply_node_metadata(&mut object, node.extras.as_ref());
        if let Some(kind) = object.semantic_class().map(str::to_string) {
            object.tags.push(kind);
        }
        if let Some(level) = inherited_level(index, nodes, &parents) {
            let collection = level_collections
                .iter()
                .find(|candidate| candidate.index == level.index)
                .map(|candidate| candidate.id)
                .unwrap_or_else(|| {
                    let mut collection = Collection::new(
                        CollectionId::default(),
                        level.name.clone().unwrap_or_else(|| format!("Level {}", level.index)),
                    );
                    collection.properties.insert(
                        "arch.level".to_string(),
                        Value::Number(level.index as f64),
                    );
                    collection.properties.insert(
                        "arch.elevation_m".to_string(),
                        Value::Number(level.elevation_m.unwrap_or(level.index as f64)),
                    );
                    let id = builder.add_collection(collection);
                    level_collections.push(LevelCollection { index: level.index, id });
                    id
                });
            object.collections.push(collection);
        }
        if let Some(mesh_index) = node.mesh {
            if let Some(meshes) = source_meshes.get(mesh_index) {
                object.meshes.extend(meshes.iter().copied().map(MeshInstance::new));
            }
        }
        if let Some(camera_index) = node.camera {
            object.camera = loaded.document.cameras_slice().get(camera_index).map(|camera| {
                if let Some(perspective) = &camera.perspective {
                    Camera {
                        perspective: true,
                        fov_y: perspective.yfov,
                        orthographic_height: 0.0,
                        near: perspective.znear,
                        far: perspective.zfar.unwrap_or(10_000.0),
                    }
                } else if let Some(orthographic) = &camera.orthographic {
                    Camera {
                        perspective: false,
                        fov_y: 0.0,
                        orthographic_height: orthographic.ymag * 2.0,
                        near: orthographic.znear,
                        far: orthographic.zfar,
                    }
                } else {
                    Camera {
                        perspective: true,
                        fov_y: 45.0f32.to_radians(),
                        orthographic_height: 0.0,
                        near: 0.01,
                        far: 10_000.0,
                    }
                }
            });
        }
        object.light = light_for_node(loaded, node);
        let id = builder.add_object(object);
        debug_assert_eq!(id, ObjectId(index as u64 + 1));
    }
    Ok(builder.finish())
}

fn flattened_mesh(scene: &SceneInput, materials: &[MaterialId]) -> Mesh {
    Mesh {
        name: "Scene".to_string(),
        positions: scene.positions.clone().into(),
        normals: scene.normals.clone().into(),
        uvs: scene.uvs.clone().into(),
        indices: scene.indices.clone().into(),
        material_slots: materials.to_vec(),
        face_material_slots: scene.tri_material.clone(),
        ..Default::default()
    }
}

fn rgba8(image: &makepad_raytrace::Image) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(image.data.len() * 4);
    for pixel in &image.data {
        rgba.extend_from_slice(&[
            (pixel >> 16) as u8,
            (pixel >> 8) as u8,
            *pixel as u8,
            (pixel >> 24) as u8,
        ]);
    }
    rgba
}

fn node_parents(nodes: &[GltfNode]) -> Vec<Option<usize>> {
    let mut parents = vec![None; nodes.len()];
    for (parent, node) in nodes.iter().enumerate() {
        for child in node.children.iter().flatten() {
            if let Some(slot) = parents.get_mut(*child) {
                *slot = Some(parent);
            }
        }
    }
    parents
}

fn node_worlds(nodes: &[GltfNode], parents: &[Option<usize>]) -> Vec<Mat4f> {
    fn visit(
        index: usize,
        nodes: &[GltfNode],
        parents: &[Option<usize>],
        worlds: &mut [Option<Mat4f>],
        visiting: &mut [bool],
    ) -> Mat4f {
        if let Some(world) = worlds[index] {
            return world;
        }
        if visiting[index] {
            return node_matrix(&nodes[index]);
        }
        visiting[index] = true;
        let local = node_matrix(&nodes[index]);
        let world = parents[index]
            .map(|parent| Mat4f::mul(&visit(parent, nodes, parents, worlds, visiting), &local))
            .unwrap_or(local);
        visiting[index] = false;
        worlds[index] = Some(world);
        world
    }
    let mut worlds = vec![None; nodes.len()];
    let mut visiting = vec![false; nodes.len()];
    for index in 0..nodes.len() {
        visit(index, nodes, parents, &mut worlds, &mut visiting);
    }
    worlds.into_iter().map(Option::unwrap).collect()
}

fn node_matrix(node: &GltfNode) -> Mat4f {
    if let Some(matrix) = node.matrix {
        return Mat4f { v: matrix };
    }
    let translation = node.translation.unwrap_or([0.0; 3]);
    let rotation = node.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let scale = node.scale.unwrap_or([1.0; 3]);
    let (x, y, z, w) = (rotation[0], rotation[1], rotation[2], rotation[3]);
    Mat4f {
        v: [
            (1.0 - 2.0 * (y * y + z * z)) * scale[0],
            (2.0 * (x * y + z * w)) * scale[0],
            (2.0 * (x * z - y * w)) * scale[0],
            0.0,
            (2.0 * (x * y - z * w)) * scale[1],
            (1.0 - 2.0 * (x * x + z * z)) * scale[1],
            (2.0 * (y * z + x * w)) * scale[1],
            0.0,
            (2.0 * (x * z + y * w)) * scale[2],
            (2.0 * (y * z - x * w)) * scale[2],
            (1.0 - 2.0 * (x * x + y * y)) * scale[2],
            0.0,
            translation[0],
            translation[1],
            translation[2],
            1.0,
        ],
    }
}

#[derive(Clone, Debug)]
struct LevelMetadata {
    index: i64,
    name: Option<String>,
    elevation_m: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
struct LevelCollection {
    index: i64,
    id: CollectionId,
}

type LineSet = (Vec<[f32; 3]>, Vec<u32>);

fn decode_lines(
    loaded: &LoadedGltf,
    mesh_index: usize,
    primitive_index: usize,
) -> Result<LineSet, LoadError> {
    let primitive = &loaded.document.meshes_slice()[mesh_index].primitives[primitive_index];
    let position = primitive.attributes.get("POSITION").copied().ok_or_else(|| {
        LoadError::Corrupt(format!(
            "line primitive {mesh_index}.{primitive_index} has no POSITION accessor"
        ))
    })?;
    let positions = makepad_gltf::read_accessor_f32x3(loaded, position)
        .map_err(|error| LoadError::Corrupt(format!("{error:?}")))?;
    let indices = match primitive.indices {
        Some(accessor) => makepad_gltf::read_accessor_indices_u32(loaded, accessor)
            .map_err(|error| LoadError::Corrupt(format!("{error:?}")))?,
        None => (0..positions.len() as u32).collect(),
    };
    Ok((positions, indices))
}

fn crease_edges(positions: &[[f32; 3]], line_sets: &[LineSet]) -> Vec<[u32; 2]> {
    let lookup: std::collections::HashMap<[u32; 3], u32> = positions
        .iter()
        .enumerate()
        .map(|(index, point)| (point.map(f32::to_bits), index as u32))
        .collect();
    let mut edges = Vec::new();
    for (line_positions, indices) in line_sets {
        for edge in indices.chunks_exact(2) {
            let Some(from) = line_positions.get(edge[0] as usize) else { continue };
            let Some(to) = line_positions.get(edge[1] as usize) else { continue };
            let Some(from) = lookup.get(&from.map(f32::to_bits)).copied() else { continue };
            let Some(to) = lookup.get(&to.map(f32::to_bits)).copied() else { continue };
            if from != to && !edges.contains(&[from, to]) && !edges.contains(&[to, from]) {
                edges.push([from, to]);
            }
        }
    }
    edges
}

fn scene_metadata(builder: &mut DocumentBuilder, loaded: &LoadedGltf) -> Vec<LevelCollection> {
    let scene = loaded
        .document
        .scene
        .and_then(|index| loaded.document.scenes_slice().get(index))
        .or_else(|| loaded.document.scenes_slice().first());
    let Some(arch) = scene
        .and_then(|scene| scene.extras.as_ref())
        .and_then(|extras| extras.key("arch"))
    else {
        return Vec::new();
    };
    for key in ["units", "north_deg"] {
        if let Some(value) = arch.key(key).and_then(json_value) {
            builder.metadata(format!("arch.{key}"), value);
        }
    }
    if let Some(site) = arch.key("site").and_then(JsonValue::object) {
        for key in [
            "lat",
            "lon",
            "elevation_m",
            "city",
            "timezone_min",
            "summer_time",
            "date_local",
            "day_of_year",
            "minute_of_day",
            "utc_offset_hours",
            "dst",
            "date",
            "time",
        ] {
            if let Some(value) = site.get(key).and_then(json_value) {
                builder.metadata(format!("arch.site.{key}"), value);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(JsonValue::Array(levels)) = arch.key("levels") {
        for level in levels {
            let Some(level) = level.object() else { continue };
            let Some(index) = level.get("index").and_then(json_number).map(|value| value as i64) else {
                continue;
            };
            let name = level
                .get("name")
                .and_then(json_string)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Level {index}"));
            let elevation = level
                .get("elevation")
                .or_else(|| level.get("elevation_m"))
                .and_then(json_number)
                .unwrap_or(index as f64);
            let mut collection = Collection::new(CollectionId::default(), name);
            collection
                .properties
                .insert("arch.level".to_string(), Value::Number(index as f64));
            collection.properties.insert(
                "arch.elevation_m".to_string(),
                Value::Number(elevation),
            );
            out.push(LevelCollection {
                index,
                id: builder.add_collection(collection),
            });
        }
    }
    out
}

fn apply_node_metadata(object: &mut Object, extras: Option<&JsonValue>) {
    let Some(extras) = extras else { return };
    if let Some(arch) = extras.key("arch").and_then(JsonValue::object) {
        for (key, raw) in arch {
            if key == "level_name" {
                continue;
            }
            if key == "priority" {
                if let Some(value) = json_number(raw) {
                    object
                        .properties
                        .insert("arch.priority".to_string(), Value::Number(value));
                }
                continue;
            }
            if let Some(value) = json_value(raw) {
                object.properties.insert(format!("arch.{key}"), value);
            }
        }
    }
    if let Some(properties) = extras.key("props").and_then(JsonValue::object) {
        for (key, raw) in properties {
            if let Some(value) = json_value(raw) {
                object.properties.insert(key.clone(), value);
            }
        }
    }
}

fn inherited_level(
    index: usize,
    nodes: &[GltfNode],
    parents: &[Option<usize>],
) -> Option<LevelMetadata> {
    let mut current = Some(index);
    let mut guard = 0;
    while let Some(index) = current {
        let extras = nodes[index].extras.as_ref();
        let arch = extras.and_then(|value| value.key("arch"));
        if let Some(index) = arch
            .and_then(|value| value.key("level"))
            .and_then(json_number)
            .map(|value| value as i64)
        {
            return Some(LevelMetadata {
                index,
                name: arch
                    .and_then(|value| value.key("level_name"))
                    .and_then(json_string)
                    .map(str::to_string),
                elevation_m: None,
            });
        }
        current = parents[index];
        guard += 1;
        if guard > nodes.len() {
            break;
        }
    }
    None
}

fn light_for_node(loaded: &LoadedGltf, node: &GltfNode) -> Option<Light> {
    let index = nested_number(node.extensions.as_ref()?, &["KHR_lights_punctual", "light"])?
        as usize;
    let lights = nested(loaded.document.extensions.as_ref()?, &["KHR_lights_punctual", "lights"])?;
    let JsonValue::Array(lights) = lights else { return None };
    let source = lights.get(index)?.object()?;
    let kind = match source.get("type").and_then(json_string)? {
        "directional" => LightKind::Directional,
        "point" => LightKind::Point,
        "spot" => LightKind::Spot,
        _ => return None,
    };
    let colour = source
        .get("color")
        .and_then(json_numbers::<3>)
        .map(|value| value.map(|value| value as f32))
        .unwrap_or([1.0; 3]);
    let spot = source.get("spot");
    Some(Light {
        kind,
        colour,
        intensity: source.get("intensity").and_then(json_number).unwrap_or(1.0) as f32,
        range: source.get("range").and_then(json_number).map(|value| value as f32),
        spot_angles: (kind == LightKind::Spot).then(|| {
            [
                spot.and_then(|value| value.key("innerConeAngle"))
                    .and_then(json_number)
                    .unwrap_or(0.0) as f32,
                spot.and_then(|value| value.key("outerConeAngle"))
                    .and_then(json_number)
                    .unwrap_or(std::f64::consts::FRAC_PI_4) as f32,
            ]
        }),
        area_size: [0.0; 2],
    })
}

fn nested<'a>(value: &'a JsonValue, path: &[&str]) -> Option<&'a JsonValue> {
    path.iter().try_fold(value, |value, key| value.key(key))
}

fn nested_number(value: &JsonValue, path: &[&str]) -> Option<f64> {
    nested(value, path).and_then(json_number)
}

fn json_number(value: &JsonValue) -> Option<f64> {
    match value {
        JsonValue::U64(value) => Some(*value as f64),
        JsonValue::U128(value) => Some(*value as f64),
        JsonValue::I64(value) => Some(*value as f64),
        JsonValue::I128(value) => Some(*value as f64),
        JsonValue::F64(value) => Some(*value),
        _ => None,
    }
}

fn json_string(value: &JsonValue) -> Option<&str> {
    match value {
        JsonValue::String(value) | JsonValue::BareIdent(value) => Some(value),
        _ => None,
    }
}

fn json_numbers<const N: usize>(value: &JsonValue) -> Option<[f64; N]> {
    let JsonValue::Array(values) = value else { return None };
    if values.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = json_number(value)?;
    }
    Some(out)
}

fn json_value(value: &JsonValue) -> Option<Value> {
    match value {
        JsonValue::String(value) | JsonValue::BareIdent(value) => {
            Some(Value::String(value.clone()))
        }
        JsonValue::Bool(value) => Some(Value::Bool(*value)),
        JsonValue::Array(values) if values.len() == 2 => {
            Some(Value::Vec2(json_numbers::<2>(value)?))
        }
        JsonValue::Array(values) if values.len() == 3 => {
            Some(Value::Vec3(json_numbers::<3>(value)?))
        }
        JsonValue::Array(values) if values.len() == 4 => {
            Some(Value::Vec4(json_numbers::<4>(value)?))
        }
        JsonValue::Null | JsonValue::Undefined | JsonValue::Object(_) | JsonValue::Array(_) => None,
        _ => json_number(value).map(Value::Number),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fab::model::{Sheet, SheetItem};
    use std::sync::{Arc, OnceLock};

    fn woodside_scene() -> Option<Arc<fab::model::Scene>> {
        static SCENE: OnceLock<Option<Arc<fab::model::Scene>>> = OnceLock::new();
        SCENE
            .get_or_init(|| {
                let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .ancestors()
                    .nth(4)
                    .expect("loader crate is inside the workspace");
                let path = root.join("local/models/samples/woodside.glb");
                if !path.exists() {
                    return None;
                }
                let document = GltfLoader
                    .load(&path, &mut |_| {})
                    .expect("converted architectural sample loads");
                Some(Arc::new(fab::model::Scene::from_document(
                    document,
                    &mut |_| {},
                )))
            })
            .clone()
    }

    fn woodside_sheets() -> Option<&'static [Sheet]> {
        static SHEETS: OnceLock<Option<Vec<Sheet>>> = OnceLock::new();
        SHEETS
            .get_or_init(|| {
                woodside_scene().map(|scene| {
                    fab::sheets::fixture::sheets_for(
                        &scene,
                        &fab::sheets::plan::PlanSettings::default(),
                    )
                })
            })
            .as_deref()
    }

    struct TestImage {
        width: usize,
        height: usize,
        luma: Vec<u8>,
    }

    impl TestImage {
        fn at(&self, x: usize, y: usize) -> u8 {
            self.luma[y * self.width + x]
        }
    }

    fn point_in_polygon(point: [f32; 2], polygon: &[[f32; 2]]) -> bool {
        let mut inside = false;
        let mut previous = polygon.len() - 1;
        for current in 0..polygon.len() {
            let a = polygon[current];
            let b = polygon[previous];
            if (a[1] > point[1]) != (b[1] > point[1])
                && point[0]
                    < (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                inside = !inside;
            }
            previous = current;
        }
        inside
    }

    /// Software-rasterize sheet fills only. This exercises the same painter's
    /// order as the sheet view without opening a window or touching the GPU.
    fn rasterize_fills(sheet: &Sheet, width: usize, height: usize) -> TestImage {
        let mut image = TestImage {
            width,
            height,
            luma: vec![255; width * height],
        };
        let sx = width as f32 / sheet.size_mm[0];
        let sy = height as f32 / sheet.size_mm[1];
        for item in &sheet.items {
            let SheetItem::Fill { points, color, .. } = item else {
                continue;
            };
            if points.len() < 3 {
                continue;
            }
            let lo = points.iter().fold([f32::MAX; 2], |lo, point| {
                [lo[0].min(point[0]), lo[1].min(point[1])]
            });
            let hi = points.iter().fold([f32::MIN; 2], |hi, point| {
                [hi[0].max(point[0]), hi[1].max(point[1])]
            });
            let x0 = (lo[0] * sx).floor().max(0.0) as usize;
            let y0 = (lo[1] * sy).floor().max(0.0) as usize;
            let x1 = ((hi[0] * sx).ceil() as usize).min(width);
            let y1 = ((hi[1] * sy).ceil() as usize).min(height);
            let luma = ((color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722)
                * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
            for y in y0..y1 {
                for x in x0..x1 {
                    let paper = [(x as f32 + 0.5) / sx, (y as f32 + 0.5) / sy];
                    if point_in_polygon(paper, points) {
                        image.luma[y * width + x] = luma;
                    }
                }
            }
        }
        image
    }

    #[test]
    fn scene_site_extras_enter_the_fab_document() {
        let json = br#"{
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{
                "nodes": [],
                "extras": {"arch": {
                    "north_deg": 77.4,
                    "site": {
                        "lat": 47.579,
                        "lon": -122.241,
                        "utc_offset_hours": -8,
                        "dst": true,
                        "date": "2024-06-21",
                        "time": "14:30"
                    }
                }}
            }]
        }"#;
        let loaded = makepad_gltf::load_gltf_from_bytes(json, None).expect("minimal glTF");
        let mut builder = DocumentBuilder::new("site extras");
        scene_metadata(&mut builder, &loaded);
        let document = builder.finish();
        assert_eq!(
            document.metadata().get("arch.site.lat").and_then(Value::number),
            Some(47.579)
        );
        assert_eq!(
            document
                .metadata()
                .get("arch.north_deg")
                .and_then(Value::number),
            Some(77.4)
        );
        assert_eq!(
            document
                .metadata()
                .get("arch.site.date")
                .and_then(Value::text),
            Some("2024-06-21")
        );
        assert_eq!(document.metadata().get("arch.site.dst"), Some(&Value::Bool(true)));
    }

    #[test]
    fn line_primitives_become_crease_edges() {
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let lines = vec![(positions.to_vec(), vec![0, 1, 1, 2])];
        assert_eq!(crease_edges(&positions, &lines), [[0, 1], [1, 2]]);
    }

    #[test]
    fn architecture_kind_and_priority_reach_the_runtime_model() {
        let extras = JsonValue::Object(std::collections::HashMap::from([(
            "arch".to_string(),
            JsonValue::Object(std::collections::HashMap::from([
                ("kind".to_string(), JsonValue::String("roof".to_string())),
                ("priority".to_string(), JsonValue::U64(9)),
            ])),
        )]));
        let mut builder = DocumentBuilder::new("metadata");
        let mut object = Object::new(ObjectId::default(), "roof face");
        apply_node_metadata(&mut object, Some(&extras));
        builder.add_object(object);

        let scene = fab::model::Scene::from_document(builder.finish(), &mut |_| {});
        assert_eq!(scene.elements[0].class, fab::model::ElementClass::Roof);
        assert!(scene.elements[0].properties.iter().any(|property| {
            property.name == "arch.priority"
                && property.value == fab::model::PropertyValue::Number(9.0)
        }));
    }

    #[test]
    fn opens_a_glb_as_an_editable_document() {
        let bytes = makepad_gltf::write_glb_mesh(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            &[0, 1, 2],
        );
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(4)
            .expect("loader crate is inside the workspace");
        let dir = repo_root.join("target/loader-tests");
        std::fs::create_dir_all(&dir).expect("create loader test directory");
        let path = dir.join(format!("triangle-{}.glb", std::process::id()));
        std::fs::write(&path, bytes).expect("write generated GLB");

        let document = GltfLoader.load(&path, &mut |_| {}).expect("generated GLB loads");
        let _ = std::fs::remove_file(&path);
        assert_eq!(document.meshes().len(), 1);
        assert_eq!(document.meshes()[0].indices.as_ref(), [0, 1, 2]);
        assert!(!document.can_undo(), "loader construction is one clean transaction");

        let scene = fab::model::Scene::from_document(document, &mut |_| {});
        assert_eq!(scene.stats.triangles, 1);
        assert_eq!(scene.stats.elements, 1);
    }

    #[test]
    fn thousand_woodside_renames_stay_below_fifty_milliseconds() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(4)
            .expect("loader crate is inside the workspace");
        let path = repo_root.join("local/models/samples/woodside.glb");
        if !path.is_file() {
            eprintln!("skipping woodside rename timing test: sample is absent");
            return;
        }

        let mut document = GltfLoader
            .load(&path, &mut |_| {})
            .expect("woodside GLB loads");
        let object = document.objects().first().expect("woodside has an object").id;
        let started = std::time::Instant::now();
        for index in 0..1_000 {
            document
                .apply(fab::document::Edit::Rename {
                    object,
                    name: format!("timed rename {index}"),
                })
                .expect("rename succeeds");
        }
        let elapsed = started.elapsed();
        eprintln!("woodside 1000 renames: {:.3} ms", elapsed.as_secs_f64() * 1_000.0);
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "1000 woodside renames took {elapsed:?}"
        );
    }

    #[test]
    fn converted_architecture_contract_if_present() {
        let Some(path) = std::env::var_os("FAB_CONVERTED_GLTF").map(std::path::PathBuf::from)
        else {
            return;
        };
        let document = GltfLoader
            .load(&path, &mut |_| {})
            .expect("converted glTF loads through the reference loader");
        assert!(document
            .objects()
            .iter()
            .any(|object| object.properties.contains_key("arch.kind")));
        assert!(document.objects().iter().any(|object| {
            object
                .properties
                .contains_key("arch.coplanar_priority")
        }));
        assert!(document.collections().iter().any(|collection| collection.level().is_some()));
        assert!(document.meshes().iter().any(|mesh| !mesh.crease_edges.is_empty()));
        assert!(document
            .materials()
            .iter()
            .any(|material| material.transmission > 0.0));
        assert!(document
            .materials()
            .iter()
            .filter(|material| material.transmission > 0.0)
            .all(|material| material.double_sided));
        assert!(document.materials().iter().any(|material| {
            material
                .properties
                .get("arch.material_class")
                .and_then(Value::text)
                == Some("glass")
        }));
        for key in [
            "arch.north_deg",
            "arch.site.lat",
            "arch.site.lon",
            "arch.site.day_of_year",
            "arch.site.minute_of_day",
        ] {
            assert!(document.metadata().contains_key(key), "missing {key}");
        }
    }

    #[test]
    fn woodside_plan_count_matches_level_count() {
        let Some(scene) = woodside_scene() else {
            return;
        };
        let expected_levels = ["SITE", "BASEMENT", "FIRST FLOOR", "SECOND FLOOR", "LOFT", "ROOF"];
        assert_eq!(
            scene.stories.iter().map(|story| story.name.as_str()).collect::<Vec<_>>(),
            expected_levels,
            "converted level table"
        );
        assert_eq!(
            scene
                .tree
                .roots
                .iter()
                .filter_map(|id| scene.element(*id))
                .filter(|element| element.is_group())
                .map(|element| element.name.as_str())
                .collect::<Vec<_>>(),
            expected_levels,
            "outliner storey roots"
        );
        assert!(scene
            .elements
            .iter()
            .filter(|element| element.has_geometry())
            .all(|element| element.story.is_some()));
        let sheets = woodside_sheets().expect("Woodside sheets");
        let plans: Vec<_> = sheets.iter().filter(|sheet| sheet.story.is_some()).collect();
        assert_eq!(plans.len(), scene.stories.len());
        assert!(plans.iter().all(|sheet| sheet.items.iter().any(|item| {
            matches!(item, SheetItem::Text { text, .. } if text.contains("cut 1.20 m"))
        })));
        for name in [
            "South Elevation",
            "North Elevation",
            "East Elevation",
            "West Elevation",
            "Building Section",
        ] {
            assert!(sheets.iter().any(|sheet| sheet.name.contains(name)), "missing {name}");
        }
        assert_eq!(
            sheets.iter().map(|sheet| sheet.name.as_str()).collect::<Vec<_>>(),
            [
                "A-100 SITE Plan",
                "A-101 BASEMENT Plan",
                "A-102 FIRST FLOOR Plan",
                "A-103 SECOND FLOOR Plan",
                "A-104 LOFT Plan",
                "A-105 ROOF Plan",
                "A-201 South Elevation",
                "A-202 North Elevation",
                "A-203 East Elevation",
                "A-204 West Elevation",
                "A-301 Building Section",
            ]
        );
    }

    #[test]
    fn woodside_south_elevation_raster_has_a_house_and_window_holes() {
        let Some(sheets) = woodside_sheets() else {
            return;
        };
        let south = sheets
            .iter()
            .find(|sheet| sheet.name.contains("South Elevation"))
            .expect("south elevation");
        let image = rasterize_fills(south, 840, 594);
        let drawing_y0 = image.height * 18 / 100;
        let mut occupied = 0usize;
        let mut bounds = [image.width, image.height, 0usize, 0usize];
        for y in drawing_y0..image.height {
            for x in 0..image.width {
                if image.at(x, y) < 245 {
                    occupied += 1;
                    bounds[0] = bounds[0].min(x);
                    bounds[1] = bounds[1].min(y);
                    bounds[2] = bounds[2].max(x);
                    bounds[3] = bounds[3].max(y);
                }
            }
        }
        let drawing_pixels = image.width * (image.height - drawing_y0);
        let fraction = occupied as f32 / drawing_pixels as f32;
        assert!((0.015..0.65).contains(&fraction), "silhouette fraction {fraction:.3}");
        let silhouette_width = bounds[2].saturating_sub(bounds[0]);
        let silhouette_height = bounds[3].saturating_sub(bounds[1]);
        assert!(silhouette_width > image.width / 5, "silhouette is a vertical bar: {bounds:?}");
        assert!(silhouette_height > image.height / 14, "silhouette is too short: {bounds:?}");

        let white_openings: Vec<_> = south
            .items
            .iter()
            .filter_map(|item| match item {
                SheetItem::Fill { points, color, .. }
                    if color[0] > 0.99 && color[1] > 0.99 && color[2] > 0.99 =>
                {
                    Some(points)
                }
                _ => None,
            })
            .collect();
        assert!(white_openings.len() > 2, "no façade openings were projected");
        let mut enclosed_holes = 0usize;
        for points in white_openings {
            let centre = points.iter().fold([0.0; 2], |sum, point| {
                [sum[0] + point[0], sum[1] + point[1]]
            });
            let centre = [
                centre[0] / points.len() as f32,
                centre[1] / points.len() as f32,
            ];
            let x = (centre[0] / south.size_mm[0] * image.width as f32) as usize;
            let y = (centre[1] / south.size_mm[1] * image.height as f32) as usize;
            if x >= image.width || y < drawing_y0 || y >= image.height || image.at(x, y) < 250 {
                continue;
            }
            let radius = image.width / 18;
            let dark_left = (x.saturating_sub(radius)..x).any(|px| image.at(px, y) < 245);
            let dark_right = (x + 1..(x + radius).min(image.width)).any(|px| image.at(px, y) < 245);
            let dark_below = (y.saturating_sub(radius)..y).any(|py| image.at(x, py) < 245);
            let dark_above = (y + 1..(y + radius).min(image.height)).any(|py| image.at(x, py) < 245);
            if dark_left && dark_right && dark_below && dark_above {
                enclosed_holes += 1;
            }
        }
        assert!(enclosed_holes > 0, "rendered elevation has no window-shaped hole");
    }
}
