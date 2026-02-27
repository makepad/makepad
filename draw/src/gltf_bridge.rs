use crate::{
    cx_2d::Cx2d,
    image_cache::{
        handle_image_cache_network_responses, load_image_from_cache, load_image_from_data_async,
        process_async_image_load, AsyncImageLoad, AsyncLoadResult, ImageError,
    },
    shader::draw_pbr::{DrawPbr, DrawPbrMaterialState, DrawPbrTextureSet, PbrMeshHandle},
};
use makepad_gltf::{
    decode_mesh_primitive, load_gltf_from_bytes, load_gltf_from_path, load_image_bytes,
    DecodedPrimitive, GltfDocument, GltfError, GltfNode, LoadedGltf,
};
use makepad_platform::*;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub struct GltfPrimitiveObject {
    pub mesh_handle: PbrMeshHandle,
    pub mesh_index: usize,
    pub primitive_index: usize,
    pub material_index: Option<usize>,
    pub bounds_min: Vec3f,
    pub bounds_max: Vec3f,
    pub centroid: Vec3f,
    pub vertex_count: usize,
}

#[derive(Clone, Debug, Default)]
pub struct GltfMeshObjects {
    pub primitives: Vec<GltfPrimitiveObject>,
}

impl GltfMeshObjects {
    /// Decode and upload every supported primitive to retained DrawPbr mesh objects.
    /// Unsupported primitives (for example non-triangle mode) are skipped.
    pub fn upload_all(
        draw: &mut DrawPbr,
        cx: &mut Cx2d,
        loaded: &LoadedGltf,
    ) -> Result<Self, GltfError> {
        let mut out = Self::default();

        for (mesh_index, mesh) in loaded.document.meshes_slice().iter().enumerate() {
            for (primitive_index, primitive) in mesh.primitives.iter().enumerate() {
                let decoded = match decode_mesh_primitive(loaded, mesh_index, primitive_index) {
                    Ok(decoded) => decoded,
                    Err(GltfError::Unsupported(_)) => continue,
                    Err(err) => return Err(err),
                };

                let mesh_handle = draw
                    .upload_decoded_primitive_mesh(cx, &decoded)
                    .map_err(GltfError::Validation)?;

                out.primitives.push(GltfPrimitiveObject {
                    mesh_handle,
                    mesh_index,
                    primitive_index,
                    material_index: primitive.material,
                    bounds_min: primitive_bounds_min(&decoded.positions),
                    bounds_max: primitive_bounds_max(&decoded.positions),
                    centroid: primitive_bounds_center(&decoded.positions),
                    vertex_count: decoded.positions.len(),
                });
            }
        }

        Ok(out)
    }

    /// Render retained primitive objects. The caller should configure DrawPbr
    /// per-draw state (for example transform/material constants) around calls here.
    pub fn draw_all(&self, draw: &mut DrawPbr, cx: &mut Cx2d) -> Result<(), GltfError> {
        for primitive in &self.primitives {
            draw.draw_mesh(cx, primitive.mesh_handle)
                .map_err(GltfError::Validation)?;
        }
        Ok(())
    }

