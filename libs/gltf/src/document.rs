use makepad_micro_serde::*;
use std::collections::HashMap;

pub const GLTF_MODE_TRIANGLES: u32 = 4;

#[derive(Clone, Debug, DeJson)]
pub struct GltfDocument {
    pub asset: GltfAsset,
    pub scene: Option<usize>,
    pub scenes: Option<Vec<GltfScene>>,
    pub nodes: Option<Vec<GltfNode>>,
    pub meshes: Option<Vec<GltfMesh>>,
    pub accessors: Option<Vec<GltfAccessor>>,
    #[rename(bufferViews)]
    pub buffer_views: Option<Vec<GltfBufferView>>,
    pub buffers: Option<Vec<GltfBuffer>>,
    pub materials: Option<Vec<GltfMaterial>>,
    pub textures: Option<Vec<GltfTexture>>,
    pub images: Option<Vec<GltfImage>>,
    pub samplers: Option<Vec<GltfSampler>>,
    pub cameras: Option<Vec<GltfCamera>>,
    pub skins: Option<Vec<JsonValue>>,
    pub animations: Option<Vec<JsonValue>>,
    #[rename(extensionsUsed)]
    pub extensions_used: Option<Vec<String>>,
    #[rename(extensionsRequired)]
    pub extensions_required: Option<Vec<String>>,
}

impl GltfDocument {
    pub fn scenes_slice(&self) -> &[GltfScene] {
        self.scenes.as_deref().unwrap_or(&[])
    }

    pub fn nodes_slice(&self) -> &[GltfNode] {
        self.nodes.as_deref().unwrap_or(&[])
    }

    pub fn meshes_slice(&self) -> &[GltfMesh] {
        self.meshes.as_deref().unwrap_or(&[])
    }

    pub fn accessors_slice(&self) -> &[GltfAccessor] {
        self.accessors.as_deref().unwrap_or(&[])
    }

    pub fn buffer_views_slice(&self) -> &[GltfBufferView] {
        self.buffer_views.as_deref().unwrap_or(&[])
    }

    pub fn buffers_slice(&self) -> &[GltfBuffer] {
        self.buffers.as_deref().unwrap_or(&[])
    }

    pub fn materials_slice(&self) -> &[GltfMaterial] {
        self.materials.as_deref().unwrap_or(&[])
    }

    pub fn textures_slice(&self) -> &[GltfTexture] {
        self.textures.as_deref().unwrap_or(&[])
    }

    pub fn images_slice(&self) -> &[GltfImage] {
        self.images.as_deref().unwrap_or(&[])
    }

    pub fn samplers_slice(&self) -> &[GltfSampler] {
        self.samplers.as_deref().unwrap_or(&[])
    }

    pub fn cameras_slice(&self) -> &[GltfCamera] {
        self.cameras.as_deref().unwrap_or(&[])
    }
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfAsset {
    pub version: String,
    #[rename(minVersion)]
    pub min_version: Option<String>,
    pub generator: Option<String>,
    pub copyright: Option<String>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfScene {
    pub name: Option<String>,
    pub nodes: Option<Vec<usize>>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfNode {
    pub name: Option<String>,
    pub camera: Option<usize>,
    pub children: Option<Vec<usize>>,
    pub skin: Option<usize>,
    pub mesh: Option<usize>,
    pub matrix: Option<[f32; 16]>,
    pub translation: Option<[f32; 3]>,
    pub rotation: Option<[f32; 4]>,
    pub scale: Option<[f32; 3]>,
    pub weights: Option<Vec<f32>>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfCamera {
    pub name: Option<String>,
    #[rename(type)]
    pub camera_type: String,
    pub perspective: Option<GltfPerspectiveCamera>,
    pub orthographic: Option<GltfOrthographicCamera>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfPerspectiveCamera {
    #[rename(aspectRatio)]
    pub aspect_ratio: Option<f32>,
    pub yfov: f32,
    pub znear: f32,
    pub zfar: Option<f32>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfOrthographicCamera {
    pub xmag: f32,
    pub ymag: f32,
    pub znear: f32,
    pub zfar: f32,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfMesh {
    pub name: Option<String>,
    pub primitives: Vec<GltfPrimitive>,
    pub weights: Option<Vec<f32>>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfPrimitive {
    pub attributes: HashMap<String, usize>,
    pub indices: Option<usize>,
    pub material: Option<usize>,
    pub mode: Option<u32>,
    pub targets: Option<Vec<HashMap<String, usize>>>,
}

impl GltfPrimitive {
    pub fn mode(&self) -> u32 {
        self.mode.unwrap_or(GLTF_MODE_TRIANGLES)
    }
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfAccessor {
    #[rename(bufferView)]
    pub buffer_view: Option<usize>,
    #[rename(byteOffset)]
    pub byte_offset: Option<usize>,
    #[rename(componentType)]
    pub component_type: u32,
    pub normalized: Option<bool>,
    pub count: usize,
    #[rename(type)]
    pub accessor_type: String,
    pub max: Option<Vec<f32>>,
    pub min: Option<Vec<f32>>,
    pub sparse: Option<GltfAccessorSparse>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfAccessorSparse {
    pub count: usize,
    pub indices: GltfAccessorSparseIndices,
    pub values: GltfAccessorSparseValues,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfAccessorSparseIndices {
    #[rename(bufferView)]
    pub buffer_view: usize,
    #[rename(byteOffset)]
    pub byte_offset: Option<usize>,
    #[rename(componentType)]
    pub component_type: u32,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfAccessorSparseValues {
    #[rename(bufferView)]
    pub buffer_view: usize,
    #[rename(byteOffset)]
    pub byte_offset: Option<usize>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfBufferView {
    pub buffer: usize,
    #[rename(byteOffset)]
    pub byte_offset: Option<usize>,
    #[rename(byteLength)]
    pub byte_length: usize,
    #[rename(byteStride)]
    pub byte_stride: Option<usize>,
    pub target: Option<u32>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfBuffer {
    pub uri: Option<String>,
    #[rename(byteLength)]
    pub byte_length: usize,
    pub name: Option<String>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfMaterial {
    pub name: Option<String>,
    #[rename(pbrMetallicRoughness)]
    pub pbr_metallic_roughness: Option<GltfPbrMetallicRoughness>,
    #[rename(normalTexture)]
    pub normal_texture: Option<GltfNormalTextureInfo>,
    #[rename(occlusionTexture)]
    pub occlusion_texture: Option<GltfOcclusionTextureInfo>,
    #[rename(emissiveTexture)]
    pub emissive_texture: Option<GltfTextureInfo>,
    #[rename(emissiveFactor)]
    pub emissive_factor: Option<[f32; 3]>,
    #[rename(alphaMode)]
    pub alpha_mode: Option<String>,
    #[rename(alphaCutoff)]
    pub alpha_cutoff: Option<f32>,
    #[rename(doubleSided)]
    pub double_sided: Option<bool>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfPbrMetallicRoughness {
    #[rename(baseColorFactor)]
    pub base_color_factor: Option<[f32; 4]>,
    #[rename(baseColorTexture)]
    pub base_color_texture: Option<GltfTextureInfo>,
    #[rename(metallicFactor)]
    pub metallic_factor: Option<f32>,
    #[rename(roughnessFactor)]
    pub roughness_factor: Option<f32>,
    #[rename(metallicRoughnessTexture)]
    pub metallic_roughness_texture: Option<GltfTextureInfo>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfTextureInfo {
    pub index: usize,
    #[rename(texCoord)]
    pub tex_coord: Option<usize>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfNormalTextureInfo {
    pub index: usize,
    #[rename(texCoord)]
    pub tex_coord: Option<usize>,
    pub scale: Option<f32>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfOcclusionTextureInfo {
    pub index: usize,
    #[rename(texCoord)]
    pub tex_coord: Option<usize>,
    pub strength: Option<f32>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfTexture {
    pub sampler: Option<usize>,
    pub source: Option<usize>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfImage {
    pub uri: Option<String>,
    #[rename(mimeType)]
    pub mime_type: Option<String>,
    #[rename(bufferView)]
    pub buffer_view: Option<usize>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, DeJson)]
pub struct GltfSampler {
    #[rename(magFilter)]
    pub mag_filter: Option<u32>,
    #[rename(minFilter)]
    pub min_filter: Option<u32>,
    #[rename(wrapS)]
    pub wrap_s: Option<u32>,
    #[rename(wrapT)]
    pub wrap_t: Option<u32>,
    pub name: Option<String>,
}