    /// Render retained primitives while allowing caller-controlled material setup.
    /// A callback is invoked before each draw call so the renderer can bind texture/state
    /// for `primitive.material_index`.
    pub fn draw_all_with_setup<F>(
        &self,
        draw: &mut DrawPbr,
        cx: &mut Cx2d,
        mut setup: F,
    ) -> Result<(), GltfError>
    where
        F: FnMut(&GltfPrimitiveObject, &mut DrawPbr) -> Result<(), GltfError>,
    {
        for primitive in &self.primitives {
            setup(primitive, draw)?;
            draw.draw_mesh(cx, primitive.mesh_handle)
                .map_err(GltfError::Validation)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct GltfDecodedPrimitiveObject {
    pub decoded: DecodedPrimitive,
    pub mesh_index: usize,
    pub primitive_index: usize,
    pub material_index: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct GltfDecodedMeshes {
    pub primitives: Vec<GltfDecodedPrimitiveObject>,
}

impl GltfDecodedMeshes {
    /// CPU-side decode only. This can run off the UI thread.
    pub fn decode_all(loaded: &LoadedGltf) -> Result<Self, GltfError> {
        let mut out = Self::default();
        for (mesh_index, mesh) in loaded.document.meshes_slice().iter().enumerate() {
            for (primitive_index, primitive) in mesh.primitives.iter().enumerate() {
                let decoded = match decode_mesh_primitive(loaded, mesh_index, primitive_index) {
                    Ok(decoded) => decoded,
                    Err(GltfError::Unsupported(_)) => continue,
                    Err(err) => return Err(err),
                };

                out.primitives.push(GltfDecodedPrimitiveObject {
                    decoded,
                    mesh_index,
                    primitive_index,
                    material_index: primitive.material,
                });
            }
        }
        Ok(out)
    }

    /// Upload pre-decoded CPU primitives to GPU retained meshes.
    /// This must run on the UI thread.
    pub fn upload_all(
        &self,
        draw: &mut DrawPbr,
        cx: &mut Cx2d,
    ) -> Result<GltfMeshObjects, GltfError> {
        let mut out = GltfMeshObjects::default();
        for primitive in &self.primitives {
            let mesh_handle = draw
                .upload_decoded_primitive_mesh(cx, &primitive.decoded)
                .map_err(GltfError::Validation)?;
            out.primitives.push(GltfPrimitiveObject {
                mesh_handle,
                mesh_index: primitive.mesh_index,
                primitive_index: primitive.primitive_index,
                material_index: primitive.material_index,
                bounds_min: primitive_bounds_min(&primitive.decoded.positions),
                bounds_max: primitive_bounds_max(&primitive.decoded.positions),
                centroid: primitive_bounds_center(&primitive.decoded.positions),
                vertex_count: primitive.decoded.positions.len(),
            });
        }
        Ok(out)
    }
}

#[derive(Clone, Debug)]
pub struct GltfMaterialState {
    pub base_color_factor: Vec4f,
    pub metallic_factor: f32,
    pub roughness_factor: f32,
    pub emissive_factor: Vec3f,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub base_color_texture: Option<usize>,
    pub metallic_roughness_texture: Option<usize>,
    pub normal_texture: Option<usize>,
    pub occlusion_texture: Option<usize>,
    pub emissive_texture: Option<usize>,
}

impl Default for GltfMaterialState {
    fn default() -> Self {
        Self {
            base_color_factor: vec4(1.0, 1.0, 1.0, 1.0),
            metallic_factor: 1.0,
            roughness_factor: 1.0,
            emissive_factor: vec3(0.0, 0.0, 0.0),
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            base_color_texture: None,
            metallic_roughness_texture: None,
            normal_texture: None,
            occlusion_texture: None,
            emissive_texture: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GltfDrawObject {
    pub mesh_handle: PbrMeshHandle,
    pub node_index: usize,
    pub mesh_index: usize,
    pub primitive_index: usize,
    pub material_index: Option<usize>,
    pub world_transform: Mat4f,
    pub local_bounds_min: Vec3f,
    pub local_bounds_max: Vec3f,
    pub local_centroid: Vec3f,
    pub local_vertex_count: usize,
}

#[derive(Clone, Debug)]
pub struct GltfDefaultView {
    pub eye: Vec3f,
    pub forward: Vec3f,
    pub fov_y_degrees: Option<f32>,
    pub near: Option<f32>,
    pub far: Option<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct GltfRenderer {
    pub mesh_objects: GltfMeshObjects,
    pub draw_objects: Vec<GltfDrawObject>,
    pub scene_center: Vec3f,
    pub materials: Vec<GltfMaterialState>,
    pub textures: Vec<Option<Texture>>,
    pub default_view: Option<GltfDefaultView>,
    texture_keys: Vec<Option<PathBuf>>,
}

impl GltfRenderer {
    pub fn load_from_bytes(
        draw: &mut DrawPbr,
        cx: &mut Cx2d,
        bytes: &[u8],
        source_path: Option<&Path>,
    ) -> Result<Self, GltfError> {
        let base_dir = source_path.and_then(|path| path.parent());
        let mut loaded = load_gltf_from_bytes(bytes, base_dir)?;
        loaded.source_path = source_path.map(|path| path.to_path_buf());
        loaded.base_dir = base_dir.map(|path| path.to_path_buf());
        Self::from_loaded(draw, cx, &loaded)
    }

    pub fn load_from_path(
        draw: &mut DrawPbr,
        cx: &mut Cx2d,
        path: impl AsRef<Path>,
    ) -> Result<Self, GltfError> {
        let loaded = load_gltf_from_path(path)?;
        Self::from_loaded(draw, cx, &loaded)
    }

    pub fn from_loaded(
        draw: &mut DrawPbr,
        cx: &mut Cx2d,
        loaded: &LoadedGltf,
    ) -> Result<Self, GltfError> {
        let mesh_objects = GltfMeshObjects::upload_all(draw, cx, loaded)?;
        Self::from_loaded_with_mesh_objects(cx, loaded, mesh_objects)
    }

    pub fn from_loaded_predecoded(
        draw: &mut DrawPbr,
        cx: &mut Cx2d,
        loaded: &LoadedGltf,
        decoded_meshes: &GltfDecodedMeshes,
    ) -> Result<Self, GltfError> {
        let mesh_objects = decoded_meshes.upload_all(draw, cx)?;
        Self::from_loaded_with_mesh_objects(cx, loaded, mesh_objects)
    }

    fn from_loaded_with_mesh_objects(
        cx: &mut Cx2d,
        loaded: &LoadedGltf,
        mesh_objects: GltfMeshObjects,
    ) -> Result<Self, GltfError> {
        let draw_objects = build_draw_objects(loaded, &mesh_objects)?;
        let scene_center = compute_scene_center(&draw_objects);
        let materials = build_materials(loaded);
        let (textures, texture_keys) = request_material_textures(cx, loaded)?;
        let default_view = build_default_view(loaded)?;

        Ok(Self {
            mesh_objects,
            draw_objects,
            scene_center,
            materials,
            textures,
            default_view,
            texture_keys,
        })
    }

    /// Feed UI-thread events so async image decode/network completions can be committed.
    /// The renderer API itself remains synchronous: `draw()` is always immediate-mode.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::NetworkResponses(e) => {
                handle_image_cache_network_responses(cx, e);
                self.poll_textures(cx);
            }
            Event::Actions(actions) => {
                self.handle_actions(cx, actions);
            }
            _ => {}
        }
    }

    pub fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if let Some(AsyncImageLoad { image_path, result }) = action.downcast_ref() {
                if let Some(result) = result.borrow_mut().take() {
                    process_async_image_load(cx, image_path, result);
                }
            }
        }
        self.poll_textures(cx);
    }

    pub fn poll_textures(&mut self, cx: &mut Cx) {
        for (texture_index, key) in self.texture_keys.iter().enumerate() {
            let Some(key) = key else {
                continue;
            };
            if let Some(texture) = load_image_from_cache(cx, key) {
                self.textures[texture_index] = Some(texture);
            }
        }
    }

    pub fn draw(&mut self, draw: &mut DrawPbr, cx: &mut Cx2d) -> Result<(), GltfError> {
        self.poll_textures(cx);

        for object in &self.draw_objects {
            draw.set_transform(object.world_transform);
            self.apply_material(draw, cx, object.material_index);
            draw.draw_mesh(cx, object.mesh_handle)
                .map_err(GltfError::Validation)?;
        }
        Ok(())
    }

    pub fn draw_with_transform(
        &mut self,
        draw: &mut DrawPbr,
        cx: &mut Cx2d,
        transform: Mat4f,
    ) -> Result<(), GltfError> {
        self.poll_textures(cx);

        for object in &self.draw_objects {
            draw.set_transform(Mat4f::mul(&transform, &object.world_transform));
            self.apply_material(draw, cx, object.material_index);
            draw.draw_mesh(cx, object.mesh_handle)
                .map_err(GltfError::Validation)?;
        }
        Ok(())
    }

    pub fn draw_with_transform_anchors<F>(
        &mut self,
        draw: &mut DrawPbr,
        cx: &mut Cx2d,
        transform: Mat4f,
        mut on_draw_call: F,
    ) -> Result<(), GltfError>
    where
        F: FnMut(Area, Vec3f),
    {
        self.poll_textures(cx);

        for object in &self.draw_objects {
            let object_transform = Mat4f::mul(&transform, &object.world_transform);
            draw.set_transform(object_transform);
            self.apply_material(draw, cx, object.material_index);
            draw.draw_mesh(cx, object.mesh_handle)
                .map_err(GltfError::Validation)?;
            let world = object_transform.transform_vec4(vec4(
                object.local_centroid.x,
                object.local_centroid.y,
                object.local_centroid.z,
                1.0,
            ));
            on_draw_call(draw.draw_vars.area, vec3(world.x, world.y, world.z));
        }
        Ok(())
    }

    fn apply_material(&self, draw: &mut DrawPbr, cx: &mut Cx2d, material_index: Option<usize>) {
        let material = material_index
            .and_then(|index| self.materials.get(index))
            .cloned()
            .unwrap_or_default();

        let base_color_texture = material
            .base_color_texture
            .and_then(|texture_index| self.textures.get(texture_index))
            .and_then(|texture| texture.clone());
        let metallic_roughness_texture = material
            .metallic_roughness_texture
            .and_then(|texture_index| self.textures.get(texture_index))
            .and_then(|texture| texture.clone());
        let normal_texture = material
            .normal_texture
            .and_then(|texture_index| self.textures.get(texture_index))
            .and_then(|texture| texture.clone());
        let occlusion_texture = material
            .occlusion_texture
            .and_then(|texture_index| self.textures.get(texture_index))
            .and_then(|texture| texture.clone());
        let emissive_texture = material
            .emissive_texture
            .and_then(|texture_index| self.textures.get(texture_index))
            .and_then(|texture| texture.clone());
        let env_texture = draw.default_env_texture(cx);

        draw.apply_material_state(&DrawPbrMaterialState {
            base_color_factor: material.base_color_factor,
            metallic_factor: material.metallic_factor,
            roughness_factor: material.roughness_factor,
            emissive_factor: material.emissive_factor,
            normal_scale: material.normal_scale,
            occlusion_strength: material.occlusion_strength,
            textures: DrawPbrTextureSet {
                base_color: base_color_texture,
                metallic_roughness: metallic_roughness_texture,
                normal: normal_texture,
                occlusion: occlusion_texture,
                emissive: emissive_texture,
                env: Some(env_texture),
            },
        });
    }
}

fn build_materials(loaded: &LoadedGltf) -> Vec<GltfMaterialState> {
    loaded
        .document
        .materials_slice()
        .iter()
        .map(|material| {
            let pbr = material.pbr_metallic_roughness.as_ref();
            let base_color = pbr
                .and_then(|pbr| pbr.base_color_factor)
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);

            GltfMaterialState {
                base_color_factor: vec4(base_color[0], base_color[1], base_color[2], base_color[3]),
                metallic_factor: pbr.and_then(|pbr| pbr.metallic_factor).unwrap_or(1.0),
                roughness_factor: pbr.and_then(|pbr| pbr.roughness_factor).unwrap_or(1.0),
                emissive_factor: material
                    .emissive_factor
                    .map(|f| vec3(f[0], f[1], f[2]))
                    .unwrap_or(vec3(0.0, 0.0, 0.0)),
                normal_scale: material
                    .normal_texture
                    .as_ref()
                    .and_then(|info| info.scale)
                    .unwrap_or(1.0),
                occlusion_strength: material
                    .occlusion_texture
                    .as_ref()
                    .and_then(|info| info.strength)
                    .unwrap_or(1.0),
                base_color_texture: pbr
                    .and_then(|pbr| pbr.base_color_texture.as_ref())
                    .map(|info| info.index),
                metallic_roughness_texture: pbr
                    .and_then(|pbr| pbr.metallic_roughness_texture.as_ref())
                    .map(|info| info.index),
                normal_texture: material.normal_texture.as_ref().map(|info| info.index),
                occlusion_texture: material.occlusion_texture.as_ref().map(|info| info.index),
                emissive_texture: material.emissive_texture.as_ref().map(|info| info.index),
            }
        })
        .collect()
}

fn request_material_textures(
    cx: &mut Cx2d,
    loaded: &LoadedGltf,
) -> Result<(Vec<Option<Texture>>, Vec<Option<PathBuf>>), GltfError> {
    let mut textures = vec![None; loaded.document.textures_slice().len()];
    let mut texture_keys = vec![None; loaded.document.textures_slice().len()];
    let mut image_bytes_cache = HashMap::<usize, Arc<Vec<u8>>>::new();

    for (texture_index, texture) in loaded.document.textures_slice().iter().enumerate() {
        let Some(image_index) = texture.source else {
            continue;
        };

        let image = loaded
            .document
            .images_slice()
            .get(image_index)
            .ok_or_else(|| {
                GltfError::Validation(format!(
                    "textures[{texture_index}] references missing image source {image_index}"
                ))
            })?;

        let bytes = if let Some(cached) = image_bytes_cache.get(&image_index) {
            cached.clone()
        } else {
            let loaded_bytes = Arc::new(load_image_bytes(loaded, image_index)?);
            image_bytes_cache.insert(image_index, loaded_bytes.clone());
            loaded_bytes
        };
        let key = gltf_image_cache_key(
            loaded,
            image_index,
            image.uri.as_deref(),
            image.mime_type.as_deref(),
        );
        texture_keys[texture_index] = Some(key.clone());

        match load_image_from_data_async(cx, &key, bytes) {
            Ok(AsyncLoadResult::Loaded) => {
                textures[texture_index] = load_image_from_cache(cx, &key);
            }
            Ok(AsyncLoadResult::Loading(_, _)) => {}
            Err(ImageError::UnsupportedFormat) => {
                // Keep texture empty and continue with factor-only shading.
            }
            Err(err) => {
                return Err(GltfError::Validation(format!(
                    "failed to queue texture decode for texture[{texture_index}]: {err}"
                )));
            }
        }
    }

    Ok((textures, texture_keys))
}

fn build_draw_objects(
    loaded: &LoadedGltf,
    mesh_objects: &GltfMeshObjects,
) -> Result<Vec<GltfDrawObject>, GltfError> {
    let mut mesh_info_by_primitive =
        HashMap::<(usize, usize), (PbrMeshHandle, Vec3f, Vec3f, Vec3f, usize)>::new();
    for primitive in &mesh_objects.primitives {
        mesh_info_by_primitive.insert(
            (primitive.mesh_index, primitive.primitive_index),
            (
                primitive.mesh_handle,
                primitive.bounds_min,
                primitive.bounds_max,
                primitive.centroid,
                primitive.vertex_count,
            ),
        );
    }

    let roots = scene_root_nodes(&loaded.document)?;
    let mut out = Vec::new();
    let mut visiting = vec![false; loaded.document.nodes_slice().len()];
    for root in roots {
        collect_node_draw_objects(
            &loaded.document,
            root,
            Mat4f::identity(),
            &mesh_info_by_primitive,
            &mut visiting,
            &mut out,
        )?;
    }
    Ok(out)
}

fn build_default_view(loaded: &LoadedGltf) -> Result<Option<GltfDefaultView>, GltfError> {
    let roots = scene_root_nodes(&loaded.document)?;
    let mut visiting = vec![false; loaded.document.nodes_slice().len()];
    for root in roots {
        if let Some(view) =
            collect_first_camera_view(&loaded.document, root, Mat4f::identity(), &mut visiting)?
        {
            return Ok(Some(view));
        }
    }
    Ok(None)
}

fn scene_root_nodes(document: &GltfDocument) -> Result<Vec<usize>, GltfError> {
    if !document.scenes_slice().is_empty() {
        let scene_index = document.scene.unwrap_or(0);
        let scene = document.scenes_slice().get(scene_index).ok_or_else(|| {
            GltfError::Validation(format!(
                "scene index {scene_index} out of bounds (len {})",
                document.scenes_slice().len()
            ))
        })?;
        return Ok(scene.nodes.clone().unwrap_or_default());
    }
    Ok(infer_root_nodes(document))
}

fn infer_root_nodes(document: &GltfDocument) -> Vec<usize> {
    let nodes = document.nodes_slice();
    let mut is_child = vec![false; nodes.len()];
    for node in nodes {
        if let Some(children) = &node.children {
            for &child in children {
                if child < is_child.len() {
                    is_child[child] = true;
                }
            }
        }
    }
    let mut roots = (0..nodes.len())
        .filter(|index| !is_child[*index])
        .collect::<Vec<_>>();
    if roots.is_empty() {
        roots.extend(0..nodes.len());
    }
    roots
}

fn collect_node_draw_objects(
    document: &GltfDocument,
    node_index: usize,
    parent_world: Mat4f,
    mesh_info_by_primitive: &HashMap<(usize, usize), (PbrMeshHandle, Vec3f, Vec3f, Vec3f, usize)>,
    visiting: &mut [bool],
    out: &mut Vec<GltfDrawObject>,
) -> Result<(), GltfError> {
    let node = document.nodes_slice().get(node_index).ok_or_else(|| {
        GltfError::Validation(format!(
            "node index {node_index} out of bounds (len {})",
            document.nodes_slice().len()
        ))
    })?;

    if visiting[node_index] {
        return Err(GltfError::Validation(format!(
            "node hierarchy contains a cycle at node {node_index}"
        )));
    }
    visiting[node_index] = true;

    let local = node_local_transform(node);
    let world = Mat4f::mul(&parent_world, &local);

    if let Some(mesh_index) = node.mesh {
        let mesh = document.meshes_slice().get(mesh_index).ok_or_else(|| {
            GltfError::Validation(format!(
                "node[{node_index}] references missing mesh {mesh_index}"
            ))
        })?;

        for (primitive_index, primitive) in mesh.primitives.iter().enumerate() {
            let Some((
                mesh_handle,
                local_bounds_min,
                local_bounds_max,
                local_centroid,
                local_vertex_count,
            )) = mesh_info_by_primitive
                .get(&(mesh_index, primitive_index))
                .copied()
            else {
                continue;
            };

            out.push(GltfDrawObject {
                mesh_handle,
                node_index,
                mesh_index,
                primitive_index,
                material_index: primitive.material,
                world_transform: world,
                local_bounds_min,
                local_bounds_max,
                local_centroid,
                local_vertex_count,
            });
        }
    }

    if let Some(children) = &node.children {
        for &child in children {
            collect_node_draw_objects(
                document,
                child,
                world,
                mesh_info_by_primitive,
                visiting,
                out,
            )?;
        }
    }

    visiting[node_index] = false;
    Ok(())
}

fn collect_first_camera_view(
    document: &GltfDocument,
    node_index: usize,
    parent_world: Mat4f,
    visiting: &mut [bool],
) -> Result<Option<GltfDefaultView>, GltfError> {
    let node = document.nodes_slice().get(node_index).ok_or_else(|| {
        GltfError::Validation(format!(
            "node index {node_index} out of bounds (len {})",
            document.nodes_slice().len()
        ))
    })?;

    if visiting[node_index] {
        return Err(GltfError::Validation(format!(
            "node hierarchy contains a cycle at node {node_index}"
        )));
    }
    visiting[node_index] = true;

    let local = node_local_transform(node);
    let world = Mat4f::mul(&parent_world, &local);

    if let Some(camera_index) = node.camera {
        let camera = document.cameras_slice().get(camera_index).ok_or_else(|| {
            GltfError::Validation(format!(
                "node[{node_index}] references missing camera {camera_index}"
            ))
        })?;

        let eye = vec3(world.v[12], world.v[13], world.v[14]);
        let mut forward = vec3(-world.v[8], -world.v[9], -world.v[10]);
        if forward.length() < 0.000_001 {
            forward = vec3(0.0, 0.0, -1.0);
        } else {
            forward = forward.normalize();
        }

        let (fov_y_degrees, near, far) = if let Some(perspective) = &camera.perspective {
            (
                Some(perspective.yfov.to_degrees()),
                Some(perspective.znear),
                perspective.zfar,
            )
        } else {
            (None, None, None)
        };

        visiting[node_index] = false;
        return Ok(Some(GltfDefaultView {
            eye,
            forward,
            fov_y_degrees,
            near,
            far,
        }));
    }

    if let Some(children) = &node.children {
        for &child in children {
            if let Some(found) = collect_first_camera_view(document, child, world, visiting)? {
                visiting[node_index] = false;
                return Ok(Some(found));
            }
        }
    }

    visiting[node_index] = false;
    Ok(None)
}

fn node_local_transform(node: &GltfNode) -> Mat4f {
    if let Some(matrix) = node.matrix {
        return Mat4f { v: matrix };
    }

    let translation = node.translation.unwrap_or([0.0, 0.0, 0.0]);
    let rotation = node.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let scale = node.scale.unwrap_or([1.0, 1.0, 1.0]);

    let translation_m = Mat4f::translation(Vec3f {
        x: translation[0],
        y: translation[1],
        z: translation[2],
    });
    let rotation_m = Pose::new(
        Quat {
            x: rotation[0],
            y: rotation[1],
            z: rotation[2],
            w: rotation[3],
        },
        Vec3f {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
    )
    .to_mat4();
    let scale_m = Mat4f::nonuniform_scaled_translation(
        Vec3f {
            x: scale[0],
            y: scale[1],
            z: scale[2],
        },
        Vec3f {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
    );

    Mat4f::mul(&Mat4f::mul(&translation_m, &rotation_m), &scale_m)
}

fn primitive_bounds_min(positions: &[[f32; 3]]) -> Vec3f {
    let mut min = vec3(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    for p in positions {
        min.x = min.x.min(p[0]);
        min.y = min.y.min(p[1]);
        min.z = min.z.min(p[2]);
    }
    if positions.is_empty() {
        vec3(0.0, 0.0, 0.0)
    } else {
        min
    }
}

fn primitive_bounds_max(positions: &[[f32; 3]]) -> Vec3f {
    let mut max = vec3(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for p in positions {
        max.x = max.x.max(p[0]);
        max.y = max.y.max(p[1]);
        max.z = max.z.max(p[2]);
    }
    if positions.is_empty() {
        vec3(0.0, 0.0, 0.0)
    } else {
        max
    }
}

fn compute_scene_center(draw_objects: &[GltfDrawObject]) -> Vec3f {
    if draw_objects.is_empty() {
        return vec3(0.0, 0.0, 0.0);
    }

    let mut weighted_sum = vec3(0.0, 0.0, 0.0);
    let mut total_weight = 0.0f32;
    for object in draw_objects {
        let local_center = vec3(
            0.5 * (object.local_bounds_min.x + object.local_bounds_max.x),
            0.5 * (object.local_bounds_min.y + object.local_bounds_max.y),
            0.5 * (object.local_bounds_min.z + object.local_bounds_max.z),
        );
        let world = object.world_transform.transform_vec4(vec4(
            local_center.x,
            local_center.y,
            local_center.z,
            1.0,
        ));
        let weight = object.local_vertex_count.max(1) as f32;
        weighted_sum += vec3(world.x, world.y, world.z) * weight;
        total_weight += weight;
    }

    if total_weight > 0.0 {
        weighted_sum / total_weight
    } else {
        vec3(0.0, 0.0, 0.0)
    }
}

fn primitive_bounds_center(positions: &[[f32; 3]]) -> Vec3f {
    if positions.is_empty() {
        return vec3(0.0, 0.0, 0.0);
    }
    let min = primitive_bounds_min(positions);
    let max = primitive_bounds_max(positions);
    vec3(
        0.5 * (min.x + max.x),
        0.5 * (min.y + max.y),
        0.5 * (min.z + max.z),
    )
}

fn gltf_image_cache_key(
    loaded: &LoadedGltf,
    image_index: usize,
    image_uri: Option<&str>,
    mime_type: Option<&str>,
) -> PathBuf {
    let ext = image_uri
        .and_then(|uri| Path::new(uri).extension().and_then(|ext| ext.to_str()))
        .or_else(|| mime_type.and_then(mime_to_extension))
        .unwrap_or("img");

    let prefix = loaded
        .source_path
        .as_ref()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "gltf".to_string());

    PathBuf::from(format!("{prefix}#image_{image_index}.{ext}"))
}

fn mime_to_extension(mime: &str) -> Option<&'static str> {
    let mime = mime.to_ascii_lowercase();
    match mime.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        _ => None,
    }
}
